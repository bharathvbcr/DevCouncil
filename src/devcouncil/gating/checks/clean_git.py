import subprocess
import logging
from devcouncil.domain.gap import Gap
from devcouncil.utils.proc import GIT_TIMEOUT

logger = logging.getLogger(__name__)

class CleanGitCheck:
    """Ensures the working tree is clean before a task starts."""

    def _is_runtime_state(self, line: str) -> bool:
        path = line[3:].strip().replace("\\", "/")
        # DevCouncil manages the root .gitignore itself (ensure_gitignore runs on
        # init and before every task), so it must not block execution.
        return path.startswith(".devcouncil/") or path == ".gitignore"
    
    def check(self, project_root, task_id: str) -> list[Gap]:
        try:
            status = subprocess.check_output(
                ["git", "status", "--porcelain"], cwd=project_root, timeout=GIT_TIMEOUT
            ).decode()
            dirty_lines = [line for line in status.splitlines() if line.strip() and not self._is_runtime_state(line)]
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
        except (subprocess.CalledProcessError, subprocess.TimeoutExpired) as e:
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
