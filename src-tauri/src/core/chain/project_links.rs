//! Tier-2/3 scan: per-project aggregate dirs (`.agents/skills`) and per-agent
//! entry surfaces (`.claude/skills`, ...), with every entry's symlink chain
//! traced and classified against the convention.

use serde::Serialize;
use std::path::{Path, PathBuf};

use super::link_tracer;
use super::warehouse::RepoInfo;

pub const AGENT_SURFACES: &[(&str, &str)] = &[
    ("claude", ".claude/skills"),
    ("codex", ".codex/skills"),
    ("copilot", ".copilot/skills"),
    ("opencode", ".opencode/skills"),
    ("qoderwork", ".qoder/skills"),
];

/// Absolute path of one [`AGENT_SURFACES`] entry inside `project`.
///
/// The table stores POSIX-shaped relatives, and `Path::join` keeps that `/`
/// verbatim, so `project.join(".claude/skills")` yields
/// `C:\proj\.claude/skills` on Windows — a path that works for filesystem calls
/// but never string-compares equal to anything built component-wise. It reaches
/// the GUI, the audit log, every repair item and every link-report entry, so
/// join per component and keep all four consumers going through here.
pub(super) fn surface_path(project: &Path, rel: &str) -> PathBuf {
    rel.split('/')
        .fold(project.to_path_buf(), |acc, part| acc.join(part))
}

#[derive(Debug, Clone, Serialize)]
pub struct TracedEntry {
    pub name: String,
    pub entry_path: String,
    pub hops: Vec<String>,
    pub final_target: String,
    /// Aggregate entries: "link_repo" | "private" | "internal" | "external" | "broken"
    /// Surface entries:  "via_agents" | "direct" | "copy" | "internal" | "external" | "broken"
    pub status: String,
    /// Warehouse repo name when the final target resolves inside one.
    pub repo: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AggregateDir {
    pub path: String,
    pub entries: Vec<TracedEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentSurface {
    pub agent: String,
    pub path: String,
    /// "dir_link" (surface itself is a symlink) | "per_entry" | "absent"
    pub kind: String,
    pub dir_link_target: Option<String>,
    /// dir_link only: resolves to this project's `.agents/skills` and exists.
    pub dir_link_ok: bool,
    /// per_entry only.
    pub entries: Vec<TracedEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProjectChain {
    pub name: String,
    pub path: String,
    pub agents_dir: Option<AggregateDir>,
    pub surfaces: Vec<AgentSurface>,
}

/// Scan the registered project inventory into per-project chains.
///
/// Each path is a project the user has enrolled for chain management. Every one
/// participates regardless of its parent directory, and appears even before any
/// skill is linked so that it can be managed. Callers pass canonically
/// de-duplicated, as-stored paths (see `ChainService`); this function does not
/// re-read a discovery root or filter by folder contents. `warehouse_roots`
/// classifies where each linked skill resolves across all Original Repository
/// roots, including Patchbay's managed central library.
pub fn discover(
    projects: &[PathBuf],
    warehouse_roots: &[PathBuf],
    repos: &[RepoInfo],
) -> Vec<ProjectChain> {
    let mut out: Vec<ProjectChain> = projects
        .iter()
        .map(|path| {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| path.to_string_lossy().to_string());
            scan_project(&name, path, warehouse_roots, repos)
        })
        .collect();
    // Same-named projects at different paths stay distinct; order by name then
    // path so the listing is stable and deterministic.
    out.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.path.cmp(&b.path)));
    out
}

fn scan_project(
    name: &str,
    path: &Path,
    warehouse_roots: &[PathBuf],
    repos: &[RepoInfo],
) -> ProjectChain {
    let agg_abs = path.join(".agents").join("skills");
    let agents_dir = if agg_abs.is_dir() {
        Some(AggregateDir {
            path: agg_abs.to_string_lossy().to_string(),
            entries: scan_entries(&agg_abs, path, &agg_abs, warehouse_roots, repos, true),
        })
    } else {
        None
    };

    let surfaces = AGENT_SURFACES
        .iter()
        .map(|(agent, rel)| {
            let surface_abs = surface_path(path, rel);
            scan_surface(agent, &surface_abs, path, &agg_abs, warehouse_roots, repos)
        })
        .collect();

    ProjectChain {
        name: name.to_string(),
        path: path.to_string_lossy().to_string(),
        agents_dir,
        surfaces,
    }
}

