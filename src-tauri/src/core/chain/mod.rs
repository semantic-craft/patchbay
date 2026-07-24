//! Three-tier skill link topology (xw fork module).
//!
//! Models the machine's skill-management convention:
//! tier 1: Patchbay's managed central library plus optional Git checkouts,
//! tier 2: per-project aggregate dir `.agents/skills/<name>` symlinking to originals,
//! tier 3: per-agent entry links `.claude/skills` -> `.agents/skills` (and codex etc).
//!
//! Policy-managed global agent surfaces (`~/.claude/skills`, ...) are not a tier:
//! this module watches that they stay empty. Vendor-managed built-in surfaces are
//! outside that policy.
//!
//! `ChainService` is the application-level contract for scans and the existing
//! link/unlink operations; the lower-level modules hold the filesystem rules.

pub mod candidates;
pub mod decisions;
pub mod doctor;
pub mod duplicates;
pub mod fork_sync;
pub mod journal;
pub mod link_tracer;
pub mod live;
pub mod ops;
pub mod preset;
pub mod project_links;
pub mod pull;
pub mod remediate;
pub mod repair;
pub mod repo_health;
pub mod repo_move;
pub mod resolve;
pub mod roots;
pub mod service;
pub mod warehouse;

pub use service::ChainService;

use serde::Serialize;
use std::path::{Path, PathBuf};

use crate::core::skill_metadata::{is_valid_skill_dir, parse_skill_md};
use crate::core::tool_adapters::ToolAdapter;

/// One valid user Skill found on a global agent surface — an axiom-1 violation,
/// carried with enough evidence to identify and resolve it.
#[derive(Debug, Clone, Serialize)]
pub struct GuardViolation {
    /// Skill name from the entry's `SKILL.md`, falling back to its directory name.
    pub skill: String,
    /// The offending entry path inside the global surface.
    pub path: String,
    /// Final resolved target when the entry is a symlink; equals `path` for a
    /// physical directory. This is the evidence that the entry is a real Skill.
    pub final_target: String,
    /// Whether the valid Skill is reached through a symlink.
    pub is_link: bool,
}

/// One global agent surface watched by the axiom-1 guard.
#[derive(Debug, Clone, Serialize)]
pub struct GuardSurface {
    /// Human-facing Agent name from the adapter catalogue.
    pub agent: String,
    pub path: String,
    /// "empty" | "absent" | "violation"
    pub state: String,
    /// Valid Skills found on the surface when state == "violation".
    pub violations: Vec<GuardViolation>,
}

/// Agent surfaces governed by this installation's project-only policy. Vendor
/// managed surfaces (for example QoderWork built-ins) are intentionally outside
/// this guard: they are not a Patchbay-managed global whitelist surface.
const GUARDED_GLOBAL_ADAPTERS: &[&str] = &["claude_code", "codex", "github_copilot", "opencode"];

/// Check that every policy-governed global agent surface is free of real user Skills.
///
/// Surface paths come from the Agent adapter catalogue (never a second
/// hard-coded table), and an entry counts only when it resolves to a valid
/// Skill per the shared skill-format detector. Empty runtime directories,
/// metadata files, and broken links are compliant. Scanning is read-only.
pub fn global_guard(adapters: &[ToolAdapter]) -> Vec<GuardSurface> {
    adapters
        .iter()
        .filter(|adapter| GUARDED_GLOBAL_ADAPTERS.contains(&adapter.key.as_str()))
        .map(|adapter| evaluate_surface(&adapter.display_name, &adapter.skills_dir()))
        .collect()
}

