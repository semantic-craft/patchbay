//! Read-only detection of duplicate Original Repository checkouts.
//!
//! Two checkouts are "the same repository" when their `origin` remote URLs
//! resolve to one normalized identity (`host/path`), never when they merely
//! share a folder name — a fork of `orgA/repo` and a fork of `orgB/repo` are
//! distinct even though both directories are called `repo`. Detection derives
//! entirely from a `ChainTopology` the Chain Service already scanned plus a
//! best-effort HEAD read per checkout, so there is one scanner and no second
//! Git implementation. Nothing here deletes, merges, or picks a winner: it
//! only groups the evidence and emits stable advisory codes a human uses to
//! choose an authority. Output is fully deterministic (groups sorted by
//! identity, checkouts by path, guidance codes sorted).

use std::collections::BTreeMap;

use serde::Serialize;

use super::repo_health;
use super::repo_health::RepoRemote;
use super::warehouse::ProjectRef;
use super::ChainTopology;

/// One checkout that shares a remote identity with at least one other. Carries
/// the evidence a human needs to choose an authority: where it lives, what
/// commit it is on, whether it is dirty, its remotes, and which projects use it.
#[derive(Debug, Clone, Serialize)]
pub struct DuplicateCheckout {
    /// Directory name of the checkout (not its identity — same-named checkouts
    /// with different remotes are never grouped together).
    pub name: String,
    /// Canonical checkout path on disk.
    pub path: String,
    /// Short HEAD sha (12 hex chars), or `None` when HEAD cannot be read.
    pub revision: Option<String>,
    /// Tracked-file dirtiness copied from the repository's scanned health.
    pub dirty: bool,
    /// Tracking state token copied from the repository's scanned health.
    pub state: String,
    /// Current branch, when HEAD is on one.
    pub branch: Option<String>,
    /// `origin` remote identity (the one this grouping keys on).
    pub origin: Option<RepoRemote>,
    /// `upstream` remote identity, shown distinctly so a fork's source is visible.
    pub upstream: Option<RepoRemote>,
    /// Registered projects that currently depend on this checkout.
    pub referenced_by: Vec<ProjectRef>,
}

/// A set of two or more checkouts that resolve to one normalized remote
/// identity, with deterministic advisory codes but no chosen authority.
#[derive(Debug, Clone, Serialize)]
pub struct DuplicateGroup {
    /// Normalized remote identity shared by every checkout, e.g.
    /// `github.com/org/repo`.
    pub identity: String,
    /// The duplicate checkouts, sorted by path.
    pub checkouts: Vec<DuplicateCheckout>,
    /// Stable, non-localized advisory codes derived from the evidence. Sorted.
    /// Never includes a delete/merge/winner recommendation.
    pub guidance: Vec<String>,
}

/// Every duplicate group found in one scan, plus the scan clock so the report
/// and Link Topology share one timestamp.
#[derive(Debug, Clone, Serialize)]
pub struct DuplicatesReport {
    /// Duplicate groups, sorted by identity. Empty when nothing is duplicated.
    pub groups: Vec<DuplicateGroup>,
    /// Copied from the topology so the report and Link Topology share one clock.
    pub scanned_at: i64,
}

/// Normalize a git remote URL to a `host/path` identity, or `None` when the URL
/// is not a recognized remote form (empty, a bare local path, or `file://…`).
///
/// Recognizes `scheme://[user@]host/path` (https, http, ssh, git) and scp-like
/// `git@host:org/repo`. The host is lowercased; the path has a leading `/`, a
/// trailing `/`, and a trailing `.git` stripped. Returns `Some(host/path)` only
/// when both parts are non-empty, so anything that does not clearly identify a
/// remote is left ungrouped (avoiding false positives). These all normalize
/// equal — `https://github.com/org/repo.git`, `https://github.com/org/repo`,
/// `git@github.com:org/repo.git`, `ssh://git@github.com/org/repo.git` →
/// `github.com/org/repo` — while `https://github.com/org/other` differs.
pub fn normalize_remote_url(url: &str) -> Option<String> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }

    let (host, raw_path) = if let Some((_scheme, rest)) = url.split_once("://") {
        // scheme://[user@]host/path
        let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
        let host = authority.rsplit('@').next().unwrap_or(authority);
        (host, path)
    } else if let Some((authority, path)) = url.split_once(':') {
        // scp-like git@host:org/repo. Guard against bare local paths and drive
        // letters: only treat as a remote when the authority carries userinfo
        // (`git@…`) or the host looks like a domain (contains a dot).
        let host = authority.rsplit('@').next().unwrap_or(authority);
        if !authority.contains('@') && !host.contains('.') {
            return None;
        }
        (host, path)
    } else {
        // A bare local filesystem path or anything else unrecognized.
        return None;
    };

    let host = host.trim().to_lowercase();
    let path = raw_path
        .trim()
        .trim_start_matches('/')
        .trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    if host.is_empty() || path.is_empty() {
        return None;
    }
    Some(format!("{host}/{path}"))
}

