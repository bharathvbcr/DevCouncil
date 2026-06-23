"""Rank 15 — secret findings carry real file:line, and skills rank by relevance."""

from devcouncil.gating.checks.secret_scan_check import SecretScanner
from devcouncil.skills.registry import select_skills


def test_secret_scan_sets_real_file_and_line():
    diff = (
        "diff --git a/src/c.py b/src/c.py\n"
        "--- a/src/c.py\n"
        "+++ b/src/c.py\n"
        "@@ -1,2 +1,3 @@\n"
        " line1\n"
        '+api_key="abcd1234efgh5678ijkl9012mnop3456"\n'
        " line2\n"
    )
    gaps = SecretScanner().scan_diff(diff, "TASK-001")
    assert gaps, "expected a secret finding"
    gap = gaps[0]
    assert gap.file == "src/c.py"
    assert gap.line == 2  # the added api_key line in the NEW file
    assert "src/c.py:2" in gap.description
    # The secret value itself must be redacted in the evidence.
    assert "abcd1234efgh5678ijkl9012mnop3456" not in " ".join(gap.evidence)


def test_secret_scan_ignores_removed_and_context_lines():
    diff = (
        "+++ b/src/d.py\n"
        "@@ -1,1 +1,1 @@\n"
        '-api_key="abcd1234efgh5678ijkl9012mnop3456"\n'  # removed line — not a new secret
        " unchanged\n"
    )
    assert SecretScanner().scan_diff(diff, "T") == []


def _write_skill(dir_path, name, keywords, body="body"):
    kws = ", ".join(f'"{k}"' for k in keywords)
    (dir_path / f"{name}.md").write_text(
        f"---\nname: {name}\ndescription: {name} skill\ntriggers:\n  keywords: [{kws}]\n---\n\n{body}\n",
        encoding="utf-8",
    )


def test_select_skills_ranks_by_relevance(tmp_path):
    lib = tmp_path / "lib"
    lib.mkdir()
    _write_skill(lib, "broad", ["python"])
    _write_skill(lib, "specific", ["python", "api"])

    selected = select_skills(goal="build a python api", library_dir=lib)

    names = [s.name for s in selected]
    assert names == ["specific", "broad"]  # more trigger hits ranks first


def test_relevance_score_weights_goal_keywords():
    from devcouncil.skills.registry import Skill, SkillTriggers

    skill = Skill(name="x", triggers=SkillTriggers(keywords=["android", "kotlin"]))
    assert skill.relevance_score("android kotlin app") == 4  # 2 keywords * weight 2
    assert skill.relevance_score("a web app") == 0
