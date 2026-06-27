"""Design-system conformance scanning: flag hardcoded literals bypassing tokens."""

from typer.testing import CliRunner

from devcouncil.cli.main import app
from devcouncil.knowledge.design import parse_design_md
from devcouncil.knowledge.design_conformance import scan_files, scan_text

runner = CliRunner()

# A tiny design system: one color token, a typography fontSize, and a spacing scale.
DESIGN = """---
name: Acme
colors:
  primary: "#3366ff"
  surface: "#ffffff"
typography:
  body:
    fontFamily: Inter
    fontSize: 16px
spacing:
  sm: 4px
  md: 8px
  lg: 16px
---
# Overview
Acme.
"""


def _ds():
    return parse_design_md(DESIGN)


def test_flags_stray_hex_but_not_token_value():
    ds = _ds()
    bad = scan_text("a { color: #ff0000; }", ds, filename="a.css")
    assert len(bad) == 1
    assert bad[0].kind == "color"
    assert bad[0].line == 1
    assert "#ff0000" in bad[0].message
    # A literal equal to a defined token value is allowed (case-insensitive).
    assert scan_text("a { color: #3366ff; }", ds) == []
    assert scan_text("a { color: #3366FF; }", ds) == []


def test_url_does_not_swallow_a_later_stray_color():
    # A `//` inside a url() or string is not a line comment, so a stray color after it on
    # the same line is still caught (regression: the naive `//`-strips dropped it).
    ds = _ds()
    quoted = scan_text("a { background: url('http://x/y'); color: #ff0000; }", ds, filename="a.css")
    assert any(v.kind == "color" and "#ff0000" in v.message for v in quoted)
    bare = scan_text("a { background: url(http://x/y); color: #ff0000; }", ds, filename="a.css")
    assert any(v.kind == "color" and "#ff0000" in v.message for v in bare)
    # A genuine JS line comment is still stripped.
    assert scan_text("const c = 1; // color: #ff0000 in a comment", ds, filename="a.ts") == []


def test_flags_px_outside_scale_but_not_in_scale():
    ds = _ds()
    assert scan_text("p { font-size: 17px; }", ds)[0].kind == "font-size"
    assert scan_text("p { font-size: 16px; }", ds) == []  # in typography scale

    assert scan_text("div { margin: 5px; }", ds)[0].kind == "spacing"
    assert scan_text("div { padding: 8px; }", ds) == []  # in spacing scale
    assert scan_text("div { margin: 0px; }", ds) == []  # zero is always allowed


def test_low_false_positives_on_non_styling_text():
    ds = _ds()
    # Hex-looking and px-looking tokens in non-style contexts must not be flagged.
    text = (
        "# heading #ff0000 in prose\n"
        "const sha = 'a1b2c3';\n"
        "// comment color: #ff0000;\n"
        "/* color: #00ff00; */\n"
        "the build took 5px... not really\n"
        "url('https://example.com/path');\n"
    )
    assert scan_text(text, ds) == []


def test_js_style_object_is_scanned():
    ds = _ds()
    vs = scan_text("const s = { backgroundColor: '#abcdef', fontSize: '20px' };", ds)
    kinds = {v.kind for v in vs}
    assert kinds == {"color", "font-size"}


def test_scan_files_only_scans_style_extensions(tmp_path):
    ds = _ds()
    (tmp_path / "styles.css").write_text("a { color: #ff0000; }", encoding="utf-8")
    (tmp_path / "notes.md").write_text("a { color: #ff0000; }", encoding="utf-8")
    (tmp_path / "data.json").write_text('{"color": "#ff0000"}', encoding="utf-8")

    paths = [tmp_path / "styles.css", tmp_path / "notes.md", tmp_path / "data.json"]
    vs = scan_files(paths, ds)
    assert len(vs) == 1
    assert vs[0].file.endswith("styles.css")


def test_scan_files_skips_unreadable_files(tmp_path):
    ds = _ds()
    missing = tmp_path / "ghost.css"  # never created
    assert scan_files([missing], ds) == []


def test_cli_check_exits_nonzero_on_violation(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    (tmp_path / "design.md").write_text(DESIGN, encoding="utf-8")
    (tmp_path / "app.css").write_text("a { color: #ff0000; }\n", encoding="utf-8")

    result = runner.invoke(app, ["design", "check"])
    assert result.exit_code == 1, result.output
    assert "#ff0000" in result.output
    assert "app.css" in result.output


def test_cli_check_clean_exits_zero(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    (tmp_path / "design.md").write_text(DESIGN, encoding="utf-8")
    (tmp_path / "app.css").write_text("a { color: #3366ff; padding: 8px; }\n", encoding="utf-8")

    result = runner.invoke(app, ["design", "check"])
    assert result.exit_code == 0, result.output
    assert "No design-token violations" in result.output


def test_cli_check_no_design_found(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    result = runner.invoke(app, ["design", "check"])
    assert result.exit_code == 1
    assert "No design.md found" in result.output


def test_cli_check_explicit_files_and_design(tmp_path):
    design = tmp_path / "ds.md"
    design.write_text(DESIGN, encoding="utf-8")
    style = tmp_path / "x.scss"
    style.write_text(".x { font-size: 99px; }\n", encoding="utf-8")

    result = runner.invoke(
        app, ["design", "check", str(style), "--design", str(design)])
    assert result.exit_code == 1
    assert "99px" in result.output


def test_hex_inside_string_literal_is_not_flagged():
    ds = _ds()
    # "color: #..." sitting inside a log/error string is a message, not a declaration.
    assert scan_text('console.log("color: #ff0000 is bad");', ds, filename="a.ts") == []
    assert scan_text("throw new Error('set color: #abcdef');", ds, filename="a.ts") == []


def test_quoted_value_and_css_in_js_still_flagged():
    ds = _ds()
    # A real JS style-object value is quoted, but the PROPERTY is outside the string.
    obj = scan_text("const s = { color: '#ff0000' };", ds, filename="a.ts")
    assert any(v.kind == "color" for v in obj)
    # styled-components backtick CSS-in-JS stays scannable (backticks aren't strings here).
    styled = scan_text("const B = styled.button`color: #ff0000;`;", ds, filename="a.ts")
    assert any(v.kind == "color" for v in styled)
