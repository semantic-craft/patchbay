//! Read-only Git health of one Original Repository (tier 1).
//!
//! Reuses the existing `git2` engine — no second Git implementation and no
//! system-`git` shelling. Every inspection is best-effort: a repository that
//! cannot be read yields `state == "scan_error"` with the reason instead of
//! propagating, so one bad checkout never aborts the surrounding warehouse
//! scan. Nothing here mutates the repository — this ticket is read-only; pull
//! and fork-sync live in later tickets.

use serde::Serialize;
use std::path::Path;

/// A configured Git remote's identity. Carries just enough to show `origin`
/// and an optional `upstream` distinctly in the Original Repositories work
/// area.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RepoRemote {
    /// Remote name as configured (`origin` / `upstream`).
    pub name: String,
    /// Fetch URL, or empty when the remote has no URL configured.
    pub url: String,
}

/// Read-only health of one repository's Git state.
///
/// Two independent axes are reported: working-tree cleanliness (`dirty`) and
/// the current branch's position against its configured upstream tracking
/// branch (`state` plus `ahead`/`behind`). They are orthogonal — a repository
/// can be both dirty and ahead.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RepoHealth {
    /// Tracked-file changes only (`git status -uno` semantics): untracked
    /// files do not block a fast-forward pull, matching pull-skill-repos.
    pub dirty: bool,
    /// Position of the current branch against its configured upstream:
    /// `"up_to_date" | "ahead" | "behind" | "diverged" | "no_upstream" |
    /// "detached" | "scan_error"`.
    pub state: String,
    /// Commits the local branch has that its upstream does not.
    pub ahead: usize,
    /// Commits the upstream has that the local branch does not.
    pub behind: usize,
    /// Current local branch, when HEAD is on one.
    pub branch: Option<String>,
    /// Reason, populated only when `state == "scan_error"`.
    pub error: Option<String>,
}

impl RepoHealth {
    fn scan_error(reason: impl Into<String>) -> Self {
        RepoHealth {
            dirty: false,
            state: "scan_error".to_string(),
            ahead: 0,
            behind: 0,
            branch: None,
            error: Some(reason.into()),
        }
    }
}

/// One repository's read-only Git status: working-tree/tracking health plus the
/// `origin` and optional `upstream` remote identities.
#[derive(Debug, Clone, Serialize)]
pub struct RepoGitStatus {
    pub health: RepoHealth,
    pub origin: Option<RepoRemote>,
    pub upstream: Option<RepoRemote>,
}

/// Inspect a repository's read-only health and remote identities.
///
/// Never panics and never mutates: any git2 failure collapses to a
/// `scan_error` health with the message, so callers can surface per-repository
/// problems without failing the whole scan.
pub fn inspect(repo_path: &Path) -> RepoGitStatus {
    let repo = match git2::Repository::open(repo_path) {
        Ok(repo) => repo,
        Err(e) => {
            return RepoGitStatus {
                health: RepoHealth::scan_error(e.message().to_string()),
                origin: None,
                upstream: None,
            }
        }
    };
    RepoGitStatus {
        health: health(&repo),
        origin: remote_identity(&repo, "origin"),
        upstream: remote_identity(&repo, "upstream"),
    }
}

/// One tracked file with uncommitted changes (issue #34's feedback card
/// evidence). Line counts cover staged + unstaged edits against HEAD.
#[derive(Debug, Clone, Serialize)]
pub struct DirtyFile {
    /// Repo-relative path (the NEW path for a rename).
    pub path: String,
    /// "added" | "modified" | "deleted" | "renamed" | "typechange" | "other"
    pub status: String,
    pub additions: usize,
    pub deletions: usize,
}

/// The uncommitted-change evidence of one repository: tracked files only
/// (`git status -uno` semantics, mirroring [`RepoHealth::dirty`]), bounded.
#[derive(Debug, Clone, Serialize)]
pub struct DirtyDiff {
    pub repo: String,
    pub files: Vec<DirtyFile>,
    /// True when the file list was cut at the cap.
    pub truncated: bool,
}

