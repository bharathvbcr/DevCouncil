"""Coverage for the DiffCoverageMeasurer instrumentation orchestrator.

Injects the measurer's callable seams and mocks ``subprocess.run`` so the Python,
JS, and Go measurement paths, command selection, and the coverage-python resolver
are exercised without a real coverage/c8/go toolchain.
"""

from __future__ import annotations

import json
import shlex
import subprocess
from pathlib import Path

from devcouncil.domain.task import Task
from devcouncil.verification import coverage_measurement as cm
from devcouncil.verification.coverage_measurement import DiffCoverageMeasurer


def _task(**over):
    base = dict(id="TASK-001", title="t", description="d")
    base.update(over)
    return Task(**base)


def _measurer(tmp_path, *, coverage_python="python", load_commands=None, can_prove=None):
    return DiffCoverageMeasurer(
        tmp_path,
        get_coverage_python=lambda: coverage_python,
        split_command=shlex.split,
        verification_env=lambda: {"PATH": "/usr/bin"},
        load_commands=lambda: (load_commands or {"test": ["pytest"]}),
        command_can_prove_acceptance=(can_prove or (lambda kind, c: "pytest" in c)),
    )


_PY_DIFF = (
    "diff --git a/src/app.py b/src/app.py\n"
    "--- a/src/app.py\n"
    "+++ b/src/app.py\n"
    "@@ -0,0 +1,2 @@\n"
    "+def go():\n"
    "+    return 1\n"
)


# ---- coverage_target_commands -------------------------------------------------

def test_target_commands_prefers_expected_tests(tmp_path):
    m = _measurer(tmp_path)
    task = _task(expected_tests=["pytest test_x.py"], allowed_commands=["ruff"])
    assert m.coverage_target_commands(task) == ["pytest test_x.py"]


def test_target_commands_uses_test_like_allowed(tmp_path):
    m = _measurer(tmp_path)
    task = _task(allowed_commands=["ruff check", "pytest -q"])
    assert m.coverage_target_commands(task) == ["pytest -q"]


def test_target_commands_falls_back_to_load_commands(tmp_path):
    m = _measurer(tmp_path, load_commands={"test": ["python -m pytest"]}, can_prove=lambda k, c: False)
    task = _task(allowed_commands=["make build"])
    assert m.coverage_target_commands(task) == ["python -m pytest"]


# ---- measure() early exits ----------------------------------------------------

def test_measure_no_measurable_changes(tmp_path):
    m = _measurer(tmp_path)
    result = m.measure(_task(expected_tests=["pytest"]), "diff --git a/README.md b/README.md\n+text\n")
    assert not result.measured
    assert "no measurable source changes" in result.reason


def test_measure_no_command_to_instrument(tmp_path):
    m = _measurer(tmp_path, load_commands={"test": []}, can_prove=lambda k, c: False)
    result = m.measure(_task(), _PY_DIFF)
    assert not result.measured
    assert "no test command" in result.reason


# ---- _resolve_coverage_python / _coverage_available ---------------------------

def test_resolve_coverage_python_uses_override(tmp_path):
    m = _measurer(tmp_path, coverage_python="/opt/py/bin/python")
    assert m._resolve_coverage_python({"PATH": "/usr/bin"}) == "/opt/py/bin/python"


def test_resolve_coverage_python_falls_back_to_which(tmp_path, monkeypatch):
    m = _measurer(tmp_path, coverage_python=None)
    monkeypatch.setattr(cm.shutil, "which", lambda name, path=None: "/found/python3" if name == "python3" else None)
    # "python" not found, "python3" found.
    monkeypatch.setattr(cm.shutil, "which", lambda name, path=None: "/found/python" if name == "python" else None)
    assert m._resolve_coverage_python({"PATH": "/usr/bin"}) == "/found/python"


def test_resolve_coverage_python_defaults_to_sys_executable(tmp_path, monkeypatch):
    m = _measurer(tmp_path, coverage_python=None)
    monkeypatch.setattr(cm.shutil, "which", lambda name, path=None: None)
    assert m._resolve_coverage_python({"PATH": ""}) == cm.sys.executable


def test_coverage_available_true(tmp_path, monkeypatch):
    m = _measurer(tmp_path)
    monkeypatch.setattr(cm.subprocess, "run", lambda *a, **k: subprocess.CompletedProcess(a, 0))
    assert m._coverage_available("python", {}) is True


