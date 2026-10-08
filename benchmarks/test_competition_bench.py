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

from competition_bench import (BenchError, MCP, Recorder, callers_projection, counterbalanced_orders, fixture_sources,
                               run_rounds, search_projection, stop, summarize_latency, qualify_search, source_guard)
import ground_truth


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


class ScheduleTests(unittest.TestCase):
    def test_every_arm_takes_every_position_equally_often(self):
        for arms in (["a", "b"], ["a", "b", "c"], ["a", "b", "c", "d"]):
            for blocks in (1, 2, 3):
                with self.subTest(arms=arms, blocks=blocks):
                    orders = counterbalanced_orders(arms, len(arms) * blocks)
                    self.assertEqual(len(orders), len(arms) * blocks)
                    for order in orders:
                        self.assertEqual(sorted(order), sorted(arms))
                    for position in range(len(arms)):
                        counts = {arm: sum(order[position] == arm for order in orders) for arm in arms}
                        self.assertEqual(set(counts.values()), {blocks}, counts)

    def test_two_arms_are_abba(self):
        self.assertEqual(counterbalanced_orders(["a", "b"], 4),
                         [["a", "b"], ["b", "a"], ["b", "a"], ["a", "b"]])

    def test_a_partial_block_is_refused(self):
        for arms, rounds in ((["a", "b"], 3), (["a", "b", "c"], 4), (["a"], 0), ([], 2)):
            with self.subTest(arms=arms, rounds=rounds), self.assertRaises(BenchError):
                counterbalanced_orders(arms, rounds)

    def test_recorded_order_is_the_executed_order(self):
        orders = counterbalanced_orders(["a", "b", "c"], 6)
        calls = []
        executed = run_rounds(orders, lambda arm, round_index, position: calls.append((round_index, position, arm)))
        self.assertEqual(executed, orders)
        self.assertEqual(calls, [(r, p, arm) for r, order in enumerate(orders) for p, arm in enumerate(order)])

    def test_a_failing_step_stops_the_schedule_rather_than_skipping(self):
        def step(arm, round_index, position):
            if round_index == 1:
                raise BenchError("boom")
        with self.assertRaises(BenchError):
            run_rounds(counterbalanced_orders(["a", "b"], 4), step)


class LatencySummaryTests(unittest.TestCase):
    def sample(self, arm, ms, position, verified=True, scenario="edit"):
        return {"arm": arm, "scenario": scenario, "position": position, "wall_ms": ms, "verified": verified}

    def test_min_median_n_cover_verified_samples_only(self):
        summary = summarize_latency([
            self.sample("a", 30.0, 0), self.sample("a", 10.0, 1), self.sample("a", 20.0, 0),
            self.sample("a", 1.0, 1, verified=False), self.sample("b", 5.0, 0, scenario="restore")])
        cell = summary["a/edit"]
        self.assertEqual((cell["n"], cell["min_ms"], cell["median_ms"], cell["max_ms"], cell["failed"]),
                         (3, 10.0, 20.0, 30.0, 1))
        self.assertEqual(cell["by_position"]["0"]["n"], 2)
        self.assertEqual(cell["by_position"]["0"]["median_ms"], 25.0)
        self.assertEqual(cell["by_position"]["1"]["min_ms"], 10.0)
        self.assertEqual(summary["b/restore"]["n"], 1)

    def test_a_cell_with_only_failures_has_no_timing(self):
        cell = summarize_latency([self.sample("a", 1.0, 0, verified=False)])["a/edit"]
        self.assertEqual((cell["n"], cell["min_ms"], cell["median_ms"], cell["failed"]), (0, None, None, 1))


