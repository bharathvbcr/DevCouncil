"""Tests for utils/json_persist.py."""

from pydantic import BaseModel

from devcouncil.utils.json_persist import read_json, read_model_json, write_json, write_model_json


class Sample(BaseModel):
    name: str
    count: int = 0


def test_write_and_read_json_roundtrip(tmp_path):
    path = tmp_path / "state.json"
    write_json(path, {"ok": True, "items": [1, 2]})
    assert read_json(path) == {"ok": True, "items": [1, 2]}


def test_write_and_read_model_json_roundtrip(tmp_path):
    path = tmp_path / "model.json"
    write_model_json(path, Sample(name="x", count=3))
    loaded = read_model_json(path, Sample)
    assert loaded.name == "x"
    assert loaded.count == 3
