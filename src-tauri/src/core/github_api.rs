//! Minimal GitHub REST client for guided backup setup. GitHub App user tokens
//! may connect only to a repository selected during installation; the advanced
//! PAT fallback can still create a private repository when missing. Tokens
//! never appear in URLs, logs, or error messages — callers store them in the
//! OS keychain.
//!
//! Errors carry stable prefixes (`GITHUB_TOKEN_INVALID`, `GITHUB_SCOPE`,
//! `GITHUB_NETWORK`) the frontend maps to plain-language copy.

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use super::skillssh_api::build_http_client;

const API_BASE: &str = "https://api.github.com";

/// Public client id for Patchbay's own GitHub App. Client ids are not secrets,
/// but the app must be independently owned and visibly named Patchbay; never
/// fall back to the upstream application's id.
pub const GITHUB_APP_CLIENT_ID: &str = match option_env!("PATCHBAY_GITHUB_APP_CLIENT_ID") {
    Some(client_id) => client_id,
    None => "",
};

fn github_app_client_id() -> Result<&'static str> {
    if GITHUB_APP_CLIENT_ID.trim().is_empty() {
        bail!("GITHUB_APP_NOT_CONFIGURED: Patchbay GitHub sign-in is not configured");
    }
    Ok(GITHUB_APP_CLIENT_ID)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GithubConnectInfo {
    pub login: String,
    pub repo_full_name: String,
    pub repo_id: u64,
    /// Credential-free HTTPS clone URL.
    pub url: String,
    pub repo_created: bool,
    /// False when the user connected a pre-existing PUBLIC repository — the
    /// UI warns, since a public backup is almost never intentional.
    /// Repositories created by the PAT fallback are always private.
    pub repo_private: bool,
}

#[derive(Deserialize)]
struct UserResp {
    login: String,
}

#[derive(Deserialize)]
struct RepoResp {
    id: u64,
    full_name: String,
    private: Option<bool>,
}

#[derive(Deserialize)]
struct InstallationResp {
    id: u64,
    repository_selection: String,
}

#[derive(Deserialize)]
struct InstallationsResp {
    installations: Vec<InstallationResp>,
}

#[derive(Deserialize)]
struct InstallationRepositoriesResp {
    repositories: Vec<RepoResp>,
}

/// GitHub repository name rules (subset): ASCII letters, digits, `-`, `_`,
/// `.`; not empty, not `.`/`..`, max 100 chars.
pub fn is_valid_repo_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 100
        && name != "."
        && name != ".."
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn request(
    client: &reqwest::blocking::Client,
    method: reqwest::Method,
    url: &str,
    token: &str,
) -> reqwest::blocking::RequestBuilder {
    client
        .request(method, url)
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
}

