#!/usr/bin/env bash
# Runs dc-sparse-encode's tests on an Apple-silicon runner, locked.
#
# tessl compiles Metal 4 shaders at build time and the parity test drives a
# real GPU. A runner without that cannot answer the question, and saying so is
# different from passing: this prints NOT RUN and exits 0 only when the Metal
# toolchain, the macOS 26 SDK or a Metal device is absent. Every other failure
# (a compile error, a failing test, a lock mismatch) fails the job.
#
# The probes are commands so a test can stand in for the runner:
#   DC_SPARSE_METAL_PROBE   exits 0 when `xcrun metal` exists
#   DC_SPARSE_SDK_VERSION   prints the macOS SDK version
#   DC_SPARSE_GPU_PROBE     prints system_profiler's display section
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
metal_probe="${DC_SPARSE_METAL_PROBE:-xcrun --find metal}"
sdk_version="${DC_SPARSE_SDK_VERSION:-xcrun --show-sdk-version}"
gpu_probe="${DC_SPARSE_GPU_PROBE:-system_profiler SPDisplaysDataType}"

not_run() {
  echo "NOT RUN: dc-sparse-encode tests: $1"
  echo "::warning::dc-sparse-encode tests NOT RUN: $1"
  exit 0
}

if ! bash -c "$metal_probe" >/dev/null 2>&1; then
  not_run "no Metal toolchain on this runner (xcodebuild -downloadComponent MetalToolchain)"
fi
sdk="$(bash -c "$sdk_version" 2>/dev/null || true)"
major="${sdk%%.*}"
if [[ ! "$major" =~ ^[0-9]+$ ]]; then
  echo "FAIL: could not read the macOS SDK version (got '$sdk')" >&2
  exit 2
fi
if (( major < 26 )); then
  not_run "macOS SDK $sdk is older than 26, which tessl's Metal 4 kernels need"
fi
if ! bash -c "$gpu_probe" 2>/dev/null | grep -qi 'metal'; then
  not_run "no Metal-capable device reported by system_profiler"
fi

cd "$here"
cargo fmt -- --check
cargo test --locked