def test_coverage_available_nonzero_false(tmp_path, monkeypatch):
    m = _measurer(tmp_path)
    monkeypatch.setattr(cm.subprocess, "run", lambda *a, **k: subprocess.CompletedProcess(a, 1))
    assert m._coverage_available("python", {}) is False


def test_coverage_available_exception_false(tmp_path, monkeypatch):
    m = _measurer(tmp_path)

    def boom(*a, **k):
        raise OSError("no python")

    monkeypatch.setattr(cm.subprocess, "run", boom)
    assert m._coverage_available("python", {}) is False


# ---- _measure_python ----------------------------------------------------------

def test_measure_python_coverage_unavailable(tmp_path, monkeypatch):
    m = _measurer(tmp_path)
    monkeypatch.setattr(m, "_coverage_available", lambda py, env: False)
    tmp_dir = tmp_path / ".devcouncil" / "tmp"
    tmp_dir.mkdir(parents=True)
    result = m._measure_python(_task(), ["pytest"], {"src/app.py": {1: "def go():"}}, tmp_dir, {}, 30)
    assert not result.measured
    assert "coverage.py not available" in result.reason


def test_measure_python_success(tmp_path, monkeypatch):
    m = _measurer(tmp_path)
    monkeypatch.setattr(m, "_coverage_available", lambda py, env: True)
    tmp_dir = tmp_path / ".devcouncil" / "tmp"
    tmp_dir.mkdir(parents=True)

    def fake_run(argv, **kwargs):
        # coverage json step: write the -o output file
        if "json" in argv and "-o" in argv:
            out = Path(argv[argv.index("-o") + 1])
            out.write_text(json.dumps({"files": {"src/app.py": {"executed_lines": [1, 2], "missing_lines": []}}}))
            return subprocess.CompletedProcess(argv, 0)
        # coverage run step: create the data file so ran_any path proceeds
        for a in argv:
            if isinstance(a, str) and a.startswith("--data-file="):
                Path(a.split("=", 1)[1]).write_text("cov-data")
        return subprocess.CompletedProcess(argv, 0)

    monkeypatch.setattr(cm.subprocess, "run", fake_run)
    changed = {"src/app.py": {1: "def go():", 2: "    return 1"}}
    result = m._measure_python(_task(), ["pytest"], changed, tmp_dir, {}, 30)
    assert result.measured
    assert result.covered_changed_lines >= 1


def test_measure_python_no_data_file(tmp_path, monkeypatch):
    m = _measurer(tmp_path)
    monkeypatch.setattr(m, "_coverage_available", lambda py, env: True)
    tmp_dir = tmp_path / ".devcouncil" / "tmp"
    tmp_dir.mkdir(parents=True)
    # subprocess.run does nothing -> data file never created.
    monkeypatch.setattr(cm.subprocess, "run", lambda argv, **k: subprocess.CompletedProcess(argv, 0))
    result = m._measure_python(_task(), ["pytest"], {"src/app.py": {1: "x = 1"}}, tmp_dir, {}, 30)
    assert not result.measured
    assert "no instrumentable Python test command" in result.reason


# ---- _measure_js --------------------------------------------------------------

def test_measure_js_success(tmp_path, monkeypatch):
    m = _measurer(tmp_path)
    tmp_dir = tmp_path / ".devcouncil" / "tmp"
    tmp_dir.mkdir(parents=True)

    def fake_run(argv, **kwargs):
        # c8 argv includes --reports-dir=<dir>
        for a in argv:
            if isinstance(a, str) and a.startswith("--reports-dir="):
                d = Path(a.split("=", 1)[1])
                d.mkdir(parents=True, exist_ok=True)
                (d / "coverage-final.json").write_text(
                    json.dumps({
                        str(tmp_path / "src" / "app.ts"): {
                            "statementMap": {"0": {"start": {"line": 1}, "end": {"line": 1}}},
                            "s": {"0": 1},
                        }
                    })
                )
        return subprocess.CompletedProcess(argv, 0)

    monkeypatch.setattr(cm.subprocess, "run", fake_run)
    changed = {"src/app.ts": {1: "export const go = () => 1;"}}
    result = m._measure_js(_task(), ["npm test"], changed, tmp_dir, {}, 30)
    assert result.measured


