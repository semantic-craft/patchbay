//! Markdown block-ization and normalized block fingerprints (design §4.1 step 2).
//!
//! A single, shared segmentation used wherever two instructions bodies are
//! compared block-by-block: Doctor's `dual_body` overlap statistics and
//! `duplicate_content` ≥50% test both key on it, and the P2 `normalize`
//! mechanical merge (#17) reuses the *same* implementation so the evidence a
//! finding shows and the merge that acts on it can never disagree.
//!
//! Segmentation rules (design §4.1):
//! - a fenced code block (```` ``` ```` or `~~~` … closing fence) is one whole
//!   block, fences included;
//! - an ATX heading line (`#`–`######` followed by space or end) is its own
//!   standalone block;
//! - otherwise, runs of non-blank lines separated by blank lines are paragraphs.
//!
//! A block's fingerprint is the SHA-256 of its *normalized* text — each line
//! right-trimmed, runs of blank lines collapsed, leading/trailing blanks dropped
//! — so cosmetic whitespace edits do not change identity while real content does.
//! This is content identity for *merge/overlap*, deliberately distinct from a
//! Doctor finding's fingerprint (which keys on file/name identity, never
//! content, so editing a body never refreshes an ignore — design §3).

use sha2::{Digest, Sha256};

/// One markdown block: its verbatim text, the 1-based line it starts on, and a
/// fingerprint over its normalized content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    /// The block's original text (verbatim, newline-joined, no trailing newline).
    pub text: String,
    /// 1-based line number where the block begins in the source.
    pub start_line: usize,
    /// SHA-256 (hex) of the normalized block content.
    pub fingerprint: String,
}

/// Whether a line (already left-trimmed) opens/closes a fenced code block, and
/// with which marker character.
fn fence_marker(trimmed: &str) -> Option<char> {
    if trimmed.starts_with("```") {
        Some('`')
    } else if trimmed.starts_with("~~~") {
        Some('~')
    } else {
        None
    }
}

/// Whether a left-trimmed line is an ATX heading: 1–6 leading `#` followed by a
/// space or the end of the line (`#tag` with no space is not a heading).
fn is_atx_heading(trimmed: &str) -> bool {
    let hashes = trimmed.chars().take_while(|&c| c == '#').count();
    if !(1..=6).contains(&hashes) {
        return false;
    }
    match trimmed[hashes..].chars().next() {
        None => true,
        Some(c) => c == ' ' || c == '\t',
    }
}

/// Segment `text` into blocks per the design's rules. Deterministic and total:
/// every non-blank line lands in exactly one block; blank lines are separators.
pub fn blockize(text: &str) -> Vec<Block> {
    let lines: Vec<&str> = text.lines().collect();
    let mut blocks = Vec::new();
    let mut para: Vec<&str> = Vec::new();
    let mut para_start = 0usize;

    // Flush any accumulated paragraph lines into a block.
    macro_rules! flush_para {
        () => {
            if !para.is_empty() {
                push_block(&mut blocks, &para, para_start);
                para.clear();
            }
        };
    }

    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_start();

        if let Some(marker) = fence_marker(trimmed) {
            flush_para!();
            let start = i + 1;
            let mut fence_lines = vec![line];
            i += 1;
            // Consume until the matching closing fence (or EOF for an unterminated
            // block — still one block, so comparison stays total).
            while i < lines.len() {
                let l = lines[i];
                fence_lines.push(l);
                i += 1;
                if fence_marker(l.trim_start()) == Some(marker) {
                    break;
                }
            }
            push_block(&mut blocks, &fence_lines, start);
            continue;
        }

        if is_atx_heading(trimmed) {
            flush_para!();
            push_block(&mut blocks, &[line], i + 1);
            i += 1;
            continue;
        }

        if line.trim().is_empty() {
            flush_para!();
            i += 1;
            continue;
        }

        if para.is_empty() {
            para_start = i + 1;
        }
        para.push(line);
        i += 1;
    }
    flush_para!();
    blocks
}

