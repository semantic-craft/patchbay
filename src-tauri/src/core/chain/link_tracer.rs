//! Symlink chain resolution: follow an entry hop by hop, recording each
//! target as written (lexically normalized, never canonicalized) so the
//! UI can show the same paths a `readlink` inspection would.

use serde::Serialize;
use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

const MAX_HOPS: usize = 16;

#[derive(Debug, Clone, Serialize)]
pub struct Trace {
    pub is_link: bool,
    /// Absolute path of each hop target, in order.
    pub hops: Vec<String>,
    /// Where the chain ends (equals the entry itself for physical paths).
    pub final_target: String,
    pub exists: bool,
    pub cyclic: bool,
}

/// Resolve `.` / `..` lexically, without touching the filesystem.
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

pub fn trace(entry: &Path) -> Trace {
    let mut hops = Vec::new();
    let mut current = entry.to_path_buf();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut cyclic = false;
    let mut is_link = false;

    for _ in 0..MAX_HOPS {
        let meta = match std::fs::symlink_metadata(&current) {
            Ok(m) => m,
            Err(_) => break,
        };
        if !meta.file_type().is_symlink() {
            break;
        }
        is_link = true;
        let target = match std::fs::read_link(&current) {
            Ok(t) => t,
            Err(_) => break,
        };
        let resolved = if target.is_absolute() {
            normalize(&target)
        } else {
            normalize(&current.parent().unwrap_or(Path::new("/")).join(target))
        };
        if !seen.insert(resolved.clone()) {
            cyclic = true;
            break;
        }
        hops.push(resolved.to_string_lossy().to_string());
        current = resolved;
    }

    let exists = std::fs::metadata(&current).is_ok();
    Trace {
        is_link,
        hops,
        final_target: current.to_string_lossy().to_string(),
        exists,
        cyclic,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("chain-tracer-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn normalize_resolves_dot_segments() {
        assert_eq!(
            normalize(Path::new("/a/b/../c/./d")),
            PathBuf::from("/a/c/d")
        );
    }

    #[test]
    fn traces_relative_two_hop_chain() {
        use crate::core::test_support::symlink_dir;

        let root = temp_root("twohop");
        let original = root.join("original");
        std::fs::create_dir(&original).unwrap();
        let mid = root.join("mid");
        symlink_dir(Path::new("original"), &mid).unwrap();
        let sub = root.join("sub");
        std::fs::create_dir(&sub).unwrap();
        let entry = sub.join("entry");
        symlink_dir(Path::new("../mid"), &entry).unwrap();

        let tr = trace(&entry);
        assert!(tr.is_link);
        assert!(tr.exists);
        assert!(!tr.cyclic);
        assert_eq!(tr.hops.len(), 2);
        assert_eq!(tr.final_target, original.to_string_lossy());

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn flags_broken_and_cyclic_links() {
        use crate::core::test_support::symlink_dir;

        let root = temp_root("broken");
        let dangling = root.join("dangling");
        symlink_dir(&root.join("nowhere"), &dangling).unwrap();
        let tr = trace(&dangling);
        assert!(tr.is_link && !tr.exists && !tr.cyclic);

        let a = root.join("a");
        let b = root.join("b");
        symlink_dir(&b, &a).unwrap();
        symlink_dir(&a, &b).unwrap();
        let tr = trace(&a);
        assert!(tr.cyclic);

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn physical_dir_is_not_a_link() {
        let root = temp_root("physical");
        let tr = trace(&root);
        assert!(!tr.is_link && tr.exists && tr.hops.is_empty());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
