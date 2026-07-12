from abc import ABC, abstractmethod
import contextlib
import copy
from functools import lru_cache
from importlib import resources
import logging
import os
from typing import TYPE_CHECKING, List, Dict, Any, Optional, cast
from pydantic import BaseModel, field_validator

if TYPE_CHECKING:
    import asyncio
import httpx
import json
from pathlib import Path
import yaml

logger = logging.getLogger(__name__)

SUPPORTED_MODEL_PROVIDERS = ("openrouter", "vertexai", "doubleword", "ollama")
PROVIDER_ALIASES = {
    "vertex-ai": "vertexai",
    "vertex_ai": "vertexai",
    "ollama-local": "ollama",
    "ollama_local": "ollama",
}
MODEL_DEFAULTS_RESOURCE = "model_defaults.yaml"


@lru_cache(maxsize=1)
def load_default_role_models_by_provider() -> Dict[str, Dict[str, str]]:
    data = resources.files(__package__).joinpath(MODEL_DEFAULTS_RESOURCE).read_text(encoding="utf-8")
    loaded = yaml.safe_load(data) or {}
    return {
        str(provider): {str(role): str(model) for role, model in roles.items()}
        for provider, roles in loaded.items()
        if isinstance(roles, dict)
    }


DEFAULT_ROLE_MODELS_BY_PROVIDER = load_default_role_models_by_provider()


class ProviderRequestError(RuntimeError):
    """A provider HTTP request failed, with an actionable, user-facing message."""

    def __init__(
        self,
        message: str,
        status_code: int | None = None,
        retry_after_seconds: float | None = None,
    ):
        super().__init__(message)
        self.status_code = status_code
        self.retry_after_seconds = retry_after_seconds


def _parse_retry_after(response: "httpx.Response") -> float | None:
    headers = getattr(response, "headers", None)
    if headers is None:
        return None
    raw = headers.get("retry-after") or headers.get("Retry-After")
    if raw is None:
        return None
    try:
        return max(0.0, float(raw))
    except (TypeError, ValueError):
        return None


def raise_for_provider_status(response: "httpx.Response", provider: str) -> None:
    """Translate an HTTP error response into an actionable ProviderRequestError.

    The raw ``httpx.HTTPStatusError`` surfaces as an unhelpful traceback; common
    statuses (auth, billing, rate limiting) have concrete remedies worth naming.
    """
    status = getattr(response, "status_code", None)
    if status is None or status < 400:
        return
    hints = {
        401: "authentication failed — check the API key in .devcouncil/secrets.env",
        402: "payment required — the account is out of credits or has no active balance; add funds and retry",
        403: "access forbidden — the API key may lack access to the requested model",
        404: "not found — check the configured model id and provider base URL",
        429: "rate limited — too many requests; wait a moment and retry",
    }
    detail = hints.get(status, "the request was rejected")
    body = ""
    text = getattr(response, "text", None)
    if isinstance(text, str):
        body = text.strip()[:300]
    message = f"{provider} API error {status}: {detail}."
    if body:
        message = f"{message} Response: {body}"
    logger.error("Provider request failed: %s", message)
    raise ProviderRequestError(
        message,
        status_code=status,
        retry_after_seconds=_parse_retry_after(response) if status == 429 else None,
    )


def _parse_provider_json(response: "httpx.Response", provider: str) -> Dict[str, Any]:
    """Parse a provider response body, translating a non-JSON body (proxy HTML
    error page, empty body on a flaky gateway) into an actionable
    ProviderRequestError instead of a raw JSONDecodeError traceback — the CLI's
    graceful-exit paths only catch ProviderRequestError/StructuredOutputError."""
    try:
        return cast(Dict[str, Any], response.json())
    except Exception as exc:
        body = (getattr(response, "text", "") or "").strip()[:300]
        raise ProviderRequestError(
            f"{provider} returned a non-JSON response body"
            + (f": {body!r}" if body else " (empty body).")
        ) from exc


def _extract_chat_content(data: Dict[str, Any], provider: str, model: str) -> Any:
    """Extract choices[0].message.content, translating a missing-choices shape into
    an actionable ProviderRequestError. OpenRouter (and OpenAI-compatible gateways)
    can return HTTP 200 whose body is an ``error`` object instead of choices (e.g.
    upstream provider failure, moderation) — a raw KeyError here would crash the
    run with no hint of the actual provider message."""
    try:
        return data["choices"][0]["message"]["content"]
    except (KeyError, IndexError, TypeError):
        detail = ""
        if isinstance(data, dict):
            err = data.get("error")
            if isinstance(err, dict):
                detail = str(err.get("message") or err)[:300]
            elif err:
                detail = str(err)[:300]
        raise ProviderRequestError(
            f"{provider} returned no completion choices for model '{model}'"
            + (f": {detail}" if detail else " (unrecognized response shape).")
        )


class LLMResponse(BaseModel):
    content: str
    model: str
    # OpenRouter (and other providers) return richer usage payloads than plain
    # token counts: a float ``cost`` plus nested ``*_details`` dicts. Keep this
    # permissive so live responses parse; downstream only reads the int token keys.
    usage: Dict[str, Any]
    raw_response: Dict[str, Any]

    @field_validator("content", mode="before")
    @classmethod
    def _coerce_null_content(cls, value: Any) -> str:
        # Providers return ``content: null`` for reasoning-only, tool-only, or
        # filtered responses. Treat that as empty text so the router's parse /
        # healing path can retry instead of crashing on a validation error.
        return value if value is not None else ""

