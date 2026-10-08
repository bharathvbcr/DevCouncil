#!/usr/bin/env python3
"""Records the torch fp32 oracle the Rust sparse encoder is judged against.

This is a reference oracle, not a shipped path: nothing runs it but a person
re-recording the fixtures beside it, and `cargo test` reads only what it
wrote. It replaces nothing at run time. The encoder it describes used to be
scripts/encode-sparse.py; this file keeps exactly that script's forward pass
and post-processing (`encode()` there), and was checked once to produce the
same JSONL bytes as the script over this corpus before the script was
deleted.

    uv venv /tmp/oracle && uv pip install --python /tmp/oracle torch transformers numpy
    /tmp/oracle/bin/python -I rust/dc-sparse-encode/tests/fixtures/record.py

It reads the model from the Hugging Face cache and never downloads: a fixture
recorded from whatever a network happened to serve is not a fixture.

What it writes, under doc-v2-mini/:

* `<doc>.ids.npy` (int64): the model tokenizer's ids with [CLS]/[SEP],
  truncated at 512 — the tokenization the Rust side must reproduce exactly.
* `<doc>.hidden.npy` (f32, [7, P, 384]) and `<doc>.positions.npy` (int64,
  [P]): the embedding output and the residual stream after each of the six
  layers, at P positions. Every position for documents of up to 128 tokens; for
  the long ones a fixed spread (first, last, middle, both sides of 256),
  because every row of a 512-token document is 5.5 MB of fixture.
* `<doc>.pooled.npy` (f32, [30522]): max over tokens of
  log1p(relu(logits)) * mask, before rounding — what the 1e-4 bound is on.
* `batch.pooled.npy` (f32, [docs, 30522]): the same documents encoded as one
  right-padded batch, which is how the script ran them.
* `expected.jsonl`: the encoding the script wrote for corpus/, header and
  all, which is what `dcgrep index --sparse` imports.

The corpus is corpus/src/*; `exact512.txt` is generated here so that it is
510 content tokens, i.e. exactly 512 with [CLS] and [SEP].
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

import numpy as np
import torch
from transformers import AutoModelForMaskedLM, AutoTokenizer

MODEL = "opensearch-project/opensearch-neural-sparse-encoding-doc-v2-mini"
MAX_TERMS_PER_DOCUMENT = 20_000
HERE = Path(__file__).resolve().parent
CORPUS = HERE / "corpus"
OUT = HERE / "doc-v2-mini"
SPREAD = [0, 1, 2, 127, 255, 256, 383, 509, 510, 511]

# The script's parity texts, so the header recorded here is the one it wrote.
PARITY_TEXTS = [
    "parse", "json", "parseJson", "parseJSONResponse", "HTTPServer",
    "http_server", "server2", "v2", "read_file", "readFile", "FILE",
    "unwrap_or_default", "tokenize", "zzqqxx", "sha256sum", "utf8",
    "supercalifragilisticexpialidocious",
    "ß", "ẞ", "ø", "Ø", "æ", "Æ", "ð", "þ", "đ", "ı", "ł", "Łódź", "œ",
    "Böse", "naïve", "café", "École", "Ångström", "žluťoučký",
    "Việt", "Tiếng", "ﬁle",
    "école", "ñino",
    "İstanbul",
    "日本語", "中文字符", "한국어", "снег", "Ελληνικά", "עברית",
    "العربية", "ไทย", "हिन्दी",
    "a​b", "a﻿b", "a b", "a\u0000b", "a\u000bb",
    "🎉", "a🎉b", "→", "±", "§", "§8", "audit §e", "€", "℃",
    "a.b.c", "foo::bar", "x-=+", "don't", "e.g.", "U.S.A.",
    "path/to/file.rs", "https://example.com/a?b=c", "user@example.com",
    "#[derive(Debug)]", "{\"k\":[1,2]}", "// line", "全角。句点",
    "", " ", "   ", "\t\n", "  spaced  out  ",
    "a" * 99, "a" * 100, "a" * 101,
]


def eprint(*args: object) -> None:
    print(*args, file=sys.stderr)


def vocabulary(tokenizer) -> list[str]:
    raw = tokenizer.get_vocab()
    tokens: list[str | None] = [None] * (max(raw.values()) + 1)
    for token, index in raw.items():
        tokens[index] = token
    return [t if t is not None else f"[unused_hole_{i}]" for i, t in enumerate(tokens)]


def query_weights(tokenizer, size: int) -> list[float]:
    from huggingface_hub import hf_hub_download

    path = hf_hub_download(MODEL, "idf.json", local_files_only=True)
    with open(path, encoding="utf-8") as handle:
        table = json.load(handle)
    vocab = tokenizer.get_vocab()
    weights = [0.0] * size
    for token, weight in table.items():
        index = vocab.get(token)
        if index is not None and index < size:
            weights[index] = float(weight)
    return weights


def exact512(tokenizer, source: str) -> str:
    """The longest line-prefix of `source` that is 510 content tokens, cut
    inside its last line at a space so the count lands exactly."""
    def count(text: str) -> int:
        return len(tokenizer(text, add_special_tokens=False)["input_ids"])

    words = source.split(" ")
    lo, hi = 0, len(words)
    while lo < hi:
        mid = (lo + hi + 1) // 2
        if count(" ".join(words[:mid])) <= 510:
            lo = mid
        else:
            hi = mid - 1
    text = " ".join(words[:lo])
    have = count(text)
    if have != 510:
        raise SystemExit(f"no word boundary gives 510 tokens (closest {have}); change the source")
    return text


def pooled_of(model, encoded) -> tuple[torch.Tensor, tuple[torch.Tensor, ...]]:
    with torch.no_grad():
        out = model(**encoded, output_hidden_states=True)
    activated = torch.log1p(torch.relu(out.logits))
    mask = encoded["attention_mask"].unsqueeze(-1)
    return torch.max(activated * mask, dim=1).values, out.hidden_states


def terms_of(row: torch.Tensor, drop: set[int]) -> list[list[float]]:
    """encode-sparse.py's post-processing, line for line."""
    nonzero = torch.nonzero(row, as_tuple=False).flatten()
    pairs = [(int(i), round(float(row[i]), 4)) for i in nonzero.tolist() if int(i) not in drop]
    pairs = [(i, w) for i, w in pairs if w > 0.0]
    if len(pairs) > MAX_TERMS_PER_DOCUMENT:
        pairs.sort(key=lambda pair: pair[1], reverse=True)
        pairs = pairs[:MAX_TERMS_PER_DOCUMENT]
    pairs.sort(key=lambda pair: pair[0])
    return [[i, w] for i, w in pairs]


