"""What the seam has to spawn to learn what the kernel can do.

Selecting and using a kernel asked the same binary three separate questions,
each in its own process:

  * `devmap --db <tmp> status`  -> which schema does it write?
  * `devmap manifest --help`    -> does it take `--graph-output`, and the three
                                   freshness digests?
  * `devmap build --help`       -> does it take `--manifest`?

The `status` probe has to happen anyway, so it is the one that should answer.
A kernel new enough to say so now reports a `capabilities` object and the two
`--help` launches are not made.

The fallback is the point of the second test. A kernel whose status carries no
`capabilities` key has given *no evidence* about its flags, which is not the
same as evidence that it lacks them — dropping the `--help` probe would turn
"older kernel" into "no devmap binary supports this map engine".
"""

from __future__ import annotations

import json
import subprocess
from pathlib import Path

from devcouncil import devmap_engine


def _kernel(path: Path, *, capabilities: dict | None, spawn_log: Path) -> Path:
    """A kernel that records every invocation, so the spawns can be counted.

    Each test gets its own `tmp_path`, so every kernel here is at a path no
    other test uses and the engine's memos — keyed on binary identity — start
    empty for it without anything being cleared. These tests deliberately do not
    clear the module's caches: doing so evicts the real kernel's entry too, and
    the next test to select a binary re-probes it, which is how one suite's
    stubbed `subprocess.run` ends up deciding another suite's kernel.
    """
    status: dict = {
        "generation_id": 1,
        "pending_count": 0,
        "node_count": 1,
        "edge_count": 0,
        "is_fresh": True,
        "expected_schema_version": 17,
    }
    if capabilities is not None:
        status["capabilities"] = capabilities
    flags = (
        "--output --graph-output --force "
        "--generated-head --indexed-hash --content-fingerprint"
    )
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        "#!/bin/sh\n"
        f'echo "$@" >> {spawn_log}\n'
        "while [ $# -gt 0 ]; do\n"
        '  case "$1" in\n'
        "    --db|--progress) shift 2 ;;\n"
        "    --json) shift ;;\n"
        "    *) break ;;\n"
        "  esac\n"
        "done\n"
        'case "$1" in\n'
        f"  manifest) echo 'Usage: devmap manifest [OPTIONS] {flags}' ;;\n"
        "  build) echo 'Usage: devmap build [OPTIONS] --manifest' ;;\n"
        f"  status) echo '{json.dumps(status)}' ;;\n"
        "  *) echo 'Usage: devmap' ;;\n"
        "esac\n"
    )
    path.chmod(0o755)
    return path


def _help_spawns(spawn_log: Path) -> list[str]:
    if not spawn_log.is_file():
        return []
    return [line for line in spawn_log.read_text().splitlines() if "--help" in line]


def test_a_kernel_that_declares_its_capabilities_is_not_probed_with_help(
    tmp_path: Path,
) -> None:
    """`status` already ran. Its answer is the answer."""
    spawn_log = tmp_path / "spawns.txt"
    binary = _kernel(
        tmp_path / "devmap",
        capabilities={
            "manifest_graph_output": True,
            "manifest_stamp_flags": True,
            "build_manifest": True,
        },
        spawn_log=spawn_log,
    )

    assert devmap_engine._manifest_accepts_stamp_flags(str(binary)) is True
    assert devmap_engine._build_accepts_manifest(str(binary)) is True

    assert _help_spawns(spawn_log) == [], (
        "the kernel declared these capabilities in its status; probing --help "
        f"for them spends a process launch on a question already answered: "
        f"{_help_spawns(spawn_log)}"
    )


