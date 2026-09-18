#!/usr/bin/env python3
"""Encode a repository with a learned sparse model, for `dcgrep index --sparse`.

This is the only part of the ranked search stack that runs a neural network,
and it runs here, offline, on the operator's machine — never inside `dcgrep`.
It reads the repository, encodes every file with a doc-side sparse encoder, and
writes one JSONL file that `dcgrep index` imports. After that the model is not
needed again: the query side of the ranking is a token/weight table that ships
inside the index, so searching stays a single static binary with no Python, no
PyTorch and no inference.

    pip install torch transformers          # not a DevCouncil dependency
    python3 scripts/encode-sparse.py --root . --out /tmp/sparse.jsonl
    dcgrep index <<< '{"root":".","sparse":"/tmp/sparse.jsonl"}'

## Which model

The default is `opensearch-project/opensearch-neural-sparse-encoding-doc-v2-mini`
— Apache-2.0, 23M parameters, about 23 MB on disk. Any model in that family
works, and they all share the same 30522-token BERT WordPiece vocabulary, so
`dcgrep` needs no change to read a larger one:

    doc-v2-mini          23M   fastest, smallest
    doc-v2-distill       67M   the usual default elsewhere
    doc-v3-distill      133M   strongest of the Apache-2.0 doc-side family

SPLADE models (`naver/splade-*`) also emit this vocabulary and work with
`--model`, but their weights are CC BY-NC-SA 4.0 — non-commercial. DevCouncil
is Apache-2.0 and ships no weights either way; using SPLADE is the operator's
licensing decision, and this script will say so and continue.

## Why `doc-`

The `doc-` models are trained so that only the document side needs the network.
A query is scored by looking each token up in a stored weight table, which is
what makes a learned ranking possible in a binary that cannot run a model. A
non-`doc-` model would need inference on every query, and `dcgrep` has nowhere
to put it.

## The parity block

The header carries sample `(text, ids)` pairs from this tokenizer. `dcgrep`
replays them through its own Rust WordPiece and refuses the build on a single
disagreement. Two tokenizers that differ do not return fewer results — they
return a ranking over ids nothing was indexed under, and every score still
looks plausible. The samples turn that into a build failure.
"""

from __future__ import annotations

import argparse
import json
import os
import random
import subprocess
import sys
from pathlib import Path

DEFAULT_MODEL = "opensearch-project/opensearch-neural-sparse-encoding-doc-v2-mini"

# Matches the ceilings dcgrep enforces on its own side. Exceeding them is not a
# crash there, it is a refused build, so they are applied here too rather than
# discovered after an hour of encoding.
MAX_TERMS_PER_DOCUMENT = 20_000
MAX_DOCUMENTS = 200_000
MAX_FILE_BYTES = 2 * 1024 * 1024

# Text-ish extensions. Deliberately a list rather than a guess: a binary file
# encoded as if it were text produces confident weights for nothing.
SOURCE_SUFFIXES = {
    ".c", ".cc", ".cpp", ".cs", ".css", ".go", ".h", ".hpp", ".html", ".java",
    ".js", ".json", ".jsx", ".kt", ".lua", ".md", ".mjs", ".php", ".proto",
    ".py", ".rb", ".rs", ".scala", ".sh", ".sql", ".swift", ".toml", ".ts",
    ".tsx", ".txt", ".vue", ".yaml", ".yml", ".zsh",
}

SKIP_DIRS = {
    ".git", ".hg", ".svn", ".devcouncil", "node_modules", "target", "dist",
    "build", "vendor", "__pycache__", ".venv", "venv", ".mypy_cache",
    ".pytest_cache", ".ruff_cache", ".next", ".cargo",
}

