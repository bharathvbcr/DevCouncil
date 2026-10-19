# Build report — GitPulse backlog, 2026-10-13

Session state with the open lanes of this wave in review. Nothing here is merged or pushed. Done was not requested: Manvi evidence is null on every card, `dev verify` was not run, and the full suite was not run. `gates.mode` is off, so ordinary edits did not take a task lease. The Definition of Done still wants those gates.

Model routing: `spawn_subagent` has no model argument. The parent wrote each brief and reviewed each diff. Implementers and attackers were `code-editor` subagents. The first implementer on a card used `isolation: worktree`. A send-back resumed that same agent with `isolation: none` and `cwd` set to its worktree. Attackers used `isolation: none` in that worktree and did not edit production when the contract failed.

DevMap blast is unknown on the cards below, not a proof of safety. Impact walks were truncated or stopped early (`walk_incomplete`), and several worktrees have no `.devcouncil/codeintel/devmap.sqlite`. GitPulse Insights on the main checkout reports generation 4460 with `analyzer_freshness: false` ("stored extraction payload is obsolete"). DevMap MCP on the same generation reports `analyzer_freshness: true` and `is_fresh: true`. That disagreement is recorded. A later Insights call returned `ok: false` with zero worktrees scanned (`git worktree deferred under load`); that scan was not treated as clean. A retry after the gitdir review succeeded: 3 worktrees, overlapping files 0, unscanned 0, sessions ok, Claude idle on main. The live generation before wave 6 is 4473. GitPulse still reports `analyzer_freshness: false`; DevMap status on that generation reports `analyzer_freshness: true`, `is_fresh: true`, and `degraded_reason: null`. The wave-6 impact gap is `GAP-WAVE6-IMPACT-INCOMPLETE`. After wave 7, Insights generation is 4476. GitPulse reports `analyzer_freshness: false` and `is_fresh: false`. DevMap status on that generation reports `analyzer_freshness: true`, `is_fresh: true`, and `degraded_reason: null`. Collisions scanned 3 worktrees with overlapping files 0, unscanned 0, and sessions ok. Claude pid 3121 is still busy on main. The wave-7 impact gap is `GAP-WAVE7-IMPACT-INCOMPLETE`. Blast stays unknown.

Updated 2026-10-14 after wave 8. Insights and DevMap status were re-run on `/Users/bharath/Code/devtools/DevCouncil`. Insights facets for worktrees, agents, changes, collisions, and the ledger all returned `ok`. Eight worktrees were scanned, none unscanned, none failed, `sessions_ok` true, `truncated` false. Overlapping dirty files: 1, `rust/devmap-analyze/src/liveness.rs`, shared by main and `gitpulse/l-rust-devmap-rust-accuracy-to-its-core-e7f8ff3f`. Claude pid 6756 is busy in that worktree with 14 dirty files, including `rust/devmap-resolve/src/resolver.rs`. Claude pids 57016 and 56829 are idle on their own worktrees. `lane/workbench` has 28 dirty files. `lane/policy` has 1. Main has 33 dirty files, 1 staged. No board task is `in_progress`. Codeintel generation 4481: GitPulse reports `analyzer_freshness: false` and `is_fresh: false` because the stored extraction payload is obsolete. DevMap status on the same generation and store reports `repository.root` as this checkout, `analyzer_freshness: true`, `is_fresh: true`, `degraded_reason: null`, 74784 edges, and 19981 nodes. That disagreement stands. The wave-8 impact gap is `GAP-WAVE8-BLAST-INCOMPLETE`. The four wave-8 worktrees are isolated git directories under `~/.grok/worktrees/devtools-devcouncil/` and are absent from this Insights list. Their commits are not objects in the main checkout. Nothing in this report is merged or pushed.

## In review

Verification is null on all of these. Parent did not re-run the implementer or attacker test commands, except the actionlint checksum curl noted under CI.

