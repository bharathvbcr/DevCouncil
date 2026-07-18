"""sha256 parse cache covers Python modules and JS/TS import specs."""

from __future__ import annotations

import json
import subprocess

from devcouncil.indexing.graph.cache import load_parse_cache, save_parse_cache
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


def test_js_parse_cache_reused_across_runs(tmp_path, monkeypatch):
    _write(tmp_path, {
        "package.json": '{"name": "app"}\n',
        "src/models.ts": "export class Model {}\n",
        "src/handlers.ts": "import { Model } from './models';\nexport const h = Model;\n",
    })
    _commit(tmp_path)

    mapper = RepoMapper(tmp_path)
    first = mapper.map_repo(liveness=False)
    assert "src/handlers.ts" in first.dependents.get("src/models.ts", [])

    cache_path = tmp_path / ".devcouncil" / "cache" / "repo_map_parse.json"
    assert cache_path.is_file()
    data = json.loads(cache_path.read_text(encoding="utf-8"))
    assert data["version"] == RepoMapper._PARSE_CACHE_VERSION
    handlers = data["files"]["src/handlers.ts"]
    assert handlers["sha256"]
    assert "./models" in handlers["specs"]

    calls: list[str] = []
    real = RepoMapper._extract_js_import_specs

    def tracking(self, source: str):
        calls.append(source[:40])
        return real(self, source)

    monkeypatch.setattr(RepoMapper, "_extract_js_import_specs", tracking)
    second = RepoMapper(tmp_path).map_repo(liveness=False)
    assert "src/handlers.ts" in second.dependents.get("src/models.ts", [])
    assert calls == []  # cache hit — no re-extraction


def test_js_and_python_share_parse_cache(tmp_path):
    _write(tmp_path, {
        "pkg/a.py": "from pkg import b\n",
        "pkg/b.py": "x = 1\n",
        "pkg/__init__.py": "",
        "src/util.ts": "export const n = 1;\n",
        "src/app.ts": "import { n } from './util';\nexport const v = n;\n",
    })
    _commit(tmp_path)
    RepoMapper(tmp_path).map_repo(liveness=False)
    data = json.loads(
        (tmp_path / ".devcouncil" / "cache" / "repo_map_parse.json").read_text(
            encoding="utf-8"
        )
    )
    files = data["files"]
    assert "modules" in files["pkg/a.py"]
    assert "specs" in files["src/app.ts"]
    assert "./util" in files["src/app.ts"]["specs"]


def test_parse_cache_version_mismatch_is_ignored(tmp_path):
    _write(tmp_path, {
        "src/a.ts": "import './b';\n",
        "src/b.ts": "export {};\n",
    })
    _commit(tmp_path)
    cache_dir = tmp_path / ".devcouncil" / "cache"
    cache_dir.mkdir(parents=True)
    (cache_dir / "repo_map_parse.json").write_text(
        json.dumps({
            "version": 1,
            "files": {
                "src/a.ts": {"sha256": "deadbeef", "specs": ["./stale"]},
            },
        }),
        encoding="utf-8",
    )
    repo_map = RepoMapper(tmp_path).map_repo(liveness=False)
    assert "src/a.ts" in repo_map.dependents.get("src/b.ts", [])
    data = json.loads(
        (cache_dir / "repo_map_parse.json").read_text(encoding="utf-8")
    )
    assert data["version"] == RepoMapper._PARSE_CACHE_VERSION
    assert "./b" in data["files"]["src/a.ts"]["specs"]


def test_save_parse_cache_is_atomic(tmp_path, monkeypatch):
    """save_parse_cache must write via atomic_write_json (temp + replace)."""
    calls: list[object] = []

    def tracking(path, payload, **kwargs):  # noqa: ANN001
        calls.append((path, payload))
        # Still perform a real write so load works.
        from devcouncil.utils.fsio import atomic_write_json as real

        return real(path, payload, **kwargs)

    monkeypatch.setattr(
        "devcouncil.indexing.graph.cache.atomic_write_json",
        tracking,
    )
    files = {"pkg/a.py": {"sha256": "abc", "symbols": [], "import_details": []}}
    save_parse_cache(tmp_path, files)
    assert calls
    assert calls[0][1]["files"] == files
    loaded = load_parse_cache(tmp_path)
    assert loaded == files


def test_extract_cached_survives_extractor_crash_without_poisoning_cache(tmp_path, monkeypatch):
    """A file that crashes its extractor is indexed as opaque and never cached."""
    from devcouncil.indexing.graph import cache as cache_mod

    target = tmp_path / "hostile.py"
    target.write_text("x = 1\n", encoding="utf-8")

    def exploding(path, source):  # noqa: ANN001
        raise RecursionError("maximum recursion depth exceeded")

    monkeypatch.setattr(cache_mod, "extract_file", exploding)
    extraction, entry = cache_mod.extract_cached(tmp_path, "hostile.py", cache={})

    assert extraction.path == "hostile.py"
    assert extraction.symbols == []
    # sha256 stays empty so this entry can never satisfy a warm-cache hit and
    # the file is retried once the extractor is fixed.
    assert entry["sha256"] == ""
