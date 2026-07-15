"""Activate the packaged, offline Tree-sitter grammar cache."""

from __future__ import annotations

import hashlib
import json
import threading
from importlib.resources import files
from pathlib import Path
from typing import Any

_ACTIVATION_LOCK = threading.Lock()
_ACTIVATION_STATUS: dict[str, Any] | None = None


def asset_root() -> Path:
    return Path(str(files("devcouncil_codeintel_grammars").joinpath("assets")))


def verify() -> dict[str, Any]:
    root = asset_root()
    manifest_path = root / "manifest.json"
    if not manifest_path.is_file():
        return {"ok": False, "error": "grammar manifest missing", "assets": str(root)}
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as exc:
        return {
            "ok": False,
            "error": f"invalid grammar manifest: {exc}",
            "assets": str(root),
        }
    checksums = dict(manifest.get("sha256") or {})
    languages = sorted(set(manifest.get("languages") or []))
    required = sorted(set(manifest.get("required_grammars") or []))
    failures: list[str] = []
    for rel, digest in checksums.items():
        path = (root / rel).resolve()
        if root.resolve() not in path.parents:
            failures.append(rel)
            continue
        if not path.is_file() or hashlib.sha256(path.read_bytes()).hexdigest() != digest:
            failures.append(rel)
    missing = sorted(set(required) - set(languages))
    error = ""
    if not checksums:
        error = "grammar manifest contains no checksummed assets"
    elif not required:
        error = "grammar manifest contains no required grammar list"
    elif missing:
        error = f"grammar wheel is missing required assets: {', '.join(missing)}"
    return {
        "ok": not failures and not error,
        "assets": str(root),
        "pack_version": manifest.get("pack_version"),
        "platform": manifest.get("platform"),
        "languages": languages,
        "required_grammars": required,
        "missing_grammars": missing,
        "failed_checksums": failures,
        "error": error,
    }


def activate() -> dict[str, Any]:
    global _ACTIVATION_STATUS
    with _ACTIVATION_LOCK:
        if _ACTIVATION_STATUS is not None:
            return dict(_ACTIVATION_STATUS)
        status = verify()
        if not status["ok"]:
            _ACTIVATION_STATUS = {**status, "activated": False}
            return dict(_ACTIVATION_STATUS)
        import tree_sitter_language_pack as pack

        pack.configure(pack.PackConfig(cache_dir=str(asset_root() / "cache")))
        available = set(pack.available_languages())
        missing = sorted(set(status["required_grammars"]) - available)
        _ACTIVATION_STATUS = {
            **status,
            "ok": not missing,
            "activated": not missing,
            "missing_grammars": missing,
            "error": (
                f"pack could not activate required grammars: {', '.join(missing)}"
                if missing
                else ""
            ),
        }
        return dict(_ACTIVATION_STATUS)


__all__ = ["activate", "asset_root", "verify"]
