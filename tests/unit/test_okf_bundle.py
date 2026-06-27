"""OKF bundle I/O: round-trip, link resolution, and validation."""

from devcouncil.knowledge.okf import (
    OKFBundle,
    OKFDocument,
    read_bundle,
    validate_bundle,
    write_bundle,
)


def _write_bundle(root):
    (root / "requirements").mkdir()
    (root / "tasks").mkdir()
    (root / "requirements" / "REQ-001.md").write_text(
        "---\ntype: Req\ntitle: R\n---\nA requirement.", encoding="utf-8"
    )
    (root / "tasks" / "TASK-001.md").write_text(
        "---\ntype: Task\ntitle: T\n---\n"
        "Implements [REQ-001](../requirements/REQ-001.md). "
        "See [external](https://example.com) and [missing](../x/Y.md).",
        encoding="utf-8",
    )


def test_read_resolves_intra_bundle_links_and_ignores_urls(tmp_path):
    _write_bundle(tmp_path)
    bundle = read_bundle(tmp_path)
    task = bundle.by_path()["tasks/TASK-001.md"]
    # The relative link resolves to a bundle-relative path; the http URL is excluded.
    assert "requirements/REQ-001.md" in task.links
    assert "x/Y.md" in task.links
    assert all("example.com" not in link for link in task.links)


def test_validate_flags_broken_links(tmp_path):
    _write_bundle(tmp_path)
    bundle = read_bundle(tmp_path)
    problems = validate_bundle(bundle)
    assert any("x/Y.md" in p for p in problems)
    assert not any("REQ-001" in p for p in problems)  # that link resolves


def test_validate_flags_missing_type(tmp_path):
    (tmp_path / "doc.md").write_text("---\ntitle: No Type\n---\nbody", encoding="utf-8")
    bundle = read_bundle(tmp_path)
    problems = validate_bundle(bundle)
    assert any("missing required 'type'" in p for p in problems)


def test_document_markdown_round_trip(tmp_path):
    doc = OKFDocument(
        type="Note", title="N", description="d", tags=["a"], body="hello", rel_path="n.md"
    )
    write_bundle(OKFBundle(documents=[doc]), tmp_path)
    reparsed = read_bundle(tmp_path).by_path()["n.md"]
    assert reparsed.type == "Note"
    assert reparsed.title == "N"
    assert reparsed.tags == ["a"]
    assert reparsed.body == "hello"


def test_links_escaping_bundle_root_are_dropped(tmp_path):
    (tmp_path / "a.md").write_text(
        "---\ntype: T\n---\nlink [up](../../etc/passwd)", encoding="utf-8"
    )
    bundle = read_bundle(tmp_path)
    assert bundle.by_path()["a.md"].links == []