| Item | Revision | Branch | What landed |
| --- | --- | --- | --- |
| `ft-0e6cfa7a04d865dcf84f08b9d54ebdc3` | 3 | `audit/schema-backup` | Pre-upgrade backup. One `BEGIN IMMEDIATE` covers the exists-check and the rename. `VACUUM INTO` runs on a second connection. The lock is released before the migration loop. `ensure_schema`'s signature is unchanged. `schema.rs` was not edited. |
| `ft-9bf9409d9f443e6d8390fac89d131790` | 3 | `audit/dcrelease-join` | Release artifact join containment. |
| `ft-54fcf95eeebb55a556c36a345545a480` | 3 | `audit/hook-quoting` | Hook command quoting. |
| `ft-81228493b280cc133be5e05d932f9f36` | 3 | `audit/ci-shell` | `build-local-artifacts` installs cargo-dist with the fixed host installer under `shell: bash`. It does not download the Linux `cargo-dist-cache` binary. Checker commit `051d16d6` closes YAML-escape and `<(curl` holes in `scripts/check-workflows.mjs` only. |
| `ft-87363ce548f556bd9002826e46a0b574` | 3 | `audit/syspath-veto` | A `Partial` parse drops `sys.path` events at or after the first error byte. Fixer `e16d03cf`, tests `06dfaa14`. `EXTRACTION_SCHEMA_VERSION` stays `"91"` (cache identity, not a store migration). `resolver.rs` was not edited. |
| `ft-6fb77a6fdff742b5eddc4536caa03efa` | 4 | `audit/brief-newline` | Stays in review. `c93af111` and `32309582` are not objects in this checkout. On main `e7a70d20`, `Input::text` still rejects only oversize and NUL. A SQL-planted line break in `title`, `kind`, `status`, `severity`, or `owner` still exports, and the brief contains a line `## Owner decisions`. That was reproduced on `audit2/brief-headings` and left unpatched there. This session logged that finding. Description, acceptance criteria, and logs still accept those breaks. Not done. |
| `ft-9450b451787bf1445d8c1794a9d452d3` | 3 | `audit/generation-race` | `list_generation_paths`, `search_page_in`, and `unresolved_sites_keyed` share one `BEGIN DEFERRED` snapshot. Implementer `adbba949`, tests `75dc7daa`. A prune between the check and the select still returns the pre-prune rows. `search_page` still uses `latest_snapshot` only. |
| `ft-b1a225db8c59b2623242237b2cc20f3f` | 5 | `audit/gitdir-containment` | Outside git dir accepted only when the back-pointer matches and the parent component is `worktrees` or `modules`. Repair `95961c6e`, adversarial tests `bb7542a2` (26 passed, attacker-reported). Parent read `git_metadata.rs` and the new tests. `check-pin.sh` stayed out of both commits. |
| `ft-15c57b2fd44a54546b0675e9e4675a49` | 3 | `audit/embedded-failure` | A Failed, Partial, budget-exhausted, or over-cap template expression is folded onto the outer outcome and the walk continues. Production `7264e0f5`, tests `60613f95`. Parent read `absorb_template_expressions` and the new tests. Attacker-reported: `embedded::tests` 15 passed, `embedded_scripts` 24 passed, four new tests failed on the parent commit. NUL and an already-expired deadline never reach this walk through `extract_file`. |
| `ft-0a53607fddfe1cc625b2b9e8b63704bb` | 10 | `audit/same-line-secrets` | `c`/`C` are prefix letters, so unhashed `cr` and `c` strings join. Hashed `br`/`cr`/`bcr`/`r` stay one span. The two-letter cap is unchanged. Production `715d2ecf`, tests `e9d9a371`. Parent read the match. Attacker-reported release ratios 3.8–4.9. Corpus pin 70/0/0/61. The board revision is 10 on this turn. This session did not append that extra log. |
| `ft-df74d364c3dfbdc458864d02a781741b` | 5 | `audit/gitdir-identity` | A canonical path that is not a directory is `invalid_input`, not a free checkout. Production `5ef16a46`, tests `7969cce`. Parent read `canonical_directory`. A non-Unicode path cannot be created on APFS, so that case was not run. |
| `ft-e5e19ae39c7223e32b77ec72f10c0a02` | 3 | `audit/fileset-truncated` | `--walk` sets `truncated` only when an eligible source file is omitted. An exact fit stays complete. `pub fn walk` still returns `Vec`. Production `ce0a46c8`, tests `e85e0a80`. Parent read `walk_into` and the tests. Attacker-reported: `--lib` 21 passed, `self_test` 1 passed. |
| `ft-716f2358c914c5022616a9bfe5acc6fd` | 3 | `audit/windows-uri` | A drive-letter path follows SQLite's URI steps. `C:\store\a#b.db` and `C:/store/a#b.db` become `file:/C:/store/a%23b.db?immutable=1`. Unix paths are unchanged, including repeated slashes. Production `cb67ab86`, tests `534e0e84`. Parent read `immutable_uri` and the locked strings. Attacker-reported filter: 11 passed. Windows SQLite was not executed. |
| `ft-0b23fbe1e5cb586e646edb8630668313` | 3 | `audit/bench-baseline` | Min-of-N tests for `terms_of` and `detect_communities`. Five repeats, alternating which input is timed first, finite minima, no comparison. Production functions were not edited. Implementer `5ba576eb`, adversarial tests `3dabec27`. Parent diff against main is tests only. Attacker-reported: each filtered test 1 passed. |
| `ft-090bccecd62bacb8482a9ea9d9b01797` | 3 | `audit2/hook-detach` | Refuse a hook executable whose canonical path is inside the repository, before the coalesce lock, and spawn that canonical path. Production `a96b491c`, adversarial tests `a7c41132`. Worktree `~/.grok/worktrees/devtools-devcouncil/subagent-01a13d4f-5a99-7552-9860-4ebb4bbae414`. Only `rust/devmap-cli/src/hook.rs`. Attacker-reported 16 passed. Parent read the canonical-path functions and the symlink tests, and did not re-run cargo. `integrate.rs` was not edited. No benchmark: one canonicalize per detach. |
| `ft-189d1ae0a0aa53df54306a74b45fb4dc` | 4 | `audit2/workflow-supply` | Actions pinned to commits, cargo-dist installed by checksum, no curl piped to a shell, `matrix.dist_args` and `github.ref_name` kept out of `run:` bodies, pull-request jobs without contents or id-token write. Production `00ba5f73`, checker precedence `c134bc69`. Worktree `~/.grok/worktrees/devtools-devcouncil/subagent-01a13d4f-5a99-7552-9860-4ee716eb6318`. Parent verified the action SHAs and the cargo-dist sha256 sums. Agent-reported `node --test` 27 passed and `npm run check:workflows` clean. Parent read `excludesPullRequest` and could not re-run node (the worktree path was refused). `rust.yml` still has `actions: write`. `dist-workspace.toml` `allow-dirty = ["ci"]` was not removed. Checker residuals are `ft-c6f4b6ff`, not more scope on this card. |
| `ft-db86455610fc3f41cd0d214dbace9fd2` | 3 | `audit2/brief-headings` | Export returns `invalid_input` and writes no brief when a label, locked field, checklist text, linked title, linked id, linked status, repository name, or workspace name contains CR, LF, U+2028, or U+2029. Production `d64b4b88`, linked-status send-back `a700b640`. Worktree `~/.grok/worktrees/devtools-devcouncil/subagent-01a13d4f-5a99-7552-9860-4ec019668c49`. Implementer-reported `workbench_briefs` 19 passed. Parent read `links_section` and did not re-run cargo. Description, criteria, and logs keep breaks. U+000B, U+000C, and U+0085 are not rejected. The five header keys stay on `ft-6fb77a6f`. `input.rs` and `schema.rs` were not edited. |
| `ft-fd75b294eca3a1dc489d38c489b0cfa1` | 4 | `audit2/dcgrep-reparse` | Unix open stays `O_NOFOLLOW \| O_NONBLOCK`. Windows open uses `FILE_FLAG_OPEN_REPARSE_POINT` and refuses `FILE_ATTRIBUTE_REPARSE_POINT`. Each file's read deadline starts when that read is armed. Default 10s via `search_with`, not a JSON field. `Duration::ZERO` is unreadable before the reader. A later file is not expired by an earlier file. `unsafe_code` is deny, with one allow beside the interrupt. Tip `7655b5e2` (after `adcd4a9d` and the rejected search-wide cap `67fc4c52`). Worktree `~/.grok/worktrees/devtools-devcouncil/subagent-01a13d4f-5a99-7552-9860-4ed55f40c00c`. Implementer-reported `--lib` 120 passed. Parent read `already_elapsed` and `arm`, and did not re-run cargo. Windows tests were not executed (`ml64.exe` missing). `secret_paths.rs` was not edited. |
| `ft-c6f4b6ff009537ea5b97014a5b2ad9b1` | 3 | `audit2/workflow-checker-bypasses` | Indirect curl shells, bracketed `matrix` and `github` expressions, and quoted `write-all`. Tip `50402cd3` after `1c5e440a`, cut from `audit2/workflow-supply` at `c134bc69`. That supply tip was not moved. Worktree `~/.grok/worktrees/devtools-devcouncil/subagent-01a13d4f-5a99-7552-9860-4ee716eb6318`. `sh -c` and `eval` read one argument. Both `$(...)` and backticks count, including a real newline inside a quoted argument. A newline is not joined for the pipe check. `curl \| python` stays allowed. `rust.yml` `actions: write` was not removed. Implementer-reported 32 passed and `npm run check:workflows` clean for 8 files. Parent read the scanner and the tests, and did not re-run node. Second attacker: 31 attacks, 0 bypasses, production untouched. No benchmark. |

