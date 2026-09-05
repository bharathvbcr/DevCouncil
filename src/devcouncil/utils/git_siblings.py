"""Who else is working in this repository, and what did they leave behind?

Two agent sessions once ran the same goal on this repository at the same time —
one in the main checkout committing to ``main``, one in a linked worktree on a
branch. Neither knew about the other for hours; both merged the same unmerged
branch and both fixed the same findings with different code, which then had to
be reconciled by hand. On another day a previous session's branch simply sat
unmerged with nothing at session start to say so.

This module answers the three questions a starting session needs answered
before it writes anything, each independently so that one failure cannot
silence the others:

* **live siblings** — every other checkout of this repository (``git worktree
  list``) that has uncommitted changes, or that something touched inside the
  activity window.
* **divergent branches** — local branches matching the configured prefixes that
  are ahead of the default branch, with how far ahead and how old the tip is.
* **behind** — commits on the default branch that this checkout does not have.

Nothing here is cached: the answer is only useful if it is current, and the
whole probe is bounded (short timeouts, capped output, capped listings) because
it runs on the session-start path.

The Class A rule applies throughout: a question that could not be answered is
reported as ``unavailable`` with the reason. "No siblings" is only ever said
when git was asked and answered. A directory that is simply not a repository is
an *answer* (nothing to report), not a failure — :func:`git_repo_state` owns
that distinction and this module defers to it.

Knobs (all optional, read fresh on every call):

``DEVCOUNCIL_SIBLING_WINDOW_MINUTES``
    How recently a checkout must have been touched to count as live (default 30).
``DEVCOUNCIL_SIBLING_BRANCH_PREFIXES``
    Comma-separated branch prefixes to watch (default ``claude/``).
``DEVCOUNCIL_DEFAULT_BRANCH``
    The integration branch to measure against (default: ``main``, else ``master``).
"""

from __future__ import annotations

import os
import time
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field
from pathlib import Path
from typing import Dict, List, Optional, Sequence, Tuple

from devcouncil.utils.proc import git_repo_state, run_git

#: Every git probe here runs on the session-start path, so the ceiling is short:
#: a session must not wait on a wedged git to be told who else is around.
GIT_PROBE_TIMEOUT: float = 5.0
#: Default activity window. A checkout touched inside it is "live".
DEFAULT_WINDOW_MINUTES: float = 30.0
#: Branch namespaces worth warning about by default.
DEFAULT_BRANCH_PREFIXES: Tuple[str, ...] = ("claude/",)
#: Integration branches tried, in order, when none is configured.
DEFAULT_BRANCH_CANDIDATES: Tuple[str, ...] = ("main", "master")

#: Caps. Every one of them is reported when it bites (`truncated`), never
#: silently applied — a capped sample must not read as complete coverage.
MAX_SIBLINGS = 8
MAX_BRANCHES = 20
MAX_TRACKED_FILES = 20_000
MAX_STATUS_PATHS = 200
MAX_GIT_OUTPUT_CHARS = 512_000
MAX_PROBE_WORKERS = 4
#: Presentation caps for the one-line hint.
MAX_FIELD_CHARS = 24
MAX_HINT_CHARS = 200
HINT_LIST_LIMIT = 2

#: Files git rewrites for almost any operation in a worktree (status refreshes
#: the index, a commit appends to logs/HEAD). Their mtimes answer "was anything
#: done in this checkout recently?" for the price of a few stat calls, which is
#: why they are consulted before the tracked-file walk.
_GIT_ACTIVITY_FILES = ("index", "HEAD", "logs/HEAD", "ORIG_HEAD", "MERGE_HEAD", "COMMIT_EDITMSG")


@dataclass(frozen=True)
class SiblingCheckout:
    """Another checkout of this repository that looks like it is in use."""

    path: str
    branch: Optional[str]
    reason: str
    age_seconds: Optional[float]


@dataclass(frozen=True)
class DivergentBranch:
    """A local branch carrying commits the default branch does not have."""

    name: str
    ahead: int
    tip_age_seconds: Optional[float]


