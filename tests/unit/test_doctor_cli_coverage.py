from __future__ import annotations

import subprocess
from pathlib import Path
from types import SimpleNamespace

from typer.testing import CliRunner

from devcouncil.cli.commands import doctor as doctor_cmd


runner = CliRunner()


def _config(provider: str = "openrouter", roles: dict[str, object] | None = None):
    return SimpleNamespace(
        models=SimpleNamespace(provider=provider, roles=roles or {}),
        knowledge=SimpleNamespace(directory=".devcouncil/knowledge"),
        provider=SimpleNamespace(),
    )


def test_ollama_probe_success_http_error_bad_json_and_exception(monkeypatch) -> None:
    class Response:
        def __init__(self, status_code: int, payload=None, json_error: Exception | None = None):
            self.status_code = status_code
            self._payload = payload
            self._json_error = json_error

        def json(self):
            if self._json_error:
                raise self._json_error
            return self._payload

    import httpx

    monkeypatch.setattr(httpx, "get", lambda url, timeout: Response(200, {"version": "1.2.3"}))
    ok, detail = doctor_cmd._probe_ollama("http://localhost:11434/v1")
    assert ok is True
    assert "v1.2.3" in detail

    monkeypatch.setattr(httpx, "get", lambda url, timeout: Response(200, {}, RuntimeError("bad json")))
    ok, detail = doctor_cmd._probe_ollama("http://localhost:11434/v1")
    assert ok is True
    assert "Reachable at http://localhost:11434." == detail

    monkeypatch.setattr(httpx, "get", lambda url, timeout: Response(503, {}))
    ok, detail = doctor_cmd._probe_ollama("http://localhost:11434/v1")
    assert ok is False
    assert "HTTP 503" in detail

    monkeypatch.setattr(httpx, "get", lambda url, timeout: (_ for _ in ()).throw(RuntimeError("down")))
    ok, detail = doctor_cmd._probe_ollama("http://localhost:11434/v1")
    assert ok is False
    assert "No Ollama server reachable" in detail


def test_ollama_models_probe_success_http_error_and_exception(monkeypatch) -> None:
    class Response:
        def __init__(self, status_code: int, payload=None):
            self.status_code = status_code
            self._payload = payload

        def json(self):
            return self._payload

    import httpx

    monkeypatch.setattr(
        httpx,
        "get",
        lambda url, timeout: Response(200, {"models": [{"name": "qwen:latest"}, {"name": ""}, {}]}),
    )
    queried, names = doctor_cmd._probe_ollama_models("http://localhost:11434/v1")
    assert queried is True
    assert names == {"qwen:latest"}

    monkeypatch.setattr(httpx, "get", lambda url, timeout: Response(404, {}))
    assert doctor_cmd._probe_ollama_models("http://localhost:11434/v1") == (False, set())

    monkeypatch.setattr(httpx, "get", lambda url, timeout: (_ for _ in ()).throw(RuntimeError("down")))
    assert doctor_cmd._probe_ollama_models("http://localhost:11434/v1") == (False, set())


def test_ollama_model_present_handles_latest_and_untagged() -> None:
    assert doctor_cmd._ollama_model_present("qwen2.5-coder", {"qwen2.5-coder:latest"})
    assert doctor_cmd._ollama_model_present("qwen2.5-coder:7b", {"qwen2.5-coder:7b"})
    assert not doctor_cmd._ollama_model_present("qwen2.5-coder:14b", {"qwen2.5-coder:7b"})


def test_knowledge_dir_uses_config_and_falls_back(monkeypatch, tmp_path: Path) -> None:
    assert doctor_cmd._knowledge_dir(tmp_path, _config()) == ".devcouncil/knowledge"
    assert doctor_cmd._knowledge_dir(tmp_path, SimpleNamespace(knowledge=SimpleNamespace(directory="custom"))) == "custom"
    monkeypatch.setattr(doctor_cmd, "load_config", lambda root: (_ for _ in ()).throw(RuntimeError("bad")))
    assert doctor_cmd._knowledge_dir(tmp_path) == ".devcouncil/knowledge"