## Still open

Wave 8's four cards are in review. None are done. The review list was 58 cards, read in two pages. `ft-4966b304` stays on `lane/workbench`. The checker-bypass card is wave 9, in review at revision 3, on `audit2/workflow-checker-bypasses` at `50402cd3`. A later Insights scan, after that review move, found 9 worktrees and 0 unscanned. `resolver.rs` is now dirty in the interface-dispatch worktree, which also overlaps `liveness.rs` with main. Language cards stay ready. See deferred.

## Deferred or blocked

Not started, or left where they already were. A failed Insights scan is not treated as "no collision."

| Item | Why |
| --- | --- |
| `ft-dbde8de0b5547abe18cc7a757beeb199` | Board is already review at revision 9. Policy and gate files are dirty on main, and `lane/policy` has 1 dirty file. Owner security-impact statement is unsigned. This session did not edit those files. |
| `ft-b9ef3ccf4f898d019658aa5ca4bbd11c` | Board is already review at revision 9. `lane/workbench` holds dirty `verify/orchestrate.go`. Not reopened. |
| `ft-1ca67c6c` | Board is already review at revision 8. Same `orchestrate.go`. Not reopened. |
| `ft-4966b304` | `lane/workbench` only. No task command was added on main. |
| `ft-3018ab76` | Ready, priority 1, revision 1. GitPulse vendor pin. Not bumped from this DevCouncil session. |
| `ft-e9b14104` | Already in review at revision 5. `rust/devmap-cli/src/integrate.rs` is dirty on main. Not reopened. |
| `ft-11a3d890984b07998d8984e55dd266e0` | Already in review at revision 5. `rust/dc-glob/src/secret_paths.rs` is dirty. Not reopened. The dcgrep deadline card did not edit that file. |
| `ft-32522da4e39ec1cd35c4b311d3a9610a` | Already in review at revision 12. Ledger-only removal of speculative edges is still an owner decision. Not implemented here. |
| `ft-bccb7d54` | Already in review at revision 4. Install env stays an owner decision. This session did not add an argv or edit the GitPulse script. |
| `ft-9ac4f289` | Already in review at revision 6. Criteria 7–10 and `protocol.rs` wiring stay deferred while `protocol.rs` is dirty on main. Not reopened. |
| `ft-befc1ff0` | Ready, revision 1. `dc-regress` blast, blob, budget, and change files are dirty on main. Not started. |
| `ft-ff2153d9` | Already in review at revision 11. Filter-driver remains an owner decision. Not reopened. |
| `ft-7121a94b` | Already in review at revision 11. Verify-runner allow-list remains an owner decision. `verify/env.go` was not edited. |
| `ft-08e1753c3c6640fa9dc25a79004f5519` | Already in review at revision 6. Not reopened. `rust/devmap-analyze/src/lib.rs` is dirty on main. |
| `ft-2e510bf3fe10e5506a0533921ede9997` | Already in review at revision 7. Not reopened. |
| `ft-a43bfbc6ac1ba32e5b0e894281bd0c47` | Backlog. `liveness.rs` is dirty on main and in the busy L-Rust worktree. Not started. |
| `ft-e221de3c`, `ft-f9665721`, `ft-c666ba7c`, `ft-0720fe1d`, `ft-8ca270ad`, `ft-736b0b22`, `ft-e3f7d1b1`, `ft-2b9adeb0` | Ready. After wave 9, Insights scanned 9 worktrees, 0 unscanned. `rust/devmap-resolve/src/resolver.rs` is dirty in `gitpulse/devmap-has-no-interface-protocol-trait-d-c631ed26` (20 dirty files). `liveness.rs` overlaps that worktree and main. L-Rust is clean with idle Claude pid 6756. L-Go has idle Claude pid 69498 on a second worktree. Those lanes hold the files. Not started. |
| `ft-c6f4b6ff009537ea5b97014a5b2ad9b1` | In review at revision 3. See the review table. Not folded into `ft-189d1ae0`. |

## Benchmark deltas

No benchmark was added for workflow text, request validation, a one-shot CLI, or a once-per-upgrade backup.

