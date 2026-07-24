//! Plan/apply write engine for the three-step link convention.
//!
//! Linking a Skill into a project is a guarded, two-phase operation:
//!
//! * [`plan_link`] inspects the filesystem read-only and returns, per item, the
//!   target path, the intended action (`created` / `exists` / `conflict` /
//!   `error`), the scope, and a snapshot of the current on-disk evidence.
//! * [`apply_link`] re-validates every boundary from scratch, refuses any item
//!   whose on-disk evidence changed since the plan (time-of-check/time-of-use
//!   guard), and only ever creates SYMLINKS — never overwriting an unrelated
//!   symlink, file, or physical directory. Conflicts are reported, not resolved.
//!
//! The write boundary is enforced in both phases and re-checked before every
//! write:
//!
//! * an Original Skill must resolve inside a configured warehouse root,
//! * the derived skill name must be a single safe path component (no traversal,
//!   no separators, no same-name aliasing),
//! * every target must stay inside the project (`path_guard::is_path_safe`), so
//!   a crafted name or a symlinked parent directory cannot escape it.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::link_tracer;
use super::project_links::{self, AGENT_SURFACES};
use crate::core::{path_guard, skill_metadata};

/// One applied (or refused) write, as returned by [`apply_link`].
#[derive(Debug, Clone, Serialize)]
pub struct OpResult {
    pub name: String,
    pub path: String,
    /// "created" | "exists" | "removed" | "absent" | "skipped" | "conflict" | "error"
    pub action: String,
    pub message: Option<String>,
}

/// The structured outcome of applying a link plan.
#[derive(Debug, Clone, Serialize)]
pub struct LinkReport {
    pub agg_dir: String,
    pub skills: Vec<OpResult>,
    pub entries: Vec<OpResult>,
}

fn ok(name: &str, path: &Path, action: &str) -> OpResult {
    OpResult {
        name: name.to_string(),
        path: path.to_string_lossy().to_string(),
        action: action.to_string(),
        message: None,
    }
}

fn with_msg(name: &str, path: &Path, action: &str, message: impl Into<String>) -> OpResult {
    OpResult {
        message: Some(message.into()),
        ..ok(name, path, action)
    }
}

// ── Plan contract ─────────────────────────────────────────────────────────

/// On-disk state of a single target path, captured at plan time and re-checked
/// at apply time. A change between preview and write is refused (TOCTOU guard).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "target", rename_all = "snake_case")]
pub enum EntryEvidence {
    /// Nothing exists at the path.
    Absent,
    /// A symlink whose raw (lexical) target is carried for comparison.
    Symlink(String),
    /// A physical directory.
    Dir,
    /// A physical non-directory (regular file, etc.).
    File,
}

/// One previewed target in a link plan — the shape the GUI renders before apply.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanItem {
    /// Skill name (aggregate/entry scope) or agent key (surface scope).
    pub name: String,
    /// Absolute path that would be written.
    pub path: String,
    /// "created" | "exists" | "conflict" | "error"
    pub action: String,
    /// Which surface the target belongs to: "aggregate" | "surface".
    pub scope: String,
    pub message: Option<String>,
}

/// A previewed, guarded link operation. Produced by [`plan_link`] and consumed
/// unchanged by [`apply_link`]. The `evidence` map is the time-of-check
/// baseline: apply refuses any target whose current on-disk state no longer
/// matches what the plan observed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkPlan {
    /// Project path as given (never canonicalized — see `project_registry`).
    pub project: String,
    pub agg_dir: String,
    /// Original Skill paths, as given, that apply will re-validate and link.
    pub originals: Vec<String>,
    pub agents: Vec<String>,
    pub skills: Vec<PlanItem>,
    pub entries: Vec<PlanItem>,
    /// Target path -> on-disk evidence observed at plan time.
    pub evidence: BTreeMap<String, EntryEvidence>,
}

// ── Filesystem helpers ────────────────────────────────────────────────────

#[cfg(unix)]
pub(super) fn make_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(not(unix))]
pub(super) fn make_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    match std::os::windows::fs::symlink_dir(target, link) {
        Ok(()) => Ok(()),
        Err(err) => {
            // Same ladder as `sync_engine::write_target` — missing
            // SeCreateSymbolicLinkPrivilege or Developer Mode off — except that
            // there is no copy rung here. A copy would satisfy the filesystem
            // but break the chain invariant: `link_tracer` has to resolve an
            // entry back to its Original, and a copy resolves to nothing. A
            // junction needs no privilege on a local NTFS volume, and std
            // reports mount points as symlinks, so tracing sees it as a link.
            //
            // `junction::create` demands an absolute target while callers pass
            // relative ones (the agent surface links to `../.agents/skills`),
            // so anchor it to the link's own directory first.
            let absolute = if target.is_absolute() {
                target.to_path_buf()
            } else {
                link.parent().unwrap_or_else(|| Path::new(".")).join(target)
            };
            // Report the original symlink error: it names the actual privilege
            // problem, where the junction error only says the retry failed.
            junction::create(&absolute, link).map_err(|_| err)
        }
    }
}

/// Unlink a symlink entry without ever following it into the Original target.
///
/// On Windows a *directory* symlink — or a junction standing in for one — must
/// be removed with `remove_dir`; `remove_file` fails with "access denied" and
/// leaves the broken link in place. The classification has to come from the
/// link's own metadata, because following it would misclassify a dangling link.
/// Mirrors the removal half of `sync_engine::remove_target`.
pub(super) fn remove_symlink(link: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileTypeExt;
        if std::fs::symlink_metadata(link)?
            .file_type()
            .is_symlink_dir()
        {
            return std::fs::remove_dir(link);
        }
    }
    std::fs::remove_file(link)
}

/// Does `entry` (a symlink) already resolve to `want`?
pub(super) fn resolves_to(entry: &Path, want: &Path) -> bool {
    let tr = link_tracer::trace(entry);
    tr.exists && link_tracer::normalize(Path::new(&tr.final_target)) == link_tracer::normalize(want)
}

/// Snapshot the on-disk state of a single path without following the final
/// component, so a symlink is recorded as a symlink rather than its target.
pub(super) fn observe(path: &Path) -> EntryEvidence {
    match std::fs::symlink_metadata(path) {
        Err(_) => EntryEvidence::Absent,
        Ok(meta) if meta.file_type().is_symlink() => {
            let target = std::fs::read_link(path)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
            EntryEvidence::Symlink(target)
        }
        Ok(meta) if meta.is_dir() => EntryEvidence::Dir,
        Ok(_) => EntryEvidence::File,
    }
}

/// The result of attempting to retire a global guard entry after a verified
/// project link. Distinguishing "changed" from "physical" lets the caller give
/// the right message (rescan-and-retry vs. never-auto-delete guidance).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GlobalRemoval {
    /// The symlink was removed.
    Removed,
    /// The entry changed since the baseline (a different symlink, or removed);
    /// left untouched.
    Changed,
    /// The entry is a physical directory/file; never auto-deleted.
    Physical,
}

