"""DevCouncil skills library: load, select, and scaffold reusable agent skills.

A *skill* is a markdown file with YAML frontmatter describing when it applies. The
``core-engineering`` skill is always selected; domain skills (android, ios, windows,
web, ai-training, ...) are selected when the goal text or the repository's files match
their triggers. Selected skills can be rendered into an agent prompt preamble or
scaffolded into a target repo's ``.claude/skills/`` directory.
"""

from __future__ import annotations

import fnmatch
import os
import re
from pathlib import Path

from pydantic import BaseModel, Field

from devcouncil.knowledge.frontmatter import build_frontmatter_markdown, split_frontmatter

LIBRARY_DIR = Path(__file__).resolve().parent / "library"


def _keyword_in_text(keyword: str, text_lower: str) -> bool:
    """Whether a trigger keyword appears in already-lowercased goal text.

    Plain alphanumeric keywords ("gin", "unity", "flutter") match on word
    boundaries so a short framework name can't fire on an unrelated word it
    happens to sit inside ("gin" in "engine", "echo" in "echoes", "go" in
    "logo"). Keywords that contain spaces or punctuation ("react native",
    ".net", "c#", "c++", "ci/cd") are distinctive enough to match as substrings.
    """
    kw = keyword.lower().strip()
    if not kw:
        return False
    if kw.isalnum():
        return re.search(rf"(?<![a-z0-9]){re.escape(kw)}(?![a-z0-9])", text_lower) is not None
    return kw in text_lower

# Directories never worth walking when matching file-based triggers.
_PRUNE_DIRS = {
    ".git", ".hg", ".svn", "node_modules", ".venv", "venv", "__pycache__",
    ".devcouncil", ".idea", ".gradle", "build", "dist", ".mypy_cache",
    ".pytest_cache", ".ruff_cache", "DerivedData", "Pods",
}
_MAX_WALK_FILES = 20_000


class SkillTriggers(BaseModel):
    keywords: list[str] = Field(default_factory=list)
    globs: list[str] = Field(default_factory=list)


class Skill(BaseModel):
    name: str
    title: str = ""
    description: str = ""
    always: bool = False
    triggers: SkillTriggers = Field(default_factory=SkillTriggers)
    body: str = ""
    source_path: Path | None = None

    def matches(self, goal: str, repo_files_present: "set[str] | None" = None) -> bool:
        """True if this skill applies to the given goal text / repo file basenames."""
        if self.always:
            return True
        goal_lower = goal.lower()
        if any(_keyword_in_text(keyword, goal_lower) for keyword in self.triggers.keywords):
            return True
        if repo_files_present:
            for pattern in self.triggers.globs:
                pat = pattern.lower()
                if any(fnmatch.fnmatch(name, pat) for name in repo_files_present):
                    return True
        return False

    def relevance_score(self, goal: str, repo_files_present: "set[str] | None" = None) -> int:
        """How strongly this skill applies — used to rank which skills ride inline before
        the size budget truncates. Goal-text keyword hits weigh more than file-glob
        presence; always-on skills sort first regardless."""
        if self.always:
            return 1_000_000
        goal_lower = goal.lower()
        score = 2 * sum(1 for keyword in self.triggers.keywords if _keyword_in_text(keyword, goal_lower))
        if repo_files_present:
            score += sum(
                1 for pattern in self.triggers.globs
                if any(fnmatch.fnmatch(name, pattern.lower()) for name in repo_files_present)
            )
        return score

    def to_skill_md(self) -> str:
        """Render as a Claude-Code-style SKILL.md (name + description frontmatter + body)."""
        return build_frontmatter_markdown(
            {"name": self.name, "description": self.description},
            self.body,
        )


# Frontmatter parsing lives in devcouncil.knowledge.frontmatter so skills and the OKF /
# design.md formats share one implementation; kept aliased here for existing callers.
_split_frontmatter = split_frontmatter


def _skill_from_meta(path: Path, meta: dict, body: str) -> Skill:
    triggers = meta.get("triggers") or {}
    return Skill(
        name=str(meta.get("name") or path.stem),
        title=str(meta.get("title") or ""),
        description=str(meta.get("description") or ""),
        always=bool(meta.get("always", False)),
        triggers=SkillTriggers(
            keywords=list(triggers.get("keywords") or []),
            globs=list(triggers.get("globs") or []),
        ),
        body=body.strip(),
        source_path=path,
    )


# Repo-local skill locations, scanned in addition to the packaged library so users
# can drop their own skill markdown into a project and have it picked up.
REPO_SKILL_DIRS = (".claude/skills", ".devcouncil/skills")


def _try_skill_from_file(path: Path) -> Skill | None:
    """Parse a markdown file into a Skill in a single read, or None if it isn't a skill.

    A markdown file is a skill only if its frontmatter carries a ``name``; plain docs
    (e.g. a contributor README) are ignored. Reads the file once — previously callers
    read it twice (an ``_is_skill_file`` check followed by a separate parse)."""
    try:
        meta, body = _split_frontmatter(path.read_text(encoding="utf-8"))
    except OSError:
        return None
    if not meta.get("name"):
        return None
    return _skill_from_meta(path, meta, body)


