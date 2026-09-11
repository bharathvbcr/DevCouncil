# Build progress hardening audit

Scope: the `devmap build` progress and result path, including shared discovery
and extraction instrumentation, diagnostics, terminal output, cancellation,
and compatibility with consumers. This is an implementation and evidence ledger;
an unchecked requirement is not a passing check.

## Requirements and evidence

| Requirement | State | Required proof |
| --- | --- | --- |
| Progress cannot hold indexing or JSON completion behind paused/closed terminal output | Passed on macOS and Linux | PTY pause/saturation/disconnect, closed/full pipes, failing builds; bounded finish and valid JSON |
| Terminal state and inherited descriptor flags survive progress output | Passed on macOS and Linux | Parent-side termios/flags checked by real PTY tests; progress never hides the cursor or changes terminal modes |
| Fast builds avoid flashing; all redraws have bounded rate and storage | Passed | 150 ms reveal, 80 ms refresh boundary tests; 100,000 updates plus 1,000 diagnostics against a stalled renderer |
| Real processed/total file counts and cache/delta statistics | Passed | Actual work-loop counters; cold/warm/full/add/change/remove tests; 100,000 concurrent completions; guarded cache hits |
| ETA/throughput are measured and never invented for unknown work | Passed | Unknown/empty/invalid counters return null estimates; minimum ten files and one second; no ETA after completion |
| One compact interactive result; verbose detail remains available | Passed | One generation summary, verbose reclaim exactly once; diagnostic failures remain visible; JSON fields retained |
| JSON remains one valid result and retains timing/coverage information | Passed on macOS and Linux | Success, unchanged, artifact error, saturated error pipe, no-progress and forced-progress cases |
| Resize, long/unicode/control-containing paths and ASCII terminals behave correctly | Passed within tested scope | Real PTY width 80 -> 24 -> 120; bounded rows, control/bidi escaping, ASCII locale unit checks. Emulator-specific scrollback reflow remains unverified |
| Interruption leaves no cursor/terminal corruption, stale writer lock or corrupt generation | Passed within tested scope | SIGINT/SIGTERM during animated lock waits; six macOS process probes at scan, resolve and persist, each followed by rebuild and SQLite integrity_check |
| Windows and Linux compile and run supported output modes | Partial | Linux native container: 22 integration + 3 renderer + 2 counter + 1 cache tests passed. Exact renderer/counter sources compile for Windows MSVC; native Windows runtime remains unverified |
| Index semantics remain equivalent with instrumentation enabled/disabled | Passed within tested scope | Cache/full extraction equality across 64 content collisions, empty/partial/failed parses; stripped source text accounted against cache hits. Deterministic cold builds and five incremental/cold equivalence cycles passed |
| Documentation and installed binary match verified source | Passed | README and audit updated; isolated source matched the owned files; release/install hashes match; MarkDev cold/unchanged PTY smoke passed |

## Baseline

- Existing first-pass changes are the four dirty files in this task; no dirty
  overlap was reported across the three checked worktrees.
- Reproduction against the installed first-pass binary: pause terminal output
  with `tcflow(TCOOFF)`, keep JSON stdout separate. `--progress never` completes
  in about 38 ms; `--progress always` remains blocked beyond two seconds and
  completes after terminal output resumes. The renderer uses blocking writes,
  a blocking channel send, and an unbounded worker join.
- First-pass verification reported 2,209 test passes and three ignored test
  entries. This is historical baseline evidence, not proof of the fixes below.
- The repository-wide formatter flagged an existing layout at
  `devmap-serve/src/daemon.rs:1296`. A formatting-only three-lines-to-one-line
  rewrite clears the standard format gate; no daemon logic was changed.
- Additional regressions were demonstrated before their fixes: a failed artifact
  stage printed a success marker; verbose artifact output retained the writer lock
  behind a full stdout pipe; and single-file summaries used plural wording.
  The current tests preserve each negative case.
