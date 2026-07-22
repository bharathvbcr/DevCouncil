#!/usr/bin/env bash
# Provider-free red→green evidence-gate demo for OpenAI Build Week judges.
#
# Creates an isolated calculator git repo, runs `dev check --verify` against a
# deliberately buggy change (blocking / red), applies the real repair + regression
# test, then reruns to a compiled zero-gap (green) pass. No API keys required.
#
# Usage (checkout):
#   bash scripts/build-week-demo.sh
#
# From npm global install (preferred judge path):
#   npm install -g devcouncil@0.4.2
#   devcouncil-build-week-demo
#
# Optional:
#   BUILD_WEEK_DEMO_ROOT=/tmp/my-demo bash scripts/build-week-demo.sh

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SAMPLE_DIR="${REPO_ROOT}/examples/build-week-demo"
GOAL='sub returns a - b'
TEST_CMD='python test_calc.py'

if [[ ! -f "${SAMPLE_DIR}/calc.py" || ! -f "${SAMPLE_DIR}/broken_calc.py" || ! -f "${SAMPLE_DIR}/test_calc.py" ]]; then
  echo "error: missing sample files under ${SAMPLE_DIR}" >&2
  exit 2
fi

resolve_dev() {
  if [[ -n "${DEVCOUNCIL_DEV_BIN:-}" && -x "${DEVCOUNCIL_DEV_BIN}" ]]; then
    printf '%s\n' "${DEVCOUNCIL_DEV_BIN}"
    return
  fi
  if [[ -x "${REPO_ROOT}/.venv/bin/dev" ]]; then
    printf '%s\n' "${REPO_ROOT}/.venv/bin/dev"
    return
  fi
  if command -v dev >/dev/null 2>&1; then
    command -v dev
    return
  fi
  if [[ -x "${REPO_ROOT}/.venv/bin/python" ]]; then
    printf '%s\n' "${REPO_ROOT}/.venv/bin/python -m devcouncil"
    return
  fi
  if command -v python3 >/dev/null 2>&1; then
    printf '%s\n' "python3 -m devcouncil"
    return
  fi
  echo "error: could not find a DevCouncil CLI (dev / python -m devcouncil)" >&2
  exit 2
}

DEV_BIN="$(resolve_dev)"
# shellcheck disable=SC2206
DEV=( ${DEV_BIN} )

PROVIDER_FREE_ENV=(
  -u OPENAI_API_KEY
  -u ANTHROPIC_API_KEY
  -u OPENROUTER_API_KEY
  -u GEMINI_API_KEY
  -u GOOGLE_API_KEY
  -u AZURE_OPENAI_API_KEY
  -u COHERE_API_KEY
  -u MISTRAL_API_KEY
  -u GROQ_API_KEY
  -u DEEPSEEK_API_KEY
  -u TOGETHER_API_KEY
  -u FIREWORKS_API_KEY
  -u XAI_API_KEY
  -u DEVCOUNCIL_API_KEY
)

run_check() {
  local label="$1"
  echo
  echo "────────────────────────────────────────────────────────────"
  echo " ${label}"
  echo "────────────────────────────────────────────────────────────"
  # Keep set +e through return: a non-zero return under set -e aborts the script.
  set +e
  env "${PROVIDER_FREE_ENV[@]}" "${DEV[@]}" check --verify \
    --project-root "${DEMO_ROOT}" \
    --goal "${GOAL}" \
    --test "${TEST_CMD}"
  local rc=$?
  return "${rc}"
}

if [[ -n "${BUILD_WEEK_DEMO_ROOT:-}" ]]; then
  DEMO_ROOT="${BUILD_WEEK_DEMO_ROOT}"
  rm -rf "${DEMO_ROOT}"
  mkdir -p "${DEMO_ROOT}"
else
  _tmp="${TMPDIR:-/tmp}"
  DEMO_ROOT="$(mktemp -d "${_tmp%/}/devcouncil-build-week-demo.XXXXXX")"
  unset _tmp
fi

echo "DevCouncil Build Week demo"
echo "  CLI:              ${DEV_BIN}"
echo "  Sample templates: ${SAMPLE_DIR}"
echo "  Generated repo:   ${DEMO_ROOT}"
echo
echo "Judges: leave this path open to inspect the repaired working tree."

cd "${DEMO_ROOT}"
git init -q
git config user.email "build-week-demo@devcouncil.local"
git config user.name "DevCouncil Build Week Demo"

cat > calc.py <<'EOF'
def add(a: int, b: int) -> int:
    return a + b
EOF
git add calc.py
git commit -q -m "baseline: add()"

cp "${SAMPLE_DIR}/broken_calc.py" calc.py
cp "${SAMPLE_DIR}/test_calc.py" test_calc.py

echo
echo ">>> Phase 1: RED — expect a blocking evidence-gate failure"
START_TS="${SECONDS}"
set +e
run_check "RED verdict (blocking gaps expected)"
RED_RC=$?
set -e

if [[ "${RED_RC}" -eq 0 ]]; then
  echo
  echo "error: expected RED (non-zero) from dev check --verify, got exit 0" >&2
  echo "Generated repo left at: ${DEMO_ROOT}" >&2
  exit 1
fi

echo
echo "RED confirmed (exit ${RED_RC}): blocking gaps — change is not verified."

cp "${SAMPLE_DIR}/calc.py" calc.py

echo
echo ">>> Phase 2: GREEN — apply repair + regression test, expect zero-gap pass"
set +e
run_check "GREEN verdict (compiled, zero blocking gaps expected)"
GREEN_RC=$?
set -e

if [[ "${GREEN_RC}" -ne 0 ]]; then
  echo
  echo "error: expected GREEN (exit 0) after repair, got exit ${GREEN_RC}" >&2
  echo "Generated repo left at: ${DEMO_ROOT}" >&2
  exit 1
fi

GREEN_JSON_FILE="${DEMO_ROOT}/.devcouncil-demo-green.json"
set +e
env "${PROVIDER_FREE_ENV[@]}" "${DEV[@]}" check --verify \
  --project-root "${DEMO_ROOT}" \
  --goal "${GOAL}" \
  --test "${TEST_CMD}" \
  --json > "${GREEN_JSON_FILE}"
JSON_RC=$?
set -e
if [[ "${JSON_RC}" -ne 0 ]]; then
  echo "error: green JSON re-check failed with exit ${JSON_RC}" >&2
  exit 1
fi

python3 - "${GREEN_JSON_FILE}" <<'PY'
import json, sys
from pathlib import Path
payload = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
if not payload.get("verified", False):
    raise SystemExit("green JSON missing verified=true")
if int(payload.get("blocking_gap_count", 1)) != 0:
    raise SystemExit(f"green JSON still has blocking gaps: {payload.get('blocking_gap_count')}")
if int(payload.get("gap_count", 1)) != 0:
    raise SystemExit(f"green JSON still has gaps: {payload.get('gap_count')}")
mode = str(payload.get("verification_mode", ""))
if mode != "compiled":
    raise SystemExit(f"expected verification_mode=compiled, got {mode!r}")
print(f"JSON confirmed: verified=true, gaps=0, mode={mode}")
PY

ELAPSED=$((SECONDS - START_TS))
echo
echo "════════════════════════════════════════════════════════════"
echo " Demo complete: RED then GREEN in ${ELAPSED}s"
echo " Generated repository (inspect me):"
echo "   ${DEMO_ROOT}"
echo "════════════════════════════════════════════════════════════"

if [[ "${ELAPSED}" -gt 60 ]]; then
  echo "warning: demo took ${ELAPSED}s (>60s target after package install)" >&2
fi

exit 0
