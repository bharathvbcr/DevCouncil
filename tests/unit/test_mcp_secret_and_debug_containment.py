"""The two containment guards an MCP client can reach, and their known bypasses.

Both defects here were found by an audit and PROVEN by execution before being
fixed, and both were fixed without a regression test because the session doing
it was interrupted. These are those tests, written afterwards and each verified
red against the pre-fix code by reverting the one fix it covers.

What makes both worth pinning rather than trusting:

* **The guard and the read were looking at different files.** `handle_read_file`
  ran `is_secret_path` on the caller's raw string, then resolved the path and
  opened the resolved one. Anything that changed between those two steps —
  letter case on a case-insensitive filesystem, a symlink — was a bypass, and
  the failure mode is silent: the tool returns the file's contents with
  `ok: true`.

* **A capability gate that a tool argument could set.**
  `devcouncil_debug_discover {"consent": true}` wrote `auto_discover: true` into
  `.devcouncil/config.yaml`, unlocking the other seven debug tools — including
  one that ran a caller-supplied script path through `subprocess.run`. A gate the
  caller can open is not a gate.

Every case below is a real filesystem: `.env` is written with a canary string and
the assertion is that the canary does not come back, rather than that some error
code was returned. An error code can be right while the bytes still leak.
"""

from __future__ import annotations

import asyncio
import json
from pathlib import Path

import pytest

from devcouncil.integrations.mcp.handlers import debug as debug_handlers
from devcouncil.integrations.mcp.handlers.read import handle_read_file
from devcouncil.integrations.mcp.util import is_secret_path

CANARY = "sk-CANARY-THIS-MUST-NOT-LEAK"


@pytest.fixture()
def secrets_project(tmp_path: Path) -> Path:
    """A project holding one real secret, reachable by several names."""
    (tmp_path / ".env").write_text(f"OPENAI_API_KEY={CANARY}\n", encoding="utf-8")
    # The two proven bypasses. `notes.txt` is the dangerous one: nothing about
    # the name a caller sends suggests a secret.
    (tmp_path / "notes.txt").symlink_to(tmp_path / ".env")
    (tmp_path / "key.pem").write_text(f"-----BEGIN KEY-----\n{CANARY}\n", encoding="utf-8")
    (tmp_path / "innocent.md").write_text("no secrets here\n", encoding="utf-8")
    return tmp_path


def read_file(root: Path, path: str) -> dict:
    payload = asyncio.run(handle_read_file(root, {"path": path}))
    return json.loads(payload[0].text)


@pytest.mark.parametrize(
    "path",
    [
        pytest.param(".env", id="exact-name"),
        # `fnmatch` is case-sensitive; macOS and Windows filesystems are not, so
        # the guard said "not a secret" and the open succeeded.
        pytest.param(".ENV", id="uppercased-bypass"),
        pytest.param(".Env", id="mixed-case-bypass"),
        # The guard saw "notes.txt". The open followed the link to `.env`.
        pytest.param("notes.txt", id="symlink-bypass"),
        pytest.param("./notes.txt", id="symlink-bypass-dot-prefixed"),
        pytest.param("key.pem", id="pem-by-glob"),
        pytest.param("KEY.PEM", id="pem-uppercased-bypass"),
    ],
)
def test_a_secret_never_leaves_the_reader_by_any_of_its_names(
    secrets_project: Path, path: str
) -> None:
    """The canary must not come back, whatever the file was called on the way in.

    Asserted on the bytes and not only on the code: a handler can return
    `code="secret_path"` on one branch while another branch returns the content,
    and only the content assertion can tell those apart.
    """
    result = read_file(secrets_project, path)

    assert CANARY not in json.dumps(result), (
        f"reading {path!r} leaked the secret's contents"
    )
    assert result.get("ok") is False, f"reading {path!r} succeeded"
    assert result.get("code") == "secret_path", (
        f"reading {path!r} was refused, but not as a secret path: {result.get('code')!r}. "
        "The distinction matters — 'not found' would send a caller looking for the file."
    )


def test_a_non_secret_file_is_still_readable(secrets_project: Path) -> None:
    """The guard must not be closed by over-blocking everything.

    A containment test that only checks refusals passes just as well against a
    handler that refuses every path, which would be a different defect.
    """
    result = read_file(secrets_project, "innocent.md")
    assert result.get("ok") is True, result
    assert "no secrets here" in json.dumps(result)


