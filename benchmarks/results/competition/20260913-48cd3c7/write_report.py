"""Generate the versioned report from fresh measurements and audited answers."""
import json
import math
from pathlib import Path
import re
import statistics

OUT = Path(__file__).resolve().parent
PRIOR = OUT.parent / '20260913-v0.2.1'
TOOLS = ['devmap', 'codegraph', 'cbm', 'graphify', 'gortex', 'gitnexus']
NAMES = dict(devmap='DevMap 0.2.1 (48cd3c7)', codegraph='CodeGraph', cbm='codebase-memory-mcp',
             graphify='Graphify', gortex='Gortex', gitnexus='GitNexus', ripgrep='ripgrep')
BUILDS = ['506a617', '48cd3c7']
TARGETS = ['db_size_gate_bytes', 'loadBounded', 'describe_corpus']

def read(name):
    return (OUT / name).read_text()

def data(name):
    return json.loads(read(name))

S = data('summary.json')
A = data('audit.json')
P = data('provenance.json')
OLD = json.loads((PRIOR / 'summary.json').read_text())
RECORDS = {r['label']: r for r in map(json.loads, read('measurements.jsonl').splitlines())}
STORAGE = list(map(json.loads, read('storage.jsonl').splitlines()))

def group(tool, stage, target=None, source=S):
    return next(x for x in source if (x['tool'], x['stage'], x['target']) == (tool, stage, target))

def sec(value):
    return f'{value:.3f} s' if value >= 1 else f'{value * 1000:.1f} ms'

def speed_factor(current, reference):
    """Describe current elapsed time relative to reference, using full medians."""
    if not all(math.isfinite(value) and value > 0 for value in (current, reference)):
        raise ValueError('Speed factors require finite, positive elapsed times')
    if current == reference:
        return '1.00× (equal)'
    factor = max(current, reference) / min(current, reference)
    precision = 3 if round(factor, 2) == 1.0 else 2
    return f'{factor:.{precision}f}× ' + ('faster' if current < reference else 'slower')

def median(tool, stage, target=None):
    return group(tool, stage, target)['seconds']['median']

def mid(tool, stage, target=None):
    return sec(median(tool, stage, target))

def spread(tool, stage):
    values = group(tool, stage)['seconds']
    return f"{sec(values['median'])} ({sec(values['min'])}–{sec(values['max'])})"

def rss(tool):
    return group(tool, 'cold')['sampled_tree_peak_bytes']['median'] / 2**20

def store(tool):
    return statistics.median(x['bytes'] for x in STORAGE if x['stage'] == 'cold' and x['tool'] == tool) / 2**20

def cases(tool):
    return [x for x in A['answers'] if x['tool'] == tool]

def score(tool):
    assert all(x['consistent'] for x in cases(tool)), 'Report must describe unstable answers explicitly'
    return sum(len(x['samples'][0]['matched']) for x in cases(tool))

def edits(tool):
    values = [x['passed'] for x in A['controls'] if x['label'].startswith('validate-edit-' + tool + '-')]
    assert len(values) == 3
    return '3/3' if all(values) else f'{sum(v is True for v in values)}/3'

def removed(tool):
    if tool == 'gortex':
        return 'Yes, exact-ID follow-up' if A['supplemental']['gortex-exact-removed']['passed'] else 'No'
    row = next(x for x in A['controls'] if x['label'] == f'negative-{tool}-devmap_competition_revision_3')
    return 'Yes' if row['passed'] is True else 'No' if row['passed'] is False else 'Inconclusive'

def winner(stage):
    return NAMES[min(TOOLS, key=lambda tool: median(tool, stage))]

