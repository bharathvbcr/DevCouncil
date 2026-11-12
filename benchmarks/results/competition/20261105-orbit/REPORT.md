# DevMap schema 29 against schema 27 and Orbit's local graph

- generated: 2026-11-05
- host: Apple M5 Pro (Mac17,8), 18 cores, 64 GB, macOS 27.2
- statistic: **minimum of 5 interleaved reps** (median and max in `measure.jsonl`)
- load: 1-minute load average 2.65–17.9 across the 300 index samples
- raw data: [`measure.jsonl`](measure.jsonl), [`probes.jsonl`](probes.jsonl), [`query.jsonl`](query.jsonl), [`provenance.json`](provenance.json)

This is the first competition run that includes Orbit. It does not remeasure
CodeGraph, codebase-memory-mcp, GitNexus, Graphify, or Gortex. Those numbers
remain the [v0.2.2 report](../20260914-v0.2.2/REPORT.md), on different corpora
and a different DevMap build. Do not read a cell here against a cell there.

The 29 Oct Orbit comparison timed schema 27. This session times that binary,
schema 29, and the same Orbit CLI on today's trees, so the version delta is
controlled and the Orbit comparison is the same session.

## Arms

| Arm | What ran |
|---|---|
| DevMap schema 29 | `devmap 0.2.5 (store schema 29, code graph schema 2, extract schema 98, build 5a4a15e7a6ff)`, built in scratch from `5a4a15e7` |
| DevMap schema 27 | `devmap 0.2.5 (store schema 27, code graph schema 2, extract schema 98, build 782b9f168833)`, rebuilt from `782b9f16` |
| Orbit | installed `orbit 0.163.0-38-g3d379c2`. `ORBIT_TELEMETRY_ENABLED=false`, state in a scratch `ORBIT_DATA_DIR`. `orbit setup` was not run |

Each arm indexed its own clone. DevMap used `--db` outside the tree and
`--progress never`. Orbit used `workspace index`. No live checkout and no
`~/.gitlab` directory was written.

| Repo | Commit | Tracked files | DevMap indexed |
|---|---|---:|---:|
| DevPrism | `348d14f8` | 933 | 698 |
| DevCouncil | `5a4a15e7` | 3,057 | 2,110 |
| GitPulse | `5394f4d3` | 3,322 | 3,191 |
| orbit-knowledge-graph | `ecfa552d` | 3,331 | 3,102 |
| scholarlm | `982f43a1` | 6,479 | 6,076 |

All 300 timings exited 0. All 75 probes found the appended symbol after the
edit and did not find it after the file was restored to its original bytes.

## Cold, refresh, edit

Minimum seconds. `×N` on the Orbit column is Orbit's time divided by schema 29
when Orbit is faster, and schema 29's time divided by Orbit when schema 29 is
faster. The winner is named.

| Repo | Stage | Schema 29 | Schema 27 | Orbit |
|---|---|---:|---:|---:|
| DevPrism | cold | 0.83 | 0.91 | **0.48** (×1.7) |
| | refresh | **0.04** | 0.04 | 0.13 |
| | edit | 0.28 | 0.41 | **0.19** (×1.5) |
| DevCouncil | cold | 3.18 | 3.41 | **1.31** (×2.4) |
| | refresh | **0.09** | 0.09 | 0.16 |
| | edit | 0.94 | 1.48 | **0.29** (×3.2) |
| GitPulse | cold | 5.82 | 6.30 | **2.12** (×2.7) |
| | refresh | **0.10** | 0.09 | 0.16 |
| | edit | 1.77 | 2.73 | **1.24** (×1.4) |
| orbit-knowledge-graph | cold | 1.83 | 2.11 | **0.87** (×2.1) |
| | refresh | **0.09** | 0.08 | 0.16 |
| | edit | 0.78 | 1.08 | **0.58** (×1.3) |
| scholarlm | cold | **7.39** (×1.45) | 8.60 | 10.70 |
| | refresh | 0.34 | 0.34 | **0.17** |
| | edit | **3.20** (×5.6) | 4.72 | 18.08 |

Schema 29 vs schema 27, from the minima and from instruction counts:

- Cold wall time is 1.07–1.16× faster on every repo. Minimum instruction counts are 1.4–3.6% **higher** on schema 29. The wall-time gain is a smaller write, not less CPU work.
- Unchanged refresh matches on all five repos.
- One-file edit is 1.38–1.57× faster. Revert drops further: DevCouncil 2.08 s to 0.94 s, GitPulse 3.77 s to 1.74 s, scholarlm 6.70 s to 3.20 s.
- On the fastest DevCouncil cold rep, `payloads` inside `persist:write` is 0.02 s against 0.11 s on schema 27's fastest cold rep. That split is one rep, not a five-rep median.

Against this Orbit binary:

- Orbit wins cold on four of five and the one-file edit on four of five.
- Schema 29 wins unchanged refresh on four of five. On scholarlm, Orbit's refresh is faster (0.17 s vs 0.34 s).
- Schema 29 wins scholarlm cold and the scholarlm edit. Orbit's edit there (18.08 s) is slower than its own cold index (10.70 s) in all five reps.