/// Inspect one global surface directory for valid-Skill violations. Read-only.
fn evaluate_surface(agent: &str, path: &Path) -> GuardSurface {
    let (state, violations) = match std::fs::read_dir(path) {
        // Absent surface: the agent has no global skills directory at all.
        Err(_) => ("absent".to_string(), Vec::new()),
        Ok(read_dir) => {
            let mut violations: Vec<GuardViolation> = read_dir
                .flatten()
                .filter_map(|entry| {
                    let entry_path = entry.path();
                    // A directory or symlink is a violation only when it
                    // resolves to a valid Skill. `is_valid_skill_dir` follows
                    // symlinks, so broken links and empty runtime directories
                    // are naturally excluded.
                    if !is_valid_skill_dir(&entry_path) {
                        return None;
                    }
                    let trace = link_tracer::trace(&entry_path);
                    let skill = parse_skill_md(&entry_path)
                        .name
                        .unwrap_or_else(|| entry.file_name().to_string_lossy().to_string());
                    Some(GuardViolation {
                        skill,
                        path: entry_path.to_string_lossy().to_string(),
                        final_target: trace.final_target,
                        is_link: trace.is_link,
                    })
                })
                .collect();
            // Stable order regardless of the filesystem's readdir order.
            violations.sort_by(|a, b| a.path.cmp(&b.path));
            if violations.is_empty() {
                ("empty".to_string(), violations)
            } else {
                ("violation".to_string(), violations)
            }
        }
    };
    GuardSurface {
        agent: agent.to_string(),
        path: path.to_string_lossy().to_string(),
        state,
        violations,
    }
}

/// Per-root scan status surfaced to the UI so a missing or unreadable root is
/// shown explicitly instead of contributing nothing and looking empty.
#[derive(Debug, Clone, Serialize)]
pub struct RootScanStatus {
    pub root: String,
    /// "ok" | "missing" | "unreadable"
    pub status: String,
    pub error: Option<String>,
    pub repo_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChainTopology {
    /// Ordered, de-duplicated Original Repository roots with per-root status.
    pub warehouse_roots: Vec<RootScanStatus>,
    pub projects_root: String,
    pub repos: Vec<warehouse::RepoInfo>,
    pub projects: Vec<project_links::ProjectChain>,
    pub guard: Vec<GuardSurface>,
    pub scanned_at: i64,
}

/// Build the full topology from tier-1 sources (the managed central library plus
/// optional Original Repository roots) and the
/// registered project inventory (tiers 2/3). `projects_root` is retained only as
/// a display hint for shortening paths in the UI; the project inventory comes
/// from `project_paths`, not from re-reading that root. `adapters` supplies the
/// Agent catalogue the Global Guard uses to follow each Agent's real global
/// surface.
pub fn build_topology(
    warehouse_roots: &[PathBuf],
    managed_root: &Path,
    projects_root: &Path,
    project_paths: &[PathBuf],
    adapters: &[ToolAdapter],
) -> ChainTopology {
    // Canonical duplicates collapse so each distinct root is scanned once.
    let deduped = roots::dedupe(warehouse_roots);

    let mut repos = Vec::new();
    let mut root_status = Vec::with_capacity(deduped.len());
    for root in &deduped {
        let scan = warehouse::scan_root(root);
        root_status.push(RootScanStatus {
            root: scan.root,
            status: scan.status,
            error: scan.error,
            repo_count: scan.repos.len(),
        });
        repos.extend(scan.repos);
    }
    repos.push(warehouse::scan_managed_root(managed_root));

    // Stable order across roots; break name ties by path so same-named repos in
    // different roots stay deterministic.
    repos.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.path.cmp(&b.path)));

    let mut source_roots = deduped.clone();
    source_roots.push(managed_root.to_path_buf());
    let source_roots = roots::dedupe(&source_roots);
    let projects = project_links::discover(project_paths, &source_roots, &repos);
    for repo in repos.iter_mut() {
        // Reverse references key on the repository's canonical path, not its
        // name, so same-named repos in different roots are never conflated.
        let repo_path = PathBuf::from(&repo.path);
        repo.referenced_by = projects
            .iter()
            .filter(|p| project_references_repo(p, &repo_path))
            .map(|p| warehouse::ProjectRef {
                name: p.name.clone(),
                path: p.path.clone(),
            })
            .collect();
    }
    ChainTopology {
        warehouse_roots: root_status,
        projects_root: projects_root.to_string_lossy().to_string(),
        repos,
        projects,
        guard: global_guard(adapters),
        scanned_at: chrono::Utc::now().timestamp_millis(),
    }
}

/// Whether any of a project's traced entries resolve to a path inside `repo_path`.
/// Matching on the resolved final target (the canonical repository identity)
/// rather than the repo name keeps same-named repos in different roots distinct.
fn project_references_repo(p: &project_links::ProjectChain, repo_path: &Path) -> bool {
    let hits = |entries: &[project_links::TracedEntry]| {
        entries
            .iter()
            .any(|e| Path::new(&e.final_target).starts_with(repo_path))
    };
    p.agents_dir.as_ref().is_some_and(|a| hits(&a.entries))
        || p.surfaces.iter().any(|s| hits(&s.entries))
}