assert len(S) == 57 and sum(x['attempted'] for x in S) == 261
assert all(x['attempted'] == x['succeeded'] for x in S)
assert all(all(sample['exact_definition'] for sample in case['samples']) for case in A['answers'])
DM = data('cold-devmap-3.stdout')
STATUS = data('devmap-final-status.stdout')
remaining = DM['resolution_rate']['unresolved_sites'] - DM['resolution_rate']['explained_sites']
PRIOR_DM = json.loads((PRIOR / 'cold-devmap-3.stdout').read_text())
PRIOR_REMAINING = PRIOR_DM['resolution_rate']['unresolved_sites'] - PRIOR_DM['resolution_rate']['explained_sites']
GRAPH_KEYS = ['files_indexed', 'symbols', 'edges']
GRAPH_IDENTICAL = (all(DM[k] == PRIOR_DM[k] for k in GRAPH_KEYS)
                   and DM['resolution_rate'] == PRIOR_DM['resolution_rate'])
GATES = [data(f'gortex-cold-{i}-gates.json') for i in (1, 2, 3)]
ready = statistics.median(x['query_ready_seconds'] for x in GATES)
enriched = statistics.median(x['enriched_seconds'] for x in GATES)
recovery = A['supplemental']['gitnexus_force_recovery']
opt_in = A['supplemental']['gortex_include_name_only']['combined_matched_pairs']
query_winners = sum(median('devmap', stage, target) == min(median(t, stage, target) for t in TOOLS)
                    for stage in ['search', 'callers'] for target in TARGETS)
assert rss('devmap') == min(rss(tool) for tool in TOOLS)
assert store('graphify') == min(store(tool) for tool in TOOLS)

