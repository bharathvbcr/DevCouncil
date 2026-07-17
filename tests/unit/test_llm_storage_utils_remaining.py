import asyncio
import json
import os
from types import SimpleNamespace

import httpx
import pytest

from devcouncil.llm import provider as provider_mod
from devcouncil.llm.provider import (
    LLMResponse,
    MockProvider,
    OllamaProvider,
    OpenRouterProvider,
    ProviderRequestError,
    VertexAIProvider,
    apply_provider_default_role_models,
    build_role_model_config,
    create_provider,
    openrouter_provider_payload,
    raise_for_provider_status,
    validate_model_provider,
)
from devcouncil.repo.sca import (
    AuditorResult,
    ScaScanner,
    _coerce_str,
    _default_runner,
    _parse_npm_audit,
    _parse_osv_scanner,
    _parse_pip_audit,
)
from devcouncil.storage import db as db_mod
from devcouncil.telemetry.cost import CostEstimator, cost_by_task, group_cost, read_cost_records
from devcouncil.utils.subprocess_env import clean_subprocess_env


class _FakeResponse:
    def __init__(self, status_code=200, data=None, text=""):
        self.status_code = status_code
        self._data = data or {}
        self.text = text

    def json(self):
        return self._data


class _FakeAsyncClient:
    def __init__(self, responses):
        self.responses = list(responses)
        self.posts = []
        self.is_closed = False

    async def post(self, url, headers=None, json=None):
        self.posts.append((url, headers, json))
        return self.responses.pop(0)

    async def aclose(self):
        self.is_closed = True


def test_provider_validation_defaults_payload_and_factory(monkeypatch, tmp_path):
    assert validate_model_provider(" vertex-ai ") == "vertexai"
    with pytest.raises(ValueError, match="Unsupported model provider"):
        validate_model_provider("bad")

    raw = {"models": {"roles": {"planner_a": {"model": provider_mod.DEFAULT_ROLE_MODELS_BY_PROVIDER["openrouter"]["planner_a"]}}}}
    assert apply_provider_default_role_models(raw, "openrouter", "ollama") is True
    assert raw["models"]["roles"]["planner_a"]["model"] == provider_mod.DEFAULT_ROLE_MODELS_BY_PROVIDER["ollama"]["planner_a"]
    assert apply_provider_default_role_models(raw, "openrouter", "ollama") is False

    roles = build_role_model_config("openrouter", model="shared", role_models={"critic_a": "critic"})
    assert set(provider_mod.DEFAULT_ROLE_MODELS_BY_PROVIDER["openrouter"]).issubset(roles)
    assert {role["model"] for role in roles.values()} >= {"shared", "critic"}
    assert openrouter_provider_payload(None) is None
    assert openrouter_provider_payload({"sort": "price", "unknown": "ignored"}) == {"sort": "price"}
    assert openrouter_provider_payload(SimpleNamespace(model_dump=lambda: {"allow_fallbacks": False, "sort": None})) == {
        "allow_fallbacks": False
    }
    assert openrouter_provider_payload(object()) is None

    assert isinstance(create_provider("openrouter", "key", tmp_path), OpenRouterProvider)
    assert create_provider("openrouter", "key", tmp_path, provider_prefs={"sort": "throughput"}).provider_prefs == {
        "sort": "throughput"
    }
    assert create_provider("ollama-local", "", tmp_path).is_local_cost_free() is True
    assert create_provider("doubleword", "key", tmp_path).base_url.endswith("/v1")
    monkeypatch.setenv("VERTEXAI_PROJECT", "proj")
    vertex = create_provider("vertexai", "token", tmp_path)
    assert isinstance(vertex, VertexAIProvider)
    assert "/projects/proj/" in vertex.base_url


def test_raise_for_provider_status_and_response_content_coercion():
    raise_for_provider_status(_FakeResponse(200), "Provider")
    with pytest.raises(ProviderRequestError) as exc:
        raise_for_provider_status(_FakeResponse(402, text="no credits" * 100), "OpenRouter")
    assert exc.value.status_code == 402
    assert "payment required" in str(exc.value)
    assert len(str(exc.value)) < 500

    response = LLMResponse(content=None, model="m", usage={}, raw_response={})
    assert response.content == ""


def test_provider_client_reuse_and_close():
    class Provider(provider_mod.Provider):
        async def complete(self, *args, **kwargs):
            raise NotImplementedError

    async def run():
        p = Provider()
        first = p._get_async_client(1.0)
        second = p._get_async_client(1.0)
        assert first is second
        await p.aclose()
        assert first.is_closed is True

    asyncio.run(run())


