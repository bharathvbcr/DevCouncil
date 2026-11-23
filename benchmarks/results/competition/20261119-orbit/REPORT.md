# One-file edit against Orbit, main 290e355b vs schema 29, 5 Nov corpora

- generated: 2026-11-19, card ft-5c093588 (measurement only, no code changed)
- host: Apple M5 Pro, 18 cores, 64 GB, macOS 27.2. **Shared and heavily loaded: 1-minute load average was 20-81 during the run** (5 Nov run: 2.65-17.9). Per-row load is in `measure.jsonl` (`load1_before`).
- statistic: minimum of 5 interleaved reps (per rep: main, schema 29, Orbit; each does edit, revert, refresh). Cold is a single warm-up run, not min-of-5.
- raw: [`measure.jsonl`](measure.jsonl) (one row per timing, with the DevMap phase split), [`provenance.json`](provenance.json)

## Read this first

1. **Wall-clock numbers here are not comparable to the 5 Nov table.** The host was at load 20-81 on 18 cores. The 5 Nov Orbit edit minima (0.19, 0.29, 1.24, 0.58 s) were not reproduced by Orbit itself in this session (0.31, 1.34, 0.61, 1.04 s on three of four). The wall bar in criterion 1 therefore cannot be judged against those constants. The same-session Orbit column is the fair wall comparison, and it is still load-skewed. Instructions retired do not depend on load and are the verdict.
2. **There is no committed harness for the 5 Nov run** (`20261105-orbit/` holds data and a report only), so the harness is rebuilt here from the 5 Nov REPORT: append one function to one file, time the build, revert, time the build, time an unchanged refresh. The 5 Nov edit file is not recorded. This run edits one mid-size source file per repo (see provenance). Orbit's edit cost depends on that file's dependency cone, so Orbit's absolute numbers differ from 5 Nov (GitPulse: 5.6B here, 32.5B on 5 Nov). DevMap's own edit instruction counts did reproduce schema 29's 5 Nov numbers (DevCouncil 45.6B vs 47.3B, GitPulse 86.7B vs 91.9B, DevPrism 11.0B vs 11.3B), which suggests DevMap's edit cost is nearly independent of the edit file.
3. **Orbit must not be given `--db`.** In the Orbit source (`commands/index/incremental/mod.rs`, `snapshot = options.db.is_none() && ...`) an explicit `--db` disables the incremental path, and Orbit then re-indexes everything on every call (first attempt here: edit 24.9B instructions, equal to its cold 25.4B). The numbers below use `ORBIT_DATA_DIR` only.
4. Main is at extract schema 100, schema 29 at extract 98 (`devmap --version`). Both are store schema 29. Main vs schema 29 differences are therefore everything between 5a4a15e7 and 290e355b, not resolve changes alone.
5. 0 budget-refused extractions in every DevMap row (`discovery_refused_files`=0, `budget_retry.refused`=0); every edit row reports exactly 1 changed file; Orbit reported 0 skipped and 0 errored files.

## One-file edit

Wall seconds min / median / max, n=5. Instructions retired in billions, min / median / max.

| Repo | Tool | Wall (s) | Instructions (B) | load1 range |
|---|---|---|---|---|
| DevPrism | main 290e355b | 0.44 / 0.47 / 0.81 | 7.75 / 7.81 / 7.89 | 20.78-23.69 |
| DevPrism | schema 29 5a4a15e7 | 0.37 / 0.43 / 0.5 | 10.96 / 11.07 / 11.12 | 20.78-22.59 |
| DevPrism | Orbit | 0.31 / 0.42 / 0.5 | 2.57 / 2.76 / 2.97 | 19.76-23.69 |
| DevCouncil | main 290e355b | 4.61 / 4.96 / 15.92 | 29 / 30.07 / 30.46 | 24.52-53.06 |
| DevCouncil | schema 29 5a4a15e7 | 2.74 / 4.55 / 9.38 | 45.63 / 47.08 / 47.38 | 26.82-51.97 |
| DevCouncil | Orbit | 1.34 / 1.73 / 2.72 | 5.37 / 5.53 / 5.85 | 42.26-51.41 |
| GitPulse | main 290e355b | 3.13 / 5.46 / 7.03 | 55.48 / 56.95 / 57.76 | 27.12-54.98 |
| GitPulse | schema 29 5a4a15e7 | 2.77 / 4.39 / 5.11 | 86.66 / 87.74 / 88.28 | 25.19-49.15 |
| GitPulse | Orbit | 0.61 / 0.73 / 0.91 | 5.6 / 5.62 / 7.92 | 24.49-45.64 |
| orbit-knowledge-graph | main 290e355b | 1.63 / 2.7 / 3.27 | 23.94 / 23.98 / 24.86 | 32.78-54.25 |
| orbit-knowledge-graph | schema 29 5a4a15e7 | 1.22 / 1.79 / 4.28 | 35.68 / 35.84 / 36.88 | 34.89-58.93 |
| orbit-knowledge-graph | Orbit | 1.04 / 1.2 / 2.67 | 10.64 / 10.73 / 14.81 | 34.41-49.96 |

