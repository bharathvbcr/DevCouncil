from __future__ import annotations

import json
import secrets
import time
from importlib import resources
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlparse

from devcouncil.app.project_status import compute_phase
from devcouncil.integrations.actions import apply_integration_target
from devcouncil.integrations.check import build_integration_check_report, integration_status_summary
from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import ArtifactGraphRepository, StateRepository, TaskRepository
from devcouncil.telemetry.traces import read_trace_events

LOGO_ASSET = "devcouncil_logo_premium.png"
LEGACY_LOGO_ASSET = "devcouncil-logo.svg"


# mtime-keyed cache so the dashboard's poll loop doesn't re-read and re-parse
# every run manifest on each refresh.
_RUN_MANIFEST_CACHE: dict[str, tuple[float, dict]] = {}


def _load_run_manifest(manifest_path: Path) -> dict | None:
    key = str(manifest_path)
    try:
        mtime = manifest_path.stat().st_mtime
    except OSError:
        return None
    cached = _RUN_MANIFEST_CACHE.get(key)
    if cached is not None and cached[0] == mtime:
        return dict(cached[1])
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8")) or {}
    except (json.JSONDecodeError, OSError):
        return None
    _RUN_MANIFEST_CACHE[key] = (mtime, manifest)
    return dict(manifest)


def recent_run_artifacts(project_root: Path, *, limit: int = 10) -> list[dict]:
    runs_dir = project_root / ".devcouncil" / "runs"
    if not runs_dir.exists():
        return []
    manifests: list[dict] = []
    for manifest_path in sorted(
        runs_dir.glob("*/agent-run.json"),
        key=lambda path: path.stat().st_mtime,
        reverse=True,
    ):
        manifest = _load_run_manifest(manifest_path)
        if manifest is None:
            continue
        manifest["manifest_path"] = str(manifest_path)
        manifests.append(manifest)
        if len(manifests) >= limit:
            break
    return manifests


# Integration status probes the filesystem (and optionally CLIs) — far too
# expensive to recompute on every 2-second dashboard poll.
_INTEGRATION_SUMMARY_TTL_SECONDS = 5.0
_INTEGRATION_SUMMARY_CACHE: dict[str, tuple[float, dict]] = {}


def _integration_summary_cached(project_root: Path) -> dict:
    key = str(project_root)
    now = time.monotonic()
    cached = _INTEGRATION_SUMMARY_CACHE.get(key)
    if cached is not None and now - cached[0] < _INTEGRATION_SUMMARY_TTL_SECONDS:
        return cached[1]
    summary = integration_status_summary(project_root)
    _INTEGRATION_SUMMARY_CACHE[key] = (now, summary)
    return summary


def _invalidate_integration_summary(project_root: Path) -> None:
    _INTEGRATION_SUMMARY_CACHE.pop(str(project_root), None)


def logo_svg() -> str:
    return resources.files("devcouncil.assets").joinpath(LEGACY_LOGO_ASSET).read_text(encoding="utf-8")


def logo_asset_bytes() -> bytes:
    return resources.files("devcouncil.assets").joinpath(LOGO_ASSET).read_bytes()


def dashboard_payload(project_root: Path) -> dict:
    db = get_db(project_root)
    if not db:
        return {
            "initialized": False,
            "phase": "UNINITIALIZED",
            "tasks": [],
            "coverage": {},
            "events": [],
            "integrations": _integration_summary_cached(project_root),
            "recent_runs": recent_run_artifacts(project_root),
        }
    with db.get_session() as session:
        graph = ArtifactGraphRepository(session).load_graph()
        state = StateRepository(session).get_state()
        phase = compute_phase(graph, state.current_phase if state else None)
        tasks = [task.model_dump() for task in TaskRepository(session).get_all()]
        return {
            "initialized": True,
            "phase": phase,
            "coverage": graph.coverage_summary(),
            "tasks": tasks,
            "events": [event.model_dump(by_alias=True) for event in list(read_trace_events(project_root))[-50:]],
            "integrations": _integration_summary_cached(project_root),
            "recent_runs": recent_run_artifacts(project_root),
        }