| Card | Result |
| --- | --- |
| Schema backup | Once per upgrade. Not a hot path. |
| dcrelease, hooks | One canonicalize per probe, or a one-shot CLI. Not measured as a hot path. |
| CI | Workflow text. No benchmark. |
| sys.path | Existing linear cap test reported passed by the attacker. No before/after numbers. |
| Brief newlines | Request validator. No benchmark. |
| Generation race | One deferred read plus the existing select. No benchmark. |
| Redaction | Attacker release binary, not parent-rerun. Ratios 3.8–4.9, including curl-bare at 4.9. Under the 5× cap. The extra linear pass is the written justification. Unhashed `cr` was a contract break and is fixed in `715d2ecf`. |
| Embedded | Failure-path record only. The success path still parses the same expressions. No before/after benchmark. |
| Fileset truncated | No benchmark. The added walk stops at the first omitted eligible file. The model encode path was not touched. Written justification for the 5% bar: this is not the encode hot path. |
| Windows URI | No benchmark. The change is the open-path string. Unix form is unchanged. Written justification: this is not a hot loop, and a criterion harness was refused. Windows SQLite was not executed. |
| Bench baseline | No numeric delta. The tests record both sides and do not compare them. `terms_of` and `detect_communities` were not changed, so there is no before/after on a touched hot path. No criterion dependency. |
| Hook detach | No benchmark. One canonicalize per detach, then the existing spawn. Not a hot loop. |
| Brief headings | No benchmark. The check runs on brief export. Not a hot loop. |
| Workflow supply | Workflow text and a Node checker. No benchmark. |
| Checker bypasses | No benchmark. `scripts/check-workflows.mjs` scans workflow text when CI runs. Not a hot loop. The matcher was widened, not put on a request path. |
| dcgrep deadline | No committed before/after of the matcher. The attacker measured a 20-file search at 34.128917 ms on `adcd4a9d`, before the two deadline send-backs. Parent did not re-measure after `7655b5e2`. Written justification: the matcher is unchanged; the added cost is one joined watchdog thread per search, not a per-byte change. A 20-file search over a second would be a regression to report. 34 ms is not. Windows was not timed. |

Actionlint digest, parent-verified by curling the checksum file to stdout: `8aca8db96f1b94770f1b0d72b6dddcb1ebb8123cb3712530b08cc387b349a3d8  actionlint_1.7.12_linux_amd64.tar.gz`. That matches `ci.yml`.

## New tasks

| Item | Task id | Status |
| --- | --- | --- |
| `ft-090bccecd62bacb8482a9ea9d9b01797` | `audit-2026-10-13-hook-detach-containment` | review, revision 3 |
| `ft-189d1ae0a0aa53df54306a74b45fb4dc` | `audit-2026-10-13-release-shell-residuals` | review, revision 4 |
| `ft-db86455610fc3f41cd0d214dbace9fd2` | `audit-2026-10-13-brief-body-headings` | review, revision 3 |
| `ft-c6f4b6ff009537ea5b97014a5b2ad9b1` | `audit-2026-10-13-workflow-checker-bypasses` | review, revision 3 |

The brief-body card is in review. U+000B, U+000C, and U+0085 still export. The five header keys still forge a heading and stay on `ft-6fb77a6f`. The checker card covers indirect curl shells (`bash -c "$(curl …)"`, `eval "$(curl …)"`, `curl | /usr/bin/env bash`, `curl | sudo -E bash`, `source <(curl …)`, `CURL | sh`), a bracketed `matrix['dist_args']` or `github['ref_name']` in a `run` body, and a pull-request job whose permissions value is a quoted `write-all`. `curl | python` is not on this card. Workflows on `audit2/workflow-supply` must still pass `npm run check:workflows`. `rust.yml` `actions: write` stays out of that card until a check shows the pinned rust-cache job works without it. Filed with `reviewed_related` for `ft-81228493`, `ft-189d1ae0`, `ft-7cb2d308`, `ft-dbde8de0`, and `ft-b9ef3ccf`. Not folded into those cards.

## Fable's decisions

