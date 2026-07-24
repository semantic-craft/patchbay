//! Instructions governance (CLAUDE.md / AGENTS.md) — read-only scanning base.
//!
//! Converges a registered project's instructions to a single canonical
//! `AGENTS.md` body plus per-agent `@AGENTS.md` wrapper entries, and makes the
//! per-agent token cost (project + global) visible. This module is the sole
//! service facade the CLI, Tauri commands, and GUI go through (design §4/§7).
//!
//! P0 (this base) is purely read-only: `scan` and `where`. Doctor rules (P1) and
//! normalize/init write operations (P2) extend this same service; no second
//! implementation. The five-key agent catalogue here is independent of the
//! chain module's four-key skill catalogue and never mutates it (§1).
//!
//! P2 adds the write-safety base (§8) — [`snapshot`] (content snapshots) and
//! [`write_guard`] (TOCTOU content guard, write-target whitelist, guarded
//! writes) — plus the two write operations that route entirely through those
//! guarded facilities: [`normalize`] (§4.1, mechanical merge / canonicalization /
//! wrapper completion) and [`init`] (§4.2, scaffold a bare project's skeleton +
//! wrapper + optional docs directory, create-only).

pub mod blocks;
pub mod doctor;
pub mod import_resolver;
pub mod init;
pub mod normalize;
pub mod scanner;
pub mod service;
pub mod snapshot;
pub mod surfaces;
pub mod token_estimate;
pub mod write_guard;

pub use service::InstructionsService;
