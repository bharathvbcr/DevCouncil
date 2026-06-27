"""Knowledge-source discovery, selection, and preamble budgeting."""

from devcouncil.knowledge.sources import (
    discover_knowledge_sources,
    render_knowledge_preamble,
    select_knowledge_sources,
)


def _seed(root):
    design = root / ".devcouncil" / "knowledge" / "design"
    okf = root / ".devcouncil" / "knowledge" / "okf"
    design.mkdir(parents=True)
    okf.mkdir(parents=True)
    (design / "design.md").write_text(
        "---\nname: Acme\n---\nUse Acme tokens everywhere.", encoding="utf-8"
    )
    (okf / "billing.md").write_text(
        "---\ntype: Table\ntitle: Invoices\ntags: [billing, revenue]\n---\n"
        "Invoices are immutable once issued.",
        encoding="utf-8",
    )
    (okf / "auth.md").write_text(
        "---\ntype: Doc\ntitle: Auth\ntags: [authentication]\n---\nUse OAuth2.",
        encoding="utf-8",
    )


def test_discover_finds_design_and_okf(tmp_path):
    _seed(tmp_path)
    sources = discover_knowledge_sources(tmp_path)
    kinds = sorted(s.kind for s in sources)
    assert kinds == ["design", "okf", "okf"]
    assert all(s.always for s in sources if s.kind == "design")


def test_design_always_selected_okf_by_keyword(tmp_path):
    _seed(tmp_path)
    selected = select_knowledge_sources(goal="Add a billing summary", project_root=tmp_path)
    names = {s.name for s in selected}
    kinds = {s.kind for s in selected}
    # design always (its name comes from frontmatter: "Acme"), billing OKF matched on its
    # tag, auth OKF not matched.
    assert "design" in kinds
    assert "billing" in names
    assert "auth" not in names
    # design (always-on) sorts first.
    assert selected[0].kind == "design"


def test_select_returns_empty_without_project_root():
    assert select_knowledge_sources(goal="x", project_root=None) == []


def test_preamble_filters_by_kind_and_respects_budget(tmp_path):
    _seed(tmp_path)
    selected = select_knowledge_sources(goal="billing and authentication", project_root=tmp_path)
    design_only = render_knowledge_preamble(selected, kind="design")
    assert "Acme tokens" in design_only
    assert "Invoices" not in design_only

    okf_only = render_knowledge_preamble(selected, kind="okf", max_chars=10)
    # Budget of 10 keeps only the first OKF block (the first is always included).
    assert okf_only.count("---") == 0  # no joiner → at most one block survived
