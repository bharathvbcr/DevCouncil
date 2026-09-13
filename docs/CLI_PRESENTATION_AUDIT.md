# CLI presentation and output reliability audit

Date: 2026-09-12. Scope: the installed DevCouncil command surfaces, terminal
presentation, output framing, failure reporting, and cancellation. This is not
an assertion that every defect in the analysis engines or every operating
system has been eliminated.

## Command coverage and ownership

| Surface | Policy and canonical owner |
| --- | --- |
| DevMap build | Existing five-stage `ProgressReporter`, measured file counters, bounded `progress::Display` |
| DevMap finite commands | Exhaustive `Commands` match in `src/presentation.rs`; one shared display and checked stdout path |
| Go host install/uninstall, enable/disable, skills, integrate/integrations, verify, gate | `cmd/devcouncil/presentation.go` selects policy; `console` owns rendering, stream checks, and bounded diagnostics |
| Go host map/graph/ast | Delegate to DevMap; preserve child arguments and output; propagate cancellation |
| DevMap MCP/serve and Go host MCP | Protocol output remains raw; DevMap tracing uses the bounded diagnostic sink |
| Hooks and generated configuration | No loader or completion card inside protocol/configuration output |
| GraphML to stdout | Raw export only; incompatible `--json` is rejected before export work |
| dcstore/dcverify/dcgrep | JSON-only boundaries; checked stdout writes, quiet transport failure |
| `dev` | Existing symlink to the same Go host |
| `dcmap` development driver | Existing thin mapcli driver; no parallel presentation implementation or new legacy surface |

DevMap's 37 explicit top-level commands have an exhaustive presentation policy.
Nested workspace, Claude, skills, and integration actions inherit their owner;
raw configuration actions explicitly opt out. Clap-generated help is unchanged.
The stress script exercises 33 finite DevMap command forms plus build, four Go
JSON command forms, raw exports, live terminals, and an actual HTTP listener.
This is command-form coverage, not every possible argument combination.

## Confirmed defects and fixes

| Finding | Evidence before the fix | Resolution and regression |
| --- | --- | --- |
| Non-build errors could block behind full stderr | Search exceeded the two-second diagnostic-pipe bound | Route runtime diagnostics through the shared bounded display; `test_progress::an_error_still_returns_json_when_the_progress_pipe_is_full` covers build/search with and without JSON |
| Host install dry-run ignored JSON framing | `install devmap --dry-run --json` began with `PREFIX=...` | Emit one plan receipt; `TestInstallJSONDryRunIsOneReceipt` |
| Host ignored a failed JSON write | Skills list returned success to a closed output pipe | Checked JSON owner and primary-write failure status; `TestJSONOutputFailureCannotReportSuccess` |
| Skipped verification was labeled verified | Off/empty/no modes returned `verified`, `passed=true` | Return `skipped`, `passed=false`; increment `completed_without_verification`; retain separate completion/skip semantics and existing gate policy |
| FTS-only repair emitted no JSON | JSON parser reached EOF on a successful repair | FTS receipt; combined FTS/pending/page-size receipt emitted once; schema remains exclusive |
| Export mixed XML and JSON | `export --out - --json` succeeded with both formats | Reject incompatible formats before export; preserve actual GraphML and the existing Git-call regression |
| JSON components panicked on disconnect | All three exited 101 with `failed printing to stdout: Broken pipe` | Thin checked stdio adapters return exit 1 without stderr panic; success and error paths tested against actual binaries |
| MCP HTTP could stop before serving requests | With full stderr, TCP bound but the first HTTP response timed out | Move tracing to the display's thread-safe bounded diagnostic endpoint; actual HTTP regression in stress script |
| Version inventory ran unbounded child probes twice | A three-second fake version binary held the probe for three seconds | The initial bounded-probe fix is superseded by passive binary identity inspection: `doctor` and `paths` do not execute discovered or configured programs; external versions stay unavailable with an explicit `probe_error`. `passive_diagnostics` covers configured commands and PATH aliases. Optional ps/date probes remain bounded |
| New renderer failed adversarial Unicode and short-write cases | Split UTF-8 became replacement characters; short diagnostics were unreported; activity newlines reached frames | Preserve UTF-8 across buffer and write boundaries; escape invalid/control bytes; detect short writes; bounded retained samples with total/omitted counts |
| Interactive installer logs stopped the loader early | First child stdout line set the result channel quiet | Route interactive installer logs through diagnostics; keep the loader active until the installation returns; `TestInteractiveInstallerLogsLeaveTheLoaderActive` |
| Fast Go commands could suppress the first frame | Real PTY skills-list returned its body and footer without a loader | First frame is painted before the clear acknowledgement; subsequent frames honor quiet state; no artificial work delay |
| Cancellation looked like a ten-minute timeout | SIGINT produced the timeout wording | Distinguish cancellation and use the canonical 100 ms subprocess cleanup grace before CLI exit; SIGINT/SIGTERM process-reap stress |