@dataclass(frozen=True)
class SessionGuardReport:
    """What the guard found, and what it could not find out.

    ``unavailable`` holds one sentence per question that could not be answered;
    ``truncated`` holds one per listing that hit a cap. An empty report with
    both empty means git was asked all three questions and answered "nothing".
    """

    live_siblings: Tuple[SiblingCheckout, ...] = ()
    divergent_branches: Tuple[DivergentBranch, ...] = ()
    behind: Optional[int] = None
    default_branch: Optional[str] = None
    current_branch: Optional[str] = None
    is_repository: bool = True
    unavailable: Tuple[str, ...] = ()
    truncated: Tuple[str, ...] = ()

    @property
    def has_findings(self) -> bool:
        """Something a starting session should look at before writing."""
        return bool(
            self.live_siblings
            or self.divergent_branches
            or (self.behind or 0) > 0
            or self.unavailable
            or self.truncated
        )

    @property
    def has_collision_risk(self) -> bool:
        """Another session may be working here, or left work behind."""
        return bool(self.live_siblings or self.divergent_branches)


@dataclass
class _Worktree:
    path: str
    branch: Optional[str] = None
    detached: bool = False
    bare: bool = False


@dataclass
class _Probe:
    """One sibling's answer, or the reason there is none."""

    sibling: Optional[SiblingCheckout] = None
    unavailable: Optional[str] = None
    truncated: Optional[str] = None


@dataclass
class _Accumulator:
    unavailable: List[str] = field(default_factory=list)
    truncated: List[str] = field(default_factory=list)


# ---------------------------------------------------------------------------
# bounded git plumbing
# ---------------------------------------------------------------------------


def _bound_text(text: str) -> str:
    """Cap a git command's output before anything parses it."""
    if len(text) <= MAX_GIT_OUTPUT_CHARS:
        return text
    return text[:MAX_GIT_OUTPUT_CHARS]


def _git_text(
    args: Sequence[str], cwd: Path, *, timeout: float = GIT_PROBE_TIMEOUT
) -> Tuple[Optional[str], str]:
    """``(stdout, "")`` when git answered, ``(None, reason)`` when it could not.

    ``run_git`` already converts a hang into returncode 124 rather than an
    exception; a missing git raises ``OSError``, which is a failure to run and
    not an answer, so it is surfaced as a reason too.
    """
    try:
        result = run_git(list(args), cwd, timeout=timeout)
    except OSError as exc:
        return None, f"could not run git: {exc}"
    if result.returncode == 124:
        return None, f"git {args[0]} timed out after {timeout:g}s"
    if result.returncode != 0:
        detail = (result.stderr or result.stdout or "").strip().splitlines()
        return None, f"git {args[0]} exited {result.returncode}: {detail[0] if detail else 'no output'}"
    return _bound_text(result.stdout or ""), ""


def _parse_worktrees(text: str) -> Tuple[List[_Worktree], bool]:
    """Parse ``git worktree list --porcelain``; ``(entries, truncated)``.

    Records are blank-line separated and start with ``worktree <path>``; the
    remaining lines (``HEAD``, ``branch``, ``bare``, ``detached``, ``locked``,
    ``prunable``) are optional and order is not guaranteed.
    """
    entries: List[_Worktree] = []
    current: Optional[_Worktree] = None
    truncated = False
    for line in text.splitlines():
        line = line.strip()
        if line.startswith("worktree "):
            if len(entries) >= MAX_SIBLINGS:
                truncated = True
                break
            current = _Worktree(path=line[len("worktree ") :].strip())
            entries.append(current)
        elif current is None:
            continue
        elif line.startswith("branch "):
            ref = line[len("branch ") :].strip()
            current.branch = ref[len("refs/heads/") :] if ref.startswith("refs/heads/") else ref
        elif line == "detached":
            current.detached = True
        elif line == "bare":
            current.bare = True
    return entries, truncated


def _current_index(entries: Sequence[_Worktree], root: Path) -> Optional[int]:
    """Which listed checkout is the one *root* is in?

    Not simple equality: ``root`` may be a subdirectory of its checkout, and in
    this repository the linked worktrees live *inside* the main checkout
    (``<main>/.claude/worktrees/<name>``), so plain containment would call the
    main checkout "this one" from inside a worktree. The deepest listed path
    that contains ``root`` is the checkout ``root`` belongs to.
    """
    best: Optional[int] = None
    best_len = -1
    for index, entry in enumerate(entries):
        try:
            candidate = Path(entry.path).resolve()
        except OSError:
            continue
        if candidate != root and candidate not in root.parents:
            continue
        length = len(candidate.parts)
        if length > best_len:
            best, best_len = index, length
    return best


