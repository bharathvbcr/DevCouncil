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
#   devmap     the code-intelligence graph (built from rust/)
#
# Usage:
#   bash scripts/install-components.sh                 # install all four
#   bash scripts/install-components.sh dcstore dcgrep  # install a subset
#   bash scripts/install-components.sh --only=devmap   # standalone devmap
#   PREFIX=/usr/local bash scripts/install-components.sh
#   DRY_RUN=1 bash scripts/install-components.sh       # build and verify only
#   bash scripts/install-components.sh --help
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

usage() {
  cat <<'EOF'
Usage: bash scripts/install-components.sh [names…] [options]

Build and install DevCouncil analysis binaries (not the Go host).

Components: dcstore  dcverify  dcgrep  devmap
Preset:     analysis (all four)

Options:
  --only NAME     install only NAME (repeatable)
  --list          print component names and exit
  --uninstall     remove named binaries from $PREFIX/bin
  --all           with --uninstall, remove every analysis binary
  --disable NAME  keep the binary, record it as skipped
  --enable NAME   clear a disable mark
  --yes           skip uninstall confirmation
  --dry-run       build and health-check without installing (or print rm)
  --prefix DIR    install root (or set PREFIX)
  -h, --help      this help

Standalone code intelligence:
  bash scripts/install-components.sh devmap

The Go host is a separate binary. Use scripts/install.sh (or
`devcouncil install` once the host is on PATH) when you want it too.
There is no uv / Python install path.
EOF
}

repo_root="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
cd "$repo_root"

PREFIX="${PREFIX:-$HOME/.local}"
PROFILE="${PROFILE:-release}"
DRY_RUN="${DRY_RUN:-}"
ACTION=install
YES=0
LIST=0
WANT_ALL=0
DISABLE_NAME=""
ENABLE_NAME=""
POS=()

while [ "$#" -gt 0 ]; do
  case "$1" in
    -h|--help) usage; exit 0 ;;
    --list) LIST=1; shift ;;
    --uninstall) ACTION=uninstall; shift ;;
    --all) WANT_ALL=1; shift ;;
    --disable)
      if [ "$#" -lt 2 ] || [ -z "$2" ]; then
        echo "--disable needs a component name (see --help)" >&2
        exit 2
      fi
      DISABLE_NAME="$2"; shift 2
      ;;
    --disable=*)
      DISABLE_NAME="${1#--disable=}"
      if [ -z "$DISABLE_NAME" ]; then
        echo "--disable needs a component name (see --help)" >&2
        exit 2
      fi
      shift
      ;;
    --enable)
      if [ "$#" -lt 2 ] || [ -z "$2" ]; then
        echo "--enable needs a component name (see --help)" >&2
        exit 2
      fi
      ENABLE_NAME="$2"; shift 2
      ;;
    --enable=*)
      ENABLE_NAME="${1#--enable=}"
      if [ -z "$ENABLE_NAME" ]; then
        echo "--enable needs a component name (see --help)" >&2
        exit 2
      fi
      shift
      ;;
    --yes|-y) YES=1; shift ;;
    --dry-run) DRY_RUN=1; shift ;;
    --prefix)
      if [ "$#" -lt 2 ] || [ -z "$2" ]; then
        echo "--prefix needs a directory (see --help)" >&2
        exit 2
      fi
      PREFIX="$2"; shift 2
      ;;
    --prefix=*)
      PREFIX="${1#--prefix=}"
      if [ -z "$PREFIX" ]; then
        echo "--prefix needs a directory (see --help)" >&2
        exit 2
      fi
      shift
      ;;
    --only)
      if [ "$#" -lt 2 ] || [ -z "$2" ]; then
        echo "--only needs a component name (see --help)" >&2
        exit 2
      fi
      POS+=("$2"); shift 2
      ;;
    --only=*)
      only="${1#--only=}"
      if [ -z "$only" ]; then
        echo "--only needs a component name (see --help)" >&2
        exit 2
      fi
      POS+=("$only")
      shift
      ;;
    --) shift; break ;;
    -*)
      echo "unknown option: $1 (see --help)" >&2
      exit 2
      ;;
    *) POS+=("$1"); shift ;;
  esac
