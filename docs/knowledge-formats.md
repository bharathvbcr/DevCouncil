# Knowledge formats: Open Knowledge Format (OKF) and design.md

DevCouncil treats durable, file-based knowledge the same way it treats its own artifacts.
Two interoperable, vendor-neutral markdown formats are supported:

- **[Open Knowledge Format (OKF)](https://cloud.google.com/blog/products/data-analytics/how-the-open-knowledge-format-can-improve-data-sharing)** —
  markdown files with YAML frontmatter (`type`, `title`, `description`, `resource`, `tags`,
  `timestamp`) arranged in an `index.md` directory hierarchy and cross-linked with plain
  markdown links to form a portable knowledge graph.
- **[design.md](https://github.com/google-labs-code/design.md)** — machine-readable design
  *tokens* (colors, typography, spacing, rounded, components) plus a human-readable body of
  canonical sections, describing a design system that AI agents can apply.

Both ride on one shared markdown+frontmatter implementation
(`src/devcouncil/knowledge/frontmatter.py`) — the same one the
[skills library](../src/devcouncil/skills/registry.py) uses.

## What DevCouncil does with them

| Direction | What | Command |
|-----------|------|---------|
| **Export** | DevCouncil's Requirement→Task→Evidence→Gap graph → a portable OKF bundle | `dev okf export` |
| **Ingest** | An external OKF bundle → durable planning/coding context | `dev okf ingest <dir>` |
| **Validate** | Check a bundle's OKF invariants | `dev okf validate <dir>` |
| **Browse** | Render an OKF bundle as a self-contained, browsable static HTML site | `dev okf html <dir>` |
| **Context** | Ingested OKF + the project `design.md` are injected into planning/council prompts and the `dev prompt` text pasted into coding CLIs | _(automatic)_ |
| **Skills → OKF** | Each engineering skill is also emitted into the bundle as a typed OKF document | `dev okf export --skills` |
| **OKF → Skills** | Ingested OKF documents typed `Engineering Skill` become selectable skills | `dev okf ingest <dir>` |
| **Lint/Export** | Validate a design system and convert its tokens | `dev design lint` / `dev design export` |
| **Design check** | Fail on hardcoded color/spacing/typography literals that bypass the design.md tokens | `dev design check [files...]` |

## Export your artifact graph as OKF

```bash
dev okf export -o ./okf_bundle
dev okf validate ./okf_bundle
```

The bundle is an `index.md` hierarchy — `requirements/`, `tasks/`, `evidence/`, `gaps/` —
where each document carries OKF frontmatter and the relationships between them are real
markdown links (so the "linked graph" property holds and any OKF-aware tool, including the
upstream OKF HTML visualizer, can consume it). `dev okf validate` confirms every document
is typed and every intra-bundle link resolves.

## Browse a bundle as HTML

```bash
dev okf html ./okf_bundle -o ./okf_site
```

`dev okf html` renders an OKF bundle into a self-contained static HTML site — an `index.html`
that groups documents by `type`, plus one page per document, with the bundle's markdown
cross-links rewritten into working in-site hyperlinks. The output is plain files with no
server or external assets, so you can open `index.html` directly or publish the directory as
is. This mirrors the upstream OKF HTML visualizer, giving the same browsable view of any
bundle that `dev okf export` (or an external producer) emits.

## Ingest knowledge as context

```bash
dev okf ingest ./vendor-okf-bundle --name vendor
```

Ingested documents land under `.devcouncil/knowledge/okf/<name>/`. They are then selected
into prompts like a domain skill — by goal keywords and by each document's `tags`. So an OKF
doc tagged `[billing, revenue]` is pulled into the context for a task about billing, giving
the planner and the coding agent the curated facts rather than letting them re-derive (or
contradict) them.

## Skills as OKF (bidirectional)

DevCouncil's engineering skills and OKF documents describe the same kind of artifact from
two angles — a skill is "guidance that fires on triggers", an OKF document is "a typed,
portable markdown node" — so the two formats bridge cleanly in both directions. The single
source of truth for the mapping is `src/devcouncil/knowledge/skill_bridge.py`, which keeps
export and ingest symmetric.

**Export — skills out as OKF documents:**

```bash
dev okf export --skills -o ./okf_bundle
```

With `--skills`, each engineering skill is also written into the bundle as an OKF document
of `type: Engineering Skill`, under `skills/` with a `skills/index.md`, cross-linked into
the bundle graph alongside the requirement/task/evidence/gap nodes. A skill's keyword
triggers become the document's OKF `tags`, so the exported node carries everything an
OKF-aware tool (or a later ingest) needs to reselect it.

**Ingest — OKF documents back in as selectable skills:**

```bash
dev okf ingest ./vendor-okf-bundle --name vendor
```

Any ingested OKF document typed `Engineering Skill` is loaded as a real Skill and selected
into planning and coding prompts alongside the packaged library — by goal keywords and by
its `tags`, exactly like a domain skill. Ingested OKF skills must live under
`.devcouncil/knowledge/okf/` (where `dev okf ingest` places them); on a name conflict the
packaged library or repo skill wins, so an external bundle can add skills but never silently
shadow a first-party one.

## Design systems with design.md

Place a design system at `.devcouncil/knowledge/design/design.md` (or a repo-root
`DESIGN.md`):

```markdown
---
name: Acme
colors:
  primary: "#1a1a1a"
  surface: "#ffffff"
typography:
  body: { fontFamily: Inter, fontSize: 16px }
rounded: { md: 8px }
components:
  button:
    backgroundColor: colors.primary
    textColor: colors.surface
    rounded: rounded.md
---
# Overview
Acme's design language.
# Colors
Primary is near-black; use it for primary actions.
```

```bash
dev design show                       # token counts + sections
dev design lint                       # broken refs, missing primary, contrast, ordering
dev design export --format css        # CSS custom properties
dev design export --format tailwind   # Tailwind theme.extend config
dev design export --format w3c        # W3C Design Tokens JSON
```

A design system is **always** injected into coding-agent prompts (a UI agent should honor it
on every task), so the agent uses your tokens and components instead of inventing ad-hoc
styles. `dev design lint` mirrors a high-value subset of the upstream `@google/design.md`
rules: broken token references, missing primary color, WCAG text/background contrast,
orphaned tokens, and canonical section ordering.

## Check code against the design system

```bash
dev design check                      # scan the project for token-bypassing literals
dev design check src/ui/Button.tsx    # scan specific files
```

`dev design check` scans source files for hardcoded color, spacing, and typography literals
(hex/`rgb()` colors, raw pixel sizes, inline font families) that bypass the `design.md`
tokens — the values an AI-generated UI should be pulling from the design system instead of
inventing. Each violation is reported with its file, line, and the offending literal, and the
command **exits non-zero** when any are found, so it drops straight into CI or a pre-commit
hook. This is how DevCouncil lets AI-generated UI work *prove* it honored the design system
rather than asserting it did.

## Configuration

The `knowledge` block in `.devcouncil/config.yaml` controls injection (all optional):

```yaml
knowledge:
  enabled: true                       # master switch
  directory: .devcouncil/knowledge    # where design/ and okf/ live
  design_always: true                 # always inject the design system
  okf_max_chars: 3000                 # budget for OKF context per prompt
  design_max_chars: 4000              # budget for the design system per prompt
```

Injection is bounded by these char budgets and fitted into the existing prompt context
budget, so a large knowledge base can never crowd out the file context an agent needs.
