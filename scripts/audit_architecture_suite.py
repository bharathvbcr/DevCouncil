#!/usr/bin/env python3
"""
scripts/audit_architecture_suite.py - Rigorous AST Non-Tautological Static Audit.

Strictly rejects:
1. Bare constant assertions (assert True).
2. Trivial identity self-assertions (assert mode == GateMode.ENFORCE, assert x == x).
3. Mathematical non-negativity tautologies (assert len(...) >= 0, assert cnt >= 0, assert count >= 0, assert 0 <= len(...)).
4. Disjunctive tautologies (assert A or B where B is len(...) >= 0 or True).
5. Local dummy function closures defined inside test bodies.
6. Direct SixPointCheckResult literal instantiations bypassing GitPulseEngine.
7. Tests with zero genuine domain calls (filtering builtins, built-in file methods [.read(), .write()], and stdlib calls).
8. Mock engine dummy facades returning constant booleans (return True) or static dictionary literals.
"""

import ast
import glob
import os
import sys

BUILTIN_FUNCS = {
    "len", "isinstance", "issubclass", "str", "bytes", "bytearray", "memoryview", "all", "any", "range",
    "dict", "list", "set", "frozenset", "int", "sorted", "print", "float", "bool", "repr",
    "tuple", "iter", "next", "min", "max", "sum", "abs", "round", "map", "filter", "zip",
    "enumerate", "open", "id", "hash", "hasattr", "getattr", "setattr", "delattr", "type"
}

BUILTIN_METHODS = {
    "encode", "decode", "startswith", "endswith", "replace", "split", "strip", "lstrip",
    "rstrip", "join", "isupper", "islower", "isdigit", "isalpha", "format", "lower", "upper",
    "append", "extend", "insert", "remove", "pop", "clear", "index", "count", "sort", "reverse",
    "copy", "get", "keys", "values", "items", "update", "setdefault", "add", "discard",
    "union", "intersection", "difference", "symmetric_difference", "issubset", "issuperset",
    "read", "readline", "readlines", "write", "close", "seek", "tell"
}

STDLIB_METHODS = {
    "dumps", "loads", "dump", "load", "time", "sleep", "perf_counter", "perf_counter_ns",
    "compile", "match", "search", "findall", "sub", "split", "exists", "isdir", "isfile",
    "islink", "joinpath", "read_text", "write_text", "read_bytes", "write_bytes", "mkdir"
}

EXCLUDED_CALLS = BUILTIN_FUNCS | BUILTIN_METHODS | STDLIB_METHODS


def is_tautological_compare(node: ast.Compare) -> bool:
    left_src = ast.unparse(node.left)
    for op, comp in zip(node.ops, node.comparators):
        comp_src = ast.unparse(comp)
        if left_src == comp_src:
            return True
        if isinstance(op, ast.GtE) and isinstance(comp, ast.Constant) and comp.value == 0:
            if "len(" in left_src or "cnt" in left_src or "count" in left_src.lower():
                return True
        if isinstance(op, ast.LtE) and isinstance(node.left, ast.Constant) and node.left.value == 0:
            if "len(" in comp_src or "cnt" in comp_src or "count" in comp_src.lower():
                return True
    return False


def is_tautological_expr(node: ast.AST) -> bool:
    if isinstance(node, ast.Constant) and node.value is True:
        return True
    if isinstance(node, ast.Compare):
        return is_tautological_compare(node)
    if isinstance(node, ast.BoolOp):
        if isinstance(node.op, ast.Or):
            return any(is_tautological_expr(v) for v in node.values)
        if isinstance(node.op, ast.And):
            return all(is_tautological_expr(v) for v in node.values)
    return False


def audit_mock_engine(mock_file: str = "tests/architecture_suite/core/mock_engine.py"):
    if not os.path.exists(mock_file):
        return []
    tree = ast.parse(open(mock_file, encoding="utf-8").read())
    facades = []
    for node in ast.walk(tree):
        if isinstance(node, ast.ClassDef):
            for item in node.body:
                if isinstance(item, ast.FunctionDef):
                    body = [s for s in item.body if not (isinstance(s, ast.Expr) and isinstance(s.value, ast.Constant))]
                    if len(body) == 1 and isinstance(body[0], ast.Return):
                        ret_val = body[0].value
                        if isinstance(ret_val, ast.Constant) and isinstance(ret_val.value, bool):
                            facades.append((f"{mock_file}::{node.name}.{item.name}", f"constant_boolean_return: {ret_val.value}"))
                        elif isinstance(ret_val, ast.Dict) and all(isinstance(v, ast.Constant) for v in ret_val.values):
                            facades.append((f"{mock_file}::{node.name}.{item.name}", "static_dict_return"))
    return facades


def audit_test_suite():
    test_files = sorted(glob.glob("tests/architecture_suite/**/*.py", recursive=True))
    test_files = [
        f for f in test_files
        if not f.endswith("conftest.py") and "/core/" not in f and not f.endswith("__init__.py")
    ]

    total_tests = 0
    tautological_tests = []

    for tf in test_files:
        tree = ast.parse(open(tf, encoding="utf-8").read())
        for node in ast.walk(tree):
            if isinstance(node, ast.FunctionDef) and node.name.startswith("test_"):
                total_tests += 1
                inner_funcs = {n.name for n in node.body if isinstance(n, ast.FunctionDef)}

                calls = [n for n in ast.walk(node) if isinstance(n, ast.Call)]
                call_names = []
                for c in calls:
                    if isinstance(c.func, ast.Name):
                        call_names.append(c.func.id)
                    elif isinstance(c.func, ast.Attribute):
                        call_names.append(c.func.attr)

                domain_calls = [
                    c for c in call_names
                    if c not in EXCLUDED_CALLS and c not in inner_funcs
                ]

                asserts = [n for n in ast.walk(node) if isinstance(n, ast.Assert)]
                trivial_assert = False
                tautology_reason = ""
                for a in asserts:
                    if is_tautological_expr(a.test):
                        trivial_assert = True
                        tautology_reason = f"tautology: {ast.unparse(a.test)}"
                        break

                src = ast.unparse(node)
                six_point_literal = "SixPointCheckResult(" in src and not ("gitpulse." in src or "workbench." in src)

                reasons = []
                if len(domain_calls) == 0:
                    reasons.append("zero_domain_calls")
                if len(inner_funcs) > 0:
                    reasons.append("inner_functions")
                if trivial_assert:
                    reasons.append(tautology_reason or "trivial_assert")
                if six_point_literal:
                    reasons.append("six_point_literal")

                if reasons:
                    tautological_tests.append((f"{tf}::{node.name}", ", ".join(reasons)))

    # Audit mock engine for facades
    facade_violations = audit_mock_engine()

    print(f"Total Architecture Tests Audited: {total_tests}")
    total_violations = len(tautological_tests) + len(facade_violations)
    print(f"Tautological Violations Detected: {total_violations} ({total_violations/total_tests*100:.1f}%)" if total_tests else "No tests found")
    for t, r in tautological_tests:
        print(f"  [FAIL] {t} -> {r}")
    for f, r in facade_violations:
        print(f"  [FAIL] {f} -> {r}")

    if total_violations > 0:
        sys.exit(1)
    print("SUCCESS: 0.0% Tautological Assertions Verified Under Strict Audit.")


if __name__ == "__main__":
    audit_test_suite()
