#!/usr/bin/env sh
set -eu

repo_root="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
cd "$repo_root"

PREFIX="${PREFIX:-$HOME/.local}"
bindir="$PREFIX/bin"
mkdir -p "$bindir"

if ! command -v go >/dev/null 2>&1; then
  echo "go is required. Install a Go toolchain and rerun this script." >&2
  exit 1
fi

echo "building Go host binary (devcouncil)"
go -C backend/go_orchestrator build -o "$bindir/devcouncil" ./cmd/devcouncil

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

sign_macos "$bindir/devcouncil"

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
  ln -sf devcouncil "$dest"
}

install_dev_link
echo "installed $bindir/devcouncil and $bindir/dev"

if command -v cargo >/dev/null 2>&1; then
  echo "installing Rust analysis components"
  PREFIX="$PREFIX" bash "$repo_root/scripts/install-components.sh"
else
  echo "note: cargo is not on PATH; skipped dcstore/dcverify/dcgrep/devmap. Install a Rust toolchain and run scripts/install-components.sh." >&2
fi

case ":$PATH:" in
  *":$bindir:"*) ;;
  *)
    echo "note: $bindir is not on PATH. Add it with: export PATH=\"$bindir:\$PATH\"" >&2
    ;;
esac

# macOS: print Apple-Silicon-aware local (Ollama) guidance. Local model size is
# bounded by unified memory, so recommend a size that will actually fit.
if [ "$(uname -s)" = Darwin ]; then
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
