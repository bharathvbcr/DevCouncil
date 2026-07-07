import subprocess
from unittest.mock import patch

from devcouncil.domain.task import Task
from devcouncil.execution.patch import PatchEngine, _GIT_APPLY_TIMEOUT_SECONDS
from devcouncil.execution.shell_session import CommandLoopBackend, GuardedShellSession

_HELLO = "echo timeout-test"


def test_patch_engine_passes_timeout_to_subprocess(tmp_path):
    engine = PatchEngine(tmp_path)
    (tmp_path / ".devcouncil").mkdir(parents=True, exist_ok=True)
    diff_content = "\n".join([
        "diff --git a/foo.txt b/foo.txt",
        "new file mode 100644",
        "index 0000000..e69de29",
        "--- /dev/null",
        "+++ b/foo.txt",
        "@@ -0,0 +1 @@",
        "+hello",
        "",
    ])

    with patch("devcouncil.execution.patch.subprocess.run") as mock_run:
        mock_run.return_value.returncode = 0
        assert engine.apply_patch(diff_content) is True
        mock_run.assert_called()
        _, kwargs = mock_run.call_args
        assert kwargs.get("timeout") == _GIT_APPLY_TIMEOUT_SECONDS


def test_shell_backend_passes_timeout(tmp_path):
    backend = CommandLoopBackend()
    with patch("devcouncil.execution.shell_session.subprocess.run") as mock_run:
        mock_run.return_value.returncode = 0
        mock_run.return_value.stdout = ""
        mock_run.return_value.stderr = ""
        backend.run_command("echo hi", tmp_path, timeout=42)
        _, kwargs = mock_run.call_args
        assert kwargs.get("timeout") == 42


def test_guarded_shell_session_timeout_message(tmp_path):
    task = Task(id="TASK-TMO", title="T", description="D", allowed_commands=[_HELLO])
    session = GuardedShellSession(tmp_path, task, command_timeout=1)

    with patch.object(session.backend, "run_command", side_effect=subprocess.TimeoutExpired(_HELLO, 1)):
        code = session.run_one(_HELLO)

    assert code == 1
