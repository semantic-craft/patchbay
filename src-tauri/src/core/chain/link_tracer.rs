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

/// Resolve a link target against the directory that will contain the link and
/// return a lexically normalized absolute path. This deliberately does not
/// canonicalize: following an aggregate Skill link would skip the project
/// `.agents/skills` layer and collapse the three-tier chain into the Original.
pub fn absolute_target_for_link(target: &Path, link: &Path) -> std::io::Result<PathBuf> {
    if target.is_absolute() {
        return Ok(normalize(target));
    }

    let parent = link.parent().unwrap_or_else(|| Path::new("."));
    let absolute_parent = if parent.is_absolute() {
        parent.to_path_buf()
    } else {
        std::env::current_dir()?.join(parent)
    };
    Ok(normalize(&absolute_parent.join(target)))
}

/// Whether an existing link uses the native target shape required by this OS.
/// Windows chain links are absolute by invariant; Unix deliberately preserves
/// the caller's relative target spelling.
pub fn native_target_shape_ok(link: &Path) -> bool {
    #[cfg(windows)]
    {
        std::fs::read_link(link).is_ok_and(|target| target.is_absolute())
    }
    #[cfg(not(windows))]
    {
        let _ = link;
        true
    }
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
    fn relative_link_target_becomes_lexically_normalized_absolute_path() {
        let root = temp_root("absolute-target");
        let link = root.join("project/.claude/skills");

        assert_eq!(
            absolute_target_for_link(Path::new("../.agents/./skills"), &link).unwrap(),
            root.join("project/.agents/skills")
        );

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn per_skill_target_stays_at_the_aggregate_layer_without_canonicalizing() {
        let root = temp_root("aggregate-layer");
        let original = root.join("warehouse/repo/skills/demo");
        std::fs::create_dir_all(&original).unwrap();
        let aggregate = root.join("project/.agents/skills/demo");
        std::fs::create_dir_all(aggregate.parent().unwrap()).unwrap();
        crate::core::test_support::symlink_dir(&original, &aggregate).unwrap();
        let surface_entry = root.join("project/.claude/skills/demo");

        assert_eq!(
            absolute_target_for_link(Path::new("../../.agents/skills/demo"), &surface_entry)
                .unwrap(),
            aggregate
        );

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn windows_relative_native_target_is_never_healthy() {
        let root = temp_root("relative-native-target");
        let target = root.join("project/.agents/skills");
        std::fs::create_dir_all(&target).unwrap();
        let link = root.join("project/.claude/skills");
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::windows::fs::symlink_dir(Path::new("../.agents/skills"), &link).unwrap();

        assert!(!native_target_shape_ok(&link));

        std::fs::remove_dir_all(&root).unwrap();
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