/// Detect duplicate Original Repository checkouts in an already-scanned topology.
///
/// Read-only: it groups `topo.repos` by their normalized `origin` identity and
/// reads each checkout's HEAD best-effort; it never mutates disk or Git. Only
/// repositories whose `origin` normalizes to `Some` participate — a checkout
/// with no origin, or one whose origin is a local path, is never grouped. A
/// group is reported only when it holds two or more checkouts. Output is
/// deterministic: groups sorted by identity, checkouts by path.
pub fn detect(topo: &ChainTopology) -> DuplicatesReport {
    // BTreeMap keeps identities in sorted order without a later sort pass.
    let mut by_identity: BTreeMap<String, Vec<DuplicateCheckout>> = BTreeMap::new();

    for repo in &topo.repos {
        let Some(origin) = repo.origin.as_ref() else {
            continue;
        };
        let Some(identity) = normalize_remote_url(&origin.url) else {
            continue;
        };
        by_identity
            .entry(identity)
            .or_default()
            .push(checkout_of(repo));
    }

    let groups = by_identity
        .into_iter()
        // A single checkout of an identity is not a duplicate.
        .filter(|(_, checkouts)| checkouts.len() >= 2)
        .map(|(identity, mut checkouts)| {
            checkouts.sort_by(|a, b| a.path.cmp(&b.path));
            let guidance = guidance_for(&checkouts);
            DuplicateGroup {
                identity,
                checkouts,
                guidance,
            }
        })
        .collect();

    DuplicatesReport {
        groups,
        scanned_at: topo.scanned_at,
    }
}

/// Project one scanned repository into a duplicate-checkout record, reading its
/// HEAD revision best-effort (the only fresh Git read this module performs).
fn checkout_of(repo: &super::warehouse::RepoInfo) -> DuplicateCheckout {
    DuplicateCheckout {
        name: repo.name.clone(),
        path: repo.path.clone(),
        revision: repo_health::head_revision(std::path::Path::new(&repo.path)),
        dirty: repo.health.dirty,
        state: repo.health.state.clone(),
        branch: repo.health.branch.clone(),
        origin: repo.origin.clone(),
        upstream: repo.upstream.clone(),
        referenced_by: repo.referenced_by.clone(),
    }
}

