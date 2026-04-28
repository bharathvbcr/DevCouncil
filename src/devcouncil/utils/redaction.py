"""Proactive secret and PII redaction for LLM-bound prompts.

Two entry points:
  - redact_string(text)  -- regex-based redaction of known secret patterns
  - redact_text(text, extra_patterns)  -- enhanced version with custom patterns and typed labels
  - redact_env_vars(text)  -- redact values in environment variable assignments
  - redact_dict(data)  -- recursively redact all string values in a dict
"""

import re
from typing import Dict, List, Optional, Pattern

# Common patterns for sensitive data — each with a human-readable label
SECRET_PATTERNS: Dict[str, Pattern] = {
    "aws_access_key": re.compile(r"(?i)\b(AKIA[0-9A-Z]{16})\b"),
    "aws_secret_key": re.compile(r"(?i)(?:aws_secret_access_key|aws_secret|secret_key)\s*[=:]\s*([0-9a-zA-Z/+]{40})"),
    "github_token": re.compile(r"(?i)\b(gh[pusr]_[A-Za-z0-9_]{36})\b"),
    "slack_token": re.compile(r"(?i)\b(xox[baprs]-[0-9]{12}-[0-9]{12}-[0-9]{12}-[a-z0-9]{32})\b"),
    "jwt": re.compile(r"(?i)\b(ey[a-zA-Z0-9_-]{10,}\.ey[a-zA-Z0-9_-]{10,}\.[a-zA-Z0-9_-]{10,})\b"),
    "generic_api_key": re.compile(r"(?i)(api[_-]?key|secret|token|password)[\"'\s]*[:=][\"'\s]*([a-zA-Z0-9_\-\.]{16,})"),
    "private_key": re.compile(r"(?s)-----BEGIN [A-Z]+ PRIVATE KEY-----.*?-----END [A-Z]+ PRIVATE KEY-----"),
    "bearer": re.compile(r"(?i)\b(Bearer\s+)([a-zA-Z0-9_\-\.]{16,})\b"),
    "database_url": re.compile(r"(?i)((?:postgresql|mysql|mongodb|redis)://[^:]+:)([^@]+)(@.+)"),
}

# For backward compatibility
def redact_string(text: str) -> str:
    """Redact known sensitive patterns from a string (legacy API)."""
    return redact_text(text)


def redact_text(text: str, extra_patterns: Optional[List[str]] = None) -> str:
    """Redact known sensitive patterns from a string.
    
    Args:
        text: The input text to redact.
        extra_patterns: Optional list of regex patterns to additionally redact,
                       labeled as [REDACTED:custom_N].
    
    Returns:
        Text with all sensitive patterns replaced with [REDACTED:type] labels.
    """
    if not isinstance(text, str):
        return text
        
    redacted_text = text
    
    for key_type, pattern in SECRET_PATTERNS.items():
        if key_type == "generic_api_key":
            # For the generic pattern, replace the value part (group 2)
            def _make_generic_replacer(kt: str):
                def replacer(match):
                    prefix = match.group(1)
                    separator = match.group(0)[len(match.group(1)):-len(match.group(2))]
                    return f"{prefix}{separator}[REDACTED:{kt}]"
                return replacer
            redacted_text = pattern.sub(_make_generic_replacer(key_type), redacted_text)
        elif key_type == "bearer":
            # Preserve "Bearer " prefix, redact the token
            def _bearer_replacer(match):
                return f"{match.group(1)}[REDACTED:bearer]"
            redacted_text = pattern.sub(_bearer_replacer, redacted_text)
        elif key_type == "database_url":
            # Preserve protocol and host, redact password
            def _db_url_replacer(match):
                return f"{match.group(1)}[REDACTED:database_url]{match.group(3)}"
            redacted_text = pattern.sub(_db_url_replacer, redacted_text)
        else:
            redacted_text = pattern.sub(f"[REDACTED:{key_type}]", redacted_text)
    
    # Apply custom extra patterns
    if extra_patterns:
        for i, pat_str in enumerate(extra_patterns):
            try:
                pat = re.compile(pat_str)
                redacted_text = pat.sub(f"[REDACTED:custom_{i}]", redacted_text)
            except re.error:
                pass  # Skip invalid patterns
            
    return redacted_text


def redact_env_vars(text: str) -> str:
    """Redact values in environment variable assignments.
    
    Handles patterns like:
      export KEY=value
      KEY='value'
      KEY="value"
    """
    if not isinstance(text, str):
        return text

    # Match: optional export, VAR_NAME = value (with optional quotes)
    env_pattern = re.compile(
        r"(?m)^(\s*(?:export\s+)?)"               # optional export
        r"([A-Z_][A-Z0-9_]*)"                      # variable name
        r"(\s*=\s*)"                                # equals sign
        r"(?:'([^']*)'|\"([^\"]*)\"|(\S+))"         # value (quoted or unquoted)
    )

    sensitive_keys = {
        "api_key", "secret", "token", "password", "passwd", "credential",
        "private_key", "access_key", "secret_key", "auth",
    }

    def _env_replacer(match):
        prefix = match.group(1)
        var_name = match.group(2)
        eq_sign = match.group(3)
        
        # Check if the variable name contains a sensitive keyword
        var_lower = var_name.lower()
        is_sensitive = any(kw in var_lower for kw in sensitive_keys)
        
        if is_sensitive:
            return f"{prefix}{var_name}{eq_sign}[REDACTED]"
        
        return match.group(0)

    return env_pattern.sub(_env_replacer, text)


def redact_dict(data: dict) -> dict:
    """Recursively redact sensitive patterns from a dictionary (e.g. JSON response)."""
    result = {}
    for k, v in data.items():
        if isinstance(v, str):
            result[k] = redact_text(v)
        elif isinstance(v, dict):
            result[k] = redact_dict(v)
        elif isinstance(v, list):
            result[k] = [
                redact_dict(item) if isinstance(item, dict)
                else redact_text(item) if isinstance(item, str)
                else item
                for item in v
            ]
        else:
            result[k] = v
    return result