The Orbit binary is the 29 Oct CLI. The file-index fix at `ecfa552` is the
commit of the orbit-knowledge-graph corpus and was not the binary under test.
A 30 Oct measurement of that fix, on a different session, reported the
scholarlm edit at 9.73 s. That number is not in this table.

## Store and memory

Store is `du` of the state directory after the fastest cold rep. RSS is the
maximum of the five cold reps.

| Repo | Schema 29 | Schema 27 | Orbit | Cold RSS, schema 29 | Cold RSS, Orbit |
|---|---:|---:|---:|---:|---:|
| DevPrism | 44 MiB | 94 MiB | 13 MiB | 447 MiB | 515 MiB |
| DevCouncil | 176 MiB | 368 MiB | 39 MiB | 1,267 MiB | 921 MiB |
| GitPulse | 288 MiB | 608 MiB | 69 MiB | 2,317 MiB | 1,365 MiB |
| orbit-knowledge-graph | 96 MiB | 208 MiB | 25 MiB | 859 MiB | 800 MiB |
| scholarlm | 432 MiB | 896 MiB | 91 MiB | 2,982 MiB | 4,515 MiB |

Schema 29's store is 46–48% of schema 27's on every repo, and 3.4–4.7× Orbit's.
Orbit uses less memory on DevCouncil, GitPulse, and its own repository.

## Graph counts

Indexed file counts match between the two DevMap builds. Symbol counts match
on three repos; GitPulse is 54,946 vs 54,945 and orbit-knowledge-graph is
23,115 vs 23,104. Edges are a few tenths of a percent higher on schema 29
(DevCouncil 104,820 vs 104,410). The graphs were not hashed, so this is not an
identity claim. Extraction failures are 3, 1, 0, 0, and 8 files, the same
counts as 29 Oct. Orbit reported 0 skipped and 0 errored on every corpus.
Orbit indexes more files than DevMap because it lists files DevMap does not
parse; the counts are each tool's own.

Direct `Calls` caller counts on eight symbols are identical on the two DevMap
builds: 14, 5, 8, 1, 5, 3, 10, 3. `TaskEditor.svelte:399` (`copyForAgent`
calling `applyNotesToDraft`) is still in DevMap's caller list and absent from
Orbit's `context` output, which lists four connections and no Svelte file.
The other seven symbols were not re-audited against source in this run.

## Queries

Median of five. DevMap is `explore --json`. Orbit is `context`. A cold process
each time.

| Symbol | Schema 29 | Schema 27 | Orbit |
|---|---:|---:|---:|
| literalWrites | 82 ms | 80 ms | 163 ms |
| toolStateCapReason | 80 ms | 91 ms | 163 ms |
| build_deletion_statements | 106 ms | 105 ms | 165 ms |
| check_grammar_bound | 96 ms | 92 ms | 163 ms |
| applyNotesToDraft | 187 ms | 184 ms | 154 ms |
| agentCwdTargets | 235 ms | 233 ms | 155 ms |
| academicPoliteEmail | 258 ms | 280 ms | 175 ms |
| buildDeterministicAttributions | 234 ms | 232 ms | 172 ms |

