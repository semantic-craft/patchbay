//! Read-only Doctor: turn topology deviations into stable, filterable findings.
//!
//! Doctor never scans the filesystem itself — it derives findings from a
//! `ChainTopology` the Chain Service already produced, so there is exactly one
//! scan and classifier. Each finding carries a stable rule identifier, a
//! severity, the same chain evidence Link Topology shows (traced hops and final
//! target), the affected objects, the next actions Patchbay could offer, and a
//! fingerprint over its material evidence. The fingerprint is what a later
//! ignore feature keys on (rule + evidence) so a changed chain is reconsidered;
//! this ticket only produces it and does not persist ignores.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::project_links::{AgentSurface, ProjectChain, TracedEntry};
use super::warehouse::RepoInfo;
use super::ChainTopology;

/// How much a finding matters, high → low. This is the severity filter axis;
/// it is deliberately coarse and orthogonal to the deviation type so the UI and
/// CLI can filter on either independently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// A dead or invalid link — the standard chain is broken.
    Violation,
    /// Works today but departs from the convention (unmanaged copy, legacy layer).
    Warning,
    /// Correct but normalizable (a direct link that skips `.agents/skills`).
    Advice,
    /// Informational only (a legitimate project-private skill, an unused original).
    Notice,
}

impl Severity {
    /// Rank for a deterministic high-to-low ordering. Not part of the wire
    /// contract; callers filter on the serialized name, not this number.
    fn rank(self) -> u8 {
        match self {
            Severity::Violation => 0,
            Severity::Warning => 1,
            Severity::Advice => 2,
            Severity::Notice => 3,
        }
    }
}

/// The deviation category a finding belongs to — the "type" filter axis. Exactly
/// the six states this ticket ships (issue #9): every finding is one of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Deviation {
    /// Symlink whose target is missing or cyclic.
    Broken,
    /// Agent-entry link straight to an original repo, skipping `.agents/skills`.
    Direct,
    /// Physical (non-symlink) skill copied into an agent surface — unmanaged.
    Copy,
    /// Physical skill living in `.agents/skills` — a legitimate project-private skill.
    ProjectPrivate,
    /// Link resolving through a retired shared-distribution layer (old `local-skills`).
    Legacy,
    /// Original repository no project references.
    Orphan,
}

impl Deviation {
    /// Stable, namespaced rule identifier surfaced on the finding and used as
    /// the ignore-record key prefix. Never localized, never reordered.
    fn rule(self) -> &'static str {
        match self {
            Deviation::Broken => "chain.broken_link",
            Deviation::Direct => "chain.direct_link",
            Deviation::Copy => "chain.unmanaged_copy",
            Deviation::ProjectPrivate => "chain.project_private",
            Deviation::Legacy => "chain.legacy_layer",
            Deviation::Orphan => "chain.orphan_original",
        }
    }

    fn severity(self) -> Severity {
        match self {
            Deviation::Broken => Severity::Violation,
            Deviation::Copy | Deviation::Legacy => Severity::Warning,
            Deviation::Direct => Severity::Advice,
            Deviation::ProjectPrivate | Deviation::Orphan => Severity::Notice,
        }
    }

    /// Next actions Patchbay could offer for this deviation. Stable codes, not
    /// localized prose; this read-only ticket advertises them without running
    /// any of them. Order is presentation, not contract.
    fn actions(self) -> &'static [&'static str] {
        match self {
            Deviation::Broken => &["repair", "remove"],
            Deviation::Direct => &["normalize"],
            Deviation::Copy => &["migrate_to_repo", "mark_private"],
            Deviation::ProjectPrivate => &["mark_private", "migrate_to_repo"],
            Deviation::Legacy => &["migrate"],
            Deviation::Orphan => &["link_to_project"],
        }
    }
}

/// One affected object referenced by a finding, e.g. the offending skill entry,
/// its project, or an original repository. `path` is the canonical identity.
#[derive(Debug, Clone, Serialize)]
pub struct AffectedObject {
    /// "skill" | "project" | "repo" | "surface"
    pub kind: String,
    pub name: String,
    pub path: String,
}

