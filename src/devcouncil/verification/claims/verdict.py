"""Merge claim check results into a corrective message / report."""

from __future__ import annotations

from dataclasses import dataclass

from devcouncil.verification.claims.models import CheckResult, Status


@dataclass
class ClaimVerdict:
    block: bool
    reason: str
    report: str


def render_report(results: list[CheckResult]) -> str:
    if not results:
        return ""
    lines = [
        "# DevCouncil stop-gate claim report",
        "",
        "| Claim | Check | Status |",
        "|---|---|---|",
    ]
    for r in results:
        claim = (r.assertion.source_text or r.assertion.kind.value).replace("|", "\\|")
        target = f" `{r.assertion.target}`" if r.assertion.target else ""
        lines.append(f"| {claim} | {r.assertion.kind.value}{target} | {r.status.name} |")
    lines.append("")
    for r in results:
        if r.detail:
            lines.append(f"## {r.assertion.kind.value} — {r.status.name}")
            lines.append(r.detail)
            lines.append("")
    return "\n".join(lines)


_STATUS_ICONS = {
    Status.PASS: "✓",
    Status.FAIL: "✗",
    Status.UNVERIFIABLE: "?",
    Status.SKIPPED: "–",
}


def summary_line(results: list[CheckResult], *, prefix: str = "🛡 devcouncil") -> str:
    """One-line verdict summary for Claude systemMessage."""
    if not results:
        return f"{prefix}: (no claims)"
    parts = []
    for r in results:
        icon = _STATUS_ICONS.get(r.status, "?")
        target = f" {r.assertion.target}" if r.assertion.target else ""
        parts.append(f"{r.assertion.kind.value}{target} {icon}")
    return f"{prefix}: " + " | ".join(parts)


def _corrective_message(failures: list[CheckResult]) -> str:
    bullets = []
    for r in failures:
        claim = r.assertion.source_text or r.assertion.kind.value
        bullets.append(f'- You claimed: "{claim}" — but verification found: {r.detail}')
    joined = "\n".join(bullets)
    return (
        "CLAIM VERIFICATION FAILED. DevCouncil re-checked your completion claims "
        "against the real environment and found discrepancies:\n\n"
        f"{joined}\n\n"
        "Do not repeat the claim. Investigate the filesystem, git, and command exit "
        "codes, fix the discrepancy, and only finish when these verifications would succeed."
    )


def decide_claims(results: list[CheckResult], blocks_so_far: int, max_blocks: int) -> ClaimVerdict:
    failures = [r for r in results if r.status is Status.FAIL]
    report = render_report(results)

    if not failures:
        return ClaimVerdict(block=False, reason="", report=report)

    if blocks_so_far >= max_blocks:
        report += (
            f"\n> Block cap reached ({blocks_so_far}/{max_blocks}): failures above were "
            "NOT re-injected into the agent. Review them yourself.\n"
        )
        return ClaimVerdict(block=False, reason="", report=report)

    return ClaimVerdict(block=True, reason=_corrective_message(failures), report=report)
