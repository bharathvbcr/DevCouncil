"""Framework, event, state, callback, and dynamic-reference synthesis."""

from __future__ import annotations

import re
from collections import defaultdict
from pathlib import Path
from typing import Any, Iterable, Mapping

from devcouncil.codeintel.resolution.abstract_state import AbstractState
from devcouncil.codeintel.resolution.frameworks import (
    COMPUTED_ROUTE_PATTERN,
    iter_event_matches,
    iter_provider_matches,
    iter_route_matches,
)
from devcouncil.indexing.graph.extract_python import FileExtraction
from devcouncil.indexing.graph.schema import CodeGraph, Confidence, GraphEdge, GraphNode, NodeKind

_REACT_STATE = re.compile(r"\[\s*(?P<state>[A-Za-z_$][\w$]*)\s*,\s*(?P<setter>[A-Za-z_$][\w$]*)\s*\]\s*=\s*(?:React\.)?useState\s*\(")
_DYNAMIC = re.compile(r"\b(?P<kind>eval|exec|getattr|setattr|import_module|Class\.forName|Activator\.CreateInstance|NSClassFromString|require|import)\s*\(")
_CALLBACK = re.compile(r"\b(?:then|catch|finally|map|filter|reduce|forEach|subscribe|useEffect)\s*\(\s*(?P<callback>[A-Za-z_$][\w$]*)")
_ABSTRACT_ALIAS_CALL = re.compile(
    r"\b(?:new\s+)?(?P<callee>[A-Za-z_$][\w$]*)\s*\("
)
_C_FUNCTION_POINTER = re.compile(r"(?P<pointer>[A-Za-z_]\w*)\s*=\s*&?(?P<target>[A-Za-z_]\w*)\s*;")
_LITERAL_EVAL = re.compile(r"\b(?:eval|exec)\(\s*(?P<quote>['\"])(?P<body>.*?)(?P=quote)\s*\)")
_BRIDGE_DECL = re.compile(
    r"(?:RCT_EXPORT_METHOD\s*\(|@ReactMethod\s+(?:fun\s+)?|AsyncFunction\s*\(\s*['\"]|Function\s*\(\s*['\"]|@objc\s+(?:public\s+)?func\s+)(?P<name>[A-Za-z_$][\w$]*)"
)
_BRIDGE_CALL = re.compile(
    r"(?:NativeModules\.[A-Za-z_$][\w$]*\.|requireNativeModule\([^)]*\)\.)(?P<name>[A-Za-z_$][\w$]*)\s*\("
)
_FILE_ROUTE_PREFIXES = (
    "src/pages/",
    "pages/",
    "src/routes/",
    "server/api/",
)


def _enclosing(nodes: list[GraphNode], path: str, line: int) -> GraphNode | None:
    candidates = [
        node
        for node in nodes
        if node.path == path
        and node.kind != NodeKind.FILE
        and node.line <= line
        and (node.end_line <= 0 or line <= node.end_line)
    ]
    return max(candidates, key=lambda node: node.line, default=None)


def _edge(
    source: str,
    target: str,
    kind: str,
    *,
    reason: str,
    score: float,
    synthesizer: str,
) -> GraphEdge:
    confidence = Confidence.INFERRED if score >= 0.6 else Confidence.AMBIGUOUS
    return GraphEdge(
        source=source,
        target=target,
        kind=kind,
        confidence=confidence,
        reason=reason,
        extras={
            "provenance": "framework"
            if kind in {"registers", "routes_to", "listens", "injects", "provides"}
            else "inferred",
            "confidence_score": score,
            "evidence": [{"synthesized_by": synthesizer}],
            "synthesized_by": synthesizer,
        },
    )


def _symbol_index(nodes: Iterable[GraphNode]) -> dict[str, list[GraphNode]]:
    result: dict[str, list[GraphNode]] = defaultdict(list)
    for node in nodes:
        if node.name:
            keys = {node.name, node.id.rsplit("::", 1)[-1]}
            for key in keys:
                result[key].append(node)
    return result


