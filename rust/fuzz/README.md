# DevMap Fuzzing Harness (`rust/fuzz`)

Standalone `cargo-fuzz` + AddressSanitizer (ASan) + libFuzzer harness for DevMap's C-grammar, unsafe/zero-copy, and stateful store surfaces.

This crate is a standalone Cargo workspace (`[workspace]` in `rust/fuzz/Cargo.toml`) and is listed in `exclude = ["fuzz"]` in `rust/Cargo.toml` so `rust/Cargo.lock` and all `--workspace --locked` CI jobs stay untouched.

## Targets

| Target | Surface | What It Checks |
|---|---|---|
| `csr_reader` | `devmap_store::csr::CsrGraphReader` / `CsrGraphWriter` | Raw untrusted bytes + structured header/offset/row-boundary mutations under ASan; asserts zero panics, zero OOB/misaligned reads, and exact round-trip fidelity for writer output. |
| `extract_parse_repair` | `devmap_extract::extract_file` + `parse_repair` (all 33 linked C/C++ grammars) | Feeds `stateful_fuzz::Source::from_bytes` into all linked grammars with raw, structure-aware, and repair-triggering inputs; asserts span UTF-8 char boundaries, non-empty failure reasons, zero fabricated calls on `Failed`/`Fallback`/`Skipped`, bounded latency, and byte-for-byte determinism. |
| `stateful_incremental_store` | `devmap_store::Store` + `ResolveReuse` + `prune_generations_except_latest` | Dual-driver stateful fuzzer feeding libFuzzer `&[u8]` into `stateful_fuzz::Source::from_bytes` to interleave file mutations, full/partial differential saves, reuse resolution, pruning, and disk reopen against a cold-build oracle. |
| `stateful_stacked_store` | `devmap_query::StackedStore` + `devmap_store::DeltaStore` | Dual-driver stateful fuzzer feeding libFuzzer `&[u8]` into `stateful_fuzz::Source::from_bytes` to interleave delta upserts, tombstones, reverts, overlay reloads, and queries against a cold flat-store oracle. |

## Prerequisites

```bash
rustup toolchain install nightly --profile minimal --component llvm-tools-preview
cargo install cargo-fuzz --version 0.13.2 --locked
```

> **Note:** AddressSanitizer + libFuzzer is best supported on Linux (`x86_64-unknown-linux-gnu`) and macOS (`aarch64-apple-darwin` / `x86_64-apple-darwin`). The nightly CI workflow `.github/workflows/fuzz.yml` runs all four targets on `ubuntu-latest`.

## Running Campaigns Locally

Run from `rust/fuzz/`:

```bash
cd rust/fuzz

# 1. Coverage-guided + ASan + value profiling (cmplog-style comparison feedback)
ASAN_OPTIONS=detect_leaks=0 \
cargo +nightly fuzz run --fuzz-dir . --sanitizer=address csr_reader -- \
  -max_total_time=120 -max_len=65536 -timeout=25 -use_value_profile=1

# 2. All 33 tree-sitter C/C++ grammars + parse_repair under ASan
ASAN_OPTIONS=detect_leaks=0 \
cargo +nightly fuzz run --fuzz-dir . --sanitizer=address extract_parse_repair -- \
  -max_total_time=300 -max_len=65536 -timeout=25 -use_value_profile=1

# 3. Dual-driver stateful incremental Store + ResolveReuse + prune
ASAN_OPTIONS=detect_leaks=0 \
cargo +nightly fuzz run --fuzz-dir . --sanitizer=address stateful_incremental_store -- \
  -max_total_time=300 -max_len=16384 -timeout=25 -use_value_profile=1

# 4. Dual-driver stateful StackedStore / DeltaStore
ASAN_OPTIONS=detect_leaks=0 \
cargo +nightly fuzz run --fuzz-dir . --sanitizer=address stateful_stacked_store -- \
  -max_total_time=300 -max_len=16384 -timeout=25 -use_value_profile=1
```

### Multi-core + CmpLog Tips

- Pass `-use_value_profile=1` to libFuzzer for comparison-operand feedback (similar to cmplog) on magic bytes, header fields, and syntax keywords.
- Pass `-jobs=4 -workers=4` to run parallel workers sharing the corpus directory.
- To run with UndefinedBehaviorSanitizer alongside ASan on Linux, pass `RUSTFLAGS="-Zsanitizer=address"`.

## Replaying & Minimizing Saved Artifacts

When a campaign finds a crash, libFuzzer writes the triggering input to `rust/fuzz/artifacts/<target>/crash-<sha1>` (or CI uploads it in `fuzz-crashes-<target>`).

```bash
cd rust/fuzz

# Replay a single artifact immediately
cargo +nightly fuzz run --fuzz-dir . --sanitizer=address <target> artifacts/<target>/crash-<id>

# Minimize a crash artifact to the smallest input that still reproduces
cargo +nightly fuzz tmin --fuzz-dir . --sanitizer=address <target> artifacts/<target>/crash-<id>
```

Stateful targets print a stable `DEVMAP_FUZZ_SIG=<oracle>|<action>` line before panicking so crashes can be grouped, deduplicated, and bisected cleanly.

## Signature-Based `git bisect`

Plain `git bisect` breaks when older commits fail to compile or fail for a *different* reason than the crash you are hunting. A signature-preserving bisect workflow distinguishes:

- **exit 0**: target ran cleanly (or did not trigger the target signature) → `good`
- **exit 1**: target reproduced the *exact* expected failure signature (`DEVMAP_FUZZ_SIG=...` or ASan frame) → `bad`
- **exit 125**: build failure or unrelated crash → `skip`

Example helper script (save outside the repo, e.g. `/tmp/bisect-fuzz.sh`, and `chmod +x`):

```bash
#!/usr/bin/env bash
set -euo pipefail

TARGET="${1:?usage: bisect-fuzz.sh <target> <artifact-path> <expected-signature>}"
ARTIFACT="${2:?missing artifact path}"
EXPECTED_SIG="${3:?missing expected signature}"

cd rust/fuzz

# 1. Build the fuzz binary; skip commits that do not compile (exit 125).
if ! cargo +nightly fuzz build --fuzz-dir . --sanitizer=address "$TARGET" >/tmp/bisect-build.log 2>&1; then
  exit 125
fi

# 2. Replay the exact artifact and capture output.
set +e
ASAN_OPTIONS=detect_leaks=0 RUST_BACKTRACE=1 \
  cargo +nightly fuzz run --fuzz-dir . --sanitizer=address "$TARGET" "$ARTIFACT" \
  >/tmp/bisect-run.log 2>&1
rc=$?
set -e

if [[ "$rc" -eq 0 ]]; then
  exit 0
fi

# 3. Only mark `bad` when the expected signature matches; skip unrelated crashes.
if grep -Fq "$EXPECTED_SIG" /tmp/bisect-run.log; then
  exit 1
fi

exit 125
```

Run the bisect from the repository root:

```bash
git bisect start HEAD <known-good-rev>
git bisect run /tmp/bisect-fuzz.sh stateful_incremental_store \
  /abs/path/to/crash-artifact \
  "DEVMAP_FUZZ_SIG=cold_equivalence|save_partial"
git bisect reset
```
