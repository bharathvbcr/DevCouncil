# DevMap skill delivery

DevMap ships five workflows: `devmap`, `devmap-debugging`, `devmap-exploring`,
`devmap-impact`, and `devmap-refactoring`. A connected MCP server exposes tools;
it does not by itself install these workflow files in every agent host.

## Install and verify

Use the existing DevCouncil CLI from an environment containing this version.
From a source checkout, install the Go binary (`bash scripts/install.sh`) and
run from the DevCouncil repository root. The target repository must already exist.

```bash
dev skills scaffold --project-root /absolute/path/to/repo \
  --destination .agents/skills \
  --skill devmap --skill devmap-debugging --skill devmap-exploring \
  --skill devmap-impact --skill devmap-refactoring
```

Repeat the command with `--check` for read-only verification: exit 0 means all
selected files match, exit 1 means files differ, are missing, or validation
failed. Errors are printed separately from the missing/differing-file count.
`--dry-run` lists differences without writing or asserting installation success.
`--skill` selects the packaged version, independent of stale repository copies.
It cannot be combined with `--all` or a selection goal.

Without `--destination`, the installer writes all three supported layouts:
`.claude/skills`, `.cursor/skills`, and `.agents/skills`. Repeat the option to
select several layouts. Claude users can also generate the native plugin with
`devmap claude plugin`; that existing path includes MCP settings and hooks in
addition to the skills. Standalone scaffolding installs skill files only.

Keep the skill files and `.devcouncil-skills.json` receipt in version control
when distributing them with a repository. The receipt contains relative paths
and content hashes, allowing subsequent installs to recognize unchanged managed
files. Check that repository ignore rules expose the intended files. Codex
discovers repository skills under `.agents/skills`; if an existing session does
not refresh its skill list, start a new session. File validation alone does not
prove that an already-running host has reloaded its catalog.

## Update and refusal behavior

The installer preflights the whole batch before creating directories. It adopts
identical files and updates an existing file only when its content still matches
the prior receipt. An unowned differing file or a local edit is refused. Preserve
or move that file before intentionally replacing it; there is no force flag.

Writes use the existing atomic-file writer, and concurrent installers serialize
through `.devcouncil-skills.lock`, with a five-second timeout. A failed write is
reported, the lock is released during normal exception handling, and a rerun can
adopt already-complete files. This is atomic per file, not a transaction over the
entire batch. An abruptly killed process can leave a lock behind; verify that no
installer owns it before manually removing it. The installer never steals locks.

Bounds are 256 skills, 16 destinations, 256 KiB per skill, 8 MiB per rendered
batch, and 4,096 receipt entries within 1 MiB. Invalid names, traversal paths,
existing destination symlinks, nonregular files, and malformed receipts are
rejected. This protects ordinary local installation; it is not a sandbox against
a hostile process swapping ancestor directories during an installation.

## Audit findings and fixes

| Reproduced gap | Resolution |
|---|---|
| Four descriptions contained unquoted YAML colon-space syntax, causing the registry to silently omit workflows. | Valid YAML in both distributions; malformed skill files now raise a path-bearing error. |
| The skills existed in source and the Claude plugin but were absent from GitPulse's Codex skill directory. | Exact packaged selection and destination controls in the existing installer; all five installed in GitPulse. |
| Maintained GitPulse guidance required both indexes despite prioritizing DevMap; the generated block linked GitNexus debugging. | Maintained precedence explicitly makes GitNexus conditional and links DevMap workflows outside the generated block. |
| Skill text categorically banned GitNexus and overstated graph results as proof of breakage. | DevMap-first guidance allows justified fallback and distinguishes static evidence, candidate tests, and runtime verification. |
| Scaffold writes could overwrite differing local files and had no bounded concurrency contract. | Hash receipt, complete preflight, bounded serialization, size/path validation, and per-file atomic writes. |
| Duplicate GitPulse ignore rules concealed the installed skills and defeated marketplace exceptions. | Narrow published-file exceptions with a Git-based regression test. |
| Initial receipt placement created a DevCouncil directory that widened later skill selection. | Root receipt file preserves selection behavior. |
| JSON `true` was accepted as receipt schema 1 through Python boolean/integer equality. | Exact integer schema validation. |

## Verification

Verified locally on macOS on 2026-09-10:

- 153 skill/registry/CLI tests and 106 integration-caller tests passed.
- 18 native plugin integration tests passed; the separately invoked Claude
  strict-validator acceptance test also passed (19 native tests total).
- All five GitPulse guidance/ignore-rule tests passed.
- Go skills package tests and DevMap CLI skill/plugin tests passed for the
  current implementation. Ten native/installed skill files passed the skill validator.
- All 22 bundled library skills installed into three host layouts (66 files);
  repeat installation and read-only verification produced no changes.
- GitPulse's explicit five-skill check reported zero missing or differing files.

The integration-caller suite emitted one existing Gemini CLI deprecation warning;
it had no failures. The two installer/CLI source files add 182 lines and remove
24, reusing the existing registry and atomic writer rather than adding a second
installer or dependency.

Run from the DevCouncil root:

```bash
go -C backend/go_orchestrator test ./devcouncil/skills -count=1
```

From `rust-port`, run `cargo test -p devmap-cli --test claude_integration`.
When the Claude CLI is available, also run the explicitly ignored acceptance
test: `cargo test -p devmap-cli --test claude_integration the_emitted_bundle_passes_claude_plugin_validate_strict -- --ignored`.
From GitPulse, run `npm test -- scripts/agent-guidance.test.ts`.

The delivery suite exercises 36 thread-contended installs and 16 subprocess
installs, managed upgrades, user conflicts, malformed YAML/JSON, traversal,
symlinks, named pipes, oversized files, Unicode/spaces in paths, lock timeout,
injected disk failure and recovery, and read-only checks. The baseline failures
were observed before the fixes; content-parity and discovery tests prevent the
original missing-workflow failure from returning.

These are focused local checks. Windows/Linux runtime installation, actual host
catalog refresh across all clients, hostile directory-swap races, and full
repository integration suites remain outside the demonstrated evidence. Passing
them does not establish universal or absolute confidence.
