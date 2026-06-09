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
from devcouncil.llm.router import ModelRouter


class RouterOutput(BaseModel):
    value: str


class CountingProvider(Provider):
    def __init__(self):
        self.calls = 0

    async def complete(self, model, messages, temperature=0.0, json_mode=False):
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
