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
