import json
import tarfile
import zipfile
from types import SimpleNamespace

import pytest

from devcouncil.artifacts.graph import ArtifactGraph
from devcouncil.domain.gap import Gap
from devcouncil.domain.requirement import Requirement
from devcouncil.domain.task import Task
from devcouncil.knowledge import fetch as fetch_mod
from devcouncil.knowledge.design import (
    DesignSystem,
    contrast_ratio,
    design_system_to_okf_document,
    lint,
    parse_design_md,
)
from devcouncil.knowledge.frontmatter import build_frontmatter_markdown, split_frontmatter
from devcouncil.knowledge.sources import (
    KnowledgeSource,
    clear_knowledge_caches,
    discover_knowledge_sources,
    render_knowledge_preamble,
    select_knowledge_sources,
)
from devcouncil.reporting.markdown_report import MarkdownReportGenerator
from devcouncil.reporting.okf_html import (
    _is_dangerous_scheme,
    _rewrite_link,
    render_bundle_html,
    render_markdown,
    write_bundle_html,
)
from devcouncil.knowledge.okf import OKFBundle, OKFDocument


def _gap(gap_id: str) -> Gap:
    return Gap(
        id=gap_id,
        severity="critical",
        gap_type="security_risk",
        description=f"Do not ship {gap_id}",
        recommended_fix="fix",
        blocking=True,
    )


def test_markdown_report_verdicts_proof_modes_gap_limit_and_live_review():
    graph = ArtifactGraph()
    graph.add_requirement(
        Requirement(
            id="REQ-1",
            title="Requirement",
            description="desc",
            priority="high",
            source="user",
            acceptance_criteria=[],
        )
    )
    graph.add_task(Task(id="TASK-1", title="Task", description="desc", requirement_ids=["REQ-1"]))
    for idx in range(30):
        graph.add_gap(_gap(f"GAP-{idx}"))
    graph.test_evidence = [
        SimpleNamespace(status="passed", mode="compiled", acceptance_criterion_id="AC-1"),
        SimpleNamespace(status="passed", mode="", acceptance_criterion_id="AC-2"),
        SimpleNamespace(status="failed", mode="vote"),
    ]
    live = {
        "pending_signals": 2,
        "cards": {"open": 3, "critical_open": 1},
        "blocking_cards": [{"id": "CARD-1", "task_id": "TASK-1", "summary": "bad"}],
    }

    blocked = MarkdownReportGenerator.generate(graph, live_review=live)
    assert "**Blocked**" in blocked
    assert "live-review blocker" in blocked
    assert "Proof rigor" in blocked
    assert "Omitted 5 additional blocking gap" in blocked
    assert "CARD-1" in blocked

    class IncompleteGraph:
        requirements = {}
        tasks = {}
        test_evidence = []

        def coverage_summary(self):
            return {
                "blocking_gaps": 0,
                "ac_without_evidence": 2,
                "total_requirements": 0,
                "requirements_without_tasks": 0,
                "total_tasks": 0,
                "tasks_without_requirements": 0,
                "total_ac": 2,
            }

        def blocking_gaps(self):
            return []

    incomplete = MarkdownReportGenerator.generate(IncompleteGraph())
    assert "**Incomplete**" in incomplete

    class PassedGraph(IncompleteGraph):
        def coverage_summary(self):
            data = super().coverage_summary()
            data["ac_without_evidence"] = 0
            data["total_ac"] = 0
            return data

    assert "**Passed**" in MarkdownReportGenerator.generate(PassedGraph())


def test_frontmatter_split_and_build_edge_cases():
    assert split_frontmatter("plain body") == ({}, "plain body")
    assert split_frontmatter("---\n- nope\n---\nbody") == ({}, "body")
    assert split_frontmatter("---\n: bad\n---\nbody")[1] == "body"

    rendered = build_frontmatter_markdown(
        {"name": "Demo", "empty": "", "items": [], "nested": {"a": 1}},
        "Body\n",
    )
    assert rendered.startswith("---\nname: Demo\nnested:")
    assert "empty" not in rendered
    assert rendered.endswith("Body\n")
    assert build_frontmatter_markdown({}, "Body") == "---\n{}\n---\n\nBody\n"
    assert build_frontmatter_markdown({}, "") == "---\n{}\n---\n"