class GroundTruthTests(unittest.TestCase):
    def test_generation_is_deterministic_per_seed(self):
        self.assertEqual(ground_truth.sources(7), ground_truth.sources(7))
        self.assertEqual(ground_truth.truth(7), ground_truth.truth(7))
        self.assertNotEqual(ground_truth.truth(7)["callers"], ground_truth.truth(8)["callers"])

    def test_every_truth_pair_is_a_call_in_the_generated_source(self):
        files = ground_truth.sources(11)
        planted = ground_truth.truth(11)
        pairs = 0
        for callee, rows in planted["callers"].items():
            for path, caller in rows:
                body = files[path].split(caller, 1)
                self.assertEqual(len(body), 2, (path, caller))
                # The caller's own definition line, up to the next definition.
                definition = body[1].split("\n\n", 1)[0]
                self.assertIn(callee + "()", definition, (path, caller, callee))
                callee_path = next(p for p, name in planted["definitions"] if name == callee)
                self.assertNotEqual(callee_path, path, "truth pairs are cross-file")
                pairs += 1
        self.assertEqual(pairs, len(ground_truth.LANGUAGES) * ground_truth.MODULES * ground_truth.CALLERS_PER_MODULE)

    def test_no_call_is_planted_that_the_truth_omits(self):
        files = ground_truth.sources(11)
        planted = ground_truth.truth(11)
        listed = {(path, caller, callee) for callee, rows in planted["callers"].items() for path, caller in rows}
        found = set()
        for path, text in files.items():
            for chunk in text.split("\n\n"):
                for _, name in planted["definitions"]:
                    if f"{name}()" in chunk and "caller" in chunk.split("(", 1)[0]:
                        defined = next((n for p, n in planted["definitions"] if p == path and n in chunk.split("(", 1)[0]), None)
                        if defined and defined != name:
                            found.add((path, defined, name))
        self.assertEqual(found, listed)

    def test_truth_is_kept_out_of_the_indexed_tree(self):
        with tempfile.TemporaryDirectory() as tmp:
            tree, answers = Path(tmp) / "tree", Path(tmp) / "truth.json"
            ground_truth.generate(tree, 3, answers)
            self.assertTrue(answers.exists())
            self.assertFalse(any(p.name.endswith(".json") for p in tree.rglob("*")))

    def test_score_reports_unmeasurable_ratios_as_none(self):
        self.assertEqual(ground_truth.score(set(), set())["recall"], None)
        self.assertEqual(ground_truth.score({("a", "b")}, set())["precision"], None)
        scored = ground_truth.score({("a", "x"), ("a", "y")}, {("a", "x"), ("b", "z")})
        self.assertEqual((scored["precision"], scored["recall"]), (0.5, 0.5))


class CallersProjectionTests(unittest.TestCase):
    def test_devmap_keeps_direct_calls_to_the_target_only(self):
        native = {"items": [
            {"edge_kind": "Calls", "source_file": "m1.py", "source_symbol": "m1.py::caller",
             "target_symbol": "m0.py::leaf"},
            {"edge_kind": "Imports", "source_file": "m2.py", "source_symbol": "m2.py",
             "target_symbol": "m0.py::leaf"}],
            "truncated": False, "hidden": 0, "resolution": "Available",
            "unresolved_namesakes": {"sites": [{"x": 1}]}}
        rows, incomplete, aside = callers_projection("devmap", "leaf", native)
        self.assertEqual(rows, {("m1.py", "caller")})
        self.assertFalse(incomplete)
        self.assertEqual(aside, {"non_call_edges": 1, "unresolved_namesake_sites": 1})

    def test_devmap_unavailable_cannot_score(self):
        with self.assertRaises(BenchError):
            callers_projection("devmap", "leaf", {"items": [], "truncated": False, "resolution": "Unavailable"})

    def test_codegraph_file_rows_are_set_aside_not_counted(self):
        native = {"symbol": "leaf", "callers": [
            {"name": "caller", "kind": "function", "filePath": "m1.py", "startLine": 3},
            {"name": "m1.py", "kind": "file", "filePath": "m1.py", "startLine": 1}]}
        rows, incomplete, aside = callers_projection("codegraph", "leaf", native)
        self.assertEqual(rows, {("m1.py", "caller")})
        self.assertFalse(incomplete)
        self.assertEqual(aside, {"non_function_rows": 1})

    def test_unknown_schema_is_refused(self):
        for tool in ("devmap", "codegraph", "cbm"):
            with self.subTest(tool=tool), self.assertRaises(BenchError):
                callers_projection(tool, "leaf", {"error": "x"})


if __name__ == "__main__":
    unittest.main()
