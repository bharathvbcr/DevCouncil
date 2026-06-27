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


def test_preamble_budget_accounts_for_separators():
    from devcouncil.knowledge.sources import KnowledgeSource
    from devcouncil.skills.registry import SkillTriggers

    def _src(body):
        return KnowledgeSource(
            name="s", kind="okf", description="s", always=False,
            triggers=SkillTriggers(), body=body, priority=50, source_path=None,
        )

    # Each block renders to "## s\n\n" + 100 chars = 107; with max=220 only ONE block fits
    # once the 7-char joiner is counted (107 + 7 + 107 = 221 > 220). The old budget ignored
    # the separator and would emit a 221-char preamble that exceeds the limit.
    out = render_knowledge_preamble([_src("x" * 100) for _ in range(4)], max_chars=220)
    assert len(out) <= 220


def test_knowledge_source_cache_reuses_and_invalidates(tmp_path):
    import os

    from devcouncil.knowledge.sources import clear_knowledge_caches

    clear_knowledge_caches()
    okf = tmp_path / ".devcouncil" / "knowledge" / "okf"
    okf.mkdir(parents=True)
    doc = okf / "doc.md"
    doc.write_text("---\nname: d\ntype: T\ntags: [x]\n---\nbody v1", encoding="utf-8")

    first = discover_knowledge_sources(tmp_path)[0]
    again = discover_knowledge_sources(tmp_path)[0]
    assert first is again  # same parse reused from cache

    # A newer mtime invalidates the cache entry so the edited content is re-parsed.
    doc.write_text("---\nname: d\ntype: T\ntags: [x]\n---\nbody v2", encoding="utf-8")
    st = doc.stat()
    os.utime(doc, ns=(st.st_atime_ns, st.st_mtime_ns + 1_000_000_000))
    updated = discover_knowledge_sources(tmp_path)[0]
    assert updated.body == "body v2"
