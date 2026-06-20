from abc import ABC, abstractmethod
import copy
from functools import lru_cache
from importlib import resources
import os
from typing import List, Dict, Any, Optional
from pydantic import BaseModel, field_validator
import httpx
import json
from pathlib import Path
import yaml

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

    def __init__(self, message: str, status_code: int | None = None):
        super().__init__(message)
        self.status_code = status_code


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
    raise ProviderRequestError(message, status_code=status)


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
    ) -> LLMResponse:
        pass


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


def create_provider(provider_name: str, api_key: str, project_root: Path = Path(".")) -> Provider:
    normalized = validate_model_provider(provider_name)
    if normalized == "openrouter":
        return OpenRouterProvider(api_key, project_root=project_root)
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
) -> None:
    try:
        from datetime import datetime, timezone

        from devcouncil.utils.redaction import redact_dict
        # Resolve against the provider's project root, not the process cwd — otherwise
        # running `dev` from another directory logged spend to the wrong project.
        log_dir = project_root / ".devcouncil" / "logs"
        log_dir.mkdir(parents=True, exist_ok=True)
        log_file = log_dir / "model_calls.jsonl"

        # task_id/run_id/timestamp/provider are optional and backward-compatible: older
        # records simply lack them and are grouped under "(unattributed)" by the cost
        # reporter. provider lets the cost ledger zero-cost local providers (ollama)
        # regardless of the open-ended model tag Ollama echoes back.
        log_payload = {
            "request": redact_dict(payload),
            "response": redact_dict(data),
            "usage": usage,
            "task_id": task_id,
            "run_id": run_id,
            "provider": provider,
            "timestamp": datetime.now(timezone.utc).isoformat(),
        }
        with open(log_file, "a", encoding="utf-8") as f:
            f.write(json.dumps(log_payload) + "\n")
    except Exception as e:
        import logging as _log
        _log.getLogger(__name__).debug("Failed to log model call: %s", e)


class OpenRouterProvider(Provider):
    def __init__(self, api_key: str, project_root: Path = Path(".")):
        self.api_key = api_key
        self.base_url = "https://openrouter.ai/api/v1"
        self.project_root = project_root

    async def complete(
        self, 
        model: str, 
        messages: List[Dict[str, str]], 
        temperature: float = 0.0,
        json_mode: bool = False,
        task_id: Optional[str] = None,
        run_id: Optional[str] = None,
    ) -> LLMResponse:
        # Deep-copy to avoid mutating the caller's messages list
        msgs = copy.deepcopy(messages)

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
            payload["response_format"] = {"type": "json_object"}
            # Ensure the user message mentions JSON
            if msgs[-1]["role"] == "user":
                msgs[-1]["content"] += "\n\nOutput must be a valid JSON object."

        async with httpx.AsyncClient(timeout=180.0) as client:
            response = await client.post(
                f"{self.base_url}/chat/completions",
                headers=headers,
                json=payload
            )
            raise_for_provider_status(response, "OpenRouter")
            data = response.json()

            resp = LLMResponse(
                content=data["choices"][0]["message"]["content"],
                model=data["model"],
                usage=data.get("usage", {}),
                raw_response=data
            )

            _log_model_call(payload, data, resp.usage, self.project_root, task_id=task_id, run_id=run_id)

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
    ) -> LLMResponse:
        msgs = copy.deepcopy(messages)
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

        async with httpx.AsyncClient(timeout=180.0) as client:
            response = await client.post(
                f"{self.base_url}/chat/completions",
                headers=headers,
                json=payload
            )
            raise_for_provider_status(response, "Doubleword")
            data = response.json()

            resp = LLMResponse(
                content=data["choices"][0]["message"]["content"],
                model=data["model"],
                usage=data.get("usage", {}),
                raw_response=data
            )

            _log_model_call(payload, data, resp.usage, self.project_root, task_id=task_id, run_id=run_id)
            return resp


class OllamaProvider(Provider):
    """Local Ollama provider via its OpenAI-compatible Chat Completions endpoint.

    Ollama serves an OpenAI-compatible API at ``http://localhost:11434/v1`` and
    needs no API key. The base URL is overridable via ``OLLAMA_BASE_URL`` (taken
    verbatim) or Ollama's native ``OLLAMA_HOST`` (normalized: a missing scheme is
    prefixed with ``http://`` and a missing ``/v1`` suffix is appended).
    """

    def __init__(self, api_key: str = "", project_root: Path = Path("."), base_url: str | None = None):
        self.api_key = api_key
        self.base_url = base_url or self._resolve_base_url()
        self.project_root = project_root

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

    async def complete(
        self,
        model: str,
        messages: List[Dict[str, str]],
        temperature: float = 0.0,
        json_mode: bool = False,
        task_id: Optional[str] = None,
        run_id: Optional[str] = None,
    ) -> LLMResponse:
        msgs = copy.deepcopy(messages)
        headers = {
            "Content-Type": "application/json",
        }
        # Ollama ignores auth, but a configured key (e.g. for a reverse proxy)
        # passes through harmlessly.
        if self.api_key:
            headers["Authorization"] = f"Bearer {self.api_key}"

        payload = {
            "model": model,
            "messages": msgs,
            "temperature": temperature,
        }

        if json_mode:
            payload["response_format"] = {"type": "json_object"}
            if msgs[-1]["role"] == "user":
                msgs[-1]["content"] += "\n\nOutput must be a valid JSON object."

        async with httpx.AsyncClient(timeout=180.0) as client:
            response = await client.post(
                f"{self.base_url}/chat/completions",
                headers=headers,
                json=payload
            )
            raise_for_provider_status(response, "Ollama")
            data = response.json()

            resp = LLMResponse(
                content=data["choices"][0]["message"]["content"],
                # Ollama may omit ``model`` or return a local tag — fall back to
                # the requested id rather than KeyError-ing on data["model"].
                model=data.get("model", model),
                usage=data.get("usage", {}),
                raw_response=data
            )

            _log_model_call(payload, data, resp.usage, self.project_root, task_id=task_id, run_id=run_id, provider="ollama")
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
    ) -> LLMResponse:
        msgs = copy.deepcopy(messages)

        payload = {
            "model": model,
            "messages": msgs,
            "temperature": temperature,
        }

        if json_mode:
            payload["response_format"] = {"type": "json_object"}
            if msgs[-1]["role"] == "user":
                msgs[-1]["content"] += "\n\nOutput must be a valid JSON object."

        async with httpx.AsyncClient(timeout=180.0) as client:
            response = await client.post(
                f"{self.base_url}/chat/completions",
                headers=self._headers(),
                json=payload
            )
            if response.status_code in {401, 403} and self._refresh_access_token_from_gcloud():
                response = await client.post(
                    f"{self.base_url}/chat/completions",
                    headers=self._headers(),
                    json=payload
                )
            raise_for_provider_status(response, "Vertex AI")
            data = response.json()

            resp = LLMResponse(
                content=data["choices"][0]["message"]["content"],
                model=data["model"],
                usage=data.get("usage", {}),
                raw_response=data
            )

            _log_model_call(payload, data, resp.usage, self.project_root, task_id=task_id, run_id=run_id)
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
