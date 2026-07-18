"""Add IndexingConfig.repo_map_unwired_cap for AC-2.1."""

from __future__ import annotations

import sys
from pathlib import Path

from devcouncil.execution.gated_write import write_file_payload

TOKEN = sys.argv[1]
path = Path("src/devcouncil/app/config.py")
text = path.read_text(encoding="utf-8")
old = """    repo_map_liveness_cap: int = Field(default=20_000, ge=1, le=100_000)
    repo_map_dependents_cap: int = Field(default=1_024, ge=1, le=4_096)
"""
new = """    repo_map_liveness_cap: int = Field(default=20_000, ge=1, le=100_000)
    repo_map_unwired_cap: int = Field(default=5_000, ge=1, le=100_000)
    repo_map_dependents_cap: int = Field(default=1_024, ge=1, le=4_096)
"""
if "repo_map_unwired_cap" in text:
    print("already present")
    raise SystemExit(0)
if old not in text:
    raise SystemExit("anchor not found")
text = text.replace(old, new, 1)
result = write_file_payload(
    Path("."),
    task_id="TASK-1",
    lease_token=TOKEN,
    rel_path="src/devcouncil/app/config.py",
    content=text,
)
print(result.get("ok"), result.get("applied_files"), result.get("error") or result.get("rejected_files"))
