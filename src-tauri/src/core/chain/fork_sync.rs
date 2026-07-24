//! Fast-forward-only fork synchronization for Original Repositories.
//!
//! A fork has two remotes with fixed roles: **`upstream` is the source of
//! truth, `origin` is the fork you push to**. This module advances
//! `origin/<branch>` up to `upstream/<branch>` by fast-forward only
//! (source → target = upstream → origin), and refuses anything that would
//! rewrite history or act on an ambiguous remote relationship.
//!
//! Like the sibling [`pull`](super::pull) module, it is deliberately narrow and
//! defensive:
//!
//! * **Preview never touches the network and never pushes.** [`preview`]
//!   classifies each repository from its *current* local refs — the configured
//!   `origin`/`upstream` remotes and the `refs/remotes/{remote}/<branch>`
//!   tracking refs — and decides, per repo, whether a fast-forward is eligible
//!   or must be skipped with a stable reason code. Scanning an outdated fork
//!   therefore never fetches and never triggers a push.
//! * **Apply only ever fast-forwards, and only pushes with a non-forcing
//!   refspec.** [`apply`] fetches `upstream`, re-verifies (TOCTOU) that
//!   `origin/<branch>` is strictly behind `upstream/<branch>` and that the
//!   upstream strictly descends from it, optionally advances the *local* branch
//!   by a SAFE fast-forward, and finally pushes the upstream commit to
//!   `origin` with a refspec that carries **no leading `+`**. A non-fast-forward
//!   rejection from the remote — or any recomputed divergence — is refused, not
//!   forced. The push refspec is never `+…`, so the remote's own
//!   fast-forward check is the last line of defense.
//!
//! Credentials for both the `upstream` fetch and the `origin` push are injected
//! in-memory from the OS keychain, reusing the generalized
//! [`pull::remote_callbacks`](super::pull::remote_callbacks).

use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use super::pull::{self, FfError};

/// One repository's read-only fork-sync classification, produced by [`preview`]
/// and consumed unchanged by [`apply`]. Every field is populated so the preview
/// *names* the source, target, branch, and lag even for a skipped repo.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForkSyncPreview {
    /// Absolute path of the Original Repository checkout.
    pub path: String,
    /// Directory name of the checkout (display identity).
    pub name: String,
    /// Current local branch, when `HEAD` is on one.
    pub branch: Option<String>,
    /// `origin` fetch URL, when the remote is configured.
    pub origin: Option<String>,
    /// `upstream` fetch URL, when the remote is configured.
    pub upstream: Option<String>,
    /// Synchronization source shorthand, e.g. `"upstream/main"`.
    pub source: Option<String>,
    /// Synchronization target shorthand, e.g. `"origin/main"`.
    pub target: Option<String>,
    /// Commits `origin/<branch>` is *ahead* of `upstream/<branch>`. Must be `0`
    /// to synchronize — any ahead commit means a fast-forward would drop work.
    pub ahead: usize,
    /// Commits `origin/<branch>` is *behind* `upstream/<branch>`.
    pub behind: usize,
    /// Whether `upstream/<branch>` strictly descends from `origin/<branch>` so
    /// the target can be fast-forwarded. The local branch's own fast-forward
    /// eligibility is re-verified at [`apply`] time (TOCTOU).
    pub fast_forwardable: bool,
    /// `"fast_forward"` when a fast-forward is eligible, `"skip"` otherwise.
    pub action: String,
    /// Stable reason code when `action == "skip"`: `"no_origin" | "no_upstream"
    /// | "detached" | "ambiguous_branch" | "up_to_date" | "diverged"`.
    pub reason: Option<String>,
}

/// A previewed, guarded fork-sync. Produced by [`preview`], carried across the
/// wire, and handed back to [`apply`] so the update acts only on what the user
/// saw.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForkSyncPlan {
    pub items: Vec<ForkSyncPreview>,
    /// When the classification was taken (epoch millis).
    pub scanned_at: i64,
}

