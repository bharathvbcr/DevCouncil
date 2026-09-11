#!/usr/bin/env sh
# Bootstrap installer for DevCouncil.
#
# Builds the Go host (devcouncil + `dev` symlink) and, when cargo is present,
# the Rust analysis binaries. First-time installs have no host binary yet —
# this script is that path. After the host is on PATH, `devcouncil install`
# is the same catalog.
#
# Usage:
#   bash scripts/install.sh                         # host + all analysis
#   bash scripts/install.sh --only=devmap           # standalone devmap (no Go host)
#   bash scripts/install.sh analysis                # rust suite, no host
#   bash scripts/install.sh host                    # Go host only
#   bash scripts/install.sh --uninstall devmap
#   bash scripts/install.sh --help

set -eu

repo_root="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
cd "$repo_root"

PREFIX="${PREFIX:-$HOME/.local}"
DRY_RUN="${DRY_RUN:-}"
ACTION=install
YES=0
LIST=0
NAMES=""
WANT_ALL=0

usage() {
  cat <<'EOF'
Usage: bash scripts/install.sh [names…] [options]

Install DevCouncil components into $PREFIX/bin (default: ~/.local/bin).

With no names this installs the Go host and every analysis binary. Pass a
preset or component to take a subset. `devmap` is a standalone app — it does
not pull in the Go host.

Presets:  all (default)  analysis  codeintel  devmap  host
Components: host  devmap  dcstore  dcverify  dcgrep

Options:
  --only NAME       install only NAME (repeatable; same as a positional name)
  --uninstall       remove the named binaries from $PREFIX/bin
  --all             with --uninstall, remove every catalog binary
  --list            print the catalog and exit
  --dry-run         print what would run
  --yes             skip uninstall confirmation
  --prefix DIR      install root (or set PREFIX)
  -h, --help        this help

Examples:
  bash scripts/install.sh --only=devmap
  bash scripts/install.sh analysis
  PREFIX=/usr/local bash scripts/install.sh
  bash scripts/install.sh --uninstall dcgrep --yes
  bash scripts/install.sh --uninstall --all --yes

There is no uv / Python install path. These are native Go and Rust binaries.
After the host is installed, `devcouncil install --help` is the same catalog.
EOF
}

append_name() {
  if [ -z "$NAMES" ]; then
    NAMES="$1"
  else
    NAMES="$NAMES $1"
  fi
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    -h|--help)
      usage
      exit 0
      ;;
    --list)
      LIST=1
      shift
      ;;
    --uninstall)
      ACTION=uninstall
      shift
      ;;
    --all)
      WANT_ALL=1
      shift
      ;;
    --dry-run)
      DRY_RUN=1
      shift
      ;;
    --yes|-y)
      YES=1
      shift
      ;;
    --prefix)
      if [ "$#" -lt 2 ] || [ -z "$2" ]; then
        echo "--prefix needs a directory (see --help)" >&2
        exit 2
      fi
      PREFIX="$2"
      shift 2
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
      append_name "$2"
      shift 2
      ;;
    --only=*)
      only="${1#--only=}"
      if [ -z "$only" ]; then
        echo "--only needs a component name (see --help)" >&2
        exit 2
      fi
      append_name "$only"
      shift
      ;;
    --host-only)
      append_name host
      shift
      ;;
    --)
      shift
      break
      ;;
    -*)
      echo "unknown option: $1 (see --help)" >&2
      exit 2
      ;;
    *)
      append_name "$1"
      shift
      ;;
  esac
done

bindir="$PREFIX/bin"

want_host() {
  # Empty names = bulk (host + analysis). Named rust-only presets skip the host.
  if [ -z "$NAMES" ]; then
    return 0
  fi
  case " $NAMES " in
    *" all "*|*" host "*) return 0 ;;
  esac
  return 1
}

want_rust() {
  if [ -z "$NAMES" ]; then
    return 0
  fi
  case " $NAMES " in
    *" all "*|*" analysis "*|*" codeintel "*|*" devmap "*|*" dcstore "*|*" dcverify "*|*" dcgrep "*)
      return 0
      ;;
  esac
  return 1
}

rust_args() {
  if [ -z "$NAMES" ]; then
    return 0
  fi
  case " $NAMES " in
    *" all "*|*" analysis "*)
      return 0
      ;;
  esac
  out=""
  for n in $NAMES; do
    case "$n" in
      devmap|codeintel) out="$out devmap" ;;
      dcstore|dcverify|dcgrep) out="$out $n" ;;
      all|analysis|host) ;;
      *)
        echo "unknown component or preset '$n' (see --help)" >&2
        exit 2
        ;;
    esac
  done
  # shellcheck disable=SC2086
  echo $out
}

if [ -n "$NAMES" ]; then
  for n in $NAMES; do
    case "$n" in
      all|analysis|codeintel|devmap|host|dcstore|dcverify|dcgrep) ;;
      *)
        echo "unknown component or preset '$n' (see --help)" >&2
        exit 2
        ;;
    esac
  done
fi

if [ "$LIST" -eq 1 ]; then
  echo "presets: all analysis codeintel devmap host"
  echo "components: host devmap dcstore dcverify dcgrep"
  exit 0
fi

if [ "$WANT_ALL" -eq 1 ] && [ -n "$NAMES" ]; then
  echo "do not mix --all with component names (see --help)" >&2
  exit 2
fi