def main() -> int:
    torch.manual_seed(0)
    tokenizer = AutoTokenizer.from_pretrained(MODEL, local_files_only=True)
    model = AutoModelForMaskedLM.from_pretrained(MODEL, local_files_only=True, dtype=torch.float32)
    model.eval()

    src = CORPUS / "src"
    long_text = (src / "long.rs").read_text(encoding="utf-8")
    (src / "exact512.txt").write_text(exact512(tokenizer, long_text), encoding="utf-8")

    # Sorted, as the script's walk was, so expected.jsonl is in its order.
    files = sorted(src.iterdir())
    names = [p.name for p in files]
    texts = [p.read_text(encoding="utf-8") for p in files]
    OUT.mkdir(parents=True, exist_ok=True)

    drop = set(tokenizer.all_special_ids or [])
    drop.discard(tokenizer.unk_token_id)

    for name, text in zip(names, texts):
        encoded = tokenizer([text], padding=True, truncation=True, max_length=512, return_tensors="pt")
        ids = encoded["input_ids"][0].numpy().astype(np.int64)
        pooled, hidden = pooled_of(model, encoded)
        n = ids.shape[0]
        positions = np.arange(n) if n <= 128 else np.array([p for p in SPREAD if p < n], dtype=np.int64)
        stack = torch.stack([h[0] for h in hidden]).numpy()[:, positions, :].astype(np.float32)
        np.save(OUT / f"{name}.ids.npy", ids)
        np.save(OUT / f"{name}.positions.npy", positions.astype(np.int64))
        np.save(OUT / f"{name}.hidden.npy", stack)
        np.save(OUT / f"{name}.pooled.npy", pooled[0].numpy().astype(np.float32))
        eprint(f"  {name}: {n} tokens, {len(positions)} recorded positions")

    encoded = tokenizer(texts, padding=True, truncation=True, max_length=512, return_tensors="pt")
    pooled, _ = pooled_of(model, encoded)
    np.save(OUT / "batch.pooled.npy", pooled.numpy().astype(np.float32))
    lengths = encoded["attention_mask"].sum(dim=1).tolist()

    tokens = vocabulary(tokenizer)
    header = {
        "schema": 1,
        "model": MODEL,
        "vocabulary": "wordpiece-30522",
        "vocab": tokens,
        "query_weights": query_weights(tokenizer, len(tokens)),
        "parity": [{"text": t, "ids": tokenizer(t, add_special_tokens=False)["input_ids"]} for t in PARITY_TEXTS],
    }
    with open(OUT / "expected.jsonl", "w", encoding="utf-8") as handle:
        handle.write(json.dumps(header, ensure_ascii=False) + "\n")
        for name, row, length in zip(names, pooled, lengths):
            terms = terms_of(row, drop)
            if not terms:
                continue
            handle.write(json.dumps({"path": f"src/{name}", "total_terms": int(length), "terms": terms},
                                    ensure_ascii=False) + "\n")
    (OUT / "documents.json").write_text(json.dumps(names) + "\n", encoding="utf-8")
    eprint(f"recorded {len(names)} documents from {MODEL} (torch {torch.__version__})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