Instruction ratios on the minimum (lower is better for DevMap):

| Repo | main / schema 29 | main / Orbit | schema 29 / Orbit |
|---|---:|---:|---:|
| DevPrism | 0.71 | 3.0 | 4.3 |
| DevCouncil | 0.64 | 5.4 | 8.5 |
| GitPulse | 0.64 | 9.9 | 15.5 |
| orbit-knowledge-graph | 0.67 | 2.3 | 3.4 |

## Unchanged refresh

| Repo | Tool | Wall (s) | Instructions (B) |
|---|---|---|---|
| DevPrism | main 290e355b | 0.07 / 0.08 / 0.19 | 0.81 / 0.83 / 0.86 |
| DevPrism | schema 29 5a4a15e7 | 0.07 / 0.08 / 0.14 | 0.8 / 0.81 / 0.84 |
| DevPrism | Orbit | 0.23 / 0.27 / 1.2 | 1.58 / 1.61 / 1.69 |
| DevCouncil | main 290e355b | 0.2 / 0.25 / 0.59 | 2.44 / 2.44 / 2.7 |
| DevCouncil | schema 29 5a4a15e7 | 0.2 / 0.39 / 1.56 | 2.17 / 2.25 / 2.69 |
| DevCouncil | Orbit | 0.41 / 0.44 / 1.75 | 1.97 / 2.02 / 2.07 |
| GitPulse | main 290e355b | 0.16 / 0.2 / 0.26 | 2.87 / 2.94 / 2.95 |
| GitPulse | schema 29 5a4a15e7 | 0.15 / 0.18 / 0.21 | 2.58 / 2.65 / 2.67 |
| GitPulse | Orbit | 0.28 / 0.28 / 0.34 | 1.84 / 1.86 / 1.86 |
| orbit-knowledge-graph | main 290e355b | 0.21 / 0.24 / 0.57 | 2.86 / 2.91 / 3.35 |
| orbit-knowledge-graph | schema 29 5a4a15e7 | 0.19 / 0.2 / 0.54 | 2.48 / 2.5 / 2.5 |
| orbit-knowledge-graph | Orbit | 0.28 / 0.31 / 0.34 | 1.92 / 1.94 / 1.95 |

## Revert (second incremental build, tree back to the original bytes) and cold

Minimum wall seconds / minimum instructions (B). Cold is one run per tool and was run at load 22.82-57.07.

| Repo | Stage | main | schema 29 | Orbit |
|---|---|---|---|---|
| DevPrism | revert | 0.46 s / 7.74 | 0.36 s / 11.02 | 0.36 s / 2.54 |
| DevPrism | cold | 1.51 s / 83.91 | 1.21 s / 78.11 | 0.84 s / 25.5 |
| DevCouncil | revert | 2.09 s / 27.95 | 2 s / 45.32 | 0.72 s / 5.05 |
| DevCouncil | cold | 30.16 s / 314.48 | 23.68 s / 290.66 | 6.06 s / 85.22 |
| GitPulse | revert | 3.2 s / 55.57 | 2.39 s / 86.52 | 0.58 s / 5.46 |
| GitPulse | cold | 28.62 s / 623.2 | 25.33 s / 574.77 | 3.87 s / 149 |
| orbit-knowledge-graph | revert | 1.59 s / 23.89 | 1.23 s / 35.72 | 0.84 s / 10.1 |
| orbit-knowledge-graph | cold | 10.01 s / 180.26 | 8.5 s / 162.1 | 5.73 s / 48.07 |

