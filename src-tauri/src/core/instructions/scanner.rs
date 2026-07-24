//! Read-only scanner for the instructions governance surface (design §1–§2, §5).
//!
//! Produces, per registered project: the canonical body, each installed agent's
//! entry state, the per-agent resident-set cost (project + global), and the
//! personal/unmanaged-layer inventory; plus the machine-level global surfaces
//! with their reader sets. Every path here is read-only — there is no filesystem
//! write anywhere in this module (P0 release gate).
//!
//! The resident-set arithmetic follows the §2 table literally, agent by agent,
//! rather than through a shared abstraction: each agent's surface has genuinely
//! different rules (Claude's `@import` expansion and `paths:`-gated rules,
//! Codex's first-non-empty global, OpenCode's AGENTS-then-CLAUDE fallback and
//! its extra read of `~/.claude/CLAUDE.md`, Antigravity's dual read).

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::import_resolver::{self, Import};
use super::surfaces::Agent;
use super::token_estimate::{est_tokens, est_tokens_bytes};

// ── output types (CLI/GUI contract, design §5) ─────────────────────────────

/// Full scan payload: one entry per scanned project, the machine's global
/// surfaces, the installed-agent set, and a scan timestamp.
#[derive(Debug, Clone, Serialize)]
pub struct ScanReport {
    pub projects: Vec<ProjectScan>,
    pub globals: Vec<GlobalFile>,
    /// Installed agent keys, in catalogue order.
    pub agents: Vec<String>,
    pub scanned_at: i64,
}

/// One project's instructions surface.
#[derive(Debug, Clone, Serialize)]
pub struct ProjectScan {
    pub path: String,
    pub canonical: CanonicalInfo,
    pub entries: Vec<EntryInfo>,
    pub resident: Vec<ResidentInfo>,
    /// Personal/agent-config layer (only counted, never governed — §1 scope).
    pub unmanaged: Vec<UnmanagedFile>,
}

/// The canonical body `<project>/AGENTS.md`.
#[derive(Debug, Clone, Serialize)]
pub struct CanonicalInfo {
    pub exists: bool,
    pub path: String,
    pub bytes: u64,
    pub lines: usize,
    pub est_tokens: u64,
}

/// One installed agent's primary entry into a project.
#[derive(Debug, Clone, Serialize)]
pub struct EntryInfo {
    /// Agent key.
    pub agent: String,
    /// `wrapper | wrapper_plus | symlink | body | missing | native`.
    pub state: String,
    pub path: String,
    pub bytes: u64,
    pub est_tokens: u64,
}

/// One installed agent's resident-set cost, split into project-side and
/// global-side bytes with a combined token estimate (§2).
#[derive(Debug, Clone, Serialize)]
pub struct ResidentInfo {
    pub agent: String,
    pub project_bytes: u64,
    pub global_bytes: u64,
    pub est_tokens: u64,
}

/// A personal/unmanaged-layer file, reported for cost visibility only.
#[derive(Debug, Clone, Serialize)]
pub struct UnmanagedFile {
    pub agent: String,
    pub path: String,
    pub bytes: u64,
    pub est_tokens: u64,
}

/// A machine-level global instructions surface and the installed agents that
/// read it.
#[derive(Debug, Clone, Serialize)]
pub struct GlobalFile {
    pub path: String,
    pub bytes: u64,
    pub est_tokens: u64,
    /// Installed agent keys that load this file, in catalogue order.
    pub readers: Vec<String>,
}

/// `where` payload: each agent's ordered read chain (§5).
#[derive(Debug, Clone, Serialize)]
pub struct AgentReadChain {
    pub agent: String,
    pub files: Vec<ReadFile>,
}

/// One file in an agent's read chain, tagged by the role it plays.
#[derive(Debug, Clone, Serialize)]
pub struct ReadFile {
    pub path: String,
    /// `canonical | entry | append | import | global | conditional`.
    pub role: String,
    pub exists: bool,
    pub bytes: u64,
    /// Import depth for `import` files; 0 for directly-read files.
    pub hop: usize,
}

// ── low-level filesystem helpers ────────────────────────────────────────────

struct FileMeta {
    exists: bool,
    bytes: u64,
    est_tokens: u64,
}

