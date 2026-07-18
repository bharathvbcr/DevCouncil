import sys
from pathlib import Path

from devcouncil.execution.gated_write import write_file_payload

token = sys.argv[1]
rel = sys.argv[2]
c = Path(rel).read_text()
r = write_file_payload(
    Path("."),
    task_id="TASK-1",
    lease_token=token,
    rel_path=rel,
    content=c,
)
print(r.get("ok"), r.get("applied_files"), r.get("error") or r.get("rejected_files"))
