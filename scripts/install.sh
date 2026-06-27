#!/usr/bin/env sh
set -eu

repo_root="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
cd "$repo_root"

if ! command -v uv >/dev/null 2>&1; then
  echo "uv is required. Install it from https://docs.astral.sh/uv/getting-started/installation/ and rerun this script." >&2
  exit 1
fi

if [ "${1:-}" = "--editable" ]; then
  uv pip install -e .
  echo "DevCouncil installed in the current environment. Try: uv run devcouncil --help"
else
  uv tool install --force .
  echo "DevCouncil installed as a uv tool. Try: devcouncil --help"
fi

# macOS: print Apple-Silicon-aware local (Ollama) guidance. Local model size is
# bounded by unified memory, so recommend a size that will actually fit and the
# OLLAMA_NUM_CTX export DevCouncil's large prompts need.
if [ "$(uname -s)" = "Darwin" ]; then
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
  if [ "$(uname -m)" = "arm64" ]; then
    echo "macOS (Apple Silicon, ${ram_gb} GB) detected. To run DevCouncil locally with Ollama:"
  else
    echo "macOS (Intel, ${ram_gb} GB) detected. To run DevCouncil locally with Ollama:"
  fi
  if ! command -v ollama >/dev/null 2>&1; then
    echo "  1. Install Ollama:        brew install ollama   (then: ollama serve)"
  fi
  echo "  2. Pull a model:          ollama pull ${model}"
  echo "  3. Raise the context:     export OLLAMA_NUM_CTX=16384"
  echo "  4. Point DevCouncil at it: dev setup --provider ollama --model ${model}"
fi
