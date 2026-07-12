from unittest.mock import patch

import subprocess
from types import SimpleNamespace

from sqlmodel import select

from devcouncil.domain.gap import Gap
from devcouncil.domain.task import Task
from devcouncil.storage.db import Database
from devcouncil.storage.models import VerificationRunModel
from devcouncil.verification.sandbox import (
    DockerSandbox,
    LocalSandbox,
    NixSandbox,
    _command_timeout,
    _run_sandboxed,
    get_sandbox,
)


def test_docker_unavailable_is_unsupported(tmp_path):
    (tmp_path / ".devcouncil").mkdir()
    (tmp_path / ".devcouncil" / "config.yaml").write_text("project:\n  name: test\n", encoding="utf-8")
    task = Task(id="T", title="t", description="d")
    with patch("devcouncil.verification.sandbox.shutil_which", return_value=None):
        result = get_sandbox("docker", tmp_path).run(task, ["pytest"], [])
    assert result.status == "unsupported"


def test_nix_unavailable_without_flake(tmp_path):
    (tmp_path / ".devcouncil").mkdir()
    (tmp_path / ".devcouncil" / "config.yaml").write_text("project:\n  name: test\n", encoding="utf-8")
    task = Task(id="T", title="t", description="d")
    result = get_sandbox("nix", tmp_path).run(task, ["pytest"], [])
    assert result.status == "unsupported"


def test_local_environment_metadata(tmp_path, monkeypatch):
    (tmp_path / ".devcouncil").mkdir()
    (tmp_path / ".devcouncil" / "config.yaml").write_text("project:\n  name: test\n", encoding="utf-8")
    (tmp_path / "uv.lock").write_text("lock\n", encoding="utf-8")
    task = Task(id="T", title="t", description="d")
    monkeypatch.setattr(
        "devcouncil.verification.sandbox.Verifier.verify_task",
        lambda self, t, r: ([], []),
    )
    import asyncio

    async def _noop(*args, **kwargs):
        return [], []

    monkeypatch.setattr(asyncio, "run", lambda coro: ([], []))
    result = LocalSandbox(tmp_path).run(task, [], [])
    assert "python" in result.environment
    assert "uv_lock_hash" in result.environment


def test_docker_command_uses_configured_image(tmp_path):
    from devcouncil.app.config import DevCouncilConfig

    config = DevCouncilConfig()
    config.verification.sandbox.docker_image = "custom:image"
    with patch("devcouncil.verification.sandbox.shutil_which", return_value="/usr/bin/docker"):
        with patch("subprocess.run") as run:
            run.return_value.returncode = 0
            result = DockerSandbox(tmp_path, config).run(
                Task(id="T", title="t", description="d"),
                ["pytest"],
                [],
            )
    assert result.sandbox == "docker"
    docker_args = run.call_args[0][0]
    assert "custom:image" in docker_args


def test_docker_run_is_persisted_when_state_db_exists(tmp_path):
    from devcouncil.app.config import DevCouncilConfig

    dev_dir = tmp_path / ".devcouncil"
    dev_dir.mkdir()
    Database(dev_dir / "state.sqlite").create_db_and_tables()
    config = DevCouncilConfig()
    config.verification.sandbox.docker_image = "custom:image"

    with patch("devcouncil.verification.sandbox.shutil_which", return_value="/usr/bin/docker"):
        with patch("subprocess.run") as run:
            run.return_value.returncode = 0
            result = DockerSandbox(tmp_path, config).run(
                Task(id="TASK-001", title="t", description="d"),
                ["pytest"],
                [],
            )

    assert result.status == "passed"
    db = Database(dev_dir / "state.sqlite")
    with db.get_session() as session:
        rows = session.exec(select(VerificationRunModel)).all()
        persisted = [(row.task_id, row.sandbox) for row in rows]
    assert len(rows) == 1
    assert persisted == [("TASK-001", "docker")]


def test_docker_setup_command_failure(tmp_path):
    from devcouncil.app.config import DevCouncilConfig

    config = DevCouncilConfig()
    config.verification.sandbox.docker_image = "img"
    config.verification.sandbox.docker_setup_commands = ["pip install -r requirements.txt"]
    with patch("devcouncil.verification.sandbox.shutil_which", return_value="/usr/bin/docker"):
        with patch("subprocess.run") as run:
            run.return_value.returncode = 1
            result = DockerSandbox(tmp_path, config).run(
                Task(id="T", title="t", description="d"),
                ["pytest"],
                [],
            )
    assert result.status == "failed"


