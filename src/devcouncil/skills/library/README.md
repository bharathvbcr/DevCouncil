# DevCouncil skills library

Each `*.md` file here (except this README) is a **skill**: reusable guidance that
DevCouncil selects for a goal/repo, embeds into `dev prompt` output, and scaffolds
into a target repo's `.claude/skills/<name>/SKILL.md`.

A skill is a markdown file with YAML frontmatter:

```markdown
---
name: my-domain            # required — unique slug; files without a name are ignored
title: My Domain Intake    # optional human title
description: One-line directive shown in `dev skills` and embedded into prompts.
always: false              # true = always selected (only core-engineering uses this)
triggers:                  # how this skill is auto-selected (omit for always-on)
  keywords: [foo, bar]     # matched against the goal text (case-insensitive substring)
  globs: ["*.foo", "build.bar"]   # matched against repo file basenames
---

# Body

The full guidance the agent reads. For domain skills, write a "senior-dev intake":
what current versions, deprecations, recommended libraries, official guidelines, and
CLI/build tools to confirm *before* writing code.
```

## Adding a skill

1. Create `<name>.md` with the frontmatter above.
2. Choose triggers: `keywords` (goal text) and/or `globs` (repo files). Both are ORed.
3. Verify: `dev skills` lists it; `dev skills show <name>` prints it.

Selection is keyword-based inside `dev prompt` (fast, no repo walk) and
keyword-plus-file-based for `dev skills` / `dev skills scaffold` / init scaffolding.
Keep `description` actionable — it is what the agent sees inline in the task prompt.

## Repo-local skills

You don't have to edit this packaged library to add a skill. Drop your own
`SKILL.md` (same frontmatter) into a project's **`.claude/skills/<name>/SKILL.md`**
(or `.devcouncil/skills/*.md`) and DevCouncil discovers it automatically — it shows
up in `dev skills` (source `repo`), participates in selection, and is folded into the
codebase-aware prompt enhancer and the task prompts. A repo-local skill **overrides a
packaged skill with the same name**, so a team can keep its own house rules for
`android`/`web`/etc. Give it `triggers` (or `always: true`) if you want it
auto-selected; without triggers it stays available but dormant.