/// Read a file's size and token estimate, following symlinks (an agent reads the
/// resolved content). Missing or unreadable files report zero.
fn file_meta(path: &Path) -> FileMeta {
    match std::fs::read(path) {
        Ok(data) => FileMeta {
            exists: true,
            bytes: data.len() as u64,
            est_tokens: est_tokens_bytes(&data),
        },
        Err(_) => FileMeta {
            exists: false,
            bytes: 0,
            est_tokens: 0,
        },
    }
}

/// Whether a path exists as an entry shape — a regular file OR a symlink,
/// including a broken one (its shape still matters for classification).
fn entry_exists(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

/// Whether a regular file exists and is non-empty (Codex's "first non-empty").
fn file_nonempty(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.len() > 0)
        .unwrap_or(false)
}

/// Top-level `*.md` files in `dir`, sorted for deterministic output.
fn md_files_in(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_file() && p.extension().is_some_and(|x| x == "md") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// Top-level files (any extension) in `dir`, sorted.
fn files_in(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_file() {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// Whether a markdown rule file carries a `paths:` key in its YAML front matter
/// (design §2: `paths:`-gated rules load conditionally and are NOT counted in
/// the resident set). Files without front matter, or unreadable ones, count as
/// resident (return false).
fn has_paths_frontmatter(path: &Path) -> bool {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => return false,
    };
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        return false;
    }
    for line in lines {
        if line.trim() == "---" {
            break;
        }
        let key = line.trim_start();
        if key.starts_with("paths:") || key.starts_with("paths :") {
            return true;
        }
    }
    false
}

/// Sum bytes and token estimate over a set of existing files.
fn sum_meta(paths: &[PathBuf]) -> (u64, u64) {
    let mut bytes = 0;
    let mut tokens = 0;
    for p in paths {
        let m = file_meta(p);
        bytes += m.bytes;
        tokens += m.est_tokens;
    }
    (bytes, tokens)
}

/// De-duplicate a path list, preserving first-seen order.
fn dedup_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for p in paths {
        if seen.insert(p.clone()) {
            out.push(p);
        }
    }
    out
}

// ── canonical + entry classification ────────────────────────────────────────

fn canonical_path(root: &Path) -> PathBuf {
    root.join("AGENTS.md")
}

fn canonical_info(root: &Path) -> CanonicalInfo {
    let path = canonical_path(root);
    let (exists, bytes, lines, est) = match std::fs::read(&path) {
        Ok(data) => {
            let text = String::from_utf8_lossy(&data);
            let lines = text.lines().count();
            (true, data.len() as u64, lines, est_tokens(&text))
        }
        Err(_) => (false, 0, 0, 0),
    };
    CanonicalInfo {
        exists,
        path: path.to_string_lossy().to_string(),
        bytes,
        lines,
        est_tokens: est,
    }
}

/// Claude's two candidate entry locations, in preference order.
fn claude_entry_locations(root: &Path) -> [PathBuf; 2] {
    [root.join("CLAUDE.md"), root.join(".claude/CLAUDE.md")]
}

/// Existing Claude entry files (both locations are scanned — design §1).
fn claude_existing_entries(root: &Path) -> Vec<PathBuf> {
    claude_entry_locations(root)
        .into_iter()
        .filter(|p| entry_exists(p))
        .collect()
}

/// Classify one Claude entry file's shape into an entry state.
fn claude_entry_state(entry: &Path) -> String {
    match std::fs::symlink_metadata(entry) {
        Err(_) => return "missing".to_string(),
        Ok(md) if md.file_type().is_symlink() => return "symlink".to_string(),
        Ok(_) => {}
    }
    let text = std::fs::read_to_string(entry).unwrap_or_default();
    let non_empty: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    match non_empty.first() {
        Some(&"@AGENTS.md") => {
            if non_empty.len() > 1 {
                "wrapper_plus".to_string()
            } else {
                "wrapper".to_string()
            }
        }
        // Real content living in the entry (dual-body / missing-canonical case),
        // or an empty file — both are a physical body, not a wrapper.
        _ => "body".to_string(),
    }
}

/// The file a native-reading agent actually loads as its project source, and
/// whether it exists. OpenCode falls back to `CLAUDE.md` when `AGENTS.md` is
/// absent (first-wins); the others read `AGENTS.md` only.
fn native_read_path(agent: Agent, root: &Path) -> (PathBuf, bool) {
    let agents_md = canonical_path(root);
    if agent == Agent::Opencode && !agents_md.exists() {
        let claude_md = root.join("CLAUDE.md");
        if claude_md.exists() {
            return (claude_md, true);
        }
    }
    let exists = agents_md.exists();
    (agents_md, exists)
}

