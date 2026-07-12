"""Dead-symbol verification gate tests."""

from __future__ import annotations

from devcouncil.domain.task import Task
from devcouncil.verification.checks.dead_symbols import detect_dead_symbol_gaps


def _gap_id(task_id: str, kind: str) -> str:
    return f"{task_id}-{kind}-1"


def _diff_for(path: str, body: str) -> str:
    lines = body.splitlines()
    hunk = "\n".join(f"+{ln}" for ln in lines)
    return (
        f"diff --git a/{path} b/{path}\n"
        f"--- a/{path}\n"
        f"+++ b/{path}\n"
        f"@@ -0,0 +1,{len(lines)} @@\n"
        f"{hunk}\n"
    )


def _task(
    *,
    title: str = "t",
    description: str = "d",
    difficulty: str = "hard",
) -> Task:
    return Task(
        id="TASK-1",
        title=title,
        description=description,
        planned_files=[],
        difficulty=difficulty,  # type: ignore[arg-type]
    )


def test_new_public_unreferenced_function_flagged(tmp_path):
    path = "mod.py"
    (tmp_path / path).write_text("def unused_helper():\n    return 1\n", encoding="utf-8")
    (tmp_path / "other.py").write_text("x = 1\n", encoding="utf-8")
    gaps = detect_dead_symbol_gaps(
        task=_task(),
        project_root=tmp_path,
        diff_content=_diff_for(path, "def unused_helper():\n    return 1\n"),
        next_gap_id=_gap_id,
        dead_symbol_blocking=True,
    )
    assert any(g.gap_type == "dead_symbol" and "unused_helper" in g.description for g in gaps)


def test_clears_on_call_reference(tmp_path):
    (tmp_path / "mod.py").write_text("def helper():\n    return 1\n", encoding="utf-8")
    (tmp_path / "caller.py").write_text("from mod import helper\nprint(helper())\n", encoding="utf-8")
    gaps = detect_dead_symbol_gaps(
        task=_task(),
        project_root=tmp_path,
        diff_content=_diff_for("mod.py", "def helper():\n    return 1\n"),
        next_gap_id=_gap_id,
        dead_symbol_blocking=True,
    )
    assert not any(g.gap_type == "dead_symbol" and g.blocking for g in gaps)


def test_clears_on_test_reference(tmp_path):
    (tmp_path / "mod.py").write_text("def helper():\n    return 1\n", encoding="utf-8")
    tests = tmp_path / "tests"
    tests.mkdir()
    (tests / "test_mod.py").write_text("from mod import helper\nassert helper() == 1\n", encoding="utf-8")
    gaps = detect_dead_symbol_gaps(
        task=_task(),
        project_root=tmp_path,
        diff_content=_diff_for("mod.py", "def helper():\n    return 1\n"),
        next_gap_id=_gap_id,
        dead_symbol_blocking=True,
    )
    assert not any(g.gap_type == "dead_symbol" and g.blocking for g in gaps)


def test_decorated_def_exempt(tmp_path):
    body = "@app.route('/')\ndef handle():\n    return 'ok'\n"
    (tmp_path / "mod.py").write_text("app = None\n" + body, encoding="utf-8")
    gaps = detect_dead_symbol_gaps(
        task=_task(),
        project_root=tmp_path,
        diff_content=_diff_for("mod.py", body),
        next_gap_id=_gap_id,
        dead_symbol_blocking=True,
    )
    assert not any("handle" in (g.description or "") for g in gaps if g.gap_type == "dead_symbol")


def test_private_and_dunder_exempt(tmp_path):
    body = "def _private():\n    return 1\n\ndef __enter__(self):\n    return self\n"
    (tmp_path / "mod.py").write_text(body, encoding="utf-8")
    gaps = detect_dead_symbol_gaps(
        task=_task(),
        project_root=tmp_path,
        diff_content=_diff_for("mod.py", body),
        next_gap_id=_gap_id,
        dead_symbol_blocking=True,
    )
    assert gaps == []


def test_intent_text_naming_exempt(tmp_path):
    (tmp_path / "mod.py").write_text("def special_api():\n    return 1\n", encoding="utf-8")
    gaps = detect_dead_symbol_gaps(
        task=_task(description="Add special_api for external callers"),
        project_root=tmp_path,
        diff_content=_diff_for("mod.py", "def special_api():\n    return 1\n"),
        next_gap_id=_gap_id,
        dead_symbol_blocking=True,
    )
    assert not any(g.gap_type == "dead_symbol" and g.blocking for g in gaps)


def test_comment_mention_does_not_clear(tmp_path):
    (tmp_path / "mod.py").write_text("def unused_helper():\n    return 1\n", encoding="utf-8")
    (tmp_path / "other.py").write_text("# unused_helper is planned later\nx = 1\n", encoding="utf-8")
    gaps = detect_dead_symbol_gaps(
        task=_task(),
        project_root=tmp_path,
        diff_content=_diff_for("mod.py", "def unused_helper():\n    return 1\n"),
        next_gap_id=_gap_id,
        dead_symbol_blocking=True,
    )
    assert any(g.gap_type == "dead_symbol" for g in gaps)


def test_no_duplicates_for_unwired_flagged_files(tmp_path):
    (tmp_path / "mod.py").write_text("def unused_helper():\n    return 1\n", encoding="utf-8")
    gaps = detect_dead_symbol_gaps(
        task=_task(),
        project_root=tmp_path,
        diff_content=_diff_for("mod.py", "def unused_helper():\n    return 1\n"),
        next_gap_id=_gap_id,
        dead_symbol_blocking=True,
        unwired_files={"mod.py"},
    )
    assert gaps == []


