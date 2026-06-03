import subprocess
import logging
from pathlib import Path

from devcouncil.domain.gap import Gap
from devcouncil.repo.gitignore import build_gitignore_content

logger = logging.getLogger(__name__)


class CleanGitCheck:
    """Ensures the working tree is clean before a task starts."""

    def _is_runtime_state(self, line: str, project_root: Path) -> bool:
        path = line[3:].strip().replace("\\", "/")
        if path.startswith(".devcouncil/"):
            return True
        status = line[:2]
        return path == ".gitignore" and self._is_managed_gitignore_update(project_root, status)

    def _is_managed_gitignore_update(self, project_root: Path, status: str) -> bool:
        gitignore_path = project_root / ".gitignore"
        try:
            current = gitignore_path.read_text(encoding="utf-8")
        except OSError:
            return False

        base = ""
        if status != "??":
            result = subprocess.run(
                ["git", "show", "HEAD:.gitignore"],
                cwd=project_root,
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="replace",
            )
            if result.returncode != 0:
                return False
            base = result.stdout

        return current == build_gitignore_content(base)

    def check(self, project_root, task_id: str) -> list[Gap]:
        root = Path(project_root)
        try:
            status = subprocess.check_output(["git", "status", "--porcelain"], cwd=root).decode()
            dirty_lines = [line for line in status.splitlines() if line.strip() and not self._is_runtime_state(line, root)]
            if dirty_lines:
                return [Gap(
                    id=f"GAP-{task_id}-DIRTY-GIT",
                    severity="high",
                    gap_type="architecture_drift",
                    task_id=task_id,
                    description="Git working tree is dirty. Execution requires a clean state for checkpointing.",
                    recommended_fix="Commit or stash your current changes before running the task.",
                    blocking=True
                )]
        except FileNotFoundError:
            logger.error("Git is not installed or not in PATH.")
            return [Gap(
                id=f"GAP-{task_id}-NO-GIT",
                severity="high",
                gap_type="architecture_drift",
                task_id=task_id,
                description="Git is not available. Cannot verify working tree cleanliness.",
                recommended_fix="Install git and ensure it is in your PATH.",
                blocking=True
            )]
        except subprocess.CalledProcessError as e:
            logger.warning("Git status check failed: %s", e)
            return [Gap(
                id=f"GAP-{task_id}-GIT-ERROR",
                severity="medium",
                gap_type="architecture_drift",
                task_id=task_id,
                description=f"Git status check failed: {e}. Directory may not be a git repository.",
                recommended_fix="Initialize a git repository with 'git init' before running tasks.",
                blocking=True
            )]
        return []
