//! Credential handling for the git backup remote.
//!
//! Policy (backup redesign §3.7): tokens must never live in URLs on disk
//! (`.git/config`, SQLite settings). Credentials embedded in a remote URL are
//! extracted into the OS keychain and injected into git at call time through
//! a static askpass script that only echoes environment variables.

use anyhow::{Context, Result};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use super::{central_repo, github_api};

const KEYRING_SERVICE: &str = "patchbay-git-backup";

/// Environment variable names consumed by the askpass script. The script
/// itself contains no secrets — it just echoes these back to git.
const ENV_USERNAME: &str = "PATCHBAY_ASKPASS_USERNAME";
const ENV_PASSWORD: &str = "PATCHBAY_ASKPASS_PASSWORD";

static PROXY_URL: OnceLock<Mutex<Option<String>>> = OnceLock::new();
static REFRESH_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RemoteCredential {
    pub username: String,
    pub password: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github_app: Option<GithubAppCredential>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GithubAppCredential {
    pub refresh_token: String,
    pub access_token_expires_at: i64,
    pub refresh_token_expires_at: i64,
    #[serde(default)]
    pub repository_id: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum RefreshDecision {
    NotNeeded,
    Refresh,
    Reauthorize,
}

fn refresh_decision(credential: &RemoteCredential, now: i64) -> RefreshDecision {
    let Some(github_app) = &credential.github_app else {
        return RefreshDecision::NotNeeded;
    };
    if now >= github_app.refresh_token_expires_at {
        RefreshDecision::Reauthorize
    } else if now >= github_app.access_token_expires_at.saturating_sub(300) {
        RefreshDecision::Refresh
    } else {
        RefreshDecision::NotNeeded
    }
}

fn apply_github_app_token(
    credential: &mut RemoteCredential,
    token: github_api::GithubAppToken,
    now: i64,
    repository_id: u64,
) {
    let expires_at = |seconds: u64| now.saturating_add(seconds.min(i64::MAX as u64) as i64);
    credential.username = "x-access-token".to_string();
    credential.password = token.access_token;
    credential.github_app = Some(GithubAppCredential {
        refresh_token: token.refresh_token,
        access_token_expires_at: expires_at(token.expires_in),
        refresh_token_expires_at: expires_at(token.refresh_token_expires_in),
        repository_id,
    });
}

pub fn github_app_credential(
    token: github_api::GithubAppToken,
    repository_id: u64,
) -> RemoteCredential {
    let mut credential = RemoteCredential {
        username: "x-access-token".to_string(),
        password: String::new(),
        github_app: None,
    };
    apply_github_app_token(
        &mut credential,
        token,
        chrono::Utc::now().timestamp(),
        repository_id,
    );
    credential
}

fn validate_github_app_scope_with<F>(credential: &RemoteCredential, validate: F) -> Result<()>
where
    F: FnOnce(&str, u64) -> Result<()>,
{
    let Some(github_app) = &credential.github_app else {
        return Ok(());
    };
    validate(&credential.password, github_app.repository_id)
}

fn validate_github_app_scope(credential: &RemoteCredential) -> Result<()> {
    validate_github_app_scope_with(credential, |token, repository_id| {
        github_api::validate_github_app_credential_scope(
            token,
            repository_id,
            proxy_url().as_deref(),
        )
    })
}

pub fn set_proxy(proxy_url: Option<String>) {
    *PROXY_URL
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = proxy_url.filter(|url| !url.is_empty());
}

fn proxy_url() -> Option<String> {
    PROXY_URL
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
}

/// Split userinfo credentials out of an http(s) URL.
///
/// Returns the extracted credential plus the sanitized URL (no userinfo).
/// `None` when the URL is not http(s) or carries no userinfo. A token-only
/// form (`https://TOKEN@host/...`) is kept faithful: username = token,
/// password = empty — exactly what git derived from the embedded URL.
pub fn split_credentials_from_url(url: &str) -> Option<(RemoteCredential, String)> {
    let trimmed = url.trim();
    let lower = trimmed.to_lowercase();
    if !lower.starts_with("https://") && !lower.starts_with("http://") {
        return None;
    }
    let scheme_end = trimmed.find("://")? + 3;
    let rest = &trimmed[scheme_end..];
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];

    let at_pos = authority.rfind('@')?;
    let userinfo = &authority[..at_pos];
    let host_part = &authority[at_pos + 1..];

    let (raw_user, raw_pass) = match userinfo.split_once(':') {
        Some((u, p)) => (u, p),
        None => (userinfo, ""),
    };
    let decode = |s: &str| {
        urlencoding::decode(s)
            .map(|c| c.into_owned())
            .unwrap_or_else(|_| s.to_string())
    };

    let sanitized = format!(
        "{}{}{}",
        &trimmed[..scheme_end],
        host_part,
        &rest[authority_end..]
    );
    Some((
        RemoteCredential {
            username: decode(raw_user),
            password: decode(raw_pass),
            github_app: None,
        },
        sanitized,
    ))
}

/// Host (including port, if any) of an http(s) URL with userinfo stripped.
/// Used as the keychain account key.
pub fn https_host(url: &str) -> Option<String> {
    let trimmed = url.trim();
    let lower = trimmed.to_lowercase();
    if !lower.starts_with("https://") && !lower.starts_with("http://") {
        return None;
    }
    let scheme_end = trimmed.find("://")? + 3;
    let rest = &trimmed[scheme_end..];
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let host = match authority.rfind('@') {
        Some(at) => &authority[at + 1..],
        None => authority,
    };
    if host.is_empty() {
        return None;
    }
    Some(host.to_ascii_lowercase())
}

fn keyring_entry(host: &str) -> Result<keyring::Entry> {
    keyring::Entry::new(KEYRING_SERVICE, host).context("Failed to open keychain entry")
}

pub fn store_credential(host: &str, cred: &RemoteCredential) -> Result<()> {
    let payload = serde_json::to_string(cred)?;
    keyring_entry(host)?
        .set_password(&payload)
        .with_context(|| format!("Failed to store git credential for {host} in OS keychain"))?;
    log::info!("git credentials: stored credential for {host} in OS keychain");
    Ok(())
}

fn load_stored_credential(host: &str) -> Result<Option<RemoteCredential>> {
    match keyring_entry(host)?.get_password() {
        Ok(payload) => {
            Ok(Some(serde_json::from_str(&payload).with_context(|| {
                format!("Corrupted keychain entry for {host}")
            })?))
        }
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e).with_context(|| format!("Failed to read git credential for {host}")),
    }
}

