//! Fast-forward-only pull for clean Original Repositories (tier 1).
//!
//! This is the one place in the chain module that mutates a checkout's Git
//! history, so it is deliberately narrow and defensive:
//!
//! * **Preview never touches the network or the repository.** [`preview`]
//!   classifies each repository from its *current* refs (reusing the same
//!   tracking logic as [`repo_health`]) and decides, per repo, whether a
//!   fast-forward is eligible or must be skipped with a stable reason code.
//! * **Apply only ever fast-forwards.** [`apply`] fetches `origin`, re-checks
//!   the working tree, and moves the branch forward *only* when the upstream is
//!   a strict descendant of `HEAD`. It never resets, stashes, merges,
//!   rebases, force-updates, or resolves conflicts — every state that could
//!   risk local work or rewrite history becomes an explicit skip or error.
//! * **The working-tree update is a SAFE checkout.** A libgit2 SAFE checkout
//!   refuses to clobber an untracked or locally-modified file that collides
//!   with the incoming tree; such a collision aborts the checkout before
//!   anything is written and is reported as `untracked_collision` with the
//!   branch left exactly where it was.
//!
//! Credentials for the `origin` fetch are injected in-memory from the OS
//! keychain, mirroring the existing git2 network engine.

use serde::{Deserialize, Serialize};
use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use super::repo_health;
use crate::core::git_credentials;

/// One repository's read-only fast-forward classification, produced by
/// [`preview`] and consumed unchanged by [`apply`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullPreview {
    /// Absolute path of the Original Repository checkout.
    pub path: String,
    /// Directory name of the checkout (display identity).
    pub name: String,
    /// Current local branch, when `HEAD` is on one.
    pub branch: Option<String>,
    /// Upstream tracking branch shorthand, e.g. `"origin/main"`.
    pub upstream: Option<String>,
    /// Commits the local branch has that its upstream does not.
    pub ahead: usize,
    /// Commits the upstream has that the local branch does not.
    pub behind: usize,
    /// Tracked-file dirtiness (`git status -uno`): untracked files never mark a
    /// repo dirty, so they do not block a fast-forward on their own.
    pub dirty: bool,
    /// `"fast_forward"` when a clean fast-forward is eligible, `"skip"`
    /// otherwise.
    pub action: String,
    /// Stable reason code when `action == "skip"`: `"dirty" | "diverged" |
    /// "ahead" | "up_to_date" | "no_upstream" | "detached" | "scan_error"`.
    pub reason: Option<String>,
}

/// A previewed, guarded pull. Produced by [`preview`], carried across the wire,
/// and handed back to [`apply`] so the update acts only on what the user saw.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullPlan {
    pub items: Vec<PullPreview>,
    /// When the classification was taken (epoch millis).
    pub scanned_at: i64,
}

/// The outcome of attempting one repository's update.
#[derive(Debug, Clone, Serialize)]
pub struct PullResult {
    pub path: String,
    pub name: String,
    /// `"updated" | "skipped" | "up_to_date" | "error"`.
    pub action: String,
    /// Short `HEAD` sha before the attempt, when known.
    pub from: Option<String>,
    /// Short `HEAD` sha after the attempt, when known.
    pub to: Option<String>,
    /// Stable code for skips (`"dirty"`, `"diverged"`, `"untracked_collision"`,
    /// …) and errors (`"auth"`, `"network"`, `"fetch"`, `"checkout"`,
    /// `"scan_error"`).
    pub reason: Option<String>,
    /// Free-form detail, populated for errors (the underlying git message).
    pub message: Option<String>,
}

/// The full apply outcome plus a fresh timestamp from the post-apply rescan.
#[derive(Debug, Clone, Serialize)]
pub struct PullOutcome {
    pub results: Vec<PullResult>,
    /// When the confirming rescan was taken (epoch millis).
    pub scanned_at: i64,
}

