import json
import logging
import os
import subprocess
from pathlib import Path
from typing import List, Dict

from pydantic import BaseModel, Field

from devcouncil.indexing.lsp import LspInspector

logger = logging.getLogger(__name__)

class RepoMap(BaseModel):
    languages: List[str]
    frameworks: List[str]
    package_managers: List[str]
    test_commands: List[str]
    important_files: List[str]
    candidate_files: List[Dict[str, str]]
    lsp: Dict[str, object] = Field(default_factory=dict)

class RepoMapper:
    def __init__(self, project_root: Path):
        self.project_root = project_root

    def _is_runtime_or_generated_file(self, path: str) -> bool:
        normalized = path.replace("\\", "/")
        parts = set(normalized.split("/"))
        if "__pycache__" in parts or normalized.endswith(".pyc"):
            return True
        if parts.intersection({".git", ".devcouncil", ".pytest_cache", ".ruff_cache", ".mypy_cache", ".venv"}):
            return True
        if normalized.startswith("dist/") or normalized.startswith("build/"):
            return True
        return False

    def get_git_files(self) -> List[str]:
        try:
            output = subprocess.check_output(
                ["git", "ls-files", "--cached", "--others", "--exclude-standard"],
                cwd=self.project_root,
                stderr=subprocess.DEVNULL
            ).decode().splitlines()
            return [path for path in output if not self._is_runtime_or_generated_file(path)]
        except Exception:
            # Fallback to os.walk if not a git repo or git missing
            files = []
            for root, _, filenames in os.walk(self.project_root):
                for f in filenames:
                    rel_path = os.path.relpath(os.path.join(root, f), self.project_root)
                    if not rel_path.startswith(".") and not self._is_runtime_or_generated_file(rel_path):
                        files.append(rel_path)
            return files

    def detect_languages(self, files: List[str]) -> List[str]:
        exts = {os.path.splitext(f)[1] for f in files}
        lang_map = {
            ".py": "python",
            ".ts": "typescript",
            ".tsx": "typescript",
            ".js": "javascript",
            ".jsx": "javascript",
            ".go": "go",
            ".rs": "rust",
            ".java": "java",
            ".c": "c",
            ".cpp": "cpp",
        }
        return sorted(list({lang_map[ext] for ext in exts if ext in lang_map}))

    def detect_frameworks(self, files: List[str]) -> List[str]:
        frameworks = []
        file_set = set(files)
        if "package.json" in file_set:
            content = (self.project_root / "package.json").read_text()
            if "next" in content:
                frameworks.append("nextjs")
            if "react" in content:
                frameworks.append("react")
            if "vue" in content:
                frameworks.append("vue")
            if "express" in content:
                frameworks.append("express")
        
        if "requirements.txt" in file_set or "pyproject.toml" in file_set:
            try:
                content = ""
                if "requirements.txt" in file_set:
                    content += (self.project_root / "requirements.txt").read_text()
                if "pyproject.toml" in file_set:
                    content += (self.project_root / "pyproject.toml").read_text()
                
                if "fastapi" in content.lower():
                    frameworks.append("fastapi")
                if "flask" in content.lower():
                    frameworks.append("flask")
                if "django" in content.lower():
                    frameworks.append("django")
            except Exception as e:
                logger.debug("Failed to read Python config files: %s", e)
        return frameworks

    def detect_package_managers(self, files: List[str]) -> List[str]:
        managers = []
        file_set = set(files)
        if "package-lock.json" in file_set:
            managers.append("npm")
        elif "package.json" in file_set:
            managers.append("npm")
        if "yarn.lock" in file_set:
            managers.append("yarn")
        if "pnpm-lock.yaml" in file_set:
            managers.append("pnpm")
        if "requirements.txt" in file_set:
            managers.append("pip")
        if "uv.lock" in file_set:
            managers.append("uv")
        if "go.sum" in file_set:
            managers.append("go mod")
        return managers

    def detect_test_commands(self, files: List[str]) -> List[str]:
        """Detect test, lint, and typecheck commands from project config."""
        commands: List[str] = []
        file_set = set(files)

        # Node.js projects: read scripts from package.json
        if "package.json" in file_set:
            try:
                pkg = json.loads((self.project_root / "package.json").read_text())
                scripts = pkg.get("scripts", {})
                pm = "pnpm" if "pnpm-lock.yaml" in file_set else (
                    "yarn" if "yarn.lock" in file_set else "npm"
                )
                for key in ["test", "lint", "typecheck", "check", "type-check"]:
                    if key in scripts:
                        if pm == "npm" and key != "test":
                            commands.append(f"npm run {key}")
                        else:
                            commands.append(f"{pm} {key}")
            except Exception as e:
                logger.debug("Failed to parse package.json scripts: %s", e)

        # Python projects
        if "pyproject.toml" in file_set or "setup.py" in file_set:
            if any(f.startswith("tests/") or f.startswith("test_") for f in files):
                commands.append("pytest")
            commands.append("ruff check .")
            commands.append("mypy .")

        # Go projects
        if "go.mod" in file_set:
            commands.append("go test ./...")
            commands.append("go vet ./...")

        # Rust projects
        if "Cargo.toml" in file_set:
            commands.append("cargo test")
            commands.append("cargo clippy")

        return commands

    def _ripgrep_search(self, goal: str, files: List[str]) -> List[Dict[str, str]]:
        """Use ripgrep for goal-keyword search if available, else fall back to naive matching."""
        candidates: List[Dict[str, str]] = []
        try:
            # Try ripgrep first for better matching
            result = subprocess.run(
                ["rg", "--files-with-matches", "--ignore-case", "--glob", "!.git", goal],
                capture_output=True, text=True, cwd=self.project_root, timeout=10,
            )
            if result.returncode == 0:
                for line in result.stdout.strip().splitlines()[:10]:
                    candidates.append({"path": line.strip(), "reason": f"ripgrep match for '{goal}'"})
                return candidates
        except Exception:
            pass  # Fall back to naive matching

        # Naive keyword matching fallback
        goal_words = set(goal.lower().split())
        for f in files:
            f_lower = f.lower()
            score = sum(1 for word in goal_words if word in f_lower)
            if score > 0:
                candidates.append({"path": f, "reason": f"Matches goal keywords (score: {score})"})
        candidates = sorted(candidates, key=lambda x: x.get("reason", ""), reverse=True)[:10]
        return candidates

    def map_repo(self, goal: str = "") -> RepoMap:
        files = self.get_git_files()

        candidates: List[Dict[str, str]] = []
        if goal:
            candidates = self._ripgrep_search(goal, files)

        return RepoMap(
            languages=self.detect_languages(files),
            frameworks=self.detect_frameworks(files),
            package_managers=self.detect_package_managers(files),
            test_commands=self.detect_test_commands(files),
            important_files=[f for f in files if f in [
                "package.json", "pyproject.toml", "README.md", "go.mod",
                "Cargo.toml", "Makefile", "Dockerfile", ".github/workflows",
            ]],
            candidate_files=candidates,
            lsp=LspInspector(self.project_root).summary(files),
        )