/// Validate a PAT, then ensure the backup repository exists under the token
/// owner's account (creating it as a private repo when missing).
pub fn connect_pat_backup_repo(
    token: &str,
    repo_name: &str,
    proxy_url: Option<&str>,
) -> Result<GithubConnectInfo> {
    if !is_valid_repo_name(repo_name) {
        bail!("Invalid repository name");
    }
    let client = build_http_client(proxy_url, 20);

    // Who owns this token? Also serves as token validation.
    let resp = request(
        &client,
        reqwest::Method::GET,
        &format!("{API_BASE}/user"),
        token,
    )
    .send()
    .context("GITHUB_NETWORK: could not reach api.github.com")?;
    let login = match resp.status().as_u16() {
        200 => resp.json::<UserResp>().context("Unexpected /user response")?.login,
        401 => bail!("GITHUB_TOKEN_INVALID: GitHub rejected the token (401)"),
        403 => bail!("GITHUB_TOKEN_INVALID: GitHub denied access (403); the token may lack permissions or be rate-limited"),
        s => bail!("GitHub /user returned HTTP {s}"),
    };

    // Find or create the repository.
    let resp = request(
        &client,
        reqwest::Method::GET,
        &format!("{API_BASE}/repos/{login}/{repo_name}"),
        token,
    )
    .send()
    .context("GITHUB_NETWORK: could not reach api.github.com")?;

    let (repo_created, repo) = match resp.status().as_u16() {
        200 => (
            false,
            resp.json::<RepoResp>().context("Unexpected repo response")?,
        ),
        404 => {
            let resp = request(&client, reqwest::Method::POST, &format!("{API_BASE}/user/repos"), token)
                .json(&serde_json::json!({
                    "name": repo_name,
                    "private": true,
                    "auto_init": false,
                    "description": "Patchbay backup",
                }))
                .send()
                .context("GITHUB_NETWORK: could not reach api.github.com")?;
            match resp.status().as_u16() {
                201 => (
                    true,
                    resp.json::<RepoResp>().context("Unexpected create-repo response")?,
                ),
                401 => bail!("GITHUB_TOKEN_INVALID: GitHub rejected the token (401)"),
                // Classic PATs without `repo` scope and fine-grained tokens
                // without Administration:write both land here.
                403 | 404 => bail!(
                    "GITHUB_SCOPE: the token cannot create repositories — it needs the 'repo' scope (classic) or Administration: write (fine-grained)"
                ),
                s => bail!("GitHub create-repo returned HTTP {s}"),
            }
        }
        401 => bail!("GITHUB_TOKEN_INVALID: GitHub rejected the token (401)"),
        403 => bail!("GITHUB_SCOPE: the token cannot read this repository (403); grant it access to {login}/{repo_name}"),
        s => bail!("GitHub repo lookup returned HTTP {s}"),
    };

    let full_name = repo.full_name;
    let repo_private = repo.private.unwrap_or(true);
    log::info!(
        "github connect: using repository {full_name} (created={repo_created}, private={repo_private})"
    );
    Ok(GithubConnectInfo {
        login,
        url: format!("https://github.com/{full_name}.git"),
        repo_id: repo.id,
        repo_full_name: full_name,
        repo_created,
        repo_private,
    })
}

fn github_app_repo_lookup_error(status: u16, login: &str, repo_name: &str) -> Option<String> {
    matches!(status, 403 | 404).then(|| {
        format!(
            "GITHUB_APP_REPO_ACCESS: {login}/{repo_name} is missing or Patchbay is not installed for that repository; create or select the private repository, then install Patchbay with access to it"
        )
    })
}

fn validate_github_app_repository_scope(
    repo_private: bool,
    selected_repo_id: u64,
    repository_selection: &str,
    accessible_repo_ids: &[u64],
) -> Result<()> {
    if !repo_private {
        bail!("GITHUB_APP_REPO_NOT_PRIVATE: Patchbay requires a private backup repository");
    }
    if repository_selection != "selected" || accessible_repo_ids != [selected_repo_id] {
        bail!(
            "GITHUB_APP_INSTALLATION_SCOPE: choose 'Only select repositories' when installing Patchbay and include the private backup repository"
        );
    }
    Ok(())
}

fn validate_github_app_token_scope(
    selected_repo_id: u64,
    token_accessible_repo_ids: &[u64],
) -> Result<()> {
    if token_accessible_repo_ids != [selected_repo_id] {
        bail!(
            "GITHUB_APP_INSTALLATION_SCOPE: GitHub did not limit the final credential to exactly the selected repository"
        );
    }
    Ok(())
}

