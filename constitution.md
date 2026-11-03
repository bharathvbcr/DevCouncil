# DevCouncil Project Constitution

## Nine Articles of Architectural Integrity

### Article I: Library-First Architecture
All domain capabilities must be implemented as standalone, embeddable libraries/crates before being exposed via CLI, daemon, or UI wrappers. Internal modules must never couple directly to IPC or HTTP transports.

### Article II: CLI as First-Class Consumer
Every capability offered by internal crates/packages must have an equivalent, scriptable, stdout/stderr-separated CLI command supporting both human-readable text and deterministic `--json` envelopes.

### Article III: Test-First Development (Red -> Green -> Refactor)
Implementation files must never be created before contract, integration, or unit test files exist. Tests must fail against unmodified code before code changes are accepted.

### Article IV: Zero Ambient Authority & Fail-Closed Gates
All gates, verifiers, and subprocess executions must fail closed. If a verification tool, binary, or dependency is missing, unconfigured, or timed out, the gate refuses the operation. A skipped gate is a failed gate.

### Article V: Single Source of Truth & Zero Duplication
Never create parallel or redundant implementations. Code intelligence belongs in DevMap; lease state in `dc-store`; worktrees in GitPulse; verification in `dc-verify`. Extend existing modules at the canonical seam.

### Article VI: Strict Memory & Allocator Symmetry
In cross-language boundaries (Go <-> Rust), memory must be released only by the allocator that allocated it. All in-process boundaries must run behind Gusset zero-panic firewalls and bounded worker stacks.

### Article VII: Simplicity Limit (<= 3 Modules)
No single specification or feature implementation may introduce more than three new modules, packages, or external dependencies without explicit written constitutional justification in `plan.md`.

### Article VIII: Anti-Abstraction / Rule of Three
Do not build speculative abstractions or generic wrappers for single callers. Three distinct production use cases are required before an abstraction layer is permitted.

### Article IX: Observability & Forensic Auditability
All state transitions, lease acquisitions, gate decisions, and verification findings must be recorded into the append-only ledger (`.devcouncil/ledger.sqlite`) with cryptographic hashes and process lineage.