# Sent through the tokenizer and recorded so dcgrep can prove its own tokenizer
# agrees. Every entry is a decision two implementations can differ on, and most
# of them are here because they *did* differ: the Rust side used a hand-written
# transliteration table where the reference does canonical decomposition, which
# disagreed on 22% of a codepoint sweep while a curated list like this one had
# reported 0.3%. A sample set that only contains what its author thought to
# doubt is not a gate.
#
# One thing not to put here. dcgrep's Unicode tables are newer than this
# tokenizer's, and on 98 codepoints — Arabic Extended-A/B marks, Indic marks,
# Supplemental Punctuation, listed as TABLE_SKEW in
# rust/dc-grep/src/lexical/wordpiece.rs — the two genuinely disagree. Adding one
# of them below makes *every* build refuse, with a parity-gate message that is
# accurate but reads like a regression in code nobody touched. The divergence is
# a miss on rare marks, not a wrong answer, and it is asserted where it can be
# described; a sample here would only convert it into an outage.
PARITY_TEXTS = [
    # Identifier shapes: camel, snake, acronym runs, digits.
    "parse", "json", "parseJson", "parseJSONResponse", "HTTPServer",
    "http_server", "server2", "v2", "read_file", "readFile", "FILE",
    "unwrap_or_default", "tokenize", "zzqqxx", "sha256sum", "utf8",
    "supercalifragilisticexpialidocious",
    # Letters that decompose to themselves and must NOT be folded to ASCII.
    "ß", "ẞ", "ø", "Ø", "æ", "Æ", "ð", "þ", "đ", "ı", "ł", "Łódź", "œ",
    # Letters that are a base plus a mark, which is dropped.
    "Böse", "naïve", "café", "École", "Ångström", "žluťoučký",
    "Việt", "Tiếng", "ﬁle",
    # The same text with the mark typed separately rather than precomposed.
    "e\u0301cole", "n\u0303ino",
    # Turkish dotted capital I, which lowercases to two characters.
    "İstanbul",
    # Scripts that reach the vocabulary one character at a time.
    "日本語", "中文字符", "한국어", "снег", "Ελληνικά", "עברית",
    "العربية", "ไทย", "हिन्दी",
    # Characters that are discarded, and characters that are not.
    "a\u200bb", "a\ufeffb", "a\u00a0b", "a\u0000b", "a\u000bb",
    "🎉", "a🎉b", "→", "±", "§", "§8", "audit §e", "€", "℃",
    # Punctuation, including the fullwidth and CJK forms.
    "a.b.c", "foo::bar", "x-=+", "don't", "e.g.", "U.S.A.",
    "path/to/file.rs", "https://example.com/a?b=c", "user@example.com",
    "#[derive(Debug)]", "{\"k\":[1,2]}", "// line", "全角。句点",
    # Whitespace and emptiness.
    "", " ", "   ", "\t\n", "  spaced  out  ",
    # The per-word ceiling, on both sides of it.
    "a" * 99, "a" * 100, "a" * 101,
]


def eprint(*args: object) -> None:
    print(*args, file=sys.stderr)


def load_model(name: str, device: str):
    """Imports torch/transformers late, so `--help` works without them."""
    try:
        import torch
        from transformers import AutoModelForMaskedLM, AutoTokenizer
    except ImportError as err:
        eprint(f"missing dependency: {err}")
        eprint("")
        eprint("This script needs PyTorch and Transformers. They are not")
        eprint("DevCouncil dependencies and nothing else in the repository")
        eprint("installs them:")
        eprint("")
        eprint("    pip install torch transformers")
        eprint("")
        eprint("dcgrep itself never loads them. Once this script has written")
        eprint("its output you can uninstall both.")
        raise SystemExit(2) from err

    if "splade" in name.lower():
        eprint(
            f"note: {name} looks like a SPLADE model. SPLADE weights are\n"
            "      CC BY-NC-SA 4.0 (non-commercial). DevCouncil is Apache-2.0\n"
            "      and ships no weights; whether that licence suits your use\n"
            "      is your call. Continuing."
        )

    tokenizer = AutoTokenizer.from_pretrained(name)
    model = AutoModelForMaskedLM.from_pretrained(name)
    model.eval()
    model.to(device)
    return torch, tokenizer, model