fn validate_github_app_installation(
    client: &reqwest::blocking::Client,
    token: &str,
    selected_repo_id: u64,
    require_token_scope: bool,
) -> Result<()> {
    let resp = request(
        client,
        reqwest::Method::GET,
        &format!("{API_BASE}/user/installations?per_page=100"),
        token,
    )
    .send()
    .context("GITHUB_NETWORK: could not inspect the Patchbay installation")?;
    let installations = match resp.status().as_u16() {
        200 => resp
            .json::<InstallationsResp>()
            .context("Unexpected installations response")?,
        401 => bail!("GITHUB_TOKEN_INVALID: GitHub rejected the token (401)"),
        status => bail!(
            "GITHUB_APP_INSTALLATION_SCOPE: GitHub could not verify the Patchbay installation (HTTP {status})"
        ),
    };

    let mut matching_installations = 0;
    let mut token_accessible_repo_ids = Vec::new();
    for installation in installations.installations {
        let resp = request(
            client,
            reqwest::Method::GET,
            &format!(
                "{API_BASE}/user/installations/{}/repositories?per_page=100",
                installation.id
            ),
            token,
        )
        .send()
        .context("GITHUB_NETWORK: could not inspect repositories selected for Patchbay")?;
        let repositories = match resp.status().as_u16() {
            200 => resp
                .json::<InstallationRepositoriesResp>()
                .context("Unexpected installation repositories response")?,
            401 => bail!("GITHUB_TOKEN_INVALID: GitHub rejected the token (401)"),
            status => bail!(
                "GITHUB_APP_INSTALLATION_SCOPE: GitHub could not verify repositories selected for Patchbay (HTTP {status})"
            ),
        };
        let repository_ids = repositories
            .repositories
            .iter()
            .map(|repository| repository.id)
            .collect::<Vec<_>>();
        token_accessible_repo_ids.extend(repository_ids.iter().copied());
        let Some(repository) = repositories
            .repositories
            .iter()
            .find(|repository| repository.id == selected_repo_id)
        else {
            continue;
        };
        matching_installations += 1;
        validate_github_app_repository_scope(
            repository.private.unwrap_or(false),
            selected_repo_id,
            &installation.repository_selection,
            &repository_ids,
        )?;
    }
    if matching_installations != 1 {
        bail!(
            "GITHUB_APP_INSTALLATION_SCOPE: Patchbay could not identify exactly one selected installation for the private backup repository"
        );
    }
    if require_token_scope {
        token_accessible_repo_ids.sort_unstable();
        token_accessible_repo_ids.dedup();
        validate_github_app_token_scope(selected_repo_id, &token_accessible_repo_ids)?;
    }
    Ok(())
}

/// Re-check the repository boundary saved with a GitHub App credential. The
/// final user token is server-scoped to this id; this check also detects if the
/// repository later becomes public or the matching installation changes.
pub fn validate_github_app_credential_scope(
    token: &str,
    repository_id: u64,
    proxy_url: Option<&str>,
) -> Result<()> {
    if repository_id == 0 {
        bail!("GITHUB_APP_REAUTH_REQUIRED: repository scope metadata is missing");
    }
    let client = build_http_client(proxy_url, 20);
    match validate_github_app_installation(&client, token, repository_id, true) {
        Err(error) if error.to_string().contains("GITHUB_TOKEN_INVALID") => {
            bail!("GITHUB_APP_REAUTH_REQUIRED: GitHub rejected the saved authorization")
        }
        result => result,
    }
}

