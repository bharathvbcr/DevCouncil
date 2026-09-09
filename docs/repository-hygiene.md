# Portable repository hygiene policy

The Rust `devmap-query::hygiene` module is the canonical policy owner. It is
available with `default-features = false`, so a host does not need a parser,
daemon, Python runtime or a separate cleanup dependency to use it.

| Public surface | Contract |
| --- | --- |
| `AGENT_RULES` | Guidance on measurement, producer evidence, preservation, native cache retention, fresh checks, user-owned schedules and honest reporting. Guidance grants no permission to delete. |
| `protected_artifact` / `protected_entry` | Conservative refusal for known dependencies, environments, agent state, credentials and model/data formats. Recognized names are not an exhaustive data classifier. |
| `local_provider` | Bounded literal relative path and producer evidence for supported generated output. Rejects traversal and unsupported output. Eligibility alone does not authorize deletion. |
| `validate_retention` | Manual retention 1–3,650 days; scheduled retention 7–3,650 days. Hosts may tighten these limits. |
| `CACHE_ADVICE` | Prefer tool-owned cache lifecycle management; preserve downloads and environments. |

`guides::agent_guide_text` appends the canonical rules to generated workspace
agent guides. The existing `write_agent_guides` ownership rules still apply:
custom or mixed guides are preserved. Refresh managed guides through the
existing Rust CLI artifact flow with `--guides`; do not overwrite a custom
`AGENTS.md` merely to distribute these rules.

GitPulse embeds this module through its existing vendored `devmap-query` crate.
Its provider module delegates to this owner. The host retains settings, consent,
Git index/ignore checks, complete bounded snapshots, active task/process/open-file
checks, policy gates, cancellation, journaling and filesystem mutation. The host
also owns scheduling and platform integration. This split lets another app reuse
the policy without importing Tauri or launching a cleanup worker.

Updating a compiled-in consumer requires re-vendoring and rebuilding that
consumer. Updating the DevCouncil CLI alone does not change an installed
GitPulse application. There is no independently running DevCouncil cleaner and
these changes do not install or enable schedules.

Validation: run `cargo test --manifest-path rust-port/Cargo.toml -p devmap-query
--no-default-features` and the same package's Clippy check. The regression cases
cover path traversal, mixed path separators, case-insensitive preservation,
retention bounds and canonical guide inclusion; existing guide tests exercise
custom-file preservation. Host tests must independently prove execution safety.
