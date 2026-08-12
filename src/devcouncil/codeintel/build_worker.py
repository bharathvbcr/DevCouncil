"""Internal subprocess entry point for supervised full graph builds.

devcouncil: allow-unwired — launched with ``python -m`` by build_control.
"""

from __future__ import annotations

import argparse
import json
import os
import signal
import sys
import threading
import time
from pathlib import Path
from typing import Callable

try:  # POSIX only; Windows falls back to process_time() below.
    import resource
except ImportError:  # pragma: no cover - Windows
    resource = None  # type: ignore[assignment]

_STDOUT_LOCK = threading.Lock()
# Set once the supervisor's pipe breaks: stdout is silenced from then on, so
# completion/heartbeat must be recorded in build_status.json instead.
_PIPE_BROKEN = threading.Event()


def _emit(**payload: object) -> None:
    line = json.dumps(payload, separators=(",", ":"))
    with _STDOUT_LOCK:
        sys.stdout.write(line + "\n")
        sys.stdout.flush()


def _silence_stdout() -> None:
    """Point stdout at devnull after the supervisor's pipe breaks.

    Without this, the interpreter's exit-time stdout flush raises
    BrokenPipeError and turns a fully committed orphan build into exit 120.
    """
    _PIPE_BROKEN.set()
    with _STDOUT_LOCK:
        try:
            devnull = open(os.devnull, "w")  # noqa: SIM115 - lives until exit
            os.dup2(devnull.fileno(), 1)
            sys.stdout = devnull
        except OSError:
            pass


def _cpu_seconds() -> float:
    """CPU consumed by this process and any reaped children.

    The supervisor uses the delta to tell "slow but working" apart from "hung".
    Extraction fans out to child processes, so children must be counted or a
    fan-out phase looks idle from the parent's own rusage.
    """
    if resource is None:  # pragma: no cover - Windows
        return float(time.process_time())
    try:
        own = resource.getrusage(resource.RUSAGE_SELF)
        kids = resource.getrusage(resource.RUSAGE_CHILDREN)
    except (OSError, ValueError):  # pragma: no cover - platform fallback
        return float(time.process_time())
    return float(
        own.ru_utime + own.ru_stime + kids.ru_utime + kids.ru_stime
    )


class SelfSupervisor:
    """Worker-side deadline enforcement that survives supervisor death.

    The parent ``dev map`` enforces stall/total budgets, but the worker is a
    detached session leader: when the parent dies (SIGHUP on a closed
    terminal, ``kill -9``), the orphan used to grind on unsupervised for
    hours while holding ``writer.lock``. This watchdog bounds every build
    from inside the worker. On deadline it repeatedly interrupts the active
    SQLite statement — signals cannot reach a thread blocked inside SQLite C
    code — and if the build still has not unwound after ``escalate_after``
    seconds it hard-kills the whole process group, which releases the flock
    and takes any pool children down with it.
    """

    def __init__(
        self,
        *,
        total_timeout: float,
        interrupt: "Callable[[], object]",
        escalate: "Callable[[], object] | None" = None,
        escalate_after: float = 30.0,
        poll_interval: float = 0.5,
        on_trip: "Callable[[str], object] | None" = None,
    ) -> None:
        self._total_timeout = max(0.1, float(total_timeout))
        self._interrupt = interrupt
        self._escalate = escalate or _kill_own_process_group
        self._escalate_after = max(0.1, float(escalate_after))
        self._poll_interval = max(0.05, float(poll_interval))
        self._on_trip = on_trip
        self._stop = threading.Event()
        self.tripped_reason = ""
        self._thread = threading.Thread(
            target=self._run, name="devcouncil-graph-worker-watchdog", daemon=True
        )

    def start(self) -> None:
        self._thread.start()

    def stop(self) -> None:
        self._stop.set()
        self._thread.join(timeout=2.0)

    def _run(self) -> None:
        started = time.monotonic()
        deadline = started + self._total_timeout
        tripped_at: float | None = None
        while not self._stop.wait(self._poll_interval):
            now = time.monotonic()
            if tripped_at is None:
                if now < deadline:
                    continue
                self.tripped_reason = (
                    f"graph worker exceeded its self-enforced total budget of "
                    f"{self._total_timeout:.1f}s"
                )
                tripped_at = now
                if self._on_trip is not None:
                    try:
                        self._on_trip(self.tripped_reason)
                    except Exception:  # noqa: BLE001 - watchdog must not die
                        pass
            try:
                self._interrupt()
            except Exception:  # noqa: BLE001 - watchdog must not die
                pass
            if now - tripped_at > self._escalate_after:
                try:
                    self._escalate()
                except Exception:  # noqa: BLE001 - watchdog must not die
                    pass
                return


