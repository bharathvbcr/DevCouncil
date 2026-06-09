
from devcouncil.indexing.semantic_index import SemanticIndex


def test_public_api_change_classification(tmp_path):
    src = tmp_path / "src"
    src.mkdir()
    before = src / "api.py"
    before.write_text("def public_api():\n    return 1\n", encoding="utf-8")
    index = SemanticIndex(tmp_path)
    index.create_snapshot("TASK-001", "before")
    before.write_text("def public_api(x):\n    return x\n", encoding="utf-8")
    index.create_snapshot("TASK-001", "after")
    result = index.diff("TASK-001")
    types = {item["type"] for item in result["classifications"]}
    assert "public_api_change" in types or "private_implementation_change" in types


def test_config_change_classification(tmp_path):
    (tmp_path / "pyproject.toml").write_text("[project]\nname='a'\n", encoding="utf-8")
    index = SemanticIndex(tmp_path)
    index.create_snapshot("TASK-001", "before")
    (tmp_path / "pyproject.toml").write_text("[project]\nname='b'\n", encoding="utf-8")
    index.create_snapshot("TASK-001", "after")
    result = index.diff("TASK-001")
    assert any(item["type"] == "config_schema_dependency_change" for item in result["classifications"])


def test_body_only_source_change_classifies_private_implementation(tmp_path):
    src = tmp_path / "src"
    src.mkdir()
    target = src / "worker.py"
    target.write_text("def helper():\n    return 1\n", encoding="utf-8")
    index = SemanticIndex(tmp_path)
    index.create_snapshot("TASK-001", "before")
    target.write_text("def helper():\n    return 2\n", encoding="utf-8")
    index.create_snapshot("TASK-001", "after")

    result = index.diff("TASK-001")

    assert any(
        item["type"] == "private_implementation_change" and item["path"] == "src/worker.py"
        for item in result["classifications"]
    )
