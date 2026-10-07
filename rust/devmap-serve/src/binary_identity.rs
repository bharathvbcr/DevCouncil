//! Whether the executable backing this process has been replaced on disk.
//!
//! One owner for both long-lived servers. The `serve` daemon retires when the
//! answer changes; the MCP server refuses tool calls. Neither may grow its own
//! notion of "the binary changed", because two would disagree about a rebuild
//! that kept the size, or about a probe that could not run.

/// Identity of an executable: `(size, mtime)`.
pub(crate) type ExecutableIdentity = Option<(u64, std::time::SystemTime)>;

/// Identity of the executable backing this process: `(size, mtime)`.
///
/// `None` when it cannot be determined — a deleted or unreadable `/proc` entry,
/// a platform without `current_exe`. `None` compares equal to `None`, so an
/// undeterminable identity never *causes* a retirement; the server then behaves
/// exactly as it did before this check existed. Failing the other way would let
/// an unreadable executable path restart the daemon on every tick.
pub(crate) fn executable_identity() -> ExecutableIdentity {
    let path = std::env::current_exe().ok()?;
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.len(), meta.modified().ok()?))
}

/// Whether a changed executable identity means this process is now stale.
///
/// Split from its callers so the policy can be asserted directly. The effect —
/// a daemon "exits at some point in the next two seconds" — is not something a
/// test can observe without racing, and the branch guards a silent failure
/// (stale answers from a rebuilt kernel), which is exactly the kind that
/// survives an untested predicate.
pub(crate) fn should_retire_for_new_binary(
    started_as: ExecutableIdentity,
    current: ExecutableIdentity,
) -> bool {
    started_as != current
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A rebuilt binary retires the daemon; an unchanged one does not.
    ///
    /// The hazard is specific: `PROTOCOL_VERSION` does not move when the kernel
    /// is rebuilt, so without this check a daemon started before a `cargo
    /// build` keeps serving the old code for its whole idle bound — 30 minutes
    /// by default — and every client reads pre-fix answers from a fixed tree.
    #[test]
    fn a_changed_executable_identity_retires_the_daemon() {
        let epoch = std::time::UNIX_EPOCH;
        let started = Some((1_000u64, epoch));

        assert!(
            !should_retire_for_new_binary(started, started),
            "an unchanged binary must not restart the daemon on every tick"
        );
        assert!(
            should_retire_for_new_binary(started, Some((2_000, epoch))),
            "a binary whose size changed is a rebuild"
        );
        assert!(
            should_retire_for_new_binary(
                started,
                Some((1_000, epoch + std::time::Duration::from_secs(1))),
            ),
            "a rebuild that happens to produce the same size still moves mtime"
        );
    }

    /// An undeterminable identity must not cause a retirement.
    ///
    /// `None` is "could not tell", not "changed". Treating it as a change would
    /// make the daemon exit on every tick on any platform or sandbox where
    /// `current_exe` fails — turning a diagnostic gap into a crash loop that
    /// respawns a process per client call.
    #[test]
    fn an_undeterminable_executable_identity_never_retires() {
        assert!(!should_retire_for_new_binary(None, None));
    }

    /// The real probe answers for this test binary, and answers consistently.
    ///
    /// Without this the two tests above would pass over a function that always
    /// returned `None` in practice, and the check would be dead.
    #[test]
    fn executable_identity_resolves_and_is_stable() {
        let first = executable_identity();
        assert!(
            first.is_some(),
            "current_exe/metadata should resolve for the test binary"
        );
        assert_eq!(
            first,
            executable_identity(),
            "identity must be stable for an unchanged binary"
        );
    }
}
