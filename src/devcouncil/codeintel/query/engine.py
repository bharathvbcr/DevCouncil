"""Generation-consistent search, exploration, path, and impact queries."""

from __future__ import annotations

from collections import defaultdict, deque
from pathlib import Path
from typing import Any, Iterable

from devcouncil.codeintel.service import CodeIntelService, get_codeintel_service
from devcouncil.indexing.graph.schema import CodeGraph, Confidence, GraphEdge, GraphNode


class CodeIntelQueryEngine:
    def __init__(self, root: Path | CodeIntelService):
        self.service = root if isinstance(root, CodeIntelService) else get_codeintel_service(root)

    def _graph(self) -> CodeGraph:
        graph = self.service.load()
        if graph is None:
            raise FileNotFoundError("no code-intelligence index; run `dev graph init`")
        from devcouncil.codeintel.debug.fingerprint import source_fingerprint

        fingerprint = source_fingerprint(self.service.project_root)
        runtime = self.service.store.runtime_observations(source_fingerprint=fingerprint)
        existing = {(edge.source, edge.target, edge.kind) for edge in graph.edges}
        for observation in runtime:
            kind = str(observation["kind"])
            key = (str(observation["source"]), str(observation["target"]), kind)
            if key in existing:
                continue
            sampled = kind in {"sampled_calls", "sampled_stack"}
            graph.edges.append(GraphEdge(
                source=key[0],
                target=key[1],
                kind=kind,
                confidence=Confidence.INFERRED if sampled else Confidence.EXTRACTED,
                reason="fingerprint-matched runtime observation",
                extras={
                    "provenance": "runtime",
                    "confidence_score": 0.75 if sampled else 1.0,
                    "source_fingerprint": fingerprint,
                    "runtime_session": observation["session_id"],
                    "count": observation["count"],
                    "evidence": observation["evidence"],
                },
            ))
            existing.add(key)
        return graph

    def _envelope(self, payload: dict[str, Any]) -> dict[str, Any]:
        from devcouncil.codeintel.sync import get_sync_coordinator

        status = self.service.status()
        sync = get_sync_coordinator(self.service.project_root).status().as_dict()
        return {
            "ok": True,
            "project_root": str(self.service.project_root),
            "generation": status.get("generation"),
            "schema_version": status.get("schema_version"),
            "analyzer_version": status.get("analyzer_version"),
            "sync": sync,
            **payload,
        }

    def search(self, query: str, *, limit: int = 50) -> dict[str, Any]:
        rows = self.service.cached_query(
            "search", f"{query}\0{limit}", lambda: self.service.store.search(query, limit=limit)
        )
        return self._envelope({"query": query, "matches": rows})

    def explore(self, query: str, *, limit: int = 20) -> dict[str, Any]:
        graph = self._graph()
        matches = self._match(graph, query)[:limit]
        inbound, outbound = self._relations(graph.edges)
        definitions: list[dict[str, Any]] = []
        for node in matches:
            content = self.service.store.content_for_path(node.path)
            snippet = self._snippet(content, node.line, node.end_line)
            callers = [self._edge_dict(edge) for edge in inbound.get(node.id, [])[:50]]
            callees = [self._edge_dict(edge) for edge in outbound.get(node.id, [])[:50]]
            definitions.append({
                "id": node.id,
                "kind": node.kind.value if hasattr(node.kind, "value") else str(node.kind),
                "path": node.path,
                "name": node.name,
                "line": node.line,
                "end_line": node.end_line,
                "language": node.language,
                "source": snippet,
                "callers": callers,
                "callees": callees,
            })
        impacted = self._impact_for_ids(graph, [node.id for node in matches], max_depth=3)
        return self._envelope({
            "query": query,
            "match_count": len(matches),
            "definitions": definitions,
            "blast_radius": impacted,
        })

    def path(self, start: str, end: str, *, max_depth: int = 32) -> dict[str, Any]:
        graph = self._graph()
        starts = {node.id for node in self._match(graph, start)}
        ends = {node.id for node in self._match(graph, end)}
        if not starts or not ends:
            return self._envelope({"from": start, "to": end, "found": False, "path": [], "reason": "endpoint not found"})
        adjacency: dict[str, list[tuple[str, GraphEdge]]] = defaultdict(list)
        for edge in graph.edges:
            adjacency[edge.source].append((edge.target, edge))
            if edge.kind in {"imports", "contains", "defines", "inherits", "implements", "overrides"}:
                adjacency[edge.target].append((edge.source, edge))
        queue = deque((node_id, 0) for node_id in starts)
        parent: dict[str, tuple[str | None, GraphEdge | None]] = {node_id: (None, None) for node_id in starts}
        found: str | None = None
        while queue:
            current, depth = queue.popleft()
            if current in ends:
                found = current
                break
            if depth >= max_depth:
                continue
            for target, edge in adjacency.get(current, []):
                if target not in parent:
                    parent[target] = (current, edge)
                    queue.append((target, depth + 1))
        if found is None:
            return self._envelope({"from": start, "to": end, "found": False, "path": []})
        steps: list[dict[str, Any]] = []
        walk = found
        while True:
            previous, path_edge = parent[walk]
            steps.append({"node": walk, "via": self._edge_dict(path_edge) if path_edge else None})
            if previous is None:
                break
            walk = previous
        steps.reverse()
        return self._envelope({"from": start, "to": end, "found": True, "length": len(steps) - 1, "path": steps})

    def impact(self, targets: Iterable[str], *, max_depth: int = 3) -> dict[str, Any]:
        graph = self._graph()
        ids: list[str] = []
        for target in targets:
            ids.extend(node.id for node in self._match(graph, target))
        return self._envelope({"targets": list(targets), "blast_radius": self._impact_for_ids(graph, ids, max_depth=max_depth)})

    def dead(self, *, minimum_confidence: str = "inferred") -> dict[str, Any]:
        graph = self._graph()
        ranks = {"ambiguous": 0, "inferred": 1, "extracted": 2}
        floor = ranks.get(minimum_confidence, 1)
        runtime_live = {
            edge.target
            for edge in graph.edges
            if edge.extras.get("provenance") == "runtime"
        }
        rows = []
        for entry in graph.dead_code:
            confidence = entry.confidence.value if hasattr(entry.confidence, "value") else str(entry.confidence)
            if ranks.get(confidence, 0) < floor:
                continue
            if entry.id in runtime_live:
                continue
            tier = "high-confidence dead candidate" if confidence == "extracted" else "unconfirmed/unwired"
            rows.append({**entry.model_dump(mode="json"), "tier": tier})
        return self._envelope({
            "minimum_confidence": minimum_confidence,
            "dead_code": rows,
            "runtime_proven_live": sorted(runtime_live),
        })

    def affected_tests(self, targets: Iterable[str], *, max_depth: int = 3) -> dict[str, Any]:
        graph = self._graph()
        target_list = list(targets)
        ids = [node.id for target in target_list for node in self._match(graph, target)]
        impact = self._impact_for_ids(graph, ids, max_depth=max_depth)
        impacted_ids = {node_id for layer in impact["layers"] for node_id in layer["nodes"]} | set(ids)
        nodes = graph.node_by_id()
        tests = sorted({
            node.path
            for node_id in impacted_ids
            if (node := nodes.get(node_id)) is not None and self._is_test_path(node.path)
        })
        return self._envelope({"targets": target_list, "tests": tests, "blast_radius": impact})

    @staticmethod
    def _match(graph: CodeGraph, query: str) -> list[GraphNode]:
        normalized = query.replace("\\", "/").lower()
        exact: list[GraphNode] = []
        partial: list[GraphNode] = []
        for node in graph.nodes:
            values = (node.id.lower(), node.path.lower(), node.name.lower())
            if normalized in values or node.id.lower().endswith(f"::{normalized}"):
                exact.append(node)
            elif any(normalized in value for value in values):
                partial.append(node)
        return exact + partial

    @staticmethod
    def _relations(edges: Iterable[GraphEdge]) -> tuple[dict[str, list[GraphEdge]], dict[str, list[GraphEdge]]]:
        inbound: dict[str, list[GraphEdge]] = defaultdict(list)
        outbound: dict[str, list[GraphEdge]] = defaultdict(list)
        for edge in edges:
            inbound[edge.target].append(edge)
            outbound[edge.source].append(edge)
        return inbound, outbound

    @staticmethod
    def _edge_dict(edge: GraphEdge | None) -> dict[str, Any]:
        if edge is None:
            return {}
        confidence = edge.confidence.value if hasattr(edge.confidence, "value") else str(edge.confidence)
        return {
            "source": edge.source,
            "target": edge.target,
            "kind": edge.kind,
            "confidence": confidence,
            "confidence_score": edge.extras.get("confidence_score", {"extracted": 1.0, "inferred": 0.7, "ambiguous": 0.4}.get(confidence, 0.4)),
            "provenance": edge.extras.get("provenance", "extracted" if confidence == "extracted" else "inferred"),
            "reason": edge.reason,
            "evidence": edge.extras.get("evidence", []),
        }

    @staticmethod
    def _snippet(content: bytes | None, start: int, end: int) -> str:
        if content is None:
            return ""
        lines = content.decode("utf-8", errors="replace").splitlines()
        if start <= 0:
            return ""
        final = end if end >= start else start
        return "\n".join(f"{line_no}: {lines[line_no - 1]}" for line_no in range(start, min(final, len(lines)) + 1))

    def _impact_for_ids(self, graph: CodeGraph, ids: Iterable[str], *, max_depth: int) -> dict[str, Any]:
        seeds = list(dict.fromkeys(ids))
        inbound, _ = self._relations(graph.edges)
        visited = set(seeds)
        frontier = set(seeds)
        layers: list[dict[str, Any]] = []
        for depth in range(1, max(1, min(8, max_depth)) + 1):
            next_frontier: set[str] = set()
            confidences: list[str] = []
            for node_id in frontier:
                for edge in inbound.get(node_id, []):
                    if edge.source in visited:
                        continue
                    next_frontier.add(edge.source)
                    confidence = edge.confidence.value if hasattr(edge.confidence, "value") else str(edge.confidence)
                    confidences.append(confidence)
            visited.update(next_frontier)
            layers.append({
                "depth": depth,
                "nodes": sorted(next_frontier),
                "confidence": self._lowest_confidence(confidences),
                "count": len(next_frontier),
            })
            frontier = next_frontier
            if not frontier:
                break
        return {"seeds": seeds, "layers": layers, "total_impacted": len(visited - set(seeds))}

    @staticmethod
    def _lowest_confidence(values: Iterable[str]) -> str:
        ranks = {"extracted": 2, "inferred": 1, "ambiguous": 0}
        values = list(values)
        return min(values, key=lambda value: ranks.get(value, 0)) if values else "extracted"

    @staticmethod
    def _is_test_path(path: str) -> bool:
        normalized = f"/{path.lower().replace('\\', '/')}"
        name = Path(path).name.lower()
        return "/test" in normalized or "/tests/" in normalized or name.startswith("test_") or ".test." in name or ".spec." in name
