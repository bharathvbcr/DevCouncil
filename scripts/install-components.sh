#!/usr/bin/env bash
# Build and install DevCouncil's analysis components.
#
# DevCouncil owns the components; a harness such as MANVI resolves each one as a
# binary and links none of them. This script produces those binaries and puts
# them somewhere a harness will find them.
#
#   dcstore    tasks and the lease that makes concurrent building safe
#   dcverify   unified-diff parsing, scope classification, rigor gates, coverage
#   dcgrep     ignore-aware repository search on ripgrep's engine
#   devmap     the code-intelligence graph (built from rust-port/)
#
# Usage:
#   bash scripts/install-components.sh                 # install all four
#   bash scripts/install-components.sh dcstore dcgrep  # install a subset
#   PREFIX=/usr/local bash scripts/install-components.sh
#   DRY_RUN=1 bash scripts/install-components.sh       # build and verify only
#
# Environment:
#   PREFIX    install root; binaries go to $PREFIX/bin (default: ~/.local)
#   PROFILE   cargo profile: release or debug (default: release)
#   DRY_RUN   set to 1 to build and health-check without installing
#
# Release is the default deliberately. A debug build of the verifier is slow
# enough to change how a turn feels, and a harness that prefers an installed
# component over a local build will pick this one up for every run.

set -euo pipefail

repo_root="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
cd "$repo_root"

PREFIX="${PREFIX:-$HOME/.local}"
PROFILE="${PROFILE:-release}"
DRY_RUN="${DRY_RUN:-}"
bindir="$PREFIX/bin"

case "$PROFILE" in
  release) profile_flag=(--release) ;;
  debug)   profile_flag=() ;;
  *) echo "PROFILE must be 'release' or 'debug', got '$PROFILE'" >&2; exit 2 ;;
esac

# Which workspace each component is built from. devmap is a separate workspace
# because it carries ~36 tree-sitter grammars, and folding it in would make
# every dc-verify test compile them.
component_workspace() {
  case "$1" in
    dcstore|dcverify|dcgrep) echo "rust" ;;
    devmap)                  echo "rust-port" ;;
    *) return 1 ;;
  esac
}

component_package() {
  case "$1" in
    dcstore)  echo "dc-store" ;;
    dcverify) echo "dc-verify" ;;
    dcgrep)   echo "dc-grep" ;;
    devmap)   echo "devmap-cli" ;;
    *) return 1 ;;
  esac
}

ALL=(dcstore dcverify dcgrep devmap)
if [ "$#" -gt 0 ]; then
  requested=("$@")
  for c in "${requested[@]}"; do
    if ! component_workspace "$c" >/dev/null 2>&1; then
      echo "unknown component '$c' (known: ${ALL[*]})" >&2
      exit 2
    fi
  done
else
  requested=("${ALL[@]}")
fi

command -v cargo >/dev/null 2>&1 || {
  echo "cargo is not on PATH; install a stable Rust toolchain first" >&2
  exit 1
}

echo "profile: $PROFILE"
echo "prefix:  $PREFIX"
echo

for name in "${requested[@]}"; do
  ws="$(component_workspace "$name")"
  pkg="$(component_package "$name")"
  echo "building $name  ($ws, package $pkg)"
  ( cd "$ws" && cargo build "${profile_flag[@]}" -p "$pkg" --bin "$name" )
done

echo

# Health-check every built binary before installing any of them.
#
# An install that replaces a working component with one that cannot answer is
# worse than no install: the harness would degrade every gate that component
# backs, and the operator would have no reason to suspect the binary they just
# installed. So the check runs first, against the build output, and a failure
# here stops the run with the old binaries still in place.
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

for name in "${requested[@]}"; do
  ws="$(component_workspace "$name")"
  built="$ws/target/${PROFILE}/$name"
  [ -x "$built" ] || { echo "expected $built to exist after the build" >&2; exit 1; }

  case "$name" in
    dcstore)
      "$built" --db "$tmp/state.sqlite" ready >/dev/null
      reply="$("$built" --db "$tmp/state.sqlite" health)"
      # The exclusion index is the lease's mutual exclusion. A store that will
      # not assert it is a store two builders can both win a task from, and an
      # older build omits the key entirely rather than answering false.
      case "$reply" in
        *'"exclusion_index":"verified"'*) ;;
        *) echo "dcstore health did not verify the exclusion index: $reply" >&2; exit 1 ;;
      esac
      ;;
    dcverify)
      reply="$("$built" health)"
      case "$reply" in
        *'"verifier":"dc-verify"'*) ;;
        *) echo "dcverify did not identify itself: $reply" >&2; exit 1 ;;
      esac
      ;;
    dcgrep)
      reply="$("$built" health 2>&1)"
      case "$reply" in
        *'"ok":true'*) ;;
        *) echo "dcgrep health failed: $reply" >&2; exit 1 ;;
      esac
      ;;
    devmap)
      "$built" --version >/dev/null
      ;;
  esac
  echo "  $name: healthy"
done

if [ -n "$DRY_RUN" ]; then
  echo
  echo "DRY_RUN set — built and health-checked, nothing installed."
  exit 0
fi

echo
mkdir -p "$bindir"
for name in "${requested[@]}"; do
  ws="$(component_workspace "$name")"
  built="$ws/target/${PROFILE}/$name"
  dest="$bindir/$name"

  # Copy then rename, rather than writing over the destination.
  #
  # `cp` onto a path a process is currently executing can fail outright, and on
  # the platforms where it does not, it rewrites the file underneath that
  # process. `mv` within one filesystem is a rename: it swaps the directory
  # entry, so anything already running keeps the inode it started with and the
  # next exec gets the new binary whole.
  cp "$built" "$dest.new"
  chmod 755 "$dest.new"
  mv -f "$dest.new" "$dest"
  echo "installed $name -> $dest"
done

echo
resolved_elsewhere=0
for name in "${requested[@]}"; do
  found="$(command -v "$name" 2>/dev/null || true)"
  if [ -z "$found" ]; then
    echo "note: $name is not on PATH — add $bindir to PATH"
    resolved_elsewhere=1
  elif [ "$found" != "$bindir/$name" ]; then
    # Naming this matters: an older copy earlier in PATH shadows what was just
    # installed, and every symptom afterwards looks like the new build is
    # broken.
    echo "warning: $name resolves to $found, not $bindir/$name — an earlier PATH entry shadows this install"
    resolved_elsewhere=1
  fi
done

if [ "$resolved_elsewhere" -eq 0 ]; then
  echo "all installed components resolve from $bindir"
fi