pub fn load_credential(host: &str) -> Result<Option<RemoteCredential>> {
    let Some(credential) = load_stored_credential(host)? else {
        return Ok(None);
    };
    let now = chrono::Utc::now().timestamp();
    match refresh_decision(&credential, now) {
        RefreshDecision::NotNeeded => {
            validate_github_app_scope(&credential)?;
            Ok(Some(credential))
        }
        RefreshDecision::Reauthorize => {
            anyhow::bail!("GITHUB_APP_REAUTH_REQUIRED: the Patchbay GitHub authorization expired")
        }
        RefreshDecision::Refresh => {
            let _guard = REFRESH_LOCK
                .get_or_init(|| Mutex::new(()))
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let Some(mut current) = load_stored_credential(host)? else {
                return Ok(None);
            };
            match refresh_decision(&current, chrono::Utc::now().timestamp()) {
                RefreshDecision::NotNeeded => {
                    validate_github_app_scope(&current)?;
                    return Ok(Some(current));
                }
                RefreshDecision::Reauthorize => anyhow::bail!(
                    "GITHUB_APP_REAUTH_REQUIRED: the Patchbay GitHub authorization expired"
                ),
                RefreshDecision::Refresh => {}
            }
            let github_app = current
                .github_app
                .as_ref()
                .context("GitHub App refresh metadata is missing")?;
            let refresh_token = github_app.refresh_token.clone();
            let repository_id = github_app.repository_id;
            let token =
                github_api::refresh_github_app_token(&refresh_token, proxy_url().as_deref())?;
            apply_github_app_token(
                &mut current,
                token,
                chrono::Utc::now().timestamp(),
                repository_id,
            );
            validate_github_app_scope(&current)?;
            store_credential(host, &current)?;
            Ok(Some(current))
        }
    }
}

pub fn delete_credential(host: &str) -> Result<()> {
    match keyring_entry(host)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => {
            log::info!("git credentials: removed credential for {host}");
            Ok(())
        }
        Err(e) => Err(e).with_context(|| format!("Failed to delete git credential for {host}")),
    }
}

/// The askpass script git invokes for username/password prompts. Static
/// content, no secrets — safe on disk. Git for Windows executes shebang
/// scripts through its bundled sh, so a single POSIX script covers all
/// platforms.
const ASKPASS_SCRIPT: &str = "#!/bin/sh\n\
# Managed by Patchbay. Supplies git credentials from the environment.\n\
case \"$1\" in\n\
  *[Uu]sername*) printf '%s\\n' \"${PATCHBAY_ASKPASS_USERNAME}\" ;;\n\
  *) printf '%s\\n' \"${PATCHBAY_ASKPASS_PASSWORD}\" ;;\n\
esac\n";

