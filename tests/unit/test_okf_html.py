"""The OKF static HTML visualizer renders a browsable, link-rewritten, XSS-safe site."""

from devcouncil.knowledge.okf import OKFBundle, OKFDocument, read_bundle, write_bundle
from devcouncil.reporting.okf_html import (
    render_bundle_html,
    render_markdown,
    write_bundle_html,
)


def _bundle() -> OKFBundle:
    return OKFBundle(documents=[
        OKFDocument(
            type="Table", title="Orders",
            description="One row per order.", resource="https://acme.example/orders",
            tags=["sales", "revenue"], rel_path="tables/orders.md",
            body="# Orders\n\nJoined with [customers](customers.md) on id.\n\n"
                 "See the [site](https://example.com/x).\n\n```\ncode <here> & there\n```\n",
        ),
        OKFDocument(type="Table", title="Customers", rel_path="tables/customers.md",
                    body="# Customers\n"),
        OKFDocument(
            type="Note", title="<script>title</script>", rel_path="evil.md",
            body="<script>alert(1)</script>\n\n**bold** and a [bad](javascript:alert(1)) link "
                 "and <b>x</b> tag.\n",
        ),
    ])


def test_index_and_pages_generated():
    pages = render_bundle_html(_bundle())
    assert "index.html" in pages
    assert "tables/orders.html" in pages
    assert "tables/customers.html" in pages
    assert "evil.html" in pages
    # Index lists documents grouped by type.
    assert "Orders" in pages["index.html"]
    assert "Customers" in pages["index.html"]


def test_intrabundle_link_rewritten_external_preserved():
    page = render_bundle_html(_bundle())["tables/orders.html"]
    # Sibling .md link -> sibling .html page (same directory).
    assert 'href="customers.html"' in page
    # External markdown link and the resource field survive as real hrefs.
    assert "https://example.com/x" in page
    assert "https://acme.example/orders" in page
    # Code block content is escaped, not interpreted.
    assert "code &lt;here&gt; &amp; there" in page


def test_hostile_content_is_escaped_and_neutralized():
    page = render_bundle_html(_bundle())["evil.html"]
    # Script payload from the body never appears as live markup.
    assert "<script>alert(1)</script>" not in page
    assert "&lt;script&gt;alert(1)&lt;/script&gt;" in page
    # Escaped title, escaped stray tag.
    assert "&lt;b&gt;x&lt;/b&gt;" in page
    # javascript: href is neutralized to '#', the scheme never reaches the output.
    assert "javascript:" not in page
    assert 'href="#"' in page
    # Real markdown still renders.
    assert "<strong>bold</strong>" in page


def test_write_bundle_html_writes_files(tmp_path):
    out = tmp_path / "site"
    written = write_bundle_html(_bundle(), out)
    assert (out / "index.html").exists()
    assert (out / "tables" / "orders.html").exists()
    assert {p.name for p in written} >= {"index.html", "orders.html", "customers.html", "evil.html"}


def test_roundtrip_from_read_bundle(tmp_path):
    # write a bundle to disk, read it back, render — exercises the on-disk path.
    src = tmp_path / "bundle"
    write_bundle(_bundle(), src)
    parsed = read_bundle(src)
    pages = render_bundle_html(parsed)
    assert "index.html" in pages
    assert 'href="customers.html"' in pages["tables/orders.html"]


def test_inline_digits_with_code_and_link_render_without_corruption():
    # Regression: placeholder stashing must not confuse stash indices with bare digits in
    # the body text. A digit >= stash length used to IndexError, and a digit equal to a real
    # placeholder index used to be replaced with the wrong stash entry.
    body = "Item 5 with `code` and [link](http://x) v2.3\n"
    html_out = render_markdown(body, "n.md", set())
    # No crash, and every literal digit from the text survives unchanged.
    assert "Item 5 with" in html_out
    assert "v2.3" in html_out
    # The stashed code and link fragments are restored, not the body digits.
    assert "<code>code</code>" in html_out
    assert 'href="http://x"' in html_out
    assert ">link</a>" in html_out
    # The placeholder token itself never leaks into the output.
    assert "__PH_" not in html_out


def test_many_inline_fragments_with_high_digits():
    # 12 code spans push stash length past single digits; bare numbers like "7" in the text
    # must remain text, not index into the stash.
    spans = " ".join(f"`c{i}`" for i in range(12))
    body = f"Numbers 7 8 9 10 11 then {spans} end.\n"
    html_out = render_markdown(body, "n.md", set())
    assert "Numbers 7 8 9 10 11 then" in html_out
    assert "<code>c11</code>" in html_out
    assert "__PH_" not in html_out


def test_obfuscated_dangerous_schemes_are_neutralized():
    # Whitespace/control chars inside a scheme (which browsers strip before resolving it)
    # must not slip past the scheme guard.
    bundle = OKFBundle(documents=[OKFDocument(
        type="Note", title="x", rel_path="n.md",
        body="[tab](java\tscript:alert(1)) [space]( javascript:alert(2)) "
             "[vb](VBScript:msgbox(3)) link.\n",
    )])
    page = render_bundle_html(bundle)["n.html"]
    # Every dangerous target collapses to href="#"; no payload reaches the output.
    assert "alert" not in page
    assert "msgbox" not in page
    assert page.count('href="#"') >= 3