/// Remove a global guard entry, but ONLY while it is still the exact symlink the
/// plan snapshotted as `baseline`.
///
/// The global path lives OUTSIDE any project, so the project-boundary helpers do
/// not apply here — the guard is purely "is it still the same symlink we planned
/// to remove?". A physical directory/file is never removed (Patchbay does not
/// auto-delete real Skill contents), and an entry whose state changed since the
/// preview is left untouched (a time-of-check/time-of-use guard).
/// [`remove_symlink`] unlinks the symlink itself, never following it into the
/// Original target.
pub(super) fn remove_global_symlink(
    path: &Path,
    baseline: &EntryEvidence,
) -> std::io::Result<GlobalRemoval> {
    let current = observe(path);
    if &current != baseline {
        // Changed since the preview. If it is now physical, say so distinctly so
        // the caller can offer manual guidance; otherwise it simply changed.
        return Ok(match current {
            EntryEvidence::Dir | EntryEvidence::File => GlobalRemoval::Physical,
            _ => GlobalRemoval::Changed,
        });
    }
    match current {
        EntryEvidence::Symlink(_) => {
            remove_symlink(path)?;
            Ok(GlobalRemoval::Removed)
        }
        // Defensive: a non-symlink baseline is never scheduled for removal, but
        // never delete a physical entry even if one is somehow presented.
        EntryEvidence::Dir | EntryEvidence::File => Ok(GlobalRemoval::Physical),
        EntryEvidence::Absent => Ok(GlobalRemoval::Changed),
    }
}

/// True when the directory that will *hold* `target` resolves inside `project`.
///
/// Only the parent is canonicalized, never `target`'s final component: a managed
/// skill symlink is meant to point at the warehouse (outside the project), so
/// following it would look like an escape. Canonicalizing the parent still
/// catches a symlinked parent directory (e.g. a `.agents` pointing elsewhere)
/// trying to redirect the write out of the project.
pub(super) fn parent_within_project(project: &Path, target: &Path) -> bool {
    match target.parent() {
        Some(parent) => path_guard::is_path_safe(project, parent),
        None => false,
    }
}

/// Basic write floor: absolute, existing directory, never home or root itself.
/// Registration and enrolment approval are enforced one level up in
/// `ChainService`; this guard keeps the filesystem engine safe on its own.
pub(super) fn validate_project(project: &Path) -> Result<(), String> {
    if !project.is_absolute() {
        return Err(format!(
            "project path must be absolute: {}",
            project.display()
        ));
    }
    if !project.is_dir() {
        return Err(format!(
            "project path is not a directory: {}",
            project.display()
        ));
    }
    let home = dirs::home_dir().unwrap_or_default();
    if project == home || project == Path::new("/") {
        return Err("refusing to operate on home or filesystem root".to_string());
    }
    Ok(())
}

/// Validate an Original Skill against the write boundary, returning its safe
/// directory name. Enforces (in order): a single safe name component, an
/// absolute path to a real Skill directory, and resolution inside one of the
/// configured warehouse roots. `is_path_safe` canonicalizes both sides, so a
/// symlink escaping a root or a `..` traversal is rejected here.
fn validate_original(original: &Path, source_roots: &[PathBuf]) -> Result<String, String> {
    let Some(name_os) = original.file_name() else {
        return Err("original has no file name".to_string());
    };
    let name = name_os.to_string_lossy().to_string();
    // Malicious name / same-name aliasing defense: the derived name must survive
    // sanitization unchanged and be a single component. We refuse rather than
    // silently rewrite, so a crafted name can never alias a different Skill.
    if name != path_guard::sanitize_name(&name)
        || name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\\')
    {
        return Err(format!("unsafe skill name: {name}"));
    }
    if !original.is_absolute() || !skill_metadata::is_valid_skill_dir(original) {
        return Err("not an absolute path to a skill directory (SKILL.md missing?)".to_string());
    }
    if !source_roots
        .iter()
        .any(|root| path_guard::is_path_safe(root, original))
    {
        return Err("skill resolves outside every configured tier-1 source".to_string());
    }
    Ok(name)
}

/// What will happen to a single link target, independent of whether we are
/// previewing or applying. `Create` is the only verdict that performs a write;
/// preview and apply share these deciders so their action strings and conflict
/// messages can never drift apart.
enum Verdict {
    Create,
    Exists,
    Conflict(String),
}

/// The one place the shared action strings and conflict messages are attached.
fn verdict_action(verdict: &Verdict) -> (&'static str, Option<String>) {
    match verdict {
        Verdict::Create => ("created", None),
        Verdict::Exists => ("exists", None),
        Verdict::Conflict(message) => ("conflict", Some(message.clone())),
    }
}

/// Decide what will happen to an aggregate skill link given its current state.
fn decide_skill(target: &Path, original: &Path, ev: &EntryEvidence) -> Verdict {
    match ev {
        EntryEvidence::Absent => Verdict::Create,
        EntryEvidence::Symlink(_) => {
            if resolves_to(target, original) {
                Verdict::Exists
            } else {
                Verdict::Conflict(format!(
                    "already links elsewhere: {}",
                    link_tracer::trace(target).final_target
                ))
            }
        }
        EntryEvidence::Dir | EntryEvidence::File => {
            Verdict::Conflict("physical entry already exists".to_string())
        }
    }
}

/// Decide what will happen to one per-skill entry link under a physical agent
/// surface. The conflict reason is not surfaced per entry (the surface summary
/// lists the affected names), so any non-linking state is a plain `Conflict`.
fn decide_entry(sentry: &Path, agg_target: &Path, ev: &EntryEvidence) -> Verdict {
    match ev {
        EntryEvidence::Absent => Verdict::Create,
        EntryEvidence::Symlink(_) if resolves_to(sentry, agg_target) => Verdict::Exists,
        _ => Verdict::Conflict(String::new()),
    }
}

/// What will happen to an agent entry surface as a whole.
enum SurfaceVerdict {
    Exists,
    Conflict(String),
    /// Absent surface: create the `.claude/skills -> ../.agents/skills` dir link.
    CreateDirLink,
    /// Physical surface: fan out into per-skill entry links.
    PerEntry,
}

fn decide_surface(surface: &Path, agg: &Path, ev: &EntryEvidence) -> SurfaceVerdict {
    match ev {
        EntryEvidence::Symlink(_) => {
            if resolves_to(surface, agg) {
                SurfaceVerdict::Exists
            } else {
                SurfaceVerdict::Conflict(
                    "surface is a symlink but does not resolve to .agents/skills".to_string(),
                )
            }
        }
        EntryEvidence::File => SurfaceVerdict::Conflict("surface exists but is a file".to_string()),
        EntryEvidence::Absent => SurfaceVerdict::CreateDirLink,
        EntryEvidence::Dir => SurfaceVerdict::PerEntry,
    }
}