Four schema 29 vs schema 27 ranges do not overlap, and those gaps are 2–22 ms.
The other four overlap. Query latency is not the schema 29 result. Orbit's
answers are smaller (0.7–5.4 KiB against DevMap's 5.9–26.7 KiB).

## Accuracy of what each index claims

A second pass, after the timing run, rebuilt one index per arm on the same
commits and checked the edges against source. The check is not a compiler
oracle. A call counts as **name-in-span** when the callee's short name occurs
in the caller's stored byte span. That can still be a comment or a type.
**Name-absent** means the span does not contain that name. Those rows were
read.

### DevMap schema 29

`Calls` edges, and how many callee names are missing from the caller span:

| Repo | Calls | Name absent from span | Name absent from the whole file |
|---|---:|---:|---:|
| DevPrism | 9,591 | 24 (0.25%) | 0 |
| DevCouncil | 43,922 | 72 (0.16%) | 12 |
| GitPulse | 71,792 | 999 (1.39%) | 61 |
| orbit-knowledge-graph | 25,503 | 125 (0.49%) | 31 |
| scholarlm | 96,110 | 229 (0.24%) | 77 |

Reading the exceptions:

- Almost every name that is missing from the span is present elsewhere in the same file. The stored span is one `#[cfg]` body and the call is in the other body of the same function. Checked on `pulse_signed_cases` (`command_timings.rs:318`, the `not(unix)` stub) and `group_has_members` (`dc-proc/src/lib.rs:1814`). The call is real. The span you are shown is the other copy.
- Names missing from the whole file are a small set. On DevCouncil, 11 of 12 are implicit `Drop` (`Display.drop`, `OwnedLock.drop`). On GitPulse, 50 of 61 are `Drop`. On scholarlm, 38 of 77 are `Drop`. On orbit-knowledge-graph, 27 of 31 are `?` lowered to `Error.from` at confidence 0.4. One alias was opened: `persistSessionSnapshot` in `persistence.ts:30` calls `captureResearchStorageLease`, and the edge points at `captureAuthSessionLease`, which is that name's alias in `cacheService.ts`. The call is real. A text search for the target name misses it. The other GitPulse leftovers (`initialize`, `apply`, `survey`, `cancel`, `is_cancelled`, `err`; 8 edges) were counted and not each read.
- One UniqueGlobal edge at confidence 0.9, from a test-file header to a symbol named `body`, has no such call in the file. That is a false edge. It is one row, not a class of rows.

Calls whose target is a struct, the bug the October compiler audit measured at 1,304 Go edges on an older DevCouncil and which schema 27 still has:

| Repo | Schema 27 Calls to Struct | Schema 29 |
|---|---:|---:|
| DevPrism | 4 | 0 |
| DevCouncil | 1,606 | 0 |
| GitPulse | 400 | 3 |
| orbit-knowledge-graph | 410 | 0 |
| scholarlm | 19,424 | 0 |

On schema 27 those are 16.8% of scholarlm's Calls (19,424 of 115,486). Schema 29 does not make that claim. Class and Variable targets remain, 644 on scholarlm (0.7% of Calls) and 72 on DevCouncil. They were not each read. They are not the struct-literal class.

In an 80-symbol sample per repo, every definition whose kind was not `File` contained its name in its span, except two Terraform variables whose qualified name is prefixed `variable.`. File nodes use the filename, which is not in the file. That sample does not prove every span.

What this index is good for: "which functions call this one", if you keep `Calls` and you read `confidence` and `walk_incomplete`. A confidence-1 call to a function or method was a real call in every exception that was opened, including aliases and the other `cfg` body. An empty caller list is not "nothing calls this": the index says so when the walk is partial, and it still does not parse Svelte, so `TaskEditor.svelte:399` is a caller only this index returns and only because that file was parsed.

What it is not: a listing of only the calls in the span it shows you, and not a complete call graph. `Drop` edges are destructors the source does not write. Confidence 0.4 includes `?`.

Schema 27 is the wrong index for that question on Go. It reports struct literals as calls, at confidence 1, and nothing in the edge says they are literals.

### Orbit

`CALLS` has no confidence. Targets that are definitions:

| Repo | Function | Method | Struct | Field | Other definition targets | ImportedSymbol targets |
|---|---:|---:|---:|---:|---:|---:|
| DevPrism | 7,567 | 488 | 564 | 125 | 80 | 10,299 |
| DevCouncil | 36,724 | 5,090 | 3,552 | 1,566 | 681 | 30,935 |
| GitPulse | 74,213 | 7,771 | 4,754 | 910 | 1,968 | 47,099 |
| orbit-knowledge-graph | 14,793 | 4,964 | 3,276 | 647 | 30 | 25,681 |
| scholarlm | 87,047 | 16,937 | 190 | 11,844 | 866 | 118,118 |

Opened examples, all stored as `CALLS`:

- Field: `e.Message`, `e.Attempts`, `req.JSON` in `apiclient/client.go`. Those are field reads.
- Struct: `GoAccountDeleteRequest { ... }` in `account.rs:363`, and `&Bundle` / `-> Report` in `dc-evidence/src/lib.rs`. Those are type uses and composite literals, the class schema 29 stopped emitting.
- Struct and field together are 6–11% of Orbit's definition-targeted `CALLS` on these repos (scholarlm 12,034 of 116,884, 10.3%; DevCouncil 5,118 of 47,614, 10.7%).

`ImportedSymbol` targets are a second, larger set. They record a use of a name that did not resolve to a local definition. They are not local callers.

`context` is a different surface from this table. It prints a short definition and a "Connections (N indexed)" list, with tests in their own section. That list is what an agent sees. It does not carry confidence, and it does not say when a language was not parsed. On `applyNotesToDraft` it lists four connections and omits `TaskEditor.svelte:399`.

What this index is good for: reading a definition and the calls it actually indexed, in a small answer, and for a store you can query. A `CALLS` row whose target is a function or method, opened in these examples, was a call. The product around it (the short `context` answer) is the useful part.

What it is not: a caller query. `CALLS` also means field read, type mention, and composite literal, with the same label as a call, and a missing caller is silent.

## Limits

- The tools do not do the same work. DevMap also computes communities, dead-code candidates, an FTS index, and a resolution ledger. Orbit's local index stops at its graph.
- The five other competitors were not remeasured.
- The Orbit file-index fix was not the binary under test.
- The edge check is name-in-span plus the rows that failed it, not a compiler oracle. Class and Variable targets on schema 29 were counted and not each read.
- Minimums are the headline because the machine was shared. Instruction counts are the check that a wall-time gap is work rather than load.
