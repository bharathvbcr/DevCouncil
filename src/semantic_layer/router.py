"""Semantic router: complexity scoring and model tier selection."""

from __future__ import annotations

import re
from dataclasses import dataclass

import numpy as np
from numpy.typing import NDArray

from .config import ModelTier, RouterConfig
from .embeddings import EmbeddingService, FloatVector

# Anchor embeddings for "simple" query prototypes (populated at startup)
SIMPLE_ANCHORS = [
    "hello",
    "what time is it",
    "define photosynthesis",
    "translate hello to french",
    "what is 2 plus 2",
]

COMPLEX_PATTERNS = re.compile(
    r"(?i)\b(implement|architect|refactor|debug|optimize|prove|derive|"
    r"multi.?step|recursive|distributed|benchmark|sql|regex|algorithm)\b"
)
CODE_PATTERNS = re.compile(r"```|<code>|def |class |function\s*\(|import ")


@dataclass
class RouteDecision:
    tier: ModelTier
    model_name: str
    complexity_score: float
    features: dict[str, float]
    intent: str


class SemanticRouter:
    def __init__(
        self,
        config: RouterConfig | None = None,
        embedder: EmbeddingService | None = None,
    ) -> None:
        self.config = config or RouterConfig()
        self.embedder = embedder or EmbeddingService.get_instance()
        self._anchor_embeddings: NDArray[np.float32] | None = None

    def _ensure_anchors(self) -> NDArray[np.float32]:
        if self._anchor_embeddings is None:
            self._anchor_embeddings = self.embedder.embed(SIMPLE_ANCHORS)
        return self._anchor_embeddings

    def _length_feature(self, query: str) -> float:
        token_estimate = len(query.split())
        return min(1.0, token_estimate / 512.0)

    def _structure_feature(self, query: str) -> float:
        score = 0.0
        if COMPLEX_PATTERNS.search(query):
            score += 0.6
        if CODE_PATTERNS.search(query):
            score += 0.4
        if query.count("?") > 1 or "\n" in query:
            score += 0.2
        return min(1.0, score)

    def _embed_dispersion_feature(self, embedding: FloatVector) -> float:
        anchors = self._ensure_anchors()
        sims = anchors @ embedding  # (N,) cosine sims
        max_sim = float(np.max(sims))
        return 1.0 - max_sim  # high dispersion = dissimilar to simple anchors

    @staticmethod
    def _domain_feature(has_rag: bool, requires_tools: bool) -> float:
        if requires_tools:
            return 1.0
        if has_rag:
            return 0.6
        return 0.0

    @staticmethod
    def _infer_intent(query: str, complexity: float) -> str:
        if CODE_PATTERNS.search(query):
            return "code"
        if complexity >= 0.6:
            return "reasoning"
        if "?" in query:
            return "qa"
        return "general"

    def route(
        self,
        query: str,
        embedding: FloatVector | None = None,
        has_rag: bool = False,
        requires_tools: bool = False,
    ) -> RouteDecision:
        vec = embedding if embedding is not None else self.embedder.embed_one(query)
        w = self.config.weights

        features = {
            "length": self._length_feature(query),
            "structure": self._structure_feature(query),
            "embed_disp": self._embed_dispersion_feature(vec),
            "domain": self._domain_feature(has_rag, requires_tools),
        }

        complexity = (
            w["length"] * features["length"]
            + w["structure"] * features["structure"]
            + w["embed_disp"] * features["embed_disp"]
            + w["domain"] * features["domain"]
        )

        tier = ModelTier.SMALL if complexity < self.config.complexity_threshold else ModelTier.LARGE
        model_name = (
            self.config.small_model if tier == ModelTier.SMALL else self.config.large_model
        )
        intent = self._infer_intent(query, complexity)

        return RouteDecision(
            tier=tier,
            model_name=model_name,
            complexity_score=complexity,
            features=features,
            intent=intent,
        )
