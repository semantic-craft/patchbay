//! Cross-platform fixture helpers shared by unit tests.
//!
//! Test modules that build symlink scenarios were gated `cfg(unix)` and so
//! vanished entirely on Windows — containment and escape assertions passed
//! vacuously there, which made a green Windows run mean less than it looked.
//! These helpers let those fixtures run on both platforms instead.

use std::io;
use std::path::Path;

/// Create a directory symlink, falling back to a junction on Windows.
///
/// A junction needs no `SeCreateSymbolicLinkPrivilege` on a local NTFS volume,
/// so a fixture works whether or not Developer Mode is enabled on the machine
/// running the suite. std reports a mount point as a symlink, so
/// `symlink_metadata`, `read_link` and `canonicalize` all observe it exactly as
/// these tests expect a symlink to behave.
///
/// A relative `target` is passed through as-is to the real symlink call, so a
/// fixture that means to test relative-target resolution still does. Only the
/// junction fallback needs it absolute (`junction::create` requires that), so
/// there it is resolved against the link's parent — the same directory a
/// relative symlink would resolve against.
pub(crate) fn symlink_dir(target: &Path, link: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link)
    }
    #[cfg(windows)]
    {
        match std::os::windows::fs::symlink_dir(target, link) {
            Ok(()) => Ok(()),
            // Surface the original error, which names the privilege problem;
            // the junction error would only say that the retry failed.
            Err(err) => {
                let absolute = if target.is_absolute() {
                    target.to_path_buf()
                } else {
                    link.parent().unwrap_or(Path::new(".")).join(target)
                };
                junction::create(&absolute, link).map_err(|_| err)
            }
        }
    }
}

/// Create a *file* symlink. Unlike [`symlink_dir`] there is no privilege-free
/// fallback: a junction is a directory-only construct, and a hard link is not a
/// symlink (`symlink_metadata().is_symlink()` is false for one), so substituting
/// either would quietly test something other than what the fixture describes.
///
/// The target need not exist — a dangling link is a legitimate fixture on both
/// platforms.
///
/// This means the file-symlink fixtures need `SeCreateSymbolicLinkPrivilege`
/// (Developer Mode) on Windows. That is the deliberate trade: these tests cover
/// the instructions subsystem's symlink *variant*, and running them for real on
/// the one Windows box in CI is worth more than a fallback that would make them
/// pass everywhere by testing something else. [`expect_symlink_file`] turns the
/// resulting failure into an actionable message rather than a bare permission
/// error.
pub(crate) fn symlink_file(target: &Path, link: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link)
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_file(target, link)
    }
}

/// `symlink_file(...).unwrap_or_else(panic_hint)`, so a machine without the
/// privilege gets told what to turn on instead of "A required privilege is not
/// held by the client. (os error 1314)".
pub(crate) fn expect_symlink_file(target: &Path, link: &Path) {
    if let Err(err) = symlink_file(target, link) {
        panic!(
            "could not create the file symlink {} -> {}: {err}\n\
             On Windows this fixture needs Developer Mode (Settings > System > \
             For developers) or an elevated shell.",
            link.display(),
            target.display(),
        );
    }
}
