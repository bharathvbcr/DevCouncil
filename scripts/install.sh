#!/usr/bin/env sh
# First-time bootstrap for DevCouncil.
#
# `devcouncil install` is the installer. This script only gets it running on
# a machine with no host yet: it builds cmd/dcinstall — the same install code,
# without the Gusset engine the host links — into a temporary directory and
# runs it with the same arguments (uninstall when --uninstall is among them).
# Every preset, flag and health check belongs to that command; `--help`
# prints them.
#
#   bash scripts/install.sh                          # host + every analysis binary
#   bash scripts/install.sh --only=devmap            # one component, no host
#   bash scripts/install.sh analysis --prefix /usr/local
#   bash scripts/install.sh --uninstall dcgrep --yes
#   bash scripts/install.sh --help
#
# Without Go, devmap alone installs with (pinned to a release tag; the
# default branch is whatever was merged last)
#   cargo install --git https://github.com/bharathvbcr/DevCouncil.git --tag vX.Y.Z --locked devmap-cli

set -eu

if ! command -v go >/dev/null 2>&1; then
  # Builtins only: with no Go there may be no other tool either. The tag is
  # this checkout's release — the constant the release gate holds
  # package.json, Cargo.toml and the vX.Y.Z tag to.
  script_dir="${0%/*}"
  [ "$script_dir" = "$0" ] && script_dir=.
  tag=vX.Y.Z
  while IFS= read -r line; do
    case "$line" in
      'const Version = "'*'"') tag="${line#const Version = \"}"; tag="v${tag%\"}" ;;
    esac
  done < "$script_dir/../backend/go_orchestrator/devcouncil/version/version.go" || :
  echo "go is required: this script builds the DevCouncil host, and the host is the installer." >&2
  echo "Without Go, install devmap alone: cargo install --git https://github.com/bharathvbcr/DevCouncil.git --tag $tag --locked devmap-cli" >&2
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

# DRY_RUN=1 is how the scripts this replaced were asked to touch nothing.
# Dropping it would turn a rehearsal into a real install.
case "${DRY_RUN:-}" in
  ''|0) ;;
  *) set -- "$@" --dry-run ;;
esac

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# dcinstall is `devcouncil install` without what the host links, so a first
# install — devmap alone, above all — needs Go but not the Gusset checkout.
go -C "$repo_root/backend/go_orchestrator" build -o "$tmp/dcinstall" ./cmd/dcinstall

status=0
DEVCOUNCIL_ROOT="$repo_root" "$tmp/dcinstall" "$verb" "$@" || status=$?
exit "$status"
