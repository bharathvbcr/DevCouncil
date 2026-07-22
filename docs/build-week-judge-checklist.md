# Build Week judge checklist (go / no-go)

Short submission gate for OpenAI Build Week. Use after CI finishes and `devcouncil@0.4.2` is on npm. Do **not** demo this repo's dogfood dashboard; use the controlled sample paths below.

## Recommended Devpost thumbnail

**Primary (use on Devpost):** `src/devcouncil/assets/devcouncil_social_preview.jpg`

Raw URL: `https://raw.githubusercontent.com/bharathvbcr/DevCouncil/main/src/devcouncil/assets/devcouncil_social_preview.jpg`

**Secondary (clean graph still, no fake scores):** `docs/assets/code-graph-demo-preview.svg`

Prefer the JPG for the Devpost thumbnail slot; use the SVG for gallery/B-roll.

## Fresh install after 0.4.2 lands

```bash
npm view devcouncil version   # expect 0.4.2
npm install -g devcouncil@0.4.2
devcouncil --help
npx --yes --package devcouncil@0.4.2 devcouncil --help
```

Until published, clone the Build Week checkout and use local `dev` (`uv sync --group dev` if needed).

## Provider-free red to green evidence gate

Primary path (npm package includes the demo script + samples — no clone required):

```bash
devcouncil-build-week-demo
# Equivalent:
bash "$(npm root -g)/devcouncil/scripts/build-week-demo.sh"
BUILD_WEEK_DEMO_ROOT=/tmp/devcouncil-judge-demo bash "$(npm root -g)/devcouncil/scripts/build-week-demo.sh"
```

Checkout fallback:

```bash
git clone https://github.com/bharathvbcr/DevCouncil.git
cd DevCouncil
bash scripts/build-week-demo.sh
```

Expect red then green (`gap_count=0`), no API keys. See [build-week-demo.md](build-week-demo.md).

## Interactive code graph (repaired canvas)

```bash
mkdir -p /tmp/devcouncil-judge-demo
dev graph demo --project-root /tmp/devcouncil-judge-demo --json
# npx --yes --package devcouncil@0.4.2 dev graph demo --project-root /tmp/devcouncil-judge-demo --json
open /tmp/devcouncil-judge-demo/.devcouncil/graph/demo.html
```

| Artifact | Path |
|---|---|
| Interactive HTML (primary) | `/tmp/devcouncil-judge-demo/.devcouncil/graph/demo.html` |
| Static SVG companion (optional) | `/tmp/devcouncil-judge-demo/.devcouncil/graph/demo.svg` |
| Checked-in gallery still | `docs/assets/code-graph-demo-preview.svg` |

### What a judge should see in demo.html

- Sidebar tabs: Graph, Dead code, Communities, Processes, Intel
- Controls: mode, search, color-by, lenses, Reset view / Clear path
- Visible force-graph nodes (`main.py`, `task_runner.py`, `verifier.py`, flagged `unused_check.py`) — not a blank canvas
- Click / two-node path / double-click neighborhood work; vendor warning stays hidden (`zoomToFit` present)

## Final go / no-go

- [ ] Fresh published npm version installs successfully.
- [ ] Published `dev check --verify` returns a deterministic verdict on the controlled sample.
- [ ] Published graph demo renders nodes and controls instead of a blank canvas.
- [ ] MCP status/task/prompt/gaps/next-actions/list-tasks handle this repository's large structured output without parse errors or unbounded context dumps.
- [ ] MCP diff shows untracked files and task-scoped diff fails closed instead of broadening scope.
- [ ] README clearly identifies eligible post-July-13 work and contains reproducible judge steps.
- [ ] Public narrated video is under three minutes and matches the published behavior.
- [ ] Devpost project has a real title/tagline, Developer Tools track, repo, video, test instructions, country, submitter type, and `/feedback` session ID.
- [ ] Devpost thumbnail uses `src/devcouncil/assets/devcouncil_social_preview.jpg` (not a logo-only or noisy dashboard screenshot).
- [ ] Existing unrelated worktree changes were not staged or overwritten.

If the package is not published or the video is not publicly playable by the last 20 minutes of the submission window, stop adding improvements and recover those two blockers first.

## Pre-tag verification (maintainers only)

```bash
npm run pack:check
node scripts/npm-runtime-smoke.mjs
bash scripts/build-week-demo.sh
git diff --check
```
