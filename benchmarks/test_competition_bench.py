"""Negative controls for the benchmark's evidence and process boundaries."""
import os
from pathlib import Path
import sys
import tempfile
import subprocess
import signal
import time
import unittest
from unittest.mock import patch

from competition_bench import BenchError, MCP, Recorder, fixture_sources, search_projection, stop, qualify_search, source_guard


class ProjectionTests(unittest.TestCase):
    def test_malformed_cbm_row_is_unverified(self):
        record = {"ok": True}
        native = {"cols": ["name"], "groups": [{"file": "x.py", "rows": [[]]}], "has_more": False}
        result = qualify_search(record, "cbm", native, set())
        self.assertEqual(record["semantic_status"], "unverified")
        self.assertEqual(result["status"], "unverified")

    def test_empty_success_does_not_qualify_a_positive_timing(self):
        record = {"ok": True}
        native = {"items": [], "truncated": False, "resolution": "Available",
                  "total": 0, "shown": 0, "hidden": 0}
        result = qualify_search(record, "devmap", native, {("target.py", "leaf")})
        self.assertFalse(result["passed"])
        self.assertEqual(record["semantic_status"], "incorrect")

    def test_unavailable_devmap_cannot_prove_absence(self):
        with self.assertRaises(BenchError):
            search_projection("devmap", {"items": [], "truncated": False,
                                        "resolution": "Unavailable"})

    def test_hidden_devmap_rows_prevent_negative_proof(self):
        result = {"items": [], "truncated": False, "resolution": "Available",
                  "total": 100, "shown": 0, "hidden": 100}
        self.assertTrue(search_projection("devmap", result)[1])

    def test_malformed_results_cannot_prove_absence(self):
        for tool in ("devmap", "codegraph", "cbm"):
            with self.subTest(tool=tool), self.assertRaises(BenchError):
                search_projection(tool, {"error": "unavailable"})

    def test_devmap_cap_and_namesake_remain_visible(self):
        rows, incomplete = search_projection("devmap", {"items": [
            {"symbol_name": "leaf", "file_path": "other.py", "source_span": "def expected(): pass"}
        ], "truncated": True, "resolution": "Available", "total": 2, "shown": 1, "hidden": 1})
        self.assertEqual(rows, {("other.py", "leaf")})
        self.assertTrue(incomplete)

    def test_cbm_zero_and_cap_are_distinct(self):
        empty = {"groups": [], "cols": ["name"], "has_more": False}
        self.assertEqual(search_projection("cbm", empty), (set(), False))
        empty["has_more"] = True
        self.assertEqual(search_projection("cbm", empty), (set(), True))

    def test_codegraph_native_count_must_match_parsed_rows(self):
        text = "**Search Results (1 found)**\n\n**leaf** (function)\nsrc/x.py:1\n`() `\n"
        native = {"content": [{"type": "text", "text": text}]}
        self.assertEqual(search_projection("codegraph", native), ({("src/x.py", "leaf")}, False))
        native["content"][0]["text"] = text.replace("(1 found)", "(2 found)")
        with self.assertRaises(BenchError):
            search_projection("codegraph", native)

    def test_cli_limit_cannot_be_complete_coverage(self):
        rows = [{"node": {"name": f"f{i}", "filePath": "x.py"}} for i in range(100)]
        self.assertTrue(search_projection("codegraph", rows)[1])

    def test_ground_truth_definitions_and_callers_are_in_source(self):
        for filename, source in fixture_sources().items():
            language = Path(filename).stem
            self.assertEqual(source.count(f"audit_{language}_leaf"), 2)
            self.assertEqual(source.count(f"audit_{language}_caller"), 1)
        compile(fixture_sources()["python.py"], "python.py", "exec")