/// Validate a GitHub App user token and connect only to an existing repository
/// selected during app installation. This path deliberately never creates a
/// repository: doing so would require broad Administration permission.
pub fn connect_github_app_backup_repo(
    token: &str,
    repo_name: &str,
    expected_repo_id: Option<u64>,
    proxy_url: Option<&str>,
) -> Result<GithubConnectInfo> {
    if !is_valid_repo_name(repo_name) {
        bail!("Invalid repository name");
    }
    let client = build_http_client(proxy_url, 20);
    let resp = request(
        &client,
        reqwest::Method::GET,
        &format!("{API_BASE}/user"),
        token,
    )
    .send()
    .context("GITHUB_NETWORK: could not reach api.github.com")?;
    let login = match resp.status().as_u16() {
        200 => {
            resp.json::<UserResp>()
                .context("Unexpected /user response")?
                .login
        }
        401 => bail!("GITHUB_TOKEN_INVALID: GitHub rejected the token (401)"),
        403 => bail!(
            "GITHUB_TOKEN_INVALID: GitHub denied access (403); the authorization may have expired"
        ),
        status => bail!("GitHub /user returned HTTP {status}"),
    };

    let resp = request(
        &client,
        reqwest::Method::GET,
        &format!("{API_BASE}/repos/{login}/{repo_name}"),
        token,
    )
    .send()
    .context("GITHUB_NETWORK: could not reach api.github.com")?;
    let status = resp.status().as_u16();
    let repo = match status {
        200 => resp
            .json::<RepoResp>()
            .context("Unexpected repo response")?,
        401 => bail!("GITHUB_TOKEN_INVALID: GitHub rejected the token (401)"),
        status => match github_app_repo_lookup_error(status, &login, repo_name) {
            Some(message) => bail!(message),
            None => bail!("GitHub repo lookup returned HTTP {status}"),
        },
    };

    if expected_repo_id.is_some_and(|repository_id| repository_id != repo.id) {
        bail!("GITHUB_APP_REPO_ACCESS: the authorized repository does not match the repository selected during setup");
    }
    validate_github_app_installation(&client, token, repo.id, expected_repo_id.is_some())?;
    let full_name = repo.full_name;
    let repo_private = repo.private.unwrap_or(false);
    log::info!(
        "github app connect: using selected repository {full_name} (private={repo_private})"
    );
    Ok(GithubConnectInfo {
        login,
        url: format!("https://github.com/{full_name}.git"),
        repo_id: repo.id,
        repo_full_name: full_name,
        repo_created: false,
        repo_private,
    })
}

// ── Device Flow (§3.2) ──

#[derive(Debug, Clone, serde::Serialize)]
pub struct DeviceFlowStart {
    pub device_code: String,
    /// The 8-character code the user types at `verification_uri`.
    pub user_code: String,
    pub verification_uri: String,
    /// Seconds until the codes expire (GitHub: 900).
    pub expires_in: u64,
    /// Minimum seconds between polls (GitHub: 5).
    pub interval: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DevicePollOutcome {
    Pending,
    /// Polled too fast — caller must add 5 seconds to its interval.
    SlowDown,
    Authorized(GithubAppToken),
}

#[derive(Debug, Clone, PartialEq)]
pub struct GithubAppToken {
    pub access_token: String,
    pub expires_in: u64,
    pub refresh_token: String,
    pub refresh_token_expires_in: u64,
}

fn device_flow_form(client_id: &str) -> Vec<(&str, &str)> {
    vec![("client_id", client_id)]
}

fn device_poll_form(
    client_id: &str,
    device_code: &str,
    repository_id: Option<u64>,
) -> Vec<(String, String)> {
    let mut form = vec![
        ("client_id".to_string(), client_id.to_string()),
        ("device_code".to_string(), device_code.to_string()),
        (
            "grant_type".to_string(),
            "urn:ietf:params:oauth:grant-type:device_code".to_string(),
        ),
    ];
    if let Some(repository_id) = repository_id {
        form.push(("repository_id".to_string(), repository_id.to_string()));
    }
    form
}

fn refresh_token_form<'a>(client_id: &'a str, refresh_token: &'a str) -> Vec<(&'a str, &'a str)> {
    vec![
        ("client_id", client_id),
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
    ]
}

fn parse_github_app_token(v: &serde_json::Value) -> Result<GithubAppToken> {
    let string_field = |key: &str| {
        v.get(key)
            .and_then(|value| value.as_str())
            .map(str::to_string)
            .with_context(|| format!("token response missing {key}"))
    };
    let integer_field = |key: &str| {
        v.get(key)
            .and_then(|value| value.as_u64())
            .with_context(|| format!("token response missing {key}"))
    };
    Ok(GithubAppToken {
        access_token: string_field("access_token")?,
        expires_in: integer_field("expires_in")?,
        refresh_token: string_field("refresh_token")?,
        refresh_token_expires_in: integer_field("refresh_token_expires_in")?,
    })
}

