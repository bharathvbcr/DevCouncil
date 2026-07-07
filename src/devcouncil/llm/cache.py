import json
import hashlib
import logging
from pathlib import Path
from typing import Optional
from devcouncil.llm.provider import LLMResponse
from devcouncil.utils.json_persist import read_json, write_json

logger = logging.getLogger(__name__)

class LLMCache:
    def __init__(self, project_root: Path):
        self.cache_dir = project_root / ".devcouncil" / "cache" / "llm"
        self.cache_dir.mkdir(parents=True, exist_ok=True)

    def _get_key(self, model: str, messages: list, temp: float, json_mode: bool, provider_fingerprint: str = "") -> str:
        data = {
            "model": model,
            "messages": messages,
            "temp": temp,
            "json_mode": json_mode,
            # Provider-specific knobs that change the output for an identical prompt
            # (e.g. Ollama's num_ctx / base_url). Empty for providers without such knobs,
            # so their cache keys are unchanged.
            "provider": provider_fingerprint,
        }
        s = json.dumps(data, sort_keys=True)
        return hashlib.sha256(s.encode("utf-8")).hexdigest()

    def get(self, model: str, messages: list, temp: float, json_mode: bool, provider_fingerprint: str = "", cache_key: Optional[str] = None) -> Optional[LLMResponse]:
        # Hashing the JSON payload is non-trivial; let callers compute the key once
        # (via ``_get_key``) and pass it to both get() and set() to avoid recomputing.
        key = cache_key if cache_key is not None else self._get_key(model, messages, temp, json_mode, provider_fingerprint)
        cache_file = self.cache_dir / f"{key}.json"
        if cache_file.exists():
            try:
                data = read_json(cache_file)
                logger.debug("LLM cache HIT model=%s key=%s", model, key[:12])
                return LLMResponse(**data)
            except Exception as e:
                logger.warning("LLM cache read failed for key=%s: %s", key[:12], e)
        logger.debug("LLM cache MISS model=%s key=%s", model, key[:12])
        return None

    def set(self, model: str, messages: list, temp: float, json_mode: bool, response: LLMResponse, provider_fingerprint: str = "", cache_key: Optional[str] = None):
        key = cache_key if cache_key is not None else self._get_key(model, messages, temp, json_mode, provider_fingerprint)
        cache_file = self.cache_dir / f"{key}.json"
        write_json(cache_file, response.model_dump())
        logger.debug("LLM cache STORE model=%s key=%s", model, key[:12])
