import ast
import logging
import re
from pathlib import Path
from typing import List

from devcouncil.domain.requirement import Requirement
from devcouncil.domain.task import Task
from devcouncil.integrations.code_review_graph import CodeReviewGraphAdapter
from devcouncil.utils.json_persist import read_json

logger = logging.getLogger(__name__)

# Context budget for injected file bodies. Skills are bounded separately; these keep
# a task that touches many/large files from blowing up the prompt. Lowest-priority
# content (later files) is truncated/omitted first, with an explicit marker.
MAX_FILE_CONTEXT_CHARS = 24_000   # total across all injected file bodies
MAX_PER_FILE_CHARS = 8_000        # cap on any single file body
MAX_SYMBOLS_PER_FILE = 40
# Global ceiling on the assembled prompt. The core (goal/requirements/scope/instructions)
# is always kept; optional context sections are fitted in priority order and the
# lowest-priority ones are dropped (with a marker) if the whole prompt would exceed this.
MAX_PROMPT_CHARS = 60_000

# Appended to the instructions when the task classifies as HARD (see
# devcouncil.verification.difficulty). Tells the executor up front that the
# anti-laziness gates will block, so it does not discover strict mode via a
# failed verify + repair loop. Kept compact — it rides in the never-dropped core.
_HARD_TASK_RIGOR_SECTION = """
## Rigor (this task is classified HARD — verification is strict)
- No stubs, placeholders, or TODO/FIXME markers in added code: the verifier scans the
  diff and BLOCKS on them. Intentional scaffolding requires the task description to
  mention "scaffolding" AND the line to carry `devcouncil: allow-stub` (declared stubs
  are surfaced for human review even when suppressed).
- Every acceptance criterion needs a passing behavioral check; changed lines must be
  exercised by the tests you run (diff coverage is enforced).
- Do not claim completion without running the expected tests and seeing them pass.
- Never delete, skip, or weaken a test to get to green.
- If part of the task is genuinely infeasible, say so explicitly and state what is
  missing instead of stubbing around it.
"""

# Rough chars-per-token for English/code; deliberately conservative so the derived
# budget under-fills the window rather than over-fills it.
_CHARS_PER_TOKEN = 4
# Tokens reserved inside the model's context window for the model's own completion plus
# the schema/JSON instructions the router appends to each call.
_RESERVED_COMPLETION_TOKENS = 1536
# Never shrink the budget below this; a window this small can't run the council anyway,
# and clamping here keeps the core prompt intact instead of pathologically truncating.
_MIN_PROMPT_CHARS = 8_000


def _local_context_window_budget(project_root: Path, cfg=None) -> int | None:
    """Char budget derived from a constrained local context window, or ``None``.

    When the run targets the local Ollama provider, the server silently truncates
    anything past its context window — so a char-only budget that ignores it lets the
    carefully-assembled prompt get cut off mid-stream. Returns a char budget that fits
    the window the provider will actually request (``OLLAMA_NUM_CTX``, else the
    provider's raised ``DEFAULT_NUM_CTX``), minus completion headroom, so
    :meth:`build_task_prompt` can cap itself. Returns ``None`` for cloud providers and
    for an explicit ``OLLAMA_NUM_CTX=0`` opt-out (server-default window, unknowable
    here), leaving the default behavior untouched. Best-effort: any error degrades
    to ``None``.

    ``cfg`` may be a pre-loaded config (loaded once per task by ``build_task_prompt``); when
    ``None`` it is loaded here so other callers keep working."""
    try:
        if cfg is None:
            from devcouncil.app.config import load_config

            cfg = load_config(project_root)
        provider = cfg.models.provider.strip().lower()
        if provider not in {"ollama", "ollama-local", "ollama_local"}:
            return None
    except Exception:
        return None

    from devcouncil.llm.provider import OllamaProvider

    num_ctx = OllamaProvider._resolve_num_ctx()
    if not num_ctx:
        # Explicit OLLAMA_NUM_CTX=0 opt-out: the server-default window applies and
        # DevCouncil cannot know it; don't guess a cap here.
        return None
    usable_tokens = num_ctx - _RESERVED_COMPLETION_TOKENS
    if usable_tokens <= 0:
        return _MIN_PROMPT_CHARS
    return max(_MIN_PROMPT_CHARS, usable_tokens * _CHARS_PER_TOKEN)

_LANG_BY_EXT = {
    ".py": "python", ".js": "javascript", ".jsx": "jsx", ".ts": "typescript",
    ".tsx": "tsx", ".go": "go", ".rs": "rust", ".java": "java", ".kt": "kotlin",
    ".swift": "swift", ".rb": "ruby", ".cs": "csharp", ".cpp": "cpp", ".c": "c",
    ".sh": "bash", ".yml": "yaml", ".yaml": "yaml", ".json": "json", ".toml": "toml",
    ".md": "markdown", ".sql": "sql",
}


