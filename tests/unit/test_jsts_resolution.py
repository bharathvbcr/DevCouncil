"""Phase 3 — JS/TS barrel re-exports and multi-tsconfig path mapping."""

from __future__ import annotations

import subprocess

from devcouncil.indexing.repo_mapper import RepoMapper


def _git(root, *args):
    subprocess.run(["git", *args], cwd=root, check=True, capture_output=True, text=True)


def _commit(root):
    _git(root, "init")
    _git(root, "-c", "user.email=t@t", "-c", "user.name=t", "add", "-A")
    _git(root, "-c", "user.email=t@t", "-c", "user.name=t", "commit", "-m", "init")


def _write(tmp_path, files):
    for rel, content in files.items():
        p = tmp_path / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(content, encoding="utf-8")


def test_barrel_export_star_followed(tmp_path):
    """Importer of a barrel also depends on ``export * from`` targets."""
    _write(tmp_path, {
        "package.json": '{"name": "app"}\n',
        "src/models.ts": "export class Model {}\n",
        "src/index.ts": "export * from './models';\n",
        "src/app.ts": "import { Model } from './index';\n",
    })
    _commit(tmp_path)
    repo_map = RepoMapper(tmp_path).map_repo()
    deps = repo_map.dependents
    assert "src/app.ts" in deps.get("src/index.ts", [])
    assert "src/app.ts" in deps.get("src/models.ts", [])


def test_barrel_named_reexport_followed(tmp_path):
    _write(tmp_path, {
        "package.json": '{"name": "app"}\n',
        "src/util.ts": "export function helper() { return 1; }\n",
        "src/barrel.ts": "export { helper } from './util';\n",
        "src/consumer.ts": "import { helper } from './barrel';\n",
    })
    _commit(tmp_path)
    repo_map = RepoMapper(tmp_path).map_repo()
    assert "src/consumer.ts" in repo_map.dependents.get("src/util.ts", [])


def test_tsconfig_extends_merges_paths(tmp_path):
    _write(tmp_path, {
        "tsconfig.base.json": (
            '{"compilerOptions": {"baseUrl": ".", "paths": {"@/*": ["src/*"]}}}\n'
        ),
        "tsconfig.json": (
            '{"extends": "./tsconfig.base.json", '
            '"compilerOptions": {"paths": {"@lib/*": ["lib/*"]}}}\n'
        ),
        "package.json": '{"name": "app"}\n',
        "src/models.ts": "export class Model {}\n",
        "lib/core.ts": "export const core = 1;\n",
        "src/app.ts": (
            "import { Model } from '@/models';\n"
            "import { core } from '@lib/core';\n"
        ),
    })
    _commit(tmp_path)
    repo_map = RepoMapper(tmp_path).map_repo()
    assert "src/app.ts" in repo_map.dependents.get("src/models.ts", [])
    assert "src/app.ts" in repo_map.dependents.get("lib/core.ts", [])


def test_tsconfig_project_references_merge_paths(tmp_path):
    _write(tmp_path, {
        "tsconfig.json": (
            '{"files": [], "references": [{"path": "./packages/ui"}]}\n'
        ),
        "packages/ui/tsconfig.json": (
            '{"compilerOptions": {"baseUrl": ".", "paths": {"@ui/*": ["src/*"]}}}\n'
        ),
        "package.json": '{"name": "app"}\n',
        "packages/ui/src/Button.ts": "export const Button = 1;\n",
        "packages/ui/src/app.ts": "import { Button } from '@ui/Button';\n",
    })
    _commit(tmp_path)
    repo_map = RepoMapper(tmp_path).map_repo()
    assert "packages/ui/src/app.ts" in repo_map.dependents.get(
        "packages/ui/src/Button.ts", []
    )


def test_nested_tsconfig_without_root_references(tmp_path):
    """Monorepo apps/*/tsconfig.json must load even when root has no references."""
    _write(tmp_path, {
        "apps/desktop/tsconfig.json": (
            '{"compilerOptions": {"baseUrl": ".", "paths": {"@/*": ["./src/*"]}}}\n'
        ),
        "package.json": '{"name": "root", "private": true}\n',
        "apps/desktop/package.json": '{"name": "@app/desktop"}\n',
        "apps/desktop/src/components/Button.ts": "export const Button = 1;\n",
        "apps/desktop/src/App.ts": "import { Button } from '@/components/Button';\n",
        "apps/desktop/src/main.ts": "import('./App');\n",
    })
    _commit(tmp_path)
    m = RepoMapper(tmp_path)
    files = {
        "apps/desktop/tsconfig.json",
        "apps/desktop/src/components/Button.ts",
        "apps/desktop/src/App.ts",
        "apps/desktop/src/main.ts",
        "apps/desktop/package.json",
        "package.json",
    }
    m._last_file_set = files
    # Clear any prior empty alias cache from construction.
    m._js_alias_cache = None
    assert (
        m._resolve_js_spec(
            "apps/desktop/src/App.ts", "@/components/Button", files
        )
        == "apps/desktop/src/components/Button.ts"
    )
    edges = m._js_import_edges(sorted(files), files)
    assert ("apps/desktop/src/App.ts", "apps/desktop/src/components/Button.ts") in edges


def test_graph_ts_named_export_list_marks_exported():
    from devcouncil.indexing.graph.extract_ts import extract_ts_js
    ext = extract_ts_js(
        "src/x.ts",
        "function helper() { return 1 }\nexport { helper }\n",
    )
    sym = next(s for s in ext.symbols if s.qualname == "helper")
    assert sym.exported is True
