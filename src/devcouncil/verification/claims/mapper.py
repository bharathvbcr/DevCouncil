"""Claim-to-check mapper: free-text completion claims → structured assertions.

Port of Oversight semantics: sentence-scoped negation/hedging; specific
assertions suppress GENERIC_DONE; capped to MAX_ASSERTIONS.
"""

from __future__ import annotations

import re

from devcouncil.verification.claims.models import Assertion, Kind

MAX_ASSERTIONS = 5

_SENTENCE_SPLIT = re.compile(r"(?<=[.!?])\s+|\n+")

_BENIGN_NEGATIVE = re.compile(
    r"\b(?:0|no|zero|without)\s+(?:\w+\s+)?(?:failures?|errors?|warnings?|regressions?|issues?)\b"
    r"|\bnone\s+failed\b",
    re.I,
)

_HEDGE = re.compile(
    r"\b(?:not|never|no|fail(?:s|ed|ing|ure)?|should|would|could|might|"
    r"will|going\s+to|plan(?:s|ning)?\s+to|need(?:s)?\s+to|todo|"
    r"haven'?t|hasn'?t|didn'?t|don'?t|doesn'?t|won'?t|can'?t|isn'?t|aren'?t|"
    r"still|yet|broken|remaining\s+work)\b",
    re.I,
)

_TESTS_PASS = re.compile(
    r"\btests?\s+(?:are\s+|all\s+|now\s+)*pass(?:es|ing)?\b"
    r"|\btest\s+suite\s+passes\b"
    r"|\b(?:pytest|jest|vitest|unittest)\b.*\b(?:green|passed|passing)\b"
    r"|\b\d+\s+(?:tests?\s+)?(?:passed|passing)\b"
    r"|\b(?:test\s+)?suite\s+is\s+green\b"
    r"|\b(?:all\s+)?checks\s+(?:are\s+)?green\b"
    r"|\b(?:unit\s+)?tests\s+succeed(?:ed)?\b"
    r"|\bci\s+is\s+(?:happy|green|passing)\b"
    r"|\beverything\s+passed\b",
    re.I,
)
_BUILD = re.compile(
    r"\bbuild\s+(?:succeeds|succeeded|passes|passed|completed\s+successfully|is\s+(?:now\s+)?(?:passing|clean|green))\b"
    r"|\bcompiles?(?:\s+cleanly)?\b"
    r"|\bbuilds\s+cleanly\b"
    r"|\bbuild\s+completed\b",
    re.I,
)
_LINT = re.compile(
    r"\blint(?:ing|er)?\s+(?:passes|passed|is\s+clean|clean)\b"
    r"|\bno\s+(?:type|lint(?:ing)?)\s+errors\b"
    r"|\btype-?check(?:ing)?\s+passes\b"
    r"|\bno\s+lint\s+(?:errors|warnings)\b"
    r"|\b(?:mypy|ruff|eslint|flake8)\b.*\b(?:passes|passed|clean)\b",
    re.I,
)
_GENERIC_DONE = re.compile(
    r"\b(?:done|completed?|finished|everything\s+works|all\s+set|task\s+is\s+complete|implementation\s+is\s+complete)\b",
    re.I,
)
_CREATE_VERB = re.compile(r"\b(?:created|added|wrote|generated|new\s+file)\b", re.I)
_MODIFY_VERB = re.compile(r"\b(?:updated|edited|modified|changed|fixed|refactored|patched|rewrote)\b", re.I)
_RAN_COMMAND = re.compile(r"\b(?:ran|executed|invoked)\s+[`\"']([^`\"'\n]+)[`\"'].*?\bsuccess", re.I)

