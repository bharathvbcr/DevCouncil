# Lane verification, fix/stacked-81b1372e (card ft-983bd7bc)

Run in the lane's own worktree with `CARGO_TARGET_DIR=<worktree>/rust/target`, `--locked`, Go with `-p 1`.
Commit under test: 2f85d2a5 plus the clippy fix that follows it. Not a benchmark; kept beside the seed-rebind result for the lane record.

| Suite | Result |
| --- | --- |
| `cargo test -p devmap-cli --test it` | 722 passed, 1 failed, 6 ignored |
| `cargo test -p devmap-cli --bin devmap` | 289 passed, 0 failed |
| `go test -p 1 ./devcouncil/verify/...` (testsupport binaries prebuilt first) | ok, 171.7 s |
| `go test -p 1 ./dc/devmap/...` | ok (earlier run) |
| `cargo test -p devmap-query --lib` / `--no-default-features --lib` | 465 / 350 passed |
| `cargo test -p devmap-store` | all passed (148 lib, 1 ignored, plus integration targets) |
| `cargo test -p devmap-serve --test mcp_repo_scope` / `--lib` | 30 / 206 passed |

## The one `it` failure

`every_command_is_in_the_presentation_stress::every_command_is_driven_by_the_presentation_stress_or_exempt_with_a_reason`:
"scripts/cli-presentation-stress.py does not drive ["seed"]".

Classification: pre-existing at 81b1372e, not caused by this lane's fixes. At 81b1372e the `seed` command is
declared (`Commands::Seed` in `rust/devmap-cli/src/cli.rs`), neither `scripts/cli-presentation-stress.py` nor the test's
`EXEMPT` list mentions `"seed"`, and none of those files changed between 81b1372e and HEAD. Proposed fix (not applied: it
is the owner-stop CLI surface and would add an exemption to a gate): an `EXEMPT` entry for `seed` with the reason
"needs a registered linked worktree and a built donor; covered by a_seeded_worktree_store_is_a_cold_builds_graph".

## Clippy

`cargo clippy --locked --all-targets` on devmap-query, devmap-store, devmap-cli, devmap-serve, devmap-extract, at 81b1372e
(clean `git archive` copy, own target dir) and at HEAD, warning sets compared by (file, message): 31 at 81b1372e,
30 at HEAD. New at HEAD: none. One warning came in with the lane (`large size difference between variants` on the hook's
`HookStore`) and was fixed by boxing the variant. Gone at HEAD: one `map_or` in `mcp.rs`.
