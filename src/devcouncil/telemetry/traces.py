import json
import logging
from pathlib import Path
from typing import Dict, Any, Optional

logger = logging.getLogger(__name__)

class TraceLogger:
    """Manages appending execution traces to a local log file."""
    
    def __init__(self, project_root: Path):
        self.log_dir = project_root / ".devcouncil" / "logs"
        self.trace_file = self.log_dir / "traces.jsonl"
        self.log_dir.mkdir(parents=True, exist_ok=True)

    def log_event(self, event_type: str, details: Dict[str, Any], run_id: Optional[str] = None):
        """Append an orchestration event trace."""
        trace = {
            "type": event_type,
            "details": details,
            "run_id": run_id,
        }
        try:
            with open(self.trace_file, "a", encoding="utf-8") as f:
                f.write(json.dumps(trace) + "\n")
        except Exception as e:
            logger.debug("Failed to write trace event %s: %s", event_type, e)