def _kill_own_process_group() -> None:
    """Last-resort exit: kill the whole group so flock and children go too."""
    sig = signal.SIGKILL if hasattr(signal, "SIGKILL") else signal.SIGTERM
    try:
        os.killpg(os.getpid(), sig)
    except (OSError, AttributeError):
        os.kill(os.getpid(), sig)


class _Heartbeat:
    """Timer-driven liveness beacon, independent of phase counters.

    Long phases (liveness tokenize, SQLite persist) emit no phase progress for
    minutes at a time. Without this, the supervisor's idle timer expires and a
    healthy worker is killed at 90%+ CPU. When the supervisor is gone
    (broken pipe or reparented to init), the beacon writes ``build_status``
    itself so ``dev map status`` / ``dev map unlock`` keep seeing the truth.
    """

    def __init__(self, build_id: str, interval: float, root: Path | None = None) -> None:
        self._build_id = build_id
        self._interval = max(0.5, float(interval))
        self._root = root
        self._stop = threading.Event()
        self._phase = "starting"
        self._completed = 0
        self._total = 0
        self._lock = threading.Lock()
        self._pipe_broken = False
        self._parent_pid = os.getppid()
        self._thread = threading.Thread(
            target=self._run, name="devcouncil-graph-worker-heartbeat", daemon=True
        )

    def observe(self, phase: str, completed: int, total: int) -> None:
        with self._lock:
            self._phase = phase
            self._completed = completed
            self._total = total

    def start(self) -> None:
        self._thread.start()

    def stop(self) -> None:
        self._stop.set()
        self._thread.join(timeout=2.0)

    def _orphaned(self) -> bool:
        try:
            return os.getppid() != self._parent_pid
        except OSError:
            return False

    def _run(self) -> None:
        while not self._stop.wait(self._interval):
            with self._lock:
                phase, completed, total = self._phase, self._completed, self._total
            cpu = round(_cpu_seconds(), 3)
            if not self._pipe_broken:
                try:
                    _emit(
                        build_id=self._build_id,
                        state="heartbeat",
                        phase=phase,
                        completed=completed,
                        total=total,
                        cpu_seconds=cpu,
                        pid=os.getpid(),
                    )
                except (OSError, ValueError):
                    # Parent closed the pipe (supervisor gone). Keep beating —
                    # from here the status file is the only truthful surface.
                    self._pipe_broken = True
                    _silence_stdout()
            if self._root is not None and (
                self._pipe_broken or _PIPE_BROKEN.is_set() or self._orphaned()
            ):
                try:
                    from devcouncil.codeintel.build_control import (
                        update_inline_build_progress,
                    )

                    update_inline_build_progress(
                        self._root,
                        phase=phase,
                        completed=completed,
                        total=total,
                        state="building",
                        mode="full",
                        worker_cpu_seconds=cpu,
                    )
                except Exception:  # noqa: BLE001 - heartbeat must never kill the build
                    pass


def _changed_paths(args: argparse.Namespace) -> set[str]:
    """Union of ``--changed-path`` args and the newline-delimited paths file.

    A repo-scale change set passed as one ``--changed-path`` per entry blows past
    ``ARG_MAX`` (``OSError: [Errno 7] Argument list too long``); the supervisor
    writes a file instead. The argv form stays supported for small sets and for
    callers that invoke the worker directly.
    """
    paths = {str(path) for path in args.changed_path if path}
    if args.changed_paths_file:
        try:
            raw = Path(args.changed_paths_file).read_text(encoding="utf-8")
        except OSError as exc:
            raise SystemExit(f"could not read --changed-paths-file: {exc}") from exc
        paths.update(line for line in raw.splitlines() if line)
    return paths