def test_check_ingested_knowledge_treats_loose_okf_docs_as_bundle(tmp_path: Path) -> None:
    okf = tmp_path / ".devcouncil" / "knowledge" / "okf"
    okf.mkdir(parents=True)
    (okf / "REQ-1.md").write_text("---\ntype: Req\ntitle: R\n---\nBody\n", encoding="utf-8")

    rows = doctor_cmd.check_ingested_knowledge(tmp_path)

    assert any(component == "Ingested OKF" and "1 bundle(s)" in notes for component, _, notes in rows)


def test_check_ingested_knowledge_reports_bundle_and_design_exceptions(monkeypatch, tmp_path: Path) -> None:
    okf = tmp_path / ".devcouncil" / "knowledge" / "okf" / "bad"
    okf.mkdir(parents=True)
    (okf / "doc.md").write_text("---\ntype: Req\ntitle: R\n---\nBody\n", encoding="utf-8")
    design = tmp_path / ".devcouncil" / "knowledge" / "design"
    design.mkdir(parents=True)
    (design / "design.md").write_text("---\nname: Bad\n---\n# Bad\n", encoding="utf-8")

    import devcouncil.knowledge.design as design_mod
    import devcouncil.knowledge.okf as okf_mod

    monkeypatch.setattr(okf_mod, "read_bundle", lambda path: (_ for _ in ()).throw(RuntimeError("broken bundle")))
    monkeypatch.setattr(design_mod, "parse_design_md", lambda path: (_ for _ in ()).throw(RuntimeError("bad design")))

    rows = doctor_cmd.check_ingested_knowledge(tmp_path)
    text = " ".join(notes for _, _, notes in rows)

    assert "failed to read bundle" in text
    assert "lint failed: bad design" in text


def test_add_logging_row_reports_existing_and_future_log(monkeypatch, tmp_path: Path) -> None:
    added: list[tuple[str, str, str]] = []
    table = SimpleNamespace(add_row=lambda *row: added.append(row))
    monkeypatch.setattr("devcouncil.telemetry.logging_setup.LOG_RELATIVE_PATH", Path(".devcouncil/logs/dev.log"))

    doctor_cmd._add_logging_row(table, tmp_path)
    assert "Will write to" in added[-1][2]

    log = tmp_path / ".devcouncil" / "logs" / "dev.log"
    log.parent.mkdir(parents=True)
    log.write_text("x" * 2048, encoding="utf-8")
    doctor_cmd._add_logging_row(table, tmp_path)
    assert "(2 KB)" in added[-1][2]


def test_render_doctor_reports_missing_tools_and_command_probe_failures(monkeypatch, tmp_path: Path) -> None:
    monkeypatch.setattr(doctor_cmd, "load_config", lambda root: _config("openrouter"))
    monkeypatch.setattr(doctor_cmd, "check_ingested_knowledge", lambda root, config=None: [])
    monkeypatch.setattr(doctor_cmd, "validate_model_provider", lambda provider: provider)
    monkeypatch.setattr(doctor_cmd, "provider_api_key_env_var", lambda provider: "OPENROUTER_API_KEY")
    monkeypatch.setattr(doctor_cmd, "load_local_secrets", lambda root: {})
    monkeypatch.setattr(doctor_cmd, "get_gcloud_access_token", lambda: None)
    monkeypatch.setattr(doctor_cmd, "detect_available_coding_cli", lambda root: False)
    monkeypatch.setattr(doctor_cmd, "resolve_automated_executor", lambda root, requested: "manual")
    monkeypatch.setattr(doctor_cmd, "CODING_CLI_PROBE_ORDER", ("alpha",))
    monkeypatch.setattr(
        doctor_cmd,
        "CODING_CLI_INTEGRATION_INFO",
        {"alpha": SimpleNamespace(label="Alpha CLI", notes="alpha setup")},
    )
    monkeypatch.setattr(doctor_cmd, "CODING_CLI_VERSION_COMMANDS", {"alpha": (("alpha", "--version"),)})

    def fake_which(command: str):
        return "/bin/alpha" if command == "alpha" else None

    monkeypatch.setattr(doctor_cmd.shutil, "which", fake_which)
    monkeypatch.setattr(
        doctor_cmd.subprocess,
        "check_output",
        lambda *args, **kwargs: (_ for _ in ()).throw(subprocess.TimeoutExpired("alpha", 10)),
    )

    result = runner.invoke(doctor_cmd.app, ["--project-root", str(tmp_path)])

    assert result.exit_code == 0
    assert "Git" in result.output
    assert "Missing" in result.output
    assert "Alpha CLI" in result.output
    assert "OPENROUTER_API_KEY" in result.output


