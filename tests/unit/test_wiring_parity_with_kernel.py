"""``indexing/wiring.py``'s path predicates against the kernel's, over one corpus.

Two implementations of "is this file vendored / generated / a test" survive:
``src/devcouncil/indexing/wiring.py`` and
``rust-port/crates/devmap-extract/src/wiring.rs``. The kernel's is the one the
map is built from; the Python one is what six prompt-time and verify-gate
callers ask, because they classify paths the kernel has no row for — diff paths
a task just added, and bare path strings pulled out of graph edge ids. Neither
can be routed through the other today (``generation_file_rows`` carries no
``role``/``is_test`` column), so the copy stays — and a copy that nothing
compares is a copy that drifts. This pins it.

Nothing here re-implements the kernel: the rule tables and the fixture lists are
*read out of ``wiring.rs`` itself*, the same way
``devmap-extract/tests/allow_unwired_and_dynamic_imports.rs`` reads
``ALLOW_UNWIRED``'s spelling out of ``wiring.py``. If the kernel file moves or
its shape changes, these tests fail rather than skip: a parity check that could
not run must not report what a parity check that ran and passed reports.

What is pinned is the rule *tables*, the kernel's own fixture sets, and — since
the kernel converged onto the Python rules — the matching rules themselves.

Four divergences used to be named here and deliberately left unasserted,
because on all four Python was the more precise side and the kernel was what
needed the change. All four are now closed in the kernel and asserted:

* ``is_test_path`` — ``wiring.rs`` returned early on ``/src/test/`` and
  ``/src/androidTest/``, which skipped the dotfile exclusion its own
  ``test_path_rule_ignores_dotfiles_inside_a_test_directory`` documents:
  ``app/src/test/.eslintrc`` was a test path to the kernel and not to Python.
  The kernel's ``the_dotfile_exclusion_survives_the_jvm_test_directories``
  fixtures are read below and run through ``wiring.is_test_path``.
* ``is_wiring_decorator`` — the kernel matched its hints as bare substrings, so
  ``@FastAPI_thing``, ``@multitask`` and ``@preregister`` were all wiring to it;
  ``is_wiring_decorated`` matches dotted hints as prefixes and bare hints as
  whole segments, on purpose (see its docstring). The kernel now does the same,
  and both the hint table *and* the kernel's own decorator fixtures are pinned.
* ``is_generated_path`` — the kernel missed ``*_pb2_grpc.pyi`` (grpcio-tools
  writes it) and accepted any ``zz_generated*`` basename where the Python regex
  requires the ``.go`` kubebuilder writes. Both arms are in the kernel's
  fixture list now, so the shared-fixture run covers them.
* console-script entry points — ``[project.scripts] cli = "pkg.mod:func"``
  named a symbol nothing resolved, so the entry function stayed a dead-symbol
  candidate. ``wiring.entry_point_symbols`` was the only implementation and was
  deleted as callerless in ``c9f9202``; the kernel's
  ``config_entry_point_symbols`` now owns the rule, and the four module-path
  candidates it tries are pinned equal to the ones ``wiring._add_module_file``
  still uses for ``entry_roots``.

One went the other way — the kernel was right and the Python copy wrong:

* ``is_test_path`` on a *file* named ``test`` or ``androidTest`` under
  ``src/``: Python wrapped the whole path in slashes before looking for
  ``/src/test/``, so ``app/src/test`` (a file) was a test directory to it and
  exempt from liveness. The kernel reads the directory part only. Its
  ``a_file_named_like_the_jvm_test_directory_is_not_a_test_path`` fixtures are
  read below.
"""

from __future__ import annotations

import inspect
import re
from pathlib import Path

import pytest

from devcouncil.indexing import wiring

REPO_ROOT = Path(__file__).resolve().parents[2]
KERNEL_WIRING = REPO_ROOT / "rust-port/crates/devmap-extract/src/wiring.rs"


def _kernel_source() -> str:
    assert KERNEL_WIRING.is_file(), (
        f"the kernel's wiring rules are not at {KERNEL_WIRING.relative_to(REPO_ROOT)}; "
        "this parity check cannot run, and must not be read as one that passed"
    )
    return KERNEL_WIRING.read_text(encoding="utf-8")