def test_fetch_bundle_directory_archives_git_and_safety(monkeypatch, tmp_path):
    bundle_dir = tmp_path / "bundle"
    bundle_dir.mkdir()
    (bundle_dir / "index.md").write_text("# Index\n", encoding="utf-8")
    fetched = fetch_mod.fetch_bundle(str(bundle_dir))
    assert fetched.directory == bundle_dir.resolve()
    assert fetched.cleanup_dir is None
    fetched.cleanup()

    zip_path = tmp_path / "docs.zip"
    with zipfile.ZipFile(zip_path, "w") as zf:
        zf.writestr("docs/a.md", "# A\n")
    fetched_zip = fetch_mod.fetch_bundle(str(zip_path))
    assert fetched_zip.suggested_name == "docs"
    assert fetched_zip.directory.name == "docs"
    assert (fetched_zip.directory / "a.md").exists()
    cleanup_dir = fetched_zip.cleanup_dir
    fetched_zip.cleanup()
    assert not cleanup_dir.exists()

    unsafe_zip = tmp_path / "unsafe.zip"
    with zipfile.ZipFile(unsafe_zip, "w") as zf:
        zf.writestr("../escape.md", "bad")
    with pytest.raises(fetch_mod.UnsafeArchiveError):
        fetch_mod.fetch_bundle(str(unsafe_zip))

    tar_path = tmp_path / "bundle.tgz"
    nested = tmp_path / "nested"
    nested.mkdir()
    (nested / "doc.md").write_text("# Doc\n", encoding="utf-8")
    with tarfile.open(tar_path, "w:gz") as tf:
        tf.add(nested, arcname="nested")
    fetched_tar = fetch_mod.fetch_bundle(str(tar_path))
    assert fetched_tar.suggested_name == "bundle"
    assert (fetched_tar.directory / "doc.md").exists()
    fetched_tar.cleanup()

    unsafe_tar = tmp_path / "unsafe.tgz"
    with tarfile.open(unsafe_tar, "w:gz") as tf:
        info = tarfile.TarInfo("../escape.md")
        info.size = 0
        tf.addfile(info)
    with pytest.raises(fetch_mod.UnsafeArchiveError):
        fetch_mod.fetch_bundle(str(unsafe_tar))

    assert fetch_mod.is_git_url("git@github.com:org/repo.git") is True
    assert fetch_mod._git_repo_name("https://github.com/org/repo.git") == "repo"
    assert fetch_mod._archive_stem("x.tar.gz") == "x"
    with pytest.raises(FileNotFoundError):
        fetch_mod.fetch_bundle(str(tmp_path / "missing"))

    monkeypatch.setattr(fetch_mod.shutil, "which", lambda name: None)
    with pytest.raises(RuntimeError, match="git is not installed"):
        fetch_mod.fetch_bundle("https://github.com/org/repo.git")

    monkeypatch.setattr(fetch_mod.shutil, "which", lambda name: "/usr/bin/git")
    monkeypatch.setattr(fetch_mod.subprocess, "run", lambda *a, **k: SimpleNamespace(returncode=1, stderr="denied", stdout=""))
    with pytest.raises(RuntimeError, match="denied"):
        fetch_mod.fetch_bundle("https://github.com/org/repo.git")


def test_design_parse_lint_contrast_and_okf_conversion(tmp_path):
    text = """---
name: Demo
colors:
  primary: "#777777"
  unused: "#000000"
typography:
  body: Inter
components:
  Button:
    color: "{colors.primary}"
    backgroundColor: "#777777"
  Card:
    padding: "{spacing.missing}"
---
# Typography
Text rules.
# Overview
Late overview.
"""
    path = tmp_path / "design.md"
    path.write_text(text, encoding="utf-8")
    ds = parse_design_md(path)
    assert ds.name == "Demo"
    assert ds.sections[0][0] == "Typography"
    assert round(contrast_ratio("#000", "#fff"), 2) == 21.0
    assert contrast_ratio("not-hex", "#fff") is None

    findings = lint(ds)
    formatted = "\n".join(f.format() for f in findings)
    assert "broken-token-reference" in formatted
    assert "contrast-ratio" in formatted
    assert "orphaned-token" in formatted
    assert "section-ordering" in formatted

    no_primary = DesignSystem(name="NoPrimary", colors={"secondary": "#fff"})
    assert any(f.rule == "missing-primary-color" for f in lint(no_primary))

    doc = design_system_to_okf_document(ds)
    assert doc.type == "Design System"
    assert "### Colors" in doc.body
    assert doc.rel_path == "design/design.md"


