from unittest.mock import patch

from sqlmodel import select

from devcouncil.domain.task import Task
from devcouncil.storage.db import Database
from devcouncil.storage.models import VerificationRunModel
from devcouncil.verification.sandbox import DockerSandbox, LocalSandbox, get_sandbox


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