class Provider(ABC):
    @abstractmethod
    async def complete(
        self,
        model: str,
        messages: List[Dict[str, str]],
        temperature: float = 0.0,
        json_mode: bool = False,
        task_id: Optional[str] = None,
        run_id: Optional[str] = None,
        json_schema: Optional[Dict[str, Any]] = None,
    ) -> LLMResponse:
        """``json_schema`` (optional, only meaningful with ``json_mode=True``) is the
        JSON Schema of the expected structured output. Providers that support
        grammar-constrained decoding (Ollama's native ``format: <schema>``) use it to
        make the model *incapable* of emitting invalid JSON — the single biggest
        reliability lever for weak/local models, which otherwise waste healing
        round-trips echoing the schema or emitting prose. Providers without such
        support ignore it."""
        pass

    def _get_async_client(self, timeout: Any) -> "httpx.AsyncClient":
        """Lazily create and reuse a single ``httpx.AsyncClient`` per provider instance.

        Building an ``AsyncClient`` (connection pool + SSL context) is expensive and the
        pool is meant to be reused across calls, so we keep one per instance rather than
        constructing a fresh client on every ``complete()``. ``timeout`` is fixed per
        provider instance (cloud providers use 180s; Ollama uses its resolved
        ``self.timeout``), so binding it at construction time is equivalent to the previous
        per-call client while still allowing the pool to be reused.

        The client is bound to the event loop that created it. If the same provider
        instance is ever driven from a *different* loop (e.g. a second ``asyncio.run``),
        the old client's pool belongs to a now-closed loop and cannot be reused — we
        detect that and rebind a fresh client to the current loop instead of failing. The
        client lives for the provider's lifetime (one run / one cached router) and is
        released on GC or via ``aclose()``; provider instances are bounded, so clients do
        not accumulate."""
        import asyncio

        loop = asyncio.get_running_loop()
        client = getattr(self, "_client", None)
        if client is not None and not client.is_closed and getattr(self, "_client_loop", None) is loop:
            return cast("httpx.AsyncClient", client)
        client = httpx.AsyncClient(timeout=timeout)
        self._client: Optional[httpx.AsyncClient] = client
        self._client_loop = loop
        return client

    def cache_fingerprint(self) -> str:
        """Provider-specific options that change the model's output and therefore must
        be part of the LLM cache key. Empty for providers whose output depends only on
        ``(model, messages, temperature, json_mode)``; overridden where a runtime knob
        (e.g. Ollama's ``num_ctx`` / base URL) silently alters results for an identical
        prompt."""
        return ""

    def is_local_cost_free(self) -> bool:
        """True for on-device providers that incur no per-token cost (Ollama). Lets the
        telemetry tracker zero local usage by PROVIDER rather than by model-id matching —
        local model tags are open-ended (``qwen2.5-coder:7b``) and may collide with priced
        entries. Mirrors the provider-based zeroing in ``telemetry/cost.py``."""
        return False


def validate_model_provider(provider_name: str) -> str:
    normalized = provider_name.strip().lower()
    normalized = PROVIDER_ALIASES.get(normalized, normalized)
    if normalized in SUPPORTED_MODEL_PROVIDERS:
        return normalized
    supported = ", ".join(SUPPORTED_MODEL_PROVIDERS)
    raise ValueError(
        f"Unsupported model provider '{provider_name}'. "
        f"Supported providers: {supported}."
    )


def apply_provider_default_role_models(
    raw_config: Dict[str, Any],
    previous_provider: str,
    new_provider: str,
) -> bool:
    """Update role defaults when switching providers without overwriting custom models."""
    new = validate_model_provider(new_provider)
    models = raw_config.setdefault("models", {})
    roles = models.setdefault("roles", {})
    try:
        previous = validate_model_provider(previous_provider)
        previous_defaults = DEFAULT_ROLE_MODELS_BY_PROVIDER[previous]
    except ValueError:
        previous_defaults = {}
    new_defaults = DEFAULT_ROLE_MODELS_BY_PROVIDER[new]
    changed = False

    for role, new_model in new_defaults.items():
        role_config = roles.setdefault(role, {})
        current_model = role_config.get("model")
        if current_model is None or current_model == previous_defaults.get(role):
            if current_model != new_model:
                role_config["model"] = new_model
                changed = True

    return changed


def build_role_model_config(
    provider: str = "openrouter",
    model: str | None = None,
    role_models: Dict[str, str] | None = None,
) -> Dict[str, Dict[str, str]]:
    """Build config-ready model role mappings for a provider.

    If ``model`` is supplied, it is used for every known role. Per-role entries
    in ``role_models`` override both provider defaults and the shared model.
    """
    normalized = validate_model_provider(provider)
    roles = {
        role: {"model": selected_model}
        for role, selected_model in DEFAULT_ROLE_MODELS_BY_PROVIDER[normalized].items()
    }
    if model:
        roles = {role: {"model": model} for role in roles}
    for role, selected_model in (role_models or {}).items():
        roles[role] = {"model": selected_model}
    return roles


def openrouter_provider_payload(prefs: Any) -> Optional[Dict[str, Any]]:
    """Translate DevCouncil's ``ProviderConfig`` (or a plain mapping) into OpenRouter's
    ``provider`` routing object.

    Returns ``None`` when no prefs are supplied so the request omits the field entirely
    and OpenRouter applies its own defaults. Only the keys OpenRouter recognizes are
    forwarded (``sort``, ``allow_fallbacks``, ``require_parameters``, ``data_collection``),
    so adding unrelated fields to ``ProviderConfig`` never leaks into the API call.
    """
    if prefs is None:
        return None
    if hasattr(prefs, "model_dump"):
        data = prefs.model_dump()
    elif isinstance(prefs, dict):
        data = prefs
    else:
        return None
    allowed = ("sort", "allow_fallbacks", "require_parameters", "data_collection")
    payload = {k: data[k] for k in allowed if data.get(k) is not None}
    return payload or None


def create_provider(
    provider_name: str,
    api_key: str,
    project_root: Path = Path("."),
    provider_prefs: Any = None,
) -> Provider:
    normalized = validate_model_provider(provider_name)
    if normalized == "openrouter":
        return OpenRouterProvider(api_key, project_root=project_root, provider_prefs=provider_prefs)
    if normalized == "doubleword":
        return DoublewordProvider(api_key, project_root=project_root)
    if normalized == "ollama":
        return OllamaProvider(api_key, project_root=project_root)
    if normalized == "vertexai":
        from devcouncil.app.config import load_local_secrets
        local_secrets = load_local_secrets(project_root)
        project_id = (
            os.environ.get("VERTEXAI_PROJECT")
            or os.environ.get("GOOGLE_CLOUD_PROJECT")
            or local_secrets.get("VERTEXAI_PROJECT")
            or local_secrets.get("GOOGLE_CLOUD_PROJECT")
        )
        location = os.environ.get("VERTEXAI_LOCATION") or local_secrets.get("VERTEXAI_LOCATION", "global")
        return VertexAIProvider(api_key, project_id=project_id, location=location, project_root=project_root)
    raise AssertionError(f"Provider validation passed for unhandled provider: {normalized}")


