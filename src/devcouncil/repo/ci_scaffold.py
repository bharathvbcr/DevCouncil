"""Scaffold a starter GitHub Actions workflow for a target repository.

DevCouncil already knows a project's test/lint/typecheck commands (config.yaml), so
it can emit a sensible CI starter that runs them. The workflow is a *template* the
user can adjust; scaffolding never overwrites an existing workflow unless forced.
"""

from __future__ import annotations

from pathlib import Path

from devcouncil.app.config import DevCouncilConfig, load_config

WORKFLOW_RELPATH = Path(".github") / "workflows" / "devcouncil.yml"
EVIDENCE_WORKFLOW_RELPATH = Path(".github") / "workflows" / "devcouncil-evidence.yml"

_PYTHON_TOOLS = {
    "pytest", "flake8", "ruff", "mypy", "tox", "python", "python3", "uv",
    "poetry", "black", "isort", "pyright", "pip-audit",
}
_NODE_TOOLS = {
    "npm", "npx", "pnpm", "yarn", "bun", "eslint", "tsc", "jest", "vitest", "node",
}
_GO_TOOLS = {"go", "govulncheck"}
_RUST_TOOLS = {"cargo", "rustc", "rustfmt", "clippy-driver"}
_PYTHON_MARKERS = ("pyproject.toml", "requirements.txt", "setup.py", "setup.cfg", "Pipfile")
_UV_LOCK = "uv.lock"


def detect_stacks(project_root: Path) -> set[str]:
    """Best-effort detection of the language stacks present in the repo."""
    stacks: set[str] = set()
    if any((project_root / marker).exists() for marker in _PYTHON_MARKERS):
        stacks.add("python")
    if (project_root / _UV_LOCK).exists():
        stacks.add("python")
    if (project_root / "package.json").exists():
        stacks.add("node")
    if (project_root / "go.mod").exists():
        stacks.add("go")
    if (project_root / "Cargo.toml").exists():
        stacks.add("rust")
    return stacks


def _command_stack(command: str) -> str | None:
    tool = command.split()[0] if command.strip() else ""
    if tool in _PYTHON_TOOLS:
        return "python"
    if tool in _NODE_TOOLS:
        return "node"
    if tool in _GO_TOOLS:
        return "go"
    if tool in _RUST_TOOLS:
        return "rust"
    return None


def _applicable_commands(commands: list[str], stacks: set[str]) -> list[str]:
    """Keep commands whose tool matches a detected stack; if none detected, keep all."""
    if not stacks:
        return list(commands)
    kept = []
    for command in commands:
        stack = _command_stack(command)
        if stack is None or stack in stacks:
            kept.append(command)
    return kept


# Optional dependency-audit step per stack. Emitted only when the matching stack is
# detected, so a Python-only repo never gets an npm audit (and vice versa). These are
# non-blocking (continue-on-error) starters the user can tighten.
_AUDIT_STEPS: dict[str, list[str]] = {
    "python": [
        "      - name: Dependency audit (pip-audit)",
        "        continue-on-error: true",
        "        run: pip-audit",
    ],
    "node": [
        "      - name: Dependency audit (npm audit)",
        "        continue-on-error: true",
        "        run: npm audit --audit-level=high",
    ],
    "go": [
        "      - name: Dependency audit (govulncheck)",
        "        continue-on-error: true",
        "        run: govulncheck ./...",
    ],
    "rust": [
        "      - name: Dependency audit (cargo audit)",
        "        continue-on-error: true",
        "        run: cargo audit",
    ],
}


def _add_audit_steps(steps: list[str], stacks: set[str]) -> None:
    """Append an optional SCA audit step for each detected stack (only)."""
    for stack in sorted(stacks):
        steps.extend(_AUDIT_STEPS.get(stack, []))


def _python_version(project_root: Path) -> str:
    version_file = project_root / ".python-version"
    if version_file.exists():
        first = version_file.read_text(encoding="utf-8").strip().splitlines()
        if first and first[0].strip():
            return first[0].strip()
    return "3.12"


def _uses_uv(project_root: Path) -> bool:
    return (project_root / _UV_LOCK).exists()


def _default_install_command(project_root: Path) -> str:
    if _uses_uv(project_root):
        return f"uv sync --all-groups --python {_python_version(project_root)}"
    return "pip install -e ."


