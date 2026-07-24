//! Content snapshots for instructions write operations (design §7/§8).
//!
//! Chain's write path only ever creates or removes *symlinks*, so its snapshot
//! records a link target. Instructions rewrites and removes *file content* — a
//! data-loss risk an order of magnitude higher — so before any rewrite or
//! symlink-to-wrapper conversion, the original bytes (and, for a symlink entry,
//! the raw link target) are copied here first ("快照先行", §8).
//!
//! Each snapshot is a directory named by a zero-padded millisecond id under the
//! OS-local app data dir — never the central repo, a project, or any git-synced
//! surface (§7). It holds a `manifest.json` plus one payload file per captured
//! regular file. The newest [`MAX_SNAPSHOTS`] snapshots are retained; older ones
//! are pruned on each capture.
//!
//! v1 ships no restore command (design §10): recovery is manual — read
//! `manifest.json`, copy each `payload` back to its `original_path`, or recreate
//! the symlink from `symlink_target`. The snapshot exists so a bad write is
//! always reversible by hand.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::core::path_guard;

/// How many snapshots to keep. Older ones are pruned on each [`capture`].
pub const MAX_SNAPSHOTS: usize = 50;

/// The manifest file written at the root of every snapshot directory.
pub const MANIFEST_NAME: &str = "manifest.json";

/// One captured target: a regular file (bytes copied to `payload`, `sha256` of
/// those bytes) or a symlink (`symlink_target` recorded, no byte payload — the
/// link's target file is never touched, §8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotEntry {
    /// Absolute source path that was captured before being rewritten/removed.
    pub original_path: String,
    /// SHA-256 of the captured bytes; empty when the source was a symlink.
    pub sha256: String,
    /// Raw (lexical) link target when the source was a symlink; `None` for a
    /// regular file.
    pub symlink_target: Option<String>,
    /// Filename, relative to the snapshot directory, holding the original bytes;
    /// `None` when there is no byte payload (a symlink).
    pub payload: Option<String>,
}

/// The `manifest.json` at the root of a snapshot directory: enough to restore
/// every captured entry by hand without any other state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotManifest {
    /// Snapshot id (also the directory name): zero-padded capture time in ms.
    pub id: String,
    /// Capture time in milliseconds since the Unix epoch.
    pub created_at: i64,
    pub entries: Vec<SnapshotEntry>,
}

/// SHA-256 of a byte slice, lowercase hex — the content fingerprint used both
/// here and by the TOCTOU guard so a snapshot and its guard always agree.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// The production snapshot root: the OS-local app data dir, never the central
/// repo (which may live on Dropbox / a git remote) or a project. On Windows this
/// is `%LOCALAPPDATA%` (non-roaming) so snapshots never sync between machines.
///
/// Returns an error when no local data dir can be determined — the caller must
/// then refuse the write, since "快照先行" means no snapshot ⇒ no mutation.
pub fn default_root() -> Result<PathBuf> {
    let base = dirs::data_local_dir()
        .ok_or_else(|| anyhow!("cannot determine a local app data directory for snapshots"))?;
    Ok(base.join("patchbay").join("instructions").join("snapshots"))
}

/// Capture `sources` into a new snapshot under `root`, then prune to the newest
/// [`MAX_SNAPSHOTS`]. Every source must currently be a regular file or a
/// symlink; a directory, a missing path, or an unreadable file is an error so
/// the caller fails safe (no snapshot ⇒ no write).
///
/// `now_ms` is supplied by the caller (the service passes
/// `chrono::Utc::now().timestamp_millis()`) so captures are deterministic under
/// test, mirroring how the scanner takes `scanned_at`.
pub fn capture(root: &Path, now_ms: i64, sources: &[PathBuf]) -> Result<SnapshotManifest> {
    std::fs::create_dir_all(root)
        .with_context(|| format!("creating snapshot root {}", root.display()))?;
    let (id, dir) = allocate_dir(root, now_ms)?;

    let mut entries = Vec::with_capacity(sources.len());
    for (index, source) in sources.iter().enumerate() {
        entries.push(capture_one(&dir, index, source)?);
    }

    let manifest = SnapshotManifest {
        id,
        created_at: now_ms,
        entries,
    };
    let manifest_json = serde_json::to_vec_pretty(&manifest).context("serializing manifest")?;
    std::fs::write(dir.join(MANIFEST_NAME), manifest_json)
        .with_context(|| format!("writing {MANIFEST_NAME} in {}", dir.display()))?;

    prune(root, MAX_SNAPSHOTS)?;
    Ok(manifest)
}