class PromptBuilder:
    def __init__(self, project_root: Path = Path(".")):
        self.project_root = project_root
        self._file_text_cache: dict[str, str] = {}

    @staticmethod
    def _lang_for(path: str) -> str:
        return _LANG_BY_EXT.get(Path(path).suffix.lower(), "")

    def _symbol_outline(self, path: str, text: str) -> List[str]:
        """Cheap top-level symbol index (signatures + line numbers) so the agent edits
        in place and uses correct names/arities instead of guessing or duplicating.

        Python uses stdlib ``ast`` (method signatures, async/@property/@staticmethod
        markers under each class). Other languages (ts/tsx/js/jsx/go/rs/java) use bounded
        regex over exported/public declarations. No tree-sitter, no model call. Never
        raises; honors the per-file symbol cap.

        Results are memoized per ``build_task_prompt`` run via ``self._outline_cache`` so
        the same file is parsed once even though both the planned-files section and the
        call-sites section need its outline. The key includes the ``text`` itself (not just
        ``path``) so that if the file's content differs between the two reads, the outline
        is recomputed from the current text rather than served stale — and using the text
        directly (rather than its hash) means there is no collision risk."""
        cache: dict[tuple[str, str], List[str]] | None = getattr(self, "_outline_cache", None)
        key = (path, text)
        if cache is not None and key in cache:
            return cache[key]
        if path.endswith(".py"):
            result = self._python_symbol_outline(text)
        else:
            result = self._regex_symbol_outline(path, text)
        if cache is not None:
            cache[key] = result
        return result

    def _python_symbol_outline(self, text: str) -> List[str]:
        try:
            tree = ast.parse(text)
        except Exception:
            return []

        def _decorator_markers(node) -> str:
            names: set[str] = set()
            for dec in getattr(node, "decorator_list", []):
                target = dec.func if isinstance(dec, ast.Call) else dec
                if isinstance(target, ast.Attribute):
                    names.add(target.attr)
                elif isinstance(target, ast.Name):
                    names.add(target.id)
            marks = [m for m in ("property", "staticmethod", "classmethod") if m in names]
            return (" @" + " @".join(marks)) if marks else ""

        def _func_sig(node) -> str:
            args = ", ".join(a.arg for a in node.args.args)
            kw = "async " if isinstance(node, ast.AsyncFunctionDef) else ""
            return f"{kw}def {node.name}({args}) L{node.lineno}{_decorator_markers(node)}"

        out: List[str] = []
        for node in tree.body:
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
                out.append(_func_sig(node))
            elif isinstance(node, ast.ClassDef):
                out.append(f"class {node.name} L{node.lineno}")
                for n in node.body:
                    if isinstance(n, (ast.FunctionDef, ast.AsyncFunctionDef)):
                        out.append("  " + _func_sig(n))
                        if len(out) >= MAX_SYMBOLS_PER_FILE:
                            break
            if len(out) >= MAX_SYMBOLS_PER_FILE:
                break
        return out[:MAX_SYMBOLS_PER_FILE]

    # Bounded per-language regexes over exported/public top-level declarations. Each
    # capture group 2 is the symbol name; group 1 (when present) is the keyword/kind.
    _OUTLINE_PATTERNS: dict[str, list[tuple[str, re.Pattern[str]]]] = {}

    def _regex_symbol_outline(self, path: str, text: str) -> List[str]:
        suffix = Path(path).suffix.lower()
        lang = {
            ".ts": "ts", ".tsx": "ts", ".js": "js", ".jsx": "js",
            ".go": "go", ".rs": "rs", ".java": "java",
        }.get(suffix)
        if not lang:
            return []
        patterns = self._regex_outline_patterns().get(lang)
        if not patterns:
            return []
        out: List[str] = []
        for lineno, raw in enumerate(text.splitlines(), start=1):
            for label, pattern in patterns:
                m = pattern.match(raw)
                if not m:
                    continue
                name = m.group("name")
                out.append(f"{label} {name} L{lineno}")
                break
            if len(out) >= MAX_SYMBOLS_PER_FILE:
                break
        return out[:MAX_SYMBOLS_PER_FILE]

    @classmethod
    def _regex_outline_patterns(cls):
        if cls._OUTLINE_PATTERNS:
            return cls._OUTLINE_PATTERNS
        n = r"(?P<name>[A-Za-z_$][\w$]*)"
        ts = [
            ("export class", re.compile(r"^\s*export\s+(?:default\s+)?(?:abstract\s+)?class\s+" + n)),
            ("export interface", re.compile(r"^\s*export\s+interface\s+" + n)),
            ("export type", re.compile(r"^\s*export\s+type\s+" + n)),
            ("export enum", re.compile(r"^\s*export\s+(?:const\s+)?enum\s+" + n)),
            ("export function", re.compile(r"^\s*export\s+(?:default\s+)?(?:async\s+)?function\s*\*?\s+" + n)),
            ("export const", re.compile(r"^\s*export\s+(?:const|let|var)\s+" + n)),
        ]
        js = [
            ("export class", re.compile(r"^\s*export\s+(?:default\s+)?class\s+" + n)),
            ("export function", re.compile(r"^\s*export\s+(?:default\s+)?(?:async\s+)?function\s*\*?\s+" + n)),
            ("export const", re.compile(r"^\s*export\s+(?:const|let|var)\s+" + n)),
            ("class", re.compile(r"^\s*class\s+" + n)),
            ("function", re.compile(r"^\s*(?:async\s+)?function\s*\*?\s+" + n)),
        ]
        go = [
            # Go exports = capitalized identifiers; func may carry a receiver.
            ("func", re.compile(r"^func\s+(?:\([^)]*\)\s*)?(?P<name>[A-Z]\w*)\s*[\(\[]")),
            ("type", re.compile(r"^type\s+(?P<name>[A-Z]\w*)\s+")),
        ]
        rs = [
            ("pub fn", re.compile(r"^\s*pub(?:\([^)]*\))?\s+(?:async\s+)?(?:unsafe\s+)?fn\s+" + n)),
            ("pub struct", re.compile(r"^\s*pub(?:\([^)]*\))?\s+struct\s+" + n)),
            ("pub enum", re.compile(r"^\s*pub(?:\([^)]*\))?\s+enum\s+" + n)),
            ("pub trait", re.compile(r"^\s*pub(?:\([^)]*\))?\s+trait\s+" + n)),
        ]
        java = [
            ("class", re.compile(r"^\s*(?:public|protected|private)?\s*(?:abstract\s+|final\s+)?class\s+" + n)),
            ("interface", re.compile(r"^\s*(?:public|protected|private)?\s*interface\s+" + n)),
            ("enum", re.compile(r"^\s*(?:public|protected|private)?\s*enum\s+" + n)),
            # public/protected methods: <modifiers> <return-type> name(
            ("method", re.compile(
                r"^\s*(?:public|protected)\s+(?:static\s+|final\s+|abstract\s+|synchronized\s+|native\s+)*"
                r"[\w<>\[\],.?\s]+?\s+(?P<name>[A-Za-z_]\w*)\s*\(")),
        ]
        cls._OUTLINE_PATTERNS = {"ts": ts, "js": js, "go": go, "rs": rs, "java": java}
        return cls._OUTLINE_PATTERNS

    def _planned_files_section(self, task: Task) -> str:
        """Inject the current (redacted) contents of the task's planned files.

        The capable production agents previously received file PATHS only and had to
        rediscover every file and guess signatures — a leading cause of wrong-arity /
        wrong-import edits that fail verification. Reading the real contents here lifts
        one-shot success. Bounded by a total + per-file char budget (see constants);
        new files are shown as headers only."""
        from devcouncil.utils.redaction import redact_string

        self._file_text_cache.clear()
        blocks: List[str] = []
        budget = MAX_FILE_CONTEXT_CHARS
        omitted = 0
        for pf in task.planned_files:
            label = pf.allowed_change
            file_path = self.project_root / pf.path
            if not (file_path.exists() and file_path.is_file()):
                blocks.append(f"### `{pf.path}` [{label}] — new file (does not exist yet)\n")
                continue
            if budget <= 0:
                omitted += 1
                continue
            try:
                raw = file_path.read_text(encoding="utf-8", errors="replace")
            except Exception:
                blocks.append(f"### `{pf.path}` [{label}] — [error reading file]\n")
                continue
            self._file_text_cache[pf.path.replace("\\", "/")] = raw
            content = redact_string(raw)
            cap = min(MAX_PER_FILE_CHARS, budget)
            truncated = len(content) > cap
            if truncated:
                content = content[:cap]
            budget -= len(content)
            symbols = self._symbol_outline(pf.path, raw)
            block = f"### `{pf.path}` [{label}]\n"
            if symbols:
                block += "Symbols: " + "; ".join(symbols) + "\n"
            block += f"```{self._lang_for(pf.path)}\n{content}\n```"
            if truncated:
                block += f"\n_[truncated to {cap} chars — open the file for the rest]_"
            blocks.append(block + "\n")
        if omitted:
            blocks.append(f"_[{omitted} more planned file(s) omitted to fit the context budget — open them directly]_\n")
        if not blocks:
            return ""
        return (
            "\n## Current file contents (read before editing)\n"
            "_Edit these in place; redacted secrets shown as ***._\n\n"
            + "\n".join(blocks)
        )

    def _load_prompt_enhancement(self):
        """Latest run's prompt-enhancement (None if absent/unreadable). Best-effort: a
        failure here must never break prompt construction."""
        try:
            from devcouncil.planning.prompt_enhancer_service import load_latest_prompt_enhancement
            return load_latest_prompt_enhancement(self.project_root)
        except Exception:
            return None

    def _load_repo_map(self) -> dict | None:
        """Parse ``.devcouncil/repo_map.json`` once per prompt (None if absent/unreadable)."""
        map_path = self.project_root / ".devcouncil" / "repo_map.json"
        if not map_path.exists():
            return None
        try:
            data = read_json(map_path)
            return data if isinstance(data, dict) else None
        except Exception:
            return None

    def _repo_map_stale(self, data: dict | None) -> bool:
        """Whether the loaded repo map is behind the repo's current state, so its
        structural context / dependents may be wrong. Best-effort; never raises."""
        if not data:
            return False
        try:
            from devcouncil.indexing.repo_mapper import RepoMapper

            return RepoMapper(self.project_root).map_is_stale(data)
        except Exception:
            return False

    _STALE_MAP_NOTE = (
        "_⚠ The repo map is behind the current code (run `dev map` to refresh); "
        "treat the structure below as approximate._\n"
    )

    _NO_MAP_NOTE = (
        "_(no repo map; run `dev map` for structural orientation)_\n"
    )

    def _repo_map_section(self, planned_paths: List[str], data: dict | None = None) -> str:
        """Structural orientation from ``.devcouncil/repo_map.json`` — the fallback used
        when the optional code-review-graph CLI is absent (the common case). Surfaces the
        subsystem(s) the planned files live in, their key files, neighbors, and flow, so
        an agent in an unfamiliar repo knows where it is before editing."""
        if data is None:
            data = self._load_repo_map()
        if not data:
            return ""
        subsystems = data.get("subsystems") or []
        files_by_path = {
            f.get("path"): f for f in (data.get("files") or []) if isinstance(f, dict)
        }
        norm = [p.replace("\\", "/") for p in planned_paths]
        relevant = [
            s for s in subsystems
            if isinstance(s, dict) and s.get("area")
            and any(p == s["area"] or p.startswith(s["area"] + "/") for p in norm)
        ]
        if not relevant:
            return ""
        lines = ["## Repo map (structural context)"]
        for s in relevant[:3]:
            lines.append(f"\n**{s.get('area')}** — {s.get('summary', '')}".rstrip())
            critical = [c for c in (s.get("critical_files") or []) if c not in norm][:6]
            if critical:
                lines.append("Key files:")
                for c in critical:
                    summary = (files_by_path.get(c) or {}).get("summary", "")
                    lines.append(f"- `{c}`" + (f" — {summary}" if summary else ""))
            neighbors = s.get("neighbors") or []
            if neighbors:
                lines.append("Neighboring subsystems: " + ", ".join(f"`{n}`" for n in neighbors[:6]))
            handoffs = s.get("handoff_paths") or []
            if handoffs:
                lines.append("Cross-subsystem flow: " + "; ".join(handoffs[:4]))
        return "\n".join(lines).strip() + "\n"

    def _skills_section(self, task: Task) -> str:
        """The full engineering-skill intake that applies to this task.

        Selection is codebase-aware (task goal keywords + the repo's own files, so an
        Android repo pulls the android skill via build.gradle even when the task text
        doesn't say "android"). The full skill text is injected inline — the senior-dev
        intake (current libraries, deprecations to avoid, the right build/test CLI
        commands) goes straight to the coding agent rather than relying on it to open
        scaffolded files. Never raises — a skills failure must not break prompt building.
        """
        try:
            from devcouncil.skills.registry import bound_skills, render_preamble, select_skills

            goal = f"{task.title}\n{task.description}"
            selected = select_skills(goal=goal, project_root=self.project_root)
            # Bound how much skill text rides inline so a repo that matches many skills
            # can't blow up the task prompt; deferred skills are still on disk.
            inline, deferred = bound_skills(selected)
            preamble = render_preamble(inline)
        except Exception:
            return ""
        if not selected or not preamble:
            return ""

        names = ", ".join(skill.name for skill in selected)
        section = (
            "\n## Engineering skills (apply before and while coding)\n"
            f"_Applicable skills: {names}. Follow this current-practice intake; "
            "don't rely on stale training data._\n\n"
            f"{preamble}\n"
        )
        if deferred:
            section += "\n_Also applicable (read the full text in `.claude/skills/<name>/SKILL.md`):_\n"
            for skill in deferred:
                blurb = skill.description or skill.title
                suffix = f" — {blurb}" if blurb else ""
                section += f"- `{skill.name}`{suffix}\n"
        return section

    def _knowledge_sections(self, task: Task, cfg=None) -> tuple[str, str]:
        """Selected design-system and OKF knowledge context for this task.

        Returns ``(design_text, knowledge_text)`` — either may be empty. Sourced from
        ``.devcouncil/knowledge/{design,okf}`` via the same trigger-based selection the
        skills library uses: a design system is always-on (a UI agent must honor it),
        OKF knowledge fires on goal keywords / document tags. Bounded by config char
        budgets. Never raises — a knowledge failure must not break prompt building.

        ``cfg`` may be a pre-loaded config (loaded once per task by ``build_task_prompt``);
        when ``None`` it is loaded here so other callers keep working."""
        try:
            from devcouncil.knowledge.sources import (
                render_knowledge_preamble,
                select_knowledge_sources,
            )

            if cfg is None:
                from devcouncil.app.config import load_config

                cfg = load_config(self.project_root)
            kcfg = cfg.knowledge
            if not kcfg.enabled:
                return "", ""
            goal = f"{task.title}\n{task.description}"
            sources = select_knowledge_sources(
                goal=goal, project_root=self.project_root,
                directory=kcfg.directory, design_always=kcfg.design_always,
            )
            design_text = render_knowledge_preamble(sources, max_chars=kcfg.design_max_chars, kind="design")
            knowledge_text = render_knowledge_preamble(sources, max_chars=kcfg.okf_max_chars, kind="okf")
        except Exception:
            return "", ""

        design_block = ""
        if design_text:
            design_block = (
                "\n## Design system (honor these tokens and rules)\n"
                "_The project's design.md. Use these tokens/components; don't invent ad-hoc styles._\n\n"
                f"{design_text}\n"
            )
        knowledge_block = ""
        if knowledge_text:
            knowledge_block = (
                "\n## Project knowledge (Open Knowledge Format)\n"
                "_Curated org/domain knowledge relevant to this task. Ground your work in it._\n\n"
                f"{knowledge_text}\n"
            )
        return design_block, knowledge_block

    # Impact block bounds: this is a short, always-on summary — keep it tight so it can
    # never crowd out the detailed file bodies it complements.
    _IMPACT_MAX_FILES = 8
    _IMPACT_MAX_DEPS = 5
    _IMPACT_MAX_NEIGHBORS = 4

    def _impact_section(self, task: Task, data: dict | None) -> str:
        """A short "changing X touches Y" block sourced purely from ``repo_map.json``.

        Unlike the optional code-review-graph structural section (which only appears when
        that CLI is installed and enabled), this is ALWAYS emitted whenever the repo map
        carries dependents or subsystem neighbors — so an agent always sees the blast
        radius of its edits (who imports each changed file, and which neighboring
        subsystems the change reaches) even on the common keyless path. Complements the
        detailed ``_dependents_section``/``_call_sites_section`` with a one-line-per-file
        orientation. When a code graph exists, also appends symbol-level inbound blast
        (depth 1) for planned modify paths. Bounded and best-effort; never raises."""
        if not data:
            return ""
        from devcouncil.indexing.subsystem_map import (
            area_for_path,
            dependents_of,
            neighbors_for_area,
        )

        lines: List[str] = []
        for pf in task.planned_files[: self._IMPACT_MAX_FILES]:
            path = pf.path.replace("\\", "/")
            is_new = pf.allowed_change == "create"
            importers = [] if is_new else dependents_of(path, data)
            area = area_for_path(path, data)
            neighbors = neighbors_for_area(area, data)
            if not importers and not neighbors and not area and not is_new:
                continue
            parts: List[str] = []
            if importers:
                shown = importers[: self._IMPACT_MAX_DEPS]
                more = f" (+{len(importers) - len(shown)})" if len(importers) > len(shown) else ""
                parts.append(
                    f"imported by {len(importers)} file(s): "
                    + ", ".join(f"`{p}`" for p in shown) + more
                )
            elif is_new:
                parts.append("new file — plan its importer before finishing")
            if area:
                neighbor_txt = ""
                if neighbors:
                    shown_n = neighbors[: self._IMPACT_MAX_NEIGHBORS]
                    neighbor_txt = "; neighbors: " + ", ".join(f"`{n}`" for n in shown_n)
                parts.append(f"subsystem `{area}`{neighbor_txt}")
            if parts:
                lines.append(f"- `{path}` → " + "; ".join(parts))

        graph_lines = self._graph_impact_lines(task)
        if not lines and not graph_lines:
            return ""
        body = "\n".join(lines) if lines else ""
        if graph_lines:
            extra = "\n".join(graph_lines)
            body = f"{body}\n{extra}" if body else extra
        return (
            "\n## Impact (changing X touches Y)\n"
            "_From the repo map — keep these call sites and neighboring subsystems working, "
            "or update them in scope._\n"
            + body
            + "\n"
        )

    def _graph_impact_lines(self, task: Task) -> List[str]:
        """Optional symbol-level inbound blast (depth 1) from the code graph."""
        try:
            from devcouncil.indexing.graph.build import load_code_graph
            from devcouncil.indexing.graph.intel import diff_impact

            graph = load_code_graph(self.project_root)
            if graph is None:
                return []
            paths = [
                pf.path.replace("\\", "/")
                for pf in task.planned_files[: self._IMPACT_MAX_FILES]
                if pf.allowed_change != "create"
            ]
            if not paths:
                return []
            result = diff_impact(
                self.project_root, graph, paths=paths, use_diff=False, max_depth=1
            )
            out: List[str] = []
            for item in result.get("paths") or []:
                layers = (item.get("blast") or {}).get("layers") or []
                depth1 = next((L for L in layers if L.get("depth") == 1), None)
                nodes = (depth1 or {}).get("nodes") or []
                if not nodes:
                    continue
                shown = nodes[: self._IMPACT_MAX_DEPS]
                more = f" (+{len(nodes) - len(shown)})" if len(nodes) > len(shown) else ""
                out.append(
                    f"- `{item['path']}` symbol callers (depth 1): "
                    + ", ".join(f"`{n}`" for n in shown)
                    + more
                )
            return out
        except Exception:
            logger.debug("graph impact lines skipped", exc_info=True)
            return []

    def _liveness_debt_section(self, task: Task, data: dict | None) -> str:
        """Surface map unwired/dead-symbol candidates that overlap the task's subsystems."""
        if not data:
            return ""
        try:
            from devcouncil.indexing.subsystem_map import (
                area_for_path,
                areas_touched,
                dead_symbol_candidates_of,
                unreachable_of,
                unwired_candidates_of,
            )

            planned = [pf.path.replace("\\", "/") for pf in task.planned_files]
            task_areas = set(areas_touched(planned, data))
            if not task_areas:
                return ""

            unwired_hits: list[str] = []
            for cand in unwired_candidates_of(data)[:40]:
                area = area_for_path(cand, data)
                if area and area in task_areas:
                    unwired_hits.append(cand)
                if len(unwired_hits) >= 6:
                    break

            unreachable_hits: list[str] = []
            for cand in unreachable_of(data)[:40]:
                area = area_for_path(cand, data)
                if area and area in task_areas:
                    unreachable_hits.append(cand)
                if len(unreachable_hits) >= 6:
                    break

            dead_hits: list[str] = []
            for cand in dead_symbol_candidates_of(data)[:40]:
                path = cand.split(":", 1)[0]
                area = area_for_path(path, data)
                if area and area in task_areas:
                    dead_hits.append(cand)
                if len(dead_hits) >= 6:
                    break

            if not unwired_hits and not unreachable_hits and not dead_hits:
                return ""
            lines = [
                "\n## Nearby liveness debt (from repo map)",
                "_Existing unwired / unreachable files and unused symbols near your "
                "subsystems — do not add to this debt; wire what you create._",
                "_Verify same-task island rule: a new file imported only by other files "
                "added in this task still needs a pre-existing non-test caller._",
            ]
            for p in unwired_hits:
                lines.append(f"- unwired: `{p}`")
            for p in unreachable_hits:
                lines.append(f"- unreachable: `{p}`")
            for p in dead_hits:
                lines.append(f"- dead symbol: `{p}`")
            return "\n".join(lines) + "\n"
        except Exception:
            logger.debug("liveness debt section skipped", exc_info=True)
            return ""

    def _dependents_section(self, task: Task, data: dict | None) -> str:
        """List, per planned file the agent will change, the files that import it — the
        blast radius. Sourced from repo_map.json's precomputed reverse-import index, so
        the agent updates or preserves call sites instead of silently breaking them."""
        dependents = (data or {}).get("dependents") or {}
        if not isinstance(dependents, dict) or not dependents:
            return ""
        lines: List[str] = []
        for pf in task.planned_files:
            # New files have no dependents yet; only existing code carries blast radius.
            if pf.allowed_change == "create":
                continue
            # Normalize the planned path to posix before the lookup — the map keys are
            # always posix, so a backslash planned path on Windows would otherwise miss
            # and silently drop the whole blast-radius entry (see _repo_map_section).
            importers = dependents.get(pf.path.replace("\\", "/")) or []
            if not importers:
                continue
            shown = importers[:8]
            more = f" (+{len(importers) - len(shown)} more)" if len(importers) > len(shown) else ""
            lines.append(f"- `{pf.path}` is imported by: " + ", ".join(f"`{p}`" for p in shown) + more)
        if not lines:
            return ""
        return (
            "\n## Dependents (blast radius)\n"
            "_These files import the files you're changing — keep their call sites working, "
            "or update them in scope._\n"
            + "\n".join(lines)
            + "\n"
        )

    # Call-sites block bounds: keep it tight so this lowest-priority context can't crowd
    # out the file bodies / dependents it complements.
    _CALL_SITES_MAX_DEP_FILES = 3      # dependent files grepped per changed file
    _CALL_SITES_MAX_SYMBOLS = 6        # exported symbols searched per changed file
    _CALL_SITES_MAX_LINES_PER_FILE = 3  # referencing lines emitted per dependent file
    _CALL_SITES_MAX_TOTAL = 24         # hard cap on emitted file:line rows
    _CALL_SITES_LINE_CHARS = 160       # truncate a long using line

    def _exported_symbol_names(self, path: str, text: str) -> List[str]:
        """Top-level symbol names from a file's outline (no signatures), used to grep
        dependents for referencing lines."""
        names: List[str] = []
        for entry in self._symbol_outline(path, text):
            stripped = entry.strip()
            # Outline rows look like "def name(args) L1", "class Name L5",
            # "export function Name L3", "  async def m(...) Lx" — pull the identifier
            # that precedes the first "(" or " L".
            head = stripped.split(" L")[0]
            head = head.split("(")[0].strip()
            ident = head.split()[-1] if head.split() else ""
            ident = ident.strip(":")
            if ident and ident.isidentifier() and ident not in names:
                names.append(ident)
            if len(names) >= self._CALL_SITES_MAX_SYMBOLS:
                break
        return names

    def _call_sites_section(self, task: Task, data: dict | None) -> str:
        """Lowest-priority context: for each changed file, show where its exported symbols
        are actually used in the top dependent files (file:line + the using line). Helps
        the agent update call sites in scope. Tightly bounded; never raises."""
        dependents = (data or {}).get("dependents") or {}
        if not isinstance(dependents, dict) or not dependents:
            return ""
        lines: List[str] = []
        emitted = 0
        for pf in task.planned_files:
            if pf.allowed_change == "create" or emitted >= self._CALL_SITES_MAX_TOTAL:
                continue
            key = pf.path.replace("\\", "/")
            importers = dependents.get(key) or []
            if not importers:
                continue
            src_path = self.project_root / pf.path
            key = pf.path.replace("\\", "/")
            src_text = self._file_text_cache.get(key)
            if src_text is None:
                try:
                    src_text = src_path.read_text(encoding="utf-8", errors="replace")
                except Exception:
                    continue
            symbols = self._exported_symbol_names(pf.path, src_text)
            if not symbols:
                continue
            file_rows: List[str] = []
            for importer in importers[: self._CALL_SITES_MAX_DEP_FILES]:
                if emitted >= self._CALL_SITES_MAX_TOTAL:
                    break
                importer_key = importer.replace("\\", "/")
                dep_text = self._file_text_cache.get(importer_key)
                if dep_text is None:
                    try:
                        dep_text = (self.project_root / importer).read_text(encoding="utf-8", errors="replace")
                    except Exception:
                        continue
                hits = 0
                for lineno, raw in enumerate(dep_text.splitlines(), start=1):
                    if hits >= self._CALL_SITES_MAX_LINES_PER_FILE or emitted >= self._CALL_SITES_MAX_TOTAL:
                        break
                    if any(self._references_symbol(raw, sym) for sym in symbols):
                        snippet = raw.strip()[: self._CALL_SITES_LINE_CHARS]
                        file_rows.append(f"  - `{importer}:{lineno}` — `{snippet}`")
                        hits += 1
                        emitted += 1
            if file_rows:
                lines.append(f"- `{pf.path}` (uses of {', '.join(f'`{s}`' for s in symbols)}):")
                lines.extend(file_rows)
        if not lines:
            return ""
        return (
            "\n## Call sites (where your symbols are used)\n"
            "_Referencing lines in dependent files — update these if you change a signature._\n"
            + "\n".join(lines)
            + "\n"
        )

    @staticmethod
    def _references_symbol(line: str, symbol: str) -> bool:
        """Whole-word match of ``symbol`` in ``line``. Cheap; avoids matching substrings
        of longer identifiers."""
        idx = line.find(symbol)
        if idx < 0:
            return False
        before = line[idx - 1] if idx > 0 else ""
        after = line[idx + len(symbol)] if idx + len(symbol) < len(line) else ""
        return not (before.isalnum() or before == "_") and not (after.isalnum() or after == "_")

    # Bound the dependency-risk block so this low-priority, opt-in context can't
    # crowd out file bodies / dependents.
    _DEP_RISKS_MAX = 12

    def _dependency_risks_section(self, data: dict | None) -> str:
        """Surface dependency vulnerabilities recorded in repo_map.json (opt-in SCA).

        Lowest-priority, optional context: warns an agent that may bump a vulnerable
        dependency. Absent unless `dev map` was run with SCA enabled. Never raises."""
        risks = (data or {}).get("dependency_risks") or []
        if not isinstance(risks, list) or not risks:
            return ""
        lines: List[str] = []
        for risk in risks[: self._DEP_RISKS_MAX]:
            if not isinstance(risk, dict):
                continue
            pkg = str(risk.get("package", "")).strip() or "(unknown)"
            version = str(risk.get("installed_version", "")).strip()
            severity = str(risk.get("severity", "")).strip() or "unknown"
            advisory = str(risk.get("advisory_id", "")).strip()
            summary = str(risk.get("summary", "")).strip()
            head = f"`{pkg}`" + (f" {version}" if version else "")
            tail = f" [{severity}]"
            if advisory:
                tail += f" {advisory}"
            if summary:
                tail += f" — {summary[:160]}"
            lines.append(f"- {head}{tail}")
        if not lines:
            return ""
        more = len(risks) - len(lines)
        if more > 0:
            lines.append(f"- _(+{more} more — see `.devcouncil/repo_map.json`)_")
        return (
            "\n## Dependency risks (known vulnerabilities)\n"
            "_Reported by a local dependency auditor. Avoid bumping a listed package to a "
            "still-vulnerable version; prefer a patched release._\n"
            + "\n".join(lines)
            + "\n"
        )

    @staticmethod
    def _fit_segments(segments: list[dict], budget: int) -> str:
        """Fit optional context segments into ``budget`` chars. Segments are kept in
        priority order (lower = more important) and emitted in display order; any that
        don't fit are dropped with an explicit marker so truncation is never silent."""
        kept: list[dict] = []
        used = 0
        dropped: list[dict] = []
        for seg in sorted(segments, key=lambda s: (s["priority"], s["order"])):
            if budget > 0 and used + len(seg["text"]) <= budget:
                kept.append(seg)
                used += len(seg["text"])
            else:
                dropped.append(seg)
        body = "".join(seg["text"] for seg in sorted(kept, key=lambda s: s["order"]))
        if dropped:
            names = ", ".join(s["name"] for s in sorted(dropped, key=lambda s: s["order"]))
            body += f"\n_[Context budget reached — omitted: {names}. Open these directly if needed.]_\n"
        return body

    def build_task_prompt(
        self, task: Task, requirements: List[Requirement], *, max_chars: int | None = None
    ) -> str:
        if max_chars is None:
            max_chars = MAX_PROMPT_CHARS
        # Per-task outline cache so a planned file's symbol outline is computed once even
        # though both the planned-files section and the call-sites section consume it.
        self._outline_cache: dict[tuple[str, str], List[str]] = {}
        # Load the project config once and share it with the helpers that need it, instead
        # of each independently re-reading + re-parsing it. Best-effort: if it fails the
        # helpers fall back to loading it themselves (and degrade the same way).
        try:
            from devcouncil.app.config import load_config

            cfg = load_config(self.project_root)
        except Exception:
            cfg = None
        # When the run targets a constrained local window (Ollama + OLLAMA_NUM_CTX),
        # cap the budget so the server doesn't silently truncate past the window. Never
        # raises the budget above the caller's value — only lowers it to fit.
        window_budget = _local_context_window_budget(self.project_root, cfg=cfg)
        if window_budget is not None:
            max_chars = min(max_chars, window_budget)

        req_map = {r.id: r for r in requirements}
        task_reqs = [req_map[rid] for rid in task.requirement_ids if rid in req_map]

        # --- Core: always kept (the goal/scope/instructions the agent must have). ---
        core = f"""# Implement {task.id}: {task.title}

## Goal
{task.description}

## Requirements
"""
        for req in task_reqs:
            core += f"- {req.id}: {req.title}\n"
            for ac in req.acceptance_criteria:
                core += f"  - [ ] {ac.description} ({ac.verification_method})\n"

        # Carry the planning prompt-enhancer's codebase-specific constraints through to the
        # one who writes the code. Otherwise that domain guidance (e.g. "division truncates
        # toward zero", "no eval") shapes only the planning debate and reaches the executor
        # only if a planner happened to encode it into an acceptance criterion.
        enhancement = self._load_prompt_enhancement()
        if enhancement is not None and (enhancement.constraints or enhancement.applied_skills):
            core += "\n## Codebase-specific constraints (from planning — honor these)\n"
            for constraint in enhancement.constraints[:8]:
                core += f"- {constraint}\n"
            if enhancement.applied_skills:
                core += f"- Apply current senior-level practices for: {', '.join(enhancement.applied_skills[:6])}.\n"

        core += "\n## Allowed files\n"
        for pf in task.planned_files:
            core += f"- `{pf.path}` ({pf.allowed_change}): {pf.reason}\n"

        if task.forbidden_changes:
            core += (
                "\n## Forbidden changes\n"
                "_Do not modify these. Verification always rejects them; on hook-enabled "
                "clients they are also blocked before the write._\n"
            )
            for fc in task.forbidden_changes:
                core += f"- `{fc}`\n"

        core += "\n## Expected tests\n"
        for et in task.expected_tests:
            core += f"- `{et}`\n"

        core += "\n## Allowed commands\n"
        for cmd in task.allowed_commands:
            core += f"- `{cmd}`\n"

        instructions = """
## Instructions
1. Implement the goal described above.
2. Ensure all acceptance criteria are met.
3. Only modify the allowed files.
4. Stay within this task's scope even inside an allowed file: change only what the
   acceptance criteria require. Do NOT remove, rename, or alter the signature of an
   existing public symbol the task did not ask you to touch — verification flags an
   unrequested public-API change as scope drift and blocks it.
5. Wire every created file and every new public symbol into its intended caller
   (import, register, or call it) before claiming done. A file nothing imports, or a
   public function nothing calls, fails verification. If the caller is outside the
   current planned files, append it with `dev scope update <task_id> --lease-token
   <token> --planned-file <caller>` (modify-op only), then edit.
6. Run the allowed commands to verify your work.
7. Provide evidence of passing tests.
"""

        # Hard-task rigor: when the task classifies as hard, verification runs in
        # strict mode (stub/effort gates block, diff coverage enforced). Saying so
        # up front is cheaper than a repair loop after the fact.
        try:
            rigor_enabled = True if cfg is None else bool(cfg.verification.rigor.enabled)
            if rigor_enabled:
                from devcouncil.verification.difficulty import estimate_difficulty

                if estimate_difficulty(task, requirements) == "hard":
                    instructions += _HARD_TASK_RIGOR_SECTION
        except Exception:
            logger.debug("hard-task rigor section skipped", exc_info=True)

        # --- Optional context: fitted within the remaining budget, dropped lowest-
        # priority first. Priority: file contents (1) > structural (2) ~ dependents (2)
        # > skills (3); display order keeps the original reading sequence. ---
        repo_map_data = self._load_repo_map()
        repo_map_stale = self._repo_map_stale(repo_map_data)
        planned_paths = [planned.path for planned in task.planned_files]
        segments: list[dict] = []

        graph_context = CodeReviewGraphAdapter(self.project_root).prompt_section(planned_paths)
        struct_text = ""
        struct_has_stale_note = False
        if graph_context:
            struct_text = f"\n{graph_context}"
        else:
            repo_map_context = self._repo_map_section(planned_paths, repo_map_data)
            if repo_map_context:
                prefix = f"\n{self._STALE_MAP_NOTE}" if repo_map_stale else ""
                struct_has_stale_note = repo_map_stale
                struct_text = f"{prefix}\n{repo_map_context}"
        if struct_text:
            segments.append({"order": 1, "priority": 2, "name": "structural context", "text": struct_text})
        elif repo_map_data is None:
            # No graph CLI and the repo map file is entirely absent (not merely stale):
            # nudge the agent to run `dev map`, surfaced the same way staleness is.
            segments.append({
                "order": 1, "priority": 2, "name": "no repo map note",
                "text": f"\n{self._NO_MAP_NOTE}",
            })

        # Always-on impact block ("changing X touches Y") from the repo map — present
        # even when the code-review-graph CLI is absent (the common case). High priority
        # (1) and short, so it survives all but the tightest budgets; ordered right after
        # structural context so the agent sees the blast radius before the file bodies.
        impact_text = self._impact_section(task, repo_map_data)
        if impact_text:
            segments.append({"order": 2, "priority": 1, "name": "impact", "text": impact_text})

        liveness_text = self._liveness_debt_section(task, repo_map_data)
        if liveness_text:
            segments.append({"order": 2, "priority": 2, "name": "liveness debt", "text": liveness_text})

        files_text = self._planned_files_section(task)
        if files_text:
            segments.append({"order": 3, "priority": 1, "name": "file contents", "text": files_text})

        dependents_section = self._dependents_section(task, repo_map_data)
        if dependents_section:
            prefix = f"\n{self._STALE_MAP_NOTE}" if (repo_map_stale and not struct_has_stale_note) else ""
            segments.append({"order": 4, "priority": 2, "name": "dependents", "text": prefix + dependents_section})

        skills_text = self._skills_section(task)
        if skills_text:
            segments.append({"order": 5, "priority": 3, "name": "engineering skills", "text": skills_text})

        # Design system (a hard constraint for UI work) and OKF project knowledge. The
        # design system rides just above skills; OKF knowledge alongside them. Both are
        # bounded by config char budgets in `_knowledge_sections`.
        design_text, knowledge_text = self._knowledge_sections(task, cfg=cfg)
        if design_text:
            segments.append({"order": 5, "priority": 2, "name": "design system", "text": design_text})
        if knowledge_text:
            segments.append({"order": 5, "priority": 3, "name": "project knowledge", "text": knowledge_text})

        # Lowest priority (4): the budget drops call sites first. It only adds value once
        # the file bodies + dependents are present anyway.
        call_sites_text = self._call_sites_section(task, repo_map_data)
        if call_sites_text:
            segments.append({"order": 6, "priority": 4, "name": "call sites", "text": call_sites_text})

        # Lowest priority (5): dependency risks are opt-in, advisory context — the
        # budget drops them first so they never displace structural/file context.
        dependency_risks_text = self._dependency_risks_section(repo_map_data)
        if dependency_risks_text:
            segments.append({"order": 7, "priority": 5, "name": "dependency risks", "text": dependency_risks_text})

        # The core + instructions are never dropped; if they alone exceed the budget the
        # model's server will truncate them, so warn loudly instead of failing silently.
        core_len = len(core) + len(instructions)
        if core_len > max_chars:
            logger.warning(
                "Task %s core prompt (%d chars) exceeds the context budget (%d chars). "
                "On a local model with a small OLLAMA_NUM_CTX this will be truncated server-side; "
                "raise OLLAMA_NUM_CTX or split the task.",
                task.id, core_len, max_chars,
            )

        optional = self._fit_segments(segments, max_chars - core_len)
        return core + optional + instructions
