"""Online threshold auto-tuner for cache similarity gate."""

from __future__ import annotations

import threading
import time
from collections import deque
from dataclasses import dataclass

import numpy as np

from .config import CacheConfig


@dataclass
class LookupEvent:
    timestamp: float
    similarity: float
    was_hit: bool
    was_correct: bool | None  # None = unlabeled, True/False from exploration


class ThresholdAutoTuner:
    EPS_FPR = 0.001  # 0.1% false positive rate ceiling
    TAU_MIN = 0.85
    TAU_MAX = 0.98
    TAU_STEP = 0.005

    def __init__(self, cache_config: CacheConfig, window_size: int = 1000) -> None:
        self.cache_config = cache_config
        self._events: deque[LookupEvent] = deque(maxlen=window_size)
        self._lock = threading.Lock()

    def record(
        self,
        similarity: float,
        was_hit: bool,
        was_correct: bool | None = None,
    ) -> None:
        with self._lock:
            self._events.append(
                LookupEvent(time.time(), similarity, was_hit, was_correct)
            )

    def _estimate_fpr(self, tau: float) -> float:
        labeled = [e for e in self._events if e.was_hit and e.was_correct is not None]
        if len(labeled) < 20:
            return 0.0  # insufficient data — assume safe
        would_hit = [e for e in labeled if e.similarity >= tau]
        if not would_hit:
            return 1.0  # tau excludes all labeled hits — treat as unsafe for selection
        false = sum(1 for e in would_hit if not e.was_correct)
        return false / len(would_hit)

    def suggest_threshold(self) -> float:
        with self._lock:
            labeled = [e for e in self._events if e.was_hit and e.was_correct is not None]
            if len(labeled) < 20:
                return self.TAU_MAX
            for tau in np.arange(self.TAU_MAX, self.TAU_MIN - self.TAU_STEP, -self.TAU_STEP):
                labeled_at_tau = [e for e in labeled if e.similarity >= float(tau)]
                if len(labeled_at_tau) < 20:
                    continue
                fpr = self._estimate_fpr(float(tau))
                if fpr <= self.EPS_FPR:
                    return float(tau)
            return self.TAU_MIN

    def apply(self) -> float:
        new_tau = self.suggest_threshold()
        self.cache_config.similarity_threshold = new_tau
        return new_tau
