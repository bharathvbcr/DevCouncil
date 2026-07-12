"""Coverage for doctor.py helpers and render branches (ollama/vertexai/unsupported,
mypy probe, coverage floor, status-doc drift, ollama probes)."""

from __future__ import annotations

from pathlib import Path
from types import SimpleNamespace

import yaml
from typer.testing import CliRunner

import devcouncil.cli.commands.doctor as doctor
from devcouncil.cli.main import app

runner = CliRunner()


def _set_provider(tmp_path: Path, provider: str, **models_extra) -> None:
    cfg_path = tmp_path / ".devcouncil" / "config.yaml"
    cfg = yaml.safe_load(cfg_path.read_text(encoding="utf-8"))
    cfg.setdefault("models", {})["provider"] = provider
    cfg_path.write_text(yaml.safe_dump(cfg), encoding="utf-8")


# --- _probe_ollama ----------------------------------------------------------------


def test_probe_ollama_reachable(monkeypatch):
    import httpx
    monkeypatch.setattr(
        httpx, "get",
        lambda url, timeout=3.0: SimpleNamespace(status_code=200, json=lambda: {"version": "0.1"}),
    )
    ok, detail = doctor._probe_ollama("http://localhost:11434/v1")
    assert ok is True
    assert "Reachable" in detail


def test_probe_ollama_http_error(monkeypatch):
    import httpx
    monkeypatch.setattr(httpx, "get", lambda url, timeout=3.0: SimpleNamespace(status_code=500, json=lambda: {}))
    ok, detail = doctor._probe_ollama("http://localhost:11434")
    assert ok is False
    assert "HTTP 500" in detail


def test_probe_ollama_reachable_json_error(monkeypatch):
    import httpx

    def _raise():
        raise ValueError("bad json")

    monkeypatch.setattr(
        httpx, "get",
        lambda url, timeout=3.0: SimpleNamespace(status_code=200, json=_raise),
    )
    ok, detail = doctor._probe_ollama("http://localhost:11434/v1")
    assert ok is True
    assert detail.endswith("11434.")


def test_probe_ollama_unreachable(monkeypatch):
    import httpx
    def boom(url, timeout=3.0):
        raise RuntimeError("connection refused")
    monkeypatch.setattr(httpx, "get", boom)
    ok, detail = doctor._probe_ollama("http://localhost:11434/v1")
    assert ok is False
    assert "No Ollama server" in detail


# --- _probe_ollama_models ---------------------------------------------------------


def test_probe_ollama_models_ok(monkeypatch):
    import httpx
    monkeypatch.setattr(
        httpx, "get",
        lambda url, timeout=3.0: SimpleNamespace(
            status_code=200, json=lambda: {"models": [{"name": "qwen:7b"}, {"name": ""}]}
        ),
    )
    ok, names = doctor._probe_ollama_models("http://localhost:11434/v1")
    assert ok is True
    assert names == {"qwen:7b"}


def test_probe_ollama_models_error(monkeypatch):
    import httpx
    def boom(url, timeout=3.0):
        raise RuntimeError("down")
    monkeypatch.setattr(httpx, "get", boom)
    ok, names = doctor._probe_ollama_models("http://localhost:11434")
    assert ok is False
    assert names == set()


# --- _ollama_model_present --------------------------------------------------------


def test_ollama_model_present_variants():
    assert doctor._ollama_model_present("qwen:7b", {"qwen:7b"}) is True
    assert doctor._ollama_model_present("qwen2.5-coder", {"qwen2.5-coder:latest"}) is True
    assert doctor._ollama_model_present("missing", {"other:latest"}) is False


# --- _parse_status_doc_areas / _subsystem_has_unit_tests --------------------------


def test_parse_status_doc_areas(tmp_path):
    doc = tmp_path / "status.md"
    doc.write_text(
        "| Area | Status |\n| :--- | :--- |\n| **CLI & Storage** | Stable: SQLite |\n",
        encoding="utf-8",
    )
    areas = doctor._parse_status_doc_areas(doc)
    assert areas["CLI & Storage"].startswith("Stable")


def test_subsystem_has_unit_tests(tmp_path):
    unit = tmp_path / "unit"
    sub = unit / "storage"
    sub.mkdir(parents=True)
    (sub / "test_x.py").write_text("def test_x():\n    pass\n", encoding="utf-8")
    assert doctor._subsystem_has_unit_tests(unit, "storage") is True
    assert doctor._subsystem_has_unit_tests(unit, "nope") is False


def test_subsystem_has_unit_tests_flat_prefix(tmp_path):
    unit = tmp_path / "unit"
    unit.mkdir(parents=True)
    (unit / "test_artifact_graph.py").write_text("def test():\n    pass\n", encoding="utf-8")
    assert doctor._subsystem_has_unit_tests(unit, "artifacts") is True