lines = ['# DevMap build 48cd3c7 competitor benchmark — 2026-09-13 UTC', '',
    'Task `52d63a1a-266d-4a5a-8a09-9b8458a98334`. This is a fresh rerun of all six graph tools and the ripgrep text baseline against the **currently installed** DevMap executable. The [previous v0.2.1 report](../20260913-v0.2.1/REPORT.md) measured build `506a617`; the installed binary has since advanced ' + str(P['commits_since_previous_report']) + ' commits to build `' + P['devmap_build']['id'] + '`, so that report no longer describes the shipping executable. Both earlier reports and their raw evidence remain unchanged. The task supplied no acceptance criteria; the prior measurement protocol defines this rerun’s scope.', '',
    f'**Verified outcome:** {winner("cold")} had the lowest cold-index median, {winner("warm")} the lowest unchanged-refresh median, and {winner("edit")} the lowest single-file edit median. DevMap returned {score("devmap")}/5 inspected caller pairs. All 261 competitive timing samples completed successfully; command completion is separate from the correctness findings below.', '',
    '**Binary and corpus.** DevMap reports `' + P['devmap_version'] + '`. Its preserved executable SHA-256 is `' + P['devmap_sha256'] + '`, and clean-build metadata identifies source commit `' + P['devmap_source_commit'] + '`. The binary was copied from the installed executable and hash-verified; it was not rebuilt or independently release-signature-qualified during this run.', '',
    'The benchmark corpus deliberately stays at commit `' + P['corpus_commit'] + '`: 1,186 tracked regular files, 46,940,219 bytes, independently hash-verified in six fresh clones before and after. Thus the binary build changed without replacing the source workload. [Provenance](provenance.json), [plan](plan.json), and [corpus manifest](corpus-manifest.json) retain the identities.', '',
    '**Positives, negatives, and comparisons.** These are observations about this workload, interfaces, and selected source cases. Different analysis scope prevents treating smaller stores or lower latency as equivalent graph quality.', '',
    '| Tool | Positives observed | Negatives and comparison limits |', '|---|---|---|',
    f'| {NAMES["devmap"]} | {score("devmap")}/5 caller pairs; {edits("devmap")} edits visible; removed probe absent: {removed("devmap")}. Lowest median in {query_winners}/6 graph-query cells and lowest sampled cold RSS, {rss("devmap"):.1f} MiB. Clean-build identity is recorded. | Edit median {mid("devmap","edit")} versus CodeGraph’s {mid("codegraph","edit")}; store {store("devmap"):.1f} MiB versus Graphify’s {store("graphify"):.1f}, CodeGraph’s {store("codegraph"):.1f}, and CBM’s {store("cbm"):.1f}. {remaining:,} unexplained attribution sites remain; the five caller pairs are not full accuracy coverage. |',
    f'| CodeGraph | Cold {mid("codegraph","cold")}, edit {mid("codegraph","edit")}, store {store("codegraph"):.1f} MiB; {edits("codegraph")} edits visible and removal check: {removed("codegraph")}. | {score("codegraph")}/5 caller pairs; missing cases are listed below. Cold sampled RSS {rss("codegraph"):.1f} MiB. Query results and analysis differ from DevMap’s composed neighborhood output. |',
    f'| codebase-memory-mcp (CBM) | {score("cbm")}/5 inspected callers; {edits("cbm")} edits visible, removal check: {removed("cbm")}; store {store("cbm"):.1f} MiB. | Cold {mid("cbm","cold")}, unchanged {mid("cbm","warm")}, edit {mid("cbm","edit")}. Standalone CLI startup is included in queries; these timings do not qualify persistent MCP latency. Native exclusions and partial parses remain below. |',
    f'| Graphify | Smallest measured store, {store("graphify"):.1f} MiB; all three definitions found; {edits("graphify")} edits visible, removal check: {removed("graphify")}. AST-only extraction completed without hosted inference. | {score("graphify")}/5 caller pairs; cold {mid("graphify","cold")}, sampled RSS {rss("graphify"):.1f} MiB. Documentation, clustering, and LLM enrichment were excluded, so full-feature performance and quality remain unmeasured. |',
    f'| Gortex | {score("gortex")}/5 callers by default, {opt_in}/5 with explicit name-only inclusion. {edits("gortex")} edits visible; exact-ID controls establish absence. It exposes separate query-ready and enriched gates. | Query-ready {sec(ready)}, full cold invocation {mid("gortex","cold")}; store {store("gortex"):.1f} MiB. Resident-daemon timings differ from standalone CLIs. Name-only caller edges remain inferred; capped fuzzy negative searches cannot establish absence. |',
    f'| GitNexus | All three definitions found; {edits("gitnexus")} edits visible; unchanged refresh {mid("gitnexus","warm")} versus cold {mid("gitnexus","cold")}. Context responses include source and relationships. | {score("gitnexus")}/5 caller pairs; deleted probe absent after normal restore: {removed("gitnexus")}. Edit {mid("gitnexus","edit")}; store {store("gitnexus"):.1f} MiB. Recovery and parser limitations are preserved below. |',
    '| ripgrep | Literal text matches without constructing a graph, providing an independent source-occurrence baseline. | Occurrences do not establish semantic definitions, direct callers, confidence, or change impact; no graph-accuracy score is assigned. |', '',
    '**Indexing latency.** Median and observed min–max. Three cold builds and edits, five unchanged refreshes.', '',
    '| Tool | Cold index | Unchanged refresh | One existing file edited |', '|---|---:|---:|---:|']
for tool in TOOLS:
    lines.append(f'| {NAMES[tool]} | {spread(tool,"cold")} | {spread(tool,"warm")} | {spread(tool,"edit")} |')
lines += ['', '**DevMap speed factors versus each competitor.** Each cell describes the installed DevMap build relative to the named tool, calculated from unrounded elapsed-time medians. “X× faster” means competitor time divided by DevMap time. “X× slower” means DevMap time divided by competitor time: for example, ' + speed_factor(median('devmap','edit'), median('codegraph','edit')).replace('× slower','') + '× slower means taking that multiple of the time. These ratios describe the measured native pipelines; they do not establish equivalent analysis or output.', '',
    '| Compared with | Cold index | Unchanged refresh | Single-file edit |', '|---|---:|---:|---:|']
for tool in TOOLS:
    if tool != 'devmap':
        lines.append('| ' + NAMES[tool] + ' | ' + ' | '.join(speed_factor(median('devmap',stage),median(tool,stage)) for stage in ['cold','warm','edit']) + ' |')
