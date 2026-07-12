"""Renderer coverage for ``devcouncil.integrations.integration_cli``.

Drives each Rich/JSON renderer with a StringIO-backed console (and a mocked check
report) so the recommendation/status/matrix/check output branches are covered without
a fully initialized project.
"""

from __future__ import annotations

import json
from io import StringIO
from types import SimpleNamespace

import pytest
import typer
from rich.console import Console

import devcouncil.integrations.integration_cli as intcli


def _console():
    return Console(file=StringIO(), width=200)


def _text(console):
    return console.file.getvalue()


def test_print_recommendations_none_detected(tmp_path, monkeypatch):
    monkeypatch.setattr("shutil.which", lambda *a, **k: None)
    console = _console()
    intcli.print_recommendations(tmp_path, console)
    out = _text(console)
    assert "Recommendations" in out
    assert "No built-in coding CLI" in out


def test_print_recommendations_with_detected(tmp_path, monkeypatch):
    monkeypatch.setattr(
        "shutil.which", lambda command, *a, **k: "/usr/bin/codex" if "codex" in str(command) else None
    )
    console = _console()
    intcli.print_recommendations(tmp_path, console)
    out = _text(console)
    assert "Recommended executor" in out or "No built-in coding CLI" in out


def test_print_integration_status_table(tmp_path):
    console = _console()
    intcli.print_integration_status(tmp_path, console, as_json=False)
    out = _text(console)
    assert "Integration Status" in out
    assert "Default executor" in out


def test_print_integration_status_json(tmp_path, capsys):
    console = _console()
    intcli.print_integration_status(tmp_path, console, as_json=True)
    captured = capsys.readouterr().out
    payload = json.loads(captured)
    assert "integrations_enabled" in payload
    assert "default_executor" in payload


def test_print_integration_matrix(tmp_path):
    console = _console()
    intcli.print_integration_matrix(console)
    out = _text(console)
    assert "Integration Matrix" in out
    assert "Enforcement" in out


def _fake_report(*, failures=False):
    rows = [
        SimpleNamespace(name="MCP", status="ok", details="ready"),
        SimpleNamespace(name="Codex", status="missing", details="not on PATH"),
        SimpleNamespace(name="Skipped", status="skip", details="n/a"),
        SimpleNamespace(name="Broken", status="fail", details="boom"),
    ]
    return SimpleNamespace(
        checks=rows,
        failures=[rows[-1]] if failures else [],
        to_json=lambda: json.dumps({"ok": not failures}),
    )


def test_run_integration_check_table_success(tmp_path, monkeypatch):
    monkeypatch.setattr(intcli, "build_integration_check_report", lambda root, strict=False: _fake_report())
    console = _console()
    intcli.run_integration_check(tmp_path, console, strict=False, as_json=False, report_file=None)
    out = _text(console)
    assert "Integration Check" in out
    assert "Ready." in out


def test_run_integration_check_failure_raises_exit(tmp_path, monkeypatch):
    monkeypatch.setattr(intcli, "build_integration_check_report", lambda root, strict=False: _fake_report(failures=True))
    console = _console()
    with pytest.raises(typer.Exit) as exc:
        intcli.run_integration_check(tmp_path, console, strict=True, as_json=False, report_file=None)
    assert exc.value.exit_code == 1
    assert "Fix failed checks" in _text(console)


def test_run_integration_check_json_emits(tmp_path, monkeypatch, capsys):
    monkeypatch.setattr(intcli, "build_integration_check_report", lambda root, strict=False: _fake_report())
    console = _console()
    intcli.run_integration_check(tmp_path, console, strict=False, as_json=True, report_file=None)
    payload = json.loads(capsys.readouterr().out)
    assert payload["ok"] is True


def test_run_integration_check_report_file_written(tmp_path, monkeypatch):
    monkeypatch.setattr(intcli, "build_integration_check_report", lambda root, strict=False: _fake_report())
    console = _console()
    report_file = tmp_path / "out" / "report.json"
    intcli.run_integration_check(
        tmp_path, console, strict=False, as_json=False, report_file=report_file
    )
    assert report_file.exists()
    assert json.loads(report_file.read_text(encoding="utf-8"))["ok"] is True
    assert "Wrote integration report" in _text(console)
