"""Export the code knowledge graph as attributed GraphML or an OKF v0.1 bundle.

devcouncil: allow-unwired — package-private; reached via graph CLI / okf_export facade.
"""

from __future__ import annotations

import os
from datetime import UTC, datetime
from pathlib import Path, PurePosixPath
from typing import Any, Dict, List, Optional, Set, Tuple

from devcouncil.indexing.graph.schema import CodeGraph, GraphNode
from devcouncil.knowledge.okf import OKFBundle, OKFDocument, write_bundle


def _xml(s: str) -> str:
    return (
        str(s)
        .replace("&", "&amp;")
        .replace('"', "&quot;")
        .replace("<", "&lt;")
        .replace(">", "&gt;")
    )


def _kind_val(obj: Any) -> str:
    if hasattr(obj, "value"):
        return str(obj.value)
    return str(obj or "")


def _dead_ids(graph: CodeGraph) -> Set[str]:
    return {d.id for d in graph.dead_code}


def file_doc_rel(path: str) -> str:
    """Bundle-relative OKF path for a code file (shared with wiki Wired-to links)."""
    from devcouncil.indexing.graph.export_links import file_doc_path

    return file_doc_path(path)


def subsystem_doc_rel(area: str) -> str:
    """Bundle-relative OKF path for a subsystem (wiki-compatible slug)."""
    from devcouncil.indexing.graph.export_links import subsystem_doc_path

    return subsystem_doc_path(area)


def _rel_link(from_rel: str, to_rel: str) -> str:
    """POSIX relative link from one bundle doc to another."""
    src = PurePosixPath(from_rel).parent
    return os.path.relpath(to_rel, start=str(src) or ".").replace("\\", "/")


def export_graphml(graph: CodeGraph) -> str:
    """GraphML with node/edge attributes (kind, confidence, area, community, dead)."""
    dead = _dead_ids(graph)
    unwired = set(graph.unwired_candidates)
    unreachable = set(graph.unreachable_files)
    lines = [
        '<?xml version="1.0" encoding="UTF-8"?>',
        '<graphml xmlns="http://graphml.graphdrawing.org/xmlns">',
        '  <key id="kind" for="node" attr.name="kind" attr.type="string"/>',
        '  <key id="path" for="node" attr.name="path" attr.type="string"/>',
        '  <key id="name" for="node" attr.name="name" attr.type="string"/>',
        '  <key id="area" for="node" attr.name="area" attr.type="string"/>',
        '  <key id="community" for="node" attr.name="community" attr.type="string"/>',
        '  <key id="dead" for="node" attr.name="dead" attr.type="boolean"/>',
        '  <key id="unwired" for="node" attr.name="unwired" attr.type="boolean"/>',
        '  <key id="unreachable" for="node" attr.name="unreachable" attr.type="boolean"/>',
        '  <key id="ekind" for="edge" attr.name="kind" attr.type="string"/>',
        '  <key id="confidence" for="edge" attr.name="confidence" attr.type="string"/>',
        '  <graph id="G" edgedefault="directed">',
    ]
    for n in graph.nodes:
        nid = _xml(n.id)
        community = (n.community or "").strip() or (n.area or "")
        lines.append(f'    <node id="{nid}">')
        lines.append(f'      <data key="kind">{_xml(_kind_val(n.kind))}</data>')
        lines.append(f'      <data key="path">{_xml(n.path)}</data>')
        lines.append(f'      <data key="name">{_xml(n.name)}</data>')
        lines.append(f'      <data key="area">{_xml(n.area)}</data>')
        lines.append(f'      <data key="community">{_xml(community)}</data>')
        lines.append(f'      <data key="dead">{"true" if (n.id in dead) else "false"}</data>')
        lines.append(
            f'      <data key="unwired">{"true" if (n.path in unwired or n.id in unwired) else "false"}</data>'
        )
        lines.append(
            f'      <data key="unreachable">'
            f'{"true" if (n.path in unreachable or n.id in unreachable) else "false"}</data>'
        )
        lines.append("    </node>")
    for i, e in enumerate(graph.edges):
        lines.append(
            f'    <edge id="e{i}" source="{_xml(e.source)}" target="{_xml(e.target)}">'
        )
        lines.append(f'      <data key="ekind">{_xml(e.kind)}</data>')
        lines.append(f'      <data key="confidence">{_xml(_kind_val(e.confidence))}</data>')
        lines.append("    </edge>")
    lines.append("  </graph>")
    lines.append("</graphml>")
    return "\n".join(lines)


