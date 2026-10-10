import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))

from pkg.util import split  # noqa: E402  C06 sys.path-resolved import


def main() -> list[str]:
    return split("a,b")


if __name__ == "__main__":
    main()
