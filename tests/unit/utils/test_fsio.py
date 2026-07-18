"""atomic_write_text/bytes/json — content fidelity, overwrite semantics, and
no leftover temp files (the helpers stage a sibling .tmp and os.replace it)."""

import json

import pytest

from devcouncil.utils.fsio import atomic_write_bytes, atomic_write_json, atomic_write_text


def _names(directory):
    return sorted(entry.name for entry in directory.iterdir())


def test_atomic_write_text_creates_file_with_exact_content(tmp_path):
    target = tmp_path / "note.txt"

    atomic_write_text(target, "hello world\n")

    assert target.read_text(encoding="utf-8") == "hello world\n"
    assert _names(tmp_path) == ["note.txt"]


def test_atomic_write_text_overwrites_and_leaves_no_tmp_files(tmp_path):
    target = tmp_path / "note.txt"

    atomic_write_text(target, "first version")
    atomic_write_text(target, "second version ✓")

    assert target.read_text(encoding="utf-8") == "second version ✓"
    assert not [name for name in _names(tmp_path) if name.endswith(".tmp")]
    assert _names(tmp_path) == ["note.txt"]


def test_atomic_write_text_accepts_str_paths(tmp_path):
    target = tmp_path / "strpath.txt"

    atomic_write_text(str(target), "via str path")

    assert target.read_text(encoding="utf-8") == "via str path"


def test_atomic_write_bytes_roundtrip_and_overwrite(tmp_path):
    target = tmp_path / "blob.bin"

    atomic_write_bytes(target, b"\x00\x01\x02binary")
    assert target.read_bytes() == b"\x00\x01\x02binary"

    atomic_write_bytes(target, b"replaced")
    assert target.read_bytes() == b"replaced"
    assert _names(tmp_path) == ["blob.bin"]


def test_atomic_write_json_writes_loadable_payload_with_trailing_newline(tmp_path):
    target = tmp_path / "payload.json"
    payload = {"b": [1, 2, 3], "a": {"nested": True}, "s": "text"}

    atomic_write_json(target, payload)

    raw = target.read_text(encoding="utf-8")
    assert raw.endswith("\n")
    assert json.loads(raw) == payload
    assert _names(tmp_path) == ["payload.json"]


def test_atomic_write_requires_existing_parent_dir(tmp_path):
    missing = tmp_path / "no_such_dir" / "f.txt"

    with pytest.raises(OSError):
        atomic_write_text(missing, "data")

    assert not (tmp_path / "no_such_dir").exists()


def test_atomic_write_text_cleans_tmp_on_failure(tmp_path, monkeypatch):
    target = tmp_path / "fail.txt"
    real_replace = __import__("os").replace

    def boom(src, dst):
        raise RuntimeError("replace failed")

    monkeypatch.setattr("os.replace", boom)
    with pytest.raises(RuntimeError, match="replace failed"):
        atomic_write_text(target, "data")
    assert not target.exists()
    assert not [p for p in tmp_path.iterdir() if p.suffix == ".tmp"]
    monkeypatch.setattr("os.replace", real_replace)


def test_atomic_write_bytes_cleans_tmp_on_failure(tmp_path, monkeypatch):
    target = tmp_path / "fail.bin"

    def boom(src, dst):
        raise OSError("nope")

    monkeypatch.setattr("os.replace", boom)
    with pytest.raises(OSError, match="nope"):
        atomic_write_bytes(target, b"abc")
    assert not target.exists()
    assert not [p for p in tmp_path.iterdir() if p.suffix == ".tmp"]
