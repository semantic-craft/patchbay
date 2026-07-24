//! Read-only projections over a scanned topology for the chain CLI.
//!
//! Like Doctor, these derive from an already-scanned `ChainTopology` so the CLI
//! and the GUI share one scan and one set of results — a `where`/`repository`
//! answer from the CLI is byte-for-byte the projection of the same topology the
//! GUI renders. Neither function touches disk or Git; both are pure over the
//! topology and deterministic in output order.

use serde::Serialize;

use super::warehouse::RepoInfo;
use super::ChainTopology;

/// One place a named Skill appears in the topology, with the tier it sits on and
/// the chain evidence for that occurrence. A single name can appear in several
/// places (an Original Repository plus the project links that reference it), so
/// `where` returns every match rather than a single "the" location.
#[derive(Debug, Clone, Serialize)]
pub struct SkillLocation {
    /// "original" (tier 1) | "aggregate" (tier 2 `.agents/skills`) | "surface" (tier 3).
    pub tier: String,
    /// Registered project path for tier 2/3 matches; `None` for an Original.
    pub project: Option<String>,
    /// Project display name for tier 2/3 matches.
    pub project_name: Option<String>,
    /// Agent name for a tier-3 surface entry; `None` otherwise.
    pub agent: Option<String>,
    /// The entry (or Original skill) path this occurrence is about.
    pub entry_path: String,
    /// Topology status token for this occurrence ("link_repo", "direct",
    /// "broken", …); "present" for an Original repository skill.
    pub status: String,
    /// Symlink hops in resolution order (empty for a physical Original).
    pub hops: Vec<String>,
    /// Where this occurrence resolves (equals `entry_path` for an Original).
    pub final_target: String,
    /// Warehouse repo name when the chain resolves inside one.
    pub repo: Option<String>,
}

/// The resolution of a single Skill name across the whole topology.
#[derive(Debug, Clone, Serialize)]
pub struct SkillResolution {
    pub skill: String,
    pub locations: Vec<SkillLocation>,
    /// Copied from the topology so the CLI and Link Topology share one clock.
    pub scanned_at: i64,
}

/// Ordering rank for a tier so `where` output is stable regardless of scan order.
fn tier_rank(tier: &str) -> u8 {
    match tier {
        "original" => 0,
        "aggregate" => 1,
        "surface" => 2,
        _ => 3,
    }
}

/// Trailing-slash-insensitive path compare for the `--project` narrowing. Kept
/// pure (no filesystem canonicalization) so this projection stays a function of
/// the topology alone; the CLI passes a registered project path verbatim.
fn same_project(project_path: &str, filter: &str) -> bool {
    project_path.trim_end_matches('/') == filter.trim_end_matches('/')
}

/// Resolve a Skill name across every tier of the topology.
///
/// With `project` set, only tier-2/3 occurrences in that registered project are
/// returned (Originals are global and are omitted, since the question becomes
/// "where does this Skill live inside project X"). With `project` unset, every
/// Original and every project reference is returned.
pub fn resolve(topo: &ChainTopology, skill: &str, project: Option<&str>) -> SkillResolution {
    let mut locations = Vec::new();

    // Tier 1: Original Repository skills. Skipped entirely when a project filter
    // is active, because an Original is not inside any project.
    if project.is_none() {
        for repo in &topo.repos {
            for repo_skill in &repo.skills {
                if repo_skill.name == skill {
                    locations.push(SkillLocation {
                        tier: "original".to_string(),
                        project: None,
                        project_name: None,
                        agent: None,
                        entry_path: repo_skill.path.clone(),
                        status: "present".to_string(),
                        hops: Vec::new(),
                        final_target: repo_skill.path.clone(),
                        repo: Some(repo.name.clone()),
                    });
                }
            }
        }
    }

    // Tiers 2/3: project references.
    for proj in &topo.projects {
        if let Some(filter) = project {
            if !same_project(&proj.path, filter) {
                continue;
            }
        }
        if let Some(agg) = &proj.agents_dir {
            for entry in &agg.entries {
                if entry.name == skill {
                    locations.push(SkillLocation {
                        tier: "aggregate".to_string(),
                        project: Some(proj.path.clone()),
                        project_name: Some(proj.name.clone()),
                        agent: None,
                        entry_path: entry.entry_path.clone(),
                        status: entry.status.clone(),
                        hops: entry.hops.clone(),
                        final_target: entry.final_target.clone(),
                        repo: entry.repo.clone(),
                    });
                }
            }
        }
        for surface in &proj.surfaces {
            for entry in &surface.entries {
                if entry.name == skill {
                    locations.push(SkillLocation {
                        tier: "surface".to_string(),
                        project: Some(proj.path.clone()),
                        project_name: Some(proj.name.clone()),
                        agent: Some(surface.agent.clone()),
                        entry_path: entry.entry_path.clone(),
                        status: entry.status.clone(),
                        hops: entry.hops.clone(),
                        final_target: entry.final_target.clone(),
                        repo: entry.repo.clone(),
                    });
                }
            }
        }
    }

    // Deterministic order: tier, then project, then entry path.
    locations.sort_by(|a, b| {
        tier_rank(&a.tier)
            .cmp(&tier_rank(&b.tier))
            .then_with(|| a.project.cmp(&b.project))
            .then_with(|| a.entry_path.cmp(&b.entry_path))
    });

    SkillResolution {
        skill: skill.to_string(),
        locations,
        scanned_at: topo.scanned_at,
    }
}

