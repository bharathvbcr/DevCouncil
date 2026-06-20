"""Context & structural intelligence: dependents normalization, no-map note,
language-aware symbol outlines, and the budgeted call-sites block."""

import json

from devcouncil.domain.task import PlannedFile, Task
from devcouncil.execution.prompt_builder import PromptBuilder


def _task(planned):
    return Task(id="TASK-001", title="T", description="D", planned_files=planned)


# ---------------------------------------------------------------------------
# rank 3 — dependents lookup normalizes backslash planned paths
# ---------------------------------------------------------------------------

def test_dependents_lookup_normalizes_backslash_path(tmp_path):
    (tmp_path / ".devcouncil").mkdir()
    repo_map = {"dependents": {"src/core/models.py": ["src/api/handlers.py"]}}
    (tmp_path / ".devcouncil" / "repo_map.json").write_text(json.dumps(repo_map), encoding="utf-8")
    # Planned path arrives with Windows separators; the map key is posix.
    task = _task([PlannedFile(path="src\\core\\models.py", reason="x", allowed_change="modify")])

    pb = PromptBuilder(tmp_path)
    section = pb._dependents_section(task, pb._load_repo_map())

    assert "Dependents (blast radius)" in section
    assert "src/api/handlers.py" in section


# ---------------------------------------------------------------------------
# rank 13a — absent repo map surfaces a "run dev map" note
# ---------------------------------------------------------------------------

def test_no_repo_map_note_when_map_absent(tmp_path):
    task = _task([PlannedFile(path="src/x.py", reason="x", allowed_change="modify")])
    prompt = PromptBuilder(tmp_path).build_task_prompt(task, [])
    assert "no repo map" in prompt.lower()
    assert "dev map" in prompt


def test_no_repo_map_note_absent_when_map_present(tmp_path):
    (tmp_path / ".devcouncil").mkdir()
    repo_map = {"dependents": {"src/x.py": ["src/y.py"]}}
    (tmp_path / ".devcouncil" / "repo_map.json").write_text(json.dumps(repo_map), encoding="utf-8")
    task = _task([PlannedFile(path="src/x.py", reason="x", allowed_change="modify")])
    prompt = PromptBuilder(tmp_path).build_task_prompt(task, [])
    assert "no repo map" not in prompt.lower()


# ---------------------------------------------------------------------------
# rank 11 — language-aware symbol outlines
# ---------------------------------------------------------------------------

def test_python_outline_emits_method_signatures_and_markers(tmp_path):
    text = (
        "class Service:\n"
        "    @property\n"
        "    def name(self):\n"
        "        return self._n\n"
        "    @staticmethod\n"
        "    def make(a, b):\n"
        "        return a + b\n"
        "    async def run(self, task):\n"
        "        return task\n"
    )
    out = PromptBuilder(tmp_path)._symbol_outline("svc.py", text)
    joined = "\n".join(out)
    assert "class Service L1" in joined
    assert "def name(self) L3 @property" in joined
    assert "def make(a, b) L6 @staticmethod" in joined
    assert "async def run(self, task) L8" in joined


def test_typescript_outline_extracts_exports(tmp_path):
    text = (
        "export function doThing(x: number): void {}\n"
        "export class Widget {}\n"
        "export interface Shape { x: number }\n"
        "const internal = 1\n"
    )
    out = PromptBuilder(tmp_path)._symbol_outline("ui/widget.ts", text)
    joined = "\n".join(out)
    assert "export function doThing L1" in joined
    assert "export class Widget L2" in joined
    assert "export interface Shape L3" in joined
    # Non-exported const must not appear.
    assert "internal" not in joined


def test_go_outline_extracts_exported_funcs_and_types(tmp_path):
    text = (
        "package main\n"
        "func Handle(w http.ResponseWriter) {}\n"
        "func (s *Server) Start() {}\n"
        "func unexported() {}\n"
        "type Config struct {}\n"
    )
    out = PromptBuilder(tmp_path)._symbol_outline("server.go", text)
    joined = "\n".join(out)
    assert "func Handle L2" in joined
    assert "func Start L3" in joined  # method with receiver, exported
    assert "type Config L5" in joined
    assert "unexported" not in joined


