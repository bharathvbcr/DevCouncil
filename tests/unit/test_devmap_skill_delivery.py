"""DevMap skill discovery, delivery, and refusal contracts across agent hosts."""

from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import json
import os
import subprocess
import sys

import pytest
import yaml
from typer.testing import CliRunner

from devcouncil.cli.main import app
from devcouncil.skills.registry import LIBRARY_DIR, Skill, load_skills, scaffold_skills

NAMES = {"devmap", "devmap-debugging", "devmap-exploring", "devmap-impact", "devmap-refactoring"}
ROOT = Path(__file__).resolve().parents[2]


def skill(name="example", body="Original"):
    return Skill(name=name, description="Example workflow", body=body)


@pytest.mark.parametrize("name", sorted(NAMES))
def test_both_distributions_have_valid_identical_guidance(name):
    source = ROOT / "rust-port/crates/devmap-cli/skills" / name / "SKILL.md"
    native_meta, native_body = source.read_text().split("---", 2)[1:]
    library_meta, library_body = (LIBRARY_DIR / f"{name}.md").read_text().split("---", 2)[1:]
    native, library = yaml.safe_load(native_meta), yaml.safe_load(library_meta)
    assert native["name"] == library["name"] == name
    assert native["description"] == library["description"]
    assert native_body.strip() == library_body.strip()
    assert "devmap paths --json" in native_body
    assert "Do not use GitNexus" not in native_body
    assert "will break" not in native_body


def test_real_cli_installs_only_requested_skills_and_checks_without_writing(tmp_path):
    runner = CliRunner()
    args = ["skills", "scaffold", "--project-root", str(tmp_path), "--destination", ".agents/skills"]
    for name in sorted(NAMES):
        args += ["--skill", name]
    missing = runner.invoke(app, args + ["--check"])
    assert missing.exit_code == 1, missing.output
    assert not list(tmp_path.iterdir())
    installed = runner.invoke(app, args)
    assert installed.exit_code == 0, installed.output
    assert {p.parent.name for p in tmp_path.rglob("SKILL.md")} == NAMES
    assert not (tmp_path / ".claude").exists()
    before = {p: (p.read_bytes(), p.stat().st_mtime_ns) for p in tmp_path.rglob("*") if p.is_file()}
    assert runner.invoke(app, args + ["--check"]).exit_code == 0
    assert before == {p: (p.read_bytes(), p.stat().st_mtime_ns) for p in before}


def test_invalid_skill_is_reported_instead_of_silently_missing(tmp_path):
    (tmp_path / "broken.md").write_text('---\nname: broken\ndescription: Examples: "bug"\n---\nBody\n')
    with pytest.raises(ValueError, match="broken.md"):
        load_skills(library_dir=tmp_path, include_okf=False)


@pytest.mark.parametrize("name", ["../escape", "/absolute", "a/b", "a\\b", "..", "", "CON", "con", "lpt1", "a" * 65])
def test_unsafe_names_fail_before_any_write(tmp_path, name):
    with pytest.raises(ValueError):
        scaffold_skills(tmp_path, [skill(), skill(name)])
    assert not list(tmp_path.iterdir())


@pytest.mark.parametrize("destination", ["../escape", "/absolute", "C:\\escape", "a/../../escape", "a\\..\\escape"])
def test_destinations_stay_inside_requested_repository(tmp_path, destination):
    with pytest.raises(ValueError):
        scaffold_skills(tmp_path, [skill()], destinations=[destination])
    assert not list(tmp_path.iterdir())


def test_conflicting_duplicates_fail_before_any_write(tmp_path):
    with pytest.raises(ValueError, match="duplicate"):
        scaffold_skills(tmp_path, [skill(), skill(body="Different")])
    assert not list(tmp_path.iterdir())


def test_managed_upgrade_preserves_user_edits_and_preflights_every_destination(tmp_path):
    scaffold_skills(tmp_path, [skill()])
    scaffold_skills(tmp_path, [skill(body="Upgraded")])
    target = tmp_path / ".agents/skills/example/SKILL.md"
    target.write_text("User-owned edits")
    untouched = (tmp_path / ".claude/skills/example/SKILL.md").read_bytes()
    with pytest.raises(ValueError, match="modified|unmanaged"):
        scaffold_skills(tmp_path, [skill(body="Next")])
    assert target.read_text() == "User-owned edits"
    assert (tmp_path / ".claude/skills/example/SKILL.md").read_bytes() == untouched


def test_concurrent_repeated_installations_converge_without_partial_files(tmp_path):
    chosen = [skill(f"example-{i}") for i in range(20)]
    with ThreadPoolExecutor(max_workers=12) as pool:
        results = list(pool.map(lambda _: scaffold_skills(tmp_path, chosen), range(36)))
    assert sum(len(result) for result in results) == 60
    assert scaffold_skills(tmp_path, chosen) == []
    assert len(list(tmp_path.rglob("SKILL.md"))) == 60
    assert not list(tmp_path.rglob("*.tmp"))


def test_symlink_target_does_not_redirect_installation(tmp_path):
    outside = tmp_path / "outside"
    outside.mkdir()
    root = tmp_path / "repo"
    root.mkdir()
    (root / ".agents").symlink_to(outside, target_is_directory=True)
    with pytest.raises(ValueError, match="symlink"):
        scaffold_skills(root, [skill()])
    assert not list(outside.iterdir())
    assert not (root / ".claude").exists()


