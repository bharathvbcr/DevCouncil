"""Install a wheel into an isolated venv and exercise its public entry points."""

from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

GRAMMAR_SAMPLES = {
    "astro": "---\nconst value = 1;\n---\n<div>{value}</div>\n",
    "c": "int run(void) { return 0; }\n",
    "cfml": '<cffunction name="run"></cffunction>\n',
    "cobol": "IDENTIFICATION DIVISION.\nPROGRAM-ID. HELLO.\nPROCEDURE DIVISION.\nSTOP RUN.\n",
    "cpp": "int run() { return 0; }\n",
    "csharp": "class Worker { static void Run() {} }\n",
    "css": ".item { color: red; }\n",
    "cuda": "__global__ void kernel() {}\n",
    "dart": "void run() {}\n",
    "erlang": "-module(worker).\n-export([run/0]).\nrun() -> ok.\n",
    "go": "package worker\nfunc run() {}\n",
    "hcl": 'resource "demo" "item" {}\n',
    "html": "<main><p>ready</p></main>\n",
    "java": "class Worker { static void run() {} }\n",
    "javascript": "function run() {}\n",
    "kotlin": "fun run() {}\n",
    "liquid": "<main>{{ value }}</main>\n",
    "lua": "function run() end\n",
    "luau": "local function run(): nil return nil end\n",
    "nix": "{ value = 1; }\n",
    "objc": "@interface Worker\n- (void)run;\n@end\n",
    "pascal": "program Worker;\nbegin\nend.\n",
    "php": "<?php function run() {} ?>\n",
    "python": "def run():\n    return None\n",
    "r": "run <- function() NULL\n",
    "ruby": "def run\nend\n",
    "rust": "fn run() {}\n",
    "scala": "object Worker { def run(): Unit = () }\n",
    "solidity": "pragma solidity ^0.8.0; contract Worker { function run() public {} }\n",
    "svelte": "<script>function run() {}</script>\n<main>ready</main>\n",
    "swift": "func run() {}\n",
    "tsx": "export function View() { return <main />; }\n",
    "typescript": "function run(): void {}\n",
    "vb": "Module Worker\nSub Run()\nEnd Sub\nEnd Module\n",
    "vue": "<template><main>ready</main></template>\n<script>function run() {}</script>\n",
}


def _python(venv: Path) -> Path:
    return venv / ("Scripts/python.exe" if os.name == "nt" else "bin/python")


def _parse(pack: object, language: str, source: str) -> None:
    parser = pack.get_parser(language)  # type: ignore[attr-defined]
    tree = parser.parse(source.encode("utf-8"))
    assert tree.root_node is not None, language


def _installed_grammar_check() -> int:
    import devcouncil_codeintel_grammars as grammars
    import tree_sitter_language_pack as pack

    status = grammars.activate()
    assert status["ok"] and status["activated"], status
    required = set(status["required_grammars"])
    assert required == set(GRAMMAR_SAMPLES), (
        sorted(required - set(GRAMMAR_SAMPLES)),
        sorted(set(GRAMMAR_SAMPLES) - required),
    )
    for language in sorted(required):
        _parse(pack, language, GRAMMAR_SAMPLES[language])

    embedded = {
        "svelte": "<script lang='ts'>const value: number = 1;</script>\n"
        "<style>.item { color: red; }</style>\n<template><div>ready</div></template>",
        "vue": "<script>const value = 1;</script>\n"
        "<style>.item { color: red; }</style>\n<template><div>ready</div></template>",
        "astro": "---\nconst value: number = 1;\n---\n<style>.item { color: red; }</style>",
        "liquid": "<main>{{ value }}</main>\n"
        "<script>const value = 1;</script>\n<style>.item { color: red; }</style>",
    }
    region_patterns = {
        "javascript": r"<script(?:\s[^>]*)?>(.*?)</script\s*>",
        "css": r"<style(?:\s[^>]*)?>(.*?)</style\s*>",
        "html": r"<template(?:\s[^>]*)?>(.*?)</template\s*>",
    }
    for container, source in embedded.items():
        _parse(pack, container, source)
        for language, pattern in region_patterns.items():
            for match in re.finditer(pattern, source, re.IGNORECASE | re.DOTALL):
                script_language = (
                    "typescript"
                    if language == "javascript" and "lang='ts'" in match.group(0)
                    else language
                )
                _parse(pack, script_language, match.group(1))
        if container == "astro":
            frontmatter = re.match(r"^---\s*\n(.*?)\n---", source, re.DOTALL)
            assert frontmatter is not None
            _parse(pack, "typescript", frontmatter.group(1))
        if container == "liquid":
            _parse(pack, "html", source)
    print(f"parsed {len(required)} required grammars and {len(embedded)} embedded fixtures")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("wheel", type=Path, nargs="?")
    parser.add_argument("--grammar", action="store_true")
    parser.add_argument("--installed-grammar-check", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.installed_grammar_check:
        return _installed_grammar_check()
    if args.wheel is None:
        parser.error("wheel is required")
    wheel = args.wheel.expanduser().resolve()
    if not wheel.is_file():
        parser.error(f"wheel not found: {wheel}")

    with tempfile.TemporaryDirectory(prefix="devcouncil-wheel-smoke-") as temp:
        venv = Path(temp) / "venv"
        subprocess.run(
            ["uv", "venv", str(venv), "--python", sys.executable],
            check=True,
        )
        executable = _python(venv)
        subprocess.run(
            ["uv", "pip", "install", "--python", str(executable), str(wheel)],
            check=True,
        )
        if args.grammar:
            subprocess.run(
                [str(executable), str(Path(__file__).resolve()), "--installed-grammar-check"],
                check=True,
            )
        else:
            subprocess.run(
                [str(executable), "-m", "devcouncil", "--help"],
                check=True,
                stdout=subprocess.DEVNULL,
            )
            code = (
                "from devcouncil.codeintel.store import CodeIntelStore; "
                "from devcouncil.integrations.mcp.handlers.tool_specs import all_tools; "
                "names={tool.name for tool in all_tools()}; "
                "assert 'devcouncil_code_explore' in names; print(CodeIntelStore.__name__)"
            )
            subprocess.run([str(executable), "-c", code], check=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
