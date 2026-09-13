"""Render this run's report from retained measurements and verification evidence."""
import json
from pathlib import Path
import re
import statistics

out=Path(__file__).resolve().parent
meta=json.loads((out/'provenance.json').read_text())
summary=json.loads((out/'summary.json').read_text())
verify=json.loads((out/'verification.json').read_text())
audit=json.loads((out/'audit.json').read_text())
log=(out/'verify.log').read_text()
results=re.findall(r'test result: (\w+)\. (\d+) passed; (\d+) failed; (\d+) ignored',log)
passed=sum(int(x[1]) for x in results);failed=sum(int(x[2]) for x in results);ignored=sum(int(x[3]) for x in results)
def row(stage,tool,target=''): return next(s for s in summary if (s['stage'],s['tool'],s['target'])==(stage,tool,target))
def seconds(x): return f'{x:.3f} s' if x>=1 else f'{x*1000:.1f} ms'
lines=[
'# DevMap competitive benchmark — 2026-09-12',
'',
'Task `52d63a1a-266d-4a5a-8a09-9b8458a98334`, revision 3. Primary repository: DevCouncil. The brief supplied no acceptance criteria or competitor list. This run compares the installed GitNexus graph indexer and ripgrep text-search baseline on DevCouncil only.',
'',
'**Verified result:** on this fixed corpus, DevMap had lower cold-index, unchanged-refresh, one-file-update, context, and direct-impact CLI latency than GitNexus. These are native pipeline measurements with different indexing and output semantics. They do not establish equivalent-work parser speed, whole-repository accuracy, or persistent MCP latency.',
'',
'**Indexing latency.** Medians with observed min–max; three cold and edit repetitions, five unchanged repetitions. Speed ratios divide GitNexus median by DevMap median.',
'',
'| Operation | DevMap median (min–max) | GitNexus median (min–max) | Ratio |',
'|---|---:|---:|---:|',
]
for stage,label in [('cold','Cold index'),('warm','Unchanged refresh'),('edit','One tracked file edited')]:
    a=row(stage,'devmap');b=row(stage,'gitnexus')
    lines.append(f"| {label} | {seconds(a['median_s'])} ({seconds(a['min_s'])}–{seconds(a['max_s'])}) | {seconds(b['median_s'])} ({seconds(b['min_s'])}–{seconds(b['max_s'])}) | {b['median_s']/a['median_s']:.2f}× |")
lines += ['', '**Memory.** Median OS-reported peak RSS per invocation, in MiB (2²⁰ bytes). This uses the repository’s `rust/tools/peak_rss.sh` helper. It is not a sampled sum of all concurrently live processes in a worker tree.', '', '| Operation | DevMap | GitNexus |','|---|---:|---:|']
for stage in ['cold','warm','edit']:
    lines.append(f"| {stage} | {row(stage,'devmap')['median_rss_bytes']/1048576:.1f} | {row(stage,'gitnexus')['median_rss_bytes']/1048576:.1f} |")
storage=[json.loads(x) for x in (out/'storage.jsonl').read_text().splitlines()]
lines += ['', '**Storage.** Median cold index: '+', '.join(f"{tool} {statistics.median(r['bytes'] for r in storage if r['stage']=='cold' and r['tool']==tool)/1048576:.1f} MiB" for tool in ['devmap','gitnexus'])+'. After the third edit: '+', '.join(f"{tool} {next(r['bytes'] for r in storage if r['stage']=='edit' and r['iteration']==3 and r['tool']==tool)/1048576:.1f} MiB" for tool in ['devmap','gitnexus'])+'. DevMap is the SQLite file; GitNexus includes its full `.gitnexus` directory and caches. Retained cold-run backups are excluded. These are logical file bytes, not allocated disk blocks.', '', '**Query latency.** Five repetitions per cell. `context` means DevMap `explore --budget 8000` and GitNexus `context --content -l 100`; `impact` uses depth 1, with test inclusion explicitly enabled in GitNexus. Ripgrep searches literal occurrences and provides no graph traversal.', '', '| Target | DevMap context | GitNexus context | DevMap impact | GitNexus impact | ripgrep text search |','|---|---:|---:|---:|---:|---:|']
for target in ['db_size_gate_bytes','loadBounded','describe_corpus']:
    metrics=[seconds(row(stage,tool,target)['median_s']) for stage,tool in [('context','devmap'),('context','gitnexus'),('impact','devmap'),('impact','gitnexus'),('search','ripgrep')]]
    lines.append('| `'+target+'` | '+' | '.join(metrics)+' |')