lines += ['', f'Gortex median internal gates were {sec(ready)} to query-ready and {sec(enriched)} to enrichment complete. The cold table includes wrapper/startup/shutdown overhead. Warm/edit/query operations use its resident daemon. The first unchanged refresh took {sec(RECORDS["warm-gortex-1"]["seconds"])}; restore took {sec(RECORDS["restore-gortex"]["seconds"])}. See [lifecycle log](gortex-cold-3.log).', '']
control = data('version-control.json')
assert control['timing_samples'] == 82
assert all(x.get('exact_definition', True) and not x.get('missing') for x in control['answers'])
lines += [f'**Alternating build control.** After the competitor campaign, {control["timing_samples"]} supplemental timing samples alternated the preserved `506a617` binary — the executable the previous report measured — and the installed `48cd3c7` binary on the same restored corpus, using independent empty/steady stores. Order reverses by repetition. Both are version 0.2.1 and differ by {P["commits_since_previous_report"]} commits. The protocol keeps three cold builds/edits and five unchanged/query repetitions per build. Both binaries passed the three definitions, five caller pairs, three edit-visibility checks, and removed-probe controls. These samples are separate from the 261-sample competitor table.', '',
    '| Operation | 506a617 median (min–max) | 48cd3c7 median (min–max) | Median change | 48cd3c7 vs 506a617 |', '|---|---:|---:|---:|---:|']
for stage, target in [('cold',None), ('warm',None), ('edit',None)] + [(s,t) for s in ['search','callers'] for t in TARGETS]:
    groups = [next(x for x in control['groups'] if (x['version'],x['stage'],x['target']) == (version,stage,target)) for version in BUILDS]
    cells = [f'{sec(x["seconds"]["median"])} ({sec(x["seconds"]["min"])}–{sec(x["seconds"]["max"])})' for x in groups]
    change = (groups[1]['seconds']['median']/groups[0]['seconds']['median']-1)*100
    factor = speed_factor(groups[1]['seconds']['median'], groups[0]['seconds']['median'])
    lines.append('| ' + stage + (' `' + target + '`' if target else '') + ' | ' + ' | '.join(cells) + f' | {change:+.1f}% | {factor} |')
lines += ['', '| Alternating cold control | Sampled process-tree RSS | Index size |', '|---|---:|---:|']
for version in BUILDS:
    sample = next(x for x in control['groups'] if x['version'] == version and x['stage'] == 'cold')
    lines.append(f'| {version} | {sample["sampled_tree_peak_bytes"]/2**20:.1f} MiB | {sample["store_bytes"]/2**20:.1f} MiB |')
control_stores = {v: next(x for x in control['groups'] if x['version'] == v and x['stage'] == 'cold')['store_bytes'] for v in BUILDS}
lines += ['', ('The two builds produced **identical** cold store sizes in this control '
    f'({control_stores["506a617"]:,} bytes each).' if control_stores['506a617'] == control_stores['48cd3c7'] else
    f'Cold store sizes differed: {control_stores["506a617"]:,} versus {control_stores["48cd3c7"]:,} bytes.') +
    ' Alternation reduces the time-separation confound; it does not eliminate shared-desktop noise, cache/order effects, or the small sample size. This is evidence about these two preserved executables on this one corpus, not a claim that every build in that range behaves identically. [Supplemental results and audited answers](version-control.json), [runner](run_version_control.py).', '',
    '**Historical DevMap comparison.** The previous report’s campaign and this one were collected at different times on a shared desktop. These deltas describe observed medians and are not controlled estimates of the effect of the build change; the alternating control above is the better-controlled comparison. Read the ranges above and the previous report’s ranges; no confidence intervals or population-wide ranking were established.', '',
    '| Operation | Previous report, build 506a617 | This run, build 48cd3c7 | Median change | Current vs previous |', '|---|---:|---:|---:|---:|']
