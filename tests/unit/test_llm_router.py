import json

import pytest
from pydantic import BaseModel

from devcouncil.llm.provider import LLMResponse, OpenRouterProvider, Provider, create_provider
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


def test_create_provider_builds_openrouter_provider():
    provider = create_provider("openrouter", "sk-test")

    assert isinstance(provider, OpenRouterProvider)
    assert provider.api_key == "sk-test"


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
