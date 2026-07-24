//! Fleet manifest: the single source of truth for the managed repo list,
//! stored as `manifest.toml` inside the meta repo (design §3). Machines pull
//! it from the hub; edits are pushed back explicitly — there is no second
//! authoritative copy anywhere.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::core::error::AppError;
use crate::core::path_guard;

/// Authority sentinel for repos that fast-forward in both directions.
pub const AUTHORITY_SHARED: &str = "shared";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    #[serde(default)]
    pub fleet: FleetSection,
    /// Hub definitions keyed by hub name (= the remote name, decision #24-2).
    #[serde(default, rename = "hub")]
    pub hubs: BTreeMap<String, Hub>,
    #[serde(default, rename = "repo")]
    pub repos: Vec<RepoEntry>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetSection {
    /// Default projects root, overridable per machine via local settings.
    pub projects_root: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hub {
    /// Base URL; a repo's address is `<url>/<name>.git`.
    pub url: String,
    /// Machine that hosts the hub. On that machine the URL resolves to a
    /// local path (design axiom 2: no self-ssh).
    pub host_machine: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoEntry {
    pub name: String,
    pub hub: String,
    /// A machine id, or [`AUTHORITY_SHARED`].
    pub authority: String,
    /// Branch bootstrap checks out and status compares against the hub.
    pub branch: String,
    /// Explicit P2 opt-in. Missing and false both mean the automatic round
    /// must not inspect or mutate this repository.
    #[serde(default, skip_serializing_if = "is_false")]
    pub auto_sync: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// Parse and validate `manifest.toml`. Validation is strict so a bad manifest
/// surfaces at read time, not as a confusing downstream git error: every repo
/// must reference a defined hub, and repo names must be plain directory names
/// (no separators / traversal — they are joined under projects_root).
pub fn parse(text: &str) -> Result<Manifest, AppError> {
    let manifest: Manifest =
        toml::from_str(text).map_err(|e| AppError::invalid_input(format!("manifest.toml: {e}")))?;
    for (name, hub) in &manifest.hubs {
        if let Err(reason) = check_hub_url(&hub.url) {
            return Err(AppError::invalid_input(format!(
                "manifest.toml: hub {name:?} has an unsafe url: {reason}"
            )));
        }
    }
    for repo in &manifest.repos {
        if repo.name.is_empty() || path_guard::sanitize_name(&repo.name) != repo.name {
            return Err(AppError::invalid_input(format!(
                "manifest.toml: unsafe repo name {:?}",
                repo.name
            )));
        }
        if repo.hub.trim().is_empty()
            || repo.authority.trim().is_empty()
            || repo.branch.trim().is_empty()
        {
            return Err(AppError::invalid_input(format!(
                "manifest.toml: repo {:?} needs non-empty hub, authority, and branch",
                repo.name
            )));
        }
        if !manifest.hubs.contains_key(&repo.hub) {
            return Err(AppError::invalid_input(format!(
                "manifest.toml: repo {:?} references undefined hub {:?}",
                repo.name, repo.hub
            )));
        }
    }
    Ok(manifest)
}

/// [`check_hub_url`] for a URL that did not come from the manifest — the fleet
/// meta repo URL, which is set locally but is fed to git exactly the same way.
/// Local origin makes it lower-risk, not safe: a value beginning with `-` still
/// reaches git as an option, so the boundary is enforced where the value is
/// written rather than trusted because a human typed it.
pub fn check_remote_url(url: &str) -> Result<(), AppError> {
    check_hub_url(url).map_err(|reason| AppError::invalid_input(format!("unsafe url: {reason}")))
}

/// Reject hub URLs that git would not treat as a plain location.
///
/// The manifest is *remote* data — it arrives from the hub and is consumed
/// automatically by every machine, including on the read-only status path. A
/// value beginning with `-` is parsed by git as an option, and
/// `--upload-pack=<cmd>` is executed, so this is a command-execution boundary
/// rather than a formatting preference. Call sites also pass `--` before the
/// URL; this check is the second layer, and it additionally pins the transport
/// to the schemes the design actually contemplates (design §1 hub profiles).
fn check_hub_url(url: &str) -> Result<(), &'static str> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err("empty");
    }
    if trimmed.starts_with('-') {
        return Err("starts with '-', which git parses as an option");
    }
    // `<helper>::<address>` is git's remote-helper syntax; `ext::` runs an
    // arbitrary command. Checked before `://` because a scheme URL has no `::`.
    if trimmed.contains("::") {
        return Err("unsupported transport scheme (remote-helper syntax)");
    }
    if let Some((scheme, _)) = trimmed.split_once("://") {
        return match scheme.to_ascii_lowercase().as_str() {
            "https" | "http" | "ssh" | "git" | "file" => Ok(()),
            // `ext::` in particular runs an arbitrary helper command.
            _ => Err("unsupported transport scheme"),
        };
    }
    // Remaining accepted forms: scp-style `host:path` and plain local paths.
    Ok(())
}

/// Serialize back to TOML (manifest edits are written to the meta repo).
pub fn to_toml(manifest: &Manifest) -> Result<String, AppError> {
    toml::to_string_pretty(manifest)
        .map_err(|e| AppError::internal(format!("serialize manifest: {e}")))
}

/// Resolve a hub's base for `self_machine`. On the hub's own host an
/// scp-style URL (`host:relative/path`) collapses to a local filesystem path
/// under `$HOME`, sidestepping the self-ssh trap; everywhere else (and for
/// URLs that are not scp-style) the URL is used as written.
pub fn resolve_hub_base(hub: &Hub, self_machine: &str) -> String {
    if hub.host_machine.as_deref() != Some(self_machine) {
        return hub.url.clone();
    }
    match split_scp_path(&hub.url) {
        Some(path) => {
            let path = PathBuf::from(path);
            // `has_root`, not `is_absolute`: the manifest is shared across
            // machines, so a POSIX hub path like `/srv/mirrors` must survive
            // being read on Windows, where it is rooted but not absolute (no
            // drive letter) and would otherwise be re-anchored under $HOME.
            if path.has_root() {
                path.to_string_lossy().into_owned()
            } else {
                dirs::home_dir()
                    .unwrap_or_default()
                    .join(path)
                    .to_string_lossy()
                    .into_owned()
            }
        }
        None => hub.url.clone(),
    }
}

/// The path part of an scp-style `host:path` URL; `None` for scheme URLs
/// (`ssh://`, `https://`), plain paths, and Windows drive letters.
fn split_scp_path(url: &str) -> Option<&str> {
    if url.contains("://") {
        return None;
    }
    let (host, path) = url.split_once(':')?;
    // A single leading letter is a Windows drive, not a host.
    if host.len() <= 1 {
        return None;
    }
    Some(path)
}

/// Full git URL of one repo on a resolved hub base.
pub fn repo_url(hub_base: &str, name: &str) -> String {
    format!("{}/{}.git", hub_base.trim_end_matches('/'), name)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
[fleet]
projects_root = "~/Projects"

[hub.metis]
url = "metis:git-mirrors/projects"
host_machine = "metis"

[[repo]]
name = "prompt-optimizer"
hub = "metis"
authority = "metis"
branch = "main"

[[repo]]
name = "paperdock"
hub = "metis"
authority = "helios"
branch = "main"
auto_sync = true
"#;

    #[test]
    fn parses_sample_manifest() {
        let m = parse(SAMPLE).unwrap();
        assert_eq!(m.fleet.projects_root.as_deref(), Some("~/Projects"));
        assert_eq!(m.hubs["metis"].url, "metis:git-mirrors/projects");
        assert_eq!(m.hubs["metis"].host_machine.as_deref(), Some("metis"));
        assert_eq!(m.repos.len(), 2);
        assert_eq!(m.repos[0].name, "prompt-optimizer");
        assert!(!m.repos[0].auto_sync);
        assert_eq!(m.repos[1].authority, "helios");
        assert!(m.repos[1].auto_sync);
    }

    #[test]
    fn round_trips_through_toml() {
        let m = parse(SAMPLE).unwrap();
        let text = to_toml(&m).unwrap();
        let again = parse(&text).unwrap();
        assert_eq!(again.repos.len(), 2);
        assert_eq!(again.hubs["metis"].url, m.hubs["metis"].url);
        assert!(!again.repos[0].auto_sync);
        assert!(again.repos[1].auto_sync);
    }

    #[test]
    fn rejects_undefined_hub_reference() {
        let bad = r#"
[[repo]]
name = "a"
hub = "nope"
authority = "shared"
branch = "main"
"#;
        let err = parse(bad).unwrap_err();
        assert!(err.message.contains("undefined hub"));
    }

    #[test]
    fn rejects_hub_urls_git_would_read_as_options_or_helper_transports() {
        // A dash-leading URL reaches git as `--upload-pack=<cmd>`, which git
        // executes — the manifest is remote data, so this must never parse.
        let injected = r#"
[hub.evil]
url = "--upload-pack=touch /tmp/pwned #"

[[repo]]
name = "alpha"
hub = "evil"
authority = "shared"
branch = "main"
"#;
        let err = parse(injected).unwrap_err();
        assert!(err.message.contains("unsafe url"), "got: {}", err.message);

        // `ext::` runs an arbitrary helper command.
        let ext = r#"
[hub.evil]
url = "ext::sh -c 'touch /tmp/pwned'"
"#;
        assert!(parse(ext)
            .unwrap_err()
            .message
            .contains("unsupported transport scheme"));

        assert!(parse("[hub.blank]\nurl = \"  \"\n")
            .unwrap_err()
            .message
            .contains("empty"));
    }

    #[test]
    fn check_remote_url_holds_the_same_line_for_locally_set_urls() {
        // The meta URL is typed by a human rather than read from the manifest,
        // which lowers the odds, not the consequence: it reaches git the same
        // way, so it gets the same allowlist (#54).
        for bad in [
            "--upload-pack=touch /tmp/pwned #",
            "ext::sh -c 'touch /tmp/pwned'",
            "   ",
        ] {
            assert!(
                check_remote_url(bad).is_err(),
                "should reject meta url {bad:?}"
            );
        }
        assert!(
            check_remote_url("metis:/Users/me/git-mirrors/projects/_patchbay-fleet.git").is_ok()
        );
    }

    #[test]
    fn accepts_the_hub_forms_the_design_contemplates() {
        for url in [
            "metis:git-mirrors/projects",
            "metis:/Users/me/git-mirrors/projects",
            "/Users/me/git-mirrors/projects",
            "https://example.com/mirrors",
            "ssh://host/srv/mirrors",
        ] {
            let text = format!("[hub.h]\nurl = \"{url}\"\n");
            assert!(parse(&text).is_ok(), "should accept {url}");
        }
    }

    #[test]
    fn rejects_unsafe_repo_names() {
        let bad = r#"
[hub.h]
url = "h:mirrors"

[[repo]]
name = "../escape"
hub = "h"
authority = "shared"
branch = "main"
"#;
        let err = parse(bad).unwrap_err();
        assert!(err.message.contains("unsafe repo name"));
    }

    #[test]
    fn rejects_blank_hub_authority_and_branch_fields() {
        for (field, value) in [("hub", "   "), ("authority", "   "), ("branch", "   ")] {
            let bad = format!(
                r#"
[hub.test]
url = "test:mirrors"

[[repo]]
name = "alpha"
hub = "{}"
authority = "{}"
branch = "{}"
"#,
                if field == "hub" { value } else { "test" },
                if field == "authority" {
                    value
                } else {
                    "shared"
                },
                if field == "branch" { value } else { "main" },
            );
            let err = parse(&bad).unwrap_err();
            assert!(
                err.message.contains("non-empty hub, authority, and branch"),
                "field {field}: {}",
                err.message
            );
        }
    }

    #[test]
    fn hub_resolves_to_local_home_path_on_host_machine() {
        let hub = Hub {
            url: "metis:git-mirrors/projects".into(),
            host_machine: Some("metis".into()),
        };
        let base = resolve_hub_base(&hub, "metis");
        let expected = dirs::home_dir()
            .unwrap()
            .join("git-mirrors/projects")
            .to_string_lossy()
            .into_owned();
        assert_eq!(base, expected);
        // Other machines keep the ssh form untouched.
        assert_eq!(
            resolve_hub_base(&hub, "helios"),
            "metis:git-mirrors/projects"
        );
    }

    #[test]
    fn hub_resolution_leaves_scheme_urls_and_absolute_paths_alone() {
        let https = Hub {
            url: "https://example.com/mirrors".into(),
            host_machine: Some("metis".into()),
        };
        assert_eq!(
            resolve_hub_base(&https, "metis"),
            "https://example.com/mirrors"
        );

        let abs = Hub {
            url: "metis:/srv/mirrors".into(),
            host_machine: Some("metis".into()),
        };
        assert_eq!(resolve_hub_base(&abs, "metis"), "/srv/mirrors");
    }

    #[test]
    fn repo_url_joins_and_trims() {
        assert_eq!(
            repo_url("metis:git-mirrors/projects", "a"),
            "metis:git-mirrors/projects/a.git"
        );
        assert_eq!(repo_url("/srv/mirrors/", "a"), "/srv/mirrors/a.git");
    }
}