# ---------------------------------------------------------------------------
# is this checkout in use?
# ---------------------------------------------------------------------------


def _git_dir_of(worktree: Path) -> Optional[Path]:
    """The directory git keeps this checkout's metadata in, without asking git.

    A linked worktree's ``.git`` is a file holding ``gitdir: <path>``; the main
    checkout's is the directory itself.
    """
    dot_git = worktree / ".git"
    try:
        if dot_git.is_dir():
            return dot_git
        if dot_git.is_file():
            text = dot_git.read_text(encoding="utf-8", errors="replace").strip()
            if text.startswith("gitdir:"):
                return Path(text.split(":", 1)[1].strip())
    except OSError:
        return None
    return None


def _newest_mtime(paths: Sequence[Path], *, cutoff: Optional[float] = None) -> Optional[float]:
    """Newest mtime among *paths*, stopping early once one beats *cutoff*."""
    newest: Optional[float] = None
    for path in paths:
        try:
            stamp = path.stat().st_mtime
        except OSError:
            continue
        if newest is None or stamp > newest:
            newest = stamp
        if cutoff is not None and newest >= cutoff:
            return newest
    return newest


def _status_paths(text: str) -> List[str]:
    """Paths named by ``git status --porcelain -z``, tolerantly and capped.

    Only used to date the sibling's newest edit, so a rename's second record
    (a bare path with no ``XY `` prefix) mis-slicing into something that does
    not exist is harmless: it fails to stat and is skipped.
    """
    out: List[str] = []
    for chunk in text.split("\0"):
        if len(chunk) < 4:
            continue
        out.append(chunk[3:])
        if len(out) >= MAX_STATUS_PATHS:
            break
    return out


def _tracked_newest_mtime(
    worktree: Path, *, cutoff: float, timeout: float
) -> Tuple[Optional[float], str, bool]:
    """Newest tracked-file mtime; ``(mtime, reason, truncated)``.

    ``reason`` is non-empty when git could not list the tracked files, and
    ``truncated`` says the walk stopped at :data:`MAX_TRACKED_FILES` without
    finding anything inside the window — a scan that did not finish must not be
    reported as a scan that finished and found nothing.
    """
    text, error = _git_text(["ls-files", "-z"], worktree, timeout=timeout)
    if text is None:
        return None, error, False
    names = [name for name in text.split("\0") if name]
    truncated = len(names) > MAX_TRACKED_FILES
    candidates = [worktree / name for name in names[:MAX_TRACKED_FILES]]
    newest = _newest_mtime(candidates, cutoff=cutoff)
    if newest is not None and newest >= cutoff:
        return newest, "", False
    return newest, "", truncated


def _probe_metadata(entry: _Worktree, *, now: float, window: float) -> Optional[_Probe]:
    """Tier 1: has any git command touched this checkout lately?

    A handful of ``stat`` calls and no subprocess at all. Any git command run
    in a checkout moves ``index`` or ``logs/HEAD``, so an agent working there
    lights this up — and when it does, the two probes below are never spawned.
    Returns ``None`` when it cannot decide.
    """
    worktree = Path(entry.path)
    if entry.bare or not worktree.is_dir():
        return _Probe()
    git_dir = _git_dir_of(worktree)
    if git_dir is None:
        return None
    cutoff = now - window
    stamp = _newest_mtime([git_dir / name for name in _GIT_ACTIVITY_FILES], cutoff=cutoff)
    if stamp is None or stamp < cutoff:
        return None
    return _Probe(
        sibling=SiblingCheckout(
            path=entry.path,
            branch=entry.branch,
            reason="recent git activity",
            age_seconds=now - stamp,
        )
    )


