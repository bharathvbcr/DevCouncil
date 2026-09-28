# What runs on Gusset, and what stays a process

Gusset runs Rust in the Go process: one staticlib per binary (the umbrella in
`rust/gusset-engine`), cooperative cancellation through `JobContext::check`,
and a panic firewall that returns `ErrPanic` and poisons the handle (I2). It
does not make Rust faster; it removes a process boundary. A workload belongs on
it only when that boundary costs more than it buys.

Decided 2026-09-28, with measurements from this repository.

## On Gusset: policy pattern matching

Every pattern question the write, read and command ladders ask goes to dc-glob
through `gussetfn.Matcher` (see `policy/matcher.go`). It qualifies on every
count the others below fail:

- **Hot.** Two or three questions per gate decision, and a decision per agent
  tool call. A process per question would cost more than the question.
- **Bounded.** dc-glob's walk has a step budget and a 16384-rune cap, so a
  question cannot run away; the missing kill switch does not matter.
- **Pure.** No I/O, no child processes, no file descriptors.
- **Honest failure.** A matcher error is a hard denial under
  `path.engine_unavailable` / `command.engine_unavailable`, never a guess.

Cost: about 20 µs per crossing, 63 µs against Go's 23 µs per write decision.

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
between bounded steps, and it has no I/O whose failure needs a process to
contain. Add it to the one umbrella archive; never a second staticlib (R14).