/// Files listed per diff before truncation — evidence, not a full diff tool.
const DIRTY_DIFF_MAX_FILES: usize = 100;

/// The uncommitted tracked changes of a repository, as per-file evidence with
/// line stats. Read-only and never panics: any git2 failure yields an empty
/// report rather than propagating (the card simply shows no file evidence).
/// Untracked files are excluded to mirror the `dirty` flag's semantics.
pub fn dirty_diff(repo_path: &Path) -> DirtyDiff {
    let repo_str = repo_path.to_string_lossy().to_string();
    let empty = DirtyDiff {
        repo: repo_str.clone(),
        files: Vec::new(),
        truncated: false,
    };
    let Ok(repo) = git2::Repository::open(repo_path) else {
        return empty;
    };
    // HEAD tree → workdir WITH index: staged and unstaged edits both count,
    // matching what a commit would pick up.
    let head_tree = repo.head().ok().and_then(|head| head.peel_to_tree().ok());
    let mut opts = git2::DiffOptions::new();
    opts.include_untracked(false);
    let Ok(diff) = repo.diff_tree_to_workdir_with_index(head_tree.as_ref(), Some(&mut opts)) else {
        return empty;
    };

    let total = diff.deltas().len();
    let mut files = Vec::new();
    for (index, delta) in diff.deltas().enumerate() {
        if files.len() >= DIRTY_DIFF_MAX_FILES {
            break;
        }
        let path = delta
            .new_file()
            .path()
            .or_else(|| delta.old_file().path())
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let status = match delta.status() {
            git2::Delta::Added => "added",
            git2::Delta::Modified => "modified",
            git2::Delta::Deleted => "deleted",
            git2::Delta::Renamed => "renamed",
            git2::Delta::Typechange => "typechange",
            _ => "other",
        };
        // Per-file line stats; a binary or unreadable patch reports 0/0.
        let (additions, deletions) = git2::Patch::from_diff(&diff, index)
            .ok()
            .flatten()
            .and_then(|patch| patch.line_stats().ok())
            .map(|(_, adds, dels)| (adds, dels))
            .unwrap_or((0, 0));
        files.push(DirtyFile {
            path,
            status: status.to_string(),
            additions,
            deletions,
        });
    }
    DirtyDiff {
        repo: repo_str,
        files,
        truncated: total > DIRTY_DIFF_MAX_FILES,
    }
}

/// Short HEAD revision (first 12 hex chars of the commit sha) of a repository,
/// or `None` when it cannot be resolved.
///
/// Read-only and best-effort: any failure — the path is not a repository, HEAD
/// is unborn, or HEAD is detached without a resolvable commit — yields `None`
/// rather than propagating, so callers can note "revision unknown" without
/// aborting a surrounding scan. Nothing here mutates the repository.
pub fn head_revision(repo_path: &Path) -> Option<String> {
    let repo = git2::Repository::open(repo_path).ok()?;
    let oid = repo.head().ok()?.target()?;
    Some(oid.to_string().chars().take(12).collect())
}

/// The working-tree + tracking health of an opened repository.
fn health(repo: &git2::Repository) -> RepoHealth {
    let tracking = tracking(repo);
    RepoHealth {
        dirty: is_dirty(repo),
        state: tracking.state,
        ahead: tracking.ahead,
        behind: tracking.behind,
        branch: tracking.branch,
        error: tracking.error,
    }
}

/// The current branch's position against its configured upstream.
struct Tracking {
    state: String,
    ahead: usize,
    behind: usize,
    branch: Option<String>,
    error: Option<String>,
}

impl Tracking {
    fn plain(state: &str, ahead: usize, behind: usize, branch: Option<String>) -> Self {
        Tracking {
            state: state.to_string(),
            ahead,
            behind,
            branch,
            error: None,
        }
    }
}

