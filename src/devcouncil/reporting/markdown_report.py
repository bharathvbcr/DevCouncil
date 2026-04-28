from devcouncil.artifacts.graph import ArtifactGraph

class MarkdownReportGenerator:
    """Generates a Markdown evidence report."""

    MAX_INLINE_GAPS = 25
    
    @staticmethod
    def generate(graph: ArtifactGraph, live_review: dict | None = None) -> str:
        summary = graph.coverage_summary()
        live_blockers = (live_review or {}).get("blocking_cards", [])

        md_output = "# DevCouncil Report\n\n"
        md_output += "## Verdict\n"
        if summary["blocking_gaps"] > 0 or live_blockers:
            parts = []
            if summary["blocking_gaps"] > 0:
                parts.append(f"{summary['blocking_gaps']} high-severity gap(s)")
            if live_blockers:
                parts.append(f"{len(live_blockers)} live-review blocker(s)")
            md_output += f"**Blocked**: {', '.join(parts)} remain.\n\n"
        else:
            md_output += "**Passed**: Ready for release.\n\n"

        md_output += "## Coverage Summary\n"
        md_output += f"- **Requirements**: {summary['total_requirements']} ({summary['requirements_without_tasks']} unmapped)\n"
        md_output += f"- **Tasks**: {summary['total_tasks']} ({summary['tasks_without_requirements']} orphaned)\n"
        md_output += f"- **Evidence**: {summary['total_ac'] - summary['ac_without_evidence']}/{summary['total_ac']} AC verified\n\n"

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
                
        return md_output
