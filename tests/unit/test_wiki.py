"""Codebase wiki generation: skeleton, incremental updates, enrichment, and OKF conformance."""

from pathlib import Path

from devcouncil.indexing.repo_mapper import RepoFileEntry, RepoMap, RepoSubsystem
from devcouncil.knowledge.okf import read_bundle, validate_bundle
from devcouncil.knowledge.wiki import (
    WikiProse,
    generate_wiki,
    slugify,
    wiki_stale_pages,
)


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


def test_generate_creates_valid_okf_bundle(tmp_path: Path):
    wiki_dir = tmp_path / ".devcouncil" / "knowledge" / "okf" / "wiki"
    result = generate_wiki(tmp_path, _repo_map(), wiki_dir, project_name="Demo")

    assert (wiki_dir / "index.md").is_file()
    assert (wiki_dir / "overview" / "development.md").is_file()
    assert (wiki_dir / "subsystems" / "src-pkg-cli.md").is_file()
    assert (wiki_dir / "log.md").is_file()
    assert (wiki_dir / ".wiki-state.json").is_file()
    assert result.problems == []  # every doc typed, every link resolves
    assert len(result.created) == 4  # index + development + 2 subsystems

    bundle = read_bundle(wiki_dir)
    assert validate_bundle(bundle) == []
    index = bundle.by_path()["index.md"]
    assert "subsystems/src-pkg-cli.md" in index.links
    cli_page = bundle.by_path()["subsystems/src-pkg-cli.md"]
    assert cli_page.type == "Subsystem"
    assert "cli" in cli_page.tags  # tags double as prompt-selection keywords
    assert "subsystems/src-pkg-core.md" in cli_page.links  # neighbor cross-link


def test_second_run_is_incremental_noop(tmp_path: Path):
    wiki_dir = tmp_path / "wiki"
    generate_wiki(tmp_path, _repo_map(), wiki_dir, project_name="Demo")
    second = generate_wiki(tmp_path, _repo_map(), wiki_dir, project_name="Demo")
    assert second.created == []
    assert second.updated == []
    assert len(second.skipped) == 4


def test_changed_subsystem_refreshes_only_its_page(tmp_path: Path):
    wiki_dir = tmp_path / "wiki"
    generate_wiki(tmp_path, _repo_map(), wiki_dir, project_name="Demo")

    changed = _repo_map()
    changed.subsystems[0].summary = "Command-line interface (now with subcommands)."
    result = generate_wiki(tmp_path, changed, wiki_dir, project_name="Demo")

    # The CLI page and the index (which embeds summaries) refresh; core is untouched.
    assert "subsystems/src-pkg-cli.md" in result.updated
    assert "subsystems/src-pkg-core.md" in result.skipped


def test_stale_pages_reported_for_missing_and_outdated(tmp_path: Path):
    wiki_dir = tmp_path / "wiki"
    repo_map = _repo_map()
    generate_wiki(tmp_path, repo_map, wiki_dir, project_name="Demo")
    assert wiki_stale_pages(tmp_path, repo_map, wiki_dir) == {}

    (wiki_dir / "subsystems" / "src-pkg-core.md").unlink()
    repo_map.subsystems[0].summary = "Different."
    stale = wiki_stale_pages(tmp_path, repo_map, wiki_dir)
    assert stale["subsystems/src-pkg-core.md"] == "missing"
    assert stale["subsystems/src-pkg-cli.md"] == "outdated"


def test_enrichment_adds_prose_and_preserves_it_on_noop_update(tmp_path: Path):
    class FakeRouter:
        async def complete_structured(self, role, messages, schema, fallback=None, **kwargs):
            assert role == "wiki_writer"
            return WikiProse(
                overview="The CLI wires user commands into the core engine.",
                key_flows=["main() parses args and dispatches"],
                agent_guidance=["Register new commands in main.py"],
            )

    wiki_dir = tmp_path / "wiki"
    result = generate_wiki(tmp_path, _repo_map(), wiki_dir, router=FakeRouter(), project_name="Demo")
    assert "subsystems/src-pkg-cli.md" in result.enriched

    page = (wiki_dir / "subsystems" / "src-pkg-cli.md").read_text(encoding="utf-8")
    assert "## Overview" in page
    assert "wires user commands" in page
    assert "## Guidance for agents" in page

    # A no-op update (no router) must not clobber the enriched page.
    generate_wiki(tmp_path, _repo_map(), wiki_dir, project_name="Demo")
    assert "wires user commands" in (wiki_dir / "subsystems" / "src-pkg-cli.md").read_text(encoding="utf-8")


def test_log_accumulates_entries_newest_first(tmp_path: Path):
    wiki_dir = tmp_path / "wiki"
    generate_wiki(tmp_path, _repo_map(), wiki_dir, project_name="Demo")
    changed = _repo_map()
    changed.subsystems[1].summary = "Core engine v2."
    generate_wiki(tmp_path, changed, wiki_dir, project_name="Demo")

    log = (wiki_dir / "log.md").read_text(encoding="utf-8")
    assert log.count("## 2") >= 2  # two timestamped entries
    assert "Created" in log and "Updated" in log


def test_slugify():
    assert slugify("src/pkg/cli/") == "src-pkg-cli"
    assert slugify("") == "root"


def test_corrupt_log_restarts_instead_of_crashing(tmp_path: Path):
    """A corrupt (non-UTF8 / malformed) log.md must not fail wiki generation."""
    wiki_dir = tmp_path / "wiki"
    generate_wiki(tmp_path, _repo_map(), wiki_dir, project_name="Demo")
    (wiki_dir / "log.md").write_bytes(b"\xff\xfe garbage \x00\x9c")
    changed = _repo_map()
    changed.subsystems[1].summary = "Core engine v3."
    result = generate_wiki(tmp_path, changed, wiki_dir, project_name="Demo")
    assert result is not None
    log = (wiki_dir / "log.md").read_text(encoding="utf-8", errors="replace")
    assert "# Wiki change log" in log


def test_load_repo_map_regenerates_on_corrupt_json(tmp_path: Path, monkeypatch):
    from devcouncil.knowledge import wiki as wiki_mod

    dev = tmp_path / ".devcouncil"
    dev.mkdir()
    (dev / "repo_map.json").write_text("{not json", encoding="utf-8")
    sentinel = _repo_map()
    called = {}

    def _fake_generate(root, map_path, *args, **kwargs):
        called["ran"] = True
        return sentinel

    monkeypatch.setattr(
        "devcouncil.indexing.map_artifacts.generate_map_artifacts", _fake_generate
    )
    result = wiki_mod._load_repo_map(tmp_path, remap=False)
    assert called.get("ran") is True
    assert result is sentinel
