from pathlib import Path

from devcouncil.app.errors import ExecutionError


def resolve_project_path(project_root: Path, path: str) -> Path:
    """Resolve a repository-relative path and reject paths outside the project."""
    root_resolved = project_root.resolve()
    resolved = (root_resolved / path).resolve()
    try:
        resolved.relative_to(root_resolved)
    except ValueError as exc:
        raise ExecutionError(f"Path traversal blocked: {path} resolves outside project root.") from exc
    return resolved