def test_rust_outline_extracts_pub_items(tmp_path):
    text = (
        "pub fn build() {}\n"
        "fn private() {}\n"
        "pub struct Engine {}\n"
        "pub trait Run {}\n"
    )
    out = PromptBuilder(tmp_path)._symbol_outline("lib.rs", text)
    joined = "\n".join(out)
    assert "pub fn build L1" in joined
    assert "pub struct Engine L3" in joined
    assert "pub trait Run L4" in joined
    assert "private" not in joined


def test_outline_respects_symbol_cap(tmp_path, monkeypatch):
    import devcouncil.execution.prompt_builder as pb_mod
    monkeypatch.setattr(pb_mod, "MAX_SYMBOLS_PER_FILE", 3)
    text = "".join(f"export function f{i}() {{}}\n" for i in range(50))
    out = PromptBuilder(tmp_path)._symbol_outline("big.ts", text)
    assert len(out) <= 3


def test_outline_unknown_language_returns_empty(tmp_path):
    out = PromptBuilder(tmp_path)._symbol_outline("notes.md", "# title\n")
    assert out == []


def test_outline_never_raises_on_garbage(tmp_path):
    # Invalid python must not raise — falls back to empty.
    assert PromptBuilder(tmp_path)._symbol_outline("broken.py", "def (:\n") == []


# ---------------------------------------------------------------------------
# rank 14 — call sites block
# ---------------------------------------------------------------------------

def _repo_with_callsites(tmp_path):
    (tmp_path / ".devcouncil").mkdir()
    src = tmp_path / "src"
    src.mkdir()
    (src / "models.py").write_text(
        "def make_user(name):\n    return name\n\n\nclass Account:\n    pass\n",
        encoding="utf-8",
    )
    (src / "handlers.py").write_text(
        "from src.models import make_user\n\n\ndef handle():\n    return make_user('a')\n",
        encoding="utf-8",
    )
    repo_map = {"dependents": {"src/models.py": ["src/handlers.py"]}}
    (tmp_path / ".devcouncil" / "repo_map.json").write_text(json.dumps(repo_map), encoding="utf-8")
    return _task([PlannedFile(path="src/models.py", reason="x", allowed_change="modify")])


def test_call_sites_section_lists_referencing_lines(tmp_path):
    task = _repo_with_callsites(tmp_path)
    pb = PromptBuilder(tmp_path)
    section = pb._call_sites_section(task, pb._load_repo_map())
    assert "Call sites" in section
    assert "src/handlers.py:" in section
    assert "make_user" in section


def test_call_sites_skipped_for_created_files(tmp_path):
    task = _repo_with_callsites(tmp_path)
    # Same map, but the planned file is a create -> no blast radius -> no call sites.
    task.planned_files[0].allowed_change = "create"
    pb = PromptBuilder(tmp_path)
    assert pb._call_sites_section(task, pb._load_repo_map()) == ""


def test_call_sites_is_lowest_priority_and_dropped_first(tmp_path):
    task = _repo_with_callsites(tmp_path)
    pb = PromptBuilder(tmp_path)
    data = pb._load_repo_map()
    full = pb.build_task_prompt(task, [], max_chars=10 ** 9)
    assert "Call sites" in full
    # Size a budget that fits the core + file contents + dependents but is one char shy
    # of also fitting the lowest-priority call-sites block, so it is the first to drop.
    files_text = pb._planned_files_section(task)
    deps_text = pb._dependents_section(task, data)
    call_text = pb._call_sites_section(task, data)
    overhead = len(full) - len(files_text) - len(deps_text) - len(call_text) - len(pb._skills_section(task))
    budget = overhead + len(files_text) + len(deps_text) + 5
    out = pb.build_task_prompt(task, [], max_chars=budget)
    assert "Current file contents" in out          # priority 1 kept
    assert "Dependents (blast radius)" in out       # priority 2 kept
    assert "Call sites (where your symbols are used)" not in out  # priority 4 dropped first
    assert "Context budget reached" in out
    assert "call sites" in out.split("omitted:")[1]  # named in the omission marker


def test_references_symbol_whole_word_only():
    assert PromptBuilder._references_symbol("x = make_user(1)", "make_user")
    # Substring of a longer identifier must not match.
    assert not PromptBuilder._references_symbol("x = make_user_helper()", "make_user")
    assert not PromptBuilder._references_symbol("nothing here", "make_user")
