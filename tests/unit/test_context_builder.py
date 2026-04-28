import subprocess

from devcouncil.execution.context_builder import ContextBuilder


def test_context_builder_structure_includes_untracked_files(tmp_path):
    subprocess.check_call(["git", "init"], cwd=tmp_path, stdout=subprocess.DEVNULL)
    tracked = tmp_path / "tracked.py"
    tracked.write_text("tracked\n", encoding="utf-8")
    subprocess.check_call(["git", "add", "tracked.py"], cwd=tmp_path, stdout=subprocess.DEVNULL)
    untracked = tmp_path / "new_file.py"
    untracked.write_text("new\n", encoding="utf-8")

    files = ContextBuilder(tmp_path).get_structure_summary()

    assert "tracked.py" in files
    assert "new_file.py" in files