fn scan_surface(
    agent: &str,
    surface_path: &Path,
    project_root: &Path,
    agg_abs: &Path,
    warehouse_roots: &[PathBuf],
    repos: &[RepoInfo],
) -> AgentSurface {
    let base = AgentSurface {
        agent: agent.to_string(),
        path: surface_path.to_string_lossy().to_string(),
        kind: "absent".to_string(),
        dir_link_target: None,
        dir_link_ok: false,
        entries: Vec::new(),
    };
    let Ok(meta) = std::fs::symlink_metadata(surface_path) else {
        return base;
    };
    if meta.file_type().is_symlink() {
        let tr = link_tracer::trace(surface_path);
        let ok = tr.exists && Path::new(&tr.final_target) == agg_abs;
        return AgentSurface {
            kind: "dir_link".to_string(),
            dir_link_target: Some(tr.final_target),
            dir_link_ok: ok,
            ..base
        };
    }
    if meta.is_dir() {
        return AgentSurface {
            kind: "per_entry".to_string(),
            entries: scan_entries(
                surface_path,
                project_root,
                agg_abs,
                warehouse_roots,
                repos,
                false,
            ),
            ..base
        };
    }
    base
}

fn scan_entries(
    dir: &Path,
    project_root: &Path,
    agg_abs: &Path,
    warehouse_roots: &[PathBuf],
    repos: &[RepoInfo],
    is_aggregate: bool,
) -> Vec<TracedEntry> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        let tr = link_tracer::trace(&path);
        if !tr.is_link && !path.is_dir() {
            // stray plain files (README etc.) are not skills
            continue;
        }
        let final_path = PathBuf::from(&tr.final_target);
        let status = classify(
            &tr,
            &final_path,
            project_root,
            agg_abs,
            warehouse_roots,
            is_aggregate,
        );
        out.push(TracedEntry {
            name,
            entry_path: path.to_string_lossy().to_string(),
            hops: tr.hops,
            final_target: tr.final_target,
            status,
            repo: repo_for(&final_path, repos),
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn classify(
    tr: &link_tracer::Trace,
    final_path: &Path,
    project_root: &Path,
    agg_abs: &Path,
    warehouse_roots: &[PathBuf],
    is_aggregate: bool,
) -> String {
    if !tr.is_link {
        return if is_aggregate { "private" } else { "copy" }.to_string();
    }
    if !tr.exists || tr.cyclic {
        return "broken".to_string();
    }
    // A surface entry is canonical when its first hop lands in `.agents/skills`,
    // regardless of where the aggregate link ultimately resolves (a physical
    // aggregate stops there; a linked aggregate continues into the warehouse).
    if !is_aggregate
        && tr
            .hops
            .first()
            .is_some_and(|hop| Path::new(hop).starts_with(agg_abs))
    {
        return "via_agents".to_string();
    }
    if warehouse_roots
        .iter()
        .any(|root| final_path.starts_with(root))
    {
        return if is_aggregate { "link_repo" } else { "direct" }.to_string();
    }
    if final_path.starts_with(project_root) {
        return "internal".to_string();
    }
    "external".to_string()
}

fn repo_for(target: &Path, repos: &[RepoInfo]) -> Option<String> {
    repos
        .iter()
        .find(|r| target.starts_with(Path::new(&r.path)))
        .map(|r| r.name.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Portable stand-in for `std::os::unix::fs::symlink`. These fixtures link
    /// directories, and gating the module on unix meant they never ran on
    /// Windows — the platform whose symlink semantics differ most.
    fn symlink(
        target: impl AsRef<std::path::Path>,
        link: impl AsRef<std::path::Path>,
    ) -> std::io::Result<()> {
        crate::core::test_support::symlink_dir(target.as_ref(), link.as_ref())
    }
    use tempfile::tempdir;

    fn make_skill(dir: &Path, name: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: links fixture\n---\n"),
        )
        .unwrap();
    }

    /// Physical surface, per-skill entry hopping through a *linked* aggregate:
    /// the canonical layout `chain link` creates on physical surfaces. This was
    /// misread as "direct" while classification keyed off the final target
    /// (which resolves into the warehouse) instead of the first hop.
    #[test]
    fn per_entry_link_through_linked_aggregate_is_via_agents() {
        let temp = tempdir().unwrap();
        let warehouse = temp.path().join("warehouse");
        let original = warehouse.join("repo").join("skills").join("demo");
        make_skill(&original, "demo");

        let project = temp.path().join("proj");
        let agg = project.join(".agents").join("skills");
        std::fs::create_dir_all(&agg).unwrap();
        symlink(&original, agg.join("demo")).unwrap();

        let surface = project.join(".claude").join("skills");
        std::fs::create_dir_all(&surface).unwrap();
        symlink(Path::new("../../.agents/skills/demo"), surface.join("demo")).unwrap();
        // A genuinely direct entry (skips the aggregate) must stay "direct".
        symlink(&original, surface.join("straight")).unwrap();

        let chains = discover(&[project.clone()], &[warehouse.clone()], &[]);
        let claude = chains[0]
            .surfaces
            .iter()
            .find(|s| s.agent == "claude")
            .unwrap();
        assert_eq!(claude.kind, "per_entry");
        let status = |name: &str| {
            claude
                .entries
                .iter()
                .find(|e| e.name == name)
                .unwrap()
                .status
                .clone()
        };
        assert_eq!(status("demo"), "via_agents");
        assert_eq!(status("straight"), "direct");
    }
}
