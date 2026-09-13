"""Summarize recorded measurements without converting failures into fast samples."""
import json,re,statistics
from pathlib import Path
from collections import defaultdict,Counter
OUT=Path(__file__).resolve().parent
rows=[json.loads(x) for x in (OUT/'measurements.jsonl').read_text().splitlines()]
assert max(Counter(r['label'] for r in rows).values())==1,'Duplicate measurement labels'
groups=defaultdict(list)
for r in rows:
 m=re.fullmatch(r'(cold|warm|edit|search|callers|text)-(devmap|graphify|gitnexus|codegraph|cbm|gortex|ripgrep)(?:-(.+))?-(\d+)',r['label'])
 if m:groups[(m[1],m[2],m[3])].append(r)
summary=[]
for (stage,tool,target),samples in sorted(groups.items()):
 ok=[x for x in samples if x['measurement_ok']]
 def stats(key):
  vals=[x[key] for x in ok if x.get(key) is not None]
  return {'median':statistics.median(vals),'min':min(vals),'max':max(vals)} if vals else None
 summary.append({'stage':stage,'tool':tool,'target':target,'attempted':len(samples),'succeeded':len(ok),'failed_labels':[x['label'] for x in samples if not x['measurement_ok']],'seconds':stats('seconds'),'time_rusage_peak_bytes':stats('rss_bytes'),'sampled_tree_peak_bytes':stats('sampled_tree_peak_rss_bytes')})
(OUT/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
for x in summary:
 if x['stage'] in ['cold','warm','edit']:print(x['tool'],x['stage'],x['succeeded'],round(x['seconds']['median'],4) if x['seconds'] else 'FAILED')