- `git -c` and `--config-env` are refused on every subcommand. No key allowlist.
- Untracked and dirty-submodule bytes are scanned. A named path that cannot be included blocks with `rigor_check_unavailable`. A blank body is never reported verified while names exist.
- CR, LF, U+2028, and U+2029 in title, kind, status, severity, and owner are rejected. They are not escaped. Description and acceptance criteria stay author body.
- Schema backup keeps the second-connection `VACUUM INTO` under one `BEGIN IMMEDIATE`. `EXCLUSIVE` stays rejected: with another connection open it did not acquire the file lock before the busy timeout.
- sys.path `Failed` with cutoff 0 can miss a real veto. That miss is recorded. The cutoff was not changed. Extraction schema stays 91. The GitPulse pin was not bumped.
- CI: the Linux `cargo-dist-cache` binary in `build-local-artifacts` was rejected. The plan job's fixed `curl \| sh` installer stays, on that one job only. The three residuals stay on `ft-189d1ae0`.
- Generation reads take a deferred snapshot, not a write lock. A truly empty generation may still be `Ok([])`. A missing generation is not an empty success.
- Gitdir: do not refuse every path outside the checkout. `start_file_watcher` watches linked-worktree metadata that lives outside the worktree root, and `ignore_rule_sources` propagates `git_metadata`'s error with `?`. Accept only Git's layouts: `<common>/worktrees/<name>` and `<super>/.git/modules/<name>`, with the private dir's `gitdir` file pointing back at this checkout's `.git`. A bare outside directory with a back-pointer and no `commondir` is not that layout and is refused. Repair `95961c6e` and tests `bb7542a2` are in review at revision 5. Not done.
- Install-env was not implemented. Passing the checkout as a new argv breaks GitPulse's public `check-schema-parity` CLI.
- Wave 6 starts only `ft-e5e19ae3` (`rust/dc-sparse-encode/src/fileset.rs`). Reparse waits on unmerged `ft-11a3d890`. PDG and LSP wait on each other: both walks are incomplete and the truncated combined answer already shows `rust/devmap-cli/src/commands/mod.rs`. `truncated` is true only when the walk omits an eligible source file. An exact fit stays complete. `pub fn walk` keeps its `Vec` return. No filesystem trait and no new walk deadline: those would change a contract this card does not authorize. That card is in review at revision 3 (`ce0a46c8`, tests `e85e0a80`). Not done.
- Wave 7 starts the Windows URI card and the baseline-bench card. `parent_dir` waits because `liveness.rs` is dirty. The Windows URI follows SQLite's own drive-letter steps (`file:/C:/...`) and does not change the Unix `file:/tmp/...` form. The benches are min-of-N tests with no speedup claim and no new dependency, because CI runs `cargo test` and the repo has no criterion. Gap `GAP-WAVE7-IMPACT-INCOMPLETE`. Both cards are in review at revision 3. Windows production `cb67ab86`, tests `534e0e84`. Bench implementer `5ba576eb`, tests `3dabec27`. Not done. The in-flight task card stays on `lane/workbench`.
- Wave 8 is the residual wave and is not in `AUDIT_REPORT.md`. It starts hook detach, workflow supply, brief body headings, and the dcgrep reparse/deadline card. Planned files do not overlap. Blast is unknown (`GAP-WAVE8-BLAST-INCOMPLETE`), not a proof that callers are disjoint. All four are in review. None are done.
- Hook detach owns a canonicalize-and-component-prefix check used only by `detach_build` and `detach_session_report`. Refuse before the lock. Spawn the canonical path (`a96b491c`), not the caller path. A sibling string prefix still detaches. A canonicalize failure is an error. The test seam that returns Ok without releasing the lock is test-only. Do not reopen the quoting card. `integrate.rs` stays untouched.
- The five header keys stay on `ft-6fb77a6f`. That reject is not in this tree. `ft-db864556` must not add it. Linked-task id, title, and status on the links line are in scope for the brief-body card because they are the same interpolated line. Relation stays the fixed SQL CASE. A deleted link interpolates the literal `deleted`.
- dcgrep deadline is per file and starts when that file's read is armed. The search-wide cap in `67fc4c52` was rejected and removed in `7655b5e2`. Do not put it back. `Duration::ZERO` does not read. Opening the file before the elapsed check is accepted. One slow file is `skipped.unreadable`, not a whole-search error. The 10s default is not a JSON field. `unsafe_code` deny plus one allow beside the interrupt is accepted. Do not add a dependency and do not widen the allow. Unix symlink and FIFO refusal stays. `secret_paths.rs` is not edited. The Windows reparse test is `cfg(windows)` and was not executed here.
- Workflow SHA pinning is a narrowing. Container rustup that fails closed, and moving `dist host` off the pull-request plan job, are accepted narrowings. Duplicate cargo-dist install script in the plan job and `build-local-artifacts` is accepted duplication. `rust.yml` `actions: write` stays until the pinned rust-cache job is shown to work without it. Checker residuals are the new ready task `ft-c6f4b6ff`, not more scope on `ft-189d1ae0`.
- Do not start a language-accuracy card or interface dispatch while Claude pid 6756 holds `resolver.rs` in the L-Rust worktree. Do not start the checker card from main. Its base is `c134bc69`. Do not mark any card done. Manvi, `dev verify`, and the full suite were not run.
- Wave 9 is only `ft-c6f4b6ff`. DevMap impact showed 31 callers, all inside the checker and its test, `truncated` false, `walk_incomplete` from corpus partial parses, so the caller list is a lower bound. The first commit `1c5e440a` missed backtick command substitution and a real newline between `bash -c` and `$(curl)`. Those are the same substitution, so they were a send-back, not a new card. `50402cd3` reads one `sh -c` or `eval` argument and accepts both spellings. Joining every newline for the pipe check stays rejected, because `curl` on one line and `| bash` on the next is a different command. `curl | python` is not a criterion and stays allowed. Do not remove `actions: write` from `rust.yml`. The card is in review at revision 3. Opus approved. Not done.
- After wave 9, do not start a language card or interface dispatch. The interface worktree holds `resolver.rs` and overlaps `liveness.rs` with main. Idle Claude sessions sit on the L-Rust and L-Go worktrees. The cross-repo pin stays unbumped. No further wave was opened.

## Owner decisions still pending

- `ft-dbde8de0`: sign the security-impact statement. The change only widens refusals.
- `ft-ff2153d9`: filter-driver decision.
- `ft-32522da4`: ledger-only removes speculative edges from the graph GitPulse reads (10.8% of Kotlin call edges, 1,278 Swift, 410 TS). This audit did not decide it. The card is already in review from earlier work.
- `ft-7121a94b`: verify-runner allow-list. Home stays readable. Names such as `DATABASE_URL` still pass the regex. That is not the install-env scrub.
- `ft-bccb7d54`: how the schema-parity child learns the DevCouncil checkout. `scripts/check-schema-parity.mjs` in the GitPulse repo reads it only from `GITPULSE_DEVCOUNCIL_ROOT`. `parseArgs` accepts `--json`, `--app`, and `--timeout`. Any other argument exits 2. `--app` is the GitPulse.app bundle, not the checkout. A scrubbed child environment that still sets `GITPULSE_DEVCOUNCIL_ROOT`, or a `--devcouncil` flag landed in a GitPulse-rooted session, is the owner's choice. This session did not edit that script or `verify/env.go`.

## Pending on every review card

Manvi / `dev verify`, and the full suite. No card moves to done until that evidence exists.

## Wave 10 — language accuracy (2026-10-15)

No card moved to review or done. No production file changed. Benchmark delta: none, because no hot path was touched. No new GitPulse task. Manvi, `dev verify`, and the full suite were not run. The board has no `in_progress` task.

### Deferred (still ready, revision 4)