/// The outcome of attempting one repository's fork-sync.
#[derive(Debug, Clone, Serialize)]
pub struct ForkSyncResult {
    pub path: String,
    pub name: String,
    /// `"synced" | "skipped" | "up_to_date" | "error"`.
    pub action: String,
    /// Short `origin/<branch>` sha before the attempt, when known.
    pub from: Option<String>,
    /// Short sha after the attempt (equal to the upstream tip), when known.
    pub to: Option<String>,
    /// Stable skip code (`"dirty"`, `"diverged"`, `"ambiguous_branch"`,
    /// `"untracked_collision"`, …) or error code (`"auth"`, `"network"`,
    /// `"fetch"`, `"checkout"`).
    pub reason: Option<String>,
    /// Free-form detail, populated for errors (the underlying git message).
    pub message: Option<String>,
}

/// The full apply outcome plus a fresh timestamp from the post-apply rescan.
#[derive(Debug, Clone, Serialize)]
pub struct ForkSyncOutcome {
    pub results: Vec<ForkSyncResult>,
    /// When the confirming rescan was taken (epoch millis).
    pub scanned_at: i64,
}

/// Classify each repository's fork-sync eligibility from its *current* local
/// refs.
///
/// Read-only: nothing here fetches, and nothing mutates the repository or any
/// remote (AC3/AC5). A repository is eligible (`action == "fast_forward"`) only
/// when both remotes exist, both `<remote>/<branch>` tracking refs resolve,
/// `origin/<branch>` is strictly behind `upstream/<branch>` with no ahead
/// commits, and the upstream strictly descends from it; every other state is a
/// `skip` carrying the precise reason so the UI can explain why sync was
/// withheld.
pub fn preview(repo_paths: &[PathBuf]) -> ForkSyncPlan {
    ForkSyncPlan {
        items: repo_paths.iter().map(|path| classify(path)).collect(),
        scanned_at: now_millis(),
    }
}

/// Attempt each previewed fork-sync, synchronizing *only* the eligible ones and
/// reflecting every skip verbatim.
///
/// For a `"fast_forward"` item the sequence is: re-open the repository,
/// TOCTOU-recheck working-tree cleanliness, fetch `upstream`, recompute the
/// `origin`/`upstream` tracking positions and re-verify the fast-forward still
/// holds, optionally advance the *local* branch by a SAFE fast-forward, then
/// push the upstream commit to `origin` with a **non-forcing** refspec. Any
/// deviation (auth/network failure, a target that is no longer a clean
/// fast-forward, a local branch that is not an ancestor of upstream, or a remote
/// non-fast-forward rejection) becomes an explicit `error`/`skipped` result and
/// leaves both `origin` and the local branch untouched.
pub fn apply(plan: &ForkSyncPlan) -> Vec<ForkSyncResult> {
    plan.items.iter().map(apply_one).collect()
}

/// Read-only per-repository classification shared by [`preview`].
fn classify(path: &Path) -> ForkSyncPreview {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut item = ForkSyncPreview {
        path: path.to_string_lossy().to_string(),
        name,
        branch: None,
        origin: None,
        upstream: None,
        source: None,
        target: None,
        ahead: 0,
        behind: 0,
        fast_forwardable: false,
        action: "skip".to_string(),
        reason: None,
    };

    // A path we cannot open as a repository has no readable `origin` to sync.
    let Ok(repo) = git2::Repository::open(path) else {
        item.reason = Some("no_origin".to_string());
        return item;
    };

    // Record both remote identities up front so the preview can show them even
    // when a later step forces a skip.
    item.origin = remote_url(&repo, "origin");
    item.upstream = remote_url(&repo, "upstream");

    // The fork relationship must be unambiguous: both remotes configured.
    if repo.find_remote("origin").is_err() {
        item.reason = Some("no_origin".to_string());
        return item;
    }
    if repo.find_remote("upstream").is_err() {
        item.reason = Some("no_upstream".to_string());
        return item;
    }

    // A detached HEAD has no branch whose fork to synchronize.
    let Some(branch) = current_branch(&repo) else {
        item.reason = Some("detached".to_string());
        return item;
    };
    item.branch = Some(branch.clone());
    item.source = Some(format!("upstream/{branch}"));
    item.target = Some(format!("origin/{branch}"));

    // Both tracking refs must resolve. A fork that never fetched `upstream`, or
    // a branch absent on either remote, is ambiguous — never guessed at.
    let (Some(origin_oid), Some(upstream_oid)) = (
        tracking_oid(&repo, "origin", &branch),
        tracking_oid(&repo, "upstream", &branch),
    ) else {
        item.reason = Some("ambiguous_branch".to_string());
        return item;
    };

    // ahead = commits origin has that upstream lacks; behind = commits upstream
    // has that origin lacks.
    let Ok((ahead, behind)) = repo.graph_ahead_behind(origin_oid, upstream_oid) else {
        // Unreachable for two valid oids in one repo; refuse rather than guess.
        item.reason = Some("ambiguous_branch".to_string());
        return item;
    };
    item.ahead = ahead;
    item.behind = behind;

    if behind == 0 && ahead == 0 {
        item.reason = Some("up_to_date".to_string());
        return item;
    }
    if ahead > 0 {
        // origin carries commits upstream lacks: a fast-forward would drop them.
        item.reason = Some("diverged".to_string());
        return item;
    }
    // behind > 0 && ahead == 0: eligible only if upstream strictly descends.
    if repo
        .graph_descendant_of(upstream_oid, origin_oid)
        .unwrap_or(false)
    {
        item.action = "fast_forward".to_string();
        item.fast_forwardable = true;
    } else {
        item.reason = Some("diverged".to_string());
    }
    item
}