fn entries_for(root: &Path, installed: &[Agent]) -> Vec<EntryInfo> {
    let mut entries = Vec::new();
    for &agent in installed {
        if agent == Agent::Claude {
            let existing = claude_existing_entries(root);
            if existing.is_empty() {
                let expected = claude_entry_locations(root)[0].clone();
                entries.push(EntryInfo {
                    agent: agent.key().to_string(),
                    state: "missing".to_string(),
                    path: expected.to_string_lossy().to_string(),
                    bytes: 0,
                    est_tokens: 0,
                });
            } else {
                for loc in existing {
                    let m = file_meta(&loc);
                    entries.push(EntryInfo {
                        agent: agent.key().to_string(),
                        state: claude_entry_state(&loc),
                        path: loc.to_string_lossy().to_string(),
                        bytes: m.bytes,
                        est_tokens: m.est_tokens,
                    });
                }
            }
        } else {
            let (read_path, exists) = native_read_path(agent, root);
            let (state, m) = if exists {
                ("native".to_string(), file_meta(&read_path))
            } else {
                (
                    "missing".to_string(),
                    FileMeta {
                        exists: false,
                        bytes: 0,
                        est_tokens: 0,
                    },
                )
            };
            entries.push(EntryInfo {
                agent: agent.key().to_string(),
                state,
                path: read_path.to_string_lossy().to_string(),
                bytes: m.bytes,
                est_tokens: m.est_tokens,
            });
        }
    }
    entries
}

// ── resident set collectors (design §2) ─────────────────────────────────────

/// Merged, de-duplicated `@import` targets reachable from a set of Claude entry
/// files. Only existing targets are returned (missing imports add no cost).
fn merged_imports(entries: &[PathBuf], home: &Path) -> Vec<Import> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for entry in entries {
        for imp in import_resolver::resolve(entry, home).imports {
            if seen.insert(imp.target.clone()) {
                out.push(imp);
            }
        }
    }
    out
}

/// Claude project-side resident files: the entry file(s) + expanded imports +
/// `CLAUDE.local.md` + `.claude/rules/*.md` without a `paths:` gate.
fn claude_project_resident(root: &Path, home: &Path) -> Vec<PathBuf> {
    let entries = claude_existing_entries(root);
    let mut files = entries.clone();
    for imp in merged_imports(&entries, home) {
        if imp.exists {
            files.push(PathBuf::from(imp.target));
        }
    }
    let local = root.join("CLAUDE.local.md");
    if local.exists() {
        files.push(local);
    }
    for rule in md_files_in(&root.join(".claude/rules")) {
        if !has_paths_frontmatter(&rule) {
            files.push(rule);
        }
    }
    dedup_paths(files)
}

/// Claude global-side resident files: `~/.claude/CLAUDE.md` + `~/.claude/rules/`
/// entries without a `paths:` gate. (The enterprise "managed policy surface"
/// named in §2 has no documented concrete path and is omitted in v1 — logged as
/// a ticket deviation.)
fn claude_global_resident(home: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let global = home.join(".claude/CLAUDE.md");
    if global.exists() {
        files.push(global);
    }
    for rule in md_files_in(&home.join(".claude/rules")) {
        if !has_paths_frontmatter(&rule) {
            files.push(rule);
        }
    }
    files
}

/// Files that make up an agent's project-side resident set (existing files).
fn project_resident(agent: Agent, root: &Path, home: &Path) -> Vec<PathBuf> {
    let exists = |rel: &str| {
        let p = root.join(rel);
        p.exists().then_some(p)
    };
    match agent {
        Agent::Claude => claude_project_resident(root, home),
        Agent::Codex => ["AGENTS.md", "AGENTS.override.md"]
            .iter()
            .filter_map(|r| exists(r))
            .collect(),
        Agent::Copilot => ["AGENTS.md", ".github/copilot-instructions.md"]
            .iter()
            .filter_map(|r| exists(r))
            .collect(),
        // OpenCode reads AGENTS.md, else CLAUDE.md (first-wins single source).
        Agent::Opencode => {
            if let Some(p) = exists("AGENTS.md") {
                vec![p]
            } else {
                exists("CLAUDE.md").into_iter().collect()
            }
        }
        Agent::Antigravity => ["AGENTS.md", "GEMINI.md"]
            .iter()
            .filter_map(|r| exists(r))
            .collect(),
    }
}