- An intermittent disconnected-PTY test also exposed a fixture defect: raw
  `openpty` descriptors could be inherited by concurrently spawned test children,
  keeping the apparent disconnected peer alive. A failing close-on-exec assertion
  proved this; the fixture now opens both descriptors with atomic close-on-exec.
  Production peer checks remain separate from this fixture correction.

## Output contract

- Progress is observational. Its transport must not change graph work, mutate
  inherited terminal flags, or hold a writer lock while awaiting a display.
- A dropped frame may be superseded; a diagnostic must be retained or its
  omission disclosed with a count. Failure to render is not a successful render.
- Completion follows every requested artifact write. Interrupted/failed work
  cannot print a success marker.
- Discovery has no known total before its walk completes. File percentages and
  ETA apply only to a measured phase with a known denominator.
- A terminal capability that cannot be established uses an explicit supported
  fallback. Untested platforms remain unverified in this ledger.

## Current verification evidence

- `/tmp/devmap-progress-final-output-order.log`: 22 macOS progress integration
  tests pass, including verbose artifact output blocked on stdout. The next writer
  acquires the lock while the first process is still waiting to print its result.
- `/tmp/devmap-progress-final-linux.log`: native ARM64 Linux, official Rust 1.98.0
  Bookworm image; 22 integration, three renderer, two counter and one cache
  equivalence tests pass on the final implementation.
- `/tmp/devmap-progress-equivalence.log`: collision/parse-outcome equivalence passes.
- `/tmp/devmap-progress-accounting-tests.log`: concurrent counters and cache tests pass.
- `/tmp/devmap-progress-windows-renderer-check.log`: `cargo check --tests --target
  x86_64-pc-windows-msvc` passes for exact copied renderer, sink and counter sources.
  Whole-crate cross compilation is unavailable without Windows C headers/SDK;
  the repository's existing Windows CI matrix remains the native execution gate.
- `/tmp/devmap-progress-interrupt-probe.log`: six signal/rebuild/integrity probes pass.
- `/tmp/devmap-progress-isolated-verify.log`: the repository's `bash verify.sh`
  completed all mandatory gates: format, workspace clippy with warnings denied,
  2,222 test passes (three ignored entries), determinism, performance, memory model,
  growth and five incremental/cold equivalence cycles. Optional mutation testing
  was skipped; it was not requested by the verification command. This run preceded
  the final output-order, wording and ASCII-fallback changes; the final source is
  checked separately below.
- That verification measured a 5,774 ms cold self-build over 1,623 files, a 152 MiB
  database against a 253 MiB gate, and 720 MiB peak RSS against an 866 MiB budget.
  Deterministic graph SHA-256:
  `76608758eb6aac3ec7fee10a55361df4bf95e057425ffff613df0132229bdc4c`.
  The synthetic memory probe measured 92,875 milli-bytes per candidate, a 96%
  width ratio and 103% of model prediction. These are bounded synthetic probes,
  not a measurement of the larger production corpus described by the script.
  Five growth snapshots were 1,163,264 / 1,359,872 / 1,425,408 / 1,392,640 /
  1,409,024 bytes with two generations retained.
- `/tmp/devmap-progress-storm-soak.log`: the normally ignored storm/restart test
  was explicitly run and passed 12 cycles in 105.19 seconds, covering 10,000-file
  creations, directory renames, delete/recreate, 1,000 rapid rename sequences and
  kill/restart convergence. RSS half-means after three warm-up cycles decreased
  from 131,496 to 111,341 KiB; this bounded run does not prove indefinite stability.
- `/tmp/devmap-progress-plugin-validation.log`: the normally ignored strict Claude
  plugin validation test was explicitly run and passed. The third ignored entry
  is a subprocess helper exercised through its passing parent store test.
- `/tmp/devmap-progress-feature-off.log`: extract/store `--no-default-features
  --all-targets` checks and the extraction counter tests passed. This is per-crate
  feature compatibility, not a claim that every compiled dependency omits grammars.
- `/tmp/devmap-progress-final-cache-failure.log`: the final macOS rerun initially
  stopped because cached generated `bindgen.rs` and `stdlib-symbols.txt` were
  missing. Only the isolated checkout's two affected dependency caches were
  cleaned before retrying; the separate release-profile cache needed the same
  repair. No source change was made for this environment failure.