def _resolve_probe(
    entry: _Worktree,
    status: Tuple[Optional[str], str],
    walk: Tuple[Optional[float], str, bool],
    *,
    now: float,
    window: float,
) -> _Probe:
    """Combine tiers 2 and 3 for a checkout tier 1 could not decide.

    ``status`` wins when it has something to say, because "uncommitted changes"
    is the sharper reason; the tracked-file walk is the fallback for a file
    rewritten with identical content and no git command to show for it. The
    reason reported always names the tier that actually fired.
    """
    worktree = Path(entry.path)
    text, error = status
    if text is None:
        return _Probe(unavailable=f"{entry.path}: {error}")
    if text.strip():
        age = _newest_mtime([worktree / p for p in _status_paths(text)])
        return _Probe(
            sibling=SiblingCheckout(
                path=entry.path,
                branch=entry.branch,
                reason="uncommitted changes",
                age_seconds=(now - age) if age else None,
            )
        )
    newest, reason, truncated = walk
    if reason:
        return _Probe(unavailable=f"{entry.path}: {reason}")
    if newest is not None and newest >= now - window:
        return _Probe(
            sibling=SiblingCheckout(
                path=entry.path,
                branch=entry.branch,
                reason="recently edited",
                age_seconds=now - newest,
            )
        )
    if truncated:
        return _Probe(truncated=f"{entry.path}: only the first {MAX_TRACKED_FILES} tracked files were dated")
    return _Probe()


def _probe_siblings(
    entries: Sequence[_Worktree], *, now: float, window: float, timeout: float
) -> List[_Probe]:
    """Probe every other checkout, cheapest signal first and the rest in parallel.

    Tier 1 decides most checkouts for free. For the ones it cannot, ``git
    status`` and the tracked-file walk are *started together* rather than in
    sequence: a quiet checkout needs both answers anyway, and a quiet-but-dirty
    one only wastes a read-only ``ls-files``. Concurrency stays bounded by
    :data:`MAX_PROBE_WORKERS` however many checkouts there are.
    """
    probes: List[_Probe] = []
    pending: List[_Worktree] = []
    for entry in entries:
        decided = _probe_metadata(entry, now=now, window=window)
        if decided is not None:
            probes.append(decided)
        else:
            pending.append(entry)
    if not pending:
        return probes
    cutoff = now - window
    with ThreadPoolExecutor(max_workers=min(MAX_PROBE_WORKERS, 2 * len(pending))) as pool:
        jobs = [
            (
                entry,
                pool.submit(_sibling_status, Path(entry.path), timeout=timeout),
                pool.submit(
                    _tracked_newest_mtime, Path(entry.path), cutoff=cutoff, timeout=timeout
                ),
            )
            for entry in pending
        ]
        for entry, status_future, walk_future in jobs:
            probes.append(
                _resolve_probe(
                    entry, status_future.result(), walk_future.result(), now=now, window=window
                )
            )
    return probes


def _sibling_status(worktree: Path, *, timeout: float) -> Tuple[Optional[str], str]:
    """``git status`` in *another session's* checkout, observing only.

    ``--no-optional-locks`` is the point: a plain ``git status`` refreshes and
    rewrites the index it reads, and this index belongs to someone else's
    working session. The guard must never perturb the tree it warns about.
    """
    return _git_text(
        ["--no-optional-locks", "status", "--porcelain", "-z", "--untracked-files=no"],
        worktree,
        timeout=timeout,
    )


# ---------------------------------------------------------------------------
# branches
# ---------------------------------------------------------------------------


def _branch_rows(root: Path, *, timeout: float) -> Tuple[Optional[List[Tuple[str, Optional[float]]]], str]:
    text, error = _git_text(
        ["for-each-ref", "--format=%(refname:short)\t%(committerdate:unix)", "refs/heads"],
        root,
        timeout=timeout,
    )
    if text is None:
        return None, error
    rows: List[Tuple[str, Optional[float]]] = []
    for line in text.splitlines():
        if not line.strip():
            continue
        name, _, stamp = line.partition("\t")
        try:
            tip: Optional[float] = float(stamp.strip())
        except ValueError:
            tip = None
        rows.append((name.strip(), tip))
    return rows, ""


