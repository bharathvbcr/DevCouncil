import json
from measure import OUT,SCRATCH,measure
B=json.loads((OUT/'tool-paths.json').read_text())
ENV={'XDG_CONFIG_HOME':str(SCRATCH/'xdg/config'),'XDG_CACHE_HOME':str(SCRATCH/'xdg/cache'),'XDG_DATA_HOME':str(SCRATCH/'xdg/data'),'CBM_CACHE_DIR':str(SCRATCH/'cbm-cache'),'DO_NOT_TRACK':'1','NO_COLOR':'1'}
for tool,root,args in [
 ('graphify','graphify',['extract',str(SCRATCH/'graphify-corpus'),'--code-only','--no-cluster']),
 ('codegraph','codegraph',['index',str(SCRATCH/'codegraph-corpus')]),
 ('codebase-memory-mcp','cbm',['cli','index_repository','--repo-path',str(SCRATCH/'cbm-corpus'),'--name','devcouncil-bench','--mode','full','--persistence','false'])]:
 measure('pilot-'+tool,[B[tool],*args],SCRATCH/(root+'-corpus'),600,ENV)
