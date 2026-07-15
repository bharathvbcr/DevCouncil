"""Runtime trace providers independent from debugger control."""

from __future__ import annotations

import json
import shutil
import subprocess
import sys
import uuid
from pathlib import Path
from typing import Any, Sequence

from devcouncil.codeintel.debug.fingerprint import build_fingerprint, executable_hash, source_fingerprint
from devcouncil.codeintel.service import get_codeintel_service


class PythonTraceProvider:
    def __init__(self, root: Path):
        self.root = root.expanduser().resolve()

    def run(self, script: Path, args: Sequence[str] = (), *, timeout: float = 300.0) -> dict[str, Any]:
        script = script if script.is_absolute() else self.root / script
        trace_path = self.root / ".devcouncil" / "codeintel" / "traces" / f"python-{uuid.uuid4().hex}.jsonl"
        source = source_fingerprint(self.root)
        build = build_fingerprint(self.root, sys.executable)
        store = get_codeintel_service(self.root).store
        session_id = store.start_runtime_session(
            provider="python-sys-setprofile",
            source_fingerprint=source,
            build_fingerprint=build,
            executable_hash=executable_hash(sys.executable),
            metadata={"script": str(script), "args": list(args), "trace_path": str(trace_path)},
        )
        try:
            proc = subprocess.run(
                [
                    sys.executable,
                    "-m",
                    "devcouncil.codeintel.debug.python_trace_runner",
                    "--root",
                    str(self.root),
                    "--output",
                    str(trace_path),
                    str(script),
                    *args,
                ],
                cwd=self.root,
                check=False,
                timeout=timeout,
            )
            observations = load_jsonl_observations(trace_path)
            store.add_runtime_observations(session_id, observations)
        finally:
            store.end_runtime_session(session_id)
        return {
            "session_id": session_id,
            "provider": "python-sys-setprofile",
            "exit_code": proc.returncode,
            "observation_count": len(observations),
            "trace_path": str(trace_path),
            "source_fingerprint": source,
            "build_fingerprint": build,
        }


class NodeCpuProfileProvider:
    def __init__(self, root: Path, executable: str | Path | None = None):
        self.root = root.expanduser().resolve()
        resolved = str(executable) if executable else shutil.which("node")
        if not resolved:
            raise FileNotFoundError("Node.js executable was not found")
        self.executable = str(Path(resolved).expanduser().resolve())

    def run(self, script: Path, args: Sequence[str] = (), *, timeout: float = 300.0) -> dict[str, Any]:
        script = script if script.is_absolute() else self.root / script
        trace_dir = self.root / ".devcouncil" / "codeintel" / "traces"
        trace_dir.mkdir(parents=True, exist_ok=True)
        trace_path = trace_dir / f"node-{uuid.uuid4().hex}.cpuprofile"
        source = source_fingerprint(self.root)
        build = build_fingerprint(self.root, self.executable)
        executable_fingerprint = executable_hash(self.executable)
        store = get_codeintel_service(self.root).store
        session_id = store.start_runtime_session(
            provider="node-cpu-profile",
            source_fingerprint=source,
            build_fingerprint=build,
            executable_hash=executable_fingerprint,
            metadata={
                "script": str(script),
                "args": list(args),
                "trace_path": str(trace_path),
                "runtime_version": _runtime_version(self.executable),
            },
        )
        try:
            proc = subprocess.run(
                [
                    self.executable,
                    "--cpu-prof",
                    f"--cpu-prof-dir={trace_dir}",
                    f"--cpu-prof-name={trace_path.name}",
                    str(script),
                    *args,
                ],
                cwd=self.root,
                check=False,
                timeout=timeout,
            )
            observations = load_node_cpu_profile(trace_path)
            store.add_runtime_observations(session_id, observations)
        finally:
            store.end_runtime_session(session_id)
        return {
            "session_id": session_id,
            "provider": "node-cpu-profile",
            "exit_code": proc.returncode,
            "observation_count": len(observations),
            "trace_path": str(trace_path),
            "source_fingerprint": source,
            "build_fingerprint": build,
            "executable_hash": executable_fingerprint,
        }


def load_jsonl_observations(path: Path) -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    for line in path.read_text(encoding="utf-8").splitlines() if path.is_file() else []:
        try:
            value = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(value, dict) and value.get("source") and value.get("target"):
            rows.append(value)
    return rows


def load_node_cpu_profile(path: Path) -> list[dict[str, Any]]:
    """Convert a Chrome/Node CPU profile's parent/child nodes to sampled edges."""

    data = json.loads(path.read_text(encoding="utf-8"))
    nodes = {int(node["id"]): node for node in data.get("nodes") or [] if isinstance(node, dict) and "id" in node}
    samples = [int(value) for value in data.get("samples") or []]
    counts: dict[int, int] = {}
    for sample in samples:
        counts[sample] = counts.get(sample, 0) + 1
    rows: list[dict[str, Any]] = []
    for parent_id, parent in nodes.items():
        parent_name = _profile_frame(parent)
        for child_id in parent.get("children") or []:
            child = nodes.get(int(child_id))
            if child is None:
                continue
            rows.append({
                "source": parent_name,
                "target": _profile_frame(child),
                "kind": "sampled_calls",
                "count": max(1, counts.get(int(child_id), 0)),
                "evidence": {"provider": "node-cpu-profile", "parent_id": parent_id, "child_id": child_id},
            })
    return rows


def import_runtime_trace(root: Path, path: Path, *, provider: str = "jsonl") -> dict[str, Any]:
    root = root.expanduser().resolve()
    path = path.expanduser().resolve()
    observations = load_node_cpu_profile(path) if path.suffix == ".cpuprofile" else load_jsonl_observations(path)
    store = get_codeintel_service(root).store
    source = source_fingerprint(root)
    session_id = store.start_runtime_session(
        provider=provider,
        source_fingerprint=source,
        build_fingerprint=build_fingerprint(root),
        metadata={"imported_path": str(path)},
    )
    store.add_runtime_observations(session_id, observations)
    store.end_runtime_session(session_id)
    return {"session_id": session_id, "provider": provider, "observation_count": len(observations)}


def _profile_frame(node: dict[str, Any]) -> str:
    frame = node.get("callFrame") or {}
    url = frame.get("url") or "<unknown>"
    function = frame.get("functionName") or "<anonymous>"
    line = int(frame.get("lineNumber", -1)) + 1
    return f"{url}:{function}:{line}"


def _runtime_version(executable: str) -> str:
    try:
        completed = subprocess.run(
            [executable, "--version"],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            check=False,
            timeout=2.0,
            text=True,
        )
    except (OSError, subprocess.TimeoutExpired):
        return ""
    return completed.stdout.strip().splitlines()[0][:128] if completed.stdout.strip() else ""
