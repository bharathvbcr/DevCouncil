import sys

import typer


def _configure_stdio() -> None:
    """Make stdout/stderr resilient to non-cp1252 characters.

    Coding agents and rich output emit Unicode such as ``✓``. On Windows the
    default console / redirected-pipe encoding is cp1252, where an un-encodable
    character raises UnicodeEncodeError mid-write. Because Rich buffers output,
    that error can surface during an unrelated later write — which previously
    got misreported as a coding agent "failing to start". Reconfigure both
    streams to UTF-8 with replacement so output can never crash the process.
    """
    for stream in (sys.stdout, sys.stderr):
        reconfigure = getattr(stream, "reconfigure", None)
        if reconfigure is None:
            continue
        try:
            reconfigure(encoding="utf-8", errors="replace")
        except (ValueError, OSError):
            try:
                reconfigure(errors="replace")
            except (ValueError, OSError):
                pass


_configure_stdio()

from devcouncil.cli.commands import (  # noqa: E402 - imports follow stdio reconfiguration
    artifacts,
    agents,
    baseline,
    check,
    config,
    cost,
    ast,
    dashboard,
    design,
    doctor,
    go,
    hook,
    init,
    integrate,
    logs,
    lsp,
    map,
    graph_cmd,
    mcp_server,
    okf,
    plan,
    prompt,
    repair,
    report,
    reset_demo_state,
    rollback,
    run,
    runs,
    setup,
    campaign,
    show,
    status,
    gaps,
    requirements,
    export,
    tasks,
    trace,
    verify,
    version,
    watch,
    wiki,
    shell,
    semantic,
    evidence,
    handoff,
    skills,
    scaffold,
    provenance,
    lease,
    gated_write,
    task_gate,
)
from devcouncil.cli.commands.watch_fs import watch_fs  # noqa: E402 - imports follow stdio reconfiguration

app = typer.Typer(
    name="dev",
    help="DevCouncil: Gated orchestrator for AI-assisted software development.",
    add_completion=False,
)

# Typer subcommands (those using app = Typer())
app.add_typer(init.app, name="init")
app.add_typer(doctor.app, name="doctor")
app.add_typer(tasks.app, name="tasks")
app.add_typer(report.app, name="report")
app.add_typer(rollback.app, name="rollback")
app.add_typer(config.app, name="config")
app.add_typer(artifacts.app, name="artifacts")
app.add_typer(agents.app, name="agents")
app.add_typer(hook.app, name="hook")
app.add_typer(version.app, name="version")
app.add_typer(mcp_server.app, name="mcp-server")
app.add_typer(integrate.app, name="integrate")
app.add_typer(integrate.app, name="integrations")
app.add_typer(trace.app, name="trace")
app.add_typer(logs.app, name="logs")
app.add_typer(cost.app, name="cost")
app.add_typer(runs.app, name="runs")
app.add_typer(setup.app, name="setup")
app.add_typer(lsp.app, name="lsp")
app.add_typer(ast.app, name="ast")
app.add_typer(dashboard.app, name="dashboard")
app.add_typer(watch.app, name="watch")
app.add_typer(semantic.app, name="semantic")
app.add_typer(evidence.app, name="evidence")
app.add_typer(skills.app, name="skills")
app.add_typer(okf.app, name="okf")
app.add_typer(design.app, name="design")
app.add_typer(wiki.app, name="wiki")
app.add_typer(campaign.app, name="campaign")
app.add_typer(provenance.resource_app, name="resource")
app.add_typer(lease.lease_app, name="lease")
app.add_typer(task_gate.scope_app, name="scope")
watch.app.command("fs")(watch_fs)

# Direct command registrations (those defined as def cmd())
app.command(name="baseline")(baseline.baseline)
app.command(name="e2e")(go.go)
app.command(name="go")(go.go)
app.command(name="map")(map.map_repo)
app.add_typer(graph_cmd.app, name="graph")
app.command(name="graph-context")(map.graph_context_cmd)
app.command(name="scaffold-ci")(scaffold.scaffold_ci_command)
app.command(name="plan")(plan.plan)
app.command(name="approve")(plan.approve)
app.command(name="prompt")(prompt.prompt)
app.command(name="reset-demo-state")(reset_demo_state.reset_demo_state)
app.command(name="run")(run.run)
# shell/handoff take a positional TASK_ID followed by options, so they must be
# plain commands — as typer sub-apps (click groups) the documented
# `dev shell TASK-001 --command ...` form fails to parse.
app.command(name="shell")(shell.shell)
app.command(name="handoff")(handoff.handoff)
app.command(name="show")(show.show)
app.command(name="verify")(verify.verify)
app.command(name="check")(check.check)
app.command(name="repair")(repair.repair)
app.command(name="status")(status.status)
app.command(name="gaps")(gaps.gaps)
app.command(name="provenance")(provenance.provenance)
app.command(name="checkout")(lease.checkout)
app.command(name="release")(lease.release)
app.command(name="write")(gated_write.write)
app.command(name="apply-patch")(gated_write.apply_patch)
app.command(name="next-task")(task_gate.next_task)
app.command(name="policy-check")(task_gate.policy_check)
app.command(name="record-command")(task_gate.record_command)
app.command(name="run-cmd")(task_gate.run_cmd)
app.command(name="verify-leased")(task_gate.verify_leased)
app.command(name="evidence-append")(task_gate.evidence_append)
app.command(name="evidence-list")(task_gate.evidence_list)
app.command(name="handoff-leased")(task_gate.handoff_leased)
app.command(name="requirements")(requirements.requirements)
app.command(name="export")(export.export_state)
app.command(name="optimize")(agents.optimize_agent)

@app.callback()
def main(
    ctx: typer.Context,
    verbose: int = typer.Option(
        0,
        "--verbose",
        "-v",
        count=True,
        help="Increase console log verbosity (-v INFO, -vv DEBUG). Everything is "
        "always captured at DEBUG in .devcouncil/logs/devcouncil.log.",
    ),
    quiet: bool = typer.Option(
        False,
        "--quiet",
        "-q",
        help="Only show errors on the console (the log file still captures everything).",
    ),
    log_level: str = typer.Option(
        None,
        "--log-level",
        help="Explicit console log level (DEBUG/INFO/WARNING/ERROR). Overrides -v/-q "
        "and the DEVCOUNCIL_LOG_LEVEL env var.",
    ),
):
    """
    DevCouncil: Gated orchestrator for AI-assisted software development.
    """
    # Configure logging once, up front, for every command. Without this the many
    # logger.info/debug calls across the orchestrator, planner, executors and
    # verifier go nowhere — which is exactly why recurring run failures were so
    # hard to diagnose. The durable DEBUG log lands in .devcouncil/logs/.
    from devcouncil.telemetry.logging_setup import configure_logging

    configure_logging(verbosity=verbose, quiet=quiet, log_level=log_level)
    return

if __name__ == "__main__":
    app()