def test_render_doctor_reports_supported_coding_cli_and_detected_recommendation(monkeypatch, tmp_path: Path) -> None:
    monkeypatch.setattr(doctor_cmd, "load_config", lambda root: _config("openrouter"))
    monkeypatch.setattr(doctor_cmd, "check_ingested_knowledge", lambda root, config=None: [])
    monkeypatch.setattr(doctor_cmd, "validate_model_provider", lambda provider: provider)
    monkeypatch.setattr(doctor_cmd, "provider_api_key_env_var", lambda provider: "OPENROUTER_API_KEY")
    monkeypatch.setattr(doctor_cmd, "load_local_secrets", lambda root: {"OPENROUTER_API_KEY": "secret"})
    monkeypatch.setattr(doctor_cmd, "get_gcloud_access_token", lambda: None)
    monkeypatch.setattr(doctor_cmd, "detect_available_coding_cli", lambda root: True)
    monkeypatch.setattr(doctor_cmd, "resolve_automated_executor", lambda root, requested: "alpha")
    monkeypatch.setattr(doctor_cmd, "CODING_CLI_PROBE_ORDER", ("alpha", "unknown"))
    monkeypatch.setattr(
        doctor_cmd,
        "CODING_CLI_INTEGRATION_INFO",
        {"alpha": SimpleNamespace(label="Alpha CLI", notes="alpha setup")},
    )
    monkeypatch.setattr(doctor_cmd, "CODING_CLI_VERSION_COMMANDS", {"alpha": (("alpha", "--version"),)})
    monkeypatch.setattr(doctor_cmd.shutil, "which", lambda command: f"/bin/{command}")
    monkeypatch.setattr(doctor_cmd.subprocess, "check_output", lambda *args, **kwargs: "tool 1.0\nsecond line\n")

    result = runner.invoke(doctor_cmd.app, ["--project-root", str(tmp_path)])

    assert result.exit_code == 0
    assert "Alpha CLI" in result.output
    assert "tool 1.0" in result.output
    assert "Use --executor alpha" in result.output
    assert "secrets.env" in result.output


def test_render_doctor_unsupported_provider_exits_early(monkeypatch, tmp_path: Path) -> None:
    monkeypatch.setattr(doctor_cmd, "load_config", lambda root: _config("acme"))
    monkeypatch.setattr(doctor_cmd, "check_ingested_knowledge", lambda root, config=None: [])
    monkeypatch.setattr(doctor_cmd, "validate_model_provider", lambda provider: (_ for _ in ()).throw(ValueError("bad")))
    monkeypatch.setattr(doctor_cmd, "SUPPORTED_MODEL_PROVIDERS", ("openrouter", "ollama"))
    monkeypatch.setattr(doctor_cmd.shutil, "which", lambda command: None)

    result = runner.invoke(doctor_cmd.app, ["--project-root", str(tmp_path)])

    assert result.exit_code == 0
    assert "models.provider" in result.output
    assert "Unsupported" in result.output
    assert "openrouter, ollama" in result.output


