# The stack: GitPulse, Manvi and DevCouncil

Status: **draft vision, 2026-11-22.** Normative for ownership and boundaries once accepted; the
other repositories link here instead of restating it. Each boundary is written as **today /
target / gap**, because several statements in the current READMEs describe the target as if it
were today (see [Truth fixes](#truth-fixes-before-any-launch)). Evidence labels: **V** verified
in code at the path given, **I** inferred, **U** unverified.

---

## 1. One sentence each

| | Is | Answers | For | Works alone? |
|---|---|---|---|---|
| **GitPulse** | The workspace: an ADE (agentic development environment) | "What are my agents doing, is it safe, and how do I land it?" | Developers supervising agents: from one Claude Code session to a fleet | Yes. A git client and agent monitor with no other piece installed |
| **Manvi** | The harness: an embeddable agent runtime | "Run an agent loop, local-model first, under policy, inside my app" | People building agents or agent-powered apps | Yes, but it needs DevCouncil's modules, by design |
| **DevCouncil** | The engines: code intelligence, verification, task state and policy as separate modules | "What does this change touch, and did the work meet its intent?" | Any agent (via plugin), any harness, CI | Target: each module on its own. Today only `devmap` installs alone without a toolchain (section 3) |
| **Jarvis** | The computer-use arm: a capability that agents built on Manvi call to operate desktop apps (owner, 2026-11-22) | "Do this task in a desktop app, safely and repeatably, and prove it worked" | Manvi-based agents: Manvi itself, Vani, and later ones | As a component, yes: its own CLI and workbench. As a product, through the agents that use it |

**The stack line:** *GitPulse is where you work. Manvi runs the agent. DevCouncil knows the code
and checks the result.*

Three consumer shapes for DevCouncil, which is the point of keeping it modular:

1. **Plugged into an existing agent.** Claude Code, Codex or Cursor installs the DevMap plugin and
   gets the code graph over MCP. No Manvi, no GitPulse.
2. **Embedded in a harness.** Manvi wraps dcstore, dcverify, devmap and the policy packages into a
   turn loop.
3. **Embedded in an app.** GitPulse vendors the read-side crates and shells out to the CLIs.

The model to copy is the editor world: language servers and test runners (DevCouncil) are not the
editor (GitPulse), and the runtime that executes code (Manvi) is neither.

---

## 2. Who owns what (today / target / gap)

One canonical owner per concern. Other repos hold thin adapters only.

| Concern | Today | Target owner | Gap to close |
|---|---|---|---|
| Code graph, impact, affected tests | DevCouncil `devmap` (21 MCP tools). GitPulse vendors the readers and serves 11 of them through `gitpulse-mcp`; `devmap_parity.rs` claims parity with upstream, but its test compares the list with itself (V, per review) | DevCouncil | GitPulse does not vendor `devmap-serve`, so `check:schema-parity` reads upstream `TOOL_NAMES` at the vendored DevCouncil commit and checks GitPulse's subset (in progress: GitPulse branch `stack/devmap-tool-parity`) |
| Verification and evidence | `dc-verify` / `dc-evidence`. Manvi has its own `serve/verify.go` checks (V, overlap extent U) | DevCouncil | Manvi's `work.verify.task` delegates to dcverify, keeping only orchestration |
| Task and lease state | `dc-store` defines `SCHEMA_VERSION` 11 and `WORKBENCH_SCHEMA` 17 (V). GitPulse reads execution tasks read-only and vendors the workbench API; `tasks/file_tasks.rs` reads Markdown briefs, it is not a store (V) | DevCouncil schema; Manvi's `work.*` is a client; GitPulse is the board UI | GitPulse `src-tauri/Cargo.toml` says "Manvi owns storage schemas", which contradicts the code. Fix the comment |
| Policy decisions (command/write gate, grants) | Code in DevCouncil `backend/go_orchestrator/policy`, `gate`, `grants` (V). Manvi wraps it (`serve/policy.go`, V). GitPulse hooks reach it through `harness::guard_command` → `manvi serve` (I) | DevCouncil decides; Manvi and GitPulse enforce | Marketing calls it "Manvi's command gate". Say "DevCouncil policy, enforced by Manvi and GitPulse" |
| `devcouncil_*` task tools (checkout, renew, verify, release, …) | DevCouncil's MCP registry has about 9; Manvi's native table has about 42, of which about 8 share names with DevCouncil's (I, from string literals). Manvi's other tools are harness tools (exec, write_file, subagents) | DevCouncil owns the shared task tools; Manvi owns its harness tools | Give the ~8 shared tools one implementation in a DevCouncil Go package that both register |
| Agent runtime: turn loop, providers, local models, managed runs, TUI | Manvi (V) | Manvi | None |
| Worktrees, sessions, collisions, claims, load, git, board UI | GitPulse (V) | GitPulse | None |
| Wiring into agent hosts (MCP registration, hooks, plugins) | Three engines: `devmap integrate` (owns the host table), `dev integrate`, and GitPulse's Connect flow (`agentIntegration.ts`, `claude_hook.rs`, `agy_plugin.rs`) (V) | One engine in DevCouncil; GitPulse is its UI | Route `dev integrate` and GitPulse Connect through the one host table |
| Skills shipped to agents | The same skills in at least four places in DevCouncil and copied into every sibling repo's `.claude/skills` and `.agents/skills`; the canonical devmap skill already differs from the installed copy (V) | The repo of the product a skill describes, shipped only by that product's plugin | Stop vendoring other products' skills into each repo; keep `sync-agent-defs.mjs` as the check |
| Hooks into agents | `gitpulse-hook` and `devmap hook`. No tracked source wires the retired `devcouncil hook …` (`cmd/devcouncil/main.go:151`); only a stale, gitignored generated plugin copy (`.devcouncil/claude-plugin/devcouncil/hooks/hooks.json`) still does (V, per review) | Documented chain, one owner per event | Have `uninstall`/`doctor` find and clear stale installed plugin copies; publish the order (section 7) |
| Schema numbers and compatibility | DevCouncil owns the constants; the parity check runs from GitPulse (`bun run check:schema-parity`); pins disagree: Manvi pins DevCouncil `d938f7f8`, Jarvis pins `230bbf04`, GitPulse's `VENDOR.json` records `c6788883` and `230bbf04`, and Jarvis pins Manvi `5e32e6ad` while Manvi is at `53c074e6` (V) | DevCouncil publishes schema numbers and reader ranges; GitPulse, where the parity check runs, holds the stack release manifest | Section 8 |
| Text search vs graph search | `dcgrep` (text) and `devmap search` (symbols) | Both, deliberately | Different questions; `dev search` is the single entry that routes. Do not merge |

---

## 3. DevCouncil: modules you can take one at a time

### Today

- The catalog already models six components and presets (`devcouncil/components/catalog.go:38-98`, V),
  and `install --only NAME` exists (V).
- **But distribution is not per module.** Only `devmap-cli` is built for release
  (`dist-workspace.toml`); `dc-store`, `dc-verify`, `dc-redact` are `dist = false`. `install.sh --only=devmap`
  first builds a Go installer (`cmd/dcinstall`), so it needs a Go toolchain even for one Rust binary
  (it does not need the gusset checkout; `scripts/install.sh:63-65`, V). So "install only what you
  need, without toolchains" is true only for `cargo install devmap-cli`, and that needs Rust.
- Each binary has three names (18 in all: `devcouncil/devc/dev`, `devmap/devc-map/dc-map`, …) (V).
- Manvi depends on DevCouncil's Go code through a sibling-path `replace` at `v0.0.0` and references
  about 15 of its packages (V). By the owner's design, `dc/*` is the declared adapter surface. The gap is
  that it is not tagged, and nothing stops a consumer importing the other exported packages.

### Target

| Module | Binary | Answers | Install alone |
|---|---|---|---|
| Code intelligence | `devmap` | Where is it, who calls it, what breaks, which tests | Prebuilt binary + its plugin |
| Verification | `dcverify` | Did the diff meet scope, rigor, coverage and the acceptance contract | Prebuilt binary; usable in CI with a diff and an evidence file |
| Task and lease state | `dcstore` | What work exists, who holds it | Prebuilt binary |
| Text search | `dcgrep` | Ignore-aware ranked text search | Prebuilt binary |
| Redaction | `dcredact` | Strip credentials from text | Prebuilt binary |
| Policy | Go packages `policy`, `gate`, `grants` | Is this command or write allowed | A tagged Go module with a declared API |
| Host | `devcouncil` | MCP server, integration engine, skills, gates, verify orchestration | Needs whichever modules you call |

Install profiles, named for who is asking:

- **"I use Claude Code / Codex / Cursor":** the DevMap plugin. One binary.
- **"I want evidence gates in CI":** `dcverify`.
- **"I am building a harness":** `devmap` + `dcstore` + `dcverify` + the policy Go module (this is Manvi's set).
- **"Everything":** the host.

### Gap list, in order

1. Prebuilt release binaries for every module (fix the Windows `tgrep-core` build that keeps
   `dc-grep` out), and make `install.sh --only=X` fetch that binary with no Go or Rust toolchain on the machine.
   Ship signed and notarised builds of the same binaries; a newcomer and a teammate both need it.
2. One canonical name per binary (the catalog's: `devcouncil`, `devmap`, `dcstore`, `dcverify`,
   `dcgrep`, `dcredact`). Other names become deprecated shims for one release, then go.
   `dev` is the riskiest: a two-letter name shadows easily on `PATH`.
3. Tag the `dc/*` adapter surface, plus the policy packages Manvi needs. Move the rest under
   `internal/` so the compiler enforces the boundary.

---

## 4. Manvi: the harness built on DevCouncil modules

**Identity.** A Go and Rust agent runtime: turn loop, providers (local models first), TUI, session
log, managed runs, and `manvi serve`, an NDJSON host protocol. Its analysis, task state, verification
and policy come from DevCouncil. Its own extension seam, `serve.Module` / `Router.Register`
(`manvi/serve/module.go`), is real: per-server, collision-checked, fail-closed (V).

**Positioning.** "The agent runtime you can embed." Two audiences:
local-model users who want a lean harness, and builders who want an agent loop inside their own
app. GitPulse is one embedder (over stdio). Jarvis plugs in as a module (in-process) and gives every
Manvi-based agent, such as Vani, a computer-use arm.

**Boundary, today → target.**

- **Rust components:** in-process over gusset where `docs/gusset-candidates.md` says the call is
  worth it (today: the gusset policy engine), and JSON-on-stdio child processes otherwise. That is the
  owner's design (2026-11-20), today and target.
- **Go:** Manvi links DevCouncil's Go adapter packages (`backend/go_orchestrator/dc/*`) natively and
  pins them, by design. Today that is a sibling `replace` at `v0.0.0`. Target: the same packages under a
  tag (section 3). The README sentence "reaches every one of them across a process boundary, linking none"
  is false for Go today and must be corrected now, not when the target lands.
- **Shared task tools:** about 8 tool names appear in both DevCouncil's MCP registry and Manvi's
  native table (I). Those task tools get one implementation in a DevCouncil Go package, and both
  register it. Manvi's other ~34 tools are harness tools and stay in Manvi.

**Jarvis: the computer-use arm.** The owner defines Jarvis as the computer-use arm for Manvi, for
Vani and for agents still to come (2026-11-22). Jarvis already ships a `vani` profile and policy
(`profiles/vani.json`, `policies/vani.json`). The arm works in three steps:

1. A model explores a desktop task once.
2. Jarvis freezes what it found into a typed capability artifact.
3. Jarvis replays that artifact with zero model calls, behind an input guard and human approval for
   account changes.

DevCouncil's `dc-evidence` then judges the result independently. Jarvis plugs in through Manvi's
`serve.Module` seam (`jarvis.*` ops), so any Manvi-based agent gains the arm by registering the
module. The boundaries:

- **Manvi** owns the agent loop and the seam.
- **Jarvis** owns desktop discovery, capability artifacts, replay, the native brokers and the
  workbench.
- **DevCouncil** owns acceptance.

Its roadmap follows what the agents need. It must ride the same stack release (section 8), not
its own pins.

---

## 5. GitPulse: an ADE for two audiences

**What an ADE is.** An IDE is organised around the file you are editing. An ADE is organised around
work in flight: agents, worktrees, tasks, sessions, evidence and the machine they run on. GitPulse
is not an editor and should say so. It launches, watches, guards and lands the work of the agents
and editors you already use.

### One app, disclosure by situation

Do not build two modes or persona presets first; the feature registry they would need is still on
`feat/labs-cloud-unified`. Instead, show surfaces when the situation calls for them:

| Situation | What appears |
|---|---|
| One repo, no agent | A fast git client: Work, Code, History, Diff. This is the newcomer's whole screen |
| A Claude Code or Codex session is detected (even one GitPulse did not launch) | The agent tree, Follow agent, turn checkpoints |
| A second agent in the same repo | Worktree lanes, collision radar, claims, peer inbox |
| More than one repo open | Fleet |
| Host pressure rises | Load governor and parked work |
| Work is ready to land | Review, merge train |

A visible **"Show everything"** switch turns all of it on for power users. The existing
`viewAttention` and view-visibility code is the seam to extend (I).

### Bridges for newcomers (cheap, do first)

- **From Claude Code:** "Keep using Claude Code. Run `claude` in your terminal as you do now; GitPulse
  finds the session, shows what it changes, and warns when two agents meet." Zero migration is
  the pitch, and session detection already makes it true.
- **From VS Code:** bind `⌘⇧P` to the command palette alongside `⌘K`, and add **Open in editor at
  `file:line`** for VS Code, Cursor, Zed, JetBrains and Xcode (GAPS.md #4). The answer to "where do I
  edit?" is "in your editor, one keystroke away".
- **First-run tour:** keep the 6-step tour and add one step: "Connect your agent". That step
  shows the same plan as Settings → Agents.

### Power surfaces (from GAPS.md, in priority order)

1. Multi-comment diff review and "send N comments to the agent" (GAPS #1), then GitHub inline
   review threads (#2).
2. Cross-session transcript search over the ledger: "which agent changed `auth.rs` last week and
   why?" (#7).
3. Live MCP call inspector (#7).
4. A turn-checkpoint rewind and fork timeline (#6).
5. Cost and quota per run and per fleet (#11).

---

## 6. GitPulse for teams

> **Blocked for launch claims.** Per the owner's 2026-11-22 launch decision, no launch material claims
> team or cloud features until the labs test gate (card ft-07313bab) is green. This section is the
> design to build toward, not copy to publish.

### What the agents already share natively (checked 2026-11-22 against vendor docs)

- **Claude Code.** A committed `.claude/settings.json` shares hooks, permissions, plugins and env.
  `extraKnownMarketplaces` + `enabledPlugins` share plugins (applied after folder trust; a
  relative-path local marketplace needs no per-user install). `.claude/skills` and `.mcp.json` are
  shared. Managed settings lock keys users cannot override.
  - **Trap:** Claude Code reads `AGENTS.md` natively only from v2.1.277, and then only when no
    `CLAUDE.md` exists, unless `CLAUDE.md` imports it with `@AGENTS.md` or the user-level "Project
    instructions" setting is `claude-md-and-agents-md`.
- **Codex.** Hierarchical `AGENTS.md` (with `AGENTS.override.md`) and project `.codex/config.toml`
  (trusted projects only). Admin `requirements.toml` (partly verified).
- **Cursor.** `.cursor/rules/*.mdc` and `AGENTS.md`. Admin Team Rules are applied first and enforced.
- **Antigravity.** `AGENTS.md`, `GEMINI.md`, `.agents/rules/*.md`. No admin lock found.

So sharing *files* is solved; every vendor does it through git. What no vendor does is tell a team,
across all four agents at once, **what each teammate's agent will actually load, and whether it
matches the team's intent.** That is GitPulse's job, and it fits the product's thesis: show the
real state rather than the claimed one.

### The Team layer: build the delta, not a new format

1. **Effective config view.** For each repo and each agent host, list what will load: which rules
   file (and whether the `CLAUDE.md`/`AGENTS.md` trap is hiding the team's rules), which skills and
   their source, which plugins at which version, which MCP servers are approved, which hooks fire, and
   whether the folder is trusted. Each item shows where it came from: repo, user, managed or plugin.
2. **Drift detection.** Compare that effective config with what the repo declares and with the
   last state the team approved. Show "your Claude Code is missing the team's MCP server" or "your
   GitPulse plugin is 1.3, the repo expects 1.4" on the Work overview, with a one-click fix that
   runs the integration engine (section 2).
3. **One source of rules, compiled out.** `AGENTS.md` is the source, because all four hosts can read it
   (Claude Code with the conditions above).
   GitPulse writes only what the hosts need beyond it: the `@AGENTS.md` import in `CLAUDE.md`, and
   `.mdc` files only for path-scoped Cursor rules. A check fails on hand edits to generated files.
4. **Team skills as an in-repo plugin.** Package the team's skills, MCP servers and hooks as a plugin
   in a relative-path marketplace inside the repo, with Claude, Codex and Cursor manifests. The
   DevMap plugin already ships all three manifests, so the pattern exists. Updating a skill is a PR.
   Teammates see "team config changed since your last pull" with the diff and one Apply.
5. **A policy floor.** In agents GitPulse launches, the repo's policy floor (DevCouncil policy,
   enforced by Manvi and GitPulse hooks) cannot be dropped by the per-user "settings sources"
   toggle. Outside GitPulse, the only real lock is the vendor's managed settings. GitPulse can
   generate the managed-settings fragment for an admin. It reports "floor not enforced on this
   machine" when the fragment is absent; it never claims a lock it does not have.
6. **New-teammate setup.** Clone, open in GitPulse, then see one checklist: binaries, plugins, trust,
   MCP approvals, index built. One Apply. This is the first-hour experience for a team, and it is
   the strongest demo for the team story.

**Customer zero is these four repos.** Their skills are already copied and drifted (section 2).
If the Team layer cannot make DevCouncil, GitPulse, Manvi and Jarvis agree, it is not ready to sell.

### Other team needs, ranked

1. PR review with inline threads and an agent pre-review (GAPS #1-3). This is where teams spend
   their time.
2. A shared task board through GitHub Issues sync (built on `feat/labs-cloud-unified`, unmerged;
   depends on decision 5).
   Git-native and server-free, which fits local-first.
3. Cross-machine presence and collisions: "a teammate's agent is editing `auth.rs` on their branch" (labs).
4. Signed and notarised builds, and prebuilt DevMap, so a teammate can install without a Rust
   toolchain (GAPS #16). This is a prerequisite rather than a feature, which is why it ships in
   Phase 1 (section 10).
5. Later: GitLab and Bitbucket, team spend rollups, and turning repeated review corrections into
   proposed rule changes.

---

## 7. Plugins and agent-facing surfaces

- **One marketplace** (GitPulse's `.claude-plugin/marketplace.json` or a dedicated repo) lists the
  stack's plugins:
  - `gitpulse`: workspace tools. Today it has 6 skills: collisions, insights, load, merge-train, tasks
    and terminal.
  - `devmap`: code intelligence.
  - `devcouncil`: verification and the task loop.

  Users add one marketplace and install what they need. The marketplace is an index only: each
  plugin stays installable from its own repo, so a DevMap-only user never needs GitPulse.
- **No skill in two plugins, and no tool served twice without a test.** GitPulse serving `devmap_*`
  tools is deliberate (one MCP config for GitPulse users), so its subset is tested against devmap's
  list.
- **One source per skill.** Each skill's source lives in its product's repo. The sibling repos'
  `.claude/skills` copies of other products' skills go away; the plugins deliver them.
- **Two MCP servers per product at most**, and say why each exists. Today DevCouncil has two:
  `devmap mcp`, the read-only graph, and `devcouncil mcp`, tasks, leases and verify. Keep them separate
  because they need different binaries. Stop emitting the `mcp-server` alias, so new configs use one spelling. Keep accepting it, because
  `package.json`, the golden manifest and `integrate.rs` reference it.
- **Hook chain, published.** For a tool call in a GitPulse-launched agent:
  1. `gitpulse-hook command-gate` / `collision-guard`. The policy decision comes from DevCouncil via Manvi.
  2. `devmap hook` for index freshness and context.
  3. No DevCouncil lifecycle hooks: they are retired. `doctor` flags any stale installed copy that still
     wires them.

  A deny from any step stops the call.
- **Version the plugins in the stack manifest**, not independently: the devmap plugin is at 0.2.5
  and the devcouncil plugin at 0.2.0 today.

---

## 8. Why three repositories, and how they stay one stack

**Why separate:**

- **Different audiences.** App users, harness builders and plugin/CI users each install
  one thing.
- **Different languages and release cadences.** A Tauri desktop app, a Go runtime and Rust engines
  each ship on their own schedule.
- **Different licences.** DevCouncil is Apache-2.0; GitPulse, Manvi and Jarvis are MIT.
- **The boundaries are the product.** JSON on stdio, the `manvi serve` protocol, MCP and the schema
  numbers are contracts. A monorepo would let them erode, and the audit shows they erode even
  across repos when nothing checks them.

**The honest cost** is lockstep on schemas and pins. Today that cost is visible: Manvi, Jarvis and
GitPulse's vendored copies record three different DevCouncil revisions.

**The fix has two parts.**

*A compatibility window.* Today `CURRENT_SCHEMA_VERSION` and `MIN_READER_SCHEMA_VERSION` are both 29
(`rust/devmap-store/src/schema.rs:1773,1805`, V), so the window is zero: any schema bump forces every
consumer to move at once. That makes "install only the modules you need" a packaging claim and not
an operational one. Target:

- Each module publishes the schema it writes and the oldest reader it supports. Each consumer
  declares the range it accepts.
- On a mismatch, the consumer refuses loudly with a distinct *incompatible* status. That status is
  never shown as "unchecked" or as a pass.
- CI tests each consumer against the current and previous (N and N-1) versions of each module it
  reads.
- Where migrations allow, `MIN_READER_SCHEMA_VERSION` trails the current version by one.

*A stack release.* DevCouncil publishes the schema numbers and reader ranges (`protocols/schema-versions/v1.json`).
The release manifest that names one revision of each repo cannot live in DevCouncil, because Manvi
and Jarvis pin the DevCouncil commit that would contain it. It lives in GitPulse beside
`bun run check:schema-parity`. A stack release ships only when that check is green, all pins agree,
and every installed consumer reports the new numbers. Marketing talks about "Stack 2026.12", not four version numbers.
Bumping a pin still needs the owner's approval, per the standing cross-app rule.

---

## 9. Marketing and framing

### Umbrella

*Workspace · Harness · Engines.* "GitPulse is where you work. Manvi runs the agent. DevCouncil knows
the code and checks the result." Each site carries a three-tile "the stack" strip linking the other
two, and each product's page leads with what it does **alone**. No separate stack brand (decision 3).

**Launch wedge (owner, 2026-11-22):** lead with the **collision radar for parallel agents, powered by
DevMap**. DevMap leads the DevCouncil README, and the other modules stay listed in a module table. The
stack story below supports the wedge; it does not replace it.

### Proving they work as a team: one demo, three layers

A single 90-second story, the same on every site:

1. A task card on the GitPulse board. **Launch** starts Claude Code (or Manvi with a local model)
   in its own worktree.
2. The agent asks DevMap for the blast radius. GitPulse shows the same impact and affected tests
   to the person.
3. The agent tries `git push --force`. The policy gate blocks it: DevCouncil's policy, enforced
   through Manvi, with the verdict shown in GitPulse.
4. A second agent opens the same file. The collision radar warns both.
5. The agent says "done". dcverify checks scope, rigor and coverage, and the card moves to Done
   only on a passed verification (otherwise Review).
6. The merge train lands it.

Every step names the layer doing the work. That is the proof of teamwork, not a slogan.

### By audience

| Audience | Message | Entry point |
|---|---|---|
| Claude Code user | "Keep Claude Code. Add a control room." | Download GitPulse; it finds your sessions |
| VS Code user | "Your editor for writing. GitPulse for directing agents and landing their work." | Download GitPulse; Open in editor is one key |
| Power user running fleets | "Every fact your agents see, you see." | The 66 MCP tools, Fleet, load governor, collisions |
| Team lead | "Know what every teammate's agent will load, and keep it in step." | Team layer (section 6). Not used until ft-07313bab is green |
| Harness or app builder | "An agent runtime you can embed, with a computer-use arm (Jarvis) when your agent needs to operate desktop apps." | Manvi + Jarvis |
| Agent user who wants nothing else | "Give your agent a code graph." | DevMap plugin |
| CI owner | "Make AI-written changes prove they met the task." | dcverify |

### Truth fixes before any launch

- Manvi's README: "linking none" is false for Go. The badge says 51 tools and the text says 54.
- GitPulse `docs/PROMO.md` sells a fast git client with "280 handlers" and a "5-stage ladder". The README
  says 66 tools and eight decision statuses, and the launch plan says 65 tools. Rewrite PROMO around
  the ADE story and generate every count from code at build time.
- GitPulse `Cargo.toml`'s "Manvi owns storage schemas" comment (section 2).
- Credit the policy gate correctly: DevCouncil policy, enforced by Manvi and GitPulse.
- Keep the WHY_GITPULSE discipline: dated comparisons, "not found" never written as "does not exist",
  and a "where others are stronger" section. It is the most credible page in the stack. Extend
  that discipline to all three sites.

---

## 10. Sequence

| Phase | Ships | Done when |
|---|---|---|
| **0. Stack hygiene** | Truth fixes (section 9); one DevCouncil revision across Manvi, Jarvis and GitPulse's vendored copies; `protocols/schema-versions/v1.json` and the stack release manifest; stale plugin copies flagged by `doctor`; skill copies collapsed to one source each; GitPulse's devmap parity test made real | `check:schema-parity` green, every installed consumer reports the same numbers, and the skill sync check passes in all four repos |
| **1. Real modularity** | Prebuilt, signed binary per DevCouncil module; `--only` installs with no toolchain; one name per binary; declared Go API; one implementation of the shared task tools; compatibility window and N/N-1 tests; signed and notarised GitPulse with bundled DevMap | A clean machine installs `dcverify` alone with no toolchain, Manvi builds against a tag, and a consumer one schema behind keeps working or refuses as *incompatible* |
| **2. ADE for two audiences** | Disclosure by situation; `⌘⇧P`; Open in editor; review loop; transcript search | A Claude Code user goes from download to watching their existing session in under two minutes |
| **3. Team layer** | Effective config view; drift detection; compiled rules; in-repo team plugin; policy floor; teammate setup checklist; GitHub Issues board sync (after decision 5) | A second developer clones one of these four repos and is configured identically with one Apply |

Do not start a later phase's marketing before its "done when" holds.

---

### Phase 0 progress (merged to each repo's main on 2026-11-23 by fast-forward; not pushed)

- **DevCouncil `stack/schema-versions-json` (`157bf84c`).** Adds `protocols/schema-versions/v1.json`.
  - Pinning tests in dc-store and devmap-store, with a strict integer parse.
  - The new test is classified in the grammar-need guard.
- **GitPulse `stack/devmap-tool-parity` (`eccb7996b`).** Adds a `check:schema-parity` row that checks GitPulse's 11 `devmap_*` tools are a subset of upstream `TOOL_NAMES` at the vendored commit.
  - An unreadable upstream is reported as *unverifiable*.
  - The `devmap_parity.rs` comments are corrected.
- **GitPulse `stack/cargo-comment-truth` (`a64b38bb1`).** The `Cargo.toml` comment now names DevCouncil as the schema owner.
- **Manvi `stack/readme-truth` (`494ed435`).** The README names every DevCouncil Go package Manvi links, and gives the true tool counts (54, or 53 with no skills) across README and docs.
  - A test guards the package list using Manvi's import graph.
  - A test guards the counts using the observed registry.

## 11. Decisions (2026-11-22)

The owner delegated these to the advisor review ("ask Fable for decision and proceed").

1. **The integration engine stays in `devmap`.** The host table is already there, and the launch leads
   with DevMap. `dev integrate` and GitPulse Connect route through it. Split it into its own module
   only when a user who wants hooks but not the code graph turns up.
2. **The shared task tools live in DevCouncil's `backend/go_orchestrator/dc/` package.** This is the
   owner's 2026-11-20 design, in which Manvi links `dc/*`. Manvi registers that package and keeps its
   own harness tools.
3. **No separate stack brand. The shared marketplace is its own index-only repo,** so a DevMap-only
   user never needs GitPulse. Creating that repo is an owner step.
4. **Superseded by the owner (2026-11-22): Jarvis is the computer-use arm for Manvi, Vani and later
   agents, not just a reference app.** It stays public. Before promoting it, reframe `REPORT.md`
   ("Jarvis assignment report") and the README's assignment-archive mentions around the arm role, and
   align its pins.
5. **`feat/labs-cloud-unified` does not merge now.** It is gated on ft-07313bab being green, as the owner
   already decided. No team or cloud claims until then.

### Needs the owner

- Aligning pins and re-vendoring:
  - Manvi pins DevCouncil `d938f7f8`.
  - Jarvis pins DevCouncil `230bbf04` and Manvi `5e32e6ad`.
  - GitPulse's `VENDOR.json` is mostly `c6788883`.
- The stack release manifest, which cannot be honest until the pins agree.
- Collapsing the skill copies across the four repos.
- Pushes, the homebrew-tap repo and its secret, and the v0.2.5 tag.
- The Jarvis reframe, the labs merge, and the marketplace repo.

## Evidence notes

- Built from four read-only audits (GitPulse surfaces, DevCouncil modularity, Manvi/Jarvis
  boundaries, the cross-repo overlap matrix), a review pass and a vendor-docs check, on 2026-11-22.
- The audits mostly used `rg` and file reads, not DevMap or GitPulse Insights, because the questions were
  about manifests, docs and file layout. That is a recorded gap: none of the symbol-level claims above
  were confirmed by the graph.
- Unverified items are marked **U** or **I** inline. Exact MCP tool counts per server were not
  recounted.
