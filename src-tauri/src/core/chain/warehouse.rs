//! Tier-1 scan: enumerate original skill repos under the warehouse root.
//! Originals are physical directories only — symlinked aliases inside the
//! warehouse would double-count and are skipped.

use serde::Serialize;
use std::path::Path;

use super::repo_health::{self, RepoHealth, RepoRemote};
use crate::core::skill_metadata;

/// Non-hidden directories never descended into (hidden dirs are always skipped).
const SKIP_DIRS: &[&str] = &["node_modules", "venv", "target", "dist", "build", "out"];
const MAX_DEPTH: usize = 4;

#[derive(Debug, Clone, Serialize)]
pub struct RepoSkill {
    pub name: String,
    pub path: String,
}

/// A registered project that depends on a repository, identified by its
/// canonical path (not its display name) so same-named projects at different
/// paths are never conflated.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ProjectRef {
    /// Display name of the project.
    pub name: String,
    /// Canonical project path — the identity used for reverse references.
    pub path: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RepoInfo {
    pub name: String,
    pub path: String,
    /// `managed` for Patchbay's central library, `checkout` for a developer
    /// Git repository discovered under a configured warehouse root.
    pub source_kind: String,
    /// Warehouse root this repo was discovered under — its source root, so the
    /// UI can attribute each Skill to the correct configured root.
    pub root: String,
    /// Read-only Git health: working-tree cleanliness plus the current branch's
    /// position against its configured upstream (ahead/behind/diverged/missing
    /// tracking/scan-error).
    pub health: RepoHealth,
    /// `origin` remote identity, when configured.
    pub origin: Option<RepoRemote>,
    /// `upstream` remote identity, shown distinctly from `origin` so a fork's
    /// source is visible. `None` when the repo has no `upstream` remote.
    pub upstream: Option<RepoRemote>,
    pub skills: Vec<RepoSkill>,
    /// Registered projects with at least one link resolving into this repo,
    /// by canonical identity. Filled by the topology assembler, not the scan.
    pub referenced_by: Vec<ProjectRef>,
}

/// Outcome of scanning a single warehouse root. Carries explicit status so a
/// missing or unreadable root is never indistinguishable from an empty one.
#[derive(Debug, Clone)]
pub struct RootScan {
    pub root: String,
    /// "ok" | "missing" | "unreadable"
    pub status: String,
    pub error: Option<String>,
    pub repos: Vec<RepoInfo>,
}

/// Enumerate original skill repos directly under one warehouse root, tagging
/// each with its source root. Reports `missing`/`unreadable` instead of
/// silently yielding no repos when the root cannot be read.
pub fn scan_root(root: &Path) -> RootScan {
    let root_str = root.to_string_lossy().to_string();
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) => {
            let status = if root.exists() {
                "unreadable"
            } else {
                "missing"
            };
            return RootScan {
                root: root_str,
                status: status.to_string(),
                error: Some(e.to_string()),
                repos: Vec::new(),
            };
        }
    };
    let mut repos = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let is_symlink = std::fs::symlink_metadata(&path)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(true);
        if is_symlink || !path.is_dir() || !path.join(".git").exists() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let mut skills = Vec::new();
        collect_skills(&path, 0, &mut skills);
        skills.sort_by(|a, b| a.name.cmp(&b.name));
        // Read-only Git health per repo. Any git2 failure collapses to a
        // scan-error health inside `inspect`, so one bad checkout never aborts
        // the scan of its siblings.
        let git = repo_health::inspect(&path);
        repos.push(RepoInfo {
            name,
            path: path.to_string_lossy().to_string(),
            source_kind: "checkout".to_string(),
            root: root_str.clone(),
            health: git.health,
            origin: git.origin,
            upstream: git.upstream,
            skills,
            referenced_by: Vec::new(),
        });
    }
    repos.sort_by(|a, b| a.name.cmp(&b.name));
    RootScan {
        root: root_str,
        status: "ok".to_string(),
        error: None,
        repos,
    }
}

/// Represent Patchbay's central library as the default tier-1 source. Skills in
/// this managed store are direct physical children; unlike developer checkouts,
/// the library is not exposed to raw pull/fork operations by the chain UI.
pub fn scan_managed_root(root: &Path) -> RepoInfo {
    let mut skills = Vec::new();
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let path = entry.path();
            let is_symlink = std::fs::symlink_metadata(&path)
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(true);
            if name.starts_with('.')
                || is_symlink
                || !path.is_dir()
                || !skill_metadata::is_valid_skill_dir(&path)
            {
                continue;
            }
            skills.push(RepoSkill {
                name,
                path: path.to_string_lossy().to_string(),
            });
        }
    }
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    let path = root.to_string_lossy().to_string();
    RepoInfo {
        name: "Patchbay Central".to_string(),
        path: path.clone(),
        source_kind: "managed".to_string(),
        root: path,
        health: RepoHealth {
            dirty: false,
            state: "up_to_date".to_string(),
            ahead: 0,
            behind: 0,
            branch: None,
            error: None,
        },
        origin: None,
        upstream: None,
        skills,
        referenced_by: Vec::new(),
    }
}

fn collect_skills(dir: &Path, depth: usize, out: &mut Vec<RepoSkill>) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || SKIP_DIRS.contains(&name.as_str()) {
            continue;
        }
        let path = entry.path();
        let is_symlink = std::fs::symlink_metadata(&path)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(true);
        if is_symlink || !path.is_dir() {
            continue;
        }
        if skill_metadata::is_valid_skill_dir(&path) {
            out.push(RepoSkill {
                name,
                path: path.to_string_lossy().to_string(),
            });
            continue;
        }
        collect_skills(&path, depth + 1, out);
    }
}
