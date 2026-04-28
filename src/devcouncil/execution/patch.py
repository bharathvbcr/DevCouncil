import subprocess
from pathlib import Path
from devcouncil.app.errors import ExecutionError

class PatchEngine:
    """Handles applying unified diff patches to the codebase."""
    
    def __init__(self, project_root: Path):
        self.project_root = project_root

    def apply_patch(self, patch_content: str) -> bool:
        """Applies a git-style patch to the repository."""
        patch_file = self.project_root / ".devcouncil" / "temp.patch"
        patch_file.parent.mkdir(parents=True, exist_ok=True)
        patch_file.write_text(patch_content, encoding="utf-8")
        
        try:
            # Using git apply for robust patch application
            subprocess.check_call(
                ["git", "apply", "--ignore-whitespace", str(patch_file)],
                cwd=self.project_root
            )
            return True
        except subprocess.CalledProcessError as e:
            raise ExecutionError(f"Failed to apply patch: {e}")
        finally:
            if patch_file.exists():
                patch_file.unlink()
