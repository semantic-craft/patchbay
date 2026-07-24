//! Remediate a Global Guard violation by moving a wrongly-global Skill into a
//! registered project's managed chain, then retiring the global entry.
//!
//! The Global Guard (see [`super::global_guard`]) flags valid user Skills that
//! leaked onto a global Agent surface (e.g. `~/.claude/skills/<skill>`), which
//! the convention keeps empty. This module drives the safe remediation: link
//! the Skill into a SELECTED registered project and target Agents, VERIFY the
//! project-local chain, and only THEN remove the offending global SYMLINK.
//!
//! Two invariants make this safety-sensitive operation trustworthy:
//!
//! * The global entry is retired only after the project-local chain is
//!   established and a rescan verifies it (its [`ApplyOutcome::verified`] flag),
//!   and only while it is still the exact symlink the plan snapshotted (a
//!   time-of-check/time-of-use guard). Any failed or conflicting link, or any
//!   change to the global entry since the preview, leaves it untouched.
//! * A PHYSICAL global Skill directory is never deleted automatically. It is
//!   surfaced with actionable manual guidance instead — Patchbay refuses to
//!   destroy real Skill contents it does not manage.
//!
//! Like the link/unlink/repair flows, remediation is a guarded two-phase
//! operation: [`ChainService::plan_remediate`](super::service::ChainService::plan_remediate)
//! previews everything read-only, and
//! [`ChainService::apply_remediate`](super::service::ChainService::apply_remediate)
//! re-validates and applies it.

use serde::{Deserialize, Serialize};

use super::ops::{self, EntryEvidence};
use super::service::ApplyOutcome;
use super::GuardSurface;

/// Manual guidance shown for a PHYSICAL global Skill directory. Patchbay never
/// auto-deletes real Skill contents, so it tells the user how to relocate it.
pub const PHYSICAL_GUIDANCE: &str = "This is a physical Skill directory on the global surface. Move it into an Original Repository or the project manually; Patchbay will not delete it.";

/// Guidance recorded when the global entry became a physical directory between
/// the preview and apply — the same never-auto-delete rule applies.
pub const BECAME_PHYSICAL_GUIDANCE: &str = "The global entry became a physical Skill directory since the preview. Patchbay will not delete it; move it into an Original Repository or the project manually.";

/// Guidance recorded when the global entry changed since the preview (a
/// different symlink, or removed). It is left untouched; rescan and retry.
pub const CHANGED_GUIDANCE: &str = "The global entry changed since the preview and was left untouched. Rescan the Global Guard and remediate again.";

/// A previewed, guarded remediation of one Global Guard violation. Produced by
/// [`ChainService::plan_remediate`](super::service::ChainService::plan_remediate)
/// and consumed unchanged by
/// [`ChainService::apply_remediate`](super::service::ChainService::apply_remediate).
/// It crosses the wire in both directions (the GUI plans, shows the preview,
/// then sends the exact plan back to apply), so it is `Deserialize` too.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemediationPlan {
    /// The offending global entry to remediate (a path on a global surface).
    pub global_path: String,
    /// Skill name of the violation, for display and audit.
    pub skill: String,
    /// Human-facing Agent name of the global surface the entry sits on.
    pub agent: String,
    /// What the global entry resolves to — the Original Skill this remediation
    /// links into the project.
    pub final_target: String,
    /// Whether the global entry is a symlink. `false` ⇒ a physical directory
    /// that is manual-only (`link_plan` is `None`, `remove_global` is `false`).
    pub is_link: bool,
    /// The selected registered project to link the Skill into.
    pub project: String,
    /// The selected target Agents inside the project.
    pub agents: Vec<String>,
    /// The project-local link plan apply will establish and verify. `None` when
    /// the global entry is physical (manual-only) and nothing can be linked.
    pub link_plan: Option<ops::LinkPlan>,
    /// Whether apply will attempt to remove the global entry — only ever `true`
    /// for a symlink, and only after the project link verifies.
    pub remove_global: bool,
    /// On-disk state of the global path at plan time, the TOCTOU baseline apply
    /// re-checks before it removes anything.
    pub global_evidence: EntryEvidence,
    /// Actionable manual guidance when the entry is physical or not linkable.
    pub guidance: Option<String>,
}

/// The result of applying a [`RemediationPlan`]: the project link outcome, then
/// whether the global entry was retired, plus a fresh Global Guard rescan.
///
/// `verified` is the end-to-end proof for a symlink remediation: the project
/// link verified AND the global entry was removed. A physical entry can never
/// be `verified` here because nothing is removed.
#[derive(Debug, Clone, Serialize)]
pub struct RemediationOutcome {
    /// The project link result (`None` for a physical/manual-only entry).
    pub link: Option<ApplyOutcome>,
    /// Whether the global entry was actually removed.
    pub global_removed: bool,
    /// End-to-end success: the link verified and the global entry was removed.
    pub verified: bool,
    /// Actionable guidance when the entry was left in place (physical, changed,
    /// or an unverified link).
    pub guidance: Option<String>,
    /// Timestamp of the post-apply rescan.
    pub scanned_at: i64,
    /// The post-apply Global Guard rescan (AC5) so the UI can confirm the
    /// surface is no longer in violation.
    pub guard: Vec<GuardSurface>,
}