def _resolved_symbol_targets(
    symbols: dict[str, list[GraphNode]],
    name: str,
    path: str,
    *,
    imported_paths: set[str] | None = None,
    aliases: Mapping[str, str] | None = None,
) -> list[GraphNode]:
    """Return one unambiguous local/imported symbol before global fallback."""
    remote = (aliases or {}).get(name, name)
    lookup_name = remote.rsplit(".", 1)[-1].rsplit(":", 1)[-1]
    targets = symbols.get(lookup_name) or []
    same_file = [target for target in targets if target.path == path]
    if len(same_file) == 1:
        return same_file
    if same_file:
        return []
    imported = [
        target for target in targets if target.path in (imported_paths or set())
    ]
    if len(imported) == 1:
        return imported
    if imported:
        return []
    return targets if len(targets) == 1 else []


def _imported_paths_by_file(
    nodes: Iterable[GraphNode],
    edges: Iterable[GraphEdge],
) -> dict[str, set[str]]:
    node_by_id = {node.id: node for node in nodes}
    imported: dict[str, set[str]] = defaultdict(set)
    for edge in edges:
        if edge.kind != "imports":
            continue
        source = node_by_id.get(edge.source)
        target = node_by_id.get(edge.target)
        source_path = source.path if source is not None else edge.source.split("::", 1)[0]
        target_path = target.path if target is not None else edge.target.split("::", 1)[0]
        if source_path and target_path and source_path != target_path:
            imported[source_path].add(target_path)
    return imported