def _ahead_behind(
    root: Path, names: Sequence[str], default_branch: str, *, timeout: float
) -> Tuple[Dict[str, Tuple[int, int]], str]:
    """``{branch: (ahead, behind)}`` relative to *default_branch*.

    One ``for-each-ref`` answers for every branch at once — including the branch
    this checkout is on, whose *behind* is the third question this module asks —
    but ``%(ahead-behind:)`` only exists from git 2.41 and an older git rejects
    the whole format. The fallback counts each branch with one ``rev-list
    --count --left-right``, which yields both numbers in a single process.
    """
    if not names:
        return {}, ""
    patterns = [f"refs/heads/{name}" for name in names]
    text, _error = _git_text(
        [
            "for-each-ref",
            f"--format=%(refname:short)\t%(ahead-behind:refs/heads/{default_branch})",
            *patterns,
        ],
        root,
        timeout=timeout,
    )
    if text is not None:
        pairs: Dict[str, Tuple[int, int]] = {}
        for line in text.splitlines():
            name, _, counts = line.partition("\t")
            fields = counts.split()
            if len(fields) < 2:
                continue
            try:
                pairs[name.strip()] = (int(fields[0]), int(fields[1]))
            except ValueError:
                continue
        if pairs:
            return pairs, ""
        # An accepted format that produced no usable row is not an answer
        # either; fall through and count the branches one at a time.

    pairs = {}
    failures: List[str] = []
    with ThreadPoolExecutor(max_workers=min(MAX_PROBE_WORKERS, len(names))) as pool:
        futures = {
            name: pool.submit(
                _git_text,
                [
                    "rev-list",
                    "--count",
                    "--left-right",
                    f"refs/heads/{name}...refs/heads/{default_branch}",
                ],
                root,
                timeout=timeout,
            )
            for name in names
        }
        for name, future in futures.items():
            out, why = future.result()
            if out is None:
                failures.append(f"{name}: {why}")
                continue
            fields = out.split()
            try:
                pairs[name] = (int(fields[0]), int(fields[1]))
            except (IndexError, ValueError):
                failures.append(f"{name}: unreadable count {out.strip()[:40]!r}")
    return pairs, "; ".join(failures[:2])


# ---------------------------------------------------------------------------
# the report
# ---------------------------------------------------------------------------


def _configured_window_minutes(explicit: Optional[float]) -> float:
    if explicit is not None:
        return max(0.0, float(explicit))
    raw = os.environ.get("DEVCOUNCIL_SIBLING_WINDOW_MINUTES", "").strip()
    if raw:
        try:
            return max(0.0, float(raw))
        except ValueError:
            pass
    return DEFAULT_WINDOW_MINUTES


def _configured_prefixes(explicit: Optional[Sequence[str]]) -> Tuple[str, ...]:
    if explicit is not None:
        return tuple(p for p in explicit if p)
    raw = os.environ.get("DEVCOUNCIL_SIBLING_BRANCH_PREFIXES", "").strip()
    if raw:
        parsed = tuple(part.strip() for part in raw.split(",") if part.strip())
        if parsed:
            return parsed
    return DEFAULT_BRANCH_PREFIXES


def _pick_default_branch(names: Sequence[str]) -> Optional[str]:
    configured = os.environ.get("DEVCOUNCIL_DEFAULT_BRANCH", "").strip()
    if configured:
        return configured if configured in names else None
    for candidate in DEFAULT_BRANCH_CANDIDATES:
        if candidate in names:
            return candidate
    return None


def inspect_session_siblings(
    root: Path,
    *,
    window_minutes: Optional[float] = None,
    branch_prefixes: Optional[Sequence[str]] = None,
    timeout: float = GIT_PROBE_TIMEOUT,
    now: Optional[float] = None,
) -> SessionGuardReport:
    """Answer the three session-guard questions for the repository at *root*.

    Never raises: every question either answers or records why it could not.
    """
    root = Path(root).expanduser()
    try:
        resolved = root.resolve()
    except OSError:
        resolved = root

    # Spawning git costs ~15 ms here before it does any work, and these three
    # questions do not depend on each other, so they are asked at once. The
    # answers are still read in order, and "is this a repository at all?" still
    # decides whether the other two mean anything.
    with ThreadPoolExecutor(max_workers=3) as pool:
        state = pool.submit(git_repo_state, resolved, timeout=timeout)
        worktrees = pool.submit(
            _git_text, ["worktree", "list", "--porcelain"], resolved, timeout=timeout
        )
        refs = pool.submit(_branch_rows, resolved, timeout=timeout)
        inside, why = state.result()
        worktree_text, worktree_error = worktrees.result()
        rows, rows_error = refs.result()

    if inside is False:
        return SessionGuardReport(is_repository=False)
    acc = _Accumulator()
    if inside is None:
        return SessionGuardReport(unavailable=(f"session guard could not check: {why}",))

    moment = time.time() if now is None else now
    window = _configured_window_minutes(window_minutes) * 60.0
    prefixes = _configured_prefixes(branch_prefixes)

    siblings, current_branch = _collect_siblings(
        resolved,
        acc,
        worktree_text,
        worktree_error,
        now=moment,
        window=window,
        timeout=timeout,
    )
    branches, behind, default_branch = _collect_branches(
        resolved,
        acc,
        rows,
        rows_error,
        current_branch=current_branch,
        prefixes=prefixes,
        now=moment,
        timeout=timeout,
    )
    return SessionGuardReport(
        live_siblings=tuple(siblings),
        divergent_branches=tuple(branches),
        behind=behind,
        default_branch=default_branch,
        current_branch=current_branch,
        unavailable=tuple(acc.unavailable),
        truncated=tuple(acc.truncated),
    )