def _const_array(source: str, name: str) -> list[str]:
    """The string literals of ``const NAME: &[&str] = &[...];``."""
    match = re.search(
        rf"const {re.escape(name)}: &\[&str\] = &\[(.*?)\];", source, re.DOTALL
    )
    assert match, f"{name} is no longer a `&[&str]` const in wiring.rs"
    return re.findall(r'"([^"]*)"', match.group(1))


def _fn_body(source: str, name: str) -> str:
    """Text from ``fn NAME(`` to the start of the next top-level item."""
    start = re.search(rf"(?m)^(?:pub )?fn {re.escape(name)}\b", source)
    assert start, f"wiring.rs no longer defines fn {name}"
    rest = source[start.end() :]
    end = re.search(r"(?m)^(?:pub )?(?:fn |const |mod )", rest)
    return rest[: end.start()] if end else rest


def _segment_equalities(body: str) -> set[str]:
    """Path-segment literals compared with ``*p == "..."`` in a fn body."""
    found = set(re.findall(r'\*p == "([^"]+)"', body))
    assert found, "the segment comparison in this fn is no longer spelled `*p == \"...\"`"
    return found


def _fixture_blocks(source: str) -> list[tuple[str, bool, list[str]]]:
    """``(predicate, expected, paths)`` for every ``for path in [...] { assert!... }``."""
    blocks: list[tuple[str, bool, list[str]]] = []
    for match in re.finditer(r"for path in \[(.*?)\] \{(.*?)\n        \}", source, re.DOTALL):
        paths = re.findall(r'"([^"]*)"', match.group(1))
        assertion = re.search(r"assert!\(\s*(!?)is_(\w+)\(path", match.group(2))
        if not assertion or not paths:
            continue
        blocks.append((f"is_{assertion.group(2)}", assertion.group(1) != "!", paths))
    assert blocks, "wiring.rs's #[test] fixture lists are no longer `for path in [...]`"
    return blocks


def _decorator_fixture_blocks(source: str) -> list[tuple[bool, list[str]]]:
    """``(expected, decorators)`` for every ``for decorator in [...] { assert!... }``.

    The decorator counterpart of :func:`_fixture_blocks`. Same discipline: the
    inputs and the kernel's verdict on them are read out of ``wiring.rs``'s own
    ``#[test]`` blocks, never restated here.
    """
    blocks: list[tuple[bool, list[str]]] = []
    for match in re.finditer(
        r"for decorator in \[(.*?)\] \{(.*?)\n        \}", source, re.DOTALL
    ):
        decorators = re.findall(r'"((?:[^"\\]|\\.)*)"', match.group(1))
        assertion = re.search(r"assert!\(\s*(!?)is_wiring_decorator\(decorator", match.group(2))
        if not assertion or not decorators:
            continue
        blocks.append((assertion.group(1) != "!", decorators))
    assert blocks, (
        "wiring.rs's is_wiring_decorator fixtures are no longer "
        "`for decorator in [...]`; this parity check cannot run"
    )
    return blocks


def _single_asserts(source: str) -> list[tuple[str, bool, str]]:
    """``(predicate, expected, path)`` for ``assert!(is_x("..."))`` one-liners."""
    return [
        (f"is_{name}", negation != "!", value)
        for negation, name, value in re.findall(
            r'assert!\(\s*(!?)is_(\w+)\(r?"((?:[^"\\]|\\.)*)"\)\s*\)', source
        )
    ]


PYTHON_PREDICATE = {
    "is_test_path": wiring.is_test_path,
    "is_vendored_path": wiring.is_vendored_path,
    "is_generated_path": wiring.is_generated_path,
}


# ----------------------------------------------------------------------
# rule tables
# ----------------------------------------------------------------------


def test_the_two_test_directory_tables_are_the_same_set() -> None:
    kernel = _segment_equalities(_fn_body(_kernel_source(), "is_test_path"))
    assert kernel == set(wiring._TEST_DIR_NAMES), (
        "the kernel treats these directory names as test directories and Python "
        f"does not, or the reverse: kernel={sorted(kernel)} "
        f"python={sorted(wiring._TEST_DIR_NAMES)}"
    )


def test_every_directory_the_kernel_calls_vendored_is_vendored_here() -> None:
    kernel = _segment_equalities(_fn_body(_kernel_source(), "is_vendored_path"))
    missed = sorted(name for name in kernel if not wiring.is_vendored_path(f"a/{name}/b.py"))
    assert not missed, (
        "the kernel exempts these directories from liveness and the Python copy "
        f"does not, so the verify gates flag symbols the map calls vendored: {missed}"
    )