def _json_response(handler: BaseHTTPRequestHandler, status: int, payload: dict) -> None:
    body = json.dumps(payload).encode("utf-8")
    handler.send_response(status)
    handler.send_header("Content-Type", "application/json")
    handler.send_header("Content-Length", str(len(body)))
    handler.end_headers()
    handler.wfile.write(body)


def _is_loopback_client(handler: BaseHTTPRequestHandler) -> bool:
    host = handler.client_address[0] if handler.client_address else ""
    return host in {"127.0.0.1", "::1", "localhost"}


def dashboard_apply_payload(
    project_root: Path,
    raw_body: bytes,
    *,
    token: str,
    provided_token: str | None,
) -> dict:
    if not token or provided_token != token:
        return {"ok": False, "error": "invalid dashboard token"}
    try:
        request = json.loads(raw_body.decode("utf-8") or "{}")
    except json.JSONDecodeError:
        return {"ok": False, "error": "invalid JSON body"}
    if not isinstance(request, dict):
        return {"ok": False, "error": "request body must be a JSON object"}
    target = str(request.get("target") or "").strip()
    include_hooks = bool(request.get("include_hooks", True))
    strict = bool(request.get("strict", False))
    try:
        report = apply_integration_target(
            project_root,
            target,
            include_hooks=include_hooks,
            strict=strict,
        )
    except ValueError as exc:
        return {"ok": False, "error": str(exc)}
    # The next poll should reflect the freshly applied integration.
    _invalidate_integration_summary(project_root)
    return report.as_dict()


