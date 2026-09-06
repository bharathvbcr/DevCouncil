"""Self-contained interactive HTML visualizer for the repository map (subsystems)."""

from __future__ import annotations

import json
import logging
from pathlib import Path
from typing import Any, Dict, List, Mapping, MutableMapping, Optional, Sequence, Tuple

from devcouncil.indexing.subsystem_map import (
    handoff_paths_established,
    role_files_established,
)
from devcouncil.indexing.viz import _canvas_controls_css, _canvas_controls_js, _vendor_js

logger = logging.getLogger(__name__)

# Keep the embedded HTML small — full liveness lists can be tens of thousands.
_LIVENESS_VIZ_CAP = 256
_ROLE_FILES_CAP = 24
_ENTRY_CRITICAL_CAP = 40


def _as_mapping(obj: Any) -> Mapping[str, Any]:
    if obj is None:
        return {}
    if isinstance(obj, Mapping):
        return obj
    dump = getattr(obj, "model_dump", None)
    if callable(dump):
        data = dump()
        return data if isinstance(data, Mapping) else {}
    return {}


def _cap_list(values: Sequence[Any] | None, limit: int) -> List[Any]:
    if not values:
        return []
    return list(values)[:limit]


def _strip_glob(hint: str) -> str:
    text = (hint or "").strip().replace("\\", "/")
    if text.endswith("/*"):
        text = text[:-2]
    return text.strip().strip("/")


def match_area(hint: str, areas: Sequence[str]) -> Optional[str]:
    """Longest-prefix / suffix match of a handoff path fragment to a subsystem area."""
    cleaned = _strip_glob(hint)
    if not cleaned:
        return None
    area_list = [a.replace("\\", "/") for a in areas if a]
    if cleaned in area_list:
        return cleaned
    best: Optional[str] = None
    best_score = -1
    for area in area_list:
        if area == cleaned or area.endswith("/" + cleaned) or cleaned.startswith(area + "/"):
            score = len(cleaned) if cleaned in area or area.endswith(cleaned) else len(area)
            if score > best_score:
                best_score = score
                best = area
            continue
        parts = area.split("/")
        for i in range(len(parts)):
            suffix = "/".join(parts[i:])
            if (
                cleaned == suffix
                or cleaned.startswith(suffix + "/")
                or suffix.startswith(cleaned + "/")
                or suffix == cleaned
            ):
                score = len(suffix)
                if score > best_score:
                    best_score = score
                    best = area
    return best


def resolve_handoff(
    handoff: str,
    areas: Sequence[str],
) -> Tuple[Optional[str], Optional[str], str]:
    """Parse ``A -> B`` handoff text into (source_area, target_area, display)."""
    raw = (handoff or "").strip()
    if " -> " not in raw:
        return None, None, raw
    left, right = raw.split(" -> ", 1)
    src = match_area(left, areas)
    dst = match_area(right, areas)
    return src, dst, raw


