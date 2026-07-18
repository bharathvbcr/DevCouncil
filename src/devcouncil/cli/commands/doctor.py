import logging
import typer
import subprocess
import os
import shutil
import sys
from pathlib import Path
from rich.console import Console
from rich.table import Table

from devcouncil.app.config import get_gcloud_access_token, load_config, load_local_secrets, provider_api_key_env_var
from devcouncil.executors.agent_registry import (
    CODING_CLI_INTEGRATION_INFO,
    CODING_CLI_PROBE_ORDER,
    CODING_CLI_VERSION_COMMANDS,
    DEPRECATED_CODING_CLIS,
    GEMINI_DEPRECATION_MESSAGE,
    detect_available_coding_cli,
    resolve_automated_executor,
)
from devcouncil.llm.provider import SUPPORTED_MODEL_PROVIDERS, validate_model_provider
from devcouncil.telemetry.stages import log_stage, log_step

app = typer.Typer()

console = Console()
logger = logging.getLogger(__name__)


def _probe_ollama(base_url: str) -> tuple[bool, str]:
    """Best-effort reachability check for a local Ollama server.

    ``base_url`` carries the OpenAI-compatible ``/v1`` suffix; the native
    ``/api/version`` endpoint lives one level up. Returns (reachable, detail);
    any failure is reported, never raised.
    """
    root = base_url.rstrip("/")
    if root.endswith("/v1"):
        root = root[: -len("/v1")].rstrip("/")
    try:
        import httpx

        resp = httpx.get(f"{root}/api/version", timeout=3.0)
        if resp.status_code < 400:
            version = ""
            try:
                version = (resp.json() or {}).get("version", "")
            except Exception:
                version = ""
            return True, f"Reachable at {root}" + (f" (v{version})." if version else ".")
        return False, f"Server at {root} returned HTTP {resp.status_code}."
    except Exception:
        return False, f"No Ollama server reachable at {root}."


def _probe_ollama_models(base_url: str) -> tuple[bool, set[str]]:
    """Best-effort list of locally-pulled Ollama model tags via native ``/api/tags``.

    Returns (queried_ok, names). ``names`` holds the reported tags (e.g.
    ``qwen2.5-coder:7b``). Any failure returns (False, set()) and is never raised — a
    green liveness probe with no pulled model is the most common "configured but doesn't
    work" trap, so this turns it into an actionable row."""
    root = base_url.rstrip("/")
    if root.endswith("/v1"):
        root = root[: -len("/v1")].rstrip("/")
    try:
        import httpx

        resp = httpx.get(f"{root}/api/tags", timeout=3.0)
        if resp.status_code >= 400:
            return False, set()
        models = (resp.json() or {}).get("models", []) or []
        names = {str(m.get("name", "")).strip() for m in models if m.get("name")}
        return True, {n for n in names if n}
    except Exception:
        return False, set()


def _ollama_model_present(model: str, pulled: set[str]) -> bool:
    """Whether ``model`` is among the pulled tags, tolerant of the implicit ``:latest``
    tag Ollama adds to untagged models."""
    if model in pulled:
        return True
    base = model.split(":", 1)[0]
    # configured "qwen2.5-coder" matches a pulled "qwen2.5-coder:latest", and vice versa.
    candidates = {model, f"{model}:latest", base, f"{base}:latest"}
    return any(tag in candidates or tag.split(":", 1)[0] == base and ":" not in model for tag in pulled)


def _knowledge_dir(project_root: Path, config=None) -> str:
    """Configured knowledge directory (honors ``knowledge.directory``), best-effort.

    Mirrors ``cli.commands.okf._knowledge_okf_dir`` so doctor inspects the same location
    ingest writes to. Any config failure falls back to the documented default rather than
    raising — doctor must keep running even with a broken config.

    ``config`` is an optional pre-loaded config (default ``None`` loads as before), so a
    single doctor invocation can reuse one ``load_config`` call across checks.
    """
    directory = ".devcouncil/knowledge"
    try:
        cfg = config if config is not None else load_config(project_root)
        directory = cfg.knowledge.directory
    except Exception as e:
        logger.debug("Failed to load knowledge directory from config, using default: %s", e)
    return directory


def check_ingested_knowledge(project_root: Path, config=None) -> list[tuple[str, str, str]]:
    """Best-effort health rows for ingested knowledge under ``<knowledge dir>/{okf,design}``.

    Returns ``(component, status_markup, notes)`` rows for the doctor table. This is the
    most common "ingested but silently broken" surface: an OKF bundle with a dangling
    cross-link, or a design.md with broken token references, both validate clean to the
    eye but degrade the prompt context. We therefore read+validate every ingested bundle
    and lint the design system, reporting counts and any problems.

    Never raises: any knowledge-layer failure becomes a ``WARN`` row, and an empty
    knowledge area yields a neutral ``INFO`` row — a project that never ingested knowledge
    is not a misconfiguration.
    """
    ok = "[green]OK[/green]"
    warn = "[yellow]WARN[/yellow]"
    info = "[cyan]INFO[/cyan]"
    rows: list[tuple[str, str, str]] = []

    directory = _knowledge_dir(project_root, config=config)
    base = project_root / directory
    okf_area = base / "okf"
    design_md = base / "design" / "design.md"

    # Identify ingested OKF bundles. `dev okf ingest` writes each bundle into its own
    # subfolder under okf/; treat each such subdir as a bundle. If documents sit loose
    # directly under okf/ (and there are no subfolder bundles), treat okf/ as one bundle.
    # Choosing one or the other avoids double-counting documents via rglob.
    bundle_dirs: list[Path] = []
    if okf_area.is_dir():
        try:
            subdir_bundles = [
                child
                for child in sorted(okf_area.iterdir())
                if child.is_dir() and any(child.rglob("*.md"))
            ]
        except Exception:
            subdir_bundles = []
        loose_docs = any(p.is_file() for p in okf_area.glob("*.md"))
        if subdir_bundles:
            bundle_dirs = subdir_bundles
        elif loose_docs:
            bundle_dirs = [okf_area]

    has_design = design_md.is_file()

    # Nothing ingested at all → neutral info line, not a failure.
    if not bundle_dirs and not has_design:
        rows.append(
            (
                "Ingested knowledge",
                info,
                f"No ingested knowledge under {directory}/ (okf/, design/). "
                "Add some with 'dev okf ingest <bundle>'.",
            )
        )
        return rows

    # --- OKF bundles -----------------------------------------------------------------
    if bundle_dirs:
        from devcouncil.knowledge.okf import read_bundle, validate_bundle

        total_docs = 0
        problems: list[str] = []
        for bdir in bundle_dirs:
            try:
                bundle = read_bundle(bdir)
                total_docs += len(bundle.documents)
                problems.extend(validate_bundle(bundle))
            except Exception as exc:  # never let a malformed bundle crash doctor
                problems.append(f"{bdir.name}: failed to read bundle ({exc})")
        summary = f"{len(bundle_dirs)} bundle(s), {total_docs} document(s)"
        if problems:
            preview = "; ".join(problems[:5])
            extra = "" if len(problems) <= 5 else f" (+{len(problems) - 5} more)"
            rows.append(
                (
                    "Ingested OKF",
                    warn,
                    f"{summary}: {len(problems)} validation problem(s): {preview}{extra}.",
                )
            )
        else:
            rows.append(("Ingested OKF", ok, f"{summary}; no validation problems."))

    # --- Design system ----------------------------------------------------------------
    if has_design:
        from devcouncil.knowledge.design import lint, parse_design_md

        try:
            findings = lint(parse_design_md(design_md))
        except Exception as exc:  # never let a malformed design.md crash doctor
            rows.append(("Ingested design.md", warn, f"present, but lint failed: {exc}."))
        else:
            if findings:
                preview = "; ".join(f.format() for f in findings[:5])
                extra = "" if len(findings) <= 5 else f" (+{len(findings) - 5} more)"
                rows.append(
                    (
                        "Ingested design.md",
                        warn,
                        f"present; {len(findings)} lint finding(s): {preview}{extra}.",
                    )
                )
            else:
                rows.append(("Ingested design.md", ok, "present; 0 lint findings."))

    return rows