class ProcessTests(unittest.TestCase):
    def test_fast_oversized_output_cannot_be_a_success(self):
        for fd in (1, 2):
            with self.subTest(fd=fd), tempfile.TemporaryDirectory() as temp, patch("competition_bench.MAX_RESPONSE", 1024):
                root = Path(temp)
                program = f"import os;os.write({fd},b'x'*2048)"
                result = Recorder(root).command("oversized", [sys.executable, "-c", program], root, os.environ.copy())
                self.assertTrue(result["output_limited"])
                self.assertFalse(result["ok"])

    def test_mcp_notification_flood_cannot_evade_aggregate_budget(self):
        with tempfile.TemporaryDirectory() as temp, patch("competition_bench.MAX_RESPONSE", 1024):
            root = Path(temp)
            program = ("import sys,json,time; r=json.loads(sys.stdin.readline()); "
                       "[(print(json.dumps({'jsonrpc':'2.0','method':'notice','params':{'message':'x'*100}}),flush=True),time.sleep(0.005)) for _ in range(40)]; "
                       "print(json.dumps({'jsonrpc':'2.0','id':r['id'],'result':{}}),flush=True)")
            session = MCP([sys.executable, "-c", program], root, os.environ.copy(), root / "mcp.stderr")
            try:
                with self.assertRaisesRegex(BenchError, "budget"):
                    session.request("tools/list", {}, timeout=1)
            finally:
                session.close()

    def test_mcp_stderr_budget_is_checked_before_success(self):
        with tempfile.TemporaryDirectory() as temp, patch("competition_bench.MAX_RESPONSE", 1024):
            root = Path(temp)
            program = ("import os,sys,json; r=json.loads(sys.stdin.readline()); os.write(2,b'x'*2048); "
                       "print(json.dumps({'jsonrpc':'2.0','id':r['id'],'result':{}}),flush=True)")
            session = MCP([sys.executable, "-c", program], root, os.environ.copy(), root / "mcp.stderr")
            try:
                with self.assertRaisesRegex(BenchError, "budget"):
                    session.request("tools/list", {}, timeout=1)
            finally:
                session.close()

    def test_corpus_change_invalidates_successful_campaign(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            with self.assertRaisesRegex(BenchError, "comparative timings invalid"):
                with source_guard(root, [{"path": "missing.py", "sha256": "unused"}], root):
                    pass

    def test_source_preservation_is_reported_on_failure(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            with self.assertRaises(RuntimeError):
                with source_guard(root, [{"path": "missing.py", "sha256": "unused"}], root):
                    raise RuntimeError("index failed")
            import json
            result = json.loads((root / "source-verification.json").read_text())
            self.assertEqual(result["checked"], 1)
            self.assertEqual(result["mismatches"][0]["path"], "missing.py")

    def test_mcp_error_frames_are_retained(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            code = "import sys,json; r=json.loads(sys.stdin.readline()); print(json.dumps({'jsonrpc':'2.0','id':r['id'],'error':{'code':-32603,'message':'failed'}}),flush=True)"
            session = MCP([sys.executable, "-c", code], root, os.environ.copy(), root / "mcp.stderr")
            try:
                with self.assertRaisesRegex(BenchError, "failed"):
                    session.request("tools/list", {}, timeout=1)
            finally:
                session.close()
            self.assertIn('"error"', (root / "mcp.frames.jsonl").read_text())

    @unittest.skipUnless(hasattr(os, "fork"), "requires Unix process groups")
    def test_exited_leader_does_not_leave_live_workers(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            program = ("import os,time,pathlib,signal; child=os.fork(); "
                       "pathlib.Path('child').write_text(str(child)) if child else None; "
                       "os._exit(0) if child else None; "
                       "signal.signal(signal.SIGTERM,lambda *_: (pathlib.Path('terminated').touch(),os._exit(0))); "
                       "pathlib.Path('ready').touch(); time.sleep(60)")
            process = subprocess.Popen([sys.executable, "-c", program], cwd=root, start_new_session=True)
            process.wait(timeout=2)
            child = int((root / "child").read_text())
            try:
                deadline = time.monotonic() + 2
                while not (root / "ready").exists() and time.monotonic() < deadline:
                    time.sleep(0.01)
                stop(process)
                deadline = time.monotonic() + 2
                while not (root / "terminated").exists() and time.monotonic() < deadline:
                    time.sleep(0.01)
                self.assertTrue((root / "terminated").exists(), "orphan worker survived leader exit")
            finally:
                try:
                    os.kill(child, signal.SIGKILL)
                except ProcessLookupError:
                    pass

    def test_failed_process_is_retained_and_never_read_as_a_pass(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            recorder = Recorder(root)
            result = recorder.command("failed", [sys.executable, "-c", "print('{}');raise SystemExit(7)"], root, os.environ.copy())
            self.assertEqual(result["exit_code"], 7)
            self.assertFalse(result["ok"])
            self.assertTrue((root / result["stdout"]).is_file())
            with self.assertRaises(BenchError):
                recorder.read(result)

    def test_timeout_is_reaped_and_retained(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            result = Recorder(root).command("timeout", [sys.executable, "-c", "import time;time.sleep(60)"], root, os.environ.copy(), timeout=0.05)
            self.assertTrue(result["timed_out"])
            self.assertFalse(result["ok"])
            self.assertIsNotNone(result["exit_code"])

    def test_mcp_eof_cannot_be_empty_success(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            session = MCP([sys.executable, "-c", "import sys;sys.stdin.readline()"], root, os.environ.copy(), root / "stderr")
            try:
                with self.assertRaisesRegex(BenchError, "stdout closed"):
                    session.request("tools/list", {}, timeout=1)
            finally:
                session.close()
            self.assertIsNotNone(session.process.poll())


if __name__ == "__main__":
    unittest.main()