fn plan_item(
    name: &str,
    path: &Path,
    action: &str,
    scope: &str,
    message: Option<&str>,
) -> PlanItem {
    PlanItem {
        name: name.to_string(),
        path: path.to_string_lossy().to_string(),
        action: action.to_string(),
        scope: scope.to_string(),
        message: message.map(|m| m.to_string()),
    }
}

// ── Plan phase (read-only) ────────────────────────────────────────────────

/// Preview linking `originals` into `<project>/.agents/skills` plus an entry on
/// each requested agent surface. Never writes: it validates the write boundary,
/// classifies every target, and snapshots the on-disk evidence apply will
/// re-check. Boundary violations and conflicts appear as items, not errors, so
/// the whole plan can be shown before anything is applied.
pub fn plan_link(
    project: &Path,
    originals: &[PathBuf],
    agents: &[String],
    warehouse_roots: &[PathBuf],
) -> Result<LinkPlan, String> {
    validate_project(project)?;
    let agg = project.join(".agents").join("skills");

    let mut evidence: BTreeMap<String, EntryEvidence> = BTreeMap::new();
    let mut skills = Vec::new();
    // Names (and their originals) that are or will be present in the aggregate,
    // so a physical agent surface gets a per-skill entry for each.
    let mut linked: Vec<String> = Vec::new();

    for original in originals {
        let fallback = original
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "?".to_string());
        let name = match validate_original(original, warehouse_roots) {
            Ok(name) => name,
            Err(reason) => {
                skills.push(plan_item(
                    &fallback,
                    original,
                    "error",
                    "aggregate",
                    Some(&reason),
                ));
                continue;
            }
        };
        let target = agg.join(&name);
        if !parent_within_project(project, &target) {
            skills.push(plan_item(
                &name,
                &target,
                "error",
                "aggregate",
                Some("target escapes the project boundary"),
            ));
            continue;
        }
        let ev = observe(&target);
        let verdict = decide_skill(&target, original, &ev);
        evidence.insert(target.to_string_lossy().to_string(), ev);
        if matches!(verdict, Verdict::Create | Verdict::Exists) {
            linked.push(name.clone());
        }
        let (action, message) = verdict_action(&verdict);
        skills.push(plan_item(
            &name,
            &target,
            action,
            "aggregate",
            message.as_deref(),
        ));
    }

    let mut entries = Vec::new();
    for agent in agents {
        let Some((_, rel)) = AGENT_SURFACES.iter().find(|(a, _)| a == agent) else {
            entries.push(plan_item(
                agent,
                project,
                "error",
                "surface",
                Some("unknown agent"),
            ));
            continue;
        };
        let surface = project_links::surface_path(project, rel);
        if !parent_within_project(project, &surface) {
            entries.push(plan_item(
                agent,
                &surface,
                "error",
                "surface",
                Some("surface escapes the project boundary"),
            ));
            continue;
        }
        let surface_ev = observe(&surface);
        let verdict = decide_surface(&surface, &agg, &surface_ev);
        evidence.insert(surface.to_string_lossy().to_string(), surface_ev);
        let item = match verdict {
            SurfaceVerdict::Exists => plan_item(agent, &surface, "exists", "surface", None),
            SurfaceVerdict::Conflict(message) => {
                plan_item(agent, &surface, "conflict", "surface", Some(&message))
            }
            SurfaceVerdict::CreateDirLink => plan_item(
                agent,
                &surface,
                "created",
                "surface",
                Some("dir link -> ../.agents/skills"),
            ),
            SurfaceVerdict::PerEntry => {
                let summary =
                    plan_per_skill_entries(project, &surface, &agg, &linked, &mut evidence);
                plan_item(
                    agent,
                    &surface,
                    &summary.action,
                    "surface",
                    Some(&summary.message),
                )
            }
        };
        entries.push(item);
    }

    Ok(LinkPlan {
        project: project.to_string_lossy().to_string(),
        agg_dir: agg.to_string_lossy().to_string(),
        originals: originals
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect(),
        agents: agents.to_vec(),
        skills,
        entries,
        evidence,
    })
}

struct SurfaceSummary {
    action: String,
    message: String,
}

/// Preview (read-only) the per-skill entry links a physical agent surface would
/// receive, recording each entry's evidence for the TOCTOU baseline.
fn plan_per_skill_entries(
    project: &Path,
    surface: &Path,
    agg: &Path,
    linked: &[String],
    evidence: &mut BTreeMap<String, EntryEvidence>,
) -> SurfaceSummary {
    let mut created = 0usize;
    let mut existed = 0usize;
    let mut conflicts: Vec<String> = Vec::new();
    for name in linked {
        let sentry = surface.join(name);
        if !parent_within_project(project, &sentry) {
            conflicts.push(format!("{name}: escapes the project boundary"));
            continue;
        }
        let ev = observe(&sentry);
        let verdict = decide_entry(&sentry, &agg.join(name), &ev);
        evidence.insert(sentry.to_string_lossy().to_string(), ev);
        match verdict {
            Verdict::Create => created += 1,
            Verdict::Exists => existed += 1,
            Verdict::Conflict(_) => conflicts.push(name.clone()),
        }
    }
    let action = if !conflicts.is_empty() {
        "conflict"
    } else if created > 0 {
        "created"
    } else {
        "exists"
    };
    SurfaceSummary {
        action: action.to_string(),
        message: format!(
            "per-skill links: {created} created, {existed} existing{}",
            if conflicts.is_empty() {
                String::new()
            } else {
                format!(", conflicts: {}", conflicts.join(", "))
            }
        ),
    }
}

// ── Apply phase (writes) ──────────────────────────────────────────────────

