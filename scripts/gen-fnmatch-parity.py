#!/usr/bin/env python3
"""Regenerate testdata/fnmatch-parity.tsv from CPython's fnmatchcase.

The Go and Rust matchers must agree with each other and with the Python
engine whose path rules they port. One fixture, generated from the authority,
read by both — so a divergence fails a test instead of quietly widening or
narrowing the write gate in one language only.

    python3 scripts/gen-fnmatch-parity.py > testdata/fnmatch-parity.tsv

fnmatchcase, not fnmatch: fnmatch runs os.path.normcase first, which folds
case on Windows. The matchers are case-sensitive. Generating the fixture with
fnmatch on Windows would pin the wrong answer.
"""

import fnmatch

# Every glob shape DevCouncil's policy engine actually uses, plus the character
# class and edge cases the matchers have to get right.
PATTERNS = [
    "*.py",
    "**/.env",
    ".env",
    ".env.*",
    "src/*",
    "**/credentials/**",
    "**/*.pem",
    ".claude/*",
    ".claude/**",
    ".git/*",
    ".devcouncil/*",
    ".github/workflows/*.yml",
    "package.json",
    "**/id_rsa",
    "src/legacy/**",
    "a[bc]d",
    "a[!bc]d",
    # CPython negates only on '!': '[^…]' is a literal set containing '^'.
    "[^a]x",
    ".env[^1]",
    "f[^a-c]o",
    "x?z",
    "[]]a",
    "**/secrets/**",
    "uv.lock",
    "*",
]

NAMES = [
    "src/foo.py",
    "foo.py",
    "a/b/.env",
    ".env",
    ".env.local",
    "src/a/b/c.py",
    "x/credentials/y",
    "credentials/y",
    "x/credentials/y/z",
    "a.pem",
    "deep/a.pem",
    ".claude/settings.json",
    ".claude/a/b",
    ".git/config",
    ".devcouncil/state.sqlite",
    ".github/workflows/ci.yml",
    "package.json",
    "sub/package.json",
    "home/id_rsa",
    "src/legacy/old.go",
    "src/legacy/a/b.go",
    "abd",
    "acd",
    "axd",
    "xyz",
    "x/z",
    "]a",
    "uv.lock",
    "p/secrets/k",
    "secrets/k",
    "",
]

# Rows the cartesian product above does not generate. CPython keeps the
# members around an out-of-order range; a regexp translation of the same class
# fails to compile and matches nothing. These rows are what made the two
# matchers disagree.
EXTRA = [
    ("[c-a-e]", "e"),
    ("[c-a-e]", "-"),
    ("[c-a-e]", "a"),
    ("[c-a-e]", "c"),
    ("[a--c]", "c"),
    ("[a--c]", "a"),
    ("[a--c]", "-"),
    ("[a--c]", "b"),
    ("[z-a]", "z"),
    ("[z-a]", "a"),
    ("*[z-a]", "hello"),
    ("[!]", "[!]"),
    ("[!]", "a"),
    ("[]", "[]"),
    ("[]", "a"),
    ("[a-c-e]", "b"),
    ("[a-c-e]", "d"),
    ("[a-c-e]", "-"),
    ("[a-c-e]", "e"),
    ("[a-c-e-g-i]", "b"),
    ("[a-c-e-g-i]", "d"),
    ("[a-c-e-g-i]", "f"),
    ("[a-c-e-g-i]", "h"),
    ("[a-c-e-g-i]", "i"),
    ("[a-c-e-g-i]", "-"),
]


def emit(pattern: str, name: str) -> None:
    if "\t" in pattern or "\n" in pattern or "\t" in name or "\n" in name:
        raise SystemExit(f"fixture cell contains a tab or newline: {pattern!r} {name!r}")
    want = fnmatch.fnmatchcase(name, pattern)
    print(f"{pattern}\t{name}\t{str(want).lower()}")


def main() -> None:
    print(
        "# pattern\tname\texpected  — generated from CPython fnmatchcase; "
        "regenerate with scripts/gen-fnmatch-parity.py"
    )
    for pattern in PATTERNS:
        for name in NAMES:
            emit(pattern, name)
    for pattern, name in EXTRA:
        emit(pattern, name)


if __name__ == "__main__":
    main()