/// Tracked-file dirtiness (`git status --porcelain -uno`): untracked and
/// ignored files are excluded so they never mark a repo dirty.
fn is_dirty(repo: &git2::Repository) -> bool {
    let mut opts = git2::StatusOptions::new();
    opts.include_untracked(false).include_ignored(false);
    match repo.statuses(Some(&mut opts)) {
        Ok(statuses) => !statuses.is_empty(),
        Err(_) => false,
    }
}

/// Classify the current branch against its configured upstream tracking branch
/// (normally `origin/<branch>`).
fn tracking(repo: &git2::Repository) -> Tracking {
    // A detached HEAD has no branch to track. Reported explicitly rather than
    // guessed at.
    if matches!(repo.head_detached(), Ok(true)) {
        return Tracking::plain("detached", 0, 0, None);
    }
    let head = match repo.head() {
        Ok(head) => head,
        // An unborn branch (freshly initialized, no commits) has nothing to
        // compare against; treat as missing tracking rather than an error.
        Err(e) if e.code() == git2::ErrorCode::UnbornBranch => {
            return Tracking::plain("no_upstream", 0, 0, None)
        }
        Err(e) => {
            return Tracking {
                state: "scan_error".to_string(),
                ahead: 0,
                behind: 0,
                branch: None,
                error: Some(e.message().to_string()),
            }
        }
    };
    let branch_name = head.shorthand().map(str::to_string);
    let Some(local_oid) = head.target() else {
        return Tracking::plain("no_upstream", 0, 0, branch_name);
    };
    let Some(name) = branch_name.as_deref() else {
        return Tracking::plain("no_upstream", 0, 0, branch_name);
    };
    let local_branch = match repo.find_branch(name, git2::BranchType::Local) {
        Ok(branch) => branch,
        Err(_) => return Tracking::plain("no_upstream", 0, 0, branch_name),
    };
    // No configured upstream tracking branch: "missing tracking".
    let upstream = match local_branch.upstream() {
        Ok(upstream) => upstream,
        Err(_) => return Tracking::plain("no_upstream", 0, 0, branch_name),
    };
    let Some(upstream_oid) = upstream.get().target() else {
        return Tracking::plain("no_upstream", 0, 0, branch_name);
    };

    match repo.graph_ahead_behind(local_oid, upstream_oid) {
        Ok((ahead, behind)) => {
            let state = match (ahead, behind) {
                (0, 0) => "up_to_date",
                (_, 0) => "ahead",
                (0, _) => "behind",
                (_, _) => "diverged",
            };
            Tracking::plain(state, ahead, behind, branch_name)
        }
        Err(e) => Tracking {
            state: "scan_error".to_string(),
            ahead: 0,
            behind: 0,
            branch: branch_name,
            error: Some(e.message().to_string()),
        },
    }
}