def test_python_calls_nothing_vendored_that_the_kernel_does_not() -> None:
    kernel = _segment_equalities(_fn_body(_kernel_source(), "is_vendored_path"))
    extra = sorted(name for name in wiring._VENDOR_DIR_NAMES if name not in kernel)
    assert not extra, (
        "the Python copy exempts directories the kernel still reports on, so a "
        f"real dead-code finding is silently cleared at verify time: {extra}"
    )


def test_every_minified_suffix_the_kernel_knows_is_vendored_here() -> None:
    kernel = _const_array(_kernel_source(), "MINIFIED_SUFFIXES")
    missed = sorted(
        suffix for suffix in kernel if not wiring.is_vendored_path(f"web/static/lib{suffix}")
    )
    assert not missed, (
        "the kernel treats these as minified bundles and the Python copy parses "
        f"them as first-class source: {missed}"
    )


def test_the_two_js_resolution_tables_are_the_same_set() -> None:
    kernel = _const_array(_kernel_source(), "JS_RESOLVE_EXTS")
    assert set(kernel) == set(wiring._JS_RESOLVE_EXTS)


def test_the_two_code_config_suffix_tables_are_the_same_set() -> None:
    kernel = {f".{suffix}" for suffix in _const_array(_kernel_source(), "CODE_CONFIG_SUFFIXES")}
    assert kernel == set(wiring._CODE_CONFIG_SUFFIXES)


def test_the_two_wiring_decorator_hint_tables_are_the_same_set() -> None:
    body = _fn_body(_kernel_source(), "is_wiring_decorator")
    hints = re.search(r"let hints = \[(.*?)\];", body, re.DOTALL)
    assert hints, "is_wiring_decorator no longer holds its hints in a `let hints = [...]`"
    kernel = set(re.findall(r'"([^"]*)"', hints.group(1)))
    assert kernel == set(wiring._WIRING_DECORATOR_HINTS)


def test_the_two_entry_point_candidate_tables_are_the_same_set() -> None:
    """The module paths a ``pkg.mod:func`` target is tried against.

    ``wiring.entry_point_symbols`` owned this rule until ``c9f9202`` deleted it
    as callerless, and the kernel had no equivalent — so a console-script entry
    function with no in-repo caller stayed a dead-symbol candidate at the tier
    agents act on. ``config_entry_point_symbols`` owns it now; the four
    candidates it tries must be the four ``_add_module_file`` still tries for
    :func:`wiring.entry_roots`, or the two disagree about which file a console
    script names.
    """
    source = _kernel_source()
    body = _fn_body(source, "config_entry_point_symbols")
    kernel_block = re.search(
        r"let candidates: \[String; ENTRY_POINT_CANDIDATES\] = \[(.*?)\n        \];",
        body,
        re.DOTALL,
    )
    assert kernel_block, (
        "config_entry_point_symbols no longer holds its module-path candidates "
        "in a `let candidates: [String; ENTRY_POINT_CANDIDATES] = [...]`; this "
        "parity check cannot run, and must not be read as one that passed"
    )
    kernel = {
        template.replace("{base}", "").replace("{module_path}", "{parts}")
        for template in re.findall(r'format!\("([^"]*)"\)', kernel_block.group(1))
    }

    declared = re.search(r"const ENTRY_POINT_CANDIDATES: usize = (\d+);", source)
    assert declared, "ENTRY_POINT_CANDIDATES is no longer a `usize` const in wiring.rs"
    assert len(kernel) == int(declared.group(1)), (
        "the kernel's candidate list and the constant the bound is expressed in "
        f"disagree: {len(kernel)} candidates, ENTRY_POINT_CANDIDATES="
        f"{declared.group(1)}"
    )

    python_source = inspect.getsource(wiring._add_module_file)
    python_block = re.search(r"candidates = \[(.*?)\n    \]", python_source, re.DOTALL)
    assert python_block, (
        "wiring._add_module_file no longer holds its candidates in a "
        "`candidates = [...]`; this parity check cannot run"
    )
    python = set(re.findall(r'f"([^"]*)"', python_block.group(1)))

    assert kernel and python, "one side's candidate list read as empty"
    assert kernel == python, (
        "the kernel resolves a `pkg.mod:func` entry point against different "
        f"module paths from the Python side: kernel={sorted(kernel)} "
        f"python={sorted(python)}"
    )


