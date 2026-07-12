"""``dev graph`` — query / trace / dead / check / process / impact / html / view / export."""

from __future__ import annotations

import json
from pathlib import Path
from typing import List, Optional

import typer
from rich.console import Console

app = typer.Typer(
    name="graph",
    help="Query and visualize the symbol-level code knowledge graph.",
    add_completion=False,
)
console = Console()
status = Console(stderr=True)


def _root(project_root: Path) -> Path:
    root = project_root.expanduser().resolve()
    from devcouncil.telemetry.logging_setup import set_log_dir

    set_log_dir(root)
    return root


def _require_graph(root: Path):
    from devcouncil.indexing.graph.build import load_code_graph

    graph = load_code_graph(root)
    if graph is None:
        status.print("[red]No code graph; run `dev map` first.[/red]")
        raise typer.Exit(code=1)
    return graph


@app.command("query")
def graph_query(
    name_or_path: str = typer.Argument(..., help="Symbol name or file path."),
    project_root: Path = typer.Option(Path("."), "--project-root"),
    json_output: bool = typer.Option(False, "--json"),
) -> None:
    """360° view: definition, callers, callees, importers."""
    from devcouncil.indexing.graph import query_symbol

    root = _root(project_root)
    result = query_symbol(root, name_or_path)
    if json_output:
        typer.echo(json.dumps(result, indent=2))
        return
    if result.get("error"):
        status.print(f"[red]{result['error']}[/red]")
        raise typer.Exit(code=1)
    defs = result.get("definitions") or []
    if not defs:
        console.print(f"No matches for {name_or_path!r}")
        return
    for d in defs:
        console.print(f"[bold]{d['id']}[/bold]  ({d.get('kind')})  {d.get('path')}:{d.get('line')}")
        console.print(f"  callers: {', '.join(d.get('callers') or []) or '(none)'}")
        console.print(f"  callees: {', '.join(d.get('callees') or []) or '(none)'}")
        console.print(f"  importers: {', '.join(d.get('importers') or []) or '(none)'}")


@app.command("trace")
def graph_trace(
    start: str = typer.Argument(..., help="Start node (name or path)."),
    end: str = typer.Argument(..., help="End node (name or path)."),
    project_root: Path = typer.Option(Path("."), "--project-root"),
    json_output: bool = typer.Option(False, "--json"),
) -> None:
    """Shortest path between two graph nodes."""
    from devcouncil.indexing.graph import trace_path

    root = _root(project_root)
    result = trace_path(root, start, end)
    if json_output:
        typer.echo(json.dumps(result, indent=2))
        return
    if result.get("error"):
        status.print(f"[red]{result['error']}[/red]")
        raise typer.Exit(code=1)
    if not result.get("found"):
        console.print(f"No path between {start!r} and {end!r}")
        raise typer.Exit(code=1)
    console.print(" → ".join(result.get("path") or []))


@app.command("dead")
def graph_dead(
    project_root: Path = typer.Option(Path("."), "--project-root"),
    json_output: bool = typer.Option(False, "--json"),
    confidence: Optional[str] = typer.Option(
        None, "--confidence", help="Exact filter: extracted|inferred|ambiguous"
    ),
    min_confidence: str = typer.Option(
        "inferred",
        "--min-confidence",
        help="Include this tier and above: extracted > inferred > ambiguous "
        "(default: inferred; pass ambiguous to show all)",
    ),
) -> None:
    """Full dead-code report with confidence tiers and reasons."""
    from collections import Counter

    from devcouncil.indexing.graph.liveness import confidence_at_least

    root = _root(project_root)
    graph = _require_graph(root)
    entries = list(graph.dead_code)
    if confidence:
        entries = [
            e
            for e in entries
            if (e.confidence.value if hasattr(e.confidence, "value") else str(e.confidence))
            == confidence
        ]
    before_min = len(entries)
    if min_confidence:
        entries = [
            e
            for e in entries
            if confidence_at_least(e.confidence, min_confidence)
        ]
    hidden = before_min - len(entries)
    if json_output:
        typer.echo(json.dumps([e.model_dump() for e in entries], indent=2))
        return
    if not entries:
        console.print("No dead-code entries.")
        if hidden:
            console.print(
                f"{hidden} lower-confidence entries hidden "
                "(--min-confidence ambiguous to show)."
            )
        return
    for e in entries:
        conf = e.confidence.value if hasattr(e.confidence, "value") else e.confidence
        console.print(
            f"{e.path}:{e.line}  {e.id}  [{conf}/{e.kind}]  {e.reason}"
        )
    reason_counts = Counter(e.reason or "(none)" for e in entries)
    console.print("")
    console.print("Reason summary:")
    for reason, n in reason_counts.most_common():
        console.print(f"  {n:4d}  {reason}")
    if hidden:
        console.print("")
        console.print(
            f"{hidden} lower-confidence entries hidden "
            "(--min-confidence ambiguous to show)."
        )