# --- check_coverage_floor ---------------------------------------------------------


def test_coverage_floor_no_pyproject(tmp_path):
    rows = doctor.check_coverage_floor(tmp_path)
    assert "No pyproject.toml" in rows[0][2]


def test_coverage_floor_configured(tmp_path):
    (tmp_path / "pyproject.toml").write_text(
        "[tool.coverage.report]\nfail_under = 90\n", encoding="utf-8"
    )
    rows = doctor.check_coverage_floor(tmp_path)
    assert "fail_under=90" in rows[0][2]


def test_coverage_floor_missing_setting(tmp_path):
    (tmp_path / "pyproject.toml").write_text("[tool.other]\nx = 1\n", encoding="utf-8")
    rows = doctor.check_coverage_floor(tmp_path)
    assert "No [tool.coverage.report] fail_under" in rows[0][2]


def test_coverage_floor_unreadable(tmp_path):
    (tmp_path / "pyproject.toml").write_text("this = = = not toml", encoding="utf-8")
    rows = doctor.check_coverage_floor(tmp_path)
    assert "Could not read" in rows[0][2]


# --- check_mypy_status ------------------------------------------------------------


def test_mypy_no_pyproject(tmp_path):
    rows = doctor.check_mypy_status(tmp_path)
    assert "No pyproject.toml" in rows[0][2]


def test_mypy_no_src(tmp_path):
    (tmp_path / "pyproject.toml").write_text("[tool.x]\n", encoding="utf-8")
    rows = doctor.check_mypy_status(tmp_path)
    assert "No src/" in rows[0][2]


def _prep_mypy_repo(tmp_path):
    (tmp_path / "pyproject.toml").write_text("[tool.x]\n", encoding="utf-8")
    (tmp_path / "src").mkdir()


def test_mypy_not_on_path(tmp_path, monkeypatch):
    _prep_mypy_repo(tmp_path)
    def boom(*a, **k):
        raise FileNotFoundError
    monkeypatch.setattr(doctor.subprocess, "run", boom)
    rows = doctor.check_mypy_status(tmp_path)
    assert "not on PATH" in rows[0][2]


def test_mypy_timeout(tmp_path, monkeypatch):
    _prep_mypy_repo(tmp_path)
    def boom(*a, **k):
        raise doctor.subprocess.TimeoutExpired(cmd="mypy", timeout=120)
    monkeypatch.setattr(doctor.subprocess, "run", boom)
    rows = doctor.check_mypy_status(tmp_path)
    assert "timed out" in rows[0][2]


def test_mypy_internal_error(tmp_path, monkeypatch):
    _prep_mypy_repo(tmp_path)
    monkeypatch.setattr(
        doctor.subprocess, "run",
        lambda *a, **k: SimpleNamespace(returncode=2, stdout="INTERNAL ERROR boom", stderr=""),
    )
    rows = doctor.check_mypy_status(tmp_path)
    assert "INTERNAL ERROR" in rows[0][2]


def test_mypy_passes(tmp_path, monkeypatch):
    _prep_mypy_repo(tmp_path)
    monkeypatch.setattr(
        doctor.subprocess, "run",
        lambda *a, **k: SimpleNamespace(returncode=0, stdout="Success: no issues", stderr=""),
    )
    rows = doctor.check_mypy_status(tmp_path)
    assert rows[0][1] == "[green]OK[/green]"


def test_mypy_errors(tmp_path, monkeypatch):
    _prep_mypy_repo(tmp_path)
    monkeypatch.setattr(
        doctor.subprocess, "run",
        lambda *a, **k: SimpleNamespace(returncode=1, stdout="x.py:1: error: bad\n", stderr=""),
    )
    rows = doctor.check_mypy_status(tmp_path)
    assert "1 error(s)" in rows[0][2]


# --- check_status_doc_drift -------------------------------------------------------


def test_status_doc_drift_no_doc(tmp_path):
    rows = doctor.check_status_doc_drift(tmp_path)
    assert rows[0][1] == "[cyan]INFO[/cyan]"


def test_status_doc_drift_mismatch(tmp_path):
    docs = tmp_path / "docs"
    docs.mkdir()
    (docs / "project-status.md").write_text(
        "| Area | Status |\n| :--- | :--- |\n| **CLI & Storage** | Stable: db |\n",
        encoding="utf-8",
    )
    # No tests/unit/storage/ → the Stable claim is unbacked → WARN.
    rows = doctor.check_status_doc_drift(tmp_path)
    assert rows[0][1] == "[yellow]WARN[/yellow]"
    assert "mismatch" in rows[0][2]


