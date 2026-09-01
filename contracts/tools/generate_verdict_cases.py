#!/usr/bin/env python3
"""Generate contracts/verdict.cases.json.

The cases are ENUMERATED from the decision space, not hand-picked. Hand-listed
fixtures rot silently: someone adds a field that changes the classification and
no case covers the combination. The cross-product cannot miss one.

The classification implemented here is the specification. Each of the three
repos implements it independently in its own language, and each runs these
cases through its own implementation. A repo that disagrees fails CI.

Run:  python3 contracts/tools/generate_verdict_cases.py
"""

from __future__ import annotations

import json
import pathlib

ACTIONS = ["allow", "warn", "deny"]
BOOLS = [False, True]

# One representative rule per severity, so a `denied` case carries a plausible
# rule rather than an empty one. Taken from `severities` in
# manvi/policy/decision.go.
RULE_FOR = {
    "deny-hard": ("path.secret", "hard"),
    "deny-soft": ("scope.unplanned", "soft"),
    "warn": ("path.protected_write", "warn"),
    "allow": ("", "none"),
}


def classify(d: dict | None) -> str:
    """The total classification function. Order of evaluation is the contract.

    A decision can satisfy several of these at once — a granted decision is
    also an allow, a demoted one is also an allow — and the earlier branch is
    always the more honest reading of what happened.
    """
    if d is None:
        return "unchecked"
    if d.get("action") == "deny":
        return "denied"
    if d.get("grant_id"):
        return "granted"
    if d.get("demoted"):
        return "demoted"
    if d.get("widened"):
        return "widened"
    if d.get("degraded"):
        return "degraded"
    if d.get("action") == "warn":
        return "warned"
    return "clean"


def build() -> list[dict]:
    cases: list[dict] = []

    # The one case with no decision behind it at all.
    cases.append(
        {
            "name": "no-decision",
            "why": "The gate did not run. This is the absence of a verdict, not a verdict of allow.",
            "decision": None,
            "expect": "unchecked",
        }
    )

    for action in ACTIONS:
        for hard in BOOLS if action == "deny" else [False]:
            for granted in BOOLS:
                for demoted in BOOLS:
                    for widened in BOOLS:
                        for degraded in BOOLS:
                            if action == "deny":
                                rule, severity = RULE_FOR["deny-hard" if hard else "deny-soft"]
                            elif action == "warn":
                                rule, severity = RULE_FOR["warn"]
                            else:
                                rule, severity = RULE_FOR["allow"]

                            d: dict = {
                                "action": action,
                                "rule": rule,
                                "severity": severity,
                                "reason": "generated contract case",
                                "target": "src/lib/example.ts",
                            }
                            if granted:
                                d["grant_id"] = "GRANT-01"
                                d["granted_by"] = "bharath"
                            if demoted:
                                d["demoted"] = "policy.file.mode=advisory (env)"
                            if widened:
                                d["widened"] = "src/lib/**"
                            if degraded:
                                d["degraded"] = ["repo_map.unavailable"]

                            name = "-".join(
                                filter(
                                    None,
                                    [
                                        action,
                                        "hard" if (action == "deny" and hard) else None,
                                        "granted" if granted else None,
                                        "demoted" if demoted else None,
                                        "widened" if widened else None,
                                        "degraded" if degraded else None,
                                    ],
                                )
                            )
                            cases.append({"name": name, "decision": d, "expect": classify(d)})
    return cases


def main() -> None:
    root = pathlib.Path(__file__).resolve().parents[1]
    cases = build()

    seen = {c["name"] for c in cases}
    assert len(seen) == len(cases), "case names must be unique"

    covered = {c["expect"] for c in cases}
    expected_states = {
        "unchecked",
        "denied",
        "granted",
        "demoted",
        "widened",
        "degraded",
        "warned",
        "clean",
    }
    missing = expected_states - covered
    assert not missing, f"enumeration failed to reach: {sorted(missing)}"

    payload = {
        "version": 1,
        "generated_by": "contracts/tools/generate_verdict_cases.py",
        "description": (
            "Cross-product of every classification-relevant field on a policy "
            "verdict, with the state each must render as. Regenerate rather "
            "than edit: hand-edited cases stop covering the space."
        ),
        "classification_order": [
            "no decision at all -> unchecked",
            "action == deny -> denied",
            "grant_id non-empty -> granted",
            "demoted non-empty -> demoted",
            "widened non-empty -> widened",
            "degraded non-empty -> degraded",
            "action == warn -> warned",
            "otherwise -> clean",
        ],
        "cases": cases,
    }

    out = root / "verdict.cases.json"
    out.write_text(json.dumps(payload, indent=2) + "\n")
    by_state: dict[str, int] = {}
    for c in cases:
        by_state[c["expect"]] = by_state.get(c["expect"], 0) + 1
    print(f"wrote {out.relative_to(root.parent)}: {len(cases)} cases")
    for state in sorted(by_state):
        print(f"  {state:<10} {by_state[state]}")


if __name__ == "__main__":
    main()
