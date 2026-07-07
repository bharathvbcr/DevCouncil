import asyncio

import httpx
import pytest

from devcouncil.integrations.github import GitHubIntegration


class _FakeResponse:
    def __init__(self, status_code: int):
        self.status_code = status_code

    def raise_for_status(self):
        if self.status_code >= 400:
            request = httpx.Request("POST", "https://api.github.com/repos/o/r/check-runs")
            response = httpx.Response(self.status_code, request=request)
            raise httpx.HTTPStatusError("error", request=request, response=response)


class _FakeClient:
    def __init__(self, statuses: list[int | Exception]):
        self._statuses = list(statuses)
        self.calls = 0

    async def post(self, url, headers=None, json=None):
        self.calls += 1
        item = self._statuses.pop(0)
        if isinstance(item, Exception):
            raise item
        return _FakeResponse(item)


def test_github_post_retries_on_429_then_succeeds():
    client = _FakeClient([429, 200])
    integration = GitHubIntegration("token", "owner/repo", "abc123")

    response = asyncio.run(
        integration._post_with_retry(
            client,
            "https://api.github.com/repos/owner/repo/check-runs",
            headers={},
            json={"name": "devcouncil"},
        )
    )

    assert response.status_code == 200
    assert client.calls == 2


def test_github_post_retries_on_timeout():
    client = _FakeClient([httpx.TimeoutException("timed out"), 200])
    integration = GitHubIntegration("token", "owner/repo", "abc123")

    response = asyncio.run(
        integration._post_with_retry(
            client,
            "https://api.github.com/repos/owner/repo/check-runs",
            headers={},
            json={"name": "devcouncil"},
        )
    )

    assert response.status_code == 200
    assert client.calls == 2


def test_github_post_does_not_retry_on_400():
    client = _FakeClient([400])
    integration = GitHubIntegration("token", "owner/repo", "abc123")

    with pytest.raises(httpx.HTTPStatusError):
        asyncio.run(
            integration._post_with_retry(
                client,
                "https://api.github.com/repos/owner/repo/check-runs",
                headers={},
                json={"name": "devcouncil"},
            )
        )

    assert client.calls == 1