def enrich_semantic_edges(
    graph: CodeGraph,
    *,
    root: Path,
    paths: set[str] | None = None,
    extractions: Mapping[str, FileExtraction] | None = None,
) -> CodeGraph:
    """Add isolated semantic nodes/edges without changing extracted identities."""

    root = root.expanduser().resolve()
    nodes = list(graph.nodes)
    edges = list(graph.edges)
    initial_node_count = len(nodes)
    initial_edge_count = len(edges)
    symbols = _symbol_index(nodes)
    imported_paths = _imported_paths_by_file(nodes, edges)
    aliases_by_path = {
        path: {
            local: remote
            for detail in extraction.import_details
            for local, remote in detail.alias_map.items()
        }
        for path, extraction in (extractions or {}).items()
    }
    existing_nodes = {node.id for node in nodes}
    existing_edges = {(edge.source, edge.target, edge.kind) for edge in edges}
    file_paths = sorted({node.path for node in nodes if node.path})
    if paths is not None:
        file_paths = [path for path in file_paths if path in paths]

    def add_node(node: GraphNode) -> None:
        if node.id not in existing_nodes:
            existing_nodes.add(node.id)
            nodes.append(node)

    def add_edge(edge: GraphEdge) -> None:
        key = (edge.source, edge.target, edge.kind)
        if edge.source in existing_nodes and edge.target in existing_nodes and key not in existing_edges:
            existing_edges.add(key)
            edges.append(edge)

    for rel in file_paths:
        try:
            text = (root / rel).read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        lines = text.splitlines()
        file_symbols = [node for node in nodes if node.path == rel]
        callable_aliases, type_aliases = _abstract_import_aliases(
            aliases_by_path.get(rel, {}),
            symbols,
        )
        abstract_state = AbstractState().analyze(
            text,
            callable_names={
                node.name
                for node in file_symbols
                if node.kind in {NodeKind.FUNCTION, NodeKind.METHOD}
            },
            type_names={
                node.name
                for node in file_symbols
                if node.kind
                in {
                    NodeKind.CLASS,
                    NodeKind.INTERFACE,
                    NodeKind.TYPE,
                    NodeKind.STRUCT,
                    NodeKind.ENUM,
                    NodeKind.TRAIT,
                }
            },
            callable_aliases=callable_aliases,
            type_aliases=type_aliases,
        )
        language = next((node.language for node in nodes if node.path == rel and node.language), "")
        normalized_rel = rel.replace("\\", "/")
        if any(normalized_rel.startswith(prefix) for prefix in _FILE_ROUTE_PREFIXES) and Path(rel).suffix.lower() in {
            ".astro", ".vue", ".svelte", ".tsx", ".ts", ".jsx", ".js"
        }:
            route_path = _file_route_path(normalized_rel)
            route_id = f"{rel}::route:FILE:{route_path}"
            add_node(GraphNode(
                id=route_id,
                kind=NodeKind.ROUTE,
                path=rel,
                name=f"FILE {route_path}",
                line=1,
                end_line=1,
                language=language,
                extras={"verb": "FILE", "route": route_path, "provenance": "framework"},
            ))
            handler = next((node for node in nodes if node.path == rel and node.kind != NodeKind.FILE), None)
            add_edge(_edge(
                rel,
                route_id,
                "registers",
                reason="reachable file registers framework route",
                score=0.95,
                synthesizer="file-route-resolver",
            ))
            add_edge(_edge(
                route_id,
                handler.id if handler else rel,
                "routes_to",
                reason="file-based framework route",
                score=0.9,
                synthesizer="file-route-resolver",
            ))
        for line_no, line in enumerate(lines, start=1):
            owner = _enclosing(nodes, rel, line_no)
            owner_id = owner.id if owner is not None else rel
            match: Any

            for match in iter_route_matches(line):
                route_id = f"{rel}::route:{match.verb}:{match.path}"
                add_node(GraphNode(
                    id=route_id,
                    kind=NodeKind.ROUTE,
                    path=rel,
                    name=f"{match.verb} {match.path}",
                    line=line_no,
                    end_line=line_no,
                    language=language,
                    extras={
                        "verb": match.verb,
                        "route": match.path,
                        "framework": match.framework,
                        "provenance": "framework",
                    },
                ))
                handler_id = _route_handler_id(
                    nodes,
                    symbols,
                    rel,
                    line_no,
                    match.handler_expression,
                    owner_id,
                    imported_paths.get(rel, set()),
                    aliases_by_path.get(rel, {}),
                    abstract_state,
                )
                registration_owner = _registration_owner(
                    nodes, rel, owner_id, handler_id
                )
                add_edge(_edge(
                    registration_owner,
                    route_id,
                    "registers",
                    reason=f"{match.framework} route registration",
                    score=0.95,
                    synthesizer=f"{match.framework}-route",
                ))
                if handler_id is not None:
                    add_edge(_edge(
                        route_id,
                        handler_id,
                        "routes_to",
                        reason=f"{match.framework} route binds handler",
                        score=0.9,
                        synthesizer=f"{match.framework}-route",
                    ))

            for match in COMPUTED_ROUTE_PATTERN.finditer(line):
                expression = match.group("expr").strip()
                if expression[:1] in {"'", '"'}:
                    continue
                for route_path in sorted(abstract_state.evaluate(expression).strings()):
                    verb = match.group("verb").upper()
                    route_id = f"{rel}::route:{verb}:{route_path}"
                    add_node(GraphNode(
                        id=route_id,
                        kind=NodeKind.ROUTE,
                        path=rel,
                        name=f"{verb} {route_path}",
                        line=line_no,
                        end_line=line_no,
                        language=language,
                        extras={
                            "verb": verb,
                            "route": route_path,
                            "framework": "computed",
                            "provenance": "inferred",
                            "expression": expression,
                        },
                    ))
                    handler_id = _route_handler_id(
                        nodes,
                        symbols,
                        rel,
                        line_no,
                        match.group("handler"),
                        owner_id,
                        imported_paths.get(rel, set()),
                        aliases_by_path.get(rel, {}),
                        abstract_state,
                    )
                    registration_owner = _registration_owner(
                        nodes, rel, owner_id, handler_id
                    )
                    add_edge(_edge(
                        registration_owner,
                        route_id,
                        "registers",
                        reason="computed route registration",
                        score=0.85,
                        synthesizer="abstract-state-route-resolver",
                    ))
                    if handler_id is not None:
                        add_edge(_edge(
                            route_id,
                            handler_id,
                            "routes_to",
                            reason="bounded computed route path",
                            score=0.75,
                            synthesizer="abstract-state-route-resolver",
                        ))

            for match in iter_event_matches(line):
                channel_id = f"event::{match.event}"
                add_node(GraphNode(
                    id=channel_id,
                    kind=NodeKind.EVENT,
                    name=match.event,
                    extras={"channel": match.event},
                ))
                if match.operation in {"emit", "sendeventwithname", "sendevent"}:
                    add_edge(_edge(
                        owner_id,
                        channel_id,
                        "emits",
                        reason="literal event emission",
                        score=0.9,
                        synthesizer="event-channel",
                    ))
                    continue
                listener_id = (
                    f"{rel}::event-listener:{match.event}:{line_no}:{match.start}"
                )
                add_node(GraphNode(
                    id=listener_id,
                    kind=NodeKind.EVENT,
                    path=rel,
                    name=match.event,
                    line=line_no,
                    end_line=line_no,
                    language=language,
                    extras={"channel": match.event, "registration": "listener"},
                ))
                add_edge(_edge(owner_id, listener_id, "registers", reason="event listener registration", score=0.95, synthesizer="event-channel"))
                add_edge(_edge(listener_id, channel_id, "subscribes", reason="literal event channel", score=0.95, synthesizer="event-channel"))
                targets = _expression_targets(
                    match.callback_expression,
                    symbols,
                    rel,
                    imported_paths.get(rel, set()),
                    aliases_by_path.get(rel, {}),
                    abstract_state,
                )
                if len(targets) == 1:
                    callback_targets = [targets[0]]
                    if match.operation == "schedule" and targets[0].kind == NodeKind.CLASS:
                        callback_targets.extend(
                            node
                            for node in nodes
                            if node.path == targets[0].path
                            and node.kind == NodeKind.METHOD
                            and str(node.extras.get("qualname") or "").startswith(
                                f"{targets[0].name}."
                            )
                            and node.name.startswith("on_")
                        )
                    for target in callback_targets:
                        add_edge(_edge(listener_id, target.id, "listens", reason="literal event listener callback", score=0.85, synthesizer="event-channel"))

            for match in _REACT_STATE.finditer(line):
                state_name = match.group("state")
                setter = match.group("setter")
                state_id = f"{rel}::state:{state_name}"
                add_node(GraphNode(
                    id=state_id,
                    kind=NodeKind.STATE,
                    path=rel,
                    name=state_name,
                    line=line_no,
                    end_line=line_no,
                    language=language,
                    extras={"setter": setter, "framework": "react"},
                ))
                add_edge(_edge(owner_id, state_id, "owns_state", reason="React useState binding", score=0.95, synthesizer="react-state"))
                for later_no, later in enumerate(lines[line_no:], start=line_no + 1):
                    if re.search(rf"\b{re.escape(setter)}\s*\(", later):
                        writer = _enclosing(nodes, rel, later_no)
                        add_edge(_edge((writer.id if writer else rel), state_id, "writes_state", reason=f"calls state setter {setter}", score=0.9, synthesizer="react-state"))

            for match in iter_provider_matches(line):
                target_name = (
                    match.target_expression.split(".")[-1].split(":")[-1]
                )
                targets = _expression_targets(
                    target_name,
                    symbols,
                    rel,
                    imported_paths.get(rel, set()),
                    aliases_by_path.get(rel, {}),
                    abstract_state,
                )
                if len(targets) != 1:
                    continue
                provider_id = f"{rel}::provider:{target_name}:{line_no}"
                add_node(GraphNode(
                    id=provider_id,
                    kind=NodeKind.PROVIDER,
                    path=rel,
                    name=target_name,
                    line=line_no,
                    end_line=line_no,
                    language=language,
                    extras={"framework": match.framework},
                ))
                add_edge(_edge(owner_id, provider_id, "registers", reason="DI provider registration", score=0.9, synthesizer=f"{match.framework}-resolver"))
                add_edge(_edge(provider_id, targets[0].id, "provides", reason="DI registration", score=0.8, synthesizer=f"{match.framework}-resolver"))
                add_edge(_edge(owner_id, provider_id, "injects", reason="DI consumer/registration", score=0.75, synthesizer=f"{match.framework}-resolver"))

            for match in _CALLBACK.finditer(line):
                callback = match.group("callback")
                callback_names = set(abstract_state.get(callback).callables) or {callback}
                for callback_name in sorted(callback_names):
                    for target in _resolved_symbol_targets(
                        symbols,
                        callback_name,
                        rel,
                        imported_paths=imported_paths.get(rel, set()),
                        aliases=aliases_by_path.get(rel, {}),
                    ):
                        add_edge(_edge(owner_id, target.id, "calls", reason="callback passed to continuation", score=0.75, synthesizer="callback-resolver"))

            for match in _ABSTRACT_ALIAS_CALL.finditer(line):
                callee = match.group("callee")
                abstract_callee = abstract_state.get(callee)
                target_names = set(abstract_callee.callables) | set(abstract_callee.types)
                for target_name in sorted(target_names):
                    if target_name == callee:
                        continue
                    for target in _resolved_symbol_targets(
                        symbols,
                        target_name,
                        rel,
                        imported_paths=imported_paths.get(rel, set()),
                        aliases=aliases_by_path.get(rel, {}),
                    ):
                        add_edge(_edge(
                            owner_id,
                            target.id,
                            "calls",
                            reason="abstract callable/type dispatch",
                            score=0.8,
                            synthesizer="abstract-dispatch",
                        ))

            if Path(rel).suffix.lower() in {".c", ".h", ".cc", ".cpp", ".cxx", ".hpp"}:
                for match in _C_FUNCTION_POINTER.finditer(line):
                    target_name = match.group("target")
                    for target in (symbols.get(target_name) or [])[:8]:
                        add_edge(_edge(owner_id, target.id, "calls", reason="C/C++ function pointer assignment", score=0.7, synthesizer="c-function-pointer"))

            for match in _DYNAMIC.finditer(line):
                kind = match.group("kind")
                dynamic_id = f"{rel}::dynamic:{kind}:{line_no}:{match.start()}"
                arguments = _dynamic_arguments(line[match.end():])
                argument_index = 1 if kind in {"getattr", "setattr"} else 0
                abstract = (
                    abstract_state.evaluate(arguments[argument_index])
                    if len(arguments) > argument_index
                    else abstract_state.get(kind)
                )
                resolved_values = sorted(abstract.strings())
                add_node(GraphNode(
                    id=dynamic_id,
                    kind=NodeKind.DYNAMIC,
                    path=rel,
                    name=kind,
                    line=line_no,
                    end_line=line_no,
                    language=language,
                    extras={
                        "sink": kind,
                        "resolved": bool(resolved_values),
                        "values": resolved_values,
                    },
                ))
                add_edge(_edge(
                    owner_id,
                    dynamic_id,
                    "dynamic_reference",
                    reason=(
                        f"resolved dynamic reference: {kind}"
                        if resolved_values
                        else f"unresolved dynamic sink: {kind}"
                    ),
                    score=0.7 if resolved_values else 0.4,
                    synthesizer="dynamic-reference",
                ))
                if kind in {"import", "require", "import_module"}:
                    for value in resolved_values:
                        target_path = _resolve_dynamic_file(rel, value, file_paths)
                        if target_path is not None:
                            add_edge(_edge(owner_id, target_path, "imports_dynamic", reason="abstract-state computed import", score=0.8, synthesizer="dynamic-import"))
                else:
                    for value in resolved_values:
                        target_name = value.rsplit(".", 1)[-1].rsplit(":", 1)[-1]
                        for target in _resolved_symbol_targets(
                            symbols,
                            target_name,
                            rel,
                            imported_paths=imported_paths.get(rel, set()),
                            aliases=aliases_by_path.get(rel, {}),
                        ):
                            add_edge(_edge(owner_id, target.id, "reflects_to", reason=f"resolved reflection via {kind}", score=0.75, synthesizer="reflection-resolver"))

            for match in _LITERAL_EVAL.finditer(line):
                body = match.group("body")
                dynamic_id = f"{rel}::dynamic:literal-eval:{line_no}:{match.start()}"
                add_node(GraphNode(
                    id=dynamic_id,
                    kind=NodeKind.DYNAMIC,
                    path=rel,
                    name="literal-eval",
                    line=line_no,
                    end_line=line_no,
                    language=language,
                    extras={"sink": "eval", "resolved": True, "derived_source": body},
                ))
                for call in re.finditer(r"\b([A-Za-z_$][\w$]*)\s*\(", body):
                    for target in (symbols.get(call.group(1)) or [])[:8]:
                        add_edge(_edge(owner_id, target.id, "calls", reason="constant eval payload", score=0.65, synthesizer="literal-eval"))

            for match in _BRIDGE_DECL.finditer(line):
                name = match.group("name")
                bridge_id = f"bridge::native-method:{name}"
                add_node(GraphNode(
                    id=bridge_id,
                    kind=NodeKind.DYNAMIC,
                    name=name,
                    extras={"channel": "native-method", "provenance": "framework"},
                ))
                add_edge(_edge(bridge_id, owner_id, "bridges_to", reason="native bridge declaration", score=0.8, synthesizer="native-bridge"))

            for match in _BRIDGE_CALL.finditer(line):
                name = match.group("name")
                bridge_id = f"bridge::native-method:{name}"
                add_node(GraphNode(
                    id=bridge_id,
                    kind=NodeKind.DYNAMIC,
                    name=name,
                    extras={"channel": "native-method", "provenance": "framework"},
                ))
                add_edge(_edge(owner_id, bridge_id, "calls", reason="React Native/Expo bridge call", score=0.8, synthesizer="native-bridge"))

    graph.nodes = nodes
    graph.edges = edges
    graph.meta["semantic_synthesized_nodes"] = len(nodes) - initial_node_count
    graph.meta["semantic_synthesized_edges"] = len(edges) - initial_edge_count
    graph.meta["semantic_resolver_version"] = 1
    return graph


