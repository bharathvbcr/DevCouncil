"""DevCouncil skills library: load, select, and scaffold reusable agent skills.

A *skill* is a markdown file with YAML frontmatter describing when it applies. The
``core-engineering`` skill is always selected; domain skills (android, ios, windows,
web, ai-training, ...) are selected when the goal text or the repository's files match
their triggers. Selected skills can be rendered into an agent prompt preamble or
scaffolded into ``.claude/skills/``, ``.cursor/skills/``, and Codex's ``.agents/skills/``.
"""

from __future__ import annotations

import fnmatch
import functools
import hashlib
import json
import logging
import os
import re
import stat
import time
from pathlib import Path

from pydantic import BaseModel, Field

from devcouncil.knowledge.frontmatter import build_frontmatter_markdown, split_frontmatter
from devcouncil.utils.fsio import atomic_write_bytes, atomic_write_json

logger = logging.getLogger(__name__)

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
    # Golden corpora describe foreign stacks but are not part of the repository's
    # implementation. Treating them as stack evidence makes one fixture select
    # unrelated skills for every task in the host repo.
    "testdata", "test_data",
}
_MAX_WALK_FILES = 20_000


class SkillTriggers(BaseModel):
    keywords: list[str] = Field(default_factory=list)
    globs: list[str] = Field(default_factory=list)
    markers: list[str] = Field(default_factory=list)


class Skill(BaseModel):
    name: str
    title: str = ""
    description: str = ""
    always: bool = False
    triggers: SkillTriggers = Field(default_factory=SkillTriggers)
    body: str = ""
    source_path: Path | None = None

    def matches(
        self,
        goal: str,
        repo_files_present: "set[str] | None" = None,
        project_root: Path | None = None,
    ) -> bool:
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
        if project_root is not None:
            for marker in self.triggers.markers:
                if (project_root / marker).exists():
                    return True
        return False

    def relevance_score(
        self,
        goal: str,
        repo_files_present: "set[str] | None" = None,
        project_root: Path | None = None,
    ) -> int:
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
        if project_root is not None:
            score += sum(1 for marker in self.triggers.markers if (project_root / marker).exists())
        return score

    def to_skill_md(self) -> str:
        """Render portable SKILL.md frontmatter and body for all supported hosts."""
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
            markers=list(triggers.get("markers") or []),
        ),
        body=body.strip(),
        source_path=path,
    )


# Repo-local skill locations, scanned in addition to the packaged library so users
# can drop their own skill markdown into a project and have it picked up.
REPO_SKILL_DIRS = (".claude/skills", ".cursor/skills", ".agents/skills", ".devcouncil/skills")

# Default destinations for ``scaffold_skills`` — Claude Code, Cursor, and Codex discover
# skills under these trees. Callers can pass an explicit list to write fewer roots.
DEFAULT_SKILL_DESTINATIONS = (".claude/skills", ".cursor/skills", ".agents/skills")


def _try_skill_from_file(path: Path) -> Skill | None:
    """Parse a markdown file into a Skill in a single read, or None if it isn't a skill.

    A markdown file is a skill only if its frontmatter carries a ``name``; plain docs
    (e.g. a contributor README) are ignored. Reads the file once — previously callers
    read it twice (an ``_is_skill_file`` check followed by a separate parse)."""
    data = _read_scaffold_file(path.resolve(), _SKILL_FILE_LIMIT)
    if data is None:
        raise FileNotFoundError(path)
    text = data.decode("utf-8")
    meta, body = _split_frontmatter(text)
    if not meta.get("name"):
        if path.name == "SKILL.md" or text.startswith("---"):
            raise ValueError(f"{path}: invalid skill frontmatter or missing name")
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