def _log_model_call(
    payload: Dict[str, Any],
    data: Dict[str, Any],
    usage: Dict[str, int],
    project_root: Path = Path("."),
    task_id: Optional[str] = None,
    run_id: Optional[str] = None,
    provider: Optional[str] = None,
    latency_ms: Optional[int] = None,
) -> None:
    try:
        from datetime import datetime, timezone

        from devcouncil.utils.redaction import redact_dict
        # Resolve against the provider's project root, not the process cwd — otherwise
        # running `dev` from another directory logged spend to the wrong project.
        # DEVCOUNCIL_LOG_DIR (set by the test suite, optionally by CI) overrides so
        # test/mocked calls never pollute a real project's spend ledger — observed:
        # 298 of 302 entries in a real model_calls.jsonl were test-fixture pings.
        override = os.environ.get("DEVCOUNCIL_LOG_DIR")
        log_dir = Path(override) if override else project_root / ".devcouncil" / "logs"
        log_dir.mkdir(parents=True, exist_ok=True)
        log_file = log_dir / "model_calls.jsonl"

        # task_id/run_id/timestamp/provider/latency_ms are optional and backward-
        # compatible: older records simply lack them and are grouped under
        # "(unattributed)" by the cost reporter. provider lets the cost ledger
        # zero-cost local providers (ollama) regardless of the open-ended model tag
        # Ollama echoes back; latency_ms makes slow local calls diagnosable from the
        # log alone (which call dominated a multi-minute verification stage).
        log_payload = {
            "request": redact_dict(payload),
            "response": redact_dict(data),
            "usage": usage,
            "task_id": task_id,
            "run_id": run_id,
            "provider": provider,
            "latency_ms": latency_ms,
            "timestamp": datetime.now(timezone.utc).isoformat(),
        }
        with open(log_file, "a", encoding="utf-8") as f:
            f.write(json.dumps(log_payload) + "\n")
    except Exception as e:
        import logging as _log
        _log.getLogger(__name__).debug("Failed to log model call: %s", e)