def test_openrouter_and_ollama_complete_payloads(monkeypatch, tmp_path):
    async def run_openrouter():
        client = _FakeAsyncClient([
            _FakeResponse(
                data={
                    "choices": [{"message": {"content": '{"ok": true}'}}],
                    "model": "model-a",
                    "usage": {"prompt_tokens": 1, "completion_tokens": 2},
                }
            )
        ])
        provider = OpenRouterProvider("key", project_root=tmp_path, provider_prefs={"sort": "price"})
        monkeypatch.setattr(provider, "_get_async_client", lambda timeout: client)
        messages = [{"role": "user", "content": "hello"}]
        resp = await provider.complete("model-a", messages, json_mode=True, task_id="TASK", run_id="RUN")
        assert resp.content == '{"ok": true}'
        url, headers, payload = client.posts[0]
        assert url.endswith("/chat/completions")
        assert headers["Authorization"] == "Bearer key"
        assert payload["response_format"] == {"type": "json_object"}
        assert payload["provider"] == {"sort": "price"}
        assert messages[0]["content"] == "hello"
        assert "Output must be a valid JSON object" in payload["messages"][0]["content"]
        assert provider.cache_fingerprint().startswith("openrouter:provider=")

    asyncio.run(run_openrouter())

    async def run_ollama():
        client = _FakeAsyncClient([
            _FakeResponse(
                data={
                    "message": {"content": '{"ok": true}'},
                    "prompt_eval_count": 3,
                    "eval_count": 4,
                }
            )
        ])
        provider = OllamaProvider(api_key="proxy", project_root=tmp_path, base_url="http://host:11434/v1", num_ctx=8192)
        monkeypatch.setattr(provider, "_get_async_client", lambda timeout: client)
        messages = [{"role": "user", "content": "hello"}]
        resp = await provider.complete("llama", messages, temperature=0.25, json_mode=True)
        url, headers, payload = client.posts[0]
        assert url == "http://host:11434/api/chat"
        assert headers["Authorization"] == "Bearer proxy"
        assert payload["format"] == "json"
        assert payload["options"] == {"temperature": 0.25, "num_ctx": 8192}
        assert resp.model == "llama"
        assert resp.usage["total_tokens"] == 7
        assert "num_ctx=8192" in provider.cache_fingerprint()

    asyncio.run(run_ollama())


def test_ollama_env_resolution_and_vertex_refresh(monkeypatch, tmp_path):
    monkeypatch.delenv("OLLAMA_BASE_URL", raising=False)
    monkeypatch.delenv("OLLAMA_HOST", raising=False)
    monkeypatch.delenv("OLLAMA_NUM_CTX", raising=False)
    monkeypatch.delenv("OLLAMA_TIMEOUT", raising=False)
    assert OllamaProvider._resolve_base_url() == "http://localhost:11434/v1"
    assert OllamaProvider._resolve_num_ctx() is None
    assert OllamaProvider._resolve_timeout() == OllamaProvider.DEFAULT_TIMEOUT

    monkeypatch.setenv("OLLAMA_HOST", "127.0.0.1:11434")
    monkeypatch.setenv("OLLAMA_NUM_CTX", "4096")
    monkeypatch.setenv("OLLAMA_TIMEOUT", "0")
    assert OllamaProvider._resolve_base_url() == "http://127.0.0.1:11434/v1"
    assert OllamaProvider._resolve_num_ctx() == 4096
    assert OllamaProvider._resolve_timeout() is None
    monkeypatch.setenv("OLLAMA_BASE_URL", "http://explicit/v1/")
    monkeypatch.setenv("OLLAMA_NUM_CTX", "-1")
    monkeypatch.setenv("OLLAMA_TIMEOUT", "bad")
    assert OllamaProvider._resolve_base_url() == "http://explicit/v1"
    assert OllamaProvider._resolve_num_ctx() is None
    assert OllamaProvider._resolve_timeout() == OllamaProvider.DEFAULT_TIMEOUT

    async def run_vertex():
        client = _FakeAsyncClient(
            [
                _FakeResponse(status_code=401, data={}, text="expired"),
                _FakeResponse(
                    data={
                        "choices": [{"message": {"content": "ok"}}],
                        "model": "gemini",
                        "usage": {"prompt_tokens": 1},
                    }
                ),
            ]
        )
        provider = VertexAIProvider("old", project_id="proj", location="us", project_root=tmp_path)
        monkeypatch.setattr(provider, "_get_async_client", lambda timeout: client)
        monkeypatch.setattr("devcouncil.app.config.get_gcloud_access_token", lambda: "new-token")
        resp = await provider.complete("gemini", [{"role": "user", "content": "hi"}], json_mode=True)
        assert resp.content == "ok"
        assert len(client.posts) == 2
        assert client.posts[1][1]["Authorization"] == "Bearer new-token"
        assert provider._headers()["Authorization"] == "Bearer new-token"

    asyncio.run(run_vertex())