/// The Original Repository inventory with health, exactly as the topology holds
/// it, plus the shared scan clock. This is the `repository-status` projection —
/// the same `RepoInfo` records Link Topology renders, no re-scan.
#[derive(Debug, Clone, Serialize)]
pub struct RepositoryStatus {
    pub repos: Vec<RepoInfo>,
    pub scanned_at: i64,
}

pub fn repository_status(topo: &ChainTopology) -> RepositoryStatus {
    RepositoryStatus {
        repos: topo.repos.clone(),
        scanned_at: topo.scanned_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::chain::project_links::{
        AgentSurface, AggregateDir, ProjectChain, TracedEntry,
    };
    use crate::core::chain::repo_health::RepoHealth;
    use crate::core::chain::warehouse::{RepoInfo, RepoSkill};

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

    fn repo(name: &str, skills: &[&str]) -> RepoInfo {
        RepoInfo {
            name: name.to_string(),
            path: format!("/wh/{name}"),
            source_kind: "checkout".to_string(),
            root: "/wh".to_string(),
            health: clean_health(),
            origin: None,
            upstream: None,
            skills: skills
                .iter()
                .map(|s| RepoSkill {
                    name: s.to_string(),
                    path: format!("/wh/{name}/{s}"),
                })
                .collect(),
            referenced_by: Vec::new(),
        }
    }

    fn traced(name: &str, status: &str, final_target: &str, repo: Option<&str>) -> TracedEntry {
        TracedEntry {
            name: name.to_string(),
            entry_path: format!("/proj/demo/.agents/skills/{name}"),
            hops: vec![final_target.to_string()],
            final_target: final_target.to_string(),
            status: status.to_string(),
            repo: repo.map(|r| r.to_string()),
        }
    }

    fn project(path: &str, agg: Vec<TracedEntry>, surface: Vec<TracedEntry>) -> ProjectChain {
        ProjectChain {
            name: "demo".to_string(),
            path: path.to_string(),
            agents_dir: Some(AggregateDir {
                path: format!("{path}/.agents/skills"),
                entries: agg,
            }),
            surfaces: vec![AgentSurface {
                agent: "claude".to_string(),
                path: format!("{path}/.claude/skills"),
                kind: "per_entry".to_string(),
                dir_link_target: None,
                dir_link_ok: false,
                entries: surface,
            }],
        }
    }

    fn topo(projects: Vec<ProjectChain>, repos: Vec<RepoInfo>) -> ChainTopology {
        ChainTopology {
            warehouse_roots: Vec::new(),
            projects_root: "/proj".to_string(),
            repos,
            projects,
            guard: Vec::new(),
            scanned_at: 4242,
        }
    }

    #[test]
    fn resolve_reports_every_tier_for_a_present_skill() {
        let t = topo(
            vec![project(
                "/proj/demo",
                vec![traced("alpha", "link_repo", "/wh/repo/alpha", Some("repo"))],
                vec![traced(
                    "alpha",
                    "via_agents",
                    "/wh/repo/alpha",
                    Some("repo"),
                )],
            )],
            vec![repo("repo", &["alpha"])],
        );

        let res = resolve(&t, "alpha", None);
        assert_eq!(res.skill, "alpha");
        assert_eq!(res.scanned_at, 4242);
        let tiers: Vec<&str> = res.locations.iter().map(|l| l.tier.as_str()).collect();
        // Deterministic order: original, then aggregate, then surface.
        assert_eq!(tiers, ["original", "aggregate", "surface"]);
        assert_eq!(res.locations[0].repo.as_deref(), Some("repo"));
        assert_eq!(res.locations[2].agent.as_deref(), Some("claude"));
    }

    #[test]
    fn resolve_returns_empty_for_an_absent_skill() {
        let t = topo(
            vec![project("/proj/demo", Vec::new(), Vec::new())],
            vec![repo("repo", &["alpha"])],
        );
        let res = resolve(&t, "ghost", None);
        assert!(res.locations.is_empty());
        assert_eq!(res.scanned_at, 4242);
    }

    #[test]
    fn resolve_project_filter_narrows_to_that_project_and_drops_originals() {
        let t = topo(
            vec![
                project(
                    "/proj/demo",
                    vec![traced("alpha", "link_repo", "/wh/repo/alpha", Some("repo"))],
                    Vec::new(),
                ),
                project(
                    "/proj/other",
                    vec![traced("alpha", "link_repo", "/wh/repo/alpha", Some("repo"))],
                    Vec::new(),
                ),
            ],
            vec![repo("repo", &["alpha"])],
        );

        // Trailing slash must not defeat the match.
        let res = resolve(&t, "alpha", Some("/proj/demo/"));
        assert_eq!(res.locations.len(), 1);
        assert_eq!(res.locations[0].tier, "aggregate");
        assert_eq!(res.locations[0].project.as_deref(), Some("/proj/demo"));
    }

    #[test]
    fn repository_status_projects_repos_and_clock_without_rescan() {
        let t = topo(
            Vec::new(),
            vec![repo("repo", &["alpha"]), repo("beta", &[])],
        );
        let status = repository_status(&t);
        assert_eq!(status.scanned_at, 4242);
        let names: Vec<&str> = status.repos.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["repo", "beta"]);
    }

    #[test]
    fn repository_status_is_empty_when_no_repositories() {
        let t = topo(Vec::new(), Vec::new());
        assert!(repository_status(&t).repos.is_empty());
    }
}
