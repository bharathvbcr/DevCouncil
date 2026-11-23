# Security and execution boundaries

DevCouncil provides local code analysis, task policy and verification evidence.
The implementation-level reference is
[repository input and verification boundaries](SECURITY_BOUNDARIES.md).
[Documentation index](README.md)

## What a consumer must distinguish

- Code analysis does not require sending source to a model. A consuming agent
  or harness has its own provider and data-handling policy.
- Task leases and write-policy results apply to participating clients. Retired
  DevCouncil hooks do not intercept every shell command or editor write.
- Rigor includes secret, stub, `runner_hijack`, and `lean_tcb` checks (with optional Lean kernel checking via `DC_VERIFY_LEAN_KERNEL` / `DC_LEAN_KERNEL_TIMEOUT_MS`) over the supplied diff, untracked files,
  recreated files, and dirty submodule commit ranges, paired with `unattested_test_run` and `oracle_tampering` hard-safety gates. Git replace-refs are disabled
  at Go and Rust git seams to prevent hidden credential commits.
- Verification errors suppress raw diff lines to prevent echoing detected secrets,
  and panic hooks suppress backtrace payload text.
- Session children (e.g. language servers under `devmap build --lsp`) have their stderr
  drained and redacted for credentials with `dc_redact` on non-zero exit, retaining
  at most 4 KiB.
- Prompt inputs and host configs (`AGENTS.md`, `CLAUDE.md`, `GEMINI.md`, `mcp.json`, `.mcp.json`,
  skill roots and plugin manifests) are protected against agent writes at any depth and across all
  postures, including yolo.
- Missing and unavailable checks need explicit handling. Read gate mode,
  skipped reasons and coverage metadata in addition to the verdict.

## Verification commands run on the host

The Go gateway executes task commands through a local shell
(`/bin/sh -c`) in the project root, with an allow-listed environment: the
base list in `verify/env.go` (PATH, HOME, the Go, Cargo, Rustup, Node, Python
and JVM roots, locale and temp variables) plus names a repository lists in
`verify.env_allow: [NAME, NAME]` in `.devcouncil/config.yaml`. A name matching
the credential name rule (KEY, SECRET, TOKEN, PASSW*, CREDENTIAL, AUTH, PRIVATE,
COOKIE; DSN, PAT, JWT, PASS, PWD, HOOK and WEBHOOK; PROXY; datastore, broker and
package-index URL/URI names; see verify-sandbox-security-impact.md) is refused there, and every admitted and refused name is recorded in
the run's evidence. Each command
is bounded by a 30-minute timeout (`verify.CommandTimeout`) and by
cancellation of the run, its process group is killed when it is stopped, and
at most 4 MiB of its output is kept. `local` is the only sandbox: `verify.ParseSandbox` refuses any
other `--sandbox` value (exit 2, or an MCP error) rather than recording a
sandbox that did not run. Docker/Nix isolation is an approved design and is
not scheduled; see the [security-impact statement](verify-sandbox-security-impact.md).
The environment scrub does not hide files: a command can still read
`~/.aws` or `~/.ssh` through HOME. Review task command
lists and project configuration before running verification, as you would
review CI workflow changes.

The MCP host bounds each call. `devcouncil_verify_task` may run for
`verify.CommandTimeout` plus two minutes; every other tool gets 60 seconds.
`DEVCOUNCIL_MCP_CALL_TIMEOUT_MS` (milliseconds) replaces both, and the
timeout reply names it. `notifications/cancelled` stops the call it names and
its process group, a call that is cancelled is not answered, and closing stdin
cancels every running call before the host exits. A verification cut short by
its budget is still recorded, with its timeout gap.

## Prompt inputs and configuration write protection

Agents cannot overwrite instructions or MCP configuration that govern their own execution:
- **Refused targets:** Direct writes to `AGENTS.md`, `CLAUDE.md`, `GEMINI.md`, `mcp.json`, and `.mcp.json`
  are refused at any directory depth.
- **Skill and plugin roots:** Skill directories (`.claude/`, `.cursor/`, `.agents/`) and plugin
  manifests discovered under plugin roots are protected from agent write mutation.
- **Posture independence:** The prompt-input rung sits outside the policy `HardRules` block,
  ensuring it enforces under all postures, including `yolo`.
- **Read-only permission:** The rule strictly targets write operations; models can always read
  instruction files to orient and execute tasks.
- **Identity-based alias detection:** Hard links and symlinks resolving to prompt inputs are
  resolved and blocked by filesystem identity (`syscall.Stat` device/inode verification).
- **Residual boundaries:** Shell interpreters (`sh -c`), archive unpackers (`tar`/`unzip`),
  git state manipulators (`reset`, `merge`, `pull`, `stash pop`), and directory `cp`/`mv` under all
  postures remain unmodelled external execution paths pinned by tests in `gate/zadv_prompt_input_test.go`.
  The one exception is a git command aimed at another session's registered worktree
  (`git -C <sibling>`): there git may only read or record (`status`, `diff`, `log`, `add`,
  `commit` without `--amend`, and the other read-only forms), and `reset`, `stash`, `switch`,
  `merge`, `rebase`, `pull` and the rest are refused. See
  [Git in another session's worktree](SECURITY_BOUNDARIES.md#git-in-another-sessions-worktree).

Installing MCP configuration does not grant universal scope enforcement.
[Integration](coding-cli-integration.md) documents which settings and hooks
are written; the [task loop](hero-loop.md) describes the verification contract.
These tools assist review and do not replace a security assessment of the
application under development.
