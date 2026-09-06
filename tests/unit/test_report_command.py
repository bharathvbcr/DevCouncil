"""CLI coverage for `dev report` — output modes, evidence exports, CI-comment
branches (env-guarded), and the rigor subcommand."""

import json

from typer.testing import CliRunner

from devcouncil.cli.main import app

runner = CliRunner()


def test_report_markdown_default(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    result = runner.invoke(app, ["report"])
    assert result.exit_code == 0


def test_report_json(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    result = runner.invoke(app, ["report", "--json"])
    assert result.exit_code == 0
    assert json.loads(result.stdout)["verdict"] == "passed"


def test_report_planning_only(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    result = runner.invoke(app, ["report", "--planning-only", "--json"])
    assert result.exit_code == 0


def test_report_evidence_json_written(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    out = tmp_path / "evidence.json"

    result = runner.invoke(app, ["report", "--evidence-json", str(out)])
    assert result.exit_code == 0
    assert out.is_file()
    assert "Wrote evidence export" in result.output
    json.loads(out.read_text(encoding="utf-8"))  # valid JSON


def test_report_evidence_html_written(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    out = tmp_path / "evidence.html"

    result = runner.invoke(app, ["report", "--evidence-html", str(out)])
    assert result.exit_code == 0
    assert out.is_file()
    assert "Wrote evidence HTML" in result.output
    assert "<" in out.read_text(encoding="utf-8")


def test_report_github_requires_env(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    monkeypatch.delenv("GITHUB_TOKEN", raising=False)
    monkeypatch.delenv("GITHUB_REPOSITORY", raising=False)
    assert runner.invoke(app, ["init"]).exit_code == 0

    result = runner.invoke(app, ["report", "--github"])
    assert result.exit_code == 0
    assert "GITHUB_TOKEN and GITHUB_REPOSITORY must be set" in result.output


def test_report_github_pr_comment_requires_env(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    monkeypatch.delenv("GITHUB_TOKEN", raising=False)
    monkeypatch.delenv("GITHUB_REPOSITORY", raising=False)
    monkeypatch.delenv("GITHUB_PR_NUMBER", raising=False)
    monkeypatch.delenv("PR_NUMBER", raising=False)
    assert runner.invoke(app, ["init"]).exit_code == 0

    result = runner.invoke(app, ["report", "--github-pr-comment"])
    assert result.exit_code == 0
    assert "GITHUB_PR_NUMBER must be set" in result.output


def test_report_gitlab_mr_comment_requires_env(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    for var in ("GITLAB_TOKEN", "GITLAB_PROJECT_ID", "GITLAB_MR_IID", "CI_MERGE_REQUEST_IID"):
        monkeypatch.delenv(var, raising=False)
    assert runner.invoke(app, ["init"]).exit_code == 0

    result = runner.invoke(app, ["report", "--gitlab-pr-comment"])
    assert result.exit_code == 0
    assert "GITLAB_TOKEN, GITLAB_PROJECT_ID, and GITLAB_MR_IID must be set" in result.output


def test_report_github_pr_comment_rejects_non_integer_pr(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    monkeypatch.setenv("GITHUB_TOKEN", "tok")
    monkeypatch.setenv("GITHUB_REPOSITORY", "owner/repo")
    monkeypatch.setenv("GITHUB_PR_NUMBER", "not-a-number")
    assert runner.invoke(app, ["init"]).exit_code == 0

    result = runner.invoke(app, ["report", "--github-pr-comment"])
    assert result.exit_code == 0
    assert "GITHUB_PR_NUMBER must be an integer" in result.output


def test_report_missing_state_errors(tmp_path, monkeypatch):
    from devcouncil.cli.commands import report as report_cmd

    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    monkeypatch.setattr(report_cmd, "get_db", lambda root: None)

    result = runner.invoke(app, ["report"])
    assert result.exit_code == 1
    assert "state is unavailable" in result.output


def test_report_rigor_json(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    result = runner.invoke(app, ["report", "rigor", "--json"])
    assert result.exit_code == 0
    json.loads(result.stdout)


def test_report_rigor_markdown(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    result = runner.invoke(app, ["report", "rigor"])
    assert result.exit_code == 0


def test_report_fail_on_blocking_exits_nonzero(tmp_path, monkeypatch):
    from devcouncil.domain.gap import Gap
    from devcouncil.storage.repositories import GapRepository

    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    from devcouncil.storage.db import get_db

    db = get_db(tmp_path)
    with db.get_session() as session:
        GapRepository(session).save(
            Gap(
                id="G1",
                severity="high",
                gap_type="test_failed",
                task_id="T",
                description="boom",
                recommended_fix="fix",
                blocking=True,
            )
        )

    result = runner.invoke(app, ["report", "--json", "--fail-on-blocking"])
    assert result.exit_code == 1


def test_report_evidence_json_write_error(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    out = tmp_path / "out.json"
    import devcouncil.cli.commands.report as report_cmd

    def _write_fail(self, *args, **kwargs):
        raise OSError("permission denied")

    monkeypatch.setattr(report_cmd.Path, "write_text", _write_fail)

    result = runner.invoke(app, ["report", "--evidence-json", str(out)])
    assert result.exit_code == 1
    assert "Failed to write evidence export" in result.output


def test_report_evidence_html_write_error(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    out = tmp_path / "out.html"
    import devcouncil.cli.commands.report as report_cmd

    def _write_fail(self, *args, **kwargs):
        raise OSError("permission denied")

    monkeypatch.setattr(report_cmd.Path, "write_text", _write_fail)

    result = runner.invoke(app, ["report", "--evidence-html", str(out)])
    assert result.exit_code == 1
    assert "Failed to write evidence HTML" in result.output


def test_report_github_success(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    monkeypatch.setenv("GITHUB_TOKEN", "tok")
    monkeypatch.setenv("GITHUB_REPOSITORY", "owner/repo")
    monkeypatch.setenv("GITHUB_SHA", "abc123def456")
    assert runner.invoke(app, ["init"]).exit_code == 0

    class _Integration:
        def __init__(self, token, repo, sha):
            self.called = True

        async def report_verification(self, graph):
            self.reported = True

    import devcouncil.cli.commands.report as report_cmd

    monkeypatch.setattr(report_cmd, "GitHubIntegration", _Integration)

    result = runner.invoke(app, ["report", "--github"])
    assert result.exit_code == 0
    assert "Successfully reported to GitHub" in result.output


def test_report_gitlab_mr_comment_invalid_iid(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    monkeypatch.setenv("GITLAB_TOKEN", "tok")
    monkeypatch.setenv("GITLAB_PROJECT_ID", "123")
    monkeypatch.setenv("GITLAB_MR_IID", "nope")
    assert runner.invoke(app, ["init"]).exit_code == 0

    result = runner.invoke(app, ["report", "--gitlab-pr-comment"])
    assert result.exit_code == 0
    assert "GITLAB_MR_IID must be an integer" in result.output


def test_report_github_pr_comment_success(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    monkeypatch.setenv("GITHUB_TOKEN", "tok")
    monkeypatch.setenv("GITHUB_REPOSITORY", "owner/repo")
    monkeypatch.setenv("GITHUB_PR_NUMBER", "42")
    assert runner.invoke(app, ["init"]).exit_code == 0

    posted = {}

    class _Commenter:
        def __init__(self, token, repo, pr):
            posted["pr"] = pr

        async def post_comment(self, body):
            posted["body"] = body

    import devcouncil.cli.commands.report as report_cmd

    monkeypatch.setattr(report_cmd, "GitHubPRCommenter", _Commenter)

    result = runner.invoke(app, ["report", "--github-pr-comment"])
    assert result.exit_code == 0
    assert posted["pr"] == 42
    assert "Posted DevCouncil PR comment" in result.output


# --- `--json` contract: exactly one JSON object on stdout, diagnostics on stderr ---
#
# These assert on `result.stdout`, never `result.output`. Under Click 8.4 `.output` is
# the stdout+stderr streams merged, so it parses identically whether or not a banner
# leaked onto stdout — it cannot detect this bug, which is why the tests above passed
# while `dev report --json --evidence-json X` emitted a banner and no payload at all.


def _init(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0


def test_report_json_evidence_json_emits_one_object(tmp_path, monkeypatch):
    """The branch wrote a file, printed a banner to stdout, and returned before the payload."""
    _init(tmp_path, monkeypatch)
    out = tmp_path / "evidence.json"

    result = runner.invoke(app, ["report", "--json", "--evidence-json", str(out)])

    assert result.exit_code == 0
    assert out.is_file()
    data = json.loads(result.stdout)
    assert data["ok"] is True
    assert data["action"] == "evidence-json"
    assert data["output_path"] == str(out)
    assert data["error"] is None
    assert "Wrote evidence export" in result.stderr
    assert "Wrote evidence export" not in result.stdout


def test_report_json_evidence_html_emits_one_object(tmp_path, monkeypatch):
    _init(tmp_path, monkeypatch)
    out = tmp_path / "evidence.html"

    result = runner.invoke(app, ["report", "--json", "--evidence-html", str(out)])

    assert result.exit_code == 0
    assert out.is_file()
    data = json.loads(result.stdout)
    assert data["action"] == "evidence-html"
    assert data["output_path"] == str(out)
    assert "Wrote evidence HTML" in result.stderr


def test_report_json_evidence_write_failure_emits_one_object(tmp_path, monkeypatch):
    """A failed write is still a `--json` invocation; it owes stdout an object."""
    _init(tmp_path, monkeypatch)
    blocker = tmp_path / "blocker"
    blocker.write_text("not a directory", encoding="utf-8")

    result = runner.invoke(
        app, ["report", "--json", "--evidence-json", str(blocker / "nested" / "evidence.json")]
    )

    assert result.exit_code == 1
    data = json.loads(result.stdout)
    assert data["ok"] is False
    assert data["action"] == "evidence-json"
    assert "Failed to write evidence export" in data["error"]
    assert "Failed to write evidence export" in result.stderr


def test_report_json_github_branch_emits_one_object(tmp_path, monkeypatch):
    """`--github` does its work in a check run and returned with stdout empty."""
    monkeypatch.delenv("GITHUB_TOKEN", raising=False)
    monkeypatch.delenv("GITHUB_REPOSITORY", raising=False)
    _init(tmp_path, monkeypatch)

    result = runner.invoke(app, ["report", "--json", "--github"])

    assert result.exit_code == 0
    data = json.loads(result.stdout)
    assert data["action"] == "github-checks"
    assert data["blocking_gaps"] == 0
    assert "GITHUB_TOKEN and GITHUB_REPOSITORY must be set" in result.stderr
    assert "GITHUB_TOKEN" not in result.stdout


def test_report_json_gitlab_branch_emits_one_object(tmp_path, monkeypatch):
    monkeypatch.delenv("GITLAB_TOKEN", raising=False)
    monkeypatch.delenv("GITLAB_PROJECT_ID", raising=False)
    monkeypatch.delenv("GITLAB_MR_IID", raising=False)
    _init(tmp_path, monkeypatch)

    result = runner.invoke(app, ["report", "--json", "--gitlab-pr-comment"])

    assert result.exit_code == 0
    assert json.loads(result.stdout)["action"] == "gitlab-mr-comment"
    assert "GITLAB_TOKEN" in result.stderr


def test_report_json_missing_state_emits_one_object(tmp_path, monkeypatch):
    from devcouncil.cli.commands import report as report_cmd

    _init(tmp_path, monkeypatch)
    monkeypatch.setattr(report_cmd, "get_db", lambda root: None)

    result = runner.invoke(app, ["report", "--json"])

    assert result.exit_code == 1
    data = json.loads(result.stdout)
    assert data["ok"] is False
    assert "state is unavailable" in data["error"]
    assert "state is unavailable" in result.stderr


def test_release_health_json_write_banners_stay_off_stdout(tmp_path, monkeypatch):
    """Both `--write-baseline` and `--output` printed a banner ahead of the payload."""
    _init(tmp_path, monkeypatch)
    out = tmp_path / "release-health.json"

    result = runner.invoke(
        app, ["report", "release-health", "--json", "--write-baseline", "--output", str(out)]
    )

    assert result.exit_code == 0
    assert out.is_file()
    json.loads(result.stdout)  # the report itself — the sole object on stdout
    assert "Wrote release-health baseline" in result.stderr
    assert "Wrote release-health report" in result.stderr
    assert "Wrote release-health" not in result.stdout


def test_release_health_json_missing_state_emits_one_object(tmp_path, monkeypatch):
    from devcouncil.cli.commands import report as report_cmd

    _init(tmp_path, monkeypatch)
    monkeypatch.setattr(report_cmd, "get_db", lambda root: None)

    result = runner.invoke(app, ["report", "release-health", "--json"])

    assert result.exit_code == 1
    assert json.loads(result.stdout)["ok"] is False