fn push_block(blocks: &mut Vec<Block>, lines: &[&str], start_line: usize) {
    let text = lines.join("\n");
    let fingerprint = fingerprint_of(&normalize(lines));
    blocks.push(Block {
        text,
        start_line,
        fingerprint,
    });
}

/// Normalize a block's lines for fingerprinting: right-trim each line, collapse
/// runs of blank lines to one, and drop leading/trailing blank lines.
fn normalize(lines: &[&str]) -> String {
    let trimmed: Vec<&str> = lines.iter().map(|l| l.trim_end()).collect();
    let mut out: Vec<&str> = Vec::with_capacity(trimmed.len());
    let mut prev_blank = false;
    for &l in &trimmed {
        let blank = l.is_empty();
        if blank && prev_blank {
            continue;
        }
        out.push(l);
        prev_blank = blank;
    }
    while out.first().is_some_and(|l| l.is_empty()) {
        out.remove(0);
    }
    while out.last().is_some_and(|l| l.is_empty()) {
        out.pop();
    }
    out.join("\n")
}

fn fingerprint_of(normalized: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(normalized.as_bytes());
    hex::encode(hasher.finalize())
}

/// The distinct block fingerprints of `text`, for set-overlap comparison.
pub fn fingerprint_set(text: &str) -> std::collections::HashSet<String> {
    blockize(text).into_iter().map(|b| b.fingerprint).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_headings_paragraphs_and_fences() {
        let text = "\
# Title

first paragraph
still first

## Section
para two

```rust
let x = 1;

let y = 2;
```
tail line
";
        let blocks = blockize(text);
        let texts: Vec<&str> = blocks.iter().map(|b| b.text.as_str()).collect();
        assert_eq!(
            texts,
            vec![
                "# Title",
                "first paragraph\nstill first",
                "## Section",
                "para two",
                "```rust\nlet x = 1;\n\nlet y = 2;\n```",
                "tail line",
            ]
        );
        // Heading blocks start on their own line numbers.
        assert_eq!(blocks[0].start_line, 1);
        assert_eq!(blocks[2].start_line, 6);
    }

    #[test]
    fn hash_prefix_is_not_a_heading() {
        // `#tag` (no space) is ordinary text, not an ATX heading.
        let blocks = blockize("#tag line\nmore\n");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].text, "#tag line\nmore");
    }

    #[test]
    fn normalization_ignores_trailing_whitespace() {
        // Two paragraphs differing only in trailing whitespace fingerprint equal.
        let a = &blockize("hello world   \nsecond line\t\n")[0];
        let b = &blockize("hello world\nsecond line\n")[0];
        assert_eq!(a.fingerprint, b.fingerprint);
    }

    #[test]
    fn distinct_content_fingerprints_differ() {
        let a = &blockize("alpha\n")[0];
        let b = &blockize("beta\n")[0];
        assert_ne!(a.fingerprint, b.fingerprint);
    }

    #[test]
    fn identical_blocks_across_files_share_fingerprints() {
        let canonical = "# Shared\n\nbody paragraph\n\n## Unique to canonical\n";
        let other = "intro\n\nbody paragraph\n\n# Shared\n";
        let cset = fingerprint_set(canonical);
        let oset = fingerprint_set(other);
        // "# Shared" and "body paragraph" appear in both.
        let overlap = cset.intersection(&oset).count();
        assert_eq!(overlap, 2);
    }

    #[test]
    fn unterminated_fence_is_a_single_block() {
        let blocks = blockize("```\nno closing fence\nstill inside\n");
        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].text.starts_with("```"));
    }

    #[test]
    fn heading_without_text_is_a_block() {
        let blocks = blockize("###\nbody\n");
        assert_eq!(blocks[0].text, "###");
        assert_eq!(blocks[1].text, "body");
    }
}