@functools.lru_cache(maxsize=32)
def load_skills(
    library_dir: Path = LIBRARY_DIR,
    project_root: Path | None = None,
    include_okf: bool = True,
) -> list[Skill]:
    """Load skills: the packaged library plus, when ``project_root`` is given, the
    repo's own skills and (when ``include_okf``) skills from ingested OKF documents.
    Repo-local skills override packaged ones with the same name.

    Result is cached per (library_dir, project_root, include_okf) for the lifetime of
    the process, mirroring the repo-basename cache below: a ``dev e2e``/``repair-all``
    run calls ``select_skills`` once per task, and re-reading+parsing the whole skill
    tree (library glob, repo ``SKILL.md`` discovery, OKF markdown) every time is pure
    waste since the on-disk skills don't change during a run. The returned list is
    shared and must be treated read-only by callers (all current callers only iterate
    it); ``clear_skill_caches()`` drops the cache when a refresh is needed.

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
                has_own_triggers = bool(
                    skill.triggers.keywords or skill.triggers.globs or skill.triggers.markers
                )
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
            except Exception as e:
                logger.debug("Failed to load knowledge directory from config, using default: %s", e)
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
    """Drop the cached repo file scans and loaded-skill sets (useful in long-running
    processes/tests). Fully resets module-level skill state so test isolation holds."""
    _basename_cache.clear()
    load_skills.cache_clear()


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
        (s, score) for s in skills if (score := s.relevance_score(goal, repo_files, project_root)) > 0
    ]
    scored.sort(key=lambda item: (not item[0].always, -item[1], item[0].name))
    return [skill for skill, _ in scored]


def skills_for_scaffold(
    goal: str = "",
    project_root: Path | None = None,
    library_dir: Path = LIBRARY_DIR,
) -> list[Skill]:
    """Skills to write under ``.claude/skills`` / ``.cursor/skills``.

    Selection still uses repo context (markers/globs), but packaged library content
    wins over already-scaffolded copies so ``dev integrate … --apply`` can refresh
    stale ``SKILL.md`` bodies.
    """
    selected = select_skills(goal, project_root, library_dir=library_dir)
    library_by_name = {s.name: s for s in load_skills(library_dir, project_root=None)}
    return [library_by_name.get(skill.name, skill) for skill in selected]


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
    deferred (their full text still lives in scaffolded .claude/skills/ and .cursor/skills/).
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


def scaffold_skills(
    project_root: Path,
    skills: list[Skill],
    destinations: tuple[str, ...] | list[str] | None = None,
    *,
    dry_run: bool = False,
) -> list[Path]:
    """Write the given skills under each destination as ``<name>/SKILL.md``.

    Defaults to Claude Code, Cursor, and Codex. Preflight the complete batch,
    serialize installers, and atomically replace only unchanged managed files.
    Unknown or locally modified content is refused. Identical existing files are
    adopted. A dry run returns differing paths without writing or claiming an
    installation succeeded. Repository-local source files remain untouched.
    """
    root = project_root.resolve(strict=True)
    if not root.is_dir():
        raise ValueError(f"{root}: skill destination root is not a directory")
    dest_rels = tuple(destinations) if destinations is not None else DEFAULT_SKILL_DESTINATIONS
    if not dest_rels or len(dest_rels) > 16 or len(skills) > 256:
        raise ValueError("Skill install limit: 1–16 destinations and at most 256 skills")
    rendered: dict[Path, bytes] = {}
    names: set[str] = set()
    for rel in dest_rels:
        _scaffold_path(root, rel)
    for skill in skills:
        if not re.fullmatch(r"[a-z0-9][a-z0-9-]{0,63}", skill.name) or skill.name in {
            "con", "prn", "aux", "nul", *(f"com{i}" for i in range(1, 10)), *(f"lpt{i}" for i in range(1, 10)),
        }:
            raise ValueError(f"Invalid skill name: {skill.name!r}")
        if skill.name in names:
            raise ValueError(f"Conflicting duplicate skill name: {skill.name}")
        names.add(skill.name)
        content = skill.to_skill_md().encode("utf-8")
        if len(content) > _SKILL_FILE_LIMIT:
            raise ValueError(f"{skill.name}: skill exceeds byte limit")
        src = skill.source_path.resolve() if skill.source_path is not None else None
        for rel in dest_rels:
            skills_root = root / rel
            # Don't re-materialize a skill that already lives in this destination.
            # Packaged library files inside a monorepo (e.g. src/.../skills/library/)
            # still scaffold — only skip when the source IS this destination root.
            if src is not None:
                try:
                    src.relative_to(skills_root.resolve())
                    continue
                except (ValueError, OSError):
                    pass
            target = _scaffold_path(root, f"{rel}/{skill.name}/SKILL.md")
            rendered[target] = content
    if sum(map(len, rendered.values())) > 8 * 1024 * 1024:
        raise ValueError("Skill install exceeds 8 MiB batch limit")
    # Creating .devcouncil here would activate marker-based DevMap selection
    # on the next invocation: installation itself must not widen the intake.
    receipt = _scaffold_path(root, ".devcouncil-skills.json")
    # Validate before creating even the lock directory. Repeat under the lock:
    # another installer may finish between the first plan and acquisition.
    plan, hashes = _scaffold_plan(root, rendered, receipt, check_only=dry_run)
    if dry_run or not rendered:
        return [target for target, _, _ in plan]
    lock = root / ".devcouncil-skills.lock"
    deadline = time.monotonic() + 5
    while True:
        try:
            lock.mkdir()
            break
        except FileExistsError:
            if time.monotonic() >= deadline:
                raise TimeoutError(f"Skill installer is busy: {lock}") from None
            time.sleep(0.01)
    try:
        plan, hashes = _scaffold_plan(root, rendered, receipt, check_only=False)
        for target, content, previous in plan:
            _scaffold_path(root, target.relative_to(root).as_posix())
            if _read_scaffold_file(target, _SKILL_FILE_LIMIT) != previous:
                raise ValueError(f"{target}: modified while installing skills")
            target.parent.mkdir(parents=True, exist_ok=True)
            atomic_write_bytes(target, content)
        document = {"schema": 1, "files": hashes}
        prior = _read_scaffold_file(receipt, _SKILL_RECEIPT_LIMIT)
        if prior is None or json.loads(prior) != document:
            receipt.parent.mkdir(parents=True, exist_ok=True)
            atomic_write_json(receipt, document, sort_keys=True)
        clear_skill_caches()
        return [target for target, _, _ in plan]
    finally:
        lock.rmdir()


_SKILL_FILE_LIMIT = 256 * 1024
_SKILL_RECEIPT_LIMIT = 1024 * 1024


def _scaffold_path(root: Path, relative: str) -> Path:
    """Validate portable relative paths and refuse symlinks below the root."""
    if not relative or "\\" in relative or ":" in relative or relative.startswith("/"):
        raise ValueError(f"Invalid skill destination: {relative!r}")
    parts = relative.split("/")
    if any(part in ("", ".", "..") for part in parts):
        raise ValueError(f"Invalid skill destination: {relative!r}")
    path = root
    for index, part in enumerate(parts):
        path /= part
        try:
            mode = path.lstat().st_mode
        except FileNotFoundError:
            continue
        if stat.S_ISLNK(mode):
            raise ValueError(f"{path}: refusing symlink in skill destination")
        if index < len(parts) - 1 and not stat.S_ISDIR(mode):
            raise ValueError(f"{path}: skill parent is not a directory")
    return path


def _read_scaffold_file(path: Path, limit: int) -> bytes | None:
    try:
        fd = os.open(path, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_NONBLOCK", 0))
    except FileNotFoundError:
        return None
    with os.fdopen(fd, "rb") as file:
        info = os.fstat(file.fileno())
        if not stat.S_ISREG(info.st_mode) or info.st_size > limit:
            raise ValueError(f"{path}: not a regular file within the {limit} byte limit")
        data = file.read(limit + 1)
        if len(data) > limit:
            raise ValueError(f"{path}: file grew beyond byte limit")
        return data


def _scaffold_plan(
    root: Path, rendered: dict[Path, bytes], receipt: Path, *, check_only: bool,
) -> tuple[list[tuple[Path, bytes, bytes | None]], dict[str, str]]:
    _scaffold_path(root, receipt.relative_to(root).as_posix())
    raw = _read_scaffold_file(receipt, _SKILL_RECEIPT_LIMIT)
    try:
        saved = json.loads(raw) if raw is not None else {"schema": 1, "files": {}}
    except (ValueError, UnicodeError) as error:
        raise ValueError(f"{receipt}: invalid skill installation receipt") from error
    if not isinstance(saved, dict) or type(saved.get("schema")) is not int or saved.get("schema") != 1 or not isinstance(saved.get("files"), dict):
        raise ValueError(f"{receipt}: invalid skill installation receipt")
    hashes = saved["files"].copy()
    if len(hashes) > 4096 or any(not isinstance(k, str) or not isinstance(v, str) or not re.fullmatch(r"[a-f0-9]{64}", v) for k, v in hashes.items()):
        raise ValueError(f"{receipt}: invalid or oversized skill installation receipt")
    plan = []
    for target, content in rendered.items():
        key = target.relative_to(root).as_posix()
        _scaffold_path(root, key)
        current = _read_scaffold_file(target, _SKILL_FILE_LIMIT)
        if current != content:
            if not check_only and current is not None and hashes.get(key) != hashlib.sha256(current).hexdigest():
                raise ValueError(f"{target}: unmanaged or locally modified skill; preserve or move it before installing")
            plan.append((target, content, current))
        hashes[key] = hashlib.sha256(content).hexdigest()
    if len(hashes) > 4096 or len(json.dumps({"schema": 1, "files": hashes}).encode("utf-8")) > _SKILL_RECEIPT_LIMIT:
        raise ValueError(f"{receipt}: skill installation receipt exceeds limit")
    return plan, hashes
