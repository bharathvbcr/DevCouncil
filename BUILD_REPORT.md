# Build report: GitPulse backlog run, 2026-10-17 to 2026-10-18

This run worked the open GitPulse cards for DevCouncil. Each lane went through an Opus brief, a Sonnet implementer, one or more adversarial Sonnet passes, and an Opus review. Fable ruled on sequencing and tradeoffs. Local `main` moved from `4a5ca10c` to `638c5a6e` (as of 2026-10-19). Still in flight: ft-915b0a03 (Swift resolver residue), which was sent back after its attack found one high and four medium regressions, and ft-c0afdfef (Python receiver residue), whose implementer is still running. **Nothing is pushed.** No card reached done: requesting done returns "no verification record exists for this task: Manvi's work.verify.task has not judged it", and that tool cannot be invoked from this session.

AUDIT_REPORT.md's waves 1–6 were already all in review when the run began, so work continued by priority, severity and Fable's order. The previous report is kept as `BUILD_REPORT-2026-10-15.md`.

## Rules adopted mid-run (owner: "proceed with Fable's advice")

- A lane does not merge until its adversarial round closes with no open high or medium findings.
- At most two lanes run at once. Load was 60–190 throughout, so a wall-clock test that fails under load is treated as no evidence and rerun alone.
- Commit before any switch, checkout, rebase or cherry-pick. This was an owner instruction.
- `EXTRACTION_SCHEMA_VERSION` is frozen at 97. Once a version is on main it is frozen.
- Main is not pushed until the consumer sweep (ft-3018ab76).
- Cards stay in review until Manvi verifies them. An empty dead-code tier never passes.
- Every merge is built with `merge-tree`, tested on the combined tree, then fast-forwarded, and the card is updated only after the fast-forward is confirmed. Other sessions merged to main three times during the run.

## Cards moved to review (merged to local main)

| Card | What landed | Adversarial rounds |
|---|---|---|
| ft-b812a97c, ft-8ed75897 (harness parts) | honest_miss split (recorded / silent / wrongly_bound); `DeadFp` empty-tier state that never passes; fail-loud `unverifiable` rows; `tools/accuracy/cmd/labelmerge` (Go) | 17 breaks, all fixed |
| ft-40f73e87 (Python fields and `super()`) | `self.x = Ctor()`/factory field typing; bounded C3 MRO; per-file import-resolved bases; typing/abc markers skipped, aliases opaque | 5 rounds; regressions fixed; residue moved to ft-572bb69d |
| ft-a7f9a7de (TS spread/assign/alias) | `js_objects` facts plus a last-write-wins walk: member writes, conditionals, escape, default-import alias, Vue/Svelte | 19 breaks fixed |
| ft-6a46dddd (Swift properties) | properties become `Field` symbols reachable only via ReceiverType/self; dead-code exemptions | 2 rounds; Fable accepted 1.63× on 30k properties (new symbols) |
| ft-572bb69d (Python residue) | one `python_scope_lookup` for every resolver path; `ScopeIndex` keeps indexing linear | 3 rounds; 12 regressions plus 2 mediums fixed |
| ft-292df47e (corpus runner refusals) | refuses builds with budget refusals or extra parse failures; per-corpus `parse_failed` baseline in corpora.json; atomic record write | 1 medium plus lows fixed |
| ft-ce979db3 (MCP status) | daemon/MCP `degraded_reason` uses the same join as the CLI (`store_degraded_reason`) | 0 high/medium; 4 lows fixed |
| ft-c7d548e1 (dependencies direction) | outbound-only `deps` for symbols and files; additive `Response.note`; shared `bounded_text` | squared count (high) and min_rung (medium) fixed; criterion 1 amended by Fable |
| ft-b79f84e2 (refusal freshness) | every query answer leaves through one `exit` that stamps the store verdict; the verdict is pinned to the generation the answer read (race fix) | 3 rounds; one real race found on main and fixed |
| ft-75ddb66b (search half) | a missed name names the partial-parse region that may hide it; case per language; stale-file check; read cap 16 (Fable) | 3 rounds |
| ft-90cef8de (split) | treesitter.rs 17,248 → 84 lines and resolver.rs 13,658 → 859 lines, now in 27 submodules; byte-identical build | pure move, so identity evidence stood in for an attack |
| ft-19604c23 (Go type use) | same-file Go type mention exemption; the dead caveat names capped and eligible languages; resolver qualifier-slot leak fixed | 2 rounds each half |
| ft-eb82c9d3 (relative build root) | one `root_parser` anchors and lexically cleans every root | 1 medium plus 2 lows fixed |
| ft-befc1ff0 (blast pathspecs) | `GIT_LITERAL_PATHSPECS=1` in the shared dc-proc git runner; the conflicting global switches are stripped | 1 medium fixed; Go call sites are logged as a follow-up |
| ft-09f2437e (budget refusals in answers) | pinning tests for dead, affected, explore and the daemon drain; caveat names "budget-refused" and a count. Fable amended criterion 2 to a corpus-wide caveat. | 0 high/medium; 2 lows fixed |
| ft-93ece913 (blast/suspects) | mostly already on main (cded4595, 4e311c09); added an unreadable basis → ContentUnavailable | the ≤1/3 wall-time target has not been measured on a quiet machine, so the card stays open |
| ft-b7362179 (resolution-rate honesty) | call-blind and vendored sites leave the headline rate; additive `measured_resolved_sites` / `measured_unresolved_sites` / `measured_explained_sites` carry the headline denominators. Opus ruled the raw totals stay a public contract, and an invariant test ties them to the measured fields. Merged as `7e110127`. | probes repointed to `measured_*`; analyze and cli suites green on the combined tree |
| ft-bd1bfac9 (Python relative imports) | a relative import resolves only inside the importing package; a bare `.` or `..` names the package, so only its `__init__.py` answers (`30a7143c`). Merged as `ec034f60`. The follow-up `c167b9d8` handles namespace-package submodules and the bare-dots binding join. | 4 attack probes failed on the first round, then were fixed before merge |

