"""Optional semantic-layer integration for DevCouncil's LLM path.

Wraps ``semantic_layer`` cache, router, and compressor components behind a lazy
adapter that degrades gracefully when optional deps are not installed.
"""

from __future__ import annotations

import asyncio
import atexit
import logging
import random
from pathlib import Path
from typing import TYPE_CHECKING, Any

from devcouncil.llm.provider import LLMResponse

if TYPE_CHECKING:
    from devcouncil.app.config import DevCouncilConfig, SemanticLayerConfig as SemanticLayerSettings

logger = logging.getLogger(__name__)

_ADAPTER_BY_ROOT: dict[Path, SemanticLayerAdapter] = {}
_SHUTDOWN_REGISTERED = False


def prompt_text_from_messages(messages: list[dict[str, str]]) -> str:
    """Stable text key for semantic cache lookup from chat messages."""
    parts: list[str] = []
    for msg in messages:
        role = msg.get("role", "user")
        content = (msg.get("content") or "").strip()
        if content:
            parts.append(f"{role}:{content}")
    return "\n".join(parts)


def semantic_deps_available() -> bool:
    """True when the lightweight semantic-layer runtime deps are importable."""
    try:
        import faiss  # noqa: F401
        import numpy  # noqa: F401
    except ImportError:
        return False
    return True


def semantic_embedding_deps_available() -> bool:
    """True when sentence-transformers is importable for embedding."""
    try:
        import sentence_transformers  # noqa: F401
    except ImportError:
        return False
    return True


def _register_shutdown_once() -> None:
    global _SHUTDOWN_REGISTERED
    if _SHUTDOWN_REGISTERED:
        return
    atexit.register(_persist_all_adapters)
    _SHUTDOWN_REGISTERED = True


def _persist_all_adapters() -> None:
    for adapter in list(_ADAPTER_BY_ROOT.values()):
        try:
            adapter.persist_cache()
        except Exception as exc:
            logger.debug("Semantic cache shutdown persist failed: %s", exc)


def reset_semantic_adapters_for_tests() -> None:
    """Clear the per-project adapter singleton — tests only."""
    _ADAPTER_BY_ROOT.clear()


