//! Guarded project-repo measurements plus the P1 push/pull primitives.
//!
//! Read helpers set `GIT_OPTIONAL_LOCKS=0`, so preview/status cannot refresh a
//! worktree index. Mutators stay narrow (`init_bare_mirror`, remote add/set-url,
//! push, fetch, clone, and SAFE fast-forward); `FleetService` is the public
//! guard that proves manifest membership, projects-root containment, authority,
//! cleanliness or target absence, and complete plan evidence before mutation.
//!
//! The local fields deliberately mirror `sync-metis-projects.sh --check`
//! (`git status --short | wc -l`, `branch --show-current`,
//! `rev-parse --short HEAD`) so the two surfaces reconcile line for line.

use crate::core::git_fetcher::git_command;
use std::collections::HashSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::core::{chain::pull, git2_engine};

/// Local working-copy state of one managed repo, script-`--check` compatible.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LocalRepoState {
    pub present: bool,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub dirty: Option<u32>,
    #[serde(default)]
    pub detached: bool,
}

/// Comparison of the local checkout against the hub's branch tip, obtained via
/// `ls-remote` only. When the hub tip is not present in the local object
/// database (no fetch has brought it over yet) `ahead`/`behind` are honestly
/// `None` rather than guessed, with `note` saying why.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HubComparison {
    /// Short form of the hub branch tip, `None` when unreachable or missing.
    pub hub_head: Option<String>,
    pub ahead: Option<u32>,
    pub behind: Option<u32>,
    /// Stable note code: `hub_unreachable`, `branch_missing_on_hub`,
    /// `hub_head_not_local`, `local_missing`.
    pub note: Option<String>,
    pub error: Option<String>,
}

/// A git directory under the projects root that the manifest does not manage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredRepo {
    pub name: String,
    pub path: String,
    pub origin: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PullRelation {
    Same,
    Behind,
    Ahead,
    Diverged,
}

