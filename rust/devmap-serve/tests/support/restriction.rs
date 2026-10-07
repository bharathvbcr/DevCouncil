//! Whether a permission fixture actually took effect for this process.
//!
//! Shared by the suites that build an unreadable or unwritable fixture, so the
//! decision about when such a fixture is inert has one owner.

#![cfg(unix)]

/// Whether the mode bits just set actually refuse this process.
///
/// Root, and filesystems that ignore mode bits, read through `0o000` and write
/// through read-only, and then the condition this test builds cannot exist.
/// The test says so on stderr and stops rather than failing on its
/// environment; wherever the restriction holds, every assertion runs.
pub fn restriction_holds(path: impl AsRef<std::path::Path>) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let path = path.as_ref();
    let Ok(meta) = std::fs::metadata(path) else {
        return true;
    };
    let mode = meta.permissions().mode() & 0o777;
    let refused = if mode & 0o444 == 0 {
        if meta.is_dir() {
            std::fs::read_dir(path).is_err()
        } else {
            std::fs::File::open(path).is_err()
        }
    } else if meta.is_dir() {
        let probe = path.join(".devmap-permission-probe");
        match std::fs::File::create(&probe) {
            Ok(_) => {
                let _ = std::fs::remove_file(&probe);
                false
            }
            Err(_) => true,
        }
    } else {
        std::fs::OpenOptions::new().append(true).open(path).is_err()
    };
    if !refused {
        eprintln!(
            "skipped: mode {mode:o} on {} does not refuse this process (running as root?); \
             the condition this test needs cannot be built here",
            path.display()
        );
    }
    refused
}
