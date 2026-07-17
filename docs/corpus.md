# Corpus side index

Advisory mixed-corpus graph for docs, PDFs, and images. Separate from the
deterministic code graph in `.devcouncil/graph/`, but can feed **opt-in verify
gates** (`corpus_stale`, `doc_code_ref`, `acceptance_corpus`) when rigor enables them.

## CLI

```bash
dev corpus build                 # default roots from indexing.corpus.paths in config.yaml
dev corpus build --path docs     # single directory or file
dev corpus query "authentication"
dev corpus status
```

## Artifacts

| Path | Role |
| :--- | :--- |
| `.devcouncil/corpus/graph.json` | Concept/relationship graph (advisory + verify input) |
| `.devcouncil/corpus/graph.html` | Optional listing (`indexing.corpus.write_html`) |

## Configuration

In `.devcouncil/config.yaml` only (no `graphify.yaml` merge):

```yaml
indexing:
  corpus:
    enabled: true
    paths: [docs, README.md]
    auto_refresh_on_verify: true
    llm_enrichment: false
    vision_captions: false
    write_html: false

verification:
  rigor:
    corpus_stale: soft   # never | soft | hard | always
    doc_code_ref: soft   # never | soft | hard | always
    acceptance_corpus: soft   # never | soft | hard | always
```

If a legacy `.devcouncil/graphify.yaml` is still present, `dev doctor` warns to
move `corpus:` settings into `config.yaml`.

## Verify gates

| Gate | When | Behavior |
| :--- | :--- | :--- |
| `corpus_stale` | Task touches corpus paths or planned docs | Corpus missing/stale vs doc fingerprints → gap (soft by default) |
| `doc_code_ref` | Diff includes docs with `src/...` refs | Broken heuristic code refs in changed docs → gap (soft by default) |
| `acceptance_corpus` | Acceptance criteria cite doc concepts | Require corpus hit or explicit evidence path (soft by default) |

When those gates are enabled and the corpus is stale, verify can auto-refresh
the corpus (`indexing.corpus.auto_refresh_on_verify: true` default).

Corpus gates are **advisory for navigation** and **targeted for verify** — they
do **not** mutate code-graph dead lists.

## Extraction (MVP)

- **Markdown / text / RST:** headings → sections; `[links](url)`, `[[wikilinks]]`, and
  backtick file refs → link/code_ref nodes with `contains` / `links_to` / `references` edges.
- **PDF:** page text via `pypdf` when installed; metadata-only otherwise.
- **Images:** path + size metadata; optional vision captions when enabled and a model is configured.

## Relation to repo map

- **`dev map`** / **`dev graph ingest`** build the deterministic **code** graph
  (`.devcouncil/repo_map.json`, `.devcouncil/graph/code_graph.json`) used by map
  verify gates and agent guides.
- **`dev corpus build`** builds a **documentation** graph for agent navigation and
  optional corpus verify gates. Cross-links from docs to code paths are heuristic
  (`src/...` refs) and do not mutate the code graph or dead-code lists.

Run `dev corpus build` after large doc changes. On doc-heavy tasks, stale corpus
may surface as a soft verify gap until refreshed.