The schema flag already rejects other repair flags; that guard was preserved.
The original export trace test had relied on the invalid mixed-format call.
It now requests a file plus JSON and checks the written GraphML, parsed node
count, and exactly one Git log call. Its original purpose was not weakened.
The existing Go subprocess source guard also remains intact; cancellation
cleanup is implemented in the canonical `proc` runner.

## Invariants

- No new dependencies, fake progress percentage, fabricated ETA, or minimum
  command duration. Animation cadence is 80 ms; non-build orbits are
  indeterminate. Build counters continue to describe measured work.
- JSON stdout and protocol stdout contain no presentation escapes. Terminal
  stderr does not cause redirected stdout to become decorated.
- Commands report success only after their work returns and primary output is
  accepted. A skipped verifier is not counted as a verifier that passed.
- Optional output queues hold 32 updates. Retained diagnostic samples hold at
  most 64 entries, each bounded to roughly 4096 bytes. Omitted samples and
  delivery failures are distinct from complete output.
- Rust renderer shutdown waits at most 100 ms; Go uses separate clear/drain
  waits of at most 100 ms each. Primary stdout has ordinary backpressure.
- Live terminal output uses an independently opened handle. Inherited
  descriptor flags and termios state are preserved. A disconnected or paused
  stderr is not a reason to lose an independent JSON result.
- Formatting buffers remain bounded, including split Unicode from child
  process streams. Terminal controls and bidirectional formatting controls
  cannot become renderer commands.
- Protocols do not acquire a second output format. Each Rust JSON component
  retains a thin local stdio adapter so it stays independently packageable.
- No authentication, authorization, rate-limit, or write-gate policy changes.
  The verification change corrects reported evidence; it does not widen access.

## Verification

Commands were run against the local working tree. Concurrent hook/integration
work was present before this task and was preserved. DevMap impact evidence was
used with source inspection; its unresolved sites and depth-limited walks are
not proof of complete caller coverage.

| Check | Observed result |
| --- | --- |
| `bash rust/verify.sh` | 3034 tests passed, 3 existing ignored tests; fmt and clippy with warnings denied passed |
| Release worker recovery | Intentional worker panic returned; cleanup and a subsequent worker completed |
| Determinism | Two cold-build graph digests matched |
| Release self-build | 913 files; 2009 ms; 141 MiB cold database against 156 MiB gate; 621 MiB peak RSS against 799 MiB budget |
| Ambiguity memory probe | 5000 sites, 16-candidate at-cap and 64-candidate above-cap legs; memory model passed; these are bounded synthetic corpora |
| Growth gate | Five builds retained two generations and plateaued |
| 40-cycle soak | Digest stable; RSS means 28,014,455 → 28,277,691 bytes; database means 1,620,923 → 1,629,661 bytes, within 10% plateau bounds |
| Go full suite | `go test ./... -race -count=1` passed all 26 packages with tests; the final installer-output adjustment also passed focused console, CLI, components, and subprocess race tests |
| Command terminal stress | Release DevMap, development Go host and debug components: 37 JSON command forms, 35 PTY runs, 3 full-stderr cases, 800 concurrent queries, 600 component disconnections, 2 cancellation cases; all passed in 9.308 seconds |
| Subprocess cleanup | 50 additional SIGINT/SIGTERM interruption-and-reap checks passed |
| Packaging | 25 script tests, release identity, six workflow files, npm pack dry-run and npm runtime smoke passed |
| Host cross-build | CGO disabled; darwin/linux/windows × amd64/arm64 compiled |

