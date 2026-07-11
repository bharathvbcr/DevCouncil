"""Self-contained HTML renderer for the requirement→task→diff→evidence graph.

Companion to :mod:`devcouncil.reporting.evidence_export` — same reviewer-facing
content, browsable in CI artifact previews without installing DevCouncil.
Written by ``dev report --evidence-html PATH``.
"""

from __future__ import annotations

import html
from datetime import datetime, timezone
from typing import Any

from devcouncil.artifacts.graph import ArtifactGraph
from devcouncil.reporting.evidence_export import EvidenceExportGenerator

_STYLE = """
body{font:16px/1.5 -apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,sans-serif;
max-width:64rem;margin:2rem auto;padding:0 1rem;color:#1a1a1a;background:#fff}
h1,h2,h3{line-height:1.25}a{color:#0b66c3}table{border-collapse:collapse;width:100%;margin:1rem 0}
th,td{border:1px solid #ddd;padding:.45rem .6rem;text-align:left;vertical-align:top}
th{background:#f6f8fa}.meta{color:#555;font-size:.92em;margin:.5rem 0}
.verdict-passed{color:#116329}.verdict-incomplete{color:#9a6700}.verdict-blocked{color:#cf222e}
.badge{display:inline-block;border-radius:999px;padding:.1em .55em;font-size:.78em;font-weight:600}
.badge-pass{background:#dafbe1;color:#116329}.badge-fail{background:#ffebe9;color:#cf222e}
.badge-warn{background:#fff8c5;color:#9a6700}.badge-advisory{background:#eef3fb;color:#0b66c3}
.gap-blocking{font-weight:600}.section{margin:2rem 0 1rem}
ul.compact{margin:.3rem 0;padding-left:1.2rem}code{background:#f3f3f3;padding:.1em .3em;border-radius:3px}
""".strip()


def _esc(text: object) -> str:
    return html.escape(str(text), quote=True)


def _verdict_class(verdict: str) -> str:
    return {
        "passed": "verdict-passed",
        "incomplete": "verdict-incomplete",
        "blocked": "verdict-blocked",
    }.get(verdict, "")


def _status_badge(status: str) -> str:
    cls = {
        "passed": "badge-pass",
        "failed": "badge-fail",
        "not_run": "badge-warn",
    }.get(status, "badge-warn")
    return f'<span class="badge {cls}">{_esc(status)}</span>'


def _proven_badge(proven: bool) -> str:
    if proven:
        return '<span class="badge badge-pass">proven</span>'
    return '<span class="badge badge-warn">unproven</span>'


def _render_gaps(gaps: list[dict[str, Any]]) -> str:
    if not gaps:
        return "<p>No gaps recorded.</p>"
    rows: list[str] = []
    for gap in gaps:
        blocking = gap.get("blocking", False)
        severity = gap.get("severity", "")
        row_cls = "gap-blocking" if blocking else ""
        kind = "blocking" if blocking else "advisory"
        badge = "badge-fail" if blocking else "badge-advisory"
        rows.append(
            "<tr>"
            f'<td class="{row_cls}"><span class="badge {badge}">{_esc(kind)}</span></td>'
            f"<td>{_esc(gap.get('id', ''))}</td>"
            f"<td>{_esc(severity)}</td>"
            f"<td>{_esc(gap.get('description', ''))}</td>"
            f"<td>{_esc(gap.get('task_id', '') or '—')}</td>"
            "</tr>"
        )
    return (
        "<table><thead><tr><th>Kind</th><th>ID</th><th>Severity</th>"
        "<th>Description</th><th>Task</th></tr></thead><tbody>"
        + "".join(rows)
        + "</tbody></table>"
    )


