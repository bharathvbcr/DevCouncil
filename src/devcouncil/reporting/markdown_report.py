from devcouncil.artifacts.graph import ArtifactGraph

class MarkdownReportGenerator:
    """Generates a Markdown evidence report."""

    MAX_INLINE_GAPS = 25
    
    @staticmethod
    def generate(
        graph: ArtifactGraph,
        live_review: dict | None = None,
        wiki_refresh: dict | None = None,
    ) -> str:
        summary = graph.coverage_summary()
        live_blockers = (live_review or {}).get("blocking_cards", [])

        unverified_ac = summary["ac_without_evidence"]

        md_output = "# DevCouncil Report\n\n"
        md_output += "## Verdict\n"
        # Three honest states:
        #   Blocked    - positive evidence of a problem (blocking gaps / live blockers).
        #   Incomplete - nothing is failing, but not every acceptance criterion has
        #                passing evidence yet (un-run, or could not be verified). NOT
        #                a failure — distinguishing this from Blocked is what keeps the
        #                "blocked" signal trustworthy (no false negatives on correct work).
        #   Passed     - no blocking gaps and every acceptance criterion is proven.
        if summary["blocking_gaps"] > 0 or live_blockers:
            parts = []
            if summary["blocking_gaps"] > 0:
                parts.append(f"{summary['blocking_gaps']} high-severity gap(s)")
            if live_blockers:
                parts.append(f"{len(live_blockers)} live-review blocker(s)")
            md_output += f"**Blocked**: {', '.join(parts)} remain.\n\n"
        elif unverified_ac > 0:
            md_output += (
                f"**Incomplete**: nothing is failing, but {unverified_ac} acceptance "
                "criterion(s) lack passing evidence (un-run or unverifiable). Not ready "
                "for release.\n\n"
            )
        else:
            md_output += "**Passed**: Ready for release.\n\n"

        md_output += "## Coverage Summary\n"
        md_output += f"- **Requirements**: {summary['total_requirements']} ({summary['requirements_without_tasks']} unmapped)\n"
        md_output += f"- **Tasks**: {summary['total_tasks']} ({summary['tasks_without_requirements']} orphaned)\n"
        md_output += f"- **Evidence**: {summary['total_ac'] - summary['ac_without_evidence']}/{summary['total_ac']} AC verified\n"
        # Proof rigor: HOW the verified criteria were proven. Precise per-criterion checks
        # (compiled/vote) are trustworthy; ``coarse`` (a passing acceptance-capable command,
        # not a check tied to the criterion) is weak evidence worth flagging to a reader.
        proof_modes: dict[str, int] = {}
        for ev in getattr(graph, "test_evidence", []):
            if getattr(ev, "status", "") == "passed":
                proof_modes[getattr(ev, "mode", "") or "unspecified"] = (
                    proof_modes.get(getattr(ev, "mode", "") or "unspecified", 0) + 1
                )
        if proof_modes:
            rigor = ", ".join(f"{count} {mode}" for mode, count in sorted(proof_modes.items()))
            md_output += f"- **Proof rigor**: {rigor}\n"
        md_output += "\n"

        md_output += "## Requirements Coverage Table\n"
        md_output += "| Requirement | Task Mapping | Status |\n"
        md_output += "|---|---|---|\n"
        
        for req in graph.requirements.values():
            linked_tasks = [t for t in graph.tasks.values() if req.id in t.requirement_ids]
            task_str = ", ".join([t.id for t in linked_tasks]) if linked_tasks else "*None*"
            status_str = "Covered" if linked_tasks else "**Unmapped**"
            md_output += f"| {req.id} {req.title} | {task_str} | {status_str} |\n"
                    
        md_output += "\n## Blocking Gaps\n"
        blocking_gaps = graph.blocking_gaps()
        if not blocking_gaps:
            md_output += "None.\n"
        else:
            for gap in blocking_gaps[:MarkdownReportGenerator.MAX_INLINE_GAPS]:
                md_output += f"### {gap.id}: {gap.description}\n"
                md_output += f"**Recommended fix**: {gap.recommended_fix}\n\n"
            if len(blocking_gaps) > MarkdownReportGenerator.MAX_INLINE_GAPS:
                remaining = len(blocking_gaps) - MarkdownReportGenerator.MAX_INLINE_GAPS
                md_output += f"_Omitted {remaining} additional blocking gap(s). Use JSON output for the full list._\n"

        if live_review is not None:
            md_output += "\n## Live Review\n"
            cards = live_review.get("cards", {})
            md_output += f"- **Pending signals**: {live_review.get('pending_signals', 0)}\n"
            md_output += f"- **Open cards**: {cards.get('open', 0)}\n"
            md_output += f"- **Open critical cards**: {cards.get('critical_open', 0)}\n"
            if not live_blockers:
                md_output += "- **Blocking cards in scope**: None.\n"
            else:
                md_output += "\n### Blocking Live-Review Cards\n"
                for card in live_blockers[:MarkdownReportGenerator.MAX_INLINE_GAPS]:
                    md_output += f"- **{card['id']}**"
                    if card.get("task_id"):
                        md_output += f" (`{card['task_id']}`)"
                    md_output += f": {card['summary']}\n"

        if wiki_refresh is not None and wiki_refresh.get("considered"):
            md_output += "\n## Wiki Refresh\n"
            md_output += f"- **Reason**: {wiki_refresh.get('reason', '')}\n"
            stale = wiki_refresh.get("stale_pages") or []
            if stale:
                md_output += f"- **Stale pages**: {len(stale)}\n"
                for page in stale[:10]:
                    md_output += f"  - {page}\n"
                
        return md_output