# Explicit map from an area row in docs/project-status.md to the tests/unit/<subsystem>/
# directory expected to back a "Stable" claim. Deliberately small and in-code (see
# IMPROVEMENTS.md: doc/status drift check) — extend it as areas graduate to Stable.
STATUS_DOC_UNIT_TEST_DIRS: tuple[tuple[str, str], ...] = (
    ("CLI & Storage", "storage"),
    ("Artifact Graph", "artifacts"),
    ("Council Debate", "council"),
    ("Diff↔Coverage Gate", "verification"),
    ("Cost & Run Telemetry", "telemetry"),
    ("Security Scanning", "gating"),
    ("Manual Executor", "executors"),
    ("Lite Check (`dev check --verify`)", "execution"),
    ("Next-Actions Contract", "reporting"),
    ("Repo Map & Code Graph", "indexing"),
    ("LSP / AST Indexing", "indexing"),
    ("Live Dashboard", "dashboard"),
)

# Flat tests/unit/test_*.py prefixes that back a subsystem when no per-subsystem dir exists.
STATUS_DOC_FLAT_TEST_PREFIXES: dict[str, tuple[str, ...]] = {
    "artifacts": ("test_artifact_graph",),
    "council": ("test_orchestrator", "test_state_machine"),
    "verification": ("test_verification", "test_verifier", "test_diff_coverage"),
    "telemetry": ("test_telemetry",),
    "executors": ("test_executors", "test_executor_", "test_claude_sdk_executor"),
    "execution": ("test_execution", "test_go_", "test_cli_check", "test_ad_hoc_check"),
    "reporting": ("test_json_report", "test_export_command", "test_pr_comments"),
    "dashboard": ("test_dashboard",),
    "indexing": (
        "test_indexing",
        "test_repo_mapper",
        "test_repo_map",
        "test_map_",
        "test_graph_",
        "test_graph_html",
        "test_graph_dead",
        "test_graph_cmd",
        "test_graph_intel",
        "test_graph_query",
        "test_graph_incremental",
        "test_graph_schema",
    ),
}


def _subsystem_has_unit_tests(unit_root: Path, subsystem: str) -> bool:
    """True when tests/unit/<subsystem>/ or mapped flat test files exist."""
    tests_dir = unit_root / subsystem
    try:
        if tests_dir.is_dir() and any(tests_dir.rglob("test_*.py")):
            return True
        if any(unit_root.glob(f"test_{subsystem}*.py")):
            return True
        for prefix in STATUS_DOC_FLAT_TEST_PREFIXES.get(subsystem, ()):
            if any(unit_root.glob(f"{prefix}*.py")):
                return True
    except Exception:
        return False
    return False


def _parse_status_doc_areas(status_doc: Path) -> dict[str, str]:
    """Parse the docs/project-status.md maturity table into {area: status-cell text}.

    Rows look like ``| **CLI & Storage** | Stable: SQLite + SQLModel, ... |``. The
    header row and the ``| :--- |`` separator row are skipped. Best-effort by design;
    the caller wraps this in try/except so a malformed doc never crashes doctor.
    """
    areas: dict[str, str] = {}
    for line in status_doc.read_text(encoding="utf-8").splitlines():
        stripped = line.strip()
        if not stripped.startswith("|"):
            continue
        cells = [cell.strip() for cell in stripped.strip("|").split("|")]
        if len(cells) < 2:
            continue
        area = cells[0].strip().strip("*").strip()
        if not area or area.lower() == "area" or set(area) <= {":", "-", " "}:
            continue
        areas[area] = cells[1]
    return areas


def check_liveness_reliability(project_root: Path) -> list[tuple[str, str, str]]:
    """Warn when map liveness used empty/unreliable entry roots."""
    ok = "[green]OK[/green]"
    warn = "[yellow]WARN[/yellow]"
    rows: list[tuple[str, str, str]] = []
    try:
        from devcouncil.utils.json_persist import read_json

        map_path = project_root / ".devcouncil" / "repo_map.json"
        if not map_path.is_file():
            return rows
        data = read_json(map_path)
        payload = data if isinstance(data, dict) else {}
        unreliable = bool(payload.get("liveness_unreachable_unreliable"))
        roots = payload.get("entry_roots") or []
        if unreliable or not roots:
            rows.append((
                "Map liveness",
                warn,
                "Production entry roots are empty or unreachable BFS was skipped. "
                "Add `indexing.entry_roots` in `.devcouncil/config.yaml` (repo-relative "
                "paths) and run `dev map`.",
            ))
        else:
            rows.append((
                "Map liveness",
                ok,
                f"{len(roots)} production entry root(s); unreachable lists are reliable.",
            ))
    except Exception:
        logger.debug("check_liveness_reliability failed", exc_info=True)
    return rows


