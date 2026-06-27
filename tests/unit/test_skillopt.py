"""Tests for the SkillOpt co-optimization loop (devcouncil.optimization.skillopt)."""

from __future__ import annotations

import asyncio
import json

import pytest

from devcouncil.optimization.skillopt import (
    GUIDANCE,
    SKILL,
    SkillEdit,
    SkillEditProposal,
    SkillOptConfig,
    _edit_signature,
    _is_permanently_redundant,
    _split_dataset,
    apply_edits,
    default_score,
    make_llm_optimizer,
    optimize_skill,
)


# ---------------------------------------------------------------------------
# apply_edits
# ---------------------------------------------------------------------------


def test_apply_edits_routes_to_targets_and_skips_noops():
    docs = {GUIDANCE: "Stay in scope.", SKILL: "Write tests."}
    edits = [
        SkillEdit(op="add", target=SKILL, find="", text="Always verify."),
        SkillEdit(op="replace", target=GUIDANCE, find="Stay in scope.", text="Stay strictly in scope."),
        SkillEdit(op="delete", target=SKILL, find="nonexistent anchor"),  # skipped
    ]
    updated, applied = apply_edits(docs, edits)
    assert len(applied) == 2
    assert "Always verify." in updated[SKILL]
    assert updated[GUIDANCE] == "Stay strictly in scope."
    # original dict is not mutated
    assert docs[SKILL] == "Write tests."


def test_apply_edits_skips_edit_for_unknown_target():
    docs = {SKILL: "body"}
    updated, applied = apply_edits(docs, [SkillEdit(op="add", target=GUIDANCE, text="x")])
    assert applied == []
    assert updated == docs


def test_edit_signature_is_target_sensitive():
    a = SkillEdit(op="add", target=SKILL, text="x")
    b = SkillEdit(op="add", target=GUIDANCE, text="x")
    assert _edit_signature(a) != _edit_signature(b)


def test_edit_signature_no_field_boundary_collision():
    # find/text boundaries must be unambiguous: 'a' + 'b c' must not collide with 'a b' + 'c'.
    a = SkillEdit(op="replace", target=SKILL, find="a", text="b c")
    b = SkillEdit(op="replace", target=SKILL, find="a b", text="c")
    assert _edit_signature(a) != _edit_signature(b)


def test_permanently_redundant_only_for_unfixable_noops():
    docs = {SKILL: "hello world"}
    # empty append + identity replace can never apply -> permanently redundant
    assert _is_permanently_redundant(SkillEdit(op="add", target=SKILL, text="\n"), docs)
    assert _is_permanently_redundant(SkillEdit(op="replace", target=SKILL, find="hello", text="hello"), docs)
    # an edit whose anchor is merely absent now may apply later -> NOT permanent
    assert not _is_permanently_redundant(SkillEdit(op="delete", target=SKILL, find="absent"), docs)
    assert not _is_permanently_redundant(SkillEdit(op="replace", target=SKILL, find="absent", text="x"), docs)


# ---------------------------------------------------------------------------
# scoring + split
# ---------------------------------------------------------------------------


def test_default_score_term_coverage():
    task = {"required_terms": ["verify", "tests"], "forbidden_terms": ["skip"]}
    # full required coverage, no forbidden term -> perfect
    assert default_score(task, "we verify and run tests") == pytest.approx(1.0)
    # half the required terms present -> partial credit, strictly below full
    partial = default_score(task, "we verify only")
    assert 0.0 < partial < 1.0
    # a forbidden term drags the score down
    assert default_score(task, "we verify and run tests but skip review") < 1.0
    assert default_score(task, "") == 0.0
    # no signal -> neutral
    assert default_score({}, "anything") == 0.5


def test_split_dataset_is_deterministic_and_nonempty():
    dataset = [{"id": f"t{i}"} for i in range(10)]
    train1, val1 = _split_dataset(dataset, 0.5, seed=0)
    train2, val2 = _split_dataset(dataset, 0.5, seed=0)
    assert [t["id"] for t in train1] == [t["id"] for t in train2]
    assert [v["id"] for v in val1] == [v["id"] for v in val2]
    assert train1 and val1
    # different seed -> (very likely) different split
    train3, _ = _split_dataset(dataset, 0.5, seed=99)
    assert isinstance(train3, list)


