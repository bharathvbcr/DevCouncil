import httpx
import logging
from devcouncil.artifacts.graph import ArtifactGraph
from devcouncil.reporting.github_check import GitHubCheckGenerator

logger = logging.getLogger(__name__)

class GitHubIntegration:
    """Manages interactions with GitHub API, specifically PR Checks."""
    
    def __init__(self, github_token: str, repository: str, commit_sha: str):
        self.github_token = github_token
        self.repository = repository
        self.commit_sha = commit_sha
        self.base_url = f"https://api.github.com/repos/{repository}"

    async def report_verification(self, graph: ArtifactGraph):
        """Creates or updates a GitHub Check Run with the current verification status."""
        payload = GitHubCheckGenerator.generate(graph)
        payload["head_sha"] = self.commit_sha
        
        headers = {
            "Authorization": f"Bearer {self.github_token}",
            "Accept": "application/vnd.github.v3+json",
            "Content-Type": "application/json"
        }
        
        async with httpx.AsyncClient() as client:
            try:
                response = await client.post(
                    f"{self.base_url}/check-runs",
                    headers=headers,
                    json=payload
                )
                response.raise_for_status()
                logger.info(f"GitHub PR Check updated for {self.repository} at {self.commit_sha}")
            except Exception as e:
                logger.error(f"Failed to report to GitHub: {e}")
                raise