class SemanticLayerAdapter:
    """Lazy facade over semantic_layer components, driven by DevCouncil config."""

    def __init__(self, settings: SemanticLayerSettings, project_root: Path = Path(".")) -> None:
        self.settings = settings
        self.project_root = project_root
        self._initialized = False
        self._init_failed = False
        self._cache: Any = None
        self._router: Any = None
        self._compressor: Any = None
        self._embedder: Any = None

    @property
    def active(self) -> bool:
        return self.settings.enabled and not self._init_failed

    @property
    def cache_enabled(self) -> bool:
        return self.active and self.settings.cache.enabled

    @property
    def router_enabled(self) -> bool:
        return self.active and self.settings.router.enabled

    @property
    def compressor_enabled(self) -> bool:
        return self.active and self.settings.compressor.enabled

    def cache_dir(self) -> Path:
        return self.project_root / ".devcouncil" / "cache" / "semantic"

    def cache_base_path(self) -> Path:
        return self.cache_dir() / self.settings.cache.namespace

    def _ensure_initialized(self) -> bool:
        if self._initialized:
            return not self._init_failed
        self._initialized = True
        if not self.settings.enabled:
            self._init_failed = True
            return False
        if not semantic_deps_available():
            logger.info(
                "Semantic layer enabled in config but optional deps are missing "
                "(install with: uv sync --group semantic). Falling back to standard LLM path."
            )
            self._init_failed = True
            return False
        try:
            from semantic_layer.cache import SemanticCache
            from semantic_layer.compressor import SemanticCompressor
            from semantic_layer.config import (
                CacheConfig,
                CompressorConfig,
                EmbeddingConfig,
                RouterConfig,
            )
            from semantic_layer.embeddings import EmbeddingService
            from semantic_layer.router import SemanticRouter

            emb = self.settings.embedding
            embedding_cfg = EmbeddingConfig(
                model_name=emb.model_name,
                dimension=emb.dimension,
                device=emb.device,  # type: ignore[arg-type]
                batch_size=emb.batch_size,
                normalize=emb.normalize,
            )
            self._embedder = EmbeddingService.get_instance(embedding_cfg)

            cache_cfg = self.settings.cache
            self._cache = SemanticCache(
                CacheConfig(
                    backend=cache_cfg.backend,  # type: ignore[arg-type]
                    similarity_threshold=cache_cfg.similarity_threshold,
                    ood_threshold=cache_cfg.ood_threshold,
                    margin_threshold=cache_cfg.margin_threshold,
                    ttl_seconds=cache_cfg.ttl_seconds,
                    max_entries=cache_cfg.max_entries,
                    namespace=cache_cfg.namespace,
                    exploration_rate=cache_cfg.exploration_rate,
                ),
                self._embedder,
                embedding_cfg.dimension,
            )
            self._load_persisted_cache()

            router_cfg = self.settings.router
            self._router = SemanticRouter(
                RouterConfig(
                    complexity_threshold=router_cfg.complexity_threshold,
                    small_model=router_cfg.small_model or "qwen2.5:1.5b",
                    large_model=router_cfg.large_model or "llama3.1:8b",
                ),
                self._embedder,
            )

            comp_cfg = self.settings.compressor
            self._compressor = SemanticCompressor(
                CompressorConfig(
                    token_budget=comp_cfg.token_budget,
                    top_k=comp_cfg.top_k,
                    chunk_token_size=comp_cfg.chunk_token_size,
                    chunk_overlap=comp_cfg.chunk_overlap,
                    min_chunk_score=comp_cfg.min_chunk_score,
                    mmr_lambda=comp_cfg.mmr_lambda,
                ),
                self._embedder,
            )
            if self.cache_enabled:
                _register_shutdown_once()
            return True
        except Exception as exc:
            logger.warning("Semantic layer initialization failed; disabling: %s", exc)
            self._init_failed = True
            return False

    def _load_persisted_cache(self) -> None:
        if self._cache is None:
            return
        base = self.cache_base_path()
        faiss_path = Path(f"{base}.faiss")
        json_path = Path(f"{base}.json")
        if not faiss_path.is_file() or not json_path.is_file():
            return
        try:
            self._cache.load(str(base))
            logger.info(
                "Loaded semantic cache from %s (%d entries)",
                base,
                len(self._cache._entries),
            )
        except Exception as exc:
            logger.warning("Could not load persisted semantic cache from %s: %s", base, exc)

    def persist_cache(self) -> None:
        """Write the in-memory FAISS index and metadata to disk."""
        if not self.cache_enabled or self._cache is None:
            return
        base = self.cache_base_path()
        try:
            base.parent.mkdir(parents=True, exist_ok=True)
            self._cache.persist(str(base))
            logger.debug("Persisted semantic cache to %s (%d entries)", base, len(self._cache._entries))
        except Exception as exc:
            logger.warning("Semantic cache persist failed: %s", exc)

    def warm_up(self) -> bool:
        """Preload the embedding model so the first LLM call is not blocked."""
        if not self.settings.enabled:
            return False
        if not self._ensure_initialized() or self._embedder is None:
            return False
        try:
            self._embedder.embed_one("warmup")
            logger.debug("Semantic embedding model warmed up (%s)", self.settings.embedding.model_name)
            return True
        except Exception as exc:
            logger.warning("Semantic embedding warm-up failed: %s", exc)
            return False

    async def _embed_one_async(self, text: str) -> Any:
        assert self._embedder is not None
        return await asyncio.to_thread(self._embedder.embed_one, text)

    def _intent_for_prompt(self, prompt: str, query_vec: Any, *, model: str, role: str) -> str:
        route = self._router.route(prompt, query_vec) if self._router else None
        intent = f"{role}:{model}"
        if route is not None:
            intent = f"{role}:{model}:{route.intent}"
        return intent

    def lookup_cache(
        self,
        messages: list[dict[str, str]],
        *,
        model: str,
        role: str,
    ) -> LLMResponse | None:
        """Return a cached LLM response for a semantically similar prompt, if any."""
        if not self.cache_enabled or not self._ensure_initialized():
            return None
        assert self._cache is not None and self._embedder is not None

        prompt = prompt_text_from_messages(messages)
        if not prompt.strip():
            return None

        try:
            query_vec = self._embedder.embed_one(prompt)
            return self._lookup_with_vector(prompt, query_vec, model=model, role=role)
        except Exception as exc:
            logger.debug("Semantic cache lookup failed: %s", exc)
            return None

    async def lookup_cache_async(
        self,
        messages: list[dict[str, str]],
        *,
        model: str,
        role: str,
    ) -> LLMResponse | None:
        """Async semantic cache lookup; embedding runs in a worker thread."""
        if not self.cache_enabled or not self._ensure_initialized():
            return None
        assert self._cache is not None and self._embedder is not None

        prompt = prompt_text_from_messages(messages)
        if not prompt.strip():
            return None

        try:
            query_vec = await self._embed_one_async(prompt)
            return self._lookup_with_vector(prompt, query_vec, model=model, role=role)
        except Exception as exc:
            logger.debug("Semantic cache lookup failed: %s", exc)
            return None

    def _lookup_with_vector(
        self,
        prompt: str,
        query_vec: Any,
        *,
        model: str,
        role: str,
    ) -> LLMResponse | None:
        assert self._cache is not None
        intent = self._intent_for_prompt(prompt, query_vec, model=model, role=role)
        force_miss = random.random() < self.settings.cache.exploration_rate
        result = self._cache.lookup(prompt, query_vec, intent=intent, force_miss=force_miss)
        if not result.hit or not result.response:
            return None

        logger.info(
            "Semantic cache HIT role=%s model=%s similarity=%.3f",
            role,
            model,
            result.similarity,
        )
        return LLMResponse(
            content=result.response,
            model=model,
            usage={"prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0},
            raw_response={
                "semantic_cache": True,
                "similarity": result.similarity,
                "entry_id": result.entry_id,
            },
        )

    def store_cache(
        self,
        messages: list[dict[str, str]],
        response: LLMResponse,
        *,
        model: str,
        role: str,
    ) -> None:
        """Persist a validated LLM response in the semantic cache."""
        if not self.cache_enabled or not self._ensure_initialized():
            return
        assert self._cache is not None and self._embedder is not None

        prompt = prompt_text_from_messages(messages)
        if not prompt.strip() or not (response.content or "").strip():
            return

        try:
            query_vec = self._embedder.embed_one(prompt)
            intent = self._intent_for_prompt(prompt, query_vec, model=model, role=role)
            self._cache.put(prompt, response.content, query_vec, intent=intent)
        except Exception as exc:
            logger.debug("Semantic cache store failed: %s", exc)

    async def store_cache_async(
        self,
        messages: list[dict[str, str]],
        response: LLMResponse,
        *,
        model: str,
        role: str,
    ) -> None:
        """Async semantic cache store; embedding runs in a worker thread."""
        if not self.cache_enabled or not self._ensure_initialized():
            return
        assert self._cache is not None and self._embedder is not None

        prompt = prompt_text_from_messages(messages)
        if not prompt.strip() or not (response.content or "").strip():
            return

        try:
            query_vec = await self._embed_one_async(prompt)
            intent = self._intent_for_prompt(prompt, query_vec, model=model, role=role)
            self._cache.put(prompt, response.content, query_vec, intent=intent)
        except Exception as exc:
            logger.debug("Semantic cache store failed: %s", exc)

    def maybe_route_model(
        self,
        messages: list[dict[str, str]],
        *,
        configured_model: str,
        role_provider: str | None,
    ) -> str:
        """Optionally override the configured model based on query complexity."""
        if not self.router_enabled or not self._ensure_initialized():
            return configured_model
        assert self._router is not None and self._embedder is not None

        if (role_provider or "").strip().lower() not in {"ollama", "ollama-local", "ollama_local"}:
            return configured_model

        prompt = prompt_text_from_messages(messages)
        if not prompt.strip():
            return configured_model

        try:
            query_vec = self._embedder.embed_one(prompt)
            return self._route_with_vector(prompt, query_vec, configured_model=configured_model)
        except Exception as exc:
            logger.debug("Semantic routing failed: %s", exc)
            return configured_model

    async def maybe_route_model_async(
        self,
        messages: list[dict[str, str]],
        *,
        configured_model: str,
        role_provider: str | None,
    ) -> str:
        """Async model routing; embedding runs in a worker thread."""
        if not self.router_enabled or not self._ensure_initialized():
            return configured_model
        assert self._router is not None and self._embedder is not None

        if (role_provider or "").strip().lower() not in {"ollama", "ollama-local", "ollama_local"}:
            return configured_model

        prompt = prompt_text_from_messages(messages)
        if not prompt.strip():
            return configured_model

        try:
            query_vec = await self._embed_one_async(prompt)
            return self._route_with_vector(prompt, query_vec, configured_model=configured_model)
        except Exception as exc:
            logger.debug("Semantic routing failed: %s", exc)
            return configured_model

    def _route_with_vector(self, prompt: str, query_vec: Any, *, configured_model: str) -> str:
        assert self._router is not None
        decision = self._router.route(prompt, query_vec)
        logger.debug(
            "Semantic route role=%s complexity=%.3f tier=%s model=%s",
            configured_model,
            decision.complexity_score,
            decision.tier.value,
            decision.model_name,
        )
        return decision.model_name

    def maybe_compress_messages(self, messages: list[dict[str, str]]) -> list[dict[str, str]]:
        """Compress long user/context payloads before an LLM call."""
        if not self.compressor_enabled or not self._ensure_initialized():
            return messages
        assert self._compressor is not None and self._embedder is not None

        prepared = self._compression_inputs(messages)
        if prepared is None:
            return messages
        query, context_docs = prepared

        try:
            query_vec = self._embedder.embed_one(query)
            return self._compress_with_vector(messages, query, context_docs, query_vec)
        except Exception as exc:
            logger.debug("Semantic compression failed: %s", exc)
            return messages

    async def maybe_compress_messages_async(self, messages: list[dict[str, str]]) -> list[dict[str, str]]:
        """Async message compression; embedding runs in a worker thread."""
        if not self.compressor_enabled or not self._ensure_initialized():
            return messages
        assert self._compressor is not None and self._embedder is not None

        prepared = self._compression_inputs(messages)
        if prepared is None:
            return messages
        query, context_docs = prepared

        try:
            query_vec = await self._embed_one_async(query)
            return self._compress_with_vector(messages, query, context_docs, query_vec)
        except Exception as exc:
            logger.debug("Semantic compression failed: %s", exc)
            return messages

    def _compression_inputs(self, messages: list[dict[str, str]]) -> tuple[str, list[str]] | None:
        min_chars = self.settings.compressor.min_chars
        total_chars = sum(len(m.get("content") or "") for m in messages)
        if total_chars < min_chars:
            return None

        user_messages = [m for m in messages if m.get("role") == "user" and (m.get("content") or "").strip()]
        if not user_messages:
            return None

        query = user_messages[-1]["content"]
        context_docs = [
            m["content"]
            for m in messages
            if m is not user_messages[-1] and (m.get("content") or "").strip()
        ]
        if not context_docs:
            blob = query
            if len(blob) <= min_chars:
                return None
            split_at = max(500, len(blob) - 2000)
            context_docs = [blob[:split_at]]
            query = blob[split_at:].strip() or blob[-2000:]
        return query, context_docs

    def _compress_with_vector(
        self,
        messages: list[dict[str, str]],
        query: str,
        context_docs: list[str],
        query_vec: Any,
    ) -> list[dict[str, str]]:
        assert self._compressor is not None
        compressed = self._compressor.compress(query, context_docs, query_vec)
        if not compressed.text.strip():
            return messages

        user_messages = [m for m in messages if m.get("role") == "user" and (m.get("content") or "").strip()]
        compressed_messages: list[dict[str, str]] = []
        replaced = False
        for msg in messages:
            if msg.get("role") == "user" and not replaced and msg is user_messages[-1]:
                compressed_messages.append(
                    {
                        "role": "user",
                        "content": f"Context:\n{compressed.text}\n\nQuestion: {query}",
                    }
                )
                replaced = True
            elif msg.get("role") != "user" or msg is not user_messages[-1]:
                if msg.get("role") != "user":
                    compressed_messages.append(dict(msg))
        if not replaced:
            compressed_messages.append(
                {"role": "user", "content": f"Context:\n{compressed.text}\n\nQuestion: {query}"}
            )
        logger.info(
            "Semantic compression applied: %d -> %d chunks, ~%d tokens",
            compressed.chunks_total,
            compressed.chunks_used,
            compressed.estimated_tokens,
        )
        return compressed_messages


def check_semantic_layer(
    project_root: Path,
    config: DevCouncilConfig | None = None,
) -> list[tuple[str, str, str]]:
    """Doctor rows for semantic-layer readiness (never raises)."""
    ok = "[green]OK[/green]"
    warn = "[yellow]WARN[/yellow]"
    info = "[cyan]INFO[/cyan]"
    rows: list[tuple[str, str, str]] = []

    try:
        from devcouncil.app.config import load_config

        cfg = config if config is not None else load_config(project_root)
    except Exception as exc:
        rows.append(("Semantic layer", info, f"Config unavailable; skipping semantic checks ({exc})."))
        return rows

    sem = cfg.semantic_layer
    if not sem.enabled:
        rows.append(
            (
                "Semantic layer",
                info,
                "Disabled. Enable with: dev config set semantic_layer.enabled true "
                "(requires: uv sync --group semantic).",
            )
        )
        return rows

    if not semantic_deps_available():
        rows.append(
            (
                "Semantic deps",
                warn,
                "Missing faiss/numpy. Install with: uv sync --group semantic.",
            )
        )
        return rows

    rows.append(("Semantic layer", ok, "Enabled in config."))

    if not semantic_embedding_deps_available():
        rows.append(
            (
                "Semantic embedder",
                warn,
                "Missing sentence-transformers. Install with: uv sync --group semantic.",
            )
        )
    else:
        rows.append(
            (
                "Semantic embedder",
                ok,
                f"sentence-transformers available; model={sem.embedding.model_name}.",
            )
        )

    cache_dir = project_root / ".devcouncil" / "cache" / "semantic"
    try:
        cache_dir.mkdir(parents=True, exist_ok=True)
        probe = cache_dir / ".write_probe"
        probe.write_text("ok", encoding="utf-8")
        probe.unlink(missing_ok=True)
        persisted = cache_dir / f"{sem.cache.namespace}.faiss"
        detail = f"Writable at {cache_dir}."
        if persisted.is_file():
            detail += f" Persisted index present ({sem.cache.namespace})."
        rows.append(("Semantic cache dir", ok, detail))
    except Exception as exc:
        rows.append(
            (
                "Semantic cache dir",
                warn,
                f"Not writable at {cache_dir}: {exc}.",
            )
        )

    adapter = _ADAPTER_BY_ROOT.get(project_root.resolve())
    if adapter is None:
        adapter = SemanticLayerAdapter(sem, project_root)
    if adapter.warm_up():
        rows.append(("Semantic warm-up", ok, "Embedding model loaded successfully."))
    else:
        rows.append(
            (
                "Semantic warm-up",
                warn,
                f"Could not load embedding model {sem.embedding.model_name!r}. "
                "First LLM call will retry or fall back to the standard path.",
            )
        )

    return rows


def load_semantic_adapter(project_root: Path) -> SemanticLayerAdapter | None:
    """Build a semantic adapter from project config, or None when disabled/unavailable."""
    try:
        from devcouncil.app.config import load_config

        cfg = load_config(project_root)
        if not cfg.semantic_layer.enabled:
            return None
        key = project_root.resolve()
        existing = _ADAPTER_BY_ROOT.get(key)
        if existing is not None:
            return existing
        adapter = SemanticLayerAdapter(cfg.semantic_layer, project_root)
        _ADAPTER_BY_ROOT[key] = adapter
        return adapter
    except FileNotFoundError:
        return None
    except Exception as exc:
        logger.debug("Could not load semantic layer config: %s", exc)
        return None
