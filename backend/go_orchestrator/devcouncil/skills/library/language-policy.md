---
name: language-policy
title: Language & Runtime Policy (Rust/Go first, native UI)
description: Before choosing a language, adding a module, service or Python package, porting code, or touching a hot path, apply the house language policy — Rust or Go for new code, Python only as a reference oracle or thin binding over a Rust core, Rust-backed packages (Polars over pandas) where Python stays, gusset when Go hosts a Rust engine in-process, and Swift/Kotlin for native app shells.
always: true
source: Owner's standing engineering policy, with the gusset fit thresholds taken from gusset's docs/choosing.md.
triggers:
  keywords: [python, pandas, polars, numpy, pip, poetry, rewrite, port, "new module", "new service", "new crate", performance, latency, throughput, "hot path", kernel, simd, gpu, ffi, cgo, pyo3, maturin, ctypes, binding, interop, gusset, tessl, uniffi, swift, kotlin, flutter, "react native", electron]
  globs: ["*.py", "pyproject.toml", "requirements*.txt", "setup.py", "go.mod", "Cargo.toml", "*.swift", "Package.swift", "*.kt", "build.gradle.kts"]
---

# Language & Runtime Policy

This applies to every repository unless that repository's own instructions say
otherwise. It decides *which language* new code is written in; the domain skills
(backend, systems, ios, android, …) then decide *how* to write it well.

## 1. New code is Rust or Go

- **Rust** for engines and anything CPU-, memory- or latency-bound: parsers,
  indexes, search, graph walks, codecs, numeric and GPU kernels, storage layers.
- **Go** for services, orchestration, CLIs, daemons and network-facing
  concurrency, where the runtime's scheduler and tooling are the point.
- Prefer a focused package or kernel of your own over a heavy general-purpose
  dependency on a hot path, even when it is more work: own the data layout, the
  allocation pattern and the bounds. Ship it with a reference oracle, parity
  tests and a committed benchmark. Adding any third-party dependency still needs
  the owner's approval.

## 2. Python is minimized, not banned

Python is permitted only as:

1. **A reference oracle or fixture generator** that Rust/Go tests compare
   against — e.g. tessl's `tools/qwen35_ref/` producing the numbers its kernels
   must match.
2. **A thin binding over a Rust core** when a Python ecosystem must consume it:
   `ctypes` over the crate's C ABI (as `tessl_torch` does over `src/capi.rs`) or
   PyO3/maturin. The binding marshals; the Rust crate computes.
3. **Throwaway analysis** (notebooks, one-off scripts) that never ships.

Never as the implementation of a shipped hot path, service, CLI or daemon.

Existing Python is not rewritten unasked — that is a separate change with its
own review. When you touch Python that is on a hot path or is a measured
bottleneck, say so and propose the Rust/Go port with a benchmark. When the owner
agrees, port it and **delete the Python in the same change** rather than leaving
two implementations to drift.

Where a repository is Python-first by necessity (PyTorch training, a
scientific stack with no Rust/Go equivalent), keep Python as orchestration and
push the inner loops into Rust kernels — tessl for Apple-silicon GEMM/NN work, or
a purpose-built crate — exposed through the binding forms above.

### Rust-backed packages over pure-Python ones

Where Python stays, its packages should do their work in Rust. When you touch
code that leans on a pure-Python package, propose the Rust-backed equivalent; when
none exists, propose porting it to Rust or writing a focused crate of your own
with a thin binding. Examples (confirm each one's current status and API before
proposing it, and ask before adding any dependency):

| Instead of | Prefer |
| --- | --- |
| pandas | Polars |
| pip / poetry / virtualenv | uv |
| flake8 / black / isort | ruff |
| `json` on a hot path | orjson |
| local PySpark / pandas SQL | DataFusion (Python bindings) |

A dataframe or text-processing step that is still slow on a Rust-backed library
is a candidate for moving out of Python entirely, not for tuning in place.

## 3. Rust and Go in one system

Pick the boundary by where the two run, then by how much work each call does.

| Situation | Use |
| --- | --- |
| Go service hosting a Rust engine **in-process**, calls doing ~hundreds of µs or more, concurrency above `GOMAXPROCS` | **gusset** (`github.com/bharathvbcr/gusset`): panic firewall, bounded pool, deadlines, poisoned handles, ABI verification. Follow its `docs/adoption.md`. |
| In-process call doing **under ~10 µs** of work | Not gusset — its fixed per-call cost is the workload. Write it in Go, or use raw cgo. Check gusset's `docs/choosing.md` table before deciding. |
| Separately versioned binaries / processes | A documented process contract (JSON on stdio, as DevCouncil's components and MANVI use). gusset is **not** an IPC transport; `gusset-ipc` for GPU driver-fault isolation is planned, not shipped — confirm its status in `docs/ipc.md` before relying on it. |

Never let a Rust panic, an unbounded thread count, or an unverified struct
layout cross the boundary by hand-rolling cgo where gusset's contract applies.

## 4. Native apps use the platform's native language

- **Apple platforms:** Swift (SwiftUI, Swift Concurrency).
- **Android:** Kotlin (Jetpack Compose, coroutines).
- **Windows:** the native stack (C#/WinUI 3 on the Windows App SDK, or C++/WinRT).
- **Linux desktop:** Rust with GTK 4 / libadwaita bindings.
- **Web:** TypeScript only for the UI layer; compute goes to a Rust/Go backend or
  Rust compiled to WebAssembly.

Logic shared across platforms lives once in a **Rust core**, and each native
shell calls it through generated bindings (e.g. UniFFI for Swift/Kotlin) or a
C ABI. Verify the binding tool's current version and docs before using it.

Do not start a new app on a cross-platform UI framework (Flutter, React Native,
Electron). Existing Tauri apps keep Tauri, with logic in Rust and the web layer
as UI only. For a *new* cross-platform desktop app, ask the owner before choosing
anything other than native-per-platform.

Platform-mandated languages are fine where the platform requires them: Gradle
Kotlin DSL, Xcode build phases, shell glue, SQL, shader languages (Metal, WGSL,
HLSL).

## 5. Reach for the house toolbox first

| Need | Tool |
| --- | --- |
| Where a symbol lives, callers, blast radius, dead code, affected tests | **DevMap** (`devmap_*` MCP tools or the `devmap` CLI). GitNexus is retired; do not install or consult it. |
| What else is live in this repo: worktrees, agent sessions, contended files | **GitPulse Insights** (`gitpulse_insights`, `gitpulse_collision_risk`) |
| Go hosting a Rust engine | **gusset** |
| GEMM / NN kernels on Apple silicon | **tessl** (Rust, Metal 4; C ABI in `src/capi.rs`) |
| An embeddable agent harness | **MANVI** (Go + Rust, zero-cgo, component contract over stdio) |

## 6. Performance claims need evidence

- Benchmark before and after with the language's own harness (`cargo bench`
  with criterion/divan, `go test -bench` + `benchstat`). Interleave A/B runs and
  report min-of-N; a sequential before/after run is not a comparison.
- Keep a parity test against the reference oracle for every kernel you write.
- Profile before optimizing (Instruments, `samply`, `pprof`), and bound every
  allocation, batch and fan-out on the hot path.
- Do not report a speedup without the committed benchmark that shows it.

## What to record before coding

- The language chosen for each new component and the one-line reason.
- For any Rust↔Go boundary: in-process (gusset or raw cgo, with the per-call
  work estimate) or out-of-process (which contract).
- For any Python you add or keep: which of the three permitted roles it plays.
