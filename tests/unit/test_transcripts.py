import json

from devcouncil.live.transcripts import (
    _claude_transcript_candidates,
    claude_project_slug,
    claude_transcript_for_session,
    claude_transcript_for_task,
    discover_sessions,
)


def test_claude_project_slug_encodes_absolute_path():
    assert claude_project_slug("/Users/foo/Code/Bar") == "-Users-foo-Code-Bar"


def test_claude_transcript_candidates_scoped_to_project(tmp_path, monkeypatch):
    fake_root = tmp_path / "claude" / "projects"
    project = tmp_path / "workspace"
    project.mkdir()
    slug = claude_project_slug(project)
    project_dir = fake_root / slug
    project_dir.mkdir(parents=True)
    other_dir = fake_root / "-other-project"
    other_dir.mkdir()
    ours = project_dir / "aaaa-bbbb.jsonl"
    theirs = other_dir / "cccc-dddd.jsonl"
    ours.write_text("{}", encoding="utf-8")
    theirs.write_text("{}", encoding="utf-8")

    monkeypatch.setattr("devcouncil.live.transcripts.CLAUDE_TRANSCRIPT_ROOT", fake_root)

    candidates = _claude_transcript_candidates(project)
    assert candidates == [ours]


def test_claude_transcript_for_task_uses_pinned_session(tmp_path, monkeypatch):
    fake_root = tmp_path / "claude" / "projects"
    project = tmp_path / "workspace"
    project.mkdir()
    slug = claude_project_slug(project)
    project_dir = fake_root / slug
    project_dir.mkdir(parents=True)
    session_id = "11111111-2222-3333-4444-555555555555"
    transcript = project_dir / f"{session_id}.jsonl"
    transcript.write_text("{}", encoding="utf-8")
    sessions_dir = project / ".devcouncil" / "sessions"
    sessions_dir.mkdir(parents=True)
    (sessions_dir / "TASK-1-claude.json").write_text(
        json.dumps({"session_id": session_id}), encoding="utf-8"
    )

    monkeypatch.setattr("devcouncil.live.transcripts.CLAUDE_TRANSCRIPT_ROOT", fake_root)

    assert claude_transcript_for_task(project, "TASK-1") == transcript
    assert claude_transcript_for_session(project, session_id) == transcript


def test_discover_sessions_never_includes_foreign_project_transcripts(tmp_path, monkeypatch):
    fake_root = tmp_path / "claude" / "projects"
    project = tmp_path / "workspace"
    project.mkdir()
    slug = claude_project_slug(project)
    project_dir = fake_root / slug
    project_dir.mkdir(parents=True)
    other_dir = fake_root / "-other-project"
    other_dir.mkdir()
    ours = project_dir / "session-a.jsonl"
    theirs = other_dir / "session-b.jsonl"
    ours.write_text("{}\n", encoding="utf-8")
    theirs.write_text("{}\n", encoding="utf-8")
    theirs.touch()
    ours.touch()

    monkeypatch.setattr("devcouncil.live.transcripts.CLAUDE_TRANSCRIPT_ROOT", fake_root)

    sessions = discover_sessions(project, client="claude")
    assert len(sessions) == 1
    assert sessions[0].transcript_path == str(ours)


def test_mirror_claude_transcript_copies_into_project(tmp_path, monkeypatch):
    from devcouncil.live.transcripts import mirror_claude_transcript

    fake_root = tmp_path / "claude" / "projects"
    project = tmp_path / "workspace"
    project.mkdir()
    slug = claude_project_slug(project)
    project_dir = fake_root / slug
    project_dir.mkdir(parents=True)
    session_id = "22222222-3333-4444-5555-666666666666"
    source = project_dir / f"{session_id}.jsonl"
    source.write_text('{"role":"assistant","content":"hi"}\n', encoding="utf-8")
    monkeypatch.setattr("devcouncil.live.transcripts.CLAUDE_TRANSCRIPT_ROOT", fake_root)

    dest = mirror_claude_transcript(project, session_id)
    assert dest == project / ".devcouncil" / "live" / "claude" / f"{session_id}.jsonl"
    assert dest.is_file()
    assert "assistant" in dest.read_text(encoding="utf-8")


def test_load_turns_parses_claude_jsonl(tmp_path):
    from devcouncil.live.transcripts import load_turns

    path = tmp_path / "session.jsonl"
    path.write_text(
        '{"role":"assistant","content":"hello"}\n'
        '{"message":{"role":"user","content":"question"}}\n'
        '{"content":""}\n',
        encoding="utf-8",
    )
    turns = load_turns(path, client="claude")
    assert len(turns) == 2
    assert turns[0].role == "assistant"
    assert turns[0].content == "hello"
    assert turns[1].role == "user"


