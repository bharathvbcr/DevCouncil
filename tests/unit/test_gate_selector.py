"""Unit tests for the incremental gate selector (path -> gates/commands mapping)."""

from devcouncil.verification.gate_selector import GateSpec, select_gates


def test_no_changed_files_selects_nothing():
    sel = select_gates([], {"lint": ["ruff check ."], "test": ["pytest"]})
    assert sel.gates == []
    assert sel.commands == []


def test_docs_only_change_skips_python_gates():
    sel = select_gates(
        ["docs/readme.md", "notes.txt"],
        {"lint": ["ruff check ."], "typecheck": ["mypy src"], "test": ["pytest"]},
    )
    assert sel.gates == []
    # Every command is recorded as skipped with a stack reason.
    reasons = {cmd: reason for cmd, reason in sel.skipped}
    assert "ruff check ." in reasons and "python" in reasons["ruff check ."]
    assert "pytest" in reasons


def test_python_change_selects_and_narrows_linters():
    sel = select_gates(
        ["src/pkg/a.py", "src/pkg/b.py"],
        {"lint": ["ruff check ."], "typecheck": ["mypy src"], "test": ["pytest -q"]},
    )
    by_kind = {g.kind: g for g in sel.gates}
    assert by_kind["lint"].command == "ruff check src/pkg/a.py src/pkg/b.py"
    assert by_kind["lint"].narrowed is True
    assert by_kind["typecheck"].command == "mypy src/pkg/a.py src/pkg/b.py"
    # pytest is never narrowed (scoping test targets is unsafe) but still selected.
    assert by_kind["test"].command == "pytest -q"
    assert by_kind["test"].narrowed is False


def test_inputs_are_scoped_to_the_gate_stack():
    sel = select_gates(
        ["src/a.py", "web/app.ts", "docs/x.md"],
        {"lint": ["ruff check .", "eslint ."]},
    )
    by_cmd = {g.command: g for g in sel.gates}
    # The python linter only depends on the python file; the JS linter on the ts file.
    ruff = next(g for k, g in by_cmd.items() if k.startswith("ruff"))
    eslint = next(g for k, g in by_cmd.items() if k.startswith("eslint"))
    assert ruff.inputs == ("src/a.py",)
    assert eslint.inputs == ("web/app.ts",)


def test_explicit_paths_are_not_narrowed():
    # The author already scoped the command to a specific path — respect it.
    sel = select_gates(["src/pkg/a.py"], {"lint": ["ruff check src/pkg/specific.py"]})
    assert sel.gates[0].command == "ruff check src/pkg/specific.py"
    assert sel.gates[0].narrowed is False


def test_shell_operators_disable_narrowing():
    sel = select_gates(["src/a.py"], {"lint": ["ruff check . && echo done"]})
    assert sel.gates[0].command == "ruff check . && echo done"
    assert sel.gates[0].narrowed is False


def test_python_dash_m_wrapper_resolves_stack():
    sel = select_gates(["src/a.py"], {"typecheck": ["python -m mypy src"]})
    assert len(sel.gates) == 1
    assert sel.gates[0].kind == "typecheck"
    assert sel.gates[0].command == "python -m mypy src/a.py"


def test_narrow_can_be_disabled():
    sel = select_gates(["src/a.py"], {"lint": ["ruff check ."]}, narrow=False)
    assert sel.gates[0].command == "ruff check ."
    assert sel.gates[0].narrowed is False


def test_agnostic_command_always_runs_with_all_inputs():
    # An unrecognized tool has no stack — it stays selected and depends on every change.
    sel = select_gates(["src/a.py", "web/app.ts"], {"test": ["make check"]})
    assert len(sel.gates) == 1
    assert sel.gates[0].inputs == ("src/a.py", "web/app.ts")


def test_gate_names_are_unique_and_stable():
    sel = select_gates(["src/a.py"], {"lint": ["ruff check .", "ruff check ."]})
    # Duplicate command collapses to one gate.
    assert len(sel.gates) == 1
    assert isinstance(sel.gates[0], GateSpec)
    assert sel.gates[0].name == "lint:ruff check src/a.py"


def test_config_only_change_selects_stack_gates():
    sel = select_gates(
        ["pyproject.toml"],
        {"lint": ["ruff check ."], "typecheck": ["mypy src"], "test": ["pytest"]},
    )
    by_kind = {g.kind: g for g in sel.gates}
    assert "lint" in by_kind
    assert "typecheck" in by_kind
    assert by_kind["lint"].command == "ruff check ."
    assert by_kind["lint"].inputs == ("pyproject.toml",)
    assert by_kind["lint"].narrowed is False
    assert by_kind["typecheck"].inputs == ("pyproject.toml",)
    # Stack-agnostic commands still run; their inputs are the config change.
    assert by_kind["test"].inputs == ("pyproject.toml",)


def test_narrowed_mypy_includes_import_dependents():
    repo_map = {"dependents": {"src/a.py": ["src/b.py"]}}
    sel = select_gates(
        ["src/a.py"],
        {"typecheck": ["mypy src"]},
        repo_map=repo_map,
    )
    assert len(sel.gates) == 1
    gate = sel.gates[0]
    assert gate.narrowed is True
    assert "src/a.py" in gate.command
    assert "src/b.py" in gate.command
    assert gate.inputs == ("src/a.py", "src/b.py")
