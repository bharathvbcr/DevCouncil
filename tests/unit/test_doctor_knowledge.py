"""Doctor's ingested-knowledge health check.

Exercises ``check_ingested_knowledge`` directly (it returns structured rows, which makes
assertions robust against rich's table wrapping) and confirms the rows it produces flow
into ``render_doctor_check`` output via Typer's CliRunner.
"""

from pathlib import Path

from typer.testing import CliRunner

from devcouncil.cli.commands.doctor import app, check_ingested_knowledge


def _flatten(rows: list[tuple[str, str, str]]) -> str:
    return " || ".join(f"{c} | {s} | {n}" for c, s, n in rows)


def _write_okf_bundle(okf_dir: Path, broken: bool = False) -> None:
    """Write a small two-document OKF bundle under ``okf_dir/sample``.

    With ``broken=True`` the task links to a document that does not exist, which
    ``validate_bundle`` must flag as a broken intra-bundle link.
    """
    bundle = okf_dir / "sample"
    (bundle / "requirements").mkdir(parents=True)
    (bundle / "tasks").mkdir(parents=True)
    (bundle / "requirements" / "REQ-001.md").write_text(
        "---\ntype: Req\ntitle: R\n---\nA requirement.", encoding="utf-8"
    )
    target = "../requirements/MISSING.md" if broken else "../requirements/REQ-001.md"
    (bundle / "tasks" / "TASK-001.md").write_text(
        f"---\ntype: Task\ntitle: T\n---\nImplements [REQ]({target}).",
        encoding="utf-8",
    )


def _write_design(base: Path) -> None:
    design = base / "design"
    design.mkdir(parents=True)
    (design / "design.md").write_text(
        "---\nname: Brand\ncolors:\n  primary: '#000000'\n"
        "  text: '#000000'\n  bg: '#ffffff'\n"
        "components:\n  button:\n    text: 'colors.text'\n    background: 'colors.bg'\n"
        "---\n# Design\nBody.",
        encoding="utf-8",
    )


def test_reports_valid_bundle_and_design(tmp_path: Path) -> None:
    base = tmp_path / ".devcouncil" / "knowledge"
    _write_okf_bundle(base / "okf")
    _write_design(base)

    rows = check_ingested_knowledge(tmp_path)
    text = _flatten(rows)

    # Bundle + doc counts surfaced, and the valid bundle reports no problems.
    assert "1 bundle(s), 2 document(s)" in text
    assert "no validation problems" in text
    assert any(c == "Ingested OKF" and "OK" in s for c, s, _ in rows)
    # Design system present and lint-clean.
    assert any(c == "Ingested design.md" and "present" in n for c, _, n in rows)


def test_broken_link_is_reported_as_problem(tmp_path: Path) -> None:
    base = tmp_path / ".devcouncil" / "knowledge"
    _write_okf_bundle(base / "okf", broken=True)

    rows = check_ingested_knowledge(tmp_path)
    text = _flatten(rows)

    okf_rows = [(s, n) for c, s, n in rows if c == "Ingested OKF"]
    assert okf_rows, "expected an Ingested OKF row"
    status, notes = okf_rows[0]
    assert "WARN" in status
    assert "validation problem" in notes
    assert "MISSING.md" in text


def test_no_ingested_knowledge_is_neutral_info(tmp_path: Path) -> None:
    rows = check_ingested_knowledge(tmp_path)
    assert len(rows) == 1
    component, status, notes = rows[0]
    assert component == "Ingested knowledge"
    assert "INFO" in status  # neutral, not a failure
    assert "No ingested knowledge" in notes


def test_design_lint_findings_reported(tmp_path: Path) -> None:
    base = tmp_path / ".devcouncil" / "knowledge"
    design = base / "design"
    design.mkdir(parents=True)
    # References an undefined token -> a broken-token-reference lint finding.
    (design / "design.md").write_text(
        "---\nname: Brand\ncolors:\n  primary: '#000000'\n"
        "components:\n  button:\n    color: 'colors.nope'\n"
        "---\n# Design\nBody.",
        encoding="utf-8",
    )

    rows = check_ingested_knowledge(tmp_path)
    design_rows = [(s, n) for c, s, n in rows if c == "Ingested design.md"]
    assert design_rows
    status, notes = design_rows[0]
    assert "WARN" in status
    assert "lint finding(s)" in notes


def test_render_doctor_check_includes_knowledge_rows(tmp_path: Path) -> None:
    base = tmp_path / ".devcouncil" / "knowledge"
    _write_okf_bundle(base / "okf")

    runner = CliRunner()
    result = runner.invoke(app, ["--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "Ingested OKF" in result.stdout