/// Apply a previewed [`LinkPlan`]. Every boundary is re-validated from scratch
/// (a forged or stale plan cannot inject an out-of-boundary original), and each
/// target is written only when its current on-disk evidence still matches what
/// the plan recorded. Anything that changed since the preview is refused as
/// `skipped` without touching the filesystem.
pub fn apply_link(plan: &LinkPlan, warehouse_roots: &[PathBuf]) -> Result<LinkReport, String> {
    let project = PathBuf::from(&plan.project);
    validate_project(&project)?;
    let agg = project.join(".agents").join("skills");
    // Refuse before creating anything if the aggregate would resolve outside the
    // project (e.g. a symlinked `.agents` pointing elsewhere).
    if !path_guard::is_path_safe(&project, &agg) {
        return Err("aggregate directory escapes the project boundary".to_string());
    }
    std::fs::create_dir_all(&agg).map_err(|e| format!("create {}: {e}", agg.display()))?;
    if !path_guard::is_path_safe(&project, &agg) {
        return Err("aggregate directory escapes the project boundary".to_string());
    }

    let originals: Vec<PathBuf> = plan.originals.iter().map(PathBuf::from).collect();

    let mut skills = Vec::new();
    let mut applied: Vec<String> = Vec::new();
    for original in &originals {
        let fallback = original
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "?".to_string());
        let name = match validate_original(original, warehouse_roots) {
            Ok(name) => name,
            Err(reason) => {
                skills.push(with_msg(&fallback, original, "error", reason));
                continue;
            }
        };
        let target = agg.join(&name);
        if !parent_within_project(&project, &target) {
            skills.push(with_msg(
                &name,
                &target,
                "error",
                "target escapes the project boundary",
            ));
            continue;
        }
        let current = observe(&target);
        if plan_evidence(plan, &target) != Some(&current) {
            skills.push(with_msg(&name, &target, "skipped", "changed since preview"));
            continue;
        }
        match decide_skill(&target, original, &current) {
            Verdict::Create => match make_symlink(original, &target) {
                Ok(()) => {
                    skills.push(ok(&name, &target, "created"));
                    applied.push(name);
                }
                Err(e) => skills.push(with_msg(&name, &target, "error", e.to_string())),
            },
            Verdict::Exists => {
                skills.push(ok(&name, &target, "exists"));
                applied.push(name);
            }
            Verdict::Conflict(message) => {
                skills.push(with_msg(&name, &target, "conflict", message))
            }
        }
    }

    let mut entries = Vec::new();
    for agent in &plan.agents {
        let Some((_, rel)) = AGENT_SURFACES.iter().find(|(a, _)| a == agent) else {
            entries.push(with_msg(agent, &project, "error", "unknown agent"));
            continue;
        };
        let surface = project_links::surface_path(&project, rel);
        if !parent_within_project(&project, &surface) {
            entries.push(with_msg(
                agent,
                &surface,
                "error",
                "surface escapes the project boundary",
            ));
            continue;
        }
        let current = observe(&surface);
        if plan_evidence(plan, &surface) != Some(&current) {
            entries.push(with_msg(
                agent,
                &surface,
                "skipped",
                "changed since preview",
            ));
            continue;
        }
        match decide_surface(&surface, &agg, &current) {
            SurfaceVerdict::Exists => entries.push(ok(agent, &surface, "exists")),
            SurfaceVerdict::Conflict(message) => {
                entries.push(with_msg(agent, &surface, "conflict", message))
            }
            SurfaceVerdict::CreateDirLink => {
                let parent = surface.parent().unwrap_or(&project);
                if let Err(e) = std::fs::create_dir_all(parent) {
                    entries.push(with_msg(agent, &surface, "error", e.to_string()));
                    continue;
                }
                let target = Path::new("..").join(".agents").join("skills");
                match make_symlink(&target, &surface) {
                    Ok(()) => entries.push(with_msg(
                        agent,
                        &surface,
                        "created",
                        "dir link -> ../.agents/skills",
                    )),
                    Err(e) => entries.push(with_msg(agent, &surface, "error", e.to_string())),
                }
            }
            SurfaceVerdict::PerEntry => entries.push(apply_per_skill_entries(
                plan, &project, &surface, &agg, agent, &applied,
            )),
        }
    }

    Ok(LinkReport {
        agg_dir: agg.to_string_lossy().to_string(),
        skills,
        entries,
    })
}

fn plan_evidence<'a>(plan: &'a LinkPlan, path: &Path) -> Option<&'a EntryEvidence> {
    plan.evidence.get(&path.to_string_lossy().to_string())
}

/// Write the per-skill entry links a physical agent surface needs, one per name
/// that was actually linked into the aggregate this run. Each entry is guarded
/// by its own evidence check and boundary check, so an entry that changed since
/// the preview is counted as changed and never overwritten.
fn apply_per_skill_entries(
    plan: &LinkPlan,
    project: &Path,
    surface: &Path,
    agg: &Path,
    agent: &str,
    applied: &[String],
) -> OpResult {
    let mut created = 0usize;
    let mut existed = 0usize;
    let mut changed = 0usize;
    let mut conflicts: Vec<String> = Vec::new();
    for name in applied {
        let sentry = surface.join(name);
        if !parent_within_project(project, &sentry) {
            conflicts.push(format!("{name}: escapes the project boundary"));
            continue;
        }
        let current = observe(&sentry);
        if plan_evidence(plan, &sentry) != Some(&current) {
            changed += 1;
            continue;
        }
        match decide_entry(&sentry, &agg.join(name), &current) {
            Verdict::Create => {
                let starget = Path::new("..")
                    .join("..")
                    .join(".agents")
                    .join("skills")
                    .join(name);
                match make_symlink(&starget, &sentry) {
                    Ok(()) => created += 1,
                    Err(e) => conflicts.push(format!("{name}: {e}")),
                }
            }
            Verdict::Exists => existed += 1,
            Verdict::Conflict(_) => conflicts.push(name.clone()),
        }
    }
    let action = if !conflicts.is_empty() {
        "conflict"
    } else if changed > 0 {
        "skipped"
    } else if created > 0 {
        "created"
    } else {
        "exists"
    };
    let mut message = format!("per-skill links: {created} created, {existed} existing");
    if changed > 0 {
        message.push_str(&format!(", {changed} changed since preview"));
    }
    if !conflicts.is_empty() {
        message.push_str(&format!(", conflicts: {}", conflicts.join(", ")));
    }
    with_msg(agent, surface, action, message)
}

// ── Unlink plan/apply (Agent-aware, scope-preserving) ─────────────────────

/// One previewed unlink action. Mirrors [`PlanItem`] but carries the extra
/// Agent-scope facets the unlink flow needs: which Agent a surface item belongs
/// to, how the target is reached (`kind`), and whether it will be removed,
/// retained, or is only reachable through the shared aggregate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnlinkItem {
    /// Skill name the item concerns.
    pub name: String,
    /// The entry or aggregate path this item describes.
    pub path: String,
    /// Which surface the target belongs to: "surface" | "aggregate".
    pub scope: String,
    /// Agent key for surface items; `None` for the shared aggregate.
    pub agent: Option<String>,
    /// How the Skill is reached: "per_agent_entry" | "shared_surface" | "aggregate".
    pub kind: String,
    /// Intended outcome: "remove" | "retain" | "shared" | "conflict" | "absent".
    pub action: String,
    pub message: Option<String>,
}

/// A previewed, guarded unlink operation. Produced by [`plan_unlink`] and
/// consumed unchanged by [`apply_unlink`]. Like [`LinkPlan`] it crosses the wire
/// in both directions and carries the TOCTOU `evidence` baseline apply re-checks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnlinkPlan {
    /// Project path as given (never canonicalized).
    pub project: String,
    /// Skill name being unlinked.
    pub skill: String,
    /// Resolved requested agents: the explicit list, or every Agent currently
    /// exposing the Skill when the caller passed an empty list.
    pub agents: Vec<String>,
    pub items: Vec<UnlinkItem>,
    /// Target path -> on-disk evidence observed at plan time, for every path an
    /// apply could touch (the removable entries plus the aggregate).
    pub evidence: BTreeMap<String, EntryEvidence>,
    /// Every Agent that will LOSE access to this Skill if the plan is applied —
    /// the shared-surface preview list.
    pub affected_agents: Vec<String>,
    /// True when the plan removes the shared aggregate (affecting every dir-link
    /// Agent) or targets a dir-link surface, so more Agents may be affected than
    /// were explicitly requested. A shared-surface operation needs explicit
    /// confirmation.
    pub shared_surface: bool,
}