/// The chain evidence behind a finding — the same hop-by-hop resolution Link
/// Topology renders, copied from the traced entry so Doctor and Topology never
/// disagree. For repo-level findings (orphan) the hops are empty and the final
/// target is the repository path itself.
#[derive(Debug, Clone, Serialize)]
pub struct Evidence {
    /// The entry (or repository) path the finding is about.
    pub entry_path: String,
    /// Symlink hops as traced by `link_tracer`, in resolution order.
    pub hops: Vec<String>,
    /// Where the chain ends (equals `entry_path` for a physical directory/repo).
    pub final_target: String,
    /// The topology status token this finding was derived from (e.g. "direct").
    pub topology_status: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    /// Stable rule identifier (e.g. `chain.direct_link`).
    pub rule: String,
    pub deviation: Deviation,
    pub severity: Severity,
    pub evidence: Evidence,
    pub affected: Vec<AffectedObject>,
    /// Stable action codes Patchbay could offer; none are executed here.
    pub actions: Vec<String>,
    /// Hash over rule + material evidence. Stable while the chain is unchanged;
    /// changes when the deviation's evidence changes, so a future ignore record
    /// keyed on it is reconsidered.
    pub fingerprint: String,
}

impl Finding {
    fn new(deviation: Deviation, evidence: Evidence, affected: Vec<AffectedObject>) -> Self {
        // Material evidence: the object identity, where it now resolves, and its
        // classified status. Re-pointing the link or changing its status yields
        // a new fingerprint; a stable chain keeps the same one.
        let fingerprint = fingerprint(
            deviation.rule(),
            &[
                &evidence.entry_path,
                &evidence.final_target,
                &evidence.topology_status,
            ],
        );
        Finding {
            rule: deviation.rule().to_string(),
            deviation,
            severity: deviation.severity(),
            evidence,
            affected,
            actions: deviation.actions().iter().map(|a| a.to_string()).collect(),
            fingerprint,
        }
    }
}

/// Which findings to keep. Empty vectors mean "no constraint on this axis", so a
/// default filter returns everything. The two axes combine with AND.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct DoctorFilter {
    #[serde(default)]
    pub severities: Vec<Severity>,
    #[serde(default)]
    pub deviations: Vec<Deviation>,
}

impl DoctorFilter {
    fn keeps(&self, finding: &Finding) -> bool {
        (self.severities.is_empty() || self.severities.contains(&finding.severity))
            && (self.deviations.is_empty() || self.deviations.contains(&finding.deviation))
    }