/// Capture a single source into `dir`, returning its manifest entry.
fn capture_one(dir: &Path, index: usize, source: &Path) -> Result<SnapshotEntry> {
    let original_path = source.to_string_lossy().to_string();
    let meta = std::fs::symlink_metadata(source)
        .with_context(|| format!("stat {} for snapshot", source.display()))?;

    if meta.file_type().is_symlink() {
        let target = std::fs::read_link(source)
            .with_context(|| format!("read_link {} for snapshot", source.display()))?
            .to_string_lossy()
            .to_string();
        return Ok(SnapshotEntry {
            original_path,
            sha256: String::new(),
            symlink_target: Some(target),
            payload: None,
        });
    }

    if meta.is_dir() {
        return Err(anyhow!(
            "refusing to snapshot a directory: {} (only files and symlinks are captured)",
            source.display()
        ));
    }

    // Regular file: copy the exact bytes into the snapshot dir.
    let bytes = std::fs::read(source)
        .with_context(|| format!("reading {} for snapshot", source.display()))?;
    let name = source
        .file_name()
        .map(|n| path_guard::sanitize_name(&n.to_string_lossy()))
        .unwrap_or_else(|| "file".to_string());
    let payload = format!("{index:04}-{name}");
    std::fs::write(dir.join(&payload), &bytes)
        .with_context(|| format!("writing snapshot payload for {}", source.display()))?;

    Ok(SnapshotEntry {
        original_path,
        sha256: sha256_hex(&bytes),
        symlink_target: None,
        payload: Some(payload),
    })
}

/// Create the snapshot directory for `now_ms`, disambiguating a collision (two
/// captures in the same millisecond) with a `-N` suffix. The id is a fixed-width
/// zero-padded millisecond count so directory names sort lexically in capture
/// order, which is what [`prune`] relies on.
fn allocate_dir(root: &Path, now_ms: i64) -> Result<(String, PathBuf)> {
    let stamp = format!("{now_ms:013}");
    for suffix in 0..1000 {
        let id = if suffix == 0 {
            stamp.clone()
        } else {
            format!("{stamp}-{suffix}")
        };
        let dir = root.join(&id);
        match std::fs::create_dir(&dir) {
            Ok(()) => return Ok((id, dir)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(anyhow!("creating snapshot dir {}: {e}", dir.display()));
            }
        }
    }
    Err(anyhow!(
        "could not allocate a snapshot directory under {} for {now_ms}",
        root.display()
    ))
}