class OpenRouterProvider(Provider):
    # Cap in-flight OpenRouter calls so acceptance-check fan-out stays under common
    # ~20 RPM free-tier limits instead of tripping 429 mid-run.
    DEFAULT_MAX_CONCURRENCY = 3

    def __init__(self, api_key: str, project_root: Path = Path("."), provider_prefs: Any = None):
        self.api_key = api_key
        self.base_url = "https://openrouter.ai/api/v1"
        self.project_root = project_root
        # OpenRouter routing preferences (sort/allow_fallbacks/require_parameters/
        # data_collection) sent as the request's ``provider`` field. None → omit it.
        self.provider_prefs = openrouter_provider_payload(provider_prefs)
        # Models whose serving stack rejected schema-constrained ``response_format``
        # (per MODEL, not per instance: one router instance fans across models).
        # Remembered so each such model pays the degrade retry only once.
        self._schema_format_unsupported: set = set()
        # Models whose endpoints reject ``response_format`` ENTIRELY (e.g. free-tier
        # endpoints advertising no response_format/structured_outputs support: with
        # ``require_parameters: true`` OpenRouter then 404s "no endpoints found" for
        # BOTH json_schema and json_object). For these, JSON-mode requests rely on the
        # prompt's JSON instruction + the router's extraction/healing path instead of
        # failing the whole run.
        self._response_format_unsupported: set = set()
        self.max_concurrency = self._resolve_max_concurrency()
        self._sem: "asyncio.Semaphore | None" = None
        self._sem_loop: "asyncio.AbstractEventLoop | None" = None
        # Client-side RPM pacing (OPENROUTER_RPM). Concurrency capping alone does
        # not bound the REQUEST RATE: 2-at-a-time short calls still exceed a ~20 RPM
        # endpoint cap and trip 429s that then burn the router's retry budget
        # mid-run. Pacing spaces request STARTS so the cap is never hit at all.
        self.requests_per_minute = self._resolve_rpm()
        self._pace_lock: "asyncio.Lock | None" = None
        self._pace_loop: "asyncio.AbstractEventLoop | None" = None
        self._next_request_at = 0.0

    @staticmethod
    def _resolve_max_concurrency() -> int | None:
        raw = os.environ.get("OPENROUTER_MAX_CONCURRENCY")
        if raw is None:
            return OpenRouterProvider.DEFAULT_MAX_CONCURRENCY
        raw = raw.strip().lower()
        if raw in {"0", "none", "off", ""}:
            return None
        try:
            value = int(raw)
        except ValueError:
            return OpenRouterProvider.DEFAULT_MAX_CONCURRENCY
        return value if value > 0 else None

    def _get_semaphore(self) -> "asyncio.Semaphore | None":
        import asyncio

        if not self.max_concurrency:
            return None
        loop = asyncio.get_running_loop()
        sem = getattr(self, "_sem", None)
        if sem is not None and getattr(self, "_sem_loop", None) is loop:
            return cast("asyncio.Semaphore", sem)
        sem = asyncio.Semaphore(self.max_concurrency)
        self._sem = sem
        self._sem_loop = loop
        return sem

    @staticmethod
    def _resolve_rpm() -> float | None:
        """Requests-per-minute pacing from OPENROUTER_RPM; None/off disables."""
        raw = os.environ.get("OPENROUTER_RPM")
        if raw is None:
            return None
        raw = raw.strip().lower()
        if raw in {"", "0", "none", "off"}:
            return None
        try:
            value = float(raw)
        except ValueError:
            logger.warning("Ignoring invalid OPENROUTER_RPM=%r", raw)
            return None
        return value if value > 0 else None

    async def _pace(self) -> None:
        """Space request starts to at most ``requests_per_minute`` per minute,
        and honor any active 429 cooldown (see ``_note_rate_limited``) even when
        RPM pacing itself is disabled.

        A short critical section reserves this request's start slot; the sleep
        happens OUTSIDE the lock so waiting requests queue timestamps rather
        than serializing their full durations."""
        if not self.requests_per_minute and self._next_request_at <= 0.0:
            return
        import asyncio
        import time

        loop = asyncio.get_running_loop()
        if self._pace_lock is None or self._pace_loop is not loop:
            self._pace_lock = asyncio.Lock()
            self._pace_loop = loop
        interval = (60.0 / self.requests_per_minute) if self.requests_per_minute else 0.0
        async with self._pace_lock:
            now = time.monotonic()
            wait = self._next_request_at - now
            self._next_request_at = max(now, self._next_request_at) + interval
        if wait > 0:
            await asyncio.sleep(wait)

    # Fallback cooldown after a 429 that carried no Retry-After header — matches
    # the router's first 429 backoff step so both layers agree on the pause.
    RATE_LIMIT_FALLBACK_COOLDOWN = 15.0

    def _note_rate_limited(self, retry_after: float | None) -> None:
        """Push the SHARED pacing slot past the provider-announced cooldown.

        Without this, only the request that received the 429 backs off; its
        concurrent siblings (acceptance-check fan-out) each slam into the same
        exhausted window and burn their own retry budgets. Called from the
        event-loop thread with no await between read and write, so the plain
        max() update is safe without the pace lock."""
        import time

        delay = retry_after if retry_after and retry_after > 0 else self.RATE_LIMIT_FALLBACK_COOLDOWN
        self._next_request_at = max(self._next_request_at, time.monotonic() + min(120.0, delay))

    # Statuses that mean "this endpoint/parameter combination is rejected" — the only
    # ones worth a degrade retry. 429/5xx are transient and must surface to the
    # caller's retry/backoff instead of permanently disabling structured output.
    _PARAM_REJECTED_STATUSES = frozenset({400, 404, 422})

    def cache_fingerprint(self) -> str:
        # Routing prefs change which upstream provider/model serves the request (and the
        # data-collection policy), so they can change the output for an identical prompt
        # and must invalidate the cache. Empty when unset so default runs share one key.
        if not self.provider_prefs:
            return ""
        return "openrouter:provider=" + json.dumps(self.provider_prefs, sort_keys=True)

    async def complete(
        self,
        model: str,
        messages: List[Dict[str, str]],
        temperature: float = 0.0,
        json_mode: bool = False,
        task_id: Optional[str] = None,
        run_id: Optional[str] = None,
        json_schema: Optional[Dict[str, Any]] = None,
    ) -> LLMResponse:
        # Only deep-copy when json_mode mutates the last message; otherwise the
        # caller's list is read but never modified, so we can use it directly.
        msgs = copy.deepcopy(messages) if json_mode else messages

        headers = {
            "Authorization": f"Bearer {self.api_key}",
            "Content-Type": "application/json",
            "HTTP-Referer": "https://github.com/devcouncil/devcouncil", # Optional
            "X-Title": "DevCouncil", # Optional
        }

        payload = {
            "model": model,
            "messages": msgs,
            "temperature": temperature,
        }

        if json_mode:
            # Schema-constrained structured output when the caller supplied a schema
            # and this model hasn't rejected it before. Cloud models on plain
            # ``json_object`` routinely OMIT fields that would be empty (observed:
            # gemini-2.5-flash dropping empty ``blocking_questions``/``final_tasks``
            # lists), which crashes planning schemas; ``json_schema`` makes the
            # response structurally complete. Models/routes that reject it degrade
            # to ``json_object`` (and, if even that is rejected, to NO
            # response_format at all) below and are remembered per model.
            if model in self._response_format_unsupported:
                pass  # prompt-only JSON; the router's extraction/healing handles it
            elif json_schema is not None and model not in self._schema_format_unsupported:
                payload["response_format"] = {
                    "type": "json_schema",
                    "json_schema": {
                        "name": "structured_output",
                        "strict": True,
                        "schema": json_schema,
                    },
                }
            else:
                payload["response_format"] = {"type": "json_object"}
            # Ensure the user message mentions JSON
            if msgs[-1]["role"] == "user":
                msgs[-1]["content"] += "\n\nOutput must be a valid JSON object."

        if self.provider_prefs:
            payload["provider"] = self.provider_prefs

        def _param_rejected(resp: "httpx.Response") -> bool:
            return getattr(resp, "status_code", 200) in self._PARAM_REJECTED_STATUSES

        import time as _time

        started = _time.monotonic()
        semaphore = self._get_semaphore()
        async with semaphore if semaphore is not None else contextlib.nullcontext():
            client = self._get_async_client(180.0)

            async def _post_paced() -> "httpx.Response":
                # Pace EVERY post (including degrade-chain retries): each one is
                # a real request against the endpoint's RPM budget.
                await self._pace()
                return await client.post(
                    f"{self.base_url}/chat/completions",
                    headers=headers,
                    json=payload,
                )

            response = await _post_paced()
            response_format = payload.get("response_format")
            if (
                isinstance(response_format, dict)
                and response_format.get("type") == "json_schema"
                and _param_rejected(response)
            ):
                # The model/route rejected schema-constrained output (unsupported
                # parameter, incompatible schema subset, ...). Degrade once to the
                # plain json_object switch — the prompt still carries the schema
                # instruction — and never pay this retry again for this model.
                logger.info(
                    "OpenRouter rejected json_schema response_format for model %s "
                    "(HTTP %s); retrying with json_object",
                    model, response.status_code,
                )
                self._schema_format_unsupported.add(model)
                payload["response_format"] = {"type": "json_object"}
                response = await _post_paced()
            if "response_format" in payload and _param_rejected(response):
                # Even ``json_object`` was rejected: some endpoints (commonly free
                # tiers) support no ``response_format`` variant at all, and with
                # ``provider.require_parameters: true`` OpenRouter answers 404
                # "no endpoints found" rather than routing around the parameter —
                # which previously killed planning outright (observed: every arm-B
                # benchmark task erroring in ~8s on such a model). Drop the field,
                # remember per model, and rely on the prompt's JSON instruction +
                # the router's extraction/healing path.
                logger.warning(
                    "OpenRouter rejected response_format entirely for model %s "
                    "(HTTP %s); retrying without structured output. JSON will be "
                    "prompt-enforced only — prefer a model with response_format "
                    "support for planning roles if this recurs.",
                    model, response.status_code,
                )
                self._response_format_unsupported.add(model)
                payload.pop("response_format", None)
                response = await _post_paced()
            if getattr(response, "status_code", None) == 429:
                # Cooldown is shared across ALL in-flight callers so the whole
                # process backs off together, not one request at a time.
                self._note_rate_limited(_parse_retry_after(response))
            raise_for_provider_status(response, "OpenRouter")
            data = _parse_provider_json(response, "OpenRouter")

            resp = LLMResponse(
                content=_extract_chat_content(data, "OpenRouter", model),
                model=data["model"],
                usage=data.get("usage", {}),
                raw_response=data
            )

            # Includes queue/pacing wait — the latency the caller experienced.
            latency_ms = int((_time.monotonic() - started) * 1000)
            _log_model_call(
                payload, data, resp.usage, self.project_root,
                task_id=task_id, run_id=run_id, provider="openrouter", latency_ms=latency_ms,
            )

            return resp


