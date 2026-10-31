"""Record the sentence-transformers reference for tests/dense_parity.rs.

Reads all-MiniLM-L6-v2 from the local Hugging Face cache (HF_HUB_OFFLINE=1,
nothing is downloaded) and writes fixture.json: for each text, the token ids
the reference tokenizer produced (with [CLS]/[SEP], truncated at the model's
max_seq_length of 256) and the L2-normalised sentence embedding.

    uv venv /tmp/oracle && uv pip install --python /tmp/oracle torch sentence-transformers numpy
    HF_HUB_OFFLINE=1 /tmp/oracle/bin/python -I record.py
"""
import json
import os
import pathlib

os.environ["HF_HUB_OFFLINE"] = "1"

from sentence_transformers import SentenceTransformer  # noqa: E402

TEXTS = [
    "how does the tool stop two agents from working on the same task at once",
    "refuse a repo-local git in the host's two git runners",
    "pub fn parse_unified(diff: &str) -> Result<Vec<FileDiff>, String> {\n    let mut files = Vec::new();\n    for line in diff.lines() {\n        if let Some(rest) = line.strip_prefix(\"+++ \") { files.push(rest.to_string()); }\n    }\n    Ok(files)\n}",
    "Naïve café: parseJSONResponse handles ÉCOLE and straße without crashing.",
    "Evidence, not model confidence, is the final authority. " * 40,
    "single",
]

model = SentenceTransformer("sentence-transformers/all-MiniLM-L6-v2", device="cpu")
assert model.max_seq_length == 256, model.max_seq_length
tok = model.tokenizer
rows = []
for text in TEXTS:
    ids = tok(text, truncation=True, max_length=model.max_seq_length)["input_ids"]
    emb = model.encode([text], normalize_embeddings=True, convert_to_numpy=True)[0]
    rows.append({"text": text, "ids": ids, "embedding": [float(x) for x in emb]})
out = pathlib.Path(__file__).with_name("fixture.json")
out.write_text(json.dumps({"model": "sentence-transformers/all-MiniLM-L6-v2", "rows": rows}))
print(f"wrote {out} with {len(rows)} rows; token counts {[len(r['ids']) for r in rows]}")