def test_measure_js_not_available(tmp_path, monkeypatch):
    m = _measurer(tmp_path)
    tmp_dir = tmp_path / ".devcouncil" / "tmp"
    tmp_dir.mkdir(parents=True)
    monkeypatch.setattr(cm.subprocess, "run", lambda argv, **k: subprocess.CompletedProcess(argv, 0))
    # pytest is not a JS runner -> c8_run_argv returns None -> no report.
    result = m._measure_js(_task(), ["pytest"], {"src/app.ts": {1: "x"}}, tmp_dir, {}, 30)
    assert not result.measured
    assert "c8 not available" in result.reason


# ---- _measure_go --------------------------------------------------------------

def test_measure_go_success(tmp_path, monkeypatch):
    m = _measurer(tmp_path)
    tmp_dir = tmp_path / ".devcouncil" / "tmp"
    tmp_dir.mkdir(parents=True)

    def fake_run(argv, **kwargs):
        for a in argv:
            if isinstance(a, str) and a.startswith("-coverprofile="):
                Path(a.split("=", 1)[1]).write_text(
                    "mode: set\nsrc/app.go:1.1,2.2 1 1\n"
                )
        return subprocess.CompletedProcess(argv, 0)

    monkeypatch.setattr(cm.subprocess, "run", fake_run)
    changed = {"src/app.go": {1: "func Go() int { return 1 }"}}
    result = m._measure_go(_task(), ["go test ./..."], changed, tmp_dir, {}, 30)
    assert result.measured


def test_measure_go_not_available(tmp_path, monkeypatch):
    m = _measurer(tmp_path)
    tmp_dir = tmp_path / ".devcouncil" / "tmp"
    tmp_dir.mkdir(parents=True)
    monkeypatch.setattr(cm.subprocess, "run", lambda argv, **k: subprocess.CompletedProcess(argv, 0))
    result = m._measure_go(_task(), ["pytest"], {"src/app.go": {1: "x"}}, tmp_dir, {}, 30)
    assert not result.measured
    assert "go test -coverprofile not available" in result.reason


# ---- measure() end-to-end (python) --------------------------------------------

def test_measure_end_to_end_python(tmp_path, monkeypatch):
    m = _measurer(tmp_path)
    monkeypatch.setattr(m, "_coverage_available", lambda py, env: True)

    def fake_run(argv, **kwargs):
        if "json" in argv and "-o" in argv:
            out = Path(argv[argv.index("-o") + 1])
            out.write_text(json.dumps({"files": {"src/app.py": {"executed_lines": [1, 2], "missing_lines": []}}}))
            return subprocess.CompletedProcess(argv, 0)
        for a in argv:
            if isinstance(a, str) and a.startswith("--data-file="):
                Path(a.split("=", 1)[1]).write_text("cov")
        return subprocess.CompletedProcess(argv, 0)

    monkeypatch.setattr(cm.subprocess, "run", fake_run)
    result = m.measure(_task(expected_tests=["pytest"]), _PY_DIFF)
    assert result.measured


_JS_DIFF = (
    "diff --git a/src/app.ts b/src/app.ts\n"
    "--- a/src/app.ts\n"
    "+++ b/src/app.ts\n"
    "@@ -0,0 +1,1 @@\n"
    "+export const go = () => 1;\n"
)

_GO_DIFF = (
    "diff --git a/src/app.go b/src/app.go\n"
    "--- a/src/app.go\n"
    "+++ b/src/app.go\n"
    "@@ -0,0 +1,1 @@\n"
    "+func Go() int { return 1 }\n"
)


def _raising_split(bad="@@bad@@"):
    def split(cmd):
        if cmd == bad:
            raise ValueError("unparseable")
        return shlex.split(cmd)

    return split


# ---- measure() dispatches to js and go paths ----------------------------------

def test_measure_dispatches_all_languages(tmp_path, monkeypatch):
    m = _measurer(tmp_path)
    from devcouncil.verification import diff_coverage as dc

    monkeypatch.setattr(m, "_measure_python", lambda *a, **k: dc.DiffCoverageResult(measured=True, tool="coverage.py"))
    monkeypatch.setattr(m, "_measure_js", lambda *a, **k: dc.DiffCoverageResult(measured=True, tool="c8"))
    monkeypatch.setattr(m, "_measure_go", lambda *a, **k: dc.DiffCoverageResult(measured=True, tool="go-cover"))
    combined = _PY_DIFF + _JS_DIFF + _GO_DIFF
    result = m.measure(_task(expected_tests=["pytest"]), combined)
    assert result.measured