/// Classify each repository's fast-forward eligibility from its *current* state.
///
/// Read-only: nothing here fetches, and nothing mutates the repository. A
/// repository is eligible (`action == "fast_forward"`) only when it is clean
/// and strictly behind its upstream; every other state is a `skip` carrying the
/// precise reason so the UI can explain why an update was withheld.
pub fn preview(repo_paths: &[PathBuf]) -> PullPlan {
    PullPlan {
        items: repo_paths.iter().map(|path| classify(path)).collect(),
        scanned_at: now_millis(),
    }
}

/// Attempt each previewed repository update, fast-forwarding *only* the eligible
/// ones and reflecting every skip verbatim.
///
/// For a `"fast_forward"` item the sequence is: re-open the repository, re-check
/// working-tree cleanliness (a TOCTOU guard against a checkout that turned dirty
/// since preview), fetch `origin`, recompute the upstream target, and — only
/// when the upstream strictly descends from `HEAD` — perform a SAFE checkout and
/// advance the branch. Any deviation (auth/network failure, a checkout that is
/// no longer a clean fast-forward, or an untracked/local collision) becomes an
/// explicit `error`/`skipped` result and leaves the repository untouched.
pub fn apply(plan: &PullPlan) -> Vec<PullResult> {
    plan.items.iter().map(apply_one).collect()
}

/// Read-only per-repository classification shared by [`preview`].
fn classify(path: &Path) -> PullPreview {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let status = repo_health::inspect(path);
    let health = status.health;
    // Order is a priority ladder: a repo that cannot be read, or whose tracked
    // working tree is dirty, is refused before its tracking position is even
    // considered — protecting local work always wins.
    let (action, reason): (&str, Option<&str>) = if health.state == "scan_error" {
        ("skip", Some("scan_error"))
    } else if health.dirty {
        ("skip", Some("dirty"))
    } else {
        match health.state.as_str() {
            "detached" => ("skip", Some("detached")),
            "no_upstream" => ("skip", Some("no_upstream")),
            "diverged" => ("skip", Some("diverged")),
            "ahead" => ("skip", Some("ahead")),
            "up_to_date" => ("skip", Some("up_to_date")),
            // Clean and strictly behind: the only fast-forward-eligible state.
            "behind" => ("fast_forward", None),
            // Defensive: an unexpected state is skipped, never fast-forwarded.
            _ => ("skip", Some("scan_error")),
        }
    };
    PullPreview {
        path: path.to_string_lossy().to_string(),
        name,
        branch: health.branch.clone(),
        upstream: upstream_shorthand(path),
        ahead: health.ahead,
        behind: health.behind,
        dirty: health.dirty,
        action: action.to_string(),
        reason: reason.map(str::to_string),
    }
}