/// Read one remote's identity, or `None` when it is not configured.
fn remote_identity(repo: &git2::Repository, name: &str) -> Option<RepoRemote> {
    let remote = repo.find_remote(name).ok()?;
    Some(RepoRemote {
        name: name.to_string(),
        url: remote.url().unwrap_or_default().to_string(),
    })
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

    #[test]
    fn dirty_diff_lists_tracked_changes_with_line_stats_and_skips_untracked() {
        let temp = tempdir().unwrap();
        let (_remote, work) = remote_and_clone(temp.path());

        // One tracked modification, one staged addition, one untracked file.
        std::fs::write(work.join("file.txt"), "base\nplus\n").unwrap();
        std::fs::write(work.join("staged.txt"), "one\ntwo\n").unwrap();
        git(&work, &["add", "staged.txt"]);
        std::fs::write(work.join("untracked.txt"), "noise").unwrap();

        let diff = dirty_diff(&work);
        assert!(!diff.truncated);
        let paths: Vec<&str> = diff.files.iter().map(|f| f.path.as_str()).collect();
        assert!(paths.contains(&"file.txt"));
        assert!(paths.contains(&"staged.txt"));
        assert!(
            !paths.contains(&"untracked.txt"),
            "untracked noise mirrors the dirty flag's -uno semantics"
        );

        let modified = diff.files.iter().find(|f| f.path == "file.txt").unwrap();
        assert_eq!(modified.status, "modified");
        // The seed's "base" has no trailing newline, so rewriting it counts as
        // -1/+2: what matters is that per-file line stats ride along.
        assert_eq!((modified.additions, modified.deletions), (2, 1));
        let staged = diff.files.iter().find(|f| f.path == "staged.txt").unwrap();
        assert_eq!(staged.status, "added");
        assert_eq!((staged.additions, staged.deletions), (2, 0));
    }

    #[test]
    fn dirty_diff_of_a_clean_checkout_is_empty_and_a_non_repo_never_fails() {
        let temp = tempdir().unwrap();
        let (_remote, work) = remote_and_clone(temp.path());
        assert!(dirty_diff(&work).files.is_empty());

        let not_a_repo = temp.path().join("plain");
        std::fs::create_dir_all(&not_a_repo).unwrap();
        let diff = dirty_diff(&not_a_repo);
        assert!(diff.files.is_empty() && !diff.truncated);
    }

    #[test]
    fn clean_checkout_is_up_to_date_with_origin() {
        let temp = tempdir().unwrap();
        let (_remote, work) = remote_and_clone(temp.path());

        let status = inspect(&work);
        assert_eq!(status.health.state, "up_to_date");
        assert!(!status.health.dirty);
        assert_eq!(status.health.ahead, 0);
        assert_eq!(status.health.behind, 0);
        assert_eq!(status.health.branch.as_deref(), Some("main"));
        assert_eq!(status.health.error, None);
        // origin identity is shown; no upstream configured.
        let origin = status.origin.expect("origin remote");
        assert_eq!(origin.name, "origin");
        assert!(origin.url.contains("remote.git"), "url: {}", origin.url);
        assert!(status.upstream.is_none());
    }

    #[test]
    fn tracked_change_is_dirty_but_untracked_is_not() {
        let temp = tempdir().unwrap();
        let (_remote, work) = remote_and_clone(temp.path());

        // Untracked file alone must not mark the repo dirty (ff-pull semantics).
        std::fs::write(work.join("untracked.txt"), "new").unwrap();
        assert!(!inspect(&work).health.dirty);

        // Editing a tracked file does.
        std::fs::write(work.join("file.txt"), "changed").unwrap();
        let status = inspect(&work);
        assert!(status.health.dirty);
        // Dirtiness is orthogonal to tracking state.
        assert_eq!(status.health.state, "up_to_date");
    }

    #[test]
    fn local_commit_is_ahead_of_origin() {
        let temp = tempdir().unwrap();
        let (_remote, work) = remote_and_clone(temp.path());

        std::fs::write(work.join("file.txt"), "local work").unwrap();
        git(&work, &["commit", "-am", "local"]);

        let health = inspect(&work).health;
        assert_eq!(health.state, "ahead");
        assert_eq!(health.ahead, 1);
        assert_eq!(health.behind, 0);
    }

    #[test]
    fn fetched_remote_commit_is_behind_origin() {
        let temp = tempdir().unwrap();
        let (remote, work) = remote_and_clone(temp.path());

        // A second clone pushes a new commit to the shared remote.
        let other = temp.path().join("other");
        assert!(Command::new("git")
            .args(["clone"])
            .arg(&remote)
            .arg(&other)
            .output()
            .unwrap()
            .status
            .success());
        std::fs::write(other.join("file.txt"), "from other").unwrap();
        git(&other, &["commit", "-am", "other"]);
        git(&other, &["push", "origin", "main"]);

        // work fetches (updating origin/main) but does not merge.
        git(&work, &["fetch", "origin"]);
        let health = inspect(&work).health;
        assert_eq!(health.state, "behind");
        assert_eq!(health.ahead, 0);
        assert_eq!(health.behind, 1);
    }

    #[test]
    fn local_and_remote_commits_diverge() {
        let temp = tempdir().unwrap();
        let (remote, work) = remote_and_clone(temp.path());

        let other = temp.path().join("other");
        assert!(Command::new("git")
            .args(["clone"])
            .arg(&remote)
            .arg(&other)
            .output()
            .unwrap()
            .status
            .success());
        std::fs::write(other.join("file.txt"), "from other").unwrap();
        git(&other, &["commit", "-am", "other"]);
        git(&other, &["push", "origin", "main"]);

        // work commits its own change, then fetches the diverging remote tip.
        std::fs::write(work.join("file.txt"), "from work").unwrap();
        git(&work, &["commit", "-am", "work"]);
        git(&work, &["fetch", "origin"]);

        let health = inspect(&work).health;
        assert_eq!(health.state, "diverged");
        assert_eq!(health.ahead, 1);
        assert_eq!(health.behind, 1);
    }

    #[test]
    fn branch_without_upstream_reports_missing_tracking() {
        let temp = tempdir().unwrap();
        let (_remote, work) = remote_and_clone(temp.path());

        // A fresh local branch has no configured upstream.
        git(&work, &["checkout", "-b", "feature"]);
        let health = inspect(&work).health;
        assert_eq!(health.state, "no_upstream");
        assert_eq!(health.branch.as_deref(), Some("feature"));
        assert_eq!(health.ahead, 0);
        assert_eq!(health.behind, 0);
    }

    #[test]
    fn origin_and_upstream_remotes_are_distinct() {
        let temp = tempdir().unwrap();
        let (_remote, work) = remote_and_clone(temp.path());

        // A fork gains an `upstream` remote pointing at its source.
        let source = temp.path().join("source.git");
        assert!(Command::new("git")
            .args(["init", "--bare"])
            .arg(&source)
            .output()
            .unwrap()
            .status
            .success());
        git(
            &work,
            &["remote", "add", "upstream", source.to_str().unwrap()],
        );

        let status = inspect(&work);
        let origin = status.origin.expect("origin");
        let upstream = status.upstream.expect("upstream");
        assert_eq!(origin.name, "origin");
        assert_eq!(upstream.name, "upstream");
        assert!(origin.url.contains("remote.git"));
        assert!(upstream.url.contains("source.git"));
        assert_ne!(origin.url, upstream.url);
    }

    #[test]
    fn detached_head_is_reported() {
        let temp = tempdir().unwrap();
        let (_remote, work) = remote_and_clone(temp.path());
        git(&work, &["checkout", "--detach", "HEAD"]);

        let health = inspect(&work).health;
        assert_eq!(health.state, "detached");
        assert!(health.branch.is_none());
    }

    #[test]
    fn head_revision_returns_short_sha_after_a_commit() {
        let temp = tempdir().unwrap();
        let (_remote, work) = remote_and_clone(temp.path());

        let revision = head_revision(&work).expect("HEAD revision after a commit");
        assert_eq!(revision.len(), 12, "short sha is 12 hex chars: {revision}");
        assert!(
            revision.chars().all(|c| c.is_ascii_hexdigit()),
            "hex: {revision}"
        );
    }

    #[test]
    fn head_revision_is_none_for_non_repository() {
        let temp = tempdir().unwrap();
        let plain = temp.path().join("not-a-repo");
        std::fs::create_dir_all(&plain).unwrap();
        assert!(head_revision(&plain).is_none());
    }

    #[test]
    fn non_repository_is_a_scan_error() {
        let temp = tempdir().unwrap();
        let plain = temp.path().join("not-a-repo");
        std::fs::create_dir_all(&plain).unwrap();

        let status = inspect(&plain);
        assert_eq!(status.health.state, "scan_error");
        assert!(status.health.error.is_some());
        assert!(status.origin.is_none());
        assert!(status.upstream.is_none());
    }
}