/// Attempt one repository's fork-sync. Never panics; every git2 failure
/// collapses to a structured skip/error result.
fn apply_one(item: &ForkSyncPreview) -> ForkSyncResult {
    // Non-eligible previews are reflected verbatim as protected refusals — apply
    // acts only on what preview marked fast-forward, and never fetches or pushes
    // for them (AC5).
    if item.action != "fast_forward" {
        return skip_result(
            item,
            item.reason.clone().unwrap_or_else(|| "skip".to_string()),
        );
    }

    let path = PathBuf::from(&item.path);
    let Ok(repo) = git2::Repository::open(&path) else {
        // Preview opened it; a now-unreadable repo has no readable origin.
        return skip_result(item, "no_origin".to_string());
    };

    // TOCTOU guard: the working tree may have become dirty since preview.
    if is_dirty(&repo) {
        return skip_result(item, "dirty".to_string());
    }

    let Some(branch) = current_branch(&repo) else {
        return skip_result(item, "detached".to_string());
    };

    // The only network fetch. A failure is classified and returned; neither the
    // local branch nor `origin` is touched.
    if let Err(failure) = fetch_remote(&repo, "upstream", &branch) {
        return error_result(item, failure.reason, &failure.message);
    }

    // Recompute both tracking positions from the freshly-fetched refs.
    let (Some(upstream_oid), Some(origin_oid)) = (
        tracking_oid(&repo, "upstream", &branch),
        tracking_oid(&repo, "origin", &branch),
    ) else {
        return skip_result(item, "ambiguous_branch".to_string());
    };
    let before = short(origin_oid);
    let (ahead, behind) = match repo.graph_ahead_behind(origin_oid, upstream_oid) {
        Ok(counts) => counts,
        Err(e) => return error_result(item, "fetch", e.message()),
    };
    if behind == 0 {
        // Nothing to advance. Either already current, or (TOCTOU) origin moved
        // ahead since preview — never merged, never forced.
        return if ahead == 0 {
            up_to_date_result(item, &before)
        } else {
            skip_result(item, "diverged".to_string())
        };
    }
    if ahead > 0 {
        // Diverged since preview: refuse rather than force origin backward.
        return skip_result(item, "diverged".to_string());
    }
    // A true fast-forward requires upstream to strictly descend from origin.
    if !repo
        .graph_descendant_of(upstream_oid, origin_oid)
        .unwrap_or(false)
    {
        return skip_result(item, "diverged".to_string());
    }

    // Optionally advance the LOCAL branch to upstream so the checkout matches
    // what we are about to push. Only when the local tip is an ancestor of
    // upstream (a genuine fast-forward); otherwise the local branch carries work
    // upstream lacks and we refuse rather than rewrite it.
    let Some((local_oid, refname)) = local_head(&repo) else {
        return skip_result(item, "detached".to_string());
    };
    if local_oid != upstream_oid {
        if !repo
            .graph_descendant_of(upstream_oid, local_oid)
            .unwrap_or(false)
        {
            return skip_result(item, "diverged".to_string());
        }
        match pull::fast_forward(
            &repo,
            upstream_oid,
            &refname,
            "patchbay: fast-forward fork-sync",
        ) {
            Ok(()) => {}
            Err(FfError::Collision) => return skip_result(item, "untracked_collision".to_string()),
            Err(FfError::Other(message)) => return error_result(item, "checkout", &message),
        }
    }

    // Push the upstream commit to origin, FAST-FORWARD ONLY. The refspec carries
    // no leading `+`, so a non-fast-forward is rejected by the remote rather than
    // forced.
    match push_fast_forward(&repo, "origin", &branch, upstream_oid) {
        PushOutcome::Ok => {
            // Reflect the successful push in the local tracking ref so the
            // post-apply rescan reports origin as current without a re-fetch.
            let _ = repo.reference(
                &format!("refs/remotes/origin/{branch}"),
                upstream_oid,
                true,
                "patchbay: fork-sync push",
            );
            synced_result(item, &before, &short(upstream_oid))
        }
        // The remote refused a non-fast-forward update: never forced.
        PushOutcome::Rejected => skip_result(item, "diverged".to_string()),
        PushOutcome::Error { reason, message } => error_result(item, reason, &message),
    }
}

