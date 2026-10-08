# What runs on Gusset, and what stays a process

Gusset runs Rust in the Go process: one staticlib per binary (the umbrella in
`rust/gusset-engine`), cooperative cancellation through `JobContext::check`,
and a panic firewall that returns `ErrPanic` and poisons the handle (I2). It
does not make Rust faster; it removes a process boundary. A workload belongs on
it only when that boundary costs more than it buys.

Decided 2026-09-28, with measurements from this repository. Policy matching
was revisited on 2026-10-08 and moved back to Go.

## Stays in Go: policy pattern matching

fnmatch (`backend/go_orchestrator/fnmatch`) is the one matcher every policy
decision is made with. No host hands its gates a matcher: devcouncil's
`openGate` and Manvi's `buildGate` and `serve` policy checks all leave
`Matcher` nil, which is `policy.GoMatcher`. Policy matching went to dc-glob
through Gusset on 2026-09-28. That left two matchers live in production, the
engine and a 250 ms fnmatch fallback behind it, and the engine was the slower
of the two.

### The measurement

`policy.BenchmarkDecisionAB` (`policy/zengine_test.go`, `-tags gussetengine`)
is the committed A/B. It runs the production write gate (hard rules,
neighbour and same-directory scope on, as the flag defaults have them) over
`policy/testdata/write_targets.txt`. That file holds the 349 paths this
repository's last 120 commits actually wrote. The A/B also has three
matcher-only sides:

- the exact pattern questions those decisions asked, replayed on each matcher;
- one trivial crossing per decision;
- each decision's questions batched into a single crossing.

Rounds are interleaved and rotate which side goes first. Each side reports its
fastest pass, a min of 105.

```
go test -tags gussetengine -run '^$' -bench DecisionAB -benchtime 1x ./policy
```

Three runs on 2026-10-08, Apple M5 Pro, Go 1.27.1, ns per write decision:

| side | run 1 | run 2 | run 3 |
| --- | ---: | ---: | ---: |
| decision, fnmatch | 60 684 | 59 590 | 62 007 |
| decision, engine | 109 391 | 91 579 | 111 900 |
| its 4.99 questions, fnmatch | 12 064 | 11 747 | 12 730 |
| its 4.99 questions, engine | 30 241 | 25 025 | 29 446 |
| the same questions batched, engine | 17 249 | 17 038 | 18 054 |
| one trivial crossing | 1 601 | 1 493 | 1 496 |

Most of a decision is neither matcher: path normalisation's `EvalSymlinks`
walk dominates the Go profile. On the questions themselves, the engine costs
2.1–2.5× fnmatch.

The earlier figures ("about 20 µs per crossing, 63 µs against Go's 23 µs per
write decision") came from the sequential `BenchmarkDecision` this replaces.
It had one synthetic path, ran each side in turn and took no minimum. The
crossing is about 1.5 µs, not 20.

### Why not batch (option b)

Measured, and it loses. The batched side sends all of a decision's questions
across in one crossing, as one case-folded list against the path every
question names. The A/B refuses to run if a decision's questions name
different paths. That is a lower bound on any batched opcode. It pays one
crossing, and MatchAny stops at the first hit, where a real batch would have
to answer every question. It still costs 17.0–18.1 µs against fnmatch's
11.7–12.7 µs for the same questions, 1.42–1.45×.

The crossings are not the cost. The cost is frame encoding and dc-glob's
matching, most of it in the case-folded secret and protected lists (19 and 24
patterns). A batch would also have to ask the ladder's questions before
knowing which rung returns, against `EvaluateFileChange`'s order contract.
gusset's own guide says the same (`gusset/docs/choosing.md`: under ~10 µs of
work per call, "don't"), and each of these questions is a few µs.

### Tried: making the engine win

Before settling on fnmatch, the engine path was rebuilt to give it its best
case. The prototype is on branch `spike/prepared-engine` (`60ed2ef3`), with
its own tests and benchmarks, and is not merged.

- dc-glob compiles a pattern once.
- The umbrella registers each pattern list once (`OPCODE_PREPARE`) and answers
  one name against many lists in a single crossing (`OPCODE_MATCH_PREPARED`).
- gussetfn keeps a content-keyed cache of prepared lists.

Back to back, a decision's questions then took 4.3 µs against fnmatch's
14.5 µs, 0.30×. Production does not ask back to back, though. A gate decides
once per agent tool call, after the process has been idle, and every such
call has to wake gusset's worker and completion reader. Both spin for only
50 µs (`WORKER_SPIN`, `ticketReaderSpin`) and then park.
`BenchmarkIsolatedDecisionAB` (`policy/zengine_test.go`) measures that shape:
the whole write decision, a sleep before each sample, 300 decisions a side.
`BenchmarkIsolatedBatchAB` on the prototype branch measures the questions
alone.

