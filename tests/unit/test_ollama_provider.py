"""Ollama (local) LLM provider — wiring, no-API-key path, and HTTP behavior.

These tests mock the httpx layer; they never require a running Ollama server.
"""

import pytest

from devcouncil.app.config import get_api_key, provider_api_key_env_var
from devcouncil.llm.provider import (
    OllamaProvider,
    build_role_model_config,
    create_provider,
    validate_model_provider,
)
from devcouncil.telemetry.cost import CostEstimator


@pytest.fixture
def anyio_backend():
    return "asyncio"


class FakeResponse:
    def __init__(self, status_code, data=None, text=""):
        self.status_code = status_code
        self._data = data or {}
        self.text = text

    def json(self):
        return self._data


def make_fake_client(calls, response):
    class FakeClient:
        def __init__(self, timeout):
            self.timeout = timeout

        async def __aenter__(self):
            return self

        async def __aexit__(self, exc_type, exc, tb):
            return False

        async def post(self, url, headers, json):
            calls.append({"url": url, "headers": headers, "json": json})
            return response

    return FakeClient


# --- registration / validation ---------------------------------------------


def test_validate_model_provider_accepts_ollama():
    assert validate_model_provider("ollama") == "ollama"
    assert validate_model_provider("OLLAMA") == "ollama"
    assert validate_model_provider("ollama-local") == "ollama"


def test_create_provider_builds_ollama_provider_without_key():
    provider = create_provider("ollama", "")

    assert isinstance(provider, OllamaProvider)
    assert provider.api_key == ""
    assert provider.base_url == "http://localhost:11434/v1"


def test_build_role_model_config_for_ollama_uses_local_default():
    roles = build_role_model_config("ollama")

    assert roles, "ollama must have packaged role defaults"
    assert all(cfg["model"] == "qwen2.5-coder:7b" for cfg in roles.values())
    # mirrors the role set of other providers
    assert "planner_a" in roles and "live_reviewer" in roles


def test_build_role_model_config_for_ollama_honors_shared_model():
    roles = build_role_model_config("ollama", model="llama3.1")
    assert all(cfg["model"] == "llama3.1" for cfg in roles.values())


# --- no-API-key path --------------------------------------------------------


def test_get_api_key_returns_empty_for_ollama_without_key(tmp_path, monkeypatch):
    monkeypatch.delenv("OLLAMA_API_KEY", raising=False)
    assert get_api_key("ollama", tmp_path) == ""


def test_get_api_key_passes_through_explicit_ollama_key(tmp_path, monkeypatch):
    monkeypatch.setenv("OLLAMA_API_KEY", "proxy-token")
    assert get_api_key("ollama", tmp_path) == "proxy-token"


def test_other_providers_still_require_a_key(tmp_path, monkeypatch):
    monkeypatch.delenv("OPENROUTER_API_KEY", raising=False)
    with pytest.raises(ValueError):
        get_api_key("openrouter", tmp_path)


def test_provider_api_key_env_var_for_ollama():
    assert provider_api_key_env_var("ollama") == "OLLAMA_API_KEY"


# --- base URL resolution ----------------------------------------------------


def test_base_url_from_ollama_base_url_env(monkeypatch):
    monkeypatch.setenv("OLLAMA_BASE_URL", "http://remote:9999/v1")
    monkeypatch.delenv("OLLAMA_HOST", raising=False)
    assert OllamaProvider().base_url == "http://remote:9999/v1"


def test_base_url_normalizes_ollama_host(monkeypatch):
    monkeypatch.delenv("OLLAMA_BASE_URL", raising=False)
    monkeypatch.setenv("OLLAMA_HOST", "127.0.0.1:11434")
    assert OllamaProvider().base_url == "http://127.0.0.1:11434/v1"


def test_base_url_explicit_arg_wins(monkeypatch):
    monkeypatch.setenv("OLLAMA_BASE_URL", "http://env:1/v1")
    assert OllamaProvider(base_url="http://explicit:2/v1").base_url == "http://explicit:2/v1"


# --- complete() HTTP behavior ----------------------------------------------


def _native_response(content="hello", model="qwen2.5-coder:7b", pe=5, ec=7):
    # Ollama native /api/chat response shape.
    data = {"message": {"role": "assistant", "content": content}, "done": True}
    if model is not None:
        data["model"] = model
    data["prompt_eval_count"] = pe
    data["eval_count"] = ec
    return FakeResponse(200, data)


@pytest.mark.anyio
async def test_complete_posts_to_native_chat_endpoint_without_auth(tmp_path, monkeypatch):
    monkeypatch.delenv("OLLAMA_BASE_URL", raising=False)
    monkeypatch.delenv("OLLAMA_HOST", raising=False)
    monkeypatch.delenv("OLLAMA_NUM_CTX", raising=False)
    calls = []
    monkeypatch.setattr(
        "devcouncil.llm.provider.httpx.AsyncClient", make_fake_client(calls, _native_response())
    )

    provider = OllamaProvider(api_key="", project_root=tmp_path)
    resp = await provider.complete("qwen2.5-coder:7b", [{"role": "user", "content": "hi"}])

    assert len(calls) == 1
    # native endpoint, derived from the /v1 base by stripping the suffix
    assert calls[0]["url"] == "http://localhost:11434/api/chat"
    assert calls[0]["json"]["stream"] is False
    assert "Authorization" not in calls[0]["headers"]
    assert resp.content == "hello"
    assert resp.model == "qwen2.5-coder:7b"
    # native token counts mapped to OpenAI-style keys
    assert resp.usage == {"prompt_tokens": 5, "completion_tokens": 7, "total_tokens": 12}


