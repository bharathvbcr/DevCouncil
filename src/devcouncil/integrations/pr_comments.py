from __future__ import annotations

import logging
import httpx
from urllib.parse import quote

from devcouncil.artifacts.graph import ArtifactGraph
from devcouncil.reporting.report_builder import ReportBuilder

logger = logging.getLogger(__name__)


class PullRequestCommentError(RuntimeError):
    pass


def build_pr_comment_body(graph: ArtifactGraph, live_review: dict | None = None) -> str:
    report = ReportBuilder.build_markdown(graph, live_review=live_review)
    return "\n".join([
        "## DevCouncil Verification",
        "",
        report,
    ])


class GitHubPRCommenter:
    def __init__(self, token: str, repository: str, pull_number: int, *, base_url: str = "https://api.github.com"):
        self.token = token
        self.repository = repository
        self.pull_number = pull_number
        self.base_url = base_url.rstrip("/")

    async def post_comment(self, body: str) -> dict:
        url = f"{self.base_url}/repos/{self.repository}/issues/{self.pull_number}/comments"
        headers = {
            "Authorization": f"Bearer {self.token}",
            "Accept": "application/vnd.github+json",
            "Content-Type": "application/json",
        }
        logger.info("Posting GitHub PR comment: repo=%s pr=%s", self.repository, self.pull_number)
        async with httpx.AsyncClient() as client:
            response = await client.post(url, headers=headers, json={"body": body})
        if response.status_code >= 400:
            logger.error("GitHub comment failed: HTTP %s for %s#%s", response.status_code, self.repository, self.pull_number)
            raise PullRequestCommentError(f"GitHub comment failed with HTTP {response.status_code}: {response.text}")
        logger.info("GitHub PR comment posted to %s#%s", self.repository, self.pull_number)
        return response.json() if response.content else {}


class GitLabMRCommenter:
    def __init__(self, token: str, project_id: str, merge_request_iid: int, *, base_url: str = "https://gitlab.com/api/v4"):
        self.token = token
        self.project_id = project_id
        self.merge_request_iid = merge_request_iid
        self.base_url = base_url.rstrip("/")

    async def post_comment(self, body: str) -> dict:
        project = quote(self.project_id, safe="")
        url = f"{self.base_url}/projects/{project}/merge_requests/{self.merge_request_iid}/notes"
        headers = {
            "PRIVATE-TOKEN": self.token,
            "Content-Type": "application/json",
        }
        logger.info("Posting GitLab MR note: project=%s mr=%s", self.project_id, self.merge_request_iid)
        async with httpx.AsyncClient() as client:
            response = await client.post(url, headers=headers, json={"body": body})
        if response.status_code >= 400:
            logger.error("GitLab comment failed: HTTP %s for project=%s mr=%s", response.status_code, self.project_id, self.merge_request_iid)
            raise PullRequestCommentError(f"GitLab comment failed with HTTP {response.status_code}: {response.text}")
        logger.info("GitLab MR note posted to project=%s mr=%s", self.project_id, self.merge_request_iid)
        return response.json() if response.content else {}
