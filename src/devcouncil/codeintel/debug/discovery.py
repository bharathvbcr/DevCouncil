"""Side-effect-free discovery of installed Debug Adapter Protocol adapters."""

from __future__ import annotations

import importlib.util
import importlib.metadata
import os
import shutil
import subprocess
import sys
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any

from devcouncil.codeintel.debug.fingerprint import executable_hash


@dataclass(frozen=True)
class AdapterInfo:
    id: str
    name: str
    command: tuple[str, ...]
    path: str
    executable_hash: str
    version: str = ""
    transport: str = "stdio"
    languages: tuple[str, ...] = ()
    requests: tuple[str, ...] = ("launch", "attach")

    def as_dict(self) -> dict[str, Any]:
        data = asdict(self)
        data["command"] = list(self.command)
        data["languages"] = list(self.languages)
        data["requests"] = list(self.requests)
        return data


_EXECUTABLES = (
    ("lldb-dap", "LLDB DAP", ("c", "cpp", "objective-c", "swift", "rust"), ("--version",)),
    ("codelldb", "CodeLLDB", ("c", "cpp", "objective-c", "swift", "rust"), ("--version",)),
    ("dlv", "Delve", ("go",), ("version",)),
    ("netcoredbg", ".NET Core Debugger", ("csharp", "vbnet"), ("--version",)),
    ("js-debug-adapter", "JavaScript Debugger", ("javascript", "typescript"), ("--version",)),
    ("node-debug2", "Node Debug2", ("javascript", "typescript"), ("--version",)),
    ("php-debug-adapter", "PHP Debug Adapter", ("php",), ("--version",)),
)


def _command_version(command: tuple[str, ...]) -> str:
    """Run an adapter's non-mutating version command with a strict bound."""
    try:
        completed = subprocess.run(
            list(command),
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            timeout=2.0,
            check=False,
            text=True,
        )
    except (OSError, subprocess.TimeoutExpired):
        return ""
    lines = [line.strip() for line in completed.stdout.splitlines() if line.strip()]
    return lines[0][:256] if lines else ""


def discover_adapters() -> list[AdapterInfo]:
    adapters: list[AdapterInfo] = []
    if importlib.util.find_spec("debugpy") is not None:
        # Preserve the virtual-environment interpreter path. Resolving its
        # symlink can escape the environment that actually contains debugpy.
        path = str(Path(sys.executable).absolute())
        try:
            version = importlib.metadata.version("debugpy")
        except importlib.metadata.PackageNotFoundError:
            version = ""
        adapters.append(AdapterInfo(
            id="debugpy",
            name="Python debugpy",
            command=(path, "-m", "debugpy.adapter"),
            path=path,
            executable_hash=executable_hash(path),
            version=version,
            languages=("python",),
        ))
    configured_node = os.environ.get("DEVCOUNCIL_NODE_DEBUG2_PATH")
    node = shutil.which("node")
    if configured_node and node:
        adapter_path = str(Path(configured_node).expanduser().resolve())
        if Path(adapter_path).is_file():
            adapters.append(AdapterInfo(
                id="node-debug2",
                name="Node Debug2",
                command=(str(Path(node).resolve()), adapter_path),
                path=adapter_path,
                executable_hash=executable_hash(adapter_path),
                version=os.environ.get("DEVCOUNCIL_NODE_DEBUG2_VERSION", ""),
                languages=("javascript", "typescript"),
            ))
    for executable, name, languages, version_args in _EXECUTABLES:
        resolved = shutil.which(executable)
        if resolved:
            path = str(Path(resolved).resolve())
            adapters.append(AdapterInfo(
                id=executable,
                name=name,
                command=(path,),
                path=path,
                executable_hash=executable_hash(path),
                version=_command_version((path, *version_args)),
                languages=languages,
            ))
    return adapters


def adapter_by_id(adapter_id: str) -> AdapterInfo | None:
    return next((adapter for adapter in discover_adapters() if adapter.id == adapter_id), None)


def adapter_by_command(command: tuple[str, ...] | list[str]) -> AdapterInfo | None:
    if not command:
        return None
    executable = str(Path(command[0]).expanduser().resolve())
    return next(
        (
            adapter
            for adapter in discover_adapters()
            if str(Path(adapter.command[0]).expanduser().resolve()) == executable
            and tuple(command[1:]) == adapter.command[1:]
        ),
        None,
    )
