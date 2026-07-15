"""Explicit one-time debugger consent stored in project configuration."""

from __future__ import annotations

from pathlib import Path

import yaml


def debug_consent_enabled(root: Path) -> bool:
    try:
        from devcouncil.app.config import load_config

        return bool(load_config(root).code_intelligence.debug.auto_discover)
    except (FileNotFoundError, ValueError):
        return False


def set_debug_consent(root: Path, enabled: bool = True) -> Path:
    path = root.expanduser().resolve() / ".devcouncil" / "config.yaml"
    if not path.is_file():
        raise FileNotFoundError(f"Config not found at {path}. Run 'dev init' first.")
    raw = yaml.safe_load(path.read_text(encoding="utf-8")) or {}
    codeintel = raw.setdefault("code_intelligence", {})
    debug = codeintel.setdefault("debug", {})
    debug["auto_discover"] = bool(enabled)
    path.write_text(yaml.safe_dump(raw, sort_keys=False), encoding="utf-8")
    return path


def require_debug_consent(root: Path) -> None:
    if not debug_consent_enabled(root):
        raise PermissionError(
            "Debugger discovery/execution is disabled. Run `dev debug discover --consent` "
            "or set code_intelligence.debug.auto_discover: true."
        )
