"""Unified adapter for Ollama, llama.cpp, and Hugging Face backends."""

from __future__ import annotations

import abc
from typing import Any

import httpx

from .config import LLMConfig


class LLMBackend(abc.ABC):
    @abc.abstractmethod
    def generate(self, model: str, prompt: str, system: str | None = None) -> str:
        ...

    def close(self) -> None:
        """Release backend resources if any."""


class OllamaBackend(LLMBackend):
    def __init__(self, config: LLMConfig | None = None) -> None:
        self.config = config or LLMConfig()
        self._client = httpx.Client(base_url=self.config.base_url, timeout=self.config.timeout_seconds)

    def generate(self, model: str, prompt: str, system: str | None = None) -> str:
        payload: dict[str, Any] = {
            "model": model,
            "prompt": prompt,
            "stream": False,
        }
        if system:
            payload["system"] = system
        resp = self._client.post("/api/generate", json=payload)
        resp.raise_for_status()
        return resp.json()["response"]

    def close(self) -> None:
        self._client.close()


class LlamaCppBackend(LLMBackend):
    """OpenAI-compatible llama.cpp server (/v1/chat/completions)."""

    def __init__(self, config: LLMConfig | None = None) -> None:
        self.config = config or LLMConfig(base_url="http://localhost:8080")
        self._client = httpx.Client(base_url=self.config.base_url, timeout=self.config.timeout_seconds)

    def generate(self, model: str, prompt: str, system: str | None = None) -> str:
        messages = []
        if system:
            messages.append({"role": "system", "content": system})
        messages.append({"role": "user", "content": prompt})
        resp = self._client.post(
            "/v1/chat/completions",
            json={"model": model, "messages": messages, "stream": False},
        )
        resp.raise_for_status()
        return resp.json()["choices"][0]["message"]["content"]

    def close(self) -> None:
        self._client.close()


class HuggingFaceBackend(LLMBackend):
    """Local HF pipeline — lazy load to avoid cold-start in semantic layer process."""

    def __init__(self, config: LLMConfig | None = None) -> None:
        self.config = config or LLMConfig()
        self._pipelines: dict[str, Any] = {}

    def _get_pipeline(self, model: str) -> Any:
        if model not in self._pipelines:
            from transformers import pipeline

            self._pipelines[model] = pipeline(
                "text-generation",
                model=model,
                device_map="auto",
            )
        return self._pipelines[model]

    def generate(self, model: str, prompt: str, system: str | None = None) -> str:
        pipe = self._get_pipeline(model)
        full_prompt = f"{system}\n\n{prompt}" if system else prompt
        out = pipe(full_prompt, max_new_tokens=512, do_sample=False)
        return out[0]["generated_text"][len(full_prompt):]


def create_backend(config: LLMConfig | None = None) -> LLMBackend:
    cfg = config or LLMConfig()
    if cfg.backend == "ollama":
        return OllamaBackend(cfg)
    if cfg.backend == "llama_cpp":
        return LlamaCppBackend(cfg)
    if cfg.backend == "hf":
        return HuggingFaceBackend(cfg)
    raise ValueError(f"Unknown backend: {cfg.backend}")