def _repo_languages(project_root: Path) -> set[str]:
    """Repo languages from repo_map.json, else a bounded filesystem sniff."""
    try:
        from devcouncil.utils.json_persist import read_json

        map_path = project_root / ".devcouncil" / "repo_map.json"
        if map_path.is_file():
            data = read_json(map_path)
            langs = (data or {}).get("languages") if isinstance(data, dict) else None
            if isinstance(langs, list) and langs:
                return {str(lang) for lang in langs}
    except Exception:
        logger.debug("repo language read from map failed", exc_info=True)
    try:
        from devcouncil.indexing.lsp import LspInspector

        return set(LspInspector(project_root).detect_languages())
    except Exception:
        logger.debug("repo language detection failed", exc_info=True)
        return set()


def check_grammar_coverage(project_root: Path) -> list[tuple[str, str, str]]:
    """Warn when tree-sitter grammars for the repo's own languages are missing.

    A missing grammar silently degrades extraction for that language (a
    Python-heavy repo indexed without the Python grammar loses most symbols),
    so surface it with the install action instead of only in graph doctor.
    """
    ok = "[green]OK[/green]"
    warn = "[yellow]WARN[/yellow]"
    rows: list[tuple[str, str, str]] = []
    try:
        from devcouncil.codeintel.languages import grammar_status

        status = grammar_status()
        # repo_map languages are lowercase; LANGUAGE_SPECS names are display-cased
        # ("Python") — compare casefolded or nothing ever matches.
        repo_langs = {lang.casefold() for lang in _repo_languages(project_root)}
        if not repo_langs:
            return rows
        missing = sorted(
            row["language"]
            for row in status.get("languages", [])
            if str(row.get("language", "")).casefold() in repo_langs
            and row.get("missing_grammars")
            # Python extraction is native stdlib-ast (cache._grammar_identity);
            # a missing python tree-sitter grammar does not degrade indexing.
            and str(row.get("grammar", "")) != "python"
        )
        if missing:
            action = status.get("action") or (
                "Install the platform-matched devcouncil-codeintel-grammars wheel."
            )
            rows.append((
                "Grammar coverage",
                warn,
                f"Missing tree-sitter grammars for repo language(s): {', '.join(missing)}. "
                f"{action}",
            ))
        else:
            rows.append((
                "Grammar coverage",
                ok,
                f"Grammars available for all repo languages ({', '.join(sorted(repo_langs))}).",
            ))
    except Exception:
        logger.debug("check_grammar_coverage failed", exc_info=True)
    return rows


def check_lsp_reference_confirmation(project_root: Path) -> list[tuple[str, str, str]]:
    """Report repo languages with no language server on PATH.

    Without a server, ``--lsp-refs`` dead-symbol confirmation cannot run for
    that language; WARN when ``indexing.lsp_refs`` is enabled, informational
    otherwise.
    """
    ok = "[green]OK[/green]"
    warn = "[yellow]WARN[/yellow]"
    rows: list[tuple[str, str, str]] = []
    try:
        from devcouncil.indexing.lsp import LspInspector

        candidates = LspInspector(project_root).server_candidates()
        if not candidates:
            return rows
        available_langs = {c.language for c in candidates if c.available}
        missing = sorted({c.language for c in candidates} - available_langs)
        lsp_refs_enabled = False
        try:
            from devcouncil.app.config import load_config

            lsp_refs_enabled = bool(load_config(project_root).indexing.lsp_refs)
        except Exception:
            logger.debug("lsp_refs config read failed", exc_info=True)
        if not missing:
            rows.append((
                "LSP servers",
                ok,
                f"Language server on PATH for: {', '.join(sorted(available_langs))}.",
            ))
        elif lsp_refs_enabled:
            rows.append((
                "LSP servers",
                warn,
                "indexing.lsp_refs is enabled but no language server is on PATH for: "
                f"{', '.join(missing)} — dead-symbol confirmation is skipped there.",
            ))
        else:
            rows.append((
                "LSP servers",
                ok,
                f"No server on PATH for: {', '.join(missing)} (detection only; install "
                "one to enable `dev map --lsp-refs` confirmation).",
            ))
    except Exception:
        logger.debug("check_lsp_reference_confirmation failed", exc_info=True)
    return rows


def check_unknown_indexing_keys(project_root: Path) -> list[tuple[str, str, str]]:
    """Warn on unknown ``indexing.*`` keys in config.yaml (typos/removed options)."""
    warn = "[yellow]WARN[/yellow]"
    rows: list[tuple[str, str, str]] = []
    try:
        import yaml

        cfg_path = project_root / ".devcouncil" / "config.yaml"
        if not cfg_path.is_file():
            return rows
        payload = yaml.safe_load(cfg_path.read_text(encoding="utf-8")) or {}
        indexing = payload.get("indexing")
        if not isinstance(indexing, dict):
            return rows
        from devcouncil.app.config import IndexingConfig

        unknown = sorted(str(k) for k in indexing if str(k) not in IndexingConfig.model_fields)
        if unknown:
            rows.append((
                "Config keys",
                warn,
                f"Unknown `indexing.*` key(s) ignored: {', '.join(unknown)} — "
                "typo or an option removed in this version.",
            ))
    except Exception:
        logger.debug("check_unknown_indexing_keys failed", exc_info=True)
    return rows


