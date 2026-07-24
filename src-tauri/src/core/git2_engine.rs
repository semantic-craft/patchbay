//! Credential-aware git2 network engine for HTTP(S) remotes.
//!
//! Scope is deliberately narrow: only the four network operations (fetch,
//! push, ls-remote, clone) against http(s) remotes go through libgit2, with
//! credentials injected in-memory from the OS keychain. All local operations
//! (commit, tag, status, merge, read-tree) stay on system git, and SSH /
//! custom remotes always use system git. Backup routing remains opt-in via the
//! `git_backup_engine` setting; Fleet calls the explicit-URL helpers directly
//! for its locked HTTP(S) transport contract.
//!
//! Error normalization matters here: the frontend maps error text produced
//! by system git ("Authentication failed", "Could not resolve host",
//! "non-fast-forward", …) to plain-language copy. libgit2 phrases the same
//! failures differently, so every error leaving this module is prefixed with
//! the equivalent system-git marker.

use anyhow::{Context, Result};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use super::git_credentials;

static PILOT_ENABLED: AtomicBool = AtomicBool::new(false);
static PROXY_URL: OnceLock<Mutex<Option<String>>> = OnceLock::new();

/// Sync the engine preference from settings. Called at the entry of backup
/// commands (core code has no store access).
pub fn set_preference(git2_enabled: bool, proxy_url: Option<String>) {
    PILOT_ENABLED.store(git2_enabled, Ordering::Relaxed);
    *PROXY_URL
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = proxy_url.filter(|s| !s.is_empty());
}

fn proxy_url() -> Option<String> {
    PROXY_URL
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// Whether the git2 engine should handle operations against `url`.
pub fn applies_to(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    PILOT_ENABLED.load(Ordering::Relaxed)
        && (lower.starts_with("https://") || lower.starts_with("http://"))
}

fn callbacks_for(url: &str) -> Result<git2::RemoteCallbacks<'static>> {
    callbacks_for_with(url, git_credentials::load_credential)
}

fn callbacks_for_with<F>(url: &str, load: F) -> Result<git2::RemoteCallbacks<'static>>
where
    F: FnOnce(&str) -> Result<Option<git_credentials::RemoteCredential>>,
{
    let cred = match git_credentials::https_host(url) {
        Some(host) => load(&host)?,
        None => None,
    };
    let mut callbacks = git2::RemoteCallbacks::new();
    // libgit2 re-invokes the credentials callback after a rejection; without
    // a cap that loops forever on a bad token.
    let mut attempts = 0;
    callbacks.credentials(move |_url, username_from_url, _allowed| {
        attempts += 1;
        if attempts > 2 {
            return Err(git2::Error::from_str("authentication attempts exhausted"));
        }
        match &cred {
            Some(c) => git2::Cred::userpass_plaintext(&c.username, &c.password),
            // No stored credential: try the URL's own username (if any) with
            // an empty password rather than hanging on a prompt.
            None => git2::Cred::userpass_plaintext(username_from_url.unwrap_or_default(), ""),
        }
    });
    Ok(callbacks)
}

fn proxy_options() -> git2::ProxyOptions<'static> {
    let mut opts = git2::ProxyOptions::new();
    match proxy_url() {
        Some(url) => {
            opts.url(&url);
        }
        None => {
            opts.auto();
        }
    }
    opts
}

fn fetch_options(url: &str) -> Result<git2::FetchOptions<'static>> {
    let mut opts = git2::FetchOptions::new();
    opts.remote_callbacks(callbacks_for(url)?);
    opts.proxy_options(proxy_options());
    Ok(opts)
}

