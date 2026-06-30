import logging
import re
import subprocess
from pathlib import Path
from devcouncil.app.errors import ExecutionError

logger = logging.getLogger(__name__)


class PatchEngine:
    """Handles applying unified diff patches to the codebase."""

    # Progressively more tolerant `git apply` invocations. The first that succeeds
    # wins; we only escalate tolerance (whitespace, 3-way merge) rather than ever
    # falling back to `--reject` (which would leave a half-applied tree + .rej files).
    _APPLY_LADDER: tuple[tuple[str, ...], ...] = (
        (),                                   # strict: exact context match
        ("--ignore-whitespace",),             # tolerate whitespace-only context drift
        ("--3way",),                          # reconstruct via blob ancestry when context moved
        ("--3way", "--ignore-whitespace"),    # both
    )

    def __init__(self, project_root: Path):
        self.project_root = project_root

    def _validate_paths(self, patch_content: str) -> None:
        """Defense-in-depth: reject a diff that touches anything outside the repo root.

        Callers that go through ``TaskRunner.apply_patch`` already gate against the
        task's planned files, but the engine is directly callable, so it validates the
        coarse containment boundary (within ``project_root``) itself."""
        root = self.project_root.resolve()
        for match in re.finditer(r"^(?:\+\+\+|---)\s+(?:[ab]/)?(\S+)", patch_content, re.MULTILINE):
            raw = match.group(1)
            if raw == "/dev/null":  # new/deleted file sentinel
                continue
            try:
                resolved = (root / raw).resolve()
            except (OSError, ValueError) as exc:
                raise ExecutionError(f"Patch references an invalid path: {raw!r} ({exc})")
            if resolved != root and root not in resolved.parents:
                raise ExecutionError(
                    f"Patch attempts to modify a path outside the project root: {raw!r}."
                )

    def apply_patch(self, patch_content: str) -> bool:
        """Applies a git-style patch to the repository.

        Tries an escalating ladder of ``git apply`` tolerances so a patch whose context
        drifted slightly (common when an agent edits the file after producing the diff)
        still applies cleanly instead of hard-failing on the strict first pass. If every
        rung fails, the error names the failing hunks from git's own report rather than a
        bare exit code."""
        self._validate_paths(patch_content)

        patch_file = self.project_root / ".devcouncil" / "temp.patch"
        patch_file.parent.mkdir(parents=True, exist_ok=True)
        patch_file.write_text(patch_content, encoding="utf-8")

        last_stderr = ""
        try:
            for extra_args in self._APPLY_LADDER:
                proc = subprocess.run(
                    ["git", "apply", *extra_args, str(patch_file)],
                    cwd=self.project_root,
                    capture_output=True,
                    text=True,
                    encoding="utf-8",
                    errors="replace",
                )
                if proc.returncode == 0:
                    logger.info("Patch applied (git apply %s)", " ".join(extra_args) or "strict")
                    return True
                last_stderr = (proc.stderr or "").strip()
                logger.debug("git apply %s failed: %s", " ".join(extra_args) or "strict", last_stderr)
            logger.error("Patch failed after all fallbacks: %s", last_stderr or "(no detail)")
            raise ExecutionError(
                "Failed to apply patch even with whitespace/3-way fallbacks. "
                f"git reported: {last_stderr or '(no detail)'}"
            )
        finally:
            if patch_file.exists():
                patch_file.unlink()
