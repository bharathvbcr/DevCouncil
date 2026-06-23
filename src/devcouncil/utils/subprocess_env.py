"""A sanitized environment for spawning *external* executables.

When DevCouncil runs from a virtualenv (a project ``.venv`` or a ``uv tool
install``), its interpreter exports markers — ``VIRTUAL_ENV``, ``PYTHONHOME``,
``PYTHONPATH``, ``UV_INTERNAL__PYTHONHOME`` — that any child process inherits.
Those markers forcibly re-point a freshly-spawned, *differently-built* Python
(e.g. the globally-installed ``devcouncil`` CLI, or a project's own
interpreter) at DevCouncil's stdlib/site-packages. The classic symptoms are
``AssertionError: SRE module mismatch`` (the child loads its own ``_sre`` C
extension against our ``re`` stdlib) and spurious ``No module named pytest``.

This mirrors :meth:`Verifier._verification_env` but is dependency-free so the
integration probes (``dev integrate check``) and any other call-site that
shells out to an *external* program can share one correct implementation.
"""

from __future__ import annotations

import os
import sys
from pathlib import Path
from typing import Dict


def clean_subprocess_env() -> Dict[str, str]:
    """Return a copy of ``os.environ`` with DevCouncil's own virtualenv stripped.

    No-op (returns a plain copy) when DevCouncil is not running inside a venv,
    so behaviour is unchanged for system / pipx-style installs.
    """
    env = dict(os.environ)
    venv_prefix = Path(sys.prefix).resolve()
    base_prefix = Path(getattr(sys, "base_prefix", sys.prefix)).resolve()
    if venv_prefix == base_prefix:
        return env  # Not inside a venv; nothing to strip.

    venv_dirs = {
        str(venv_prefix).lower(),
        str((venv_prefix / "Scripts").resolve()).lower(),
        str((venv_prefix / "bin").resolve()).lower(),
    }
    path = env.get("PATH", "")
    kept = []
    for entry in path.split(os.pathsep):
        if not entry:
            continue
        try:
            normalized = str(Path(entry).resolve()).lower()
        except Exception:
            normalized = entry.lower()
        if normalized in venv_dirs:
            continue
        kept.append(entry)
    env["PATH"] = os.pathsep.join(kept)

    own_prefixes = {str(venv_prefix), str(base_prefix)}
    for marker in ("VIRTUAL_ENV", "PYTHONHOME"):
        value = env.get(marker)
        if not value:
            continue
        try:
            resolved = str(Path(value).resolve())
        except Exception:
            resolved = value
        if resolved in own_prefixes:
            env.pop(marker, None)
    # uv stashes the interpreter home here and re-applies it to child pythons.
    env.pop("UV_INTERNAL__PYTHONHOME", None)
    return env