fn parse_device_poll_response(v: serde_json::Value) -> Result<DevicePollOutcome> {
    if let Some(err) = v.get("error").and_then(|e| e.as_str()) {
        return match err {
            "authorization_pending" => Ok(DevicePollOutcome::Pending),
            "slow_down" => Ok(DevicePollOutcome::SlowDown),
            "expired_token" => bail!("GITHUB_DEVICE_EXPIRED: the verification code expired"),
            "access_denied" => bail!("GITHUB_DEVICE_DENIED: authorization was declined on GitHub"),
            other => bail!("GitHub device flow failed: {other}"),
        };
    }

    Ok(DevicePollOutcome::Authorized(parse_github_app_token(&v)?))
}

/// Request a device + user code pair to start the flow.
pub fn device_flow_start(proxy_url: Option<&str>) -> Result<DeviceFlowStart> {
    let client_id = github_app_client_id()?;
    let client = build_http_client(proxy_url, 20);
    let resp = client
        .post("https://github.com/login/device/code")
        .header("Accept", "application/json")
        .form(&device_flow_form(client_id))
        .send()
        .context("GITHUB_NETWORK: could not reach github.com")?;
    if !resp.status().is_success() {
        bail!(
            "GitHub device-code endpoint returned HTTP {}",
            resp.status()
        );
    }
    let v: serde_json::Value = resp.json().context("Unexpected device-code response")?;
    let field = |k: &str| {
        v.get(k)
            .and_then(|x| x.as_str())
            .map(str::to_string)
            .with_context(|| format!("device-code response missing {k}"))
    };
    Ok(DeviceFlowStart {
        device_code: field("device_code")?,
        user_code: field("user_code")?,
        verification_uri: field("verification_uri")?,
        expires_in: v.get("expires_in").and_then(|x| x.as_u64()).unwrap_or(900),
        interval: v.get("interval").and_then(|x| x.as_u64()).unwrap_or(5),
    })
}

/// One poll of the token endpoint. The caller owns the pacing loop
/// (`interval` seconds between calls, +5s on `SlowDown`, stop at expiry).
pub fn device_flow_poll(
    device_code: &str,
    repository_id: Option<u64>,
    proxy_url: Option<&str>,
) -> Result<DevicePollOutcome> {
    let client_id = github_app_client_id()?;
    let client = build_http_client(proxy_url, 20);
    let resp = client
        .post("https://github.com/login/oauth/access_token")
        .header("Accept", "application/json")
        .form(&device_poll_form(client_id, device_code, repository_id))
        .send()
        .context("GITHUB_NETWORK: could not reach github.com")?;
    let v: serde_json::Value = resp.json().context("Unexpected token response")?;

    parse_device_poll_response(v)
}

