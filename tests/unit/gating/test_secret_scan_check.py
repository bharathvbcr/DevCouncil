"""SecretScanner.scan_diff — pure diff scanning: file/line attribution across
hunks and files, the emitted gap contract, and redaction of evidence."""

from devcouncil.gating.checks.secret_scan_check import SecretScanner

TASK_ID = "TASK-9"

# Two files; the first file has two hunks. The new-file line counter must reset
# on every hunk header and the file attribution must switch on "+++ b/".
MULTI_FILE_DIFF = (
    "diff --git a/src/app/config.py b/src/app/config.py\n"
    "--- a/src/app/config.py\n"
    "+++ b/src/app/config.py\n"
    "@@ -1,3 +1,4 @@\n"
    " import os\n"
    "-OLD_FLAG = False\n"
    '+AWS_ID = "AKIAABCDEFGHIJKLMNOP"\n'
    "+NEW_FLAG = True\n"
    "@@ -20,2 +21,3 @@\n"
    " def load():\n"
    '+KEY2 = "AKIAQRSTUVWXYZ234567"\n'
    "     return None\n"
    "diff --git a/b/settings.py b/b/settings.py\n"
    "--- a/b/settings.py\n"
    "+++ b/b/settings.py\n"
    "@@ -0,0 +1,2 @@\n"
    '+MAPS = "AIza' + "B" * 35 + '"\n'
    "+PORT = 8080\n"
)


def test_scan_diff_attributes_file_and_new_file_line_across_hunks_and_files():
    gaps = SecretScanner().scan_diff(MULTI_FILE_DIFF, TASK_ID)

    assert [(g.file, g.line) for g in gaps] == [
        ("src/app/config.py", 2),
        ("src/app/config.py", 22),
        ("b/settings.py", 1),
    ]
    assert [g.id for g in gaps] == [
        "GAP-TASK-9-SECRET-AWS_ACCESS_KEY-2-1",
        "GAP-TASK-9-SECRET-AWS_ACCESS_KEY-22-2",
        "GAP-TASK-9-SECRET-GOOGLE_API_KEY-1-3",
    ]
    assert gaps[0].description == "Potential aws_access_key found in src/app/config.py:2."


def test_scan_diff_gaps_are_blocking_critical_security_risks_with_redacted_evidence():
    gaps = SecretScanner().scan_diff(MULTI_FILE_DIFF, TASK_ID)

    assert all(g.severity == "critical" for g in gaps)
    assert all(g.blocking for g in gaps)
    assert all(g.gap_type == "security_risk" for g in gaps)
    assert all(g.task_id == TASK_ID for g in gaps)

    combined_evidence = " ".join(item for g in gaps for item in g.evidence)
    assert "AKIAABCDEFGHIJKLMNOP" not in combined_evidence
    assert "AKIAQRSTUVWXYZ234567" not in combined_evidence
    assert "AIza" + "B" * 35 not in combined_evidence
    assert "[REDACTED:aws_access_key]" in gaps[0].evidence[0]
    assert "[REDACTED:google_api_key]" in gaps[2].evidence[0]


def test_scan_diff_returns_no_gaps_for_benign_or_removed_only_changes():
    # A diff that only REMOVES a secret (replacing it with an env lookup) is the
    # desired fix and must not be flagged; only added lines are scanned.
    diff = (
        "diff --git a/src/ok.py b/src/ok.py\n"
        "--- a/src/ok.py\n"
        "+++ b/src/ok.py\n"
        "@@ -1,3 +1,3 @@\n"
        " import os\n"
        '-AWS_ID = "AKIAABCDEFGHIJKLMNOP"\n'
        '+AWS_ID = os.environ["AWS_ID"]\n'
    )

    assert SecretScanner().scan_diff(diff, TASK_ID) == []
    assert SecretScanner().scan_diff("", TASK_ID) == []