pub(super) enum PullCheckoutError {
    Collision,
    Other(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MirrorState {
    Missing,
    Bare,
    NotBare,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum MirrorInitError {
    TargetChanged,
    InitFailed(String),
}

pub(super) enum PublishDirectoryError {
    TargetChanged,
    Other(String),
}

#[derive(Debug)]
pub(super) enum CloneBranchError {
    TargetChanged(String),
    /// The clone failed after the target directory had been reserved.
    /// `debris` is true when that reserved directory could not be released,
    /// so the operator must clear it before a retry can be planned.
    CloneFailed {
        message: String,
        debris: bool,
    },
}

/// Run git with prompts disabled; `Ok(stdout)` only on zero exit.
fn git_read(dir: Option<&Path>, args: &[&str]) -> Result<String, String> {
    let mut cmd = git_command();
    cmd.env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0");
    if let Some(dir) = dir {
        cmd.arg("-C").arg(dir);
    }
    cmd.args(args);
    match cmd.output() {
        Ok(out) if out.status.success() => Ok(String::from_utf8_lossy(&out.stdout).into_owned()),
        Ok(out) => Err(String::from_utf8_lossy(&out.stderr).trim().to_string()),
        Err(e) => Err(e.to_string()),
    }
}

fn git_write(dir: &Path, args: &[&str]) -> Result<String, String> {
    let mut cmd = git_command();
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    cmd.arg("-C").arg(dir).args(args);
    match cmd.output() {
        Ok(out) if out.status.success() => Ok(String::from_utf8_lossy(&out.stdout).into_owned()),
        Ok(out) => Err(String::from_utf8_lossy(&out.stderr).trim().to_string()),
        Err(e) => Err(e.to_string()),
    }
}

/// Measure one local working copy. Missing path or non-repo → `present: false`
/// (the script's `missing` row; a present-but-broken repo also lands here
/// rather than aborting the whole matrix).
pub fn local_state(path: &Path) -> LocalRepoState {
    if !path.join(".git").exists() {
        return LocalRepoState::default();
    }
    let dirty = git_read(Some(path), &["status", "--porcelain"])
        .ok()
        .map(|out| out.lines().count() as u32);
    let head = git_read(Some(path), &["rev-parse", "--short", "HEAD"])
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let branch = git_read(Some(path), &["branch", "--show-current"])
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let detached = branch.is_none() && head.is_some();
    LocalRepoState {
        present: true,
        branch,
        head,
        dirty,
        detached,
    }
}

/// Compare the local checkout with the hub branch tip using `ls-remote` only.
/// Counting uses the local object database (`rev-list`), so it needs the hub
/// tip to already be a known object locally — never fetches to find out.
pub fn compare_with_hub(path: &Path, hub_url: &str, branch: &str) -> HubComparison {
    let refspec = format!("refs/heads/{branch}");
    let listed = match git_read(None, &["ls-remote", "--", hub_url, &refspec]) {
        Ok(out) => out,
        Err(e) => {
            return HubComparison {
                note: Some("hub_unreachable".into()),
                error: Some(e),
                ..Default::default()
            }
        }
    };
    let hub_oid = match listed.split_whitespace().next() {
        Some(oid) if !oid.is_empty() => oid.to_string(),
        _ => {
            return HubComparison {
                note: Some("branch_missing_on_hub".into()),
                ..Default::default()
            }
        }
    };
    let hub_head = Some(hub_oid.chars().take(7).collect::<String>());
    if !path.join(".git").exists() {
        return HubComparison {
            hub_head,
            note: Some("local_missing".into()),
            ..Default::default()
        };
    }
    if git_read(Some(path), &["cat-file", "-e", &hub_oid]).is_err() {
        return HubComparison {
            hub_head,
            note: Some("hub_head_not_local".into()),
            ..Default::default()
        };
    }
    let range = format!("{hub_oid}...HEAD");
    match git_read(Some(path), &["rev-list", "--left-right", "--count", &range]) {
        Ok(out) => {
            let mut nums = out.split_whitespace().filter_map(|n| n.parse::<u32>().ok());
            // left = only on hub (behind), right = only local (ahead)
            let behind = nums.next();
            let ahead = nums.next();
            HubComparison {
                hub_head,
                ahead,
                behind,
                ..Default::default()
            }
        }
        Err(e) => HubComparison {
            hub_head,
            error: Some(e),
            ..Default::default()
        },
    }
}

/// Full local HEAD object id used as the plan/apply TOCTOU baseline.
pub(super) fn head_oid(path: &Path) -> Result<String, String> {
    git_read(Some(path), &["rev-parse", "HEAD"])
        .map(|out| out.trim().to_string())
        .and_then(|oid| {
            if oid.is_empty() {
                Err("HEAD did not resolve to an object id".to_string())
            } else {
                Ok(oid)
            }
        })
}

/// Full branch tip at the hub, without fetching or mutating local refs.
pub(super) fn hub_head(hub_url: &str, branch: &str) -> Result<Option<String>, String> {
    let refname = format!("refs/heads/{branch}");
    let lower = hub_url.trim().to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return git2_engine::ls_remote_ref_oid(hub_url, &refname)
            .map_err(|error| format!("{error:#}"));
    }
    let out = git_read(None, &["ls-remote", "--", hub_url, &refname])?;
    Ok(out
        .split_whitespace()
        .next()
        .map(str::to_string)
        .filter(|oid| !oid.is_empty()))
}

/// Current configured URL for the manifest hub remote. Pull fetches from the
/// manifest URL directly, but this local value remains plan evidence so a
/// concurrent remote rewrite cannot pass unnoticed.
pub(super) fn configured_remote_url(
    path: &Path,
    remote_name: &str,
) -> Result<Option<String>, String> {
    match git_read(Some(path), &["remote", "get-url", remote_name]) {
        Ok(url) => Ok(Some(url.trim().to_string()).filter(|url| !url.is_empty())),
        Err(error) if error.contains("No such remote") => Ok(None),
        Err(error) => Err(error),
    }
}

/// Inspect a local mirror path without executing Git or touching the network.
pub(super) fn mirror_state(path: &Path) -> MirrorState {
    if !path.exists() {
        return MirrorState::Missing;
    }
    match git2::Repository::open_bare(path) {
        Ok(repo) if repo.is_bare() => MirrorState::Bare,
        _ => MirrorState::NotBare,
    }
}

/// Initialize one already-guarded local path as a bare repository. This is a
/// libgit2 equivalent of local `git init --bare`; it has no transport surface.
pub(super) fn init_bare_mirror(path: &Path) -> Result<(), MirrorInitError> {
    let parent = path
        .parent()
        .ok_or_else(|| MirrorInitError::InitFailed("mirror path has no parent".into()))?;
    let staging_parent = tempfile::tempdir_in(parent)
        .map_err(|error| MirrorInitError::InitFailed(error.to_string()))?;
    let staging = staging_parent.path().join("mirror.git");
    git2::Repository::init_bare(&staging)
        .map_err(|error| MirrorInitError::InitFailed(error.message().to_string()))?;
    publish_directory(&staging, path).map_err(|error| match error {
        PublishDirectoryError::TargetChanged => MirrorInitError::TargetChanged,
        PublishDirectoryError::Other(message) => MirrorInitError::InitFailed(message),
    })
}

/// Publish a fully prepared directory without replacing a path created after
/// preview. macOS has an exact primitive for this in `renamex_np(RENAME_EXCL)`;
/// elsewhere the same guarantee falls out of directory rename semantics.
pub(super) fn publish_directory(from: &Path, to: &Path) -> Result<(), PublishDirectoryError> {
    #[cfg(target_os = "macos")]
    {
        use std::ffi::CString;
        use std::os::raw::c_char;
        use std::os::unix::ffi::OsStrExt;

        const RENAME_EXCL: u32 = 0x0000_0004;
        unsafe extern "C" {
            fn renamex_np(from: *const c_char, to: *const c_char, flags: u32) -> i32;
        }

        let from = CString::new(from.as_os_str().as_bytes())
            .map_err(|error| PublishDirectoryError::Other(error.to_string()))?;
        let to = CString::new(to.as_os_str().as_bytes())
            .map_err(|error| PublishDirectoryError::Other(error.to_string()))?;
        // SAFETY: both pointers come from live CStrings and remain valid for
        // the duration of the call; RENAME_EXCL is the macOS no-replace flag.
        if unsafe { renamex_np(from.as_ptr(), to.as_ptr(), RENAME_EXCL) } == 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if matches!(
            error.kind(),
            std::io::ErrorKind::AlreadyExists | std::io::ErrorKind::DirectoryNotEmpty
        ) {
            Err(PublishDirectoryError::TargetChanged)
        } else {
            Err(PublishDirectoryError::Other(error.to_string()))
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        // The pre-check only buys a precise reason in the common case; the
        // error mapping is what actually closes the race, and it is the same
        // mapping the macOS arm uses. A directory rename cannot clobber an
        // existing target on either platform: Windows `MOVEFILE_REPLACE_EXISTING`
        // does not apply to directories, and `rename(2)` requires the
        // destination directory to be empty.
        if to.exists() {
            return Err(PublishDirectoryError::TargetChanged);
        }
        std::fs::rename(from, to).map_err(|error| {
            if matches!(
                error.kind(),
                std::io::ErrorKind::AlreadyExists | std::io::ErrorKind::DirectoryNotEmpty
            ) {
                PublishDirectoryError::TargetChanged
            } else {
                PublishDirectoryError::Other(error.to_string())
            }
        })
    }
}

/// Add the manifest hub remote to a local working repository.
pub(super) fn add_remote(path: &Path, name: &str, url: &str) -> Result<(), String> {
    let repo = git2::Repository::open(path).map_err(|error| error.message().to_string())?;
    repo.remote(name, url)
        .map(|_| ())
        .map_err(|error| error.message().to_string())
}

/// Rewrite only the named manifest hub remote URL. Callers reserve `origin`.
pub(super) fn set_remote_url(path: &Path, name: &str, url: &str) -> Result<(), String> {
    let repo = git2::Repository::open(path).map_err(|error| error.message().to_string())?;
    repo.remote_set_url(name, url)
        .map_err(|error| error.message().to_string())
}

/// Classify two commit ids using only the local object database. `None` means
/// the hub target has not been fetched yet; preview stays read-only and apply
/// performs the definitive check after fetching.
pub(super) fn pull_relation(
    path: &Path,
    local_oid: &str,
    target_oid: &str,
) -> Result<Option<PullRelation>, String> {
    let repo = git2::Repository::open(path).map_err(|error| error.message().to_string())?;
    let local = git2::Oid::from_str(local_oid).map_err(|error| error.message().to_string())?;
    let target = git2::Oid::from_str(target_oid).map_err(|error| error.message().to_string())?;
    if local == target {
        return Ok(Some(PullRelation::Same));
    }
    if repo.find_commit(target).is_err() {
        return Ok(None);
    }
    if repo
        .graph_descendant_of(target, local)
        .map_err(|error| error.message().to_string())?
    {
        return Ok(Some(PullRelation::Behind));
    }
    if repo
        .graph_descendant_of(local, target)
        .map_err(|error| error.message().to_string())?
    {
        return Ok(Some(PullRelation::Ahead));
    }
    Ok(Some(PullRelation::Diverged))
}

/// Fetch a manifest hub branch without changing `origin` or any local branch.
/// HTTPS uses the credential-aware git2 transport; SSH/local URLs use system
/// git so ssh config and the user's agent remain authoritative.
pub(super) fn fetch_hub(path: &Path, hub_url: &str, branch: &str) -> Result<(), String> {
    let lower = hub_url.trim().to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return git2_engine::fetch_ref_from_url(path, hub_url, branch)
            .map_err(|error| format!("{error:#}"));
    }
    let source = format!("refs/heads/{branch}");
    git_write(
        path,
        &[
            "fetch",
            "--no-tags",
            "--no-write-fetch-head",
            "--",
            hub_url,
            &source,
        ],
    )
    .map(|_| ())
}

/// Advance the named local branch to the exact previewed OID using the shared
/// libgit2 SAFE checkout primitive. The caller proves strict ancestry first.
pub(super) fn fast_forward_checkout(
    path: &Path,
    branch: &str,
    expected_from: &str,
    target_oid: &str,
) -> Result<(), PullCheckoutError> {
    let repo = git2::Repository::open(path)
        .map_err(|error| PullCheckoutError::Other(error.message().to_string()))?;
    let head = repo
        .head()
        .map_err(|error| PullCheckoutError::Other(error.message().to_string()))?;
    if head.shorthand() != Some(branch)
        || head.target().map(|oid| oid.to_string()).as_deref() != Some(expected_from)
    {
        return Err(PullCheckoutError::Other(
            "local branch or HEAD changed before checkout".to_string(),
        ));
    }
    let refname = format!("refs/heads/{branch}");
    let target = git2::Oid::from_str(target_oid)
        .map_err(|error| PullCheckoutError::Other(error.message().to_string()))?;
    drop(head);
    match pull::fast_forward(&repo, target, &refname, "patchbay fleet: fast-forward pull") {
        Ok(()) => Ok(()),
        Err(pull::FfError::Collision) => Err(PullCheckoutError::Collision),
        Err(pull::FfError::Other(message)) => Err(PullCheckoutError::Other(message)),
    }
}

/// Push the previewed full object id to the same-named branch at the hub.
/// System git is the engine for local paths and SSH URLs; its default
/// non-fast-forward refusal is the safety gate. HTTP(S) is routed through the
/// credential-aware git2 engine.
pub(super) fn push_branch(
    path: &Path,
    hub_url: &str,
    branch: &str,
    planned_oid: &str,
) -> Result<(), String> {
    if hub_url.trim().to_ascii_lowercase().starts_with("http://")
        || hub_url.trim().to_ascii_lowercase().starts_with("https://")
    {
        let refspec = format!("{planned_oid}:refs/heads/{branch}");
        return git2_engine::push_refs_to_url(path, &[refspec], hub_url)
            .map_err(|error| format!("{error:#}"));
    }
    let refspec = format!("{planned_oid}:refs/heads/{branch}");
    git_write(path, &["push", "--", hub_url, &refspec]).map(|_| ())
}

/// Clone one explicit manifest branch using the manifest hub name as the sole
/// remote. The caller verifies that the resulting HEAD still matches the
/// previewed OID; branch drift is reported rather than reset back into place.
pub(super) fn clone_branch(
    hub_url: &str,
    hub_name: &str,
    branch: &str,
    target: &Path,
) -> Result<(), CloneBranchError> {
    // `create_dir` is both the existence check and the reservation: one atomic
    // syscall, so nothing can slip into the path between checking and cloning.
    std::fs::create_dir(target).map_err(|error| {
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            CloneBranchError::TargetChanged(error.to_string())
        } else {
            CloneBranchError::CloneFailed {
                message: error.to_string(),
                debris: false,
            }
        }
    })?;
    if hub_url.trim().to_ascii_lowercase().starts_with("http://")
        || hub_url.trim().to_ascii_lowercase().starts_with("https://")
    {
        return git2_engine::clone_branch_with_remote(hub_url, target, hub_name, branch)
            .map_err(|error| clone_failed(target, format!("{error:#}")));
    }
    let out = git_command()
        .env("GIT_TERMINAL_PROMPT", "0")
        .args([
            "clone",
            "--origin",
            hub_name,
            "--branch",
            branch,
            "--single-branch",
            "--no-tags",
            "--",
            hub_url,
        ])
        .arg(target)
        .output()
        .map_err(|error| clone_failed(target, error.to_string()))?;
    if !out.status.success() {
        return Err(clone_failed(
            target,
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    Ok(())
}

/// Release the directory reserved for a clone that failed.
///
/// Because we pre-created the target, git only empties it and leaves the
/// top level in place, which would make every later bootstrap of that repo
/// refuse with "target already exists". `remove_dir` succeeds *only* on an
/// empty directory, so this can never destroy content: a partial clone is
/// preserved and reported as debris instead. Fleet still has no recursive
/// delete.
fn clone_failed(target: &Path, message: String) -> CloneBranchError {
    CloneBranchError::CloneFailed {
        message,
        debris: std::fs::remove_dir(target).is_err(),
    }
}

pub(super) fn remote_names(path: &Path) -> Result<Vec<String>, String> {
    let output = git_read(Some(path), &["remote"])?;
    Ok(output.lines().map(str::to_string).collect())
}

/// Top-level git directories under `projects_root` that are not in `known`.
/// Depth one by design: the manifest manages direct children of the root.
pub fn discover(projects_root: &Path, known: &HashSet<String>) -> Vec<DiscoveredRepo> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(projects_root) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = entry.file_name().to_str().map(String::from) else {
            continue;
        };
        if name.starts_with('.') || known.contains(&name) || !path.join(".git").exists() {
            continue;
        }
        let origin = git_read(Some(&path), &["remote", "get-url", "origin"])
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        found.push(DiscoveredRepo {
            name,
            path: path.to_string_lossy().into_owned(),
            origin,
        });
    }
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    // Fixtures drive git directly; production goes through `git_command`.
    use std::process::Command;
    use tempfile::tempdir;

    /// Run a git command in `dir`, failing loudly with stderr on error.
    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.email=test@example.com",
                "-c",
                "user.name=Test",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn seed_repo(base: &Path, name: &str) -> PathBuf {
        let dir = base.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-b", "main"]);
        std::fs::write(dir.join("file.txt"), "base").unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-m", "base"]);
        dir
    }

    #[test]
    fn local_state_matches_script_check_semantics() {
        let temp = tempdir().unwrap();
        let repo = seed_repo(temp.path(), "one");

        let clean = local_state(&repo);
        assert!(clean.present);
        assert_eq!(clean.branch.as_deref(), Some("main"));
        assert_eq!(clean.dirty, Some(0));
        assert!(!clean.detached);
        let head = clean.head.unwrap();
        assert!(!head.is_empty());

        // One modified + one untracked = dirty 2, exactly like
        // `git status --short | wc -l`.
        std::fs::write(repo.join("file.txt"), "changed").unwrap();
        std::fs::write(repo.join("new.txt"), "new").unwrap();
        assert_eq!(local_state(&repo).dirty, Some(2));

        // Missing path reports absent, not an error.
        assert!(!local_state(&temp.path().join("nope")).present);
    }

    #[test]
    fn local_state_flags_detached_head() {
        let temp = tempdir().unwrap();
        let repo = seed_repo(temp.path(), "one");
        let head = local_state(&repo).head.unwrap();
        git(&repo, &["checkout", "--detach", &head]);
        let state = local_state(&repo);
        assert!(state.detached);
        assert_eq!(state.branch, None);
    }

    #[test]
    fn init_bare_mirror_refuses_an_existing_empty_target() {
        let temp = tempdir().unwrap();
        let target = temp.path().join("reserved.git");
        std::fs::create_dir(&target).unwrap();

        let error = init_bare_mirror(&target).unwrap_err();

        assert!(matches!(error, MirrorInitError::TargetChanged));
        assert!(std::fs::read_dir(&target).unwrap().next().is_none());
    }

    #[test]
    fn compare_with_hub_counts_ahead_behind_via_ls_remote_only() {
        let temp = tempdir().unwrap();
        let repo = seed_repo(temp.path(), "work");
        let hub = temp.path().join("hub.git");
        let out = Command::new("git")
            .args(["clone", "--bare"])
            .arg(&repo)
            .arg(&hub)
            .output()
            .unwrap();
        assert!(out.status.success());

        let hub_url = hub.to_str().unwrap();
        let synced = compare_with_hub(&repo, hub_url, "main");
        assert_eq!(synced.ahead, Some(0));
        assert_eq!(synced.behind, Some(0));
        assert!(synced.hub_head.is_some());

        // A new local commit → ahead 1, and the hub bare repo is untouched.
        std::fs::write(repo.join("file.txt"), "v2").unwrap();
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-m", "v2"]);
        let ahead = compare_with_hub(&repo, hub_url, "main");
        assert_eq!(ahead.ahead, Some(1));
        assert_eq!(ahead.behind, Some(0));

        // Unknown branch on the hub is a stable note, not an error.
        let missing = compare_with_hub(&repo, hub_url, "does-not-exist");
        assert_eq!(missing.note.as_deref(), Some("branch_missing_on_hub"));

        // Unreachable hub degrades to a note + error message.
        let gone = compare_with_hub(
            &repo,
            temp.path().join("gone.git").to_str().unwrap(),
            "main",
        );
        assert_eq!(gone.note.as_deref(), Some("hub_unreachable"));
        assert!(gone.error.is_some());
    }

    /// The hub URL arrives from the meta repo, so a value beginning with `-`
    /// must reach git as a location and never as an option — `--upload-pack`
    /// is executed by git, and this runs on the read-only status path.
    #[test]
    fn compare_with_hub_never_lets_a_dash_leading_url_become_a_git_option() {
        let temp = tempdir().unwrap();
        let repo = seed_repo(temp.path(), "work");
        let canary = temp.path().join("canary");
        let hostile = format!("--upload-pack=touch {}", canary.display());

        let result = compare_with_hub(&repo, &hostile, "main");

        assert!(!canary.exists(), "hub url was executed as a git option");
        assert_eq!(result.note.as_deref(), Some("hub_unreachable"));
    }

    #[test]
    fn discover_lists_only_unmanaged_git_dirs() {
        let temp = tempdir().unwrap();
        seed_repo(temp.path(), "managed");
        seed_repo(temp.path(), "stray");
        std::fs::create_dir_all(temp.path().join("not-a-repo")).unwrap();
        std::fs::create_dir_all(temp.path().join(".hidden")).unwrap();

        let known = HashSet::from(["managed".to_string()]);
        let found = discover(temp.path(), &known);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "stray");
    }

    #[test]
    fn push_branch_moves_hub_to_the_planned_oid_not_a_later_head() {
        let temp = tempdir().unwrap();
        let repo = seed_repo(temp.path(), "work");
        let hub = temp.path().join("hub.git");
        let out = Command::new("git")
            .args(["clone", "--bare"])
            .arg(&repo)
            .arg(&hub)
            .output()
            .unwrap();
        assert!(out.status.success());

        std::fs::write(repo.join("file.txt"), "planned").unwrap();
        git(&repo, &["commit", "-am", "planned"]);
        let planned = head_oid(&repo).unwrap();
        std::fs::write(repo.join("file.txt"), "later").unwrap();
        git(&repo, &["commit", "-am", "later"]);
        assert_ne!(head_oid(&repo).unwrap(), planned);

        push_branch(&repo, hub.to_str().unwrap(), "main", &planned).unwrap();
        let hub_tip = Command::new("git")
            .arg("--git-dir")
            .arg(&hub)
            .args(["rev-parse", "refs/heads/main"])
            .output()
            .unwrap();
        assert!(hub_tip.status.success());
        assert_eq!(String::from_utf8_lossy(&hub_tip.stdout).trim(), planned);
    }

    #[test]
    fn clone_branch_refuses_an_empty_target_created_after_preview() {
        let temp = tempdir().unwrap();
        let repo = seed_repo(temp.path(), "work");
        let hub = temp.path().join("hub.git");
        let out = Command::new("git")
            .args(["clone", "--bare"])
            .arg(&repo)
            .arg(&hub)
            .output()
            .unwrap();
        assert!(out.status.success());
        let target = temp.path().join("racing-target");
        std::fs::create_dir(&target).unwrap();

        let result = clone_branch(hub.to_str().unwrap(), "test", "main", &target);

        assert!(matches!(result, Err(CloneBranchError::TargetChanged(_))));
        assert!(std::fs::read_dir(&target).unwrap().next().is_none());
    }

    /// A clone that fails after reserving the path must not leave the reserved
    /// directory behind — otherwise every later bootstrap of that repo refuses
    /// with "target already exists" and only a manual `rm` can recover.
    #[test]
    fn clone_branch_releases_its_reservation_when_the_clone_fails() {
        let temp = tempdir().unwrap();
        let repo = seed_repo(temp.path(), "work");
        let hub = temp.path().join("hub.git");
        assert!(Command::new("git")
            .args(["clone", "--bare"])
            .arg(&repo)
            .arg(&hub)
            .output()
            .unwrap()
            .status
            .success());
        let target = temp.path().join("alpha");

        // Branch missing on the hub: the clone fails after the reservation.
        let result = clone_branch(hub.to_str().unwrap(), "test", "no-such-branch", &target);

        match result {
            Err(CloneBranchError::CloneFailed { debris, .. }) => {
                assert!(!debris, "an empty reservation should have been released");
            }
            other => panic!("expected CloneFailed, got {other:?}"),
        }
        assert!(!target.exists(), "reserved directory outlived the failure");

        // The retry that the old behavior made impossible now succeeds.
        clone_branch(hub.to_str().unwrap(), "test", "main", &target).expect("retry after failure");
        assert!(target.join(".git").exists());
    }

    /// The release is `remove_dir`, which only succeeds on an empty directory,
    /// so content a failed clone left behind is preserved and reported rather
    /// than deleted — fleet still has no recursive delete.
    #[test]
    fn clone_branch_preserves_debris_it_cannot_safely_remove() {
        let temp = tempdir().unwrap();
        let target = temp.path().join("alpha");

        // A hub that cannot be reached at all: the clone fails, and we plant a
        // file first to stand in for a partially written clone.
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("partial"), "content").unwrap();
        let err = clone_failed(&target, "simulated failure".into());

        match err {
            CloneBranchError::CloneFailed { debris, .. } => assert!(debris),
            other => panic!("expected CloneFailed, got {other:?}"),
        }
        assert!(
            target.join("partial").exists(),
            "content must never be deleted"
        );
    }