def test_split_dataset_no_leakage_at_extreme_fraction():
    # With val_fraction=1.0 every item hashes into val; the fallback must MOVE one item
    # into train (not copy it), so the two splits never share an example.
    dataset = [{"id": f"t{i}"} for i in range(3)]
    train, val = _split_dataset(dataset, 1.0, seed=0)
    assert train and val
    train_ids = {t["id"] for t in train}
    val_ids = {v["id"] for v in val}
    assert train_ids.isdisjoint(val_ids)
    assert train_ids | val_ids == {"t0", "t1", "t2"}


# ---------------------------------------------------------------------------
# the loop (injected rollout + optimizer, no live model)
# ---------------------------------------------------------------------------


def _compose_rollout(docs, task):
    """Deterministic rollout: the trajectory is just the combined document text, so a
    score change is fully attributable to an edit."""
    async def _inner():
        return f"{docs.get(GUIDANCE, '')}\n{docs.get(SKILL, '')}"

    return _inner()


def _queued_optimizer(proposals):
    """An optimizer that returns each queued proposal in turn (last one repeats)."""
    calls = {"n": 0}

    async def optimizer(docs, feedback, rejected, skill_name):
        idx = min(calls["n"], len(proposals) - 1)
        calls["n"] += 1
        return proposals[idx]

    return optimizer


def test_loop_accepts_edit_that_improves_validation():
    dataset = [{"id": "t1", "prompt": "do x", "required_terms": ["verify"]}]
    proposal = SkillEditProposal(edits=[SkillEdit(op="add", target=SKILL, text="Always verify your work.")])
    result = asyncio.run(
        optimize_skill(
            skill_name="demo",
            docs={GUIDANCE: "", SKILL: "Write code."},
            dataset=dataset,
            rollout=_compose_rollout,
            optimizer=_queued_optimizer([proposal]),
            config=SkillOptConfig(epochs=3),
        )
    )
    assert result.improved
    assert result.seed_val_score == 0.0
    assert result.best_val_score == 1.0
    assert "verify" in result.best_skill_body.lower()
    assert result.accepted_edit_count == 1


def test_loop_rejects_unhelpful_edit_and_remembers_it():
    dataset = [{"id": "t1", "prompt": "do x", "required_terms": ["verify"]}]
    # An edit that doesn't add the required term -> no validation gain -> rejected.
    useless = SkillEditProposal(edits=[SkillEdit(op="add", target=SKILL, text="Some unrelated note.")])
    result = asyncio.run(
        optimize_skill(
            skill_name="demo",
            docs={GUIDANCE: "", SKILL: "Write code."},
            dataset=dataset,
            rollout=_compose_rollout,
            optimizer=_queued_optimizer([useless]),
            config=SkillOptConfig(epochs=3),
        )
    )
    assert not result.improved
    assert result.best_skill_body == "Write code."
    assert result.rejected_edit_count == 1
    # After the first rejection the edit is buffered; later epochs filter it to a no-op.
    assert any(e.note == "no-op proposal" for e in result.epochs)


def test_loop_co_optimizes_guidance_and_skill_simultaneously():
    # One task requiring a term that lives in guidance AND a term that lives in skill.
    dataset = [{"id": "t1", "prompt": "do x", "required_terms": ["alpha", "beta"]}]
    proposal = SkillEditProposal(
        edits=[
            SkillEdit(op="add", target=GUIDANCE, text="alpha guidance."),
            SkillEdit(op="add", target=SKILL, text="beta skill."),
        ]
    )
    result = asyncio.run(
        optimize_skill(
            skill_name="demo",
            docs={GUIDANCE: "Be helpful.", SKILL: "Write code."},
            dataset=dataset,
            rollout=_compose_rollout,
            optimizer=_queued_optimizer([proposal]),
            config=SkillOptConfig(epochs=2),
        )
    )
    assert result.improved
    assert result.best_val_score == 1.0
    assert "alpha" in result.best_guidance_body.lower()
    assert "beta" in result.best_skill_body.lower()
    accepted_epoch = next(e for e in result.epochs if e.accepted)
    assert accepted_epoch.edited_targets == [GUIDANCE, SKILL]