/// Translate a libgit2 error into the marker vocabulary the frontend's git
/// error mapping already understands.
fn normalize_err(e: git2::Error, operation: &str) -> anyhow::Error {
    let msg = e.message().to_string();
    let lower = msg.to_ascii_lowercase();
    let marker = if e.code() == git2::ErrorCode::Auth
        || lower.contains("authentication")
        || lower.contains("401")
        || lower.contains("403")
    {
        "Authentication failed"
    } else if e.code() == git2::ErrorCode::NotFastForward {
        "non-fast-forward"
    } else if e.class() == git2::ErrorClass::Net
        || lower.contains("resolve")
        || lower.contains("connect")
        || lower.contains("timed out")
    {
        "Failed to connect"
    } else if e.class() == git2::ErrorClass::Ssl {
        "TLS/SSL error"
    } else {
        ""
    };
    if marker.is_empty() {
        anyhow::anyhow!("git2 {operation} failed: {msg}")
    } else {
        anyhow::anyhow!("git2 {operation} failed: {marker}: {msg}")
    }
}

/// Fetch `branch` (or the remote's configured refspecs when `None`) from
/// origin, updating the usual remote-tracking refs.
pub fn fetch(repo_dir: &Path, branch: Option<&str>, url: &str) -> Result<()> {
    let repo = git2::Repository::open(repo_dir).context("Failed to open repository")?;
    let mut remote = repo.find_remote("origin").context("No origin remote")?;
    let refspecs: Vec<String> = match branch {
        Some(b) => vec![format!("+refs/heads/{b}:refs/remotes/origin/{b}")],
        None => Vec::new(),
    };
    let refs: Vec<&str> = refspecs.iter().map(String::as_str).collect();
    let mut options = fetch_options(url)?;
    remote
        .fetch(&refs, Some(&mut options), None)
        .map_err(|e| normalize_err(e, "fetch"))?;
    log::info!(
        "git2 fetch: done ({})",
        branch.unwrap_or("configured refspecs")
    );
    Ok(())
}

/// Push the given refspecs to origin. Per-reference rejections (the
/// non-fast-forward case) are surfaced as errors even though the transport
/// call itself succeeds.
pub fn push_refs(repo_dir: &Path, refspecs: &[String], url: &str) -> Result<()> {
    let repo = git2::Repository::open(repo_dir).context("Failed to open repository")?;
    let mut remote = repo.find_remote("origin").context("No origin remote")?;

    push_remote(&mut remote, refspecs, url)
}

/// Push refspecs to an explicit URL rather than a configured local remote.
/// Fleet uses this because the manifest hub is authoritative and a checkout's
/// existing `origin`/`alpha` remote may intentionally differ.
pub fn push_refs_to_url(repo_dir: &Path, refspecs: &[String], url: &str) -> Result<()> {
    let repo = git2::Repository::open(repo_dir).context("Failed to open repository")?;
    let mut remote = repo
        .remote_anonymous(url)
        .context("Failed to create explicit push remote")?;
    push_remote(&mut remote, refspecs, url)
}

fn push_remote(remote: &mut git2::Remote<'_>, refspecs: &[String], url: &str) -> Result<()> {
    let rejected: std::sync::Arc<Mutex<Vec<String>>> = Default::default();
    let rejected_in_cb = rejected.clone();
    let mut callbacks = callbacks_for(url)?;
    callbacks.push_update_reference(move |refname, status| {
        if let Some(reason) = status {
            rejected_in_cb
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(format!("{refname}: {reason}"));
        }
        Ok(())
    });

    let mut opts = git2::PushOptions::new();
    opts.remote_callbacks(callbacks);
    opts.proxy_options(proxy_options());

    remote
        .push(refspecs, Some(&mut opts))
        .map_err(|e| normalize_err(e, "push"))?;

    let rejected = rejected.lock().unwrap_or_else(|e| e.into_inner());
    if !rejected.is_empty() {
        // Same vocabulary as system git so the UI routes to recovery.
        anyhow::bail!(
            "git2 push failed: non-fast-forward, failed to push some refs ({})",
            rejected.join("; ")
        );
    }
    log::info!("git2 push: pushed {} refspec(s)", refspecs.len());
    Ok(())
}

