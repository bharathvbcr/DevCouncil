import typer
from devcouncil.cli.commands import (
    artifacts,
    baseline,
    config,
    doctor,
    hook,
    init,
    integrate,
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
)

app = typer.Typer(
    name="dev",
    help="DevCouncil: Gated orchestrator for AI-assisted software development.",
    add_completion=False,
)

# Typer subcommands (those using app = Typer())
app.add_typer(init.app, name="init")
app.add_typer(doctor.app, name="doctor")
app.add_typer(prompt.app, name="prompt")
app.add_typer(tasks.app, name="tasks")
app.add_typer(show.app, name="show")
app.add_typer(report.app, name="report")
app.add_typer(rollback.app, name="rollback")
app.add_typer(config.app, name="config")
app.add_typer(artifacts.app, name="artifacts")
app.add_typer(hook.app, name="hook")
app.add_typer(version.app, name="version")
app.add_typer(mcp_server.app, name="mcp-server")
app.add_typer(integrate.app, name="integrate")
app.add_typer(integrate.app, name="integrations")
app.add_typer(trace.app, name="trace")
app.add_typer(setup.app, name="setup")

# Direct command registrations (those defined as def cmd())
app.command(name="baseline")(baseline.baseline)
app.command(name="map")(map.map_repo)
app.command(name="plan")(plan.plan)
app.command(name="reset-demo-state")(reset_demo_state.reset_demo_state)
app.command(name="run")(run.run)
app.command(name="verify")(verify.verify)
app.command(name="repair")(repair.repair)
app.command(name="status")(status.status)

@app.callback()
def main():
    """
    DevCouncil: Gated orchestrator for AI-assisted software development.
    """
    pass

if __name__ == "__main__":
    app()
