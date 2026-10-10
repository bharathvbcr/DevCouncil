import click

from pkg.core import Engine


@click.command()
@click.option("--n", default=1)
def main(n):
    """C17 console_scripts entry (pyproject [project.scripts]) + click command."""
    eng = Engine.create()
    eng.step()


@click.group()
def grp():
    pass


@grp.command("sub")
def sub_cmd():
    """C17 click subcommand registered by decorator only."""
    return Engine(2).run()


if __name__ == "__main__":
    main()
