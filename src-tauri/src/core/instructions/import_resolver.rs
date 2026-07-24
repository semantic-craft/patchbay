//! `@path` import resolution for instructions files — the content-side isomorph
//! of the chain module's symlink `link_tracer`.
//!
//! Claude Code treats a bare `@path` token as an "content symlink": the imported
//! file's text is spliced into context. This resolver follows those imports from
//! a root file hop by hop (≤4 hops, design §1), resolving each target relative to
//! the file it appears in, skipping tokens inside code spans / fenced blocks, and
//! detecting cycles. It never evaluates content — only path existence (§8).
//!
//! Only Claude expands imports; other agents read a `@path` line as literal text
//! (matrix conclusion ②). Callers therefore run this for the `claude` surface
//! only.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use serde::Serialize;

/// Maximum import depth followed from the root file. A file's direct imports are
/// hop 1; their imports hop 2; and so on. Imports beyond this are not followed
/// and set `truncated`.
pub const MAX_HOPS: usize = 4;

/// One resolved `@path` import.
#[derive(Debug, Clone, Serialize)]
pub struct Import {
    /// The path token as written (the text after `@`).
    pub raw: String,
    /// Absolute path of the file the token appears in.
    pub source: String,
    /// 1-based line number of the import within its source file.
    pub line: usize,
    /// Resolved, lexically normalized absolute target path.
    pub target: String,
    /// Whether the target exists as a regular file.
    pub exists: bool,
    /// Import depth from the root file (1 = imported directly by the root).
    pub hop: usize,
}

/// The flattened import graph reachable from a root file.
#[derive(Debug, Clone, Serialize, Default)]
pub struct ImportResolution {
    /// Imports in discovery order, de-duplicated by resolved target (first
    /// occurrence wins), so a diamond import is counted once.
    pub imports: Vec<Import>,
    /// True when at least one import lay beyond `MAX_HOPS` and was not followed.
    pub truncated: bool,
    /// True when an import points back at a file already on its own ancestor
    /// path (a genuine cycle, not merely a re-import).
    pub cyclic: bool,
}

/// Resolve `.`/`..` lexically, without touching the filesystem (same policy as
/// the chain link tracer: report the path a `readlink`-style inspection shows).
fn normalize(path: &Path) -> PathBuf {
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

/// Resolve an import token written in `source_file` to an absolute path.
/// `~` and `~/…` expand against `home`; absolute tokens are normalized as-is;
/// everything else resolves against the source file's directory.
fn resolve_target(source_file: &Path, raw: &str, home: &Path) -> PathBuf {
    let candidate = if raw == "~" {
        home.to_path_buf()
    } else if let Some(rest) = raw.strip_prefix("~/") {
        home.join(rest)
    } else {
        let p = Path::new(raw);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            source_file.parent().unwrap_or(Path::new("/")).join(p)
        }
    };
    normalize(&candidate)
}

/// Blank out inline code spans in `line`, preserving character indices so line
/// content outside code is untouched. Backtick delimiters toggle a "code"
/// region; a `@token` inside backticks is thereby ignored.
fn mask_inline_code(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut in_code = false;
    for ch in line.chars() {
        if ch == '`' {
            in_code = !in_code;
            out.push(' ');
        } else if in_code {
            out.push(' ');
        } else {
            out.push(ch);
        }
    }
    out
}

/// Extract `@path` import tokens from markdown `text`, skipping fenced code
/// blocks and inline code spans. Returns `(1-based line, raw token)` pairs. A
/// token is recognized only at a word boundary (line start or after whitespace),
/// so `foo@bar.com` and email-like text are not treated as imports.
fn scan_import_tokens(text: &str) -> Vec<(usize, String)> {
    let mut tokens = Vec::new();
    let mut in_fence = false;
    for (idx, raw_line) in text.lines().enumerate() {
        let trimmed = raw_line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        let masked = mask_inline_code(raw_line);
        let bytes = masked.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'@' {
                let boundary = i == 0 || bytes[i - 1].is_ascii_whitespace();
                if boundary {
                    // Token = everything up to the next ASCII whitespace.
                    let start = i + 1;
                    let mut end = start;
                    while end < bytes.len() && !bytes[end].is_ascii_whitespace() {
                        end += 1;
                    }
                    if end > start {
                        let token = &masked[start..end];
                        tokens.push((idx + 1, token.to_string()));
                    }
                    i = end;
                    continue;
                }
            }
            i += 1;
        }
    }
    tokens
}

/// Whether a file contains at least one import token (used only to decide
/// whether hop-limit truncation actually dropped anything).
fn file_has_imports(path: &Path) -> bool {
    match std::fs::read_to_string(path) {
        Ok(text) => !scan_import_tokens(&text).is_empty(),
        Err(_) => false,
    }
}

/// Resolve every `@path` import reachable from `root_file` (≤`MAX_HOPS` hops).
/// De-duplicates targets, flags hop-limit truncation and genuine cycles, and
/// records whether each target exists. Never reads a target's content beyond the
/// import scan; unreadable or non-UTF-8 files simply yield no further imports.
pub fn resolve(root_file: &Path, home: &Path) -> ImportResolution {
    let root_norm = normalize(root_file);
    let mut result = ImportResolution::default();
    let mut recorded: HashSet<PathBuf> = HashSet::new();
    let mut ancestors: Vec<PathBuf> = vec![root_norm.clone()];
    walk(
        &root_norm,
        1,
        home,
        &mut ancestors,
        &mut recorded,
        &mut result,
    );
    result
}

