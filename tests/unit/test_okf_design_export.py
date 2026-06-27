"""Exporting a design.md into an OKF bundle stays typed, linked, and validate-clean.

Symmetric with skills: the design system rides the same bundle as the artifact graph, so the
export must remain a single connected, validate-clean graph. The design node is a typed
``Design System`` OKF document, a ``design/index.md`` links to it, and the root index links
to that design index. Default behavior (no design) must remain an artifact-only bundle.
"""

from devcouncil.artifacts.graph import ArtifactGraph
from devcouncil.domain.requirement import AcceptanceCriterion, Requirement
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.knowledge.design import design_system_to_okf_document, parse_design_md
from devcouncil.knowledge.okf import read_bundle, validate_bundle
from devcouncil.reporting.okf_bundle_writer import OKFBundleWriter

_DESIGN_MD = """---
name: Acme
colors:
  primary: "#1a1a1a"
  surface: "#ffffff"
typography:
  body:
    fontFamily: Inter
    fontSize: 16px
components:
  button:
    backgroundColor: colors.primary
    textColor: colors.surface
---
# Overview
Acme's design system.

# Colors
Primary is near-black.
"""


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
    return g


def test_design_system_to_okf_document_renders_tokens_and_body():
    ds = parse_design_md(_DESIGN_MD)
    doc = design_system_to_okf_document(ds)
    assert doc.type == "Design System"
    assert doc.title == "Acme"
    assert doc.rel_path == "design/design.md"
    # Tokens and rationale both make it into a deterministic body.
    assert "primary" in doc.body
    assert "#1a1a1a" in doc.body
    assert "fontFamily: Inter" in doc.body
    assert "Acme's design system." in doc.body
    # Deterministic: same input -> identical rendering.
    assert design_system_to_okf_document(parse_design_md(_DESIGN_MD)).body == doc.body


def test_export_with_design_is_typed_linked_and_valid(tmp_path):
    out = tmp_path / "bundle"
    ds = parse_design_md(_DESIGN_MD)
    OKFBundleWriter.generate(
        _graph(), out, project_name="Demo", include_design=True, design=ds
    )
    bundle = read_bundle(out)
    by_path = bundle.by_path()

    # The design system is its own typed OKF document under design/.
    assert "design/design.md" in by_path
    assert by_path["design/design.md"].type == "Design System"

    # The design index links to it, and the root index links to the design index, so the
    # design node joins the connected graph rather than dangling.
    design_index = by_path["design/index.md"]
    assert design_index.type == "OKF Index"
    assert "design/design.md" in design_index.links
    assert "design/index.md" in by_path["index.md"].links

    # Whole bundle still satisfies the OKF invariants (typed docs, no broken links).
    assert validate_bundle(bundle) == []


def test_export_without_design_omits_it(tmp_path):
    out = tmp_path / "bundle"
    # Default: no design included.
    OKFBundleWriter.generate(_graph(), out, project_name="Demo")
    bundle = read_bundle(out)
    by_path = bundle.by_path()

    assert not any(p.startswith("design/") for p in by_path)
    assert "design/index.md" not in by_path["index.md"].links
    assert validate_bundle(bundle) == []


def test_include_design_flag_requires_design_payload(tmp_path):
    out = tmp_path / "bundle"
    # include_design=True but no design supplied -> still an artifact-only bundle.
    OKFBundleWriter.generate(_graph(), out, include_design=True, design=None)
    bundle = read_bundle(out)
    assert not any(p.startswith("design/") for p in bundle.by_path())
    assert validate_bundle(bundle) == []
