"""Self-contained interactive HTML visualizer for the code knowledge graph."""

from __future__ import annotations

import json
import logging
from pathlib import Path
from typing import Any, Dict, List, Set

from devcouncil.indexing.graph.build import graph_path, load_code_graph
from devcouncil.indexing.graph.schema import (
    CodeGraph,
    Confidence,
    DeadCodeEntry,
    GraphEdge,
    GraphNode,
    NodeKind,
)

logger = logging.getLogger(__name__)

_VENDOR_REL = Path("assets") / "vendor" / "force-graph.min.js"
_FILE_EDGE_KINDS = frozenset({"imports"})
_SYMBOL_EDGE_KINDS = frozenset({"calls", "inherits", "implements", "overrides", "named_import"})


def _vendor_js() -> str:
    """Load vendored force-graph (or a tiny fallback shim)."""
    try:
        import importlib.resources as resources

        pkg = resources.files("devcouncil")
        path = pkg.joinpath("assets/vendor/force-graph.min.js")
        if path.is_file():
            return path.read_text(encoding="utf-8")
    except Exception:
        pass
    # Repo checkout path
    here = Path(__file__).resolve().parents[1] / "assets" / "vendor" / "force-graph.min.js"
    if here.is_file():
        return here.read_text(encoding="utf-8")
    # Minimal fallback so the page still loads with a message
    return (
        "window.ForceGraph=function(){return{"
        "graphData:function(){return this},nodeId:function(){return this},"
        "nodeLabel:function(){return this},nodeAutoColorBy:function(){return this},"
        "nodeVal:function(){return this},linkColor:function(){return this},"
        "linkDirectionalParticles:function(){return this},"
        "linkDirectionalParticleWidth:function(){return this},"
        "onNodeClick:function(){return this},"
        "width:function(){return this},height:function(){return this},_missing:true};};"
    )


def _conf_val(obj: Any) -> str:
    if hasattr(obj, "value"):
        return str(obj.value)
    return str(obj or "")


def _node_community(n: Any, graph: CodeGraph) -> str:
    """Prefer intel community; fall back to area when communities are absent."""
    extras = getattr(n, "extras", None) or {}
    if isinstance(extras, dict):
        c = extras.get("community") or extras.get("community_id")
        if c is not None and str(c).strip():
            return str(c)
    # compute_communities writes the Louvain label to the node attribute —
    # the old code never read it, so community coloring silently degraded
    # to area grouping.
    direct = getattr(n, "community", "") or ""
    if str(direct).strip():
        return str(direct)
    meta = graph.meta or {}
    by_id = meta.get("node_communities") or meta.get("communities_by_node") or {}
    if isinstance(by_id, dict) and n.id in by_id:
        return str(by_id[n.id])
    area = getattr(n, "area", "") or ""
    return area or "unknown"


def _processes(graph: CodeGraph) -> List[Dict[str, Any]]:
    meta = graph.meta or {}
    raw = meta.get("processes") or meta.get("process_flows") or []
    if not isinstance(raw, list):
        return []
    out: List[Dict[str, Any]] = []
    for p in raw[:100]:
        if isinstance(p, dict):
            out.append(p)
        else:
            out.append({"name": str(p), "steps": []})
    return out


