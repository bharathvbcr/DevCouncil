from devcouncil.cli.commands.doctor import _subsystem_maturity_rows, render_doctor_check
from io import StringIO
from rich.console import Console


def test_subsystem_maturity_includes_preview_features():
    rows = _subsystem_maturity_rows()
    tiers = {area: tier for area, tier, _ in rows}
    assert tiers["CLI & Storage"] == "stable"
    assert tiers["Coding CLI Executors"] == "preview"
    # Promoted out of Experimental once it joined the lease-gated write path + shared
    # verify/next-actions loop (parity with MCP), backed by test_native_closed_loop.py.
    assert tiers["Native Executor"] == "preview"


def test_doctor_renders_maturity_section(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    tmp_path.mkdir(parents=True, exist_ok=True)
    console = Console(file=StringIO(), width=120)
    from devcouncil.cli.commands import doctor as doctor_mod

    original = doctor_mod.console
    doctor_mod.console = console
    try:
        render_doctor_check(tmp_path)
    finally:
        doctor_mod.console = original
    output = console.file.getvalue()
    assert "Maturity:" in output or "Subsystem Maturity" in output
    assert "Preview" in output
    assert "Semantic layer" in output