def discover_repo_skills(project_root: Path) -> list[Skill]:
    """Find user-authored skills in a repo (``.claude/skills/**/SKILL.md`` etc.).

    Honors the same frontmatter contract as the packaged library; files without a
    ``name`` (e.g. plain docs) are ignored.
    """
    found: list[Skill] = []
    seen: set[Path] = set()
    for rel in REPO_SKILL_DIRS:
        base = project_root / rel
        if not base.exists():
            continue
        candidates = sorted(base.rglob("SKILL.md")) + sorted(base.glob("*.md"))
        for path in candidates:
            resolved = path.resolve()
            if resolved in seen:
                continue
            skill = _try_skill_from_file(path)
            if skill is None:
                continue
            seen.add(resolved)
            found.append(skill)
    return found


def load_okf_skills(project_root: Path, directory: str = ".devcouncil/knowledge") -> list[Skill]:
    """Load skills from ingested OKF documents typed as engineering skills.

    Reads every ``*.md`` under ``<project_root>/<directory>/okf/`` (recursively), parses
    each as an :class:`~devcouncil.knowledge.okf.OKFDocument`, and keeps the ones whose
    ``type`` marks them as a skill (the OKF->Skill conversion returns ``None`` for any
    other node type — BigQuery tables, tasks, ...). This is how an OKF bundle ingested
    from another repo contributes its skills to selection alongside the packaged library.

    ``index.md`` files are skipped: a bundle index is navigation scaffolding, not a node.
    The document's ``rel_path`` is set relative to the okf dir so its skill ``name`` derives
    from the file stem (matching how the export side names ``skills/<name>.md``).
    """
    # Lazy import to avoid a registry <-> skill_bridge import cycle (see that module);
    # OKFDocument is cycle-safe (knowledge.okf doesn't import skills) but kept here too
    # to keep the OKF-ingest dependency local to the one function that uses it.
    from devcouncil.knowledge.okf import OKFDocument
    from devcouncil.knowledge.skill_bridge import okf_document_to_skill

    okf_dir = project_root / directory / "okf"
    if not okf_dir.exists():
        return []
    skills: list[Skill] = []
    for path in sorted(okf_dir.rglob("*.md")):
        if path.name == "index.md":
            continue
        rel = path.relative_to(okf_dir).as_posix()
        doc = OKFDocument.from_markdown(path.read_text(encoding="utf-8"), rel_path=rel)
        skill = okf_document_to_skill(doc)
        if skill is not None:
            skills.append(skill)
    return skills


def load_skills(
    library_dir: Path = LIBRARY_DIR,
    project_root: Path | None = None,
    include_okf: bool = True,
) -> list[Skill]:
    """Load skills: the packaged library plus, when ``project_root`` is given, the
    repo's own skills and (when ``include_okf``) skills from ingested OKF documents.
    Repo-local skills override packaged ones with the same name.

    OKF-derived skills are merged in last and only for names not already taken, so a
    packaged library skill or a repo-local skill always wins a name conflict over an
    ingested bundle node (the local definition is authoritative and carries richer
    selection metadata like globs that OKF tags can't represent).

    Always-on skills come first, then alphabetical. Markdown files without skill
    frontmatter (e.g. a contributor README) are ignored.
    """
    by_name: dict[str, Skill] = {}
    if library_dir.exists():
        for path in sorted(library_dir.glob("*.md")):
            skill = _try_skill_from_file(path)
            if skill is not None:
                by_name[skill.name] = skill
    if project_root is not None:
        for skill in discover_repo_skills(project_root):
            base = by_name.get(skill.name)
            if base is not None:
                # A repo-local copy of a library skill (commonly a scaffolded
                # passthrough whose SKILL.md frontmatter is only name+description)
                # overrides the body/description, but must INHERIT the library's
                # selection metadata when it doesn't declare its own — otherwise
                # scaffolding a skill silently strips its `always`/triggers and the
                # skill stops being selected (selection would return nothing).
                has_own_triggers = bool(skill.triggers.keywords or skill.triggers.globs)
                skill = skill.model_copy(update={
                    "always": skill.always or base.always,
                    "triggers": skill.triggers if has_own_triggers else base.triggers,
                })
            by_name[skill.name] = skill  # repo-local wins on name conflict
        if include_okf:
            # Honor a custom knowledge.directory: `dev okf ingest` and knowledge-source
            # discovery both write/read under the configured dir, so the skill-ingest read
            # path must too — otherwise ingested OKF skills land somewhere this never looks
            # and silently never get selected. Best-effort; lazy import keeps app.config out
            # of the skills package's module-load graph.
            knowledge_dir = ".devcouncil/knowledge"
            try:
                from devcouncil.app.config import load_config
                knowledge_dir = load_config(project_root).knowledge.directory
            except Exception:
                pass
            for skill in load_okf_skills(project_root, directory=knowledge_dir):
                # Only fill gaps: library + repo-local skills win on name conflict.
                by_name.setdefault(skill.name, skill)
    skills = list(by_name.values())
    skills.sort(key=lambda s: (not s.always, s.name))
    return skills


