"""Git-native checkpoint service with legacy patch compatibility."""

from __future__ import annotations

import json
import logging
import subprocess
from pathlib import Path

from pydantic import BaseModel

from devcouncil.verification.verifier import Verifier

logger = logging.getLogger(__name__)


class CheckpointResult(BaseModel):
    task_id: str
    ref: str | None = None
    patch_path: str | None = None
    json_path: str | None = None
    git_ref_created: bool = False
    message: str = ""


class CheckpointService:
    REF_BEFORE = "refs/devcouncil/tasks/{task_id}/before"
    REF_AFTER = "refs/devcouncil/tasks/{task_id}/after"
    REF_ATTEMPT = "refs/devcouncil/tasks/{task_id}/attempts/{attempt}"

    def __init__(self, project_root: Path):
        self.project_root = project_root.resolve()
        self.checkpoint_dir = self.project_root / ".devcouncil" / "checkpoints"
        self.checkpoint_dir.mkdir(parents=True, exist_ok=True)

    def create_before(self, task_id: str) -> CheckpointResult:
        return self._create(task_id, stage="before")

    def create_after(self, task_id: str) -> CheckpointResult:
        return self._create(task_id, stage="after")

    def create_attempt(self, task_id: str, attempt: int) -> CheckpointResult:
        ref_template = self.REF_ATTEMPT.format(task_id=task_id, attempt=attempt)
        return self._create(task_id, stage="attempt", ref_name=ref_template)

    def rollback(self, task_id: str) -> CheckpointResult:
        before_ref = self.REF_BEFORE.format(task_id=task_id)
        after_ref = self.REF_AFTER.format(task_id=task_id)
        after_patch = self.checkpoint_dir / f"{task_id}-after.patch"

        if self._ref_exists(after_ref) and self._ref_exists(before_ref):
            try:
                diff = subprocess.check_output(
                    ["git", "diff", before_ref, after_ref],
                    cwd=self.project_root,
                    text=True,
                    encoding="utf-8",
                    errors="replace",
                )
                if diff.strip():
                    subprocess.run(
                        ["git", "apply", "-R", "--whitespace=nowarn"],
                        cwd=self.project_root,
                        input=diff,
                        text=True,
                        check=True,
                    )
                    return CheckpointResult(
                        task_id=task_id,
                        ref=after_ref,
                        git_ref_created=True,
                        message="Rolled back using git refs.",
                    )
            except subprocess.CalledProcessError as exc:
                return CheckpointResult(
                    task_id=task_id,
                    message=f"Git ref rollback failed: {exc}",
                )

        if after_patch.exists():
            try:
                subprocess.check_call(
                    ["git", "apply", "-R", str(after_patch)],
                    cwd=self.project_root,
                )
                return CheckpointResult(
                    task_id=task_id,
                    patch_path=str(after_patch),
                    message="Rolled back using after patch.",
                )
            except subprocess.CalledProcessError as exc:
                return CheckpointResult(
                    task_id=task_id,
                    patch_path=str(after_patch),
                    message=f"Patch rollback failed: {exc}",
                )

        return CheckpointResult(
            task_id=task_id,
            message="No checkpoint refs or after patch found.",
        )

    def import_legacy_patch(self, task_id: str) -> CheckpointResult:
        before_patch = self.checkpoint_dir / f"{task_id}-before.patch"
        if not before_patch.exists():
            return CheckpointResult(
                task_id=task_id,
                message="No legacy before patch to import.",
            )
        ref = self.REF_BEFORE.format(task_id=task_id)
        created = self._update_ref(ref)
        return CheckpointResult(
            task_id=task_id,
            ref=ref if created else None,
            patch_path=str(before_patch),
            git_ref_created=created,
            message="Imported legacy before patch ref when possible.",
        )

    def _create(
        self,
        task_id: str,
        *,
        stage: str,
        ref_name: str | None = None,
    ) -> CheckpointResult:
        ref = ref_name or (
            self.REF_BEFORE.format(task_id=task_id)
            if stage == "before"
            else self.REF_AFTER.format(task_id=task_id)
        )
        patch_path = self.checkpoint_dir / f"{task_id}-{stage}.patch"
        json_path: str | None = None

        git_ref_created = self._update_ref(ref)
        try:
            diff = Verifier(self.project_root).get_diff()
            if diff:
                patch_path.write_text(diff, encoding="utf-8")
        except Exception as exc:
            # Without a patch (and if the ref also failed) rollback is impossible —
            # never let this fail silently.
            logger.warning("Failed to capture %s checkpoint patch for %s: %s", stage, task_id, exc)

        if stage == "before":
            snapshot = {
                "task_id": task_id,
                "changed_files": Verifier(self.project_root).get_changed_files(),
            }
            snapshot_path = self.checkpoint_dir / f"{task_id}-before.json"
            snapshot_path.write_text(json.dumps(snapshot, indent=2), encoding="utf-8")
            json_path = str(snapshot_path)

        return CheckpointResult(
            task_id=task_id,
            ref=ref if git_ref_created else None,
            patch_path=str(patch_path) if patch_path.exists() else None,
            json_path=json_path,
            git_ref_created=git_ref_created,
            message=f"Checkpoint {stage} created.",
        )

    def _update_ref(self, ref: str) -> bool:
        try:
            head = subprocess.check_output(
                ["git", "rev-parse", "HEAD"],
                cwd=self.project_root,
                text=True,
                encoding="utf-8",
                errors="replace",
            ).strip()
            if not head:
                return False
            subprocess.check_call(
                ["git", "update-ref", ref, head],
                cwd=self.project_root,
            )
            return True
        except (subprocess.CalledProcessError, FileNotFoundError):
            return False

    def _ref_exists(self, ref: str) -> bool:
        try:
            subprocess.check_output(
                ["git", "rev-parse", "--verify", ref],
                cwd=self.project_root,
                stderr=subprocess.DEVNULL,
                text=True,
            )
            return True
        except (subprocess.CalledProcessError, FileNotFoundError):
            return False