def _abstract_import_aliases(
    aliases: Mapping[str, str],
    symbols: Mapping[str, list[GraphNode]],
) -> tuple[dict[str, str], dict[str, str]]:
    callable_aliases: dict[str, str] = {}
    type_aliases: dict[str, str] = {}
    callable_kinds = {NodeKind.FUNCTION, NodeKind.METHOD}
    type_kinds = {
        NodeKind.CLASS,
        NodeKind.INTERFACE,
        NodeKind.TYPE,
        NodeKind.STRUCT,
        NodeKind.ENUM,
        NodeKind.TRAIT,
    }
    for local, remote in aliases.items():
        remote_name = remote.rsplit(".", 1)[-1].rsplit(":", 1)[-1]
        candidates = symbols.get(remote_name) or []
        kinds = {candidate.kind for candidate in candidates}
        if kinds and kinds <= callable_kinds:
            callable_aliases[local] = remote_name
        elif kinds and kinds <= type_kinds:
            type_aliases[local] = remote_name
    return callable_aliases, type_aliases


def _expression_targets(
    expression: str,
    symbols: dict[str, list[GraphNode]],
    path: str,
    imported_paths: set[str],
    aliases: Mapping[str, str],
    abstract_state: AbstractState,
) -> list[GraphNode]:
    bare = expression.strip().split(".")[-1].split(":")[-1]
    value = abstract_state.get(bare)
    names = set(value.callables) | set(value.types)
    if value.unknown:
        names.add(bare)
    targets = {
        target.id: target
        for name in names
        for target in _resolved_symbol_targets(
            symbols,
            name,
            path,
            imported_paths=imported_paths,
            aliases=aliases,
        )
    }
    return list(targets.values()) if len(targets) == 1 else []


