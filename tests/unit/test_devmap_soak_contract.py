"""Execute the soak harness with failures injected at its external boundaries."""
from __future__ import annotations

import os
from pathlib import Path
import socket
import subprocess
import sys
import threading

import pytest

SOAK = Path(__file__).resolve().parents[2] / "rust-port/tools/soak.sh"


@pytest.fixture
def soak_process(tmp_path):
    root = tmp_path / "corpus"
    root.mkdir()
    (root / "source.py").write_text("def helper(): return 1\ndef main(): return helper()\n")
    binary = tmp_path / "devmap"
    binary.write_text(f"#!{sys.executable}\n" + '''
import json, os, sqlite3, sys
from pathlib import Path
mode = os.environ.get("SOAK_INJECT", "valid")
db = Path.cwd() / "fixture.sqlite"
command = next((arg for arg in sys.argv[1:] if arg in {"build", "status", "search", "dead"}), "")
if command == "build":
    if mode == "corrupt":
        db.write_bytes(b"not a sqlite database")
    else:
        with sqlite3.connect(db) as conn:
            conn.executescript("CREATE TABLE IF NOT EXISTS generations (id INTEGER); CREATE TABLE IF NOT EXISTS generation_edges (generation_id INTEGER, source_symbol TEXT, target_symbol TEXT, edge_kind TEXT); DELETE FROM generations; DELETE FROM generation_edges; INSERT INTO generations VALUES (1);")
            if mode != "empty":
                conn.execute("INSERT INTO generation_edges VALUES (1, 'main', 'helper', 'Calls')")
elif command == "status":
    print(json.dumps({"db_path": str(db)}))
elif command in {"search", "dead"}:
    if mode == "query":
        sys.exit(17)
    print(json.dumps({"items": []}))
''')
    binary.chmod(0o755)

    def run(mode):
        return subprocess.run(
            ["bash", str(SOAK), str(root), "1"],
            env={**os.environ, "DEVMAP_BIN": str(binary), "SOAK_INJECT": mode},
            capture_output=True, text=True, timeout=15,
        )
    return run


@pytest.mark.parametrize("mode", ["query", "corrupt", "empty"])
def test_an_unperformed_soak_check_cannot_report_success(soak_process, mode):
    result = soak_process(mode)
    assert result.returncode != 0, result.stdout + result.stderr
    assert "SOAK SMOKE OK" not in result.stdout


def test_a_measured_soak_still_passes(soak_process):
    result = soak_process("valid")
    assert result.returncode == 0, result.stdout + result.stderr
    assert "SOAK SMOKE OK" in result.stdout


def test_the_daemon_probe_rejects_an_error_envelope(tmp_path):
    # Run the exact Python probe embedded in the shell script against a real
    # socket. Valid JSON alone is not evidence that the requested query ran.
    source = SOAK.read_text()
    marker = 'python3 - "$ENDPOINT" "$1" "${2:-validate}" <<\'PY\'\n'
    program = source.split(marker, 1)[1].split("\nPY", 1)[0]
    endpoint = Path("/tmp") / f"devmap-soak-contract-{os.getpid()}.sock"
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as listener:
        listener.bind(str(endpoint))
        listener.listen(1)
        listener.settimeout(5)

        def reject():
            connection, _ = listener.accept()
            with connection:
                connection.recv(4096)
                connection.sendall(b'{"ok":false,"error":"injected refusal"}\n')
        worker = threading.Thread(target=reject)
        worker.start()
        try:
            result = subprocess.run([sys.executable, "-", str(endpoint), "{}"],
                                    input=program, text=True, capture_output=True, timeout=5)
        finally:
            worker.join(timeout=5)
            endpoint.unlink(missing_ok=True)
    assert result.returncode != 0, "the probe accepted an error as a successful query"
