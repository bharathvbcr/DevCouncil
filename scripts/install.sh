#!/usr/bin/env sh
# First-time bootstrap for DevCouncil.
#
# `devcouncil install` is the installer. This script only gets a host binary
# to run it: it builds the Go host from this checkout into a temporary
# directory and runs `devcouncil install` with the same arguments —
# `devcouncil uninstall` when --uninstall is among them. Every preset, flag
# and health check belongs to that command; `--help` prints them.
#
#   bash scripts/install.sh                          # host + every analysis binary
#   bash scripts/install.sh --only=devmap            # one component, no host
#   bash scripts/install.sh analysis --prefix /usr/local
#   bash scripts/install.sh --uninstall dcgrep --yes
#   bash scripts/install.sh --help
#
# Without Go, devmap alone installs with
#   cargo install --git https://github.com/bharathvbcr/DevCouncil.git --locked devmap-cli

set -eu

if ! command -v go >/dev/null 2>&1; then
  echo "go is required: this script builds the DevCouncil host, and the host is the installer." >&2
  echo "Without Go, install devmap alone: cargo install --git https://github.com/bharathvbcr/DevCouncil.git --locked devmap-cli" >&2
  exit 1
fi

repo_root="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"

verb=install
for arg do
  shift
  if [ "$arg" = --uninstall ]; then
    verb=uninstall
  else
    set -- "$@" "$arg"
  fi
done

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

go -C "$repo_root/backend/go_orchestrator" build -o "$tmp/devcouncil" ./cmd/devcouncil

status=0
DEVCOUNCIL_ROOT="$repo_root" "$tmp/devcouncil" "$verb" "$@" || status=$?
exit "$status"
