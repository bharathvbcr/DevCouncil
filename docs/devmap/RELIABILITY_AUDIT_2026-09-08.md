# Devmap reliability audit — 8 September 2026

The audit reproduced and repaired five failure classes in the Rust kernel and its verification harness. The fixes strengthen conservative resolution, source coherence, and explicit failure reporting. They do not establish compiler-level semantic completeness across arbitrary programs or certify untested platforms.

## Checkout and change ownership

Work began from DevCouncil `ffde66713ab122e0ca8db5cd1773b6ee452aaf2b` in the isolated `codex/devmap-reliability-audit` worktree. Concurrent release work subsequently landed on main; this branch was fast-forwarded to `c99cbe9` (DevMap 0.1.1 / DevCouncil 0.4.5) before final verification. The corresponding GitPulse consumer branch starts at `2ad61c7`. Concurrent changes to the separate `rust/dc-grep` workspace were preserved.

No dependency was added. SQLite remains schema 19; extraction payload version 38 invalidates older binding metadata. The GitPulse copies are generated with `scripts/vendor-crates.mjs`, rather than independently patched. Its SQL test fixture now carries the source hash emitted by the real writer.

## Findings and repairs

| Finding | Reproduced behavior | Repair and durable coverage |
|---|---|---|
| RA1: lexical bindings | Parameters and assignments bound to unrelated module functions or imports at deterministic confidence. Receiver types leaked across sibling functions, captured module parameters, and anonymous callbacks. | Exact use-site binding records retain their declaring scope. Calls and value references consult them before import/global resolution. Python defaults use the enclosing evaluation scope. Valid outer calls and pytest fixture requests remain represented. |
| RA2: symbol identity | Same-named methods in one file collapsed into the first declaration; the second candidate disappeared from impact results. Nested functions could borrow a sibling's helper. | Candidate indexes retain full qualified identities. Ambiguity is deduplicated and capped by identity while retaining the full candidate count. Lexical parent traversal resolves nested declarations. Exact type ownership preserves same-named classes across files; unproven superclass dispatch abstains. |
| RA3: snapshot/source coherence | An empty watcher queue reported freshness after an unwatched edit or deletion. Search combined a stored name with changed file bytes. | Status verifies analyzer payload identity, source inventory/hashes, discovery refusals, recorded HEAD when available, and generation stability. Symbol rows carry the hash from their own SQL generation. Changed/unreadable source snippets are withheld with a reason; stored hits remain visible. Query response envelopes carry a `source_freshness` object (`fresh` null + `reason` when whole-tree freshness was not checked). |
| RA4: bounded reads | A file growing after discovery escaped the size ceiling. Preview and PDG also read source without that bound. | One reader enforces the 1 MiB limit during the actual read, rejects non-regular files without blocking on FIFO open, checks stable file identity/metadata, and preserves UTF-8/read failures. Scanning, daemon reads, snippets, preview, and PDG share it. Preview strings and stdin are bounded too. |
| RA5: false-passing stress checks | The soak swallowed query errors, hashed failed/empty SQLite reads, accepted daemon error envelopes, and claimed digest stability before the final restore had been indexed. | The harness fails each boundary explicitly, bounds IPC response bytes and total time, waits for stable freshness through watcher debounce, compares the final digest, and checks freshness after shutdown. Restoration avoids rewriting already-matching bytes. |

The original checkout also failed compilation because `devmap-query` re-exported `paths` twice. Mainline independently fixed that before this branch was reconciled; this audit preserves that correction.

## Regression evidence

Every runtime failure class was characterized against the unfixed implementation. The initial semantic suite failed 10 of 12 cases; three additional scope cases failed; two reference-identity cases failed; the receiver/capture extension failed all three added cases. Source-coherence testing reproduced `original_name` paired with the body of `modified_name`. Quiet-queue freshness, obsolete analyzer metadata, oversized reads, and oversized preview comparisons each failed their new assertions before repair.

The expanded semantic target now passes 22 cases, covering Python, JavaScript, TypeScript, Go, Rust, Java, nested functions, imports, callbacks, captures, fixture references, and a 40-candidate ambiguity capped at 16 emitted edges. Six mutation tests replaced lexical/receiver lookup with absent or fabricated answers; all six were caught by the new semantic suite.

Harness injection tests initially failed four cases while their valid control passed. The real-daemon teardown test then reproduced a success report with one pending source change. The repaired harness passes both the injected boundaries and the real shutdown/freshness check.

## Verification

Counts below describe distinct runs and overlap; they must not be summed as unique coverage.

| Check | Verified result |
|---|---|
| Complete Rust workspace | 2,283 tests passed; formatting and strict workspace/all-target Clippy passed. |
| Embedder configuration | Per-crate checks/tests with `--no-default-features` for extract, resolve, analyze, store, and query: 602 tests passed. |
| Python client/unit seams | 183 tests passed; Ruff passed on changed Python files. |
| Python integration stress | 8 tests passed, including eight concurrent builders, kill during persistence, hostile trees, a 2,000-file burst, stalled daemon fallback, budget refusal, and final soak restoration. |
| GitPulse consumer | 19 code-intelligence unit tests and 7 embedding/link integration tests passed. Source-change refusal reaches the host without losing the stored symbol. Vendored/CLI schema both 19. |
| Original executable probes | Parameter shadowing, both ambiguous method identities in impact, unwatched-edit freshness, and rebuild recovery all pass against the release candidate. |
| Targeted mutation testing | 6 selected mutations tested, 6 caught, with an unmutated baseline. This is not workspace-wide mutation coverage. |