/// Keep the newest `keep` snapshot directories under `root`, removing the rest.
/// Snapshot dirs are named by fixed-width id, so a lexical sort is chronological.
/// Non-snapshot entries (a stray file, an unrelated dir) are ignored.
fn prune(root: &Path, keep: usize) -> Result<()> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(root)
        .with_context(|| format!("listing snapshot root {}", root.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|e| e.path())
        .collect();
    if dirs.len() <= keep {
        return Ok(());
    }
    dirs.sort();
    let cutoff = dirs.len() - keep;
    for stale in &dirs[..cutoff] {
        // Best-effort: a failure to prune one old snapshot must not abort a write
        // whose fresh snapshot already landed.
        let _ = std::fs::remove_dir_all(stale);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn captures_file_bytes_and_hash() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().join("snaps");
        let src = tmp.path().join("AGENTS.md");
        fs::write(&src, "canonical body\n").unwrap();

        let manifest = capture(&root, 1_700_000_000_000, &[src.clone()]).unwrap();
        assert_eq!(manifest.id, "1700000000000");
        assert_eq!(manifest.entries.len(), 1);

        let entry = &manifest.entries[0];
        assert_eq!(entry.original_path, src.to_string_lossy());
        assert_eq!(entry.sha256, sha256_hex(b"canonical body\n"));
        assert!(entry.symlink_target.is_none());

        // Payload holds the exact original bytes — the basis for manual restore.
        let payload = entry.payload.as_ref().unwrap();
        let snapped = fs::read(root.join(&manifest.id).join(payload)).unwrap();
        assert_eq!(snapped, b"canonical body\n");
    }

    #[test]
    fn manual_restore_from_manifest_recovers_original() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().join("snaps");
        let src = tmp.path().join("CLAUDE.md");
        fs::write(&src, "old wrapper\n").unwrap();

        let manifest = capture(&root, 1_700_000_000_001, &[src.clone()]).unwrap();
        // Simulate a bad rewrite, then restore by hand exactly as a human would.
        fs::write(&src, "clobbered").unwrap();
        let entry = &manifest.entries[0];
        let payload = root
            .join(&manifest.id)
            .join(entry.payload.as_ref().unwrap());
        fs::copy(&payload, &entry.original_path).unwrap();

        assert_eq!(fs::read_to_string(&src).unwrap(), "old wrapper\n");
    }

    #[test]
    fn captures_symlink_target_without_payload() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().join("snaps");
        let target = tmp.path().join("AGENTS.md");
        fs::write(&target, "body").unwrap();
        let link = tmp.path().join("CLAUDE.md");
        crate::core::test_support::expect_symlink_file(&target, &link);

        let manifest = capture(&root, 1_700_000_000_002, &[link.clone()]).unwrap();
        let entry = &manifest.entries[0];
        assert_eq!(
            entry.symlink_target.as_deref(),
            Some(&*target.to_string_lossy())
        );
        assert!(entry.payload.is_none());
        assert!(entry.sha256.is_empty());
        // The link's target file is untouched by the snapshot.
        assert_eq!(fs::read_to_string(&target).unwrap(), "body");
    }

    #[test]
    fn refuses_directory_source() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().join("snaps");
        let dir_src = tmp.path().join("docs");
        fs::create_dir(&dir_src).unwrap();
        assert!(capture(&root, 1, &[dir_src]).is_err());
    }

    #[test]
    fn refuses_missing_source() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().join("snaps");
        let missing = tmp.path().join("nope.md");
        assert!(capture(&root, 1, &[missing]).is_err());
    }

    #[test]
    fn collision_in_same_millisecond_gets_distinct_dirs() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().join("snaps");
        let src = tmp.path().join("a.md");
        fs::write(&src, "x").unwrap();

        let first = capture(&root, 42, &[src.clone()]).unwrap();
        let second = capture(&root, 42, &[src.clone()]).unwrap();
        assert_ne!(first.id, second.id);
        assert!(root.join(&first.id).is_dir());
        assert!(root.join(&second.id).is_dir());
    }

    #[test]
    fn prunes_to_the_newest_max_snapshots() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().join("snaps");
        let src = tmp.path().join("a.md");
        fs::write(&src, "x").unwrap();

        // Distinct, increasing timestamps → distinct, chronologically-sorted ids.
        for ts in 0..(MAX_SNAPSHOTS as i64 + 5) {
            capture(&root, 1_000 + ts, &[src.clone()]).unwrap();
        }

        let mut remaining: Vec<String> = fs::read_dir(&root)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().unwrap().is_dir())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        remaining.sort();
        assert_eq!(remaining.len(), MAX_SNAPSHOTS);
        // The oldest five were pruned; the newest survivor is the last stamp.
        assert_eq!(
            remaining.last().unwrap(),
            &format!("{:013}", 1_000 + MAX_SNAPSHOTS as i64 + 4)
        );
        assert!(!remaining.iter().any(|id| id == &format!("{:013}", 1_000)));
    }

    #[test]
    fn default_root_is_under_local_data_not_a_project() {
        // Sanity: the resolved production root sits under the OS local data dir
        // and carries the patchbay/instructions identity (never a project root).
        let root = default_root().unwrap();
        assert!(root.ends_with("patchbay/instructions/snapshots"));
        let local = dirs::data_local_dir().unwrap();
        assert!(root.starts_with(&local));
    }
}
