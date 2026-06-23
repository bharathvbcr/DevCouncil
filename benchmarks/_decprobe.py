import json
import sys
d = json.load(open(sys.argv[1]))
reqs = d.get("final_requirements", [])
tasks = d.get("final_tasks", [])
acs = sum(len(r.get("acceptance_criteria", [])) for r in reqs)
print("requirements:", len(reqs), " acceptance_criteria:", acs, " tasks:", len(tasks))
for t in tasks:
    files = [p["path"] for p in t.get("planned_files", [])]
    print("  -", t["id"], ":", t["title"][:55], " ACs:", len(t.get("acceptance_criterion_ids", [])), " files:", files)