lines += [
'',
'Context answers include different amounts of source and graph data, so the query columns are task-oriented observations, not equal-output microbenchmarks. GitNexus’s zero-caller Rust impact response is also a correctness miss; its fast empty answer must not be interpreted as successful traversal. Full sample spreads and RSS are in [summary.json](summary.json).',
'',
'**Answer checks.** Both tools found the exact definition and expected file for all three named symbols in all five context repetitions. Source inspection established five distinct direct caller pairs: two Rust test callers, two Go callers, and one Python caller. DevMap returned 5/5; GitNexus returned 3/5, missing both Rust test callers. Results were consistent across five repetitions and between context and impact. Neither returned an extra `Calls` pair for these examples. This is a small, selected diagnostic set, not a representative accuracy evaluation.',
'',
'| Target | Source-inspected direct callers | DevMap | GitNexus |',
'|---|---|---:|---:|',
'| Rust `db_size_gate_bytes` | `size_gate_constants_are_their_declared_magnitudes`; `db_size_gate_scales_with_file_count_and_keeps_a_floor` | 2/2 | 0/2 |',
'| Go `loadBounded` | `Load`; `TestAnInternedGraphIsHeldToTheSameBound` | 2/2 | 2/2 |',
'| Python `describe_corpus` | `main` in `benchmarks/map_bench.py` | 1/1 | 1/1 |',
'',
'Source anchors at the measured commit: [Rust local caller](https://github.com/bharathvbcr/DevCouncil/blob/'+meta['commit']+'/rust/devmap-extract/src/model.rs#L1634), [Rust cross-file caller](https://github.com/bharathvbcr/DevCouncil/blob/'+meta['commit']+'/rust/devmap-cli/tests/test_scholarlm_findings.rs#L774), [Go production caller](https://github.com/bharathvbcr/DevCouncil/blob/'+meta['commit']+'/backend/go_orchestrator/repomap/repomap.go#L327), [Go test caller](https://github.com/bharathvbcr/DevCouncil/blob/'+meta['commit']+'/backend/go_orchestrator/repomap/compact_test.go#L544), [Python caller](https://github.com/bharathvbcr/DevCouncil/blob/'+meta['commit']+'/benchmarks/map_bench.py#L1086).',
'',
'Both tools found each new definition after three sequential edits to `benchmarks/tasks.py`, and both rejected a never-existing name. Restoring the original file exposed a separate GitNexus failure: `analyze` returned `Already up to date`, but `context devmap_competition_revision_3` still returned the deleted symbol. The actual file SHA-256 differed from the stored `fileHashes` entry. DevMap removed all three probe symbols, reported fresh source/analyzer state, and restored the same edge digest as a cold build. All 1,186 corpus file hashes matched the initial snapshot after restoration.',
'',
'Failure evidence: [restore output](restore-gitnexus.stdout), [stale symbol response](removed-gitnexus-3.stdout), [removed-symbol checks](removed-symbol-checks.json), [GitNexus stored hashes](gitnexus-index-meta.json), and [original corpus hashes](corpus-manifest.json). DevMap evidence: [final status](devmap-final-status.stdout), [cold/restored edge digest](devmap-restore-digest.json). The unchanged-refresh timing series ran before these edits, when both indexes matched the corpus; the failed restore is not included in that series.',
'',
'A second normal GitNexus analyze repeated the stale response. A forced rebuild cleared the deleted symbol and restored the original 16,284-node / 47,123-edge counts. This recovery was an untimed diagnostic outside the comparative samples; see [recovery commands and responses](gitnexus-recovery.json).',
'',
'**Coverage and interpretation limits.** DevMap indexed 857 files, 10,193 symbols and 33,789 edges. GitNexus reported 1,062 inventoried files, 16,284 nodes and 47,123 edges in cold runs, plus communities and execution flows; its node taxonomy and file inventory differ. The recorded file inventories intersect at 824 paths. Those totals cannot be treated as a recall comparison. GitNexus reported one unavailable COBOL parser. DevMap reported eight SQL import-extraction gaps and one PowerShell file recovered by patterns.',
'',
'DevMap also reported 23,011 resolved call sites and 162,590 unresolved sites, of which 70,806 were explained as builtin/runtime/external sites. Its reported net resolution rate was 20.0%; this is its own resolver metric, not measured recall. Its query envelopes explicitly warned of 91,784 remaining unattributed sites repository-wide. Depth limits and some context-budget truncation were present and are retained in [audit.json](audit.json). No complete blast radius or whole-repository correctness claim is made.',
'',
'**Run protocol and provenance.**',
'',
'- Corpus: committed DevCouncil `'+meta['commit']+'`; 1,186 files, 46,940,219 bytes. Two local clones of that existing repository served as isolated benchmark data. Pre-existing uncommitted work was excluded from the corpus and preserved in the original checkout.',
'- Host: Apple M5 Pro, 18 logical CPUs, 64 GiB RAM, macOS 27.0 arm64. Shared desktop host; no CPU pinning or controlled idle environment. Load averages accompany every measurement.',
'- DevMap: `'+meta['devmap_version']+'`; copied before measurement; SHA-256 `'+meta['devmap_sha256']+'`. This dirty-build identity is not sufficient to recreate its exact source patch from Git; the measured binary is retained locally at `'+meta['devmap_binary']+'`.',
'- Competitors: GitNexus '+meta['gitnexus_version']+' and '+meta['rg_version']+'. Existing installations only; no dependencies added. GitNexus used `--index-only`, an isolated `GITNEXUS_HOME`, default parser concurrency, and default embeddings-off mode. Its [installed CLI help](gitnexus-analyze-help.stdout) was authoritative for flags; upstream reference: [official CLI source](https://github.com/abhigyanpatwari/GitNexus/blob/main/gitnexus/src/cli/index.ts).',
'- Cold runs start with empty application indexes; OS file caches are not flushed. Tool order alternates across repetitions. Builds and queries run serially; repository verification started only after the comparison finished. Pilot/control/restore samples are excluded from comparative statistics.',
'- Wall time includes process launch, the common Bash/RSS wrapper and command completion. This overhead matters for the shortest queries; it is retained without subtraction. No persistent server, semantic embeddings, hosted provider, cross-platform run, synthetic corpus, or external repository was measured.',
'- Ninety-seven comparative samples across 21 groups; 115 total timed records including pilots and controls. All timed commands exited successfully and produced a positive RSS measurement. The 101 generic execution checks passed; the stronger source-derived audit separately records GitNexus’s caller and restore failures. A successful command is not equated with a correct answer.',
'',
'**Repository verification.** `bash rust/verify.sh` exited '+str(verify['exit_code'])+' after '+f"{verify['seconds']:.1f}"+' seconds. Parsed test-run summaries: '+str(passed)+' passed, '+str(failed)+' failed, '+str(ignored)+' ignored (executed test cases, not unique coverage). This gate ran against the current checkout, including pre-existing edits; it does not prove the copied dirty release binary is reproducibly built from the committed corpus. See [verification.json](verification.json) and [full gate log](verify.log).',
]
if 'ALL GATES GREEN' in log:
    lines += ['', 'Format, Clippy, workspace tests, release worker panic recovery, determinism, self-build latency/storage/RSS, memory-model probe, growth plateau, and five-cycle incremental equivalence passed. Optional mutation testing was skipped by the script; the five-cycle soak does not establish long-run growth.']