def test_js_export_function_symmetrical(tmp_path):
    path = "lib.ts"
    body = "export function unusedHelper() { return 1; }\n"
    (tmp_path / path).write_text(body, encoding="utf-8")
    (tmp_path / "other.ts").write_text("export const x = 1;\n", encoding="utf-8")
    gaps = detect_dead_symbol_gaps(
        task=_task(),
        project_root=tmp_path,
        diff_content=_diff_for(path, body),
        next_gap_id=_gap_id,
        dead_symbol_blocking=True,
    )
    assert any(g.gap_type == "dead_symbol" and "unusedHelper" in g.description for g in gaps)


def test_same_file_use_outside_span_clears(tmp_path):
    body = (
        "class ModelsConfig:\n    name: str = 'x'\n\n"
        "class AppConfig:\n    models: ModelsConfig = ModelsConfig()\n"
    )
    (tmp_path / "config.py").write_text(body, encoding="utf-8")
    gaps = detect_dead_symbol_gaps(
        task=_task(),
        project_root=tmp_path,
        diff_content=_diff_for("config.py", body),
        next_gap_id=_gap_id,
        dead_symbol_blocking=True,
    )
    assert not any(
        "ModelsConfig" in (g.description or "") and g.blocking
        for g in gaps if g.gap_type == "dead_symbol"
    )
    # AppConfig unused outside its span → still flagged.
    assert any(
        "AppConfig" in (g.description or "") and g.blocking
        for g in gaps if g.gap_type == "dead_symbol"
    )


def test_recursive_self_ref_still_flagged(tmp_path):
    body = "def unused():\n    return unused()\n"
    (tmp_path / "mod.py").write_text(body, encoding="utf-8")
    gaps = detect_dead_symbol_gaps(
        task=_task(),
        project_root=tmp_path,
        diff_content=_diff_for("mod.py", body),
        next_gap_id=_gap_id,
        dead_symbol_blocking=True,
    )
    assert any(g.gap_type == "dead_symbol" and "unused" in g.description for g in gaps)


def test_all_export_exempt_from_dead_gap(tmp_path):
    body = '__all__ = ["exported"]\n\ndef exported():\n    return 1\n'
    (tmp_path / "mod.py").write_text(body, encoding="utf-8")
    gaps = detect_dead_symbol_gaps(
        task=_task(),
        project_root=tmp_path,
        diff_content=_diff_for("mod.py", body),
        next_gap_id=_gap_id,
        dead_symbol_blocking=True,
    )
    assert not any("exported" in (g.description or "") for g in gaps if g.gap_type == "dead_symbol")


def test_js_export_brace_form_detected(tmp_path):
    path = "lib.ts"
    body = "function unusedHelper() { return 1; }\nexport { unusedHelper };\n"
    (tmp_path / path).write_text(body, encoding="utf-8")
    (tmp_path / "other.ts").write_text("export const x = 1;\n", encoding="utf-8")
    gaps = detect_dead_symbol_gaps(
        task=_task(),
        project_root=tmp_path,
        diff_content=_diff_for(path, body),
        next_gap_id=_gap_id,
        dead_symbol_blocking=True,
    )
    assert any(g.gap_type == "dead_symbol" and "unusedHelper" in g.description for g in gaps)


def test_js_export_default_form_detected(tmp_path):
    path = "widget.ts"
    body = "export default function UnusedWidget() { return null; }\n"
    (tmp_path / path).write_text(body, encoding="utf-8")
    (tmp_path / "other.ts").write_text("export const x = 1;\n", encoding="utf-8")
    gaps = detect_dead_symbol_gaps(
        task=_task(),
        project_root=tmp_path,
        diff_content=_diff_for(path, body),
        next_gap_id=_gap_id,
        dead_symbol_blocking=True,
    )
    assert any(g.gap_type == "dead_symbol" and "UnusedWidget" in g.description for g in gaps)


def test_string_literal_does_not_clear_verify_token_scan(tmp_path):
    """String-literal identifiers in other files must not clear a new dead symbol."""
    path = "mod.py"
    body = "def cost_by_task():\n    return {}\n"
    (tmp_path / path).write_text(body, encoding="utf-8")
    (tmp_path / "status.py").write_text(
        "def report():\n    return {'cost_by_task': 1}\n",
        encoding="utf-8",
    )
    gaps = detect_dead_symbol_gaps(
        task=_task(),
        project_root=tmp_path,
        diff_content=_diff_for(path, body),
        next_gap_id=_gap_id,
        dead_symbol_blocking=True,
        git_files=[path, "status.py"],
    )
    assert any(g.gap_type == "dead_symbol" and "cost_by_task" in g.description for g in gaps)


def test_vendor_tokens_do_not_clear_verify_scan(tmp_path):
    path = "mod.py"
    body = "def lonely_helper():\n    return 1\n"
    (tmp_path / path).write_text(body, encoding="utf-8")
    vendor = tmp_path / "assets" / "vendor"
    vendor.mkdir(parents=True)
    (vendor / "bundle.min.js").write_text(
        "function lonely_helper(){return 1}\n", encoding="utf-8"
    )
    gaps = detect_dead_symbol_gaps(
        task=_task(),
        project_root=tmp_path,
        diff_content=_diff_for(path, body),
        next_gap_id=_gap_id,
        dead_symbol_blocking=True,
        git_files=[path, "assets/vendor/bundle.min.js"],
    )
    assert any(g.gap_type == "dead_symbol" and "lonely_helper" in g.description for g in gaps)
