"""Unit tests for acceptance-command evidence classification."""

from devcouncil.verification.command_evidence import (
    command_has_acceptance_evidence,
    command_is_trivial_evidence,
)


class TestCommandHasAcceptanceEvidence:
    def test_accepts_common_test_runners(self):
        assert command_has_acceptance_evidence("pytest tests/test_auth.py")
        assert command_has_acceptance_evidence("python -m pytest tests/")
        assert command_has_acceptance_evidence("cargo test")
        assert command_has_acceptance_evidence("go test ./...")
        assert command_has_acceptance_evidence("npm run vitest")

    def test_accepts_typecheck_and_lint_keywords(self):
        assert command_has_acceptance_evidence("mypy src/")
        assert command_has_acceptance_evidence("ruff check .")
        assert command_has_acceptance_evidence("npm run typecheck")

    def test_accepts_inline_assertions(self):
        assert command_has_acceptance_evidence('python -c "assert 1 == 1"')
        assert command_has_acceptance_evidence("node -e \"expect(1).toBe(1)\"")

    def test_rejects_trivial_self_cert_commands(self):
        assert not command_has_acceptance_evidence('python -c "print(\'ok\')"')
        assert not command_has_acceptance_evidence("echo passed")
        assert not command_has_acceptance_evidence("true")

    def test_rejects_empty_or_whitespace(self):
        assert not command_has_acceptance_evidence("")
        assert not command_has_acceptance_evidence("   ")

    def test_case_insensitive(self):
        assert command_has_acceptance_evidence("PyTest tests/")
        assert not command_has_acceptance_evidence('Python -c "Print(1)"')

    def test_rejects_quoted_keyword_smuggling(self):
        # Structure-aware: a keyword inside a quoted argument of a non-runner
        # executable must never qualify — the classic self-certification move.
        assert not command_has_acceptance_evidence('echo "tests assert ok"')
        assert not command_has_acceptance_evidence('bash -c "echo test"')
        assert not command_has_acceptance_evidence('printf "pytest passed"')

    def test_accepts_test_path_arguments_and_scripts(self):
        assert command_has_acceptance_evidence("python tests/test_x.py")
        assert command_has_acceptance_evidence("./run_tests.sh")
        assert command_has_acceptance_evidence("bash tests/smoke.sh")

    def test_accepts_python_dash_m_unittest(self):
        assert command_has_acceptance_evidence("python -m unittest discover")
        assert command_has_acceptance_evidence("python3 -m pytest -q")

    def test_rejects_version_and_bare_interpreter(self):
        assert not command_has_acceptance_evidence("python --version")
        assert not command_has_acceptance_evidence("node")


class TestCommandIsTrivialEvidence:
    """Verify-time DENY-list: trusted planner/config commands keep evidential value
    unless they provably exercise nothing. This is deliberately more permissive than
    the strict allowlist used for untrusted agent input."""

    def test_keyword_less_behavioral_commands_are_not_trivial(self):
        # The false negatives the allowlist alone would cause.
        assert not command_is_trivial_evidence("make check")
        assert not command_is_trivial_evidence("./run_smoke.sh")
        assert not command_is_trivial_evidence("python scripts/validate.py")
        assert not command_is_trivial_evidence("npm run e2e")

    def test_allowlisted_test_commands_are_never_trivial(self):
        assert not command_is_trivial_evidence("pytest tests/test_auth.py")
        assert not command_is_trivial_evidence('python -c "import m; assert m.f() == 1"')

    def test_identity_checks_are_trivial(self):
        assert command_is_trivial_evidence("python --version")
        assert command_is_trivial_evidence("node --help")

    def test_pure_printing_and_system_queries_are_trivial(self):
        assert command_is_trivial_evidence("echo passed")
        assert command_is_trivial_evidence("true")
        assert command_is_trivial_evidence("ls -la src/")
        assert command_is_trivial_evidence("cat src/app.py")
        assert command_is_trivial_evidence("date")

    def test_git_state_queries_are_trivial(self):
        assert command_is_trivial_evidence("git status --porcelain")
        assert command_is_trivial_evidence("git diff --name-only")

    def test_printing_one_liners_are_trivial_but_asserting_ones_are_not(self):
        assert command_is_trivial_evidence("python -c \"print('ok')\"")
        assert not command_is_trivial_evidence('python -c "import app; assert app.VALUE == 1"')

    def test_empty_is_trivial(self):
        assert command_is_trivial_evidence("")
        assert command_is_trivial_evidence("   ")