def _collect_siblings(
    root: Path,
    acc: _Accumulator,
    text: Optional[str],
    error: str,
    *,
    now: float,
    window: float,
    timeout: float,
) -> Tuple[List[SiblingCheckout], Optional[str]]:
    if text is None:
        acc.unavailable.append(f"live sibling checkouts: {error}")
        # The branch questions still need to know which branch this checkout is
        # on, and the worktree listing was where that came from.
        out, _why = _git_text(["symbolic-ref", "--short", "-q", "HEAD"], root, timeout=timeout)
        return [], (out.strip() or None) if out is not None else None

    entries, capped = _parse_worktrees(text)
    if capped:
        acc.truncated.append(f"only the first {MAX_SIBLINGS} checkouts were probed")
    current = _current_index(entries, root)
    current_branch = entries[current].branch if current is not None else None
    others = [entry for index, entry in enumerate(entries) if index != current]
    if not others:
        return [], current_branch

    siblings: List[SiblingCheckout] = []
    for probe in _probe_siblings(others, now=now, window=window, timeout=timeout):
        if probe.sibling is not None:
            siblings.append(probe.sibling)
        if probe.unavailable:
            acc.unavailable.append(f"live sibling checkouts: {probe.unavailable}")
        if probe.truncated:
            acc.truncated.append(probe.truncated)
    siblings.sort(key=lambda s: (s.age_seconds if s.age_seconds is not None else float("inf")))
    return siblings, current_branch


def _collect_branches(
    root: Path,
    acc: _Accumulator,
    rows: Optional[List[Tuple[str, Optional[float]]]],
    error: str,
    *,
    current_branch: Optional[str],
    prefixes: Sequence[str],
    now: float,
    timeout: float,
) -> Tuple[List[DivergentBranch], Optional[int], Optional[str]]:
    if rows is None:
        acc.unavailable.append(f"divergent branches: {error}")
        acc.unavailable.append(f"commits this checkout is behind: {error}")
        return [], None, None

    names = [name for name, _ in rows]
    default_branch = _pick_default_branch(names)
    if default_branch is None:
        reason = "no default branch found (looked for " + ", ".join(DEFAULT_BRANCH_CANDIDATES) + ")"
        acc.unavailable.append(f"divergent branches: {reason}")
        acc.unavailable.append(f"commits this checkout is behind: {reason}")
        return [], None, None

    candidates = [
        (name, tip)
        for name, tip in rows
        if name != default_branch
        and name != current_branch
        and any(name.startswith(prefix) for prefix in prefixes)
    ]
    if len(candidates) > MAX_BRANCHES:
        acc.truncated.append(f"only the first {MAX_BRANCHES} matching branches were counted")
        candidates = candidates[:MAX_BRANCHES]

    # The branch this checkout is on rides along in the same probe: its
    # *behind* count is the third question, and asking for it separately would
    # be another 15 ms of process spawn for a number git already has in hand.
    queried = [name for name, _ in candidates]
    if current_branch and current_branch not in queried:
        queried.append(current_branch)
    pairs, failures = _ahead_behind(root, queried, default_branch, timeout=timeout)
    if failures:
        acc.unavailable.append(f"divergent branches: {failures}")
    divergent = [
        DivergentBranch(
            name=name, ahead=pairs[name][0], tip_age_seconds=(now - tip) if tip else None
        )
        for name, tip in candidates
        if pairs.get(name, (0, 0))[0] > 0
    ]
    divergent.sort(key=lambda b: (-b.ahead, b.name))

    behind: Optional[int] = None
    if current_branch and current_branch in pairs:
        behind = pairs[current_branch][1]
    else:
        # Detached HEAD, or the branch probe could not answer for it.
        out, why = _git_text(
            ["rev-list", "--count", f"HEAD..refs/heads/{default_branch}"], root, timeout=timeout
        )
        if out is None:
            acc.unavailable.append(f"commits this checkout is behind: {why}")
        else:
            try:
                behind = int(out.strip() or "0")
            except ValueError:
                acc.unavailable.append(
                    f"commits this checkout is behind: unreadable count {out.strip()[:40]!r}"
                )
    return divergent, behind, default_branch


