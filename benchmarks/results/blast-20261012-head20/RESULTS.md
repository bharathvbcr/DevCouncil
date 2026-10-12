# blast / suspects A/B, `--since HEAD~20`, 2026-10-12

Arm a = `cded4595^` (pre-batching), arm b = lane/blast2 (cded4595 + d78cd9de). Release builds,
`benchmarks/blast_ab.py --rounds 8`, ABBA order, frozen clone of this repo (HEAD~20 = 206 changed files).
**Machine load average was 80-118 throughout (other agents running); wall times are inflated and noisy,
spawn counts are not.** The harness stopped after round 7 of 8 with "the arms disagree on the answer":
arm a reports 201 changed files, arm b 206 (inferred, not verified: arm a drops paths git quotes, the bug the
`-z` change fixed). Records are the raw per-sample lines; no summary.json was written.

| command | arm | n | min wall s | median wall s | git spawns (min-max) |
|---|---|---|---|---|---|
| blast | a | 8 | 77.83 | 87.50 | 1118-1161 |
| blast | b | 8 | 39.19 | 65.53 | 59-129 |
| suspects | a | 8 | 1.07 | 1.67 | 18-18 |
| suspects | b | 8 | 0.78 | 1.00 | 7-7 |
