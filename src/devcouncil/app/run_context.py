from pathlib import Path
from datetime import datetime, timezone
from pydantic import BaseModel, Field
from typing import Optional, List, Dict, Any
import logging

from devcouncil.utils.json_persist import write_json

logger = logging.getLogger(__name__)

class RunContext(BaseModel):
    """Context object encapsulating the current execution run."""
    run_id: str
    project_root: str
    goal: Optional[str] = None
    start_time: datetime = Field(default_factory=lambda: datetime.now(timezone.utc))
    active_tasks: List[str] = Field(default_factory=list)
    metadata: Dict[str, Any] = Field(default_factory=dict)
    
    @property
    def run_dir(self) -> Path:
        return Path(self.project_root) / ".devcouncil" / "runs" / self.run_id

    def initialize(self):
        """Create the necessary run directory and sub-folders."""
        self.run_dir.mkdir(parents=True, exist_ok=True)
        logger.info(f"Initialized run context at {self.run_dir}")

    def save_artifact(self, filename: str, content: str):
        """Save a text artifact associated with this run."""
        path = self.run_dir / filename
        path.write_text(content, encoding="utf-8")
        logger.debug(f"Saved artifact to {path}")

    def save_json_artifact(self, filename: str, data: Any):
        """Save a JSON artifact associated with this run."""
        path = self.run_dir / filename
        write_json(path, data)
        logger.debug(f"Saved JSON artifact to {path}")
