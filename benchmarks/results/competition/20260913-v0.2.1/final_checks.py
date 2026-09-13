"""Additional exact-location checks, text baseline, and stale-index recovery."""
import json,subprocess,shutil
from measure import OUT,SCRATCH,measure
from run_expanded import ENV,B,TARGETS,dm,validate_snapshot
roots={t:SCRATCH/(t+'-corpus') for t in ['devmap','gitnexus','graphify','codegraph','cbm','gortex']}
# Trace responses omit caller files for package-scoped Go QNs; resolve them
# through the tool's own structured symbol search instead of guessing paths.
names=['size_gate_constants_are_their_declared_magnitudes','db_size_gate_scales_with_file_count_and_keeps_a_floor','TestAnInternedGraphIsHeldToTheSameBound','Load','main']
measure('cbm-caller-locations',[B['cbm'],'cli','search_graph','--project','devcouncil-bench','--name-pattern','^('+'|'.join(names)+')$','--format','json','--limit','100'],roots['cbm'],60,ENV)
d=json.loads((OUT/'cbm-caller-locations.stdout').read_text());assert not d.get('has_more')
for i in range(1,6):
 for name in TARGETS:
  measure(f'text-ripgrep-{name}-{i}',['rg','--json','--fixed-strings',name,'.'],roots['devmap'],60,ENV)
measure('devmap-final-status',dm('status'),roots['devmap'],60,ENV)
# Preserve normal-refresh metadata before checking the documented forced rebuild.
meta=roots['gitnexus']/'.gitnexus/meta.json';shutil.copy2(meta,OUT/'gitnexus-stale-meta.json')
measure('gitnexus-repeat-normal-refresh',[B['gitnexus'],'analyze',str(roots['gitnexus']),'--index-only','--name','devcouncil-expanded'],roots['gitnexus'],300,ENV)
argv=[B['gitnexus'],'context','devmap_competition_revision_3','-r',str(roots['gitnexus']),'-f','benchmarks/tasks.py']
measure('gitnexus-repeat-stale-query',argv,roots['gitnexus'],60,ENV)
measure('gitnexus-force-recovery',[B['gitnexus'],'analyze',str(roots['gitnexus']),'--index-only','--name','devcouncil-expanded','--force'],roots['gitnexus'],300,ENV)
measure('gitnexus-after-force-query',argv,roots['gitnexus'],60,ENV)
before=json.loads((OUT/'gitnexus-repeat-stale-query.stdout').read_text())
after=json.loads((OUT/'gitnexus-after-force-query.stdout').read_text())
(OUT/'gitnexus-recovery-observation.json').write_text(json.dumps({'repeated_normal_refresh_stale':before.get('symbol',{}).get('name')=='devmap_competition_revision_3','force_clears_deleted_symbol':after.get('error')=="Symbol 'devmap_competition_revision_3' not found"},indent=2)+'\n')
shutil.copy2(meta,OUT/'gitnexus-recovered-meta.json')
(OUT/'all-corpora-final-verification.json').write_text(json.dumps([validate_snapshot(t) for t in roots],indent=2)+'\n')
print('COMPLETE',flush=True)
