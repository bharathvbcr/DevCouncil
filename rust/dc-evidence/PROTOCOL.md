# Acceptance evidence protocol, version 1

`dc-evidence` evaluates immutable evidence against a separately admitted contract.
It does not control a desktop, call a model, execute commands, read a database or
access files. The existing `dcverify` executable exposes its filesystem adapter
as `evidence-check`. Existing `check` and `health.schema_version: 1` remain intact;
health additionally advertises `evidence_schema_versions: [1]`.

## Authority and results

The host pins the exact contract bytes, capability snapshot hash, run ID,
session ID and positive worker epoch **before** execution. Those admission values
must arrive independently of the bundle. The worker/coordinator owns observation
facts; model responses and application text cannot supply a verifier verdict or
replace the admitted contract. The evaluator computes predicates itself.

Hashes prove byte identity, not observation authenticity. A malicious producer
can forge both bytes and their hashes. This protocol assumes the observation
worker and admission coordinator are trusted. Offline replay verifies consistency
of recorded evidence; it is not proof of a fresh native run or protection against
a compromised host. Artifact integrity is checked separately from fact predicates:
the caller must derive facts from its trusted capture/output adapters. A valid
file hash alone does not prove that an action happened or a requested task worked.

An evaluated result always has `ok: true` and a separate verdict:

- `passed`: all required criteria passed and all evidence prerequisites held.
- `failed`: a predicate failed, an action failed, an identity binding disagreed,
  or declared artifact bytes failed integrity validation.
- `incomplete`: required facts or artifacts are unavailable; a journal is
  incomplete/degraded; a disposition is unknown, cancelled or denied; or an
  action lacks a subsequent observation. A known failure dominates incompleteness.

Malformed/oversized/unsupported protocol inputs return `ok: false` and an error,
with process exit 2. Evaluated `failed` and `incomplete` return exit 0: exit status
reports whether evaluation ran, not acceptance. Consumers must inspect `verdict`.
Diagnostics contain identifiers/codes rather than the actual fact values.

## Canonical wire

The complete byte-exact example is in `fixtures/v1/`: `contract.json`,
`bundle.json`, `expected.json`, `capabilities.json` and `note.txt`. Public Rust
types live in `src/types.rs`; `expected.json` represents independent admission
data, not values a consumer should discover from `bundle.json`.

All listed fields are required except `epoch_transitions`, `policy_sha256`,
`outcome`, `interventions`, `human_actions`, `recoveries`, and `locator_hits`.
Absent optional lists default to empty; absent `outcome` / `policy_sha256` mean
those assertions cannot be discharged. Unknown fields, duplicate object keys (including
inside fact values), unknown enum values, and unsupported versions are refused.
Identifiers are 1–256 bytes without control characters. Hashes are 64 lowercase
hex characters identifying SHA-256 of exact bytes. Reformatting JSON changes its
identity; Go and Rust must not hash their independently reserialized objects.

```json
{
  "schema_version": 1,
  "id": "save-note",
  "criteria": [{
    "id": "note-content",
    "fact": "document.text",
    "required": true,
    "predicate": {"op": "equals", "value": "Hello Jarvis"}
  }]
}
```

`fact` names an exact top-level key in the final observation's `facts` object,
or one of the reserved bundle facts below; it is not a JSON pointer or executable
expression. At least one criterion must be required. Optional criteria are still
reported but do not block acceptance.

Reserved bundle facts (resolved from the evidence record, never from observation
text — observation keys with the same name cannot override them):

| Fact | Meaning |
|---|---|
| `outcome.id` | Bundle `outcome.id` when present; missing outcome is incomplete |
| `outcome.kind` | Bundle `outcome.kind` (`success` or `business`) |
| `human_control_returned` | Always present boolean: true iff any `interventions[].status` is `returned` |

Contracts that expect a business outcome such as member-not-found assert
`outcome.id` equals `not_found` (and typically `outcome.kind` equals `business`).
That expected-not-found case **passes**. A contract that expected a success
outcome (`outcome.id` equals a success id, or `outcome.kind` equals `success`)
against a bundle that concluded `not_found` **fails** — business terminals are
not hard failures in the journal, but they fail a success contract.