for stage, target in [('cold',None), ('warm',None), ('edit',None)] + [(s,t) for s in ['search','callers'] for t in TARGETS]:
    old = group('devmap', stage, target, OLD)['seconds']['median']
    now = median('devmap', stage, target)
    label = stage + (' `' + target + '`' if target else '')
    lines.append(f'| {label} | {sec(old)} | {sec(now)} | {(now/old-1)*100:+.1f}% | {speed_factor(now,old)} |')
def control_median(version, stage, target=None):
    return next(x for x in control['groups'] if (x['version'], x['stage'], x['target']) == (version, stage, target))['seconds']['median']

campaign_search = statistics.median(median('devmap', 'search', t) for t in TARGETS)
control_search = {v: statistics.median(control_median(v, 'search', t) for t in TARGETS) for v in BUILDS}
lines += ['', 'Positive timing deltas mean slower observed medians; negative deltas mean faster. Both binaries carry clean-build metadata, so unlike the previous report’s historical row there is no unrecorded source patch here. Shared OS/compiler/package caches were not flushed, load varies, and absolute clone paths changed. These are additional limits on causal interpretation.', '',
    f'**Read the two comparisons together.** The historical table above shows this run’s query medians as slower than the previous report’s, but the alternating control — which measured *both* builds back to back on the same corpus — put them at {sec(control_search["506a617"])} and {sec(control_search["48cd3c7"])} for definition search, against {sec(campaign_search)} for the same build during the competitor campaign. The campaign and the control disagree by more for one build across sessions than the two builds disagree within a session. That points at campaign conditions — interleaved competitor processes and desktop load — rather than at a query regression between `506a617` and `48cd3c7`. The controlled result is the one to rely on; neither establishes a population-level claim.', '',
    '**Definitions, callers, and freshness.** Every tool found all three exact source definitions in all five repetitions. Caller sets were stable across the repetitions. Only the five pre-inspected direct caller pairs are scored; additional returned edges remain unjudged.', '',
    '| Tool | Matched caller pairs | Edits visible | Deleted probe absent after normal restore |', '|---|---:|---:|---|']
for tool in TOOLS:
    lines.append(f'| {NAMES[tool]} | {score(tool)}/5 | {edits(tool)} | {removed(tool)} |')
lines += ['', '| Definition | Exact source path | Expected direct callers |', '|---|---|---|']
for case in cases('devmap'):
    path = next(x for x in data('caller-source-evidence.json') if case['target'] in x['source'])['file']
    lines.append('| `' + case['target'] + '` | `' + path + '` | ' + '; '.join('`' + x + '`' for x in case['expected']) + ' |')
lines += ['', 'Missing caller pairs in the default responses:', '']
for tool in TOOLS:
    missing = sorted({pair for case in cases(tool) for pair in case['samples'][0]['missing']})
    lines.append('- ' + NAMES[tool] + ': ' + ('; '.join('`' + pair + '`' for pair in missing) if missing else 'none in this five-pair sample') + '.')
lines += ['', f'Gortex’s supplemental `get_callers` request used `min_tier:"text_matched"` and `exclude_tests:false`, returning a combined {opt_in}/5 inspected pairs. This one supplemental request does not replace its default score or query timings. Exact-ID checks returned `condition:"symbol_not_found"` for deleted/nonexistent IDs and succeeded for a known positive ID. [Opt-in callers](gortex-rust-callers-include-name-only.stdout), [deleted-ID control](gortex-exact-removed.stdout), [positive control](gortex-exact-present.stdout).', '',
    f'GitNexus retained the deleted probe after a second ordinary refresh: **{"yes" if recovery["repeated_normal_refresh_stale"] else "no"}**. A forced rebuild cleared it: **{"yes" if recovery["force_clears_deleted_symbol"] else "no"}**. The original responses and metadata precede recovery: [ordinary refresh](gitnexus-repeat-normal-refresh.stdout), [normal answer](gitnexus-repeat-stale-query.stdout), [forced rebuild](gitnexus-force-recovery.stdout), [recovered answer](gitnexus-after-force-query.stdout).', '',
    'The five-pair source snippets are preserved in [caller-source-evidence.json](caller-source-evidence.json). The [audit](audit.json) retains every repetition, missing/additional pairs, failed/inconclusive controls, and query truncation/coverage flags. A successful command is not a successful semantic check.', '',
    '**Query latency.** Medians over five repetitions per cell. Search is each native definition lookup; callers uses native response traversal. Gortex includes client-to-daemon transport, while the other graph tools start standalone processes.', '',
    '| Tool | Rust search | Go search | Python search | Rust callers | Go callers | Python callers |', '|---|---:|---:|---:|---:|---:|---:|']
