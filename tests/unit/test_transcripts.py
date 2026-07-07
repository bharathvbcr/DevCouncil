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