| Predicate | Shape and meaning |
|---|---|
| `exists` | `{"op":"exists"}`; fact is present and non-null; false and zero exist |
| `equals` | `{"op":"equals","value":...}`; structural typed JSON equality, no string coercion |
| `not_equals` | Same shape; inverse of structural equality, but a missing fact is incomplete |
| `contains` | Expected string is a substring of actual string; expected array elements all occur in actual array using structural equality |
| `minimum` | Expected and actual are finite numbers with absolute value at most 2^53−1; compare using f64 |

JSON equality distinguishes integer and floating representations (`1` and `1.0`)
as serde_json does; use consistent values or `minimum` for numeric thresholds.
`contains` with incompatible observation types and `minimum` with nonnumeric or
out-of-range observations are incomplete. Criteria are checks of the recorded
state, not proof that the chosen criterion is itself sufficient for user intent.

The bundle carries `schema_version`, `run_id`, `session_id`, `epoch`,
`contract_sha256`, `capability_sha256`, optional `policy_sha256`, optional
`outcome`, `journal_complete`, `degraded`, `actions`, `observations`,
`epoch_transitions`, `artifacts`, `interventions`, `human_actions`,
`recoveries`, and `locator_hits`:

```json
{
  "outcome": {"id":"saved","kind":"success"},
  "policy_sha256": "…64 lowercase hex…",
  "actions": [{"id":"action-1","sequence":1,"disposition":"succeeded"}],
  "observations": [{
    "sequence":2,"run_id":"run-1","session_id":"desktop-1","epoch":1,
    "after_action_id":"action-1",
    "facts":{"document.text":"Hello Jarvis"},"artifact_ids":["note"]
  }],
  "artifacts": [{"id":"note","path":"note.txt","sha256":"...","size_bytes":12}],
  "interventions": [],
  "human_actions": [],
  "recoveries": [],
  "locator_hits": []
}
```

Field names are stable snake_case for Manvi/Jarvis alignment (Manvi workflow
schema may still be landing; keep these exact keys):

| Field | Shape |
|---|---|
| `outcome` | `{id, kind}` where `kind` is `success` or `business` |
| `policy_sha256` | 64 lowercase hex SHA-256 of the reviewed policy document |
| `interventions[]` | `{id, sequence, reason_code, status}` with `status` in `requested`/`returned`/`abandoned` |
| `human_actions[]` | `{id, sequence, kind, intervention_id}` referencing an intervention |
| `recoveries[]` | `{id, sequence, step_id}` for each applied declared recovery |
| `locator_hits[]` | `{target, strategy_index, sequence}`; index `> 0` is drift |

Side records use their own strictly increasing positive `sequence` spaces and do
not share the action/observation/epoch-transition sequence namespace. At most 256
entries each. `human_actions[].intervention_id` must name a declared intervention.

The shortened fragment above illustrates member shapes; the complete fixture
contains all required bindings and actual digests. Each action/observation list
is strictly increasing by positive sequence, and sequences are unique across
actions, observations and epoch transitions. Gaps are allowed because other coordinator events may exist outside
this projection. `journal_complete` is the trusted coordinator's coverage claim.
Action IDs are unique. Every action must have a later observation of the same
run/session and the epoch active at its sequence, identifying the most recent action at that sequence. Empty
actions or observations cannot pass. An observation before the final action
cannot establish final state; only the last observation's facts discharge criteria.

The bundle's `epoch` always identifies the initial independently pinned admission
epoch. A trusted coordinator may append an acknowledged worker transition:

```json
{"epoch_transitions":[
  {"sequence":3,"from_epoch":1,"to_epoch":2,"reason":"pause"},
  {"sequence":4,"from_epoch":2,"to_epoch":3,"reason":"resume"}
]}
```

Transitions must increase in sequence and form a continuous chain starting at
the admission epoch: `from_epoch` equals the active epoch and `to_epoch` is
strictly greater. `reason` follows identifier bounds. An observation uses the
epoch active immediately before its sequence; old observations retain their
original epoch. An observation must follow the final transition before any final
criterion can pass. A transition does not resolve an unknown action disposition.
Only a coordinator acknowledgment of an actual worker epoch change may author
this record; model/application assertions are insufficient. Transition hashes do
not independently authenticate this coordinator claim. `bundle-resumed.json` is
a complete compatible fixture using the original admission and artifact bytes.