| Card | Why it stays ready |
| --- | --- |
| `ft-c666ba7c` L-Python | `known_failing.tsv` still lists only `py.C23b` (honest miss; the row note accepts that). Binding the untyped pytest fixture return goes through `resolver.rs` and `treesitter.rs`, both dirty in the interface worktree. |
| `ft-0720fe1d` L-Kotlin | Fixture gate is 61/61, precision 1.000 (58/58). ChronosFlow Kotlin recall target 0.850 is unmeasured in `record.json` `open_gates`. The brief names `resolver.rs`. |
| `ft-8ca270ad` L-Swift | Fixture gate is 79/79, precision 1.000 (79/79). ChronosFlow and DevType Swift recall target 0.900 is unmeasured. The brief names `resolver.rs`. |
| `ft-736b0b22` L-TS | Fixture gate is 71/71, precision 1.000 (70/70). scholarlm TypeScript recall target 0.850 is unmeasured. The interface worktree has dirty `rust/testdata/accuracy/ts/edges.tsv` and `expected.tsv`. |

### Left in review

| Card | Revision | Why it was not reopened |
| --- | --- | --- |
| `ft-e221de3c` L-Go | 7 | Already in review. Worktree `l-go-devmap-go-accuracy-to-its-core-rece-c75b63d0` is dirty (11 files, no live session): `rust/devmap-analyze/tests/real_corpus_accuracy.rs`, `rust/testdata/accuracy/README.md`, `.github/workflows/ci.yml`, `.github/dependabot.yml`, `scripts/ci-local.mjs`, untracked `rust/testdata/accuracy/go.labels.json` and `tools/accuracy/**`. scholarlm Go recall target 0.950 is unmeasured. |
| `ft-f9665721` L-Rust | 6 | Already in review. Remaining rows: `rs.c04` miss, `rs.c24b` silent_miss, `rs.c26e` silent_miss. Claude pid 6756 is idle on the clean L-Rust worktree. A second implementation was not started. GitPulse Rust recall targets 0.950 (free/self) and 0.750 (non-self) are unmeasured. |
| `ft-4cd88dbe` accuracy harness | 8 | Overlaps the dirty L-Go corpus runner and accuracy README. |
| `ft-e3f7d1b1` interface dispatch | 7 | That worktree holds the files a language card would edit. 20 dirty files, no live session. |
| `ft-32522da4` S2 | 12 | The ledger-only speculative-edge decision is still an owner decision. |

### Holds this turn

`gitpulse_insights` and `gitpulse_collision_risk` both returned `ok: true`. Nine worktrees scanned, zero unscanned, `sessions_ok` true, `truncated` false. One overlapping dirty file: `rust/devmap-analyze/src/liveness.rs` on main and on `gitpulse/devmap-has-no-interface-protocol-trait-d-c631ed26`. `shared_worktree_files` is 0.

Interface worktree dirty paths include `rust/devmap-resolve/src/resolver.rs` (+131/−17), `rust/devmap-extract/src/treesitter.rs`, `rust/devmap-analyze/src/liveness.rs`, `heritage.rs`, `languages.rs`, `cache.rs`, `rust/devmap-query/src/engine/traversal.rs`, `rust/devmap-query/src/model.rs`, `docs/devmap/DIVERGENCES.md`, the two TypeScript accuracy tables, plus untracked `typescript_interface_dispatch.rs` and `unresolved_member.rs`.

Idle Claude sessions: pid 6756 on the clean L-Rust worktree, pid 57016 on `.claude/worktrees/cranky-pare-0866b9` (clean), pid 56829 on `.claude/worktrees/silly-morse-7c6a7b` (clean). `lane/workbench` has 28 dirty files. `lane/policy` has 1. Main has 34 dirty files. The second L-Go worktree (`…-79a53535`) is clean.

### Fixture evidence (verified this turn)

```
cargo test --manifest-path rust/Cargo.toml -p devmap-analyze --test accuracy_fixtures -- --nocapture --test-threads=8 the_fixtures_meet_the_gates_and_the_ratchet_only_tightens
```

Exit 0. Binary `rust/target/debug/deps/accuracy_fixtures-89661c80059043aa`. Finished in 0.42s.

| Language | Present | Recall | Precision at 0.9 | Honest-miss |
| --- | --- | --- | --- | --- |
| go | 51/51 | 1.000 | 1.000 (50/50) | n/a (0/0) |
| kotlin | 61/61 | 1.000 | 1.000 (58/58) | n/a (0/0) |
| python | 48/49 | 0.980 | 1.000 (39/39) | 1.000 (1/1) |
| rust | 110/113 | 0.973 | 1.000 (108/108) | 0.333 (1/3) |
| swift | 79/79 | 1.000 | 1.000 (79/79) | n/a (0/0) |
| typescript | 71/71 | 1.000 | 1.000 (70/70) | n/a (0/0) |

`known_failing.tsv` still has those four rows. The ratchet was not blessed. Python's one miss is the honest `py.C23b` row. Rust's honest-miss 1/3 matches `rs.c04` honest and `rs.c24b` / `rs.c26e` silent.

### Corpus

`record.json` `open_gates` still marks every pinned corpus stale, measured at DevMap `40b6bbbf8319d8bcab0f6d93abfc664206dfea7b` (2026-10-10), and not shown to include the 8 required commits. Every language recall target, honest-miss rate, confident-edge precision, UniqueGlobal precision, and 0.9-tier dead false-positive rate is unmeasured because there are no labels. AmbiguousGlobal counts on that stale measurement: ChronosFlow Kotlin 1222, ChronosFlow Swift 387, DevType Swift 1278, GitPulse Rust 425, Lappi-decision Python 395, scholarlm Go 13, scholarlm TypeScript 832. The ignored corpus runner was not run. A green fixture gate does not meet a corpus floor.

### Blast

