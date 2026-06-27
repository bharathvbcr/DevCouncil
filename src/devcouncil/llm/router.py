from typing import List, Dict, Any, Type, Optional, TypeVar
import copy
import json
import logging
import asyncio
from pathlib import Path

from pydantic import BaseModel
from devcouncil.llm.provider import Provider, LLMResponse
from devcouncil.llm.cache import LLMCache
from devcouncil.telemetry.tracker import TelemetryTracker
from devcouncil.telemetry.traces import TraceLogger

logger = logging.getLogger(__name__)
StructuredModel = TypeVar("StructuredModel", bound=BaseModel)


class StructuredOutputError(RuntimeError):
    """A model could not produce valid structured output for a role, even after
    a healing retry. Carries the role/model so the CLI can give actionable advice
    (usually: switch that role to a more capable model)."""

    def __init__(self, message: str, *, role: str, model: str):
        super().__init__(message)
        self.role = role
        self.model = model


class ModelRouter:
    # Independent fresh attempts at producing valid structured output before
    # giving up. Even capable models occasionally emit malformed JSON; a second
    # clean attempt usually succeeds. Malformed responses are never cached, so a
    # retry is genuinely fresh rather than re-serving the same bad JSON.
    STRUCTURED_ATTEMPTS = 2

    def __init__(
        self,
        provider: Provider,
        role_config: Dict[str, Dict[str, Any]],
        project_root: Path = Path("."),
    ):
        self.provider = provider
        self.role_config = role_config
        self.project_root = project_root

    @staticmethod
    def _extract_json(content: str) -> str:
        """Best-effort extraction of a JSON document from a model response.

        Handles the common ways a model wraps valid JSON: triple-backtick fences and
        surrounding prose ("Here you go: {...} thanks"). Strips fences, returns the
        whole thing if it already parses, otherwise scans for the first balanced
        object/array (string- and escape-aware so braces inside string values don't
        confuse it). Falls back to the de-fenced text so the existing healing path
        still produces a meaningful error. A strict superset of plain fence-stripping
        — clean/fenced JSON is returned unchanged."""
        text = content.strip()
        if "```json" in text:
            text = text.split("```json", 1)[1].split("```", 1)[0].strip()
        elif "```" in text:
            text = text.split("```", 1)[1].split("```", 1)[0].strip()
        try:
            json.loads(text)
            return text
        except Exception:
            pass
        for opener, closer in (("{", "}"), ("[", "]")):
            start = text.find(opener)
            if start == -1:
                continue
            depth = 0
            in_str = False
            escaped = False
            for i in range(start, len(text)):
                ch = text[i]
                if in_str:
                    if escaped:
                        escaped = False
                    elif ch == "\\":
                        escaped = True
                    elif ch == '"':
                        in_str = False
                    continue
                if ch == '"':
                    in_str = True
                elif ch == opener:
                    depth += 1
                elif ch == closer:
                    depth -= 1
                    if depth == 0:
                        candidate = text[start:i + 1]
                        try:
                            json.loads(candidate)
                            return candidate
                        except Exception:
                            break
        return text

    async def _complete_with_retry(
        self,
        *,
        model: str,
        messages: List[Dict[str, str]],
        temperature: float,
        run_id: Optional[str],
        attempts: int = 3,
    ) -> "LLMResponse":
        """Provider completion with bounded exponential-backoff retry. Used for BOTH the
        initial call and the healing call so a transient fault in either is retried (and,
        if still failing, surfaced to the caller's fallback logic) rather than aborting
        the run."""
        for attempt in range(attempts):
            try:
                return await self.provider.complete(
                    model=model,
                    messages=messages,
                    temperature=temperature,
                    json_mode=True,
                    run_id=run_id,
                )
            except Exception as exc:
                if attempt == attempts - 1:
                    raise
                logger.warning(
                    "LLM request failed (attempt %d/%d): %s. Retrying...",
                    attempt + 1, attempts, exc,
                )
                await asyncio.sleep(2 ** attempt)
        raise RuntimeError("unreachable")  # loop either returns or raises

    async def complete_structured(
        self,
        role: str,
        messages: List[Dict[str, str]],
        schema: Type[StructuredModel],
        temperature: Optional[float] = None,
        run_id: Optional[str] = None,
        fallback: Optional[StructuredModel] = None,
        _attempt: int = 0,
    ) -> StructuredModel:
        config = self.role_config.get(role)
        if not config:
            raise ValueError(f"No config found for role: {role}")
        
        model = config["model"]
        temp = temperature if temperature is not None else config.get("temperature", 0.0)
        
        # Deep-copy to avoid mutating the caller's messages list
        msgs = copy.deepcopy(messages)
        
        # Add schema instructions to system or user message
        schema_json = json.dumps(schema.model_json_schema(), indent=2)
        instruction = f"\n\nYou MUST output a JSON object matching this schema:\n{schema_json}"
        
        found_system = False
        for msg in msgs:
            if msg["role"] == "system":
                msg["content"] += instruction
                found_system = True
                break
        
        if not found_system:
            msgs.insert(0, {"role": "system", "content": f"You are a helpful assistant.{instruction}"})

        logger.info("LLM call: role=%s model=%s run_id=%s", role, model, run_id)

        cache = LLMCache(self.project_root)
        tracker = TelemetryTracker(self.project_root)
        traces = TraceLogger(self.project_root)

        # Provider knobs (e.g. Ollama num_ctx / base_url) that change the output for an
        # identical prompt must be part of the cache key, else raising OLLAMA_NUM_CTX
        # after a truncated answer would keep serving the stale response.
        provider_fp = self.provider.cache_fingerprint()
        # Zero local (Ollama) usage by provider so telemetry matches the cost ledger.
        provider_local = self.provider.is_local_cost_free()

        # Check cache first
        response = cache.get(model, msgs, temp, True, provider_fp)
        cache_hit = response is not None

        if not response:
            response = await self._complete_with_retry(
                model=model, messages=msgs, temperature=temp, run_id=run_id
            )

        if response is None:
            raise RuntimeError(f"LLM request for role {role} did not return a response.")

        if not cache_hit:
            tracker.log_usage(model, response.usage, local=provider_local)

        logger.info(
            "LLM response: role=%s model=%s tokens=%s",
            role, response.model, response.usage,
        )
        
        try:
            # Extract JSON from fences/surrounding prose (balanced-aware).
            content = self._extract_json(response.content)
            data = json.loads(content)
            result = schema.model_validate(data)
            if not cache_hit:
                cache.set(model, msgs, temp, True, response, provider_fp)  # cache only validated output
            return result
        except Exception as e:
            logger.warning(f"Initial parse failed for {role}, attempting healing: {e}")
            traces.log_event(
                "llm_structured_parse_failed",
                {
                    "role": role,
                    "model": response.model,
                    "schema": schema.__name__,
                    "error": str(e),
                    "content_preview": response.content[:500],
                },
                run_id=run_id,
                summary=f"Structured response parse failed for {role}; attempting repair.",
            )
            
            # Healing attempt: Ask the model to fix its own JSON
            healing_prompt = f"""
The following JSON was returned but failed to parse or validate against the schema.
Error: {str(e)}
Content:
{response.content}

Please return the corrected JSON object only. No prose.
"""
            # The healing completion runs INSIDE this try (with the same retry/backoff as
            # the initial call). A transient failure here (429/timeout) must be treated as
            # "healing failed" so it routes into the fresh-attempt/fallback logic below,
            # not propagate as a raw provider error that defeats the supplied fallback.
            healed_response = None
            try:
                # We use a lower temperature for healing
                healed_response = await self._complete_with_retry(
                    model=model,
                    messages=[{"role": "user", "content": healing_prompt}],
                    temperature=0.0,
                    run_id=run_id,
                )
                tracker.log_usage(healed_response.model, healed_response.usage, local=provider_local)
                healed_content = self._extract_json(healed_response.content)
                data = json.loads(healed_content)
                result = schema.model_validate(data)
                cache.set(model, msgs, temp, True, healed_response, provider_fp)
                return result
            except Exception as final_e:
                logger.error(f"Healing failed for {role}: {final_e}")
                traces.log_event(
                    "llm_structured_parse_repair_failed",
                    {
                        "role": role,
                        "model": healed_response.model if healed_response else model,
                        "schema": schema.__name__,
                        "error": str(final_e),
                        "original_content_preview": response.content[:500],
                        "healed_content_preview": healed_response.content[:500] if healed_response else "(healing request failed)",
                    },
                    run_id=run_id,
                    summary=f"Structured response repair failed for {role}.",
                )
                if _attempt + 1 < self.STRUCTURED_ATTEMPTS:
                    # A fresh, independent attempt often succeeds where one bad draft
                    # (plus its repair) failed. The malformed response was never
                    # cached, so this re-runs the completion rather than re-reading it.
                    # Prepend a strict JSON-only instruction (on a copy, so the caller's
                    # messages are untouched) to nudge the retry toward parseable output.
                    logger.warning(
                        "Structured output failed for role '%s'; retrying fresh "
                        "(attempt %d/%d).",
                        role, _attempt + 2, self.STRUCTURED_ATTEMPTS,
                    )
                    strict_messages = copy.deepcopy(messages)
                    strict_messages.insert(0, {
                        "role": "system",
                        "content": (
                            "Respond with a single valid JSON object only — no prose, no "
                            "markdown fences, no trailing text. It must parse with a strict "
                            "JSON parser and match the requested schema."
                        ),
                    })
                    return await self.complete_structured(
                        role,
                        strict_messages,
                        schema,
                        temperature=temperature,
                        run_id=run_id,
                        fallback=fallback,
                        _attempt=_attempt + 1,
                    )
                if fallback is not None:
                    # Degradable role (e.g. critique/rebuttal/enhancement): keep
                    # planning alive on weaker models instead of crashing the run.
                    logger.warning(
                        "Role '%s' (model '%s') could not produce valid %s; "
                        "using a safe fallback so planning can continue.",
                        role, model, schema.__name__,
                    )
                    return fallback
                raise StructuredOutputError(
                    f"Model '{model}' for role '{role}' could not produce valid "
                    f"{schema.__name__} JSON, even after a repair attempt. "
                    f"Use a more capable model for this role "
                    f"(e.g. 'dev config models --role {role} --model <model>'). "
                    f"Parser error: {final_e}",
                    role=role,
                    model=model,
                )
