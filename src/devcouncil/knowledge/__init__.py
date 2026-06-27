"""Knowledge formats: Open Knowledge Format (OKF) and design.md support.

DevCouncil treats durable, file-based knowledge the same way it treats its own
artifacts. This package adds two interoperable, vendor-neutral markdown formats:

* :mod:`devcouncil.knowledge.okf` — the Open Knowledge Format (Google Cloud, v0.1):
  markdown + YAML frontmatter arranged into a cross-linked knowledge graph. Used both
  to *export* DevCouncil's artifact graph as a portable bundle and to *ingest* external
  org knowledge as planning context.
* :mod:`devcouncil.knowledge.design` — the design.md spec (google-labs-code, alpha):
  machine-readable design tokens plus human-readable rationale, with lint/export tooling.

Both ride on :mod:`devcouncil.knowledge.frontmatter` (a single markdown+YAML frontmatter
implementation shared with the skills library) and are surfaced as selectable
:class:`devcouncil.knowledge.sources.KnowledgeSource` objects injected into prompts.
"""

from devcouncil.knowledge.frontmatter import (
    build_frontmatter_markdown,
    split_frontmatter,
)

__all__ = ["build_frontmatter_markdown", "split_frontmatter"]