def vocabulary(tokenizer) -> list[str]:
    """Token strings indexed by id, dense and complete.

    `get_vocab()` is a dict and its order is not the id order. Building the
    list by position is what makes ids mean the same thing on both sides —
    the single most consequential line in this script.
    """
    raw = tokenizer.get_vocab()
    size = max(raw.values()) + 1
    tokens: list[str | None] = [None] * size
    for token, index in raw.items():
        tokens[index] = token
    # A hole would shift every later id. Fill it with a name that can never
    # collide with a real token rather than leaving a null in the JSON.
    return [token if token is not None else f"[unused_hole_{i}]" for i, token in enumerate(tokens)]


def query_weights(tokenizer, size: int) -> list[float]:
    """The weight each token contributes when it appears in a query.

    For the `doc-` models this is the IDF table the model was distilled
    against, published with the model as `idf.json`. When it is absent every
    in-vocabulary token weighs 1.0, which is a flat query side: still a valid
    learned ranking, because the document weights are what the model computed,
    but a weaker one. The fallback is announced rather than assumed.
    """
    weights = [0.0] * size
    try:
        from huggingface_hub import hf_hub_download  # noqa: PLC0415 (late)

        path = hf_hub_download(tokenizer.name_or_path, "idf.json")
    except Exception as err:  # noqa: BLE001 - any failure means "no table"
        eprint(f"note: no idf.json for this model ({err.__class__.__name__}); "
               "queries will weigh every known token equally.")
        vocab = tokenizer.get_vocab()
        for index in vocab.values():
            if index < size:
                weights[index] = 1.0
        return weights

    with open(path, encoding="utf-8") as handle:
        table = json.load(handle)
    vocab = tokenizer.get_vocab()
    for token, weight in table.items():
        index = vocab.get(token)
        if index is not None and index < size:
            weights[index] = float(weight)
    return weights


def special_ids(tokenizer) -> set[int]:
    """Ids that carry no meaning about a document.

    `[CLS]`, `[SEP]` and `[PAD]` appear in every encoding, so a weight on them
    ranks every document equally and wastes a posting per file.
    """
    ids = set(tokenizer.all_special_ids or [])
    ids.discard(tokenizer.unk_token_id)
    return ids


def files_from_dcgrep(
    dcgrep: str, root: Path, max_files: int
) -> tuple[list[Path], bool] | None:
    """Ask the indexer which files it would index, rather than guessing.

    `dcgrep files` is the same walk `dcgrep index` performs: the same ignore
    rules, the same hidden-file and size decisions, the same `.gitignore`. A
    document encoded for a path the walk will not admit is work thrown away —
    over this repository the local walk below produced 498 of them, a third of
    the run — and, worse, a file the walk *does* admit that this misses is a
    hole in the ranking that nothing reports.

    Two implementations of "which files are in this repository" drift. This one
    has no second implementation to drift from.

    Returns None when dcgrep cannot be reached, so the caller can fall back and
    say so rather than silently encoding nothing. Otherwise returns the paths
    and whether the listing was complete — a truncated listing is a prefix of
    the corpus, and encoding a prefix produces an index that is learned for the
    files it reached and BM25 for the rest, which is not what "learned" claims.
    """
    request = json.dumps({
        "root": str(root),
        "max_results": max_files,
        # The producer reads every file whole; the searcher's own ceiling is
        # what decides which are indexable, so it is left at its default.
    })
    try:
        proc = subprocess.run([dcgrep, "files"], input=request.encode(),
                              capture_output=True, timeout=300)
        reply = json.loads(proc.stdout or b"{}")
    except (OSError, subprocess.SubprocessError, json.JSONDecodeError) as err:
        eprint(f"note: could not ask {dcgrep} for the file list ({err.__class__.__name__}).")
        return None
    if not reply.get("ok"):
        eprint(f"note: {dcgrep} files refused: {str(reply.get('error'))[:200]}")
        return None
    # Only the extensions this script can read as text. The walk admits more
    # than the encoder should encode.
    paths = [root / rel for rel in reply.get("paths", [])]
    kept = [p for p in paths if p.suffix.lower() in SOURCE_SUFFIXES]
    return kept, not reply.get("truncated")


