import json
import logging
import shutil
import subprocess
from pathlib import Path
from typing import Any, Iterable

from pydantic import BaseModel, Field

from devcouncil.app.config import load_config

logger = logging.getLogger(__name__)


class CodeReviewGraphContext(BaseModel):
    available: bool
    summary: str
    command: str = "code-review-graph"
    changed_files: list[str] = Field(default_factory=list)
    impacted_files: list[str] = Field(default_factory=list)
    related_tests: list[str] = Field(default_factory=list)
    raw: str = ""


class CodeReviewGraphAdapter:
    """Optional shell adapter for code-review-graph without a hard dependency."""

    def __init__(self, project_root: Path, command: str | None = None):
        self.project_root = project_root
        self._config: Any = None
        self.command = command or self._configured_command()

    def _get_config(self) -> Any:
        if self._config is None:
            self._config = load_config(self.project_root)
        return self._config

    def is_enabled(self) -> bool:
        try:
            return self._get_config().integrations.code_review_graph.enabled
        except Exception:
            return False

    def is_available(self) -> bool:
        return shutil.which(self.command) is not None

    def get_context(self, files: Iterable[str] = ()) -> CodeReviewGraphContext:
        changed_files = [file for file in files if file]
        if not self.is_enabled():
            return CodeReviewGraphContext(
                available=False,
                summary="code-review-graph integration is disabled.",
                command=self.command,
                changed_files=changed_files,
            )
        if not self.is_available():
            return CodeReviewGraphContext(
                available=False,
                summary="code-review-graph command was not found on PATH.",
                command=self.command,
                changed_files=changed_files,
            )

        commands = self._candidate_commands(changed_files)
        errors: list[str] = []
        for command in commands:
            result = self._run(command)
            if result.returncode == 0 and result.output.strip():
                logger.debug("code-review-graph context obtained via: %s", " ".join(command))
                return self._parse_context(result.output, changed_files)
            errors.append(result.output.strip() or f"exit {result.returncode}")

        logger.warning("code-review-graph ran but returned no context (%d command(s) tried)", len(commands))
        return CodeReviewGraphContext(
            available=True,
            summary="code-review-graph ran but did not return context.",
            command=self.command,
            changed_files=changed_files,
            raw="\n".join(errors),
        )

    def prompt_section(self, files: Iterable[str] = ()) -> str:
        context = self.get_context(files)
        if not context.available:
            return ""

        lines = ["## Structural graph context", context.summary]
        if context.impacted_files:
            lines.append("\nImpacted files:")
            lines.extend(f"- `{path}`" for path in context.impacted_files[:20])
        if context.related_tests:
            lines.append("\nRelated tests:")
            lines.extend(f"- `{path}`" for path in context.related_tests[:20])
        return "\n".join(lines).strip() + "\n"

    def _configured_command(self) -> str:
        try:
            return self._get_config().integrations.code_review_graph.command
        except Exception:
            return "code-review-graph"

    def _candidate_commands(self, files: list[str]) -> list[list[str]]:
        commands = [[self.command, "detect-changes", "--json"]]
        if files:
            commands.append([self.command, "get-review-context", "--json", *files])
        commands.append([self.command, "status", "--json"])
        return commands

    def _parse_context(self, output: str, changed_files: list[str]) -> CodeReviewGraphContext:
        data: dict[str, Any] = {}
        try:
            parsed = json.loads(output)
            if isinstance(parsed, dict):
                data = parsed
        except json.JSONDecodeError:
            pass

        impacted = self._string_list(
            data.get("impacted_files")
            or data.get("impact_radius")
            or data.get("files")
            or []
        )
        tests = self._string_list(data.get("related_tests") or data.get("tests") or [])
        summary = str(data.get("summary") or "code-review-graph context available.")
        return CodeReviewGraphContext(
            available=True,
            summary=summary,
            command=self.command,
            changed_files=changed_files,
            impacted_files=impacted,
            related_tests=tests,
            raw=output,
        )

    def _string_list(self, value: Any) -> list[str]:
        if isinstance(value, list):
            results: list[str] = []
            for item in value:
                if isinstance(item, (str, int)):
                    results.append(str(item))
                elif isinstance(item, dict):
                    path = item.get("path") or item.get("file") or item.get("name") or item.get("id")
                    if path:
                        results.append(str(path))
            return results
        if isinstance(value, dict):
            if "path" in value or "file" in value:
                return [str(value.get("path") or value.get("file"))]
            return [str(key) for key in value.keys()]
        return []

    def _run(self, command: list[str]) -> "_RunResult":
        try:
            result = subprocess.run(
                command,
                cwd=self.project_root,
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="replace",
                timeout=20,
            )
            return _RunResult(
                returncode=result.returncode,
                output=(result.stdout or "") + (result.stderr or ""),
            )
        except Exception as exc:
            return _RunResult(returncode=1, output=str(exc))


class _RunResult(BaseModel):
    returncode: int
    output: str
