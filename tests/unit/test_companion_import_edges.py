"""rank 18 — import/dependents graph for non-Python repos (TS/JS + Go), feeding the
same build_dependents reverse index used by the prompt layer."""

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


# ---------------------------------------------------------------------------
# TS / JS relative import resolution
# ---------------------------------------------------------------------------

def test_js_relative_imports_resolve_to_files(tmp_path):
    _write(tmp_path, {
        "package.json": "{\"name\": \"app\"}\n",
        "src/models.ts": "export class Model {}\n",
        "src/util.ts": "export function helper() {}\n",
        "src/handlers.ts": (
            "import { Model } from './models';\n"
            "import { helper } from './util';\n"
            "export function handle() { return new Model(); }\n"
        ),
        "src/index.ts": "export { handle } from './handlers';\n",
    })
    _commit(tmp_path)
    repo_map = RepoMapper(tmp_path).map_repo()
    deps = repo_map.dependents
    assert "src/handlers.ts" in deps.get("src/models.ts", [])
    assert "src/handlers.ts" in deps.get("src/util.ts", [])
    assert "src/index.ts" in deps.get("src/handlers.ts", [])


def test_js_require_and_index_resolution(tmp_path):
    _write(tmp_path, {
        "package.json": "{\"name\": \"app\"}\n",
        "lib/core/index.js": "module.exports = { core: 1 };\n",
        "lib/app.js": "const core = require('./core');\n",
    })
    _commit(tmp_path)
    repo_map = RepoMapper(tmp_path).map_repo()
    # './core' resolves to lib/core/index.js
    assert "lib/app.js" in repo_map.dependents.get("lib/core/index.js", [])


def test_js_bare_package_imports_are_not_edges(tmp_path):
    _write(tmp_path, {
        "package.json": "{\"name\": \"app\"}\n",
        "src/a.ts": "import React from 'react';\nexport const x = 1;\n",
    })
    _commit(tmp_path)
    repo_map = RepoMapper(tmp_path).map_repo()
    # No relative edges -> no dependents at all from this file.
    assert repo_map.dependents == {} or "react" not in str(repo_map.dependents)


# ---------------------------------------------------------------------------
# Go package import resolution
# ---------------------------------------------------------------------------

def test_go_package_imports_resolve(tmp_path):
    _write(tmp_path, {
        "go.mod": "module example.com/app\n\ngo 1.21\n",
        "main.go": (
            "package main\n\n"
            "import (\n"
            "\t\"example.com/app/core\"\n"
            "\t\"fmt\"\n"
            ")\n\n"
            "func main() { fmt.Println(core.Hello()) }\n"
        ),
        "core/core.go": "package core\n\nfunc Hello() string { return \"hi\" }\n",
    })
    _commit(tmp_path)
    repo_map = RepoMapper(tmp_path).map_repo()
    deps = repo_map.dependents
    assert "main.go" in deps.get("core/core.go", [])
    # stdlib import (fmt) creates no edge.
    assert all("fmt" not in k for k in deps)


def test_go_external_imports_ignored(tmp_path):
    _write(tmp_path, {
        "go.mod": "module example.com/app\n\ngo 1.21\n",
        "main.go": (
            "package main\n\n"
            "import \"github.com/other/pkg\"\n\n"
            "func main() {}\n"
        ),
    })
    _commit(tmp_path)
    repo_map = RepoMapper(tmp_path).map_repo()
    assert repo_map.dependents == {}


# ---------------------------------------------------------------------------
# never-raise / schema preservation
# ---------------------------------------------------------------------------

def test_all_import_edges_never_raises(tmp_path):
    _write(tmp_path, {
        "go.mod": "module x\n",
        "broken.ts": "import { from './oops\n",  # malformed
        "broken.go": "package main\nimport (\n",   # unterminated block
    })
    _commit(tmp_path)
    mapper = RepoMapper(tmp_path)
    # Must not raise and must return a list.
    edges = mapper._all_import_edges(mapper.get_git_files())
    assert isinstance(edges, list)


def test_dependents_respect_max_cap(tmp_path):
    importers = {f"src/m{i}.ts": "import { X } from './target';\n" for i in range(20)}
    files = {"package.json": "{}\n", "src/target.ts": "export const X = 1;\n", **importers}
    _write(tmp_path, files)
    _commit(tmp_path)
    repo_map = RepoMapper(tmp_path).map_repo()
    dep_list = repo_map.dependents.get("src/target.ts", [])
    assert len(dep_list) <= RepoMapper._DEPENDENTS_MAX