/// Exchange an expiring GitHub App user token for a new access/refresh pair.
/// Device-flow refreshes need the public client id but no embedded secret.
pub fn refresh_github_app_token(
    refresh_token: &str,
    proxy_url: Option<&str>,
) -> Result<GithubAppToken> {
    let client_id = github_app_client_id()?;
    let client = build_http_client(proxy_url, 20);
    let resp = client
        .post("https://github.com/login/oauth/access_token")
        .header("Accept", "application/json")
        .form(&refresh_token_form(client_id, refresh_token))
        .send()
        .context("GITHUB_NETWORK: could not reach github.com")?;
    let v: serde_json::Value = resp.json().context("Unexpected token refresh response")?;
    if let Some(error) = v.get("error").and_then(|value| value.as_str()) {
        bail!("GITHUB_APP_REAUTH_REQUIRED: GitHub could not refresh the authorization ({error})");
    }
    parse_github_app_token(&v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_name_validation() {
        assert!(is_valid_repo_name("patchbay-backup"));
        assert!(is_valid_repo_name("My_Backup.2026"));
        assert!(!is_valid_repo_name(""));
        assert!(!is_valid_repo_name("."));
        assert!(!is_valid_repo_name(".."));
        assert!(!is_valid_repo_name("has space"));
        assert!(!is_valid_repo_name("has/slash"));
        assert!(!is_valid_repo_name(&"x".repeat(101)));
    }

    #[test]
    fn github_app_device_flow_requests_no_oauth_scopes() {
        assert_eq!(
            device_flow_form("Iv1.patchbay-client"),
            vec![("client_id", "Iv1.patchbay-client")],
        );
    }

    #[test]
    fn github_app_final_device_token_is_repository_scoped() {
        assert_eq!(
            device_poll_form("Iv1.patchbay-client", "device-code", Some(42)),
            vec![
                ("client_id".to_string(), "Iv1.patchbay-client".to_string()),
                ("device_code".to_string(), "device-code".to_string()),
                (
                    "grant_type".to_string(),
                    "urn:ietf:params:oauth:grant-type:device_code".to_string(),
                ),
                ("repository_id".to_string(), "42".to_string()),
            ]
        );
    }

    #[test]
    fn github_app_device_token_keeps_refresh_metadata() {
        let outcome = parse_device_poll_response(serde_json::json!({
            "access_token": "ghu_access",
            "expires_in": 28_800,
            "refresh_token": "ghr_refresh",
            "refresh_token_expires_in": 15_897_600,
            "token_type": "bearer"
        }))
        .unwrap();

        assert_eq!(
            outcome,
            DevicePollOutcome::Authorized(GithubAppToken {
                access_token: "ghu_access".to_string(),
                expires_in: 28_800,
                refresh_token: "ghr_refresh".to_string(),
                refresh_token_expires_in: 15_897_600,
            }),
        );
    }

    #[test]
    fn github_app_requires_selected_repository_access() {
        assert_eq!(
            github_app_repo_lookup_error(200, "alice", "patchbay-backup"),
            None
        );
        for status in [403, 404] {
            let message = github_app_repo_lookup_error(status, "alice", "patchbay-backup")
                .expect("missing or unselected repositories must be rejected");
            assert!(message.contains("GITHUB_APP_REPO_ACCESS"));
            assert!(message.contains("alice/patchbay-backup"));
            assert!(message.contains("install"));
        }
    }

    #[test]
    fn github_app_requires_selected_private_repository_installation() {
        assert!(validate_github_app_repository_scope(true, 42, "selected", &[42]).is_ok());

        let public = validate_github_app_repository_scope(false, 42, "selected", &[42])
            .expect_err("public backup repositories must be rejected");
        assert!(public.to_string().contains("GITHUB_APP_REPO_NOT_PRIVATE"));

        for result in [
            validate_github_app_repository_scope(true, 42, "all", &[42]),
            validate_github_app_repository_scope(true, 42, "selected", &[42, 43]),
            validate_github_app_repository_scope(true, 42, "selected", &[43]),
        ] {
            let broad = result.expect_err("the app must be limited to exactly the selected repo");
            assert!(
                broad.to_string().contains("GITHUB_APP_INSTALLATION_SCOPE"),
                "unexpected error: {broad:#}"
            );
        }
    }

    #[test]
    fn github_app_final_token_can_see_only_the_selected_repository() {
        assert!(validate_github_app_token_scope(42, &[42]).is_ok());
        let error = validate_github_app_token_scope(42, &[42, 43])
            .expect_err("the final token must be hard-scoped to one repository");
        assert!(error.to_string().contains("GITHUB_APP_INSTALLATION_SCOPE"));
    }

    #[test]
    fn github_app_refresh_never_embeds_a_client_secret() {
        assert_eq!(
            refresh_token_form("Iv1.patchbay-client", "ghr_refresh"),
            vec![
                ("client_id", "Iv1.patchbay-client"),
                ("grant_type", "refresh_token"),
                ("refresh_token", "ghr_refresh"),
            ],
        );
    }
}