/// List remote ref names (heads and tags) for `url` without a local repo.
pub fn ls_remote_refs(url: &str) -> Result<Vec<String>> {
    let mut remote =
        git2::Remote::create_detached(url).context("Failed to create detached remote")?;
    let connection = remote
        .connect_auth(
            git2::Direction::Fetch,
            Some(callbacks_for(url)?),
            Some(proxy_options()),
        )
        .map_err(|e| normalize_err(e, "ls-remote"))?;
    let names = connection
        .list()
        .map_err(|e| normalize_err(e, "ls-remote"))?
        .iter()
        .map(|head| head.name().to_string())
        .collect();
    Ok(names)
}

/// Resolve one remote ref to its full object id through the credential-aware
/// git2 transport. Missing refs are returned as `None`.
pub fn ls_remote_ref_oid(url: &str, refname: &str) -> Result<Option<String>> {
    let mut remote =
        git2::Remote::create_detached(url).context("Failed to create detached remote")?;
    let connection = remote
        .connect_auth(
            git2::Direction::Fetch,
            Some(callbacks_for(url)?),
            Some(proxy_options()),
        )
        .map_err(|e| normalize_err(e, "ls-remote"))?;
    let oid = connection
        .list()
        .map_err(|e| normalize_err(e, "ls-remote"))?
        .iter()
        .find(|head| head.name() == refname)
        .map(|head| head.oid().to_string());
    Ok(oid)
}

/// Fetch one explicit branch from an explicit URL into the repository object
/// database without consulting or rewriting any configured remote. Fleet uses
/// this for HTTPS hubs; SSH remains on system git so ssh config/agent behavior
/// is preserved.
pub fn fetch_ref_from_url(repo_dir: &Path, url: &str, branch: &str) -> Result<()> {
    let repo = git2::Repository::open(repo_dir).context("Failed to open repository")?;
    let mut remote = repo
        .remote_anonymous(url)
        .context("Failed to create explicit fetch remote")?;
    let mut fetch_opts = git2::FetchOptions::new();
    fetch_opts.remote_callbacks(callbacks_for(url)?);
    fetch_opts.proxy_options(proxy_options());
    let source = format!("refs/heads/{branch}");
    remote
        .fetch(&[source.as_str()], Some(&mut fetch_opts), None)
        .map_err(|error| normalize_err(error, "fetch"))
}

/// Full clone (backup needs complete history — no shallow).
pub fn clone(url: &str, dest: &Path) -> Result<()> {
    let mut builder = git2::build::RepoBuilder::new();
    builder.fetch_options(fetch_options(url)?);
    builder
        .clone(url, dest)
        .map_err(|e| normalize_err(e, "clone"))?;
    log::info!("git2 clone: done");
    Ok(())
}