GitPulse codeintel this turn: generation 4490, `analyzer_freshness` false, `is_fresh` false, reason "stored extraction payload is obsolete". The saved `devmap_impact` on `resolver.rs` (depth 2) stopped at depth 2 (`walk_incomplete`), with layers truncated (shown 0 of 2), source freshness unchecked, 1 file failed to parse, 4 recovered by pattern, 6 files with ERROR/MISSING regions, and an unresolved-ledger check of 512 of 1614 keys. That answer is a lower bound. `devmap_affected_tests` was not called. Blast is unknown.

### Fable

- Leave L-Python, L-Kotlin, L-Swift, and L-TS ready while the interface worktree holds `resolver.rs`, `treesitter.rs`, `liveness.rs`, or the TypeScript accuracy tables.
- A fixture row that already passes does not move its card to review while `open_gates` still lists the corpus recall target as unmeasured. A card is not sliced into a partial that cannot meet its criteria.
- `py.C23b` stays on `ft-c666ba7c`. `rs.c04`, `rs.c24b`, and `rs.c26e` stay on `ft-f9665721`.
- Leave L-Go, L-Rust, the harness, interface dispatch, and S2 in review. Their dirty files stay untouched.
- No card is done. Nothing was merged, force-pushed, or pin-bumped. `ft-3018ab76` stays ready.
- Owner decisions above are unchanged: `ft-dbde8de0`, `ft-ff2153d9`, `ft-32522da4`, `ft-7121a94b`, `ft-bccb7d54`.

Pending on every review card is still Manvi, `dev verify`, and the full suite.

## Wave 11 — language-accuracy stress (2026-10-15)

No card moved. No production file changed. Benchmark delta: none. No new GitPulse task. The new test is uncommitted on main: `rust/devmap-analyze/tests/accuracy_language_stress.rs`. That path was free. `resolver.rs`, `treesitter.rs`, `liveness.rs`, `languages.rs`, the TypeScript accuracy tables, `real_corpus_accuracy.rs`, and `rust/testdata/accuracy/README.md` were not edited.

Insights and collision were re-read before the edit. Both `ok: true`, 9 worktrees, 0 unscanned. The only overlapping dirty file is still `rust/devmap-analyze/src/liveness.rs`. DevMap `devmap_status` reports root `/Users/bharath/Code/devtools/DevCouncil`, generation 4491, `is_fresh` true, `degraded_reason` null. GitPulse codeintel on the same store reports `analyzer_freshness` false (payload obsolete). Impact on `rust/devmap-analyze/tests/common/mod.rs` returned `truncated` false, shown 45 of 45, with source freshness unchecked, so that caller list is a lower bound. The new test only calls `common::build`. It does not change that file.

The test runs the fixture path (`scan_tree` → extract → resolve → analyse) on hostile trees. The first run failed two tests. The failures that are real misses stayed as assertions. The ones that already match a pinned contract were asserted as that contract.

Verified, after the assertion fix, `cargo test --manifest-path rust/Cargo.toml -p devmap-analyze --test accuracy_language_stress --test accuracy_fixtures -- --test-threads=8`:

- `accuracy_language_stress`: 9 passed, 0 failed, 0.20s. Binary `accuracy_language_stress-f0f2b85c28799e40`.
- `accuracy_fixtures`: 12 passed, 0 failed, 1.60s, including `the_fixtures_meet_the_gates_and_the_ratchet_only_tightens`.

What the stress run holds:

- A high edge from a same-file call stays on the local namesake for Go (bare and method), Rust (bare), and Python (bare and typed method). A cyrillic lookalike of `Latin` takes no high edge. A 64-deep Go nest and a 200-function chain stay on the local file. Two parallel builds match the serial digest.
- Rust, TypeScript, Swift, and Kotlin typed method calls (`box.ping()` beside a second `Box.ping`) produce no high edge to the foreign file. The ledger row is `UninferredReceiver` on `ping`. The test accepts a later local high edge. A silent miss or a foreign high edge fails it. Those misses stay on `ft-f9665721`, `ft-736b0b22`, `ft-8ca270ad`, and `ft-0720fe1d`. Python's typed method did bind locally, so `py.C23b` remains the untyped pytest fixture, not this shape.
- An invalid UTF-8 sibling, a NUL, a syntax wreck, and a file past `MAX_SOURCE_BYTES` leave the readable Go call resolved. The UTF-8 failure is `Unreadable`. The oversized file is `over_limit`. A symlink out of the tree is `EscapesRoot` and contributes no symbol. A gitignored sibling is absent. A missing root is an error. An empty root is an empty graph.
- `naïve/local.go` (CRLF), `odd/my file.go`, and `pkg/.hidden.go` resolve. A root `.hidden.go` is recorded `NonSource` and is not in the graph. That matches `is_ignored_path` in `languages.rs`, which the interface worktree already holds.

`scan_tree` and `Resolver::resolve_all` take no cancel token. The wedge check is a 20s budget per build. This run was on macOS. Windows and Linux were not executed. Manvi, `dev verify`, and the full suite were not run.

### Fable

- Leave the language cards, the harness, interface dispatch, and S2 where wave 10 left them. The method misses are those cards, and the fix is in `resolver.rs`, which the interface worktree holds.
- The stress file is a fence on the product path. It does not meet a corpus recall target, so it does not move a card to review.
- A root dotfile staying `NonSource` is the existing `languages.rs` rule. Do not edit that file while the interface worktree holds it.
- No card is done. Nothing was merged or pin-bumped. Owner decisions are unchanged.

## Wave 12 — merge pass (2026-10-15)

No card moved to review or done. No branch was merged. Nothing was pushed, deleted, or pin-bumped. `main` is still `e7a70d20`. The full classification is in `BRANCH_REPORT.md` under "2026-10-15 pass 3".

Insights and collision both `ok: true`: 9 worktrees, 0 unscanned, 0 in-progress tasks. The only overlapping dirty file is `rust/devmap-analyze/src/liveness.rs` (main and the interface worktree). Two Claude sessions are idle on already-merged branches. DevMap status generation 4500 says `is_fresh` true. GitPulse codeintel on that same generation says `is_fresh` false. Gap `GAP-BRANCH-PASS3-FRESHNESS`. Blast was not used to order a merge.