def test_knowledge_source_discovery_selection_render_and_cache(tmp_path):
    clear_knowledge_caches()
    design_dir = tmp_path / ".devcouncil" / "knowledge" / "design"
    okf_dir = tmp_path / ".devcouncil" / "knowledge" / "okf" / "bundle"
    design_dir.mkdir(parents=True)
    okf_dir.mkdir(parents=True)
    (design_dir / "design.md").write_text("---\nname: Design\n---\nDesign body\n", encoding="utf-8")
    (okf_dir / "doc.md").write_text(
        "---\nname: Billing\ndescription: Billing facts\ntags: [invoice]\ntriggers:\n  keywords: [payment]\n---\nBilling body\n",
        encoding="utf-8",
    )
    (okf_dir / "index.md").write_text("# skip\n", encoding="utf-8")

    sources = discover_knowledge_sources(tmp_path)
    assert [s.kind for s in sources] == ["design", "okf"]
    assert discover_knowledge_sources(tmp_path) is sources

    selected = select_knowledge_sources("payment invoice UI", tmp_path)
    assert [s.name for s in selected] == ["Design", "Billing"]
    assert selected[0].matches("anything") is True
    assert selected[1].matches("payment") is True
    assert selected[1].relevance_score("payment invoice") > selected[1].relevance_score("payment")
    assert "Billing body" in selected[1].render()
    assert "Design body" in render_knowledge_preamble(selected, max_chars=20, kind="design")
    assert render_knowledge_preamble([], max_chars=10) == ""

    manual = KnowledgeSource(name="Always", kind="okf", always=True, priority=1, body="Always body")
    assert manual.matches("") is True
    assert manual.relevance_score("") > 1_000_000


def test_okf_html_markdown_safety_links_tables_and_write(tmp_path):
    docs = [
        OKFDocument(
            type="Guide",
            title="Intro",
            description="<desc>",
            resource="javascript:alert(1)",
            tags=["tag"],
            timestamp="2026-01-01",
            body=(
                "# Heading\n\n"
                "Paragraph with `code`, **bold**, *italic*, [safe](other.md#part), "
                "[bad](java\tscript:alert(1)).\n\n"
                "- item\n"
                "1. first\n\n"
                "| A | B |\n|---|---|\n| [x](https://example.com) | y |\n\n"
                "```\n<html>\n```\n\n---\n"
            ),
            rel_path="intro.md",
        ),
        OKFDocument(type="Guide", title="Other", body="Other", rel_path="other.md"),
        OKFDocument(type="Skip", title="NoPath", body="skip", rel_path=""),
    ]
    bundle = OKFBundle(documents=docs)
    assert _is_dangerous_scheme(" java\tscript:alert(1)") is True
    assert _rewrite_link("other.md#part", "intro.md", {"intro.md", "other.md"}) == "other.html#part"
    assert _rewrite_link("javascript:bad", "intro.md", {"intro.md"}) == "#"

    html = render_markdown(docs[0].body, "intro.md", {"intro.md", "other.md"})
    assert "<h1>Heading</h1>" in html
    assert "<code>code</code>" in html
    assert "<strong>bold</strong>" in html
    assert "<em>italic</em>" in html
    assert 'href="other.html#part"' in html
    assert 'href="#"' in html
    assert "<table>" in html
    assert "&lt;html&gt;" in html
    assert "<hr>" in html

    pages = render_bundle_html(bundle)
    assert set(pages) == {"index.html", "intro.html", "other.html"}
    assert 'href="#">javascript:alert(1)</a>' in pages["intro.html"]
    assert "&lt;desc&gt;" in pages["intro.html"]

    written = write_bundle_html(bundle, tmp_path / "site")
    assert sorted(path.name for path in written) == ["index.html", "intro.html", "other.html"]
    assert (tmp_path / "site" / "index.html").exists()