def _default_dev_command(project_root: Path) -> str:
    if _uses_uv(project_root):
        return f"uv run --python {_python_version(project_root)} dev"
    return "dev"


def _evidence_python_setup_steps(project_root: Path, *, install_command: str) -> list[str]:
    py_version = _python_version(project_root)
    if _uses_uv(project_root):
        return [
            "      - name: Install uv",
            "        uses: astral-sh/setup-uv@v7",
            "      - name: Set up Python",
            "        uses: actions/setup-python@v6",
            "        with:",
            f'          python-version: "{py_version}"',
            "      - name: Install DevCouncil",
            f"        run: {install_command}",
        ]
    return [
        "      - name: Set up Python",
        "        uses: actions/setup-python@v5",
        "        with:",
        f'          python-version: "{py_version}"',
        "      - name: Install DevCouncil",
        f"        run: {install_command}",
    ]


def render_workflow(
    project_root: Path,
    default_branch: str = "main",
    config: DevCouncilConfig | None = None,
) -> str:
    """Render the workflow YAML text deterministically from config + detected stacks."""
    if config is None:
        config = load_config(project_root)
    stacks = detect_stacks(project_root)
    commands = config.commands

    steps: list[str] = [
        "      - name: Checkout",
        "        uses: actions/checkout@v4",
    ]
    if "python" in stacks:
        steps += [
            "      - name: Set up Python",
            "        uses: actions/setup-python@v5",
            "        with:",
            f'          python-version: "{_python_version(project_root)}"',
        ]
    if "node" in stacks:
        steps += [
            "      - name: Set up Node",
            "        uses: actions/setup-node@v4",
            "        with:",
            '          node-version: "20"',
        ]
    if "go" in stacks:
        steps += [
            "      - name: Set up Go",
            "        uses: actions/setup-go@v5",
            "        with:",
            '          go-version: "stable"',
        ]
    if "rust" in stacks:
        steps += [
            "      - name: Set up Rust",
            "        uses: dtolnay/rust-toolchain@stable",
            "        with:",
            "          components: clippy",
        ]

    def add_command_steps(label: str, raw_commands: list[str]) -> None:
        for command in _applicable_commands(raw_commands, stacks):
            steps.append(f"      - name: {label} ({command.split()[0]})")
            steps.append(f"        run: {command}")

    add_command_steps("Lint", commands.lint)
    add_command_steps("Typecheck", commands.typecheck)
    add_command_steps("Test", commands.test)
    _add_audit_steps(steps, stacks)

    body = "\n".join(steps)
    return (
        "# Starter CI workflow generated by DevCouncil from .devcouncil/config.yaml.\n"
        "# Adjust the setup steps, dependency install, and commands for your stack.\n"
        "name: DevCouncil CI\n"
        "\n"
        "on:\n"
        "  push:\n"
        f'    branches: ["{default_branch}"]\n'
        "  pull_request:\n"
        f'    branches: ["{default_branch}"]\n'
        "\n"
        "jobs:\n"
        "  checks:\n"
        "    runs-on: ubuntu-latest\n"
        "    steps:\n"
        f"{body}\n"
    )


