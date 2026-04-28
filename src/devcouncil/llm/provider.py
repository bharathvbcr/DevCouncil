from abc import ABC, abstractmethod
import copy
from typing import List, Dict, Any, Optional
from pydantic import BaseModel
import httpx
import json
from pathlib import Path

SUPPORTED_MODEL_PROVIDERS = ("openrouter",)


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
    if normalized in SUPPORTED_MODEL_PROVIDERS:
        return normalized
    supported = ", ".join(SUPPORTED_MODEL_PROVIDERS)
    raise ValueError(
        f"Unsupported model provider '{provider_name}'. "
        f"Supported providers: {supported}."
    )


def create_provider(provider_name: str, api_key: str) -> Provider:
    normalized = validate_model_provider(provider_name)
    if normalized == "openrouter":
        return OpenRouterProvider(api_key)
    raise AssertionError(f"Provider validation passed for unhandled provider: {normalized}")

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
            
            # Log the call
            try:
                from devcouncil.utils.redaction import redact_dict
                log_dir = Path(".devcouncil/logs")
                log_dir.mkdir(parents=True, exist_ok=True)
                log_file = log_dir / "model_calls.jsonl"
                
                # Create a redacted copy of both request and response for logging
                log_payload = {
                    "request": redact_dict(payload),
                    "response": redact_dict(data),
                    "usage": resp.usage,
                }
                with open(log_file, "a", encoding="utf-8") as f:
                    f.write(json.dumps(log_payload) + "\n")
            except Exception as e:
                import logging as _log
                _log.getLogger(__name__).debug("Failed to log model call: %s", e)
                
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