def check_mapping_stack(project_root: Path) -> list[tuple[str, str, str]]:
    """Doctor rows for repo map + code graph freshness (native mapping stack)."""
    rows: list[tuple[str, str, str]] = []
    ok = "[green]OK[/green]"
    warn = "[yellow]WARN[/yellow]"
    graphify_legacy = project_root / ".devcouncil" / "graphify.yaml"
    if graphify_legacy.is_file():
        rows.append((
            "Legacy graphify.yaml",
            warn,
            "``.devcouncil/graphify.yaml`` is deprecated. Move ``corpus:`` settings into "
            "``.devcouncil/config.yaml`` under ``indexing.corpus`` and remove the file.",
        ))
    rows.extend(check_repo_map_freshness(project_root))
    rows.extend(check_liveness_reliability(project_root))
    rows.extend(check_grammar_coverage(project_root))
    rows.extend(check_lsp_reference_confirmation(project_root))
    rows.extend(check_unknown_indexing_keys(project_root))
    graph_path = project_root / ".devcouncil" / "graph" / "code_graph.json"
    if not graph_path.is_file():
        store_has_graph = False
        try:
            from devcouncil.indexing.graph.build import load_code_graph

            store_has_graph = load_code_graph(project_root) is not None
        except Exception:
            logger.debug("store probe for missing graph JSON failed", exc_info=True)
        if store_has_graph:
            rows.append((
                "Code graph",
                warn,
                "JSON export ``.devcouncil/graph/code_graph.json`` is missing but the "
                "SQLite store has a graph. Run ``dev map`` to re-export it.",
            ))
        else:
            rows.append((
                "Code graph",
                warn,
                "Missing ``.devcouncil/graph/code_graph.json``. Run ``dev map`` or ``dev graph ingest``.",
            ))
    else:
        try:
            from devcouncil.indexing.graph.build import load_code_graph

            if load_code_graph(project_root) is None:
                rows.append(("Code graph", warn, "Unreadable or empty code graph export."))
            else:
                rows.append(("Code graph", ok, "Present and loadable."))
        except Exception:
            rows.append(("Code graph", warn, "Could not load code graph export."))
    return rows


def check_repo_map_freshness(project_root: Path) -> list[tuple[str, str, str]]:
    """Doctor rows for ``.devcouncil/repo_map.json`` fingerprint freshness."""
    try:
        from devcouncil.indexing.repo_mapper import RepoMapper
        from devcouncil.utils.json_persist import read_json

        map_path = project_root / ".devcouncil" / "repo_map.json"
        ok = "[green]OK[/green]"
        warn = "[yellow]Stale[/yellow]"
        if not map_path.is_file():
            return [(
                "Repo map",
                warn,
                "Missing `.devcouncil/repo_map.json`. Run `dev map` before verify or checkout.",
            )]
        loaded = read_json(map_path)
        data = loaded if isinstance(loaded, dict) else {}
        mapper = RepoMapper(project_root)
        if mapper.map_is_stale(data):
            head = str(data.get("generated_head") or "(unknown)")[:12]
            current = mapper._git_head()[:12]
            if head == current and head != "(unknown)":
                detail = (
                    f"Working-tree content changed since the last map "
                    f"(git HEAD {current}). Run `dev map` or rely on auto-refresh "
                    f"at checkout/verify."
                )
            else:
                detail = (
                    f"Behind current code (map HEAD {head}, repo HEAD {current}). "
                    "Run `dev map` or rely on auto-refresh at checkout/verify."
                )
            return [(
                "Repo map",
                warn,
                detail,
            )]
        return [("Repo map", ok, "Fresh — fingerprints match the current repository.")]
    except Exception:
        return []


def check_execution_containment(project_root: Path, config=None) -> list[tuple[str, str, str]]:
    """Doctor rows for YOLO/containment posture (scope gate, write hooks, risky profiles)."""
    try:
        from devcouncil.app.config import load_config
        from devcouncil.executors.agent_registry import load_agent_profiles

        cfg = config if config is not None else load_config(project_root)
        rows: list[tuple[str, str, str]] = []
        ok = "[green]OK[/green]"
        warn = "[yellow]Risky[/yellow]"

        if cfg.execution.enforce_file_scope_pre_verify:
            rows.append((
                "Pre-verify scope gate",
                ok,
                "execution.enforce_file_scope_pre_verify is enabled — OOS CLI writes are reverted before verify.",
            ))
        else:
            rows.append((
                "Pre-verify scope gate",
                warn,
                "Off by default. Enable execution.enforce_file_scope_pre_verify for battle-test containment.",
            ))

        write_gate = bool(getattr(cfg.integrations.claude, "write_gate", False))
        if write_gate:
            rows.append((
                "Claude write-gate",
                ok,
                "integrations.claude.write_gate is enabled. Run `dev integrate hooks --apply` if hooks are missing.",
            ))
        else:
            rows.append((
                "Claude write-gate",
                warn,
                "Disabled. Run `dev integrate claude --apply --write-gate` for PreToolUse containment.",
            ))

        for name, profile in load_agent_profiles(project_root).items():
            mode = (profile.permission_mode or "").strip().lower()
            if mode == "bypasspermissions":
                rows.append((
                    f"Profile {name}",
                    warn,
                    "permission_mode bypassPermissions bypasses all CLI edit gates.",
                ))
            extra = profile.extra_args or []
            for index, arg in enumerate(extra):
                if arg == "--permission-mode" and index + 1 < len(extra):
                    if str(extra[index + 1]).lower() == "bypasspermissions":
                        rows.append((
                            f"Profile {name}",
                            warn,
                            "extra_args include --permission-mode bypassPermissions.",
                        ))
        return rows
    except Exception:
        return []


def check_local_monitor_sampling(project_root: Path, config=None) -> list[tuple[str, str, str]]:
    """Doctor rows for local-monitor verification safety.

    Calibration probes (2026-07-03, benchmarks/results/local_monitor_*) showed a
    local monitor with single-shot acceptance checks rubber-stamping real defects,
    while samples>=3 + per-criterion compilation caught 6/6 with zero false passes.
    Auto-resolution picks the safe local settings; these rows surface EXPLICIT
    overrides that disable them — at setup time, where users actually look, rather
    than only as mid-run log warnings. Cloud monitors produce no rows (single-shot
    is their intended default). Never raises.
    """
    try:
        from devcouncil.app.config import load_config, role_runs_on_local_provider

        cfg = config if config is not None else load_config(project_root)
        rows: list[tuple[str, str, str]] = []
        warn = "[yellow]Risky[/yellow]"

        local_monitor = role_runs_on_local_provider(cfg, "implementation_reviewer")
        for message in cfg.verification.acceptance_checks.unsafe_override_warnings(local_monitor):
            rows.append(("Local monitor (acceptance checks)", warn, message))
        local_reviewer = role_runs_on_local_provider(cfg, "live_reviewer")
        for message in cfg.verification.reviewer_checks.unsafe_override_warnings(local_reviewer):
            rows.append(("Local reviewer (live review)", warn, message))

        if not rows and (local_monitor or local_reviewer):
            samples, repairs, per_criterion = cfg.verification.acceptance_checks.resolved(local_monitor)
            votes = cfg.verification.reviewer_checks.resolved(local_reviewer)
            rows.append((
                "Local monitor ensembling",
                "[green]OK[/green]",
                f"Acceptance checks: samples={samples}, repair_attempts={repairs}, "
                f"per_criterion={per_criterion}; reviewer votes={votes}.",
            ))
        return rows
    except Exception:
        return []  # config problems already surface via other doctor rows


