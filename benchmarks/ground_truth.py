"""Seeded caller/definition ground truth that no tool under test computed.

`generate(root, seed)` writes one small package per language under
`root/gt_<language>/` and returns the truth it planted: every definition, and
for every leaf function the exact set of functions that call it across files.
The answers come from the generator's own plan, so a tool's callers answer can
be scored against something other than another tool's output.

Names are unique per language and per module, so the truth does not depend on
how a tool disambiguates namesakes; it measures whether a cross-file call is
found at all (recall) and whether anything else is reported (precision).
"""
from __future__ import annotations

import json
from pathlib import Path
import random

LANGUAGES = ("python", "go", "rust", "typescript")
MODULES = 6
CALLERS_PER_MODULE = 2


def _plan(seed: int) -> dict[str, list[dict]]:
    """Per language, per module: which leaf each of its callers calls."""
    rng = random.Random(seed)
    plan: dict[str, list[dict]] = {}
    for language in LANGUAGES:
        modules = []
        for index in range(MODULES):
            others = [other for other in range(MODULES) if other != index]
            modules.append({"index": index,
                            "calls": [rng.choice(others) for _ in range(CALLERS_PER_MODULE)]})
        plan[language] = modules
    return plan


def leaf(language: str, index: int) -> str:
    return f"gt_{language}_leaf_{index}"


def caller(language: str, index: int, slot: int) -> str:
    return f"gt_{language}_caller_{index}_{slot}"


def module_path(language: str, index: int) -> str:
    extension = {"python": "py", "go": "go", "rust": "rs", "typescript": "ts"}[language]
    return f"gt_{language}/m{index}.{extension}"


def _source(language: str, index: int, calls: list[int]) -> str:
    own = leaf(language, index)
    targets = sorted(set(calls))
    if language == "python":
        lines = [f"from gt_python.m{t} import {leaf(language, t)}" for t in targets]
        lines += ["", "", f"def {own}():", "    return 1"]
        for slot, target in enumerate(calls):
            lines += ["", "", f"def {caller(language, index, slot)}():",
                      f"    return {leaf(language, target)}()"]
    elif language == "go":
        # One package across files: a cross-file call needs no import.
        lines = ["package gtgo", "", f"func {own}() int {{ return 1 }}"]
        for slot, target in enumerate(calls):
            lines += ["", f"func {caller(language, index, slot)}() int {{ return {leaf(language, target)}() }}"]
    elif language == "rust":
        lines = [f"use super::m{t}::{leaf(language, t)};" for t in targets]
        lines += ["", f"pub fn {own}() -> i32 {{ 1 }}"]
        for slot, target in enumerate(calls):
            lines += ["", f"pub fn {caller(language, index, slot)}() -> i32 {{ {leaf(language, target)}() }}"]
    else:
        lines = [f'import {{ {leaf(language, t)} }} from "./m{t}";' for t in targets]
        lines += ["", f"export function {own}(): number {{ return 1; }}"]
        for slot, target in enumerate(calls):
            lines += ["", f"export function {caller(language, index, slot)}(): number {{ return {leaf(language, target)}(); }}"]
    return "\n".join(lines) + "\n"


def sources(seed: int) -> dict[str, str]:
    """Relative path -> file content, deterministic for a seed."""
    files: dict[str, str] = {}
    for language, modules in _plan(seed).items():
        for module in modules:
            files[module_path(language, module["index"])] = _source(language, module["index"], module["calls"])
        if language == "rust":
            files["gt_rust/mod.rs"] = "".join(f"pub mod m{i};\n" for i in range(MODULES))
        if language == "python":
            files["gt_python/__init__.py"] = ""
    return files


def truth(seed: int) -> dict:
    """Definitions and, per leaf, the exact (file, caller) set that calls it."""
    definitions: list[list[str]] = []
    callers: dict[str, list[list[str]]] = {}
    for language, modules in _plan(seed).items():
        for module in modules:
            path = module_path(language, module["index"])
            definitions.append([path, leaf(language, module["index"])])
            callers.setdefault(leaf(language, module["index"]), [])
            for slot in range(len(module["calls"])):
                definitions.append([path, caller(language, module["index"], slot)])
        for module in modules:
            path = module_path(language, module["index"])
            for slot, target in enumerate(module["calls"]):
                callers[leaf(language, target)].append([path, caller(language, module["index"], slot)])
    for rows in callers.values():
        rows.sort()
    return {"seed": seed, "languages": list(LANGUAGES), "modules_per_language": MODULES,
            "definitions": sorted(definitions), "callers": dict(sorted(callers.items()))}


def generate(root: Path, seed: int, truth_path: Path | None = None) -> dict:
    """Write the fixture tree under `root`, and the truth to `truth_path`.

    The truth file is kept out of the indexed tree when a path is given, so no
    tool under test can read its own answers from the corpus."""
    for relative, content in sources(seed).items():
        target = root / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(content)
    planted = truth(seed)
    if truth_path is not None:
        truth_path.write_text(json.dumps(planted, indent=2) + "\n")
    return planted


def score(expected: set[tuple[str, str]], returned: set[tuple[str, str]]) -> dict:
    """Precision and recall of one callers answer. An empty truth set has no
    recall to measure, and an empty answer has no precision to measure: both
    are reported as None rather than as 0 or 1."""
    hits = expected & returned
    return {"expected": len(expected), "returned": len(returned), "true_positives": len(hits),
            "precision": len(hits) / len(returned) if returned else None,
            "recall": len(hits) / len(expected) if expected else None,
            "missed": sorted(expected - returned), "extra": sorted(returned - expected)}
