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

    def __init__(self, project_root: Path, repo_map: dict | None = None):
        self.project_root = project_root.resolve()
        repo_map = repo_map or {}
        # file -> the files that import it (reverse import edges from `dev map`).
        # A test that imports the changed file is direct evidence it exercises
        # that file — a far stronger signal than guessing a test from its name.
        self._dependents: dict[str, list[str]] = repo_map.get("dependents", {}) or {}
        self._known_files: set[str] = {
            f["path"] for f in (repo_map.get("files") or []) if f.get("path")
        }
        # file -> subsystem area, and area -> its declared test files
        # (role_files["tests"]), so a change with no direct test importer can fall
        # back to its subsystem's tests instead of the whole-package default.
        self._area_by_file: dict[str, str] = {
            f["path"]: f["area"]
            for f in (repo_map.get("files") or [])
            if f.get("path") and f.get("area")
        }
        self._tests_by_area: dict[str, list[str]] = {}
        for sub in repo_map.get("subsystems") or []:
            area = sub.get("area")
            tests = (sub.get("role_files") or {}).get("tests") or []
            if area and tests:
                self._tests_by_area[area] = list(tests)

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
            # Repo-map-derived tests first: a test that imports the changed file
            # (or, failing that, its subsystem's declared tests) actually
            # exercises it, so it outranks the name-based guesses below.
            for command, confidence, reason in self._map_candidates(normalized):
                if command in seen:
                    continue
                seen.add(command)
                suggestions.append(
                    EvidenceSuggestion(
                        command=command,
                        confidence=confidence,
                        reason=reason,
                        paths=[normalized],
                    )
                )
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

    @staticmethod
    def _looks_like_test(path: str) -> bool:
        base = path.rsplit("/", 1)[-1]
        return base.startswith("test_") or base.endswith("_test.py")

    def _file_exists(self, rel: str) -> bool:
        return rel in self._known_files or (self.project_root / rel).exists()

    def _map_candidates(self, path: str) -> list[tuple[str, Literal["high", "medium", "low"], str]]:
        """Test commands grounded in the repo map, strongest signal first."""
        out: list[tuple[str, Literal["high", "medium", "low"], str]] = []
        # 1. Tests that directly import the changed file (real coverage evidence).
        for importer in self._dependents.get(path, []):
            imp = importer.replace("\\", "/")
            if self._looks_like_test(imp) and self._file_exists(imp):
                out.append(
                    (f"pytest {imp}", "high", "Test imports the changed file (repo map dependents)")
                )
        # 2. Fallback: the subsystem's declared tests for this file's area.
        if not out:
            area = self._area_by_file.get(path)
            for test_file in self._tests_by_area.get(area or "", []):
                tf = test_file.replace("\\", "/")
                if self._file_exists(tf):
                    out.append(
                        (f"pytest {tf}", "medium", f"Subsystem '{area}' test file (repo map role_files)")
                    )
        return out

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
