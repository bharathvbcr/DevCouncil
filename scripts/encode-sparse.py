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
# agrees. Chosen for the decisions that actually differ between
# implementations: camelCase, digits, a compound that must split, punctuation,
# an accent, CJK, and a word no vocabulary has.
PARITY_TEXTS = [
    "parse", "json", "parseJson", "parseJSONResponse", "HTTPServer",
    "http_server", "server2", "v2", "read_file", "readFile", "FILE",
    "unwrap_or_default", "Böse", "naïve", "日本語", "снег",
    "a.b.c", "foo::bar", "x-=+", "  spaced  out  ", "",
    "supercalifragilisticexpialidocious", "zzqqxx", "tokenize",
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


def walk(root: Path, max_files: int) -> list[Path]:
    """Files worth encoding, in a stable order.

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
    parser.add_argument("--dcgrep", default="dcgrep", help="the binary --self-test drives")
    args = parser.parse_args()

    if args.self_test:
        return self_test(args.dcgrep)
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

    torch, tokenizer, model = load_model(args.model, args.device)
    tokens = vocabulary(tokenizer)
    weights = query_weights(tokenizer, len(tokens))
    drop = special_ids(tokenizer)

    files = walk(root, args.max_files)
    if not files:
        eprint(f"no encodable files under {root}")
        return 1
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
