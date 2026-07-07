import json

import pytest
from pydantic import BaseModel

from devcouncil.app.config import ModelRoleConfig, ProviderConfig, get_api_key
from devcouncil.llm.provider import (
    LLMResponse,
    OllamaProvider,
    OpenRouterProvider,
    DoublewordProvider,
    Provider,
    VertexAIProvider,
    apply_provider_default_role_models,
    build_role_model_config,
    create_provider,
    load_default_role_models_by_provider,
    openrouter_provider_payload,
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
    # Reference the shipped defaults instead of hard-coding model ids, so the test
    # keeps validating the swap behavior when the default models are rotated.
    from devcouncil.llm.provider import DEFAULT_ROLE_MODELS_BY_PROVIDER

    openrouter_default = DEFAULT_ROLE_MODELS_BY_PROVIDER["openrouter"]["spec_writer"]
    vertexai_defaults = DEFAULT_ROLE_MODELS_BY_PROVIDER["vertexai"]
    raw_config = {
        "models": {
            "roles": {
                "spec_writer": {"model": openrouter_default},
                "planner_a": {"model": "custom/model"},
            }
        }
    }

    changed = apply_provider_default_role_models(raw_config, "openrouter", "vertexai")

    assert changed is True
    roles = raw_config["models"]["roles"]
    assert roles["spec_writer"]["model"] == vertexai_defaults["spec_writer"]
    assert roles["planner_a"]["model"] == "custom/model"
    assert roles["live_reviewer"]["model"] == vertexai_defaults["live_reviewer"]


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


# --- OpenRouter provider routing preferences ------------------------------------


class _FakeResp:
    status_code = 200
    text = ""
    def json(self):
        return {"choices": [{"message": {"content": "{}"}}], "model": "m", "usage": {}}


def _fake_client_factory(calls):
    class _C:
        def __init__(self, timeout=None):
            self.is_closed = False
        async def post(self, url, headers, json):
            calls.append(json)
            return _FakeResp()
    return _C


def test_openrouter_provider_payload_maps_and_filters():
    assert openrouter_provider_payload(None) is None
    assert openrouter_provider_payload(ProviderConfig()) == {
        "sort": "price", "allow_fallbacks": True, "require_parameters": True, "data_collection": "deny",
    }
    # unknown keys are dropped; None values omitted
    assert openrouter_provider_payload({"sort": "throughput", "bogus": 1, "data_collection": None}) == {"sort": "throughput"}


@pytest.mark.anyio
async def test_openrouter_sends_provider_prefs_in_payload(tmp_path, monkeypatch):
    calls = []
    monkeypatch.setattr("devcouncil.llm.provider.httpx.AsyncClient", _fake_client_factory(calls))
    p = OpenRouterProvider("k", project_root=tmp_path, provider_prefs=ProviderConfig(sort="throughput"))
    await p.complete("some/model", [{"role": "user", "content": "hi"}])
    assert calls[0]["provider"]["sort"] == "throughput"
    assert calls[0]["provider"]["data_collection"] == "deny"


@pytest.mark.anyio
async def test_openrouter_omits_provider_field_when_unset(tmp_path, monkeypatch):
    calls = []
    monkeypatch.setattr("devcouncil.llm.provider.httpx.AsyncClient", _fake_client_factory(calls))
    p = OpenRouterProvider("k", project_root=tmp_path)  # no prefs
    await p.complete("some/model", [{"role": "user", "content": "hi"}])
    assert "provider" not in calls[0]


# --- Schema-echo robustness -----------------------------------------------------

def test_looks_like_schema_echo_detects_schema_documents():
    from devcouncil.llm.router import ModelRouter as _MR
    # A genuine instance is not flagged.
    assert _MR._looks_like_schema_echo('{"value": "ok"}') is False
    # Echoed schema docs are flagged on their marker keys.
    assert _MR._looks_like_schema_echo('{"$defs": {}, "properties": {}, "type": "object"}') is True
    assert _MR._looks_like_schema_echo('{"properties": {"x": {"type": "string"}}}') is True
    assert _MR._looks_like_schema_echo("not json") is False
    assert _MR._looks_like_schema_echo('[1,2,3]') is False


@pytest.mark.anyio
async def test_healing_recovers_from_schema_echo(tmp_path, monkeypatch):
    """First response echoes the schema (parses, fails validation); the healing retry
    returns a valid instance and is accepted."""
    monkeypatch.chdir(tmp_path)

    class SchemaEchoThenValid(Provider):
        def __init__(self):
            self.calls = 0
            self.prompts = []
        async def complete(self, model, messages, temperature=0.0, json_mode=False, task_id=None, run_id=None):
            self.calls += 1
            self.prompts.append(messages[-1]["content"])
            if self.calls == 1:
                content = '{"$defs": {}, "properties": {"value": {"type": "string"}}, "type": "object"}'
            else:
                content = '{"value": "healed"}'
            return LLMResponse(content=content, model=model,
                               usage={"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2},
                               raw_response={})

    provider = SchemaEchoThenValid()
    router = ModelRouter(provider, {"critic_a": {"model": "weak/local"}}, project_root=tmp_path)
    result = await router.complete_structured("critic_a", [{"role": "user", "content": "x"}], RouterOutput)
    assert result.value == "healed"
    # The healing prompt called out the schema-echo explicitly.
    assert any("returned the JSON *schema*" in p for p in provider.prompts)


@pytest.mark.anyio
async def test_role_override_to_openrouter_inherits_routing_prefs(tmp_path):
    """A role overriding to OpenRouter (while the default provider is something else)
    still picks up the project's provider-routing prefs from config."""
    _write = (tmp_path / ".devcouncil")
    _write.mkdir(parents=True)
    (_write / "secrets.env").write_text("OPENROUTER_API_KEY=sk-or-test\n", encoding="utf-8")
    (_write / "config.yaml").write_text(
        "provider:\n  sort: throughput\n  data_collection: deny\n"
        "models:\n  provider: ollama\n  roles:\n    critic_a:\n      model: x\n      provider: openrouter\n",
        encoding="utf-8",
    )
    # Default provider is a dummy; the override should build a real OpenRouter provider.
    router = ModelRouter(CountingProvider(), {"critic_a": {"model": "x", "provider": "openrouter"}}, project_root=tmp_path)
    p = router._provider_for_role({"model": "x", "provider": "openrouter"})
    # ProviderConfig fills defaults, so all four routing keys are present; the explicitly
    # set ones reflect the config.
    assert p.provider_prefs["sort"] == "throughput"
    assert p.provider_prefs["data_collection"] == "deny"


# --- benchmark-surfaced fixes: timeout fail-fast + schema-aware healing -------


class TimeoutProvider(Provider):
    """Every call raises an httpx timeout with an EMPTY message — the shape that
    previously produced the useless "failed (attempt 1/3): ." log and a blind
    600s-per-attempt retry loop."""

    def __init__(self):
        self.calls = 0

    async def complete(self, model, messages, temperature=0.0, json_mode=False, run_id=None, **kwargs):
        import httpx

        self.calls += 1
        raise httpx.ReadTimeout("")


def test_timeout_fails_fast_without_blind_retries(tmp_path):
    """A provider timeout must NOT be retried with the identical request: each
    retry costs another full provider window (600s on local Ollama) and the
    structured-output layers stack those stalls past external kill timeouts
    (benchmark arms died at exit 124 without ever producing a verdict). It must
    surface immediately as an actionable ProviderRequestError."""
    import asyncio

    from devcouncil.llm.provider import ProviderRequestError

    provider = TimeoutProvider()
    router = ModelRouter(provider, {"critic_a": {"model": "local/slow"}}, project_root=tmp_path)
    with pytest.raises(ProviderRequestError) as excinfo:
        asyncio.run(
            router.complete_structured(
                role="critic_a", messages=[{"role": "user", "content": "x"}], schema=RouterOutput
            )
        )
    assert provider.calls == 1  # no blind identical-request retries
    message = str(excinfo.value)
    # Actionable: names the knobs that actually fix a too-slow local model.
    assert "OLLAMA_TIMEOUT" in message and "OLLAMA_THINK" in message


class MissingFieldThenHealedProvider(Provider):
    """First response is valid JSON but omits a required field (the
    gemini-2.5-flash failure mode on providers without grammar-constrained
    decoding); the healing call — which must now carry the schema — succeeds."""

    def __init__(self):
        self.calls = 0
        self.healing_messages = None

    async def complete(self, model, messages, temperature=0.0, json_mode=False, run_id=None, **kwargs):
        self.calls += 1
        if self.calls == 1:
            return LLMResponse(content=json.dumps({}), model=model,
                               usage={"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2},
                               raw_response={})
        self.healing_messages = messages
        return LLMResponse(content=json.dumps({"value": "healed"}), model=model,
                           usage={"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2},
                           raw_response={})


@pytest.mark.anyio
async def test_healing_call_includes_schema(tmp_path):
    """The healing request must include the schema instruction: a "Field
    required" repair is impossible when the model cannot see which fields the
    schema requires (previously the healing prompt carried only the error text
    and the bad content, so missing-field failures healed into the same
    missing-field output and crashed planning runs)."""
    provider = MissingFieldThenHealedProvider()
    router = ModelRouter(provider, {"critic_a": {"model": "weak/cloud"}}, project_root=tmp_path)
    result = await router.complete_structured("critic_a", [{"role": "user", "content": "x"}], RouterOutput)
    assert result.value == "healed"
    combined = "\n".join(m["content"] for m in provider.healing_messages)
    assert "INSTANCE of this schema" in combined  # schema instruction present
    assert '"value"' in combined  # the actual schema fields are visible


def test_planning_schemas_tolerate_omitted_empty_lists():
    """Models on non-grammar-constrained providers omit empty list fields instead
    of sending []. That is not a planning failure: SpecOutput/ArbiterDecision/
    PlanOutput must default such fields, while genuinely required content
    (requirements / final_tasks / tasks) still fails validation when missing."""
    from pydantic import ValidationError

    from devcouncil.planning.arbiter_service import ArbiterDecision
    from devcouncil.planning.plan_service import PlanOutput
    from devcouncil.planning.spec_service import SpecOutput

    req = {
        "id": "REQ-1", "title": "t", "description": "d", "priority": "high",
        # "source" omitted: defaults to "planner" (weak models drop provenance)
    }
    spec = SpecOutput.model_validate({"requirements": [req]})
    assert spec.assumptions == [] and spec.blocking_questions == []
    assert spec.requirements[0].source == "planner"

    decision = ArbiterDecision.model_validate(
        {"final_requirements": [req], "final_tasks": []}
    )
    assert decision.accepted_finding_ids == [] and decision.rejected_finding_ids == []

    plan = PlanOutput.model_validate({"tasks": []})
    assert plan.id == "PLAN" and plan.rationale == ""

    with pytest.raises(ValidationError):
        SpecOutput.model_validate({})  # requirements stays required
    with pytest.raises(ValidationError):
        ArbiterDecision.model_validate({"final_requirements": [req]})  # final_tasks required
    with pytest.raises(ValidationError):
        PlanOutput.model_validate({"id": "PLAN-A", "rationale": "r"})  # tasks required


# --- OpenRouter structured outputs (json_schema response_format) ---------------


class _FakeRespSeq:
    """Fake client returning queued responses (status, body) in order."""

    def __init__(self, calls, responses):
        self._calls = calls
        self._responses = responses

    def __call__(self, timeout=None):
        outer = self

        class _C:
            is_closed = False

            async def post(self, url, headers, json):
                import copy as _copy

                # Snapshot: the degrade path mutates the payload dict in place,
                # so appending the live reference would alias every entry.
                outer._calls.append(_copy.deepcopy(json))
                status, body = outer._responses.pop(0)
                resp = _FakeResp()
                resp.status_code = status
                resp.text = "" if status < 400 else "unsupported response_format"
                if body is not None:
                    resp.json = lambda: body
                return resp

        return _C()


_OK_BODY = {"choices": [{"message": {"content": "{}"}}], "model": "m", "usage": {}}


@pytest.mark.anyio
async def test_openrouter_sends_json_schema_response_format(tmp_path, monkeypatch):
    # With a schema available, OpenRouter must request schema-CONSTRAINED output:
    # on plain json_object, cloud models routinely omit fields that would be
    # empty (gemini-2.5-flash dropping empty list fields crashed planning runs).
    calls = []
    monkeypatch.setattr(
        "devcouncil.llm.provider.httpx.AsyncClient", _FakeRespSeq(calls, [(200, _OK_BODY)])
    )
    p = OpenRouterProvider("k", project_root=tmp_path)
    schema = {"type": "object", "properties": {"value": {"type": "string"}}}
    await p.complete("some/model", [{"role": "user", "content": "hi"}], json_mode=True, json_schema=schema)
    rf = calls[0]["response_format"]
    assert rf["type"] == "json_schema"
    assert rf["json_schema"]["schema"] == schema
    assert rf["json_schema"]["strict"] is True


@pytest.mark.anyio
async def test_openrouter_json_schema_rejected_degrades_and_is_remembered(tmp_path, monkeypatch):
    # A model/route that rejects json_schema degrades once to json_object and
    # never pays the retry again for that model; other models still try it.
    calls = []
    monkeypatch.setattr(
        "devcouncil.llm.provider.httpx.AsyncClient",
        _FakeRespSeq(calls, [(400, None), (200, _OK_BODY), (200, _OK_BODY), (200, _OK_BODY)]),
    )
    p = OpenRouterProvider("k", project_root=tmp_path)
    schema = {"type": "object", "properties": {"value": {"type": "string"}}}
    await p.complete("weak/model", [{"role": "user", "content": "hi"}], json_mode=True, json_schema=schema)
    assert calls[0]["response_format"]["type"] == "json_schema"
    assert calls[1]["response_format"]["type"] == "json_object"

    await p.complete("weak/model", [{"role": "user", "content": "again"}], json_mode=True, json_schema=schema)
    assert calls[2]["response_format"]["type"] == "json_object"  # remembered

    await p.complete("strong/model", [{"role": "user", "content": "hi"}], json_mode=True, json_schema=schema)
    assert calls[3]["response_format"]["type"] == "json_schema"  # per-model, not global


@pytest.mark.anyio
async def test_openrouter_json_mode_without_schema_unchanged(tmp_path, monkeypatch):
    calls = []
    monkeypatch.setattr(
        "devcouncil.llm.provider.httpx.AsyncClient", _FakeRespSeq(calls, [(200, _OK_BODY)])
    )
    p = OpenRouterProvider("k", project_root=tmp_path)
    await p.complete("some/model", [{"role": "user", "content": "hi"}], json_mode=True)
    assert calls[0]["response_format"] == {"type": "json_object"}


# --- degradable roles fall back on provider errors ------------------------------


def test_provider_error_uses_fallback_for_degradable_role(tmp_path):
    """A role with a fallback must degrade on a provider failure (fail-fast
    timeout, exhausted retries) exactly as on unparseable output — a critique/
    rebuttal role crashing an entire planning run over one slow monitor call is
    the failure mode, not the feature."""
    import asyncio

    provider = TimeoutProvider()
    router = ModelRouter(provider, {"critic_a": {"model": "local/slow"}}, project_root=tmp_path)
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
    assert provider.calls == 1  # no retry storm against a timing-out model


# --- provider response-shape robustness ------------------------------------------


class _ShapeResp:
    """Response with a configurable body; status 200."""

    def __init__(self, body=None, text=""):
        self.status_code = 200
        self.text = text
        self._body = body

    def json(self):
        if self._body is None:
            raise ValueError("not json")
        return self._body


def _client_returning(resp):
    class _C:
        def __init__(self, timeout=None):
            self.is_closed = False

        async def post(self, url, headers, json):
            return resp

    return _C


@pytest.mark.anyio
async def test_openrouter_error_body_without_choices_raises_actionable_error(tmp_path, monkeypatch):
    """OpenRouter can return HTTP 200 whose body is an ``error`` object instead of
    choices (upstream failure/moderation). That must surface as ProviderRequestError
    (which the CLI catches gracefully) carrying the provider's message — not a raw
    KeyError traceback."""
    from devcouncil.llm.provider import ProviderRequestError

    body = {"error": {"message": "upstream provider is overloaded", "code": 502}}
    monkeypatch.setattr(
        "devcouncil.llm.provider.httpx.AsyncClient", _client_returning(_ShapeResp(body=body))
    )
    p = OpenRouterProvider("k", project_root=tmp_path)
    with pytest.raises(ProviderRequestError, match="overloaded"):
        await p.complete("some/model", [{"role": "user", "content": "hi"}])


@pytest.mark.anyio
async def test_provider_non_json_body_raises_actionable_error(tmp_path, monkeypatch):
    """A proxy HTML error page / empty body on HTTP 200 must become an actionable
    ProviderRequestError, not a JSONDecodeError only debuggable from a traceback."""
    from devcouncil.llm.provider import ProviderRequestError

    monkeypatch.setattr(
        "devcouncil.llm.provider.httpx.AsyncClient",
        _client_returning(_ShapeResp(body=None, text="<html>Bad Gateway</html>")),
    )
    p = OpenRouterProvider("k", project_root=tmp_path)
    with pytest.raises(ProviderRequestError, match="non-JSON"):
        await p.complete("some/model", [{"role": "user", "content": "hi"}])


def test_provider_retry_delay_backoff():
    from devcouncil.llm.provider import ProviderRequestError
    from devcouncil.llm.router import _provider_retry_delay

    assert _provider_retry_delay(ProviderRequestError("x", status_code=429), 0) == 15.0
    assert _provider_retry_delay(ProviderRequestError("x", status_code=429), 2) == 60.0
    assert _provider_retry_delay(
        ProviderRequestError("x", status_code=429, retry_after_seconds=45), 0
    ) == 45.0
    assert _provider_retry_delay(RuntimeError("other"), 1) == 2.0


def test_openrouter_max_concurrency_from_env(monkeypatch):
    monkeypatch.delenv("OPENROUTER_MAX_CONCURRENCY", raising=False)
    assert OpenRouterProvider._resolve_max_concurrency() == 3
    monkeypatch.setenv("OPENROUTER_MAX_CONCURRENCY", "5")
    assert OpenRouterProvider._resolve_max_concurrency() == 5
    monkeypatch.setenv("OPENROUTER_MAX_CONCURRENCY", "0")
    assert OpenRouterProvider._resolve_max_concurrency() is None


@pytest.mark.anyio
async def test_openrouter_concurrency_limits_in_flight(tmp_path, monkeypatch):
    import asyncio

    in_flight = 0
    peak = 0

    class SlowResp:
        status_code = 200

        def json(self):
            return {
                "model": "m",
                "choices": [{"message": {"content": "ok"}}],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1},
            }

        def raise_for_status(self):
            return None

    class SlowClient:
        def __init__(self, timeout=None):
            self.is_closed = False

        async def post(self, url, headers, json):
            nonlocal in_flight, peak
            in_flight += 1
            peak = max(peak, in_flight)
            await asyncio.sleep(0.05)
            in_flight -= 1
            return SlowResp()

    monkeypatch.setattr("devcouncil.llm.provider.httpx.AsyncClient", SlowClient)
    monkeypatch.setenv("OPENROUTER_MAX_CONCURRENCY", "2")
    p = OpenRouterProvider("k", project_root=tmp_path)
    await asyncio.gather(*[
        p.complete("m", [{"role": "user", "content": "hi"}])
        for _ in range(6)
    ])
    assert peak <= 2


# --- benchmark-surfaced fix: 429s get their own retry budget -------------------


class RateLimitedThenOkProvider(Provider):
    """Raises 429 more times than the generic attempt budget (5) allows, then
    succeeds — the OpenRouter limit_rpm burst that previously killed runs."""

    def __init__(self, failures: int):
        self.calls = 0
        self.failures = failures

    async def complete(self, model, messages, temperature=0.0, json_mode=False, run_id=None, **kwargs):
        from devcouncil.llm.provider import ProviderRequestError

        self.calls += 1
        if self.calls <= self.failures:
            raise ProviderRequestError(
                "rate limited", status_code=429, retry_after_seconds=0.01
            )
        return LLMResponse(content=json.dumps({"value": "ok"}), model=model, usage={}, raw_response={})


def test_rate_limits_have_their_own_retry_budget(tmp_path, monkeypatch):
    """Six consecutive 429s exceed the generic 5-attempt budget but must still
    succeed: rate limiting is the provider saying WHEN to come back, not a fault
    in the request (observed: benchmark tasks dying blocked on limit_rpm)."""
    import asyncio

    provider = RateLimitedThenOkProvider(failures=6)
    router = ModelRouter(provider, {"critic_a": {"model": "m"}}, project_root=tmp_path)
    result = asyncio.run(
        router.complete_structured(
            role="critic_a", messages=[{"role": "user", "content": "x"}], schema=RouterOutput
        )
    )
    assert result.value == "ok"
    assert provider.calls == 7


def test_rate_limit_budget_is_bounded(tmp_path, monkeypatch):
    """A hard quota (429s that never stop) must still fail in bounded time."""
    import asyncio

    from devcouncil.llm.provider import ProviderRequestError

    monkeypatch.setenv("DEVCOUNCIL_RATE_LIMIT_RETRIES", "1")
    provider = RateLimitedThenOkProvider(failures=100)
    router = ModelRouter(provider, {"critic_a": {"model": "m"}}, project_root=tmp_path)
    with pytest.raises(ProviderRequestError):
        asyncio.run(
            router.complete_structured(
                role="critic_a", messages=[{"role": "user", "content": "x"}], schema=RouterOutput
            )
        )
    # 1 rate-limit retry, then the remaining 429s consume the generic budget
    # (5 attempts); the exhausted error propagates without fresh structured passes.
    assert provider.calls == 6