def walk(root: Path, max_files: int) -> list[Path]:
    """Files worth encoding, in a stable order — without asking the indexer.

    The fallback for when `dcgrep` is not on the path. It does not read
    `.gitignore`, so it will encode files the index will then decline; the
    build reports the difference as `lexical_unmatched`.

    Sorted, so two runs over an unchanged tree produce byte-identical output
    and a diff of two encodings shows what the model did rather than what the
    filesystem felt like returning.
    """
    found: list[Path] = []
    for base, dirs, names in os.walk(root):
        dirs[:] = sorted(d for d in dirs if d not in SKIP_DIRS and not d.startswith("."))
        for name in sorted(names):
            path = Path(base, name)
            if path.suffix.lower() not in SOURCE_SUFFIXES:
                continue
            try:
                if path.is_symlink() or not path.is_file():
                    continue
                if path.stat().st_size > MAX_FILE_BYTES:
                    continue
            except OSError:
                continue
            found.append(path)
            if len(found) >= max_files:
                eprint(f"note: stopping at {max_files} files; pass --max-files to raise it.")
                return found
    return found


def encode(torch, tokenizer, model, device: str, texts: list[str], drop: set[int]):
    """One batch of documents to `([(id, weight), ...], token_count)` each."""
    encoded = tokenizer(
        texts,
        padding=True,
        truncation=True,
        max_length=512,
        return_tensors="pt",
    ).to(device)
    with torch.no_grad():
        logits = model(**encoded).logits
    # The doc-side sparse representation: log(1 + relu(logits)), max-pooled
    # over the sequence and masked to real tokens. This is the transformation
    # the OpenSearch doc- models are trained to produce.
    activated = torch.log1p(torch.relu(logits))
    mask = encoded["attention_mask"].unsqueeze(-1)
    pooled = torch.max(activated * mask, dim=1).values

    # The real length of each document in tokens, after truncation at 512.
    # dcgrep stores it as the document length; for a learned vocabulary it does
    # not scale the weights (the model already normalised them), so it is
    # reported rather than applied — which is a reason to make it true, not a
    # reason to put the nonzero count there and call it a length.
    lengths = encoded["attention_mask"].sum(dim=1).tolist()

    out = []
    for row, length in zip(pooled, lengths):
        nonzero = torch.nonzero(row, as_tuple=False).flatten()
        pairs = [
            (int(i), round(float(row[i]), 4))
            for i in nonzero.tolist()
            if int(i) not in drop
        ]
        pairs = [(i, w) for i, w in pairs if w > 0.0]
        if len(pairs) > MAX_TERMS_PER_DOCUMENT:
            # Keep the heaviest. A document truncated by id order would be
            # described by whichever tokens happened to sort first.
            pairs.sort(key=lambda pair: pair[1], reverse=True)
            pairs = pairs[:MAX_TERMS_PER_DOCUMENT]
        pairs.sort(key=lambda pair: pair[0])
        out.append((pairs, int(length)))
    return out


def build_header(model: str, tokens: list[str], weights: list[float], tokenize) -> dict:
    """The header line, including the parity block dcgrep gates the build on.

    `tokenize` is the tokenizer's own text-to-ids function. It is a parameter
    rather than a closed-over global so the self-test can drive this exact code
    path without a model, which is the difference between a producer that has
    been run once by hand and one that is checked.
    """
    return {
        "schema": 1,
        "model": model,
        "vocabulary": "wordpiece-30522",
        "vocab": tokens,
        "query_weights": weights,
        # dcgrep replays these through its own tokenizer and refuses the build
        # if a single one disagrees.
        "parity": [{"text": text, "ids": tokenize(text)} for text in PARITY_TEXTS],
    }