def dashboard_html(token: str = "") -> str:
    safe_token = token.replace('"', "")
    return f"""<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <meta name="devcouncil-dashboard-token" content="{safe_token}">
  <title>DevCouncil Dashboard</title>
  <style>
    body {{ margin: 0; font-family: ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; background: #f7f7f5; color: #202124; }}
    header {{ padding: 18px 28px; border-bottom: 1px solid #d9d9d4; background: #ffffff; display: flex; align-items: center; justify-content: space-between; gap: 18px; }}
    main {{ padding: 24px 28px; display: grid; grid-template-columns: 1fr 1fr; gap: 18px; }}
    section {{ background: #ffffff; border: 1px solid #d9d9d4; border-radius: 8px; padding: 16px; }}
    h1 {{ font-size: 20px; margin: 0; }}
    h2 {{ font-size: 15px; margin: 0 0 12px; }}
    .brand {{ display: flex; align-items: center; gap: 12px; min-width: 0; }}
    .brand img {{ width: 54px; height: 54px; flex: 0 0 auto; filter: drop-shadow(0 8px 12px rgba(12, 36, 52, 0.22)); }}
    .phase-line {{ white-space: nowrap; }}
    .phase {{ font-weight: 700; }}
    table {{ width: 100%; border-collapse: collapse; font-size: 13px; }}
    th, td {{ text-align: left; border-bottom: 1px solid #ededeb; padding: 8px; vertical-align: top; }}
    pre {{ margin: 0; white-space: pre-wrap; font-size: 12px; }}
    .toolbar {{ display: flex; flex-wrap: wrap; gap: 8px; margin-bottom: 12px; }}
    button {{ border: 1px solid #b9b9b2; background: #ffffff; color: #202124; border-radius: 6px; padding: 6px 10px; font: inherit; cursor: pointer; }}
    button:disabled {{ color: #898984; cursor: default; background: #f3f3f0; }}
    .status-pill {{ display: inline-block; min-width: 58px; padding: 2px 6px; border-radius: 999px; font-size: 12px; text-align: center; background: #ededeb; }}
    .status-ok {{ background: #dff3e4; color: #155724; }}
    .status-warn {{ background: #fff3cd; color: #664d03; }}
    .status-fail {{ background: #f8d7da; color: #842029; }}
    #integration-result {{ margin-top: 10px; font-size: 12px; }}
    @media (max-width: 800px) {{ main {{ grid-template-columns: 1fr; padding: 16px; }} header {{ padding: 16px; }} }}
  </style>
</head>
<body>
  <header><div class="brand"><img src="/assets/devcouncil_logo_premium.png" alt="DevCouncil logo"><h1>DevCouncil Dashboard</h1></div><div class="phase-line">Phase: <span id="phase" class="phase">loading</span></div></header>
  <main>
    <section><h2>Coverage</h2><pre id="coverage">{{}}</pre></section>
    <section><h2>Tasks</h2><table><thead><tr><th>ID</th><th>Status</th><th>Title</th></tr></thead><tbody id="tasks"></tbody></table></section>
    <section style="grid-column: 1 / -1;">
      <h2>CLI Integrations</h2>
      <div class="toolbar">
        <button type="button" data-target="all">Apply Detected</button>
        <button type="button" data-target="hooks">Install Hooks</button>
        <button type="button" id="run-check">Run Check</button>
      </div>
      <pre id="integration-result"></pre>
      <table>
        <thead><tr><th>Client</th><th>PATH</th><th>MCP</th><th>Hooks</th><th>Launcher</th><th>Configured</th><th>Notes</th><th>Action</th></tr></thead>
        <tbody id="integrations"></tbody>
      </table>
    </section>
    <section style="grid-column: 1 / -1;">
      <h2>Recent Agent Runs</h2>
      <table>
        <thead><tr><th>Run</th><th>Task</th><th>Agent</th><th>Status</th><th>Transcript</th></tr></thead>
        <tbody id="runs"></tbody>
      </table>
    </section>
    <section style="grid-column: 1 / -1;"><h2>Recent Trace Events</h2><pre id="events"></pre></section>
  </main>
  <script>
    function setText(cell, value) {{
      cell.textContent = value == null ? '' : String(value);
      return cell;
    }}

    function statusClass(value) {{
      if (value === 'ok') return 'status-pill status-ok';
      if (value === 'missing' || value === 'drifted') return 'status-pill status-warn';
      return 'status-pill';
    }}

    function statusCell(value) {{
      const cell = document.createElement('td');
      const pill = document.createElement('span');
      pill.className = statusClass(value);
      pill.textContent = value || 'n/a';
      cell.appendChild(pill);
      return cell;
    }}

    const token = document.querySelector('meta[name="devcouncil-dashboard-token"]').content;

    async function applyIntegration(target) {{
      const resultBox = document.getElementById('integration-result');
      resultBox.textContent = `Running ${{target}}...`;
      const res = await fetch('/api/integrations/apply', {{
        method: 'POST',
        headers: {{
          'Content-Type': 'application/json',
          'X-DevCouncil-Dashboard-Token': token,
        }},
        body: JSON.stringify({{ target }}),
      }});
      const payload = await res.json();
      resultBox.textContent = JSON.stringify(payload, null, 2);
      await refresh();
    }}

    async function runIntegrationCheck() {{
      const resultBox = document.getElementById('integration-result');
      const res = await fetch('/api/integrations/check');
      const payload = await res.json();
      resultBox.textContent = JSON.stringify(payload, null, 2);
    }}

    async function refresh() {{
      const res = await fetch('/api/status');
      const data = await res.json();
      document.getElementById('phase').textContent = data.phase;
      document.getElementById('coverage').textContent = JSON.stringify(data.coverage, null, 2);
      const body = document.getElementById('tasks');
      body.replaceChildren(...(data.tasks || []).map(t => {{
        const row = document.createElement('tr');
        row.appendChild(setText(document.createElement('td'), t.id));
        row.appendChild(setText(document.createElement('td'), t.status));
        row.appendChild(setText(document.createElement('td'), t.title));
        return row;
      }}));
      const integrationsBody = document.getElementById('integrations');
      const capabilities = ((data.integrations || {{}}).capabilities || []);
      integrationsBody.replaceChildren(...capabilities.map(item => {{
        const row = document.createElement('tr');
        row.appendChild(setText(document.createElement('td'), item.label || item.name));
        row.appendChild(setText(document.createElement('td'), item.on_path ? 'yes' : 'no'));
        row.appendChild(setText(document.createElement('td'), item.mcp ? 'yes' : 'no'));
        row.appendChild(setText(document.createElement('td'), item.hooks ? 'yes' : 'verification'));
        row.appendChild(setText(document.createElement('td'), item.launcher_shim ? 'yes' : 'no'));
        row.appendChild(statusCell(item.config_status));
        row.appendChild(setText(document.createElement('td'), item.notes || ''));
        const action = document.createElement('td');
        const button = document.createElement('button');
        button.type = 'button';
        button.dataset.target = item.apply_target || item.name;
        button.textContent = item.config_status === 'ok' ? 'Reapply' : 'Fix';
        button.disabled = !item.fixable;
        action.appendChild(button);
        row.appendChild(action);
        return row;
      }}));

      const runsBody = document.getElementById('runs');
      runsBody.replaceChildren(...(data.recent_runs || []).map(run => {{
        const row = document.createElement('tr');
        row.appendChild(setText(document.createElement('td'), run.run_id));
        row.appendChild(setText(document.createElement('td'), run.task_id));
        row.appendChild(setText(document.createElement('td'), run.agent));
        row.appendChild(setText(document.createElement('td'), run.status || 'unknown'));
        row.appendChild(setText(document.createElement('td'), run.transcript || ''));
        return row;
      }}));

      document.getElementById('events').textContent = JSON.stringify(data.events || [], null, 2);
    }}

    document.addEventListener('click', event => {{
      const button = event.target;
      if (!(button instanceof HTMLButtonElement)) return;
      if (button.id === 'run-check') {{
        runIntegrationCheck();
        return;
      }}
      const target = button.dataset.target;
      if (target) applyIntegration(target);
    }});

    refresh();
    setInterval(refresh, 2000);
  </script>
</body>
</html>"""


