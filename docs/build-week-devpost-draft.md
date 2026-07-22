# OpenAI Build Week 2026 — Devpost draft (DevCouncil)

**Status:** Draft for human paste into Devpost. Do **not** invent a Codex `/feedback` session ID — copy it from your Codex session after the video and package path are green.

**Track:** Developer Tools

**Repository:** https://github.com/bharathvbcr/DevCouncil

---

## 1. Suggested project title + tagline

**Title:** DevCouncil — Evidence-Gated Agent Orchestration

**Tagline:** Coding agents claim success; DevCouncil requires scoped diffs, verification, and a requirement trail before green.

**Alternates (if character limits bite):**

| Title | Tagline |
|---|---|
| DevCouncil | Model confidence is not the final authority — evidence is. |
| DevCouncil: Gated AI Orchestrator | Red-to-green verification + interactive code graph for Codex and coding CLIs. |

---

## 2. Short project description (candid eligibility)

DevCouncil is a **pre-existing** gated AI orchestration product: it turns agent claims into scoped tasks, stop gates, claim checks, and machine-readable evidence. This Build Week submission is **not** a from-scratch build.

**Eligible window:** only meaningful extensions on or after **July 13, 2026**.

| Marker | Commit | Role |
|---|---|---|
| Baseline (before eligible work) | `3cfd5d1` | Pre–Build Week baseline |
| First eligible commit | `6f5bd73` | Start of eligible history |

**Eligible themes (high level):**

- Canonical SQLite code-intelligence index, multi-language grammars, incremental watching, graph queries/community detection, and a self-contained interactive code-graph HTML artifact (including a ForceGraph compatibility fix so the packed demo is not a blank canvas).
- Stronger deterministic verification: stop gates, claim checking, diff-to-evidence coverage, task leases, PDG/corpus checks, bounded repair, machine-readable next actions.
- Deeper CLI / MCP / dashboard / coding-agent / CI integration, plus the installable npm judge path aimed at **`devcouncil@0.4.2`**.
- Build Week–focused reliability: lossless MCP JSON for large structured CLI output, task-scoped `get_diff` that includes untracked files and fails closed, and a provider-free `scripts/build-week-demo.sh` red→green sample.

**Codex / GPT-5.6 role (honest):** used as an engineering partner to map paths, challenge claims, run install/browser checks, implement focused repairs, and verify behavior. The maintainer set requirements, scope, and acceptance evidence. This does **not** claim that all eligible code was authored exclusively by Codex/GPT-5.6.

**Paste-ready short blurb (~100–150 words):**

> Coding agents often claim success without proving requirements. DevCouncil is a gated engineering workflow: every change is scoped, verified, and traceable back to a requirement. Model confidence is not the final authority; evidence is.
>
> DevCouncil existed before OpenAI Build Week. This submission covers only post–July 13, 2026 extensions (baseline `3cfd5d1`, eligible from `6f5bd73`): interactive code-graph packaging fixes, stronger deterministic verification, MCP reliability for real project-sized JSON and diffs, and a provider-free red→green demo judges can run without API keys via `devcouncil@0.4.2` and `bash scripts/build-week-demo.sh`.

---

## 3. Judge test instructions (published path)

**Prerequisites:** macOS, Linux, or Windows · Node.js 18+ · Python 3.12+ · Git. **No model provider API key** for the deterministic demos below.

```bash
# Prefer the Build Week release:
npm install -g devcouncil@0.4.2
devcouncil --help

# Core demo: red verdict → apply fix → green verdict (no API keys)
devcouncil-build-week-demo
# Equivalent: bash "$(npm root -g)/devcouncil/scripts/build-week-demo.sh"

# Interactive graph artifact (self-contained HTML)
mkdir -p /tmp/devcouncil-judge-demo
dev graph demo --project-root /tmp/devcouncil-judge-demo --json
# Open /tmp/devcouncil-judge-demo/.devcouncil/graph/demo.html
```

**What “good” looks like:**

1. `devcouncil --help` prints CLI help from the installed package.
2. `devcouncil-build-week-demo` prints one **red / not-verified** verdict, then one **green / verified** verdict; final assert shows `verification_mode=compiled` and `gap_count=0`; generated repo path remains on disk.
3. Graph `demo.html` shows **visible nodes** (not a blank canvas); click / double-click filters, path highlighting, and neighborhoods work without console exceptions from an incompatible ForceGraph API.

**Fallback** (only if `0.4.2` is not yet on the registry): clone the repo checkout that contains the Build Week fixes and run the same commands (`uv sync --group dev` if you need a local `dev`).

**Optional Codex/MCP path (for agent-control judges):** configure DevCouncil MCP in Codex; call status / task / gaps / diff tools on a real project and confirm structured results parse (no `cli_parse_error` on large JSON) and diffs include untracked files when present. Prefer the controlled sample over this repo’s historical dogfood dashboard.

Walkthrough detail: [docs/build-week-demo.md](build-week-demo.md).

---

## 4. Full video narration script (2:30–2:50)

**Target length:** ~2:40. **Tone:** calm, concrete, show terminal + browser. **Do not** show DevCouncil’s noisy historical gap dashboard; use the controlled sample and repaired graph.

