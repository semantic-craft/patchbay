//! Locate relink candidates for broken chain links (issue #30).
//!
//! A broken finding's evidence names the dead target; this module answers
//! "where did that Skill go?" with deterministic, bounded evidence:
//!
//! * **git_rename** — the repository that used to contain the dead path has a
//!   recent commit renaming it (the strongest clue, with the commit time).
//! * **same_name** — an identically named Original Skill scanned elsewhere in
//!   the topology (the whole-repo-moved case).
//! * **similar_name** — a near-named sibling (`ppt-master` → `ppt-master-v2`),
//!   scored by normalized edit distance.
//!
//! Everything is read-only and best-effort: Git failures degrade to "no clue",
//! never to an error, so candidate location can run inside a scan-shaped call.
//! The repair planner consumes [`best_relink_target`]; the Doctor evidence card
//! consumes [`locate`] through `ChainService::locate_candidates`.

use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::doctor::{Deviation, Finding};
use super::ChainTopology;

/// Minimum score a candidate needs before the repair planner will relink a
/// broken entry to it instead of falling back to removal. High enough that a
/// vaguely similar name (< ~3/4 of characters shared) never drives a write.
pub const RELINK_THRESHOLD: u8 = 75;
/// Candidates reported per finding, best first.
const MAX_CANDIDATES: usize = 3;
/// Commits walked (newest first) when looking for a Git rename of the dead
/// path. Bounds the cost per broken finding on large repositories.
const MAX_COMMITS: usize = 50;
/// Similarity floor below which a near-name is noise, not evidence.
const MIN_SIMILARITY: f64 = 0.6;

/// One place the missing Original may have gone, with deterministic evidence.
#[derive(Debug, Clone, Serialize)]
pub struct Candidate {
    /// Absolute path of the candidate Original Skill directory.
    pub path: String,
    /// The candidate's directory name.
    pub name: String,
    /// Confidence 0–100: 98 git_rename, 95 same_name, else the rounded
    /// name-similarity capped at 90 so a near-name never outranks an exact one.
    pub score: u8,
    /// "git_rename" | "same_name" | "similar_name"
    pub reason: String,
    /// Commit time (unix seconds) of the rename; `git_rename` only.
    pub renamed_at: Option<i64>,
}

/// Wire shape of `chain_locate_candidates`: per-fingerprint candidates from
/// one fresh scan. Read-only evidence — deliberately NOT part of the repair
/// plan, whose `evidence` map stays a pure TOCTOU baseline.
#[derive(Debug, Clone, Serialize)]
pub struct CandidatesReport {
    /// Finding fingerprint → candidates, best first. A requested fingerprint
    /// with no current broken finding or no plausible target is absent.
    pub candidates: BTreeMap<String, Vec<Candidate>>,
    /// Scan clock the evidence was derived from.
    pub scanned_at: i64,
}

/// Candidates for one broken finding, best first (score desc, then path).
/// Empty for a finding that is not `Broken` or has nowhere plausible to point.
pub fn locate(topo: &ChainTopology, finding: &Finding) -> Vec<Candidate> {
    if finding.deviation != Deviation::Broken {
        return Vec::new();
    }
    let dead = PathBuf::from(&finding.evidence.final_target);
    let skill = skill_of(finding);
    if skill.is_empty() {
        return Vec::new();
    }

    // Keyed by path so each location keeps only its best-scoring evidence.
    let mut by_path: BTreeMap<String, Candidate> = BTreeMap::new();
    let mut add = |candidate: Candidate| {
        by_path
            .entry(candidate.path.clone())
            .and_modify(|cur| {
                if candidate.score > cur.score {
                    *cur = candidate.clone();
                }
            })
            .or_insert(candidate);
    };

    if let Some(renamed) = git_rename_candidate(topo, &dead) {
        add(renamed);
    }

    for repo in &topo.repos {
        for repo_skill in &repo.skills {
            if Path::new(&repo_skill.path) == dead.as_path() {
                continue; // the dead location itself is never where it went
            }
            if repo_skill.name == skill {
                add(Candidate {
                    path: repo_skill.path.clone(),
                    name: repo_skill.name.clone(),
                    score: 95,
                    reason: "same_name".to_string(),
                    renamed_at: None,
                });
            } else {
                let sim = similarity(&repo_skill.name, &skill);
                if sim >= MIN_SIMILARITY {
                    add(Candidate {
                        path: repo_skill.path.clone(),
                        name: repo_skill.name.clone(),
                        score: ((sim * 100.0).round() as u8).min(90),
                        reason: "similar_name".to_string(),
                        renamed_at: None,
                    });
                }
            }
        }
    }

    let mut candidates: Vec<Candidate> = by_path.into_values().collect();
    // BTreeMap already yields path order; a stable sort keeps it as tiebreak.
    candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.score));
    candidates.truncate(MAX_CANDIDATES);
    candidates
}

