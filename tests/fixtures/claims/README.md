# Claim fixtures

Labeled regression corpus for the claim-to-check mapper. Each `*.jsonl` file is
auto-loaded by `tests/test_claim_fixtures.py`.

One JSON object per line:

```json
{"text": "<agent completion message>", "expected": [{"kind": "tests_pass", "target": null}], "note": "optional"}
```

- `expected` is the exact set of assertions the mapper should extract
  (`kind` + `target`); order doesn't matter, `[]` means "extract nothing".
- Generate new batches with the `false-claim-generator` project agent.
- A case whose label intentionally documents a known mapper gap should set
  `"xfail": true` — it is reported but doesn't fail the suite.
