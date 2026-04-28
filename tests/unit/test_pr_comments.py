import pytest

from devcouncil.artifacts.graph import ArtifactGraph
from devcouncil.integrations.pr_comments import (
    GitHubPRCommenter,
    GitLabMRCommenter,
    PullRequestCommentError,
    build_pr_comment_body,
)


@pytest.mark.anyio
async def test_github_pr_commenter_posts_issue_comment(monkeypatch):
    calls = {}

    class FakeClient:
        async def __aenter__(self):
            return self

        async def __aexit__(self, *_args):
            return None

        async def post(self, url, headers, json):
            calls["url"] = url
            calls["headers"] = headers
            calls["json"] = json

            class Response:
                status_code = 201
                content = b"{}"
                text = "{}"

                def json(self):
                    return {}

            return Response()

    monkeypatch.setattr("httpx.AsyncClient", FakeClient)

    await GitHubPRCommenter("token", "owner/repo", 7).post_comment("body")

    assert calls["url"].endswith("/repos/owner/repo/issues/7/comments")
    assert calls["json"] == {"body": "body"}
    assert calls["headers"]["Authorization"] == "Bearer token"


@pytest.mark.anyio
async def test_gitlab_mr_commenter_posts_note(monkeypatch):
    calls = {}

    class FakeClient:
        async def __aenter__(self):
            return self

        async def __aexit__(self, *_args):
            return None

        async def post(self, url, headers, json):
            calls["url"] = url
            calls["headers"] = headers
            calls["json"] = json

            class Response:
                status_code = 201
                content = b"{}"
                text = "{}"

                def json(self):
                    return {}

            return Response()

    monkeypatch.setattr("httpx.AsyncClient", FakeClient)

    await GitLabMRCommenter("token", "123", 4).post_comment("body")

    assert calls["url"].endswith("/projects/123/merge_requests/4/notes")
    assert calls["json"] == {"body": "body"}
    assert calls["headers"]["PRIVATE-TOKEN"] == "token"


@pytest.mark.anyio
async def test_gitlab_mr_commenter_url_encodes_path_project_ids(monkeypatch):
    calls = {}

    class FakeClient:
        async def __aenter__(self):
            return self

        async def __aexit__(self, *_args):
            return None

        async def post(self, url, headers, json):
            calls["url"] = url

            class Response:
                status_code = 201
                content = b"{}"
                text = "{}"

                def json(self):
                    return {}

            return Response()

    monkeypatch.setattr("httpx.AsyncClient", FakeClient)

    await GitLabMRCommenter("token", "group/project", 4).post_comment("body")

    assert "/projects/group%2Fproject/merge_requests/4/notes" in calls["url"]


@pytest.mark.anyio
async def test_github_pr_commenter_raises_on_http_error(monkeypatch):
    class FakeClient:
        async def __aenter__(self):
            return self

        async def __aexit__(self, *_args):
            return None

        async def post(self, url, headers, json):
            class Response:
                status_code = 403
                content = b"denied"
                text = "denied"

            return Response()

    monkeypatch.setattr("httpx.AsyncClient", FakeClient)

    with pytest.raises(PullRequestCommentError, match="HTTP 403"):
        await GitHubPRCommenter("token", "owner/repo", 7).post_comment("body")


def test_pr_comment_body_wraps_report():
    body = build_pr_comment_body(ArtifactGraph())

    assert body.startswith("## DevCouncil Verification")
    assert "# DevCouncil Report" in body
