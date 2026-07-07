import asyncio
import logging

import httpx

from devcouncil.artifacts.graph import ArtifactGraph
from devcouncil.reporting.github_check import GitHubCheckGenerator

logger = logging.getLogger(__name__)

_RETRYABLE_STATUS = {429, 500, 502, 503, 504}


class GitHubIntegration:
    """Manages interactions with GitHub API, specifically PR Checks."""
    
    def __init__(self, github_token: str, repository: str, commit_sha: str):
        self.github_token = github_token
        self.repository = repository
        self.commit_sha = commit_sha
        self.base_url = f"https://api.github.com/repos/{repository}"

    async def _post_with_retry(
        self,
        client: httpx.AsyncClient,
        url: str,
        *,
        headers: dict,
        json: dict,
        max_attempts: int = 3,
    ) -> httpx.Response:
        last_exc: Exception | None = None
        for attempt in range(1, max_attempts + 1):
            try:
                response = await client.post(url, headers=headers, json=json)
                if response.status_code in _RETRYABLE_STATUS and attempt < max_attempts:
                    delay = min(30.0, 2.0 ** attempt)
                    logger.warning(
                        "GitHub POST %s returned %s; retrying %d/%d in %.0fs",
                        url,
                        response.status_code,
                        attempt,
                        max_attempts,
                        delay,
                    )
                    await asyncio.sleep(delay)
                    continue
                response.raise_for_status()
                return response
            except httpx.TimeoutException as exc:
                last_exc = exc
                if attempt >= max_attempts:
                    raise
                delay = min(30.0, 2.0 ** attempt)
                logger.warning(
                    "GitHub POST %s timed out; retrying %d/%d in %.0fs",
                    url,
                    attempt,
                    max_attempts,
                    delay,
                )
                await asyncio.sleep(delay)
            except httpx.HTTPStatusError:
                raise
            except httpx.HTTPError as exc:
                last_exc = exc
                if attempt >= max_attempts:
                    raise
                delay = min(30.0, 2.0 ** attempt)
                logger.warning(
                    "GitHub POST %s failed (%s); retrying %d/%d in %.0fs",
                    url,
                    exc,
                    attempt,
                    max_attempts,
                    delay,
                )
                await asyncio.sleep(delay)
        if last_exc is not None:
            raise last_exc
        raise RuntimeError("GitHub POST failed without a response")

    async def report_verification(self, graph: ArtifactGraph):
        """Creates or updates a GitHub Check Run with the current verification status."""
        payload = GitHubCheckGenerator.generate(graph)
        payload["head_sha"] = self.commit_sha
        
        headers = {
            "Authorization": f"Bearer {self.github_token}",
            "Accept": "application/vnd.github.v3+json",
            "Content-Type": "application/json",
        }
        
        async with httpx.AsyncClient() as client:
            try:
                await self._post_with_retry(
                    client,
                    f"{self.base_url}/check-runs",
                    headers=headers,
                    json=payload,
                )
                logger.info("GitHub PR Check updated for %s at %s", self.repository, self.commit_sha)
            except Exception as e:
                logger.error("Failed to report to GitHub: %s", e)
                raise
