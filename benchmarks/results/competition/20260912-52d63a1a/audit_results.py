"""Validate recorded benchmark answers against source-inspected cases."""
import json
from pathlib import Path
import re
from collections import Counter

OUT=Path(__file__).resolve().parent
EXPECTED={
 'db_size_gate_bytes': {
  'rust/devmap-extract/src/model.rs::size_gate_constants_are_their_declared_magnitudes',
  'rust/devmap-cli/tests/test_scholarlm_findings.rs::db_size_gate_scales_with_file_count_and_keeps_a_floor',
 },
 'loadBounded': {
  'backend/go_orchestrator/repomap/repomap.go::Load',
  'backend/go_orchestrator/repomap/compact_test.go::TestAnInternedGraphIsHeldToTheSameBound',
 },
 'describe_corpus': {'benchmarks/map_bench.py::main'},
}
PATHS={'db_size_gate_bytes':'rust/devmap-extract/src/model.rs','loadBounded':'backend/go_orchestrator/repomap/repomap.go','describe_corpus':'benchmarks/map_bench.py'}
rows=[json.loads(s) for s in (OUT/'measurements.jsonl').read_text().splitlines()]
counts=Counter(r['label'] for r in rows)
assert all(n==1 for n in counts.values()),'Duplicate measurement labels'
summary=json.loads((OUT/'summary.json').read_text())
assert len(summary)==21,len(summary)
for s in summary:
 expected=3 if s['stage'] in ['cold','edit'] else 5
 assert s['attempted']==s['measured']==expected and s['failures']==0,s
answers=[]
flags=[]
def walk_flags(x,path=''):
 if isinstance(x,dict):
  for k,v in x.items():
   p=path+'.'+k
   if k in ['truncated','walk_incomplete','nodes_omitted','hidden','hidden_count'] and v: flags.append({'path':p,'value':v})
   walk_flags(v,p)
 elif isinstance(x,list):
  for i,v in enumerate(x): walk_flags(v,path+f'[{i}]')
for tool in ['devmap','gitnexus']:
 for target,expected in EXPECTED.items():
  samples=[]
  for i in range(1,6):
   context=json.loads((OUT/f'context-{tool}-{target}-{i}.stdout').read_text())
   impact=json.loads((OUT/f'impact-{tool}-{target}-{i}.stdout').read_text())
   if tool=='devmap':
    defs=[x for x in context['definitions']['items'] if x['symbol_name']==target and x['file_path']==PATHS[target]]
    assert len(defs)==1,(tool,target,i)
    got={x['source_symbol'] for x in impact['items'] if x['edge_kind']=='Calls' and x['target_symbol']==PATHS[target]+'::'+target}
    context_got={x['source_symbol'] for x in defs[0]['callers']['items'] if x['edge_kind']=='Calls'}
   else:
    assert context['status']=='found' and context['symbol']['name']==target and context['symbol']['filePath']==PATHS[target]
    got={x['filePath']+'::'+x['name'] for x in impact['byDepth'].get('1',[]) if x['relationType']=='CALLS'}
    context_got={x['filePath']+'::'+x['name'] for x in context.get('incoming',{}).get('calls',[])}
   assert got==context_got,(tool,target,'context and impact disagree')
   samples.append({'iteration':i,'found':sorted(got),'missing':sorted(expected-got),'unexpected':sorted(got-expected)})
   if i==1:
    walk_flags(context,f'context-{tool}-{target}-1');walk_flags(impact,f'impact-{tool}-{target}-1')
  assert all(s['found']==samples[0]['found'] for s in samples),'Unstable answers'
  answers.append({'tool':tool,'target':target,'expected':sorted(expected),'found':samples[0]['found'],'missing':samples[0]['missing'],'unexpected':samples[0]['unexpected'],'consistent_samples':5})
# Empty answers must be successful misses, not unavailable queries.
for tool in ['devmap','gitnexus']:
 for prefix,name in [('missing', 'devmap_competition_no_such_symbol_52d63a1a')]+[(f'removed-{i}',f'devmap_competition_revision_{i}') for i in [1,2,3]]:
  filename=f'missing-{tool}.stdout' if prefix=='missing' else f'removed-{tool}-{prefix.split("-")[1]}.stdout'
  data=json.loads((OUT/filename).read_text())
  if tool=='devmap':
   assert data['resolution']=='Available' and not data['items'],filename
  elif tool=='gitnexus' and prefix!='missing' and data.get('status')=='found':
   assert data['status']=='found' and data['symbol']['name']==name,filename
  else:
   assert data['error']==f"Symbol '{name}' not found",filename
report={'measurement_records':len(rows),'benchmark_samples':sum(s['attempted'] for s in summary),'sample_groups':len(summary),'all_measurements_ok':all(r['measurement_ok'] for r in rows),'answer_cases':answers,'query_limits':flags,'generic_execution_checks':json.loads((OUT/'checks.json').read_text()),'removed_symbol_checks':json.loads((OUT/'removed-symbol-checks.json').read_text())}
(OUT/'audit.json').write_text(json.dumps(report,indent=2)+'\n')
print(json.dumps({k:v for k,v in report.items() if k not in ['query_limits','generic_execution_checks','removed_symbol_checks']},indent=2))
print('query limit flags:',len(flags))
