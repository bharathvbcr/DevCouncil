"""Shared transient-failure detection and retry for subprocess executors."""

from __future__ import annotations

import logging
import subprocess
import time
from collections.abc import Callable
from pathlib import Path

logger = logging.getLogger(__name__)

# Output signatures of failures caused by the NETWORK/PROVIDER, not the task.
# Matched case-insensitively against stderr plus the tail of stdout.
TRANSIENT_FAILURE_MARKERS = (
    "connection closed",
    "connection reset",
    "connection refused",
    "connection error",
    "connection aborted",
    "econnreset",
    "econnrefused",
    "etimedout",
    "socket hang up",
    "mid-response",
    "network error",
    "fetch failed",
    "temporarily unavailable",
    "service unavailable",
    "internal server error",
    "bad gateway",
    "gateway timeout",
    "overloaded",
    "rate limit",
    "too many requests",
    "request timed out",
    "timeout awaiting",
    "tls handshake",
    "dns",
)


def transient_failure_reason(result: subprocess.CompletedProcess[str]) -> str | None:
    """Return the matched transient marker when a failed run looks provider-caused."""
    haystack = f"{result.stderr or ''}\n{(result.stdout or '')[-4000:]}".lower()
    for marker in TRANSIENT_FAILURE_MARKERS:
        if marker in haystack:
            return marker
    return None


def transient_error_in_text(text: str) -> str | None:
    """Return a transient marker found in an exception message, else None."""
    haystack = (text or "").lower()
    for marker in TRANSIENT_FAILURE_MARKERS:
        if marker in haystack:
            return marker
    return None


def transient_retry_limit(project_root: Path, *, default: int = 2) -> int:
    """Max transient-failure retries from ``execution.transient_retry_attempts``."""
    try:
        from devcouncil.app.config import load_config

        return max(0, int(load_config(project_root).execution.transient_retry_attempts))
    except Exception:
        return default


def run_subprocess_with_transient_retry(
    project_root: Path,
    *,
    label: str,
    task_id: str,
    run_once: Callable[[], subprocess.CompletedProcess[str]],
) -> subprocess.CompletedProcess[str]:
    """Run a subprocess callable, retrying on known transient failure signatures."""
    result = run_once()
    retry_limit = transient_retry_limit(project_root)
    retries = 0
    while result.returncode != 0 and retries < retry_limit:
        reason = transient_failure_reason(result)
        if reason is None:
            break
        retries += 1
        delay = min(30.0, 5.0 * retries)
        logger.warning(
            "%s failed with a transient error for %s (%s); retrying %d/%d in %.0fs",
            label,
            task_id,
            reason,
            retries,
            retry_limit,
            delay,
        )
        time.sleep(delay)
        result = run_once()
    return result
