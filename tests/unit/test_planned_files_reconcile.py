from devcouncil.domain.task import PlannedFile, Task
from devcouncil.planning.planned_files_reconcile import (
    expand_scope_with_dependents,
    reconcile_planned_files,
    repo_files_from_map,
)


def _task(*planned: PlannedFile, tid: str = "T1") -> Task:
    return Task(id=tid, title="t", description="d", planned_files=list(planned))


def test_repairs_typo_via_unique_basename_match():
    task = _task(PlannedFile(path="src/serivce/foo.ts", reason="fix", allowed_change="modify"))
    tasks, warnings = reconcile_planned_files([task], ["src/service/foo.ts"])
    assert tasks[0].planned_files[0].path == "src/service/foo.ts"
    assert any("repaired" in w for w in warnings)


def test_keeps_existing_path_untouched():
    task = _task(PlannedFile(path="src/service/foo.ts", reason="fix", allowed_change="modify"))
    tasks, warnings = reconcile_planned_files([task], ["src/service/foo.ts", "src/other.ts"])
    assert tasks[0].planned_files[0].path == "src/service/foo.ts"
    assert warnings == []


def test_keeps_create_even_when_absent_from_map():
    task = _task(PlannedFile(path="src/brand/new.ts", reason="new", allowed_change="create"))
    tasks, warnings = reconcile_planned_files([task], ["src/service/foo.ts"])
    assert tasks[0].planned_files[0].path == "src/brand/new.ts"
    assert warnings == []


def test_ambiguous_basename_is_kept_not_dropped_and_warned():
    task = _task(PlannedFile(path="foo.ts", reason="fix", allowed_change="modify"))
    tasks, warnings = reconcile_planned_files(
        [task], ["src/a/foo.ts", "src/b/foo.ts"]
    )
    # Never tightens scope: the (unresolvable) entry survives.
    assert tasks[0].planned_files[0].path == "foo.ts"
    assert any("ambiguous" in w for w in warnings)


def test_no_basename_match_is_kept_and_warned():
    task = _task(PlannedFile(path="src/ghost.ts", reason="fix", allowed_change="delete"))
    tasks, warnings = reconcile_planned_files([task], ["src/service/foo.ts"])
    assert tasks[0].planned_files[0].path == "src/ghost.ts"
    assert any("no matching file" in w for w in warnings)


def test_empty_repo_files_is_a_graceful_noop():
    task = _task(PlannedFile(path="src/whatever.ts", reason="fix", allowed_change="modify"))
    tasks, warnings = reconcile_planned_files([task], [])
    assert tasks[0].planned_files[0].path == "src/whatever.ts"
    assert warnings == []


def test_normalizes_paths_before_matching():
    task = _task(
        PlannedFile(path="./src/service/foo.ts", reason="fix", allowed_change="modify"),
        PlannedFile(path="src\\service\\bar.ts", reason="fix", allowed_change="modify"),
    )
    tasks, warnings = reconcile_planned_files(
        [task], ["src/service/foo.ts", "src/service/bar.ts"]
    )
    paths = {pf.path for pf in tasks[0].planned_files}
    assert paths == {"src/service/foo.ts", "src/service/bar.ts"}
    assert warnings == []


def test_expand_adds_dependents_of_modified_file():
    task = _task(PlannedFile(path="src/foo.py", reason="fix", allowed_change="modify"))
    dependents = {"src/foo.py": ["src/bar.py", "src/baz.py"]}
    repo_files = ["src/foo.py", "src/bar.py", "src/baz.py"]
    tasks, warnings = expand_scope_with_dependents([task], dependents, repo_files)
    paths = {pf.path for pf in tasks[0].planned_files}
    assert paths == {"src/foo.py", "src/bar.py", "src/baz.py"}
    assert all(
        pf.allowed_change == "modify" for pf in tasks[0].planned_files if pf.path != "src/foo.py"
    )
    assert any("widened scope" in w for w in warnings)


def test_expand_ignores_create_and_read_only():
    task = _task(
        PlannedFile(path="src/new.py", reason="new", allowed_change="create"),
        PlannedFile(path="src/ref.py", reason="ref", allowed_change="read_only"),
    )
    dependents = {"src/new.py": ["src/a.py"], "src/ref.py": ["src/b.py"]}
    tasks, warnings = expand_scope_with_dependents([task], dependents, ["src/a.py", "src/b.py"])
    assert len(tasks[0].planned_files) == 2
    assert warnings == []


def test_expand_skips_dependents_absent_from_repo():
    task = _task(PlannedFile(path="src/foo.py", reason="fix", allowed_change="modify"))
    dependents = {"src/foo.py": ["src/ghost.py"]}
    tasks, warnings = expand_scope_with_dependents([task], dependents, ["src/foo.py"])
    assert {pf.path for pf in tasks[0].planned_files} == {"src/foo.py"}
    assert warnings == []


def test_expand_does_not_duplicate_existing_planned_file():
    task = _task(
        PlannedFile(path="src/foo.py", reason="fix", allowed_change="modify"),
        PlannedFile(path="src/bar.py", reason="also", allowed_change="modify"),
    )
    dependents = {"src/foo.py": ["src/bar.py"]}
    tasks, warnings = expand_scope_with_dependents(
        [task], dependents, ["src/foo.py", "src/bar.py"]
    )
    assert len(tasks[0].planned_files) == 2
    assert warnings == []


def test_expand_caps_per_file():
    task = _task(PlannedFile(path="src/foo.py", reason="fix", allowed_change="modify"))
    deps = [f"src/dep{i}.py" for i in range(20)]
    tasks, _ = expand_scope_with_dependents(
        [task], {"src/foo.py": deps}, ["src/foo.py", *deps], max_per_file=8
    )
    assert len(tasks[0].planned_files) == 9  # 1 original + 8 capped additions


def test_expand_empty_dependents_is_noop():
    task = _task(PlannedFile(path="src/foo.py", reason="fix", allowed_change="modify"))
    tasks, warnings = expand_scope_with_dependents([task], {}, ["src/foo.py"])
    assert len(tasks[0].planned_files) == 1
    assert warnings == []


def test_repo_files_from_map_reads_objects_and_dicts():
    class _Entry:
        def __init__(self, path):
            self.path = path

    class _Map:
        files = [_Entry("src/a.ts"), _Entry("src/b.ts")]

    assert repo_files_from_map(_Map()) == ["src/a.ts", "src/b.ts"]
    assert repo_files_from_map({"files": [{"path": "src/c.ts"}]}) == ["src/c.ts"]
    assert repo_files_from_map(None) == []