if [ "$ACTION" = uninstall ]; then
  if [ "$WANT_ALL" -ne 1 ] && [ -z "$NAMES" ]; then
    echo "uninstall requires a component name or --all (see --help)" >&2
    exit 2
  fi
  targets=""
  if [ "$WANT_ALL" -eq 1 ]; then
    targets="devcouncil devmap dcstore dcverify dcgrep"
  else
    rust="$(rust_args)"
    if want_host; then
      targets="devcouncil $rust"
    else
      targets="$rust"
    fi
  fi
  if [ "$YES" -ne 1 ] && [ -z "$DRY_RUN" ]; then
    echo "This will remove from $bindir: $targets" >&2
    echo "Re-run with --yes to confirm." >&2
    exit 2
  fi
  for name in $targets; do
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
  if want_host; then
    dest="$bindir/dev"
    if [ -L "$dest" ]; then
      target="$(readlink "$dest")"
      base="$(basename -- "$target")"
      case "$base" in
        devcouncil|devcouncil.exe)
          if [ -n "$DRY_RUN" ]; then
            echo "rm $dest"
          else
            rm -f "$dest"
            echo "removed $dest"
          fi
          ;;
        *)
          echo "left $dest in place (symlink to $target, not devcouncil)" >&2
          ;;
      esac
    fi
  fi
  exit 0
fi

if want_host; then
  if ! command -v go >/dev/null 2>&1; then
    echo "go is required to install the host. Install a Go toolchain, or pass --only=devmap to skip it." >&2
    exit 1
  fi
  echo "building Go host binary (devcouncil)"
  if [ -n "$DRY_RUN" ]; then
    echo "go -C backend/go_orchestrator build -o $bindir/devcouncil ./cmd/devcouncil"
  else
    mkdir -p "$bindir"
    go -C backend/go_orchestrator build -o "$bindir/devcouncil" ./cmd/devcouncil
  fi

  sign_macos() {
    bin="$1"
    [ "$(uname -s)" = Darwin ] || return 0
    if ! command -v codesign >/dev/null 2>&1; then
      echo "note: codesign not found; $bin is unsigned" >&2
      return 0
    fi
    if ! codesign --force --sign - "$bin"; then
      echo "note: ad-hoc codesign failed for $bin" >&2
      return 0
    fi
  }

  if [ -z "$DRY_RUN" ]; then
    sign_macos "$bindir/devcouncil"
  fi

  # `dev` is a symlink to this host. Refuse to replace a real `dev` that is not
  # ours — Shopify's CLI, a personal script, or a directory of that name.
  install_dev_link() {
    dest="$bindir/dev"
    if [ -d "$dest" ] && [ ! -L "$dest" ]; then
      echo "refusing to replace $dest (it is a directory). The Go host is $bindir/devcouncil." >&2
      return 1
    fi
    if [ -e "$dest" ] || [ -L "$dest" ]; then
      if [ -L "$dest" ]; then
        target="$(readlink "$dest")"
        base="$(basename -- "$target")"
        case "$base" in
          devcouncil|devcouncil.exe) ;;
          *)
            echo "refusing to replace $dest (symlink to $target, not devcouncil). The Go host is $bindir/devcouncil." >&2
            return 1
            ;;
        esac
      else
        echo "refusing to replace $dest (not a symlink to our binary). The Go host is $bindir/devcouncil." >&2
        return 1
      fi
    fi
    if [ -n "$DRY_RUN" ]; then
      echo "ln -sf devcouncil $dest"
      return 0
    fi
    ln -sf devcouncil "$dest"
  }

  install_dev_link
  if [ -n "$DRY_RUN" ]; then
    echo "would install $bindir/devcouncil and $bindir/dev"
  else
    echo "installed $bindir/devcouncil and $bindir/dev"
  fi
fi

if want_rust; then
  rust="$(rust_args)"
  if command -v cargo >/dev/null 2>&1; then
    echo "installing Rust analysis components"
    if [ -n "$DRY_RUN" ]; then
      echo "PREFIX=$PREFIX DRY_RUN=1 bash $repo_root/scripts/install-components.sh $rust"
    else
      # shellcheck disable=SC2086
      PREFIX="$PREFIX" DRY_RUN="$DRY_RUN" bash "$repo_root/scripts/install-components.sh" $rust
    fi
  else
    echo "note: cargo is not on PATH; skipped analysis components. Install a Rust toolchain and run scripts/install-components.sh." >&2
  fi
fi

case ":$PATH:" in
  *":$bindir:"*) ;;
  *)
    echo "note: $bindir is not on PATH. Add it with: export PATH=\"$bindir:\$PATH\"" >&2
    ;;
esac

# macOS: print Apple-Silicon-aware local (Ollama) guidance. Local model size is
# bounded by unified memory, so recommend a size that will actually fit.
if [ "$(uname -s)" = Darwin ] && want_host; then
  ram_bytes="$(sysctl -n hw.memsize 2>/dev/null || echo 0)"
  ram_gb=$((ram_bytes / 1024 / 1024 / 1024))
  if [ "$ram_gb" -ge 48 ]; then
    model="qwen2.5-coder:32b"
  elif [ "$ram_gb" -ge 24 ]; then
    model="qwen2.5-coder:14b"
  else
    model="qwen2.5-coder:7b"
  fi

  echo ""
  if [ "$(uname -m)" = arm64 ]; then
    echo "macOS (Apple Silicon, ${ram_gb} GB) detected. To run a local model with Ollama:"
  else
    echo "macOS (Intel, ${ram_gb} GB) detected. To run a local model with Ollama:"
  fi
  if ! command -v ollama >/dev/null 2>&1; then
    echo "  1. Install Ollama:        brew install ollama   (then: ollama serve)"
  fi
  echo "  2. Pull a model:          ollama pull ${model}"
  echo "  3. Raise the context:     export OLLAMA_NUM_CTX=16384"
fi

echo ""
echo "Try: dev --help"
echo "     devmap --version"
echo "     devcouncil install --help"