done
while [ "$#" -gt 0 ]; do POS+=("$1"); shift; done

bindir="$PREFIX/bin"

case "$PROFILE" in
  release) profile_flag=(--release) ;;
  debug)   profile_flag=() ;;
  *) echo "PROFILE must be 'release' or 'debug', got '$PROFILE'" >&2; exit 2 ;;
esac

# Which workspace each component is built from. All four live in rust/;
# `cargo test -p dc-verify` still does not compile tree-sitter grammars.
component_workspace() {
  case "$1" in
    dcstore|dcverify|dcgrep|devmap) echo "rust" ;;
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
state_dir="$PREFIX/share/devcouncil"
disabled_file="$state_dir/disabled"

if [ "$LIST" -eq 1 ]; then
  echo "components: ${ALL[*]}"
  echo "preset: analysis"
  exit 0
fi

if [ -n "$DISABLE_NAME" ]; then
  case "$DISABLE_NAME" in
    dcstore|dcverify|dcgrep|devmap) ;;
    *)
      echo "unknown component '$DISABLE_NAME' (known: ${ALL[*]})" >&2
      exit 2
      ;;
  esac
  mkdir -p "$state_dir"
  touch "$disabled_file"
  if ! grep -qx "$DISABLE_NAME" "$disabled_file" 2>/dev/null; then
    printf '%s\n' "$DISABLE_NAME" >>"$disabled_file"
  fi
  echo "disabled $DISABLE_NAME (binary left in $bindir)"
  exit 0
fi

if [ -n "$ENABLE_NAME" ]; then
  case "$ENABLE_NAME" in
    dcstore|dcverify|dcgrep|devmap) ;;
    *)
      echo "unknown component '$ENABLE_NAME' (known: ${ALL[*]})" >&2
      exit 2
      ;;
  esac
  if [ -f "$disabled_file" ]; then
    tmp="$(mktemp)"
    grep -vx "$ENABLE_NAME" "$disabled_file" >"$tmp" || true
    mv "$tmp" "$disabled_file"
  fi
  echo "enabled $ENABLE_NAME"
  exit 0
fi

requested=()
if [ "$WANT_ALL" -eq 1 ] && [ "${#POS[@]}" -gt 0 ]; then
  echo "do not mix --all with component names (see --help)" >&2
  exit 2
fi
if [ "${#POS[@]}" -gt 0 ]; then
  for n in "${POS[@]}"; do
    case "$n" in
      analysis|all) requested+=("${ALL[@]}") ;;
      codeintel) requested+=("devmap") ;;
      dcstore|dcverify|dcgrep|devmap) requested+=("$n") ;;
      *)
        echo "unknown component '$n' (known: ${ALL[*]} analysis)" >&2
        exit 2
        ;;
    esac
  done
elif [ "$ACTION" = uninstall ] && [ "$WANT_ALL" -ne 1 ]; then
  echo "uninstall requires a component name or --all (see --help)" >&2
  exit 2
else
  requested=("${ALL[@]}")
fi

if [ "$ACTION" = uninstall ]; then
  if [ "$YES" -ne 1 ] && [ -z "$DRY_RUN" ]; then
    echo "This will remove from $bindir: ${requested[*]}" >&2
    echo "Re-run with --yes to confirm." >&2
    exit 2
  fi
  for name in "${requested[@]}"; do
    dest="$bindir/$name"
    if [ -d "$dest" ] && [ ! -L "$dest" ]; then
      echo "note: left directory $dest in place" >&2
      continue
    fi
    if [ -n "$DRY_RUN" ]; then
      echo "rm $dest"
      continue
    fi
    rm -f "$dest"
    echo "removed $dest"
  done
  exit 0
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