fn unlink_item(
    skill: &str,
    path: &Path,
    scope: &str,
    agent: Option<&str>,
    kind: &str,
    action: &str,
    message: Option<&str>,
) -> UnlinkItem {
    UnlinkItem {
        name: skill.to_string(),
        path: path.to_string_lossy().to_string(),
        scope: scope.to_string(),
        agent: agent.map(|a| a.to_string()),
        kind: kind.to_string(),
        action: action.to_string(),
        message: message.map(|m| m.to_string()),
    }
}

/// The OpResult name for an unlink item: the Agent key for a surface item, or
/// `.agents` for the shared aggregate — matching the historic unlink report.
fn unlink_op_name(item: &UnlinkItem) -> &str {
    match &item.agent {
        Some(agent) => agent.as_str(),
        None => ".agents",
    }
}

/// The observed shape of an Agent surface relevant to unlink.
enum SurfaceShape {
    /// `<surface>` is a real directory holding per-skill entry links.
    PerEntry,
    /// `<surface>` is itself a symlink to `.agents/skills`; the Agent sees every
    /// aggregate Skill through the whole-dir link, with no per-skill entry.
    DirLink,
    /// No usable surface exists.
    Absent,
}

/// One Agent surface as surveyed for the unlink plan.
struct SurfaceState {
    agent: &'static str,
    rel: &'static str,
    shape: SurfaceShape,
    /// Whether this Agent currently exposes the Skill being unlinked.
    exposes: bool,
}

/// Classify every Agent surface and whether it exposes `skill`. A per-entry
/// surface exposes the Skill when it holds a matching entry; a dir-link surface
/// exposes it whenever the aggregate carries the Skill (`agg_present`).
fn survey_surfaces(project: &Path, skill: &str, agg_present: bool) -> Vec<SurfaceState> {
    AGENT_SURFACES
        .iter()
        .map(|(agent, rel)| {
            let surface = project_links::surface_path(project, rel);
            let (shape, exposes) = match observe(&surface) {
                EntryEvidence::Symlink(_) => (SurfaceShape::DirLink, agg_present),
                EntryEvidence::Dir => {
                    let entry = surface.join(skill);
                    let exposes = !matches!(observe(&entry), EntryEvidence::Absent);
                    (SurfaceShape::PerEntry, exposes)
                }
                _ => (SurfaceShape::Absent, false),
            };
            SurfaceState {
                agent,
                rel,
                shape,
                exposes,
            }
        })
        .collect()
}

/// Preview removing `skill` from `project` for the given `agents`, preserving
/// every access that must survive. Never writes: it classifies each Agent
/// surface, decides per-Agent entry removals versus shared-surface operations,
/// decides whether the aggregate can be removed or must be retained, and
/// snapshots the on-disk evidence apply re-checks.
///
/// An empty `agents` list means "every Agent currently exposing the Skill"
/// (unlink from all). Physical directories and project-private Originals are
/// never scheduled for removal — they surface as conflicts.
pub fn plan_unlink(project: &Path, skill: &str, agents: &[String]) -> Result<UnlinkPlan, String> {
    validate_project(project)?;
    if skill.is_empty() || skill.contains('/') || skill.starts_with('.') {
        return Err(format!("invalid skill name: {skill}"));
    }

    let agg_entry = project.join(".agents").join("skills").join(skill);
    let agg_ev = observe(&agg_entry);
    let agg_present = !matches!(agg_ev, EntryEvidence::Absent);

    let surfaces = survey_surfaces(project, skill, agg_present);

    // Requested Agents: the explicit list, or every Agent that exposes the Skill.
    let requested: Vec<String> = if agents.is_empty() {
        surfaces
            .iter()
            .filter(|s| s.exposes)
            .map(|s| s.agent.to_string())
            .collect()
    } else {
        agents.to_vec()
    };
    let is_requested = |agent: &str| requested.iter().any(|a| a == agent);

    let mut items: Vec<UnlinkItem> = Vec::new();
    let mut evidence: BTreeMap<String, EntryEvidence> = BTreeMap::new();
    let mut affected: Vec<String> = Vec::new();
    let mut any_dir_link_item = false;

    // Per requested Agent surface. Iterating AGENT_SURFACES keeps the order
    // deterministic before the final sort.
    for state in &surfaces {
        if !is_requested(state.agent) {
            continue;
        }
        let surface = project.join(state.rel);
        match state.shape {
            SurfaceShape::PerEntry => {
                let entry = surface.join(skill);
                let ev = observe(&entry);
                match ev {
                    EntryEvidence::Symlink(_) => {
                        // A per-Agent entry link: removable for this Agent alone.
                        evidence.insert(entry.to_string_lossy().to_string(), ev);
                        items.push(unlink_item(
                            skill,
                            &entry,
                            "surface",
                            Some(state.agent),
                            "per_agent_entry",
                            "remove",
                            None,
                        ));
                        affected.push(state.agent.to_string());
                    }
                    EntryEvidence::Dir | EntryEvidence::File => items.push(unlink_item(
                        skill,
                        &entry,
                        "surface",
                        Some(state.agent),
                        "per_agent_entry",
                        "conflict",
                        Some("physical entry, not removing"),
                    )),
                    EntryEvidence::Absent => items.push(unlink_item(
                        skill,
                        &entry,
                        "surface",
                        Some(state.agent),
                        "per_agent_entry",
                        "absent",
                        None,
                    )),
                }
            }
            SurfaceShape::DirLink => {
                // No per-skill entry to remove: the Agent reaches the Skill only
                // through the shared aggregate. Hiding it is a shared-surface op.
                any_dir_link_item = true;
                items.push(unlink_item(
                    skill,
                    &surface,
                    "surface",
                    Some(state.agent),
                    "shared_surface",
                    "shared",
                    Some(
                        "reached via shared .claude/skills dir link; removal is a shared-surface operation",
                    ),
                ));
            }
            SurfaceShape::Absent => items.push(unlink_item(
                skill,
                &surface,
                "surface",
                Some(state.agent),
                "per_agent_entry",
                "absent",
                Some("no agent surface"),
            )),
        }
    }

    // Aggregate: remove it only when no Agent will still reference it afterwards.
    // A per-entry Agent not being unlinked whose entry is still a valid symlink,
    // or any dir-link Agent not being unlinked, keeps the aggregate required.
    let still_ref = surfaces
        .iter()
        .filter(|s| !is_requested(s.agent))
        .filter(|s| match s.shape {
            SurfaceShape::DirLink => true,
            SurfaceShape::PerEntry => matches!(
                observe(&project.join(s.rel).join(skill)),
                EntryEvidence::Symlink(_)
            ),
            SurfaceShape::Absent => false,
        })
        .count();

    let mut aggregate_removed = false;
    let agg_item = if still_ref > 0 {
        unlink_item(
            skill,
            &agg_entry,
            "aggregate",
            None,
            "aggregate",
            "retain",
            Some(&format!("still required by {still_ref} agent(s)")),
        )
    } else {
        match &agg_ev {
            EntryEvidence::Symlink(_) => {
                aggregate_removed = true;
                evidence.insert(agg_entry.to_string_lossy().to_string(), agg_ev.clone());
                unlink_item(
                    skill,
                    &agg_entry,
                    "aggregate",
                    None,
                    "aggregate",
                    "remove",
                    None,
                )
            }
            EntryEvidence::Dir | EntryEvidence::File => unlink_item(
                skill,
                &agg_entry,
                "aggregate",
                None,
                "aggregate",
                "conflict",
                Some("physical entry (project-private original), not removing"),
            ),
            EntryEvidence::Absent => unlink_item(
                skill,
                &agg_entry,
                "aggregate",
                None,
                "aggregate",
                "absent",
                None,
            ),
        }
    };
    items.push(agg_item);

    // Removing the aggregate hides the Skill from every dir-link Agent at once.
    if aggregate_removed {
        for state in &surfaces {
            if matches!(state.shape, SurfaceShape::DirLink) {
                affected.push(state.agent.to_string());
            }
        }
    }
    affected.sort();
    affected.dedup();

    // A shared-surface operation is one that removes the shared aggregate (thus
    // affecting Agents beyond those requested) or that targets a dir-link Agent
    // whose only removal lever is the shared aggregate.
    let exceeds = affected.iter().any(|agent| !is_requested(agent));
    let shared_surface = any_dir_link_item || (aggregate_removed && exceeds);

    items.sort_by(|a, b| {
        a.scope
            .cmp(&b.scope)
            .then_with(|| {
                a.agent
                    .as_deref()
                    .unwrap_or("")
                    .cmp(b.agent.as_deref().unwrap_or(""))
            })
            .then_with(|| a.path.cmp(&b.path))
    });

    Ok(UnlinkPlan {
        project: project.to_string_lossy().to_string(),
        skill: skill.to_string(),
        agents: requested,
        items,
        evidence,
        affected_agents: affected,
        shared_surface,
    })
}