The workspace's three default ignored entries are accounted for separately: the long daemon storm was explicitly run; `concurrent_prune_writer_child` is a subprocess helper exercised by its parent; the external Claude plugin validator was explicitly run and passed in 2.22 seconds. The main suite's ignored count is not represented as executed coverage.

## Stress and performance measurements

- **40 storm/kill/restart cycles:** passed in 251.17 seconds. Shapes include 10,000 creations, mass directory rename, deletion/recreation with new content, and 1,000 chained rename cycles. All endpoints recovered without manual cleanup. Final node/edge counts and symbol membership matched a cold build. RSS half-means were 126,172 → 129,739 KiB (+2.8%).
- **100 rebuild cycles:** every restored digest matched baseline. RSS half-means were 25,413,355 → 25,440,471 bytes; store half-means were 1,453,305 → 1,453,433 bytes, within the unchanged 10% limits.
- **Daemon storage diagnosis:** the first 150-cycle run failed its growth gate before the five-minute maintenance checkpoint. A 1,200-cycle investigation observed bounded WAL allocation and reclamation (18,473,040 → 1,425,408 combined bytes at cycle 781). This was measured behavior, not a reason to raise the threshold. That investigation's memory/storage results were valid, but its original digest-success wording was rejected because teardown left pending work; RA5 repairs that claim.
- **Self-build gate:** 1,633 files, 5.410 seconds, 167 MiB cold database, 760 MiB peak RSS; below the 10-second, 255-MiB store, 2-GiB absolute RSS, and 884-MiB scaled RSS gates.
- **Ambiguity memory model:** 5,000 sites × 100 candidates (500,000 weighed, 80,000 emitted edges), then 200 candidates. Measured cost was 110% of the model and per-candidate cost changed to 99% when width doubled, within the existing limits.
- **Fresh status cost:** three complete status probes over 1,634 files took 101.84, 100.26, and 113.20 ms; each reported fresh with zero pending paths. Queries continue to read only returned source files, bounded by 1 MiB per file, rather than rescanning the whole tree.

**Final completion:** `verify.sh` passed again with 2,283 workspace tests, format, strict Clippy, deterministic digest `6fd5be321abc9f1183a8b77bade5a98a529a1a8d345a3a7c51410ba1e3128c26`, and all mandatory performance/growth gates. The final self-build covered 1,634 files in 5.370 seconds at 749 MiB peak RSS. The memory model measured 111% of prediction and 97% width ratio.

The repaired harness then passed **1,200 daemon cycles**, including the actual final digest comparison, stable freshness through debounce, and the stopped-state freshness check. RSS half-means were 28,554,550 → 28,697,916 bytes (+0.50%); combined store/WAL half-means were 18,329,965 → 12,204,096 bytes. Peak RSS was 28,966,912 bytes; the stopped store was 1,654,784 bytes. The 10% plateau thresholds were unchanged. Logs: `/tmp/devmap-audit-verify-final.log`, `/tmp/devmap-audit-daemon-verified-soak.log`, `/tmp/devmap-audit-python-stress-verified.log`.

GitPulse impact analysis classified the shared kernel changes as critical: 13 files, 68 indexed symbols, and 18 affected flows. Changes were reviewed against the intended extraction, resolution, store, query, and host fixture paths; the consumer source and schema checks passed.

## Contracts and limits

Ambiguous targets remain speculative; bare calls through unknown local values remain unresolved, while receiver guesses retain their heuristic evidence tiers. Stable ordering, omitted counts, parse deadlines, cancellation, durable transactions, and query budgets remain enforced. Source freshness and graph completeness are separate claims: matching bytes cannot make an unsupported extractor complete.

Source status is a bounded observation of a live filesystem, not a filesystem snapshot lock. Later edits can invalidate it. Source reads use a non-cryptographic content identity and are not an adversarial tamper-proof storage guarantee. Whole-file verification costs more I/O than the previous unchecked prefix read. Preview content paths must be regular files; empty input can use an empty file or stdin.

This pass ran on macOS/Apple Silicon. Linux and Windows behavior, physical GitPulse UI delivery, power-loss recovery, a multi-week production soak, and exhaustive mutation coverage were not established. Existing language limitations remain explicit: disabled/unlinked grammars and languages without call/import extractors report gaps; dynamic dispatch, reflection, monkey-patching, compiler overload/MRO semantics, and general runtime binding inference are not proved complete. These are limits on using a static map as sole authority for destructive changes.

## Reproduction

From `rust-port/`, use `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0` for the same local debug/test build configuration:

```sh
./verify.sh
cargo test --locked -p devmap-resolve --test semantic_scope_and_identity
DEVMAP_SOAK_CYCLES=40 cargo test --locked -p devmap-cli --test daemon_storm_soak -- --ignored --nocapture
DEVMAP_BIN=/absolute/path/to/devmap SOAK_CSV=/tmp/build.csv tools/soak.sh /scratch/corpus 100
DEVMAP_BIN=/absolute/path/to/devmap SOAK_CSV=/tmp/daemon.csv tools/soak.sh /scratch/corpus 1200 --daemon
```

Run the feature-off matrix per crate, as `.github/workflows/rust-port.yml` does; workspace feature unification can otherwise hide missing gates. Python stress requires `DEVMAP_BINARY` to point to the tested release binary. The audit used an immutable candidate with SHA-256 `f92d16c18acadd7f418c4f388690d1e701a3876e0ebcfbf121e24b15e444440d`.

Detailed local logs are `/tmp/devmap-audit-*.log`; stress samples are `/tmp/devmap-audit-*-soak.csv`. The source regressions are retained in the repository, so transient log retention is not required to reproduce the contracts.
