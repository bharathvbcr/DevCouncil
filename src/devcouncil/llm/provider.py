from abc import ABC, abstractmethod
import copy
from functools import lru_cache
from importlib import resources
import os
from typing import List, Dict, Any, Optional
from pydantic import BaseModel
import httpx
import json
from pathlib import Path
import yaml

SUPPORTED_MODEL_PROVIDERS = ("openrouter", "vertexai", "doubleword")
PROVIDER_ALIASES = {
    "vertex-ai": "vertexai",
    "vertex_ai": "vertexai",
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


class LLMResponse(BaseModel):
    content: str
    model: str
    usage: Dict[str, int]
    raw_response: Dict[str, Any]

class Provider(ABC):
    @abstractmethod
    async def complete(
        self, 
        model: str, 
        messages: List[Dict[str, str]], 
        temperature: float = 0.0,
        json_mode: bool = False
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
        return OpenRouterProvider(api_key)
    if normalized == "doubleword":
        return DoublewordProvider(api_key)
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
        return VertexAIProvider(api_key, project_id=project_id, location=location)
    raise AssertionError(f"Provider validation passed for unhandled provider: {normalized}")


def _log_model_call(payload: Dict[str, Any], data: Dict[str, Any], usage: Dict[str, int]) -> None:
    try:
        from devcouncil.utils.redaction import redact_dict
        log_dir = Path(".devcouncil/logs")
        log_dir.mkdir(parents=True, exist_ok=True)
        log_file = log_dir / "model_calls.jsonl"

        log_payload = {
            "request": redact_dict(payload),
            "response": redact_dict(data),
            "usage": usage,
        }
        with open(log_file, "a", encoding="utf-8") as f:
            f.write(json.dumps(log_payload) + "\n")
    except Exception as e:
        import logging as _log
        _log.getLogger(__name__).debug("Failed to log model call: %s", e)


class OpenRouterProvider(Provider):
    def __init__(self, api_key: str):
        self.api_key = api_key
        self.base_url = "https://openrouter.ai/api/v1"

    async def complete(
        self, 
        model: str, 
        messages: List[Dict[str, str]], 
        temperature: float = 0.0,
        json_mode: bool = False
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
            response.raise_for_status()
            data = response.json()
            
            resp = LLMResponse(
                content=data["choices"][0]["message"]["content"],
                model=data["model"],
                usage=data.get("usage", {}),
                raw_response=data
            )
            
            _log_model_call(payload, data, resp.usage)
                
            return resp


class DoublewordProvider(Provider):
    def __init__(self, api_key: str):
        self.api_key = api_key
        self.base_url = "https://api.doubleword.ai/v1"

    async def complete(
        self,
        model: str,
        messages: List[Dict[str, str]],
        temperature: float = 0.0,
        json_mode: bool = False
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
            response.raise_for_status()
            data = response.json()

            resp = LLMResponse(
                content=data["choices"][0]["message"]["content"],
                model=data["model"],
                usage=data.get("usage", {}),
                raw_response=data
            )

            _log_model_call(payload, data, resp.usage)
            return resp


class VertexAIProvider(Provider):
    """Vertex AI provider using Google's OpenAI-compatible Chat Completions API."""

    def __init__(self, access_token: str, project_id: str | None = None, location: str | None = None):
        self.access_token = access_token
        self.project_id = project_id or os.environ.get("VERTEXAI_PROJECT") or os.environ.get("GOOGLE_CLOUD_PROJECT")
        self.location = location or os.environ.get("VERTEXAI_LOCATION", "global")

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
        json_mode: bool = False
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
            response.raise_for_status()
            data = response.json()

            resp = LLMResponse(
                content=data["choices"][0]["message"]["content"],
                model=data["model"],
                usage=data.get("usage", {}),
                raw_response=data
            )

            _log_model_call(payload, data, resp.usage)
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
        json_mode: bool = False
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