/// Derive stable, sorted advisory codes from a group's checkouts. These describe
/// the evidence only; none recommends deleting, merging, or picking a winner.
fn guidance_for(checkouts: &[DuplicateCheckout]) -> Vec<String> {
    let mut codes = Vec::new();

    // Dirtiness across the group.
    if checkouts.iter().any(|c| c.dirty) {
        codes.push("some_dirty".to_string());
    } else {
        codes.push("all_clean".to_string());
    }

    // Referencing projects across the group.
    let referenced = checkouts
        .iter()
        .filter(|c| !c.referenced_by.is_empty())
        .count();
    if referenced == 0 {
        codes.push("none_referenced".to_string());
    } else if referenced < checkouts.len() {
        codes.push("some_unreferenced".to_string());
    }

    // Distinct known revisions: two readable HEADs that differ mean the
    // checkouts are not on the same commit, which is decisive evidence.
    let mut revisions: Vec<&str> = checkouts
        .iter()
        .filter_map(|c| c.revision.as_deref())
        .collect();
    revisions.sort_unstable();
    revisions.dedup();
    if revisions.len() >= 2 {
        codes.push("differing_revisions".to_string());
    }

    codes.sort();
    codes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::chain::repo_health::{RepoHealth, RepoRemote};
    use crate::core::chain::warehouse::{ProjectRef, RepoInfo};

    /// A clean, up-to-date working tree — grouping keys off remote identity, not
    /// repo state, so these hand-built fixtures assume healthy checkouts.
    fn clean_health() -> RepoHealth {
        RepoHealth {
            dirty: false,
            state: "up_to_date".to_string(),
            ahead: 0,
            behind: 0,
            branch: None,
            error: None,
        }
    }

    /// A repository with an `origin` URL and no reverse references. `path` is its
    /// identity; `name` is the directory basename.
    fn repo(name: &str, path: &str, origin: Option<&str>) -> RepoInfo {
        RepoInfo {
            name: name.to_string(),
            path: path.to_string(),
            source_kind: "checkout".to_string(),
            root: "/wh".to_string(),
            health: clean_health(),
            origin: origin.map(|url| RepoRemote {
                name: "origin".to_string(),
                url: url.to_string(),
            }),
            upstream: None,
            skills: Vec::new(),
            referenced_by: Vec::new(),
        }
    }

    fn topo_with(repos: Vec<RepoInfo>) -> ChainTopology {
        ChainTopology {
            warehouse_roots: Vec::new(),
            projects_root: "/proj".to_string(),
            repos,
            projects: Vec::new(),
            guard: Vec::new(),
            scanned_at: 0,
        }
    }

    #[test]
    fn url_aliases_normalize_to_one_identity() {
        let identity = "github.com/org/repo";
        for url in [
            "https://github.com/org/repo.git",
            "https://github.com/org/repo",
            "https://github.com/org/repo/",
            "git@github.com:org/repo.git",
            "git@github.com:org/repo",
            "ssh://git@github.com/org/repo.git",
            "  https://GitHub.com/org/repo.git  ",
        ] {
            assert_eq!(
                normalize_remote_url(url).as_deref(),
                Some(identity),
                "url {url} should normalize to {identity}"
            );
        }
    }

    #[test]
    fn different_repositories_do_not_normalize_equal() {
        assert_ne!(
            normalize_remote_url("https://github.com/org/repo"),
            normalize_remote_url("https://github.com/org/other")
        );
        // Same repo name under different orgs is a different identity.
        assert_ne!(
            normalize_remote_url("https://github.com/orgA/repo"),
            normalize_remote_url("https://github.com/orgB/repo")
        );
    }

    #[test]
    fn local_and_empty_urls_are_not_identities() {
        for url in [
            "",
            "   ",
            "/Users/me/Projects/repo",
            "../sibling/repo",
            "file:///Users/me/Projects/repo",
            "not-a-url",
        ] {
            assert_eq!(
                normalize_remote_url(url),
                None,
                "url {url:?} must not be a remote identity"
            );
        }
    }

    #[test]
    fn true_duplicates_group_across_url_aliases() {
        let topo = topo_with(vec![
            repo(
                "repo",
                "/wh/a/repo",
                Some("https://github.com/org/repo.git"),
            ),
            repo(
                "checkout-two",
                "/wh/b/repo",
                Some("git@github.com:org/repo.git"),
            ),
        ]);
        let report = detect(&topo);
        assert_eq!(report.groups.len(), 1);
        let group = &report.groups[0];
        assert_eq!(group.identity, "github.com/org/repo");
        assert_eq!(group.checkouts.len(), 2);
        // Sorted by path.
        assert_eq!(group.checkouts[0].path, "/wh/a/repo");
        assert_eq!(group.checkouts[1].path, "/wh/b/repo");
    }

    #[test]
    fn same_name_different_remote_is_not_a_duplicate() {
        // Two checkouts both named "repo" but forks of different orgs must not be
        // reported as duplicates — grouping is by identity, not folder name.
        let topo = topo_with(vec![
            repo(
                "repo",
                "/wh/a/repo",
                Some("https://github.com/orgA/repo.git"),
            ),
            repo(
                "repo",
                "/wh/b/repo",
                Some("https://github.com/orgB/repo.git"),
            ),
        ]);
        assert!(detect(&topo).groups.is_empty());
    }

    #[test]
    fn repo_without_origin_is_never_grouped() {
        let topo = topo_with(vec![
            repo(
                "repo",
                "/wh/a/repo",
                Some("https://github.com/org/repo.git"),
            ),
            repo("repo", "/wh/b/repo", None),
        ]);
        // Only one checkout has a normalizable origin, so no duplicate group.
        assert!(detect(&topo).groups.is_empty());
    }

    #[test]
    fn origin_that_is_a_local_path_is_never_grouped() {
        let topo = topo_with(vec![
            repo("repo", "/wh/a/repo", Some("/srv/mirror/repo.git")),
            repo("repo", "/wh/b/repo", Some("/srv/mirror/repo.git")),
        ]);
        // Identical local-path origins are still not remote identities.
        assert!(detect(&topo).groups.is_empty());
    }

    #[test]
    fn guidance_reports_dirty_and_unreferenced_evidence() {
        let mut dirty = repo(
            "repo",
            "/wh/a/repo",
            Some("https://github.com/org/repo.git"),
        );
        dirty.health.dirty = true;
        dirty.referenced_by = vec![ProjectRef {
            name: "consumer".to_string(),
            path: "/proj/consumer".to_string(),
        }];
        // The second checkout is clean and referenced by nothing.
        let clean = repo("repo", "/wh/b/repo", Some("git@github.com:org/repo.git"));

        let report = detect(&topo_with(vec![dirty, clean]));
        assert_eq!(report.groups.len(), 1);
        let guidance = &report.groups[0].guidance;
        assert!(guidance.contains(&"some_dirty".to_string()), "{guidance:?}");
        assert!(
            guidance.contains(&"some_unreferenced".to_string()),
            "{guidance:?}"
        );
        // No delete/merge/winner code is ever emitted.
        assert!(!guidance
            .iter()
            .any(|c| c.contains("delete") || c.contains("merge")));
        // Hand-built fixtures have unreadable HEADs, so no revision claim.
        assert!(!guidance.contains(&"differing_revisions".to_string()));
        // Deterministic sorted order.
        let mut sorted = guidance.clone();
        sorted.sort();
        assert_eq!(guidance, &sorted);
    }

    #[test]
    fn guidance_reports_all_clean_and_none_referenced() {
        let topo = topo_with(vec![
            repo(
                "repo",
                "/wh/a/repo",
                Some("https://github.com/org/repo.git"),
            ),
            repo("repo", "/wh/b/repo", Some("git@github.com:org/repo.git")),
        ]);
        let report = detect(&topo);
        let guidance = &report.groups[0].guidance;
        assert!(guidance.contains(&"all_clean".to_string()), "{guidance:?}");
        assert!(
            guidance.contains(&"none_referenced".to_string()),
            "{guidance:?}"
        );
    }
}
