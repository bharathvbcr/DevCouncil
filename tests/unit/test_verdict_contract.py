"""DevCouncil emits the shared policy-verdict contract.

Manvi owns the verdict vocabulary; DevCouncil and GitPulse are consumers. These
tests hold this repo to `contracts/verdict.schema.json` three ways:

1.  **Statically** — every deny/warn decision the policy engine can construct
    names a rule. A refusal with no rule is a refusal a consumer cannot
    classify, explain, grant against, or count.
2.  **By schema** — decisions the engine actually produces validate against the
    JSON Schema, including its `additionalProperties: false`.
3.  **By severity** — this repo's rule→severity map agrees with the contract's,
    which is what decides whether a denial is demotable or grantable at all.
"""

from __future__ import annotations

import ast
import json
import pathlib

import jsonschema
import pytest

from devcouncil.execution.policy_engine import (
    SEVERITY_BY_RULE,
    PolicyDecision,
    TaskPolicyEngine,
)

CONTRACTS = pathlib.Path(__file__).resolve().parents[2] / "contracts"
ENGINE_SRC = (
    pathlib.Path(__file__).resolve().parents[2]
    / "src"
    / "devcouncil"
    / "execution"
    / "policy_engine.py"
)


@pytest.fixture(scope="module")
def schema() -> dict:
    return json.loads((CONTRACTS / "verdict.schema.json").read_text())


@pytest.fixture(scope="module")
def rule_enum(schema: dict) -> set[str]:
    return set(schema["$defs"]["ruleId"]["enum"])


# --- 1. static: no unnamed refusals ----------------------------------------


def _decision_calls() -> list[ast.Call]:
    tree = ast.parse(ENGINE_SRC.read_text())
    return [
        node
        for node in ast.walk(tree)
        if isinstance(node, ast.Call)
        and isinstance(node.func, ast.Name)
        and node.func.id == "PolicyDecision"
    ]


def _kwarg(call: ast.Call, name: str) -> ast.expr | None:
    for kw in call.keywords:
        if kw.arg == name:
            return kw.value
    return None


def _literal(node: ast.expr | None) -> str | None:
    if isinstance(node, ast.Constant) and isinstance(node.value, str):
        return node.value
    return None


def test_the_engine_constructs_decisions_at_all() -> None:
    # If this drops to zero the other static tests pass vacuously.
    assert len(_decision_calls()) >= 20


def test_every_refusal_names_a_rule(rule_enum: set[str]) -> None:
    unnamed: list[tuple[int, str]] = []
    for call in _decision_calls():
        action = _literal(_kwarg(call, "action"))
        if action not in ("deny", "warn"):
            continue
        rule = _literal(_kwarg(call, "rule"))
        if not rule:
            reason = _literal(_kwarg(call, "reason")) or "<templated>"
            unnamed.append((call.lineno, reason[:60]))
    assert unnamed == [], (
        "these refusals carry no rule id, so a consumer cannot tell which rung "
        f"fired: {unnamed}"
    )


def test_every_named_rule_is_in_the_contract(rule_enum: set[str]) -> None:
    unknown: list[tuple[int, str]] = []
    for call in _decision_calls():
        rule = _literal(_kwarg(call, "rule"))
        if rule is not None and rule not in rule_enum:
            unknown.append((call.lineno, rule))
    assert unknown == [], (
        f"rules not in contracts/verdict.schema.json: {unknown}. "
        "Manvi owns this vocabulary; a rule is added there first."
    )


def test_a_clean_allow_names_no_rule() -> None:
    # The inverse of the refusal rule. An allow that names a rung is either a
    # warn, or a bug that will render as "a rule fired" in a consumer's UI.
    for call in _decision_calls():
        if _literal(_kwarg(call, "action")) != "allow":
            continue
        rule = _literal(_kwarg(call, "rule"))
        assert not rule, f"line {call.lineno}: an allow names rule {rule!r}"


# --- 2. by schema: what the engine actually emits ---------------------------


def _validate(decision: PolicyDecision, schema: dict) -> None:
    payload = decision.model_dump(exclude_none=True)
    jsonschema.validate(payload, schema)


def test_decisions_the_engine_produces_validate(tmp_path: pathlib.Path, schema: dict) -> None:
    engine = TaskPolicyEngine(project_root=tmp_path)

    produced = [
        engine.evaluate_command("", None),
        engine.evaluate_command("rm -rf /", None),
        engine.evaluate_command("dev status", None),
        engine.evaluate_hook_command("git commit --no-verify -m x"),
        engine.evaluate_hook_command("git push --force origin main"),
        engine.evaluate_hook_command("git push origin main"),
        engine.evaluate_hook_command("git status"),
    ]

    for decision in produced:
        _validate(decision, schema)

    # A fixture that only ever produced allows would validate and prove nothing.
    actions = {d.action for d in produced}
    assert "deny" in actions, "no refusal exercised"
    assert "allow" in actions, "no allow exercised"


def test_severity_is_derived_and_fails_closed(schema: dict) -> None:
    known = PolicyDecision(action="deny", reason="r", target="t", rule="scope.unplanned")
    assert known.severity == "soft"

    clean = PolicyDecision(action="allow", reason="r", target="t")
    assert clean.severity == "none"

    # The direction that matters: a rule this build does not know is hard, so
    # it cannot be demoted or granted away by a consumer that does not know it
    # either.
    unknown = PolicyDecision(action="deny", reason="r", target="t", rule="rule.from.the.future")
    assert unknown.severity == "hard"

    # It still validates as a *shape*; the enum check is a separate test, so a
    # future rule is a contract failure rather than a crash.
    jsonschema.validate(known.model_dump(exclude_none=True), schema)


# --- 3. by severity: agreement with the contract ----------------------------


def test_severity_map_matches_the_contract(schema: dict) -> None:
    contract_map = {
        rule: spec["const"]
        for rule, spec in schema["$defs"]["severityByRule"]["properties"].items()
    }

    ours = {r: s for r, s in SEVERITY_BY_RULE.items() if r != ""}

    assert ours == contract_map, (
        "this repo's rule severities disagree with contracts/verdict.schema.json. "
        "Severity decides whether a denial can be demoted or granted at all, so a "
        "disagreement here means the same write is negotiable in one product and "
        "not in another."
    )


def test_contract_severities_use_only_declared_values(schema: dict) -> None:
    allowed = set(schema["$defs"]["severity"]["enum"])
    for rule, severity in SEVERITY_BY_RULE.items():
        assert severity in allowed, f"{rule} has severity {severity!r}, not in {sorted(allowed)}"