def build_map_viz_payload(repo_map: Any) -> Dict[str, Any]:
    """Build a slim map visualizer payload (no ``files[]`` / ``dependents{}``)."""
    data = _as_mapping(repo_map)
    subsystems_raw = data.get("subsystems") or []
    if not isinstance(subsystems_raw, list):
        subsystems_raw = []

    subsystems: List[Dict[str, Any]] = []
    areas: List[str] = []
    for sub in subsystems_raw:
        if not isinstance(sub, Mapping):
            continue
        area = str(sub.get("area") or "").replace("\\", "/")
        if not area:
            continue
        areas.append(area)
        role_files = sub.get("role_files") or {}
        roles: Dict[str, List[str]] = {}
        if isinstance(role_files, Mapping):
            for role, paths in role_files.items():
                if isinstance(paths, list):
                    roles[str(role)] = [str(p) for p in paths[:_ROLE_FILES_CAP]]
        subsystems.append(
            {
                "id": area,
                "area": area,
                "name": area.rsplit("/", 1)[-1] or area,
                "summary": str(sub.get("summary") or ""),
                "entry_points": _cap_list(sub.get("entry_points") or [], _ENTRY_CRITICAL_CAP),
                "critical_files": _cap_list(sub.get("critical_files") or [], _ENTRY_CRITICAL_CAP),
                "neighbors": [str(n).replace("\\", "/") for n in (sub.get("neighbors") or []) if n],
                "handoff_paths": [str(h) for h in (sub.get("handoff_paths") or []) if h],
                "role_files": roles,
            }
        )

    area_set = set(areas)
    nodes = [
        {
            "id": s["id"],
            "name": s["name"],
            "area": s["area"],
            "summary": s["summary"],
            "val": max(1, len(s["neighbors"]) + len(s["handoff_paths"]) + len(s["entry_points"])),
            "entry": bool(s["entry_points"]),
            "flags": [],
        }
        for s in subsystems
    ]

    links: List[Dict[str, Any]] = []
    seen_edges: set[Tuple[str, str, str]] = set()
    unresolved_handoffs: List[Dict[str, str]] = []

    for s in subsystems:
        src = s["id"]
        for neighbor in s["neighbors"]:
            dst = neighbor if neighbor in area_set else match_area(neighbor, areas)
            if not dst or dst == src:
                continue
            key = (src, dst, "neighbor")
            rev = (dst, src, "neighbor")
            if key in seen_edges or rev in seen_edges:
                continue
            seen_edges.add(key)
            links.append({"source": src, "target": dst, "kind": "neighbor"})
        for handoff in s["handoff_paths"]:
            h_src, h_dst, display = resolve_handoff(handoff, areas)
            source = h_src or src
            if not h_dst:
                unresolved_handoffs.append({"from": src, "text": display})
                continue
            if source not in area_set:
                source = src
            if h_dst == source:
                continue
            key = (source, h_dst, "handoff")
            if key in seen_edges:
                continue
            seen_edges.add(key)
            links.append(
                {
                    "source": source,
                    "target": h_dst,
                    "kind": "handoff",
                    "label": display,
                }
            )

    return {
        "nodes": nodes,
        "links": links,
        "subsystems": subsystems,
        "unresolved_handoffs": unresolved_handoffs[:100],
        "liveness": {
            "entry_roots": _cap_list(data.get("entry_roots") or [], _LIVENESS_VIZ_CAP),
            "unwired_candidates": _cap_list(data.get("unwired_candidates") or [], _LIVENESS_VIZ_CAP),
            "unreachable_files": _cap_list(data.get("unreachable_files") or [], _LIVENESS_VIZ_CAP),
            "dead_symbol_candidates": _cap_list(
                data.get("dead_symbol_candidates") or [], _LIVENESS_VIZ_CAP
            ),
            "liveness_unreachable_unreliable": bool(
                data.get("liveness_unreachable_unreliable") or False
            ),
        },
        "meta": {
            "languages": _cap_list(data.get("languages") or [], 32),
            "generated_head": str(data.get("generated_head") or ""),
            "graph_html": "graph/graph.html",
            # Whether the producer derived the crossing relation. The detail
            # pane printed "(none)" for an empty list, which is the one reading
            # an un-derived field cannot support.
            "handoff_paths_computed": handoff_paths_established(data),
            "role_files_computed": role_files_established(data),
        },
    }


