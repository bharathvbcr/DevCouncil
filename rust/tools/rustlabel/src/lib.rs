//! rustlabel: the Rust counterpart of `tools/accuracy/cmd/golabel`.
//!
//! `golabel` asks `go/types` what every call in a Go checkout binds to;
//! rustlabel asks rust-analyzer (`textDocument/definition`, and
//! `textDocument/implementation` for a trait method) and writes the Rust half
//! of the same labels file, so the corpus runner
//! (`devmap-analyze/tests/real_corpus_accuracy.rs`) can score DevMap's Rust
//! recall and precision against an oracle that is not DevMap.
//!
//! It is a measuring instrument and ships in no released binary: a workspace
//! member with `publish = false` and `dist = false`, like `tools/soak`, linking
//! `devmap-resolve` for the one language-server client the workspace has.
//!
//! What an answer is worth: a site rust-analyzer cannot answer is recorded as
//! unknown and is never a label, and a caller with any such site is not in the
//! oracle's `covered_callers`, so DevMap's edges from it are left unjudged
//! rather than judged wrong. See [`run`] for the buckets and
//! `rust/testdata/accuracy/README.md` ("Rust labels") for the limits.

pub mod classify;
pub mod cli;
pub mod labels;
pub mod namer;
pub mod run;
pub mod server;
pub mod sites;
