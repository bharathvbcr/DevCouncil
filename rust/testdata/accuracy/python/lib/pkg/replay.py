"""Added for the dead-policy card: the corpus's one confident true positive.

`replay_texts_from_prompts` is referenced nowhere; it is the shape of
`qd_train/replay.py::replay_texts_from_prompts`, which the 2026-10-09 audit
confirmed dead with `rg -uu`.
"""


def replay_texts_from_prompts(prompts):
    return [p.strip() for p in prompts]