def test_docker_verification_command_failure(tmp_path):
    from devcouncil.app.config import DevCouncilConfig

    config = DevCouncilConfig()
    config.verification.sandbox.docker_image = "img"
    with patch("devcouncil.verification.sandbox.shutil_which", return_value="/usr/bin/docker"):
        with patch("subprocess.run") as run:
            run.return_value = SimpleNamespace(returncode=2)
            result = DockerSandbox(tmp_path, config).run(
                Task(id="T", title="t", description="d"),
                ["pytest -q"],
                [],
            )
    assert result.status == "failed"


def test_nix_sandbox_passes_with_flake(tmp_path):
    from devcouncil.app.config import DevCouncilConfig

    (tmp_path / "flake.nix").write_text("{}", encoding="utf-8")
    config = DevCouncilConfig()
    with patch("devcouncil.verification.sandbox.shutil_which", return_value="/usr/bin/nix"):
        with patch("subprocess.run") as run:
            run.return_value.returncode = 0
            result = NixSandbox(tmp_path, config).run(
                Task(id="T", title="t", description="d"),
                ["echo ok"],
                [],
            )
    assert result.status == "passed"
    assert "attr" in result.environment


def test_nix_sandbox_command_failure(tmp_path):
    from devcouncil.app.config import DevCouncilConfig

    (tmp_path / "flake.nix").write_text("{}", encoding="utf-8")
    config = DevCouncilConfig()
    with patch("devcouncil.verification.sandbox.shutil_which", return_value="/usr/bin/nix"):
        with patch("subprocess.run") as run:
            run.return_value.returncode = 3
            result = NixSandbox(tmp_path, config).run(
                Task(id="T", title="t", description="d"),
                ["false"],
                [],
            )
    assert result.status == "failed"


def test_run_sandboxed_timeout_returns_124():
    with patch("subprocess.run", side_effect=subprocess.TimeoutExpired(cmd="sleep", timeout=1)):
        proc = _run_sandboxed(["sleep", "999"], timeout=1.0)
    assert proc.returncode == 124


def test_command_timeout_fallback_on_bad_config():
    class _Bad:
        execution = SimpleNamespace(command_timeout="not-a-number")

    assert _command_timeout(_Bad()) == 300.0


def test_local_sandbox_failed_when_blocking_gap(tmp_path, monkeypatch):
    (tmp_path / ".devcouncil").mkdir()
    (tmp_path / ".devcouncil" / "config.yaml").write_text("project:\n  name: test\n", encoding="utf-8")
    task = Task(id="T", title="t", description="d")
    blocking = Gap(
        id="G1",
        severity="high",
        gap_type="test_failed",
        task_id="T",
        description="fail",
        recommended_fix="fix",
        blocking=True,
    )

    async def _verify(self, task, requirements):
        return [blocking], []

    monkeypatch.setattr(
        "devcouncil.verification.sandbox.Verifier.verify_task",
        _verify,
    )
    result = LocalSandbox(tmp_path).run(task, ["pytest"], [])
    assert result.status == "failed"


def test_get_sandbox_defaults_to_local(tmp_path):
    (tmp_path / ".devcouncil").mkdir()
    (tmp_path / ".devcouncil" / "config.yaml").write_text("project:\n  name: test\n", encoding="utf-8")
    sandbox = get_sandbox("local", tmp_path)
    assert isinstance(sandbox, LocalSandbox)


def test_environment_metadata_uv_lookup_failure(tmp_path, monkeypatch):
    (tmp_path / ".devcouncil").mkdir()
    (tmp_path / ".devcouncil" / "config.yaml").write_text("project:\n  name: test\n", encoding="utf-8")
    task = Task(id="T", title="t", description="d")
    monkeypatch.setattr(
        "devcouncil.verification.sandbox.Verifier.verify_task",
        lambda self, t, r: ([], []),
    )
    import asyncio

    monkeypatch.setattr(asyncio, "run", lambda coro: ([], []))
    monkeypatch.setattr(
        "subprocess.check_output",
        lambda *a, **k: (_ for _ in ()).throw(FileNotFoundError()),
    )
    result = LocalSandbox(tmp_path).run(task, [], [])
    assert "python" in result.environment
    assert "uv" not in result.environment