_QUOTED_SPAN = re.compile(r"[`\"']([^`\"'\n]+)[`\"']")
_BARE_PATH = re.compile(r"(?<![\w./\\-])(?:[A-Za-z]:[/\\])?[\w.-]+(?:[/\\][\w.-]+)+(?![\w/\\])")
_KNOWN_EXTENSIONS = {
    "py", "js", "ts", "tsx", "jsx", "json", "md", "txt", "toml", "yaml", "yml",
    "html", "css", "scss", "sql", "sh", "ps1", "psm1", "bat", "cs", "java",
    "go", "rs", "c", "cpp", "h", "hpp", "rb", "php", "xml", "csv", "ini",
    "cfg", "lock", "env",
}


def _has_known_extension(token: str) -> bool:
    name = token.replace("\\", "/").rsplit("/", 1)[-1]
    if "." not in name:
        return False
    return name.rsplit(".", 1)[-1].lower() in _KNOWN_EXTENSIONS


def _looks_like_path(token: str, bare: bool = False) -> bool:
    token = token.strip()
    if not token or " " in token:
        return False
    if bare:
        return _has_known_extension(token)
    if "/" in token or "\\" in token:
        return True
    return _has_known_extension(token)


def _extract_paths(sentence: str) -> list[str]:
    paths: list[str] = []
    seen: set[str] = set()
    for match in _QUOTED_SPAN.finditer(sentence):
        token = match.group(1).strip().rstrip(".,;:!?")
        if _looks_like_path(token) and token not in seen:
            seen.add(token)
            paths.append(token)
    without_quotes = _QUOTED_SPAN.sub(" ", sentence)
    for match in _BARE_PATH.finditer(without_quotes):
        token = match.group(0).rstrip(".,;:!?")
        if _looks_like_path(token, bare=True) and token not in seen:
            seen.add(token)
            paths.append(token)
    return paths


def map_claims(text: str) -> list[Assertion]:
    """Parse a completion message into a deduplicated, capped assertion list."""
    if not text or not text.strip():
        return []

    specific: list[Assertion] = []
    generic: list[Assertion] = []
    seen: set[tuple[Kind, str | None]] = set()

    def add(bucket: list[Assertion], kind: Kind, target: str | None, source: str) -> None:
        key = (kind, target)
        if key in seen:
            return
        seen.add(key)
        bucket.append(Assertion(kind=kind, target=target, source_text=source.strip()))

    for sentence in _SENTENCE_SPLIT.split(text):
        sentence = sentence.strip()
        if not sentence:
            continue
        sanitized = _BENIGN_NEGATIVE.sub(" ", sentence)
        if _HEDGE.search(sanitized):
            continue

        if _TESTS_PASS.search(sentence):
            add(specific, Kind.TESTS_PASS, None, sentence)
        if _BUILD.search(sentence):
            add(specific, Kind.BUILD_SUCCEEDS, None, sentence)
        if _LINT.search(sentence):
            add(specific, Kind.LINT_CLEAN, None, sentence)

        ran = _RAN_COMMAND.search(sentence)
        if ran:
            add(specific, Kind.COMMAND_SUCCEEDED, ran.group(1).strip(), sentence)

        paths = _extract_paths(sentence)
        if paths:
            if _CREATE_VERB.search(sentence):
                for path in paths:
                    add(specific, Kind.FILE_CREATED, path, sentence)
            elif _MODIFY_VERB.search(sentence):
                for path in paths:
                    add(specific, Kind.FILE_UPDATED, path, sentence)

        if _GENERIC_DONE.search(sentence):
            add(generic, Kind.GENERIC_DONE, None, sentence)

    priority = {
        Kind.TESTS_PASS: 0,
        Kind.BUILD_SUCCEEDS: 1,
        Kind.LINT_CLEAN: 2,
        Kind.COMMAND_SUCCEEDED: 3,
        Kind.FILE_CREATED: 4,
        Kind.FILE_UPDATED: 5,
    }
    specific.sort(key=lambda a: priority.get(a.kind, 9))
    result = specific if specific else generic
    return result[:MAX_ASSERTIONS]