The terminal stress report is generated with the command below. Its emitted
counts are the actual runs, not an extrapolation to all flags or platforms.
It checks 16/40/80/120-column PTYs, auto/always/never, C locale, TERM=dumb,
NO_COLOR, redirected stdout with terminal stderr, paused terminal output,
configuration purity, raw GraphML, full diagnostic pipes, HTTP startup,
concurrent queries, cancellation, and component pipe disconnects.

```sh
# From the repository root, using already-built binaries:
python3 scripts/cli-presentation-stress.py \
  --devmap rust/target/release/devmap \
  --host /tmp/devcouncil-ui-host \
  --components rust/target/debug \
  --iterations 100 \
  --report /tmp/devcouncil-ui-stress-final.json

# Full repository verification:
bash rust/verify.sh
go -C backend/go_orchestrator test ./... -race -count=1
```

The development host in the stress command is produced with
`go -C backend/go_orchestrator build -o /tmp/devcouncil-ui-host ./cmd/devcouncil`.
The stress script never installs software and confines mutations to a temporary
repository. Release binaries are used for latency checks: diagnostic hashing of
large development binaries is substantially slower in an unoptimized build.

## Installed-binary verification

The official installer completed successfully:

```sh
bash scripts/install.sh host devmap dcstore dcverify dcgrep
```

Shell resolution confirmed `devmap`, `devcouncil`, `dev`, `dcstore`, `dcverify`,
and `dcgrep` under `/Users/bharath/.local/bin`; `dev` points to `devcouncil`.
The five installed binaries passed local signature verification. These are
local builds with ad-hoc signing, not a notarized or published release.

A second stress run used only those installed binaries: 37 JSON command forms,
35 PTY runs, three full-stderr cases, 200 concurrent queries, 150 component
disconnects, and two cancellation cases passed in 5.433 seconds. Eight further
host command-family PTYs and 50 SIGINT/SIGTERM interruption-and-reap checks
also passed. The final DevMap index was fresh and query-ready, with zero pending
or quarantined entries and no rebuild required.

The previous binaries were copied and hash-verified before installation in
`/Users/bharath/.local/share/devcouncil/backups/command-ui-20260912T170145Z`.
That directory also contains the installed binary hashes and post-install stress
receipts. Existing server processes were not restarted; new launches use the
updated executables. Source changes remain uncommitted, and concurrent changes
outside this presentation work were preserved.

## Qualification limits

Native runtime and PTY evidence is macOS arm64. Linux and Windows host builds
are compile checks, not physical console qualification. Windows deliberately
uses plain output; no unsupported animation promise is made. Native Terminal
app automation was unavailable; PTYs exercise the real terminal device API but
are not a screenshot of Terminal.app.

Optional cargo mutation testing was not run. The full pipeline explicitly
reports that skip; hand-authored failure regressions and the 40-cycle soak do
not substitute for mutation coverage. Filesystem operations stalled inside the
kernel, forcibly killed parents, and descendants that escape their process
group cannot be claimed universally bounded by portable user-space code.
No production deployment, release publication, or exhaustive audit of unrelated
engine, sandbox, or policy functionality is claimed.
