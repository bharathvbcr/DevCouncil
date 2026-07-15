from __future__ import annotations

import json
from pathlib import Path

from devcouncil.codeintel.languages import (
    LANGUAGE_SPECS,
    detect_language,
    grammar_status,
    supported_languages,
)
from devcouncil.codeintel.query import CodeIntelQueryEngine
from devcouncil.codeintel.resolution import AbstractState, AbstractValue, enrich_semantic_edges
from devcouncil.codeintel.resolution.frameworks import FRAMEWORK_MANIFEST
from devcouncil.codeintel.service import get_codeintel_service
from devcouncil.indexing.graph.extract_python import ExtractedImport, FileExtraction
from devcouncil.indexing.graph.schema import CodeGraph, GraphEdge, GraphNode, NodeKind


def test_language_manifest_covers_release_matrix_without_download() -> None:
    names = supported_languages()
    expected_grammars = {
        "astro", "c", "cfml", "cobol", "cpp", "csharp", "css", "cuda", "dart",
        "erlang", "go", "hcl", "html", "java", "javascript", "kotlin", "liquid",
        "lua", "luau", "nix", "objc", "pascal", "php", "python", "r", "ruby",
        "rust", "scala", "solidity", "svelte", "swift", "tsx", "typescript", "vb",
        "vue",
    }
    assert {"Python", "TypeScript", "ArkTS", "C++", "Swift", "Svelte", "Terraform/OpenTofu", "Nix"} <= set(names)
    assert len(names) == 35
    assert {
        grammar
        for spec in LANGUAGE_SPECS
        for grammar in (spec.grammar, *spec.embedded)
    } == expected_grammars
    assert detect_language("app.ets").name == "ArkTS"  # type: ignore[union-attr]
    assert detect_language("main.tf").grammar == "hcl"  # type: ignore[union-attr]
    assert detect_language("Worker.cs").grammar == "csharp"  # type: ignore[union-attr]
    assert detect_language("Worker.vb").grammar == "vb"  # type: ignore[union-attr]
    status = grammar_status()
    assert status["downloaded_at_runtime"] is False
    assert status["required_count"] == len(names)
    svelte = next(row for row in status["languages"] if row["language"] == "Svelte")
    assert {"css", "html"} <= set(svelte["required_grammars"])


def test_bounded_abstract_state_evaluates_strings_paths_and_branches() -> None:
    state = AbstractState()
    state.assign("root", AbstractValue.scalar("src"))
    state.assign("name", AbstractValue(scalars=frozenset({"a.py", "b.py"})))

    assert state.evaluate_python("root + '/' + name").strings() == frozenset({"src/a.py", "src/b.py"})
    assert state.evaluate_python("os.path.join(root, name)").strings() == frozenset({"src/a.py", "src/b.py"})
    assert state.evaluate_python("'yes' if flag else 'no'").strings() == frozenset({"yes", "no"})


def test_abstract_state_analyzes_aliases_types_wrappers_and_templates() -> None:
    state = AbstractState()
    state.analyze(
        "const prefix = '/api';\n"
        "const callback = handler;\n"
        "const token = Service;\n"
        "function route(suffix) { return `${prefix}${suffix}`; }\n"
        "const routePath = route('/items');\n",
        callable_names={"handler"},
        type_names={"Service"},
    )

    assert state.get("callback").callables == frozenset({"handler"})
    assert state.get("token").types == frozenset({"Service"})
    assert state.get("routePath").strings() == frozenset({"/api/items"})