@app.command("check")
def graph_check_cmd(
    project_root: Path = typer.Option(Path("."), "--project-root"),
    json_output: bool = typer.Option(False, "--json"),
    top: int = typer.Option(15, "--top", help="How many god nodes to list."),
) -> None:
    """God nodes (top-connected) and circular-import detection."""
    from devcouncil.indexing.graph.intel import graph_check

    root = _root(project_root)
    graph = _require_graph(root)
    report = graph_check(graph, top_n=top)
    if json_output:
        typer.echo(json.dumps(report, indent=2))
        return
    console.print(f"[bold]God nodes[/bold] (top {top} by degree)")
    for g in report.get("god_nodes") or []:
        console.print(
            f"  {g.get('degree'):>4}  {g.get('id')}  ({g.get('kind')})"
        )
    cycles = report.get("circular_imports") or []
    console.print(f"\n[bold]Circular imports[/bold] ({len(cycles)})")
    if not cycles:
        console.print("  (none)")
    for c in cycles[:30]:
        console.print("  " + " → ".join(c.get("nodes") or []) + " → …")


@app.command("process")
def graph_process(
    entry: Optional[str] = typer.Argument(
        None, help="Optional entry root path or name filter."
    ),
    project_root: Path = typer.Option(Path("."), "--project-root"),
    json_output: bool = typer.Option(False, "--json"),
    max_depth: int = typer.Option(6, "--max-depth"),
) -> None:
    """BFS call-flows from entry roots (named, step-ordered, depth-capped)."""
    from devcouncil.indexing.graph.intel import extract_processes

    root = _root(project_root)
    graph = _require_graph(root)
    processes = extract_processes(graph, entry=entry, max_depth=max_depth)
    if json_output:
        typer.echo(json.dumps(processes, indent=2))
        return
    if not processes:
        console.print("No processes found.")
        return
    for p in processes:
        console.print(f"[bold]{p.get('name')}[/bold]  (depth {p.get('depth')})")
        console.print("  " + " → ".join(p.get("steps") or []))


@app.command("impact")
def graph_impact(
    paths: Optional[List[str]] = typer.Argument(
        None, help="Paths to analyze (omit with --diff for working-tree changes)."
    ),
    diff: bool = typer.Option(
        False, "--diff", help="Use working-tree changed files as the seed set."
    ),
    project_root: Path = typer.Option(Path("."), "--project-root"),
    json_output: bool = typer.Option(False, "--json"),
    max_depth: int = typer.Option(3, "--max-depth", help="Inbound blast depth (1–3)."),
) -> None:
    """Diff / path blast radius via enclosing symbols and inbound callers."""
    from devcouncil.indexing.graph.intel import diff_impact

    root = _root(project_root)
    graph = _require_graph(root)
    if not diff and not paths:
        status.print("[red]Provide paths or --diff.[/red]")
        raise typer.Exit(code=1)
    result = diff_impact(
        root,
        graph,
        paths=paths,
        use_diff=diff,
        max_depth=max(1, min(3, max_depth)),
    )
    if json_output:
        typer.echo(json.dumps(result, indent=2))
        return
    if not result.get("paths"):
        console.print("No impacted paths.")
        return
    for item in result["paths"]:
        console.print(f"[bold]{item['path']}[/bold]")
        syms = item.get("symbols") or []
        if syms:
            console.print("  symbols: " + ", ".join(s["id"] for s in syms[:8]))
        for layer in (item.get("blast") or {}).get("layers") or []:
            nodes = layer.get("nodes") or []
            console.print(
                f"  depth {layer['depth']} [{layer['confidence']}]: "
                f"{len(nodes)} — " + ", ".join(nodes[:6])
                + (" …" if len(nodes) > 6 else "")
            )


