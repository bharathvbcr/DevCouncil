from typing import List, Dict, Any, Type, Optional, TypeVar
import copy
import functools
import json
import logging
import asyncio
import time
from pathlib import Path

from pydantic import BaseModel
from devcouncil.llm.provider import Provider, LLMResponse
from devcouncil.llm.cache import LLMCache
from devcouncil.telemetry.tracker import TelemetryTracker
from devcouncil.telemetry.traces import TraceLogger

logger = logging.getLogger(__name__)
StructuredModel = TypeVar("StructuredModel", bound=BaseModel)


@functools.lru_cache(maxsize=128)
def _cached_schema_json(schema_class) -> str:
    return json.dumps(schema_class.model_json_schema(), indent=2)


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
        # LLMCache and TraceLogger do disk I/O (mkdir) in their constructors, so build
        # them once here and reuse across calls. TelemetryTracker is deliberately *not*
        # hoisted: it is constructed per-call so log_usage's reload-before-save stays
        # concurrent-write safe.
        self._cache = LLMCache(self.project_root)
        self._traces = TraceLogger(self.project_root)
        # Lazily-built providers for roles that override ``models.provider`` with
        # their own ``provider:`` (e.g. live_reviewer on Ollama while planners run
        # on OpenRouter). Keyed by normalized provider name; the default provider
        # passed in above is reused for roles without an override.
        self._role_providers: Dict[str, Provider] = {}

    def _provider_for_role(self, role_config: Dict[str, Any]) -> Provider:
        """Resolve the provider for a role, honoring a per-role ``provider`` override.

        Roles without an override use the default provider supplied at construction.
        Overriding roles get a provider built on demand (and cached) from the
        configured credentials, so one router can fan a single run across multiple
        providers."""
        role_provider = role_config.get("provider")
        if not role_provider:
            return self.provider
        # Local imports avoid a circular import at module load (provider/config
        # both reference this package).
        from devcouncil.llm.provider import create_provider, validate_model_provider
        from devcouncil.app.config import get_api_key

        normalized = validate_model_provider(role_provider)
        if normalized not in self._role_providers:
            api_key = get_api_key(normalized, self.project_root)
            # An override to OpenRouter must still honor the project's provider-routing
            # prefs (sort/allow_fallbacks/data_collection); other providers ignore them.
            # Best-effort: a missing/invalid config just yields default routing.
            prefs = None
            if normalized == "openrouter":
                try:
                    from devcouncil.app.config import load_config

                    prefs = load_config(self.project_root).provider
                except Exception:
                    prefs = None
            self._role_providers[normalized] = create_provider(
                normalized, api_key, project_root=self.project_root, provider_prefs=prefs
            )
        return self._role_providers[normalized]

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

    @staticmethod
    def _looks_like_schema_echo(text: str) -> bool:
        """True when the model returned the JSON *schema* instead of an instance.

        Weaker/local models sometimes parrot the schema document we showed them
        (``{"$defs": ..., "properties": ..., "type": "object"}``). That parses as JSON
        but never validates, so detecting it lets the healing retry give a pointed
        correction instead of the generic "fix your JSON" nudge."""
        try:
            obj = json.loads(text)
        except Exception:
            return False
        if not isinstance(obj, dict):
            return False
        markers = {"$schema", "$defs", "properties", "additionalProperties", "$ref"}
        return bool(markers & set(obj.keys()))

    async def _complete_with_retry(
        self,
        *,
        model: str,
        messages: List[Dict[str, str]],
        temperature: float,
        run_id: Optional[str],
        provider: Optional[Provider] = None,
        attempts: int = 3,
    ) -> "LLMResponse":
        """Provider completion with bounded exponential-backoff retry. Used for BOTH the
        initial call and the healing call so a transient fault in either is retried (and,
        if still failing, surfaced to the caller's fallback logic) rather than aborting
        the run. ``provider`` defaults to the router's default provider but may be a
        per-role provider for roles that override ``models.provider``."""
        provider = provider or self.provider
        for attempt in range(attempts):
            try:
                return await provider.complete(
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
        provider = self._provider_for_role(config)
        
        # Deep-copy to avoid mutating the caller's messages list
        msgs = copy.deepcopy(messages)
        
        # Add schema instructions to system or user message. Spell out "instance, not
        # the schema" explicitly: weaker/local models otherwise sometimes echo the schema
        # document back (``{"$defs": ..., "properties": ..., "type": "object"}``), which
        # parses as JSON but fails validation and wastes a healing round.
        schema_json = _cached_schema_json(schema)
        instruction = (
            "\n\nYou MUST output a single JSON object that is an INSTANCE of this schema — "
            "real values for each field. Do NOT output the schema itself; never include "
            'keys like "$defs", "$schema", "properties", or "type".\nSchema:\n'
            f"{schema_json}"
        )
        
        found_system = False
        for msg in msgs:
            if msg["role"] == "system":
                msg["content"] += instruction
                found_system = True
                break
        
        if not found_system:
            msgs.insert(0, {"role": "system", "content": f"You are a helpful assistant.{instruction}"})

        logger.info("LLM call: role=%s model=%s run_id=%s", role, model, run_id)

        cache = self._cache
        tracker = TelemetryTracker(self.project_root)
        traces = self._traces

        # Provider knobs (e.g. Ollama num_ctx / base_url) that change the output for an
        # identical prompt must be part of the cache key, else raising OLLAMA_NUM_CTX
        # after a truncated answer would keep serving the stale response.
        provider_fp = provider.cache_fingerprint()
        # Zero local (Ollama) usage by provider so telemetry matches the cost ledger.
        provider_local = provider.is_local_cost_free()

        # Check cache first
        response = cache.get(model, msgs, temp, True, provider_fp)
        cache_hit = response is not None

        started = time.monotonic()
        if not response:
            response = await self._complete_with_retry(
                model=model, messages=msgs, temperature=temp, run_id=run_id, provider=provider
            )
        elapsed = time.monotonic() - started

        if response is None:
            raise RuntimeError(f"LLM request for role {role} did not return a response.")

        if not cache_hit:
            tracker.log_usage(model, response.usage, local=provider_local)

        # Include latency + cache status: on a slow (e.g. local) model this is what tells
        # you *which* call dominated a multi-minute planning/verification stage.
        logger.info(
            "LLM response: role=%s model=%s tokens=%s %s",
            role, response.model, response.usage,
            "cache_hit" if cache_hit else f"{elapsed:.1f}s",
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
            
            # Healing attempt: Ask the model to fix its own JSON. If it echoed the schema
            # back instead of an instance, say so explicitly — the generic "fix it" nudge
            # otherwise tends to produce the schema again.
            echo_hint = ""
            if self._looks_like_schema_echo(self._extract_json(response.content)):
                echo_hint = (
                    "\nIMPORTANT: You returned the JSON *schema* (it contains keys like "
                    '"$defs"/"properties"/"type"), not a value. Return a concrete INSTANCE: '
                    "a JSON object whose keys are the schema's property names, each with a "
                    "real value of the correct type."
                )
            healing_prompt = f"""
The following JSON was returned but failed to parse or validate against the schema.
Error: {str(e)}
Content:
{response.content}
{echo_hint}
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
                    provider=provider,
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
                    strict_messages = [
                        {
                            "role": "system",
                            "content": (
                                "Respond with a single valid JSON object only — no prose, no "
                                "markdown fences, no trailing text. It must parse with a strict "
                                "JSON parser and match the requested schema."
                            ),
                        },
                        *messages,
                    ]
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
