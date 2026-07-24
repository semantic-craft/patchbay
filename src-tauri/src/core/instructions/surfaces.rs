//! The instructions module's own agent catalogue (design §1).
//!
//! The chain module's `AGENT_SURFACES` models four skill-deploying agents; the
//! instructions governance surface is a different, five-key set (claude / codex
//! / copilot / opencode / antigravity) with its own entry-file, native-read,
//! wrapper, append-layer, global-surface, and install-detection facts. This
//! table is self-contained and never mutates the chain catalogue.
//!
//! This file owns only agent identity and *install detection*; the per-agent
//! file layout and resident-set arithmetic (§2) live in `scanner.rs`, kept
//! explicit-per-agent rather than in a clever shared table because each agent's
//! surface has genuinely different quirks.

use std::path::{Path, PathBuf};

/// A supported instructions agent. Order is the stable catalogue order used for
/// every list this module emits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    Claude,
    Codex,
    Copilot,
    Opencode,
    Antigravity,
}

impl Agent {
    /// Every supported agent, in stable catalogue order.
    pub const ALL: [Agent; 5] = [
        Agent::Claude,
        Agent::Codex,
        Agent::Copilot,
        Agent::Opencode,
        Agent::Antigravity,
    ];

    /// Stable, never-localized key used in JSON payloads and `--agent` filters.
    pub fn key(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
            Agent::Copilot => "copilot",
            Agent::Opencode => "opencode",
            Agent::Antigravity => "antigravity",
        }
    }

    /// Human-facing name (for future GUI use; CLI payloads use `key`).
    pub fn display_name(self) -> &'static str {
        match self {
            Agent::Claude => "Claude Code",
            Agent::Codex => "Codex",
            Agent::Copilot => "GitHub Copilot",
            Agent::Opencode => "OpenCode",
            Agent::Antigravity => "Antigravity",
        }
    }

    /// Parse a `--agent` key back to an `Agent`.
    pub fn from_key(key: &str) -> Option<Agent> {
        Agent::ALL.into_iter().find(|a| a.key() == key)
    }

    /// Executable names that, if present on PATH, prove the agent is installed.
    fn detect_bins(self) -> &'static [&'static str] {
        match self {
            Agent::Claude => &["claude"],
            Agent::Codex => &["codex"],
            Agent::Copilot => &["copilot"],
            Agent::Opencode => &["opencode"],
            Agent::Antigravity => &["agy"],
        }
    }

    /// Home-relative directories whose existence proves the agent is installed.
    /// Antigravity is deliberately keyed on `~/.gemini/antigravity-cli` (not
    /// `~/.gemini` itself, which may be a plain Gemini CLI remnant — design §1).
    fn detect_dirs(self) -> &'static [&'static str] {
        match self {
            Agent::Claude => &[".claude"],
            Agent::Codex => &[".codex"],
            Agent::Copilot => &[".copilot"],
            Agent::Opencode => &[".config/opencode", ".opencode"],
            Agent::Antigravity => &[".gemini/antigravity-cli"],
        }
    }

    /// Whether this agent is installed, given a home directory and the PATH
    /// search directories. Pure so it can be tested hermetically.
    pub fn is_installed(self, home: &Path, path_dirs: &[PathBuf]) -> bool {
        self.detect_bins()
            .iter()
            .any(|bin| bin_on_path(bin, path_dirs))
            || self.detect_dirs().iter().any(|rel| home.join(rel).is_dir())
    }
}

/// Whether an executable named `name` exists in any of `path_dirs`. On Windows,
/// the common launcher extensions are also tried.
fn bin_on_path(name: &str, path_dirs: &[PathBuf]) -> bool {
    let candidates: Vec<String> = if cfg!(windows) {
        vec![
            name.to_string(),
            format!("{name}.exe"),
            format!("{name}.cmd"),
            format!("{name}.bat"),
        ]
    } else {
        vec![name.to_string()]
    };
    path_dirs.iter().any(|dir| {
        candidates
            .iter()
            .any(|candidate| dir.join(candidate).is_file())
    })
}

/// PATH search directories from the environment, in order.
pub fn path_dirs_from_env() -> Vec<PathBuf> {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default()
}

/// Installed agents in catalogue order, given explicit home and PATH inputs.
pub fn installed_agents(home: &Path, path_dirs: &[PathBuf]) -> Vec<Agent> {
    Agent::ALL
        .into_iter()
        .filter(|a| a.is_installed(home, path_dirs))
        .collect()
}

/// Installed agents resolved against the live home directory and PATH.
pub fn installed_agents_live(home: &Path) -> Vec<Agent> {
    installed_agents(home, &path_dirs_from_env())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn keys_and_roundtrip_are_stable() {
        for agent in Agent::ALL {
            assert_eq!(Agent::from_key(agent.key()), Some(agent));
        }
        assert_eq!(Agent::from_key("nope"), None);
        let keys: Vec<&str> = Agent::ALL.iter().map(|a| a.key()).collect();
        assert_eq!(
            keys,
            ["claude", "codex", "copilot", "opencode", "antigravity"]
        );
    }

    #[test]
    fn detects_agent_by_home_directory() {
        let home = tempdir().unwrap();
        fs::create_dir_all(home.path().join(".claude")).unwrap();
        let installed = installed_agents(home.path(), &[]);
        assert_eq!(installed, vec![Agent::Claude]);
    }

    #[test]
    fn antigravity_gemini_dir_alone_does_not_count() {
        // A bare ~/.gemini (Gemini CLI remnant) must NOT mark Antigravity
        // installed; only ~/.gemini/antigravity-cli does.
        let home = tempdir().unwrap();
        fs::create_dir_all(home.path().join(".gemini")).unwrap();
        assert!(!Agent::Antigravity.is_installed(home.path(), &[]));

        fs::create_dir_all(home.path().join(".gemini/antigravity-cli")).unwrap();
        assert!(Agent::Antigravity.is_installed(home.path(), &[]));
    }

    #[test]
    fn detects_agent_by_path_binary() {
        // Not unix-only: `bin_on_path` tests the bare name before the Windows
        // launcher extensions, and detection is `is_file()`, never an exec bit.
        let home = tempdir().unwrap();
        let bindir = tempdir().unwrap();
        fs::write(bindir.path().join("codex"), "#!/bin/sh\n").unwrap();
        let installed = installed_agents(home.path(), &[bindir.path().to_path_buf()]);
        assert_eq!(installed, vec![Agent::Codex]);
    }

    #[test]
    fn opencode_detected_under_either_config_dir() {
        let home = tempdir().unwrap();
        fs::create_dir_all(home.path().join(".opencode")).unwrap();
        assert!(Agent::Opencode.is_installed(home.path(), &[]));

        let home2 = tempdir().unwrap();
        fs::create_dir_all(home2.path().join(".config/opencode")).unwrap();
        assert!(Agent::Opencode.is_installed(home2.path(), &[]));
    }
}
