//! Byte/character-based token estimation for instructions surfaces.
//!
//! The governance model measures cost in bytes/characters (the executable
//! thresholds — Codex's 32 KiB, Antigravity's 12k characters — are themselves
//! byte/character quantities). A true per-vendor tokenizer would need a heavy
//! dependency or a network call for accuracy the display doesn't warrant, so
//! tokens are only ever an *estimate*, always shown with a leading `~`.
//!
//! Formula: `~tokens = ⌈non-CJK chars / 3.5 + CJK chars × 1.0⌉`. CJK characters
//! carry far more information per glyph than Latin text, so they are weighted
//! separately. ±30% error is acceptable for the cost display.
//!
//! Re-calibrated from design §2's original `/4 + ×0.7` (which systematically
//! under-estimated — a cost warning should not read low). The current constants
//! keep every sampled file (English, CJK-heavy, and CJK-mixed) within ±30% of
//! *both* the legacy `cl100k_base` and the modern `o200k_base` tokenizers, so
//! the display is trustworthy regardless of which agent's tokenizer applies.
//! Pending a §2 ratification of these constants (#12 amendment).

/// Whether `ch` counts as a CJK character for the estimate. Covers the ranges
/// that dominate Chinese/Japanese/Korean instructions text — unified ideographs
/// and their common extensions, kana, Hangul, and fullwidth/CJK punctuation.
/// The estimate tolerates ±30%, so the boundary need not be exhaustive.
fn is_cjk(ch: char) -> bool {
    matches!(ch as u32,
        0x3000..=0x303F   // CJK symbols and punctuation
        | 0x3040..=0x309F // Hiragana
        | 0x30A0..=0x30FF // Katakana
        | 0x3400..=0x4DBF // CJK Unified Ideographs Extension A
        | 0x4E00..=0x9FFF // CJK Unified Ideographs
        | 0xAC00..=0xD7AF // Hangul syllables
        | 0xF900..=0xFAFF // CJK compatibility ideographs
        | 0xFF00..=0xFFEF // Halfwidth and fullwidth forms
        | 0x20000..=0x2A6DF // CJK Unified Ideographs Extension B
        | 0x2A700..=0x2EBEF // Extensions C–F
    )
}

/// Non-CJK characters per estimated token (≈3.5 for technical/markdown text).
const NON_CJK_CHARS_PER_TOKEN: f64 = 3.5;
/// Estimated tokens per CJK character (≈1 for modern tokenizers).
const CJK_TOKENS_PER_CHAR: f64 = 1.0;

/// Estimated token count for `text`. Counts every character, splitting CJK from
/// the rest; whitespace and punctuation fall in the non-CJK bucket. The result
/// is rounded up so a non-empty file never estimates to zero.
pub fn est_tokens(text: &str) -> u64 {
    let mut cjk: u64 = 0;
    let mut other: u64 = 0;
    for ch in text.chars() {
        if is_cjk(ch) {
            cjk += 1;
        } else {
            other += 1;
        }
    }
    ((other as f64) / NON_CJK_CHARS_PER_TOKEN + (cjk as f64) * CJK_TOKENS_PER_CHAR).ceil() as u64
}

/// Estimated tokens for a UTF-8 file's bytes; non-UTF-8 or unreadable files
/// estimate to zero (their bytes are still reported separately by the scanner).
pub fn est_tokens_bytes(bytes: &[u8]) -> u64 {
    match std::str::from_utf8(bytes) {
        Ok(text) => est_tokens(text),
        Err(_) => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_text_is_zero() {
        assert_eq!(est_tokens(""), 0);
    }

    #[test]
    fn ascii_uses_non_cjk_char_rate() {
        // 7 ASCII chars → ceil(7/3.5) = 2.
        assert_eq!(est_tokens("abcdefg"), 2);
        // 8 ASCII chars → ceil(8/3.5) = ceil(2.29) = 3 (rounds up).
        assert_eq!(est_tokens("abcdefgh"), 3);
    }

    #[test]
    fn cjk_uses_one_token_per_char() {
        // 10 Han characters → ceil(10 * 1.0) = 10.
        let han = "一二三四五六七八九十";
        assert_eq!(han.chars().count(), 10);
        assert_eq!(est_tokens(han), 10);
    }

    #[test]
    fn mixed_text_sums_both_buckets() {
        // "你好abcd": 2 CJK + 4 non-CJK → ceil(4/3.5 + 2*1.0) = ceil(3.14) = 4.
        assert_eq!(est_tokens("你好abcd"), 4);
    }

    #[test]
    fn kana_and_hangul_count_as_cjk() {
        assert!(is_cjk('あ'));
        assert!(is_cjk('カ'));
        assert!(is_cjk('한'));
        assert!(!is_cjk('a'));
        assert!(!is_cjk(' '));
    }

    #[test]
    fn non_utf8_bytes_estimate_zero() {
        assert_eq!(est_tokens_bytes(&[0xff, 0xfe, 0x00]), 0);
        assert_eq!(est_tokens_bytes(b"abcdefgh"), 3);
    }
}