def check_coverage_floor(project_root: Path) -> list[tuple[str, str, str]]:
    """Doctor row for ``[tool.coverage.report] fail_under`` in pyproject.toml."""
    ok = "[green]OK[/green]"
    warn = "[yellow]WARN[/yellow]"
    pyproject = project_root / "pyproject.toml"
    if not pyproject.is_file():
        return [("Coverage floor", warn, "No pyproject.toml; cannot verify fail_under.")]
    try:
        import tomllib

        data = tomllib.loads(pyproject.read_text(encoding="utf-8"))
        fail_under = (
            data.get("tool", {})
            .get("coverage", {})
            .get("report", {})
            .get("fail_under")
        )
    except Exception as exc:
        return [("Coverage floor", warn, f"Could not read pyproject.toml: {exc}.")]
    if fail_under is None:
        return [
            (
                "Coverage floor",
                warn,
                "No [tool.coverage.report] fail_under in pyproject.toml; CI will not enforce a coverage minimum.",
            )
        ]
    return [
        (
            "Coverage floor",
            ok,
            f"fail_under={fail_under} configured in pyproject.toml (enforced by `coverage report`).",
        )
    ]


def _mypy_command(project_root: Path) -> list[str]:
    """Resolve mypy without making a doctor check depend on network access."""
    project_mypy = project_root / ".venv" / "bin" / "mypy"
    if project_mypy.is_file():
        return [str(project_mypy), "src"]
    if shutil.which("uv"):
        return ["uv", "run", "--python", "3.12", "mypy", "src"]
    return [sys.executable, "-m", "mypy", "src"]


def check_mypy_status(project_root: Path) -> list[tuple[str, str, str]]:
    """Doctor row summarizing canonical mypy health for this repository."""
    ok = "[green]OK[/green]"
    warn = "[yellow]WARN[/yellow]"
    pyproject = project_root / "pyproject.toml"
    if not pyproject.is_file():
        return [("mypy green", warn, "No pyproject.toml; skipping mypy probe.")]
    src_root = project_root / "src"
    if not src_root.is_dir():
        return [("mypy green", warn, "No src/ directory; skipping mypy probe.")]
    command = _mypy_command(project_root)
    command_text = subprocess.list2cmdline(command)
    try:
        proc = subprocess.run(
            command,
            cwd=project_root,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=120,
        )
    except FileNotFoundError:
        return [("mypy green", warn, f"{command_text} is unavailable; install uv and development dependencies.")]
    except subprocess.TimeoutExpired:
        return [("mypy green", warn, f"{command_text} timed out after 120s.")]
    except Exception as exc:
        return [("mypy green", warn, f"{command_text} probe failed: {exc}.")]

    output = (proc.stdout or "") + (proc.stderr or "")
    if "No module named mypy" in output or "No module named 'mypy'" in output:
        return [
            (
                "mypy green",
                warn,
                f"{command_text} is unavailable; install uv and development dependencies.",
            )
        ]
    if "INTERNAL ERROR" in output:
        return [
            (
                "mypy green",
                warn,
                f"{command_text} crashed with INTERNAL ERROR; pin or upgrade mypy in dev dependencies.",
            )
        ]
    if proc.returncode == 0:
        return [("mypy green", ok, f"{command_text} passed with zero errors.")]
    error_count = output.count(": error:")
    preview = output.strip().splitlines()[-1] if output.strip() else "mypy failed"
    return [
        (
            "mypy green",
            warn,
            f"{command_text} reported {error_count} error(s). Last line: {preview}",
        )
    ]


def check_status_doc_drift(project_root: Path) -> list[tuple[str, str, str]]:
    """Doctor rows verifying docs/project-status.md "Stable" claims against tests/unit/.

    For each mapped area (``STATUS_DOC_UNIT_TEST_DIRS``) whose status-table row claims
    Stable, require a non-empty ``tests/unit/<subsystem>/`` directory (at least one
    ``test_*.py``) to back the claim; mismatches become ``WARN`` rows so the status doc
    cannot drift ahead of the test suite unnoticed. A mapped area missing from the doc
    is also reported — that means the in-code mapping went stale.

    Never raises: a missing status doc yields a neutral ``INFO`` row (most projects that
    embed DevCouncil have no such doc), and any parse failure becomes a ``WARN`` row.
    """
    ok = "[green]OK[/green]"
    warn = "[yellow]WARN[/yellow]"
    info = "[cyan]INFO[/cyan]"

    status_doc = project_root / "docs" / "project-status.md"
    if not status_doc.is_file():
        return [
            (
                "Status-doc drift",
                info,
                "No docs/project-status.md in this project; skipping status-vs-tests drift check.",
            )
        ]

    try:
        areas = _parse_status_doc_areas(status_doc)
    except Exception as exc:  # never let a malformed status doc crash doctor
        return [("Status-doc drift", warn, f"Could not parse docs/project-status.md: {exc}.")]

    mismatches: list[str] = []
    verified = 0
    unit_root = project_root / "tests" / "unit"
    for area, subsystem in STATUS_DOC_UNIT_TEST_DIRS:
        status_text = areas.get(area)
        if status_text is None:
            mismatches.append(
                f"'{area}' is in doctor's drift mapping but not in the status table "
                "(update STATUS_DOC_UNIT_TEST_DIRS)"
            )
            continue
        if not status_text.strip().lower().startswith("stable"):
            continue  # only Stable rows claim unit-test-backed maturity
        try:
            has_tests = _subsystem_has_unit_tests(unit_root, subsystem)
        except Exception:
            has_tests = False
        if has_tests:
            verified += 1
        else:
            try:
                flat_matches = any(unit_root.glob(f"test_{subsystem}*.py"))
            except Exception:
                flat_matches = False
            hint = (
                f" (flat tests/unit/test_{subsystem}*.py files exist but no per-subsystem dir)"
                if flat_matches
                else ""
            )
            mismatches.append(
                f"'{area}' claims Stable but tests/unit/{subsystem}/ is missing or empty{hint}"
            )

    if mismatches:
        preview = "; ".join(mismatches[:5])
        extra = "" if len(mismatches) <= 5 else f" (+{len(mismatches) - 5} more)"
        return [
            (
                "Status-doc drift",
                warn,
                f"{len(mismatches)} mismatch(es) between docs/project-status.md and tests/unit/: "
                f"{preview}{extra}.",
            )
        ]
    return [
        (
            "Status-doc drift",
            ok,
            f"{verified} Stable claim(s) in docs/project-status.md backed by non-empty "
            "tests/unit/<subsystem>/ directories.",
        )
    ]