class DoublewordProvider(Provider):
    def __init__(self, api_key: str, project_root: Path = Path(".")):
        self.api_key = api_key
        self.base_url = "https://api.doubleword.ai/v1"
        self.project_root = project_root

    async def complete(
        self,
        model: str,
        messages: List[Dict[str, str]],
        temperature: float = 0.0,
        json_mode: bool = False,
        task_id: Optional[str] = None,
        run_id: Optional[str] = None,
        json_schema: Optional[Dict[str, Any]] = None,  # accepted for interface parity; not used
    ) -> LLMResponse:
        # Only deep-copy when json_mode mutates the last message; otherwise the
        # caller's list is read but never modified, so we can use it directly.
        msgs = copy.deepcopy(messages) if json_mode else messages
        headers = {
            "Authorization": f"Bearer {self.api_key}",
            "Content-Type": "application/json",
        }
        payload = {
            "model": model,
            "messages": msgs,
            "temperature": temperature,
        }

        if json_mode:
            payload["response_format"] = {"type": "json_object"}
            if msgs[-1]["role"] == "user":
                msgs[-1]["content"] += "\n\nOutput must be a valid JSON object."

        import time as _time

        started = _time.monotonic()
        client = self._get_async_client(180.0)
        response = await client.post(
            f"{self.base_url}/chat/completions",
            headers=headers,
            json=payload,
        )
        raise_for_provider_status(response, "Doubleword")
        latency_ms = int((_time.monotonic() - started) * 1000)
        data = _parse_provider_json(response, "Doubleword")

        resp = LLMResponse(
            content=_extract_chat_content(data, "Doubleword", model),
            model=data["model"],
            usage=data.get("usage", {}),
            raw_response=data
        )

        _log_model_call(
            payload, data, resp.usage, self.project_root,
            task_id=task_id, run_id=run_id, provider="doubleword", latency_ms=latency_ms,
        )
        return resp