# ---------------------------------------------------------------------------
# presentation
# ---------------------------------------------------------------------------


def _cap(text: str, limit: int) -> str:
    return text if len(text) <= limit else text[: limit - 1] + "…"


def _age_phrase(seconds: Optional[float]) -> str:
    if seconds is None:
        return "age unknown"
    if seconds < 60:
        return "just now"
    if seconds < 3600:
        return f"{int(seconds // 60)} min ago"
    if seconds < 86400:
        return f"{int(seconds // 3600)} h ago"
    return f"{int(seconds // 86400)} d ago"


def _listing(items: Sequence[str]) -> str:
    """``"a, b (+3 more)"`` — the overflow count leads so a cap cannot eat it."""
    shown = list(items[:HINT_LIST_LIMIT])
    extra = len(items) - len(shown)
    prefix = f"(+{extra} more) " if extra > 0 else ""
    return prefix + ", ".join(shown)


def session_guard_hints(report: SessionGuardReport) -> List[str]:
    """Short clauses for the session-start Continuity line, each length-capped."""
    hints: List[str] = []
    if report.live_siblings:
        items = [
            f"{_cap(Path(s.path).name, MAX_FIELD_CHARS)} "
            f"[{_cap(s.branch or 'detached', MAX_FIELD_CHARS)}] {s.reason} {_age_phrase(s.age_seconds)}"
            for s in report.live_siblings
        ]
        noun = "checkout" if len(items) == 1 else "checkouts"
        hints.append(_cap(f"{len(items)} live sibling {noun}: {_listing(items)}", MAX_HINT_CHARS))
    if report.divergent_branches:
        items = [
            f"{_cap(b.name, MAX_FIELD_CHARS)} +{b.ahead} ({_age_phrase(b.tip_age_seconds)})"
            for b in report.divergent_branches
        ]
        noun = "branch" if len(items) == 1 else "branches"
        hints.append(
            _cap(
                f"{len(items)} unmerged {noun} ahead of {report.default_branch}: {_listing(items)}",
                MAX_HINT_CHARS,
            )
        )
    if (report.behind or 0) > 0:
        hints.append(
            _cap(
                f"this checkout is {report.behind} behind {report.default_branch} — "
                "merge or rebase before you start",
                MAX_HINT_CHARS,
            )
        )
    if report.truncated:
        hints.append(_cap("partial: " + "; ".join(report.truncated), MAX_HINT_CHARS))
    if report.unavailable:
        hints.append(_cap("could not check: " + "; ".join(report.unavailable), MAX_HINT_CHARS))
    return hints


def session_guard_detail(report: SessionGuardReport) -> str:
    """One line for ``dev map doctor``: full paths, no basename shortening."""
    if not report.is_repository:
        return "not a git repository"
    parts: List[str] = []
    for sibling in report.live_siblings:
        parts.append(
            f"{sibling.path} [{sibling.branch or 'detached'}] {sibling.reason} "
            f"{_age_phrase(sibling.age_seconds)}"
        )
    for branch in report.divergent_branches:
        parts.append(
            f"{branch.name} is {branch.ahead} ahead of {report.default_branch} "
            f"(tip {_age_phrase(branch.tip_age_seconds)})"
        )
    if (report.behind or 0) > 0:
        parts.append(f"this checkout is {report.behind} behind {report.default_branch}")
    if report.truncated:
        parts.append("partial: " + "; ".join(report.truncated))
    if report.unavailable:
        parts.append("could not check: " + "; ".join(report.unavailable))
    if not parts:
        return "none"
    return "; ".join(parts)