fn askpass_script_path() -> PathBuf {
    central_repo::base_dir().join("git-askpass.sh")
}

fn ensure_askpass_script() -> Result<PathBuf> {
    let path = askpass_script_path();
    let up_to_date = std::fs::read_to_string(&path)
        .map(|current| current == ASKPASS_SCRIPT)
        .unwrap_or(false);
    if !up_to_date {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, ASKPASS_SCRIPT).context("Failed to write askpass script")?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(path)
}

/// Environment to inject into a git subprocess so it can authenticate against
/// `url` without credentials on disk. Empty when not applicable: non-http(s)
/// URL, URL still carrying embedded userinfo (git uses it directly), or no
/// stored credential for the host. Keychain and refresh failures are returned
/// so callers can surface reauthorization instead of attempting anonymous git.
pub fn credential_env_for_url(url: &str) -> Result<Vec<(String, String)>> {
    credential_env_for_url_with(url, load_credential)
}

fn credential_env_for_url_with<F>(url: &str, load: F) -> Result<Vec<(String, String)>>
where
    F: FnOnce(&str) -> Result<Option<RemoteCredential>>,
{
    let Some(host) = https_host(url) else {
        return Ok(Vec::new());
    };
    if split_credentials_from_url(url).is_some() {
        return Ok(Vec::new());
    }
    let cred = match load(&host)? {
        Some(cred) => cred,
        None => return Ok(Vec::new()),
    };
    let script = ensure_askpass_script()?;
    Ok(vec![
        (
            "GIT_ASKPASS".to_string(),
            script.to_string_lossy().to_string(),
        ),
        (ENV_USERNAME.to_string(), cred.username),
        (ENV_PASSWORD.to_string(), cred.password),
        ("GIT_TERMINAL_PROMPT".to_string(), "0".to_string()),
    ])
}