class OllamaProvider(Provider):
    """Local Ollama provider via its NATIVE ``/api/chat`` endpoint.

    Ollama needs no API key. The base URL is overridable via ``OLLAMA_BASE_URL``
    (taken verbatim) or Ollama's native ``OLLAMA_HOST`` (normalized: a missing scheme
    is prefixed with ``http://`` and a missing ``/v1`` suffix is appended). The actual
    request goes to the native ``/api/chat`` endpoint (derived by stripping a trailing
    ``/v1``) rather than the OpenAI-compatible ``/v1/chat/completions`` — because the
    native endpoint is the only one that honors ``options.num_ctx`` (set via
    ``OLLAMA_NUM_CTX``) and ``format: json``. DevCouncil's planning prompts are large
    (up to ~15k tokens), so without a raised ``num_ctx`` Ollama's small default context
    would silently truncate them.
    """

    def __init__(
        self,
        api_key: str = "",
        project_root: Path = Path("."),
        base_url: str | None = None,
        num_ctx: int | None = None,
    ):
        self.api_key = api_key
        self.base_url = base_url or self._resolve_base_url()
        self.project_root = project_root
        self.num_ctx = num_ctx if num_ctx is not None else self._resolve_num_ctx()
        self.max_num_ctx = self._resolve_max_num_ctx()
        self.keep_alive = self._resolve_keep_alive()
        self.timeout = self._resolve_timeout()
        self.think = self._resolve_think()
        self.num_predict = self._resolve_num_predict()
        self.max_concurrency = self._resolve_max_concurrency()
        # Set when a server rejects the ``think`` field (older Ollama / non-thinking
        # model) so subsequent calls skip it instead of paying a retry every time.
        self._think_unsupported = False
        self._sem: "asyncio.Semaphore | None" = None
        self._sem_loop: "asyncio.AbstractEventLoop | None" = None

    # Local generation latency is unbounded (cold loads, CPU-only hosts, large
    # ``num_ctx``) and is not a network failure, so Ollama gets a generous default
    # and an explicit override rather than the cloud providers' fixed 180s.
    DEFAULT_TIMEOUT = 600.0

    # Default context window when OLLAMA_NUM_CTX is unset. Ollama's own server
    # default (2k–4k depending on version) silently TRUNCATES DevCouncil's planning
    # and verification prompts (up to ~15k tokens) — the model then plans/reviews
    # against half a prompt, which surfaces as garbage plans and miscalibrated
    # verdicts on local models. 16k covers the largest prompt DevCouncil builds.
    # Override (up or down, e.g. for a VRAM-limited host) with OLLAMA_NUM_CTX.
    DEFAULT_NUM_CTX = 16384

    # Default model keep-alive when OLLAMA_KEEP_ALIVE is unset. Ollama unloads a
    # model after 5 minutes idle; a gated run interleaves LLM calls with long
    # non-LLM phases (executor runs can exceed 5m), so each council/verify stage
    # would otherwise pay a multi-minute cold reload of a 30B+ model. "30m" keeps
    # the model resident across a typical task cycle at zero cost when idle-free.
    DEFAULT_KEEP_ALIVE = "30m"

    # Ceiling for the ADAPTIVE context window (see complete()): when a prompt would
    # not fit the configured num_ctx, the request's num_ctx is raised to fit — up to
    # this cap — instead of letting Ollama silently truncate the prompt (which makes
    # the model review/plan against half a prompt and surfaces as garbage output and
    # miscalibrated verdicts). Capped because num_ctx directly scales KV-cache VRAM.
    # Override with OLLAMA_MAX_NUM_CTX; an explicit OLLAMA_NUM_CTX also raises it
    # (the cap is never below the configured window).
    DEFAULT_MAX_NUM_CTX = 65536

    # Crude chars-per-token estimate for sizing the adaptive window. 3 chars/token
    # deliberately over-estimates tokens (most code/text averages ~3.5-4), so the
    # adaptive window errs toward "too big" rather than silent truncation.
    _CHARS_PER_TOKEN = 3.0
    # Headroom reserved for generation + chat template overhead when fitting a
    # prompt into the adaptive window.
    _RESPONSE_HEADROOM_TOKENS = 2048

    @staticmethod
    def _resolve_keep_alive() -> str | None:
        """Keep-alive from ``OLLAMA_KEEP_ALIVE`` (Ollama duration string like ``10m``,
        ``0`` to unload immediately, ``-1`` to pin forever). Unset falls back to
        :data:`DEFAULT_KEEP_ALIVE`; the literal ``default`` defers to the server."""
        raw = os.environ.get("OLLAMA_KEEP_ALIVE")
        if raw is None:
            return OllamaProvider.DEFAULT_KEEP_ALIVE
        raw = raw.strip()
        if not raw or raw.lower() == "default":
            return None  # omit from payload; let the server decide
        return raw

    @staticmethod
    def _resolve_think() -> bool | str | None:
        """Thinking-mode / thinking-BUDGET override from ``OLLAMA_THINK``.

        Unset -> ``None`` (omit the field; the server/model default applies).
        Truthy (``1``/``true``/``on``/``yes``) -> request thinking explicitly.
        Falsy (``0``/``false``/``off``/``no``) -> disable thinking.
        ``low``/``medium``/``high`` -> a thinking-budget LEVEL, passed through
        verbatim (Ollama >= 0.12 supports string levels on budget-capable models,
        e.g. gpt-oss; the existing 400-degrade path covers servers/models that
        reject it).

        Why this exists: on thinking-capable local models (qwen3 family, deepseek-r1,
        Ornith) the reasoning channel dominates latency for DevCouncil's structured
        review/verification calls — measured locally at ~65x (156s with thinking vs
        2.4s without for one acceptance-check compile). Thinking often *helps* answer
        quality, so DevCouncil does not flip the default; this knob lets a user trade
        latency for quality per host. Servers/models that reject the field degrade
        gracefully (one retry without it, then it is skipped for the provider's life).
        """
        raw = os.environ.get("OLLAMA_THINK")
        if raw is None:
            return None
        raw = raw.strip().lower()
        if raw in {"1", "true", "on", "yes"}:
            return True
        if raw in {"0", "false", "off", "no"}:
            return False
        if raw in {"low", "medium", "high"}:
            return raw
        return None

    # Default client-side cap on in-flight requests to one Ollama server. Callers
    # legitimately fan out (per-criterion acceptance compiles x samples can launch
    # 20+ concurrent calls), but a local server generates (near-)serially — so the
    # HTTP read timeout of a QUEUED request starts ticking long before the server
    # even sees it, and late requests time out at any timeout setting (the observed
    # benchmark failure). Capping in-flight requests makes queue wait happen
    # client-side, where it does not count against the per-request timeout.
    DEFAULT_MAX_CONCURRENCY = 2

    @staticmethod
    def _resolve_max_concurrency() -> int | None:
        """In-flight request cap from ``OLLAMA_MAX_CONCURRENCY`` (positive int).
        ``0``/``none``/``off`` disables the cap (e.g. for a serving stack that
        genuinely parallelizes); unset/invalid falls back to
        :data:`DEFAULT_MAX_CONCURRENCY`."""
        raw = os.environ.get("OLLAMA_MAX_CONCURRENCY")
        if raw is None:
            return OllamaProvider.DEFAULT_MAX_CONCURRENCY
        raw = raw.strip().lower()
        if raw in {"0", "none", "off", ""}:
            return None
        try:
            value = int(raw)
        except ValueError:
            return OllamaProvider.DEFAULT_MAX_CONCURRENCY
        return value if value > 0 else None

    def _get_semaphore(self) -> "asyncio.Semaphore | None":
        """Lazily create the concurrency semaphore, rebound per event loop.

        Like ``_get_async_client``: an ``asyncio.Semaphore`` belongs to the loop
        that created it, and one provider instance may be driven from successive
        ``asyncio.run`` loops. ``None`` when the cap is disabled."""
        import asyncio

        if not self.max_concurrency:
            return None
        loop = asyncio.get_running_loop()
        sem = getattr(self, "_sem", None)
        if sem is not None and getattr(self, "_sem_loop", None) is loop:
            return cast("asyncio.Semaphore", sem)
        sem = asyncio.Semaphore(self.max_concurrency)
        self._sem = sem
        self._sem_loop = loop
        return sem

    @staticmethod
    def _resolve_num_predict() -> int | None:
        """Generation-token cap from ``OLLAMA_NUM_PREDICT`` (positive int; unset or
        invalid -> no cap). Bounds the WORST CASE of an unbounded thinking spiral: a
        reasoning model that never stops thinking otherwise generates until the HTTP
        timeout (600s default), and the router's structured-output layers can stack
        those stalls past an outer scheduler/benchmark kill. With a cap, a runaway
        call instead returns quickly with ``done_reason=length`` and flows into the
        existing truncation warning + healing path."""
        raw = os.environ.get("OLLAMA_NUM_PREDICT")
        if not raw:
            return None
        try:
            value = int(raw)
        except (TypeError, ValueError):
            return None
        return value if value > 0 else None

    @staticmethod
    def _resolve_max_num_ctx() -> int:
        """Adaptive-context ceiling from ``OLLAMA_MAX_NUM_CTX`` (positive int).
        Unset/invalid falls back to :data:`DEFAULT_MAX_NUM_CTX`."""
        raw = os.environ.get("OLLAMA_MAX_NUM_CTX")
        if not raw:
            return OllamaProvider.DEFAULT_MAX_NUM_CTX
        try:
            value = int(raw)
        except (TypeError, ValueError):
            return OllamaProvider.DEFAULT_MAX_NUM_CTX
        return value if value > 0 else OllamaProvider.DEFAULT_MAX_NUM_CTX

    def _effective_num_ctx(self, messages: List[Dict[str, str]]) -> int | None:
        """The context window to request for THIS call.

        Starts from the configured ``num_ctx`` and, when the prompt's estimated token
        count (plus response headroom) would overflow it, grows the window to fit — up
        to ``max_num_ctx`` (never below an explicitly configured window). Ollama
        silently TRUNCATES a prompt that exceeds num_ctx, so without this a large
        verification diff or planning prompt gets reviewed half-read; a too-large
        request merely costs KV-cache memory. Returns None when num_ctx is disabled
        (explicit server-default opt-out)."""
        if not self.num_ctx:
            return None
        prompt_chars = sum(len(m.get("content") or "") for m in messages)
        needed = int(prompt_chars / self._CHARS_PER_TOKEN) + self._RESPONSE_HEADROOM_TOKENS
        if needed <= self.num_ctx:
            return self.num_ctx
        ceiling = max(self.max_num_ctx, self.num_ctx)
        effective = min(needed, ceiling)
        if effective > self.num_ctx:
            logger.info(
                "Ollama: raising num_ctx %d -> %d for a ~%d-token prompt "
                "(prevents silent server-side truncation; cap OLLAMA_MAX_NUM_CTX=%d)",
                self.num_ctx, effective, needed - self._RESPONSE_HEADROOM_TOKENS, ceiling,
            )
        return effective

    @staticmethod
    def _resolve_timeout() -> float | None:
        """Read timeout from ``OLLAMA_TIMEOUT`` seconds (positive float). ``0``/``none``/
        ``off`` disables it entirely for very slow local models; unset/invalid falls back
        to :data:`DEFAULT_TIMEOUT`."""
        raw = os.environ.get("OLLAMA_TIMEOUT")
        if raw is None:
            return OllamaProvider.DEFAULT_TIMEOUT
        raw = raw.strip().lower()
        if raw in {"0", "none", "off", ""}:
            return None
        try:
            value = float(raw)
        except ValueError:
            return OllamaProvider.DEFAULT_TIMEOUT
        return value if value > 0 else None

    def cache_fingerprint(self) -> str:
        # num_ctx (and the adaptive ceiling), think, and the target server all change the
        # response for an identical prompt (a larger window avoids the truncation a smaller
        # one silently applies; thinking alters generation; a different endpoint is a
        # different model server), so each must invalidate the cache. Key on the
        # *normalized* /api/chat endpoint, not the raw base_url, so equivalent configs
        # (OLLAMA_HOST vs OLLAMA_BASE_URL, with/without a trailing /v1) collapse to one key.
        return (
            f"ollama:num_ctx={self.num_ctx};max_num_ctx={self.max_num_ctx};"
            f"think={self.think};num_predict={self.num_predict};"
            f"endpoint={self._chat_endpoint()}"
        )

    def is_local_cost_free(self) -> bool:
        return True

    @staticmethod
    def _resolve_base_url() -> str:
        explicit = os.environ.get("OLLAMA_BASE_URL")
        if explicit:
            return explicit.rstrip("/")
        host = os.environ.get("OLLAMA_HOST")
        if host:
            host = host.strip()
            if "://" not in host:
                host = f"http://{host}"
            host = host.rstrip("/")
            if not host.endswith("/v1"):
                host = f"{host}/v1"
            return host
        return "http://localhost:11434/v1"

    @staticmethod
    def _resolve_num_ctx() -> int | None:
        """Context window from ``OLLAMA_NUM_CTX`` (positive int). Unset/invalid falls
        back to :data:`DEFAULT_NUM_CTX` — never the server default, which is small
        enough to silently truncate DevCouncil's planning prompts. ``0``/negative
        explicitly requests the server default (opt-out)."""
        raw = os.environ.get("OLLAMA_NUM_CTX")
        if not raw:
            return OllamaProvider.DEFAULT_NUM_CTX
        try:
            value = int(raw)
        except (TypeError, ValueError):
            return OllamaProvider.DEFAULT_NUM_CTX
        return value if value > 0 else None

    def _chat_endpoint(self) -> str:
        """Native chat endpoint derived from base_url (strip a trailing ``/v1``)."""
        root = self.base_url.rstrip("/")
        if root.endswith("/v1"):
            root = root[: -len("/v1")].rstrip("/")
        return f"{root}/api/chat"

    async def complete(
        self,
        model: str,
        messages: List[Dict[str, str]],
        temperature: float = 0.0,
        json_mode: bool = False,
        task_id: Optional[str] = None,
        run_id: Optional[str] = None,
        json_schema: Optional[Dict[str, Any]] = None,
    ) -> LLMResponse:
        # Only deep-copy when json_mode mutates the last message; otherwise the
        # caller's list is read but never modified, so we can use it directly.
        msgs = copy.deepcopy(messages) if json_mode else messages
        headers = {
            "Content-Type": "application/json",
        }
        # Ollama ignores auth, but a configured key (e.g. for a reverse proxy)
        # passes through harmlessly.
        if self.api_key:
            headers["Authorization"] = f"Bearer {self.api_key}"

        # Native /api/chat options. temperature and num_ctx live under "options"; the
        # window is sized per call (see _effective_num_ctx) so a large verification /
        # planning prompt is never silently truncated server-side.
        options: Dict[str, Any] = {"temperature": temperature}
        effective_ctx = self._effective_num_ctx(msgs)
        if effective_ctx:
            options["num_ctx"] = effective_ctx
        # Cap generation so a thinking model that never stops reasoning returns a
        # truncated (healable) response instead of running into the HTTP timeout.
        if self.num_predict:
            options["num_predict"] = self.num_predict

        payload: Dict[str, Any] = {
            "model": model,
            "messages": msgs,
            "stream": False,
            "options": options,
        }
        # Keep the model resident between DevCouncil's interleaved LLM / non-LLM
        # phases so a 30B+ local model isn't cold-reloaded mid-run (minutes each time).
        if self.keep_alive is not None:
            payload["keep_alive"] = self.keep_alive
        # Explicit thinking-mode request (OLLAMA_THINK). Omitted when unset or when a
        # previous call learned this server/model rejects the field.
        if self.think is not None and not self._think_unsupported:
            payload["think"] = self.think

        if json_mode:
            # Native structured output. Passing the actual JSON SCHEMA (supported by
            # Ollama >= 0.5) constrains DECODING to the schema's grammar — a local
            # model literally cannot emit prose, fences, or a schema echo, which
            # eliminates most healing retries. Plain "json" is the fallback for
            # callers without a schema (and for older servers, handled below).
            payload["format"] = json_schema if json_schema else "json"
            if msgs[-1]["role"] == "user":
                msgs[-1]["content"] += "\n\nOutput must be a valid JSON object."

        import contextlib
        import time as _time

        client = self._get_async_client(self.timeout)
        started = _time.monotonic()
        # Serialize in-flight requests up to max_concurrency: a local server
        # generates (near-)serially, so without this a caller fan-out (e.g.
        # per-criterion acceptance compiles x samples) queues requests SERVER-side
        # where their read timeouts tick while waiting — late requests then time
        # out at any timeout setting. The degrade retries below stay inside the
        # slot so one logical call holds one slot start-to-finish.
        semaphore = self._get_semaphore()
        async with (semaphore if semaphore is not None else contextlib.nullcontext()):
            response = await client.post(
                self._chat_endpoint(),
                headers=headers,
                json=payload,
            )
            if "think" in payload and getattr(response, "status_code", 200) >= 400:
                # Older Ollama servers (< 0.9) and non-thinking models reject the ``think``
                # field. Drop it once, remember, and never fail the run over a latency knob.
                logger.info(
                    "Ollama rejected the think field (HTTP %s); retrying without it",
                    response.status_code,
                )
                self._think_unsupported = True
                payload.pop("think", None)
                response = await client.post(
                    self._chat_endpoint(),
                    headers=headers,
                    json=payload,
                )
            if json_schema is not None and getattr(response, "status_code", 200) >= 400:
                # An Ollama server predating schema-constrained ``format`` rejects the
                # request (400). Degrade once to the plain "json" switch rather than
                # failing the run over an optional optimization.
                logger.info(
                    "Ollama rejected schema-constrained format (HTTP %s); retrying with format=json",
                    response.status_code,
                )
                payload["format"] = "json"
                response = await client.post(
                    self._chat_endpoint(),
                    headers=headers,
                    json=payload,
                )
        raise_for_provider_status(response, "Ollama")
        # Includes any client-side queue wait — that is the latency the caller
        # actually experienced, which is what makes a slow stage diagnosable.
        latency_ms = int((_time.monotonic() - started) * 1000)
        data = _parse_provider_json(response, "Ollama")

        # Native response shape: {"message": {"content": ..., "thinking": ...},
        # "model": ..., "prompt_eval_count": N, "eval_count": M}. Map token counts
        # to the OpenAI-style keys the cost ledger/tracker expect.
        prompt_tokens = int(data.get("prompt_eval_count", 0) or 0)
        completion_tokens = int(data.get("eval_count", 0) or 0)
        usage = {
            "prompt_tokens": prompt_tokens,
            "completion_tokens": completion_tokens,
            "total_tokens": prompt_tokens + completion_tokens,
        }
        message = data.get("message") or {}
        content = message.get("content", "") or ""
        if not content.strip() and (message.get("thinking") or "").strip():
            # A thinking model that spent its whole budget reasoning (or answered
            # inside the reasoning channel) returns an empty content. Surface the
            # thinking text so the router's extraction/healing path has SOMETHING
            # to parse instead of failing on an empty string.
            content = message["thinking"]
            logger.warning(
                "Ollama returned empty content with a non-empty thinking channel "
                "(model=%s); using the thinking text for parsing. If this recurs, "
                "set OLLAMA_THINK=false for this host.",
                data.get("model", model),
            )
        if data.get("done_reason") == "length":
            # Generation hit the token limit mid-answer — structured output is very
            # likely cut off. Loud, actionable log rather than a silent parse failure.
            logger.warning(
                "Ollama generation truncated by length (model=%s, eval_count=%s). "
                "Structured output may be incomplete; on a thinking model consider "
                "OLLAMA_THINK=false or a larger context (OLLAMA_NUM_CTX/OLLAMA_MAX_NUM_CTX).",
                data.get("model", model), completion_tokens,
            )
        resp = LLMResponse(
            content=content,
            # Ollama may omit ``model`` or return a local tag — fall back to
            # the requested id rather than KeyError-ing.
            model=data.get("model", model),
            usage=usage,
            raw_response=data,
        )

        _log_model_call(
            payload, data, resp.usage, self.project_root,
            task_id=task_id, run_id=run_id, provider="ollama", latency_ms=latency_ms,
        )
        return resp