def test_a_kernel_that_declares_nothing_still_falls_back_to_help(
    tmp_path: Path,
) -> None:
    """No `capabilities` key is the absence of evidence, not a denial.

    Rejecting such a kernel would turn "built before the key existed" into "does
    not support the map engine", which is the same conflation the doctor's
    artifact check had: a question that could not be asked must not be answered
    as though it had been asked and refused.
    """
    spawn_log = tmp_path / "spawns.txt"
    binary = _kernel(tmp_path / "devmap", capabilities=None, spawn_log=spawn_log)

    assert devmap_engine._manifest_accepts_stamp_flags(str(binary)) is True
    assert devmap_engine._build_accepts_manifest(str(binary)) is True

    assert _help_spawns(spawn_log), (
        "with nothing declared, the only way to know is to ask; the --help "
        "probe must still run"
    )


def test_a_kernel_that_denies_a_capability_is_believed(tmp_path: Path) -> None:
    """A declared `false` is evidence, and it is not second-guessed by --help.

    The declaration is derived from the kernel's own parser, so it is a stronger
    statement than a substring found in help text.
    """
    spawn_log = tmp_path / "spawns.txt"
    binary = _kernel(
        tmp_path / "devmap",
        capabilities={
            "manifest_graph_output": True,
            "manifest_stamp_flags": False,
            "build_manifest": False,
        },
        spawn_log=spawn_log,
    )

    assert devmap_engine._manifest_accepts_stamp_flags(str(binary)) is False
    assert devmap_engine._build_accepts_manifest(str(binary)) is False
    assert _help_spawns(spawn_log) == []


def test_a_probe_that_could_not_run_is_not_remembered_as_an_answer(
    tmp_path: Path,
) -> None:
    """One failed spawn must not condemn a working kernel for the whole process.

    The memos are keyed on a binary's identity, so a cached verdict is never
    revisited — and re-probing is the only thing that could clear it. That makes
    caching a *failure* permanent by construction: a probe that lost a fork, ran
    out of file descriptors, or was interrupted took the binary out of service
    until the process exited, and `find_engine_binary` then reported "no devmap
    binary supports this map engine" about a kernel that was fine all along.

    Observed: a test elsewhere in the suite replaces `subprocess.run` for the
    duration of one call. Any probe landing in that window cached its own
    failure, and every later selection in that process failed.

    Only an answer is memoised now.
    """
    spawn_log = tmp_path / "spawns.txt"
    binary = _kernel(
        tmp_path / "devmap",
        capabilities={
            "manifest_graph_output": True,
            "manifest_stamp_flags": True,
            "build_manifest": True,
        },
        spawn_log=spawn_log,
    )

    # The probe cannot run at all: every spawn raises, exactly as it would if
    # the fork failed or something had swapped `subprocess.run` out.
    real_run = subprocess.run

    def _refuses(*args, **kwargs):
        raise OSError("no subprocess for you")

    subprocess.run = _refuses  # type: ignore[assignment]
    try:
        assert devmap_engine._declared_capability(str(binary), "build_manifest") is None
    finally:
        subprocess.run = real_run  # type: ignore[assignment]

    # The binary was always fine. Now that spawning works, asking again must
    # actually ask — not replay the failure.
    assert devmap_engine._declared_capability(str(binary), "build_manifest") is True
    assert devmap_engine._build_accepts_manifest(str(binary)) is True


def test_the_capability_probe_and_the_schema_probe_are_one_launch(
    tmp_path: Path,
) -> None:
    """Both answers come out of the same `status` run, memoised together."""
    spawn_log = tmp_path / "spawns.txt"
    binary = _kernel(
        tmp_path / "devmap",
        capabilities={
            "manifest_graph_output": True,
            "manifest_stamp_flags": True,
            "build_manifest": True,
        },
        spawn_log=spawn_log,
    )

    assert devmap_engine._expected_schema_version(str(binary)) == 17
    assert devmap_engine._manifest_accepts_stamp_flags(str(binary)) is True
    assert devmap_engine._build_accepts_manifest(str(binary)) is True

    status_spawns = [
        line for line in spawn_log.read_text().splitlines() if "status" in line
    ]
    assert len(status_spawns) == 1, (
        f"the schema and the capabilities come from one probe: {status_spawns}"
    )