def render_map_html(repo_map: Any) -> str:
    """Return a self-contained subsystem map HTML document (no network)."""
    payload = build_map_viz_payload(repo_map)
    raw = json.dumps(payload, separators=(",", ":"), ensure_ascii=False)
    raw = raw.replace("<", "\\u003c").replace(">", "\\u003e").replace("&", "\\u0026")
    vendor = _vendor_js()
    html = f"""<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8"/>
<meta name="viewport" content="width=device-width, initial-scale=1"/>
<title>DevCouncil Repo Map</title>
<style>
:root {{ --bg:#0f1419; --panel:#1a2332; --fg:#e7ecf3; --muted:#8b9bb4; --accent:#3d8bfd; --dead:#e35d6a; --entry:#34d399; }}
* {{ box-sizing:border-box; }}
body {{ margin:0; font:14px/1.4 ui-sans-serif,system-ui,sans-serif; background:var(--bg); color:var(--fg); display:flex; height:100vh; }}
#sidebar {{ width:360px; background:var(--panel); padding:12px; overflow:auto; border-right:1px solid #243044; display:flex; flex-direction:column; }}
#graph {{ flex:1; position:relative; }}
h1 {{ font-size:16px; margin:0 0 8px; }}
h2 {{ font-size:13px; margin:12px 0 6px; color:var(--muted); font-weight:600; }}
input,select,button {{ width:100%; margin:4px 0 8px; padding:6px 8px; background:#0f1419; color:var(--fg); border:1px solid #334155; border-radius:4px; }}
button.primary {{ cursor:pointer; background:var(--accent); border:none; font-weight:600; }}
a.link {{ color:var(--accent); text-decoration:none; }}
a.link:hover {{ text-decoration:underline; }}
label {{ color:var(--muted); font-size:12px; display:block; }}
#detail {{ margin-top:8px; font-size:12px; color:var(--muted); white-space:pre-wrap; }}
#counts {{ margin:8px 0 4px; font-size:12px; color:var(--accent); font-weight:600; }}
.badge {{ display:inline-block; padding:2px 6px; border-radius:3px; font-size:11px; margin:2px 4px 2px 0; background:#0f1419; border:1px solid #334155; }}
.badge.entry {{ border-color:var(--entry); color:var(--entry); }}
.badge.dead {{ border-color:var(--dead); color:var(--dead); }}
.badge.warn {{ border-color:#f59e0b; color:#f59e0b; }}
.list {{ list-style:none; padding:0; margin:0; font-size:12px; }}
.list li {{ padding:4px 0; border-bottom:1px solid #243044; cursor:pointer; }}
.list li:hover {{ color:var(--accent); }}
.muted {{ color:var(--muted); }}
.interaction-hint {{ margin:8px 0; padding:8px; border:1px solid #334155; border-radius:4px; background:#0f1419; color:var(--fg); font-size:12px; line-height:1.45; }}
/*__CANVAS_CONTROLS_CSS__*/
</style>
</head>
<body>
<aside id="sidebar">
  <h1>DevCouncil Repo Map</h1>
  <div id="counts" aria-live="polite">Subsystems: -- · Edges: --</div>
  <div class="interaction-hint"><strong>Click</strong> a subsystem for entry points, neighbors, and handoffs. Open the <a class="link" href="graph/graph.html">code graph</a> for file/symbol detail.</div>
  <label>Search</label>
  <input id="search" placeholder="area or summary" autocomplete="off"/>
  <div id="livenessBadges"></div>
  <h2>Subsystems</h2>
  <ul class="list" id="subList"></ul>
  <h2>Detail</h2>
  <div id="detail" class="muted">Select a subsystem to inspect.</div>
</aside>
<div id="graph">
  <div id="vendorWarn" style="display:none;position:absolute;inset:0;z-index:10;padding:40px;color:var(--dead);font-size:14px;background:var(--bg)">
    force-graph vendor bundle is missing — the canvas cannot render.<br/>
    Expected at <code>src/devcouncil/assets/vendor/force-graph.min.js</code>.
  </div>
</div>
<script>{vendor}</script>
<script>
const DATA = {raw};
let selected = [];

function escapeHtml(s) {{
  return String(s||'').replace(/[&<>"']/g, c => ({{'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}}[c]));
}}

function lid(l, key) {{
  const v = l[key];
  return typeof v === 'object' && v ? v.id : v;
}}

function subsystemById(id) {{
  return (DATA.subsystems || []).find(s => s.id === id);
}}

function filtered() {{
  const q = (document.getElementById('search').value||'').toLowerCase();
  let nodes = (DATA.nodes || []).filter(n => {{
    if (!q) return true;
    const sub = subsystemById(n.id) || {{}};
    return n.id.toLowerCase().includes(q)
      || (n.name||'').toLowerCase().includes(q)
      || (sub.summary||'').toLowerCase().includes(q);
  }});
  const ids = new Set(nodes.map(n => n.id));
  const links = (DATA.links || []).filter(l => ids.has(lid(l,'source')) && ids.has(lid(l,'target')));
  return {{nodes, links}};
}}

function showDetail(id) {{
  const s = subsystemById(id);
  const d = document.getElementById('detail');
  if (!s) {{ d.textContent = 'No detail.'; return; }}
  let html = '<strong>'+escapeHtml(s.area)+'</strong>\\n';
  if (s.summary) html += escapeHtml(s.summary)+'\\n\\n';
  html += 'entry points:\\n'+(s.entry_points||[]).slice(0,12).map(p => '  · '+escapeHtml(p)).join('\\n')+'\\n';
  html += 'critical files:\\n'+(s.critical_files||[]).slice(0,12).map(p => '  · '+escapeHtml(p)).join('\\n')+'\\n';
  html += 'neighbors:\\n'+((s.neighbors||[]).map(p => '  · '+escapeHtml(p)).join('\\n') || '  (none)')+'\\n';
  const handoffFallback = (DATA.meta && DATA.meta.handoff_paths_computed) ? '  (none)' : '  (not computed for this map)';
  html += 'handoffs:\\n'+((s.handoff_paths||[]).map(p => '  · '+escapeHtml(p)).join('\\n') || handoffFallback)+'\\n';
  const roles = s.role_files || {{}};
  const roleKeys = Object.keys(roles);
  if (!roleKeys.length && !(DATA.meta && DATA.meta.role_files_computed)) {{
    html += 'roles:\\n  (not computed for this map)\\n';
  }}
  if (roleKeys.length) {{
    html += 'roles:\\n';
    roleKeys.forEach(r => {{
      html += '  '+escapeHtml(r)+': '+escapeHtml((roles[r]||[]).slice(0,6).join(', '))+'\\n';
    }});
  }}
  const unresolved = (DATA.unresolved_handoffs || []).filter(u => u.from === id);
  if (unresolved.length) {{
    html += '\\nunresolved handoffs (text only):\\n';
    unresolved.forEach(u => {{ html += '  · '+escapeHtml(u.text)+'\\n'; }});
  }}
  d.innerHTML = html;
}}

function renderLiveness() {{
  const live = DATA.liveness || {{}};
  const el = document.getElementById('livenessBadges');
  const parts = [];
  parts.push('<span class="badge entry">entry '+((live.entry_roots||[]).length)+'</span>');
  parts.push('<span class="badge dead">unwired '+((live.unwired_candidates||[]).length)+'</span>');
  parts.push('<span class="badge">unreachable '+((live.unreachable_files||[]).length)+'</span>');
  parts.push('<span class="badge dead">dead symbols '+((live.dead_symbol_candidates||[]).length)+'</span>');
  if (live.liveness_unreachable_unreliable) parts.push('<span class="badge warn">unreachable unreliable</span>');
  el.innerHTML = parts.join('');
}}

function renderSubList(fd) {{
  const ul = document.getElementById('subList');
  ul.innerHTML = (fd.nodes||[]).map(n =>
    '<li data-id="'+escapeHtml(n.id)+'">'+escapeHtml(n.area || n.id)+'</li>'
  ).join('') || '<li class="muted">(none)</li>';
  ul.querySelectorAll('li[data-id]').forEach(li => li.addEventListener('click', () => {{
    selected = [li.getAttribute('data-id')];
    showDetail(selected[0]);
    redraw();
  }}));
}}

const elem = document.getElementById('graph');
const Graph = ForceGraph();
const g = Graph(elem)
  .nodeId('id')
  .nodeLabel(n => n.area || n.id)
  .nodeAutoColorBy(n => n.area || n.id)
  .nodeVal(n => n.val || 1)
  .linkColor(l => l.kind === 'handoff' ? '#a78bfa' : '#3d8bfd')
  .linkDirectionalParticles(l => l.kind === 'handoff' ? 2 : 1)
  .linkDirectionalParticleWidth(1.5)
  .onNodeClick((n) => {{
    selected = [n.id];
    showDetail(n.id);
    redraw();
  }});

window.__dcCanvasOpts = {{
  getSelectedIds: () => selected.slice(),
  legendHtml:
    '<div class="leg-row"><span class="swatch edge-neighbor"></span> neighbor</div>'
    + '<div class="leg-row"><span class="swatch edge-handoff"></span> handoff</div>'
    + '<div class="leg-row"><span class="swatch entry"></span> has entry points</div>',
  onEscape: () => {{ selected = []; redraw(); document.getElementById('detail').textContent = 'Select a subsystem to inspect.'; }}
}};
/*__CANVAS_CONTROLS_JS__*/

function fitView() {{
  if (g && typeof g.zoomToFit === 'function') {{
    requestAnimationFrame(() => {{
      try {{ g.zoomToFit(400, 40); }} catch (err) {{ /* vendor stub */ }}
    }});
  }}
}}

function updateCounts(fd) {{
  const el = document.getElementById('counts');
  if (el) el.textContent = 'Subsystems: '+(fd.nodes||[]).length+' / '+((DATA.nodes||[]).length)
    + ' · Edges: '+(fd.links||[]).length+' / '+((DATA.links||[]).length);
}}

function redraw() {{
  const fd = filtered();
  g.graphData(fd);
  g.width(elem.clientWidth).height(elem.clientHeight);
  if (typeof window.__dcApplyNodeStyle === 'function') window.__dcApplyNodeStyle();
  updateCounts(fd);
  renderSubList(fd);
}}

document.getElementById('search').addEventListener('input', redraw);
window.addEventListener('resize', () => g.width(elem.clientWidth).height(elem.clientHeight));
if (!g || typeof g.zoomToFit !== 'function') document.getElementById('vendorWarn').style.display = 'block';
renderLiveness();
redraw();
fitView();
</script>
</body>
</html>
"""
    return (
        html.replace("/*__CANVAS_CONTROLS_CSS__*/", _canvas_controls_css())
        .replace("/*__CANVAS_CONTROLS_JS__*/", _canvas_controls_js())
    )


def write_map_html(
    root: Path,
    *,
    open_browser: bool = False,
    repo_map: Any | None = None,
) -> Path:
    """Write ``.devcouncil/map.html`` from the on-disk repo map."""
    data = repo_map
    if data is None:
        map_path = root / ".devcouncil" / "repo_map.json"
        if not map_path.is_file():
            raise FileNotFoundError("No repo map found; run `dev map` first.")
        from devcouncil.utils.json_persist import read_json

        data = read_json(map_path) or {}
        if not isinstance(data, (Mapping, MutableMapping)):
            raise FileNotFoundError("No repo map found; run `dev map` first.")
    out = root / ".devcouncil" / "map.html"
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(render_map_html(data), encoding="utf-8")
    if open_browser:
        import webbrowser

        webbrowser.open(out.resolve().as_uri())
    return out
