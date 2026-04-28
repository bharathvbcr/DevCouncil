import typer
import yaml
from pathlib import Path
from rich.console import Console
from devcouncil.app.config import load_config

app = typer.Typer(help="Manage DevCouncil configuration")
console = Console()

@app.command("models")
def models(
    role: str = typer.Option(None, "--role", "-r", help="Specific role to show/edit"),
    model: str = typer.Option(None, "--model", "-m", help="New model string to set for the role")
):
    """View or edit model role configuration."""
    try:
        load_config(Path("."))
    except FileNotFoundError as e:
        console.print(f"[red]{e}[/red]")
        return

    config_path = Path(".devcouncil/config.yaml")
    
    with open(config_path) as f:
        raw_config = yaml.safe_load(f) or {}

    if not role:
        console.print("[bold]Model Configuration[/bold]")
        for r, m in raw_config.get("models", {}).get("roles", {}).items():
            console.print(f"  [cyan]{r}[/cyan]: {m.get('model')}")
        return

    if not model:
        m = raw_config.get("models", {}).get("roles", {}).get(role)
        if m:
            console.print(f"[cyan]{role}[/cyan]: {m.get('model')}")
        else:
            console.print(f"[red]Role '{role}' not found.[/red]")
        return

    if "models" not in raw_config:
        raw_config["models"] = {"roles": {}}
    if "roles" not in raw_config["models"]:
        raw_config["models"]["roles"] = {}
        
    if role not in raw_config["models"]["roles"]:
        raw_config["models"]["roles"][role] = {}
        
    raw_config["models"]["roles"][role]["model"] = model
    
    with open(config_path, "w") as f:
        yaml.dump(raw_config, f, default_flow_style=False)
        
    console.print(f"[green]Updated '{role}' to use model '{model}'[/green]")