def test_every_advertised_framework_fixture_synthesizes_semantics(
    tmp_path: Path,
) -> None:
    fixture_path = (
        Path(__file__).parents[1]
        / "fixtures"
        / "codeintel"
        / "frameworks"
        / "manifest.json"
    )
    fixtures = json.loads(fixture_path.read_text(encoding="utf-8"))
    advertised = {(spec.family, spec.name) for spec in FRAMEWORK_MANIFEST}
    covered: set[tuple[str, str]] = set()
    fixture_index = 0
    for family, entries in fixtures.items():
        for entry in entries:
            fixture_index += 1
            matching = {
                (spec.family, spec.name)
                for spec in FRAMEWORK_MANIFEST
                if spec.pattern.search(entry["source"])
            }
            assert (family, entry["framework"]) in matching, entry
            covered.add((family, entry["framework"]))
            rel = f"fixture_{fixture_index}.ts"
            (tmp_path / rel).write_text(entry["source"], encoding="utf-8")
            nodes = [
                GraphNode(id=rel, kind=NodeKind.FILE, path=rel, name=rel),
                GraphNode(
                    id=f"{rel}::handler",
                    kind=NodeKind.FUNCTION,
                    path=rel,
                    name="handler",
                    line=1,
                    end_line=1,
                ),
                GraphNode(
                    id=f"{rel}::Service",
                    kind=NodeKind.CLASS,
                    path=rel,
                    name="Service",
                    line=1,
                    end_line=1,
                ),
            ]
            graph = CodeGraph(nodes=nodes)
            enrich_semantic_edges(graph, root=tmp_path, paths={rel})
            if family == "routing":
                assert any(
                    node.kind == NodeKind.ROUTE
                    and node.extras.get("framework") == entry["framework"]
                    for node in graph.nodes
                ), entry
                assert any(edge.kind == "routes_to" for edge in graph.edges), entry
            elif family == "dependency-injection":
                assert any(
                    node.kind == NodeKind.PROVIDER
                    and node.extras.get("framework") == entry["framework"]
                    for node in graph.nodes
                ), entry
                assert any(edge.kind == "provides" for edge in graph.edges), entry
            elif "listener" in entry["framework"] or entry["framework"] == "watchdog-observer":
                assert any(edge.kind == "listens" for edge in graph.edges), entry
            else:
                assert any(edge.kind == "emits" for edge in graph.edges), entry
    assert advertised <= covered


def test_semantic_resolver_synthesizes_routes_events_state_di_and_dynamic(tmp_path: Path) -> None:
    source = tmp_path / "app.tsx"
    source.write_text(
        "function handler() {\n"
        "  const [count, setCount] = useState(0);\n"
        "  bus.emit('ready');\n"
        "  setCount(1);\n"
        "  eval(code);\n"
        "}\n"
        "app.get('/items', handler);\n"
        "container.bind(handler);\n",
        encoding="utf-8",
    )
    graph = CodeGraph(nodes=[
        GraphNode(id="app.tsx", kind=NodeKind.FILE, path="app.tsx", name="app.tsx", language="tsx"),
        GraphNode(id="app.tsx::handler", kind=NodeKind.FUNCTION, path="app.tsx", name="handler", line=1, end_line=6, language="tsx"),
    ])

    enrich_semantic_edges(graph, root=tmp_path)

    kinds = {node.kind for node in graph.nodes}
    edge_kinds = {edge.kind for edge in graph.edges}
    assert {NodeKind.ROUTE, NodeKind.EVENT, NodeKind.STATE, NodeKind.PROVIDER, NodeKind.DYNAMIC} <= kinds
    assert {"routes_to", "emits", "owns_state", "writes_state", "provides", "dynamic_reference"} <= edge_kinds


def test_semantic_resolver_links_file_routes_literal_eval_and_native_bridge(tmp_path: Path) -> None:
    source = tmp_path / "src" / "pages" / "items" / "[id].tsx"
    source.parent.mkdir(parents=True)
    source.write_text(
        "function helper() {}\n"
        "function page() { eval('helper()'); NativeModules.Camera.open(); }\n",
        encoding="utf-8",
    )
    graph = CodeGraph(nodes=[
        GraphNode(id="src/pages/items/[id].tsx", kind=NodeKind.FILE, path="src/pages/items/[id].tsx", name="[id].tsx", language="tsx"),
        GraphNode(id="src/pages/items/[id].tsx::helper", kind=NodeKind.FUNCTION, path="src/pages/items/[id].tsx", name="helper", line=1, end_line=1, language="tsx"),
        GraphNode(id="src/pages/items/[id].tsx::page", kind=NodeKind.FUNCTION, path="src/pages/items/[id].tsx", name="page", line=2, end_line=2, language="tsx"),
    ])

    enrich_semantic_edges(graph, root=tmp_path)

    route = next(node for node in graph.nodes if node.kind == NodeKind.ROUTE)
    assert route.extras["route"] == "/items/:id"
    assert any(edge.reason == "constant eval payload" and edge.target.endswith("::helper") for edge in graph.edges)
    assert any(edge.target == "bridge::native-method:open" for edge in graph.edges)


