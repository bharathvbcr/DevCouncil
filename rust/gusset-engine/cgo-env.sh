#!/usr/bin/env bash
# Build the umbrella archive and print the cgo environment that links it.
#
#   eval "$(rust/gusset-engine/cgo-env.sh --export)"   # a shell
#   rust/gusset-engine/cgo-env.sh >> "$GITHUB_ENV"      # a CI job
#   rust/gusset-engine/cgo-env.sh --no-build ...        # archive already built
#
# Why CGO_CFLAGS carries the archive's hash: Go's build and test caches key a
# cgo package on the text of its flags, not on the bytes of the libraries it
# links. Rebuild libgusset.a and a plain `go build` relinks the old Rust from
# the cache, and `go test` can replay a cached "ok" measured against it. CI
# restores GOCACHE, so the same holds there. A -D naming the hash changes the
# flags whenever the archive changes, which is the only thing Go looks at.
#
# Always runs the incremental cargo build: building only when the archive is
# missing linked whatever an older checkout left behind.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
mode=env
build=1
for arg in "$@"; do
  case "$arg" in
    --export) mode=export ;;
    --no-build) build=0 ;;
    *) echo "cgo-env.sh: unknown argument $arg" >&2; exit 2 ;;
  esac
done

if [[ ! -f "$here/../../../gusset/crates/gusset/Cargo.toml" ]]; then
  echo "cgo-env.sh: gusset is not checked out next to DevCouncil ($here/../../../gusset)" >&2
  exit 1
fi
if [[ "$build" -eq 1 ]]; then
  cargo build --release --locked --manifest-path "$here/Cargo.toml" >&2
fi
archive="$here/target/release/libgusset.a"
if [[ ! -f "$archive" ]]; then
  echo "cgo-env.sh: $archive is missing" >&2
  exit 1
fi
if command -v sha256sum >/dev/null 2>&1; then
  sum="$(sha256sum "$archive" | cut -c1-32)"
else
  sum="$(shasum -a 256 "$archive" | cut -c1-32)"
fi

ldflags="-L$here/target/release"
# Replace, never stack: runtime/cgo builds with -Werror, and a second -D of
# the same macro from re-sourcing this in one shell is a redefinition error.
base="$(printf '%s' "${CGO_CFLAGS:--O2 -g}" | sed -E 's/(^| )-DDEVCOUNCIL_GUSSET_ENGINE_SHA256=[^ ]*//g; s/^ +//')"
cflags="${base:+$base }-DDEVCOUNCIL_GUSSET_ENGINE_SHA256=$sum"
if [[ "$mode" == export ]]; then
  printf 'export CGO_ENABLED=1\nexport CGO_LDFLAGS=%q\nexport CGO_CFLAGS=%q\n' "$ldflags" "$cflags"
else
  printf 'CGO_ENABLED=1\nCGO_LDFLAGS=%s\nCGO_CFLAGS=%s\n' "$ldflags" "$cflags"
fi
