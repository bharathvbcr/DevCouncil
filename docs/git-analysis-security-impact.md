# Security-impact statement: repository-chosen programs in analysis reads

Task `ft-ff2153d9` ("a repository DevCouncil analyzes can make it execute a
program"). This is the statement the task's acceptance criteria ask the pull
request to carry; it is kept here so it survives with the code.
[Security boundaries](SECURITY_BOUNDARIES.md) · [Documentation index](README.md)

## The class

A repository DevCouncil analyzes is input. Its working tree, its `.git/config`
(which can arrive with an archive or a copied checkout), and the arguments an
agent passes about it must not choose a program DevCouncil runs. The write gate
refuses writes under `.git/` in enforce mode, so a gated agent cannot plant
config; these changes cover config that arrived with the repository, advisory
and off modes, and agents not routed through the gate.

## What changed, and what each change narrows

| Way in | Before | After | Where |
|---|---|---|---|
| `core.fsmonitor`, hooks, `log.showSignature`/`gpg.program`, `diff.external`, textconv | ran during ordinary reads | switched off on every read's command line | `proc.GitArgs`, `dc_proc::guarded_args` |
| `filter.<name>.clean/smudge/process` from repository scope | ran during `diff` | reset to the operator's value or empty; `required` to the operator's or false | `proc.readRepositoryConfig`, `dc_proc::filter_overrides` |
| Promisor lazy fetch (`core.sshCommand`, credential and remote helpers) | ran during `diff HEAD` and `blame` in a partial clone | `GIT_NO_LAZY_FETCH=1` on every git; a missing object fails the read | `proc.gitEnv`, `dc_proc::git_env` |
| A git that ignores `GIT_NO_LAZY_FETCH` | — | refuses a partial clone's read, naming its version; ordinary clones proceed | `proc.GitSession.Command`, `dc_proc::git_with_program` |
| A submodule's own filters and fsmonitor | ran through git's per-submodule status during `diff HEAD` | `--ignore-submodules=dirty` on `diff`/`status`; dirtiness read per submodule under the guards | `proc.GitSession.DirtySubmodules` |
| A revision beginning with `-` (`--ext-diff`, `--output=<file>`) | re-enabled `diff.external`, truncated files, answered clean | refused at the MCP boundary and in dc-regress; only object ids reach `diff`/`log`/`show`/`blame` | `devmap-serve` `check_since`, `dc_regress::history::commit_oid` |
| Bare-name programs (`rm`, `ps`, language servers, `dcrelease`'s and the golden replay's `git`) | first match on `PATH`, including repository directories | absolute path, or resolved outside the repository | `hook.rs`, `installation.rs`, `lsp.rs`, `cmd/dcrelease`, `cmd/devcouncil-golden-replay` |
| Resolution boundary | the root a command was handed (`repo/backend`) | the repository's top, found without running git | `proc.repositoryTop`, `dc_proc::repository_top` |

Every change narrows what repository content or a caller's argument can make
DevCouncil run. None widens access, touches authentication, authorization,
secrets or quotas, or changes what a gate allows an agent to write.

## Behaviour that changes for users

- A partial clone's read with a missing object now fails where it used to fetch
  the object (and run whatever the fetch runs). Fetch the objects first, or work
  in a full clone.
- On a git that does not honour `GIT_NO_LAZY_FETCH`, a partial clone cannot be
  analysed at all; an ordinary clone is unaffected. `devmap doctor` reports the
  git version and a warning.
- Verification's git state no longer reads a failed `git diff` as "no change":
  only a repository with no `HEAD` falls back to listing untracked files.
- A project with no `.git` of its own, beneath a home directory that is itself a
  git repository (a dotfiles checkout), now has that home directory as its
  boundary — git itself treats it as part of that repository — so components in
  `~/.local/bin` are refused there, loudly, as repository-local.

## Owner decisions

- **Filter drivers (2026-10-09):** neutralize filters the repository's own
  scope defines (local and worktree, including `include.path` from tracked
  files); keep the operator's (system, global, command), so a global git-lfs
  install keeps working.
- **Global-scope keys (2026-10-09):** left alone. They are the operator's.
- **Submodules:** first decided "keep submodule behaviour"; the 2026-10-09 audit
  then reproduced a submodule's own filter running during the superproject's
  `diff HEAD`. Behaviour is kept — dirty submodules are still reported, by
  get_diff as `M` with no line counts and by verification as a changed path —
  but git no longer reads them itself.
- **D9, git versions (Fable, 2026-10-09):** `GIT_NO_LAZY_FETCH=1` everywhere;
  version detected once per resolved binary per process; refuse only a partial
  clone's read, with a typed error naming the version; not a warning, not a
  global version floor; `doctor` prints the version. The decision named git
  2.44 as the threshold and asked for it to be confirmed against git's release
  notes. Checked against git's source: the variable is read in `environment.c`
  from v2.45.0, and in `promisor-remote.c` in the CVE-2024-32465 maintenance
  releases v2.39.4, v2.40.2, v2.41.1, v2.42.2, v2.43.4 and v2.44.1; **v2.44.0
  does not read it**. The rule implemented is therefore "a git that does not
  honour the variable", which is what the threshold stood for, rather than the
  literal "below 2.44", which would have let v2.44.0 fetch. A git whose version
  cannot be read is treated as not honouring it.

## Known limits

- A session (one get_diff, one verification's git state) re-reads the config
  when the repository's own config files change, but does not watch files
  pulled in through `include.path`; a session lives for one operation.
- A gitlink in a repository with no `.gitmodules` is not checked for
  dirtiness (the check is skipped without a spawn when `.gitmodules` is
  absent). The diff reads run nothing inside it either way.
- Windows: the guards are applied there too, but the planted-program tests are
  shell scripts and run only on unix.
- Copies outside this repository — Manvi's `crates/dc-proc` and GitPulse's
  vendored `dc-proc`/`dc-regress`/`devmap-extract` — keep the old behaviour
  until they are updated.