def test_route_matrix_and_bounded_computed_paths(tmp_path: Path) -> None:
    routes = tmp_path / "routes.tsx"
    routes.write_text(
        "const prefix = '/api';\n"
        "function handler() {}\n"
        "app.get(prefix + '/items', handler);\n"
        "app.MapPost('/orders', handler);\n"
        "@Get('/nest')\n"
        "<Route path='/react' element={<Page />} />\n"
        "const vue = { path: '/vue', component: Page };\n",
        encoding="utf-8",
    )
    graph = CodeGraph(nodes=[
        GraphNode(id="routes.tsx", kind=NodeKind.FILE, path="routes.tsx", name="routes.tsx", language="tsx"),
        GraphNode(id="routes.tsx::handler", kind=NodeKind.FUNCTION, path="routes.tsx", name="handler", line=2, end_line=2, language="tsx"),
    ])

    enrich_semantic_edges(graph, root=tmp_path)

    routes_by_path = {
        str(node.extras["route"]): node
        for node in graph.nodes
        if node.kind == NodeKind.ROUTE
    }
    assert {"/api/items", "/orders", "/nest", "/react", "/vue"} <= routes_by_path.keys()
    assert routes_by_path["/api/items"].extras["expression"] == "prefix + '/items'"
    assert routes_by_path["/orders"].extras["framework"] == "aspnet"
    assert routes_by_path["/nest"].extras["framework"] == "nest"
    assert routes_by_path["/react"].extras["framework"] == "react-router"
    assert routes_by_path["/vue"].extras["framework"] == "vue-router"


def test_semantic_resolver_does_not_bind_ambiguous_framework_targets(
    tmp_path: Path,
) -> None:
    for path in ("a.ts", "b.ts"):
        (tmp_path / path).write_text(
            "function handler() {}\nclass Service {}\n",
            encoding="utf-8",
        )
    (tmp_path / "app.ts").write_text(
        "app.get('/items', handler);\n"
        "bus.on('ready', handler);\n"
        "container.bind(Service);\n",
        encoding="utf-8",
    )
    graph = CodeGraph(nodes=[
        GraphNode(id="app.ts", kind=NodeKind.FILE, path="app.ts", name="app.ts"),
        GraphNode(id="a.ts", kind=NodeKind.FILE, path="a.ts", name="a.ts"),
        GraphNode(id="a.ts::handler", kind=NodeKind.FUNCTION, path="a.ts", name="handler"),
        GraphNode(id="a.ts::Service", kind=NodeKind.CLASS, path="a.ts", name="Service"),
        GraphNode(id="b.ts", kind=NodeKind.FILE, path="b.ts", name="b.ts"),
        GraphNode(id="b.ts::handler", kind=NodeKind.FUNCTION, path="b.ts", name="handler"),
        GraphNode(id="b.ts::Service", kind=NodeKind.CLASS, path="b.ts", name="Service"),
    ])

    enrich_semantic_edges(graph, root=tmp_path)

    ambiguous_targets = {
        "a.ts::handler",
        "a.ts::Service",
        "b.ts::handler",
        "b.ts::Service",
    }
    assert not any(
        edge.kind in {"routes_to", "listens", "provides"}
        and edge.target in ambiguous_targets
        for edge in graph.edges
    )