- `/tmp/devmap-progress-final-isolated.log`: final-source formatting, workspace
  clippy with warnings denied, and all 2,224 test passes completed (zero failures,
  three ignored entries as explained above). Its subsequent release step stopped
  on the stale release cache; `/tmp/devmap-progress-final-release.log` records the
  separate release rebuild after clearing the two affected dependencies.
- `/tmp/devmap-progress-final-release.log`: optimized release build passed after
  the cache repair (2 minutes 15 seconds).
- The shared workspace's full suite stopped on an unrelated concurrently added
  `devmap-query/tests/viz_projection.rs` test missing its feature classification.
  That work was preserved. Verification and installation use an isolated checkout
  at HEAD `06398b0288193e6c3560bf1d50c0fd6e32c40303` plus only this change's 11 files.
  The unrelated visualization edits were excluded from both the isolated build
  and the installed binary.

## Installed result

- Installed executable: `/Users/bharath/.cargo/bin/devmap`, version
  `devmap 0.1.0 (store schema 19, code graph schema 2)`.
  SHA-256 matches the isolated release executable:
  `7d55a70b42f24a3689ebf25caa2d7f458457a1bf8cef454ff90e0e694bc2b61d`.
  Installation was atomic; the prior executable remains backed up at
  `/tmp/devmap-progress-previous-binary`.
- `/tmp/devmap-progress-final-install-smoke.log`: a one-file human build and
  unchanged build show correct singular wording. Initial ad-hoc smoke assertions
  incorrectly assumed function-only graph counts and a different unchanged-summary
  phrase; they were corrected against the existing graph contract and actual
  summary source, without changing production code or repository regressions.
  Their output is retained in `/tmp/devmap-progress-smoke-assertion-failure.log`.
- The installed binary indexed MarkDev's 288 files using a temporary database:
  cold build 3.050 seconds with animation; unchanged build 0.069 seconds without
  animation. Both returned valid JSON with `progress_output.incomplete=false`,
  exact 288-file deltas and unchanged PTY termios. MarkDev source and its regular
  database were not changed. JSON and terminal transcripts are saved under
  `/tmp/devmap-progress-markdev-{cold,unchanged}*`.
- The installed binary also checked the canonical DevCouncil index: generation
  1,257 remained current for 1,625 files. Its one oversized vendored COBOL parser
  refusal was disclosed on stderr and in JSON; it was not reported as complete
  source coverage. The progress receipt was complete. Evidence:
  `/tmp/devmap-progress-final-repo-build.{json,stderr}`.
- The final owned source snapshot and hashes are retained at
  `/tmp/devmap-progress-reviewed-source.tar.gz` and
  `/tmp/devmap-progress-source-hashes.json`; the temporary verification worktree
  can be discarded without losing edits. No dependencies, commits or release
  tags were added. The container service was restored to its initial stopped state.

## Boundaries

- The transport queue holds at most 32 events; retained diagnostics at most 64;
  each queued message is capped at 4 KiB plus a truncation marker. The receipt
  distinguishes total diagnostics, accepted writes, retained text and omissions.
- Shutdown waits at most 100 ms. Pipe writes are isolated from the build thread;
  terminal handles are separately reopened nonblocking, and parent flags stay intact.
- A successful write means bytes were accepted by the stream, not physically seen.
  Primary stdout may block under its own normal backpressure. No output mode
  promises delivery to a reader that has disappeared.
- Windows deliberately uses ASCII plain progress until native console animation
  is implemented and verified; no ANSI capability or UTF-8 console code page is
  assumed. Exact renderer/sink/counter compilation is not full application or
  native Windows runtime validation.

## Sources

- POSIX terminal identity: <https://man7.org/linux/man-pages/man3/ttyname.3.html>
- Independent open-file descriptions and nonblocking I/O:
  <https://man7.org/linux/man-pages/man2/open.2.html>
- Windows console handles:
  <https://learn.microsoft.com/en-us/windows/console/console-handles>
