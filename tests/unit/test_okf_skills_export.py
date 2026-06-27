"""Exporting engineering skills into an OKF bundle keeps the graph typed and link-valid.

The skill nodes ride the same bundle as the artifact graph, so the export must stay a
single connected, validate-clean graph: every skill is a typed OKF document, a
``skills/index.md`` links to each of them, and the root index links to that skills index.
Default behavior (no skills) must remain an artifact-only bundle.
"""

from devcouncil.artifacts.graph import ArtifactGraph
from devcouncil.domain.evidence import DiffEvidence
from devcouncil.domain.gap import Gap
from devcouncil.domain.requirement import AcceptanceCriterion, Requirement
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.knowledge.okf import read_bundle, validate_bundle
from devcouncil.knowledge.skill_bridge import SKILL_OKF_TYPE
from devcouncil.reporting.okf_bundle_writer import OKFBundleWriter
from devcouncil.skills.registry import Skill, SkillTriggers, load_skills


def _graph() -> ArtifactGraph:
    g = ArtifactGraph()
    g.add_requirement(Requirement(
        id="REQ-001", title="Login", description="Users can log in.",
        priority="high", source="user",
        acceptance_criteria=[AcceptanceCriterion(
            id="AC-1", description="valid creds succeed", verification_method="unit_test")],
    ))
    g.add_task(Task(
        id="TASK-001", title="Implement login", description="Add login handler.",
        requirement_ids=["REQ-001"],
        planned_files=[PlannedFile(path="auth.py", reason="handler", allowed_change="create")],
        status="verified",
    ))
    g.add_diff_evidence(DiffEvidence(
        task_id="TASK-001", changed_files=["auth.py"], added_files=["auth.py"],
        deleted_files=[], diff_summary="Added login handler.",
    ))
    g.add_gap(Gap(
        id="GAP-001", severity="high", gap_type="missing_test", task_id="TASK-001",
        requirement_id="REQ-001", description="No test for login.",
        recommended_fix="Add a unit test.", blocking=True,
    ))
    return g


def _skills() -> list[Skill]:
    return [
        Skill(
            name="web", title="Web Frontend",
            description="React/TypeScript web app guidance.",
            triggers=SkillTriggers(keywords=["react", "web"], globs=["package.json"]),
            body="Prefer typed components and colocated tests.",
        ),
        Skill(
            name="ios", title="iOS & SwiftUI",
            description="SwiftUI / iOS guidance.",
            triggers=SkillTriggers(keywords=["swift", "ios"], globs=["*.swift"]),
            body="Use value types and previews.",
        ),
    ]


def test_export_with_skills_is_typed_linked_and_valid(tmp_path):
    out = tmp_path / "bundle"
    OKFBundleWriter.generate(
        _graph(), out, project_name="Demo", include_skills=True, skills=_skills()
    )
    bundle = read_bundle(out)
    by_path = bundle.by_path()

    # Each skill is its own typed OKF document under skills/.
    assert "skills/web.md" in by_path
    assert "skills/ios.md" in by_path
    assert by_path["skills/web.md"].type == SKILL_OKF_TYPE
    assert by_path["skills/ios.md"].type == SKILL_OKF_TYPE
    # Keyword triggers survive as tags; globs are intentionally not represented.
    assert "react" in by_path["skills/web.md"].tags

    # The skills index links to every skill, and the root index links to the skills index,
    # so the skill nodes join the connected graph rather than dangling.
    skills_index = by_path["skills/index.md"]
    assert skills_index.type == "OKF Index"
    assert "skills/web.md" in skills_index.links
    assert "skills/ios.md" in skills_index.links
    assert "skills/index.md" in by_path["index.md"].links

    # Whole bundle still satisfies the OKF invariants (typed docs, no broken links).
    assert validate_bundle(bundle) == []


def test_export_without_skills_omits_them(tmp_path):
    out = tmp_path / "bundle"
    # Default: no skills included.
    OKFBundleWriter.generate(_graph(), out, project_name="Demo")
    bundle = read_bundle(out)
    by_path = bundle.by_path()

    assert not any(p.startswith("skills/") for p in by_path)
    assert "skills/index.md" not in by_path["index.md"].links
    assert validate_bundle(bundle) == []


def test_include_skills_flag_requires_skills_payload(tmp_path):
    # include_skills=True but no skills supplied → still an artifact-only bundle.
    out = tmp_path / "bundle"
    OKFBundleWriter.generate(_graph(), out, include_skills=True, skills=None)
    bundle = read_bundle(out)
    assert not any(p.startswith("skills/") for p in bundle.by_path())
    assert validate_bundle(bundle) == []


def test_export_with_real_library_skills_validates_clean(tmp_path):
    # Guards against a future packaged skill whose body contains a repo-relative markdown
    # link (e.g. `[x](../docs/y.md)`): read_bundle would resolve it to an intra-bundle edge
    # and validate_bundle would flag it broken. The synthetic-skill tests above can't catch
    # that, so export the actual library and assert the whole bundle stays link-valid.
    real_skills = load_skills()
    assert real_skills, "expected packaged library skills to load"
    out = tmp_path / "bundle"
    OKFBundleWriter.generate(
        _graph(), out, project_name="Demo", include_skills=True, skills=real_skills
    )
    bundle = read_bundle(out)
    assert validate_bundle(bundle) == []