def test_measure_timeout_config_falls_back(tmp_path, monkeypatch):
    m = _measurer(tmp_path)
    from devcouncil.verification import diff_coverage as dc

    monkeypatch.setattr(cm, "load_config", lambda root: (_ for _ in ()).throw(RuntimeError("no cfg")))
    captured = {}

    def fake_py(task, commands, changed, tmp_dir, env, timeout):
        captured["timeout"] = timeout
        return dc.DiffCoverageResult(measured=True, tool="coverage.py")

    monkeypatch.setattr(m, "_measure_python", fake_py)
    m.measure(_task(expected_tests=["pytest"]), _PY_DIFF)
    assert captured["timeout"] == 300


# ---- _measure_python extra branches -------------------------------------------

def test_measure_python_skips_unparseable_command(tmp_path, monkeypatch):
    m = _measurer(tmp_path)
    m._split_command = _raising_split()
    monkeypatch.setattr(m, "_coverage_available", lambda py, env: True)
    tmp_dir = tmp_path / ".devcouncil" / "tmp"
    tmp_dir.mkdir(parents=True)
    monkeypatch.setattr(cm.subprocess, "run", lambda argv, **k: subprocess.CompletedProcess(argv, 0))
    result = m._measure_python(_task(), ["@@bad@@"], {"src/app.py": {1: "x"}}, tmp_dir, {}, 30)
    assert not result.measured
    assert "no instrumentable" in result.reason


def test_measure_python_inline_and_noninstrumentable(tmp_path, monkeypatch):
    m = _measurer(tmp_path)
    monkeypatch.setattr(m, "_coverage_available", lambda py, env: True)
    tmp_dir = tmp_path / ".devcouncil" / "tmp"
    tmp_dir.mkdir(parents=True)

    def fake_run(argv, **kwargs):
        if "json" in argv and "-o" in argv:
            out = Path(argv[argv.index("-o") + 1])
            out.write_text(json.dumps({"files": {"src/app.py": {"executed_lines": [1], "missing_lines": []}}}))
            return subprocess.CompletedProcess(argv, 0)
        for a in argv:
            if isinstance(a, str) and a.startswith("--data-file="):
                Path(a.split("=", 1)[1]).write_text("cov")
        return subprocess.CompletedProcess(argv, 0)

    monkeypatch.setattr(cm.subprocess, "run", fake_run)
    # "make build" is non-instrumentable (cov_argv None -> continue); the inline command runs.
    commands = ["make build", 'python -c "print(1)"']
    result = m._measure_python(_task(), commands, {"src/app.py": {1: "x"}}, tmp_dir, {}, 30)
    assert result.measured


def test_measure_python_run_exception_then_no_data(tmp_path, monkeypatch):
    m = _measurer(tmp_path)
    monkeypatch.setattr(m, "_coverage_available", lambda py, env: True)
    tmp_dir = tmp_path / ".devcouncil" / "tmp"
    tmp_dir.mkdir(parents=True)

    def boom(argv, **kwargs):
        raise OSError("run failed")

    monkeypatch.setattr(cm.subprocess, "run", boom)
    result = m._measure_python(_task(), ["pytest"], {"src/app.py": {1: "x"}}, tmp_dir, {}, 30)
    assert not result.measured
    assert "no instrumentable" in result.reason


def test_measure_python_json_parse_failure(tmp_path, monkeypatch):
    m = _measurer(tmp_path)
    monkeypatch.setattr(m, "_coverage_available", lambda py, env: True)
    tmp_dir = tmp_path / ".devcouncil" / "tmp"
    tmp_dir.mkdir(parents=True)

    def fake_run(argv, **kwargs):
        if "json" in argv and "-o" in argv:
            Path(argv[argv.index("-o") + 1]).write_text("{not json")
            return subprocess.CompletedProcess(argv, 0)
        for a in argv:
            if isinstance(a, str) and a.startswith("--data-file="):
                Path(a.split("=", 1)[1]).write_text("cov")
        return subprocess.CompletedProcess(argv, 0)

    monkeypatch.setattr(cm.subprocess, "run", fake_run)
    result = m._measure_python(_task(), ["pytest"], {"src/app.py": {1: "x"}}, tmp_dir, {}, 30)
    assert not result.measured
    assert "coverage.py failed" in result.reason


# ---- _measure_js extra branches -----------------------------------------------