/// The outcome of pushing to `origin` with a non-forcing refspec.
enum PushOutcome {
    /// The fast-forward push succeeded.
    Ok,
    /// The remote refused the update as a non-fast-forward.
    Rejected,
    /// A transport-level failure (auth/network/other) before any ref moved.
    Error {
        reason: &'static str,
        message: String,
    },
}

/// Push `target_oid` to `refs/heads/<branch>` on `remote_name` by fast-forward
/// only.
///
/// The refspec is `"<sha>:refs/heads/<branch>"` — deliberately **without** a
/// leading `+` — so the remote's fast-forward check rejects any non-descendant
/// update instead of overwriting it. A rejection is surfaced both through the
/// [`git2::ErrorCode::NotFastForward`] transport error and through the
/// `push_update_reference` callback (libgit2 reports per-ref rejections there
/// even when the transport call itself returns `Ok`), and either path maps to
/// [`PushOutcome::Rejected`]. This function never force-pushes, rebases, or
/// merges.
fn push_fast_forward(
    repo: &git2::Repository,
    remote_name: &str,
    branch: &str,
    target_oid: git2::Oid,
) -> PushOutcome {
    let mut remote = match repo.find_remote(remote_name) {
        Ok(remote) => remote,
        Err(e) => {
            return PushOutcome::Error {
                reason: pull::classify_fetch_reason(&e),
                message: e.message().to_string(),
            }
        }
    };
    let url = remote.url().unwrap_or_default().to_string();
    // Non-forcing: no leading `+`. A non-fast-forward is rejected, never forced.
    let refspec = format!("{target_oid}:refs/heads/{branch}");

    // libgit2 reports per-reference rejections (the non-fast-forward case) via
    // this callback even when the transport `push` call returns Ok.
    let rejected: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let rejected_cb = rejected.clone();
    let mut callbacks = match pull::remote_callbacks(&url) {
        Ok(callbacks) => callbacks,
        Err(error) => {
            return PushOutcome::Error {
                reason: "auth",
                message: format!("{error:#}"),
            }
        }
    };
    callbacks.push_update_reference(move |refname, status| {
        if let Some(reason) = status {
            rejected_cb
                .borrow_mut()
                .push(format!("{refname}: {reason}"));
        }
        Ok(())
    });

    let mut push_opts = git2::PushOptions::new();
    push_opts.remote_callbacks(callbacks);
    let mut proxy = git2::ProxyOptions::new();
    proxy.auto();
    push_opts.proxy_options(proxy);

    if let Err(e) = remote.push(&[refspec.as_str()], Some(&mut push_opts)) {
        if e.code() == git2::ErrorCode::NotFastForward {
            return PushOutcome::Rejected;
        }
        return PushOutcome::Error {
            reason: pull::classify_fetch_reason(&e),
            message: e.message().to_string(),
        };
    }
    if !rejected.borrow().is_empty() {
        return PushOutcome::Rejected;
    }
    PushOutcome::Ok
}

