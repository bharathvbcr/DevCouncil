# DevCouncil hook migration audit — 2026-09-12

**Verified outcome:** the installed `dev` and `devcouncil` aliases accept the reported
`hook user-prompt-submit --client claude --project-root /Users/bharath/Code/scholarlm`
command with exit 0, empty stdout and empty stderr. ScholarLM's eight known host
configuration paths contain no recognized legacy registrations. User-level Claude
settings were also checked separately and are clean.

The nine ScholarLM Claude registrations observed at the start were removed by a
concurrent writer. This audit independently verified that state; it did not claim
that writer's cleanup as its own. The final installed binary was built and tested
from this isolated worktree, not from the concurrently changing checkout.

## Scope and ownership

- Baseline: DevCouncil `ee07c18`; canonical checkout `/Users/bharath/Code/devtools/DevCouncil`.
- Final source: branch `codex/hook-compat-hardening`, worktree `/Users/bharath/.codex/worktrees/devcouncil-hook-hardening-20260912`.
- Changes remain uncommitted and unmerged. Another writer changed the same Go hook
  files during this audit; Rust loader changes in the shared checkout are unrelated.
- The original overlapping Go edits were preserved under the canonical checkout's
  `.git/codex-backups/hook-overlap-20260912/` before isolation.
- The cleanup inventory has eight paths across Claude, Cursor, Codex, Gemini,
  Grok and OpenCode. Global settings require an explicit home-directory root.
  Git map-refresh hooks, other plugins' hooks and plugin caches are outside this scope.

## Reproduced issues and fixes

Every behavioral issue below was reproduced before its fix. The existing baseline
suite passed before new regressions were added, demonstrating its coverage gaps.

| Verified issue | Resulting behavior / evidence |
|---|---|
| The native host rejected `hook` with exit 2 | `cmd/devcouncil/hook.go:12` provides an inert event compatibility path, including unknown events/flags. No stdin reads, project access, output decisions or subprocesses. |
| An overlapping proposed fix forwarded retired events into DevMap | `TestRetiredHookNeverStartsDependencies` failed against that proposal. Separate DevMap hooks retain ownership of index maintenance. |
| `disable hooks --dry-run` actually cleaned settings | All cleanup aliases now share the same validated modes. Conflicting modes and incomplete arguments are refused. |
| Cursor and Grok shared files were deleted wholesale | Cleanup recognizes exact DevCouncil invocations, filters flat or grouped entries, and deletes only files emptied by removal of owned entries. Empty unowned files remain. |
| Substring matching removed `mydev hook`, `echo 'dev hook'`, and other unrelated commands; quoted paths were missed | Direct-invocation parsing supports quoted Unix/Windows executable paths and preserves compound commands. A customized DevCouncil-named hook produces a refusal requiring review. |
| Codex/Gemini and OpenCode cleanup were missing | All six historical native host layouts are covered. OpenCode keeps MCP and other plugins; relative, absolute and file-URI references are removed with the generated plugin. |
| Rewriting changed 9007199254740993 to 9007199254740992 and pruned unrelated empty groups | JSON numbers use exact tokens; unrelated fields/groups are preserved. |
| Rewriting widened 0600 settings to 0644 | Original permission bits are preserved for replacements and backups. |
| Earlier files changed before a later malformed file was discovered | The entire selected scope is planned and validated before applying changes. |
| Symlinked files/parents, oversized or ambiguous JSON, invalid modes and uninspectable hook shapes could look successful | Scoped filesystem handles, regular-file checks, 1 MiB reads, 64-level JSON depth, duplicate-key checks, and explicit shape/mode validation now refuse these cases. |
| Concurrent cleanup or external edits could lose state | Exclusive cleanup lock, before/after identity and byte checks, unique staging files, exact recoverable backups and truthful partial receipts. |
| `--write-gate` claimed containment while ignoring the request | First refused before writes; **removed outright 2026-09-13**, together with `execution.hook_gate.mode` and its `contain` label. `gate status` no longer reports either — it was printing `write_gate: true` for a gate that did not exist. Both are answered by name, so an existing script gets the reason rather than "unknown flag". |

**Security impact:** no authorization, lease, verification-mode, host-permission,
credential or billing policy was changed. Retired event compatibility performs no
verification and emits no approval decision. Existing MCP policy remains its own
contract; applications must call and honor it. Unsupported hook enforcement is
reported explicitly rather than implied by a successful installer receipt.