else:
    lines += ['', 'The full gate did not report success. Its actual stop point and failure output are retained in the linked log; later stages must not be treated as passed.']
lines += [
'',
'The default ignored entries were `the_emitted_bundle_passes_claude_plugin_validate_strict` (opt-in external Claude CLI validation), `a_storm_kill_restart_cycle_converges_on_the_cold_build` (opt-in minutes-long soak), and `concurrent_prune_writer_child`. The third is a subprocess helper explicitly launched with `--ignored` by the passing `a_reader_never_answers_from_a_generation_another_process_pruned` test; it is not an unexecuted coverage gap.',
'',
'Additional uncommitted source paths appeared during this task from concurrent work. The benchmark task only wrote its result directory and ignored scratch area. The gate result applies to the source observed during that run, not later concurrent edits; see [final checkout status](source-status-after.txt).',
'',
'**Reproduction and artifacts.** Re-summarize and re-audit without rerunning tools:',
'',
'```sh',
'python3 '+str(out/'summarize.py'),
'python3 '+str(out/'audit_results.py'),
'```',
'',
'Run the same comparison again with fresh output and scratch directories. The defaults reuse the retained binary and original commit; `--binary` can select a different existing build, whose actual hash/version will be recorded:',
'',
'```sh',
'python3 '+str(out/'reproduce.py')+' '+chr(92),
'  --out '+meta['repo']+'/benchmarks/results/competition/reproduction-52d63a1a '+chr(92),
'  --scratch '+meta['repo']+'/.devcouncil/benchmarks/reproduction-52d63a1a',
'```',
'',
'The reproduction command intentionally refuses existing destination paths. It runs the competitive workload and answer audit; rerun `bash rust/verify.sh` separately for the repository gate. Exact timing is hardware/load dependent, and competitors must already be installed. Results include [raw command records](measurements.jsonl), per-command stdout/stderr, [summary statistics](summary.json), [answer audit](audit.json), [provenance](provenance.json), full corpus hashes, and the task-local scripts. Product code and the retired mapping harness were not changed; no commit or push was performed.',
]
(out/'REPORT.md').write_text('\n'.join(lines)+'\n')
print(out/'REPORT.md')
