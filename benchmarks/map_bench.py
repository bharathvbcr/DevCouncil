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
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
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

ALL_STAGES = ("cold", "warm", "touch", "manifest", "e2e")


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
    return {
        "min_s": min(samples),
        "median_s": statistics.median(samples),
        "max_s": max(samples),
        "samples": len(samples),
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
        return time_stage(self._build, repeat=self.repeat, setup=self._reset_store)

    def stage_warm(self) -> Dict[str, float]:
        self._reset_store()
        self._build()  # prime; not timed
        return time_stage(self._build, repeat=self.repeat)

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
            return time_stage(self._build, repeat=self.repeat, setup=setup)
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
        "| stage | min | median | max | throughput |",
        "|---|---|---|---|---|",
    ]
    for name in ALL_STAGES:
        stat = stages.get(name)
        if not stat:
            continue
        rate = (
            f"{code_files / stat['min_s']:.0f} files/s"
            if name in ("cold",) and stat["min_s"] > 0
            else "—"
        )
        lines.append(
            f"| `{name}` | {_fmt(stat['min_s'])} | {_fmt(stat['median_s'])} "
            f"| {_fmt(stat['max_s'])} | {rate} |"
        )

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
    args = parser.parse_args(argv)

    repo = args.repo.expanduser().resolve()
    if not repo.is_dir():
        print(f"error: no such repo: {repo}", file=sys.stderr)
        return 2
    if args.repeat < 1:
        print("error: --repeat must be >= 1", file=sys.stderr)
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
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