def run_dashboard(project_root: Path, host: str = "127.0.0.1", port: int = 8765) -> None:
    dashboard_token = secrets.token_urlsafe(24)

    class DashboardServer(ThreadingHTTPServer):
        allow_reuse_address = True
        daemon_threads = True

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):  # noqa: N802
            parsed = urlparse(self.path)
            if parsed.path == f"/assets/{LOGO_ASSET}":
                body = logo_asset_bytes()
                self.send_response(200)
                self.send_header("Content-Type", "image/png")
                self.send_header("Cache-Control", "public, max-age=3600")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
                return
            if parsed.path == f"/assets/{LEGACY_LOGO_ASSET}":
                body = logo_svg().encode("utf-8")
                self.send_response(200)
                self.send_header("Content-Type", "image/svg+xml; charset=utf-8")
                self.send_header("Cache-Control", "public, max-age=3600")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
                return
            if parsed.path == "/api/integrations/check":
                body = json.dumps(build_integration_check_report(project_root).as_dict()).encode("utf-8")
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
                return
            if parsed.path == "/api/status":
                body = json.dumps(dashboard_payload(project_root)).encode("utf-8")
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
                return
            if parsed.path.startswith("/api/"):
                body = json.dumps({"error": "Not found"}).encode("utf-8")
                self.send_response(404)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
                return
            body = dashboard_html(dashboard_token).encode("utf-8")
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def do_POST(self):  # noqa: N802
            parsed = urlparse(self.path)
            if parsed.path != "/api/integrations/apply":
                _json_response(self, 404, {"ok": False, "error": "Not found"})
                return
            if not _is_loopback_client(self):
                _json_response(self, 403, {"ok": False, "error": "dashboard mutations require loopback client"})
                return
            length = int(self.headers.get("Content-Length") or "0")
            raw_body = self.rfile.read(length)
            provided = self.headers.get("X-DevCouncil-Dashboard-Token")
            payload = dashboard_apply_payload(
                project_root,
                raw_body,
                token=dashboard_token,
                provided_token=provided,
            )
            _json_response(self, 200 if payload.get("ok") else 403, payload)

        def log_message(self, format, *args):  # noqa: A002
            return

    server = DashboardServer((host, port), Handler)
    server.serve_forever()