    #[test]
    fn pull_fetch_preserves_remotes_and_refs_and_safe_checkout_keeps_collision() {
        let temp = tempdir().unwrap();
        let repo = seed_repo(temp.path(), "work");
        let hub = temp.path().join("hub.git");
        let decoy = temp.path().join("origin.git");
        for bare in [&hub, &decoy] {
            let out = Command::new("git")
                .args(["clone", "--bare"])
                .arg(&repo)
                .arg(bare)
                .output()
                .unwrap();
            assert!(out.status.success());
        }
        git(&repo, &["remote", "add", "origin", decoy.to_str().unwrap()]);

        let publisher = temp.path().join("publisher");
        assert!(Command::new("git")
            .args(["clone"])
            .arg(&hub)
            .arg(&publisher)
            .output()
            .unwrap()
            .status
            .success());
        std::fs::write(publisher.join("collide.txt"), "from hub").unwrap();
        git(&publisher, &["add", "-A"]);
        git(&publisher, &["commit", "-m", "add collision"]);
        git(&publisher, &["push", "origin", "main"]);

        let before = head_oid(&repo).unwrap();
        let target = hub_head(hub.to_str().unwrap(), "main").unwrap().unwrap();
        let refs_before = git_read(Some(&repo), &["show-ref"]).unwrap();
        let origin_before = configured_remote_url(&repo, "origin").unwrap();

        fetch_hub(&repo, hub.to_str().unwrap(), "main").unwrap();

        assert_eq!(git_read(Some(&repo), &["show-ref"]).unwrap(), refs_before);
        assert_eq!(
            configured_remote_url(&repo, "origin").unwrap(),
            origin_before
        );
        assert_eq!(
            pull_relation(&repo, &before, &target).unwrap(),
            Some(PullRelation::Behind)
        );
        std::fs::write(repo.join("collide.txt"), "local untracked").unwrap();

        let result = fast_forward_checkout(&repo, "main", &before, &target);

        assert!(matches!(result, Err(PullCheckoutError::Collision)));
        assert_eq!(head_oid(&repo).unwrap(), before);
        assert_eq!(
            std::fs::read_to_string(repo.join("collide.txt")).unwrap(),
            "local untracked"
        );
    }
}