def _dynamic_arguments(suffix: str) -> list[str]:
    """Return the current call's arguments without consuming later calls."""
    depth = 0
    quote = ""
    current: list[str] = []
    arguments: list[str] = []
    for index, char in enumerate(suffix):
        if quote:
            current.append(char)
            if char == quote and (index == 0 or suffix[index - 1] != "\\"):
                quote = ""
            continue
        if char in {"'", '"', "`"}:
            quote = char
            current.append(char)
        elif char in "([{":
            depth += 1
            current.append(char)
        elif char in ")]}":
            if char == ")" and depth == 0:
                value = "".join(current).strip()
                if value:
                    arguments.append(value)
                break
            depth = max(0, depth - 1)
            current.append(char)
        elif char == "," and depth == 0:
            arguments.append("".join(current).strip())
            current = []
        else:
            current.append(char)
    return arguments


def _resolve_dynamic_file(
    source_path: str,
    value: str,
    file_paths: Iterable[str],
) -> str | None:
    available = set(file_paths)
    raw = value.replace("\\", "/").split("?", 1)[0]
    base = (
        (Path(source_path).parent / raw).as_posix()
        if raw.startswith(".")
        else raw.lstrip("/")
    )
    candidates = [base]
    candidates.extend(f"{base}{suffix}" for suffix in (
        ".py", ".pyi", ".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs",
        ".go", ".rs", ".java", ".cs", ".swift", ".kt", ".rb", ".php",
    ))
    candidates.extend(f"{base}/index{suffix}" for suffix in (
        ".ts", ".tsx", ".js", ".jsx", ".py",
    ))
    return next((candidate for candidate in candidates if candidate in available), None)