def test_semantic_resolver_prefers_explicitly_imported_framework_target(
    tmp_path: Path,
) -> None:
    (tmp_path / "routes.ts").write_text(
        "import { handler } from './handlers';\napp.get('/items', handler);\n",
        encoding="utf-8",
    )
    for path in ("handlers.ts", "other.ts"):
        (tmp_path / path).write_text(
            "export function handler() {}\n",
            encoding="utf-8",
        )
    graph = CodeGraph(
        nodes=[
            GraphNode(id="routes.ts", kind=NodeKind.FILE, path="routes.ts", name="routes.ts"),
            GraphNode(id="handlers.ts", kind=NodeKind.FILE, path="handlers.ts", name="handlers.ts"),
            GraphNode(
                id="handlers.ts::handler",
                kind=NodeKind.FUNCTION,
                path="handlers.ts",
                name="handler",
            ),
            GraphNode(id="other.ts", kind=NodeKind.FILE, path="other.ts", name="other.ts"),
            GraphNode(
                id="other.ts::handler",
                kind=NodeKind.FUNCTION,
                path="other.ts",
                name="handler",
            ),
        ],
        edges=[
            GraphEdge(
                source="routes.ts",
                target="handlers.ts",
                kind="imports",
            )
        ],
    )

    enrich_semantic_edges(graph, root=tmp_path)

    route_edges = [edge for edge in graph.edges if edge.kind == "routes_to"]
    assert [edge.target for edge in route_edges] == ["handlers.ts::handler"]


def test_abstract_state_drives_framework_dynamic_import_and_reflection(
    tmp_path: Path,
) -> None:
    (tmp_path / "app.ts").write_text(
        "const prefix = '/api';\n"
        "const callback = handler;\n"
        "const token = Service;\n"
        "const modulePath = './worker';\n"
        "const className = 'Service';\n"
        "app.get(prefix + '/items', callback);\n"
        "container.bind(token);\n"
        "callback();\n"
        "new token();\n"
        "import(modulePath);\n"
        "Class.forName(className);\n",
        encoding="utf-8",
    )
    (tmp_path / "worker.ts").write_text("export const value = 1;\n", encoding="utf-8")
    graph = CodeGraph(nodes=[
        GraphNode(id="app.ts", kind=NodeKind.FILE, path="app.ts", name="app.ts"),
        GraphNode(
            id="app.ts::handler",
            kind=NodeKind.FUNCTION,
            path="app.ts",
            name="handler",
        ),
        GraphNode(
            id="app.ts::Service",
            kind=NodeKind.CLASS,
            path="app.ts",
            name="Service",
        ),
        GraphNode(id="worker.ts", kind=NodeKind.FILE, path="worker.ts", name="worker.ts"),
    ])

    enrich_semantic_edges(graph, root=tmp_path)

    assert any(
        node.kind == NodeKind.ROUTE and node.extras.get("route") == "/api/items"
        for node in graph.nodes
    )
    assert any(edge.kind == "routes_to" and edge.target.endswith("::handler") for edge in graph.edges)
    assert any(edge.kind == "provides" and edge.target.endswith("::Service") for edge in graph.edges)
    assert any(
        edge.kind == "calls"
        and edge.reason == "abstract callable/type dispatch"
        and edge.target.endswith("::handler")
        for edge in graph.edges
    )
    assert any(
        edge.kind == "calls"
        and edge.reason == "abstract callable/type dispatch"
        and edge.target.endswith("::Service")
        for edge in graph.edges
    )
    assert any(edge.kind == "imports_dynamic" and edge.target == "worker.ts" for edge in graph.edges)
    assert any(edge.kind == "reflects_to" and edge.target.endswith("::Service") for edge in graph.edges)