def test_status_doc_drift_backed(tmp_path):
    docs = tmp_path / "docs"
    docs.mkdir()
    # Every mapped area must appear in the table; only CLI & Storage claims Stable
    # (and is backed by a real test dir), the rest are Preview so they're skipped.
    lines = ["| Area | Status |", "| :--- | :--- |"]
    for area, _sub in doctor.STATUS_DOC_UNIT_TEST_DIRS:
        status = "Stable: db" if area == "CLI & Storage" else "Preview: wip"
        lines.append(f"| **{area}** | {status} |")
    (docs / "project-status.md").write_text("\n".join(lines) + "\n", encoding="utf-8")
    storage_tests = tmp_path / "tests" / "unit" / "storage"
    storage_tests.mkdir(parents=True)
    (storage_tests / "test_db.py").write_text("def test():\n    pass\n", encoding="utf-8")
    rows = doctor.check_status_doc_drift(tmp_path)
    assert rows[0][1] == "[green]OK[/green]"


# --- check_local_monitor_sampling -------------------------------------------------


def test_local_monitor_sampling_config_error_returns_empty(tmp_path, monkeypatch):
    import devcouncil.app.config as config_mod
    def boom(root):
        raise RuntimeError("no config")
    monkeypatch.setattr(config_mod, "load_config", boom)
    assert doctor.check_local_monitor_sampling(tmp_path) == []


# --- render_doctor_check: provider paths via CLI ----------------------------------


def test_doctor_unsupported_provider(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    def boom(provider):
        raise ValueError("unsupported")

    monkeypatch.setattr(doctor, "validate_model_provider", boom)
    result = runner.invoke(app, ["doctor", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "Unsupported" in result.output


def test_doctor_ollama_unreachable(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    _set_provider(tmp_path, "ollama")
    monkeypatch.setattr(doctor, "_probe_ollama", lambda base: (False, "No Ollama server."))
    result = runner.invoke(app, ["doctor", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "OLLAMA" in result.output


def test_doctor_ollama_reachable_with_models(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    _set_provider(tmp_path, "ollama")
    monkeypatch.setattr(doctor, "_probe_ollama", lambda base: (True, "Reachable."))
    monkeypatch.setattr(doctor, "_probe_ollama_models", lambda base: (True, {"somemodel:latest"}))
    result = runner.invoke(app, ["doctor", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "OLLAMA server" in result.output


def test_doctor_all_tools_missing_and_no_coding_cli(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    # Force every external tool probe to report absent.
    monkeypatch.setattr(doctor.shutil, "which", lambda name: None)
    monkeypatch.setattr(doctor.subprocess, "check_output", lambda *a, **k: "")
    monkeypatch.setattr(doctor, "detect_available_coding_cli", lambda root: [])
    monkeypatch.setattr(doctor, "resolve_automated_executor", lambda root, x: None)
    result = runner.invoke(app, ["doctor", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "Missing" in result.output


def test_doctor_config_load_failure(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    def boom(root):
        raise RuntimeError("broken config")

    monkeypatch.setattr(doctor, "load_config", boom)
    result = runner.invoke(app, ["doctor", "--project-root", str(tmp_path)])
    # config=None falls back to the openrouter default provider; doctor still renders.
    assert result.exit_code == 0


def test_doctor_api_key_present_in_env(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    monkeypatch.setenv("OPENROUTER_API_KEY", "sk-test")
    result = runner.invoke(app, ["doctor", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "Found in environment" in result.output


def test_doctor_api_key_present_in_secrets(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    monkeypatch.delenv("OPENROUTER_API_KEY", raising=False)
    secrets = tmp_path / ".devcouncil" / "secrets.env"
    secrets.write_text("OPENROUTER_API_KEY=sk-secret\n", encoding="utf-8")
    result = runner.invoke(app, ["doctor", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "secrets.env" in result.output


def test_doctor_vertexai_project_present(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    _set_provider(tmp_path, "vertexai")
    monkeypatch.setenv("VERTEXAI_PROJECT", "my-proj")
    result = runner.invoke(app, ["doctor", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "my-proj" in result.output or "VERTEXAI_PROJECT" in result.output


def test_doctor_ollama_num_ctx_small_warns(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    _set_provider(tmp_path, "ollama")
    monkeypatch.setattr(doctor, "_probe_ollama", lambda base: (True, "Reachable."))
    monkeypatch.setattr(doctor, "_probe_ollama_models", lambda base: (False, set()))
    monkeypatch.setenv("OLLAMA_NUM_CTX", "100")
    result = runner.invoke(app, ["doctor", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "num_ctx" in result.output


def test_doctor_vertexai_provider(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    _set_provider(tmp_path, "vertexai")
    monkeypatch.delenv("VERTEXAI_PROJECT", raising=False)
    monkeypatch.delenv("GOOGLE_CLOUD_PROJECT", raising=False)
    monkeypatch.setattr(doctor, "get_gcloud_access_token", lambda: None)
    result = runner.invoke(app, ["doctor", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "VERTEXAI" in result.output
