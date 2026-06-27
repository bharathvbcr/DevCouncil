"""Export DevCouncil's artifact graph as an Open Knowledge Format (OKF) bundle.

DevCouncil already keeps a durable Requirement→Task→Evidence→Gap graph; OKF is the natural
portable wire format for it. This writer renders that graph as a cross-linked directory of
markdown+frontmatter documents (an :class:`devcouncil.knowledge.okf.OKFBundle`) that any
OKF-aware agent — or the upstream OKF HTML visualizer — can consume, with the relationships
expressed as real markdown links so the "linked graph" property holds.
"""

from __future__ import annotations

import re
from pathlib import Path

from devcouncil.artifacts.graph import ArtifactGraph
from devcouncil.indexing.repo_mapper import RepoMap
from devcouncil.knowledge.okf import OKFBundle, OKFDocument, write_bundle

_SLUG_RE = re.compile(r"[^a-zA-Z0-9._-]+")


def _slug(value: str) -> str:
    return _SLUG_RE.sub("-", value).strip("-") or "item"


def _link(rel_from: str, rel_to: str, text: str) -> str:
    """A markdown link from one bundle document to another, as a relative path."""
    from_parts = rel_from.split("/")[:-1]
    to_parts = rel_to.split("/")
    # Compute a relative path (both live under the same bundle root).
    i = 0
    while i < len(from_parts) and i < len(to_parts) - 1 and from_parts[i] == to_parts[i]:
        i += 1
    up = [".."] * (len(from_parts) - i)
    down = to_parts[i:]
    rel = "/".join(up + down) or rel_to.split("/")[-1]
    return f"[{text}]({rel})"