    /// Retain only findings matching both axes, preserving order.
    pub fn apply(&self, findings: Vec<Finding>) -> Vec<Finding> {
        findings.into_iter().filter(|f| self.keeps(f)).collect()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DoctorReport {
    pub findings: Vec<Finding>,
    /// Findings currently hidden by a persisted ignore/project-private decision,
    /// returned unfiltered so the UI's "Ignored" panel is complete and each can
    /// be restored. Populated by the Chain Service from persisted decisions;
    /// `diagnose` itself never reads them.
    pub ignored: Vec<Finding>,
    /// Count of visible findings before the active filter was applied, so the UI
    /// can show "N of M" without a second scan. Ignored findings are excluded
    /// from this count and returned separately in `ignored`.
    pub total: usize,
    /// Copied from the topology so the report and Link Topology share one clock.
    pub scanned_at: i64,
}

/// Derive findings from an already-scanned topology. Read-only and deterministic:
/// it inspects the topology's classified entries and never touches disk or Git.
pub fn diagnose(topo: &ChainTopology) -> Vec<Finding> {
    let mut findings = Vec::new();

    for project in &topo.projects {
        if let Some(agg) = &project.agents_dir {
            for entry in &agg.entries {
                if let Some(finding) = aggregate_finding(project, entry) {
                    findings.push(finding);
                }
            }
        }
        for surface in &project.surfaces {
            // A directory entry link that does not resolve to this project's
            // `.agents/skills` is a broken agent entry — the tier-3 shape failed.
            if surface.kind == "dir_link" && !surface.dir_link_ok {
                findings.push(broken_surface_finding(project, surface));
            }
            for entry in &surface.entries {
                if let Some(finding) = surface_finding(project, surface, entry) {
                    findings.push(finding);
                }
            }
        }
    }

    for repo in &topo.repos {
        // Only optional developer checkouts can be orphaned. Patchbay Central is
        // the managed inventory and remains valid even when none of its skills
        // is currently linked into a project.
        if repo.source_kind == "checkout"
            && repo.referenced_by.is_empty()
            && !repo.skills.is_empty()
        {
            findings.push(orphan_finding(repo));
        }
    }

    // Deterministic high-to-low order for stable presentation. Tests assert set
    // membership, not this order (it is not part of the contract).
    findings.sort_by(|a, b| {
        a.severity
            .rank()
            .cmp(&b.severity.rank())
            .then_with(|| a.rule.cmp(&b.rule))
            .then_with(|| a.evidence.entry_path.cmp(&b.evidence.entry_path))
    });
    findings
}

/// Map an aggregate (`.agents/skills`) entry's classification to a deviation.
/// Healthy `link_repo` links and project-internal links produce nothing.
fn aggregate_finding(project: &ProjectChain, entry: &TracedEntry) -> Option<Finding> {
    let deviation = match entry.status.as_str() {
        "broken" => Deviation::Broken,
        "private" => Deviation::ProjectPrivate,
        // Resolves outside every warehouse root and outside the project: the
        // retired shared-distribution layer is the only class that lands here.
        "external" => Deviation::Legacy,
        _ => return None,
    };
    Some(Finding::new(
        deviation,
        entry_evidence(entry),
        entry_affected(project, entry),
    ))
}

/// Map an agent-surface (`.claude/skills`, …) entry's classification to a
/// deviation. Healthy `via_agents` links produce nothing.
fn surface_finding(
    project: &ProjectChain,
    surface: &AgentSurface,
    entry: &TracedEntry,
) -> Option<Finding> {
    let deviation = match entry.status.as_str() {
        "broken" => Deviation::Broken,
        "direct" => Deviation::Direct,
        "copy" => Deviation::Copy,
        // As with aggregate entries, a link resolving outside every managed
        // location is a retired shared-distribution remnant.
        "external" => Deviation::Legacy,
        _ => return None,
    };
    let mut affected = entry_affected(project, entry);
    affected.push(AffectedObject {
        kind: "surface".to_string(),
        name: surface.agent.clone(),
        path: surface.path.clone(),
    });
    Some(Finding::new(deviation, entry_evidence(entry), affected))
}

/// A directory-level agent entry link that fails the `.claude/skills →
/// .agents/skills` shape (missing target or pointed elsewhere).
fn broken_surface_finding(project: &ProjectChain, surface: &AgentSurface) -> Finding {
    let target = surface.dir_link_target.clone().unwrap_or_default();
    let evidence = Evidence {
        entry_path: surface.path.clone(),
        hops: Vec::new(),
        final_target: target,
        topology_status: "dir_link_broken".to_string(),
    };
    let affected = vec![
        AffectedObject {
            kind: "surface".to_string(),
            name: surface.agent.clone(),
            path: surface.path.clone(),
        },
        AffectedObject {
            kind: "project".to_string(),
            name: project.name.clone(),
            path: project.path.clone(),
        },
    ];
    Finding::new(Deviation::Broken, evidence, affected)
}

fn orphan_finding(repo: &RepoInfo) -> Finding {
    let evidence = Evidence {
        entry_path: repo.path.clone(),
        hops: Vec::new(),
        final_target: repo.path.clone(),
        topology_status: "orphan".to_string(),
    };
    let affected = vec![AffectedObject {
        kind: "repo".to_string(),
        name: repo.name.clone(),
        path: repo.path.clone(),
    }];
    Finding::new(Deviation::Orphan, evidence, affected)
}

/// The traced chain of an entry, reused verbatim as Doctor evidence so it
/// matches what Link Topology renders for the same entry.
fn entry_evidence(entry: &TracedEntry) -> Evidence {
    Evidence {
        entry_path: entry.entry_path.clone(),
        hops: entry.hops.clone(),
        final_target: entry.final_target.clone(),
        topology_status: entry.status.clone(),
    }
}

fn entry_affected(project: &ProjectChain, entry: &TracedEntry) -> Vec<AffectedObject> {
    let mut affected = vec![
        AffectedObject {
            kind: "skill".to_string(),
            name: entry.name.clone(),
            path: entry.entry_path.clone(),
        },
        AffectedObject {
            kind: "project".to_string(),
            name: project.name.clone(),
            path: project.path.clone(),
        },
    ];
    if let Some(repo) = &entry.repo {
        affected.push(AffectedObject {
            kind: "repo".to_string(),
            name: repo.clone(),
            path: entry.final_target.clone(),
        });
    }
    affected
}

/// Deterministic hash over a rule id and its material evidence, hex-encoded.
/// NUL-separated so `["a", "bc"]` and `["ab", "c"]` never collide.
fn fingerprint(rule: &str, parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(rule.as_bytes());
    for part in parts {
        hasher.update([0u8]);
        hasher.update(part.as_bytes());
    }
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::chain::project_links::{AggregateDir, TracedEntry};
    use crate::core::chain::repo_health::RepoHealth;
    use crate::core::chain::warehouse::{ProjectRef, RepoInfo, RepoSkill};

    /// A clean, up-to-date working tree — the Git health these topology fixtures
    /// assume, since Doctor rules key off links, not repo state.
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

    fn entry(name: &str, status: &str, final_target: &str, repo: Option<&str>) -> TracedEntry {
        TracedEntry {
            name: name.to_string(),
            entry_path: format!("/proj/.claude/skills/{name}"),
            hops: vec![final_target.to_string()],
            final_target: final_target.to_string(),
            status: status.to_string(),
            repo: repo.map(|r| r.to_string()),
        }
    }

    fn topo_with(projects: Vec<ProjectChain>, repos: Vec<RepoInfo>) -> ChainTopology {
        ChainTopology {
            warehouse_roots: Vec::new(),
            projects_root: "/proj".to_string(),
            repos,
            projects,
            guard: Vec::new(),
            scanned_at: 0,
        }
    }

    fn surface_project(agent: &str, entries: Vec<TracedEntry>) -> ProjectChain {
        ProjectChain {
            name: "demo".to_string(),
            path: "/proj/demo".to_string(),
            agents_dir: None,
            surfaces: vec![AgentSurface {
                agent: agent.to_string(),
                path: format!("/proj/demo/.{agent}/skills"),
                kind: "per_entry".to_string(),
                dir_link_target: None,
                dir_link_ok: false,
                entries,
            }],
        }
    }

    fn rule_ids(findings: &[Finding]) -> Vec<&str> {
        findings.iter().map(|f| f.rule.as_str()).collect()
    }

    #[test]
    fn clean_topology_produces_no_findings() {
        let project = ProjectChain {
            name: "demo".to_string(),
            path: "/proj/demo".to_string(),
            agents_dir: Some(AggregateDir {
                path: "/proj/demo/.agents/skills".to_string(),
                entries: vec![entry("ok", "link_repo", "/wh/repo/skills/ok", Some("repo"))],
            }),
            surfaces: vec![AgentSurface {
                agent: "claude".to_string(),
                path: "/proj/demo/.claude/skills".to_string(),
                kind: "dir_link".to_string(),
                dir_link_target: Some("/proj/demo/.agents/skills".to_string()),
                dir_link_ok: true,
                entries: Vec::new(),
            }],
        };
        let repo = RepoInfo {
            name: "repo".to_string(),
            path: "/wh/repo".to_string(),
            source_kind: "checkout".to_string(),
            root: "/wh".to_string(),
            health: clean_health(),
            origin: None,
            upstream: None,
            skills: vec![RepoSkill {
                name: "ok".to_string(),
                path: "/wh/repo/skills/ok".to_string(),
            }],
            referenced_by: vec![ProjectRef {
                name: "demo".to_string(),
                path: "/proj/demo".to_string(),
            }],
        };
        assert!(diagnose(&topo_with(vec![project], vec![repo])).is_empty());
    }

    #[test]
    fn each_deviation_maps_to_its_rule_and_severity() {
        let project = surface_project(
            "claude",
            vec![
                entry("d", "direct", "/wh/repo/skills/d", Some("repo")),
                entry("c", "copy", "/proj/demo/.claude/skills/c", None),
                entry("b", "broken", "/nowhere", None),
                entry("l", "external", "/home/local-skills/shared/l", None),
            ],
        );
        let private = ProjectChain {
            name: "priv".to_string(),
            path: "/proj/priv".to_string(),
            agents_dir: Some(AggregateDir {
                path: "/proj/priv/.agents/skills".to_string(),
                entries: vec![entry("p", "private", "/proj/priv/.agents/skills/p", None)],
            }),
            surfaces: Vec::new(),
        };
        let orphan = RepoInfo {
            name: "lonely".to_string(),
            path: "/wh/lonely".to_string(),
            source_kind: "checkout".to_string(),
            root: "/wh".to_string(),
            health: clean_health(),
            origin: None,
            upstream: None,
            skills: vec![RepoSkill {
                name: "s".to_string(),
                path: "/wh/lonely/skills/s".to_string(),
            }],
            referenced_by: Vec::new(),
        };

        let findings = diagnose(&topo_with(vec![project, private], vec![orphan]));
        let ids = rule_ids(&findings);
        for expected in [
            "chain.direct_link",
            "chain.unmanaged_copy",
            "chain.broken_link",
            "chain.legacy_layer",
            "chain.project_private",
            "chain.orphan_original",
        ] {
            assert!(ids.contains(&expected), "missing {expected} in {ids:?}");
        }
        let by_rule = |rule: &str| findings.iter().find(|f| f.rule == rule).unwrap();
        assert_eq!(by_rule("chain.broken_link").severity, Severity::Violation);
        assert_eq!(by_rule("chain.unmanaged_copy").severity, Severity::Warning);
        assert_eq!(by_rule("chain.legacy_layer").severity, Severity::Warning);
        assert_eq!(by_rule("chain.direct_link").severity, Severity::Advice);
        assert_eq!(by_rule("chain.project_private").severity, Severity::Notice);
        assert_eq!(by_rule("chain.orphan_original").severity, Severity::Notice);
    }

    #[test]
    fn findings_carry_chain_evidence_and_actions() {
        let project = surface_project(
            "claude",
            vec![entry("d", "direct", "/wh/repo/skills/d", Some("repo"))],
        );
        let findings = diagnose(&topo_with(vec![project], Vec::new()));
        let direct = &findings[0];
        assert_eq!(direct.evidence.final_target, "/wh/repo/skills/d");
        assert_eq!(direct.evidence.hops, vec!["/wh/repo/skills/d".to_string()]);
        assert_eq!(direct.evidence.topology_status, "direct");
        assert_eq!(direct.actions, vec!["normalize".to_string()]);
        assert!(direct
            .affected
            .iter()
            .any(|o| o.kind == "skill" && o.name == "d"));
        assert!(direct.affected.iter().any(|o| o.kind == "surface"));
    }

    #[test]
    fn broken_directory_entry_link_is_a_violation() {
        let project = ProjectChain {
            name: "demo".to_string(),
            path: "/proj/demo".to_string(),
            agents_dir: None,
            surfaces: vec![AgentSurface {
                agent: "codex".to_string(),
                path: "/proj/demo/.codex/skills".to_string(),
                kind: "dir_link".to_string(),
                dir_link_target: Some("/proj/demo/.agents/skills".to_string()),
                dir_link_ok: false,
                entries: Vec::new(),
            }],
        };
        let findings = diagnose(&topo_with(vec![project], Vec::new()));
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule, "chain.broken_link");
        assert_eq!(findings[0].severity, Severity::Violation);
    }

    #[test]
    fn fingerprint_is_stable_but_changes_with_evidence() {
        let base = surface_project(
            "claude",
            vec![entry("d", "direct", "/wh/repo/skills/d", Some("repo"))],
        );
        let again = surface_project(
            "claude",
            vec![entry("d", "direct", "/wh/repo/skills/d", Some("repo"))],
        );
        let moved = surface_project(
            "claude",
            vec![entry("d", "direct", "/wh/other/skills/d", Some("other"))],
        );
        let fp = |p: ProjectChain| {
            diagnose(&topo_with(vec![p], Vec::new()))[0]
                .fingerprint
                .clone()
        };
        assert_eq!(fp(base), fp(again), "unchanged evidence keeps fingerprint");
        assert_ne!(
            fp(surface_project(
                "claude",
                vec![entry("d", "direct", "/wh/repo/skills/d", Some("repo"))]
            )),
            fp(moved),
            "re-pointed link changes fingerprint"
        );
    }

    #[test]
    fn filter_narrows_by_severity_and_type() {
        let project = surface_project(
            "claude",
            vec![
                entry("d", "direct", "/wh/repo/skills/d", Some("repo")),
                entry("b", "broken", "/nowhere", None),
            ],
        );
        let all = diagnose(&topo_with(vec![project], Vec::new()));
        assert_eq!(all.len(), 2);

        let by_sev = DoctorFilter {
            severities: vec![Severity::Violation],
            deviations: Vec::new(),
        }
        .apply(all.clone());
        assert_eq!(rule_ids(&by_sev), vec!["chain.broken_link"]);

        let by_type = DoctorFilter {
            severities: Vec::new(),
            deviations: vec![Deviation::Direct],
        }
        .apply(all.clone());
        assert_eq!(rule_ids(&by_type), vec!["chain.direct_link"]);

        // Empty filter is a no-op.
        assert_eq!(DoctorFilter::default().apply(all.clone()).len(), 2);

        // AND across axes with no overlap yields nothing.
        let none = DoctorFilter {
            severities: vec![Severity::Notice],
            deviations: vec![Deviation::Direct],
        }
        .apply(all);
        assert!(none.is_empty());
    }
}