## Phase split of the main edit (DevMap build JSON, seconds, minimum / median of 5)

The rep-level phase times are load-skewed; read the ratios, not the seconds. `resolver:index` is a sub-stage of "scanning and extracting".

| Repo | Tool | whole build | scan+extract | of which resolver:index | resolving | analyzing | persisting |
|---|---|---|---|---|---|---|---|
| DevPrism | main | 0.428 / 0.448 | 0.161 / 0.177 | 0.078 / 0.085 | 0.114 / 0.119 | 0.075 / 0.083 | 0.075 / 0.076 |
| DevPrism | s29 | 0.359 / 0.414 | 0.123 / 0.144 | 0.047 / 0.049 | 0.064 / 0.073 | 0.076 / 0.086 | 0.087 / 0.096 |
| DevCouncil | main | 4.504 / 4.844 | 1.011 / 1.739 | 0.421 / 0.446 | 1.584 / 1.769 | 0.286 / 0.38 | 0.76 / 1.139 |
| DevCouncil | s29 | 2.675 / 4.481 | 1.29 / 1.675 | 0.215 / 0.366 | 0.322 / 0.637 | 0.289 / 0.339 | 0.771 / 1.681 |
| GitPulse | main | 3.033 / 5.337 | 1.167 / 1.894 | 0.773 / 0.808 | 0.769 / 1.666 | 0.398 / 0.44 | 0.607 / 1.068 |
| GitPulse | s29 | 2.73 / 4.325 | 0.771 / 1.345 | 0.377 / 0.4 | 0.5 / 0.811 | 0.382 / 0.467 | 0.775 / 1.573 |
| orbit-knowledge-graph | main | 1.589 / 2.638 | 0.689 / 0.784 | 0.33 / 0.344 | 0.338 / 0.418 | 0.194 / 0.21 | 0.241 / 0.351 |
| orbit-knowledge-graph | s29 | 1.196 / 1.769 | 0.452 / 0.625 | 0.164 / 0.193 | 0.245 / 0.342 | 0.212 / 0.269 | 0.286 / 0.359 |

Dominant phase: on main the resolve work (`resolver:index` + `resolving`) is the largest block on all four repos, about 46-48% of the minimum-instruction rep's wall (DevPrism 0.195 of 0.428 s, DevCouncil 2.25 of 4.86 s, GitPulse 1.56 of 3.24 s, orbit-knowledge-graph 0.76 of 1.59 s). Persisting is next (DevCouncil 0.76 s, 16%). On schema 29 the same blocks were about 20-30% of the build. Whole-tree resolve still dominates and this run does not show it shrinking.

CPU against wall, DevCouncil edit, 3 alternating extra runs at load about 25 (not in the tables above):

| Tool | wall (s) | user+sys (s) | user+sys / wall | instructions (B) |
|---|---|---|---|---|
| main | 3.71 | 3.20 | 0.86 | 30.1 |
| s29 | 2.39 | 4.16 | 1.74 | 46.6 |
| main | 2.14 | 2.93 | 1.37 | 29.2 |
| s29 | 1.71 | 4.07 | 2.38 | 46.2 |
| main | 2.29 | 2.90 | 1.27 | 29.1 |
| s29 | 2.67 | 4.19 | 1.57 | 46.3 |

Main retires about 37% fewer instructions and spends about 27% less CPU, but its wall time is no better: its CPU/wall ratio is 0.9-1.4 against 1.6-2.4 on schema 29. The reduced work is therefore less parallel (inferred from the ratio; thread-level profiling was not run). Consistent with that, the resolver phases are longer in wall on main than on schema 29 in every repo despite the lower total instructions. This is the most useful lead from this run: the saving came from phases that were parallel, and what is left (resolve) runs narrower.