class OKFBundleWriter:
    """Builds an OKF bundle from an :class:`ArtifactGraph` and writes it to disk."""

    @staticmethod
    def build(
        graph: ArtifactGraph,
        repo_map: RepoMap | None = None,
        project_name: str = "DevCouncil Project",
        timestamp: str = "",
        include_skills: bool = False,
        skills: "list | None" = None,
        include_design: bool = False,
        design=None,
    ) -> OKFBundle:
        """Build the in-memory :class:`OKFBundle` (no filesystem writes).

        When ``include_skills`` is set and ``skills`` are supplied, each engineering skill
        is rendered as an OKF document (via the shared :mod:`skill_bridge`) under
        ``skills/`` and indexed by a ``skills/index.md`` node so the bundle stays a single
        connected, link-valid graph. Default behavior (no skills) is unchanged.

        Symmetrically, when ``include_design`` is set and a ``design`` (a
        :class:`devcouncil.knowledge.design.DesignSystem`) is supplied, it is rendered as a
        ``Design System`` OKF document under ``design/`` and indexed by ``design/index.md``,
        which the root index links. Default behavior (no design) is unchanged.
        """
        docs: list[OKFDocument] = []

        # Pre-compute task ids covering each requirement, for cross-linking.
        tasks_for_req: dict[str, list[str]] = {}
        for task in graph.tasks.values():
            for rid in task.requirement_ids:
                tasks_for_req.setdefault(rid, []).append(task.id)

        # --- Requirements ---
        for req in graph.requirements.values():
            rel = f"requirements/{_slug(req.id)}.md"
            lines = [req.description.strip(), ""]
            if req.acceptance_criteria:
                lines.append("### Acceptance criteria")
                for ac in req.acceptance_criteria:
                    flag = "required" if ac.required else "optional"
                    lines.append(f"- `{ac.id}` ({ac.verification_method}, {flag}): {ac.description}")
                lines.append("")
            covering = tasks_for_req.get(req.id, [])
            if covering:
                lines.append("### Covered by")
                for tid in covering:
                    lines.append(f"- {_link(rel, f'tasks/{_slug(tid)}.md', tid)}")
            docs.append(OKFDocument(
                type="DevCouncil Requirement",
                title=f"{req.id}: {req.title}",
                description=req.description.strip()[:280],
                tags=["requirement", req.priority, req.source],
                timestamp=timestamp,
                body="\n".join(lines).strip(),
                rel_path=rel,
            ))

        # --- Tasks ---
        diffs_by_task: dict[str, list] = {}
        for de in graph.diff_evidence:
            diffs_by_task.setdefault(de.task_id, []).append(de)
        gaps_by_task: dict[str, list] = {}
        for gap in graph.gaps.values():
            if gap.task_id:
                gaps_by_task.setdefault(gap.task_id, []).append(gap)

        for task in graph.tasks.values():
            rel = f"tasks/{_slug(task.id)}.md"
            lines = [task.description.strip(), ""]
            if task.requirement_ids:
                lines.append("### Implements")
                for rid in task.requirement_ids:
                    label = rid
                    if rid in graph.requirements:
                        lines.append(f"- {_link(rel, f'requirements/{_slug(rid)}.md', label)}")
                    else:
                        lines.append(f"- {label}")
                lines.append("")
            if task.planned_files:
                lines.append("### Planned files")
                for pf in task.planned_files:
                    lines.append(f"- `{pf.path}` ({pf.allowed_change}): {pf.reason}")
                lines.append("")
            if task.id in diffs_by_task:
                lines.append("### Evidence")
                for idx, _de in enumerate(diffs_by_task[task.id]):
                    ev_rel = f"evidence/{_slug(task.id)}-diff-{idx}.md"
                    lines.append(f"- {_link(rel, ev_rel, 'diff evidence')}")
                lines.append("")
            if task.id in gaps_by_task:
                lines.append("### Open gaps")
                for gap in gaps_by_task[task.id]:
                    lines.append(f"- {_link(rel, f'gaps/{_slug(gap.id)}.md', gap.id)}")
            docs.append(OKFDocument(
                type="DevCouncil Task",
                title=f"{task.id}: {task.title}",
                description=task.description.strip()[:280],
                tags=["task", task.status],
                timestamp=timestamp,
                body="\n".join(lines).strip(),
                rel_path=rel,
            ))

        # --- Evidence (diff evidence as documents; resource points at the diff summary) ---
        for task_id, evs in diffs_by_task.items():
            for idx, de in enumerate(evs):
                rel = f"evidence/{_slug(task_id)}-diff-{idx}.md"
                body_lines = [de.diff_summary.strip(), ""]
                if de.changed_files:
                    body_lines.append("**Changed files:**")
                    body_lines.extend(f"- `{f}`" for f in de.changed_files)
                if task_id in graph.tasks:
                    body_lines.append("")
                    body_lines.append(f"Produced by {_link(rel, f'tasks/{_slug(task_id)}.md', task_id)}.")
                docs.append(OKFDocument(
                    type="DevCouncil Evidence",
                    title=f"Diff evidence for {task_id}",
                    description=de.diff_summary.strip()[:280],
                    tags=["evidence", "diff"],
                    timestamp=timestamp,
                    body="\n".join(body_lines).strip(),
                    rel_path=rel,
                ))

        # --- Gaps ---
        for gap in graph.gaps.values():
            rel = f"gaps/{_slug(gap.id)}.md"
            lines = [gap.description.strip(), ""]
            lines.append(f"**Recommended fix:** {gap.recommended_fix.strip()}")
            lines.append("")
            related = []
            if gap.requirement_id and gap.requirement_id in graph.requirements:
                related.append(_link(rel, f"requirements/{_slug(gap.requirement_id)}.md", gap.requirement_id))
            if gap.task_id and gap.task_id in graph.tasks:
                related.append(_link(rel, f"tasks/{_slug(gap.task_id)}.md", gap.task_id))
            if related:
                lines.append("**Related:** " + ", ".join(related))
            docs.append(OKFDocument(
                type="DevCouncil Gap",
                title=f"{gap.id}: {gap.gap_type}",
                description=gap.description.strip()[:280],
                tags=["gap", gap.severity, "blocking" if gap.blocking else "non-blocking"],
                timestamp=timestamp,
                body="\n".join(lines).strip(),
                rel_path=rel,
            ))

        # --- Engineering skills (optional) ---
        skill_docs = (
            OKFBundleWriter._build_skill_docs(skills, timestamp)
            if include_skills and skills
            else []
        )
        docs.extend(skill_docs)

        # --- Design system (optional) ---
        design_doc = (
            OKFBundleWriter._build_design_doc(design, timestamp)
            if include_design and design is not None
            else None
        )
        if design_doc is not None:
            docs.append(design_doc)

        # --- index.md hierarchy ---
        docs.extend(
            OKFBundleWriter._build_indexes(graph, project_name, timestamp, skill_docs, design_doc)
        )

        return OKFBundle(documents=docs)

    @staticmethod
    def _build_design_doc(design, timestamp: str) -> OKFDocument:
        """Render a design system as an OKF document under ``design/``.

        Conversion goes through :func:`design.design_system_to_okf_document` so the bundle
        and any future design ingest share one DesignSystem<->OKF mapping. The bundle-level
        ``timestamp`` is stamped on (the renderer itself leaves it empty as library content).
        """
        from devcouncil.knowledge.design import design_system_to_okf_document

        doc = design_system_to_okf_document(design)
        if timestamp:
            doc = doc.model_copy(update={"timestamp": timestamp})
        return doc

    @staticmethod
    def _build_skill_docs(skills: "list", timestamp: str) -> list[OKFDocument]:
        """Render engineering skills as OKF documents under ``skills/``.

        Conversion goes through :func:`skill_bridge.skill_to_okf_document` so the export
        side and the ingest side share one Skill<->OKF mapping and can't drift. The
        bundle-level ``timestamp`` is stamped on so skill nodes carry the same export time
        as the rest of the bundle (the bridge itself leaves it empty as library content).
        """
        from devcouncil.knowledge.skill_bridge import skill_to_okf_document

        out: list[OKFDocument] = []
        for skill in skills:
            doc = skill_to_okf_document(skill)
            if timestamp:
                doc = doc.model_copy(update={"timestamp": timestamp})
            out.append(doc)
        return out

    @staticmethod
    def _build_indexes(
        graph: ArtifactGraph,
        project_name: str,
        timestamp: str,
        skill_docs: "list[OKFDocument] | None" = None,
        design_doc: "OKFDocument | None" = None,
    ) -> list[OKFDocument]:
        indexes: list[OKFDocument] = []
        summary = graph.coverage_summary()
        skill_docs = skill_docs or []

        def section_index(folder: str, title: str, items: list[tuple[str, str]]) -> None:
            rel = f"{folder}/index.md"
            lines = [f"{len(items)} {title.lower()}.", ""]
            for item_id, item_title in items:
                lines.append(f"- {_link(rel, f'{folder}/{_slug(item_id)}.md', item_title)}")
            indexes.append(OKFDocument(
                type="OKF Index",
                title=title,
                description=f"{len(items)} {title.lower()} exported from DevCouncil.",
                timestamp=timestamp,
                body="\n".join(lines).strip(),
                rel_path=rel,
            ))

        section_index("requirements", "Requirements",
                      [(r.id, f"{r.id}: {r.title}") for r in graph.requirements.values()])
        section_index("tasks", "Tasks",
                      [(t.id, f"{t.id}: {t.title}") for t in graph.tasks.values()])
        section_index("gaps", "Gaps",
                      [(g.id, f"{g.id}: {g.gap_type}") for g in graph.gaps.values()])

        # Skills index — links to each skill document so they join the connected graph.
        if skill_docs:
            rel = "skills/index.md"
            lines = [f"{len(skill_docs)} engineering skills.", ""]
            for doc in skill_docs:
                lines.append(f"- {_link(rel, doc.rel_path, doc.title or doc.rel_path)}")
            indexes.append(OKFDocument(
                type="OKF Index",
                title="Skills",
                description=f"{len(skill_docs)} engineering skills exported from DevCouncil.",
                timestamp=timestamp,
                body="\n".join(lines).strip(),
                rel_path=rel,
            ))

        # Design index — links to the single design document so it joins the graph.
        if design_doc is not None:
            rel = "design/index.md"
            lines = [
                "1 design system.",
                "",
                f"- {_link(rel, design_doc.rel_path, design_doc.title or design_doc.rel_path)}",
            ]
            indexes.append(OKFDocument(
                type="OKF Index",
                title="Design System",
                description="Design system exported from DevCouncil.",
                timestamp=timestamp,
                body="\n".join(lines).strip(),
                rel_path=rel,
            ))

        # Root index links to each section index.
        root_lines = [
            f"Knowledge bundle exported from **{project_name}** in Open Knowledge Format.",
            "",
            "### Coverage",
            f"- Requirements: {summary['total_requirements']}",
            f"- Tasks: {summary['total_tasks']}",
            f"- Acceptance criteria: {summary['total_ac']} ({summary['ac_without_evidence']} without evidence)",
            f"- Gaps: {summary['total_gaps']} ({summary['blocking_gaps']} blocking)",
            "",
            "### Sections",
            "- [Requirements](requirements/index.md)",
            "- [Tasks](tasks/index.md)",
            "- [Gaps](gaps/index.md)",
        ]
        if skill_docs:
            root_lines.append("- [Skills](skills/index.md)")
        if design_doc is not None:
            root_lines.append("- [Design System](design/index.md)")
        indexes.append(OKFDocument(
            type="OKF Index",
            title=project_name,
            description="DevCouncil artifact graph exported as an OKF knowledge bundle.",
            timestamp=timestamp,
            body="\n".join(root_lines).strip(),
            rel_path="index.md",
        ))
        return indexes

    @staticmethod
    def generate(
        graph: ArtifactGraph,
        output_dir: Path,
        repo_map: RepoMap | None = None,
        project_name: str = "DevCouncil Project",
        timestamp: str = "",
        include_skills: bool = False,
        skills: "list | None" = None,
        include_design: bool = False,
        design=None,
    ) -> list[Path]:
        """Build the bundle and write it under ``output_dir``; returns written paths."""
        bundle = OKFBundleWriter.build(
            graph,
            repo_map,
            project_name,
            timestamp,
            include_skills=include_skills,
            skills=skills,
            include_design=include_design,
            design=design,
        )
        return write_bundle(bundle, output_dir)