def test_imported_callback_aliases_resolve_routes_events_and_di(tmp_path: Path) -> None:
    (tmp_path / "app.ts").write_text(
        "const callback = importedHandler;\n"
        "const token = ImportedService;\n"
        "app.get('/items', callback);\n"
        "bus.on('ready', callback);\n"
        "container.bind(token);\n",
        encoding="utf-8",
    )
    (tmp_path / "handlers.ts").write_text("", encoding="utf-8")
    graph = CodeGraph(
        nodes=[
            GraphNode(id="app.ts", kind=NodeKind.FILE, path="app.ts", name="app.ts"),
            GraphNode(
                id="handlers.ts",
                kind=NodeKind.FILE,
                path="handlers.ts",
                name="handlers.ts",
            ),
            GraphNode(
                id="handlers.ts::handler",
                kind=NodeKind.FUNCTION,
                path="handlers.ts",
                name="handler",
            ),
            GraphNode(
                id="handlers.ts::Service",
                kind=NodeKind.CLASS,
                path="handlers.ts",
                name="Service",
            ),
        ],
        edges=[GraphEdge(source="app.ts", target="handlers.ts", kind="imports")],
    )
    extraction = FileExtraction(
        path="app.ts",
        language="typescript",
        import_details=[
            ExtractedImport(
                module="./handlers",
                names=["handler", "Service"],
                alias_map={
                    "importedHandler": "handler",
                    "ImportedService": "Service",
                },
            )
        ],
    )

    enrich_semantic_edges(
        graph,
        root=tmp_path,
        extractions={"app.ts": extraction},
    )

    assert any(
        edge.kind == "routes_to" and edge.target == "handlers.ts::handler"
        for edge in graph.edges
    )
    assert any(
        edge.kind == "listens" and edge.target == "handlers.ts::handler"
        for edge in graph.edges
    )
    assert any(
        edge.kind == "provides" and edge.target == "handlers.ts::Service"
        for edge in graph.edges
    )


def test_configuration_route_resolves_inherited_controller_method(
    tmp_path: Path,
) -> None:
    (tmp_path / "routes.rb").write_text(
        "get '/items', ItemsController.index\n",
        encoding="utf-8",
    )
    graph = CodeGraph(nodes=[
        GraphNode(id="routes.rb", kind=NodeKind.FILE, path="routes.rb", name="routes.rb"),
        GraphNode(
            id="controllers.rb::BaseController",
            kind=NodeKind.CLASS,
            path="controllers.rb",
            name="BaseController",
        ),
        GraphNode(
            id="controllers.rb::ItemsController",
            kind=NodeKind.CLASS,
            path="controllers.rb",
            name="ItemsController",
        ),
        GraphNode(
            id="controllers.rb::ItemsController.index",
            kind=NodeKind.METHOD,
            path="controllers.rb",
            name="index",
            extras={"qualname": "ItemsController.index"},
        ),
    ], edges=[
        GraphEdge(source="routes.rb", target="controllers.rb", kind="imports"),
        GraphEdge(
            source="controllers.rb::ItemsController",
            target="controllers.rb::BaseController",
            kind="inherits",
        ),
    ])

    enrich_semantic_edges(graph, root=tmp_path)

    assert any(
        edge.kind == "routes_to"
        and edge.target == "controllers.rb::ItemsController.index"
        for edge in graph.edges
    )


def test_multiple_abstract_framework_targets_remain_unresolved(tmp_path: Path) -> None:
    (tmp_path / "app.ts").write_text(
        "const callback = flag ? first : second;\n"
        "app.get('/items', callback);\n",
        encoding="utf-8",
    )
    graph = CodeGraph(nodes=[
        GraphNode(id="app.ts", kind=NodeKind.FILE, path="app.ts", name="app.ts"),
        GraphNode(id="app.ts::first", kind=NodeKind.FUNCTION, path="app.ts", name="first"),
        GraphNode(id="app.ts::second", kind=NodeKind.FUNCTION, path="app.ts", name="second"),
    ])

    enrich_semantic_edges(graph, root=tmp_path)

    assert not any(edge.kind == "routes_to" for edge in graph.edges)


def test_query_engine_returns_generation_source_path_and_impact(tmp_path: Path) -> None:
    source = tmp_path / "app.py"
    source.write_text("def target():\n    return 1\n", encoding="utf-8")
    graph = CodeGraph(nodes=[
        GraphNode(id="app.py", kind=NodeKind.FILE, path="app.py", name="app.py", language="python"),
        GraphNode(id="app.py::target", kind=NodeKind.FUNCTION, path="app.py", name="target", line=1, end_line=2, language="python"),
    ])
    service = get_codeintel_service(tmp_path)
    service.persist(graph)
    engine = CodeIntelQueryEngine(service)

    result = engine.explore("target")
    assert result["project_root"] == str(tmp_path.resolve())
    assert result["generation"] == 1
    assert "1: def target():" in result["definitions"][0]["source"]
    assert engine.search("target")["matches"][0]["name"] == "target"
