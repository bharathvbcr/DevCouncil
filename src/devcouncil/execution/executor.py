from abc import ABC, abstractmethod
from typing import List
from pydantic import BaseModel
from devcouncil.domain.task import Task
from devcouncil.domain.requirement import Requirement

class ExecutionResult(BaseModel):
    success: bool
    message: str

class Executor(ABC):
    @abstractmethod
    def run_task(self, task: Task, requirements: List[Requirement]) -> ExecutionResult:
        """Execute the task and return the result."""
        pass