def write_encoding(out: Path, root: Path, files: list[Path], header: dict,
                   encode_batch, batch_size: int) -> int:
    """Writes the JSONL, atomically, and returns how many documents it holds.

    `encode_batch(texts) -> [(pairs, token_count), ...]`.
    """
    written = 0
    # A partial file must not be importable as a whole one. Written beside the
    # target and renamed, so an interrupted run leaves no index-shaped debris.
    staging = out.with_name(out.name + ".partial")
    with open(staging, "w", encoding="utf-8") as handle:
        handle.write(json.dumps(header, ensure_ascii=False) + "\n")
        for start in range(0, len(files), batch_size):
            batch = files[start : start + batch_size]
            texts, keep = [], []
            for path in batch:
                try:
                    texts.append(path.read_text(encoding="utf-8"))
                    keep.append(path)
                except (OSError, UnicodeDecodeError):
                    continue
            if not texts:
                continue
            for path, (pairs, length) in zip(keep, encode_batch(texts)):
                if not pairs:
                    continue
                handle.write(
                    json.dumps(
                        {
                            "path": path.relative_to(root).as_posix(),
                            # The document's length in tokens, not its byte
                            # size and not its count of distinct tokens.
                            "total_terms": length,
                            "terms": [[i, w] for i, w in pairs],
                        },
                        ensure_ascii=False,
                    )
                    + "\n"
                )
                written += 1
            done = min(start + batch_size, len(files))
            eprint(f"  {done}/{len(files)}")
    staging.replace(out)
    return written


# Ranges the conformance corpus sweeps, one codepoint at a time. Chosen to
# cover every class the tokenizer treats differently — not a sample of what
# looked interesting.
RECORD_RANGES = [
    (0x0020, 0x024F, "ASCII + Latin-1 + Latin Ext-A/B"),
    (0x0250, 0x02FF, "IPA + modifiers"),
    (0x0300, 0x036F, "combining marks"),
    (0x0370, 0x03FF, "Greek"),
    (0x0400, 0x04FF, "Cyrillic"),
    (0x0530, 0x058F, "Armenian"),
    (0x0590, 0x05FF, "Hebrew"),
    # Through 08FF rather than 06FF: Arabic Extended-A holds marks assigned
    # after the model tokenizer's Unicode tables were built, and stopping at
    # 06FF is what kept 23 of them out of this fixture.
    (0x0600, 0x08FF, "Arabic + Syriac + Thaana + NKo + Arabic Ext-A"),
    # Every Indic block, not Devanagari alone. Bengali, Gujarati, Oriya,
    # Telugu, Kannada, Malayalam and Sinhala each contributed skewed marks.
    (0x0900, 0x0DFF, "Devanagari through Sinhala"),
    (0x0E00, 0x0FFF, "Thai + Lao + Tibetan"),
    (0x1000, 0x109F, "Myanmar"),
    (0x1600, 0x169F, "Canadian Syllabics tail + Ogham"),
    (0x1800, 0x18AF, "Mongolian"),
    (0x1B00, 0x1B7F, "Balinese"),
    (0x1DC0, 0x1DFF, "combining marks supplement"),
    (0x1E00, 0x1EFF, "Latin Extended Additional"),
    (0x2000, 0x206F, "punctuation + spaces"),
    # Supplemental Punctuation: U+2E43..U+2E5D are the largest single group of
    # characters this build splits as punctuation and the model's tokeniser
    # does not.
    (0x2E00, 0x2E7F, "supplemental punctuation"),
    (0x20A0, 0x20CF, "currency"),
    (0x2100, 0x21FF, "letterlike + arrows"),
    (0x2200, 0x22FF, "math operators"),
    (0x2500, 0x257F, "box drawing"),
    (0x2600, 0x26FF, "misc symbols"),
    (0x3000, 0x303F, "CJK punctuation"),
    (0x4E00, 0x4E7F, "CJK ideographs (sample)"),
    (0xAC00, 0xAC7F, "Hangul syllables (sample)"),
    (0xFE00, 0xFEFF, "variation selectors + BOM"),
    (0xFF00, 0xFF6F, "fullwidth forms"),
    (0x1F600, 0x1F64F, "emoji"),
]