def test_oversized_existing_skill_is_refused_without_reading_all_of_it(tmp_path):
    target = tmp_path / ".agents/skills/example/SKILL.md"
    target.parent.mkdir(parents=True)
    with target.open("wb") as file:
        file.truncate(8 * 1024 * 1024)
    with pytest.raises(ValueError, match="limit|large"):
        scaffold_skills(tmp_path, [skill()])
    assert not (tmp_path / ".claude").exists()


def test_cli_rejects_unknown_requested_skill_without_claiming_up_to_date(tmp_path):
    result = CliRunner().invoke(app, ["skills", "scaffold", "--project-root", str(tmp_path), "--skill", "not-a-skill"])
    assert result.exit_code != 0
    assert "Unknown skill" in result.output
    assert not list(tmp_path.iterdir())


@pytest.mark.parametrize("value", ["{broken", "[]", '{"schema":2,"files":{}}', '{"schema":true,"files":{}}', '{"schema":1,"files":{"x":1}}'])
def test_corrupt_receipt_is_not_permission_to_overwrite(tmp_path, value):
    receipt = tmp_path / ".devcouncil-skills.json"
    receipt.write_text(value)
    with pytest.raises(ValueError, match="receipt"):
        scaffold_skills(tmp_path, [skill()])
    assert list(tmp_path.iterdir()) == [receipt]


def test_partial_io_failure_is_visible_recoverable_and_releases_lock(tmp_path, monkeypatch):
    import devcouncil.skills.registry as registry

    real_write = registry.atomic_write_bytes
    calls = 0

    def fail_second(path, contents):
        nonlocal calls
        calls += 1
        if calls == 2:
            raise OSError("injected disk failure")
        real_write(path, contents)

    monkeypatch.setattr(registry, "atomic_write_bytes", fail_second)
    with pytest.raises(OSError, match="injected disk failure"):
        scaffold_skills(tmp_path, [skill()])
    assert not (tmp_path / ".devcouncil-skills.lock").exists()
    assert not (tmp_path / ".devcouncil-skills.json").exists()
    assert len(list(tmp_path.rglob("SKILL.md"))) == 1
    monkeypatch.setattr(registry, "atomic_write_bytes", real_write)
    assert len(scaffold_skills(tmp_path, [skill()])) == 2
    assert scaffold_skills(tmp_path, [skill()]) == []


def test_busy_lock_has_bounded_wait_and_is_not_stolen(tmp_path, monkeypatch):
    import devcouncil.skills.registry as registry

    lock = tmp_path / ".devcouncil-skills.lock"
    lock.mkdir()
    clock = iter([0, 6])
    monkeypatch.setattr(registry.time, "monotonic", lambda: next(clock))
    with pytest.raises(TimeoutError, match="busy"):
        scaffold_skills(tmp_path, [skill()])
    assert list(tmp_path.iterdir()) == [lock]


def test_install_does_not_create_a_marker_that_widens_next_selection(tmp_path):
    from devcouncil.skills.registry import select_skills

    before = {entry.name for entry in select_skills("", tmp_path)}
    scaffold_skills(tmp_path, [skill()])
    assert not (tmp_path / ".devcouncil").exists()
    assert {entry.name for entry in select_skills("", tmp_path)} == before


def test_process_contention_keeps_one_complete_receipt(tmp_path):
    script = """
from pathlib import Path
import sys
from devcouncil.skills.registry import Skill, scaffold_skills
scaffold_skills(Path(sys.argv[1]), [Skill(name='portable', description='Portable', body='Complete')])
"""
    with ThreadPoolExecutor(max_workers=8) as pool:
        results = list(pool.map(lambda _: subprocess.run(
            [sys.executable, "-c", script, str(tmp_path)],
            capture_output=True, text=True, timeout=20,
        ), range(16)))
    assert all(result.returncode == 0 for result in results), [r.stderr for r in results if r.returncode]
    assert len(list(tmp_path.rglob("SKILL.md"))) == 3
    assert len(json.loads((tmp_path / ".devcouncil-skills.json").read_text())["files"]) == 3


@pytest.mark.skipif(not hasattr(os, "mkfifo"), reason="POSIX named pipe probe")
def test_named_pipe_is_refused_without_blocking(tmp_path):
    target = tmp_path / ".agents/skills/example/SKILL.md"
    target.parent.mkdir(parents=True)
    os.mkfifo(target)
    with pytest.raises(ValueError, match="regular file"):
        scaffold_skills(tmp_path, [skill()])


def test_all_three_hosts_discover_same_corpus_under_unicode_space_paths(tmp_path):
    root = tmp_path / "répo with spaces [fixture]"
    root.mkdir()
    chosen = [entry for entry in load_skills(project_root=None) if entry.name in NAMES]
    assert len(scaffold_skills(root, chosen)) == 15
    for name in NAMES:
        bodies = {(root / host / name / "SKILL.md").read_bytes() for host in [".agents/skills", ".claude/skills", ".cursor/skills"]}
        assert len(bodies) == 1
