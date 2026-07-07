"""Git ref names for per-task checkpoints.

Owned by the domain layer so the writer (execution.checkpoints.CheckpointService)
and the reader (verification.verifier.Verifier) share one definition of the ref
layout without importing each other — checkpoints.py imports Verifier, so the
verifier must not import checkpoints back. This module must stay dependency-free
(no imports from execution/ or verification/).
"""

TASK_REF_PREFIX = "refs/devcouncil/tasks"

REF_BEFORE_TEMPLATE = TASK_REF_PREFIX + "/{task_id}/before"


def task_before_ref(task_id: str) -> str:
    """Ref pointing at the snapshot taken before ``task_id`` started."""
    return REF_BEFORE_TEMPLATE.format(task_id=task_id)