def record(model: str, out_dir: Path, root: Path) -> int:
    """Re-record rust/dc-grep/tests/fixtures/wordpiece-* from the real tokenizer.

    The Rust conformance test judges this crate's WordPiece against these
    files. They are committed so that check runs on an ordinary `cargo test`,
    with no PyTorch and no download — a check that needs a model to run is a
    check that stops running. Re-record when the model or its tokenizer
    changes, and read the diff: a change here is a change in what queries mean.
    """
    _, tokenizer, _ = load_model(model, "cpu")
    tokens = vocabulary(tokenizer)

    def ids_of(text: str) -> list[int]:
        return tokenizer(text, add_special_tokens=False)["input_ids"]

    cases: list[str] = []
    for lo, hi, _label in RECORD_RANGES:
        for cp in range(lo, hi + 1):
            cases.append(chr(cp))
            cases.append(f"x{chr(cp)}y")
    cases.extend(PARITY_TEXTS)

    # Real lines out of the repository, which cover multi-word behaviour a
    # single-codepoint sweep cannot reach.
    rng = random.Random(20260917)
    harvested: list[str] = []
    for path in walk(root, 4000):
        try:
            text = path.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            continue
        lines = [ln.strip() for ln in text.splitlines() if 3 < len(ln.strip()) < 160]
        if lines:
            harvested.extend(rng.sample(lines, min(4, len(lines))))
    rng.shuffle(harvested)
    cases.extend(harvested[:900])

    out_dir.mkdir(parents=True, exist_ok=True)
    with open(out_dir / "wordpiece-vocab.txt", "w", encoding="utf-8") as fh:
        for token in tokens:
            if "\n" in token:
                eprint(f"vocabulary token {token!r} contains a newline")
                return 1
            fh.write(token + "\n")

    with open(out_dir / "wordpiece-conformance.jsonl", "w", encoding="utf-8") as fh:
        fh.write(json.dumps({
            "model": model,
            "note": ("Recorded from the model's own HuggingFace tokenizer. "
                     "Regenerate with scripts/encode-sparse.py --record."),
            "pairs": len(cases),
        }, ensure_ascii=False) + "\n")
        for text in cases:
            fh.write(json.dumps({"text": text, "ids": ids_of(text)},
                                ensure_ascii=False) + "\n")

    for name in ("wordpiece-vocab.txt", "wordpiece-conformance.jsonl"):
        eprint(f"  {name}: {(out_dir / name).stat().st_size:,} bytes")
    eprint(f"recorded {len(cases)} cases from {model}")
    return 0