/// Files that make up an agent's global-side resident set (existing files).
fn global_resident(agent: Agent, home: &Path) -> Vec<PathBuf> {
    let exists = |rel: &str| {
        let p = home.join(rel);
        p.exists().then_some(p)
    };
    match agent {
        Agent::Claude => claude_global_resident(home),
        // First non-empty of the base then the override.
        Agent::Codex => {
            for rel in [".codex/AGENTS.md", ".codex/AGENTS.override.md"] {
                let p = home.join(rel);
                if file_nonempty(&p) {
                    return vec![p];
                }
            }
            vec![]
        }
        Agent::Copilot => exists(".copilot/copilot-instructions.md")
            .into_iter()
            .collect(),
        // OpenCode also loads Claude's global file.
        Agent::Opencode => [".config/opencode/AGENTS.md", ".claude/CLAUDE.md"]
            .iter()
            .filter_map(|r| exists(r))
            .collect(),
        Agent::Antigravity => exists(".gemini/GEMINI.md").into_iter().collect(),
    }
}

fn resident_for(root: &Path, home: &Path, installed: &[Agent]) -> Vec<ResidentInfo> {
    installed
        .iter()
        .map(|&agent| {
            let (project_bytes, project_tokens) = sum_meta(&project_resident(agent, root, home));
            let (global_bytes, global_tokens) = sum_meta(&global_resident(agent, home));
            ResidentInfo {
                agent: agent.key().to_string(),
                project_bytes,
                global_bytes,
                est_tokens: project_tokens + global_tokens,
            }
        })
        .collect()
}

// ── unmanaged (personal) layer ──────────────────────────────────────────────

/// Personal/agent-config files for one agent in a project — listed for cost, not
/// governed (design §1 scope; §8 never-touch list).
fn unmanaged_paths(agent: Agent, root: &Path) -> Vec<PathBuf> {
    match agent {
        Agent::Claude => {
            let mut files = Vec::new();
            let local = root.join("CLAUDE.local.md");
            if local.exists() {
                files.push(local);
            }
            files.extend(md_files_in(&root.join(".claude/rules")));
            files
        }
        Agent::Codex => {
            let p = root.join("AGENTS.override.md");
            p.exists().then_some(p).into_iter().collect()
        }
        Agent::Opencode => {
            let p = root.join("opencode.json");
            p.exists().then_some(p).into_iter().collect()
        }
        Agent::Antigravity => files_in(&root.join(".agents/rules")),
        Agent::Copilot => vec![],
    }
}

fn unmanaged_for(root: &Path, installed: &[Agent]) -> Vec<UnmanagedFile> {
    let mut out = Vec::new();
    for &agent in installed {
        for path in unmanaged_paths(agent, root) {
            let m = file_meta(&path);
            out.push(UnmanagedFile {
                agent: agent.key().to_string(),
                path: path.to_string_lossy().to_string(),
                bytes: m.bytes,
                est_tokens: m.est_tokens,
            });
        }
    }
    out
}

// ── globals (machine level) ─────────────────────────────────────────────────

/// Global surfaces across all installed agents, each with the reader set that
/// loads it. The union of every installed agent's global-resident files; a file
/// read by several agents (e.g. `~/.claude/CLAUDE.md` by claude and opencode)
/// appears once with both readers.
fn globals_for(home: &Path, installed: &[Agent]) -> Vec<GlobalFile> {
    // Preserve catalogue order of readers by inserting agents in ALL order.
    let mut readers: BTreeMap<PathBuf, Vec<String>> = BTreeMap::new();
    for &agent in installed {
        for path in global_resident(agent, home) {
            readers
                .entry(path)
                .or_default()
                .push(agent.key().to_string());
        }
    }
    readers
        .into_iter()
        .map(|(path, readers)| {
            let m = file_meta(&path);
            GlobalFile {
                path: path.to_string_lossy().to_string(),
                bytes: m.bytes,
                est_tokens: m.est_tokens,
                readers,
            }
        })
        .collect()
}

