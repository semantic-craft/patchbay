//! Live repair run: step events plus pause/takeover control (issue #32).
//!
//! The deterministic repair pipeline (check → locate → rebuild → verify) is
//! narrated as a fixed four-step script over an event callback, so the
//! workbench card can render a live tick-off while the engine works. Control
//! is cooperative and only ever consulted at STEP BOUNDARIES:
//!
//! * before `check`, `locate` and `rebuild` the run honors `pause` (blocks
//!   until resumed) and `takeover` (aborts with ZERO writes);
//! * once `rebuild` starts, the run always finishes `verify` + journal — a
//!   write is never left without its undo record, so there is no torn state
//!   a takeover could produce.
//!
//! The module owns only the event/control plumbing; the orchestration lives
//! in `ChainService::repair_live` so the emitter stays a plain callback and
//! the Rust tests need no Tauri runtime.

use serde::Serialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// One narrated step transition. `seq` is a per-run monotonic counter so the
/// frontend can drop stale/out-of-order deliveries defensively.
#[derive(Debug, Clone, Serialize)]
pub struct LiveEvent {
    pub run_id: String,
    pub seq: u32,
    /// "check" | "locate" | "rebuild" | "verify"
    pub step: String,
    /// "start" | "done" | "failed"
    pub status: String,
    /// Raw evidence line(s) for the step — paths, candidate scores, item
    /// edits. Locale-neutral data; the frontend owns the step labels.
    pub detail: Option<String>,
}

/// Cooperative control flag for one live run.
const RUNNING: u8 = 0;
const PAUSED: u8 = 1;
const TAKEOVER: u8 = 2;

#[derive(Debug, Clone, Default)]
pub struct LiveControl(Arc<AtomicU8>);

/// What a step boundary decided after consulting the control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Proceed,
    Takeover,
}

impl LiveControl {
    pub fn pause(&self) {
        // Takeover is terminal — pause must not resurrect an aborted run.
        let _ = self
            .0
            .compare_exchange(RUNNING, PAUSED, Ordering::SeqCst, Ordering::SeqCst);
    }

    pub fn resume(&self) {
        let _ = self
            .0
            .compare_exchange(PAUSED, RUNNING, Ordering::SeqCst, Ordering::SeqCst);
    }

    pub fn takeover(&self) {
        self.0.store(TAKEOVER, Ordering::SeqCst);
    }

    /// Block while paused; return the boundary's decision. Polling keeps the
    /// runner a plain blocking thread with no async machinery.
    pub fn checkpoint(&self) -> Decision {
        loop {
            match self.0.load(Ordering::SeqCst) {
                TAKEOVER => return Decision::Takeover,
                PAUSED => std::thread::sleep(std::time::Duration::from_millis(50)),
                _ => return Decision::Proceed,
            }
        }
    }
}

/// The run's terminal result, returned by the `chain_repair_live` invoke —
/// events narrate progress, this carries the outcome.
#[derive(Debug, Clone, Serialize)]
pub struct LiveOutcome {
    /// True when a takeover aborted the run BEFORE rebuild — zero writes
    /// happened and the user falls back to the manual flow.
    pub aborted: bool,
    /// The apply outcome (with journal id) once rebuild ran; `None` iff
    /// `aborted`.
    pub outcome: Option<super::repair::RepairOutcome>,
}

// ── Run registry (control lookup for the control command) ─────────────────

fn registry() -> &'static Mutex<HashMap<String, LiveControl>> {
    static REGISTRY: OnceLock<Mutex<HashMap<String, LiveControl>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Register a fresh control for `run_id`, returning it. A stale entry under
/// the same id (a retried run) is replaced.
pub fn register(run_id: &str) -> LiveControl {
    let control = LiveControl::default();
    registry()
        .lock()
        .unwrap()
        .insert(run_id.to_string(), control.clone());
    control
}

/// Look up a live run's control. `None` when the run already finished.
pub fn control_of(run_id: &str) -> Option<LiveControl> {
    registry().lock().unwrap().get(run_id).cloned()
}

/// Drop a finished run's control.
pub fn unregister(run_id: &str) {
    registry().lock().unwrap().remove(run_id);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn takeover_is_terminal_over_pause_and_resume() {
        let control = LiveControl::default();
        control.takeover();
        control.pause();
        control.resume();
        assert_eq!(control.checkpoint(), Decision::Takeover);
    }

    #[test]
    fn checkpoint_blocks_while_paused_until_resumed() {
        let control = LiveControl::default();
        control.pause();
        let unblocker = control.clone();
        let handle = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(120));
            unblocker.resume();
        });
        let started = std::time::Instant::now();
        assert_eq!(control.checkpoint(), Decision::Proceed);
        assert!(started.elapsed() >= std::time::Duration::from_millis(100));
        handle.join().unwrap();
    }

    #[test]
    fn registry_round_trips_and_unregisters() {
        let control = register("run-1");
        control.pause();
        let found = control_of("run-1").expect("registered");
        // The same underlying flag: resuming through the looked-up handle
        // unblocks the original.
        found.resume();
        assert_eq!(control.checkpoint(), Decision::Proceed);
        unregister("run-1");
        assert!(control_of("run-1").is_none());
    }
}
