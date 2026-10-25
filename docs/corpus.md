# Corpus side index (Retired)

The Python `dev corpus` prototype was deleted in Phase 7. There is no `dev corpus` command on the Go host (unknown command, exit 2). There is no `corpus_stale` / `acceptance_corpus` check on Go `verify.Run()`.

Repository search and concept exploration are `devmap search` (and Manvi `devcouncil_grep`). See [PHASE7_LONG_TAIL.md](https://github.com/bharathvbcr/DevCouncil/blob/b18cea3f23d2df9b3ba9d995a8d7b183aafa923e/docs/PHASE7_LONG_TAIL.md).

Do not enable `indexing.corpus` in `.devcouncil/config.yaml` expecting a builder to appear. Leftover `.devcouncil/graphify.yaml` is inert.