def _add_logging_row(table, project_root: Path) -> None:
    """Append a logging-health row: where the durable run log lives and how big it
    is, so a user chasing a recurring failure knows exactly where to look."""
    from devcouncil.telemetry.logging_setup import _resolve_log_path

    log_path = _resolve_log_path(project_root)
    if log_path.exists():
        size_kb = log_path.stat().st_size / 1024
        detail = f"{log_path} ({size_kb:.0f} KB). View: dev logs tail"
    else:
        detail = f"Will write to {log_path} on first command. View: dev logs tail"
    table.add_row("logging", "[green]OK[/green]", detail)


def _subsystem_maturity_rows() -> list[tuple[str, str, str]]:
    """Curated maturity tiers aligned with docs/project-status.md."""
    return [
        ("CLI & Storage", "stable", "SQLite + SQLModel; core workflow"),
        ("Artifact Graph / Coverage", "stable", "Coverage engine and reports"),
        ("Council Debate / Planning", "stable", "Multi-agent planning and critique"),
        ("Manual Executor", "stable", "Sidecar mode"),
        ("Diff↔Coverage Gate", "stable", "Signal-first; opt-in blocking"),
        ("Next-Actions / dev check --verify", "stable", "Deterministic evidence gate"),
        ("Ollama provider", "stable", "Offline planning without API keys"),
        ("Engineering Skills", "stable", "dev skills listing/scaffolding"),
        ("Cost & Run Telemetry", "stable", "dev cost show / dev runs"),
        ("Security Scanning", "stable", "Secret redaction and detection"),
        ("Coding CLI Executors", "preview", "Codex, Claude, OpenCode, Antigravity, Warp, Cursor, …; Gemini deprecated"),
        ("Repair Loop (dev go)", "stable", "Bounded self-repair; correction manifest + no-progress"),
        ("LLM repair inference", "preview", "Optional RepairService manifest sharpening"),
        ("MCP Server (Claude hero loop)", "stable", "Certified checkout→verify loop; golden e2e"),
        ("Multi-agent Campaign", "preview", "dev campaign — parallel DAG + Reviewer QC"),
        ("OKF / design.md", "preview", "dev okf / dev design commands"),
        ("CI Scaffolding", "preview", "dev scaffold-ci starter workflow"),
        ("One-command onboarding (dev boot)", "preview", "setup + integrate --apply + go"),
        ("GitHub PR Checks / Comments", "preview", "dev report --github*"),
        ("Repo Map & Code Graph", "stable", "dev map / dev graph; liveness + query/dead/impact/html"),
        ("LSP / AST Indexing", "preview", "dev lsp inspect (detection-only) / dev ast"),
        ("Live Dashboard", "stable", "local-only operator UI; loopback + token-guarded apply"),
        ("Coding CLI Hooks", "preview", "Stop gate on Claude/Codex Stop hooks; assist seeded on integrate"),
        ("Stop gate & claim checks", "preview", "Completion-claim mapper + optional active-task verify"),
        ("Corpus side index", "preview", "dev corpus + optional corpus/doc-ref rigor gates"),
        ("PDG / CFG / taint", "preview", "Opt-in Python PDG; off by default"),
        ("Native Executor", "preview", "Lease-gated writes + shared verify/next-actions loop; sandbox/timeout parity"),
    ]


def _render_maturity_section(table) -> None:
    """Append subsystem maturity rows so preview features are visible upfront."""
    stable = "[green]Stable[/green]"
    preview = "[yellow]Preview[/yellow]"
    experimental = "[magenta]Experimental[/magenta]"
    label_for = {"stable": stable, "preview": preview, "experimental": experimental}
    for area, tier, notes in _subsystem_maturity_rows():
        if tier == "preview":
            notes = f"{notes} — API/output may change."
        table.add_row(area, label_for.get(tier, tier), notes)


def _print_maturity_table() -> None:
    maturity = Table(title="DevCouncil Subsystem Maturity (see docs/project-status.md)")
    maturity.add_column("Area", style="cyan")
    maturity.add_column("Tier", style="magenta")
    maturity.add_column("Notes", style="green")
    _render_maturity_section(maturity)
    console.print(maturity)


