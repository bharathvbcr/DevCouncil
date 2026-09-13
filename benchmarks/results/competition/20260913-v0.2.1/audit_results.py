"""Audit source-inspected caller pairs and index freshness in retained native answers."""
import json,re
from pathlib import Path
from collections import Counter
OUT=Path(__file__).resolve().parent
PATHS={'db_size_gate_bytes':'rust/devmap-extract/src/model.rs','loadBounded':'backend/go_orchestrator/repomap/repomap.go','describe_corpus':'benchmarks/map_bench.py'}
EXPECTED={
 'db_size_gate_bytes':{'rust/devmap-extract/src/model.rs::size_gate_constants_are_their_declared_magnitudes','rust/devmap-cli/tests/test_scholarlm_findings.rs::db_size_gate_scales_with_file_count_and_keeps_a_floor'},
 'loadBounded':{'backend/go_orchestrator/repomap/repomap.go::Load','backend/go_orchestrator/repomap/compact_test.go::TestAnInternedGraphIsHeldToTheSameBound'},
 'describe_corpus':{'benchmarks/map_bench.py::main'}}
TOOLS=['devmap','graphify','gitnexus','codegraph','cbm','gortex']
def read(label):return (OUT/(label+'.stdout')).read_text()
def found(t,s,name,path):
 if t=='graphify':return s.startswith('Node: '+name+'()\n') and re.search(r'^  Source:\s+'+re.escape(path)+r' L\d+',s,re.M) is not None
 d=json.loads(s)
 if t=='devmap':
  assert d['resolution']=='Available'
  return any(x['symbol_name']==name and x['file_path']==path for x in d['items'])
 if t=='gitnexus':
  if 'error' in d:assert d['error']==f"Symbol '{name}' not found";return False
  return d.get('status')=='found' and d['symbol']['name']==name and d['symbol']['filePath']==path
 if t=='codegraph':return any(x['node']['name']==name and x['node']['filePath']==path for x in d)
 if t=='cbm':return any(g.get('file')==path and row[d['cols'].index('name')]==name for g in d['groups'] for row in g['rows'])
 if t=='gortex':return any(x['name']==name and x['file_path'].removeprefix('devcouncil-bench/')==path for x in d['results'])
 raise ValueError(t)
def cbm_locations():
 d=json.loads(read('cbm-caller-locations'));lookup={}
 for g in d['groups']:
  for row in g['rows']:
   name=row[d['cols'].index('name')];lookup.setdefault((g['qn_prefix'],name),set()).add(g['file']+'::'+name)
 return lookup

def callers(t,s,target):
 if t=='graphify':
  assert s.startswith('Affected nodes for '+target+'()')
  return {m[2]+'::'+m[1] for m in re.finditer(r'^- (.+)\(\) \[calls\] (.+):L\d+\s*$',s,re.M)}
 d=json.loads(s)
 if t=='devmap':
  defs=[x for x in d['definitions']['items'] if x['symbol_name']==target and x['file_path']==PATHS[target]];assert len(defs)==1
  return {x['source_symbol'] for x in defs[0]['callers']['items'] if x['edge_kind']=='Calls'}
 if t=='gitnexus':return {x['filePath']+'::'+x['name'] for x in d.get('incoming',{}).get('calls',[])}
 if t=='codegraph':return {x['filePath']+'::'+x['name'] for x in d['callers']}
 if t=='gortex':return {x['from'].removeprefix('devcouncil-bench/') for x in d['edges'] if x['kind']=='calls' and x['to']=='devcouncil-bench/'+PATHS[target]+'::'+target}
 if t=='cbm':
  c=d.get('callers',{});lookup=cbm_locations();result=set()
  for g in c.get('groups',[]):
   for row in g['rows']:
    name=row[c['cols'].index('name')];locations=lookup.get((g['qn_prefix'],name),set());assert len(locations)==1,(g,name,locations)
    result.update(locations)
  assert len(result)==d.get('callers_total',0)
  return result
 raise ValueError(t)

def flags(value,path=''):
 result=[]
 if isinstance(value,dict):
  for k,v in value.items():
   if k in ['truncated','has_more','walk_incomplete','hidden','nodes_omitted','next','text_matched_suppressed','_truncated_by_budget','next_cursor'] and v:result.append({'path':path+'.'+k,'value':v})
   result+=flags(v,path+'.'+k)
 elif isinstance(value,list):
  for i,v in enumerate(value):result+=flags(v,path+f'[{i}]')
 return result

