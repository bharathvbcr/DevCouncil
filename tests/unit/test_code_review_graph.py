import yaml

from devcouncil.integrations.code_review_graph import CodeReviewGraphAdapter


def _write_config(tmp_path, enabled=True):
    config_dir = tmp_path / ".devcouncil"
    config_dir.mkdir()
    (config_dir / "config.yaml").write_text(
        yaml.safe_dump({
            "integrations": {
                "code_review_graph": {
                    "enabled": enabled,
                    "command": "code-review-graph",
                    "optional": True,
                }
            }
        }),
        encoding="utf-8",
    )


def test_code_review_graph_disabled_without_config(tmp_path):
    context = CodeReviewGraphAdapter(tmp_path).get_context(["src/app.py"])

    assert context.available is False
    assert "disabled" in context.summary


def test_code_review_graph_missing_binary_degrades_cleanly(tmp_path, monkeypatch):
    _write_config(tmp_path)
    monkeypatch.setattr("shutil.which", lambda command: None)

    context = CodeReviewGraphAdapter(tmp_path).get_context(["src/app.py"])

    assert context.available is False
    assert "not found" in context.summary


def test_code_review_graph_parses_json_context(tmp_path, monkeypatch):
    _write_config(tmp_path)
    monkeypatch.setattr("shutil.which", lambda command: command)

    class Result:
        returncode = 0
        stdout = '{"summary":"ok","impacted_files":["src/app.py"],"related_tests":["tests/test_app.py"]}'
        stderr = ""

    monkeypatch.setattr("subprocess.run", lambda *args, **kwargs: Result())

    context = CodeReviewGraphAdapter(tmp_path).get_context(["src/app.py"])

    assert context.available is True
    assert context.impacted_files == ["src/app.py"]
    assert context.related_tests == ["tests/test_app.py"]


def test_code_review_graph_parses_nested_file_objects(tmp_path, monkeypatch):
    _write_config(tmp_path)
    monkeypatch.setattr("shutil.which", lambda command: command)

    class Result:
        returncode = 0
        stdout = (
            '{"summary":"ok",'
            '"impact_radius":[{"path":"src/app.py"},{"file":"src/lib.py"}],'
            '"tests":[{"path":"tests/test_app.py"}]}'
        )
        stderr = ""

    monkeypatch.setattr("subprocess.run", lambda *args, **kwargs: Result())

    context = CodeReviewGraphAdapter(tmp_path).get_context(["src/app.py"])

    assert context.impacted_files == ["src/app.py", "src/lib.py"]
    assert context.related_tests == ["tests/test_app.py"]


def test_graph_context_answers_in_process_without_spawning_an_interpreter(tmp_path, monkeypatch):
    """`devcouncil_graph_context` must not pay a second Python startup.

    The handler shelled out to ``python -m devcouncil graph-context --json``,
    whose whole body is ``CodeReviewGraphAdapter(root).get_context(files)`` —
    the same call the handler already makes three lines below as its fallback.
    Measured at 0.69s per invocation for a constant negative answer, effectively
    all of it interpreter and import time, on a tool the reviewer subagent is
    instructed to call.
    """
    import asyncio
    import json
    import subprocess

    from devcouncil.integrations.mcp.handlers import graph as graph_handlers

    _write_config(tmp_path, enabled=False)

    # Spy at the process boundary rather than on a helper, so the assertion
    # keeps meaning after the helper is gone.
    spawned: list[object] = []
    real_run = subprocess.run

    def _spy(*args, **kwargs):
        spawned.append(args[0] if args else kwargs.get("args"))
        return real_run(*args, **kwargs)

    monkeypatch.setattr(subprocess, "run", _spy)

    result = asyncio.run(
        graph_handlers.handle_graph_context(tmp_path, {"files": ["src/app.py"]})
    )

    assert spawned == [], f"no subprocess may be spawned, saw {spawned}"
    payload = json.loads(result[0].text)
    assert payload["available"] is False
    assert payload["changed_files"] == ["src/app.py"]