def test_train_eval_is_cached_across_stalled_epochs():
    # best_docs only changes on acceptance, so stalled (rejected/no-op) epochs must NOT
    # re-run the train rollouts. Each train task should be rolled out exactly once.
    dataset = [{"id": f"t{i}", "prompt": "p", "required_terms": ["verify"]} for i in range(4)]
    train, _ = _split_dataset(dataset, 0.5, seed=0)
    train_ids = {t["id"] for t in train}
    calls: list[str] = []

    def counting_rollout(docs, task):
        async def _inner():
            calls.append(task["id"])
            return f"{docs.get(GUIDANCE, '')}\n{docs.get(SKILL, '')}"

        return _inner()

    useless = SkillEditProposal(edits=[SkillEdit(op="add", target=SKILL, text="unrelated note")])
    result = asyncio.run(
        optimize_skill(
            skill_name="demo",
            docs={GUIDANCE: "", SKILL: "Write code."},
            dataset=dataset,
            rollout=counting_rollout,
            optimizer=_queued_optimizer([useless]),
            config=SkillOptConfig(epochs=3, val_fraction=0.5, seed=0),
        )
    )
    assert not result.improved
    for tid in train_ids:
        assert calls.count(tid) == 1, f"train task {tid} re-evaluated on a stalled epoch"


def test_train_eval_recomputed_after_acceptance():
    # After an accepted edit changes best_docs, the train cache must be invalidated so the
    # next epoch re-evaluates against the new docs.
    dataset = [{"id": f"t{i}", "prompt": "p", "required_terms": ["alpha", "beta"]} for i in range(4)]
    train, _ = _split_dataset(dataset, 0.5, seed=0)
    train_ids = {t["id"] for t in train}
    calls: list[str] = []

    def counting_rollout(docs, task):
        async def _inner():
            calls.append(task["id"])
            return f"{docs.get(GUIDANCE, '')}\n{docs.get(SKILL, '')}"

        return _inner()

    # Epoch 1 adds "alpha" (val 0 -> 0.5, accepted but not perfect); epoch 2 must recompute
    # train on the new docs, then adds "beta" to reach 1.0.
    prop_alpha = SkillEditProposal(edits=[SkillEdit(op="add", target=SKILL, text="alpha note")])
    prop_beta = SkillEditProposal(edits=[SkillEdit(op="add", target=SKILL, text="beta note")])
    result = asyncio.run(
        optimize_skill(
            skill_name="demo",
            docs={GUIDANCE: "", SKILL: "Write code."},
            dataset=dataset,
            rollout=counting_rollout,
            optimizer=_queued_optimizer([prop_alpha, prop_beta]),
            config=SkillOptConfig(epochs=4, val_fraction=0.5, seed=0),
        )
    )
    assert result.improved
    assert result.best_val_score == 1.0
    # train evaluated twice: once in epoch 1, once after epoch-1 acceptance invalidated the cache.
    for tid in train_ids:
        assert calls.count(tid) == 2


def test_loop_enforces_edit_budget_per_epoch():
    dataset = [{"id": "t1", "prompt": "do x", "required_terms": ["verify"]}]
    # Five candidate edits but a budget of two: only two may land in the epoch.
    proposal = SkillEditProposal(
        edits=[SkillEdit(op="add", target=SKILL, text=f"note {i} verify") for i in range(5)]
    )
    result = asyncio.run(
        optimize_skill(
            skill_name="demo",
            docs={GUIDANCE: "", SKILL: "Write code."},
            dataset=dataset,
            rollout=_compose_rollout,
            optimizer=_queued_optimizer([proposal]),
            config=SkillOptConfig(epochs=1, max_edits_per_epoch=2),
        )
    )
    assert result.epochs[0].applied_edits == 2


# ---------------------------------------------------------------------------
# router-backed optimizer (MockProvider) parses a real proposal
# ---------------------------------------------------------------------------


def test_make_llm_optimizer_parses_proposal_via_router(tmp_path):
    from devcouncil.llm.provider import MockProvider
    from devcouncil.llm.router import ModelRouter

    proposal_json = json.dumps(
        {
            "edits": [{"op": "add", "target": "skill", "find": "", "text": "Always verify.", "reason": "r"}],
            "rationale": "add verification",
        }
    )
    provider = MockProvider({"opt": proposal_json})
    router = ModelRouter(provider, {"skill_optimizer": {"model": "opt", "temperature": 0.0}}, project_root=tmp_path)
    optimizer = make_llm_optimizer(router)

    dataset = [{"id": "t1", "prompt": "do x", "required_terms": ["verify"]}]
    result = asyncio.run(
        optimize_skill(
            skill_name="demo",
            docs={GUIDANCE: "", SKILL: "Write code."},
            dataset=dataset,
            rollout=_compose_rollout,
            optimizer=optimizer,
            config=SkillOptConfig(epochs=2),
        )
    )
    assert result.improved
    assert "verify" in result.best_skill_body.lower()


