# Integration-test consolidation: build time, executables and size

Measured 2026-10-10 on macOS arm64 (18 cores, shared with other agents' builds,
so absolute seconds are noisy and only the interleaved minimum is compared),
Rust 1.99.0, debug profile, `cargo test --locked --no-run`.

- **A** is `c379182d`: one integration-test target per file under `tests/`.
- **B** is `216adc5e`: one target per crate (`tests/it.rs`) plus seven files
  that need a process of their own.
- Crates: `devmap-query`, `devmap-cli`, `devmap-extract`, `devmap-resolve`.
- Method: `rust/tools/test_link_ab.sh`. Each run first `cargo clean -p`s the four
  crates (third-party dependencies stay built) and then builds their test
  targets. Runs alternate A, B, A, B, A, B, each tree with a private
  `CARGO_TARGET_DIR`. Wall, user and sys seconds come from `/usr/bin/time -p`;
  executables and bytes from cargo's JSON `compiler-artifact` messages for
  those packages. `cargo --timings` was not used.

| run | tree | wall s | user s | sys s | executables | bytes |
|---|---|---|---|---|---|---|
| 1 | A | 40.95 | 200.66 | 67.90 | 276 | 14256878280 |
| 1 | B | 25.15 | 133.74 | 20.32 | 28 | 1534167440 |
| 2 | A | 32.76 | 198.93 | 70.44 | 276 | 14256878280 |
| 2 | B | 17.24 | 84.59 | 15.49 | 28 | 1534167440 |
| 3 | A | 31.27 | 185.25 | 68.46 | 276 | 14256878280 |
| 3 | B | 14.91 | 82.29 | 14.42 | 28 | 1534167440 |

| minimum of 3 | A | B | change |
|---|---|---|---|
| wall s | 31.27 | 14.91 | -52% |
| user s | 185.25 | 82.29 | -56% |
| sys s | 67.90 | 14.42 | -79% |
| test executables | 276 | 28 | -90% |
| executable bytes | 14.26 GB | 1.53 GB | -89% |

Collected tests per crate, from `cargo test -p <crate> -- --list`, equal before
and after except for one new guard test per crate
(`every_test_file_is_a_module_or_isolated_and_declared`): devmap-resolve 588,
devmap-query 707, devmap-cli 736, devmap-extract 884 before.

What this does not show: whole-workspace `--all-targets` time (the other
crates, `devmap-store`, `devmap-analyze` and `devmap-serve`, were not
consolidated), CI wall time (not measured; the saving is mostly CPU and disk, so
a 2- or 4-core runner gains more than this 18-core machine), and run time of
the tests, which is unchanged in kind: one process runs a crate's tests with
libtest's thread pool instead of one process per file.

## Files that keep a process of their own

| crate | file | why |
|---|---|---|
| devmap-query | `a_cold_query_pays_for_its_answer_not_the_generation`, `a_delete_this_list_reads_for_its_answer`, `a_fan_out_pays_for_its_index_once_per_direction`, `query_work_is_bounded_by_the_answer` | each installs a counting `#[global_allocator]`, and counts every allocation in the process |
| devmap-query | `artifact_writer_security` | assumes the process has made no earlier atomic write (it plants the temporary names the writer will pick) |
| devmap-extract | `global_excludes_are_one_rule` | sets `GIT_CONFIG_GLOBAL` in the process environment |
| devmap-extract | `a_deadline_reaches_the_children_the_child_started` | counts the threads of the process |
| devmap-cli | `global_excludes_bind_the_daemon` | sets `GIT_CONFIG_GLOBAL` in the process environment |
| devmap-cli | `test_time_scale` | sets `DEVMAP_TEST_TIME_SCALE` in the process environment |

## Adding a test

Add `mod <file>;` to the crate's `tests/it.rs` (with
`#[cfg(feature = "parse")]` above it if it needs a grammar). A file that is
neither declared there nor listed in `ISOLATED` fails
`every_test_file_is_a_module_or_isolated_and_declared`; with `autotests =
false` it would otherwise never run.