Dispositions are `succeeded`, `not_dispatched`, `failed`, `unknown`, `cancelled`, `denied`.
`not_dispatched` records a trusted native receipt proving no input was sent.
It does not require post-input evidence and cannot itself support an outcome
observation. A subsequent successful action still needs fresh evidence. A final
undispatched attempt cannot reuse an older observation to pass. Every attempt
retains a distinct identity; uncertain delivery must remain `unknown`.
Every unknown/cancelled/denied recorded attempt leaves this bundle incomplete,
even if a later attempt succeeds. Recovered actions should be recorded with a
known final disposition only after independent reconciliation, never by guessing
that a timed-out request did not execute. Repeated inputs produce identical reports;
replay must not request new model outputs or control the desktop.

Artifact IDs and paths are unique, and observation references must name declared
artifacts exactly once. Paths are portable relative slash-separated paths, with
no parent/dot/empty components, backslashes, drive/URI syntax, control characters,
Windows device names or trailing spaces/dots. Every declared artifact is checked,
including artifacts not referenced by the last observation.

## Limits and file adapter

| Input | Hard maximum |
|---|---:|
| Contract JSON | 1 MiB |
| Bundle JSON | 4 MiB |
| Criteria; facts per observation; degraded diagnostics | 256 each |
| Actions, observations and epoch transitions combined | 4,096 |
| Artifact references | 128 |
| Individual artifact | 16 MiB |
| All artifact bytes | 64 MiB |
| JSON nesting | serde_json's bounded parser depth |

The CLI checks regular files and lengths before bounded reads, refuses symlink
components and nonregular objects (including FIFOs), and compares opened file
identity on Unix. Use canonical paths when the OS temporary directory contains
symlink aliases. There are no network fetches or archive extraction.

This portable standard-library adapter assumes the admitted bundle directory is
not being maliciously renamed/replaced during evaluation. It is not a
descriptor-relative filesystem sandbox: on non-Unix systems the opening check
compares size/type/mtime, and no platform claims protection against every
concurrent ancestor substitution. The supervising host must apply its process
deadline. The pure library accepts already-read bytes, so a stronger host file
resolver can supply them without duplicating acceptance rules.

## Invocation and compatibility

From `rust/`, after `cargo build -p dc-verify --bin dcverify`:

```sh
target/debug/dcverify evidence-check \
  --contract dc-evidence/fixtures/v1/contract.json \
  --bundle dc-evidence/fixtures/v1/bundle.json \
  --artifacts-root dc-evidence/fixtures/v1 \
  --expected-contract-sha256 bdf5f2cf064b49e340ba043d78511e8e10bdf6ee4276daa0b03fc5fc8922e816 \
  --expected-capability-sha256 1a4bf7dac3b78318718a5047937473da4898acfb53cbbc9afdbe0c7fa9f3eec2 \
  --expected-run-id run-1 --expected-session-id desktop-1 --expected-epoch 1
```

Expected output includes `verdict: "passed"`, `contract_sha256`, `bundle_sha256`,
`run_id`, `criteria: [{id, required, verdict, reason}]` and
`issues: [{code, verdict}]`. `issues` is empty for this fixture.

New Go clients check the evidence capability rather than raising the legacy diff
schema requirement. Unknown versions never get coerced into version 1. Bundles
are immutable exports, independent of Manvi's storage schema. A future migration
must preserve original bytes/digest and create a derived bundle explicitly; old
missing provenance never receives defaults that imply successful verification.

## Verification

`cargo test -p dc-evidence -p dc-verify` exercises the pure evaluator and real CLI.
Tests cover input binding, wrong runs/sessions/epochs, empty/missing/stale facts,
unknown/failed actions, final-state reversal, duplicate/out-of-order events,
artifact absence/corruption/path refusal, schema/default/depth/byte bounds,
structural JSON predicates and repeatable reports. Unix CLI tests use actual
symlinks and a FIFO. These fixture tests are not native desktop or statistical
task-reliability evidence; downstream Go producer/consumer and platform runs
remain separate gates.