for tool in TOOLS:
    lines.append('| ' + NAMES[tool] + ' | ' + ' | '.join(mid(tool,s,t) for s in ['search','callers'] for t in TARGETS) + ' |')
lines += ['', '**DevMap query speed factors versus each competitor.** Each cell describes DevMap relative to that tool for the named operation. The same elapsed-time ratio convention applies. Different output scopes and missed callers remain part of the interpretation.', '',
    '| Compared with | Rust search | Go search | Python search | Rust callers | Go callers | Python callers |', '|---|---:|---:|---:|---:|---:|---:|']
for tool in TOOLS:
    if tool != 'devmap':
        lines.append('| ' + NAMES[tool] + ' | ' + ' | '.join(speed_factor(median('devmap',s,t),median(tool,s,t)) for s in ['search','callers'] for t in TARGETS) + ' |')
lines += ['', 'Ripgrep literal search medians: ' + ', '.join('`' + t + '`: ' + mid('ripgrep','text',t) for t in TARGETS) + '.', '',
    'DevMap definition-search elapsed-time ratios against the ripgrep text baseline: ' + ', '.join('`' + t + '`: ' + speed_factor(median('devmap','search',t),median('ripgrep','text',t)) for t in TARGETS) + '. These are different operations: exact symbol lookup against an existing graph versus literal occurrences over source text. The ratios are not a claim of equivalent retrieval.', '',
    'Outputs differ: DevMap `explore` includes a broader neighborhood, Graphify `explain` includes connections, and GitNexus `context --content` includes source. CodeGraph `callers` and Gortex depth-one traversal are narrower. CBM CLI timings include opening a fresh process and are not its persistent MCP engine latency. Missing callers are correctness misses in this sample, not successful fast traversal.', '',
    '**Memory and storage.** Cold-run medians. RSS samples aggregate the measured process tree and supervised workers; Gortex includes the resident daemon and descendants.', '',
    '| Tool | Sampled process-tree peak RSS | Cold index/cache storage |', '|---|---:|---:|']
for tool in TOOLS:
    lines.append(f'| {NAMES[tool]} | {rss(tool):.1f} MiB | {store(tool):.1f} MiB |')
lines += ['', 'RSS is sampled roughly every 50 ms plus `ps` overhead, can miss short peaks, and double-counts shared pages. Raw BSD `time` RSS is also retained but is not used for ranking because it misses CBM’s supervised worker and may omit an un-waited daemon. Storage is logical file size of the native index/cache directories (including Gortex database/WAL), excluding installations, language/model caches, archived cold stores, and corpus Git objects. Different formats and retained data make these descriptive totals, not normalized compression scores.', '',
    '**Coverage inventories and warnings.** Counts preserve each native surface and stage; they are not accuracy scores.']