def _communities_summary(nodes: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
    counts: Dict[str, int] = {}
    for n in nodes:
        key = n.get("community") or n.get("area") or "unknown"
        counts[key] = counts.get(key, 0) + 1
    return [
        {"id": k, "label": k, "size": counts[k]}
        for k in sorted(counts.keys(), key=lambda x: (-counts[x], x))
    ]


def _level_payload(
    graph: CodeGraph,
    *,
    file_level: bool,
    dead_by_id: Dict[str, Any],
    dead_paths: Set[str],
    unwired: Set[str],
    unreachable: Set[str],
    entry_set: Set[str],
) -> Dict[str, Any]:
    degree: Dict[str, int] = {}
    for e in graph.edges:
        if file_level:
            if e.kind not in _FILE_EDGE_KINDS:
                continue
            if "::" in e.source or "::" in e.target:
                continue
        else:
            if e.kind not in _SYMBOL_EDGE_KINDS and e.kind not in {"calls", "inherits", "implements", "overrides"}:
                # Allow calls/inherits family; skip pure file imports in symbol mode
                if e.kind == "imports" and ("::" not in e.source and "::" not in e.target):
                    continue
        degree[e.source] = degree.get(e.source, 0) + 1
        degree[e.target] = degree.get(e.target, 0) + 1

    nodes: List[Dict[str, Any]] = []
    for n in graph.nodes:
        kind = n.kind.value if hasattr(n.kind, "value") else str(n.kind)
        if file_level:
            if kind != "file":
                continue
        else:
            if kind == "file":
                continue
        nid = n.id
        flags: List[str] = []
        dead_conf = ""
        if nid in dead_by_id:
            flags.append("dead")
            dead_conf = _conf_val(dead_by_id[nid].confidence)
        elif n.path in dead_paths and kind == "file":
            flags.append("dead")
        if n.path in unwired or nid in unwired:
            flags.append("unwired")
        if n.path in unreachable or nid in unreachable:
            flags.append("unreachable")
        community = _node_community(n, graph)
        is_entry = nid in entry_set or n.path in entry_set or any(
            nid.startswith(f"{er}::") or n.path == er for er in entry_set
        )
        nodes.append(
            {
                "id": nid,
                "name": n.name or n.path,
                "path": n.path,
                "kind": kind,
                "area": n.area or "",
                "community": community,
                "line": n.line,
                "val": max(1, degree.get(nid, 1)),
                "flags": flags,
                "dead_confidence": dead_conf,
                "entry": is_entry,
            }
        )
    node_ids = {n["id"] for n in nodes}
    links: List[Dict[str, Any]] = []
    for e in graph.edges:
        if e.source not in node_ids or e.target not in node_ids:
            continue
        if file_level and e.kind not in _FILE_EDGE_KINDS:
            continue
        if not file_level and e.kind == "imports" and "::" not in e.source and "::" not in e.target:
            continue
        if not file_level and e.kind in {"contains", "defines", "documents"}:
            continue
        links.append(
            {
                "source": e.source,
                "target": e.target,
                "kind": e.kind,
                "confidence": _conf_val(e.confidence),
            }
        )
    return {
        "nodes": nodes,
        "links": links,
        "areas": sorted({n["area"] for n in nodes if n.get("area")}),
        "communities": _communities_summary(nodes),
    }


def _adjacency(links: List[Dict[str, Any]]) -> Dict[str, List[str]]:
    adj: Dict[str, List[str]] = {}
    for link in links:
        s, t = link["source"], link["target"]
        adj.setdefault(s, []).append(t)
        adj.setdefault(t, []).append(s)
    return adj


def _payload_from_graph(graph: CodeGraph, *, file_level: bool = True) -> Dict[str, Any]:
    """Build a compact viz payload with file and symbol modes."""
    dead_by_id = {d.id: d for d in graph.dead_code}
    dead_paths = {d.path for d in graph.dead_code}
    unwired = set(graph.unwired_candidates)
    unreachable = set(graph.unreachable_files)
    entry_set = set(graph.entry_roots)

    file_payload = _level_payload(
        graph,
        file_level=True,
        dead_by_id=dead_by_id,
        dead_paths=dead_paths,
        unwired=unwired,
        unreachable=unreachable,
        entry_set=entry_set,
    )
    symbol_payload = _level_payload(
        graph,
        file_level=False,
        dead_by_id=dead_by_id,
        dead_paths=dead_paths,
        unwired=unwired,
        unreachable=unreachable,
        entry_set=entry_set,
    )
    # Build neighbor indexes for detail panel (callers/callees/importers)
    callers: Dict[str, List[str]] = {}
    callees: Dict[str, List[str]] = {}
    importers: Dict[str, List[str]] = {}
    for e in graph.edges:
        if e.kind == "calls":
            callers.setdefault(e.target, []).append(e.source)
            callees.setdefault(e.source, []).append(e.target)
        elif e.kind == "imports":
            importers.setdefault(e.target, []).append(e.source)

    dead_dump = []
    for d in graph.dead_code[:2000]:
        dead_dump.append(
            {
                "id": d.id,
                "path": d.path,
                "line": d.line,
                "kind": d.kind,
                "confidence": _conf_val(d.confidence),
                "reason": d.reason,
            }
        )

    meta = graph.meta or {}
    active = "file" if file_level else "symbol"
    return {
        "mode": active,
        "file": file_payload,
        "symbol": symbol_payload,
        "dead_code": dead_dump,
        "processes": _processes(graph),
        "god_nodes": list(meta.get("god_nodes") or [])[:30],
        "hotspots": list(meta.get("hotspots") or [])[:30],
        "circular_imports": list(meta.get("circular_imports") or [])[:30],
        "neighbors": {"callers": callers, "callees": callees, "importers": importers},
        "meta": {
            "entry_roots": list(graph.entry_roots)[:100],
            "schema_version": graph.schema_version,
            "has_communities": bool(
                (graph.meta or {}).get("communities")
                or (graph.meta or {}).get("node_communities")
                or any(
                    (getattr(n, "extras", None) or {}).get("community")
                    for n in graph.nodes[:200]
                )
            ),
        },
        # Back-compat flat fields used by older tests / consumers
        "nodes": file_payload["nodes"] if file_level else symbol_payload["nodes"],
        "links": file_payload["links"] if file_level else symbol_payload["links"],
        "areas": file_payload["areas"] if file_level else symbol_payload["areas"],
    }


def render_graph_html(
    graph: CodeGraph,
    *,
    file_level: bool = True,
) -> str:
    """Return a self-contained HTML document (no network)."""
    payload = _payload_from_graph(graph, file_level=file_level)
    # Escape </script> in JSON so embed cannot break out
    raw = json.dumps(payload, separators=(",", ":"), ensure_ascii=False)
    raw = raw.replace("<", "\\u003c").replace(">", "\\u003e").replace("&", "\\u0026")
    vendor = _vendor_js()
    return f"""<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8"/>
<meta name="viewport" content="width=device-width, initial-scale=1"/>
<title>DevCouncil Code Graph</title>
<style>
:root {{ --bg:#0f1419; --panel:#1a2332; --fg:#e7ecf3; --muted:#8b9bb4; --accent:#3d8bfd; --dead:#e35d6a; --entry:#34d399; }}
* {{ box-sizing:border-box; }}
body {{ margin:0; font:14px/1.4 ui-sans-serif,system-ui,sans-serif; background:var(--bg); color:var(--fg); display:flex; height:100vh; }}
#sidebar {{ width:360px; background:var(--panel); padding:0; overflow:hidden; border-right:1px solid #243044; display:flex; flex-direction:column; }}
#graph {{ flex:1; position:relative; }}
h1 {{ font-size:16px; margin:0 0 8px; }}
.tabs {{ display:flex; border-bottom:1px solid #243044; }}
.tab {{ flex:1; padding:8px 4px; text-align:center; cursor:pointer; color:var(--muted); font-size:12px; border:none; background:transparent; }}
.tab.active {{ color:var(--fg); border-bottom:2px solid var(--accent); }}
.panel {{ display:none; padding:12px; overflow:auto; flex:1; }}
.panel.active {{ display:block; }}
input,select,button {{ width:100%; margin:4px 0 8px; padding:6px 8px; background:#0f1419; color:var(--fg); border:1px solid #334155; border-radius:4px; }}
button.primary {{ cursor:pointer; background:var(--accent); border:none; font-weight:600; }}
label {{ color:var(--muted); font-size:12px; display:block; }}
label.inline {{ display:flex; align-items:center; gap:6px; margin:4px 0; }}
label.inline input {{ width:auto; margin:0; }}
#detail {{ margin-top:8px; font-size:12px; color:var(--muted); white-space:pre-wrap; }}
.interaction-hint {{ margin:8px 0; padding:8px; border:1px solid #334155; border-radius:4px; background:#0f1419; color:var(--fg); font-size:12px; line-height:1.45; }}
#counts {{ margin:8px 0 4px; font-size:12px; color:var(--accent); font-weight:600; }}
.flag-dead {{ color:var(--dead); }}
.badge-entry {{ color:var(--entry); font-weight:600; }}
.list {{ list-style:none; padding:0; margin:0; font-size:12px; }}
.list li {{ padding:4px 0; border-bottom:1px solid #243044; cursor:pointer; }}
.list li:hover {{ color:var(--accent); }}
.muted {{ color:var(--muted); }}
.row {{ display:flex; gap:6px; }}
.row > * {{ flex:1; }}
</style>
</head>
<body>
<aside id="sidebar">
  <div class="tabs">
    <button class="tab active" data-tab="graph">Graph</button>
    <button class="tab" data-tab="dead">Dead code</button>
    <button class="tab" data-tab="communities">Communities</button>
    <button class="tab" data-tab="processes">Processes</button>
    <button class="tab" data-tab="intel">Intel</button>
  </div>
  <div id="panel-graph" class="panel active">
    <h1>DevCouncil Code Graph</h1>
    <div id="counts" aria-live="polite">Nodes: -- · Edges: -- · Filtered: --</div>
    <div class="interaction-hint" id="interactionHint"><strong>Click</strong> a node for details. <strong>Select two nodes</strong> to highlight the shortest path. <strong>Double-click</strong> a node to expand its neighborhood.</div>
    <label>Mode</label>
    <select id="mode"><option value="file">File-level</option><option value="symbol">Symbol-level</option></select>
    <label>Search</label>
    <input id="search" placeholder="name or path" autocomplete="off"/>
    <label>Color by</label>
    <select id="colorBy"><option value="community">Community / area</option><option value="area">Area only</option><option value="kind">Kind</option></select>
    <label>Area / community</label>
    <select id="area"><option value="">(all)</option></select>
    <label>Edge kind</label>
    <select id="ekind"><option value="">(all)</option><option>imports</option><option>calls</option><option>inherits</option><option>implements</option><option>overrides</option><option>decorates</option></select>
    <label>Edge confidence</label>
    <select id="econf"><option value="">(all)</option><option>extracted</option><option>inferred</option><option>ambiguous</option></select>
    <label class="inline"><input type="checkbox" id="lensDead"/> Lens: dead</label>
    <label class="inline"><input type="checkbox" id="lensUnwired"/> Lens: unwired</label>
    <label class="inline"><input type="checkbox" id="lensUnreach"/> Lens: unreachable</label>
    <label>Dead confidence</label>
    <select id="deadConf"><option value="">(any)</option><option>extracted</option><option>inferred</option><option>ambiguous</option></select>
    <div class="row"><button class="primary" id="reset">Reset view</button><button class="primary" id="clearPath">Clear path</button></div>
    <div id="detail" class="muted">Select a node to inspect callers, callees, and path state.</div>
  </div>
  <div id="panel-dead" class="panel">
    <h1>Dead code</h1>
    <label>Sort by</label>
    <select id="deadSort"><option value="path">path</option><option value="confidence">confidence</option><option value="kind">kind</option><option value="id">id</option></select>
    <ul class="list" id="deadList"></ul>
  </div>
  <div id="panel-communities" class="panel">
    <h1>Communities</h1>
    <p class="muted" id="commHint">Colored by community when present; falls back to area.</p>
    <ul class="list" id="commList"></ul>
  </div>
  <div id="panel-processes" class="panel">
    <h1>Processes</h1>
    <p class="muted" id="procHint">Entry-root call flows (when intel is available).</p>
    <ul class="list" id="procList"></ul>
  </div>
  <div id="panel-intel" class="panel">
    <h1>Hotspots</h1>
    <p class="muted">Churn × fan-in over the last 90 days — highest refactor risk first.</p>
    <ul class="list" id="hotList"></ul>
    <h1 style="margin-top:12px">God nodes</h1>
    <p class="muted">Highest coupling (degree, fan-in/out, PageRank).</p>
    <ul class="list" id="godList"></ul>
    <h1 style="margin-top:12px">Circular imports</h1>
    <ul class="list" id="cycleList"></ul>
  </div>
</aside>
<div id="graph">
  <div id="vendorWarn" style="display:none;position:absolute;inset:0;z-index:10;padding:40px;color:var(--dead);font-size:14px;background:var(--bg)">
    force-graph vendor bundle is missing — the canvas cannot render.<br/>
    Expected at <code>src/devcouncil/assets/vendor/force-graph.min.js</code>.
    Sidebar data (dead code, intel, processes) still works.
  </div>
</div>
<script>{vendor}</script>
<script>
const DATA = {raw};
let mode = DATA.mode || 'file';
let selected = [];
let pathHighlight = new Set();
let expandIds = null; // null = show all filtered; Set = neighborhood focus

document.getElementById('mode').value = mode;

function activePayload() {{
  return (mode === 'symbol' ? DATA.symbol : DATA.file) || {{nodes:[], links:[], areas:[], communities:[]}};
}}

function refillArea() {{
  const areaSel = document.getElementById('area');
  const cur = areaSel.value;
  areaSel.innerHTML = '<option value="">(all)</option>';
  const p = activePayload();
  const vals = new Set([...(p.areas||[]), ...((p.communities||[]).map(c => c.id))]);
  [...vals].filter(Boolean).sort().forEach(a => {{
    const o=document.createElement('option'); o.value=a; o.textContent=a; areaSel.appendChild(o);
  }});
  if ([...vals].includes(cur)) areaSel.value = cur;
}}

function lid(l, key) {{
  const v = l[key];
  return typeof v === 'object' && v ? v.id : v;
}}

function filtered() {{
  const p = activePayload();
  const q = (document.getElementById('search').value||'').toLowerCase();
  const area = document.getElementById('area').value;
  const lensDead = document.getElementById('lensDead').checked;
  const lensUnwired = document.getElementById('lensUnwired').checked;
  const lensUnreach = document.getElementById('lensUnreach').checked;
  const deadConf = document.getElementById('deadConf').value;
  const anyLens = lensDead || lensUnwired || lensUnreach;
  let nodes = p.nodes.filter(n => {{
    if (expandIds && !expandIds.has(n.id)) return false;
    if (area && n.area !== area && n.community !== area) return false;
    if (anyLens) {{
      const flags = n.flags || [];
      let ok = false;
      if (lensDead && flags.includes('dead')) ok = true;
      if (lensUnwired && flags.includes('unwired')) ok = true;
      if (lensUnreach && flags.includes('unreachable')) ok = true;
      if (!ok) return false;
      if (lensDead && deadConf && n.dead_confidence && n.dead_confidence !== deadConf) return false;
    }}
    if (q && !(n.id.toLowerCase().includes(q) || (n.path||'').toLowerCase().includes(q) || (n.name||'').toLowerCase().includes(q))) return false;
    return true;
  }});
  const ids = new Set(nodes.map(n => n.id));
  const ek = document.getElementById('ekind').value;
  const econf = document.getElementById('econf').value;
  const links = p.links.filter(l => {{
    const s = lid(l,'source'), t = lid(l,'target');
    if (!ids.has(s)||!ids.has(t)) return false;
    if (ek && l.kind !== ek) return false;
    if (econf && l.confidence !== econf) return false;
    return true;
  }});
  return {{nodes, links}};
}}

function bfsPath(start, end, links) {{
  const adj = {{}};
  links.forEach(l => {{
    const s = lid(l,'source'), t = lid(l,'target');
    (adj[s]=adj[s]||[]).push(t);
    (adj[t]=adj[t]||[]).push(s);
  }});
  if (start === end) return [start];
  const q = [start], prev = {{[start]: null}};
  while (q.length) {{
    const cur = q.shift();
    for (const nxt of (adj[cur]||[])) {{
      if (nxt in prev) continue;
      prev[nxt] = cur;
      if (nxt === end) {{
        const path = [end];
        let x = end;
        while (prev[x] !== null) {{ path.push(prev[x]); x = prev[x]; }}
        return path.reverse();
      }}
      q.push(nxt);
    }}
  }}
  return null;
}}

function neighborExpand(id, links, depth) {{
  const adj = {{}};
  links.forEach(l => {{
    const s = lid(l,'source'), t = lid(l,'target');
    (adj[s]=adj[s]||[]).push(t);
    (adj[t]=adj[t]||[]).push(s);
  }});
  const seen = new Set([id]);
  let frontier = [id];
  for (let d=0; d<depth; d++) {{
    const next = [];
    frontier.forEach(n => (adj[n]||[]).forEach(x => {{
      if (!seen.has(x)) {{ seen.add(x); next.push(x); }}
    }}));
    frontier = next;
  }}
  return seen;
}}

function escapeHtml(s) {{
  return String(s||'').replace(/[&<>"']/g, c => ({{'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}}[c]));
}}

function showDetail(n) {{
  const nb = DATA.neighbors || {{}};
  const callers = (nb.callers||{{}})[n.id] || [];
  const callees = (nb.callees||{{}})[n.id] || [];
  const importers = (nb.importers||{{}})[n.id] || [];
  const d = document.getElementById('detail');
  let html = '<strong>'+escapeHtml(n.id)+'</strong>\\n';
  html += 'kind: '+escapeHtml(n.kind)+'\\n';
  html += 'community: '+escapeHtml(n.community||'')+'\\n';
  html += 'area: '+escapeHtml(n.area||'')+'\\n';
  html += 'path: '+escapeHtml(n.path||'');
  if (n.line) html += '\\nline: '+n.line;
  if (n.entry) html += '\\n<span class="badge-entry">entry root</span>';
  if ((n.flags||[]).length) html += '\\n<span class="flag-dead">'+escapeHtml(n.flags.join(', '))+(n.dead_confidence?' ['+escapeHtml(n.dead_confidence)+']':'')+'</span>';
  html += '\\n\\ncallers: '+(callers.slice(0,12).map(escapeHtml).join(', ')||'(none)');
  html += '\\ncallees: '+(callees.slice(0,12).map(escapeHtml).join(', ')||'(none)');
  html += '\\nimporters: '+(importers.slice(0,12).map(escapeHtml).join(', ')||'(none)');
  if (selected.length === 2) {{
    html += '\\n\\nselected path: '+escapeHtml(selected.join(' → '));
  }}
  d.innerHTML = html;
}}

function colorKey(n) {{
  const by = document.getElementById('colorBy').value;
  if (by === 'kind') return n.kind || 'unknown';
  if (by === 'area') return n.area || 'unknown';
  return n.community || n.area || 'unknown';
}}

const elem = document.getElementById('graph');
const Graph = ForceGraph();
const g = Graph(elem)
  .nodeId('id')
  .nodeLabel(n => (n.entry ? '★ ' : '') + (n.path || n.id))
  .nodeAutoColorBy(n => colorKey(n))
  .nodeVal(n => n.val || 1)
  .linkColor(l => {{
    const s = lid(l,'source'), t = lid(l,'target');
    if (pathHighlight.size && pathHighlight.has(s) && pathHighlight.has(t)) return '#f472b6';
    return l.kind==='calls' ? '#f59e0b' : (l.kind==='inherits'||l.kind==='implements'||l.kind==='overrides') ? '#a78bfa' : l.kind==='decorates' ? '#2dd4bf' : '#64748b';
  }})
  .linkDirectionalParticles(l => {{
    const s = lid(l,'source'), t = lid(l,'target');
    if (pathHighlight.size && pathHighlight.has(s) && pathHighlight.has(t)) return 4;
    return l.kind==='calls' ? 2 : 1;
  }})
  .linkDirectionalParticleWidth(l => {{
    const s = lid(l,'source'), t = lid(l,'target');
    if (pathHighlight.size && pathHighlight.has(s) && pathHighlight.has(t)) return 3;
    return 1.5;
  }})
  .onNodeClick((n, event) => {{
    // ForceGraph exposes click events but no double-click chain method.
    // The native event detail increments for a double click, so use it to
    // preserve neighbor expansion without depending on a nonexistent API.
    if (event && event.detail >= 2) {{
      const links = activePayload().links;
      expandIds = neighborExpand(n.id, links, 1);
      redraw();
      return;
    }}
    showDetail(n);
    if (selected.length === 1 && selected[0] === n.id) {{ selected = []; pathHighlight = new Set(); redraw(); return; }}
    if (selected.length >= 2) selected = [];
    selected.push(n.id);
    if (selected.length === 2) {{
      const fd = filtered();
      const path = bfsPath(selected[0], selected[1], fd.links.length ? fd.links : activePayload().links);
      pathHighlight = path ? new Set(path) : new Set();
      showDetail(n);
    }}
    redraw();
  }});

function updateCounts(fd) {{
 const totalNodes = (activePayload().nodes || []).length;
 const totalEdges = (activePayload().links || []).length;
 const shownNodes = (fd.nodes || []).length;
 const shownEdges = (fd.links || []).length;
 const el = document.getElementById("counts");
 if (el) el.textContent = "Nodes: " + shownNodes + " / " + totalNodes + " · Edges: " + shownEdges + " / " + totalEdges + " · Filtered: " + Math.max(0, totalNodes - shownNodes) + (expandIds ? " · neighborhood focus" : "");
}}

function fitView() {{
  if (g && typeof g.zoomToFit === 'function') {{
    requestAnimationFrame(() => {{
      try {{ g.zoomToFit(400, 40); }} catch (err) {{ /* vendor stub */ }}
    }});
  }}
}}

function redraw() {{
 const fd = filtered();
 g.graphData(fd);
 g.nodeAutoColorBy(n => colorKey(n));
 g.width(elem.clientWidth).height(elem.clientHeight);
 updateCounts(fd);
}}

function renderDeadList() {{
  const ul = document.getElementById('deadList');
  const sort = document.getElementById('deadSort').value;
  const items = [...(DATA.dead_code||[])];
  items.sort((a,b) => String(a[sort]||'').localeCompare(String(b[sort]||'')));
  ul.innerHTML = items.slice(0,500).map(d =>
    '<li data-id="'+escapeHtml(d.id)+'"><span class="flag-dead">['+escapeHtml(d.confidence)+']</span> '+escapeHtml(d.id)+'<br/><span class="muted">'+escapeHtml(d.path)+':'+(d.line||0)+' — '+escapeHtml(d.reason||'')+'</span></li>'
  ).join('') || '<li class="muted">(none)</li>';
  ul.querySelectorAll('li[data-id]').forEach(li => li.addEventListener('click', () => {{
    const id = li.getAttribute('data-id');
    mode = id.includes('::') ? 'symbol' : 'file';
    document.getElementById('mode').value = mode;
    expandIds = new Set([id]);
    document.querySelector('.tab[data-tab="graph"]').click();
    refillArea();
    redraw();
    const node = (activePayload().nodes||[]).find(n => n.id === id);
    if (node) showDetail(node);
  }}));
}}

function renderCommunities() {{
  const ul = document.getElementById('commList');
  const p = activePayload();
  const items = p.communities || [];
  document.getElementById('commHint').textContent = DATA.meta && DATA.meta.has_communities
    ? 'Louvain communities (or area fallback).'
    : 'Communities not computed yet — showing area groupings.';
  ul.innerHTML = items.map(c =>
    '<li data-c="'+escapeHtml(c.id)+'">'+escapeHtml(c.label)+' <span class="muted">('+c.size+')</span></li>'
  ).join('') || '<li class="muted">(none)</li>';
  ul.querySelectorAll('li[data-c]').forEach(li => li.addEventListener('click', () => {{
    document.getElementById('area').value = li.getAttribute('data-c');
    document.querySelector('.tab[data-tab="graph"]').click();
    redraw();
  }}));
}}

function renderProcesses() {{
  const ul = document.getElementById('procList');
  const procs = DATA.processes || [];
  document.getElementById('procHint').textContent = procs.length
    ? 'Click a process to focus its steps.'
    : 'No processes in graph meta yet (intel phase). Entry roots: '+((((DATA.meta&&DATA.meta.entry_roots)||[]).slice(0,8).join(', '))||'(none)');
  ul.innerHTML = procs.map((p,i) => {{
    const name = p.name || p.entry || ('process '+i);
    const steps = (p.steps || p.nodes || []).slice(0,20);
    return '<li data-i="'+i+'"><strong>'+escapeHtml(name)+'</strong><br/><span class="muted">'+escapeHtml(steps.join(' → '))+'</span></li>';
  }}).join('') || '<li class="muted">(none — entry roots listed above)</li>';
  ul.querySelectorAll('li[data-i]').forEach(li => li.addEventListener('click', () => {{
    const p = procs[Number(li.getAttribute('data-i'))];
    const steps = p.steps || p.nodes || [];
    if (steps.length) {{
      expandIds = new Set(steps);
      mode = String(steps[0]).includes('::') ? 'symbol' : 'file';
      document.getElementById('mode').value = mode;
      document.querySelector('.tab[data-tab="graph"]').click();
      refillArea();
      redraw();
    }}
  }}));
}}

function focusNode(id) {{
  mode = String(id).includes('::') ? 'symbol' : 'file';
  document.getElementById('mode').value = mode;
  expandIds = new Set([id]);
  document.querySelector('.tab[data-tab="graph"]').click();
  refillArea();
  redraw();
  const node = (activePayload().nodes||[]).find(n => n.id === id);
  if (node) showDetail(node);
}}

function renderIntel() {{
  const hot = document.getElementById('hotList');
  hot.innerHTML = (DATA.hotspots||[]).map(h =>
    '<li data-id="'+escapeHtml(h.path)+'"><strong>'+escapeHtml(h.path)+'</strong><br/><span class="muted">score '+h.score+' — churn '+h.churn+', fan-in '+h.fan_in+'</span></li>'
  ).join('') || '<li class="muted">(no churn data — needs git history)</li>';
  const god = document.getElementById('godList');
  god.innerHTML = (DATA.god_nodes||[]).map(gn =>
    '<li data-id="'+escapeHtml(gn.id)+'"><strong>'+escapeHtml(gn.name||gn.path||gn.id)+'</strong><br/><span class="muted">deg '+gn.degree+' (in '+(gn.fan_in||0)+' / out '+(gn.fan_out||0)+')'+(gn.pagerank?' — pr '+gn.pagerank:'')+'</span></li>'
  ).join('') || '<li class="muted">(none)</li>';
  const cyc = document.getElementById('cycleList');
  cyc.innerHTML = (DATA.circular_imports||[]).map(c =>
    '<li><span class="flag-dead">len '+c.length+'</span> <span class="muted">'+escapeHtml((c.nodes||[]).join(' → '))+'</span></li>'
  ).join('') || '<li class="muted">(none)</li>';
  [...hot.querySelectorAll('li[data-id]'), ...god.querySelectorAll('li[data-id]')].forEach(li =>
    li.addEventListener('click', () => focusNode(li.getAttribute('data-id'))));
}}

document.querySelectorAll('.tab').forEach(tab => tab.addEventListener('click', () => {{
  document.querySelectorAll('.tab').forEach(t => t.classList.remove('active'));
  document.querySelectorAll('.panel').forEach(p => p.classList.remove('active'));
  tab.classList.add('active');
  document.getElementById('panel-'+tab.dataset.tab).classList.add('active');
  if (tab.dataset.tab === 'dead') renderDeadList();
  if (tab.dataset.tab === 'communities') renderCommunities();
  if (tab.dataset.tab === 'processes') renderProcesses();
  if (tab.dataset.tab === 'intel') renderIntel();
}}));

document.getElementById('mode').addEventListener('change', () => {{
  mode = document.getElementById('mode').value;
  expandIds = null;
  selected = [];
  pathHighlight = new Set();
  refillArea();
  redraw();
  renderCommunities();
}});
['search','area','ekind','econf','lensDead','lensUnwired','lensUnreach','deadConf','colorBy','deadSort'].forEach(id => {{
  const el = document.getElementById(id);
  el.addEventListener('input', redraw);
  el.addEventListener('change', () => {{ redraw(); if (id==='deadSort') renderDeadList(); }});
}});
document.getElementById('reset').addEventListener('click', () => {{
  document.getElementById('search').value='';
  document.getElementById('area').value='';
  document.getElementById('ekind').value='';
  document.getElementById('econf').value='';
  document.getElementById('lensDead').checked=false;
  document.getElementById('lensUnwired').checked=false;
  document.getElementById('lensUnreach').checked=false;
  document.getElementById('deadConf').value='';
  expandIds = null;
  selected = [];
  pathHighlight = new Set();
  redraw();
  fitView();
}});
document.getElementById('clearPath').addEventListener('click', () => {{
  selected = [];
  pathHighlight = new Set();
  redraw();
}});
window.addEventListener('resize', () => g.width(elem.clientWidth).height(elem.clientHeight));
if (!g || typeof g.zoomToFit !== 'function') document.getElementById('vendorWarn').style.display = 'block';
refillArea();
redraw();
fitView();
renderDeadList();
renderCommunities();
renderProcesses();
renderIntel();
</script>
</body>
</html>
"""


def write_graph_html(
    root: Path,
    *,
    open_browser: bool = False,
    symbols: bool = False,
) -> Path:
    """Write ``.devcouncil/graph/graph.html`` from the on-disk code graph."""
    graph = load_code_graph(root)
    if graph is None:
        raise FileNotFoundError("No code graph found; run `dev map` first.")
    out = graph_path(root).with_name("graph.html")
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(render_graph_html(graph, file_level=not symbols), encoding="utf-8")
    if open_browser:
        import webbrowser

        webbrowser.open(out.resolve().as_uri())
    return out


def sample_demo_graph() -> CodeGraph:
    """Return a deterministic graph that exercises every visualizer surface."""
    nodes = [
        GraphNode(
            id="src/devcouncil/cli/main.py",
            kind=NodeKind.FILE,
            path="src/devcouncil/cli/main.py",
            name="main.py",
            area="cli",
            language="python",
            community="demo-community",
        ),
        GraphNode(
            id="src/devcouncil/execution/task_runner.py",
            kind=NodeKind.FILE,
            path="src/devcouncil/execution/task_runner.py",
            name="task_runner.py",
            area="execution",
            language="python",
            community="demo-community",
        ),
        GraphNode(
            id="src/devcouncil/verification/verifier.py",
            kind=NodeKind.FILE,
            path="src/devcouncil/verification/verifier.py",
            name="verifier.py",
            area="verification",
            language="python",
            community="verification",
        ),
        GraphNode(
            id="src/devcouncil/verification/unused_check.py",
            kind=NodeKind.FILE,
            path="src/devcouncil/verification/unused_check.py",
            name="unused_check.py",
            area="verification",
            language="python",
            community="verification",
        ),
        GraphNode(
            id="src/devcouncil/cli/main.py::main",
            kind=NodeKind.FUNCTION,
            path="src/devcouncil/cli/main.py",
            name="main",
            line=20,
            end_line=34,
            area="cli",
            language="python",
            community="demo-community",
        ),
        GraphNode(
            id="src/devcouncil/execution/task_runner.py::run_task",
            kind=NodeKind.FUNCTION,
            path="src/devcouncil/execution/task_runner.py",
            name="run_task",
            line=42,
            end_line=78,
            area="execution",
            language="python",
            community="demo-community",
        ),
        GraphNode(
            id="src/devcouncil/verification/verifier.py::verify",
            kind=NodeKind.FUNCTION,
            path="src/devcouncil/verification/verifier.py",
            name="verify",
            line=55,
            end_line=92,
            area="verification",
            language="python",
            community="verification",
        ),
        GraphNode(
            id="src/devcouncil/verification/unused_check.py::unused_check",
            kind=NodeKind.FUNCTION,
            path="src/devcouncil/verification/unused_check.py",
            name="unused_check",
            line=8,
            end_line=12,
            area="verification",
            language="python",
            community="verification",
        ),
    ]
    edges = [
        GraphEdge(
            source="src/devcouncil/cli/main.py",
            target="src/devcouncil/execution/task_runner.py",
            kind="imports",
        ),
        GraphEdge(
            source="src/devcouncil/execution/task_runner.py",
            target="src/devcouncil/verification/verifier.py",
            kind="imports",
        ),
        GraphEdge(
            source="src/devcouncil/cli/main.py::main",
            target="src/devcouncil/execution/task_runner.py::run_task",
            kind="calls",
        ),
        GraphEdge(
            source="src/devcouncil/execution/task_runner.py::run_task",
            target="src/devcouncil/verification/verifier.py::verify",
            kind="calls",
        ),
    ]
    return CodeGraph(
        nodes=nodes,
        edges=edges,
        dead_code=[
            DeadCodeEntry(
                id="src/devcouncil/verification/unused_check.py::unused_check",
                path="src/devcouncil/verification/unused_check.py",
                line=8,
                kind="function",
                confidence=Confidence.EXTRACTED,
                reason="no inbound calls and token-scan agrees",
            )
        ],
        entry_roots=["src/devcouncil/cli/main.py"],
        unwired_candidates=["src/devcouncil/verification/unused_check.py"],
        meta={
            "processes": [
                {
                    "name": "entry_flow",
                    "entry": "src/devcouncil/cli/main.py",
                    "steps": [
                        "src/devcouncil/cli/main.py::main",
                        "src/devcouncil/execution/task_runner.py::run_task",
                        "src/devcouncil/verification/verifier.py::verify",
                    ],
                }
            ]
        },
    )


def render_graph_preview_svg(graph: CodeGraph | None = None) -> str:
    """Render a small dependency-flow preview without browser or JS support."""
    from html import escape

    demo = graph or sample_demo_graph()
    files = [node for node in demo.nodes if node.kind == NodeKind.FILE][:4]
    positions = {
        node.id: (90 + index * 210, 150 + (index % 2) * 95)
        for index, node in enumerate(files)
    }
    edge_lines: list[str] = []
    for edge in demo.edges:
        if edge.kind != "imports" or edge.source not in positions or edge.target not in positions:
            continue
        x1, y1 = positions[edge.source]
        x2, y2 = positions[edge.target]
        edge_lines.append(
            f'<path d="M{x1 + 66},{y1} L{x2 - 66},{y2}" stroke="#64748b" '
            'stroke-width="3" fill="none" marker-end="url(#arrow)"/>'
        )
    node_groups: list[str] = []
    dead_paths = {entry.path for entry in demo.dead_code}
    for node in files:
        x, y = positions[node.id]
        color = "#e35d6a" if node.path in dead_paths else "#3d8bfd"
        label = escape(node.name or node.path)
        node_groups.append(
            f'<g><rect x="{x - 68}" y="{y - 28}" width="136" height="56" rx="10" '
            f'fill="{color}"/><text x="{x}" y="{y + 5}" text-anchor="middle" '
            f'font-family="ui-sans-serif,system-ui" font-size="13" fill="#ffffff">{label}</text></g>'
        )
    return "\n".join(
        [
            '<svg xmlns="http://www.w3.org/2000/svg" width="860" height="360" viewBox="0 0 860 360" role="img" aria-labelledby="title desc">',
            '<title id="title">DevCouncil Code Graph</title>',
            '<desc id="desc">Sample entry, execution, verification, and dead-code dependency flow.</desc>',
            '<rect width="860" height="360" fill="#0f1419"/>',
            '<defs><marker id="arrow" markerWidth="8" markerHeight="8" refX="6" refY="3" orient="auto"><path d="M0,0 L0,6 L7,3 z" fill="#64748b"/></marker></defs>',
            '<text x="36" y="48" font-family="ui-sans-serif,system-ui" font-size="24" font-weight="700" fill="#e7ecf3">DevCouncil Code Graph</text>',
            '<text x="36" y="75" font-family="ui-sans-serif,system-ui" font-size="13" fill="#8b9bb4">Native dependency and liveness preview</text>',
            *edge_lines,
            *node_groups,
            '</svg>',
        ]
    )


def write_graph_demo(root: Path, *, open_browser: bool = False) -> dict[str, Path]:
    """Write deterministic HTML and SVG demo artifacts without requiring a map."""
    out_dir = root / ".devcouncil" / "graph"
    out_dir.mkdir(parents=True, exist_ok=True)
    graph = sample_demo_graph()
    html_path = out_dir / "demo.html"
    svg_path = out_dir / "demo.svg"
    html_path.write_text(render_graph_html(graph), encoding="utf-8")
    svg_path.write_text(render_graph_preview_svg(graph), encoding="utf-8")
    if open_browser:
        import webbrowser

        webbrowser.open(html_path.resolve().as_uri())
    return {"html": html_path, "svg": svg_path}