def _render_requirements(requirements: list[dict[str, Any]]) -> str:
    parts: list[str] = []
    for req in requirements:
        parts.append(f"<h3 id=\"{_esc(req.get('id', ''))}\">{_esc(req.get('id', ''))}: {_esc(req.get('title', ''))}</h3>")
        parts.append(f"<p class=\"meta\">{_esc(req.get('description', ''))}</p>")
        task_ids = req.get("task_ids") or []
        if task_ids:
            links = ", ".join(f'<a href="#task-{_esc(tid)}">{_esc(tid)}</a>' for tid in task_ids)
            parts.append(f"<p class=\"meta\">Tasks: {links}</p>")
        parts.append(
            "<table><thead><tr><th>AC</th><th>Description</th><th>Method</th>"
            "<th>Status</th><th>Evidence</th></tr></thead><tbody>"
        )
        for ac in req.get("acceptance_criteria") or []:
            evidence_items = ac.get("evidence") or []
            if evidence_items:
                ev_html = "<ul class=\"compact\">" + "".join(
                    "<li>"
                    f"{_status_badge(ev.get('status', ''))} "
                    f"<code>{_esc(ev.get('command', ''))}</code> — {_esc(ev.get('summary', ''))}"
                    "</li>"
                    for ev in evidence_items
                ) + "</ul>"
            else:
                ev_html = "<em>none</em>"
            parts.append(
                "<tr>"
                f"<td>{_esc(ac.get('id', ''))}</td>"
                f"<td>{_esc(ac.get('description', ''))}</td>"
                f"<td>{_esc(ac.get('verification_method', ''))}</td>"
                f"<td>{_proven_badge(bool(ac.get('proven')))}</td>"
                f"<td>{ev_html}</td>"
                "</tr>"
            )
        parts.append("</tbody></table>")
    return "\n".join(parts)


def _render_tasks(tasks: list[dict[str, Any]]) -> str:
    parts: list[str] = []
    for task in tasks:
        tid = task.get("id", "")
        parts.append(
            f"<h3 id=\"task-{_esc(tid)}\">{_esc(tid)}: {_esc(task.get('title', ''))}</h3>"
        )
        parts.append(
            f"<p class=\"meta\">Status: {_esc(task.get('status', ''))} · "
            f"Requirements: {_esc(', '.join(task.get('requirement_ids') or []))}</p>"
        )
        diffs = task.get("diffs") or []
        if not diffs:
            parts.append("<p class=\"meta\">No diff evidence linked.</p>")
            continue
        parts.append("<ul class=\"compact\">")
        for diff in diffs:
            changed = diff.get("changed_files") or []
            added = diff.get("added_files") or []
            deleted = diff.get("deleted_files") or []
            parts.append("<li>")
            if changed:
                parts.append(f"Changed: {_esc(', '.join(changed))}<br>")
            if added:
                parts.append(f"Added: {_esc(', '.join(added))}<br>")
            if deleted:
                parts.append(f"Deleted: {_esc(', '.join(deleted))}<br>")
            parts.append(_esc(diff.get("diff_summary", "")))
            parts.append("</li>")
        parts.append("</ul>")
    return "\n".join(parts)


class EvidenceHtmlGenerator:
    """Renders the artifact graph as a single self-contained HTML document."""

    FORMAT = "devcouncil-evidence-html"
    VERSION = 1

    @classmethod
    def generate(
        cls,
        graph: ArtifactGraph,
        live_review: dict | None = None,
        wiki_refresh: dict | None = None,
    ) -> str:
        payload = EvidenceExportGenerator.generate(
            graph, live_review=live_review, wiki_refresh=wiki_refresh
        )
        import json

        data = json.loads(payload)
        verdict = data.get("verdict", "incomplete")
        summary = data.get("coverage_summary") or {}
        generated_at = data.get("generated_at") or datetime.now(timezone.utc).isoformat()

        body = [
            "<!DOCTYPE html>",
            '<html lang="en">',
            "<head>",
            '<meta charset="utf-8">',
            "<title>DevCouncil Evidence Report</title>",
            f"<style>{_STYLE}</style>",
            "</head>",
            "<body>",
            "<h1>DevCouncil Evidence Report</h1>",
            f'<p class="meta">Generated {_esc(generated_at)} · format {_esc(cls.FORMAT)} v{cls.VERSION}</p>',
            f'<p class="{_verdict_class(verdict)}"><strong>Verdict:</strong> {_esc(verdict)}</p>',
            "<p class=\"meta\">"
            f"Requirements: {_esc(summary.get('total_requirements', 0))} · "
            f"Tasks: {_esc(summary.get('total_tasks', 0))} · "
            f"Blocking gaps: {_esc(summary.get('blocking_gaps', 0))} · "
            f"AC without evidence: {_esc(summary.get('ac_without_evidence', 0))}"
            "</p>",
            '<div class="section"><h2>Gaps</h2>',
            "<p class=\"meta\">GitHub Checks fail only on blocking gaps; advisory gaps appear here for reviewers.</p>",
            _render_gaps(data.get("gaps") or []),
            "</div>",
            '<div class="section"><h2>Requirements &amp; Acceptance Criteria</h2>',
            _render_requirements(data.get("requirements") or []),
            "</div>",
            '<div class="section"><h2>Tasks &amp; Diffs</h2>',
            _render_tasks(data.get("tasks") or []),
            "</div>",
            "</body></html>",
        ]
        return "\n".join(body)