def test_load_turns_nested_content_blocks(tmp_path):
    from devcouncil.live.transcripts import load_turns

    path = tmp_path / "session.jsonl"
    path.write_text(
        json.dumps(
            {
                "message": {
                    "role": "assistant",
                    "content": [{"text": "part1"}, {"text": "part2"}],
                }
            }
        )
        + "\n",
        encoding="utf-8",
    )
    turns = load_turns(path)
    assert turns[0].content == "part1\npart2"


def test_latest_assistant_turn_uses_cache(tmp_path, monkeypatch):
    from devcouncil.live.transcripts import latest_assistant_turn

    path = tmp_path / "session.jsonl"
    path.write_text('{"role":"assistant","content":"cached"}\n', encoding="utf-8")
    first = latest_assistant_turn(path)
    assert first is not None
    assert first.content == "cached"

    path.write_text('{"role":"assistant","content":"changed"}\n', encoding="utf-8")
    # Without stat change, cache may still return old — force new mtime by touching
    import os
    os.utime(path, None)
    second = latest_assistant_turn(path)
    assert second.content == "changed"


def test_latest_assistant_turn_oserror_falls_back(tmp_path, monkeypatch):
    from devcouncil.live import transcripts as tr

    path = tmp_path / "session.jsonl"
    path.write_text('{"role":"assistant","content":"ok"}\n', encoding="utf-8")

    def _raise_stat(self):
        raise OSError("nope")

    monkeypatch.setattr(tr.Path, "stat", _raise_stat)
    turn = tr.latest_assistant_turn(path)
    assert turn is not None
    assert turn.content == "ok"


def test_discover_sessions_generic_client(tmp_path):
    live_dir = tmp_path / ".devcouncil" / "live" / "custom"
    live_dir.mkdir(parents=True)
    transcript = live_dir / "sess.jsonl"
    transcript.write_text('{"role":"assistant","content":"x"}\n', encoding="utf-8")

    sessions = discover_sessions(tmp_path, client="custom")
    assert len(sessions) == 1
    assert sessions[0].client == "custom"


def test_discover_sessions_skips_missing_files(tmp_path):
    live_dir = tmp_path / ".devcouncil" / "live" / "custom"
    live_dir.mkdir(parents=True)
    assert discover_sessions(tmp_path, client="custom") == []


def test_claude_transcript_for_session_empty_id(tmp_path):
    assert claude_transcript_for_session(tmp_path, "") is None


def test_mirror_claude_transcript_oserror(tmp_path, monkeypatch):
    from devcouncil.live import transcripts as tr

    fake_root = tmp_path / "claude" / "projects"
    project = tmp_path / "workspace"
    project.mkdir()
    slug = claude_project_slug(project)
    project_dir = fake_root / slug
    project_dir.mkdir(parents=True)
    session_id = "abc"
    source = project_dir / f"{session_id}.jsonl"
    source.write_text("{}", encoding="utf-8")
    monkeypatch.setattr(tr, "CLAUDE_TRANSCRIPT_ROOT", fake_root)
    monkeypatch.setattr(tr.shutil, "copy2", lambda *a, **k: (_ for _ in ()).throw(OSError("disk full")))

    assert tr.mirror_claude_transcript(project, session_id) is None


def test_role_from_type_field():
    from devcouncil.live.transcripts import _role

    assert _role({"type": "tool"}) == "tool"
    assert _role({"type": "unknown-thing"}) == "unknown"


def test_safe_lines_oserror(tmp_path, monkeypatch):
    from devcouncil.live import transcripts as tr

    path = tmp_path / "missing.jsonl"
    monkeypatch.setattr(tr.Path, "read_text", lambda self, **k: (_ for _ in ()).throw(OSError("nope")))
    assert tr._safe_lines(path) == []


def test_read_claude_session_id_invalid_json(tmp_path):
    from devcouncil.live.transcripts import _read_claude_session_id

    sessions_dir = tmp_path / ".devcouncil" / "sessions"
    sessions_dir.mkdir(parents=True)
    (sessions_dir / "TASK-1-claude.json").write_text("{bad json", encoding="utf-8")
    assert _read_claude_session_id(tmp_path, "TASK-1") is None


def test_claude_transcript_for_task_without_pin(tmp_path):
    assert claude_transcript_for_task(tmp_path, "TASK-1") is None
