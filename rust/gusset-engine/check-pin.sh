#!/usr/bin/env bash
# Fails unless the Gusset sibling checkout is at the revision setup-gusset pins.
#
#   check-pin.sh [<action.yml> [<gusset checkout>]]
#
# CI builds Gusset at the pin and Cargo.lock records that revision's version.
# A sibling at another revision either breaks the --locked build in a way that
# reads like a stale DevCouncil lock, or builds green here against code CI
# never sees. Exit 1 on a mismatch, 2 when the pin or the sibling HEAD cannot
# be read: a check that could not run is not a pass.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
action="${1:-$here/../../.github/actions/setup-gusset/action.yml}"
sibling="${2:-$here/../../../gusset}"

pin="$(sed -n 's/^[[:space:]]*ref:[[:space:]]*\([0-9a-f]\{40\}\)[[:space:]]*$/\1/p' "$action" 2>/dev/null | head -n1 || true)"
if [[ -z "$pin" ]]; then
  echo "check-pin: could not read a 40-hex Gusset ref from $action" >&2
  exit 2
fi
if ! head="$(git -C "$sibling" rev-parse HEAD 2>/dev/null)"; then
  echo "check-pin: $sibling is not a git checkout; cannot read its HEAD" >&2
  exit 2
fi
if [[ "$head" != "$pin" ]]; then
  echo "check-pin: Gusset sibling $sibling is at ${head:0:12} but CI pins ${pin:0:12}." >&2
  echo "Check out the pin (git -C $sibling checkout ${pin:0:12}), or move the pin in $action in the same change as the code that needs it." >&2
  exit 1
fi
echo "check-pin: Gusset sibling is at the pin ${pin:0:12}"
