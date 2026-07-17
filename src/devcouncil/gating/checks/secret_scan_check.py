import logging
import re
from typing import List
from devcouncil.domain.gap import Gap
from devcouncil.utils.redaction import SECRET_PATTERNS, redact_string

logger = logging.getLogger(__name__)

# Captures the new-file starting line from a unified-diff hunk header (@@ -a,b +c,d @@).
_HUNK_RE = re.compile(r"^@@ -\d+(?:,\d+)? \+(\d+)(?:,\d+)? @@")


class SecretScanner:
    """Scans code diffs for potential secrets (API keys, tokens, etc.)."""

    def scan_diff(self, diff_content: str, task_id: str) -> List[Gap]:
        gaps: List[Gap] = []
        current_file = "unknown_file"
        new_line_no = 0  # line number in the new file, tracked across hunks
        counter = 0      # ensures unique gap ids

        for line in diff_content.splitlines():
            if line.startswith("+++ b/"):
                current_file = line[6:]
                continue
            if line.startswith("+++") or line.startswith("---") or line.startswith("diff "):
                continue
            hunk = _HUNK_RE.match(line)
            if hunk:
                new_line_no = int(hunk.group(1))
                continue
            if line.startswith("-"):
                continue  # removed line — does not advance the new-file counter
            if line.startswith("+"):
                for key_type, pattern in SECRET_PATTERNS.items():
                    if pattern.search(line):
                        counter += 1
                        logger.warning(
                            "Potential sensitive pattern detected in %s:%d (task %s)",
                            current_file, new_line_no, task_id,
                        )
                        gaps.append(Gap(
                            id=f"GAP-{task_id}-SECRET-{key_type.upper()}-{new_line_no}-{counter}",
                            severity="critical",
                            gap_type="security_risk",
                            task_id=task_id,
                            description=f"Potential {key_type} found in {current_file}:{new_line_no}.",
                            evidence=[redact_string(line.strip())],
                            recommended_fix="Remove the secret and use environment variables or a secret manager.",
                            blocking=True,
                            # Populate the routing fields so the security NextAction points
                            # the agent straight at the file:line instead of forcing a re-grep.
                            file=current_file,
                            line=new_line_no,
                        ))
                new_line_no += 1
                continue
            # Context or blank line — advances the new-file counter.
            new_line_no += 1
        return gaps
