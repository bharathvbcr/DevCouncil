# Coding CLI integration

Choose code navigation (`devmap`) and optionally task tooling (`devcouncil`).
These are separate MCP servers. Integration writes configuration and project
assets; successful file validation does not prove that a running editor has
reloaded or trusted them. [Documentation index](README.md)

## Standalone DevMap

Both native installers accept `cursor`, `claude`, `codex`, `antigravity`,
`opencode` and `warp`. The one source is the Rust `Host` enum; `devcouncil
integrate` hands every host name to `devmap integrate`, which refuses unknown
names with the list, and the Go host itself answers the legacy `gemini` /
`aider` targets with where they went. This is the repository's adapter set, not a claim
about every host's current capabilities.

From the target repository:

```bash
devmap integrate cursor --dry-run
devmap integrate cursor
devmap integrate cursor --check
```

There is no `--apply` on `devmap integrate`: an invocation without `--dry-run`
or `--check` applies changes. Read the JSON receipt with `--json` when scripting.

| Host | DevMap configuration surfaces |
|---|---|
| Cursor | Global MCP registration, owned project entries, managed guides/rule, `.cursor/skills` and `.cursor/hooks.json` |
| Claude | Global MCP registration and the project `.mcp.json` entry (both withheld while the Dev Map plugin is enabled), managed guides and `.claude/skills`; separate `devmap claude plugin` generates a plugin bundle |
| Codex | User `~/.codex/config.toml` MCP registration, `.agents/skills`, project plugin/hook assets; host trust may need renewal |
| Antigravity | `.agents/mcp_config.json` and `.agents/skills` |
| OpenCode | `opencode.json`, its project skills layout and a DevMap post-tool plugin |
| Warp | `.devcouncil/integrations/warp-mcp.json`; no project skill destination is installed by this adapter |

```mermaid
flowchart TD
    subgraph CLI["Integration Commands"]
        DM_Int["devmap integrate &lt;host&gt;"]
        DC_Int["devcouncil integrate &lt;host&gt;"]
    end

    subgraph Adapters["Supported Host Adapters"]
        Cursor["Cursor"]
        Claude["Claude Code"]
        Codex["Codex"]
        AGY["Antigravity"]
        OpenCode["OpenCode"]
        Warp["Warp"]
    end

    subgraph ConfigSurfaces["Target Configuration Surfaces"]
        UserConfig["Global User Settings<br/>(Global MCP configs, ~/.codex)"]
        ProjectConfig["Project Config<br/>(.cursor/mcp.json, .mcp.json, .agents/mcp_config.json, opencode.json)"]
        Skills["Skills Libraries<br/>(.cursor/skills, .claude/skills, .agents/skills)"]
        Guides["Managed Guides<br/>(AGENTS.md, CLAUDE.md)"]
    end

    DM_Int --> Cursor & Claude & Codex & AGY & OpenCode & Warp
    DC_Int -->|"Merges task tools & delegates"| DM_Int

    Cursor --> UserConfig & ProjectConfig & Skills & Guides
    Claude --> UserConfig & ProjectConfig & Skills & Guides
    Codex --> UserConfig & Skills & Guides
    AGY --> ProjectConfig & Skills & Guides
    OpenCode --> ProjectConfig & Skills & Guides
    Warp --> ProjectConfig & Guides
```

All adapters also use the managed guide-writing path. Inspect the receipt for
exact files and changes: global settings may be touched. Existing unrelated
server entries are preserved. Do not assume a project-only dry run previews
only project-local effects.

Every DevMap MCP call should include absolute `repo_path`; verify
`repository.root` in the answer. One global registration can serve multiple
repositories. Hard-coded database paths and duplicate server registrations can
otherwise route a query to the wrong index. `devmap paths --json` reports
binary/configuration diagnostics.

## Optional Go-host task tools

Install the full native suite first, then preview/apply the Go host:

```bash
devcouncil integrate cursor --dry-run
devcouncil integrate cursor --apply
devcouncil integrate cursor --check
```

`devcouncil integrate <host>` runs `devmap integrate <host> --servers-stdin`
and passes it the `devcouncil` server: this binary by absolute path, `mcp`, and
`DEVCOUNCIL_PROJECT_ROOT`. DevMap writes that entry into the host's project
document in the host's own shape, beside its own `devmap` entry, and reports
every file it examined; the Go receipt is that report. It needs a `devmap`
(`--devmap-bin`, `DEVMAP_BIN`, or one on `PATH` outside the repository) and
refuses without one. `--check` asks DevMap too, so it covers DevMap's assets.
See [the ownership decision](architecture.md#host-integration-and-skill-installation-one-owner).

| Host | Where the `devcouncil` server is registered |
|---|---|
| Cursor | `.cursor/mcp.json`; Go also writes `.cursor/rules/devcouncil.mdc` |
| Claude | `.mcp.json` |
| Codex | Project `.codex/config.toml` (`[mcp_servers.devcouncil]`, read for trusted projects); every other byte of the file is kept |
| Antigravity | `.agents/mcp_config.json` |
| OpenCode | `opencode.json` |
| Warp | `.devcouncil/integrations/warp-mcp.json` |

The Go host serves checkout, renew/release lease, next task, diff, gaps,
verification and write-policy checks. It does not serve arbitrary file-edit
or shell tools. Tasks require initialized state and a consuming agent that
honors policy/results. See [MCP task loop](hero-loop.md).

## Hooks, skills and enforcement

DevMap navigation hooks maintain code context. DevCouncil's old lifecycle
write/stop gates are retired. Installing either server does not intercept all
ordinary editor writes, and `--write-gate` is no longer supported.

Use `devcouncil skills list` and `devcouncil skills scaffold --dry-run` to inspect
the Go host's engineering skills. Embedded instructions can retain migration
limits; they are guidance, not a permissions mechanism. Both skill sets go
through one installer: `devcouncil skills scaffold` hands its library to
`devmap skills install --library-stdin`, which owns `.devcouncil-skills.json`.

Reload the host after updating a binary or configuration. For Codex hook
assets, the generated receipt calls out the host trust step; changing a hook
can require renewed trust. File checks and physical host execution are
separate verification steps.

## Remove retired DevCouncil hooks

```bash
dev hook status --project-root /path/to/project
dev hook disable --project-root /path/to/project --client claude --dry-run
dev hook disable --project-root /path/to/project --client claude
```

Status can be nonzero because registrations remain or inspection failed; read
its receipt to distinguish them. Cleanup preserves unrelated tools and records
recoverable backups. Old event invocations are silent no-ops, not verification.
See [CLI cleanup details](cli-reference.md#retired-hook-compatibility-and-cleanup).

Source owners:
[`rust/devmap-cli/src/integrate.rs`](../rust/devmap-cli/src/integrate.rs),
[`backend/go_orchestrator/devcouncil/integrate/integrate.go`](../backend/go_orchestrator/devcouncil/integrate/integrate.go).