def self_test(dcgrep: str) -> int:
    """Checks everything about this producer that does not need a model.

    Deliberately not a mock of dcgrep: the encoding this writes is handed to
    the real binary, which is the only thing that can say whether the file is
    acceptable. What is faked is the *model* — a hand-written vocabulary and a
    dict of token ids — because a model is what this script exists to avoid
    needing at any other moment.

        python3 scripts/encode-sparse.py --self-test --out /tmp/x.jsonl
    """
    import subprocess
    import tempfile

    failures = []

    def check(name, condition, detail=""):
        if condition:
            print(f"  ok    {name}")
        else:
            print(f"  FAIL  {name}: {detail}")
            failures.append(name)

    # 1. Vocabulary densification. `get_vocab()` is a dict, and a vocabulary
    #    rebuilt in dict order rather than id order would assign every weight
    #    to the wrong token — silently, with every score still plausible.
    class Tok:
        def __init__(self, vocab, name="fake"):
            self._vocab = vocab
            self.name_or_path = name
            self.all_special_ids = []
            self.unk_token_id = 0

        def get_vocab(self):
            # Deliberately shuffled, the way a real one is unordered.
            return dict(sorted(self._vocab.items(), key=lambda kv: kv[0]))

    ordered = ["[UNK]", "parse", "json", "server", "route", "handler", "##json", "gamma"]
    tok = Tok({token: i for i, token in enumerate(ordered)})
    check("vocabulary() restores id order", vocabulary(tok) == ordered, vocabulary(tok))

    # 2. A hole in the id space must not shift every later id.
    holed = Tok({"a": 0, "c": 2})
    dense = vocabulary(holed)
    check("a hole keeps later ids in place",
          len(dense) == 3 and dense[0] == "a" and dense[2] == "c", dense)

    # 3. Special tokens are dropped, but never the unknown token: [UNK] is a
    #    real signal about a document and the other three are not.
    tok.all_special_ids = [0, 5]
    check("special ids drop everything but [UNK]", special_ids(tok) == {5}, special_ids(tok))
    tok.all_special_ids = []

    # 4. The walk. Extensions, skip directories and symlinks.
    with tempfile.TemporaryDirectory() as tmp:
        room = Path(tmp)
        (room / "src").mkdir()
        (room / "node_modules").mkdir()
        (room / ".git").mkdir()
        (room / "src" / "a.rs").write_text("parse json\n")
        (room / "src" / "b.bin").write_text("binary-ish\n")
        (room / "node_modules" / "c.js").write_text("skip me\n")
        (room / ".git" / "d.py").write_text("skip me\n")
        found = {p.relative_to(room).as_posix() for p in walk(room, MAX_DOCUMENTS)}
        check("walk takes source and skips the rest", found == {"src/a.rs"}, found)
        capped = walk(room, 1)
        check("walk honours its own ceiling", len(capped) == 1, capped)

    # 5. End to end against the real binary. The ids below are what dcgrep's
    #    own WordPiece produces for this vocabulary; if this script's header or
    #    document shape were wrong, the build would refuse.
    known = {
        "parse": [1], "json": [2], "parseJson": [1, 6], "nothingatall": [0],
    }
    with tempfile.TemporaryDirectory() as tmp:
        room = Path(tmp)
        repo = room / "repo"
        (repo / "src").mkdir(parents=True)
        (repo / "src" / "alpha.rs").write_text("fn alpha() {}\n")
        (repo / "src" / "beta.rs").write_text("fn beta() {}\n")
        out = room / "sparse.jsonl"

        global PARITY_TEXTS
        saved = PARITY_TEXTS
        PARITY_TEXTS = list(known)
        try:
            header = build_header("self-test", ordered, [0.0, 2.0, 3.0, 1.0, 1.0, 1.0, 2.5, 1.0],
                                  lambda text: known[text])
            files = walk(repo, MAX_DOCUMENTS)
            # alpha outweighs beta on the one query token, and nothing else in
            # the two files distinguishes them.
            weights = {"alpha.rs": 0.9, "beta.rs": 0.1}
            written = write_encoding(
                out, repo, files, header,
                lambda texts: [([(2, weights[name])], 12)
                               for name in [p.name for p in files]],
                batch_size=8,
            )
        finally:
            PARITY_TEXTS = saved
        check("the encoding holds both documents", written == 2, written)

        def run(cmd, req):
            proc = subprocess.run([dcgrep, cmd], input=json.dumps(req).encode(),
                                  capture_output=True)
            try:
                return json.loads(proc.stdout or b"{}")
            except json.JSONDecodeError:
                return {"_unparseable": proc.stdout[:200].decode("utf-8", "replace")}

        built = run("index", {"root": str(repo), "sparse": str(out)})
        check("dcgrep accepts the encoding this script writes",
              built.get("ok") is True, str(built)[:300])
        check("it publishes a learned index",
              built.get("lexical_vocabulary") == "wordpiece-30522", str(built)[:200])
        check("it attributes the weights to this producer",
              built.get("lexical_model") == "self-test", str(built)[:200])
        check("every document was placed",
              built.get("lexical_files") == 2 and not built.get("lexical_unmatched"),
              str(built)[:200])

        ranked = run("rank", {"query": "json", "root": str(repo)})
        paths = [f["path"] for f in ranked.get("files", [])]
        check("the ranking follows the weights this script emitted",
              paths == ["src/alpha.rs", "src/beta.rs"], f"{paths}: {str(ranked)[:200]}")

        # And the gate: a header whose ids this build does not reproduce must
        # refuse. If it did not, nothing would protect a real model run.
        lying = build_header("self-test", ordered, [0.0] * 8, lambda text: [3])
        write_encoding(out, repo, files, lying,
                       lambda texts: [([(2, 0.5)], 12) for _ in texts], batch_size=8)
        refused = run("index", {"root": str(repo), "sparse": str(out)})
        check("a tokenizer disagreement refuses the build",
              refused.get("ok") is False and "disagrees" in str(refused.get("error", "")),
              str(refused)[:200])

    print("")
    print(f"self-test: {'FAILED' if failures else 'passed'}"
          f" ({len(failures)} failure{'' if len(failures) == 1 else 's'})")
    return 1 if failures else 0


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Encode a repository for dcgrep's learned sparse ranking.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__,
    )
    parser.add_argument("--root", default=".", help="repository to encode")
    parser.add_argument("--out", help="JSONL file to write")
    parser.add_argument("--model", default=DEFAULT_MODEL, help=f"default: {DEFAULT_MODEL}")
    parser.add_argument("--batch-size", type=int, default=8)
    parser.add_argument("--max-files", type=int, default=MAX_DOCUMENTS)
    parser.add_argument("--device", default="cpu", help="cpu, cuda, or mps")
    parser.add_argument("--self-test", action="store_true",
                        help="check this script against the real dcgrep, without a model")
    parser.add_argument("--dcgrep", default="dcgrep",
                        help="the binary that supplies the file list, and that --self-test drives")
    parser.add_argument("--walk", action="store_true",
                        help="use this script's own walk instead of asking dcgrep")
    parser.add_argument("--record", action="store_true",
                        help="re-record the Rust tokenizer conformance fixture")
    parser.add_argument("--fixtures", default="rust/dc-grep/tests/fixtures",
                        help="where --record writes")
    args = parser.parse_args()

    if args.self_test:
        return self_test(args.dcgrep)
    if args.record:
        return record(args.model, Path(args.fixtures), Path(args.root).resolve())
    if not args.out:
        eprint("--out is required")
        return 2

    root = Path(args.root).resolve()
    if not root.is_dir():
        eprint(f"{root} is not a directory")
        return 2
    if args.batch_size < 1:
        eprint("--batch-size must be at least 1")
        return 2
    if args.max_files < 1:
        eprint("--max-files must be at least 1")
        return 2

    # Which files to encode is settled before the model is loaded. Both
    # answers that end the run — "the listing is a prefix" and "there is
    # nothing here to encode" — are known without a tokenizer, and finding
    # them out after a multi-hundred-megabyte download is a worse way to learn
    # them.
    listed = None
    if not args.walk:
        listed = files_from_dcgrep(args.dcgrep, root, args.max_files)
    if listed is None:
        eprint("falling back to this script's own walk, which does not read "
               ".gitignore; expect lexical_unmatched to be non-zero.")
        files = walk(root, args.max_files)
    else:
        files, complete = listed
        if not complete:
            # A prefix is not the corpus. The files past the cut reach the
            # index through the walk and rank by BM25, inside a build that
            # reports itself as learned — visible only as lexical_unindexed,
            # which nobody reads when the build says ok.
            #
            # Refused rather than warned, unless the operator chose the bound
            # themselves: the default is "encode this repository", and a
            # partial answer to that is wrong rather than smaller.
            asked = args.max_files
            if asked == parser.get_default("max_files"):
                eprint(f"{args.dcgrep} truncated the file list at {asked}. "
                       "This encoding would cover a prefix of the repository "
                       "and leave the rest ranking by BM25. Raise the "
                       "searcher's MAX_LIST_RESULTS, or pass --max-files "
                       "explicitly to encode a prefix on purpose.")
                return 1
            eprint(f"warning: encoding a prefix — the file list was truncated "
                   f"at the --max-files you passed ({asked}). Files past it "
                   "will be searchable but ranked by BM25, and the build will "
                   "count them in lexical_unindexed.")
    if not files:
        eprint(f"no encodable files under {root}")
        return 1

    torch, tokenizer, model = load_model(args.model, args.device)
    tokens = vocabulary(tokenizer)
    weights = query_weights(tokenizer, len(tokens))
    drop = special_ids(tokenizer)

    eprint(f"encoding {len(files)} files with {args.model} on {args.device}")

    header = build_header(
        args.model, tokens, weights,
        lambda text: tokenizer(text, add_special_tokens=False)["input_ids"],
    )
    out = Path(args.out)
    written = write_encoding(
        out, root, files, header,
        lambda texts: encode(torch, tokenizer, model, args.device, texts, drop),
        args.batch_size,
    )

    eprint(f"wrote {written} documents to {out}")
    eprint("")
    eprint("Build the index with it:")
    eprint(f"    echo '{json.dumps({'root': str(root), 'sparse': str(out)})}' | dcgrep index")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