@app.command("html")
def graph_html(
    project_root: Path = typer.Option(Path("."), "--project-root"),
    open_browser: bool = typer.Option(False, "--open", help="Open in the default browser."),
    symbols: bool = typer.Option(
        False,
        "--symbols",
        help="Default the visualizer to symbol-level mode (calls/inherits) instead of file imports.",
    ),
) -> None:
    """Write a self-contained interactive ``graph.html``."""
    from devcouncil.indexing.viz import write_graph_html

    root = _root(project_root)
    try:
        out = write_graph_html(root, open_browser=open_browser, symbols=symbols)
    except FileNotFoundError as exc:
        status.print(f"[red]{exc}[/red]")
        raise typer.Exit(code=1) from exc
    status.print(f"[green]Wrote {out}[/green]")


@app.command("view")
def graph_view(
    project_root: Path = typer.Option(Path("."), "--project-root"),
    port: int = typer.Option(8765, "--port"),
) -> None:
    """Serve/open the graph HTML via a tiny local HTTP server."""
    import http.server
    import socketserver
    import threading
    import webbrowser

    from devcouncil.indexing.viz import write_graph_html

    root = _root(project_root)
    try:
        out = write_graph_html(root, open_browser=False)
    except FileNotFoundError as exc:
        status.print(f"[red]{exc}[/red]")
        raise typer.Exit(code=1) from exc

    directory = str(out.parent)

    class Handler(http.server.SimpleHTTPRequestHandler):
        def __init__(self, *args, **kwargs):
            super().__init__(*args, directory=directory, **kwargs)

        def log_message(self, fmt, *args):  # noqa: A003
            return

    with socketserver.TCPServer(("127.0.0.1", port), Handler) as httpd:
        url = f"http://127.0.0.1:{port}/graph.html"
        status.print(f"[green]Serving {url}  (Ctrl-C to stop)[/green]")
        threading.Timer(0.3, lambda: webbrowser.open(url)).start()
        try:
            httpd.serve_forever()
        except KeyboardInterrupt:
            status.print("Stopped.")


@app.command("export")
def graph_export(
    format: str = typer.Option(
        "graphml",
        "--format",
        help="graphml | okf | okf-links",
    ),
    output: Path = typer.Option(Path("-"), "--output", "-o"),
    project_root: Path = typer.Option(Path("."), "--project-root"),
) -> None:
    """Export the code graph as attributed GraphML or an OKF v0.1 bundle."""
    from devcouncil.indexing.graph.export import export_graphml, write_code_graph_okf

    root = _root(project_root)
    graph = _require_graph(root)
    fmt = format.lower().strip()
    if fmt == "graphml":
        text = export_graphml(graph)
        if str(output) == "-":
            typer.echo(text)
        else:
            out = output if output.is_absolute() else root / output
            out.parent.mkdir(parents=True, exist_ok=True)
            out.write_text(text, encoding="utf-8")
            status.print(f"[green]Wrote {out}[/green]")
        return
    if fmt == "okf":
        if str(output) == "-":
            status.print("[red]OKF export requires -o <directory>[/red]")
            raise typer.Exit(code=1)
        out_dir = output if output.is_absolute() else root / output
        try:
            written_dir, paths = write_code_graph_okf(root, out_dir, graph=graph)
        except FileNotFoundError as exc:
            status.print(f"[red]{exc}[/red]")
            raise typer.Exit(code=1) from exc
        status.print(f"[green]Wrote OKF bundle ({len(paths)} docs) to {written_dir}[/green]")
        return
    if fmt in {"okf-links"}:
        rows = []
        for e in graph.edges:
            if e.kind in {"imports", "calls"}:
                rows.append(f"{e.source} --{e.kind}--> {e.target}")
        text = "\n".join(rows)
        if str(output) == "-":
            typer.echo(text)
        else:
            out = output if output.is_absolute() else root / output
            out.parent.mkdir(parents=True, exist_ok=True)
            out.write_text(text, encoding="utf-8")
            status.print(f"[green]Wrote {out}[/green]")
        return
    status.print(f"[red]Unknown format: {format}[/red]")
    raise typer.Exit(code=1)