// ── top-level scan ──────────────────────────────────────────────────────────

fn scan_project(root: &Path, home: &Path, installed: &[Agent]) -> ProjectScan {
    ProjectScan {
        path: root.to_string_lossy().to_string(),
        canonical: canonical_info(root),
        entries: entries_for(root, installed),
        resident: resident_for(root, home, installed),
        unmanaged: unmanaged_for(root, installed),
    }
}

/// Scan the given project roots against `home` and the installed-agent set.
/// Read-only. `scanned_at` is a caller-supplied timestamp so the function stays
/// deterministic for tests.
pub fn scan_with(
    project_paths: &[PathBuf],
    home: &Path,
    installed: &[Agent],
    scanned_at: i64,
) -> ScanReport {
    ScanReport {
        projects: project_paths
            .iter()
            .map(|root| scan_project(root, home, installed))
            .collect(),
        globals: globals_for(home, installed),
        agents: installed.iter().map(|a| a.key().to_string()).collect(),
        scanned_at,
    }
}

// ── where (per-agent read chain, design §5) ─────────────────────────────────

fn read_file(path: &Path, role: &str, hop: usize) -> ReadFile {
    let m = file_meta(path);
    ReadFile {
        path: path.to_string_lossy().to_string(),
        role: role.to_string(),
        exists: m.exists,
        bytes: m.bytes,
        hop,
    }
}

/// Build Claude's read chain: entry wrapper(s) → imports (the canonical body via
/// its `@AGENTS.md` import, plus any deeper imports) → personal append files →
/// `paths:`-gated conditional rules → global surfaces.
fn claude_read_chain(root: &Path, home: &Path) -> Vec<ReadFile> {
    let mut files = Vec::new();
    let entries = claude_existing_entries(root);
    if entries.is_empty() {
        files.push(read_file(&claude_entry_locations(root)[0], "entry", 0));
    } else {
        for e in &entries {
            files.push(read_file(e, "entry", 0));
        }
    }
    let canonical = canonical_path(root);
    for imp in merged_imports(&entries, home) {
        let target = PathBuf::from(&imp.target);
        // The wrapper's `@AGENTS.md` reaches the canonical body; deeper imports
        // are ordinary import content.
        let role = if target == canonical {
            "canonical"
        } else {
            "import"
        };
        let mut file = read_file(&target, role, imp.hop);
        // Trust the resolver's existence verdict (it already stat'd the target).
        file.exists = imp.exists;
        files.push(file);
    }
    // Personal append files (always loaded, project side).
    let local = root.join("CLAUDE.local.md");
    if local.exists() {
        files.push(read_file(&local, "append", 0));
    }
    let rules_dir = root.join(".claude/rules");
    for rule in md_files_in(&rules_dir) {
        let role = if has_paths_frontmatter(&rule) {
            "conditional"
        } else {
            "append"
        };
        files.push(read_file(&rule, role, 0));
    }
    // Global surfaces.
    for g in claude_global_resident(home) {
        files.push(read_file(&g, "global", 0));
    }
    for rule in md_files_in(&home.join(".claude/rules")) {
        if has_paths_frontmatter(&rule) {
            files.push(read_file(&rule, "conditional", 0));
        }
    }
    files
}

/// Build a native agent's read chain: canonical body → append layer(s) → global
/// surfaces → conditional/personal files.
fn native_read_chain(agent: Agent, root: &Path, home: &Path) -> Vec<ReadFile> {
    let mut files = Vec::new();
    let (read_path, _) = native_read_path(agent, root);
    files.push(read_file(&read_path, "canonical", 0));

    // Project append layers.
    match agent {
        Agent::Copilot => {
            files.push(read_file(
                &root.join(".github/copilot-instructions.md"),
                "append",
                0,
            ));
        }
        Agent::Antigravity => {
            files.push(read_file(&root.join("GEMINI.md"), "append", 0));
        }
        Agent::Codex => {
            let override_md = root.join("AGENTS.override.md");
            if override_md.exists() {
                files.push(read_file(&override_md, "append", 0));
            }
        }
        _ => {}
    }

    // Global surfaces.
    for g in global_resident(agent, home) {
        files.push(read_file(&g, "global", 0));
    }

    // Conditional/personal files.
    match agent {
        Agent::Opencode => {
            let cfg = root.join("opencode.json");
            if cfg.exists() {
                files.push(read_file(&cfg, "conditional", 0));
            }
        }
        Agent::Antigravity => {
            for rule in files_in(&root.join(".agents/rules")) {
                files.push(read_file(&rule, "conditional", 0));
            }
        }
        _ => {}
    }
    files
}

