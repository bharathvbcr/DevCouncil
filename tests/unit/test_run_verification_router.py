"""The coding-CLI executor path must build an LLM-review router so the
implementation_reviewer gate (optionally on a local provider) engages during
execution — not just for the native agent. Regression guard for the bug where
CLI agents (claude/codex/…) verified deterministically with no LLM review.
"""

from pathlib import Path


from devcouncil.cli.commands.run import _build_verification_router
from devcouncil.llm.provider import OllamaProvider, OpenRouterProvider


def _write_hybrid_config(root: Path):
    dc = root / ".devcouncil"
    dc.mkdir(parents=True, exist_ok=True)
    (dc / "secrets.env").write_text("OPENROUTER_API_KEY=sk-or-test\n", encoding="utf-8")
    (dc / "config.yaml").write_text(
        """
models:
  provider: openrouter
  roles:
    planner_a:
      model: google/gemini-2.5-flash
    implementation_reviewer:
      model: ornith
      provider: ollama
    live_reviewer:
      model: ornith
      provider: ollama
""",
        encoding="utf-8",
    )


def test_build_verification_router_routes_review_role_to_ollama(tmp_path):
    _write_hybrid_config(tmp_path)
    router = _build_verification_router(tmp_path)
    assert router is not None
    # Planning role keeps the default (OpenRouter) provider...
    assert isinstance(router._provider_for_role(router.role_config["planner_a"]), OpenRouterProvider)
    # ...while the execution-time review role routes to local Ollama.
    review = router._provider_for_role(router.role_config["implementation_reviewer"])
    assert isinstance(review, OllamaProvider)


def test_build_verification_router_none_without_config(tmp_path):
    # No .devcouncil/config.yaml → degrade to deterministic-only verification, not an error.
    assert _build_verification_router(tmp_path) is None


def test_build_verification_router_sets_reviewer_on_verifier(tmp_path):
    _write_hybrid_config(tmp_path)
    from devcouncil.verification.verifier import Verifier

    router = _build_verification_router(tmp_path)
    assert Verifier(tmp_path, router=router).reviewer is not None
    assert Verifier(tmp_path, router=None).reviewer is None
