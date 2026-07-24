//! Meta repo access: the small bare repo on the hub (`_patchbay-fleet.git`)
//! holding `manifest.toml` plus per-machine status reports under `machines/`.
//!
//! This is the ONLY thing fleet P0 ever writes to, and it does so through a
//! local cache kept in the app data directory — never inside the projects
//! root. State files are idempotently regenerable, so recovery from any
//! non-fast-forward surprise is simply re-clone and re-write (design §1).

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::core::error::AppError;
use crate::core::fleet::{
    manifest::{self, Manifest},
    repo_ops,
};
use crate::core::path_guard;

/// One machine's self-reported state, `machines/<id>.json` (design §3).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MachineReport {
    pub machine: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// RFC3339 UTC; consumers show staleness honestly (design axiom 2).
    pub reported_at: String,
    pub repos: Vec<ReportedRepo>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReportedRepo {
    pub name: String,
    pub present: bool,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub dirty: Option<u32>,
    #[serde(default)]
    pub detached: bool,
    pub ahead: Option<u32>,
    pub behind: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Freshness of the local cache after [`MetaRepo::ensure_fresh`].
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum MetaSyncState {
    Fresh,
    /// Hub unreachable; the existing cache is served with this warning.
    Stale {
        error: String,
    },
}

/// Outcome of a report write, mirrored into CLI/GUI payloads.
#[derive(Debug, Clone, Serialize)]
pub struct ReportOutcome {
    /// "reported" | "unchanged"
    pub action: String,
    pub pushed: bool,
    pub commit: Option<String>,
}

/// Result of the narrow P2 manifest edit used by the per-repository automatic
/// round toggle. General manifest editing remains the responsibility of #43.
#[derive(Debug, Clone, Serialize)]
pub struct AutoSyncUpdateOutcome {
    pub repo: String,
    pub enabled: bool,
    pub pushed: bool,
    pub commit: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ManifestWriteOutcome {
    /// `updated`, `unchanged`, or `conflict`.
    pub action: String,
    pub pushed: bool,
    pub commit: Option<String>,
    pub manifest_digest: String,
    pub message: Option<String>,
}

pub struct MetaRepo {
    url: String,
    cache: PathBuf,
}

impl MetaRepo {
    /// Cache under an explicit directory (tests inject a temp dir).
    pub fn at(url: impl Into<String>, cache: PathBuf) -> Self {
        Self {
            url: url.into(),
            cache,
        }
    }

    pub fn cache_dir(&self) -> &Path {
        &self.cache
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    fn git(&self, args: &[&str]) -> Result<String, String> {
        let mut cmd = Command::new("git");
        cmd.env("GIT_TERMINAL_PROMPT", "0");
        cmd.arg("-C").arg(&self.cache);
        // Commits need an identity independent of per-machine git config.
        cmd.args([
            "-c",
            "user.name=patchbay-fleet",
            "-c",
            "user.email=fleet@patchbay.local",
            "-c",
            "commit.gpgsign=false",
        ]);
        cmd.args(args);
        match cmd.output() {
            Ok(out) if out.status.success() => {
                Ok(String::from_utf8_lossy(&out.stdout).into_owned())
            }
            Ok(out) => Err(String::from_utf8_lossy(&out.stderr).trim().to_string()),
            Err(e) => Err(e.to_string()),
        }
    }

    /// Bring the cache up to date: clone when absent, otherwise ff-only pull.
    /// A pull that cannot fast-forward triggers the re-clone path (into a
    /// sibling temp dir, swapped in only on success — a failed re-clone keeps
    /// the old cache and degrades to [`MetaSyncState::Stale`]).
    pub fn ensure_fresh(&self) -> Result<MetaSyncState, AppError> {
        if !self.cache.join(".git").exists() {
            self.clone_fresh()?;
            return Ok(MetaSyncState::Fresh);
        }
        match self.git(&["pull", "--ff-only"]) {
            Ok(_) => Ok(MetaSyncState::Fresh),
            Err(pull_err) => match self.clone_fresh() {
                Ok(()) => Ok(MetaSyncState::Fresh),
                Err(_) => Ok(MetaSyncState::Stale { error: pull_err }),
            },
        }
    }

    fn clone_fresh(&self) -> Result<(), AppError> {
        let parent = self
            .cache
            .parent()
            .ok_or_else(|| AppError::internal("meta cache has no parent dir"))?;
        std::fs::create_dir_all(parent).map_err(AppError::io)?;
        let staging = tempfile::tempdir_in(parent).map_err(AppError::io)?;
        let dest = staging.path().join("meta");
        let out = Command::new("git")
            .env("GIT_TERMINAL_PROMPT", "0")
            .arg("clone")
            // `--` keeps a URL that begins with a dash from being parsed as an
            // option (e.g. `--upload-pack=<cmd>`, which git would execute).
            .arg("--")
            .arg(&self.url)
            .arg(&dest)
            .output()
            .map_err(AppError::io)?;
        if !out.status.success() {
            return Err(AppError::git(format!(
                "clone fleet meta repo {}: {}",
                self.url,
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        if self.cache.exists() {
            std::fs::remove_dir_all(&self.cache).map_err(AppError::io)?;
        }
        std::fs::rename(&dest, &self.cache).map_err(AppError::io)?;
        Ok(())
    }

    /// Recreate a missing local-path meta bare from the already validated
    /// cache. The bare is assembled in a sibling temp directory and published
    /// with an atomic no-replace rename only after the cached HEAD was pushed
    /// successfully. Remote URLs never enter this path, so no ssh command can
    /// be executed.
    pub fn init_missing_local_from_cache(&self) -> Result<(), AppError> {
        let target = PathBuf::from(&self.url);
        if !target.is_absolute() {
            return Err(AppError::invalid_input(
                "fleet meta repo initialization is host-only; run fleet init on the hub host",
            ));
        }
        if target.exists() {
            return Err(AppError::invalid_input(format!(
                "fleet meta repo path already exists: {}",
                target.display()
            )));
        }
        self.read_manifest_snapshot()?;
        let branch = self
            .git(&["symbolic-ref", "--short", "HEAD"])
            .map_err(AppError::git)?
            .trim()
            .to_string();
        if branch.is_empty() {
            return Err(AppError::git("fleet meta cache has no branch"));
        }
        let parent = target
            .parent()
            .ok_or_else(|| AppError::invalid_input("fleet meta repo path has no parent"))?;
        std::fs::create_dir_all(parent).map_err(AppError::io)?;
        let staging_parent = tempfile::tempdir_in(parent).map_err(AppError::io)?;
        let staging = staging_parent.path().join("meta.git");
        let output = Command::new("git")
            .args(["init", "--bare", "--initial-branch", &branch])
            .arg(&staging)
            .output()
            .map_err(AppError::io)?;
        if !output.status.success() {
            return Err(AppError::git(format!(
                "initialize local fleet meta repo: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        let refspec = format!("HEAD:refs/heads/{branch}");
        self.git(&["push", "--", staging.to_string_lossy().as_ref(), &refspec])
            .map_err(AppError::git)?;
        repo_ops::publish_directory(&staging, &target).map_err(|error| match error {
            repo_ops::PublishDirectoryError::TargetChanged => AppError::invalid_input(format!(
                "fleet meta repo path changed after preview: {}",
                target.display()
            )),
            repo_ops::PublishDirectoryError::Other(message) => {
                AppError::io(std::io::Error::other(message))
            }
        })?;
        Ok(())
    }

    pub fn read_manifest(&self) -> Result<Manifest, AppError> {
        self.read_manifest_snapshot().map(|(manifest, _)| manifest)
    }

    /// Read the manifest and bind it to a content digest. Fleet write plans
    /// carry this digest across preview/apply so an intervening manifest edit
    /// is a conflict without treating unrelated machine-report commits as one.
    pub fn read_manifest_snapshot(&self) -> Result<(Manifest, String), AppError> {
        let path = self.cache.join("manifest.toml");
        let text = std::fs::read_to_string(&path).map_err(|e| {
            AppError::not_found(format!(
                "manifest.toml missing from fleet meta repo ({}): {e}",
                self.url
            ))
        })?;
        let digest = format!("{:x}", Sha256::digest(text.as_bytes()));
        Ok((manifest::parse(&text)?, digest))
    }

    /// Change only one existing repo's P2 opt-in flag and push the manifest.
    /// A push race is recovered by re-cloning and replaying the same narrow
    /// edit once, matching the report writer's bounded retry semantics.
    pub fn set_repo_auto_sync(
        &self,
        repo_name: &str,
        enabled: bool,
    ) -> Result<AutoSyncUpdateOutcome, AppError> {
        let mut last_err = None;
        for attempt in 0..2 {
            match self.ensure_fresh()? {
                MetaSyncState::Fresh => {}
                MetaSyncState::Stale { error } => {
                    return Err(AppError::git(format!(
                        "fleet manifest update requires fresh metadata: {error}"
                    )))
                }
            }
            let mut manifest = self.read_manifest()?;
            let entry = manifest
                .repos
                .iter_mut()
                .find(|entry| entry.name == repo_name)
                .ok_or_else(|| {
                    AppError::not_found(format!(
                        "repository {repo_name:?} is not in the fleet manifest"
                    ))
                })?;
            if entry.auto_sync == enabled {
                return Ok(AutoSyncUpdateOutcome {
                    repo: repo_name.to_string(),
                    enabled,
                    pushed: false,
                    commit: None,
                });
            }
            entry.auto_sync = enabled;

            let target = self.cache.join("manifest.toml");
            if !path_guard::is_path_safe(&self.cache, &target) {
                return Err(AppError::invalid_input(
                    "manifest target escapes fleet meta cache",
                ));
            }
            let text = manifest::to_toml(&manifest)?;
            std::fs::write(&target, text).map_err(AppError::io)?;
            self.git(&["add", "manifest.toml"]).map_err(AppError::git)?;
            let message = format!(
                "fleet auto sync: {repo_name} {}",
                if enabled { "on" } else { "off" }
            );
            self.git(&["commit", "-m", &message])
                .map_err(AppError::git)?;
            let commit = self
                .git(&["rev-parse", "--short", "HEAD"])
                .map(|value| value.trim().to_string())
                .ok();
            match self.git(&["push", "origin", "HEAD"]) {
                Ok(_) => {
                    return Ok(AutoSyncUpdateOutcome {
                        repo: repo_name.to_string(),
                        enabled,
                        pushed: true,
                        commit,
                    })
                }
                Err(error) => {
                    last_err = Some(error);
                    if attempt == 0 {
                        self.clone_fresh()?;
                    }
                }
            }
        }
        Err(AppError::git(format!(
            "push fleet auto-sync manifest update failed after retry: {}",
            last_err.unwrap_or_default()
        )))
    }

    pub fn head_oid(&self) -> Result<String, AppError> {
        self.git(&["rev-parse", "HEAD"])
            .map(|head| head.trim().to_string())
            .map_err(AppError::git)
    }

    /// Every machine report in the cache. Unparsable files become warnings so
    /// one bad report cannot blank the whole matrix.
    pub fn read_reports(&self) -> (Vec<MachineReport>, Vec<String>) {
        let mut reports = Vec::new();
        let mut warnings = Vec::new();
        let Ok(entries) = std::fs::read_dir(self.cache.join("machines")) else {
            return (reports, warnings);
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            match std::fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|text| {
                    serde_json::from_str::<MachineReport>(&text).map_err(|e| e.to_string())
                }) {
                Ok(report) => reports.push(report),
                Err(e) => warnings.push(format!("{}: {e}", path.display())),
            }
        }
        reports.sort_by(|a, b| a.machine.cmp(&b.machine));
        (reports, warnings)
    }

    /// Write this machine's report and push it to the hub. On a push race
    /// (another machine reported in between) the cache is re-synced and the
    /// write retried — per-machine files make the retry conflict-free.
    pub fn write_report(&self, report: &MachineReport) -> Result<ReportOutcome, AppError> {
        let safe_id = path_guard::sanitize_name(&report.machine);
        if safe_id != report.machine || report.machine.is_empty() {
            return Err(AppError::invalid_input(format!(
                "unsafe machine id {:?}",
                report.machine
            )));
        }
        let mut last_err: Option<String> = None;
        for _attempt in 0..2 {
            self.ensure_fresh()?;
            let rel = format!("machines/{safe_id}.json");
            let target = self.cache.join(&rel);
            // The only write surface in fleet P0: guard it to the cache.
            if !path_guard::is_path_safe(&self.cache, &target) {
                return Err(AppError::invalid_input(format!(
                    "report target escapes meta cache: {rel}"
                )));
            }
            let json = serde_json::to_string_pretty(report)
                .map_err(|e| AppError::internal(format!("serialize report: {e}")))?;
            std::fs::create_dir_all(target.parent().unwrap()).map_err(AppError::io)?;
            std::fs::write(&target, format!("{json}\n")).map_err(AppError::io)?;

            self.git(&["add", &rel]).map_err(AppError::git)?;
            let staged = self
                .git(&["status", "--porcelain"])
                .map_err(AppError::git)?;
            if staged.trim().is_empty() {
                return Ok(ReportOutcome {
                    action: "unchanged".into(),
                    pushed: false,
                    commit: None,
                });
            }
            let message = format!("fleet report: {} {}", report.machine, report.reported_at);
            self.git(&["commit", "-m", &message])
                .map_err(AppError::git)?;
            let commit = self
                .git(&["rev-parse", "--short", "HEAD"])
                .map(|s| s.trim().to_string())
                .ok();
            match self.git(&["push", "origin", "HEAD"]) {
                Ok(_) => {
                    return Ok(ReportOutcome {
                        action: "reported".into(),
                        pushed: true,
                        commit,
                    })
                }
                Err(e) => last_err = Some(e),
            }
        }
        Err(AppError::git(format!(
            "push fleet report failed after retry: {}",
            last_err.unwrap_or_default()
        )))
    }

    /// Write a validated manifest and push it to the hub. A report-only push
    /// race is retried after re-syncing because the manifest digest remains
    /// unchanged; a concurrent manifest edit returns a conflict instead.
    pub fn write_manifest(
        &self,
        expected_digest: &str,
        manifest: &Manifest,
    ) -> Result<ManifestWriteOutcome, AppError> {
        let text = manifest::to_toml(manifest)?;
        let next_digest = format!("{:x}", Sha256::digest(text.as_bytes()));
        let mut last_err: Option<String> = None;
        for _attempt in 0..2 {
            if let MetaSyncState::Stale { error } = self.ensure_fresh()? {
                return Err(AppError::git(format!(
                    "fleet manifest update requires fresh metadata: {error}"
                )));
            }
            let (_, current_digest) = self.read_manifest_snapshot()?;
            if current_digest != expected_digest {
                return Ok(ManifestWriteOutcome {
                    action: "conflict".into(),
                    pushed: false,
                    commit: None,
                    manifest_digest: current_digest,
                    message: Some("fleet manifest changed since preview".into()),
                });
            }

            let rel = "manifest.toml";
            let target = self.cache.join(rel);
            if !path_guard::is_path_safe(&self.cache, &target) {
                return Err(AppError::invalid_input(
                    "manifest target escapes meta cache",
                ));
            }
            std::fs::write(&target, &text).map_err(AppError::io)?;
            self.git(&["add", rel]).map_err(AppError::git)?;
            let staged = self
                .git(&["status", "--porcelain", "--", rel])
                .map_err(AppError::git)?;
            if staged.trim().is_empty() {
                return Ok(ManifestWriteOutcome {
                    action: "unchanged".into(),
                    pushed: false,
                    commit: None,
                    manifest_digest: current_digest,
                    message: None,
                });
            }
            self.git(&[
                "commit",
                "-m",
                "fleet manifest: update managed repositories",
            ])
            .map_err(AppError::git)?;
            let commit = self
                .git(&["rev-parse", "--short", "HEAD"])
                .map(|value| value.trim().to_string())
                .ok();
            match self.git(&["push", "origin", "HEAD"]) {
                Ok(_) => {
                    return Ok(ManifestWriteOutcome {
                        action: "updated".into(),
                        pushed: true,
                        commit,
                        manifest_digest: next_digest,
                        message: None,
                    })
                }
                Err(error) => last_err = Some(error),
            }
        }
        Err(AppError::git(format!(
            "push fleet manifest failed after retry: {}",
            last_err.unwrap_or_default()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn git_in(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.email=test@example.com",
                "-c",
                "user.name=Test",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    const MANIFEST: &str = r#"
[hub.test]
url = "test:mirrors"

[[repo]]
name = "alpha"
hub = "test"
authority = "shared"
branch = "main"
"#;

    /// Bare meta repo seeded with a manifest and one foreign machine report.
    fn seeded_meta(base: &Path) -> PathBuf {
        let bare = base.join("_patchbay-fleet.git");
        let out = Command::new("git")
            .args(["init", "--bare", "--initial-branch=main"])
            .arg(&bare)
            .output()
            .unwrap();
        assert!(out.status.success());
        let seed = base.join("seed");
        std::fs::create_dir_all(&seed).unwrap();
        git_in(&seed, &["init", "-b", "main"]);
        git_in(&seed, &["remote", "add", "origin", bare.to_str().unwrap()]);
        std::fs::write(seed.join("manifest.toml"), MANIFEST).unwrap();
        std::fs::create_dir_all(seed.join("machines")).unwrap();
        std::fs::write(
            seed.join("machines/other.json"),
            serde_json::to_string_pretty(&MachineReport {
                machine: "other".into(),
                display_name: Some("Other Mac".into()),
                reported_at: "2026-07-18T00:00:00Z".into(),
                repos: vec![ReportedRepo {
                    name: "alpha".into(),
                    present: true,
                    branch: Some("main".into()),
                    head: Some("abc1234".into()),
                    dirty: Some(0),
                    ..Default::default()
                }],
            })
            .unwrap(),
        )
        .unwrap();
        git_in(&seed, &["add", "-A"]);
        git_in(&seed, &["commit", "-m", "seed"]);
        git_in(&seed, &["push", "origin", "main"]);
        bare
    }

    fn report(machine: &str, at: &str) -> MachineReport {
        MachineReport {
            machine: machine.into(),
            display_name: None,
            reported_at: at.into(),
            repos: vec![],
        }
    }

    #[test]
    fn clones_reads_manifest_and_reports() {
        let temp = tempdir().unwrap();
        let bare = seeded_meta(temp.path());
        let meta = MetaRepo::at(bare.to_str().unwrap(), temp.path().join("cache/meta"));

        assert!(matches!(meta.ensure_fresh().unwrap(), MetaSyncState::Fresh));
        let manifest = meta.read_manifest().unwrap();
        assert_eq!(manifest.repos[0].name, "alpha");
        let (reports, warnings) = meta.read_reports();
        assert!(warnings.is_empty());
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].machine, "other");
    }

    #[test]
    fn write_report_pushes_to_hub() {
        let temp = tempdir().unwrap();
        let bare = seeded_meta(temp.path());
        let meta = MetaRepo::at(bare.to_str().unwrap(), temp.path().join("cache/meta"));

        let outcome = meta
            .write_report(&report("selfie", "2026-07-18T01:00:00Z"))
            .unwrap();
        assert_eq!(outcome.action, "reported");
        assert!(outcome.pushed);

        // The hub's tip now contains the file (verified via the bare repo).
        let out = Command::new("git")
            .arg("--git-dir")
            .arg(&bare)
            .args(["show", "HEAD:machines/selfie.json"])
            .output()
            .unwrap();
        assert!(out.status.success());
        let shown: MachineReport =
            serde_json::from_slice(&out.stdout).expect("pushed report parses");
        assert_eq!(shown.machine, "selfie");
    }

    #[test]
    fn write_manifest_commits_and_pushes_a_distinct_change() {
        let temp = tempdir().unwrap();
        let bare = seeded_meta(temp.path());
        let meta = MetaRepo::at(bare.to_str().unwrap(), temp.path().join("cache/meta"));
        meta.ensure_fresh().unwrap();
        let (mut manifest, digest) = meta.read_manifest_snapshot().unwrap();
        manifest.repos.push(manifest::RepoEntry {
            name: "beta".into(),
            hub: "test".into(),
            authority: "shared".into(),
            branch: "main".into(),
            auto_sync: false,
        });

        let outcome = meta.write_manifest(&digest, &manifest).unwrap();

        assert_eq!(outcome.action, "updated");
        assert!(outcome.pushed);
        let subject = Command::new("git")
            .arg("--git-dir")
            .arg(&bare)
            .args(["log", "-1", "--format=%s"])
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8(subject.stdout).unwrap().trim(),
            "fleet manifest: update managed repositories"
        );
        let shown = Command::new("git")
            .arg("--git-dir")
            .arg(&bare)
            .args(["show", "HEAD:manifest.toml"])
            .output()
            .unwrap();
        assert!(shown.status.success());
        assert_eq!(
            manifest::parse(&String::from_utf8(shown.stdout).unwrap())
                .unwrap()
                .repos
                .len(),
            2
        );
    }

    #[cfg(unix)]
    #[test]
    fn write_manifest_survives_a_report_push_race_via_retry() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempdir().unwrap();
        let bare = seeded_meta(temp.path());
        let meta = MetaRepo::at(bare.to_str().unwrap(), temp.path().join("cache/meta"));
        meta.ensure_fresh().unwrap();
        let (mut manifest, digest) = meta.read_manifest_snapshot().unwrap();
        manifest.repos.push(manifest::RepoEntry {
            name: "beta".into(),
            hub: "test".into(),
            authority: "shared".into(),
            branch: "main".into(),
            auto_sync: false,
        });

        let racer = temp.path().join("racer");
        let cloned = Command::new("git")
            .arg("clone")
            .arg(&bare)
            .arg(&racer)
            .output()
            .unwrap();
        assert!(cloned.status.success());
        std::fs::write(racer.join("machines/racer.json"), "{}\n").unwrap();
        git_in(&racer, &["add", "machines/racer.json"]);
        git_in(
            &racer,
            &["commit", "-m", "fleet report: racer 2026-07-18T04:00:00Z"],
        );

        let hook = meta.cache_dir().join(".git/hooks/pre-push");
        let marker = temp.path().join("race-fired");
        std::fs::write(
            &hook,
            format!(
                "#!/bin/sh\nif [ ! -e '{}' ]; then\n  touch '{}'\n  git -C '{}' push origin main\nfi\n",
                marker.display(),
                marker.display(),
                racer.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();

        let outcome = meta.write_manifest(&digest, &manifest).unwrap();

        assert!(marker.exists(), "fixture must force the first push race");
        assert!(outcome.pushed);
        let manifest_text = Command::new("git")
            .arg("--git-dir")
            .arg(&bare)
            .args(["show", "HEAD:manifest.toml"])
            .output()
            .unwrap();
        assert_eq!(
            manifest::parse(&String::from_utf8(manifest_text.stdout).unwrap())
                .unwrap()
                .repos
                .len(),
            2
        );
        let raced_report = Command::new("git")
            .arg("--git-dir")
            .arg(&bare)
            .args(["show", "HEAD:machines/racer.json"])
            .output()
            .unwrap();
        assert!(raced_report.status.success());
    }

    #[test]
    fn write_report_survives_a_push_race_via_retry() {
        let temp = tempdir().unwrap();
        let bare = seeded_meta(temp.path());
        let meta = MetaRepo::at(bare.to_str().unwrap(), temp.path().join("cache/meta"));
        meta.ensure_fresh().unwrap();

        // Another machine pushes between our clone and our push.
        let racer = MetaRepo::at(bare.to_str().unwrap(), temp.path().join("racer/meta"));
        racer
            .write_report(&report("racer", "2026-07-18T02:00:00Z"))
            .unwrap();

        let outcome = meta
            .write_report(&report("selfie", "2026-07-18T03:00:00Z"))
            .unwrap();
        assert!(outcome.pushed);
        // Both reports survive on the hub (per-machine files, no conflicts).
        let (reports, _) = {
            racer.ensure_fresh().unwrap();
            racer.read_reports()
        };
        let ids: Vec<_> = reports.iter().map(|r| r.machine.as_str()).collect();
        assert!(ids.contains(&"racer") && ids.contains(&"selfie"));
    }

    #[test]
    fn diverged_cache_recovers_by_recloning() {
        let temp = tempdir().unwrap();
        let bare = seeded_meta(temp.path());
        let meta = MetaRepo::at(bare.to_str().unwrap(), temp.path().join("cache/meta"));
        meta.ensure_fresh().unwrap();

        // Rewrite hub history so the cache can no longer fast-forward.
        let rewrite = temp.path().join("rewrite");
        let out = Command::new("git")
            .arg("clone")
            .arg(&bare)
            .arg(&rewrite)
            .output()
            .unwrap();
        assert!(out.status.success());
        git_in(&rewrite, &["commit", "--amend", "-m", "rewritten"]);
        git_in(&rewrite, &["push", "--force", "origin", "main"]);

        assert!(matches!(meta.ensure_fresh().unwrap(), MetaSyncState::Fresh));
        let head = meta.git(&["log", "-1", "--format=%s"]).unwrap();
        assert_eq!(head.trim(), "rewritten");
    }

    #[test]
    fn unreachable_hub_with_cache_degrades_to_stale() {
        let temp = tempdir().unwrap();
        let bare = seeded_meta(temp.path());
        let meta = MetaRepo::at(bare.to_str().unwrap(), temp.path().join("cache/meta"));
        meta.ensure_fresh().unwrap();

        std::fs::remove_dir_all(&bare).unwrap();
        match meta.ensure_fresh().unwrap() {
            MetaSyncState::Stale { error } => assert!(!error.is_empty()),
            other => panic!("expected stale, got {other:?}"),
        }
        // Cached manifest still serves reads.
        assert_eq!(meta.read_manifest().unwrap().repos[0].name, "alpha");
    }

    #[test]
    fn write_report_rejects_unsafe_machine_ids() {
        let temp = tempdir().unwrap();
        let bare = seeded_meta(temp.path());
        let meta = MetaRepo::at(bare.to_str().unwrap(), temp.path().join("cache/meta"));
        let err = meta
            .write_report(&report("../evil", "2026-07-18T00:00:00Z"))
            .unwrap_err();
        assert!(err.message.contains("unsafe machine id"));
    }

    #[test]
    fn repo_auto_sync_toggle_pushes_only_the_opt_in_field() {
        let temp = tempdir().unwrap();
        let bare = seeded_meta(temp.path());
        let meta = MetaRepo::at(bare.to_str().unwrap(), temp.path().join("cache/meta"));

        let outcome = meta.set_repo_auto_sync("alpha", true).unwrap();
        assert_eq!(outcome.repo, "alpha");
        assert!(outcome.enabled);
        assert!(outcome.pushed);

        let out = Command::new("git")
            .arg("--git-dir")
            .arg(&bare)
            .args(["show", "HEAD:manifest.toml"])
            .output()
            .unwrap();
        assert!(out.status.success());
        let manifest = manifest::parse(&String::from_utf8(out.stdout).unwrap()).unwrap();
        assert!(manifest.repos[0].auto_sync);
        assert_eq!(manifest.repos[0].authority, "shared");
        assert_eq!(manifest.hubs["test"].url, "test:mirrors");
    }

    #[test]
    fn repo_auto_sync_toggle_refuses_unknown_repo() {
        let temp = tempdir().unwrap();
        let bare = seeded_meta(temp.path());
        let meta = MetaRepo::at(bare.to_str().unwrap(), temp.path().join("cache/meta"));

        let err = meta.set_repo_auto_sync("missing", true).unwrap_err();
        assert!(err.message.contains("not in the fleet manifest"));
    }
}
