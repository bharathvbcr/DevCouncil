pub mod admission;
mod binary_identity;
pub mod daemon;
pub mod mcp;
pub mod mcp_http;
pub mod protocol;
pub mod repo_scope;
pub mod root_resolve;
pub mod session_log;
pub mod watcher;

pub use admission::{Admission, Admitted};
pub use daemon::{default_ipc_path_for, Daemon, DEFAULT_DRAIN_BATCH_LIMIT};
pub use mcp::{serve_stdio, tool_specs, StoreSlot};
pub use mcp_http::serve_http;
pub use protocol::{
    coverage_gaps_json, freshness_degraded_reason, handle_stream, index_is_fresh, IpcCommand,
    IpcRequest, PROTOCOL_VERSION,
};
pub use root_resolve::{rebuild_required_reason, RootResolveInput};
pub use watcher::start_file_watcher;

/// Lock `mutex`, recovering it if an earlier holder panicked.
///
/// This is the one poisoning policy for every `std::sync::Mutex` in this
/// crate. The daemon and MCP transports are built with `panic = "unwind"` so a
/// worker panic is caught at its `JoinError` and the process keeps serving.
/// Poisoning would undo that: one panic while a shared lock was held, and every
/// later request that touched the lock would panic too (an `expect`) or be
/// refused for the rest of the process's life (an `Err` turned into an error
/// reply). Neither is better than the data under the lock.
///
/// Recovery is sound here because no guarded value has a cross-field invariant
/// a panic could leave half-kept. The scalars and `Option` records (activity
/// time, unapplied edits, roots, counters) change by one assignment, so a
/// panic leaves the previous value. The two collections — the store LRU and
/// the in-flight table — can at worst lose one entry, which costs a store
/// reopen or a cancellation that finds nothing to cancel, both already
/// handled paths.
pub(crate) fn lock_recover<T: ?Sized>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