| isolated, after a sleep of | fnmatch p50 / p90 | engine p50 / p90 |
| --- | ---: | ---: |
| whole decision, 1 ms (two runs) | 86–122 / 124–201 µs | 159–213 / 247–344 µs |
| whole decision, 10 ms (two runs) | 136–181 / 195–270 µs | 245–295 / 362–428 µs |
| questions only, prepared and batched, 1 ms | 17–19 / 28–35 µs | 48–50 / 74–81 µs |
| questions only, prepared and batched, 10 ms | 21–23 / 47–59 µs | 84–89 / 113–123 µs |

Even the best engine path loses about 3× on the questions, and that carries
into every decision. Gusset cannot close the gap within its own rules:
- A longer spin window would only help if it outlasted the gap between tool
  calls, which means burning a core continuously.
- A call that runs on the caller's thread would avoid the wake, but gusset's
  R8 forbids it ("heavy work runs on Rust-spawned threads; the cgo call only
  submits and returns"), and its guide sends work under ~10 µs to Go or raw
  cgo.

Policy matching is a few microseconds of work, asked sparsely, so it is the
case gusset says not to take.

### What the engine still does

- **Oracle.** dc-glob is the matcher dc-verify links, and the Gusset bridge is
  how its answers are held equal to fnmatch. Under `-tags gussetengine`,
  `rust/verify.sh` runs the whole policy suite with the engine as the default
  matcher (`policy/zengine_test.go`): adversarial, hardening and fuzz seeds,
  plus `TestEngineDecisionsEqualGoDecisions`. The oracle there never falls
  back to fnmatch, because a fallback would compare fnmatch with itself. A
  question past its 30 s bound is an error, and an error is a decision no Go
  decision equals (`TestOracleNeverAnswersWithFnmatch`).
- **gusset-check.** devcouncil, Manvi and GitPulse still link the umbrella
  archive, and `gusset-check` still proves it loads, matches and contains a
  panic (I2).

Removed with this decision: `gussetfn.Matcher` (with its `DefaultTimeout`
fallback and `Fallbacks` counter), devcouncil's `policyMatcher`, Manvi's
`gussetcheck.Matcher` and `gussetcheck.Ready`, and `serve`'s `requireGusset`.
The last of these refused every policy check with `E_INTERNAL` when the engine
failed its check. The rule IDs `path.engine_unavailable` and
`command.engine_unavailable` stay in the verdict contract. A gate given a
matcher that fails, as the oracle can, still denies under them.

Hosts keep linking the archive. devcouncil, Manvi, GitPulse and Jarvis keep
`gusset-check`, by the owner's decision on 2026-10-08, so the oracle and the
check stay exercised against the archive each host ships. A failed opcode
registration is now refused at open, never left to the wrong decoder:
`devcouncil_gusset_init` returns how many opcodes failed, and
`gussetfn.engine` refuses to open a handle on a non-zero count.

## Stays a process: `dcverify`

Considered and rejected.

- **Cold.** One `Check` per verification run (`devcouncil/verify/rigor.go`),
  alongside `git` and the test suite, which take seconds. Spawning the binary
  costs about 3 ms (measured: 50 runs of a debug build). Moving it in-process
  saves a rounding error.
- **Unbounded input.** It parses whatever diff and coverage profile the
  repository produces. As a process it runs under `proc` group control with
  output and stderr bounds and can be killed. On Gusset, cancellation is
  cooperative; an engine stuck between `check()` calls cannot be reclaimed, and
  `Shutdown` can only leave it to process exit.
- **Chosen independence.** `RigorClient` refuses a `dcverify` inside the
  repository under analysis (`proc.LookPathOutside`), and the binary is
  installed and upgraded separately (`devcouncil install --only=dcverify`) and
  used by hosts other than this one. Linking it would couple its release to
  every host's.

## Stays a process: `dcgrep`, `dcstore`, `devmap`

- **`dcgrep`** is per call, like matching, but the call is a search: a median
  of 343 ms for one regex over this repository (debug build) against the same
  ~3 ms spawn. The boundary is at most 1% of the call, and a search over an
  unexpectedly large tree is exactly what needs a kill switch.
- **`dcstore`** is I/O-bound and already amortises its process with a `serve`
  session answering many requests over one pipe.
- **`devmap`** is a long-lived server with its own index and memory ceiling;
  in-process it would share the host's heap and lose its isolation for no
  latency gain.

## When to revisit

A candidate earns a Gusset opcode when it is called often enough that a spawn is
a real fraction of each call, its work is bounded or checks its `JobContext`
between bounded steps, it has no I/O whose failure needs a process to contain,
and its caller has an answer for "the engine is busy" that is not a failure.
It also has to be faster in Rust than in Go on the real work, measured with an
interleaved A/B over real inputs. Policy matching met every other condition
and failed this one. Add it to the one umbrella archive; never a second
staticlib (R14).