def render_evidence_workflow(
    project_root: Path,
    default_branch: str = "main",
    *,
    install_command: str | None = None,
    dev_command: str | None = None,
) -> str:
    """Render a PR evidence workflow: verify → JSON/HTML artifacts → optional GitHub integrations.

    Expects ``GITHUB_TOKEN`` (for Checks/comments), ``GITHUB_REPOSITORY``, ``GITHUB_SHA``,
    and ``GITHUB_PR_NUMBER`` (or ``PR_NUMBER``) when posting PR comments. The Check
    conclusion is blocking-only (``--fail-on-blocking``); advisory gaps stay in artifacts.
    """
    if install_command is None:
        install_command = _default_install_command(project_root)
    if dev_command is None:
        dev_command = _default_dev_command(project_root)

    stacks = detect_stacks(project_root)
    steps: list[str] = [
        "      - name: Checkout",
        "        uses: actions/checkout@v4",
        "        with:",
        "          fetch-depth: 0",
    ]
    if "python" in stacks or not stacks:
        steps += _evidence_python_setup_steps(project_root, install_command=install_command)
    steps += [
        "      - name: DevCouncil verify (scoped diff)",
        "        env:",
        "          # pull_request → base branch SHA; push → previous commit (github.event.before).",
        "          # New branch / first push: before is all zeros — skip verify (no diff base).",
        "          VERIFY_BASE: ${{ github.event.pull_request.base.sha || github.event.before }}",
        "        run: |",
        '          ZERO_SHA="0000000000000000000000000000000000000000"',
        '          if [ "$VERIFY_BASE" = "$ZERO_SHA" ]; then',
        '            VERIFY_BASE=""',
        "          fi",
        '          if [ -n "$VERIFY_BASE" ]; then',
        f"            {dev_command} check --verify --base \"$VERIFY_BASE\" --persist --project-root .",
        "          else",
        '            echo "::notice::Skipping DevCouncil verify — no diff base (first push / new branch). Push again or open a PR for scoped verify."',
        "            exit 0",
        "          fi",
        "      - name: Write evidence JSON artifact",
        f"        run: {dev_command} report --evidence-json .devcouncil/evidence.json --fail-on-blocking --project-root .",
        "      - name: Write evidence HTML artifact",
        f"        run: {dev_command} report --evidence-html .devcouncil/evidence.html --fail-on-blocking --project-root .",
        "      - name: Upload evidence artifacts",
        "        uses: actions/upload-artifact@v4",
        "        with:",
        '          name: devcouncil-evidence',
        "          path: |",
        "            .devcouncil/evidence.json",
        "            .devcouncil/evidence.html",
        "          if-no-files-found: error",
        "      - name: Post GitHub Check (blocking gaps only)",
        "        if: ${{ secrets.GITHUB_TOKEN != '' }}",
        "        env:",
        "          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}",
        "          GITHUB_REPOSITORY: ${{ github.repository }}",
        "          GITHUB_SHA: ${{ github.sha }}",
        f"        run: {dev_command} report --github --fail-on-blocking --project-root .",
        "      - name: Post PR comment (includes advisory gaps)",
        "        if: github.event_name == 'pull_request'",
        "        env:",
        "          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}",
        "          GITHUB_REPOSITORY: ${{ github.repository }}",
        "          GITHUB_SHA: ${{ github.sha }}",
        "          GITHUB_PR_NUMBER: ${{ github.event.pull_request.number }}",
        f"        run: {dev_command} report --github-pr-comment --fail-on-blocking --project-root .",
    ]
    body = "\n".join(steps)
    return (
        "# DevCouncil evidence workflow — verify, export artifacts, optional GitHub Check/comment.\n"
        "# Generated by DevCouncil; adjust install/dev commands for your stack.\n"
        "# Check conclusion fails only on blocking gaps; advisory gaps appear in artifacts/comments.\n"
        "name: DevCouncil Evidence\n"
        "\n"
        "permissions:\n"
        "  contents: read\n"
        "  checks: write\n"
        "  pull-requests: write\n"
        "\n"
        "on:\n"
        "  push:\n"
        f'    branches: ["{default_branch}"]\n'
        "  pull_request:\n"
        f'    branches: ["{default_branch}"]\n'
        "\n"
        "jobs:\n"
        "  evidence:\n"
        "    runs-on: ubuntu-latest\n"
        "    steps:\n"
        f"{body}\n"
    )


def scaffold_ci(project_root: Path, force: bool = False) -> Path | None:
    """Write the starter workflow. Returns the path, or None if one already exists.

    Does not overwrite an existing ``.github/workflows/devcouncil.yml`` unless
    ``force`` is set, so re-running is safe and user edits are preserved.
    """
    project_root = project_root.resolve()
    target = project_root / WORKFLOW_RELPATH
    if target.exists() and not force:
        return None
    config = load_config(project_root)
    default_branch = config.project.default_branch or "main"
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(
        render_workflow(project_root, default_branch, config), encoding="utf-8"
    )
    return target


def scaffold_evidence_ci(project_root: Path, force: bool = False) -> Path | None:
    """Write the evidence export workflow. Returns the path, or None if one already exists."""
    project_root = project_root.resolve()
    target = project_root / EVIDENCE_WORKFLOW_RELPATH
    if target.exists() and not force:
        return None
    config = load_config(project_root)
    default_branch = config.project.default_branch or "main"
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(
        render_evidence_workflow(
            project_root,
            default_branch,
            install_command=_default_install_command(project_root),
            dev_command=_default_dev_command(project_root),
        ),
        encoding="utf-8",
    )
    return target