fn read_chain(agent: Agent, root: &Path, home: &Path) -> Vec<ReadFile> {
    if agent == Agent::Claude {
        claude_read_chain(root, home)
    } else {
        native_read_chain(agent, root, home)
    }
}

/// Per-agent read chain for a project. `agents` selects which agents to report;
/// pass the installed set (scan default) or a single requested agent.
pub fn where_with(root: &Path, home: &Path, agents: &[Agent]) -> Vec<AgentReadChain> {
    agents
        .iter()
        .map(|&agent| AgentReadChain {
            agent: agent.key().to_string(),
            files: read_chain(agent, root, home),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn write(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    #[test]
    fn canonical_reports_bytes_lines_tokens() {
        let root = tempdir().unwrap();
        write(&root.path().join("AGENTS.md"), "# Title\n\nbody line\n");
        let info = canonical_info(root.path());
        assert!(info.exists);
        assert_eq!(info.bytes, "# Title\n\nbody line\n".len() as u64);
        assert_eq!(info.lines, 3);
        assert!(info.est_tokens > 0);
    }

    #[test]
    fn missing_canonical_is_reported_absent() {
        let root = tempdir().unwrap();
        let info = canonical_info(root.path());
        assert!(!info.exists);
        assert_eq!(info.bytes, 0);
        assert_eq!(info.lines, 0);
    }

    #[test]
    fn claude_wrapper_states_are_classified() {
        let root = tempdir().unwrap();
        // pure wrapper
        write(&root.path().join("CLAUDE.md"), "@AGENTS.md\n");
        assert_eq!(
            claude_entry_state(&root.path().join("CLAUDE.md")),
            "wrapper"
        );
        // wrapper with append layer
        write(
            &root.path().join("CLAUDE.md"),
            "@AGENTS.md\n\n<!-- patchbay:append claude -->\n\nextra\n",
        );
        assert_eq!(
            claude_entry_state(&root.path().join("CLAUDE.md")),
            "wrapper_plus"
        );
        // real body content
        write(&root.path().join("CLAUDE.md"), "# Real instructions\n");
        assert_eq!(claude_entry_state(&root.path().join("CLAUDE.md")), "body");
    }

    #[test]
    fn claude_symlink_entry_is_classified() {
        let root = tempdir().unwrap();
        write(&root.path().join("AGENTS.md"), "# body\n");
        crate::core::test_support::expect_symlink_file(
            Path::new("AGENTS.md"),
            &root.path().join("CLAUDE.md"),
        );
        assert_eq!(
            claude_entry_state(&root.path().join("CLAUDE.md")),
            "symlink"
        );
    }

    #[test]
    fn native_entry_state_and_opencode_fallback() {
        let root = tempdir().unwrap();
        let home = tempdir().unwrap();
        // No AGENTS.md, but CLAUDE.md present → opencode falls back, codex missing.
        write(&root.path().join("CLAUDE.md"), "@AGENTS.md\n");
        let installed = [Agent::Codex, Agent::Opencode];
        let entries = entries_for(root.path(), &installed);
        let codex = entries.iter().find(|e| e.agent == "codex").unwrap();
        assert_eq!(codex.state, "missing");
        let opencode = entries.iter().find(|e| e.agent == "opencode").unwrap();
        assert_eq!(opencode.state, "native");
        assert!(opencode.path.ends_with("CLAUDE.md"));
        let _ = home;
    }

    #[test]
    fn claude_resident_counts_entry_import_local_and_rules() {
        let root = tempdir().unwrap();
        let home = tempdir().unwrap();
        write(&root.path().join("CLAUDE.md"), "@AGENTS.md\n");
        write(&root.path().join("AGENTS.md"), "canonical body\n");
        write(&root.path().join("CLAUDE.local.md"), "personal\n");
        write(&root.path().join(".claude/rules/always.md"), "always on\n");
        write(
            &root.path().join(".claude/rules/scoped.md"),
            "---\npaths: src/**\n---\nscoped\n",
        );

        let files = claude_project_resident(root.path(), home.path());
        let names: HashSet<String> = files
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert!(names.contains("CLAUDE.md"));
        assert!(names.contains("AGENTS.md")); // via @import
        assert!(names.contains("CLAUDE.local.md"));
        assert!(names.contains("always.md"));
        // paths:-gated rule is conditional-load, NOT resident.
        assert!(!names.contains("scoped.md"));
    }

    #[test]
    fn codex_global_takes_first_non_empty() {
        let home = tempdir().unwrap();
        write(&home.path().join(".codex/AGENTS.md"), ""); // empty → skip
        write(&home.path().join(".codex/AGENTS.override.md"), "override\n");
        let files = global_resident(Agent::Codex, home.path());
        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("AGENTS.override.md"));
    }

    #[test]
    fn globals_reader_set_unions_claude_and_opencode() {
        let home = tempdir().unwrap();
        write(&home.path().join(".claude/CLAUDE.md"), "global claude\n");
        let installed = [Agent::Claude, Agent::Opencode];
        let globals = globals_for(home.path(), &installed);
        let claude_global = globals
            .iter()
            .find(|g| g.path.ends_with(".claude/CLAUDE.md"))
            .unwrap();
        assert_eq!(claude_global.readers, vec!["claude", "opencode"]);
    }

    #[test]
    fn unmanaged_lists_personal_layer() {
        let root = tempdir().unwrap();
        write(&root.path().join("CLAUDE.local.md"), "local\n");
        write(&root.path().join("AGENTS.override.md"), "override\n");
        let installed = [Agent::Claude, Agent::Codex];
        let unmanaged = unmanaged_for(root.path(), &installed);
        let paths: Vec<&str> = unmanaged.iter().map(|u| u.path.as_str()).collect();
        assert!(paths.iter().any(|p| p.ends_with("CLAUDE.local.md")));
        assert!(paths.iter().any(|p| p.ends_with("AGENTS.override.md")));
    }

    #[test]
    fn scan_with_produces_full_payload() {
        let root = tempdir().unwrap();
        let home = tempdir().unwrap();
        write(&root.path().join("AGENTS.md"), "canonical\n");
        write(&root.path().join("CLAUDE.md"), "@AGENTS.md\n");
        let installed = [Agent::Claude];
        let report = scan_with(&[root.path().to_path_buf()], home.path(), &installed, 1234);
        assert_eq!(report.scanned_at, 1234);
        assert_eq!(report.agents, vec!["claude"]);
        assert_eq!(report.projects.len(), 1);
        let proj = &report.projects[0];
        assert!(proj.canonical.exists);
        assert_eq!(proj.entries.len(), 1);
        assert_eq!(proj.entries[0].state, "wrapper");
        assert_eq!(proj.resident.len(), 1);
        assert!(proj.resident[0].project_bytes > 0);
    }

    #[test]
    fn where_claude_chain_marks_canonical_via_import() {
        let root = tempdir().unwrap();
        let home = tempdir().unwrap();
        write(&root.path().join("AGENTS.md"), "canonical\n");
        write(&root.path().join("CLAUDE.md"), "@AGENTS.md\n");
        let chains = where_with(root.path(), home.path(), &[Agent::Claude]);
        assert_eq!(chains.len(), 1);
        let roles: Vec<(&str, &str)> = chains[0]
            .files
            .iter()
            .map(|f| (f.role.as_str(), f.path.as_str()))
            .collect();
        // entry CLAUDE.md, then canonical AGENTS.md (reached via @import).
        assert!(roles
            .iter()
            .any(|(r, p)| *r == "entry" && p.ends_with("CLAUDE.md")));
        assert!(roles
            .iter()
            .any(|(r, p)| *r == "canonical" && p.ends_with("AGENTS.md")));
    }

    #[test]
    fn where_native_chain_has_canonical_and_append() {
        let root = tempdir().unwrap();
        let home = tempdir().unwrap();
        write(&root.path().join("AGENTS.md"), "canonical\n");
        write(&root.path().join("GEMINI.md"), "gemini append\n");
        let chains = where_with(root.path(), home.path(), &[Agent::Antigravity]);
        let roles: Vec<&str> = chains[0].files.iter().map(|f| f.role.as_str()).collect();
        assert!(roles.contains(&"canonical"));
        assert!(roles.contains(&"append"));
    }
}