| Time | Shot / on-screen | Narration (read aloud) |
|---|---|---|
| **0:00–0:20** | Title card or social preview; cut to agent chat claiming “done” with no proof | “Coding agents — including Codex and other prompt-taking CLIs — often claim success without proving the change satisfied the original requirements. DevCouncil turns that claim into a gated engineering workflow: every change is scoped, verified, and traceable back to a requirement.” |
| **0:20–0:40** | Terminal: `npm install -g devcouncil@0.4.2` then `devcouncil --help` | “Judges can install the Build Week package in one step. Here is `devcouncil@0.4.2` — help works from a fresh install. No rebuild of the monorepo required.” |
| **0:40–1:15** | Terminal: `devcouncil-build-week-demo` — pause on red, then green | “The core demo needs no API keys. We scaffold a tiny calculator repo, introduce a deliberate bug, and run `dev check --verify`. First verdict: blocking — red. We apply the real fix and regression test, then rerun. Second verdict: compiled mode, zero gaps — green. Evidence, not vibes.” |
| **1:15–1:50** | Browser: open `demo.html`; click node, show filter / path / neighborhood | “Side feature judges can open immediately: a self-contained interactive code graph. Packed ForceGraph is fixed so you get a real canvas, not a blank page. Filters, path highlighting, and neighborhoods make blast radius visible.” |
| **1:50–2:20** | Diagram or quick MCP/Codex panel: Requirement → Task → Diff → Evidence; brief status/diff tool | “The control loop for agents is Requirement, Task, Diff, Evidence. Through MCP, Codex can read status and diffs that stay correct under real project-sized JSON — including untracked files — so the agent resumes from evidence instead of chat memory.” |
| **2:20–2:45** | README eligible commits / short list of Build Week themes | “DevCouncil predates Build Week. Only post–July thirteenth extensions are in this submission — baseline three-c-f-d-five-d-one, eligible from six-f-five-b-d-seven-three. Codex and GPT-5.6 helped map, repair, and verify; the maintainer owned requirements and acceptance. We do not claim exclusive authorship of every eligible line.” |
| **2:45–2:50** | Closing title / tagline on screen | “Model confidence is not the final authority; evidence is.” |

**Recording notes**

- Keep total under **3:00**; prefer **2:30–2:50** with audible narration.
- Upload publicly (YouTube or Vimeo); verify playback signed-out / private window.
- Thumbnail: repaired graph screenshot **or** clean green evidence report — not the 267-gap dogfood dashboard.

---

## 5. Devpost required-fields checklist

Fill every field before submit. Re-open the draft after save and verify.

| Field | Value / action | Done? |
|---|---|---|
| Project title | e.g. **DevCouncil — Evidence-Gated Agent Orchestration** | ☐ |
| Tagline | e.g. **Coding agents claim success; DevCouncil requires evidence.** | ☐ |
| Track | **Developer Tools** | ☐ |
| Project description | Paste §2 blurb; keep eligibility candid | ☐ |
| Built with / tech tags | Python, TypeScript/Node, MCP, SQLite, tree-sitter, Git, npm | ☐ |
| Repository URL | https://github.com/bharathvbcr/DevCouncil (public; eligible history visible) | ☐ |
| Demo / video URL | Public narrated video under 3 min (after upload) | ☐ |
| Thumbnail / cover image | Graph or clean evidence report | ☐ |
| “How to test” / judge instructions | Paste §3 (npm `0.4.2`, packaged demo bin, graph) | ☐ |
| Platforms supported | Paste §6 | ☐ |
| Country | Submitter’s country | ☐ |
| Submitter type | Individual / team (as applicable) | ☐ |
| Team members | As required by Devpost | ☐ |
| **Primary Codex `/feedback` session ID** | **Human must copy from Codex `/feedback` — do not invent or guess** | ☐ |
| Optional secondary session IDs | Only if Devpost allows; still copy from `/feedback` | ☐ |

### Codex session ID — human-only

1. In the primary Codex session used for Build Week work, run `/feedback` (or the product’s equivalent feedback surface).
2. Copy the **session ID** exactly as shown.
3. Paste into the Devpost field reserved for Codex feedback / session ID.
4. **Do not** use a placeholder, fabricated UUID, or an ID from chat logs that was not shown by `/feedback`.

Submit only after: (a) public video plays, (b) published `devcouncil@0.4.2` judge path is green (or documented fallback), (c) every checklist row above is filled.

---

## 6. Platforms supported

| Platform | Support |
|---|---|
| **macOS** | Supported |
| **Linux** | Supported |
| **Windows** | Supported |

**Runtime prerequisites:** Node.js 18+, Python 3.12+, Git.

**Judge demos:** provider-free CLI + graph HTML; no cloud model key required for `build-week-demo.sh` or `dev graph demo`.

**Agent integration (optional for scoring narrative):** Codex and other MCP-capable coding CLIs on the same platforms.

---

## Human next steps (after this draft)

1. Confirm `npm view devcouncil version` shows `0.4.2` (or use README fallback wording until publish lands).
2. Record + upload video from §4; confirm public playback.
3. Paste title, tagline, description, judge steps, platforms into Devpost.
4. Copy **real** `/feedback` Codex session ID into Devpost.
5. Thumbnail + final review of every required field → submit.
