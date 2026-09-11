#!/usr/bin/env bash
# The provider-free red→green demo used Python `dev check --verify`.
# That command was deleted with the Python package (Phase 7).
set -euo pipefail

cat >&2 <<'EOF'
The provider-free demo (`dev check --verify`) was retired with the Python package.

Live host commands are Go:
  bash scripts/install.sh
  devcouncil verify TASK_ID
  devcouncil --help

Sample calculator fixtures remain in examples/build-week-demo/ (Python tests
for the fixture itself, not a product CLI). See docs/PHASE7_LONG_TAIL.md.
EOF
exit 2