def test_is_secret_path_folds_case_only_when_it_reaches_the_same_file(
    tmp_path: Path,
) -> None:
    """The matcher itself, and the exact shape of its case handling.

    Pinned separately because the handler-level test would still pass if the
    handler compensated for a case-sensitive matcher some other way, and the
    next caller of `is_secret_path` would inherit the hole.

    The fold is deliberately conditional on `os.stat` agreeing that the two
    spellings are one file, and this test asserts that condition rather than
    plain case-insensitivity. An unconditional fold would refuse `.ENV` on a
    case-sensitive filesystem, where it is a *different, ordinary* file that
    the patterns were never meant to protect — over-refusing a path the user
    can legitimately read. The bypass only ever mattered when the alternate
    spelling actually reached the secret, which is what is set up here.
    """
    (tmp_path / ".env").write_text("K=x\n", encoding="utf-8")
    (tmp_path / "id_rsa").write_text("KEY\n", encoding="utf-8")
    (tmp_path / "key.pem").write_text("KEY\n", encoding="utf-8")

    for name in (".env", "id_rsa", "key.pem"):
        assert is_secret_path(tmp_path, name) is True, f"{name} is a secret by its own name"

    same_file = (tmp_path / ".ENV").exists()
    for name in (".ENV", ".Env", "ID_RSA", "KEY.PEM"):
        verdict = is_secret_path(tmp_path, name)
        if same_file:
            # Case-insensitive filesystem: the spelling reaches the secret, so
            # it must be refused. This is the proven macOS/NTFS bypass.
            assert verdict is True, f"{name} reaches the secret file and was not refused"
        else:
            # Case-sensitive filesystem: a different file, and refusing it would
            # be over-blocking. Asserted rather than skipped, so the narrowing is
            # a tested property on Linux instead of an untested claim.
            assert verdict is False, (
                f"{name} is a distinct file on this filesystem and must not be refused"
            )


class TestDebugConsentCannotBeSelfGranted:
    """`consent: true` is a question, not a grant."""

    def test_the_argument_does_not_unlock_the_debugger(self, tmp_path: Path) -> None:
        (tmp_path / ".devcouncil").mkdir()
        config = tmp_path / ".devcouncil" / "config.yaml"

        with pytest.raises(PermissionError) as refusal:
            asyncio.run(debug_handlers._discover(tmp_path, {"consent": True}))

        # The message has to name the way a user actually grants it, or the
        # agent that hit this has no next step and will retry the same call.
        message = str(refusal.value)
        assert "dev debug discover --consent" in message or "auto_discover" in message, message

        assert not config.exists() or "auto_discover" not in config.read_text(), (
            "the tool argument wrote consent into the project config; a gate the "
            "caller can open is not a gate"
        )


class TestDebugPathsAreContainedInTheProjectRoot:
    """`script`/`path`/`source` reach `subprocess.run`, so they are contained.

    `resolve_root` only ever constrained `projectPath`. The provider's own first
    line was ``script if script.is_absolute() else self.root / script`` — no
    resolve, no containment — so an absolute path ran verbatim.
    """

    @pytest.mark.parametrize(
        "raw, why",
        [
            pytest.param("/etc/passwd", "an absolute path outside the root", id="absolute"),
            pytest.param("../../../etc/passwd", "parent traversal", id="traversal"),
            pytest.param("escape.py", "a symlink pointing outside the root", id="symlink"),
        ],
    )
    def test_a_path_leaving_the_root_is_refused(
        self, tmp_path: Path, raw: str, why: str
    ) -> None:
        outside = tmp_path.parent / "outside_target.py"
        outside.write_text("print('should never run')\n", encoding="utf-8")
        root = tmp_path / "project"
        root.mkdir()
        (root / "escape.py").symlink_to(outside)

        with pytest.raises(debug_handlers.DebugPathOutsideRoot):
            debug_handlers._contained(root, {"script": raw}, "script")

    def test_a_path_inside_the_root_is_still_accepted(self, tmp_path: Path) -> None:
        """Containment that refuses everything is not containment."""
        (tmp_path / "app.py").write_text("x = 1\n", encoding="utf-8")
        resolved = debug_handlers._contained(tmp_path, {"script": "app.py"}, "script")
        assert resolved == (tmp_path / "app.py").resolve()

    def test_a_relative_path_resolves_against_the_root_not_the_process_cwd(
        self, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        """The server's working directory must not decide what a caller reaches.

        A relative path taken against `os.getcwd()` would resolve somewhere the
        caller never named and the root never contained — and the containment
        check would then pass against the wrong base.
        """
        root = tmp_path / "project"
        root.mkdir()
        (root / "app.py").write_text("x = 1\n", encoding="utf-8")
        elsewhere = tmp_path / "elsewhere"
        elsewhere.mkdir()
        (elsewhere / "app.py").write_text("x = 2\n", encoding="utf-8")
        monkeypatch.chdir(elsewhere)

        resolved = debug_handlers._contained(root, {"script": "app.py"}, "script")
        assert resolved == (root / "app.py").resolve()