What was checked and left:

- `gitpulse/l-rust-devmap-rust-accuracy-to-its-core-e7f8ff3f` is 0 behind and 2 ahead (`d7b83bdf`, `fc63f951`), worktree clean, `git merge-tree` clean (tree `fab392b3`). It stays. A merge would overwrite main's uncommitted `liveness.rs`, and the interface worktree holds uncommitted edits to `resolver.rs`, `treesitter.rs`, `liveness.rs`, `cache.rs`, `DIVERGENCES.md`, `embedded_scripts.rs`, and `extract_bindings.rs`. The branch deletes `rs.c04`, `rs.c24b`, and `rs.c26e` from `known_failing.tsv`. Those rows stay on `ft-f9665721` until that branch can land without mixing those dirty trees. The commits were not tested in this pass.
- `lane/workbench` (`765db331`) stays. Schema 10 / workbench 14, 62 behind, 28 dirty files. `git merge-tree` is clean (tree `18cdc96b`) and that is not permission to merge. The schema stop still applies.
- `audit2/disk-writes` (`bac12dcd`), `audit2/store-lifecycle` (`6897dc9d`), and `audit2/daemon-backslash` (`d9154f29`) are ancestors of `main`. Cherry is empty. Pass 2's "still blocked" note for those three commits is closed.
- `refs/backup/2026-10-13/audit2/manifests` (`86e41976`) is an older WIP. Its successor `fc984740` is on `main`. It was not merged again.
- The 32 review SHAs named above, from `c93af111` through `1c5e440a`, are missing objects here. No branch exists for them.
- Dirty work on the interface worktree, the L-Go worktree `…c75b63d0`, `lane/policy`, and this checkout was not committed.

`npm run ci:local` was not re-run. No merge landed, so there was no end-of-wave suite. Manvi and `dev verify` were not run. Benchmark delta: none.

### Fable

- Leave `ft-f9665721` in review. Do not merge its branch while `liveness.rs` is dirty on main or while the interface worktree holds uncommitted edits to the files that branch changes.
- Leave `lane/workbench` unmerged until the schema bump is signed off and the 28 dirty files are committed by their author or left out.
- Do not commit another session's dirty worktree to make a merge fit. Do not stash or reset `schema.rs` or `liveness.rs`.
- No card is done. `ft-3018ab76` stays ready. Owner decisions are unchanged: `ft-dbde8de0`, `ft-ff2153d9`, `ft-32522da4`, `ft-7121a94b`, `ft-bccb7d54`.

## Wave 13 — contract skip guard (2026-10-15)

`ft-2b9adeb0` moved ready (revision 6) → in progress (revision 7) → review. It is not done. Nothing was merged or pushed. Branch `audit/contract-skip-guard` at `ca30d273`, worktree `/Users/bharath/Code/devtools/DevCouncil-contract-skip-guard`, one file: `.github/workflows/analysis-plane.yml` (+26). The step "Flag and state-dir contracts must have run" runs `TestDevmapGlobalFlagTablesMatchTheRealBinary` and `TestStateDirAgreesWithDevmapOnEveryRung` and fails the job on a non-zero status, a `--- SKIP` line, or a missing `--- PASS` line.

Left alone, because another session holds them: L-Python `ft-c666ba7c`, L-Kotlin `ft-0720fe1d`, L-Swift `ft-8ca270ad`, and L-TS `ft-736b0b22` moved to in progress at revision 5 during this wave. Claude pid 36664 was busy on main. Two locked Claude worktrees appeared (`worktree-agent-a2ef74cf7dd734456`, `worktree-agent-ac36dd3b8fbf0419b`). `ft-befc1ff0` stays ready because `rust/dc-regress/src/blast.rs` is dirty on main. `ft-3018ab76` stays ready: `cache.rs` is dirty in the interface worktree, and the card says the GitPulse re-vendor is a pin bump for a GitPulse session. L-Rust and `lane/workbench` stay unmerged for the wave 12 reasons.

Insights and collision at the start of the wave: both `ok: true`, 9 worktrees, 0 unscanned, one overlapping dirty file `rust/devmap-analyze/src/liveness.rs`. DevMap status generation 4502, `is_fresh` true, `repository.root` this checkout. GitPulse codeintel on that generation says `is_fresh` false. Impact on `analysis-plane.yml` returned no indexed traversal start and `source_freshness.fresh` null. Gap `GAP-WAVE13-WORKFLOW-UNINDEXED`.

Verified:

- `TestStateDirAgreesWithDevmapOnEveryRung`: PASS, 436.82s, 8 subtests, no SKIP, in the worktree. `go test` exit 1 overall because `./cmd/devcouncil` failed to link there (`_devcouncil_gusset_init` undefined; `libgusset.a` was not in that worktree).
- `TestDevmapGlobalFlagTablesMatchTheRealBinary`: PASS, 288.57s, no SKIP, on this checkout, where `rust/gusset-engine/target/release/libgusset.a` already exists. `go test -list` with the same `-run` regex printed that test name.
- A sample log containing `--- SKIP:` trips the guard's grep. A sample log of the two `--- PASS:` lines does not.

CI builds the archive in `setup-gusset` before this step. That local link failure is the fresh worktree, not the workflow. Manvi, `dev verify`, and `ci:local` were not run. Benchmark delta: none.

Still open on `ft-2b9adeb0`: a schema under `protocols/` for the dcstore, dcverify, and dcgrep stdio contracts, replayed in CI; and the Lappi record cross-check running in CI against a pinned checker, or being deleted.

### Fable

- Do not join the four language cards while they are in progress.
- Do not merge this branch in the same wave. Main is dirty, and Claude is busy in that checkout.
- The guard does not close the card. Leave it in review until the two remaining criteria are met and Manvi has verified the head.
- No card is done. Owner decisions are unchanged.