## Benchmarks on touched paths

Instructions retired, min-of-N, interleaved.

| Path | Result |
|---|---|
| Python, TS and Go fixture builds | within ±2% of base on every lane |
| Swift fixture | +2.0% |
| Swift, 2000 shared names | 1.26× |
| Swift, 30k properties in one type | 1.63×, RSS 395 vs 189 MB. Accepted by Fable because it measures new symbols. The follow-up is folded into ft-f6abefd9. |
| TS 10k-alias hostile tree | 1.27× (bound 1.5×) |
| Python indexing, 2000 functions × 10 imports | 1.01× |
| Search miss over 2,000 partial files | 0.39 → 1.9 ms (~5×), which misses the 3× target. Accepted by Fable and recorded on the cap constant. A hit costs 1.04–1.06×. |
| Refusal freshness exit | 0.97–1.00× |
| resolver/treesitter split | ±0.5% |

## New and folded cards

- **New:**
  - ft-572bb69d — Python residue. Later worked and merged.
  - ft-38ef7393 — store charged partial-parse ranges as structured rows. Owner decision; it is a store schema change.
  - ft-eb82c9d3 — relative build root. Merged.
  - 10 split cards from the four stale umbrellas (label `split-2026-10-17`): ft-bd1bfac9, ft-1d03bb95, ft-c0afdfef, ft-eda9b3a6, ft-87b28bb6, ft-5ad86836, ft-80c5d100, ft-915b0a03, ft-292df47e (merged), ft-09f2437e.
- **Folded into existing cards:**
  - the 30k-symbol insert cost → ft-f6abefd9;
  - the generation-id reuse case → ft-c13da1b7;
  - the Go resolver half → ft-19604c23;
  - the wave-7 re-vendor target → ft-3018ab76.
- **Released to backlog:** ft-c666ba7c, ft-0720fe1d, ft-8ca270ad, ft-47124ed7 (each re-filed as the split cards above), and ft-befc1ff0, moved back to ready.

## Fable's decisions

- Waves serialise on resolver.rs. The Python follow-up ran in parallel with TS only because it fixed regressions on main.
- Schema handling:
  - accept the 94→95 bump;
  - hold the version across lanes;
  - bump to 96 once 95 was on main;
  - freeze at 97.
- BLESS only when monotone. Labels are justified in the label file.
- Typing markers are decided by resolved module, not by an allowlist. Opus narrowed this: builtins stay opaque, and the method allowlist was later deleted as unsound.
- An empty dead tier never passes.
- Do not merge `accuracy/rustlabel` on a shallow review. Another session merged it on 2026-10-18; see below.
- Release the stale umbrella cards. Do the split before further language work.
- deps criterion 1 is amended: outbound-only stays honest.
- Accept the Swift 1.63× (new symbols) and the search read cap of 16. Reject a cross-query cache.

## Deferred or blocked, with reasons

- **ft-3018ab76** — GitPulse re-vendor to extract v97, plus the Jarvis and Manvi pins. Each pin bump needs the owner and must be done from those repos. GitPulse's DevMap view already reports "stored extraction payload is obsolete" for this repo.
- **ft-aeb4a53f** — destructive schema migration (owner).
- **ft-1f7825c8** — release archives. Blocked on CI green (ft-7cb2d308), the release decision, and a supply-chain sign-off.
- **ft-75ddb66b criterion 3, ft-6a46dddd criterion 5, and the Python probes tagged "needs extractor change"** — waiting on the schema unfreeze or the corpus labels.
- **Corpus relabelling, the five "unmeasured" baselines** (LiquiTask, SalEdge, AcademiaTrack, Chronicle, DevPrism), **and the corpus re-measure** — owner work outside this repo.

## Owner decisions pending

1. Re-vendor GitPulse to extract v97 and bump the Jarvis and Manvi pins, then push main once.
2. Run Manvi verification for the review cards.
3. Relabel the corpora and measure the five missing parse-failure baselines.
4. Store schema: structured charged ranges (ft-38ef7393) and payload identity (ft-3bdf1450).
5. The count-only parse baseline cannot see a swapped failing file. Recording the set needs hashed paths.
6. `accuracy/rustlabel` was merged by another session (`1c4866d2`) after Fable advised a full read first. Decide whether it needs that review now.
7. Disk: about 37 GiB free. The gate holds deletes of finished build directories: `tmp/bra-target` and `tmp/lp-target` (8.1 GB), plus each agent worktree's `target-lane`. New lanes are paused until space is freed.
8. Leftover git state: one stash entry, `stash@{0}`, "lane-fix-search-partial". Its content is already committed, but the gate held the drop. Also scratch branches `attack/base-timing`, `attack2/timing-base`, `attack5/base-run`.

## Not verified

- No workspace-wide `cargo test`. Each merge was verified on the affected crates' full suites on the combined tree.
- Go host tests ran only for `dc/devmap`.
- DevMap MCP was unavailable for the whole run (ENOENT). Lanes navigated with the CLI and source reads.
- Manvi verification has not run.
- No corpus re-measure was done.