def main() -> int:
    parser = argparse.ArgumentParser(add_help=False)
    parser.add_argument("--root", required=True)
    parser.add_argument("--build-id", required=True)
    parser.add_argument("--changed-path", action="append", default=[])
    parser.add_argument("--changed-paths-file", default="")
    parser.add_argument("--no-liveness", action="store_true")
    parser.add_argument("--heartbeat-interval", type=float, default=5.0)
    parser.add_argument(
        "--total-timeout",
        type=float,
        default=0.0,
        help="Self-enforced total build budget in seconds (0 = read config).",
    )
    args = parser.parse_args()
    root = Path(args.root).expanduser().resolve()
    changed = _changed_paths(args)

    total_timeout = float(args.total_timeout or 0.0)
    if total_timeout <= 0.0:
        total_timeout = 900.0
        try:
            from devcouncil.app.config import load_config

            total_timeout = float(load_config(root).indexing.build_total_timeout_seconds)
        except Exception:  # noqa: BLE001 - config problems must not unbound the build
            pass

    def _interrupt_store_writes() -> None:
        from devcouncil.codeintel.service import get_codeintel_service

        get_codeintel_service(root).store.interrupt_writes()

    def _record_self_timeout(reason: str) -> None:
        try:
            _emit(
                build_id=args.build_id,
                state="failed",
                phase="self_timeout",
                completed=0,
                total=1,
                reason=reason,
                cpu_seconds=round(_cpu_seconds(), 3),
            )
        except (OSError, ValueError):
            pass
        try:
            from devcouncil.codeintel.build_control import update_inline_build_progress

            update_inline_build_progress(
                root,
                phase="self_timeout",
                state="timed_out",
                mode="full",
                reason=reason,
                worker_cpu_seconds=round(_cpu_seconds(), 3),
            )
        except Exception:  # noqa: BLE001 - status write is best-effort
            pass

    # The parent supervisor (when alive) enforces the configured budget first;
    # the worker's own deadline sits a grace behind it so an orphaned worker is
    # still bounded, without double-kill races while supervised.
    supervisor = SelfSupervisor(
        total_timeout=total_timeout + 30.0,
        interrupt=_interrupt_store_writes,
        on_trip=_record_self_timeout,
    )
    heartbeat = _Heartbeat(args.build_id, args.heartbeat_interval, root=root)

    def progress(phase: str, completed: int, total: int) -> None:
        heartbeat.observe(phase, completed, total)
        try:
            _emit(
                build_id=args.build_id,
                state="building",
                phase=phase,
                completed=completed,
                total=total,
                cpu_seconds=round(_cpu_seconds(), 3),
            )
        except (OSError, ValueError):
            # Supervisor gone (broken pipe). The orphan keeps building under
            # its own deadline; the heartbeat's status-file writes take over.
            _silence_stdout()

    from devcouncil.codeintel.build_control import graph_build_session
    from devcouncil.indexing.graph.build import (
        CompatibilityGraphTooLarge,
        build_code_graph,
        write_code_graph,
    )

    heartbeat.start()
    supervisor.start()
    try:
        # Acquire the cross-process writer lease in the child so an orphaned worker
        # (parent crash) still serializes against watchers/MCP writers.
        with graph_build_session(root):
            graph = build_code_graph(
                root,
                changed_paths=changed,
                liveness=not args.no_liveness,
                progress=progress,
            )
            graph.meta.update({
                "incremental": False,
                "changed_paths": sorted(changed),
                "affected_paths": sorted(changed),
                "affected_fraction": 1.0,
                "resolution_scope": "full",
            })
            compatibility_export = "healthy"
            reason = ""
            try:
                write_code_graph(root, graph, _lease_held=True, progress=progress)
            except CompatibilityGraphTooLarge as exc:
                compatibility_export = "degraded"
                reason = str(exc)
            final_state = "degraded" if reason else "complete"
            try:
                _emit(
                    build_id=args.build_id,
                    state=final_state,
                    phase="complete",
                    completed=1,
                    total=1,
                    compatibility_export=compatibility_export,
                    reason=reason,
                    cpu_seconds=round(_cpu_seconds(), 3),
                )
            except (OSError, ValueError):
                _silence_stdout()
            if _PIPE_BROKEN.is_set():
                # The supervisor never saw (or will never see) the final line:
                # the generation is committed, so record completion in the
                # status file — the only remaining truthful surface.
                try:
                    from devcouncil.codeintel.build_control import (
                        update_inline_build_progress,
                    )

                    update_inline_build_progress(
                        root,
                        phase="complete",
                        completed=1,
                        total=1,
                        state=final_state,
                        mode="full",
                        reason=reason,
                        worker_cpu_seconds=round(_cpu_seconds(), 3),
                    )
                except Exception:  # noqa: BLE001 - status write is best-effort
                    pass
            return 0
    finally:
        supervisor.stop()
        heartbeat.stop()
        if args.changed_paths_file:
            # The supervisor cleans this up too; both sides tolerate the other
            # having won so a dead supervisor cannot leak handoff files.
            Path(args.changed_paths_file).unlink(missing_ok=True)


if __name__ == "__main__":
    raise SystemExit(main())
