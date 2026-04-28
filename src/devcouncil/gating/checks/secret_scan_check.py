from typing import List
from devcouncil.domain.gap import Gap
from devcouncil.utils.redaction import SECRET_PATTERNS, redact_string

class SecretScanner:
    """Scans code diffs for potential secrets (API keys, tokens, etc.)."""
    
    def scan_diff(self, diff_content: str, task_id: str) -> List[Gap]:
        gaps = []
        lines = diff_content.splitlines()
        current_file = "unknown_file"
        
        for i, line in enumerate(lines):
            if line.startswith("+++ b/"):
                current_file = line[6:]
                continue
                
            # Only scan added lines in diff
            if not line.startswith("+") or line.startswith("+++"):
                continue
                
            for key_type, pattern in SECRET_PATTERNS.items():
                if pattern.search(line):
                    gaps.append(Gap(
                        id=f"GAP-{task_id}-SECRET-{key_type.upper()}-{i}",
                        severity="critical",
                        gap_type="security_risk",
                        task_id=task_id,
                        description=f"Potential {key_type} found in {current_file} (diff line {i+1}).",
                        evidence=[redact_string(line.strip())],
                        recommended_fix="Remove the secret and use environment variables or a secret manager.",
                        blocking=True
                    ))
        return gaps