graphify = read('cold-graphify-3.stdout')
gf = re.search(r'found (\d+) code', graphify)
gn = re.search(r'(\d+) nodes, (\d+) edges', graphify)
codegraph = read('cold-codegraph-3.stdout')
cf = re.search(r'Indexed ([\d,]+) files', codegraph)
cn = re.search(r'([\d,]+) nodes, ([\d,]+) edges', codegraph)
cbm = data('cold-cbm-3.stdout')
gitnexus = data('gitnexus-recovered-meta.json')['stats']
gortex = data('gortex-final-stats.stdout')
status_row = next(line for line in read('gortex-final-status.stdout').splitlines() if '│ devcouncil-bench ' in line)
fields = [s.strip() for s in status_row.split('│')]
lines += ['', '| Native surface | Reported file measure | Symbols/nodes | Edges |', '|---|---|---:|---:|',
    f'| [DevMap cold](cold-devmap-3.stdout) | {DM["files_indexed"]} indexed | {DM["symbols"]:,} | {DM["edges"]:,} |',
    f'| [Graphify cold](cold-graphify-3.stdout) | {gf[1]} code files scanned | {int(gn[1]):,} | {int(gn[2]):,} |',
    f'| [CodeGraph cold](cold-codegraph-3.stdout) | {cf[1]} indexed | {cn[1]} | {cn[2]} |',
    f'| [CBM cold full](cold-cbm-3.stdout) | No comparable top-level total | {cbm["nodes"]:,} | {cbm["edges"]:,} |',
    f'| [GitNexus recovered metadata](gitnexus-recovered-meta.json) | {gitnexus["files"]:,} inventory files | {gitnexus["nodes"]:,} | {gitnexus["edges"]:,} |',
    f'| [Gortex final daemon status](gortex-final-status.stdout) | {fields[3]} tracked-repo files | {fields[4]} | {fields[5]} |',
    f'| [Gortex final query stats](gortex-final-stats.stdout) | {gortex["by_kind"]["file"]} file-kind nodes | {gortex["total_nodes"]:,} | {gortex["total_edges"]:,} |', '',
    'Gortex’s status and statistics count different surfaces at separate moments; their counting scope and ongoing derived work have not been reconciled. A discrepancy alone does not establish stale data or a defect.', '',
    f'DevMap reports {DM["resolution_rate"]["unresolved_sites"]:,} unresolved sites, of which {DM["resolution_rate"]["explained_sites"]:,} are classified as explained, leaving {remaining:,}. ' +
    (f'Build `506a617` produced exactly the same cold graph on this corpus — {PRIOR_DM["files_indexed"]} files, {PRIOR_DM["symbols"]:,} symbols, {PRIOR_DM["edges"]:,} edges, and an identical per-language resolution breakdown leaving {PRIOR_REMAINING:,}. '
     f'So the {P["commits_since_previous_report"]} intervening commits, which include framework/route extractor changes, did not alter what DevMap extracted from this workload. That is a statement about this corpus only: it does not show the extractor changes are inert on code that exercises them.'
     if GRAPH_IDENTICAL else
     f'Build `506a617` produced {PRIOR_DM["symbols"]:,} symbols and {PRIOR_DM["edges"]:,} edges, leaving {PRIOR_REMAINING:,}; this difference has not been validated as a change in actual caller accuracy.') +
    f' The final [status](devmap-final-status.stdout) reports {STATUS["coverage_gaps"]["import_blind"]["total"]} SQL import-extractor gaps and {STATUS["coverage_gaps"]["pattern_recovered"]["total"]} pattern-recovered file. Coverage warnings are retained even where selected queries pass.', '',
    f'CBM reports {cbm["parse_partial_count"]} partially parsed files and excluded directories {", ".join(cbm["excluded"]["dirs"])}. The examples are capped, with full counts retained. Additional current-run warnings and controls are recorded in [observations](observations.md).', '',
    '**Protocol and interpretation.** The machine was ' + P['machine'] + ', ' + str(P['cpu_count']) + ' logical CPUs, 64 GiB RAM, ' + P['platform'] + '. Other desktop work continued. Cold means an empty application index; shared OS/compiler/install/pilot caches were not flushed. Main tool order alternated by repetition; Gortex ran separately so its daemon did not contend with those measured invocations.', '',
    '| Version | Native mode |', '|---|---|',
    '| ' + NAMES['devmap'] + ' | Preserved binary; standalone `build`, `search`, `explore --budget 8000`; progress disabled |',
    '| Graphify 0.9.59 (`graphifyy`) | `extract --code-only --no-cluster`, `explain`, `affected --relation calls --depth 1` |',
    '| Gortex 0.64.3 | SQLite daemon, default local type enrichment, `reindex_repository`, `query symbol/callers`; embeddings disabled |',
    '| GitNexus 1.6.9 | `analyze --index-only`, embeddings disabled, `context --content` |',
    '| CodeGraph 1.6.0 | `init --yes` for cold; `sync`, `query`, `callers` |',
    '| codebase-memory-mcp 0.10.8 | Standalone `cli index_repository --mode full --persistence false`, `search_graph`, `trace_path --include-tests true` |',
    '| ripgrep 15.1.0 | `rg --json --fixed-strings` over the same corpus |', '',
    'Competitor installation hashes and Graphify dependency versions were checked against the prior run. They were reused locally; no new dependencies or global agent registrations were added. Source documentation and recorded native help informed the reused flags. Full commands, cwd, elapsed time, load averages, sampled RSS, exit status, and stdout/stderr paths are in [measurements.jsonl](measurements.jsonl); spreads are in [summary.json](summary.json).', '',
    '**What remains unverified.** One snapshot, one machine, three/five repetitions per operation, three definitions, and five caller pairs do not cover all languages or graph accuracy. The 261 timing samples repeat a small workload; they are not 261 repositories or distinct semantic tasks. Additional returned callers are unjudged, and no precision/recall population estimate is claimed.', '',
    'The edit case appends a new function to one existing Python file, then restores the original bytes. It does not test file creation, rename/deletion, multi-file refactors, sustained watcher throughput, crash recovery, or concurrent edits. Gortex retains its watcher and background enrichment; request wall time may include work outside the native operation duration. Filters, local language providers, symbol taxonomies, and output contents differ.', '',
    'Persistent MCP throughput/latency parity, semantic relevance, UI quality, embeddings/LLM enrichment, actual coding-agent completion, paid inference cost, and token savings were not measured. Vendor cost/token-saving estimates in raw outputs are not measured outcomes. No hosted inference evaluation was run. The Rust/product test suite was not rerun for this benchmark-artifact task; historical test passes in the initial report do not qualify this build.', '',
    '**Inferred follow-up priorities, not completed improvements.** Profile DevMap’s edit phases before optimizing the gap with CodeGraph — it is the one stage where a competitor is consistently ahead, and the gap widened in this run. Examine store contents and reclaim behavior before calling its larger directory wasted space. '
    + ('Add a corpus that actually exercises the changed framework/route extractors: this snapshot produced a byte-identical graph across 49 commits of extractor work, so it cannot detect regressions or improvements in exactly the code that changed. '
       if GRAPH_IDENTICAL else 'Inspect the unresolved-attribution classification difference with more source-checked cases. ')
    + 'Expand to more corpora, languages, edit types, matched provider availability/output, and persistent interfaces before making general product claims.', '',
    '**Reproduction and evidence.** See [PLAN.md](PLAN.md), [preparation script](prepare_rerun.py), [reused-script hashes](reused-artifacts.json), [binary hashes](installed-binaries.json), [Graphify dependencies](graphify-installed-requirements.txt), and [source verification](all-corpora-final-verification.json). The prepared corpus and executable are under the scratch path in [plan.json](plan.json). Existing cold state is never overwritten by the preparation workflow.', '',
    'Run the scripts serially as described in PLAN.md. For report-only regeneration from retained results, use `python3 benchmarks/results/competition/20260913-48cd3c7/write_report.py` from the repository root. The generator owns this narrative; [observations.md](observations.md) records directly inspected native warnings. No commit, push, or external publication is part of this run.', '']
(OUT / 'REPORT.md').write_text('\n'.join(lines))
print('Wrote', OUT / 'REPORT.md')
