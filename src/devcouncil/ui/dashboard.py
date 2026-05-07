from __future__ import annotations

import json
from importlib import resources
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlparse

from devcouncil.app.project_status import compute_phase
from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import ArtifactGraphRepository, StateRepository, TaskRepository
from devcouncil.telemetry.traces import read_trace_events

LOGO_ASSET = "devcouncil-logo.svg"


def logo_svg() -> str:
    return resources.files("devcouncil.assets").joinpath(LOGO_ASSET).read_text(encoding="utf-8")


def dashboard_payload(project_root: Path) -> dict:
    db = get_db(project_root)
    if not db:
        return {"initialized": False, "phase": "UNINITIALIZED", "tasks": [], "coverage": {}, "events": []}
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
        }


def dashboard_html() -> str:
    return """<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>DevCouncil Dashboard</title>
  <style>
    body { margin: 0; font-family: ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; background: #f7f7f5; color: #202124; }
    header { padding: 18px 28px; border-bottom: 1px solid #d9d9d4; background: #ffffff; display: flex; align-items: center; justify-content: space-between; gap: 18px; }
    main { padding: 24px 28px; display: grid; grid-template-columns: 1fr 1fr; gap: 18px; }
    section { background: #ffffff; border: 1px solid #d9d9d4; border-radius: 8px; padding: 16px; }
    h1 { font-size: 20px; margin: 0; }
    h2 { font-size: 15px; margin: 0 0 12px; }
    .brand { display: flex; align-items: center; gap: 12px; min-width: 0; }
    .brand img { width: 54px; height: 54px; flex: 0 0 auto; filter: drop-shadow(0 8px 12px rgba(12, 36, 52, 0.22)); }
    .phase-line { white-space: nowrap; }
    .phase { font-weight: 700; }
    table { width: 100%; border-collapse: collapse; font-size: 13px; }
    th, td { text-align: left; border-bottom: 1px solid #ededeb; padding: 8px; vertical-align: top; }
    pre { margin: 0; white-space: pre-wrap; font-size: 12px; }
    @media (max-width: 800px) { main { grid-template-columns: 1fr; padding: 16px; } header { padding: 16px; } }
  </style>
</head>
<body>
  <header><div class="brand"><img src="/assets/devcouncil-logo.svg" alt="DevCouncil logo"><h1>DevCouncil Dashboard</h1></div><div class="phase-line">Phase: <span id="phase" class="phase">loading</span></div></header>
  <main>
    <section><h2>Coverage</h2><pre id="coverage">{}</pre></section>
    <section><h2>Tasks</h2><table><thead><tr><th>ID</th><th>Status</th><th>Title</th></tr></thead><tbody id="tasks"></tbody></table></section>
    <section style="grid-column: 1 / -1;"><h2>Recent Trace Events</h2><pre id="events"></pre></section>
  </main>
  <script>
    function setText(cell, value) {
      cell.textContent = value == null ? '' : String(value);
      return cell;
    }
    async function refresh() {
      const res = await fetch('/api/status');
      const data = await res.json();
      document.getElementById('phase').textContent = data.phase;
      document.getElementById('coverage').textContent = JSON.stringify(data.coverage, null, 2);
      const body = document.getElementById('tasks');
      body.replaceChildren(...(data.tasks || []).map(t => {
        const row = document.createElement('tr');
        row.appendChild(setText(document.createElement('td'), t.id));
        row.appendChild(setText(document.createElement('td'), t.status));
        row.appendChild(setText(document.createElement('td'), t.title));
        return row;
      }));
      document.getElementById('events').textContent = JSON.stringify(data.events || [], null, 2);
    }
    refresh();
    setInterval(refresh, 2000);
  </script>
</body>
</html>"""


def run_dashboard(project_root: Path, host: str = "127.0.0.1", port: int = 8765) -> None:
    class DashboardServer(ThreadingHTTPServer):
        allow_reuse_address = True
        daemon_threads = True

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):  # noqa: N802
            parsed = urlparse(self.path)
            if parsed.path == f"/assets/{LOGO_ASSET}":
                body = logo_svg().encode("utf-8")
                self.send_response(200)
                self.send_header("Content-Type", "image/svg+xml; charset=utf-8")
                self.send_header("Cache-Control", "public, max-age=3600")
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
            body = dashboard_html().encode("utf-8")
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, format, *args):  # noqa: A002
            return

    server = DashboardServer((host, port), Handler)
    server.serve_forever()