/// The single candidate the repair planner may relink to: the best-scoring one
/// clearing [`RELINK_THRESHOLD`]. With `prefer_root` (a detected repo-move
/// destination, issue #33) a qualifying candidate under that root wins over
/// an equal-scoring one elsewhere — a storm repair must land in the detected
/// new location, not on a same-name tie broken by path order. `None` means a
/// broken entry has no defensible new target and removal stays the only
/// automatic repair.
pub fn best_relink_target(
    topo: &ChainTopology,
    finding: &Finding,
    prefer_root: Option<&str>,
) -> Option<Candidate> {
    let qualifying: Vec<Candidate> = locate(topo, finding)
        .into_iter()
        .filter(|candidate| candidate.score >= RELINK_THRESHOLD)
        .collect();
    if let Some(root) = prefer_root {
        if let Some(preferred) = qualifying
            .iter()
            .find(|candidate| Path::new(&candidate.path).starts_with(root))
        {
            return Some(preferred.clone());
        }
    }
    qualifying.into_iter().next()
}

/// The skill name a finding is about: its "skill" affected object, falling back
/// to the entry path's file name.
pub(super) fn skill_of(finding: &Finding) -> String {
    if let Some(obj) = finding.affected.iter().find(|obj| obj.kind == "skill") {
        return obj.name.clone();
    }
    Path::new(&finding.evidence.entry_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// Search the repository that used to contain `dead` for a commit renaming it.
/// Directory renames surface as per-file renames, so the first delta whose old
/// path lies under the dead directory pins down the new directory. Best-effort:
/// any Git failure yields `None`.
fn git_rename_candidate(topo: &ChainTopology, dead: &Path) -> Option<Candidate> {
    // Longest-prefix match so a nested checkout wins over its parent.
    let repo_info = topo
        .repos
        .iter()
        .filter(|repo| dead.starts_with(&repo.path))
        .max_by_key(|repo| repo.path.len())?;
    let repo_root = Path::new(&repo_info.path);
    let dead_rel = dead.strip_prefix(repo_root).ok()?;

    let repo = git2::Repository::open(repo_root).ok()?;
    let mut walk = repo.revwalk().ok()?;
    walk.push_head().ok()?;

    for oid in walk.take(MAX_COMMITS) {
        let Ok(oid) = oid else { continue };
        let Ok(commit) = repo.find_commit(oid) else {
            continue;
        };
        // Root commits have nothing to diff against; merges diff first-parent.
        let Ok(parent) = commit.parent(0) else {
            continue;
        };
        let (Ok(old_tree), Ok(new_tree)) = (parent.tree(), commit.tree()) else {
            continue;
        };
        let Ok(mut diff) = repo.diff_tree_to_tree(Some(&old_tree), Some(&new_tree), None) else {
            continue;
        };
        let mut find = git2::DiffFindOptions::new();
        find.renames(true);
        if diff.find_similar(Some(&mut find)).is_err() {
            continue;
        }
        for delta in diff.deltas() {
            if delta.status() != git2::Delta::Renamed {
                continue;
            }
            let (Some(old), Some(new)) = (delta.old_file().path(), delta.new_file().path()) else {
                continue;
            };
            // A renamed file under the dead directory: strip the shared inner
            // suffix from the new path to recover where the directory went.
            let Ok(inner) = old.strip_prefix(dead_rel) else {
                continue;
            };
            let Some(new_dir_rel) = strip_suffix_components(new, inner) else {
                continue;
            };
            let new_dir = repo_root.join(&new_dir_rel);
            if !new_dir.is_dir() {
                continue; // renamed again or gone since — not a live target
            }
            return Some(Candidate {
                path: new_dir.to_string_lossy().to_string(),
                name: new_dir
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default(),
                score: 98,
                reason: "git_rename".to_string(),
                renamed_at: Some(commit.time().seconds()),
            });
        }
    }
    None
}

/// `path` minus a trailing `suffix`, component-wise; `None` when it does not
/// end with that suffix. (`skills/x-v2/SKILL.md` − `SKILL.md` = `skills/x-v2`.)
fn strip_suffix_components(path: &Path, suffix: &Path) -> Option<PathBuf> {
    if !path.ends_with(suffix) {
        return None;
    }
    let keep = path.components().count() - suffix.components().count();
    Some(path.components().take(keep).collect())
}

/// Normalized similarity of two names: `1 − levenshtein / max_len`, in [0, 1].
fn similarity(a: &str, b: &str) -> f64 {
    if a == b {
        return 1.0;
    }
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let max_len = a.len().max(b.len());
    if max_len == 0 {
        return 1.0;
    }
    1.0 - (levenshtein(&a, &b) as f64) / (max_len as f64)
}

fn levenshtein(a: &[char], b: &[char]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let subst = prev[j] + usize::from(ca != cb);
            cur[j + 1] = subst.min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::chain::doctor::{AffectedObject, Evidence, Severity};
    use crate::core::chain::repo_health::RepoHealth;
    use crate::core::chain::warehouse::{RepoInfo, RepoSkill};
    use std::process::Command;
    use tempfile::tempdir;

    fn repo_info(path: &Path, skills: &[(&str, PathBuf)]) -> RepoInfo {
        RepoInfo {
            name: path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default(),
            path: path.to_string_lossy().to_string(),
            source_kind: "checkout".to_string(),
            root: path
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default(),
            health: RepoHealth {
                dirty: false,
                state: "no_upstream".to_string(),
                ahead: 0,
                behind: 0,
                branch: None,
                error: None,
            },
            origin: None,
            upstream: None,
            skills: skills
                .iter()
                .map(|(name, path)| RepoSkill {
                    name: (*name).to_string(),
                    path: path.to_string_lossy().to_string(),
                })
                .collect(),
            referenced_by: Vec::new(),
        }
    }

    fn topology(repos: Vec<RepoInfo>) -> ChainTopology {
        ChainTopology {
            warehouse_roots: Vec::new(),
            projects_root: String::new(),
            repos,
            projects: Vec::new(),
            guard: Vec::new(),
            scanned_at: 7,
        }
    }

    fn broken_finding(skill: &str, entry: &Path, dead: &Path) -> Finding {
        Finding {
            rule: "chain.broken_link".to_string(),
            deviation: Deviation::Broken,
            severity: Severity::Violation,
            evidence: Evidence {
                entry_path: entry.to_string_lossy().to_string(),
                hops: Vec::new(),
                final_target: dead.to_string_lossy().to_string(),
                topology_status: "broken".to_string(),
            },
            affected: vec![AffectedObject {
                kind: "skill".to_string(),
                name: skill.to_string(),
                path: entry.to_string_lossy().to_string(),
            }],
            actions: Vec::new(),
            fingerprint: format!("fp-{skill}"),
        }
    }

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

    #[test]
    fn same_name_elsewhere_is_the_top_candidate() {
        let temp = tempdir().unwrap();
        let dead = temp.path().join("old-repo/skills/demo-skill");
        let moved = temp.path().join("new-repo/skills/demo-skill");
        std::fs::create_dir_all(&moved).unwrap();
        let repo = temp.path().join("new-repo");
        let topo = topology(vec![repo_info(&repo, &[("demo-skill", moved.clone())])]);
        let finding = broken_finding("demo-skill", &temp.path().join("entry"), &dead);

        let found = locate(&topo, &finding);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].reason, "same_name");
        assert_eq!(found[0].score, 95);
        assert_eq!(found[0].path, moved.to_string_lossy());

        let best = best_relink_target(&topo, &finding, None).expect("clears the threshold");
        assert_eq!(best.path, moved.to_string_lossy());
    }

    #[test]
    fn near_name_scores_by_edit_distance_and_far_name_is_dropped() {
        let temp = tempdir().unwrap();
        let repo = temp.path().join("repo");
        let near = repo.join("skills/ppt-master-v2");
        let far = repo.join("skills/zotero");
        let topo = topology(vec![repo_info(
            &repo,
            &[("ppt-master-v2", near.clone()), ("zotero", far)],
        )]);
        let dead = repo.join("skills/ppt-master");
        let finding = broken_finding("ppt-master", &temp.path().join("entry"), &dead);

        let found = locate(&topo, &finding);
        assert_eq!(found.len(), 1, "zotero is below the similarity floor");
        assert_eq!(found[0].reason, "similar_name");
        // levenshtein("ppt-master", "ppt-master-v2") = 3 over max len 13 → 77.
        assert_eq!(found[0].score, 77);

        // 77 clears the relink threshold; the planner may point at it.
        assert!(best_relink_target(&topo, &finding, None).is_some());
    }

    #[test]
    fn prefer_root_breaks_a_same_name_tie_toward_the_detected_move() {
        let temp = tempdir().unwrap();
        // Two same-name candidates (score tie); without a preference the
        // path-sorted first wins, with one the storm destination wins.
        let central = temp.path().join("a-central/demo-skill");
        let moved = temp.path().join("z-moved-repo/skills/demo-skill");
        std::fs::create_dir_all(&central).unwrap();
        std::fs::create_dir_all(&moved).unwrap();
        let topo = topology(vec![
            repo_info(
                &temp.path().join("a-central"),
                &[("demo-skill", central.clone())],
            ),
            repo_info(
                &temp.path().join("z-moved-repo"),
                &[("demo-skill", moved.clone())],
            ),
        ]);
        let dead = temp.path().join("gone/skills/demo-skill");
        let finding = broken_finding("demo-skill", &temp.path().join("entry"), &dead);

        let default_pick = best_relink_target(&topo, &finding, None).unwrap();
        assert_eq!(default_pick.path, central.to_string_lossy());

        let prefer = temp.path().join("z-moved-repo");
        let preferred =
            best_relink_target(&topo, &finding, Some(prefer.to_string_lossy().as_ref())).unwrap();
        assert_eq!(preferred.path, moved.to_string_lossy());
    }

    #[test]
    fn weak_similarity_is_listed_but_never_relinked() {
        let temp = tempdir().unwrap();
        let repo = temp.path().join("repo");
        let sibling = repo.join("skills/grimm");
        let topo = topology(vec![repo_info(&repo, &[("grimm", sibling)])]);
        let dead = repo.join("skills/grill");
        let finding = broken_finding("grill", &temp.path().join("entry"), &dead);

        let found = locate(&topo, &finding);
        // levenshtein("grill", "grimm") = 2 over max len 5 → 60: evidence only.
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].score, 60);
        assert!(best_relink_target(&topo, &finding, None).is_none());
    }

    #[test]
    fn the_dead_location_itself_is_never_a_candidate() {
        let temp = tempdir().unwrap();
        let repo = temp.path().join("repo");
        let dead = repo.join("skills/demo-skill");
        // A stale topology may still list the dead path as a repo skill.
        let topo = topology(vec![repo_info(&repo, &[("demo-skill", dead.clone())])]);
        let finding = broken_finding("demo-skill", &temp.path().join("entry"), &dead);
        assert!(locate(&topo, &finding).is_empty());
    }

    #[test]
    fn non_broken_findings_have_no_candidates() {
        let temp = tempdir().unwrap();
        let repo = temp.path().join("repo");
        let skill = repo.join("skills/demo-skill");
        let topo = topology(vec![repo_info(&repo, &[("demo-skill", skill.clone())])]);
        let mut finding = broken_finding("demo-skill", &temp.path().join("entry"), &skill);
        finding.deviation = Deviation::Direct;
        assert!(locate(&topo, &finding).is_empty());
    }

    #[test]
    fn git_rename_is_detected_with_commit_time() {
        let temp = tempdir().unwrap();
        let repo = temp.path().join("repo");
        // Joined per component: this path is string-compared against what the
        // locator produces, and `join("skills/demo-skill")` would keep the `/`
        // verbatim on Windows. The git arguments below stay POSIX — that is
        // git's own path syntax, not the OS's.
        let old_dir = repo.join("skills").join("demo-skill");
        std::fs::create_dir_all(&old_dir).unwrap();
        std::fs::write(old_dir.join("SKILL.md"), "---\nname: demo-skill\n---\n").unwrap();
        git(&repo, &["init", "-b", "main"]);
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-m", "add skill"]);
        git(&repo, &["mv", "skills/demo-skill", "skills/demo-skill-v2"]);
        git(&repo, &["commit", "-m", "rename skill"]);

        let new_dir = repo.join("skills").join("demo-skill-v2");
        let topo = topology(vec![repo_info(
            &repo,
            &[("demo-skill-v2", new_dir.clone())],
        )]);
        let finding = broken_finding("demo-skill", &temp.path().join("entry"), &old_dir);

        let found = locate(&topo, &finding);
        let rename = found
            .iter()
            .find(|candidate| candidate.reason == "git_rename")
            .expect("the rename commit is found");
        assert_eq!(rename.score, 98);
        assert_eq!(rename.path, new_dir.to_string_lossy());
        assert!(rename.renamed_at.is_some());

        // The rename outranks the same location's similar-name evidence.
        let best = best_relink_target(&topo, &finding, None).expect("relinkable");
        assert_eq!(best.reason, "git_rename");
    }
}