/// A classified remote fetch failure (mirrors the reason vocabulary the pull
/// module already surfaces).
struct FetchFailure {
    /// Stable reason code: `"auth" | "network" | "fetch"`.
    reason: &'static str,
    /// Underlying git message.
    message: String,
}

/// Fetch the single branch's tracking ref from `remote_name`, injecting
/// credentials from the OS keychain. For fork-sync this is only ever the
/// `upstream` fetch — the sole network *read* in the flow.
fn fetch_remote(
    repo: &git2::Repository,
    remote_name: &str,
    branch: &str,
) -> Result<(), FetchFailure> {
    let mut remote = repo.find_remote(remote_name).map_err(|e| FetchFailure {
        reason: pull::classify_fetch_reason(&e),
        message: e.message().to_string(),
    })?;
    let url = remote.url().unwrap_or_default().to_string();
    let refspec = format!("+refs/heads/{branch}:refs/remotes/{remote_name}/{branch}");

    let mut fetch_opts = git2::FetchOptions::new();
    fetch_opts.remote_callbacks(pull::remote_callbacks(&url).map_err(|error| FetchFailure {
        reason: "auth",
        message: format!("{error:#}"),
    })?);
    let mut proxy = git2::ProxyOptions::new();
    proxy.auto();
    fetch_opts.proxy_options(proxy);

    remote
        .fetch(&[refspec.as_str()], Some(&mut fetch_opts), None)
        .map_err(|e| FetchFailure {
            reason: pull::classify_fetch_reason(&e),
            message: e.message().to_string(),
        })
}

/// The current local branch name, or `None` when `HEAD` is detached or unborn.
fn current_branch(repo: &git2::Repository) -> Option<String> {
    if repo.head_detached().ok()? {
        return None;
    }
    repo.head().ok()?.shorthand().map(str::to_string)
}

/// The current `HEAD` commit oid paired with its fully-qualified ref name, or
/// `None` when `HEAD` is not on a resolvable branch.
fn local_head(repo: &git2::Repository) -> Option<(git2::Oid, String)> {
    let head = repo.head().ok()?;
    let oid = head.target()?;
    let name = head.name()?.to_string();
    Some((oid, name))
}

/// The oid of a `refs/remotes/<remote>/<branch>` tracking ref, or `None` when it
/// does not resolve (the remote was never fetched, or the branch is absent).
fn tracking_oid(repo: &git2::Repository, remote: &str, branch: &str) -> Option<git2::Oid> {
    repo.refname_to_id(&format!("refs/remotes/{remote}/{branch}"))
        .ok()
}

/// A remote's fetch URL, or `None` when it is not configured.
fn remote_url(repo: &git2::Repository, name: &str) -> Option<String> {
    repo.find_remote(name).ok()?.url().map(str::to_string)
}

/// Tracked-file dirtiness (`git status --porcelain -uno`): untracked and ignored
/// files are excluded so they never block a fork-sync. Mirrors `repo_health` and
/// `pull`.
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

fn skip_result(item: &ForkSyncPreview, reason: String) -> ForkSyncResult {
    ForkSyncResult {
        path: item.path.clone(),
        name: item.name.clone(),
        action: "skipped".to_string(),
        from: None,
        to: None,
        reason: Some(reason),
        message: None,
    }
}

fn error_result(item: &ForkSyncPreview, reason: &str, message: &str) -> ForkSyncResult {
    ForkSyncResult {
        path: item.path.clone(),
        name: item.name.clone(),
        action: "error".to_string(),
        from: None,
        to: None,
        reason: Some(reason.to_string()),
        message: Some(message.to_string()),
    }
}

fn up_to_date_result(item: &ForkSyncPreview, revision: &str) -> ForkSyncResult {
    ForkSyncResult {
        path: item.path.clone(),
        name: item.name.clone(),
        action: "up_to_date".to_string(),
        from: Some(revision.to_string()),
        to: Some(revision.to_string()),
        reason: None,
        message: None,
    }
}