/// Route all keyring access in this test process to keyring's in-memory mock
/// store, so tests never touch the developer's real OS keychain.
#[cfg(test)]
pub(crate) fn use_mock_keyring() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        keyring::set_default_credential_builder(keyring::mock::default_credential_builder());
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_extracts_user_and_password() {
        let (cred, sanitized) =
            split_credentials_from_url("https://alice:s3cret@github.com/acme/repo.git").unwrap();
        assert_eq!(cred.username, "alice");
        assert_eq!(cred.password, "s3cret");
        assert_eq!(sanitized, "https://github.com/acme/repo.git");
    }

    #[test]
    fn split_extracts_token_only_form() {
        let (cred, sanitized) =
            split_credentials_from_url("https://ghp_token123@github.com/acme/repo.git").unwrap();
        assert_eq!(cred.username, "ghp_token123");
        assert_eq!(cred.password, "");
        assert_eq!(sanitized, "https://github.com/acme/repo.git");
    }

    #[test]
    fn split_decodes_percent_encoding() {
        let (cred, _) =
            split_credentials_from_url("https://user:p%40ss%2Fword@example.com/r.git").unwrap();
        assert_eq!(cred.password, "p@ss/word");
    }

    #[test]
    fn split_none_without_userinfo() {
        assert!(split_credentials_from_url("https://github.com/acme/repo.git").is_none());
    }

    #[test]
    fn split_none_for_ssh() {
        assert!(split_credentials_from_url("git@github.com:acme/repo.git").is_none());
        assert!(split_credentials_from_url("ssh://git@github.com/acme/repo.git").is_none());
    }

    #[test]
    fn split_keeps_port_and_path() {
        let (_, sanitized) =
            split_credentials_from_url("https://u:p@gitlab.example.com:8443/g/r.git").unwrap();
        assert_eq!(sanitized, "https://gitlab.example.com:8443/g/r.git");
    }

    #[test]
    fn https_host_strips_userinfo_and_lowercases() {
        assert_eq!(
            https_host("https://u:p@GitHub.com/acme/repo.git").as_deref(),
            Some("github.com")
        );
        assert_eq!(
            https_host("https://gitlab.example.com:8443/g/r.git").as_deref(),
            Some("gitlab.example.com:8443")
        );
        assert_eq!(https_host("git@github.com:acme/repo.git"), None);
    }

    #[test]
    fn askpass_script_answers_by_prompt() {
        // Verify the script routes "Username"/"Password" prompts to the right
        // environment variable — the contract git relies on.
        assert!(ASKPASS_SCRIPT.contains("*[Uu]sername*"));
        assert!(ASKPASS_SCRIPT.contains(ENV_USERNAME));
        assert!(ASKPASS_SCRIPT.contains(ENV_PASSWORD));
        // No secrets baked into the script itself.
        assert!(!ASKPASS_SCRIPT.contains("token"));
    }

    #[test]
    fn legacy_pat_credential_stays_backward_compatible() {
        let credential: RemoteCredential =
            serde_json::from_str(r#"{"username":"alice","password":"ghp_existing"}"#).unwrap();

        assert_eq!(credential.username, "alice");
        assert_eq!(credential.password, "ghp_existing");
        assert_eq!(credential.github_app, None);
        assert_eq!(
            refresh_decision(&credential, 1_000),
            RefreshDecision::NotNeeded
        );
    }

    #[test]
    fn github_app_credential_refreshes_before_access_token_expiry() {
        let credential = RemoteCredential {
            username: "x-access-token".to_string(),
            password: "ghu_access".to_string(),
            github_app: Some(GithubAppCredential {
                refresh_token: "ghr_refresh".to_string(),
                access_token_expires_at: 2_000,
                refresh_token_expires_at: 5_000,
                repository_id: 42,
            }),
        };

        assert_eq!(
            refresh_decision(&credential, 1_699),
            RefreshDecision::NotNeeded
        );
        assert_eq!(
            refresh_decision(&credential, 1_700),
            RefreshDecision::Refresh
        );
        assert_eq!(
            refresh_decision(&credential, 5_000),
            RefreshDecision::Reauthorize
        );
    }

    #[test]
    fn refreshed_github_app_credential_rotates_both_tokens() {
        let mut credential = RemoteCredential {
            username: "x-access-token".to_string(),
            password: "ghu_old".to_string(),
            github_app: Some(GithubAppCredential {
                refresh_token: "ghr_old".to_string(),
                access_token_expires_at: 2_000,
                refresh_token_expires_at: 5_000,
                repository_id: 42,
            }),
        };
        apply_github_app_token(
            &mut credential,
            crate::core::github_api::GithubAppToken {
                access_token: "ghu_new".to_string(),
                expires_in: 28_800,
                refresh_token: "ghr_new".to_string(),
                refresh_token_expires_in: 15_897_600,
            },
            10_000,
            42,
        );

        assert_eq!(credential.password, "ghu_new");
        assert_eq!(
            credential.github_app,
            Some(GithubAppCredential {
                refresh_token: "ghr_new".to_string(),
                access_token_expires_at: 38_800,
                refresh_token_expires_at: 15_907_600,
                repository_id: 42,
            }),
        );
    }

    #[test]
    fn github_app_credential_remembers_repository_scope() {
        let credential = github_app_credential(
            crate::core::github_api::GithubAppToken {
                access_token: "ghu_access".to_string(),
                expires_in: 28_800,
                refresh_token: "ghr_refresh".to_string(),
                refresh_token_expires_in: 15_897_600,
            },
            42,
        );

        assert_eq!(credential.github_app.unwrap().repository_id, 42);
    }

    #[test]
    fn github_app_scope_revalidation_error_reaches_git() {
        let credential = RemoteCredential {
            username: "x-access-token".to_string(),
            password: "ghu_access".to_string(),
            github_app: Some(GithubAppCredential {
                refresh_token: "ghr_refresh".to_string(),
                access_token_expires_at: i64::MAX,
                refresh_token_expires_at: i64::MAX,
                repository_id: 42,
            }),
        };
        let error = validate_github_app_scope_with(&credential, |_, _| {
            anyhow::bail!("GITHUB_APP_INSTALLATION_SCOPE: installation expanded")
        })
        .expect_err("repository boundary changes must stop git authentication");
        assert!(error.to_string().contains("GITHUB_APP_INSTALLATION_SCOPE"));
    }

    #[test]
    fn expired_github_app_error_reaches_system_git_credentials() {
        let error = credential_env_for_url_with(
            "https://expired-system-git.patchbay.test/owner/repo.git",
            |_| {
                anyhow::bail!(
                    "GITHUB_APP_REAUTH_REQUIRED: the Patchbay GitHub authorization expired"
                )
            },
        )
        .expect_err("expired app authorization must not become empty credentials");
        assert!(error.to_string().contains("GITHUB_APP_REAUTH_REQUIRED"));
    }
}
