"""Validate AC-2.1 exactly as acceptance check does."""

from __future__ import annotations

from devcouncil.app.config import IndexingConfig

assert hasattr(IndexingConfig, "repo_map_unwired_cap")
field = IndexingConfig.model_fields["repo_map_unwired_cap"]
assert field.ge == 1
assert field.le == 100_000
assert field.default == 5_000
print("AC-2.1 pass")