/// Apply a previewed [`UnlinkPlan`]. Each `remove` item is re-observed and
/// refused as `skipped` if its on-disk evidence changed since the preview
/// (TOCTOU guard). A target is unlinked only while it is still a validated
/// symlink — [`remove_symlink`] never recurses, never deletes a real directory,
/// and never touches the Original target. Non-removing items (retain/shared/absent/
/// conflict) are reflected verbatim so the report is complete.
pub fn apply_unlink(plan: &UnlinkPlan) -> Result<Vec<OpResult>, String> {
    let project = PathBuf::from(&plan.project);
    validate_project(&project)?;

    let mut results = Vec::new();
    for item in &plan.items {
        let name = unlink_op_name(item);
        let path = PathBuf::from(&item.path);
        if item.action != "remove" {
            // retain / shared / absent / conflict: reflected without any write.
            results.push(OpResult {
                name: name.to_string(),
                path: item.path.clone(),
                action: item.action.clone(),
                message: item.message.clone(),
            });
            continue;
        }
        let current = observe(&path);
        if plan.evidence.get(&item.path) != Some(&current) {
            results.push(with_msg(name, &path, "skipped", "changed since preview"));
            continue;
        }
        match current {
            EntryEvidence::Symlink(_) => match remove_symlink(&path) {
                Ok(()) => results.push(ok(name, &path, "removed")),
                Err(e) => results.push(with_msg(name, &path, "error", e.to_string())),
            },
            // Never remove a physical directory or file, even if the plan said so.
            _ => results.push(with_msg(
                name,
                &path,
                "conflict",
                "became a physical entry since preview, not removing",
            )),
        }
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::{tempdir, TempDir};

    /// Portable stand-in for `std::os::unix::fs::symlink`, which is what these
    /// fixtures used while this module was `cfg(unix)`-gated. Every fixture
    /// links a directory, so the production helper is equivalent — and on
    /// Windows it drives the very symlink/junction ladder under test instead of
    /// skipping the module. Takes `AsRef` to match the std signature.
    fn symlink(target: impl AsRef<Path>, link: impl AsRef<Path>) -> std::io::Result<()> {
        make_symlink(target.as_ref(), link.as_ref())
    }

    struct Fixture {
        temp: TempDir,
        warehouse: PathBuf,
        original: PathBuf,
        project: PathBuf,
    }

    fn make_skill(dir: &Path, name: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: ops fixture\n---\n"),
        )
        .unwrap();
    }

    fn setup() -> Fixture {
        let temp = tempdir().unwrap();
        let warehouse = temp.path().join("warehouse");
        let original = warehouse.join("repo").join("skills").join("demo-skill");
        make_skill(&original, "demo-skill");
        let project = temp.path().join("proj");
        std::fs::create_dir_all(&project).unwrap();
        Fixture {
            temp,
            warehouse,
            original,
            project,
        }
    }

    fn roots(f: &Fixture) -> Vec<PathBuf> {
        vec![f.warehouse.clone()]
    }

    fn apply_fresh(f: &Fixture, originals: &[PathBuf], agents: &[String]) -> LinkReport {
        let rs = roots(f);
        let plan = plan_link(&f.project, originals, agents, &rs).unwrap();
        apply_link(&plan, &rs).unwrap()
    }

    #[test]
    fn plan_is_read_only() {
        let f = setup();
        let rs = roots(&f);
        let plan = plan_link(&f.project, &[f.original.clone()], &["claude".into()], &rs).unwrap();
        assert_eq!(plan.skills[0].action, "created");
        assert_eq!(plan.entries[0].action, "created");
        // The preview writes nothing to disk.
        assert!(!f.project.join(".agents").exists());
        assert!(!f.project.join(".claude").exists());
    }

    #[test]
    fn applies_and_is_idempotent_and_chain_resolves() {
        let f = setup();
        let report = apply_fresh(&f, &[f.original.clone()], &["claude".into()]);
        assert_eq!(report.skills[0].action, "created");
        assert_eq!(report.entries[0].action, "created");

        // A second plan sees "exists"; applying it stays idempotent.
        let report2 = apply_fresh(&f, &[f.original.clone()], &["claude".into()]);
        assert_eq!(report2.skills[0].action, "exists");
        assert_eq!(report2.entries[0].action, "exists");

        // The full chain resolves: .claude/skills/demo-skill -> original.
        let via_surface = f.project.join(".claude/skills/demo-skill");
        let tr = link_tracer::trace(&via_surface);
        assert!(tr.exists);
        assert_eq!(
            link_tracer::normalize(Path::new(&tr.final_target)),
            link_tracer::normalize(&f.original)
        );
    }

    #[test]
    fn physical_surface_gets_per_skill_links_and_conflicts_are_kept() {
        let f = setup();
        // Pre-existing physical surface holding an unrelated physical skill.
        std::fs::create_dir_all(f.project.join(".claude/skills/hand-made")).unwrap();
        let report = apply_fresh(&f, &[f.original.clone()], &["claude".into()]);
        assert_eq!(report.skills[0].action, "created");
        assert_eq!(report.entries[0].action, "created");
        assert!(report.entries[0]
            .message
            .as_deref()
            .unwrap()
            .contains("1 created"));
        // The unrelated physical skill is left untouched.
        assert!(f.project.join(".claude/skills/hand-made").is_dir());

        // A different original that collides on the aggregate name is a conflict,
        // never an overwrite of the existing link.
        let clash = f.warehouse.join("other-repo").join("demo-skill");
        make_skill(&clash, "demo-skill");
        let report3 = apply_fresh(&f, &[clash], &["claude".into()]);
        assert_eq!(report3.skills[0].action, "conflict");
        // The original link still points at the first original.
        assert!(resolves_to(
            &f.project.join(".agents/skills/demo-skill"),
            &f.original
        ));
    }

    #[test]
    fn original_outside_every_warehouse_root_is_refused() {
        let f = setup();
        // A perfectly valid skill directory, but outside the configured root.
        let outside = f.temp.path().join("loose").join("stray-skill");
        make_skill(&outside, "stray-skill");
        let report = apply_fresh(&f, &[outside.clone()], &["claude".into()]);
        assert_eq!(report.skills[0].action, "error");
        assert!(report.skills[0]
            .message
            .as_deref()
            .unwrap()
            .contains("outside every configured"));
        // Nothing was written and the stray skill is untouched.
        assert!(!f.project.join(".agents/skills/stray-skill").exists());
        assert!(outside.join("SKILL.md").is_file());
    }

    #[test]
    fn symlink_escape_original_is_refused() {
        let f = setup();
        // A real skill outside the warehouse, reached by a symlink placed inside
        // it. Canonicalization must see through the symlink and refuse the write.
        let outside = f.temp.path().join("outside").join("real-skill");
        make_skill(&outside, "real-skill");
        let escape = f.warehouse.join("repo").join("skills").join("escape");
        symlink(&outside, &escape).unwrap();

        let report = apply_fresh(&f, &[escape.clone()], &["claude".into()]);
        assert_eq!(report.skills[0].action, "error");
        assert!(!f.project.join(".agents/skills/escape").exists());
        assert!(outside.join("SKILL.md").is_file());
    }

    #[test]
    fn malicious_skill_name_is_refused() {
        let f = setup();
        // A valid skill directory whose name is not a safe single component.
        let evil = f.warehouse.join("repo").join("skills").join(".evil");
        make_skill(&evil, "evil");
        let report = apply_fresh(&f, &[evil], &["claude".into()]);
        assert_eq!(report.skills[0].action, "error");
        assert!(report.skills[0]
            .message
            .as_deref()
            .unwrap()
            .contains("unsafe skill name"));
        let agg = f.project.join(".agents/skills");
        assert!(!agg.exists() || std::fs::read_dir(&agg).unwrap().next().is_none());
    }

    #[test]
    fn traversal_component_in_original_is_refused() {
        let f = setup();
        // A crafted original ending in a `..` traversal segment has no usable
        // file name and must not resolve to a writable aggregate target.
        let traversal = f.warehouse.join("repo").join("skills").join("..");
        let report = apply_fresh(&f, &[traversal], &["claude".into()]);
        assert_eq!(report.skills[0].action, "error");
        let agg = f.project.join(".agents/skills");
        assert!(!agg.exists() || std::fs::read_dir(&agg).unwrap().next().is_none());
    }

    #[test]
    fn changed_evidence_after_plan_refuses_and_preserves_protected_file() {
        let f = setup();
        let rs = roots(&f);
        let plan = plan_link(&f.project, &[f.original.clone()], &["claude".into()], &rs).unwrap();
        assert_eq!(plan.skills[0].action, "created");

        // Between preview and apply, a protected physical file appears at the
        // exact aggregate target the plan intended to create.
        std::fs::create_dir_all(f.project.join(".agents/skills")).unwrap();
        let target = f.project.join(".agents/skills/demo-skill");
        std::fs::write(&target, "do not overwrite me").unwrap();

        let report = apply_link(&plan, &rs).unwrap();
        assert_eq!(report.skills[0].action, "skipped");
        assert!(report.skills[0]
            .message
            .as_deref()
            .unwrap()
            .contains("changed since preview"));
        // The protected file is intact — not replaced by a symlink.
        assert!(target.is_file());
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "do not overwrite me"
        );
    }

    #[test]
    fn changed_evidence_symlink_swap_is_refused_without_overwrite() {
        let f = setup();
        let rs = roots(&f);
        let plan = plan_link(&f.project, &[f.original.clone()], &["claude".into()], &rs).unwrap();

        // Attacker swaps in a symlink to a sensitive path at the target.
        std::fs::create_dir_all(f.project.join(".agents/skills")).unwrap();
        let target = f.project.join(".agents/skills/demo-skill");
        let sensitive = f.temp.path().join("sensitive");
        std::fs::write(&sensitive, "secret").unwrap();
        symlink(&sensitive, &target).unwrap();

        let report = apply_link(&plan, &rs).unwrap();
        assert_eq!(report.skills[0].action, "skipped");
        // The attacker's symlink is untouched; nothing was written through it.
        assert_eq!(std::fs::read_link(&target).unwrap(), sensitive);
        assert_eq!(std::fs::read_to_string(&sensitive).unwrap(), "secret");
    }

    #[test]
    fn symlinked_aggregate_parent_escape_is_refused() {
        let f = setup();
        // `.agents` is a symlink pointing outside the project; the aggregate must
        // not be followed out of bounds.
        let outside = f.temp.path().join("escape-target");
        std::fs::create_dir_all(&outside).unwrap();
        symlink(&outside, f.project.join(".agents")).unwrap();

        let rs = roots(&f);
        let plan = plan_link(&f.project, &[f.original.clone()], &["claude".into()], &rs).unwrap();
        let err = apply_link(&plan, &rs).unwrap_err();
        assert!(err.contains("escapes the project boundary"));
        // No skill link was created inside the escape target.
        assert!(!outside.join("skills/demo-skill").exists());
    }

    #[test]
    fn conflicting_symlink_elsewhere_is_preserved() {
        let f = setup();
        // Pre-existing aggregate link pointing at a different skill.
        let other = f.warehouse.join("repo").join("skills").join("other-skill");
        make_skill(&other, "other-skill");
        std::fs::create_dir_all(f.project.join(".agents/skills")).unwrap();
        symlink(&other, f.project.join(".agents/skills/demo-skill")).unwrap();

        let report = apply_fresh(&f, &[f.original.clone()], &["claude".into()]);
        assert_eq!(report.skills[0].action, "conflict");
        // The pre-existing link is untouched.
        assert_eq!(
            std::fs::read_link(f.project.join(".agents/skills/demo-skill")).unwrap(),
            other
        );
    }

    /// Force per-entry (physical) agent surfaces so demo-skill gets an
    /// individual link per agent, then link the given agents.
    fn apply_per_entry(f: &Fixture, agents: &[String]) {
        for agent in agents {
            let rel = AGENT_SURFACES
                .iter()
                .find(|(a, _)| a == agent)
                .map(|(_, rel)| *rel)
                .unwrap();
            std::fs::create_dir_all(f.project.join(rel)).unwrap();
        }
        apply_fresh(f, &[f.original.clone()], agents);
    }

    fn item_for<'a>(plan: &'a UnlinkPlan, agent: &str) -> Option<&'a UnlinkItem> {
        plan.items
            .iter()
            .find(|i| i.agent.as_deref() == Some(agent))
    }

    fn agg_item(plan: &UnlinkPlan) -> &UnlinkItem {
        plan.items.iter().find(|i| i.scope == "aggregate").unwrap()
    }

    #[test]
    fn per_agent_unlink_preserves_other_agents_and_aggregate() {
        // Two per-entry agents; unlink the Skill from one Agent only.
        let f = setup();
        apply_per_entry(&f, &["claude".into(), "codex".into()]);
        assert!(f.project.join(".claude/skills/demo-skill").exists());
        assert!(f.project.join(".codex/skills/demo-skill").exists());

        let plan = plan_unlink(&f.project, "demo-skill", &["claude".into()]).unwrap();
        assert!(
            !plan.shared_surface,
            "per-entry removal is not shared-surface"
        );
        assert_eq!(item_for(&plan, "claude").unwrap().action, "remove");
        assert_eq!(item_for(&plan, "claude").unwrap().kind, "per_agent_entry");
        // codex was not requested, so it is not an item; the aggregate is retained.
        assert!(item_for(&plan, "codex").is_none());
        assert_eq!(agg_item(&plan).action, "retain");

        apply_unlink(&plan).unwrap();
        // claude lost access; codex kept it; aggregate and Original intact (AC2/AC4/AC5).
        assert!(!f.project.join(".claude/skills/demo-skill").exists());
        assert!(f.project.join(".codex/skills/demo-skill").exists());
        assert!(f.project.join(".agents/skills/demo-skill").exists());
        assert!(f.original.join("SKILL.md").exists());
    }

    #[test]
    fn dir_link_surface_is_a_shared_surface_operation() {
        // A fresh single-agent link makes `.claude/skills` a whole-dir symlink.
        let f = setup();
        apply_fresh(&f, &[f.original.clone()], &["claude".into()]);
        assert!(std::fs::symlink_metadata(f.project.join(".claude/skills"))
            .unwrap()
            .file_type()
            .is_symlink());

        let plan = plan_unlink(&f.project, "demo-skill", &["claude".into()]).unwrap();
        assert!(
            plan.shared_surface,
            "dir-link removal is a shared-surface op (AC1/AC3)"
        );
        assert!(plan.affected_agents.contains(&"claude".to_string()));
        assert_eq!(item_for(&plan, "claude").unwrap().kind, "shared_surface");
        assert_eq!(item_for(&plan, "claude").unwrap().action, "shared");
        // claude is the only agent, so the shared aggregate is what gets removed.
        assert_eq!(agg_item(&plan).action, "remove");

        apply_unlink(&plan).unwrap();
        assert!(!f.project.join(".agents/skills/demo-skill").exists());
        // The dir link itself and the Original are never touched (AC5).
        assert!(std::fs::symlink_metadata(f.project.join(".claude/skills"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(f.original.join("SKILL.md").exists());
    }

    #[test]
    fn unlink_all_removes_links_but_never_physical_dirs() {
        let f = setup();
        apply_per_entry(&f, &["claude".into()]);

        // Empty agents == unlink from every Agent currently exposing the Skill.
        let plan = plan_unlink(&f.project, "demo-skill", &[]).unwrap();
        apply_unlink(&plan).unwrap();
        assert!(!f.project.join(".agents/skills/demo-skill").exists());
        assert!(!f.project.join(".claude/skills/demo-skill").exists());
        assert!(f.original.join("SKILL.md").exists());

        // A physical project-private original in the aggregate is refused (AC5).
        std::fs::create_dir_all(f.project.join(".agents/skills/private-one")).unwrap();
        let plan = plan_unlink(&f.project, "private-one", &[]).unwrap();
        let results = apply_unlink(&plan).unwrap();
        assert!(results.iter().any(|r| r.action == "conflict"));
        assert!(f.project.join(".agents/skills/private-one").exists());
    }

    #[test]
    fn apply_skips_targets_changed_since_preview() {
        let f = setup();
        apply_per_entry(&f, &["claude".into(), "codex".into()]);
        let plan = plan_unlink(&f.project, "demo-skill", &["claude".into()]).unwrap();

        // TOCTOU: the entry disappears between preview and apply.
        remove_symlink(&f.project.join(".claude/skills/demo-skill")).unwrap();
        let results = apply_unlink(&plan).unwrap();
        assert!(
            results.iter().any(|r| r.action == "skipped"),
            "changed evidence must skip, not blindly remove"
        );
        // codex's access is untouched.
        assert!(f.project.join(".codex/skills/demo-skill").exists());
    }

    #[test]
    fn remove_global_symlink_removes_matching_link_only() {
        let temp = tempdir().unwrap();
        let target = temp.path().join("original");
        std::fs::create_dir_all(&target).unwrap();
        let link = temp.path().join("global-skill");
        symlink(&target, &link).unwrap();

        let baseline = observe(&link);
        assert_eq!(
            remove_global_symlink(&link, &baseline).unwrap(),
            GlobalRemoval::Removed
        );
        assert!(!link.exists(), "the global symlink is gone");
        assert!(target.exists(), "the Original target is untouched");
    }

    #[test]
    fn remove_global_symlink_never_deletes_a_physical_entry() {
        let temp = tempdir().unwrap();
        let dir = temp.path().join("global-skill");
        std::fs::create_dir_all(&dir).unwrap();

        let baseline = observe(&dir);
        assert_eq!(
            remove_global_symlink(&dir, &baseline).unwrap(),
            GlobalRemoval::Physical
        );
        assert!(dir.exists(), "a physical global directory is never deleted");
    }

    #[test]
    fn remove_global_symlink_refuses_changed_evidence() {
        let temp = tempdir().unwrap();
        let link = temp.path().join("global-skill");
        symlink(temp.path().join("a"), &link).unwrap();
        let stale = observe(&link);

        // Repoint it since the baseline was taken (TOCTOU).
        remove_symlink(&link).unwrap();
        symlink(temp.path().join("b"), &link).unwrap();

        assert_eq!(
            remove_global_symlink(&link, &stale).unwrap(),
            GlobalRemoval::Changed
        );
        // symlink_metadata does not follow the link, so a present-but-dangling
        // symlink still counts as "left untouched".
        assert!(
            std::fs::symlink_metadata(&link).is_ok(),
            "a changed entry is left untouched"
        );
    }
}