def _patch_common_ollama(monkeypatch, tmp_path: Path, config) -> None:
    monkeypatch.setattr(doctor_cmd, "load_config", lambda root: config)
    monkeypatch.setattr(doctor_cmd, "check_ingested_knowledge", lambda root, config=None: [])
    monkeypatch.setattr(doctor_cmd, "validate_model_provider", lambda provider: "ollama")
    monkeypatch.setattr(doctor_cmd, "detect_available_coding_cli", lambda root: False)
    monkeypatch.setattr(doctor_cmd, "resolve_automated_executor", lambda root, requested: "manual")
    monkeypatch.setattr(doctor_cmd, "CODING_CLI_PROBE_ORDER", ())
    monkeypatch.setattr(doctor_cmd.shutil, "which", lambda command: None)

    import devcouncil.llm.provider as provider_mod

    monkeypatch.setattr(provider_mod.OllamaProvider, "_resolve_base_url", staticmethod(lambda: "http://ollama/v1"))
    monkeypatch.setattr(provider_mod.OllamaProvider, "_resolve_num_ctx", staticmethod(lambda: 1024))
    monkeypatch.setattr(
        "devcouncil.hardware.describe_host",
        lambda: SimpleNamespace(
            platform_label="Linux",
            chip_label="discrete GPU host",
            memory_label="8 GB VRAM (GPU)",
            recommended_ollama_model="qwen2.5-coder:7b",
        ),
    )


def test_render_doctor_ollama_unreachable_and_small_context(monkeypatch, tmp_path: Path) -> None:
    _patch_common_ollama(monkeypatch, tmp_path, _config("ollama"))
    monkeypatch.setattr(doctor_cmd, "_probe_ollama", lambda base_url: (False, "No server"))

    result = runner.invoke(doctor_cmd.app, ["--project-root", str(tmp_path)])

    assert result.exit_code == 0
    assert "OLLAMA server" in result.output
    assert "Start it with" in result.output
    assert "may be too small" in result.output
    assert "Recommended local model" in result.output


def test_render_doctor_ollama_model_listing_branches(monkeypatch, tmp_path: Path) -> None:
    class Role:
        def __init__(self, model: str):
            self.model = model

    _patch_common_ollama(monkeypatch, tmp_path, _config("ollama", {"planner": Role("qwen:7b")}))
    monkeypatch.setattr(doctor_cmd, "_probe_ollama", lambda base_url: (True, "Reachable"))

    monkeypatch.setattr(doctor_cmd, "_probe_ollama_models", lambda base_url: (False, set()))
    listing_failed = runner.invoke(doctor_cmd.app, ["--project-root", str(tmp_path)])
    assert "Could not list pulled models" in listing_failed.output

    monkeypatch.setattr(doctor_cmd, "_probe_ollama_models", lambda base_url: (True, {"other:latest"}))
    missing = runner.invoke(doctor_cmd.app, ["--project-root", str(tmp_path)])
    assert "Configured model(s) not pulled" in missing.output

    monkeypatch.setattr(doctor_cmd, "_probe_ollama_models", lambda base_url: (True, {"qwen:7b"}))
    present = runner.invoke(doctor_cmd.app, ["--project-root", str(tmp_path)])
    assert "All configured models present locally" in present.output

    bad_config = _config("ollama", {"planner": object()})
    _patch_common_ollama(monkeypatch, tmp_path, bad_config)
    monkeypatch.setattr(doctor_cmd, "_probe_ollama", lambda base_url: (True, "Reachable"))
    monkeypatch.setattr(doctor_cmd, "_probe_ollama_models", lambda base_url: (True, {"qwen:7b"}))
    no_models = runner.invoke(doctor_cmd.app, ["--project-root", str(tmp_path)])
    assert "No role models configured" in no_models.output


