# Evidence component validation

Validated locally on macOS, 2026-09-09, in the isolated `codex/jarvis-evidence`
worktree based on `fe357ac`. No native desktop or live provider claims follow
from these component tests.

- Before implementation, `cargo test -p dc-verify` completed successfully. Its
  Python CLI interoperability check reported the existing unavailable-interpreter
  skip path; the Rust diff/coverage/rigor tests executed.
- Added `evidence_capability_is_additive_to_legacy_health`; it failed against the
  unmodified executable, whose response was
  `{"ok":true,"verifier":"dc-verify","schema_version":1}`.
- After implementation, 24 pure evaluator tests and nine real executable tests
  pass, including actual Unix symlink/FIFO refusal, corrupted and unavailable
  artifacts, independent contract identity, and legacy diff compatibility.
- The acknowledged pause/resume regression failed before epoch-transition
  implementation with `unknown field epoch_transitions`, then passed. Additional
  tests reject stale epoch observations, old final state, sequence collisions,
  invalid epoch chains and oversized transition journals; unknown actions remain
  incomplete after recovery.
- `cargo test --workspace` completed successfully. The pre-existing Python
  interop tests may return through their documented skip paths without the
  checkout's `.venv`; this run does not claim Python interoperability.
- `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo fmt --all -- --check`, and `git diff --check` pass.
- The documented fixture CLI invocation returns `verdict: passed`, with exact
  bundle digest `ddd8322dc701e368a09bc551178ac4532a25eedf44caaa44215969d3568773f9`.

- The Go consumer and Rust CLI passed the actual shared fixture and resumed-run
  compatibility checks; a wrong-run fixture is refused. Genuine Jarvis macOS
  balance replay also produced an independently evaluated passing bundle.
- A refused-input regression failed before `not_dispatched` support, then passed.
  Proven-no-input attempts remain distinct from unknown/failed delivery and cannot
  supply acceptance evidence themselves. The component suites and all-target
  Clippy were rerun after this addition.

Remaining gates: Windows and Linux execution of this CLI; wider native capture
truth and account-changing application outcomes.
The portable CLI trusts its input directory against malicious concurrent path
substitution; see `PROTOCOL.md`. All new code is Rust; fixture inputs and protocol
documentation are JSON/text. Local source revisions are pinned by Jarvis;
publication is separate from local validation.