The previous removal fixtures were corrected to contain actual owned hooks.
A shared file's mere existence or emptiness is not ownership evidence. The separate
regression `TestHookCleanupLeavesUnownedEmptyFiles` enforces that distinction.

## Verification

| Check | Verified result |
|---|---|
| `GOMAXPROCS=8 go test -race ./... -count=1` | All 25 packages with tests passed; packages without tests are not counted as test coverage. |
| Focused race repetition | Host/cleanup suites passed 20 repeats; the OpenCode correction passed a further 10 repeats. Each concurrent-cleanup test launches 32 writers. |
| Final fuzz campaign | 186,079 generated executions passed, after an earlier 232,400-execution campaign. The property checks foreign JSON preservation and idempotence with strings capped at 10,000 bytes; this is not exhaustive input coverage. |
| Actual executable matrix | 960 executions: 20 event forms × 8 client labels × 6 payloads, eight concurrent workers. All exit 0 with empty stdout/stderr. Payloads include malformed JSON, invalid bytes and more than 1 MiB. |
| Installed cleanup smoke | All eight paths exercised through the installed CLI; dry-run, exact backups, permissions, foreign data and repeated clean status passed. |
| Stalled input | 20 additional executable cases returned while the producer kept stdin open. |
| Observed matrix latency | Median 0.0124 s; maximum 0.1688 s on this run. These are local observations, not latency guarantees. |
| Other Go checks | `go vet ./...`, targeted formatting and `git diff --check` passed. |
| Repository CI checks | 25 script tests passed; release identity consistent; six workflows passed lint; npm package/runtime smoke passed. |
| Cross-platform build | Host and cleanup test executables compiled for Windows/amd64 and Linux/amd64. They were not executed natively there. |
| Installed artifact | Official `scripts/install.sh host` used; installed SHA-256 exactly matches the stressed artifact; `dev` resolves to `devcouncil`; `codesign --verify --strict` passes (local ad-hoc signature). |

Installed SHA-256: `68ffb6b7d51fbba7aa4f6dbe3c4691b5016b5e278c87aec221eeb14529fb2c91`.

Local receipts, logs, executable harness and the prior installed binary are in
[the qualification directory](../.devcouncil/hooks-qualification/).
A portable [source patch](../.devcouncil/hooks-qualification/hook-hardening.patch) is included for reconciliation; it is not auto-applied to the shared checkout.
The raw [binary matrix receipt](../.devcouncil/hooks-qualification/binary-stress.json)
and [ScholarLM status](../.devcouncil/hooks-qualification/scholarlm-status.json)
are included there. These are ignored local evidence, not release artifacts.

## Remaining limits

- **Unverified:** native Windows/Linux runtime and physical host UI sessions.
  This run verified the macOS binaries and configuration contracts, not every
  version of every client. Reload Claude to stop launching its cached registrations.
- **Unverified:** multi-file recovery under power loss or adversarial filesystem
  replacement in the final check-to-rename interval. Each replacement is atomic;
  the whole cleanup is not a filesystem transaction. Later I/O failure returns
  a partial receipt and the original backups. Backups preserve bytes and permission
  bits, not a promise of ACL/xattr replication.
- **Known scope:** customized unnamed shell wrappers and unknown plugin locations
  are preserved, not guessed. A recognized managed name with an unrecognized command
  produces an explicit refusal. No home-wide or all-repository cleanup was performed.
- **Partial evidence:** DevMap caller/affected-test walks report unresolved attribution
  and depth/cap limits. Source review and the full Go suite supplement those walks.
- **Not run:** the Rust quick/full suite, since this isolated patch changes no Rust
  code; remote CI, publication and merge were not requested. The source patch still
  needs reconciliation with the concurrent writer before merging into the shared branch.

## Operator controls

```sh
dev hook status --project-root /path/to/project --client claude
dev hook disable --project-root /path/to/project --client claude --dry-run
dev disable hooks --project-root /path/to/project --client claude
```

`status` returns 1 for remaining registrations **or** a refused inspection; read
its receipt and diagnostic to distinguish them. `--dry-run` never applies changes.
Omit `--client` to inspect the six supported cleanup clients at the selected root.

The host-output contract was checked against the official
[Claude hook reference](https://code.claude.com/docs/en/hooks) and
[Cursor hook documentation](https://prod.cursor.com/docs/hooks).
Claude documents exit 2 as a blocking hook result; Cursor's configuration is a
shared collection of hook entries, which supports selective removal rather than
assuming the whole file belongs to DevCouncil.