class VertexAIProvider(Provider):
    """Vertex AI provider using Google's OpenAI-compatible Chat Completions API."""

    def __init__(self, access_token: str, project_id: str | None = None, location: str | None = None, project_root: Path = Path(".")):
        self.access_token = access_token
        self.project_id = project_id or os.environ.get("VERTEXAI_PROJECT") or os.environ.get("GOOGLE_CLOUD_PROJECT")
        self.location = location or os.environ.get("VERTEXAI_LOCATION", "global")
        self.project_root = project_root

    @property
    def base_url(self) -> str:
        if not self.project_id:
            raise ValueError(
                "Vertex AI project is not configured. Set VERTEXAI_PROJECT or GOOGLE_CLOUD_PROJECT."
            )
        return (
            f"https://aiplatform.googleapis.com/v1/projects/{self.project_id}"
            f"/locations/{self.location}/endpoints/openapi"
        )

    def _headers(self) -> Dict[str, str]:
        return {
            "Authorization": f"Bearer {self.access_token}",
            "Content-Type": "application/json",
        }

    def _refresh_access_token_from_gcloud(self) -> bool:
        from devcouncil.app.config import get_gcloud_access_token

        refreshed = get_gcloud_access_token()
        if not refreshed:
            return False
        self.access_token = refreshed
        return True

    async def complete(
        self,
        model: str,
        messages: List[Dict[str, str]],
        temperature: float = 0.0,
        json_mode: bool = False,
        task_id: Optional[str] = None,
        run_id: Optional[str] = None,
        json_schema: Optional[Dict[str, Any]] = None,  # accepted for interface parity; not used
    ) -> LLMResponse:
        # Only deep-copy when json_mode mutates the last message; otherwise the
        # caller's list is read but never modified, so we can use it directly.
        msgs = copy.deepcopy(messages) if json_mode else messages

        payload = {
            "model": model,
            "messages": msgs,
            "temperature": temperature,
        }

        if json_mode:
            payload["response_format"] = {"type": "json_object"}
            if msgs[-1]["role"] == "user":
                msgs[-1]["content"] += "\n\nOutput must be a valid JSON object."

        import time as _time

        started = _time.monotonic()
        client = self._get_async_client(180.0)
        response = await client.post(
            f"{self.base_url}/chat/completions",
            headers=self._headers(),
            json=payload,
        )
        if response.status_code in {401, 403} and self._refresh_access_token_from_gcloud():
            response = await client.post(
                f"{self.base_url}/chat/completions",
                headers=self._headers(),
                json=payload,
            )
        raise_for_provider_status(response, "Vertex AI")
        latency_ms = int((_time.monotonic() - started) * 1000)
        data = _parse_provider_json(response, "Vertex AI")

        resp = LLMResponse(
            content=_extract_chat_content(data, "Vertex AI", model),
            model=data["model"],
            usage=data.get("usage", {}),
            raw_response=data
        )

        _log_model_call(
            payload, data, resp.usage, self.project_root,
            task_id=task_id, run_id=run_id, provider="vertexai", latency_ms=latency_ms,
        )
        return resp

class MockProvider(Provider):
    """Mock provider for dry runs and testing."""
    def __init__(self, responses: Optional[Dict[str, Any]] = None):
        # responses can be a dict of model -> str OR model -> list of str
        self.responses = responses or {}
        self._counts: Dict[str, int] = {}

    async def complete(
        self,
        model: str,
        messages: List[Dict[str, str]],
        temperature: float = 0.0,
        json_mode: bool = False,
        task_id: Optional[str] = None,
        run_id: Optional[str] = None,
        json_schema: Optional[Dict[str, Any]] = None,  # accepted for interface parity; not used
    ) -> LLMResponse:
        res = self.responses.get(model, '{"mock": "response"}')
        
        if isinstance(res, list):
            count = self._counts.get(model, 0)
            content = res[min(count, len(res)-1)]
            self._counts[model] = count + 1
        else:
            content = res
            
        return LLMResponse(
            content=content,
            model=f"mock/{model}",
            usage={"prompt_tokens": 10, "completion_tokens": 10, "total_tokens": 20},
            raw_response={"choices": [{"message": {"content": content}}]}
        )