def get_skill(name: str, library_dir: Path = LIBRARY_DIR, project_root: Path | None = None) -> Skill | None:
    for skill in load_skills(library_dir, project_root):
        if skill.name == name:
            return skill
    return None


# Per-process cache of the repo file scan, keyed by (resolved path, root mtime).
# Selecting skills for every task in a `dev e2e`/`repair-all` run would otherwise walk
# the whole tree once per task. Keyed on the root dir's mtime so adding/removing a
# top-level marker file (package.json, build.gradle, go.mod, ...) invalidates it.
_basename_cache: dict[tuple[str, int], set[str]] = {}
_BASENAME_CACHE_MAX = 32


def clear_skill_caches() -> None:
    """Drop the cached repo file scans (useful in long-running processes/tests)."""
    _basename_cache.clear()


def _walk_repo_basenames(project_root: Path) -> set[str]:
    names: set[str] = set()
    count = 0
    for _dirpath, dirnames, filenames in os.walk(project_root):
        dirnames[:] = [d for d in dirnames if d not in _PRUNE_DIRS]
        for filename in filenames:
            names.add(filename.lower())
            count += 1
            if count >= _MAX_WALK_FILES:
                return names
    return names


def _collect_repo_basenames(project_root: Path) -> set[str]:
    """Lowercased basenames of files in the repo, with heavy dirs pruned and a cap.

    Result is cached per (resolved path, root mtime) so repeated selections within one
    run (e.g. one prompt per task in an e2e flow) don't re-walk the tree each time.
    """
    try:
        key: tuple[str, int] | None = (str(project_root.resolve()), project_root.stat().st_mtime_ns)
    except OSError:
        key = None
    if key is not None:
        cached = _basename_cache.get(key)
        if cached is not None:
            return cached
    names = _walk_repo_basenames(project_root)
    if key is not None:
        if len(_basename_cache) >= _BASENAME_CACHE_MAX:
            _basename_cache.clear()
        _basename_cache[key] = names
    return names


def select_skills(
    goal: str = "",
    project_root: Path | None = None,
    library_dir: Path = LIBRARY_DIR,
) -> list[Skill]:
    """Select the skills that apply to a goal and/or repository.

    Includes repo-local skills (``.claude/skills/**``) when ``project_root`` is given.
    """
    skills = load_skills(library_dir, project_root)
    repo_files = _collect_repo_basenames(project_root) if project_root else set()
    # Score each skill once and keep the ones that apply: for a Skill, matches() is exactly
    # relevance_score() > 0 (always-on -> 1_000_000; otherwise a positive score requires a
    # keyword/glob hit, which is what matches() tests), so a single pass replaces the old
    # match-then-score double walk. (This equivalence is Skill-specific — do NOT copy it to
    # KnowledgeSource, whose nonzero priority floor breaks it.)
    # Rank by relevance so the most applicable domain skill survives the inline budget on a
    # polyglot repo; always-on skills keep their leading position; ties break by name.
    scored = [
        (s, score) for s in skills if (score := s.relevance_score(goal, repo_files)) > 0
    ]
    scored.sort(key=lambda item: (not item[0].always, -item[1], item[0].name))
    return [skill for skill, _ in scored]


def render_preamble(skills: list[Skill]) -> str:
    """Concatenate skill bodies into a single prompt preamble block."""
    if not skills:
        return ""
    sections = [skill.body.strip() for skill in skills if skill.body.strip()]
    return "\n\n---\n\n".join(sections).strip()


def bound_skills(
    skills: list[Skill],
    max_skills: int = 5,
    max_chars: int = 14000,
) -> "tuple[list[Skill], list[Skill]]":
    """Split selected skills into (inline, deferred) to bound prompt size.

    Skills are kept in order (always-on first), so the core skill is always inline;
    once the skill count or the cumulative body size would be exceeded, the rest are
    deferred (their full text still lives in the scaffolded .claude/skills/ files).
    """
    inline: list[Skill] = []
    total = 0
    for skill in skills:
        body = skill.body.strip()
        if not body:
            continue
        if len(inline) >= max_skills or (inline and total + len(body) > max_chars):
            break
        inline.append(skill)
        total += len(body)
    inline_set = {id(s) for s in inline}
    deferred = [s for s in skills if id(s) not in inline_set and s.body.strip()]
    return inline, deferred


def scaffold_skills(project_root: Path, skills: list[Skill]) -> list[Path]:
    """Write the given skills into ``<project_root>/.claude/skills/<name>/SKILL.md``.

    Only rewrites a file when its content changes, so re-running is a no-op.
    """
    written: list[Path] = []
    skills_root = project_root / ".claude" / "skills"
    proot = project_root.resolve()
    for skill in skills:
        # Don't re-materialize a skill that already lives inside this repo.
        if skill.source_path is not None:
            try:
                skill.source_path.resolve().relative_to(proot)
                continue
            except ValueError:
                pass
        target = skills_root / skill.name / "SKILL.md"
        content = skill.to_skill_md()
        if target.exists() and target.read_text(encoding="utf-8") == content:
            continue
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(content, encoding="utf-8")
        written.append(target)
    return written
