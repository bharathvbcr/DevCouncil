import subprocess
import logging
from devcouncil.domain.gap import Gap

logger = logging.getLogger(__name__)

class CleanGitCheck:
    """Ensures the working tree is clean before a task starts."""
    
    def check(self, project_root, task_id: str) -> list[Gap]:
        try:
            status = subprocess.check_output(["git", "status", "--porcelain"], cwd=project_root).decode()
            if status.strip():
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
