import json
import hashlib
from pathlib import Path
from typing import Optional
from devcouncil.llm.provider import LLMResponse

class LLMCache:
    def __init__(self, project_root: Path):
        self.cache_dir = project_root / ".devcouncil" / "cache" / "llm"
        self.cache_dir.mkdir(parents=True, exist_ok=True)

    def _get_key(self, model: str, messages: list, temp: float, json_mode: bool) -> str:
        data = {
            "model": model,
            "messages": messages,
            "temp": temp,
            "json_mode": json_mode
        }
        s = json.dumps(data, sort_keys=True)
        return hashlib.sha256(s.encode("utf-8")).hexdigest()

    def get(self, model: str, messages: list, temp: float, json_mode: bool) -> Optional[LLMResponse]:
        key = self._get_key(model, messages, temp, json_mode)
        cache_file = self.cache_dir / f"{key}.json"
        if cache_file.exists():
            try:
                with open(cache_file, "r") as f:
                    data = json.load(f)
                    return LLMResponse(**data)
            except Exception:
                pass
        return None

    def set(self, model: str, messages: list, temp: float, json_mode: bool, response: LLMResponse):
        key = self._get_key(model, messages, temp, json_mode)
        cache_file = self.cache_dir / f"{key}.json"
        with open(cache_file, "w") as f:
            json.dump(response.model_dump(), f)