def test_render_doctor_vertexai_env_token_project_and_location(monkeypatch, tmp_path: Path) -> None:
    monkeypatch.setattr(doctor_cmd, "load_config", lambda root: _config("vertexai"))
    monkeypatch.setattr(doctor_cmd, "check_ingested_knowledge", lambda root, config=None: [])
    monkeypatch.setattr(doctor_cmd, "validate_model_provider", lambda provider: "vertexai")
    monkeypatch.setattr(doctor_cmd, "provider_api_key_env_var", lambda provider: "VERTEXAI_API_KEY")
    monkeypatch.setattr(doctor_cmd, "load_local_secrets", lambda root: {})
    monkeypatch.setattr(doctor_cmd, "get_gcloud_access_token", lambda: "token")
    monkeypatch.setattr(doctor_cmd, "detect_available_coding_cli", lambda root: False)
    monkeypatch.setattr(doctor_cmd, "resolve_automated_executor", lambda root, requested: "manual")
    monkeypatch.setattr(doctor_cmd, "CODING_CLI_PROBE_ORDER", ())
    monkeypatch.setattr(doctor_cmd.shutil, "which", lambda command: None)
    monkeypatch.setenv("GOOGLE_CLOUD_PROJECT", "env-project")
    monkeypatch.setenv("VERTEXAI_LOCATION", "us-central1")

    result = runner.invoke(doctor_cmd.app, ["--project-root", str(tmp_path)])

    assert result.exit_code == 0
    assert "Resolvable via gcloud" in result.output
    assert "VERTEXAI_PROJECT" in result.output
    assert "us-central1" in result.output


def test_render_doctor_vertexai_missing_project_and_default_location(monkeypatch, tmp_path: Path) -> None:
    monkeypatch.setattr(doctor_cmd, "load_config", lambda root: _config("vertexai"))
    monkeypatch.setattr(doctor_cmd, "check_ingested_knowledge", lambda root, config=None: [])
    monkeypatch.setattr(doctor_cmd, "validate_model_provider", lambda provider: "vertexai")
    monkeypatch.setattr(doctor_cmd, "provider_api_key_env_var", lambda provider: "VERTEXAI_API_KEY")
    monkeypatch.setattr(doctor_cmd, "load_local_secrets", lambda root: {})
    monkeypatch.setattr(doctor_cmd, "get_gcloud_access_token", lambda: None)
    monkeypatch.setattr(doctor_cmd, "detect_available_coding_cli", lambda root: False)
    monkeypatch.setattr(doctor_cmd, "resolve_automated_executor", lambda root, requested: "manual")
    monkeypatch.setattr(doctor_cmd, "CODING_CLI_PROBE_ORDER", ())
    monkeypatch.setattr(doctor_cmd.shutil, "which", lambda command: None)
    monkeypatch.delenv("VERTEXAI_PROJECT", raising=False)
    monkeypatch.delenv("GOOGLE_CLOUD_PROJECT", raising=False)
    monkeypatch.delenv("VERTEXAI_LOCATION", raising=False)
    monkeypatch.delenv("VERTEXAI_API_KEY", raising=False)

    result = runner.invoke(doctor_cmd.app, ["--project-root", str(tmp_path)])

    assert result.exit_code == 0
    assert "Required for vertexai" in result.output
    assert "global" in result.output


def test_doctor_callback_returns_for_subcommand_and_invokes_renderer(monkeypatch, tmp_path: Path) -> None:
    called: list[Path] = []
    monkeypatch.setattr(doctor_cmd, "render_doctor_check", lambda root: called.append(root))

    doctor_cmd.doctor(SimpleNamespace(invoked_subcommand="child"), project_root=tmp_path)
    assert called == []

    doctor_cmd.doctor(SimpleNamespace(invoked_subcommand=None), project_root=tmp_path)
    assert called == [tmp_path.resolve()]