def test_rejected_multi_edit_batch_keeps_good_edit_eligible():
    # A good edit dragged below the gate by a bad partner must NOT be banned forever:
    # next epoch it should be accepted on its own. Custom scorer: +1 for "good", -1 for "bad".
    def score(task, traj):
        return (1.0 if "good" in traj else 0.0) - (1.0 if "bad" in traj else 0.0)

    dataset = [{"id": "t1", "prompt": "p"}]
    batch = SkillEditProposal(edits=[
        SkillEdit(op="add", target=SKILL, text="good"),
        SkillEdit(op="add", target=SKILL, text="bad"),
    ])
    good_only = SkillEditProposal(edits=[SkillEdit(op="add", target=SKILL, text="good")])
    result = asyncio.run(
        optimize_skill(
            skill_name="demo",
            docs={GUIDANCE: "", SKILL: "base"},
            dataset=dataset,
            rollout=_compose_rollout,
            optimizer=_queued_optimizer([batch, good_only]),
            score=score,
            config=SkillOptConfig(epochs=4, max_edits_per_epoch=3),
        )
    )
    assert result.improved
    assert "good" in result.best_skill_body
    assert "bad" not in result.best_skill_body  # the bad partner never lands


def test_noop_patience_stops_a_stuck_optimizer():
    dataset = [{"id": "t1", "prompt": "p", "required_terms": ["verify"]}]
    # An edit whose anchor never exists -> always a no-op; optimizer keeps proposing it.
    stuck = SkillEditProposal(edits=[SkillEdit(op="replace", target=SKILL, find="ABSENT", text="x verify")])
    result = asyncio.run(
        optimize_skill(
            skill_name="demo",
            docs={GUIDANCE: "", SKILL: "base"},
            dataset=dataset,
            rollout=_compose_rollout,
            optimizer=_queued_optimizer([stuck]),
            config=SkillOptConfig(epochs=10, noop_patience=2),
        )
    )
    assert not result.improved
    # Stops after noop_patience consecutive no-ops, not after all 10 epochs.
    assert len(result.epochs) == 2
    assert all(e.note == "no-op proposal" for e in result.epochs)


def test_rollout_exception_degrades_to_zero_not_crash():
    dataset = [
        {"id": "t1", "prompt": "p", "required_terms": ["verify"]},
        {"id": "t2", "prompt": "p", "required_terms": ["verify"]},
    ]

    def flaky_rollout(docs, task):
        async def _inner():
            if task["id"] == "t2":
                raise RuntimeError("boom")
            return f"{docs.get(GUIDANCE, '')}\n{docs.get(SKILL, '')}"

        return _inner()

    proposal = SkillEditProposal(edits=[SkillEdit(op="add", target=SKILL, text="verify")])
    # Must not raise despite t2 always failing.
    result = asyncio.run(
        optimize_skill(
            skill_name="demo",
            docs={GUIDANCE: "", SKILL: "base"},
            dataset=dataset,
            rollout=flaky_rollout,
            optimizer=_queued_optimizer([proposal]),
            config=SkillOptConfig(epochs=2, val_fraction=0.5, seed=0),
        )
    )
    assert isinstance(result.best_val_score, float)


def test_scores_are_clamped_to_unit_interval():
    dataset = [{"id": "t1", "prompt": "p"}]
    # A custom scorer that returns out-of-[0,1] values must not break the loop: >1 clamps to
    # 1.0 (treated as perfect -> early stop), <0 clamps to 0.0.
    high = asyncio.run(
        optimize_skill(
            skill_name="d", docs={GUIDANCE: "", SKILL: "b"}, dataset=dataset,
            rollout=_compose_rollout, optimizer=_queued_optimizer([SkillEditProposal(edits=[])]),
            score=lambda task, traj: 5.0, config=SkillOptConfig(epochs=2),
        )
    )
    assert high.seed_val_score == 1.0
    assert 0.0 <= high.best_val_score <= 1.0

    low = asyncio.run(
        optimize_skill(
            skill_name="d", docs={GUIDANCE: "", SKILL: "b"}, dataset=dataset,
            rollout=_compose_rollout,
            optimizer=_queued_optimizer([SkillEditProposal(edits=[SkillEdit(op="add", target=SKILL, text="z")])]),
            score=lambda task, traj: -3.0, config=SkillOptConfig(epochs=1),
        )
    )
    assert low.seed_val_score == 0.0
