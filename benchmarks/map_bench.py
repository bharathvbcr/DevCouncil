#!/usr/bin/env python
"""`dev map` performance benchmark — staged, reproducible, and honest about scope.

This measures the *mapping engine*, not the council loop (see `run_bench.py` for
that). It exists because `dev map` is on the hot path of every agent turn: the
map is rebuilt by hooks, by the watcher, and by hand, so a second of avoidable
overhead is a second paid hundreds of times a day.

**Staged, not just end-to-end.** A single wall-clock number for `dev map` cannot
tell you whether the cost is indexing, artifact generation, or the Python
wrapper around them — and those have wildly different fixes. Each stage is timed
in isolation against a scratch store so one slow stage cannot hide behind
another:

| stage       | what it measures                                            |
|-------------|-------------------------------------------------------------|
| `cold`      | full index of every discovered file into an empty store      |
| `warm`      | incremental build that finds no changed file (the common case)|
| `touch`     | incremental build after one file's mtime/content changes     |
| `manifest`  | writing `repo_map.json` + `code_graph.json` from the store    |
| `e2e`       | the real `dev map` command, wrapper and all                   |
| `search`    | FTS lookup against the persisted graph                        |
| `impact`    | reverse blast radius of the most-called symbol in the corpus  |
| `path`      | `trace from to` across a real call edge                       |
| `dead`      | the whole-corpus liveness report                              |
| `growth`    | store size per generation across repeated churned builds      |

**Peak RSS beside wall time.** A stage that got faster by holding the whole
corpus in memory has not got better, and until now nothing here would have
said so. Each build and query stage records peak resident bytes measured with
`/usr/bin/time`, through `rust-port/tools/peak_rss.sh` — the same helper
`verify.sh` uses, sourced rather than re-implemented, because two spellings of
"peak RSS" that disagree is worse than one. An unavailable reading is a stage
failure, never a silent zero.

**A query stage that matched nothing is a failed stage.** `search`, `impact`
and `path` each run once untimed and assert their answer is non-empty before
any timing begins. Timing a query that returns no rows measures the empty path
and reports it as the cost of the work — K4's shape, in a benchmark.

**Variance, not a number.** Every stage reports min, median, max, mean and
relative spread over `--repeat` runs (>= 3 required). The minimum is the
headline because wall-clock has a hard floor and an unbounded tail, but a run
whose spread is wide is a run whose minimum should not be quoted alone.

**Minimum, not mean.** Each stage runs `--repeat` times and the *minimum* is
reported. Wall-clock latency has a hard floor and an unbounded tail: background
load can only ever make a run slower, so the mean of a noisy sample measures the
machine's mood, while the minimum measures the code. Median and max are recorded
alongside it so a pathological tail is still visible rather than discarded.

**A scratch store, always.** Every stage writes to a temporary database and
temporary artifacts, never to the target repository's `.devcouncil/`. Benchmarks
that mutate the tree they measure are not repeatable, and this one is run
against real working repositories.

Usage:
    python benchmarks/map_bench.py                      # this repo
    python benchmarks/map_bench.py --repo ~/src/big     # any tree
    python benchmarks/map_bench.py --stages cold,warm --repeat 5
    python benchmarks/map_bench.py --baseline results/map/<ts>.json   # compare
    python benchmarks/map_bench.py --synthetic 5000     # generated corpus
    python benchmarks/map_bench.py --stages growth --growth-rounds 12
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import sqlite3
import statistics
import subprocess
import sys
import tempfile
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Callable, Dict, List, Optional

REPO_ROOT = Path(__file__).resolve().parent.parent
RESULTS_DIR = Path(__file__).resolve().parent / "results" / "map"

ALL_STAGES = (
    "cold",
    "warm",
    "touch",
    "manifest",
    "e2e",
    "search",
    "impact",
    "path",
    "dead",
    "growth",
)

# Stages whose numbers describe the *engine*. `e2e` is excluded because it
# measures the Python wrapper as much as the kernel, and `growth` because it
# reports sizes rather than a single duration.
DURATION_STAGES = tuple(s for s in ALL_STAGES if s != "growth")


class BenchError(RuntimeError):
    """A stage could not be measured. Never downgraded to a zero or a skip."""


# --------------------------------------------------------------------------
# process helpers
# --------------------------------------------------------------------------


def run(argv: List[str], *, cwd: Path, timeout: float = 1800.0) -> subprocess.CompletedProcess:
    """Run a command, raising with captured output on failure.

    A benchmark that silently times a *failed* command reports the cost of the
    error path as if it were the cost of the work, which is worse than no
    number at all.
    """
    try:
        completed = subprocess.run(
            argv, cwd=str(cwd), capture_output=True, text=True, timeout=timeout
        )
    except subprocess.TimeoutExpired as exc:
        raise BenchError(f"timed out after {timeout:.0f}s: {' '.join(argv)}") from exc
    except OSError as exc:
        raise BenchError(f"could not run {argv[0]}: {exc}") from exc
    if completed.returncode != 0:
        tail = (completed.stderr or completed.stdout or "").strip().splitlines()[-10:]
        raise BenchError(
            f"exit {completed.returncode}: {' '.join(argv)}\n" + "\n".join(tail)
        )
    return completed


def find_devmap() -> str:
    """Locate the devmap binary, preferring this checkout's release build.

    Deliberately *not* `DevMapClient._find_devmap_binary`: the benchmark must
    measure the binary in this working tree, not whichever older one happens to
    be first on `PATH`. A stale `~/.cargo/bin/devmap` reporting the same version
    string is exactly the confusion this avoids.
    """
    candidates = [
        REPO_ROOT / "rust-port" / "target" / "release" / "devmap",
        REPO_ROOT / "rust-port" / "target" / "debug" / "devmap",
    ]
    for candidate in candidates:
        if candidate.is_file() and os.access(candidate, os.X_OK):
            return str(candidate)
    found = shutil.which("devmap")
    if found:
        return found
    raise BenchError(
        "no devmap binary found — build it with "
        "`cargo build --release -p devmap-cli` in rust-port/"
    )


PEAK_RSS_HELPER = REPO_ROOT / "rust-port" / "tools" / "peak_rss.sh"


def peak_rss_bytes(argv: List[str], *, cwd: Path, timeout: float = 1800.0) -> int:
    """Peak resident bytes of one command, via `rust-port/tools/peak_rss.sh`.

    Sourced rather than re-implemented. `/usr/bin/time` spells the flag `-l` on
    BSD/macOS and `-v` on GNU, and reports bytes on one and kibibytes on the
    other; that probe already exists for `verify.sh`, and a second copy here
    would be a second thing to get wrong. Per SC15, a policy that lives in two
    places goes stale in the copy nobody runs.

    An unmeasurable reading raises. A benchmark that reports 0 MiB for a build
    it could not measure is exactly the "a check that could not run reports what
    a check that ran reports" failure this repository keeps closing.
    """
    if not PEAK_RSS_HELPER.is_file():
        raise BenchError(f"peak-RSS helper missing: {PEAK_RSS_HELPER}")
    with tempfile.NamedTemporaryFile(prefix="devmap-rss-", suffix=".txt") as capture:
        script = f'. "{PEAK_RSS_HELPER}"; peak_rss_bytes "{capture.name}" "$@"'
        completed = subprocess.run(
            ["bash", "-c", script, "_", *argv],
            cwd=str(cwd),
            capture_output=True,
            text=True,
            timeout=timeout,
        )
    if completed.returncode != 0:
        tail = (completed.stderr or "").strip().splitlines()[-6:]
        raise BenchError(
            "could not measure peak RSS for "
            f"{' '.join(argv)}\n" + "\n".join(tail)
        )
    try:
        return int(completed.stdout.strip())
    except ValueError as exc:
        raise BenchError(
            f"peak-RSS helper returned {completed.stdout!r}, not a byte count"
        ) from exc


def binary_identity(binary: str) -> Dict[str, object]:
    """Record what was actually measured, so two results can be compared.

    Size and mtime, not just the version string: every build of this workspace
    reports `devmap 0.1.0`, so a version alone cannot distinguish an optimized
    binary from the one it replaced.
    """
    path = Path(binary)
    try:
        stat = path.stat()
        size, mtime = stat.st_size, stat.st_mtime
    except OSError:
        size, mtime = -1, -1.0
    try:
        version = subprocess.run(
            [binary, "--version"], capture_output=True, text=True, timeout=30
        ).stdout.strip()
    except (OSError, subprocess.SubprocessError):
        version = "unknown"
    return {
        "path": str(path),
        "version": version,
        "size_bytes": size,
        "mtime": datetime.fromtimestamp(mtime, timezone.utc).isoformat() if mtime > 0 else None,
    }


# --------------------------------------------------------------------------
# corpus description
# --------------------------------------------------------------------------

_CODE_SUFFIXES = {
    ".py", ".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs", ".go", ".rs", ".java",
    ".rb", ".swift", ".kt", ".kts", ".c", ".cc", ".cpp", ".cxx", ".h", ".hpp",
    ".m", ".mm", ".cs", ".php", ".scala", ".sh", ".bash", ".zsh", ".lua", ".r",
    ".dart", ".ex", ".exs", ".erl", ".sol", ".sql", ".svelte", ".astro", ".nix",
}


def describe_corpus(repo: Path) -> Dict[str, object]:
    """Count tracked files and code files so throughput has a denominator.

    Falls back to a filesystem walk outside a git repository — the engine
    indexes trees, not just checkouts, and a benchmark that only runs on git
    repos cannot measure the case it is most often asked about.
    """
    try:
        out = subprocess.run(
            ["git", "ls-files"], cwd=str(repo), capture_output=True, text=True, timeout=120
        )
        files = [f for f in out.stdout.splitlines() if f] if out.returncode == 0 else []
    except (OSError, subprocess.SubprocessError):
        files = []
    if not files:
        files = [
            str(p.relative_to(repo))
            for p in repo.rglob("*")
            if p.is_file() and ".git" not in p.parts
        ]
    code = [f for f in files if Path(f).suffix.lower() in _CODE_SUFFIXES]
    total_bytes = 0
    for rel in code:
        try:
            total_bytes += (repo / rel).stat().st_size
        except OSError:
            pass
    return {
        "tracked_files": len(files),
        "code_files": len(code),
        "code_bytes": total_bytes,
    }


# --------------------------------------------------------------------------
# synthetic corpus
# --------------------------------------------------------------------------


def generate_synthetic(root: Path, files: int, *, fanout: int = 6) -> None:
    """Write `files` Python modules with a deterministic call graph.

    A real repository is the corpus that matters, but it is one size and it
    changes under the benchmark. A generated tree is the only way to ask "what
    does this cost at 4x the files" and get an answer that is not confounded by
    also being different code.

    The shape is chosen so the graph is not trivial: each module defines eight
    functions and calls `fanout` functions in *other* modules, picked by a fixed
    stride, so resolution has real cross-file work and the call graph is
    connected rather than a pile of islands. Names repeat across modules on
    purpose — that is what produces the ambiguous fan-out SC4 describes, and a
    corpus without it would flatter the resolver.
    """
    if files < 1:
        raise BenchError("--synthetic needs at least one file")
    package = root / "synth"
    package.mkdir(parents=True, exist_ok=True)
    (package / "__init__.py").write_text("", encoding="utf-8")
    for index in range(files):
        lines = [f"\"\"\"Generated module {index}.\"\"\"", ""]
        for offset in range(1, fanout + 1):
            other = (index + offset * 7) % files
            lines.append(f"from synth.m{other} import shared_{offset}")
        lines.append("")
        for slot in range(8):
            lines.append(f"def shared_{slot}(value):")
            lines.append(f"    return value + {slot}")
            lines.append("")
        lines.append(f"def entry_{index}(value):")
        lines.append("    total = value")
        for offset in range(1, fanout + 1):
            lines.append(f"    total = shared_{offset}(total)")
        lines.append("    return total")
        lines.append("")
        (package / f"m{index}.py").write_text("\n".join(lines), encoding="utf-8")


# --------------------------------------------------------------------------
# timing
# --------------------------------------------------------------------------


def time_stage(
    fn: Callable[[], None],
    *,
    repeat: int,
    setup: Optional[Callable[[], None]] = None,
) -> Dict[str, float]:
    """Run `fn` `repeat` times, returning min/median/max seconds.

    `setup` runs before each iteration and is *not* timed — that is what makes
    `cold` measurable more than once.
    """
    samples: List[float] = []
    for _ in range(repeat):
        if setup is not None:
            setup()
        start = time.perf_counter()
        fn()
        samples.append(time.perf_counter() - start)
    return summarize(samples)


def summarize(samples: List[float]) -> Dict[str, float]:
    """min/median/max/mean/stdev and relative spread for one stage.

    The minimum stays the headline for the reason above, but a minimum quoted
    alone is a single number pretending to be a measurement. `rel_spread` —
    `(max - min) / min` — is the one figure that says whether the headline can
    be trusted: at 0.05 the machine was quiet, at 0.8 the number describes the
    machine's mood and any comparison against a baseline is noise.
    """
    ordered = sorted(samples)
    low = ordered[0]
    return {
        "min_s": low,
        "median_s": statistics.median(ordered),
        "max_s": ordered[-1],
        "mean_s": statistics.fmean(ordered),
        "stdev_s": statistics.stdev(ordered) if len(ordered) > 1 else 0.0,
        "rel_spread": (ordered[-1] - low) / low if low > 0 else 0.0,
        "samples": len(ordered),
    }


class MapBench:
    def __init__(self, repo: Path, binary: str, repeat: int, workdir: Path) -> None:
        self.repo = repo
        self.binary = binary
        self.repeat = repeat
        self.workdir = workdir
        self.db = workdir / "bench.sqlite"
        self.map_out = workdir / "repo_map.json"
        self.graph_out = workdir / "code_graph.json"
        self._target_cache: Optional[Dict[str, str]] = None

    # -- store lifecycle ---------------------------------------------------

    def _reset_store(self) -> None:
        for suffix in ("", "-wal", "-shm"):
            path = Path(str(self.db) + suffix)
            try:
                path.unlink()
            except FileNotFoundError:
                pass
            except OSError as exc:
                raise BenchError(f"cannot reset scratch store {path}: {exc}") from exc

    def _base_argv(self) -> List[str]:
        return [self.binary, "--db", str(self.db), "--progress", "never"]

    def _build(self) -> None:
        run([*self._base_argv(), "build", str(self.repo)], cwd=self.repo)

    def _query(self, *args: str) -> str:
        return run([*self._base_argv(), *args], cwd=self.repo).stdout

    # -- query targets, discovered from the store --------------------------

    def _targets(self) -> Dict[str, str]:
        """Query arguments drawn from the store the benchmark just built.

        Hard-coded arguments would make every stage corpus-specific, and — far
        worse — would silently degrade to timing the empty path the moment the
        corpus changed. These are read back from the graph, so the query
        always has something to find, and picked deterministically (ties broken
        by name) so two runs measure the same work.
        """
        if self._target_cache is not None:
            return self._target_cache
        try:
            conn = sqlite3.connect(f"file:{self.db}?mode=ro", uri=True)
        except sqlite3.Error as exc:
            raise BenchError(f"cannot read the scratch store: {exc}") from exc
        try:
            latest = conn.execute("SELECT MAX(id) FROM generations").fetchone()[0]
            if latest is None:
                raise BenchError("the scratch store holds no generation to query")
            busiest = conn.execute(
                """SELECT source_symbol, target_symbol, COUNT(*) c
                     FROM generation_edges
                    WHERE generation_id = ? AND edge_kind = 'Calls'
                      AND source_symbol <> target_symbol
                 GROUP BY source_symbol, target_symbol
                 ORDER BY c DESC, target_symbol, source_symbol
                    LIMIT 1""",
                (latest,),
            ).fetchone()
            if not busiest:
                raise BenchError("the scratch store holds no call edge to trace")
            common_name = conn.execute(
                """SELECT name, COUNT(*) c FROM generation_nodes
                    WHERE generation_id = ? AND LENGTH(name) >= 4
                 GROUP BY name ORDER BY c DESC, name LIMIT 1""",
                (latest,),
            ).fetchone()
            if not common_name:
                raise BenchError("the scratch store holds no symbol name to search")
        finally:
            conn.close()
        self._target_cache = {
            "search": common_name[0],
            "impact": busiest[1],
            "trace_from": busiest[0],
            "trace_to": busiest[1],
        }
        return self._target_cache

    def _assert_answers(self, label: str, output: str) -> None:
        """Refuse to time a query whose answer is empty.

        A query surface that returns nothing runs the *fast* path: no rows to
        format, no source to read. Timing that and reporting it as the cost of
        `impact` is K4's failure with a stopwatch attached — the number is real,
        internally consistent, and describes work nobody asked for.
        """
        body = output.strip()
        if not body:
            raise BenchError(f"`{label}` produced no output; refusing to time an empty answer")
        lowered = body.lower()
        if "no matches" in lowered or "no results" in lowered or "not found" in lowered:
            raise BenchError(
                f"`{label}` matched nothing ({body.splitlines()[0][:120]!r}); "
                "refusing to time an empty answer"
            )

    def _manifest(self) -> None:
        run(
            [
                *self._base_argv(),
                "manifest",
                "--output",
                str(self.map_out),
                "--graph-output",
                str(self.graph_out),
                "--force",
            ],
            cwd=self.repo,
        )

    # -- stages ------------------------------------------------------------

    def stage_cold(self) -> Dict[str, float]:
        result = time_stage(self._build, repeat=self.repeat, setup=self._reset_store)
        self._reset_store()
        result["rss_bytes"] = float(
            peak_rss_bytes([*self._base_argv(), "build", str(self.repo)], cwd=self.repo)
        )
        return result

    def stage_warm(self) -> Dict[str, float]:
        self._reset_store()
        self._build()  # prime; not timed
        result = time_stage(self._build, repeat=self.repeat)
        result["rss_bytes"] = float(
            peak_rss_bytes([*self._base_argv(), "build", str(self.repo)], cwd=self.repo)
        )
        return result

    def stage_touch(self) -> Dict[str, float]:
        """Incremental build after one real content change.

        The file is a scratch file created inside the repo and removed
        afterwards, and its content changes every iteration. Touching only the
        mtime would measure the discovery fast-path rather than reindexing, and
        reusing identical content lets a content-addressed store skip the work
        the stage exists to time.
        """
        probe = self.repo / "_devmap_bench_probe.py"
        counter = {"n": 0}

        def setup() -> None:
            counter["n"] += 1
            probe.write_text(
                f"def bench_probe_{counter['n']}():\n"
                f"    return {counter['n']}\n",
                encoding="utf-8",
            )

        self._reset_store()
        setup()
        self._build()  # prime with the probe present; not timed
        try:
            result = time_stage(self._build, repeat=self.repeat, setup=setup)
            setup()
            result["rss_bytes"] = float(
                peak_rss_bytes([*self._base_argv(), "build", str(self.repo)], cwd=self.repo)
            )
            return result
        finally:
            try:
                probe.unlink()
            except OSError:
                pass

    def stage_manifest(self) -> Dict[str, float]:
        self._reset_store()
        self._build()
        result = time_stage(self._manifest, repeat=self.repeat)
        for artifact, key in ((self.map_out, "map"), (self.graph_out, "graph")):
            try:
                result[f"{key}_bytes"] = float(artifact.stat().st_size)
            except OSError:
                result[f"{key}_bytes"] = -1.0
        return result

    # -- query stages ------------------------------------------------------

    def _prime(self) -> None:
        """A store with exactly one generation, built once for a query stage."""
        if self._target_cache is None:
            self._reset_store()
            self._build()
            self._targets()

    def _query_stage(self, label: str, argv_for: Callable[[Dict[str, str]], List[str]]) -> Dict[str, float]:
        self._prime()
        argv = argv_for(self._targets())
        self._assert_answers(label, self._query(*argv))  # untimed, and it must answer
        result = time_stage(lambda: self._query(*argv), repeat=self.repeat)
        result["rss_bytes"] = float(peak_rss_bytes([*self._base_argv(), *argv], cwd=self.repo))
        result["argument"] = " ".join(argv[1:])
        return result

    def stage_search(self) -> Dict[str, float]:
        return self._query_stage("search", lambda t: ["search", t["search"]])

    def stage_impact(self) -> Dict[str, float]:
        return self._query_stage("impact", lambda t: ["impact", t["impact"]])

    def stage_path(self) -> Dict[str, float]:
        return self._query_stage(
            "path", lambda t: ["trace", t["trace_from"], t["trace_to"]]
        )

    def stage_dead(self) -> Dict[str, float]:
        # `dead` legitimately reports nothing on a corpus with no dead code, so
        # it is the one query stage without an emptiness assertion — an empty
        # `dead` is a real answer about the tree, not a query that missed.
        self._prime()
        result = time_stage(lambda: self._query("dead"), repeat=self.repeat)
        result["rss_bytes"] = float(
            peak_rss_bytes([*self._base_argv(), "dead"], cwd=self.repo)
        )
        return result

    # -- growth ------------------------------------------------------------

    def stage_growth(self, rounds: int = 10) -> Dict[str, float]:
        """Store bytes per generation across repeated churned builds.

        Write amplification is recorded in `STATUS.md` under SC1/SC2 as a
        *label*. This turns it into a curve: one file's content changes between
        builds, so every build is a genuinely new generation, and the file size,
        the live payload and the freelist are read after each.

        Two things the curve has to separate, which a single "the store grew"
        number cannot:

        * **Growth that retention bounds.** `GENERATION_RETENTION` keeps two
          generations, so the curve should climb for the first few rounds and
          then flatten. A curve that keeps climbing means retention is not
          holding.
        * **Free space that reclaim leaves behind.** `vacuum_if_needed` moves
          pages to the freelist and truncates; `freelist_ratio` after each round
          says whether it actually did. K5 is the precedent: the reclaim policy
          read a pre-checkpoint counter and reported a 0 ms vacuum as success on
          a store that was one-third garbage, so this reads `freelist_count` and
          `page_count` from the file itself rather than trusting the report.
        """
        probe = self.repo / "_devmap_bench_growth.py"
        self._reset_store()
        self._target_cache = None
        curve: List[Dict[str, float]] = []
        try:
            for round_index in range(rounds):
                probe.write_text(
                    f"def bench_growth_{round_index}():\n"
                    f"    return {round_index}\n",
                    encoding="utf-8",
                )
                started = time.perf_counter()
                self._build()
                elapsed = time.perf_counter() - started
                curve.append({"round": float(round_index), "build_s": elapsed, **self._store_shape()})
        finally:
            try:
                probe.unlink()
            except OSError:
                pass

        first, last = curve[0], curve[-1]
        # The plateau is measured over the second half, because the first few
        # rounds are the store filling up rather than the steady state.
        tail = curve[len(curve) // 2 :]
        plateau = [row["file_bytes"] for row in tail]
        return {
            "rounds": float(rounds),
            "curve": curve,  # type: ignore[dict-item]
            "first_generation_bytes": first["file_bytes"],
            "final_bytes": last["file_bytes"],
            "growth_ratio": last["file_bytes"] / first["file_bytes"] if first["file_bytes"] else 0.0,
            "plateau_spread": (max(plateau) - min(plateau)) / min(plateau) if min(plateau) else 0.0,
            "final_freelist_ratio": last["freelist_ratio"],
            "max_freelist_ratio": max(row["freelist_ratio"] for row in curve),
            # `min_s` so the generic reporting path has something to print; the
            # curve above is what this stage is actually for.
            "min_s": min(row["build_s"] for row in curve),
            "median_s": statistics.median([row["build_s"] for row in curve]),
            "max_s": max(row["build_s"] for row in curve),
            "samples": float(rounds),
        }

    def _store_shape(self) -> Dict[str, float]:
        """File size, live payload and free pages, read from the store itself."""
        total = 0
        for suffix in ("", "-wal"):
            try:
                total += Path(str(self.db) + suffix).stat().st_size
            except OSError:
                pass
        try:
            conn = sqlite3.connect(f"file:{self.db}?mode=ro", uri=True)
        except sqlite3.Error as exc:
            raise BenchError(f"cannot read the scratch store: {exc}") from exc
        try:
            freelist = conn.execute("PRAGMA freelist_count").fetchone()[0]
            pages = conn.execute("PRAGMA page_count").fetchone()[0]
            page_size = conn.execute("PRAGMA page_size").fetchone()[0]
            generations = conn.execute("SELECT COUNT(*) FROM generations").fetchone()[0]
            payload = conn.execute(
                "SELECT COALESCE(SUM(LENGTH(extraction_json)), 0) FROM generation_files"
            ).fetchone()[0]
            cached = conn.execute(
                "SELECT COUNT(*), COALESCE(SUM(LENGTH(payload_json)), 0) FROM extraction_cache"
            ).fetchone()
        finally:
            conn.close()
        return {
            "file_bytes": float(total),
            "page_bytes": float(pages * page_size),
            "freelist_bytes": float(freelist * page_size),
            "freelist_ratio": (freelist / pages) if pages else 0.0,
            "generations": float(generations),
            "extraction_json_bytes": float(payload),
            "extraction_cache_rows": float(cached[0]),
            "extraction_cache_bytes": float(cached[1]),
        }

    def stage_e2e(self) -> Dict[str, float]:
        """The real `dev map`, wrapper included.

        Run through `python -m devcouncil.cli.main` rather than the `dev`
        console script so the interpreter and the source tree being measured
        are the ones in this checkout, not whatever `dev` on PATH was installed
        from. Writes to the target repo's own `.devcouncil/` — that is what the
        command does, and stubbing it out would measure something else.
        """
        argv = [sys.executable, "-m", "devcouncil.cli.main", "map"]

        def once() -> None:
            run(argv, cwd=self.repo)

        once()  # prime the store so this measures steady state, not a cold index
        return time_stage(once, repeat=self.repeat)


# --------------------------------------------------------------------------
# reporting
# --------------------------------------------------------------------------


def _fmt(seconds: float) -> str:
    return f"{seconds * 1000:.0f}ms" if seconds < 1 else f"{seconds:.2f}s"


def render_markdown(payload: Dict[str, object]) -> str:
    corpus = payload["corpus"]
    stages: Dict[str, Dict[str, float]] = payload["stages"]  # type: ignore[assignment]
    code_files = int(corpus["code_files"])  # type: ignore[index]

    lines = [
        "# `dev map` performance",
        "",
        f"- repo: `{payload['repo']}`",
        f"- files: {corpus['tracked_files']} tracked, {code_files} code "  # type: ignore[index]
        f"({int(corpus['code_bytes']) / 1e6:.1f} MB)",  # type: ignore[index]
        f"- binary: `{payload['binary']['path']}` "  # type: ignore[index]
        f"({payload['binary']['version']}, "  # type: ignore[index]
        f"{int(payload['binary']['size_bytes']) / 1e6:.1f} MB)",  # type: ignore[index]
        f"- repeats: {payload['repeat']} (minimum reported)",
        f"- generated: {payload['generated_at']}",
        "",
        "| stage | min | median | max | spread | peak RSS | throughput |",
        "|---|---|---|---|---|---|---|",
    ]
    for name in DURATION_STAGES:
        stat = stages.get(name)
        if not stat:
            continue
        rate = (
            f"{code_files / stat['min_s']:.0f} files/s"
            if name in ("cold",) and stat["min_s"] > 0
            else "—"
        )
        spread = f"{stat['rel_spread'] * 100:.0f}%" if "rel_spread" in stat else "—"
        rss = (
            f"{stat['rss_bytes'] / 1024 / 1024:.0f} MiB"
            if stat.get("rss_bytes")
            else "not measured"
        )
        lines.append(
            f"| `{name}` | {_fmt(stat['min_s'])} | {_fmt(stat['median_s'])} "
            f"| {_fmt(stat['max_s'])} | {spread} | {rss} | {rate} |"
        )

    noisy = [
        name
        for name in DURATION_STAGES
        if stages.get(name, {}).get("rel_spread", 0.0) > 0.5
    ]
    if noisy:
        lines += [
            "",
            f"> Spread above 50% on {', '.join(f'`{n}`' for n in noisy)} — those "
            "minima describe a busy machine as much as the code, and should not "
            "be compared against a baseline taken on a quiet one.",
        ]

    growth = stages.get("growth")
    if growth and growth.get("curve"):
        curve = growth["curve"]  # type: ignore[index]
        lines += [
            "",
            "## Store growth per generation",
            "",
            "One file's content changes between builds, so every round is a real "
            "new generation.",
            "",
            "| round | generations | store | live `extraction_json` | free | free % | build |",
            "|---|---|---|---|---|---|---|",
        ]
        for row in curve:  # type: ignore[union-attr]
            lines.append(
                f"| {int(row['round'])} | {int(row['generations'])} "
                f"| {row['file_bytes'] / 1e6:.1f} MB "
                f"| {row['extraction_json_bytes'] / 1e6:.1f} MB "
                f"| {row['freelist_bytes'] / 1e6:.1f} MB "
                f"| {row['freelist_ratio'] * 100:.1f}% "
                f"| {_fmt(row['build_s'])} |"
            )
        lines += [
            "",
            f"- first generation: {growth['first_generation_bytes'] / 1e6:.1f} MB; "
            f"after {int(growth['rounds'])} rounds: {growth['final_bytes'] / 1e6:.1f} MB "
            f"({growth['growth_ratio']:.2f}x)",
            f"- steady-state spread over the second half: "
            f"{growth['plateau_spread'] * 100:.1f}% "
            "(a flat tail is retention holding; a rising one is not)",
            f"- freelist after the last build: "
            f"{growth['final_freelist_ratio'] * 100:.1f}%, peak "
            f"{growth['max_freelist_ratio'] * 100:.1f}% — read from the file, not "
            "from the vacuum's own report (K5)",
        ]

    manifest = stages.get("manifest")
    if manifest and manifest.get("graph_bytes", -1) > 0:
        lines += [
            "",
            f"Artifacts: `repo_map.json` {int(manifest['map_bytes']) / 1e6:.2f} MB, "
            f"`code_graph.json` {int(manifest['graph_bytes']) / 1e6:.2f} MB",
        ]

    comparison = payload.get("comparison")
    if comparison:
        lines += ["", "## vs baseline", "", "| stage | baseline | now | change |", "|---|---|---|---|"]
        for name, delta in comparison.items():  # type: ignore[union-attr]
            lines.append(
                f"| `{name}` | {_fmt(delta['baseline_s'])} | {_fmt(delta['current_s'])} "
                f"| {delta['pct']:+.1f}% |"
            )
    return "\n".join(lines) + "\n"


def compare(current: Dict[str, Dict[str, float]], baseline_path: Path) -> Dict[str, Dict[str, float]]:
    try:
        baseline = json.loads(baseline_path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as exc:
        raise BenchError(f"cannot read baseline {baseline_path}: {exc}") from exc
    prior: Dict[str, Dict[str, float]] = baseline.get("stages") or {}
    out: Dict[str, Dict[str, float]] = {}
    for name, stat in current.items():
        before = prior.get(name)
        if not before or not before.get("min_s"):
            continue
        out[name] = {
            "baseline_s": before["min_s"],
            "current_s": stat["min_s"],
            "pct": (stat["min_s"] - before["min_s"]) / before["min_s"] * 100.0,
        }
    return out


# --------------------------------------------------------------------------


def main(argv: Optional[List[str]] = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--repo", type=Path, default=REPO_ROOT, help="tree to index")
    parser.add_argument("--repeat", type=int, default=3, help="iterations per stage")
    parser.add_argument(
        "--stages",
        default=",".join(ALL_STAGES),
        help=f"comma-separated subset of: {', '.join(ALL_STAGES)}",
    )
    parser.add_argument("--binary", default=None, help="devmap binary (default: this checkout)")
    parser.add_argument("--baseline", type=Path, default=None, help="prior result JSON to diff")
    parser.add_argument("--out-dir", type=Path, default=RESULTS_DIR)
    parser.add_argument("--label", default="", help="free-text tag recorded in the result")
    parser.add_argument(
        "--synthetic",
        type=int,
        default=0,
        help="generate a corpus of N Python modules and benchmark that instead of --repo",
    )
    parser.add_argument(
        "--growth-rounds", type=int, default=10, help="builds in the `growth` stage"
    )
    args = parser.parse_args(argv)

    if args.repeat < 3:
        # Three is the floor for reporting a spread at all. A one-run benchmark
        # is not a measurement, and this file's own header says so.
        print("error: --repeat must be >= 3; a single run is not a measurement", file=sys.stderr)
        return 2
    if args.growth_rounds < 2:
        print("error: --growth-rounds must be >= 2 to describe a curve", file=sys.stderr)
        return 2

    synthetic_dir: Optional[tempfile.TemporaryDirectory] = None
    if args.synthetic:
        synthetic_dir = tempfile.TemporaryDirectory(prefix="devmap-synth-")
        repo = Path(synthetic_dir.name)
        try:
            generate_synthetic(repo, args.synthetic)
        except BenchError as exc:
            print(f"error: {exc}", file=sys.stderr)
            return 2
        print(
            f"generated a synthetic corpus of {args.synthetic} modules in {repo}",
            file=sys.stderr,
        )
    else:
        repo = args.repo.expanduser().resolve()
        if not repo.is_dir():
            print(f"error: no such repo: {repo}", file=sys.stderr)
            return 2

    requested = [s.strip() for s in args.stages.split(",") if s.strip()]
    unknown = [s for s in requested if s not in ALL_STAGES]
    if unknown:
        print(f"error: unknown stage(s): {', '.join(unknown)}", file=sys.stderr)
        return 2

    try:
        binary = args.binary or find_devmap()
    except BenchError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1

    corpus = describe_corpus(repo)
    print(
        f"benchmarking {repo} — {corpus['tracked_files']} tracked / "
        f"{corpus['code_files']} code files, {args.repeat} repeat(s)",
        file=sys.stderr,
    )

    stages: Dict[str, Dict[str, float]] = {}
    with tempfile.TemporaryDirectory(prefix="devmap-bench-") as tmp:
        bench = MapBench(repo, binary, args.repeat, Path(tmp))
        for name in requested:
            print(f"  {name} ...", end="", flush=True, file=sys.stderr)
            try:
                if name == "growth":
                    stages[name] = bench.stage_growth(args.growth_rounds)
                else:
                    stages[name] = getattr(bench, f"stage_{name}")()
            except BenchError as exc:
                print(f" FAILED\n{exc}", file=sys.stderr)
                return 1
            print(f" {_fmt(stages[name]['min_s'])}", file=sys.stderr)

    payload: Dict[str, object] = {
        "generated_at": datetime.now(timezone.utc).isoformat(timespec="seconds"),
        "repo": str(repo),
        "label": args.label,
        "repeat": args.repeat,
        "binary": binary_identity(binary),
        "corpus": corpus,
        "corpus_kind": "synthetic" if args.synthetic else "repository",
        "synthetic_files": args.synthetic,
        "stages": stages,
        "python": sys.version.split()[0],
    }
    if args.baseline:
        try:
            payload["comparison"] = compare(stages, args.baseline.expanduser().resolve())
        except BenchError as exc:
            print(f"error: {exc}", file=sys.stderr)
            return 1

    out_dir = args.out_dir.expanduser().resolve()
    out_dir.mkdir(parents=True, exist_ok=True)
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    (out_dir / f"{stamp}.json").write_text(
        json.dumps(payload, indent=2, sort_keys=True), encoding="utf-8"
    )
    report = render_markdown(payload)
    (out_dir / f"{stamp}.md").write_text(report, encoding="utf-8")
    print(report)
    print(f"wrote {out_dir / f'{stamp}.json'}", file=sys.stderr)
    if synthetic_dir is not None:
        synthetic_dir.cleanup()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