fn synced_result(item: &ForkSyncPreview, from: &str, to: &str) -> ForkSyncResult {
    ForkSyncResult {
        path: item.path.clone(),
        name: item.name.clone(),
        action: "synced".to_string(),
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

    /// Initialize a bare repository with `main` as the initial branch.
    fn init_bare(path: &Path) {
        assert!(Command::new("git")
            .args(["init", "--bare", "--initial-branch=main"])
            .arg(path)
            .output()
            .unwrap()
            .status
            .success());
    }

    /// A fork fixture: a bare `upstream.git` (the source), a bare `origin.git`
    /// (the fork/target), both seeded with the same `base` commit on `main`, and
    /// a `work` clone of `origin` that also has an `upstream` remote already
    /// fetched. Returns `(upstream_bare, origin_bare, work)`.
    fn fork_fixture(base: &Path) -> (PathBuf, PathBuf, PathBuf) {
        let upstream = base.join("upstream.git");
        let origin = base.join("origin.git");
        let seed = base.join("seed");
        let work = base.join("work");
        init_bare(&upstream);
        init_bare(&origin);

        std::fs::create_dir_all(&seed).unwrap();
        git(&seed, &["init", "-b", "main"]);
        std::fs::write(seed.join("file.txt"), "base").unwrap();
        git(&seed, &["add", "-A"]);
        git(&seed, &["commit", "-m", "base"]);
        git(
            &seed,
            &["remote", "add", "upstream", upstream.to_str().unwrap()],
        );
        git(
            &seed,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        git(&seed, &["push", "upstream", "main"]);
        git(&seed, &["push", "origin", "main"]);

        // work clones origin (so `origin` is its origin) and adds+fetches upstream.
        assert!(Command::new("git")
            .args(["clone"])
            .arg(&origin)
            .arg(&work)
            .output()
            .unwrap()
            .status
            .success());
        git(
            &work,
            &["remote", "add", "upstream", upstream.to_str().unwrap()],
        );
        git(&work, &["fetch", "upstream"]);
        (upstream, origin, work)
    }

    /// Push one new commit to `bare`'s `main` from a throwaway clone. `mutate`
    /// writes the working-tree change to commit.
    fn push_commit_to(base: &Path, bare: &Path, mutate: impl FnOnce(&Path)) {
        let other = base.join(format!("other-{}", uuid::Uuid::new_v4()));
        assert!(Command::new("git")
            .args(["clone"])
            .arg(bare)
            .arg(&other)
            .output()
            .unwrap()
            .status
            .success());
        mutate(&other);
        git(&other, &["add", "-A"]);
        git(&other, &["commit", "-m", "change"]);
        git(&other, &["push", "origin", "main"]);
    }

    /// The oid `bare`'s `main` currently points at, read via system git
    /// (independent of the code under test). Bare repos have no working tree, so
    /// `rev-parse` runs against the repository directory directly.
    fn bare_main(bare: &Path) -> String {
        let out = Command::new("git")
            .arg("--git-dir")
            .arg(bare)
            .args(["rev-parse", "main"])
            .output()
            .unwrap();
        assert!(out.status.success(), "rev-parse main in bare failed");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// Current `HEAD` sha of a checkout via system git.
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

    fn only(plan: &ForkSyncPlan) -> &ForkSyncPreview {
        assert_eq!(plan.items.len(), 1, "expected exactly one previewed repo");
        &plan.items[0]
    }

    #[test]
    fn fork_behind_upstream_previews_and_synchronizes_fast_forward() {
        let temp = tempdir().unwrap();
        let (upstream, origin, work) = fork_fixture(temp.path());
        // Advance upstream; the fork's clone fetches it so origin/main is behind
        // upstream/main. origin.git itself is untouched (still at base).
        push_commit_to(temp.path(), &upstream, |dir| {
            std::fs::write(dir.join("file.txt"), "from upstream").unwrap();
        });
        git(&work, &["fetch", "upstream"]);
        let upstream_head = bare_main(&upstream);
        let origin_before = bare_main(&origin);
        assert_ne!(upstream_head, origin_before, "fork should be behind");

        // Preview names source, target, branch, and lag (AC1/AC2).
        let plan = preview(&[work.clone()]);
        let item = only(&plan);
        assert_eq!(item.action, "fast_forward");
        assert!(item.fast_forwardable);
        assert_eq!(item.reason, None);
        assert_eq!(item.branch.as_deref(), Some("main"));
        assert_eq!(item.source.as_deref(), Some("upstream/main"));
        assert_eq!(item.target.as_deref(), Some("origin/main"));
        assert_eq!(item.ahead, 0);
        assert_eq!(item.behind, 1);

        let results = apply(&plan);
        assert_eq!(results.len(), 1);
        let result = &results[0];
        assert_eq!(result.action, "synced", "result: {result:?}");
        assert_eq!(result.from.as_deref(), Some(&origin_before[..12]));
        assert_eq!(result.to.as_deref(), Some(&upstream_head[..12]));

        // The bare origin's main now equals the upstream head — advanced by
        // fast-forward push (AC6). The local branch was fast-forwarded too.
        assert_eq!(bare_main(&origin), upstream_head);
        assert_eq!(head_sha(&work), upstream_head);
        assert_eq!(
            std::fs::read_to_string(work.join("file.txt")).unwrap(),
            "from upstream"
        );
    }

    #[test]
    fn diverged_fork_is_refused_and_origin_is_unchanged() {
        let temp = tempdir().unwrap();
        let (upstream, origin, work) = fork_fixture(temp.path());
        // origin gains a commit upstream lacks, and upstream advances too, so the
        // two histories genuinely diverge.
        push_commit_to(temp.path(), &origin, |dir| {
            std::fs::write(dir.join("file.txt"), "from origin").unwrap();
        });
        push_commit_to(temp.path(), &upstream, |dir| {
            std::fs::write(dir.join("other.txt"), "from upstream").unwrap();
        });
        git(&work, &["fetch", "origin"]);
        git(&work, &["fetch", "upstream"]);
        let origin_before = bare_main(&origin);

        let plan = preview(&[work.clone()]);
        let item = only(&plan);
        assert_eq!(item.action, "skip");
        assert_eq!(item.reason.as_deref(), Some("diverged"));
        assert!(!item.fast_forwardable);
        assert!(item.ahead > 0, "origin has a commit upstream lacks");

        let result = &apply(&plan)[0];
        assert_eq!(result.action, "skipped");
        assert_eq!(result.reason.as_deref(), Some("diverged"));
        // Never force-pushed: the bare origin's ref is byte-identical (AC4).
        assert_eq!(bare_main(&origin), origin_before);
    }

    #[test]
    fn missing_upstream_remote_is_skipped() {
        let temp = tempdir().unwrap();
        let (_upstream, _origin, work) = fork_fixture(temp.path());
        // Remove the upstream remote entirely: the fork relationship is undefined.
        git(&work, &["remote", "remove", "upstream"]);

        let plan = preview(&[work.clone()]);
        let item = only(&plan);
        assert_eq!(item.action, "skip");
        assert_eq!(item.reason.as_deref(), Some("no_upstream"));
        assert!(item.upstream.is_none());

        let result = &apply(&plan)[0];
        assert_eq!(result.action, "skipped");
        assert_eq!(result.reason.as_deref(), Some("no_upstream"));
    }

    #[test]
    fn up_to_date_fork_is_skipped_and_origin_unchanged() {
        let temp = tempdir().unwrap();
        let (_upstream, origin, work) = fork_fixture(temp.path());
        // Fresh fixture: origin/main and upstream/main both at base.
        let origin_before = bare_main(&origin);

        let plan = preview(&[work.clone()]);
        let item = only(&plan);
        assert_eq!(item.action, "skip");
        assert_eq!(item.reason.as_deref(), Some("up_to_date"));
        assert_eq!(item.ahead, 0);
        assert_eq!(item.behind, 0);
        // The source/target are still named for a fork that is already current.
        assert_eq!(item.source.as_deref(), Some("upstream/main"));
        assert_eq!(item.target.as_deref(), Some("origin/main"));

        let result = &apply(&plan)[0];
        assert_eq!(result.action, "skipped");
        assert_eq!(result.reason.as_deref(), Some("up_to_date"));
        assert_eq!(bare_main(&origin), origin_before);
    }

    #[test]
    fn upstream_never_fetched_is_ambiguous() {
        let temp = tempdir().unwrap();
        let (upstream, _origin, work) = fork_fixture(temp.path());
        // Configure a fresh upstream remote with no tracking ref: drop the
        // fetched refs so `refs/remotes/upstream/main` no longer resolves.
        git(&work, &["remote", "remove", "upstream"]);
        git(
            &work,
            &["remote", "add", "upstream", upstream.to_str().unwrap()],
        );

        let plan = preview(&[work.clone()]);
        let item = only(&plan);
        assert_eq!(item.action, "skip");
        assert_eq!(item.reason.as_deref(), Some("ambiguous_branch"));
        // The upstream remote IS configured — its URL is shown — but unfetched.
        assert!(item.upstream.is_some());

        let result = &apply(&plan)[0];
        assert_eq!(result.action, "skipped");
        assert_eq!(result.reason.as_deref(), Some("ambiguous_branch"));
    }

    #[test]
    fn preview_never_fetches_or_pushes() {
        let temp = tempdir().unwrap();
        let (upstream, origin, work) = fork_fixture(temp.path());
        push_commit_to(temp.path(), &upstream, |dir| {
            std::fs::write(dir.join("file.txt"), "from upstream").unwrap();
        });
        git(&work, &["fetch", "upstream"]);
        let origin_before = bare_main(&origin);

        // Previewing a behind fork classifies it as fast-forwardable but must not
        // touch the bare origin at all (AC5).
        let plan = preview(&[work.clone()]);
        assert_eq!(only(&plan).action, "fast_forward");
        assert_eq!(bare_main(&origin), origin_before);
    }

    #[test]
    fn dirty_fork_is_skipped_at_apply_and_origin_unchanged() {
        let temp = tempdir().unwrap();
        let (upstream, origin, work) = fork_fixture(temp.path());
        push_commit_to(temp.path(), &upstream, |dir| {
            std::fs::write(dir.join("file.txt"), "from upstream").unwrap();
        });
        git(&work, &["fetch", "upstream"]);
        let origin_before = bare_main(&origin);

        // Preview sees a clean, behind fork (untracked/tracked cleanliness).
        let plan = preview(&[work.clone()]);
        assert_eq!(only(&plan).action, "fast_forward");

        // A tracked-file edit dirties the tree after preview (TOCTOU).
        std::fs::write(work.join("file.txt"), "local edit").unwrap();
        let result = &apply(&plan)[0];
        assert_eq!(result.action, "skipped");
        assert_eq!(result.reason.as_deref(), Some("dirty"));
        // Nothing pushed; the local edit is preserved.
        assert_eq!(bare_main(&origin), origin_before);
        assert_eq!(
            std::fs::read_to_string(work.join("file.txt")).unwrap(),
            "local edit"
        );
    }

    #[test]
    fn detached_head_is_skipped() {
        let temp = tempdir().unwrap();
        let (_upstream, _origin, work) = fork_fixture(temp.path());
        git(&work, &["checkout", "--detach", "HEAD"]);

        let plan = preview(&[work.clone()]);
        let item = only(&plan);
        assert_eq!(item.action, "skip");
        assert_eq!(item.reason.as_deref(), Some("detached"));
        assert!(item.branch.is_none());
    }

    #[test]
    fn plan_round_trips_through_serde() {
        // The plan crosses the wire to the frontend and back into apply.
        let plan = ForkSyncPlan {
            items: vec![ForkSyncPreview {
                path: "/tmp/repo".to_string(),
                name: "repo".to_string(),
                branch: Some("main".to_string()),
                origin: Some("https://example.com/fork.git".to_string()),
                upstream: Some("https://example.com/source.git".to_string()),
                source: Some("upstream/main".to_string()),
                target: Some("origin/main".to_string()),
                ahead: 0,
                behind: 3,
                fast_forwardable: true,
                action: "fast_forward".to_string(),
                reason: None,
            }],
            scanned_at: 42,
        };
        let json = serde_json::to_string(&plan).unwrap();
        let back: ForkSyncPlan = serde_json::from_str(&json).unwrap();
        assert_eq!(back.scanned_at, 42);
        assert_eq!(back.items[0].action, "fast_forward");
        assert_eq!(back.items[0].behind, 3);
        assert_eq!(back.items[0].source.as_deref(), Some("upstream/main"));
    }
}