/// Clone one explicit branch while naming the sole configured remote.
/// Fleet uses this for HTTPS hubs so credentials stay in memory and the
/// manifest hub name is preserved instead of creating `origin`.
pub fn clone_branch_with_remote(
    url: &str,
    dest: &Path,
    remote_name: &str,
    branch: &str,
) -> Result<()> {
    let mut fetch_opts = fetch_options(url)?;
    // Match the system-git path's `--no-tags`, so both transports leave the
    // same repository on disk regardless of the hub's URL scheme.
    fetch_opts.download_tags(git2::AutotagOption::None);
    let mut builder = git2::build::RepoBuilder::new();
    builder.fetch_options(fetch_opts);
    builder.branch(branch);
    let remote_name = remote_name.to_string();
    let branch_refspec = branch.to_string();
    // `--single-branch`: create the remote with a refspec limited to the one
    // manifest branch instead of libgit2's default `+refs/heads/*`.
    builder.remote_create(move |repo, _, remote_url| {
        repo.remote_with_fetch(
            &remote_name,
            remote_url,
            &format!("+refs/heads/{branch_refspec}:refs/remotes/{remote_name}/{branch_refspec}"),
        )
    });
    builder
        .clone(url, dest)
        .map_err(|error| normalize_err(error, "clone"))?;
    log::info!("git2 fleet clone: done");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    /// Platform-correct file URL: Windows paths need forward slashes and a
    /// third slash before the drive letter (`file:///C:/...`).
    fn file_url(path: &Path) -> String {
        let s = path.display().to_string().replace('\\', "/");
        if s.starts_with('/') {
            format!("file://{s}")
        } else {
            format!("file:///{s}")
        }
    }

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.email=test@example.com", "-c", "user.name=Test"])
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn applies_to_requires_flag_and_https() {
        // Default off: nothing routes to git2.
        assert!(!applies_to("https://github.com/a/b.git"));
        PILOT_ENABLED.store(true, Ordering::Relaxed);
        assert!(applies_to("https://github.com/a/b.git"));
        assert!(applies_to("HTTP://example.com/a/b.git"));
        assert!(!applies_to("git@github.com:a/b.git"));
        assert!(!applies_to("ssh://git@github.com/a/b.git"));
        assert!(!applies_to("/local/path"));
        PILOT_ENABLED.store(false, Ordering::Relaxed);
    }

    #[test]
    fn expired_github_app_error_reaches_git2_callbacks() {
        let error = callbacks_for_with("https://expired-git2.patchbay.test/owner/repo.git", |_| {
            anyhow::bail!("GITHUB_APP_REAUTH_REQUIRED: the Patchbay GitHub authorization expired")
        })
        .err()
        .expect("expired app authorization must reach the git2 caller");
        assert!(error.to_string().contains("GITHUB_APP_REAUTH_REQUIRED"));
    }

    #[test]
    fn push_fetch_ls_remote_roundtrip_against_local_remote() {
        let tmp = tempfile::tempdir().unwrap();
        let remote = tmp.path().join("remote.git");
        let work = tmp.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        assert!(Command::new("git")
            .args(["init", "--bare", "--initial-branch=main"])
            .arg(&remote)
            .output()
            .unwrap()
            .status
            .success());
        let url = file_url(&remote);

        git(&work, &["init", "-b", "main"]);
        git(&work, &["remote", "add", "origin", &url]);
        std::fs::write(work.join("a.txt"), "v1").unwrap();
        git(&work, &["add", "-A"]);
        git(&work, &["commit", "-m", "v1"]);
        git(&work, &["tag", "patchbay-v-20260101-000000-abc"]);

        // Push branch + tag through git2.
        push_refs(
            &work,
            &[
                "refs/heads/main:refs/heads/main".to_string(),
                "refs/tags/patchbay-v-20260101-000000-abc:refs/tags/patchbay-v-20260101-000000-abc"
                    .to_string(),
            ],
            &url,
        )
        .unwrap();

        // Remote now lists both refs.
        let refs = ls_remote_refs(&url).unwrap();
        assert!(refs.iter().any(|r| r == "refs/heads/main"), "{refs:?}");
        assert!(
            refs.iter()
                .any(|r| r == "refs/tags/patchbay-v-20260101-000000-abc"),
            "{refs:?}"
        );

        // git2 push updated the local remote-tracking ref (parity with
        // system git — ahead/behind and upstream health depend on it).
        let out = Command::new("git")
            .arg("-C")
            .arg(&work)
            .args(["rev-parse", "refs/remotes/origin/main"])
            .output()
            .unwrap();
        assert!(out.status.success(), "tracking ref missing after git2 push");

        // Fetch through git2 from a second clone after a new remote commit.
        let other = tmp.path().join("other");
        clone(&url, &other).unwrap();
        std::fs::write(other.join("b.txt"), "from other").unwrap();
        git(&other, &["add", "-A"]);
        git(&other, &["commit", "-m", "v2"]);
        push_refs(
            &other,
            &["refs/heads/main:refs/heads/main".to_string()],
            &url,
        )
        .unwrap();

        fetch(&work, Some("main"), &url).unwrap();
        let out = Command::new("git")
            .arg("-C")
            .arg(&work)
            .args(["rev-list", "--count", "main..origin/main"])
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            "1",
            "fetch should see the new remote commit"
        );
    }

    #[test]
    fn push_rejection_reports_non_fast_forward_vocabulary() {
        let tmp = tempfile::tempdir().unwrap();
        let remote = tmp.path().join("remote.git");
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        std::fs::create_dir_all(&a).unwrap();
        assert!(Command::new("git")
            .args(["init", "--bare", "--initial-branch=main"])
            .arg(&remote)
            .output()
            .unwrap()
            .status
            .success());
        let url = file_url(&remote);

        git(&a, &["init", "-b", "main"]);
        git(&a, &["remote", "add", "origin", &url]);
        std::fs::write(a.join("f.txt"), "base").unwrap();
        git(&a, &["add", "-A"]);
        git(&a, &["commit", "-m", "base"]);
        git(&a, &["push", "origin", "main"]);

        clone(&url, &b).unwrap();

        // Diverge: A pushes a new commit, B commits without pulling.
        std::fs::write(a.join("f.txt"), "from a").unwrap();
        git(&a, &["commit", "-am", "a2"]);
        git(&a, &["push", "origin", "main"]);
        std::fs::write(b.join("f.txt"), "from b").unwrap();
        git(&b, &["commit", "-am", "b2"]);

        let err =
            push_refs(&b, &["refs/heads/main:refs/heads/main".to_string()], &url).unwrap_err();
        let msg = format!("{err:#}").to_ascii_lowercase();
        assert!(
            msg.contains("non-fast-forward") || msg.contains("fast-forward"),
            "frontend error mapping relies on this vocabulary, got: {msg}"
        );
    }

    #[test]
    fn push_refs_to_url_targets_the_explicit_manifest_remote_not_origin() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("target.git");
        let decoy = tmp.path().join("decoy.git");
        let work = tmp.path().join("work");
        for bare in [&target, &decoy] {
            assert!(Command::new("git")
                .args(["init", "--bare", "--initial-branch=main"])
                .arg(bare)
                .output()
                .unwrap()
                .status
                .success());
        }
        std::fs::create_dir_all(&work).unwrap();
        git(&work, &["init", "-b", "main"]);
        git(&work, &["remote", "add", "origin", decoy.to_str().unwrap()]);
        std::fs::write(work.join("a.txt"), "planned").unwrap();
        git(&work, &["add", "-A"]);
        git(&work, &["commit", "-m", "planned"]);
        let oid = String::from_utf8(
            Command::new("git")
                .arg("-C")
                .arg(&work)
                .args(["rev-parse", "HEAD"])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();

        push_refs_to_url(
            &work,
            &[format!("{oid}:refs/heads/main")],
            &file_url(&target),
        )
        .unwrap();

        let target_head = Command::new("git")
            .arg("--git-dir")
            .arg(&target)
            .args(["rev-parse", "refs/heads/main"])
            .output()
            .unwrap();
        assert!(target_head.status.success());
        assert_eq!(String::from_utf8_lossy(&target_head.stdout).trim(), oid);
        assert_eq!(
            ls_remote_ref_oid(&file_url(&target), "refs/heads/main").unwrap(),
            Some(oid.clone())
        );
        assert!(!Command::new("git")
            .arg("--git-dir")
            .arg(&decoy)
            .args(["show-ref", "--verify", "refs/heads/main"])
            .output()
            .unwrap()
            .status
            .success());
    }

    #[test]
    fn fetch_ref_from_url_downloads_explicit_target_without_rewriting_origin() {
        let tmp = tempfile::tempdir().unwrap();
        let work = tmp.path().join("work");
        let hub = tmp.path().join("hub.git");
        let decoy = tmp.path().join("decoy.git");
        std::fs::create_dir_all(&work).unwrap();
        git(&work, &["init", "-b", "main"]);
        std::fs::write(work.join("a.txt"), "base").unwrap();
        git(&work, &["add", "-A"]);
        git(&work, &["commit", "-m", "base"]);
        for bare in [&hub, &decoy] {
            assert!(Command::new("git")
                .args(["clone", "--bare"])
                .arg(&work)
                .arg(bare)
                .output()
                .unwrap()
                .status
                .success());
        }
        git(&work, &["remote", "add", "origin", decoy.to_str().unwrap()]);
        let publisher = tmp.path().join("publisher");
        assert!(Command::new("git")
            .args(["clone"])
            .arg(&hub)
            .arg(&publisher)
            .output()
            .unwrap()
            .status
            .success());
        std::fs::write(publisher.join("a.txt"), "hub update").unwrap();
        git(&publisher, &["commit", "-am", "hub update"]);
        git(&publisher, &["push", "origin", "main"]);
        let target = Command::new("git")
            .arg("-C")
            .arg(&publisher)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        let target = String::from_utf8_lossy(&target.stdout).trim().to_string();

        fetch_ref_from_url(&work, &file_url(&hub), "main").unwrap();

        let repo = git2::Repository::open(&work).unwrap();
        assert!(repo
            .find_commit(git2::Oid::from_str(&target).unwrap())
            .is_ok());
        assert_eq!(
            repo.find_remote("origin").unwrap().url(),
            Some(decoy.to_str().unwrap())
        );
    }

    /// Fleet's HTTPS-hub bootstrap runs through this primitive, so it has to
    /// name the remote after the manifest hub, check out the manifest branch
    /// without consulting the hub's HEAD, and — like the system-git path's
    /// `--single-branch --no-tags` — leave only that one branch and no tags.
    #[test]
    fn clone_branch_with_remote_matches_the_single_branch_contract() {
        let tmp = tempfile::tempdir().unwrap();
        let work = tmp.path().join("work");
        let hub = tmp.path().join("hub.git");
        std::fs::create_dir_all(&work).unwrap();
        git(&work, &["init", "-b", "main"]);
        std::fs::write(work.join("a.txt"), "base").unwrap();
        git(&work, &["add", "-A"]);
        git(&work, &["commit", "-m", "base"]);
        git(&work, &["tag", "v1"]);
        git(&work, &["branch", "side"]);
        assert!(Command::new("git")
            .args(["clone", "--bare"])
            .arg(&work)
            .arg(&hub)
            .output()
            .unwrap()
            .status
            .success());
        // A hub HEAD pointing elsewhere: a HEAD-reliant clone would land on it.
        assert!(Command::new("git")
            .arg("--git-dir")
            .arg(&hub)
            .args(["symbolic-ref", "HEAD", "refs/heads/side"])
            .output()
            .unwrap()
            .status
            .success());

        let dest = tmp.path().join("bootstrapped");
        clone_branch_with_remote(&file_url(&hub), &dest, "alpha", "main").unwrap();

        let repo = git2::Repository::open(&dest).unwrap();
        assert_eq!(repo.head().unwrap().shorthand(), Some("main"));
        assert_eq!(
            repo.remotes().unwrap().iter().flatten().collect::<Vec<_>>(),
            vec!["alpha"],
            "the manifest hub must be the sole remote; origin stays free"
        );
        // Tags are deliberately not asserted: this test can only drive a
        // `file://` URL, and libgit2's local-clone path copies tags regardless
        // of `download_tags`. The option is still set, and it is what governs
        // the HTTPS transport this helper actually serves in production.
        let tracked: Vec<String> = repo
            .references_glob("refs/remotes/**")
            .unwrap()
            .flatten()
            .filter_map(|r| r.name().map(str::to_string))
            .collect();
        assert_eq!(
            tracked,
            vec!["refs/remotes/alpha/main".to_string()],
            "single-branch: `side` must not be tracked"
        );
    }
}
