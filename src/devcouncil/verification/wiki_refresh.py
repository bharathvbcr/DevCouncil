"""Post-verify wiki-freshness trigger for large refactors.

The agent-facing codebase wiki (``dev wiki``) drifts as the code changes. A big
refactor — one that spans several subsystems or touches many files — is exactly the kind
of change that leaves the wiki stale. This module decides, after a verify run, whether a
change was "large" and either FLAGS the pages that a refresh would rewrite (cheap, the
default) or actually triggers ``dev wiki update --no-llm`` as a post-step
(``verification.wiki_refresh.auto_update``).

Deliberately best-effort and non-blocking: it is an orientation aid, never a gate. Any
failure (no map, no wiki, config error) degrades to "not considered" and is logged, not
raised. Flagging is free of model calls; auto-update runs the deterministic skeleton.
"""

from __future__ import annotations

import logging
from dataclasses import dataclass, field
from pathlib import Path
from typing import TYPE_CHECKING, List, Mapping, Optional, Sequence

from devcouncil.indexing.subsystem_map import areas_touched

if TYPE_CHECKING:
    from devcouncil.domain.gap import Gap

logger = logging.getLogger(__name__)


@dataclass
class WikiRefreshOutcome:
    considered: bool = False       # was the change large enough to act on?
    triggered: bool = False        # did we actually run an update?
    subsystems_touched: int = 0
    files_touched: int = 0
    stale_pages: List[str] = field(default_factory=list)
    reason: str = ""

    def to_dict(self) -> dict:
        return {
            "considered": self.considered,
            "triggered": self.triggered,
            "subsystems_touched": self.subsystems_touched,
            "files_touched": self.files_touched,
            "stale_pages": list(self.stale_pages),
            "reason": self.reason,
        }


def _resolve_config(project_root: Path, config):
    if config is not None:
        return config
    try:
        from devcouncil.app.config import load_config

        return load_config(project_root).verification.wiki_refresh
    except Exception as exc:
        logger.debug("wiki refresh: config unavailable: %s", exc)
        return None


def is_large_change(
    *, files_touched: int, subsystems_touched: int, min_files: int, min_subsystems: int
) -> bool:
    """A change is "large" when it spans many subsystems OR touches many files."""
    return subsystems_touched >= min_subsystems or files_touched >= min_files


def _stale_pages(project_root: Path) -> List[str]:
    """Pages a refresh would rewrite, or ``[]`` when the wiki/map is unavailable."""
    try:
        from devcouncil.cli.commands.wiki import wiki_dir_for
        from devcouncil.indexing.repo_mapper import RepoMap
        from devcouncil.knowledge.wiki import wiki_stale_pages

        map_path = project_root / ".devcouncil" / "repo_map.json"
        wiki_dir = wiki_dir_for(project_root)
        if not map_path.is_file() or not (wiki_dir / "index.md").is_file():
            return []
        repo_map = RepoMap.model_validate_json(map_path.read_text(encoding="utf-8"))
        return sorted(wiki_stale_pages(project_root, repo_map, wiki_dir).keys())
    except Exception as exc:
        logger.debug("wiki refresh: stale-page detection failed: %s", exc)
        return []


def _run_update(project_root: Path) -> bool:
    """Run the deterministic wiki refresh (no model calls). True on success."""
    try:
        from devcouncil.knowledge.wiki import refresh_wiki

        refresh_wiki(project_root, llm=False, force=False, remap=False)
        return True
    except Exception as exc:
        logger.warning("wiki refresh: auto-update failed: %s", exc)
        return False


def wiki_refresh_advisory_gap(
    outcome: WikiRefreshOutcome,
    *,
    task_id: str,
    gap_id: str,
) -> "Gap | None":
    """Return a non-blocking advisory gap when stale wiki pages were flagged."""
    if not (outcome.considered and not outcome.triggered and outcome.stale_pages):
        return None
    from devcouncil.domain.gap import Gap

    preview = ", ".join(outcome.stale_pages[:5])
    if len(outcome.stale_pages) > 5:
        preview = f"{preview} (+{len(outcome.stale_pages) - 5} more)"
    return Gap(
        id=gap_id,
        severity="low",
        gap_type="architecture_drift",
        task_id=task_id,
        description=(
            f"Codebase wiki may be stale after this large change: "
            f"{len(outcome.stale_pages)} page(s) need refresh ({preview})."
        ),
        evidence=list(outcome.stale_pages[:10]),
        recommended_fix="Run `dev wiki update` to refresh agent-facing documentation.",
        blocking=False,
    )


def evaluate_wiki_refresh(
    project_root: Path,
    changed_files: Sequence[str],
    *,
    repo_map: Optional[Mapping] = None,
    config=None,
) -> WikiRefreshOutcome:
    """Decide whether the change warrants a wiki refresh and act per config.

    Returns a :class:`WikiRefreshOutcome`. When the change is large: with
    ``auto_update`` it runs ``dev wiki update --no-llm`` and reports ``triggered``;
    otherwise it reports the ``stale_pages`` a manual ``dev wiki update`` would rewrite.
    """
    root = Path(project_root)
    changed = [p for p in changed_files if p and p.strip()]
    cfg = _resolve_config(root, config)
    if cfg is not None and not getattr(cfg, "enabled", True):
        return WikiRefreshOutcome(reason="wiki refresh disabled", files_touched=len(changed))

    min_files = int(getattr(cfg, "min_files", 8)) if cfg is not None else 8
    min_subsystems = int(getattr(cfg, "min_subsystems", 3)) if cfg is not None else 3
    auto_update = bool(getattr(cfg, "auto_update", False)) if cfg is not None else False

    if repo_map is None:
        try:
            from devcouncil.utils.json_persist import read_json

            map_path = root / ".devcouncil" / "repo_map.json"
            repo_map = read_json(map_path) if map_path.is_file() else None
        except Exception:
            repo_map = None

    areas = areas_touched(changed, repo_map) if repo_map else []
    files_touched = len(changed)
    subsystems_touched = len(areas)

    if not is_large_change(
        files_touched=files_touched,
        subsystems_touched=subsystems_touched,
        min_files=min_files,
        min_subsystems=min_subsystems,
    ):
        return WikiRefreshOutcome(
            considered=False,
            files_touched=files_touched,
            subsystems_touched=subsystems_touched,
            reason=(
                f"change below threshold ({files_touched} file(s), "
                f"{subsystems_touched} subsystem(s); need >={min_files} files "
                f"or >={min_subsystems} subsystems)"
            ),
        )

    outcome = WikiRefreshOutcome(
        considered=True,
        files_touched=files_touched,
        subsystems_touched=subsystems_touched,
    )
    if auto_update:
        outcome.triggered = _run_update(root)
        outcome.reason = (
            "large change — ran `dev wiki update --no-llm`"
            if outcome.triggered else "large change — auto-update attempted but failed"
        )
        logger.info(
            "wiki refresh: large change (%d files / %d subsystems) — auto-update %s",
            files_touched, subsystems_touched, "ok" if outcome.triggered else "failed",
        )
    else:
        outcome.stale_pages = _stale_pages(root)
        outcome.reason = (
            f"large change — {len(outcome.stale_pages)} stale wiki page(s); "
            "run `dev wiki update`"
            if outcome.stale_pages
            else "large change — wiki may be stale; run `dev wiki update`"
        )
        logger.info(
            "wiki refresh: large change (%d files / %d subsystems) — %d stale page(s) "
            "flagged (run `dev wiki update`)",
            files_touched, subsystems_touched, len(outcome.stale_pages),
        )
    return outcome
