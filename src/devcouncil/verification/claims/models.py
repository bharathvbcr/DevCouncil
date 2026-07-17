"""Core data types for claim verification."""

from __future__ import annotations

from dataclasses import dataclass
from enum import Enum


class Kind(str, Enum):
    TESTS_PASS = "tests_pass"
    BUILD_SUCCEEDS = "build_succeeds"
    LINT_CLEAN = "lint_clean"
    FILE_CREATED = "file_created"
    FILE_UPDATED = "file_updated"
    COMMAND_SUCCEEDED = "command_succeeded"
    GENERIC_DONE = "generic_done"


@dataclass(frozen=True)
class Assertion:
    """A checkable claim extracted from the agent's completion message."""

    kind: Kind
    target: str | None = None
    source_text: str = ""


class Status(str, Enum):
    PASS = "pass"
    FAIL = "fail"
    UNVERIFIABLE = "unverifiable"
    SKIPPED = "skipped"


@dataclass
class CheckResult:
    assertion: Assertion
    status: Status
    detail: str = ""