#[allow(clippy::too_many_arguments)]
fn walk(
    file: &Path,
    hop: usize,
    home: &Path,
    ancestors: &mut Vec<PathBuf>,
    recorded: &mut HashSet<PathBuf>,
    result: &mut ImportResolution,
) {
    if hop > MAX_HOPS {
        // Past the hop limit: don't record, but note if anything was dropped.
        if file_has_imports(file) {
            result.truncated = true;
        }
        return;
    }
    let text = match std::fs::read_to_string(file) {
        Ok(t) => t,
        Err(_) => return,
    };
    for (line, raw) in scan_import_tokens(&text) {
        let target = resolve_target(file, &raw, home);
        let exists = target.is_file();
        let is_cycle = ancestors.contains(&target);
        if is_cycle {
            result.cyclic = true;
        }
        if recorded.insert(target.clone()) {
            result.imports.push(Import {
                raw,
                source: file.to_string_lossy().to_string(),
                line,
                target: target.to_string_lossy().to_string(),
                exists,
                hop,
            });
        }
        if exists && !is_cycle {
            ancestors.push(target.clone());
            walk(&target, hop + 1, home, ancestors, recorded, result);
            ancestors.pop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn resolves_relative_import_target() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("CLAUDE.md");
        fs::write(&root, "@AGENTS.md\n").unwrap();
        fs::write(dir.path().join("AGENTS.md"), "# body\n").unwrap();

        let res = resolve(&root, dir.path());
        assert_eq!(res.imports.len(), 1);
        assert_eq!(res.imports[0].raw, "AGENTS.md");
        assert_eq!(res.imports[0].hop, 1);
        assert!(res.imports[0].exists);
        assert_eq!(
            res.imports[0].target,
            dir.path().join("AGENTS.md").to_string_lossy()
        );
        assert!(!res.truncated && !res.cyclic);
    }

    #[test]
    fn missing_target_is_reported_not_followed() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("CLAUDE.md");
        fs::write(&root, "@AGENTS.md\n").unwrap();

        let res = resolve(&root, dir.path());
        assert_eq!(res.imports.len(), 1);
        assert!(!res.imports[0].exists);
    }

    #[test]
    fn skips_imports_in_code_span_and_fence() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("CLAUDE.md");
        fs::write(
            &root,
            "Use `@not-an-import` inline.\n\n```\n@also-not.md\n```\n\n@real.md\n",
        )
        .unwrap();
        fs::write(dir.path().join("real.md"), "x").unwrap();

        let res = resolve(&root, dir.path());
        let raws: Vec<&str> = res.imports.iter().map(|i| i.raw.as_str()).collect();
        assert_eq!(raws, ["real.md"]);
    }

    #[test]
    fn does_not_treat_email_as_import() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("CLAUDE.md");
        fs::write(&root, "Contact foo@bar.com for help.\n").unwrap();

        let res = resolve(&root, dir.path());
        assert!(res.imports.is_empty());
    }

    #[test]
    fn truncates_beyond_max_hops() {
        // Chain a.md -> b.md -> c.md -> d.md -> e.md -> f.md (5 imports deep).
        let dir = tempdir().unwrap();
        let names = ["a", "b", "c", "d", "e", "f"];
        for w in names.windows(2) {
            fs::write(
                dir.path().join(format!("{}.md", w[0])),
                format!("@{}.md\n", w[1]),
            )
            .unwrap();
        }
        fs::write(dir.path().join("f.md"), "leaf\n").unwrap();

        let root = dir.path().join("a.md");
        let res = resolve(&root, dir.path());
        // hops 1..=4 recorded: b, c, d, e. f would be hop 5 → truncated.
        let raws: Vec<&str> = res.imports.iter().map(|i| i.raw.as_str()).collect();
        assert_eq!(raws, ["b.md", "c.md", "d.md", "e.md"]);
        assert!(res.truncated);
        assert!(!res.cyclic);
    }

    #[test]
    fn detects_cycle_without_infinite_loop() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.md"), "@b.md\n").unwrap();
        fs::write(dir.path().join("b.md"), "@a.md\n").unwrap();

        let root = dir.path().join("a.md");
        let res = resolve(&root, dir.path());
        assert!(res.cyclic);
        // a→b recorded (hop 1); b→a points back at the root → cycle, a already
        // recorded as ancestor so not re-followed.
        let raws: Vec<&str> = res.imports.iter().map(|i| i.raw.as_str()).collect();
        assert_eq!(raws, ["b.md", "a.md"]);
    }

    #[test]
    fn diamond_import_is_not_a_cycle() {
        // root imports b and c; both import d. d must be recorded once, no cycle.
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("root.md"), "@b.md\n@c.md\n").unwrap();
        fs::write(dir.path().join("b.md"), "@d.md\n").unwrap();
        fs::write(dir.path().join("c.md"), "@d.md\n").unwrap();
        fs::write(dir.path().join("d.md"), "leaf\n").unwrap();

        let res = resolve(&dir.path().join("root.md"), dir.path());
        assert!(!res.cyclic);
        let d_count = res.imports.iter().filter(|i| i.raw == "d.md").count();
        assert_eq!(d_count, 1);
    }

    #[test]
    fn expands_home_prefixed_import() {
        let home = tempdir().unwrap();
        let proj = tempdir().unwrap();
        fs::write(proj.path().join("CLAUDE.md"), "@~/global.md\n").unwrap();
        fs::write(home.path().join("global.md"), "g").unwrap();

        let res = resolve(&proj.path().join("CLAUDE.md"), home.path());
        assert_eq!(res.imports.len(), 1);
        assert_eq!(
            res.imports[0].target,
            home.path().join("global.md").to_string_lossy()
        );
        assert!(res.imports[0].exists);
    }
}