def _route_handler_id(
    nodes: list[GraphNode],
    symbols: dict[str, list[GraphNode]],
    path: str,
    line: int,
    suffix: str,
    owner_id: str,
    imported_paths: set[str],
    aliases: Mapping[str, str],
    abstract_state: AbstractState,
) -> str | None:
    identifiers = re.findall(r"[A-Za-z_$][\w$]*", suffix)
    for identifier in reversed(identifiers):
        value = abstract_state.get(identifier)
        resolved_names = set(value.callables) | set(value.types)
        if len(resolved_names) > 1:
            return None
        candidate_names = resolved_names or {identifier}
        resolved: dict[str, GraphNode] = {}
        had_candidates = False
        for candidate_name in candidate_names:
            had_candidates = had_candidates or bool(symbols.get(candidate_name))
            for target in _resolved_symbol_targets(
                symbols,
                candidate_name,
                path,
                imported_paths=imported_paths,
                aliases=aliases,
            ):
                resolved[target.id] = target
        if len(resolved) == 1:
            return next(iter(resolved))
        if len(resolved) > 1 or had_candidates:
            return None
    if owner_id != path:
        return owner_id
    following = [
        node
        for node in nodes
        if node.path == path and node.kind != NodeKind.FILE and line <= node.line <= line + 8
    ]
    return min(following, key=lambda node: node.line).id if following else None


def _registration_owner(
    nodes: list[GraphNode],
    path: str,
    owner_id: str,
    target_id: str | None,
) -> str:
    """Avoid self-cycles for decorator/annotation-style registrations."""
    if target_id is None or owner_id != target_id:
        return owner_id
    target = next((node for node in nodes if node.id == target_id), None)
    qualname = str((target.extras if target else {}).get("qualname") or "")
    if "." in qualname:
        parent = qualname.rsplit(".", 1)[0]
        parent_id = next(
            (
                node.id
                for node in nodes
                if node.path == path
                and str(node.extras.get("qualname") or node.name) == parent
            ),
            None,
        )
        if parent_id is not None:
            return parent_id
    return path


def _file_route_path(path: str) -> str:
    for prefix in _FILE_ROUTE_PREFIXES:
        if path.startswith(prefix):
            path = path[len(prefix):]
            break
    stem = str(Path(path).with_suffix("")).replace("\\", "/")
    stem = re.sub(r"/(?:index|page)$", "", stem)
    stem = stem.replace("[...", "*").replace("[", ":").replace("]", "")
    return "/" + stem.strip("/")
