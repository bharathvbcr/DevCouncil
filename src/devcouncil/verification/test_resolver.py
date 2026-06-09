"""Evidence suggestion from changed files."""

from __future__ import annotations

from pathlib import Path
from typing import Literal

from pydantic import BaseModel

from devcouncil.domain.task import Task


class EvidenceSuggestion(BaseModel):
    command: str
    confidence: Literal["high", "medium", "low"]
    reason: str
    paths: list[str] = []


class TestResolver:
    __test__ = False

    def __init__(self, project_root: Path):
        self.project_root = project_root.resolve()

    def suggest_for_task(
        self,
        task: Task,
        changed_files: list[str] | None = None,
    ) -> list[EvidenceSuggestion]:
        files = changed_files or [pf.path for pf in task.planned_files]
        suggestions: list[EvidenceSuggestion] = []
        seen: set[str] = set()

        for path in files:
            normalized = path.replace("\\", "/")
            for candidate in self._candidates_for_path(normalized):
                if candidate in seen:
                    continue
                seen.add(candidate)
                confidence, reason = self._confidence_for(normalized, candidate)
                suggestions.append(
                    EvidenceSuggestion(
                        command=candidate,
                        confidence=confidence,
                        reason=reason,
                        paths=[normalized],
                    )
                )
        return suggestions

    def _candidates_for_path(self, path: str) -> list[str]:
        candidates: list[str] = []
        if path.startswith("src/") and path.endswith(".py"):
            module = path.removeprefix("src/").removesuffix(".py")
            parts = module.split("/")
            if len(parts) >= 2:
                nested = self.project_root / "tests" / parts[0] / f"test_{parts[-1]}.py"
                if nested.exists():
                    candidates.append(f"pytest {nested.relative_to(self.project_root).as_posix()}")
            flat = self.project_root / "tests" / f"test_{parts[-1]}.py"
            if flat.exists():
                candidates.append(f"pytest {flat.relative_to(self.project_root).as_posix()}")
            if "cli/commands/" in path:
                unit = self.project_root / "tests/unit/test_cli_commands.py"
                if unit.exists():
                    candidates.append(f"pytest {unit.relative_to(self.project_root).as_posix()}")
            if path.endswith("policy_engine.py"):
                unit = self.project_root / "tests/unit/test_task_policy_engine.py"
                if unit.exists():
                    candidates.append(f"pytest {unit.relative_to(self.project_root).as_posix()}")
        if path == "src/auth.py":
            auth_test = self.project_root / "tests/test_auth.py"
            if auth_test.exists():
                candidates.append("pytest tests/test_auth.py")
        if not candidates:
            candidates.append("pytest tests/unit")
        return candidates

    def _confidence_for(self, path: str, command: str) -> tuple[Literal["high", "medium", "low"], str]:
        if "test_auth.py" in command and path.endswith("auth.py"):
            return "high", "Direct auth module mapping"
        if path.endswith("policy_engine.py") and "test_task_policy_engine.py" in command:
            return "high", "Policy engine unit test exists"
        if "cli/commands" in path and "test_cli_commands.py" in command:
            return "high", "CLI command module maps to cli command tests"
        if command.startswith("pytest tests/") and command != "pytest tests/unit":
            return "high", "Nested module test path exists"
        if "test_" in command:
            return "medium", "Related test file match"
        return "low", "Package-level fallback"