## Criteria

| # | Criterion | Verdict | Evidence |
|---|---|---|---|
| 1 | Edit no slower than Orbit's edit (0.19, 0.29, 1.24, 0.58 s), or record the dominant phase | **Wall bar NOT MET; phase recorded.** | Main wall min 0.44 / 4.61 / 3.13 / 1.63 s vs same-session Orbit 0.31 / 1.34 / 0.61 / 1.04 s, at load 20-58. Not certifiable as a quiet-machine result. Dominant phase and why it does not shrink: see the phase table and the CPU/wall table above, and `resolve_all.rs:66-71`: the resolve covers the whole tree on every changed build so liveness and communities mean the same thing on both paths. Main also costs 2.3-9.9x Orbit's instructions. |
| 2 | Same edit retires fewer instructions than schema 29 (DevCouncil 47.3B, GitPulse 91.9B) | **MET** | main min 29.0B (DevCouncil) and 55.5B (GitPulse) vs the 5 Nov 47.3B and 91.9B, and vs 45.6B and 86.7B for schema 29 in this session. DevPrism 7.75B vs 10.96B, orbit-knowledge-graph 23.9B vs 35.7B. Ratio main/schema 29: 0.64-0.71 on all four. |
| 3 | Unchanged refresh at or under the 5 Nov minima (0.04, 0.09, 0.10, 0.09 s) | **NOT MET on wall, not attributable to the change.** | Main refresh min 0.07 / 0.20 / 0.16 / 0.21 s; schema 29 in the same session 0.07 / 0.20 / 0.15 / 0.19 s, so the 5 Nov constants are not reachable on this host at this load, by either build. Instructions: main is +1% to +15% above schema 29 (DevPrism 0.81 vs 0.80, DevCouncil 2.44 vs 2.17, GitPulse 2.87 vs 2.58, orbit-knowledge-graph 2.86 vs 2.48 B). That small instruction increase is real and unexplained here (candidates: extract schema 100, the newer scan path); wall difference is inside noise. |
| 4 | Incremental liveness and communities match a cold build of the same tree | **MET for dead and communities; one statistic differs.** | DevCouncil clone, edit applied, incremental build vs fresh cold build of the same bytes, main binary. `devmap dead --json --budget 100000`: byte-identical (211 items, not truncated, same `walk_incomplete`). `devmap manifest`: `subsystems` (16, the communities), `dead_clusters`, `unwired_candidates` identical. The manifests differ in 7 fields: `freshness.generation_id` (expected) and `resolution_rate` resolved-site counts: total 77,233 incremental vs 77,062 cold (+171, 0.2%), Go 14,336 vs 14,209, Rust 60,410 vs 60,366, Go net permille 781 vs 779. So an incremental build reports slightly more resolved sites than a cold one; it does not change dead or communities in this sample. One repo, one edit. |
| 5 | A test or bench fails against schema 29 before the change | **NOT APPLICABLE** | No code was changed. |

## What I would do next (not done)

The edit is no longer a "retires 3-8x Orbit's instructions" problem on DevPrism and orbit-knowledge-graph (3.0x and 2.3x), but is still 5.4x on DevCouncil and 9.9x on GitPulse (Orbit's GitPulse count depends on the edit file, see item 2). The measured lead is parallelism: main's saving came out of parallel work and the remaining resolve narrows to roughly one to one-and-a-half cores. A parallel resolve over the whole tree would keep the whole-tree semantics that `resolve_all.rs` documents. It needs a quiet host to prove, and the `resolution_rate` incremental-vs-cold difference in criterion 4 should be understood first so a parallel resolve does not hide it.

## Limits

- Load 20-81 throughout; wall times and phase seconds are indicative. Instructions are not.
- Load gate was relaxed from 30 to 60 after the host stayed above 30 for over 40 minutes; no row was kept from a run started at load 60 or higher.
- Edit file differs from 5 Nov and is not the same across tools' dependency cones; Orbit's absolute edit numbers are file-dependent.
- scholarlm (the fifth 5 Nov corpus) was not in scope and was not run.
- Criterion 4 is one repo and one appended function.
