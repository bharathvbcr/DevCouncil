"""Summarize all recorded samples without including pilots or error paths."""
import json
from pathlib import Path
import re
import statistics

out=Path(__file__).resolve().parent
rows=[json.loads(s) for s in (out/'measurements.jsonl').read_text().splitlines()]
groups={}
for row in rows:
    match=re.fullmatch(r'(cold|warm|edit|context|impact|search)-(devmap|gitnexus|ripgrep)(?:-(.*))?-(\d+)',row['label'])
    if not match: continue
    stage,tool,target,iteration=match.groups()
    key=(stage,tool,target or '')
    groups.setdefault(key,[]).append(row)
summary=[]
for (stage,tool,target),samples in sorted(groups.items()):
    valid=[r for r in samples if r['measurement_ok']]
    times=[r['seconds'] for r in valid]
    rss=[r['rss_bytes'] for r in valid]
    stat={'stage':stage,'tool':tool,'target':target,'attempted':len(samples),'measured':len(valid),'failures':len(samples)-len(valid),'samples_s':times}
    if times:
        stat.update({'min_s':min(times),'median_s':statistics.median(times),'max_s':max(times),'mean_s':statistics.mean(times),'spread_pct':100*(max(times)-min(times))/statistics.median(times),'median_rss_bytes':statistics.median(rss),'max_rss_bytes':max(rss)})
    summary.append(stat)
(out/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
for s in summary:
    print(s['stage'],s['tool'],s['target'],s['measured'],f"{s.get('median_s',0):.4f}s",f"{s.get('median_rss_bytes',0)/1048576:.1f} MiB")
