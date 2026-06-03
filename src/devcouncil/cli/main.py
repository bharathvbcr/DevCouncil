import typer

from devcouncil.cli.commands import (
    artifacts,
    agents,
    baseline,
    config,
    ast,
    dashboard,
    doctor,
    go,
    hook,
    init,
    integrate,
    lsp,
    map,
    mcp_server,
    plan,
    prompt,
    repair,
    report,
    reset_demo_state,
    rollback,
    run,
    setup,
    show,
    status,
    tasks,
    trace,
    verify,
    version,
    watch,
    shell,
    semantic,
    evidence,
    handoff,
)
from devcouncil.cli.commands.watch_fs import watch_fs

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
app.add_typer(setup.app, name="setup")
app.add_typer(lsp.app, name="lsp")
app.add_typer(ast.app, name="ast")
app.add_typer(dashboard.app, name="dashboard")
app.add_typer(watch.app, name="watch")
app.add_typer(shell.app, name="shell")
app.add_typer(semantic.app, name="semantic")
app.add_typer(evidence.app, name="evidence")
app.add_typer(handoff.app, name="handoff")
watch.app.command("fs")(watch_fs)

# Direct command registrations (those defined as def cmd())
app.command(name="baseline")(baseline.baseline)
app.command(name="e2e")(go.go)
app.command(name="go")(go.go)
app.command(name="map")(map.map_repo)
app.command(name="plan")(plan.plan)
app.command(name="prompt")(prompt.prompt)
app.command(name="reset-demo-state")(reset_demo_state.reset_demo_state)
app.command(name="run")(run.run)
app.command(name="show")(show.show)
app.command(name="verify")(verify.verify)
app.command(name="repair")(repair.repair)
app.command(name="status")(status.status)
app.command(name="optimize")(agents.optimize_agent)

@app.callback()
def main(ctx: typer.Context):
    """
    DevCouncil: Gated orchestrator for AI-assisted software development.
    """
    return

if __name__ == "__main__":
    app()