def _areas_from_graph(graph: CodeGraph) -> Dict[str, List[GraphNode]]:
    by_area: Dict[str, List[GraphNode]] = {}
    for n in graph.nodes:
        if _kind_val(n.kind) != "file":
            continue
        area = n.area or "root"
        by_area.setdefault(area, []).append(n)
    return by_area


def _import_targets(graph: CodeGraph) -> Dict[str, Set[str]]:
    out: Dict[str, Set[str]] = {}
    for e in graph.edges:
        if e.kind != "imports":
            continue
        if "::" in e.source or "::" in e.target:
            continue
        out.setdefault(e.source, set()).add(e.target)
    return out


def _call_targets(graph: CodeGraph) -> Dict[str, Set[str]]:
    out: Dict[str, Set[str]] = {}
    for e in graph.edges:
        if e.kind != "calls":
            continue
        out.setdefault(e.source, set()).add(e.target)
    return out


def build_code_graph_okf(
    graph: CodeGraph,
    *,
    project_name: str = "code-graph",
    timestamp: Optional[str] = None,
) -> OKFBundle:
    """Build an OKF v0.1 bundle describing subsystems and code files."""
    ts = timestamp or datetime.now(UTC).strftime("%Y-%m-%dT%H:%M:%SZ")
    docs: List[OKFDocument] = []
    by_area = _areas_from_graph(graph)
    imports_of = _import_targets(graph)
    calls_of = _call_targets(graph)
    dead = _dead_ids(graph)
    unwired = set(graph.unwired_candidates)
    unreachable = set(graph.unreachable_files)

    index_rel = "index.md"
    sub_links = "\n".join(
        f"- [{area}]({_rel_link(index_rel, subsystem_doc_rel(area))})"
        for area in sorted(by_area)
    )
    docs.append(
        OKFDocument(
            type="Code Graph",
            title=project_name,
            description="Symbol-level code knowledge graph export.",
            tags=["code-graph", "devcouncil"],
            timestamp=ts,
            rel_path=index_rel,
            body="\n".join(
                [
                    f"# {project_name}",
                    "",
                    f"Nodes: {len(graph.nodes)} · Edges: {len(graph.edges)}",
                    "",
                    "## Subsystems",
                    "",
                    sub_links or "_No file nodes._",
                    "",
                    "## Entry roots",
                    "",
                    "\n".join(f"- `{r}`" for r in graph.entry_roots[:50]) or "_None._",
                ]
            ),
        )
    )

    for area, files in sorted(by_area.items()):
        sub_rel = subsystem_doc_rel(area)
        file_links = "\n".join(
            f"- [{n.path}]({_rel_link(sub_rel, file_doc_rel(n.path))})"
            for n in sorted(files, key=lambda x: x.path)
        )
        neighbor_areas: Set[str] = set()
        for n in files:
            for tgt in imports_of.get(n.path, ()):
                for a2, flist in by_area.items():
                    if any(f.path == tgt for f in flist) and a2 != area:
                        neighbor_areas.add(a2)
        neighbor_links = "\n".join(
            f"- [{a}]({_rel_link(sub_rel, subsystem_doc_rel(a))})"
            for a in sorted(neighbor_areas)
        )
        docs.append(
            OKFDocument(
                type="Code Subsystem",
                title=area,
                description=f"Files in area {area}",
                resource=area,
                tags=["subsystem", *area.split("/")[:2]],
                timestamp=ts,
                rel_path=sub_rel,
                body="\n".join(
                    [
                        f"# {area}",
                        "",
                        "## Files",
                        "",
                        file_links,
                        "",
                        "## Wired to (neighbor areas)",
                        "",
                        neighbor_links or "_None._",
                    ]
                ),
            )
        )

    symbols_by_path: Dict[str, List[GraphNode]] = {}
    for n in graph.nodes:
        if _kind_val(n.kind) == "file":
            continue
        if n.path:
            symbols_by_path.setdefault(n.path, []).append(n)

    file_paths = {
        n.path for n in graph.nodes if _kind_val(n.kind) == "file" and n.path
    }

    for n in graph.nodes:
        if _kind_val(n.kind) != "file":
            continue
        path = n.path
        rel = file_doc_rel(path)
        flags = []
        if path in unwired:
            flags.append("unwired")
        if path in unreachable:
            flags.append("unreachable")
        if any(d.path == path for d in graph.dead_code):
            flags.append("has-dead-symbols")

        import_links = []
        for tgt in sorted(imports_of.get(path, ())):
            if tgt not in file_paths:
                continue
            import_links.append(
                f"- imports [{tgt}]({_rel_link(rel, file_doc_rel(tgt))})"
            )

        call_links = []
        for src_sym in symbols_by_path.get(path, []):
            for tgt in sorted(calls_of.get(src_sym.id, ())):
                tgt_path = tgt.split("::", 1)[0]
                if tgt_path not in file_paths:
                    continue
                call_links.append(
                    f"- `{src_sym.name}` calls `{tgt}` → "
                    f"[{tgt_path}]({_rel_link(rel, file_doc_rel(tgt_path))})"
                )

        sym_lines = []
        for s in sorted(symbols_by_path.get(path, []), key=lambda x: (x.line, x.name)):
            mark = " **dead**" if s.id in dead else ""
            sym_lines.append(
                f"- `{s.name}` ({_kind_val(s.kind)}:{s.line}){mark}"
            )

        area = n.area or "root"
        body_parts = [
            f"# {path}",
            "",
            f"Area: [{area}]({_rel_link(rel, subsystem_doc_rel(area))})",
            f"Community: {n.community or n.area or '—'}",
            f"Flags: {', '.join(flags) if flags else 'none'}",
            "",
            "## Symbols",
            "",
            "\n".join(sym_lines) or "_None._",
            "",
            "## Imports",
            "",
            "\n".join(import_links) or "_None._",
            "",
            "## Calls",
            "",
            "\n".join(call_links[:80]) or "_None._",
        ]
        docs.append(
            OKFDocument(
                type="Code File",
                title=Path(path).name,
                description=path,
                resource=path,
                tags=["file", n.area or "root", *(flags[:3])],
                timestamp=ts,
                rel_path=rel,
                body="\n".join(body_parts),
            )
        )

    from devcouncil.indexing.graph.export_links import relative_md_link

    sub_index_body = ["# Subsystems", ""] + [
        f"- {relative_md_link('subsystems/index.md', subsystem_doc_rel(area), area)}"
        for area in sorted(by_area)
    ]
    docs.append(
        OKFDocument(
            type="Index",
            title="Subsystems",
            description="Index of subsystem pages in the code-graph OKF bundle.",
            tags=["index", "code-graph"],
            timestamp=ts,
            rel_path="subsystems/index.md",
            body="\n".join(sub_index_body) if by_area else "# Subsystems\n\n_None._",
        )
    )
    file_nodes = [n for n in graph.nodes if _kind_val(n.kind) == "file"]
    file_index_body = ["# Code files", ""] + [
        f"- {relative_md_link('files/index.md', file_doc_rel(n.path), n.path)}"
        for n in sorted(file_nodes, key=lambda x: x.path)
    ]
    docs.append(
        OKFDocument(
            type="Index",
            title="Code files",
            description="Index of code-file pages in the code-graph OKF bundle.",
            tags=["index", "code-graph"],
            timestamp=ts,
            rel_path="files/index.md",
            body="\n".join(file_index_body) if file_nodes else "# Code files\n\n_None._",
        )
    )
    for d in docs:
        if d.rel_path == "index.md" and "## Subsystems" in d.body:
            d.body = d.body.replace(
                "## Subsystems",
                "## Indexes\n\n"
                "- [Subsystems](subsystems/index.md)\n"
                "- [Code files](files/index.md)\n\n"
                "## Subsystems",
                1,
            )
            break

    return OKFBundle(documents=docs)


def write_code_graph_okf(
    root: Path,
    out_dir: Path,
    *,
    graph: Optional[CodeGraph] = None,
    project_name: Optional[str] = None,
) -> Tuple[Path, List[Path]]:
    """Write an OKF bundle for the code graph under ``out_dir``.

    Returns ``(out_dir, written_paths)``.
    """
    from devcouncil.indexing.graph.build import load_code_graph

    root = root.expanduser().resolve()
    if graph is None:
        graph = load_code_graph(root)
    if graph is None:
        raise FileNotFoundError("No code graph found; run `dev map` first.")
    name = project_name or root.name
    bundle = build_code_graph_okf(graph, project_name=name)
    out = out_dir if out_dir.is_absolute() else root / out_dir
    out.mkdir(parents=True, exist_ok=True)
    written = write_bundle(bundle, out)
    return out, written
