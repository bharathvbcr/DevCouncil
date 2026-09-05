pub mod daemon;
pub mod mcp;
pub mod mcp_http;
pub mod protocol;
pub mod watcher;

pub use daemon::{default_ipc_path_for, Daemon, DEFAULT_DRAIN_BATCH_LIMIT};
pub use mcp::{serve_stdio, tool_specs, StoreSlot};
pub use mcp_http::serve_http;
pub use protocol::{
    freshness_degraded_reason, handle_stream, index_is_fresh, IpcCommand, IpcRequest,
    PROTOCOL_VERSION,
};
pub use watcher::start_file_watcher;
