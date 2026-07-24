//! Window-level glass status (#37).
//!
//! The glass effect itself is applied once during setup (see `lib.rs`); this
//! module resolves which tier actually took effect and reports it to the
//! frontend, which keys `data-window-glass` on `<html>` off it so the CSS
//! wallpaper can yield to the native material. Tiers:
//!
//! - `liquid-glass`: macOS 26+ NSGlassEffectView applied
//! - `vibrancy`: older macOS, NSVisualEffectView fallback applied
//! - `none`: apply failed or non-macOS — frontend keeps the opaque CSS
//!   wallpaper, so the transparent window never shows through bare.

use std::sync::OnceLock;

static WINDOW_GLASS_TIER: OnceLock<&'static str> = OnceLock::new();

/// Resolve the tier label from what happened at apply time. Pure so the
/// degradation chain is unit-testable without AppKit.
pub(crate) fn resolve_tier(macos: bool, glass_supported: bool, apply_ok: bool) -> &'static str {
    if !macos || !apply_ok {
        "none"
    } else if glass_supported {
        "liquid-glass"
    } else {
        "vibrancy"
    }
}

/// Record the tier resolved during setup. Later calls (there are none today)
/// keep the first value, which is the honest one: the effect is only applied
/// once.
pub(crate) fn record_tier(tier: &'static str) {
    let _ = WINDOW_GLASS_TIER.set(tier);
}

#[tauri::command]
pub fn window_glass_status() -> &'static str {
    WINDOW_GLASS_TIER.get().copied().unwrap_or("none")
}

#[cfg(test)]
mod tests {
    use super::resolve_tier;

    #[test]
    fn macos26_with_successful_apply_is_liquid_glass() {
        assert_eq!(resolve_tier(true, true, true), "liquid-glass");
    }

    #[test]
    fn older_macos_with_successful_apply_is_vibrancy() {
        assert_eq!(resolve_tier(true, false, true), "vibrancy");
    }

    #[test]
    fn failed_apply_degrades_to_none_regardless_of_support() {
        assert_eq!(resolve_tier(true, true, false), "none");
        assert_eq!(resolve_tier(true, false, false), "none");
    }

    #[test]
    fn non_macos_is_none_even_if_apply_reports_ok() {
        // The plugin no-ops (Ok) off macOS; that must not claim a glass tier.
        assert_eq!(resolve_tier(false, true, true), "none");
        assert_eq!(resolve_tier(false, false, true), "none");
    }
}