/// Attempt one repository's update. Never panics; every git2 failure collapses
/// to a structured skip/error result.
fn apply_one(item: &PullPreview) -> PullResult {
    // Non-eligible previews are reflected verbatim as protected refusals — apply
    // acts only on what preview marked fast-forward.
    if item.action != "fast_forward" {
        return skip_result(
            item,
            item.reason.clone().unwrap_or_else(|| "skip".to_string()),
        );
    }

    let path = PathBuf::from(&item.path);
    let repo = match git2::Repository::open(&path) {
        Ok(repo) => repo,
        Err(e) => return error_result(item, "scan_error", e.message()),
    };

    // TOCTOU guard: the working tree may have become dirty since preview.
    if is_dirty(&repo) {
        return skip_result(item, "dirty".to_string());
    }

    let head = match repo.head() {
        Ok(head) => head,
        Err(e) => return error_result(item, "scan_error", e.message()),
    };
    let Some(local_oid) = head.target() else {
        return skip_result(item, "detached".to_string());
    };
    let Some(branch) = head.shorthand().map(str::to_string) else {
        return skip_result(item, "detached".to_string());
    };
    let Some(refname) = head.name().map(str::to_string) else {
        return error_result(item, "scan_error", "HEAD has no reference name");
    };
    let before = short(local_oid);
    drop(head);

    // The only network step. A fetch failure is classified and returned; the
    // repository is untouched.
    if let Err(failure) = fetch_origin(&repo, &branch) {
        return error_result(item, failure.reason, &failure.message);
    }

    // Recompute the upstream target from the freshly-fetched tracking ref.
    let Some(upstream_oid) = upstream_oid(&repo, &branch) else {
        return skip_result(item, "no_upstream".to_string());
    };
    let (ahead, behind) = match repo.graph_ahead_behind(local_oid, upstream_oid) {
        Ok(counts) => counts,
        Err(e) => return error_result(item, "scan_error", e.message()),
    };
    if behind == 0 {
        // Nothing to fast-forward. Either already current, or (TOCTOU) the local
        // branch moved ahead since preview — never merged, never forced.
        return if ahead == 0 {
            up_to_date_result(item, &before)
        } else {
            skip_result(item, "ahead".to_string())
        };
    }
    if ahead > 0 {
        // Diverged since preview: refuse rather than merge or rewrite history.
        return skip_result(item, "diverged".to_string());
    }
    // A true fast-forward requires the upstream to strictly descend from HEAD.
    if !repo
        .graph_descendant_of(upstream_oid, local_oid)
        .unwrap_or(false)
    {
        return skip_result(item, "diverged".to_string());
    }

    match fast_forward(&repo, upstream_oid, &refname, "patchbay: fast-forward pull") {
        Ok(()) => updated_result(item, &before, &short(upstream_oid)),
        // A SAFE checkout refused to overwrite colliding local state.
        Err(FfError::Collision) => skip_result(item, "untracked_collision".to_string()),
        Err(FfError::Other(message)) => error_result(item, "checkout", &message),
    }
}

/// Why a fast-forward could not be completed after eligibility was confirmed.
///
/// Exposed to sibling chain/fleet modules so they can reuse the same SAFE
/// fast-forward primitive for fork sync and manifest-hub pulls.
pub(crate) enum FfError {
    /// A SAFE checkout would have clobbered an untracked or modified local file.
    Collision,
    /// Any other git2 failure while updating the working tree or refs.
    Other(String),
}

/// Advance a clean, strictly-behind branch to `target` by fast-forward only.
///
/// Before touching the working tree we refuse any incoming path that would
/// overwrite an untracked working-tree file: libgit2's SAFE checkout, unlike
/// git's own fast-forward, will happily clobber an untracked collision, so we
/// guard against it explicitly (see [`has_untracked_collision`]). The working
/// tree is then updated with a **SAFE** checkout (never `force`), backed by a
/// conflict-abort callback as defense in depth. Only once the tree is in place
/// is the branch ref advanced and `HEAD` re-pointed — moving the ref last means
/// a refused checkout leaves both the tree and history untouched.
///
/// `reflog` is the reflog message stamped on the advanced branch ref, letting
/// callers record which flow moved the branch. Exposed for fork-sync and fleet
/// pull, both of which reuse this exact primitive.
pub(crate) fn fast_forward(
    repo: &git2::Repository,
    target: git2::Oid,
    refname: &str,
    reflog: &str,
) -> Result<(), FfError> {
    // Protect untracked local work the incoming tree would otherwise overwrite.
    match has_untracked_collision(repo, target) {
        Ok(true) => return Err(FfError::Collision),
        Ok(false) => {}
        Err(e) => return Err(FfError::Other(e.message().to_string())),
    }

    let object = repo
        .find_object(target, None)
        .map_err(|e| FfError::Other(e.message().to_string()))?;

    // Defense in depth: a conflict during the SAFE checkout aborts it (the
    // callback returns `false`) before the working tree is modified.
    let conflict = Rc::new(Cell::new(false));
    let flag = conflict.clone();
    let mut checkout = git2::build::CheckoutBuilder::new();
    checkout.safe();
    checkout.notify_on(git2::CheckoutNotificationType::CONFLICT);
    checkout.notify(move |_ty, _path, _baseline, _target, _workdir| {
        flag.set(true);
        false
    });

    if let Err(e) = repo.checkout_tree(&object, Some(&mut checkout)) {
        if conflict.get() || e.code() == git2::ErrorCode::Conflict {
            return Err(FfError::Collision);
        }
        return Err(FfError::Other(e.message().to_string()));
    }

    let mut reference = repo
        .find_reference(refname)
        .map_err(|e| FfError::Other(e.message().to_string()))?;
    reference
        .set_target(target, reflog)
        .map_err(|e| FfError::Other(e.message().to_string()))?;
    repo.set_head(refname)
        .map_err(|e| FfError::Other(e.message().to_string()))?;
    Ok(())
}