def render_doctor_check(project_root: Path = Path(".")):
    # Load config once for the whole invocation; the diagnostic checks below reuse
    # this instead of re-reading config.yaml. None falls back to per-check loading.
    try:
        config = load_config(project_root)
    except Exception:
        config = None

    def _command_version(command: list[str]) -> str | None:
        executable = shutil.which(command[0])
        if not executable:
            return None

        resolved_command = [executable, *command[1:]]
        use_shell = os.name == "nt" and Path(executable).suffix.lower() in {".bat", ".cmd", ".ps1"}
        invocation = subprocess.list2cmdline(resolved_command) if use_shell else resolved_command
        try:
            return subprocess.check_output(
                invocation,
                stderr=subprocess.STDOUT,
                text=True,
                encoding="utf-8",
                errors="replace",
                shell=use_shell,
                timeout=10,
            ).splitlines()[0].strip()
        except Exception:
            return None

    table = Table(title="DevCouncil Doctor Check")
    table.add_column("Component", style="cyan")
    table.add_column("Status", style="magenta")
    table.add_column("Notes", style="green")

    # Check Git
    git_ver = _command_version(["git", "--version"])
    if git_ver:
        table.add_row("Git", "[green]OK[/green]", git_ver)
    else:
        table.add_row("Git", "[red]Missing[/red]", "Git is required for repo mapping and checkpoints.")

    # Check uv
    uv_ver = _command_version(["uv", "--version"])
    if uv_ver:
        table.add_row("uv", "[green]OK[/green]", uv_ver)
    else:
        table.add_row("uv", "[red]Missing[/red]", "Install uv to run or install DevCouncil.")

    # Check CLI shims
    if shutil.which("devcouncil"):
        table.add_row("devcouncil CLI", "[green]OK[/green]", "Found on PATH.")
    else:
        table.add_row("devcouncil CLI", "[yellow]Missing[/yellow]", "Run via 'uv run devcouncil' or install with 'uv tool install --force .'.")

    # Check ripgrep
    rg_ver = _command_version(["rg", "--version"])
    if rg_ver:
        table.add_row("ripgrep (rg)", "[green]OK[/green]", rg_ver)
    else:
        table.add_row("ripgrep (rg)", "[yellow]Missing[/yellow]", "ripgrep is highly recommended for fast repo mapping.")

    # Check supported coding CLIs (driven by the agent registry, so new
    # built-in agents show up here without doctor edits).
    for name in CODING_CLI_PROBE_ORDER:
        info = CODING_CLI_INTEGRATION_INFO.get(name)
        if info is None:
            continue
        version = None
        for probe in CODING_CLI_VERSION_COMMANDS.get(name, ()):
            version = _command_version(list(probe))
            if version:
                break
        if version:
            table.add_row(info.label, "[green]OK[/green]", f"{version}. Setup: {info.notes}.")
        else:
            table.add_row(
                info.label,
                "[yellow]Missing[/yellow]",
                f"Optional. Install {info.label}, then use: {info.notes}.",
            )

    for name in sorted(DEPRECATED_CODING_CLIS):
        if name in CODING_CLI_PROBE_ORDER:
            continue
        info = CODING_CLI_INTEGRATION_INFO.get(name)
        if info is None:
            continue
        table.add_row(
            info.label,
            "[dim]Deprecated[/dim]",
            GEMINI_DEPRECATION_MESSAGE,
        )

    detected = detect_available_coding_cli(project_root)
    resolved = resolve_automated_executor(project_root, None)
    if detected:
        table.add_row(
            "Recommended coding CLI",
            "[green]OK[/green]",
            f"Use --executor {resolved} for dev go / dev run (detected on PATH).",
        )
    else:
        table.add_row(
            "Recommended coding CLI",
            "[yellow]Missing[/yellow]",
            "No built-in coding CLI on PATH. Run dev integrate recommend after installing one.",
        )

    # Ingested-knowledge health (added before the provider branch so it appears on every
    # code path, including the early returns for ollama / unsupported providers).
    for component, status, notes in check_ingested_knowledge(project_root, config=config):
        table.add_row(component, status, notes)

    from devcouncil.llm.semantic_bridge import check_semantic_layer

    for component, status, notes in check_semantic_layer(project_root, config=config):
        table.add_row(component, status, notes)

    # Status-doc drift: keep docs/project-status.md "Stable" claims honest against the
    # actual tests/unit/ layout. Placed with the knowledge rows so it also runs on the
    # ollama / unsupported-provider early-return paths.
    for component, status, notes in check_status_doc_drift(project_root):
        table.add_row(component, status, notes)

    for component, status, notes in check_coverage_floor(project_root):
        table.add_row(component, status, notes)

    for component, status, notes in check_mypy_status(project_root):
        table.add_row(component, status, notes)

    for component, status, notes in check_mapping_stack(project_root):
        table.add_row(component, status, notes)

    for component, status, notes in check_execution_containment(project_root, config=config):
        table.add_row(component, status, notes)

    # Local-monitor verification safety: flag explicit config that disables the
    # ensembling a local monitor/reviewer needs (see check docstring for the data).
    for component, status, notes in check_local_monitor_sampling(project_root, config=config):
        table.add_row(component, status, notes)

    try:
        provider = config.models.provider if config is not None else "openrouter"
    except Exception:
        provider = "openrouter"
    try:
        provider = validate_model_provider(provider)
    except ValueError:
        supported = ", ".join(SUPPORTED_MODEL_PROVIDERS)
        table.add_row(
            "models.provider",
            "[red]Unsupported[/red]",
            f"{provider} is configured, but this runtime supports: {supported}.",
        )
        _add_logging_row(table, project_root)
        console.print(table)
        _print_maturity_table()
        return
    if provider == "ollama":
        # Use the provider's own resolver so the displayed URL reflects OLLAMA_HOST
        # (with scheme/-/v1 normalization), not just OLLAMA_BASE_URL.
        from devcouncil.execution.prompt_builder import MAX_PROMPT_CHARS
        from devcouncil.llm.provider import OllamaProvider

        base_url = OllamaProvider._resolve_base_url()
        num_ctx = OllamaProvider._resolve_num_ctx()
        # DevCouncil's planning prompts reach ~MAX_PROMPT_CHARS chars (~4 chars/token);
        # recommend a context window that covers the prompt plus headroom for output.
        recommended_ctx = 16384
        min_ctx = max(8192, (MAX_PROMPT_CHARS // 4))
        table.add_row(
            "OLLAMA",
            "[green]OK[/green]",
            f"Local provider; no API key required (server: {base_url}).",
        )

        # Is the local Ollama server actually up? A native /api/version probe is
        # cheap and turns the most common failure ("provider configured but
        # `ollama serve` not running") into an actionable row instead of a
        # connection traceback on the first model call.
        reachable, detail = _probe_ollama(base_url)
        if reachable:
            table.add_row("OLLAMA server", "[green]OK[/green]", detail)
        else:
            table.add_row(
                "OLLAMA server",
                "[yellow]WARN[/yellow]",
                f"{detail} Start it with 'ollama serve' (or 'brew services start ollama').",
            )

        # A reachable server with the configured model NOT pulled is the most common
        # "all-green doctor, 404 on first call" trap. Verify the role models exist locally.
        if reachable and config is not None:
            try:
                configured_models = sorted(
                    {role.model for role in config.models.roles.values() if role.model}
                )
            except Exception:
                configured_models = []
            queried_ok, pulled = _probe_ollama_models(base_url)
            if not queried_ok:
                table.add_row(
                    "OLLAMA models",
                    "[yellow]WARN[/yellow]",
                    "Could not list pulled models (/api/tags). Ensure each configured model is pulled.",
                )
            elif not configured_models:
                table.add_row(
                    "OLLAMA models",
                    "[yellow]WARN[/yellow]",
                    "No role models configured; run 'dev setup --provider ollama --model <model>'.",
                )
            else:
                missing = [m for m in configured_models if not _ollama_model_present(m, pulled)]
                if missing:
                    pulls = "; ".join(f"ollama pull {m}" for m in missing)
                    table.add_row(
                        "OLLAMA models",
                        "[yellow]WARN[/yellow]",
                        f"Configured model(s) not pulled: {', '.join(missing)}. Pull first: {pulls}.",
                    )
                else:
                    table.add_row(
                        "OLLAMA models",
                        "[green]OK[/green]",
                        f"All configured models present locally ({', '.join(configured_models)}).",
                    )

        if num_ctx is None:
            # Only reachable via the explicit OLLAMA_NUM_CTX=0 opt-out: unset now
            # resolves to the provider's raised DEFAULT_NUM_CTX, never the server default.
            table.add_row(
                "OLLAMA num_ctx",
                "[yellow]WARN[/yellow]",
                f"OLLAMA_NUM_CTX=0 opts into Ollama's small server default (~2048-4096), "
                f"which will truncate DevCouncil's large planning prompts. Unset it (default "
                f"{OllamaProvider.DEFAULT_NUM_CTX}) or set OLLAMA_NUM_CTX={recommended_ctx}.",
            )
        elif num_ctx < min_ctx:
            table.add_row(
                "OLLAMA num_ctx",
                "[yellow]WARN[/yellow]",
                f"OLLAMA_NUM_CTX={num_ctx} may be too small for planning prompts "
                f"(~{MAX_PROMPT_CHARS // 4} tokens); recommend >= {recommended_ctx}.",
            )
        else:
            table.add_row(
                "OLLAMA num_ctx",
                "[green]OK[/green]",
                f"context window = {num_ctx} tokens (auto-grows per request up to "
                f"{OllamaProvider._resolve_max_num_ctx()} for oversized prompts; cap with OLLAMA_MAX_NUM_CTX).",
            )

        think = OllamaProvider._resolve_think()
        if think is None:
            table.add_row(
                "OLLAMA think",
                "[green]OK[/green]",
                "server default. On thinking models (qwen3/deepseek-r1/...) the reasoning "
                "channel can dominate review latency; OLLAMA_THINK=false trades some check "
                "quality for much faster verification calls.",
            )
        else:
            table.add_row("OLLAMA think", "[green]OK[/green]", f"OLLAMA_THINK={'true' if think else 'false'}.")

        # Local model size is bounded by host memory — unified RAM on Apple Silicon,
        # VRAM on a discrete-GPU box, system RAM otherwise. Surface a model that will
        # actually fit on this host (any OS), not a one-size default.
        from devcouncil import hardware

        host = hardware.describe_host()
        table.add_row(
            host.platform_label,
            "[green]OK[/green]",
            f"{host.chip_label}, {host.memory_label}. "
            f"Recommended local model: {host.recommended_ollama_model} "
            f"(dev setup --provider ollama --model {host.recommended_ollama_model}).",
        )

        _add_logging_row(table, project_root)
        console.print(table)
        _print_maturity_table()
        return
    env_var = provider_api_key_env_var(provider)
    local_secrets = load_local_secrets(project_root)
    if os.environ.get(env_var):
        table.add_row(env_var, "[green]OK[/green]", f"Found in environment for {provider}.")
    elif local_secrets.get(env_var):
        table.add_row(env_var, "[green]OK[/green]", f"Found in .devcouncil/secrets.env for {provider}.")
    elif provider == "vertexai" and get_gcloud_access_token():
        table.add_row(env_var, "[green]OK[/green]", "Resolvable via gcloud auth print-access-token.")
        table.caption = "Resolvable via gcloud auth print-access-token."
    else:
        table.add_row(env_var, "[yellow]Missing[/yellow]", f"Required if using {provider} provider. Run 'dev setup'.")

    if provider == "vertexai":
        project = (
            os.environ.get("VERTEXAI_PROJECT")
            or os.environ.get("GOOGLE_CLOUD_PROJECT")
            or local_secrets.get("VERTEXAI_PROJECT")
            or local_secrets.get("GOOGLE_CLOUD_PROJECT")
        )
        if project:
            source = "environment" if os.environ.get("VERTEXAI_PROJECT") or os.environ.get("GOOGLE_CLOUD_PROJECT") else ".devcouncil/secrets.env"
            table.add_row("VERTEXAI_PROJECT", "[green]OK[/green]", f"Found in {source}.")
        else:
            table.add_row(
                "VERTEXAI_PROJECT",
                "[yellow]Missing[/yellow]",
                "Required for vertexai. Run 'dev setup --provider vertexai --vertex-project PROJECT_ID'.",
            )

        location = os.environ.get("VERTEXAI_LOCATION") or local_secrets.get("VERTEXAI_LOCATION", "global")
        table.add_row("VERTEXAI_LOCATION", "[green]OK[/green]", location)

    _add_logging_row(table, project_root)
    console.print(table)
    _print_maturity_table()


@app.callback(invoke_without_command=True)
def doctor(
    ctx: typer.Context,
    project_root: Path = typer.Option(
        Path("."),
        "--project-root",
        help="Repository root containing .devcouncil/config.yaml.",
    ),
):
    """
    Check the environment for DevCouncil prerequisites.
    """
    if ctx.invoked_subcommand is not None:
        return

    root = project_root.expanduser().resolve()
    from devcouncil.telemetry.logging_setup import set_log_dir
    set_log_dir(root)
    logger.info("dev doctor: project_root=%s", root)
    with log_stage("doctor", project_root=root):
        log_step("doctor/1: running environment checks", project_root=root, trace=True)
        render_doctor_check(root)
        log_step("doctor/complete", project_root=root, trace=True)