def test_the_two_generated_header_windows_are_the_same() -> None:
    body = _fn_body(_kernel_source(), "source_has_generated_header")
    markers = re.search(r"let markers = \[(.*?)\];", body, re.DOTALL)
    prefixes = re.search(r"let prefixes = \[(.*?)\];", body, re.DOTALL)
    take = re.search(r"\.take\((\d+)\)", body)
    assert markers and prefixes and take, "source_has_generated_header changed shape in wiring.rs"
    assert set(re.findall(r'"([^"]*)"', markers.group(1))) == set(wiring._GENERATED_HEADER_MARKERS)
    assert set(re.findall(r'"([^"]*)"', prefixes.group(1))) == set(wiring._COMMENT_LINE_PREFIXES)
    assert int(take.group(1)) == wiring._GENERATED_SNIFF_LINES


# ----------------------------------------------------------------------
# the kernel's own fixtures, run through the Python predicates
# ----------------------------------------------------------------------


def _shared_fixtures() -> list[tuple[str, bool, str]]:
    source = _kernel_source()
    cases = [
        (predicate, expected, path)
        for predicate, expected, paths in _fixture_blocks(source)
        for path in paths
    ]
    cases += _single_asserts(source)
    return [case for case in cases if case[0] in PYTHON_PREDICATE]


SHARED_FIXTURES = _shared_fixtures()


def test_the_fixture_list_was_actually_read() -> None:
    """A capped or empty read must not pass as full agreement."""
    assert len(SHARED_FIXTURES) >= 30, (
        "wiring.rs's #[test] fixtures no longer parse into path/verdict pairs; "
        f"only {len(SHARED_FIXTURES)} were read, so agreement below proves nothing"
    )
    assert {case[0] for case in SHARED_FIXTURES} == set(PYTHON_PREDICATE), (
        "a predicate dropped out of the shared fixture list: "
        f"{sorted({case[0] for case in SHARED_FIXTURES})}"
    )


@pytest.mark.parametrize(("predicate", "expected", "path"), SHARED_FIXTURES)
def test_python_agrees_with_the_kernels_own_fixtures(
    predicate: str, expected: bool, path: str
) -> None:
    answer = PYTHON_PREDICATE[predicate](path.replace("\\\\", "\\"))
    assert answer is expected, (
        f"{predicate}({path!r}): the kernel's own test asserts {expected} and the "
        f"Python copy answers {answer}"
    )


DECORATOR_FIXTURES = [
    (expected, decorator)
    for expected, decorators in _decorator_fixture_blocks(_kernel_source())
    for decorator in decorators
]


def test_the_decorator_fixture_list_was_actually_read() -> None:
    """A capped or empty read must not pass as full agreement."""
    assert len(DECORATOR_FIXTURES) >= 20, (
        "wiring.rs's is_wiring_decorator fixtures no longer parse into "
        f"decorator/verdict pairs; only {len(DECORATOR_FIXTURES)} were read, so "
        "agreement below proves nothing"
    )
    assert {expected for expected, _ in DECORATOR_FIXTURES} == {True, False}, (
        "the decorator fixtures now assert in only one direction, and a rule "
        "that answers True to everything would satisfy them"
    )


@pytest.mark.parametrize(("expected", "decorator"), DECORATOR_FIXTURES)
def test_python_agrees_with_the_kernels_decorator_fixtures(
    expected: bool, decorator: str
) -> None:
    """The matching *rule*, not just the hint table.

    The kernel used to compare a hint as a bare substring of the whole line, so
    ``@multitask`` was a ``task`` and ``@preregister`` a ``register``. Because
    the annotation it feeds targets the *file* and ``is_file_exempt`` exempts
    every symbol in that file, one such line hid a whole file from the dead-code
    scan while the Python gates still reported on it. The kernel now matches
    dotted hints as prefixes and bare hints as whole segments, exactly as
    :func:`wiring.is_wiring_decorated` does, and this runs the kernel's own
    fixtures through the Python one to keep it that way.
    """
    answer = wiring.is_wiring_decorated([decorator])
    assert answer is expected, (
        f"is_wiring_decorator({decorator!r}): the kernel's own test asserts "
        f"{expected} and `is_wiring_decorated` answers {answer}"
    )
