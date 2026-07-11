"""Unit tests for the content-hash keyed gate-result cache."""

from devcouncil.verification.gate_cache import GateResultCache
from devcouncil.verification.gate_selector import GateSpec


def _gate(name="lint:ruff", command="ruff check a.py", inputs=("a.py",)):
    return GateSpec(name=name, kind="lint", command=command, inputs=tuple(inputs))


def test_unknown_gate_is_not_green(tmp_path):
    cache = GateResultCache(tmp_path)
    assert cache.is_green(_gate()) is False


def test_recorded_pass_is_green_until_input_changes(tmp_path):
    (tmp_path / "a.py").write_text("x = 1\n", encoding="utf-8")
    cache = GateResultCache(tmp_path)
    gate = _gate()
    cache.record(gate, passed=True, summary="ok")
    assert cache.is_green(gate) is True

    # Editing the input invalidates the green result.
    (tmp_path / "a.py").write_text("x = 2\n", encoding="utf-8")
    assert cache.is_green(gate) is False


def test_recorded_failure_is_never_green(tmp_path):
    (tmp_path / "a.py").write_text("x = 1\n", encoding="utf-8")
    cache = GateResultCache(tmp_path)
    gate = _gate()
    cache.record(gate, passed=False, summary="boom")
    assert cache.is_green(gate) is False


def test_changing_command_invalidates_cache(tmp_path):
    (tmp_path / "a.py").write_text("x = 1\n", encoding="utf-8")
    cache = GateResultCache(tmp_path)
    cache.record(_gate(command="ruff check a.py"), passed=True)
    # Same name, different command string -> different hash -> not green.
    assert cache.is_green(_gate(command="ruff check --fix a.py")) is False


def test_missing_input_file_has_stable_absent_hash(tmp_path):
    cache = GateResultCache(tmp_path)
    gate = _gate(inputs=("does_not_exist.py",))
    cache.record(gate, passed=True)
    assert cache.is_green(gate) is True
    # Creating the previously-absent file changes the hash and invalidates.
    (tmp_path / "does_not_exist.py").write_text("y = 1\n", encoding="utf-8")
    assert cache.is_green(gate) is False


def test_persists_and_reloads_across_instances(tmp_path):
    (tmp_path / "a.py").write_text("x = 1\n", encoding="utf-8")
    gate = _gate()
    cache = GateResultCache(tmp_path)
    cache.record(gate, passed=True, summary="cached-ok")
    cache.save()
    assert cache.path.is_file()

    fresh = GateResultCache(tmp_path)
    assert fresh.is_green(gate) is True
    assert fresh.cached_summary(gate) == "cached-ok"


def test_corrupt_cache_degrades_to_empty(tmp_path):
    cache_path = tmp_path / ".devcouncil" / "cache" / "gate_results.json"
    cache_path.parent.mkdir(parents=True)
    cache_path.write_text("{not json", encoding="utf-8")
    cache = GateResultCache(tmp_path)
    # No crash; nothing cached.
    assert cache.is_green(_gate()) is False


def test_input_hash_independent_of_input_order(tmp_path):
    (tmp_path / "a.py").write_text("a\n", encoding="utf-8")
    (tmp_path / "b.py").write_text("b\n", encoding="utf-8")
    cache = GateResultCache(tmp_path)
    h1 = cache.input_hash(_gate(inputs=("a.py", "b.py")))
    h2 = cache.input_hash(_gate(inputs=("b.py", "a.py")))
    assert h1 == h2


def test_config_change_invalidates_green_gate(tmp_path):
    (tmp_path / "a.py").write_text("x = 1\n", encoding="utf-8")
    (tmp_path / "pyproject.toml").write_text("[project]\nname = 'x'\n", encoding="utf-8")
    cache = GateResultCache(tmp_path)
    gate = _gate()
    cache.record(gate, passed=True, summary="ok")
    assert cache.is_green(gate) is True

    (tmp_path / "pyproject.toml").write_text("[project]\nname = 'y'\n", encoding="utf-8")
    assert cache.is_green(gate) is False
