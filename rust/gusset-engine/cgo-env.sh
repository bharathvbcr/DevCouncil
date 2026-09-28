#!/usr/bin/env bash
# Build the umbrella archive and print the cgo environment that links it.
#
#   eval "$(rust/gusset-engine/cgo-env.sh --export)"   # a shell
#   rust/gusset-engine/cgo-env.sh >> "$GITHUB_ENV"      # a CI job
#   rust/gusset-engine/cgo-env.sh --no-build ...        # archive already built
#   rust/gusset-engine/cgo-env.sh --target=x86_64-unknown-linux-musl
#                                    # archive for another target
#
# A *-linux-musl target also prints CC (musl-gcc unless CC is set) and
# GUSSET_STATIC_EXTLDFLAGS, for a fully static binary:
#
#   go build -tags netgo,osusergo \
#     -ldflags "-linkmode external -extldflags '$GUSSET_STATIC_EXTLDFLAGS'" ...
#
# Both parts of those flags are load-bearing, and both failures are silent
# until a panic. musl-gcc on a glibc host otherwise resolves the Rust
# unwinder from the host's libgcc_eh.a, which needs glibc's _dl_find_object:
# the link fails. Linking Rust's own self-contained libunwind.a fixes the
# link, but `gcc -static` omits PT_GNU_EH_FRAME, which that unwinder looks
# frames up through: the binary links, every match works, and the first Rust
# panic aborts the process ("failed to initiate panic, error 5") instead of
# coming back as ErrPanic. That is the I2 firewall gone. gusset-check catches
# it; run it on the static binary.
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
target=""
for arg in "$@"; do
  case "$arg" in
    --export) mode=export ;;
    --no-build) build=0 ;;
    --target=?*) target="${arg#--target=}" ;;
    *) echo "cgo-env.sh: unknown argument $arg" >&2; exit 2 ;;
  esac
done

if [[ ! -f "$here/../../../gusset/crates/gusset/Cargo.toml" ]]; then
  echo "cgo-env.sh: gusset is not checked out next to DevCouncil ($here/../../../gusset)" >&2
  exit 1
fi
# Go splits CGO_LDFLAGS on spaces and gussetfn's #cgo -L uses ${SRCDIR}, which
# cgo refuses to expand to a path with spaces; fail here, by name.
if [[ "$here" == *" "* ]]; then
  echo "cgo-env.sh: $here contains a space, which cgo cannot link from" >&2
  exit 1
fi
if [[ "$build" -eq 1 ]]; then
  # --target-dir pins the output where gussetfn's #cgo -L and the hash below
  # look. Without it an inherited CARGO_TARGET_DIR (two lanes on one
  # checkout) sent the fresh archive elsewhere and this script keyed Go on
  # the stale one it then linked.
  cargo build --release --locked --manifest-path "$here/Cargo.toml" --target-dir "$here/target" \
    ${target:+--target "$target"} >&2
fi
# With --target cargo writes under target/<triple>/release, and the host
# archive in target/release is for a different machine: link only the one
# built for this target.
libdir="$here/target${target:+/$target}/release"
archive="$libdir/libgusset.a"
if [[ ! -f "$archive" ]]; then
  echo "cgo-env.sh: $archive is missing" >&2
  exit 1
fi
if command -v sha256sum >/dev/null 2>&1; then
  sum="$(sha256sum "$archive" | cut -c1-32)"
else
  sum="$(shasum -a 256 "$archive" | cut -c1-32)"
fi

ldflags="-L$libdir"
# Replace, never stack: runtime/cgo builds with -Werror, and a second -D of
# the same macro from re-sourcing this in one shell is a redefinition error.
# Seed from `go env`, which reports the process env, a `go env -w` setting or
# Go's default -O2 -g, in that order; the process env alone missed -w.
current="$(go env CGO_CFLAGS 2>/dev/null || printf '%s' "${CGO_CFLAGS:--O2 -g}")"
base="$(printf '%s' "$current" | sed -E 's/(^| )-DDEVCOUNCIL_GUSSET_ENGINE_SHA256=[^ ]*//g; s/^ +//')"
cflags="${base:+$base }-DDEVCOUNCIL_GUSSET_ENGINE_SHA256=$sum"
static=""
cc=""
if [[ "$target" == *-linux-musl ]]; then
  unwind="$(rustc --print sysroot)/lib/rustlib/$target/lib/self-contained/libunwind.a"
  if [[ ! -f "$unwind" ]]; then
    echo "cgo-env.sh: $unwind is missing; rustup target add $target" >&2
    exit 1
  fi
  if [[ "$unwind" == *" "* ]]; then
    echo "cgo-env.sh: $unwind contains a space, which -extldflags cannot carry" >&2
    exit 1
  fi
  static="-static -Wl,--eh-frame-hdr $unwind"
  cc="${CC:-musl-gcc}"
fi
if [[ "$mode" == export ]]; then
  printf 'export CGO_ENABLED=1\nexport CGO_LDFLAGS=%q\nexport CGO_CFLAGS=%q\n' "$ldflags" "$cflags"
  if [[ -n "$static" ]]; then
    printf 'export CC=%q\nexport GUSSET_STATIC_EXTLDFLAGS=%q\n' "$cc" "$static"
  fi
else
  printf 'CGO_ENABLED=1\nCGO_LDFLAGS=%s\nCGO_CFLAGS=%s\n' "$ldflags" "$cflags"
  if [[ -n "$static" ]]; then
    printf 'CC=%s\nGUSSET_STATIC_EXTLDFLAGS=%s\n' "$cc" "$static"
  fi
fi