#[cfg(test)]
mod guard_tests {
    use super::{build_topology, global_guard, GuardSurface};
    use crate::core::tool_adapters::{default_tool_adapters, ToolAdapter};
    use std::fs;
    use std::path::Path;
    use tempfile::tempdir;

    /// A built-in adapter whose global skills dir is pinned to `surface`.
    fn adapter_pinned_to(key: &str, surface: &Path) -> ToolAdapter {
        let mut adapter = default_tool_adapters()
            .into_iter()
            .find(|a| a.key == key)
            .expect("known adapter key");
        adapter.override_skills_dir = Some(surface.to_string_lossy().into_owned());
        adapter
    }

    /// Write a minimal valid Skill (a directory containing `SKILL.md`).
    fn make_skill(dir: &Path, name: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: guard fixture\n---\n"),
        )
        .unwrap();
    }

    fn only_surface(surfaces: Vec<GuardSurface>) -> GuardSurface {
        assert_eq!(surfaces.len(), 1, "expected exactly one guarded surface");
        surfaces.into_iter().next().unwrap()
    }

    #[test]
    fn absent_surface_is_compliant() {
        let temp = tempdir().unwrap();
        let surface = temp.path().join("missing/skills");
        let adapter = adapter_pinned_to("claude_code", &surface);

        let guarded = only_surface(global_guard(&[adapter]));
        assert_eq!(guarded.agent, "Claude Code");
        assert_eq!(guarded.state, "absent");
        assert!(guarded.violations.is_empty());
    }

    #[test]
    fn empty_surface_is_compliant() {
        let temp = tempdir().unwrap();
        let surface = temp.path().join("skills");
        fs::create_dir_all(&surface).unwrap();
        let adapter = adapter_pinned_to("claude_code", &surface);

        let guarded = only_surface(global_guard(&[adapter]));
        assert_eq!(guarded.state, "empty");
        assert!(guarded.violations.is_empty());
    }

    #[test]
    fn empty_runtime_directory_and_metadata_are_not_violations() {
        // Regression: a runtime placeholder directory (no SKILL.md) and stray
        // metadata files must not be reported as Skills.
        let temp = tempdir().unwrap();
        let surface = temp.path().join("skills");
        fs::create_dir_all(surface.join("runtime")).unwrap();
        fs::write(surface.join("config.json"), "{}").unwrap();
        fs::write(surface.join(".DS_Store"), "").unwrap();
        let adapter = adapter_pinned_to("codex", &surface);

        let guarded = only_surface(global_guard(&[adapter]));
        assert_eq!(guarded.state, "empty");
        assert!(guarded.violations.is_empty());
    }

    #[test]
    fn valid_skill_directory_is_a_violation_with_evidence() {
        let temp = tempdir().unwrap();
        let surface = temp.path().join("skills");
        make_skill(&surface.join("rogue-skill"), "rogue-skill");
        let adapter = adapter_pinned_to("claude_code", &surface);

        let guarded = only_surface(global_guard(&[adapter]));
        assert_eq!(guarded.state, "violation");
        assert_eq!(guarded.violations.len(), 1);
        let violation = &guarded.violations[0];
        assert_eq!(violation.skill, "rogue-skill");
        assert_eq!(
            violation.path,
            surface.join("rogue-skill").to_string_lossy()
        );
        // Physical directory: evidence target is the entry itself, not a link.
        assert!(!violation.is_link);
        assert_eq!(violation.final_target, violation.path);
    }

    #[test]
    fn violations_are_sorted_for_stable_output() {
        let temp = tempdir().unwrap();
        let surface = temp.path().join("skills");
        make_skill(&surface.join("zeta"), "zeta");
        make_skill(&surface.join("alpha"), "alpha");
        let adapter = adapter_pinned_to("claude_code", &surface);

        let guarded = only_surface(global_guard(&[adapter]));
        let skills: Vec<&str> = guarded
            .violations
            .iter()
            .map(|v| v.skill.as_str())
            .collect();
        assert_eq!(skills, ["alpha", "zeta"]);
    }

    #[cfg(unix)]
    #[test]
    fn broken_symlink_is_not_a_violation() {
        let temp = tempdir().unwrap();
        let surface = temp.path().join("skills");
        fs::create_dir_all(&surface).unwrap();
        std::os::unix::fs::symlink(temp.path().join("nowhere"), surface.join("dangling")).unwrap();
        let adapter = adapter_pinned_to("claude_code", &surface);

        let guarded = only_surface(global_guard(&[adapter]));
        assert_eq!(guarded.state, "empty");
        assert!(guarded.violations.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_to_valid_skill_is_a_violation_with_resolved_evidence() {
        let temp = tempdir().unwrap();
        let original = temp.path().join("warehouse/real-skill");
        make_skill(&original, "real-skill");
        let surface = temp.path().join("skills");
        fs::create_dir_all(&surface).unwrap();
        std::os::unix::fs::symlink(&original, surface.join("linked")).unwrap();
        let adapter = adapter_pinned_to("opencode", &surface);

        let guarded = only_surface(global_guard(&[adapter]));
        assert_eq!(guarded.state, "violation");
        let violation = &guarded.violations[0];
        assert_eq!(violation.skill, "real-skill");
        assert!(violation.is_link);
        assert_eq!(violation.final_target, original.to_string_lossy());
    }

    #[test]
    fn surface_paths_come_from_the_adapter_catalogue() {
        // AC: paths come from the adapter catalogue, not a second hard-coded
        // table. OpenCode's real global surface is `.config/opencode/skills`,
        // never the project-local `.opencode/skills` the old table watched.
        let opencode = default_tool_adapters()
            .into_iter()
            .find(|a| a.key == "opencode")
            .unwrap();
        let expected_path = opencode.skills_dir().to_string_lossy().into_owned();

        let guarded = only_surface(global_guard(std::slice::from_ref(&opencode)));
        assert_eq!(guarded.agent, "OpenCode");
        assert_eq!(guarded.path, expected_path);
        assert!(
            guarded.path.contains("opencode"),
            "guard should watch an OpenCode path, got {}",
            guarded.path
        );
        // Compare by component: `guarded.path` is a native path string, so
        // `str::ends_with("/.opencode/skills")` can never match on Windows and
        // the negation would pass for free.
        assert!(
            !Path::new(&guarded.path).ends_with(".opencode/skills"),
            "guard must not watch OpenCode's project-local path, got {}",
            guarded.path
        );
    }

    #[test]
    fn valid_skill_under_alternate_global_path_is_detected() {
        // A Skill placed under OpenCode's actual (`.config/...`) global path is
        // detected — the concrete audit regression this ticket fixes.
        let temp = tempdir().unwrap();
        let surface = temp.path().join(".config/opencode/skills");
        make_skill(&surface.join("stowaway"), "stowaway");
        let adapter = adapter_pinned_to("opencode", &surface);

        let guarded = only_surface(global_guard(&[adapter]));
        assert_eq!(guarded.state, "violation");
        assert_eq!(guarded.violations[0].skill, "stowaway");
    }

    #[test]
    fn build_topology_wires_the_adapter_driven_guard() {
        let temp = tempdir().unwrap();
        let warehouse = temp.path().join("warehouse");
        let projects = temp.path().join("projects");
        let managed = temp.path().join("central");
        fs::create_dir_all(&warehouse).unwrap();
        fs::create_dir_all(&projects).unwrap();
        fs::create_dir_all(&managed).unwrap();
        let surface = temp.path().join("skills");
        make_skill(&surface.join("leaked"), "leaked");
        let adapter = adapter_pinned_to("claude_code", &surface);

        let topo = build_topology(
            std::slice::from_ref(&warehouse),
            &managed,
            &projects,
            &[],
            &[adapter],
        );
        let guarded = only_surface(topo.guard);
        assert_eq!(guarded.state, "violation");
        assert_eq!(guarded.violations[0].skill, "leaked");
    }

    #[test]
    fn vendor_managed_global_surfaces_are_outside_the_guard_policy() {
        let temp = tempdir().unwrap();
        let surface = temp.path().join(".qoderwork/skills");
        make_skill(&surface.join("builtin"), "builtin");
        let adapter = adapter_pinned_to("qoderwork", &surface);

        assert!(global_guard(&[adapter]).is_empty());
    }
}