def test_mock_provider_sequence_and_model_call_logging(tmp_path):
    async def run():
        provider = MockProvider({"m": ["one", "two"]})
        assert (await provider.complete("m", [])).content == "one"
        assert (await provider.complete("m", [])).content == "two"
        assert (await provider.complete("m", [])).content == "two"
        assert (await provider.complete("unknown", [])).content == '{"mock": "response"}'

    asyncio.run(run())

    provider_mod._log_model_call(
        {"api_key": "sk-abcdefghijklmnopqrst", "messages": [{"content": "hi"}]},
        {"model": "m", "secret": "sk-abcdefghijklmnopqrst"},
        {"prompt_tokens": 1},
        tmp_path,
        task_id="TASK",
        run_id="RUN",
        provider="ollama",
    )
    ledger = tmp_path / ".devcouncil" / "logs" / "model_calls.jsonl"
    content = ledger.read_text(encoding="utf-8")
    assert "sk-abcdefghijklmnopqrst" not in content
    assert '"task_id": "TASK"' in content


def test_sca_parsers_scanner_dedup_and_default_runner(monkeypatch, tmp_path):
    assert _coerce_str(None, "x") == "x"
    assert _coerce_str(7) == "7"
    pip_risks = _parse_pip_audit(
        {
            "dependencies": [
                {"name": "pkg", "version": "1.0", "vulns": [{"id": "PYSEC", "severity": "high", "description": "bad"}]},
                {"name": "ignored", "vulns": "bad-shape"},
            ]
        }
    )
    assert pip_risks[0].as_dict()["advisory_id"] == "PYSEC"
    assert _parse_pip_audit("bad") == []

    npm_risks = _parse_npm_audit(
        {"vulnerabilities": {"left-pad": {"severity": "moderate", "range": "<1", "via": [{"source": 123, "title": "bad"}]}}}
    )
    assert npm_risks[0].package == "left-pad"
    assert _parse_npm_audit([]) == []

    osv_risks = _parse_osv_scanner(
        {
            "results": [
                {
                    "packages": [
                        {
                            "package": {"name": "crate", "version": "0.1"},
                            "vulnerabilities": [{"id": "OSV-1", "summary": "bad", "severity": [{"type": "CVSS_V3", "score": "9.8"}]}],
                        }
                    ]
                }
            ]
        }
    )
    assert osv_risks[0].severity == "CVSS_V3"
    assert _parse_osv_scanner({}) == []

    (tmp_path / "requirements.txt").write_text("pkg==1\n", encoding="utf-8")
    (tmp_path / "package-lock.json").write_text("{}", encoding="utf-8")
    outputs = {
        "pip-audit": json.dumps({"dependencies": [{"name": "pkg", "version": "1", "vulns": [{"id": "DUP"}]}]}),
        "npm": json.dumps({"vulnerabilities": {"pkg": {"range": "1", "via": [{"source": "DUP"}]}}}),
        "osv-scanner": "{bad json",
    }

    def runner(argv, root):
        return AuditorResult(returncode=1, stdout=outputs[argv[0]], stderr="")

    scanner = ScaScanner(tmp_path, auditor_runner=runner)
    assert scanner.available_auditors() == ["pip-audit", "npm", "osv-scanner"]
    risks = scanner.scan()
    assert len(risks) == 1
    assert risks[0].package == "pkg"

    real_runner = _default_runner(timeout=1)
    monkeypatch.setattr(
        "devcouncil.repo.sca.subprocess.run",
        lambda *a, **k: SimpleNamespace(returncode=0, stdout="{}", stderr=""),
    )
    assert real_runner(["tool"], tmp_path).returncode == 0
    monkeypatch.setattr("devcouncil.repo.sca.subprocess.run", lambda *a, **k: (_ for _ in ()).throw(FileNotFoundError("nope")))
    assert real_runner(["missing"], tmp_path).returncode == -1