@pytest.mark.anyio
async def test_complete_honors_json_mode_native_format(tmp_path, monkeypatch):
    calls = []
    monkeypatch.setattr(
        "devcouncil.llm.provider.httpx.AsyncClient",
        make_fake_client(calls, _native_response(content="{}")),
    )
    provider = OllamaProvider(api_key="", base_url="http://localhost:11434/v1", project_root=tmp_path)
    await provider.complete(
        "qwen2.5-coder:7b", [{"role": "user", "content": "give json"}], json_mode=True
    )
    body = calls[0]["json"]
    assert body["format"] == "json"
    assert body["messages"][-1]["content"].endswith("Output must be a valid JSON object.")


@pytest.mark.anyio
async def test_num_ctx_from_env_passed_in_options(tmp_path, monkeypatch):
    monkeypatch.setenv("OLLAMA_NUM_CTX", "16384")
    calls = []
    monkeypatch.setattr(
        "devcouncil.llm.provider.httpx.AsyncClient", make_fake_client(calls, _native_response())
    )
    provider = OllamaProvider(project_root=tmp_path)
    await provider.complete("m", [{"role": "user", "content": "hi"}], temperature=0.2)
    options = calls[0]["json"]["options"]
    assert options["num_ctx"] == 16384
    assert options["temperature"] == 0.2


@pytest.mark.anyio
async def test_num_ctx_absent_when_unset(tmp_path, monkeypatch):
    monkeypatch.delenv("OLLAMA_NUM_CTX", raising=False)
    calls = []
    monkeypatch.setattr(
        "devcouncil.llm.provider.httpx.AsyncClient", make_fake_client(calls, _native_response())
    )
    provider = OllamaProvider(project_root=tmp_path)
    await provider.complete("m", [{"role": "user", "content": "hi"}])
    assert "num_ctx" not in calls[0]["json"]["options"]


def test_chat_endpoint_derivation():
    assert OllamaProvider(base_url="http://localhost:11434/v1")._chat_endpoint() == "http://localhost:11434/api/chat"
    assert OllamaProvider(base_url="http://remote:9999")._chat_endpoint() == "http://remote:9999/api/chat"


@pytest.mark.anyio
async def test_complete_falls_back_to_requested_model_when_omitted(tmp_path, monkeypatch):
    calls = []
    monkeypatch.setattr(
        "devcouncil.llm.provider.httpx.AsyncClient",
        make_fake_client(calls, _native_response(content="x", model=None, pe=0, ec=0)),
    )
    provider = OllamaProvider(project_root=tmp_path)
    resp = await provider.complete("llama3.1", [{"role": "user", "content": "hi"}])
    assert resp.model == "llama3.1"
    assert resp.usage == {"prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0}


@pytest.mark.anyio
async def test_complete_sends_authorization_when_key_present(tmp_path, monkeypatch):
    calls = []
    monkeypatch.setattr(
        "devcouncil.llm.provider.httpx.AsyncClient",
        make_fake_client(calls, _native_response(model="m")),
    )
    provider = OllamaProvider(api_key="proxy-token", base_url="http://localhost:11434/v1", project_root=tmp_path)
    await provider.complete("m", [{"role": "user", "content": "hi"}])
    assert calls[0]["headers"]["Authorization"] == "Bearer proxy-token"


# --- num_ctx resolution + doctor smoke -------------------------------------


def test_resolve_num_ctx(monkeypatch):
    monkeypatch.delenv("OLLAMA_NUM_CTX", raising=False)
    assert OllamaProvider._resolve_num_ctx() is None
    monkeypatch.setenv("OLLAMA_NUM_CTX", "16384")
    assert OllamaProvider._resolve_num_ctx() == 16384
    monkeypatch.setenv("OLLAMA_NUM_CTX", "0")
    assert OllamaProvider._resolve_num_ctx() is None
    monkeypatch.setenv("OLLAMA_NUM_CTX", "not-an-int")
    assert OllamaProvider._resolve_num_ctx() is None


@pytest.mark.parametrize("num_ctx", [None, "16384"])
def test_doctor_ollama_branch_runs(tmp_path, monkeypatch, num_ctx):
    # Guards the doctor ollama branch (num_ctx warning path) against import/runtime
    # errors for both unset and set context.
    from devcouncil.cli.commands.doctor import render_doctor_check
    from devcouncil.cli.commands.init import initialize_project

    if num_ctx is None:
        monkeypatch.delenv("OLLAMA_NUM_CTX", raising=False)
    else:
        monkeypatch.setenv("OLLAMA_NUM_CTX", num_ctx)
    initialize_project(tmp_path, model_provider="ollama", with_map=False, with_skills=False)
    render_doctor_check(tmp_path)  # must not raise


# --- cost is $0 for local models -------------------------------------------


def test_local_models_cost_zero():
    usage = {"prompt_tokens": 1000, "completion_tokens": 1000}
    assert CostEstimator.estimate_cost("qwen2.5-coder:7b", usage) == 0.0
    assert CostEstimator.estimate_cost("ollama/llama3", usage) == 0.0
    assert CostEstimator.estimate_cost("ollama:custom-tag", usage) == 0.0
