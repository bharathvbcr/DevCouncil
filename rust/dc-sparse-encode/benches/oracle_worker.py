#!/usr/bin/env python3
"""Torch oracle for `dc-sparse-encode --bench`.

Not a shipped encoder. The process is started once, loads the model from the
local Hugging Face cache, and times the forward. Startup is not part of the
measurement. Torch stays on CPU. Putting it on MPS would time it against
tessl on the same GPU.

Protocol, after one `READY` line:

    ENCODE <nbytes>\\n
    <nbytes of utf-8, documents separated by NUL>
    OK <seconds> <checksum>

`QUIT` ends the process. A missing cache is an error. This never downloads.
"""

from __future__ import annotations

import sys
import time

import torch
from transformers import AutoModelForMaskedLM, AutoTokenizer


def read_exact(n: int) -> bytes:
    buf = bytearray()
    while len(buf) < n:
        chunk = sys.stdin.buffer.read(n - len(buf))
        if not chunk:
            raise SystemExit(0)
        buf += chunk
    return bytes(buf)


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: oracle_worker.py MODEL_ID", file=sys.stderr)
        return 2
    model_id = sys.argv[1]
    tokenizer = AutoTokenizer.from_pretrained(model_id, local_files_only=True)
    model = AutoModelForMaskedLM.from_pretrained(
        model_id, local_files_only=True, dtype=torch.float32
    )
    model.eval()
    model.to("cpu")
    print("READY", flush=True)

    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return 0
        text = line.decode("utf-8").strip()
        if text == "QUIT" or text == "":
            return 0
        if not text.startswith("ENCODE "):
            print(f"expected ENCODE, got {text!r}", file=sys.stderr)
            return 2
        nbytes = int(text.split()[1])
        blob = read_exact(nbytes).decode("utf-8")
        documents = blob.split("\0") if blob else []
        started = time.perf_counter()
        encoded = tokenizer(
            documents,
            padding=True,
            truncation=True,
            max_length=512,
            return_tensors="pt",
        )
        with torch.no_grad():
            logits = model(**encoded).logits
        activated = torch.log1p(torch.relu(logits))
        mask = encoded["attention_mask"].unsqueeze(-1)
        pooled = torch.max(activated * mask, dim=1).values
        checksum = float(pooled.sum())
        elapsed = time.perf_counter() - started
        print(f"OK {elapsed:.9f} {checksum:.8f}", flush=True)


if __name__ == "__main__":
    raise SystemExit(main())
