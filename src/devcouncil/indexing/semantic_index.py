"""Semantic snapshots and diff classification.

NOTE (Phase 4 legacy audit): task-scoped semantic diffs for verification — distinct
from the code symbol graph in ``indexing/graph/``. Keep; not overlapping dead-code.
"""

from __future__ import annotations

import ast
import hashlib
import json
import re
from datetime import datetime, timezone
from pathlib import Path

from devcouncil.indexing.ast_matcher import AstMatcher
from devcouncil.indexing.lsp import LspInspector
from devcouncil.indexing.walk import iter_project_files
from devcouncil.storage.db import get_db
from devcouncil.storage.native import SemanticDiffRepository
from devcouncil.utils.json_persist import read_json, write_json

_CONFIG_FILES = {
    "pyproject.toml",
    "package.json",
    "uv.lock",
    "schema.prisma",
    "docker-compose.yml",
    "Dockerfile",
}


class SemanticIndex:
    def __init__(self, project_root: Path):
        self.project_root = project_root.resolve()
        self.semantic_dir = self.project_root / ".devcouncil" / "semantic"
        self.matcher = AstMatcher(self.project_root)

    def snapshot_path(self, task_id: str, stage: str) -> Path:
        return self.semantic_dir / task_id / f"{stage}.json"

    def create_snapshot(self, task_id: str, stage: str) -> Path:
        # Walk the tree ONCE and share the file list across every collector. Previously
        # create_snapshot triggered four independent full-tree rglob() traversals (symbol
        # matching, source-file hashing, import extraction, and LSP language detection);
        # each collector now filters this single in-memory list with its own predicate, so
        # the on-disk output is unchanged while the filesystem is walked a single time.
        all_files = list(iter_project_files(self.project_root))
        rel_files = [str(path.relative_to(self.project_root)) for path in all_files]
        symbols = self._collect_symbols(all_files)
        source_files, imports = self._collect_source_data(all_files)
        payload = {
            "task_id": task_id,
            "created_at": datetime.now(timezone.utc).isoformat(),
            "repo_map_path": str(self.project_root / ".devcouncil" / "repo_map.json"),
            "files": self._config_file_entries(),
            "source_files": source_files,
            "symbols": symbols,
            "imports": imports,
            "public_symbols": [s for s in symbols if s.get("public")],
            "lsp": json.loads(LspInspector(self.project_root).summary_json(rel_files)),
        }
        path = self.snapshot_path(task_id, stage)
        path.parent.mkdir(parents=True, exist_ok=True)
        write_json(path, payload)
        return path

    def diff(self, task_id: str) -> dict:
        before_path = self.snapshot_path(task_id, "before")
        after_path = self.snapshot_path(task_id, "after")
        if not after_path.exists():
            self.create_snapshot(task_id, "after")
        before = read_json(before_path) if before_path.exists() else {}
        after = read_json(after_path) if after_path.exists() else {}
        classifications = self._classify(before, after)
        summary = ", ".join(item["type"] for item in classifications) or "no semantic changes"
        db = get_db(self.project_root)
        if db:
            with db.get_session() as session:
                SemanticDiffRepository(session).save(
                    task_id,
                    str(before_path),
                    str(after_path),
                    classifications,
                    summary,
                )
        return {"classifications": classifications, "summary": summary}

    def _collect_symbols(self, files: list[Path] | None = None) -> list[dict]:
        symbols: list[dict] = []
        for match in self.matcher.match(limit=500, files=files):
            symbols.append({
                "path": match.path,
                "language": match.language,
                "kind": match.kind,
                "name": match.name,
                "line": match.line,
                "signature": match.text.strip(),
                "public": match.name[:1].isupper() or "export" in match.text,
            })
        return symbols

    def _collect_source_data(
        self, files: list[Path] | None = None
    ) -> tuple[list[dict], list[dict]]:
        """Hash every source file and extract its imports in a SINGLE read pass.

        Previously ``_source_file_entries`` read every source file's bytes for hashing
        while ``_collect_imports`` independently re-read (and, for Python, re-parsed) the
        same files. This reads each file once: the raw bytes feed the SHA-256 entry, and
        the decoded text feeds import extraction. The import set is a subset of the hashed
        set (it additionally skips dot-prefixed paths), so both outputs match the originals
        exactly — source entries are still sorted by path, imports stay in traversal order.
        """
        if files is None:
            files = list(iter_project_files(self.project_root))
        source_entries: list[dict] = []
        imports: list[dict] = []
        for path in files:
            if not path.is_file() or path.suffix not in {".py", ".ts", ".tsx", ".js", ".go", ".rs"}:
                continue
            rel = path.relative_to(self.project_root).as_posix()
            if self._is_ignored_path(rel):
                continue
            raw = path.read_bytes()
            source_entries.append({"path": rel, "sha256": hashlib.sha256(raw).hexdigest()})
            # Imports additionally skip dot-prefixed relative paths. Filter on the path
            # relative to the project root — filtering on the absolute path would skip
            # everything when the repo itself lives under a dot-directory.
            rel_parts = Path(rel).parts
            if any(part.startswith(".") for part in rel_parts):
                continue
            # Decode the bytes we already read, normalizing newlines the same way
            # Path.read_text(newline=None) did, so parsed import statements are identical.
            source = raw.decode("utf-8", errors="replace").replace("\r\n", "\n").replace("\r", "\n")
            if path.suffix == ".py":
                try:
                    tree = ast.parse(source)
                except (SyntaxError, ValueError):
                    continue
                for node in ast.walk(tree):
                    if isinstance(node, (ast.Import, ast.ImportFrom)):
                        segment = ast.get_source_segment(source, node) or ""
                        imports.append({"path": rel, "statement": segment})
            else:
                for line in source.splitlines():
                    if re.match(r"^\s*(import|from)\s+", line):
                        imports.append({"path": rel, "statement": line.strip()})
        return sorted(source_entries, key=lambda item: item["path"]), imports

    def _classify(self, before: dict, after: dict) -> list[dict]:
        before_symbols = {(s["path"], s["name"]): s for s in before.get("symbols", [])}
        after_symbols = {(s["path"], s["name"]): s for s in after.get("symbols", [])}
        results: list[dict] = []

        for key, after_sym in after_symbols.items():
            before_sym = before_symbols.get(key)
            path = after_sym["path"]
            if path.startswith("tests/") or "/test_" in path:
                results.append({"type": "test_only_change", "path": path, "name": after_sym["name"]})
                continue
            if Path(path).name in _CONFIG_FILES:
                results.append({"type": "config_schema_dependency_change", "path": path, "name": after_sym["name"]})
                continue
            if before_sym is None and after_sym.get("public"):
                results.append({"type": "exported_symbol_added", "path": path, "name": after_sym["name"]})
            elif before_sym and before_sym.get("signature") != after_sym.get("signature"):
                if after_sym.get("public"):
                    results.append({"type": "public_api_change", "path": path, "name": after_sym["name"]})
                else:
                    results.append({"type": "private_implementation_change", "path": path, "name": after_sym["name"]})

        before_imports = {(i["path"], i["statement"]) for i in before.get("imports", [])}
        after_imports = {(i["path"], i["statement"]) for i in after.get("imports", [])}
        for added in after_imports - before_imports:
            results.append({"type": "import_dependency_change", "path": added[0], "statement": added[1]})

        for key in before_symbols:
            if key not in after_symbols and before_symbols[key].get("public"):
                results.append({
                    "type": "exported_symbol_removed",
                    "path": before_symbols[key]["path"],
                    "name": before_symbols[key]["name"],
                })

        before_files = {item["path"]: item.get("content", "") for item in before.get("files", []) if isinstance(item, dict)}
        after_files = {item["path"]: item.get("content", "") for item in after.get("files", []) if isinstance(item, dict)}
        for path, content in after_files.items():
            if Path(path).name in _CONFIG_FILES and before_files.get(path) != content:
                results.append({"type": "config_schema_dependency_change", "path": path})

        classified_paths = {item.get("path") for item in results}
        before_source = {
            item["path"]: item.get("sha256", "")
            for item in before.get("source_files", [])
            if isinstance(item, dict)
        }
        after_source = {
            item["path"]: item.get("sha256", "")
            for item in after.get("source_files", [])
            if isinstance(item, dict)
        }
        for path, digest in after_source.items():
            if path in classified_paths:
                continue
            if before_source.get(path) == digest:
                continue
            if path.startswith("tests/") or "/test_" in path:
                results.append({"type": "test_only_change", "path": path})
            elif Path(path).name not in _CONFIG_FILES:
                results.append({"type": "private_implementation_change", "path": path})
        return results

    def _config_file_entries(self) -> list[dict]:
        entries: list[dict] = []
        for name in _CONFIG_FILES:
            path = self.project_root / name
            if path.exists():
                entries.append({
                    "path": name,
                    "content": path.read_text(encoding="utf-8", errors="replace"),
                })
        return entries

    def _is_ignored_path(self, rel_path: str) -> bool:
        ignored_parts = {".git", ".devcouncil", "__pycache__", ".venv", "node_modules", "dist", "build", "target", "vendor"}
        return any(part in ignored_parts for part in Path(rel_path).parts)
