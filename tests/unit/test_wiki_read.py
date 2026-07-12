"""Read-only wiki lookup (knowledge.wiki_read) via a real generated OKF bundle."""

from __future__ import annotations

from pathlib import Path

from devcouncil.indexing.repo_mapper import RepoFileEntry, RepoMap, RepoSubsystem
from devcouncil.knowledge.wiki import generate_wiki
from devcouncil.knowledge.wiki_read import read_wiki_page, wiki_dir_for


def _repo_map() -> RepoMap:
    return RepoMap(
        languages=["python"],
        frameworks=["typer"],
        package_managers=["uv"],
        test_commands=["pytest"],
        important_files=["src/pkg/cli/main.py"],
        candidate_files=[],
        files=[
            RepoFileEntry(path="src/pkg/cli/main.py", area="src/pkg/cli", kind="code",
                          language="python", summary="CLI entrypoint"),
            RepoFileEntry(path="src/pkg/core/engine.py", area="src/pkg/core", kind="code",
                          language="python", summary="Core engine"),
        ],
        subsystems=[
            RepoSubsystem(
                area="src/pkg/cli",
                summary="Command-line interface.",
                entry_points=["src/pkg/cli/main.py"],
                critical_files=["src/pkg/cli/main.py"],
                neighbors=["src/pkg/core"],
                handoff_paths=["src/pkg/cli -> src/pkg/core"],
                role_files={"entry": ["src/pkg/cli/main.py"]},
            ),
            RepoSubsystem(
                area="src/pkg/core",
                summary="Core engine and orchestration.",
                entry_points=["src/pkg/core/engine.py"],
                critical_files=["src/pkg/core/engine.py"],
                neighbors=["src/pkg/cli"],
                handoff_paths=[],
                role_files={},
            ),
        ],
    )


def _generate(tmp_path: Path) -> None:
    generate_wiki(tmp_path, _repo_map(), wiki_dir_for(tmp_path), project_name="Demo")


def test_no_wiki_returns_not_found(tmp_path):
    result = read_wiki_page(tmp_path)
    assert result["ok"] is False
    assert result["code"] == "not_found"


def test_listing_returns_pages(tmp_path):
    _generate(tmp_path)
    result = read_wiki_page(tmp_path)
    assert result["ok"] is True
    pages = result["pages"]
    rel_paths = {p["page"] for p in pages}
    assert "index.md" in rel_paths
    assert "subsystems/src-pkg-cli.md" in rel_paths
    assert "log.md" not in rel_paths  # log is excluded


def test_fetch_specific_page(tmp_path):
    _generate(tmp_path)
    result = read_wiki_page(tmp_path, page="subsystems/src-pkg-cli.md")
    assert result["ok"] is True
    assert result["page"] == "subsystems/src-pkg-cli.md"
    assert isinstance(result["body"], str) and result["body"]
    assert result["truncated"] is False


def test_fetch_unknown_page(tmp_path):
    _generate(tmp_path)
    result = read_wiki_page(tmp_path, page="does/not/exist.md")
    assert result["ok"] is False
    assert result["code"] == "not_found"
    assert "index.md" in result["available"]


def test_query_search_finds_page(tmp_path):
    _generate(tmp_path)
    result = read_wiki_page(tmp_path, query="command-line interface cli")
    assert result["ok"] is True
    assert result["query"] == "command-line interface cli"
    assert result["matches"]
    # top match should be surfaced with a body
    assert "body" in result


def test_query_with_no_matches(tmp_path):
    _generate(tmp_path)
    result = read_wiki_page(tmp_path, query="zzz_no_such_term_qqq")
    assert result["ok"] is True
    assert result["matches"] == []
    assert "available" in result
