import json

import pytest
from pydantic import BaseModel

from devcouncil.app.config import get_api_key
from devcouncil.llm.provider import (
    LLMResponse,
    OpenRouterProvider,
    DoublewordProvider,
    Provider,
    VertexAIProvider,
    apply_provider_default_role_models,
    build_role_model_config,
    create_provider,
    load_default_role_models_by_provider,
    validate_model_provider,
)
from devcouncil.llm.router import ModelRouter, StructuredOutputError


class RouterOutput(BaseModel):
    value: str


class BrokenJsonProvider(Provider):
    """Always returns content that can never validate against RouterOutput,
    so both the initial parse and the healing retry fail."""

    def __init__(self):
        self.calls = 0

    async def complete(self, model, messages, temperature=0.0, json_mode=False, run_id=None, **kwargs):
        self.calls += 1
        return LLMResponse(
            content="not json at all {",
            model=model,
            usage={"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2},
            raw_response={},
        )


def test_complete_structured_raises_structured_output_error_without_fallback(tmp_path):
    router = ModelRouter(BrokenJsonProvider(), {"critic_a": {"model": "weak/free"}}, project_root=tmp_path)
    import asyncio

    with pytest.raises(StructuredOutputError) as excinfo:
        asyncio.run(router.complete_structured(role="critic_a", messages=[{"role": "user", "content": "x"}], schema=RouterOutput))
    assert excinfo.value.role == "critic_a"
    assert excinfo.value.model == "weak/free"


class FlakyProvider(Provider):
    """Returns malformed JSON for the first `fail_first` calls, then valid JSON —
    simulating a model that botches one structured attempt but recovers on a fresh one."""

    def __init__(self, fail_first=2):
        self.calls = 0
        self.fail_first = fail_first

    async def complete(self, model, messages, temperature=0.0, json_mode=False, run_id=None, **kwargs):
        self.calls += 1
        content = "not json {" if self.calls <= self.fail_first else json.dumps({"value": "recovered"})
        return LLMResponse(content=content, model=model,
                           usage={"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2},
                           raw_response={})


def test_complete_structured_retries_fresh_and_recovers(tmp_path):
    import asyncio

    provider = FlakyProvider(fail_first=2)  # first attempt (complete + heal) fail; second succeeds
    router = ModelRouter(provider, {"critic_a": {"model": "weak/flaky"}}, project_root=tmp_path)
    result = asyncio.run(
        router.complete_structured(role="critic_a", messages=[{"role": "user", "content": "x"}], schema=RouterOutput)
    )
    assert result.value == "recovered"
    assert provider.calls >= 3  # proves it made a fresh attempt rather than giving up


def test_complete_structured_returns_fallback_on_failure(tmp_path):
    router = ModelRouter(BrokenJsonProvider(), {"critic_a": {"model": "weak/free"}}, project_root=tmp_path)
    import asyncio

    fallback = RouterOutput(value="degraded")
    result = asyncio.run(
        router.complete_structured(
            role="critic_a",
            messages=[{"role": "user", "content": "x"}],
            schema=RouterOutput,
            fallback=fallback,
        )
    )
    assert result.value == "degraded"


class CountingProvider(Provider):
    def __init__(self):
        self.calls = 0

    async def complete(self, model, messages, temperature=0.0, json_mode=False, run_id=None, **kwargs):
        self.calls += 1
        content = json.dumps({"value": "ok"})
        return LLMResponse(
            content=content,
            model=model,
            usage={"prompt_tokens": 7, "completion_tokens": 3, "total_tokens": 10},
            raw_response={"choices": [{"message": {"content": content}}]},
        )


@pytest.fixture
def anyio_backend():
    return "asyncio"


def test_create_provider_rejects_unsupported_provider():
    with pytest.raises(ValueError, match="Unsupported model provider 'acme'"):
        create_provider("acme", "sk-test")


def test_validate_model_provider_accepts_vertex_aliases():
    assert validate_model_provider("vertexAI") == "vertexai"
    assert validate_model_provider("vertex-ai") == "vertexai"
    assert validate_model_provider("vertex_ai") == "vertexai"


def test_create_provider_builds_openrouter_provider():
    provider = create_provider("openrouter", "sk-test")

    assert isinstance(provider, OpenRouterProvider)
    assert provider.api_key == "sk-test"


def test_create_provider_builds_doubleword_provider():
    provider = create_provider("doubleword", "dw-test")

    assert isinstance(provider, DoublewordProvider)
    assert provider.api_key == "dw-test"
    assert provider.base_url == "https://api.doubleword.ai/v1"


def test_create_provider_builds_vertexai_provider(monkeypatch):
    monkeypatch.setenv("VERTEXAI_PROJECT", "test-project")
    monkeypatch.setenv("VERTEXAI_LOCATION", "us-central1")

    provider = create_provider("vertexai", "ya29.test")

    assert isinstance(provider, VertexAIProvider)
    assert provider.access_token == "ya29.test"
    assert provider.base_url == (
        "https://aiplatform.googleapis.com/v1/projects/test-project"
        "/locations/us-central1/endpoints/openapi"
    )


def test_vertexai_provider_requires_project(monkeypatch):
    monkeypatch.delenv("VERTEXAI_PROJECT", raising=False)
    monkeypatch.delenv("GOOGLE_CLOUD_PROJECT", raising=False)

    provider = create_provider("vertexai", "ya29.test")

    with pytest.raises(ValueError, match="Vertex AI project is not configured"):
        _ = provider.base_url


def test_create_provider_reads_vertexai_project_from_local_secrets(tmp_path, monkeypatch):
    monkeypatch.delenv("VERTEXAI_PROJECT", raising=False)
    monkeypatch.delenv("GOOGLE_CLOUD_PROJECT", raising=False)
    monkeypatch.delenv("VERTEXAI_LOCATION", raising=False)
    secrets = tmp_path / ".devcouncil" / "secrets.env"
    secrets.parent.mkdir()
    secrets.write_text(
        "VERTEXAI_PROJECT=secret-project\nVERTEXAI_LOCATION=us-central1\n",
        encoding="utf-8",
    )

    provider = create_provider("vertexai", "ya29.test", project_root=tmp_path)

    assert isinstance(provider, VertexAIProvider)
    assert provider.project_id == "secret-project"
    assert provider.location == "us-central1"


def test_get_api_key_falls_back_to_gcloud_for_vertexai(tmp_path, monkeypatch):
    monkeypatch.delenv("VERTEXAI_ACCESS_TOKEN", raising=False)
    monkeypatch.setattr("devcouncil.app.config.shutil.which", lambda command: "gcloud" if command == "gcloud" else None)
    monkeypatch.setattr(
        "devcouncil.app.config.subprocess.check_output",
        lambda *args, **kwargs: "ya29.gcloud\n",
    )

    assert get_api_key("vertexai", tmp_path) == "ya29.gcloud"


def test_get_api_key_reports_gcloud_hint_for_missing_vertexai_token(tmp_path, monkeypatch):
    monkeypatch.delenv("VERTEXAI_ACCESS_TOKEN", raising=False)
    monkeypatch.setattr("devcouncil.app.config.shutil.which", lambda command: None)

    with pytest.raises(ValueError, match="gcloud auth login"):
        get_api_key("vertexai", tmp_path)


def test_apply_provider_default_role_models_updates_only_previous_defaults():
    raw_config = {
        "models": {
            "roles": {
                "spec_writer": {"model": "anthropic/claude-sonnet-4.6"},
                "planner_a": {"model": "custom/model"},
            }
        }
    }

    changed = apply_provider_default_role_models(raw_config, "openrouter", "vertexai")

    assert changed is True
    roles = raw_config["models"]["roles"]
    assert roles["spec_writer"]["model"] == "google/gemini-2.5-flash"
    assert roles["planner_a"]["model"] == "custom/model"
    assert roles["live_reviewer"]["model"] == "google/gemini-2.5-flash"


def test_apply_provider_default_role_models_tolerates_unsupported_previous_provider():
    raw_config = {"models": {"roles": {"planner_a": {"model": "custom/model"}}}}

    changed = apply_provider_default_role_models(raw_config, "acme", "vertexai")

    assert changed is True
    roles = raw_config["models"]["roles"]
    assert roles["planner_a"]["model"] == "custom/model"
    assert roles["arbiter"]["model"] == "google/gemini-2.5-flash"


def test_build_role_model_config_applies_shared_and_per_role_models():
    roles = build_role_model_config(
        "vertex-ai",
        model="google/shared-model",
        role_models={"critic_b": "google/critic-model"},
    )

    assert roles["spec_writer"]["model"] == "google/shared-model"
    assert roles["critic_b"]["model"] == "google/critic-model"


def test_default_role_models_are_loaded_from_resource_file():
    defaults = load_default_role_models_by_provider()

    assert "openrouter" in defaults
    assert "vertexai" in defaults
    assert "doubleword" in defaults
    assert "spec_writer" in defaults["openrouter"]


@pytest.mark.anyio
async def test_vertexai_provider_refreshes_gcloud_token_once_on_auth_failure(monkeypatch):
    calls = []

    class FakeResponse:
        def __init__(self, status_code, data=None):
            self.status_code = status_code
            self._data = data or {}

        def raise_for_status(self):
            if self.status_code >= 400:
                raise AssertionError(f"unexpected status {self.status_code}")

        def json(self):
            return self._data

    class FakeClient:
        def __init__(self, timeout):
            self.timeout = timeout

        async def __aenter__(self):
            return self

        async def __aexit__(self, exc_type, exc, tb):
            return False

        async def post(self, url, headers, json):
            calls.append({"url": url, "headers": headers, "json": json})
            if len(calls) == 1:
                return FakeResponse(401)
            return FakeResponse(
                200,
                {
                    "choices": [{"message": {"content": '{"value": "ok"}'}}],
                    "model": "google/gemini-2.0-flash-001",
                    "usage": {"total_tokens": 3},
                },
            )

    monkeypatch.setattr("devcouncil.llm.provider.httpx.AsyncClient", FakeClient)
    monkeypatch.setattr("devcouncil.app.config.shutil.which", lambda command: "gcloud" if command == "gcloud" else None)
    monkeypatch.setattr("devcouncil.app.config.subprocess.check_output", lambda *args, **kwargs: "ya29.fresh\n")

    provider = VertexAIProvider("ya29.expired", project_id="test-project", location="global")

    response = await provider.complete(
        "google/gemini-2.0-flash-001",
        [{"role": "user", "content": "Return JSON"}],
        json_mode=True,
    )

    assert response.content == '{"value": "ok"}'
    assert len(calls) == 2
    assert calls[0]["headers"]["Authorization"] == "Bearer ya29.expired"
    assert calls[1]["headers"]["Authorization"] == "Bearer ya29.fresh"
    assert provider.access_token == "ya29.fresh"


@pytest.mark.anyio
async def test_router_does_not_count_cached_usage_twice(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    provider = CountingProvider()
    router = ModelRouter(provider, {"role": {"model": "openai/gpt-4o", "temperature": 0.0}})
    messages = [{"role": "user", "content": "Return ok"}]

    await router.complete_structured("role", messages, RouterOutput)
    await router.complete_structured("role", messages, RouterOutput)

    assert provider.calls == 1
    telemetry = json.loads((tmp_path / ".devcouncil" / "logs" / "telemetry.json").read_text(encoding="utf-8"))
    assert telemetry["total_prompt_tokens"] == 7
    assert telemetry["total_completion_tokens"] == 3


# --- Per-role provider routing --------------------------------------------------

from devcouncil.app.config import ModelRoleConfig
from devcouncil.llm.provider import OllamaProvider


def test_model_role_config_normalizes_and_validates_provider():
    assert ModelRoleConfig(model="m").provider is None
    assert ModelRoleConfig(model="m", provider="ollama-local").provider == "ollama"
    with pytest.raises(Exception):
        ModelRoleConfig(model="m", provider="nope")


def test_provider_for_role_uses_default_when_unset(tmp_path):
    default = CountingProvider()
    router = ModelRouter(default, {"planner_a": {"model": "x/y"}}, project_root=tmp_path)
    assert router._provider_for_role({"model": "x/y"}) is default


def test_provider_for_role_builds_and_caches_override(tmp_path):
    default = CountingProvider()
    router = ModelRouter(default, {}, project_root=tmp_path)
    cfg = {"model": "ornith", "provider": "ollama"}
    p1 = router._provider_for_role(cfg)
    p2 = router._provider_for_role(cfg)
    assert isinstance(p1, OllamaProvider)
    assert p1 is not default
    assert p1 is p2  # cached per provider name


@pytest.mark.anyio
async def test_router_routes_roles_to_distinct_providers(tmp_path, monkeypatch):
    """Planning role uses the default provider; a role with provider: ollama is
    routed to a separately-built Ollama provider in the same router."""
    monkeypatch.chdir(tmp_path)
    default = CountingProvider()

    captured = {}

    class FakeOllama(OllamaProvider):
        async def complete(self, model, messages, temperature=0.0, json_mode=False, task_id=None, run_id=None):
            captured["ollama_model"] = model
            return LLMResponse(
                content='{"value": "ok"}', model=model,
                usage={"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2},
                raw_response={},
            )

    monkeypatch.setattr("devcouncil.llm.provider.OllamaProvider", FakeOllama)

    router = ModelRouter(
        default,
        {
            "planner_a": {"model": "or/planner"},
            "live_reviewer": {"model": "ornith", "provider": "ollama"},
        },
        project_root=tmp_path,
    )

    await router.complete_structured("planner_a", [{"role": "user", "content": "x"}], RouterOutput)
    await router.complete_structured("live_reviewer", [{"role": "user", "content": "x"}], RouterOutput)

    assert default.calls == 1  # only the planning role hit the default provider
    assert captured["ollama_model"] == "ornith"  # the override role hit Ollama
