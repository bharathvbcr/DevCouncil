from typing import List, Dict, Any, Type, Optional, TypeVar, cast
import copy
import functools
import json
import logging
import asyncio
import os
import re
import time
from pathlib import Path

import httpx
from pydantic import BaseModel
from devcouncil.llm.provider import Provider, LLMResponse, ProviderRequestError
from devcouncil.llm.cache import LLMCache
from devcouncil.telemetry.tracker import TelemetryTracker
from devcouncil.telemetry.traces import TraceLogger
from devcouncil.telemetry.stages import log_step

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


def _provider_retry_delay(exc: Exception, attempt: int) -> float:
    """Seconds to wait before retrying a failed provider call."""
    if isinstance(exc, ProviderRequestError):
        if exc.retry_after_seconds is not None:
            return float(min(120.0, max(1.0, exc.retry_after_seconds)))
        if exc.status_code == 429:
            # OpenRouter tiers often cap at ~20 RPM; back off generously.
            return float(min(90.0, 15.0 * (2 ** attempt)))
    return min(30.0, float(2 ** attempt))


def _rate_limit_retry_budget() -> int:
    """How many 429 responses to wait out per call (beyond the normal attempts).

    Default 8: with the 429 backoff above that is ~7 minutes of patience —
    enough to ride out an RPM-window burst, small enough that a hard quota
    (daily cap) still fails the call in bounded time. Override with
    DEVCOUNCIL_RATE_LIMIT_RETRIES; 0 disables the separate budget."""
    raw = os.environ.get("DEVCOUNCIL_RATE_LIMIT_RETRIES", "")
    try:
        value = int(raw)
    except ValueError:
        return 8
    return max(0, value)


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
        semantic_adapter: Optional[Any] = None,
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
        # Optional semantic cache / routing / compression (config-driven, lazy).
        if semantic_adapter is not None:
            self._semantic = semantic_adapter
        else:
            from devcouncil.llm.semantic_bridge import load_semantic_adapter

            self._semantic = load_semantic_adapter(project_root)
        if self._semantic is not None:
            self._semantic.warm_up()
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

    # Inline reasoning blocks emitted by thinking models when the serving stack does
    # not split them into a separate channel (local runners with a mismatched chat
    # template are the common case). Removed before JSON extraction: the reasoning
    # prose routinely contains JSON-looking examples that would otherwise be picked
    # up instead of the real answer that follows the block.
    _THINK_BLOCK_RE = re.compile(
        r"<(think|thinking|reasoning|thought)>.*?</\1>", re.DOTALL | re.IGNORECASE
    )
    # An UNCLOSED reasoning tag (generation cut off or template quirk): everything
    # from the opener is reasoning; nothing after it to salvage, but text BEFORE a
    # dangling opener (rare) may hold the answer.
    _THINK_OPEN_RE = re.compile(r"<(think|thinking|reasoning|thought)>", re.IGNORECASE)

    @staticmethod
    def _balanced_candidates(text: str, opener: str, closer: str):
        """Yield every balanced top-level ``opener...closer`` span in ``text``,
        string- and escape-aware so braces inside string values don't confuse it."""
        start = text.find(opener)
        while start != -1:
            depth = 0
            in_str = False
            escaped = False
            end = -1
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
                        end = i
                        break
            if end == -1:
                return
            yield text[start:end + 1]
            start = text.find(opener, end + 1)

    @classmethod
    def _extract_json(cls, content: str) -> str:
        """Best-effort extraction of a JSON document from a model response.

        Handles the common ways a model wraps valid JSON: inline ``<think>`` blocks
        (local/thinking models whose serving stack leaves reasoning in the content),
        triple-backtick fences, and surrounding prose ("Here you go: {...} thanks").
        Strips reasoning blocks and fences, returns the whole thing if it already
        parses, otherwise scans EVERY balanced object/array candidate (string- and
        escape-aware) and returns the first that parses — a JSON-looking fragment in
        leading prose no longer masks the real answer that follows it. Falls back to
        the de-fenced text so the existing healing path still produces a meaningful
        error. A strict superset of plain fence-stripping — clean/fenced JSON is
        returned unchanged."""
        text = content.strip()
        # Drop closed reasoning blocks; on a dangling opener keep only what precedes it.
        if "<" in text:
            stripped = cls._THINK_BLOCK_RE.sub("", text).strip()
            dangling = cls._THINK_OPEN_RE.search(stripped)
            if dangling and "</" not in stripped[dangling.start():]:
                before = stripped[:dangling.start()].strip()
                after = stripped[dangling.end():].strip()
                # The answer usually follows the (cut-off) reasoning; prefer whichever
                # side actually contains a JSON-ish payload.
                stripped = after if ("{" in after or "[" in after) else before
            if stripped:
                text = stripped
        if "```json" in text:
            text = text.split("```json", 1)[1].split("```", 1)[0].strip()
        elif "```" in text:
            text = text.split("```", 1)[1].split("```", 1)[0].strip()
        try:
            json.loads(text)
            return text
        except Exception as e:
            logger.debug("Response is not directly parseable JSON, scanning balanced candidates: %s", e)
        for opener, closer in (("{", "}"), ("[", "]")):
            for candidate in cls._balanced_candidates(text, opener, closer):
                try:
                    json.loads(candidate)
                    return cast(str, candidate)
                except Exception:
                    continue
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
        attempts: int = 5,
        json_schema: Optional[Dict[str, Any]] = None,
    ) -> "LLMResponse":
        """Provider completion with bounded exponential-backoff retry. Used for BOTH the
        initial call and the healing call so a transient fault in either is retried (and,
        if still failing, surfaced to the caller's fallback logic) rather than aborting
        the run. ``provider`` defaults to the router's default provider but may be a
        per-role provider for roles that override ``models.provider``. ``json_schema``
        flows through to providers that support grammar-constrained structured output
        (Ollama); others ignore it."""
        if run_id is None:
            # Fall back to the orchestrator-declared run so model_calls.jsonl
            # records stay attributable even when call sites don't thread run_id.
            from devcouncil.telemetry.context import get_current_run_id

            run_id = get_current_run_id()
        provider = provider or self.provider
        # Only pass json_schema to providers whose ``complete`` accepts it — duck-typed
        # or third-party Provider implementations may predate the parameter, and a
        # structured-output OPTIMIZATION must never break them.
        extra_kwargs: Dict[str, Any] = {}
        if json_schema is not None:
            import inspect

            try:
                if "json_schema" in inspect.signature(provider.complete).parameters:
                    extra_kwargs["json_schema"] = json_schema
            except (TypeError, ValueError):
                pass
        # Rate limiting (429) gets its OWN, larger budget: it is the provider
        # telling us exactly when to come back (Retry-After), not a fault in the
        # request — counting it against the shared ``attempts`` let a busy
        # endpoint exhaust the budget and kill a run that only needed patience
        # (observed: benchmark tasks dying blocked on limit_rpm mid-run).
        max_rate_limit_retries = _rate_limit_retry_budget()
        attempt = 0
        rate_limit_retries = 0
        while True:
            try:
                return await provider.complete(
                    model=model,
                    messages=messages,
                    temperature=temperature,
                    json_mode=True,
                    run_id=run_id,
                    **extra_kwargs,
                )
            except httpx.TimeoutException as exc:
                # A timeout already consumed the provider's ENTIRE request window
                # (600s by default on a local Ollama host). Retrying the identical
                # request almost always times out again — and the structured-output
                # layers above this (healing call + fresh attempts) would multiply
                # the stall until the whole run is killed from outside (observed:
                # benchmark arms burning their full 20-minute budget on 2x 600s
                # timeouts and dying with exit 124 before producing a verdict).
                # Fail fast with an actionable message instead.
                raise ProviderRequestError(
                    f"LLM request to model '{model}' timed out after the provider's "
                    f"request window ({exc!r}). If this is a local (Ollama) model, "
                    "generation is too slow for the configured window: raise "
                    "OLLAMA_TIMEOUT, cap generation with OLLAMA_NUM_PREDICT, reduce "
                    "thinking with OLLAMA_THINK=low or OLLAMA_THINK=false, or use a "
                    "smaller/faster model."
                ) from exc
            except ProviderRequestError as exc:
                if exc.status_code == 429 and rate_limit_retries < max_rate_limit_retries:
                    rate_limit_retries += 1
                    delay = _provider_retry_delay(exc, rate_limit_retries)
                    logger.warning(
                        "LLM provider rate-limited (429, retry %d/%d): %r. Retrying in %.0fs...",
                        rate_limit_retries, max_rate_limit_retries, exc, delay,
                    )
                    await asyncio.sleep(delay)
                    continue
                attempt += 1
                if attempt >= attempts:
                    raise
                delay = _provider_retry_delay(exc, attempt - 1)
                logger.warning(
                    "LLM provider request failed (attempt %d/%d): %r. Retrying in %.0fs...",
                    attempt, attempts, exc, delay,
                )
                await asyncio.sleep(delay)
            except Exception as exc:
                attempt += 1
                if attempt >= attempts:
                    raise
                delay = _provider_retry_delay(exc, attempt - 1)
                # %r, not %s: common failures (httpx.ReadTimeout, CancelledError)
                # stringify to an EMPTY message, which previously logged the useless
                # "failed (attempt 1/3): ." and made timeouts undiagnosable from logs.
                logger.warning(
                    "LLM request failed (attempt %d/%d): %r. Retrying in %.0fs...",
                    attempt, attempts, exc, delay,
                )
                await asyncio.sleep(delay)

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
        role_provider = config.get("provider")

        # Deep-copy to avoid mutating the caller's messages list
        msgs = copy.deepcopy(messages)

        # Optional long-context compression before schema injection.
        if self._semantic is not None:
            msgs = await self._semantic.maybe_compress_messages_async(msgs)

        # Optional complexity-based model routing (local Ollama only when enabled).
        if self._semantic is not None:
            model = await self._semantic.maybe_route_model_async(
                msgs,
                configured_model=model,
                role_provider=role_provider,
            )
        
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
        semantic_cache_hit = False

        if not cache_hit and self._semantic is not None:
            response = await self._semantic.lookup_cache_async(msgs, model=model, role=role)
            if response is not None:
                cache_hit = True
                semantic_cache_hit = True

        # Structured-output schema for providers with grammar-constrained decoding
        # (Ollama's native ``format: <schema>``): the model cannot emit invalid JSON,
        # which on weak/local models eliminates most schema echoes and healing rounds.
        structured_schema = schema.model_json_schema()

        started = time.monotonic()
        if not response:
            try:
                response = await self._complete_with_retry(
                    model=model, messages=msgs, temperature=temp, run_id=run_id, provider=provider,
                    json_schema=structured_schema,
                )
            except ProviderRequestError as exc:
                # A degradable role (caller supplied a fallback) must degrade on a
                # provider failure exactly as it does on unparseable output: the
                # fallback exists to keep the run alive on a flaky/slow model, and a
                # fail-fast timeout (see _complete_with_retry) or exhausted retries
                # is the same class of "this role produced nothing usable".
                if fallback is not None:
                    logger.warning(
                        "Role '%s' (model '%s') provider request failed (%s); "
                        "using a safe fallback so the run can continue.",
                        role, model, exc,
                    )
                    traces.log_event(
                        "llm_provider_request_failed_fallback",
                        {"role": role, "model": model, "error": str(exc)},
                        run_id=run_id,
                        summary=f"Provider request failed for {role}; degraded to fallback.",
                    )
                    return fallback
                raise
        elapsed = time.monotonic() - started

        if response is None:
            raise RuntimeError(f"LLM request for role {role} did not return a response.")

        if not cache_hit:
            tracker.log_usage(model, response.usage, local=provider_local)

        cache_label = "cache_hit" if cache_hit else f"{elapsed:.1f}s"
        if semantic_cache_hit:
            cache_label = "semantic_cache_hit"

        # Include latency + cache status: on a slow (e.g. local) model this is what tells
        # you *which* call dominated a multi-minute planning/verification stage.
        logger.info(
            "LLM response: role=%s model=%s tokens=%s %s",
            role, response.model, response.usage,
            cache_label,
        )
        log_step(
            f"llm/{role}: {response.model} {cache_label}",
            project_root=self.project_root,
            run_id=run_id,
            role=role,
            model=response.model,
            latency_s=round(elapsed, 2) if not cache_hit else 0,
            cache_hit=cache_hit,
            semantic_cache_hit=semantic_cache_hit,
        )
        
        try:
            # Extract JSON from fences/surrounding prose (balanced-aware).
            content = self._extract_json(response.content)
            data = json.loads(content)
            result = schema.model_validate(data)
            if not cache_hit:
                cache.set(model, msgs, temp, True, response, provider_fp)  # cache only validated output
            if self._semantic is not None and not semantic_cache_hit:
                await self._semantic.store_cache_async(msgs, response, model=model, role=role)
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
            # The healing call must SEE the schema. Previously it got only the error
            # + bad content, so a "Field required" failure asked the model to invent
            # the missing fields blind — on providers without grammar-constrained
            # decoding (OpenRouter/Vertex, e.g. gemini-2.5-flash omitting empty
            # list fields) healing then failed the same way and the whole planning
            # run crashed. Reuse the same schema instruction as the initial call.
            healing_messages = [
                {"role": "system", "content": f"You repair malformed JSON.{instruction}"},
                {"role": "user", "content": healing_prompt},
            ]
            # The healing completion runs INSIDE this try (with the same retry/backoff as
            # the initial call). A transient failure here (429/timeout) must be treated as
            # "healing failed" so it routes into the fresh-attempt/fallback logic below,
            # not propagate as a raw provider error that defeats the supplied fallback.
            healed_response = None
            try:
                # We use a lower temperature for healing
                healed_response = await self._complete_with_retry(
                    model=model,
                    messages=healing_messages,
                    temperature=0.0,
                    run_id=run_id,
                    provider=provider,
                    json_schema=structured_schema,
                )
                tracker.log_usage(healed_response.model, healed_response.usage, local=provider_local)
                healed_content = self._extract_json(healed_response.content)
                data = json.loads(healed_content)
                result = schema.model_validate(data)
                cache.set(model, msgs, temp, True, healed_response, provider_fp)
                if self._semantic is not None:
                    await self._semantic.store_cache_async(msgs, healed_response, model=model, role=role)
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