def test_cost_records_grouping_and_local_zero(monkeypatch, tmp_path):
    log_dir = tmp_path / ".devcouncil" / "logs"
    log_dir.mkdir(parents=True)
    (log_dir / "model_calls.jsonl").write_text(
        "\n".join(
            [
                "{bad",
                json.dumps(
                    {
                        "task_id": "TASK-1",
                        "run_id": "RUN-1",
                        "timestamp": "now",
                        "provider": "ollama",
                        "response": {"model": "mistral:latest"},
                        "usage": {"prompt_tokens": 1000, "completion_tokens": 1000},
                    }
                ),
                json.dumps(
                    {
                        "response": {"model": "unknown-model"},
                        "usage": {"prompt_tokens": 1000, "completion_tokens": 1000},
                    }
                ),
                json.dumps({"response": "bad", "usage": "bad"}),
            ]
        ),
        encoding="utf-8",
    )

    assert CostEstimator.estimate_cost("ollama/qwen", {"prompt_tokens": 999}) == 0.0
    assert CostEstimator.estimate_cost("unknown", {"prompt_tokens": 1000, "completion_tokens": 1000}) == 0.02
    records = read_cost_records(tmp_path)
    assert len(records) == 3
    assert records[0]["cost"] == 0.0
    summary = group_cost(tmp_path)
    assert summary["total_calls"] == 3
    assert summary["by_task"]["TASK-1"]["calls"] == 1
    assert summary["by_task"]["(unattributed)"]["calls"] == 2
    assert cost_by_task(tmp_path) == summary["by_task"]

    monkeypatch.setattr("devcouncil.telemetry.cost._model_calls_file", lambda root: tmp_path / "missing.jsonl")
    assert read_cost_records(tmp_path) == []


def test_storage_get_db_cache_reset_and_schema_version(monkeypatch, tmp_path):
    assert db_mod.get_db(tmp_path) is None
    dev_dir = tmp_path / ".devcouncil"
    dev_dir.mkdir()
    db = db_mod.get_db(tmp_path)
    assert db is db_mod.get_db(tmp_path)
    assert (dev_dir / "state.sqlite").exists()
    with db.get_session() as session:
        current = session.get(db_mod.SchemaVersionModel, "singleton")
        current.version = db_mod.SCHEMA_VERSION + 1
        session.add(current)
    db_mod.reset_db_cache()
    with pytest.raises(RuntimeError, match="Unsupported DevCouncil schema version"):
        db_mod.get_db(tmp_path)

    db_mod.reset_db_cache()
    (dev_dir / "state.sqlite").unlink()
    fresh = db_mod.get_db(tmp_path)
    assert fresh is not None
    db_mod.reset_db_cache()


def test_clean_subprocess_env_strips_current_virtualenv(monkeypatch, tmp_path):
    env_prefix = tmp_path / "venv"
    base_prefix = tmp_path / "base"
    (env_prefix / "bin").mkdir(parents=True)
    (env_prefix / "Scripts").mkdir()
    monkeypatch.setattr("devcouncil.utils.subprocess_env.sys.prefix", str(env_prefix))
    monkeypatch.setattr("devcouncil.utils.subprocess_env.sys.base_prefix", str(base_prefix), raising=False)
    monkeypatch.setenv("PATH", os.pathsep.join([str(env_prefix / "bin"), str(tmp_path / "other"), ""]))
    monkeypatch.setenv("VIRTUAL_ENV", str(env_prefix))
    monkeypatch.setenv("PYTHONHOME", str(base_prefix))
    monkeypatch.setenv("UV_INTERNAL__PYTHONHOME", str(base_prefix))

    cleaned = clean_subprocess_env()

    assert str(env_prefix / "bin") not in cleaned["PATH"]
    assert str(tmp_path / "other") in cleaned["PATH"]
    assert "VIRTUAL_ENV" not in cleaned
    assert "PYTHONHOME" not in cleaned
    assert "UV_INTERNAL__PYTHONHOME" not in cleaned

    monkeypatch.setattr("devcouncil.utils.subprocess_env.sys.base_prefix", str(env_prefix), raising=False)
    monkeypatch.setenv("VIRTUAL_ENV", str(env_prefix))
    no_strip = clean_subprocess_env()
    assert no_strip["VIRTUAL_ENV"] == str(env_prefix)