def main():
 rows=[json.loads(l) for l in (OUT/'measurements.jsonl').read_text().splitlines()];records={r['label']:r for r in rows};assert len(records)==len(rows)
 answers=[];controls=[];limits=[]
 for t in TOOLS:
  for target,path in PATHS.items():
   samples=[]
   for i in range(1,6):
    a=f'search-{t}-{target}-{i}';b=f'callers-{t}-{target}-{i}'
    assert records[a]['measurement_ok'] and records[b]['measurement_ok'],(a,b)
    exact=found(t,read(a),target,path);got=callers(t,read(b),target)
    samples.append({'iteration':i,'exact_definition':exact,'returned':sorted(got),'matched':sorted(got&EXPECTED[target]),'missing':sorted(EXPECTED[target]-got),'additional_unjudged':sorted(got-EXPECTED[target])})
    if i==1 and t!='graphify':limits.extend({'label':label,**f} for label in [a,b] for f in flags(json.loads(read(label))))
   answers.append({'tool':t,'target':target,'expected':sorted(EXPECTED[target]),'consistent':all(s['returned']==samples[0]['returned'] and s['exact_definition']==samples[0]['exact_definition'] for s in samples),'samples':samples})
  for i in range(1,4):
   label=f'validate-edit-{t}-{i}';name=f'devmap_competition_revision_{i}'
   controls.append({'label':label,'expected':'present','passed':records[label]['measurement_ok'] and found(t,read(label),name,'benchmarks/tasks.py')})
  for name in ['devmap_competition_revision_3','devmap_competition_nonexistent_52d63a1a']:
   label=f'negative-{t}-{name}';s=read(label)
   # A correctly reported absence must have a successful query and a recognized empty shape.
   absent=not found(t,s,name,'benchmarks/tasks.py')
   if t=='graphify' and absent:assert s.strip()==f"No node matching '{name}' found."
   limited=t=='gortex' and (json.loads(s).get('truncated') or json.loads(s).get('_truncated_by_budget'))
   if t=='gortex':limits.extend({'label':label,**f} for f in flags(json.loads(s)))
   controls.append({'label':label,'expected':'absent','passed':None if limited else records[label]['measurement_ok'] and absent,'inconclusive':bool(limited),'reason':'Capped fuzzy search cannot establish absence' if limited else None})
 for stage,n in [('cold',3),('warm',5),('edit',3)]:
  for t in TOOLS:
   for i in range(1,n+1):assert records[f'{stage}-{t}-{i}']['measurement_ok']
 corpus=json.loads((OUT/'corpus-verification.json').read_text())+[json.loads((OUT/'gortex-corpus-verification.json').read_text())]
 assert len(corpus)==6 and all(x['files']==1186 and not x['mismatches'] for x in corpus)
 supplement={}
 for label in ['gortex-exact-removed','gortex-exact-nonexistent']:
  d=json.loads(read(label));assert records[label]['measurement_ok'] and d.get('condition')=='symbol_not_found'
  supplement[label]={'passed':True,'condition':d['condition']}
 d=json.loads(read('gortex-exact-present'));assert d['name']=='db_size_gate_bytes' and d['file_path']=='devcouncil-bench/'+PATHS['db_size_gate_bytes']
 extra=callers('gortex',read('gortex-rust-callers-include-name-only'),'db_size_gate_bytes')
 supplement['gortex_include_name_only']={'rust_callers':sorted(extra),'combined_matched_pairs':len(extra&EXPECTED['db_size_gate_bytes'])+sum(len(x['samples'][0]['matched']) for x in answers if x['tool']=='gortex' and x['target']!='db_size_gate_bytes'),'note':'One supplemental run; default-query measurements and score are preserved.'}
 assert records['gitnexus-repeat-stale-query']['measurement_ok'] and records['gitnexus-after-force-query']['measurement_ok']
 supplement['gitnexus_force_recovery']=json.loads((OUT/'gitnexus-recovery-observation.json').read_text())
 output={'supplemental':supplement,'records':len(rows),'answers':answers,'controls':controls,'query_limits':limits,'corpus':corpus,'timed_invocation_failures':[r['label'] for r in rows if not r['measurement_ok']]}
 (OUT/'audit.json').write_text(json.dumps(output,indent=2)+'\n')
 for t in TOOLS:
  a=[x for x in answers if x['tool']==t]
  print(t,'definitions',sum(x['samples'][0]['exact_definition'] for x in a),'/3 callers',sum(len(x['samples'][0]['matched']) for x in a),'/5 consistent',all(x['consistent'] for x in a))
 print('Control failures',[x['label'] for x in controls if x['passed'] is False]);print('Initially inconclusive controls, resolved by exact lookup',[x['label'] for x in controls if x['passed'] is None])
if __name__=='__main__':main()
