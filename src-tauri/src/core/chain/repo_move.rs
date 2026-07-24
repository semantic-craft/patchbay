//! Common-cause analysis for broken-link storms: repository moves (issue #33).
//!
//! When several broken entries' dead targets share one root, and a scanned
//! repository elsewhere holds SAME-NAME skill directories for ALL of them,
//! the symptoms collapse into one root cause: the repository was moved. The
//! detection is pure over the scanned topology plus the diagnosed findings —
//! the evidence of a whole-repo move is precisely "everything reappears by
//! name somewhere else", so no Git probing is needed here.
//!
//! Judgment rules (per the #26 spec):
//!
//! * a move pair `(old_root, new_root)` is derived per broken finding by
//!   stripping the longest common trailing components between its dead target
//!   and a same-name candidate's path;
//! * a group must cover at least [`MIN_GROUP`] broken findings; and
//! * EVERY broken finding whose dead target lies under `old_root` must have a
//!   same-name candidate under `new_root` — a partial reappearance is not a
//!   clean migration and produces no aggregate.

use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use super::doctor::{Deviation, Finding};
use super::ChainTopology;

/// Minimum broken findings before symptoms aggregate into a storm card.
pub const MIN_GROUP: usize = 2;

/// One detected repository move: the root cause plus its blast radius.
#[derive(Debug, Clone, Serialize)]
pub struct RepoMove {
    /// The vanished root every dead target shares.
    pub old_root: String,
    /// The scanned location holding same-name skills for every member.
    pub new_root: String,
    /// Display name of the destination repository (its directory name).
    pub repo_name: String,
    /// Affected skill names, sorted.
    pub skills: Vec<String>,
    /// Fingerprints of the aggregated broken findings, sorted.
    pub fingerprints: Vec<String>,
    /// The aggregated findings' entry paths, sorted (the per-link list).
    pub entry_paths: Vec<String>,
}

/// Wire shape of `chain_repo_moves`: the storm groups from one fresh scan.
#[derive(Debug, Clone, Serialize)]
pub struct RepoMoveReport {
    pub groups: Vec<RepoMove>,
    pub scanned_at: i64,
}

/// Detect repository moves across the diagnosed findings. Deterministic:
/// groups come out sorted by (old_root, new_root).
pub fn detect(topo: &ChainTopology, findings: &[Finding]) -> Vec<RepoMove> {
    let broken: Vec<&Finding> = findings
        .iter()
        .filter(|finding| finding.deviation == Deviation::Broken)
        .collect();
    if broken.len() < MIN_GROUP {
        return Vec::new();
    }

    // Vote per finding: every same-name candidate elsewhere derives one
    // (old_root, new_root) pair. BTreeMaps keep the outcome deterministic.
    type Pair = (String, String);
    let mut groups: BTreeMap<Pair, Vec<&Finding>> = BTreeMap::new();
    for finding in &broken {
        let dead = PathBuf::from(&finding.evidence.final_target);
        let skill = super::candidates::skill_of(finding);
        if skill.is_empty() {
            continue;
        }
        for repo in &topo.repos {
            for repo_skill in &repo.skills {
                if repo_skill.name != skill || Path::new(&repo_skill.path) == dead.as_path() {
                    continue;
                }
                if let Some(pair) = move_pair(&dead, Path::new(&repo_skill.path)) {
                    let members = groups.entry(pair).or_default();
                    if !members
                        .iter()
                        .any(|member| member.fingerprint == finding.fingerprint)
                    {
                        members.push(finding);
                    }
                }
            }
        }
    }

    groups
        .into_iter()
        .filter_map(|((old_root, new_root), members)| {
            if members.len() < MIN_GROUP {
                return None;
            }
            // Completeness: every broken finding stranded under old_root must
            // be covered — a partial reappearance is not a whole-repo move.
            let stranded = broken
                .iter()
                .filter(|finding| Path::new(&finding.evidence.final_target).starts_with(&old_root))
                .count();
            if members.len() != stranded {
                return None;
            }
            let mut skills: Vec<String> = members
                .iter()
                .map(|finding| super::candidates::skill_of(finding))
                .collect();
            skills.sort();
            skills.dedup();
            let mut fingerprints: Vec<String> = members
                .iter()
                .map(|finding| finding.fingerprint.clone())
                .collect();
            fingerprints.sort();
            let mut entry_paths: Vec<String> = members
                .iter()
                .map(|finding| finding.evidence.entry_path.clone())
                .collect();
            entry_paths.sort();
            let repo_name = Path::new(&new_root)
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| new_root.clone());
            Some(RepoMove {
                old_root,
                new_root,
                repo_name,
                skills,
                fingerprints,
                entry_paths,
            })
        })
        .collect()
}

