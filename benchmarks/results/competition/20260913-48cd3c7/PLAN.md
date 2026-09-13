# DevMap build 48cd3c7 benchmark rerun

Task `52d63a1a-266d-4a5a-8a09-9b8458a98334`. Rerun the competitor comparison
against the DevMap executable that is actually installed now, and retain the
documented positives, negatives, and comparison limits. Run timestamps use UTC.

## Why this rerun exists

The [previous report](../20260913-v0.2.1/REPORT.md) measured DevMap build
`506a617`. The installed executable has since advanced 49 commits to build
`48cd3c7` (`git rev-list --count 506a617..48cd3c7`), changing 38 files under
`rust/` including `devmap-extract/src/frameworks.rs`, `devmap-query/src/freshness.rs`,
and a new `devmap-query/src/stat_memo.rs`. Both binaries report version 0.2.1,
so they are distinguished by build id throughout. `prepare_rerun.py` refuses to
run if the installed binary's SHA-256 matches the one the previous report
already measured.

## Protocol

Use the installed clean-build DevMap binary, preserve a private copy and its
SHA-256, and record its source commit. Reuse the prior verified Graphify
0.9.59, Gortex 0.64.3, GitNexus 1.6.9, CodeGraph 1.6.0,
codebase-memory-mcp 0.10.8, and ripgrep 15.1.0 installations, asserting their
recorded binary hashes before use. No dependency installation, global
registration, or hosted inference is needed.

All six graph tools receive new clones of the original 1,186-file DevCouncil
commit `ee07c183b127e7291c38f993f4d71ca75f1121dc`. Verify every tracked file
against the prior manifest before and after. The corpus commit deliberately
differs from the binary's source commit, so the workload is held fixed while
the binary changes. Each tool receives fresh local index and configuration
state. Gortex uses a separately owned local daemon on its own socket.

Keep the previous operation flags, three cold runs, five unchanged refreshes,
three single-file edits, five query repetitions for three symbols, five
source-inspected caller pairs, update/removal/absence controls, and the
ripgrep baseline. Record failures and incomplete answers as findings.
Graph tools run serially; Gortex runs after the standalone-tool campaign.
Do not flush shared OS/compiler/package caches or interrupt other tasks.

The comparison with the previous report's campaign is historical: different
shared-desktop load and warmed caches prevent attributing every timing
difference to the build change. The alternating control below is the
better-controlled comparison.

After the main campaign, `run_version_control.py` adds 82 supplemental timing
samples alternating the preserved `506a617` and `48cd3c7` binaries, with
independent stores on the same restored corpus. Those samples and their
controls remain separate from the 261-sample competitor table.

No broad graph-accuracy, persistent-MCP, coding-agent-success, or product-test
qualification follows from this small fixed workload.

## Execution order

`prepare_rerun.py` creates the private corpus/state and records reused script
hashes. Run `run_expanded.py`, `run_gortex.py`, `final_checks.py`,
`gortex_followup.py`, and `run_version_control.py` serially, then `summarize.py`
and `audit_results.py`. The final checks record whether the prior GitNexus
freshness failure recurs; the audit does not require an old competitor failure
to recur. Generate the report with `write_report.py`. Existing result
directories must be preserved; the preparation and cold-run scripts refuse
reuse of existing scratch state.