def test_measure_js_skips_unparseable_command(tmp_path, monkeypatch):
    m = _measurer(tmp_path)
    m._split_command = _raising_split()
    tmp_dir = tmp_path / ".devcouncil" / "tmp"
    tmp_dir.mkdir(parents=True)
    monkeypatch.setattr(cm.subprocess, "run", lambda argv, **k: subprocess.CompletedProcess(argv, 0))
    result = m._measure_js(_task(), ["@@bad@@"], {"src/app.ts": {1: "x"}}, tmp_dir, {}, 30)
    assert not result.measured


def test_measure_js_run_exception(tmp_path, monkeypatch):
    m = _measurer(tmp_path)
    tmp_dir = tmp_path / ".devcouncil" / "tmp"
    tmp_dir.mkdir(parents=True)

    def boom(argv, **kwargs):
        raise OSError("c8 failed")

    monkeypatch.setattr(cm.subprocess, "run", boom)
    result = m._measure_js(_task(), ["npm test"], {"src/app.ts": {1: "x"}}, tmp_dir, {}, 30)
    assert not result.measured


def test_measure_js_report_unreadable(tmp_path, monkeypatch):
    m = _measurer(tmp_path)
    tmp_dir = tmp_path / ".devcouncil" / "tmp"
    tmp_dir.mkdir(parents=True)

    def fake_run(argv, **kwargs):
        for a in argv:
            if isinstance(a, str) and a.startswith("--reports-dir="):
                d = Path(a.split("=", 1)[1])
                d.mkdir(parents=True, exist_ok=True)
                (d / "coverage-final.json").write_text("{not json")
        return subprocess.CompletedProcess(argv, 0)

    monkeypatch.setattr(cm.subprocess, "run", fake_run)
    result = m._measure_js(_task(), ["npm test"], {"src/app.ts": {1: "x"}}, tmp_dir, {}, 30)
    assert not result.measured
    assert "c8 report unreadable" in result.reason


# ---- _measure_go extra branches -----------------------------------------------

def test_measure_go_skips_unparseable_command(tmp_path, monkeypatch):
    m = _measurer(tmp_path)
    m._split_command = _raising_split()
    tmp_dir = tmp_path / ".devcouncil" / "tmp"
    tmp_dir.mkdir(parents=True)
    monkeypatch.setattr(cm.subprocess, "run", lambda argv, **k: subprocess.CompletedProcess(argv, 0))
    result = m._measure_go(_task(), ["@@bad@@"], {"src/app.go": {1: "x"}}, tmp_dir, {}, 30)
    assert not result.measured


def test_measure_go_run_exception(tmp_path, monkeypatch):
    m = _measurer(tmp_path)
    tmp_dir = tmp_path / ".devcouncil" / "tmp"
    tmp_dir.mkdir(parents=True)

    def boom(argv, **kwargs):
        raise OSError("go failed")

    monkeypatch.setattr(cm.subprocess, "run", boom)
    result = m._measure_go(_task(), ["go test ./..."], {"src/app.go": {1: "x"}}, tmp_dir, {}, 30)
    assert not result.measured


def test_measure_go_profile_unreadable(tmp_path, monkeypatch):
    m = _measurer(tmp_path)
    tmp_dir = tmp_path / ".devcouncil" / "tmp"
    tmp_dir.mkdir(parents=True)

    def fake_run(argv, **kwargs):
        for a in argv:
            if isinstance(a, str) and a.startswith("-coverprofile="):
                Path(a.split("=", 1)[1]).write_text("mode: set\n")
        return subprocess.CompletedProcess(argv, 0)

    monkeypatch.setattr(cm.subprocess, "run", fake_run)
    monkeypatch.setattr(
        "devcouncil.verification.diff_coverage.parse_go_coverprofile",
        lambda text, root: (_ for _ in ()).throw(RuntimeError("bad profile")),
    )
    result = m._measure_go(_task(), ["go test ./..."], {"src/app.go": {1: "x"}}, tmp_dir, {}, 30)
    assert not result.measured
    assert "go cover profile unreadable" in result.reason


def test_from_verifier_builds_bound_measurer(tmp_path):
    verifier = type(
        "V",
        (),
        {
            "project_root": tmp_path,
            "_coverage_python": None,
            "_load_commands": lambda self=None: {"test": ["pytest"]},
            "_command_can_prove_acceptance": staticmethod(lambda kind, c: True),
        },
    )()
    measurer = DiffCoverageMeasurer.from_verifier(verifier)
    assert measurer.project_root == tmp_path