/// The move pair a dead target and a same-name candidate imply: strip their
/// longest common trailing components (at least the skill directory itself)
/// and return the differing roots. `None` when the whole paths coincide or
/// either root would be empty.
fn move_pair(dead: &Path, candidate: &Path) -> Option<(String, String)> {
    let d: Vec<Component> = dead.components().collect();
    let c: Vec<Component> = candidate.components().collect();
    let mut shared = 0;
    while shared < d.len() - 1
        && shared < c.len() - 1
        && d[d.len() - 1 - shared] == c[c.len() - 1 - shared]
    {
        shared += 1;
    }
    if shared == 0 {
        return None;
    }
    let old_root: PathBuf = d[..d.len() - shared].iter().collect();
    let new_root: PathBuf = c[..c.len() - shared].iter().collect();
    if old_root.as_os_str().is_empty() || new_root.as_os_str().is_empty() || old_root == new_root {
        return None;
    }
    Some((
        old_root.to_string_lossy().to_string(),
        new_root.to_string_lossy().to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::chain::doctor::{AffectedObject, Evidence, Severity};
    use crate::core::chain::repo_health::RepoHealth;
    use crate::core::chain::warehouse::{RepoInfo, RepoSkill};

    fn repo(path: &str, skills: &[(&str, &str)]) -> RepoInfo {
        RepoInfo {
            name: Path::new(path)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default(),
            path: path.to_string(),
            source_kind: "checkout".to_string(),
            root: "/wh".to_string(),
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
                    path: (*path).to_string(),
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

    fn broken(skill: &str, dead: &str) -> Finding {
        let entry = format!("/proj/.claude/skills/{skill}");
        Finding {
            rule: "chain.broken_link".to_string(),
            deviation: Deviation::Broken,
            severity: Severity::Violation,
            evidence: Evidence {
                entry_path: entry.clone(),
                hops: Vec::new(),
                final_target: dead.to_string(),
                topology_status: "broken".to_string(),
            },
            affected: vec![
                AffectedObject {
                    kind: "skill".to_string(),
                    name: skill.to_string(),
                    path: entry,
                },
                AffectedObject {
                    kind: "project".to_string(),
                    name: "proj".to_string(),
                    path: "/proj".to_string(),
                },
            ],
            actions: Vec::new(),
            fingerprint: format!("fp-{skill}"),
        }
    }

    #[test]
    fn a_clean_whole_repo_move_aggregates_into_one_group() {
        let topo = topology(vec![repo(
            "/wh/xw-writing-v2",
            &[
                ("ppt-master", "/wh/xw-writing-v2/skills/ppt-master"),
                ("zotero", "/wh/xw-writing-v2/skills/zotero"),
                ("unrelated", "/wh/xw-writing-v2/skills/unrelated"),
            ],
        )]);
        let findings = vec![
            broken("ppt-master", "/wh/xw-writing/skills/ppt-master"),
            broken("zotero", "/wh/xw-writing/skills/zotero"),
        ];

        let groups = detect(&topo, &findings);
        assert_eq!(groups.len(), 1);
        let group = &groups[0];
        // Roots are rebuilt from path components, so normalize the OS separator
        // before comparing: the logical root is the same on Windows (`\wh\...`).
        assert_eq!(group.old_root.replace('\\', "/"), "/wh/xw-writing");
        assert_eq!(group.new_root.replace('\\', "/"), "/wh/xw-writing-v2");
        assert_eq!(group.repo_name, "xw-writing-v2");
        assert_eq!(group.skills, vec!["ppt-master", "zotero"]);
        assert_eq!(group.fingerprints, vec!["fp-ppt-master", "fp-zotero"]);
    }

    #[test]
    fn a_partial_reappearance_is_not_a_move() {
        // zotero has NO same-name skill under the candidate root: the storm is
        // not a clean migration, so no aggregate may claim it is.
        let topo = topology(vec![repo(
            "/wh/xw-writing-v2",
            &[("ppt-master", "/wh/xw-writing-v2/skills/ppt-master")],
        )]);
        let findings = vec![
            broken("ppt-master", "/wh/xw-writing/skills/ppt-master"),
            broken("zotero", "/wh/xw-writing/skills/zotero"),
        ];

        assert!(detect(&topo, &findings).is_empty());
    }

    #[test]
    fn a_single_broken_link_never_aggregates() {
        let topo = topology(vec![repo(
            "/wh/xw-writing-v2",
            &[("ppt-master", "/wh/xw-writing-v2/skills/ppt-master")],
        )]);
        let findings = vec![broken("ppt-master", "/wh/xw-writing/skills/ppt-master")];

        assert!(detect(&topo, &findings).is_empty());
    }

    #[test]
    fn unrelated_breaks_in_another_root_do_not_block_the_group() {
        // Two skills moved to v2; a third break points at a DIFFERENT old root
        // and must neither join nor veto the group.
        let topo = topology(vec![repo(
            "/wh/xw-writing-v2",
            &[
                ("ppt-master", "/wh/xw-writing-v2/skills/ppt-master"),
                ("zotero", "/wh/xw-writing-v2/skills/zotero"),
            ],
        )]);
        let findings = vec![
            broken("ppt-master", "/wh/xw-writing/skills/ppt-master"),
            broken("zotero", "/wh/xw-writing/skills/zotero"),
            broken("other", "/elsewhere/repo/skills/other"),
        ];

        let groups = detect(&topo, &findings);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].skills, vec!["ppt-master", "zotero"]);
    }

    #[test]
    fn non_broken_findings_are_ignored() {
        let topo = topology(vec![repo(
            "/wh/xw-writing-v2",
            &[
                ("ppt-master", "/wh/xw-writing-v2/skills/ppt-master"),
                ("zotero", "/wh/xw-writing-v2/skills/zotero"),
            ],
        )]);
        let mut a = broken("ppt-master", "/wh/xw-writing/skills/ppt-master");
        a.deviation = Deviation::Direct;
        let findings = vec![a, broken("zotero", "/wh/xw-writing/skills/zotero")];

        assert!(detect(&topo, &findings).is_empty());
    }

    #[test]
    fn the_dead_location_itself_is_not_a_destination() {
        // A stale topology still lists the dead paths as repo skills; the
        // "move" to the identical location must not be derived.
        let topo = topology(vec![repo(
            "/wh/xw-writing",
            &[
                ("ppt-master", "/wh/xw-writing/skills/ppt-master"),
                ("zotero", "/wh/xw-writing/skills/zotero"),
            ],
        )]);
        let findings = vec![
            broken("ppt-master", "/wh/xw-writing/skills/ppt-master"),
            broken("zotero", "/wh/xw-writing/skills/zotero"),
        ];

        assert!(detect(&topo, &findings).is_empty());
    }
}
