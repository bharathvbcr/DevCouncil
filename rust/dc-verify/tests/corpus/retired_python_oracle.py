"""The retired Python gates, measured on the same hand-labelled corpora as Rust.

rust/STATUS.md compares `scan_secrets` and `classify_scope` with the Python
they replaced. This script is where the Python column comes from, so the
comparison can be re-run rather than taken on trust. It is a reference oracle:
it ships nothing and nothing calls it.

  python3 -I rust/dc-verify/tests/corpus/retired_python_oracle.py

Secrets: `SECRET_PATTERNS` is loaded from `redaction.py` as it stood before
the Python was retired (`3286db5e^`), read out of git and executed verbatim —
not copied, so it cannot drift from what actually ran. Each line is searched
as `+<line>`, as `SecretScanner.scan_diff` did.

Scope: the retired rule, `cf not in planned_paths` (`orphan_diff.py`), where
`planned_paths = {pf.path}` (`verify_orchestration.py:93`) and the changed
files are what `git diff --name-only` lists — the destination only, for a
rename. That rule is modelled here, not executed: it ran against a live
repository, and the corpus is diffs.
"""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path

RETIRED_AT = "3286db5e^"
CORPUS = Path(__file__).resolve().parent
REPO = CORPUS.parents[3]


def matrix() -> dict[str, int]:
    return {"tp": 0, "fp": 0, "fn": 0, "tn": 0}


def record(m: dict[str, int], positive: bool, flagged: bool) -> None:
    if positive:
        m["tp" if flagged else "fn"] += 1
    else:
        m["fp" if flagged else "tn"] += 1


def report(name: str, m: dict[str, int]) -> str:
    def ratio(num: int, den: int) -> str:
        return f"{num / den:.3f} ({num}/{den})" if den else "undefined"

    return (
        f"{name}: tp {m['tp']} fp {m['fp']} fn {m['fn']} tn {m['tn']}; "
        f"precision {ratio(m['tp'], m['tp'] + m['fp'])}, "
        f"recall {ratio(m['tp'], m['tp'] + m['fn'])}"
    )


def retired_secret_patterns() -> list:
    source = subprocess.check_output(
        [
            "git",
            "-C",
            str(REPO),
            "show",
            f"{RETIRED_AT}:src/devcouncil/utils/redaction.py",
        ],
        text=True,
    )
    namespace: dict = {}
    exec(compile(source, "redaction.py", "exec"), namespace)
    return list(namespace["SECRET_PATTERNS"].values())


def measure_secrets(path: Path, patterns: list) -> dict[str, int]:
    m = matrix()
    for line in path.read_text(encoding="utf-8").splitlines():
        if not line or line.startswith("#"):
            continue
        label, _path, _note, content = line.split("\t", 3)
        if label not in ("secret", "clean"):
            sys.exit(f"{path.name}: label {label!r} is not secret or clean")
        record(m, label == "secret", any(p.search("+" + content) for p in patterns))
    return m


def name_only(diff: list[str]) -> list[str]:
    """The paths `git diff --name-only` prints for a diff: one per file, the
    destination for a rename or copy."""
    names: list[str] = []
    current: str | None = None
    renamed_to: str | None = None
    for line in diff:
        if line.startswith("diff --git "):
            if current is not None:
                names.append(renamed_to or current)
            rest = line[len("diff --git ") :]
            current = (
                rest.split('" "')[1].rstrip('"')[2:]
                if rest.startswith('"')
                else rest.split(" b/", 1)[1]
            )
            renamed_to = None
        elif line.startswith(("rename to ", "copy to ")):
            renamed_to = line.split(" to ", 1)[1]
    if current is not None:
        names.append(renamed_to or current)
    return names


def measure_scope(path: Path) -> dict[str, int]:
    m = matrix()
    cases: list[dict] = []
    for line in path.read_text(encoding="utf-8").splitlines():
        if line.startswith("### case:"):
            cases.append({"planned": [], "labels": {}, "diff": []})
        elif not cases:
            continue
        elif line.startswith("### planned:"):
            cases[-1]["planned"].append(line.split(":", 1)[1].strip())
        elif line.startswith(("### orphan:", "### in:")):
            key, value = line[4:].split(":", 1)
            cases[-1]["labels"][value.strip()] = key == "orphan"
        elif not line.startswith("###"):
            cases[-1]["diff"].append(line)
    for case in cases:
        planned = set(case["planned"])
        orphans = {p for p in name_only(case["diff"]) if p not in planned}
        for p, positive in case["labels"].items():
            record(m, positive, p in orphans)
    return m


def main() -> None:
    patterns = retired_secret_patterns()
    for name in ("secrets.tsv", "secrets_holdout.tsv", "secrets_holdout_2.tsv"):
        print(
            report(
                f"retired secret scan on {name}",
                measure_secrets(CORPUS / name, patterns),
            )
        )
    print(
        report("retired orphan rule on scope.txt", measure_scope(CORPUS / "scope.txt"))
    )


if __name__ == "__main__":
    main()