/// Whether fast-forwarding to `target` would overwrite an untracked working-tree
/// file. Read-only.
///
/// Every path the incoming tree adds or changes (relative to the current `HEAD`
/// tree) is checked against the working tree: an existing file there that Git
/// does not track (`WT_NEW`) is a collision. The branch is already known to be
/// clean, so *tracked* files the update rewrites are unmodified and safe to
/// advance — only untracked files sitting in the way need protecting, matching
/// git's own "untracked working tree files would be overwritten" refusal.
fn has_untracked_collision(
    repo: &git2::Repository,
    target: git2::Oid,
) -> Result<bool, git2::Error> {
    let Some(workdir) = repo.workdir().map(Path::to_path_buf) else {
        // A bare repository has no working tree to protect.
        return Ok(false);
    };
    let head_tree = repo.head()?.peel_to_tree()?;
    let target_tree = repo.find_commit(target)?.tree()?;
    let diff = repo.diff_tree_to_tree(Some(&head_tree), Some(&target_tree), None)?;
    for delta in diff.deltas() {
        let Some(path) = delta.new_file().path() else {
            continue;
        };
        if !workdir.join(path).exists() {
            continue;
        }
        if let Ok(status) = repo.status_file(path) {
            if status.contains(git2::Status::WT_NEW) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// A classified `origin` fetch failure.
struct FetchFailure {
    /// Stable reason code: `"auth" | "network" | "fetch"`.
    reason: &'static str,
    /// Underlying git message.
    message: String,
}

/// Fetch the single branch's tracking ref from `origin`, injecting credentials
/// from the OS keychain. This is the only operation in the module that reaches
/// the network.
fn fetch_origin(repo: &git2::Repository, branch: &str) -> Result<(), FetchFailure> {
    let mut remote = repo.find_remote("origin").map_err(|e| FetchFailure {
        reason: classify_fetch_reason(&e),
        message: e.message().to_string(),
    })?;
    let url = remote.url().unwrap_or_default().to_string();
    let refspec = format!("+refs/heads/{branch}:refs/remotes/origin/{branch}");

    let mut fetch_opts = git2::FetchOptions::new();
    fetch_opts.remote_callbacks(remote_callbacks(&url).map_err(|error| FetchFailure {
        reason: "auth",
        message: format!("{error:#}"),
    })?);
    let mut proxy = git2::ProxyOptions::new();
    proxy.auto();
    fetch_opts.proxy_options(proxy);

    remote
        .fetch(&[refspec.as_str()], Some(&mut fetch_opts), None)
        .map_err(|e| FetchFailure {
            reason: classify_fetch_reason(&e),
            message: e.message().to_string(),
        })
}

/// Map a git2 fetch/push error to a stable, frontend-friendly reason code.
///
/// Shared with the sibling `fork_sync` module, which classifies both its
/// `upstream` fetch and its `origin` push failures through this same mapping.
pub(crate) fn classify_fetch_reason(e: &git2::Error) -> &'static str {
    let lower = e.message().to_ascii_lowercase();
    if e.code() == git2::ErrorCode::Auth
        || lower.contains("authentication")
        || lower.contains("401")
        || lower.contains("403")
    {
        "auth"
    } else if e.class() == git2::ErrorClass::Net
        || lower.contains("resolve")
        || lower.contains("connect")
        || lower.contains("timed out")
        || lower.contains("timeout")
    {
        "network"
    } else {
        "fetch"
    }
}

/// Credentials callbacks for a fetch or push against `url`, sourced in-memory
/// from the OS keychain by host. Generalized over the remote's URL (not a fixed
/// `origin`) so `fork_sync` can drive an `upstream` fetch and an `origin` push
/// through the same credential path. Mirrors the git2 network engine: the
/// attempt count is capped so libgit2 cannot loop forever re-invoking the
/// callback on rejection.
pub(crate) fn remote_callbacks(url: &str) -> anyhow::Result<git2::RemoteCallbacks<'static>> {
    remote_callbacks_with(url, git_credentials::load_credential)
}

fn remote_callbacks_with<F>(url: &str, load: F) -> anyhow::Result<git2::RemoteCallbacks<'static>>
where
    F: FnOnce(&str) -> anyhow::Result<Option<git_credentials::RemoteCredential>>,
{
    let cred = match git_credentials::https_host(url) {
        Some(host) => load(&host)?,
        None => None,
    };
    let mut callbacks = git2::RemoteCallbacks::new();
    let mut attempts = 0;
    callbacks.credentials(move |_url, username_from_url, _allowed| {
        attempts += 1;
        if attempts > 2 {
            return Err(git2::Error::from_str("authentication attempts exhausted"));
        }
        match &cred {
            Some(c) => git2::Cred::userpass_plaintext(&c.username, &c.password),
            None => git2::Cred::userpass_plaintext(username_from_url.unwrap_or_default(), ""),
        }
    });
    Ok(callbacks)
}

/// The upstream tracking branch shorthand (e.g. `"origin/main"`) of the current
/// branch, or `None` when detached or without configured tracking. Read-only.
fn upstream_shorthand(path: &Path) -> Option<String> {
    let repo = git2::Repository::open(path).ok()?;
    if repo.head_detached().ok()? {
        return None;
    }
    let head = repo.head().ok()?;
    let name = head.shorthand()?;
    let local = repo.find_branch(name, git2::BranchType::Local).ok()?;
    let upstream = local.upstream().ok()?;
    upstream.name().ok().flatten().map(str::to_string)
}

/// The oid the current branch's configured upstream currently points at.
fn upstream_oid(repo: &git2::Repository, branch: &str) -> Option<git2::Oid> {
    let local = repo.find_branch(branch, git2::BranchType::Local).ok()?;
    let upstream = local.upstream().ok()?;
    upstream.get().target()
}

/// Tracked-file dirtiness (`git status --porcelain -uno`): untracked and ignored
/// files are excluded so they never mark a repo dirty. Mirrors `repo_health`.
fn is_dirty(repo: &git2::Repository) -> bool {
    let mut opts = git2::StatusOptions::new();
    opts.include_untracked(false).include_ignored(false);
    match repo.statuses(Some(&mut opts)) {
        Ok(statuses) => !statuses.is_empty(),
        Err(_) => false,
    }
}

/// Short (12 hex char) form of a commit sha, matching `repo_health::head_revision`.
fn short(oid: git2::Oid) -> String {
    oid.to_string().chars().take(12).collect()
}

fn now_millis() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn skip_result(item: &PullPreview, reason: String) -> PullResult {
    PullResult {
        path: item.path.clone(),
        name: item.name.clone(),
        action: "skipped".to_string(),
        from: None,
        to: None,
        reason: Some(reason),
        message: None,
    }
}

fn error_result(item: &PullPreview, reason: &str, message: &str) -> PullResult {
    PullResult {
        path: item.path.clone(),
        name: item.name.clone(),
        action: "error".to_string(),
        from: None,
        to: None,
        reason: Some(reason.to_string()),
        message: Some(message.to_string()),
    }
}

fn up_to_date_result(item: &PullPreview, revision: &str) -> PullResult {
    PullResult {
        path: item.path.clone(),
        name: item.name.clone(),
        action: "up_to_date".to_string(),
        from: Some(revision.to_string()),
        to: Some(revision.to_string()),
        reason: None,
        message: None,
    }
}

fn updated_result(item: &PullPreview, from: &str, to: &str) -> PullResult {
    PullResult {
        path: item.path.clone(),
        name: item.name.clone(),
        action: "updated".to_string(),
        from: Some(from.to_string()),
        to: Some(to.to_string()),
        reason: None,
        message: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::process::Command;
    use tempfile::tempdir;

    #[test]
    fn remote_callbacks_propagate_github_app_reauthorization() {
        let error =
            remote_callbacks_with("https://expired-chain.patchbay.test/owner/repo.git", |_| {
                anyhow::bail!(
                    "GITHUB_APP_REAUTH_REQUIRED: the Patchbay GitHub authorization expired"
                )
            })
            .err()
            .expect("expired app authorization must reach chain callers");
        assert!(error.to_string().contains("GITHUB_APP_REAUTH_REQUIRED"));
    }

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

    /// A bare remote with one commit on `main`, plus a working clone tracking it.
    /// Returns `(remote_dir, work_dir)`.
    fn remote_and_clone(base: &Path) -> (PathBuf, PathBuf) {
        let remote = base.join("remote.git");
        let seed = base.join("seed");
        let work = base.join("work");
        assert!(Command::new("git")
            .args(["init", "--bare", "--initial-branch=main"])
            .arg(&remote)
            .output()
            .unwrap()
            .status
            .success());

        std::fs::create_dir_all(&seed).unwrap();
        git(&seed, &["init", "-b", "main"]);
        git(
            &seed,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        std::fs::write(seed.join("file.txt"), "base").unwrap();
        git(&seed, &["add", "-A"]);
        git(&seed, &["commit", "-m", "base"]);
        git(&seed, &["push", "origin", "main"]);

        assert!(Command::new("git")
            .args(["clone"])
            .arg(&remote)
            .arg(&work)
            .output()
            .unwrap()
            .status
            .success());
        (remote, work)
    }

    /// Push a second commit to the shared remote's `main` from a throwaway clone.
    /// `mutate` writes the working-tree change to commit. Returns nothing; the
    /// caller fetches into its own checkout to observe the new tip.
    fn push_remote_commit(base: &Path, remote: &Path, mutate: impl FnOnce(&Path)) {
        let other = base.join(format!("other-{}", uuid::Uuid::new_v4()));
        assert!(Command::new("git")
            .args(["clone"])
            .arg(remote)
            .arg(&other)
            .output()
            .unwrap()
            .status
            .success());
        mutate(&other);
        git(&other, &["add", "-A"]);
        git(&other, &["commit", "-m", "remote change"]);
        git(&other, &["push", "origin", "main"]);
    }

    /// Current `HEAD` sha of a checkout via system git (independent of the code
    /// under test).
    fn head_sha(dir: &Path) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        assert!(out.status.success());
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn only<'a>(plan: &'a PullPlan) -> &'a PullPreview {
        assert_eq!(plan.items.len(), 1, "expected exactly one previewed repo");
        &plan.items[0]
    }

    #[test]
    fn clean_behind_previews_fast_forward_and_applies_update() {
        let temp = tempdir().unwrap();
        let (remote, work) = remote_and_clone(temp.path());
        push_remote_commit(temp.path(), &remote, |dir| {
            std::fs::write(dir.join("file.txt"), "from other").unwrap();
        });
        // Update origin/main without moving HEAD: the repo is now clean & behind.
        git(&work, &["fetch", "origin"]);

        let plan = preview(&[work.clone()]);
        let preview_item = only(&plan);
        assert_eq!(preview_item.action, "fast_forward");
        assert_eq!(preview_item.reason, None);
        assert_eq!(preview_item.behind, 1);
        assert_eq!(preview_item.ahead, 0);
        assert!(!preview_item.dirty);
        assert_eq!(preview_item.branch.as_deref(), Some("main"));
        assert_eq!(preview_item.upstream.as_deref(), Some("origin/main"));

        let before = head_sha(&work);
        let results = apply(&plan);
        assert_eq!(results.len(), 1);
        let result = &results[0];
        assert_eq!(result.action, "updated", "result: {result:?}");
        assert_eq!(result.from.as_deref(), Some(&before[..12]));
        assert!(result.to.is_some());
        assert_ne!(result.from, result.to, "HEAD must have advanced");

        // The working tree now matches the remote head, by fast-forward only.
        assert_eq!(
            std::fs::read_to_string(work.join("file.txt")).unwrap(),
            "from other"
        );
        assert_ne!(head_sha(&work), before);
    }

    #[test]
    fn dirty_behind_is_skipped_and_changes_nothing() {
        let temp = tempdir().unwrap();
        let (remote, work) = remote_and_clone(temp.path());
        push_remote_commit(temp.path(), &remote, |dir| {
            std::fs::write(dir.join("file.txt"), "from other").unwrap();
        });
        git(&work, &["fetch", "origin"]);
        // A tracked-file edit makes the working tree dirty.
        std::fs::write(work.join("file.txt"), "local edit").unwrap();

        let plan = preview(&[work.clone()]);
        let preview_item = only(&plan);
        assert_eq!(preview_item.action, "skip");
        assert_eq!(preview_item.reason.as_deref(), Some("dirty"));

        let before = head_sha(&work);
        let result = &apply(&plan)[0];
        assert_eq!(result.action, "skipped");
        assert_eq!(result.reason.as_deref(), Some("dirty"));

        // Nothing changed: HEAD unmoved and the dirty edit preserved.
        assert_eq!(head_sha(&work), before);
        assert_eq!(
            std::fs::read_to_string(work.join("file.txt")).unwrap(),
            "local edit"
        );
    }

    #[test]
    fn diverged_is_skipped_and_head_is_unchanged() {
        let temp = tempdir().unwrap();
        let (remote, work) = remote_and_clone(temp.path());
        push_remote_commit(temp.path(), &remote, |dir| {
            std::fs::write(dir.join("file.txt"), "from other").unwrap();
        });
        // Local diverges: its own commit plus the fetched remote tip.
        std::fs::write(work.join("file.txt"), "from work").unwrap();
        git(&work, &["commit", "-am", "local work"]);
        git(&work, &["fetch", "origin"]);

        let plan = preview(&[work.clone()]);
        let preview_item = only(&plan);
        assert_eq!(preview_item.action, "skip");
        assert_eq!(preview_item.reason.as_deref(), Some("diverged"));

        let before = head_sha(&work);
        let result = &apply(&plan)[0];
        assert_eq!(result.action, "skipped");
        assert_eq!(result.reason.as_deref(), Some("diverged"));
        // Never force-updated: a diverged repo's HEAD is identical before/after.
        assert_eq!(head_sha(&work), before);
    }

    #[test]
    fn branch_without_upstream_is_skipped() {
        let temp = tempdir().unwrap();
        let (_remote, work) = remote_and_clone(temp.path());
        git(&work, &["checkout", "-b", "feature"]);

        let plan = preview(&[work.clone()]);
        let preview_item = only(&plan);
        assert_eq!(preview_item.action, "skip");
        assert_eq!(preview_item.reason.as_deref(), Some("no_upstream"));
        assert_eq!(preview_item.upstream, None);

        let before = head_sha(&work);
        let result = &apply(&plan)[0];
        assert_eq!(result.action, "skipped");
        assert_eq!(result.reason.as_deref(), Some("no_upstream"));
        assert_eq!(head_sha(&work), before);
    }

    #[test]
    fn up_to_date_repo_is_skipped() {
        let temp = tempdir().unwrap();
        let (_remote, work) = remote_and_clone(temp.path());

        let plan = preview(&[work.clone()]);
        let preview_item = only(&plan);
        assert_eq!(preview_item.action, "skip");
        assert_eq!(preview_item.reason.as_deref(), Some("up_to_date"));

        let result = &apply(&plan)[0];
        assert_eq!(result.action, "skipped");
        assert_eq!(result.reason.as_deref(), Some("up_to_date"));
    }

    #[test]
    fn untracked_collision_is_skipped_and_preserves_local_file() {
        let temp = tempdir().unwrap();
        let (remote, work) = remote_and_clone(temp.path());
        // The remote adds a brand-new file X.
        push_remote_commit(temp.path(), &remote, |dir| {
            std::fs::write(dir.join("collide.txt"), "remote version").unwrap();
        });
        git(&work, &["fetch", "origin"]);
        // Locally, X already exists as an UNTRACKED file with different contents.
        std::fs::write(work.join("collide.txt"), "local untracked").unwrap();

        // Untracked files never mark the repo dirty, so it still previews as a
        // clean fast-forward — the collision is only discovered at checkout.
        let plan = preview(&[work.clone()]);
        let preview_item = only(&plan);
        assert_eq!(preview_item.action, "fast_forward");
        assert!(!preview_item.dirty);

        let before = head_sha(&work);
        let result = &apply(&plan)[0];
        assert_eq!(
            result.action, "skipped",
            "a SAFE checkout must refuse the collision: {result:?}"
        );
        assert_eq!(result.reason.as_deref(), Some("untracked_collision"));

        // The local file is preserved untouched and HEAD never moved.
        assert_eq!(
            std::fs::read_to_string(work.join("collide.txt")).unwrap(),
            "local untracked"
        );
        assert_eq!(head_sha(&work), before);
    }

    #[test]
    fn fetch_failure_is_an_error_and_leaves_head_unmoved() {
        let temp = tempdir().unwrap();
        let (remote, work) = remote_and_clone(temp.path());
        push_remote_commit(temp.path(), &remote, |dir| {
            std::fs::write(dir.join("file.txt"), "from other").unwrap();
        });
        // Make the repo clean & behind, then break origin so the apply-time
        // fetch cannot reach it. This exercises the same fetch-error branch that
        // an authentication failure takes (auth/network/fetch all route here).
        git(&work, &["fetch", "origin"]);
        let bogus = temp.path().join("does-not-exist.git");
        git(
            &work,
            &["remote", "set-url", "origin", bogus.to_str().unwrap()],
        );

        let plan = preview(&[work.clone()]);
        assert_eq!(only(&plan).action, "fast_forward");

        let before = head_sha(&work);
        let result = &apply(&plan)[0];
        assert_eq!(result.action, "error", "result: {result:?}");
        assert!(
            matches!(result.reason.as_deref(), Some("auth" | "network" | "fetch")),
            "fetch error reason: {:?}",
            result.reason
        );
        assert!(result.message.is_some());
        // A failed fetch must never advance the branch.
        assert_eq!(head_sha(&work), before);
    }

    #[test]
    fn scan_error_for_non_repository_is_skipped() {
        let temp = tempdir().unwrap();
        let plain = temp.path().join("not-a-repo");
        std::fs::create_dir_all(&plain).unwrap();

        let plan = preview(&[plain.clone()]);
        let preview_item = only(&plan);
        assert_eq!(preview_item.action, "skip");
        assert_eq!(preview_item.reason.as_deref(), Some("scan_error"));

        let result = &apply(&plan)[0];
        assert_eq!(result.action, "skipped");
        assert_eq!(result.reason.as_deref(), Some("scan_error"));
    }

    #[test]
    fn plan_round_trips_through_serde() {
        // The plan crosses the wire to the frontend and back into apply.
        let plan = PullPlan {
            items: vec![PullPreview {
                path: "/tmp/repo".to_string(),
                name: "repo".to_string(),
                branch: Some("main".to_string()),
                upstream: Some("origin/main".to_string()),
                ahead: 0,
                behind: 2,
                dirty: false,
                action: "fast_forward".to_string(),
                reason: None,
            }],
            scanned_at: 42,
        };
        let json = serde_json::to_string(&plan).unwrap();
        let back: PullPlan = serde_json::from_str(&json).unwrap();
        assert_eq!(back.scanned_at, 42);
        assert_eq!(back.items[0].action, "fast_forward");
        assert_eq!(back.items[0].behind, 2);
    }
}
