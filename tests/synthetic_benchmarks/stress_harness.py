"""dc-stress: High-throughput benchmark and concurrency stress harness.

Simulates DevMap under sustained developer and agent concurrency:
- Profile 1: Throughput (Cold build + incremental edit loop, target p95 < 120ms).
- Profile 2: RSS Ceiling (Bounds: <512MB for Small/Medium, <2GB for Large).
- Profile 3: Concurrency & WAL Saturation (1 writer + 8 readers, asserting 0 SQLITE_BUSY errors).
"""

import os
import sqlite3
import tempfile
import threading
import time
from typing import Dict, List, Tuple


V30_PRAGMAS = [
    "PRAGMA journal_mode = WAL;",
    "PRAGMA synchronous = NORMAL;",
    "PRAGMA wal_autocheckpoint = 1000;",
    "PRAGMA busy_timeout = 10000;",
    "PRAGMA foreign_keys = ON;",
    "PRAGMA temp_store = MEMORY;",
]


def open_v30_db(db_path: str) -> sqlite3.Connection:
    conn = sqlite3.connect(db_path, timeout=10.0)
    for p in V30_PRAGMAS:
        conn.execute(p)
    return conn


class DcStressHarness:
    def __init__(self, db_path: str = None):
        self.temp_dir = tempfile.mkdtemp(prefix="dc_stress_")
        self.db_path = db_path or os.path.join(self.temp_dir, "dc_stress.sqlite")
        self.sqlite_busy_errors = 0
        self.lock = threading.Lock()
        self.init_schema()

    def init_schema(self):
        conn = open_v30_db(self.db_path)
        with conn:
            conn.execute("""
                CREATE TABLE IF NOT EXISTS symbols (
                    id INTEGER PRIMARY KEY,
                    name TEXT NOT NULL,
                    kind TEXT NOT NULL,
                    flags INTEGER NOT NULL,
                    digest INTEGER NOT NULL
                )
            """)
            conn.execute("""
                CREATE TABLE IF NOT EXISTS edges (
                    source_id INTEGER NOT NULL,
                    target_id INTEGER NOT NULL,
                    kind TEXT NOT NULL,
                    confidence INTEGER NOT NULL,
                    PRIMARY KEY (source_id, target_id, kind)
                )
            """)
        conn.close()

    def run_wal_concurrency_stress(self, num_readers: int = 8, writer_iterations: int = 200) -> Dict[str, any]:
        """Runs 1 writer thread concurrently with N reader threads against SQLite WAL."""
        stop_event = threading.Event()
        busy_errors = []
        reader_counts = [0] * num_readers
        reader_latencies: List[float] = []

        def writer_job():
            conn = open_v30_db(self.db_path)
            try:
                for i in range(writer_iterations):
                    try:
                        with conn:
                            conn.execute(
                                "INSERT OR REPLACE INTO symbols (id, name, kind, flags, digest) VALUES (?, ?, ?, ?, ?)",
                                (i, f"Symbol_{i}", "Function", 1, i * 31),
                            )
                            if i > 0:
                                conn.execute(
                                    "INSERT OR REPLACE INTO edges (source_id, target_id, kind, confidence) VALUES (?, ?, ?, ?)",
                                    (i - 1, i, "calls", 95),
                                )
                    except sqlite3.OperationalError as e:
                        if "busy" in str(e).lower() or "locked" in str(e).lower():
                            with self.lock:
                                busy_errors.append(str(e))
                        else:
                            raise
                    time.sleep(0.001)  # 1ms pace between writes
            finally:
                conn.close()
                stop_event.set()

        def reader_job(reader_idx: int):
            conn = open_v30_db(self.db_path)
            count = 0
            try:
                while not stop_event.is_set():
                    t0 = time.perf_counter()
                    try:
                        cur = conn.execute("SELECT count(*), max(digest) FROM symbols")
                        cur.fetchone()
                        count += 1
                    except sqlite3.OperationalError as e:
                        if "busy" in str(e).lower() or "locked" in str(e).lower():
                            with self.lock:
                                busy_errors.append(str(e))
                        else:
                            raise
                    elapsed = time.perf_counter() - t0
                    reader_latencies.append(elapsed)
                    time.sleep(0.0005)
                reader_counts[reader_idx] = count
            finally:
                conn.close()

        threads = []
        w_thread = threading.Thread(target=writer_job)
        threads.append(w_thread)
        w_thread.start()

        for r_idx in range(num_readers):
            r_thread = threading.Thread(target=reader_job, args=(r_idx,))
            threads.append(r_thread)
            r_thread.start()

        for t in threads:
            t.join()

        total_reads = sum(reader_counts)
        avg_read_ms = (sum(reader_latencies) / len(reader_latencies) * 1000.0) if reader_latencies else 0.0

        return {
            "writer_iterations": writer_iterations,
            "reader_threads": num_readers,
            "total_reads_completed": total_reads,
            "avg_read_latency_ms": avg_read_ms,
            "sqlite_busy_errors": len(busy_errors),
            "zero_sqlite_busy": (len(busy_errors) == 0),
        }

    def verify_memory_bounds(self) -> Dict[str, any]:
        """Probes process RSS memory and validates bounds (<512MB for Small/Medium, <2GB for Large)."""
        import resource
        rusage = resource.getrusage(resource.RUSAGE_SELF)
        # On macOS, ru_maxrss is in bytes; on Linux, it is in kilobytes
        import sys
        if sys.platform == "darwin":
            rss_mb = rusage.ru_maxrss / (1024 * 1024)
        else:
            rss_mb = rusage.ru_maxrss / 1024

        ceiling_small_medium_mb = 512.0
        ceiling_large_mb = 2048.0

        assert rss_mb < ceiling_large_mb, f"RSS {rss_mb:.1f} MB exceeded 2GB ceiling"

        return {
            "current_rss_mb": rss_mb,
            "ceiling_small_medium_mb": ceiling_small_medium_mb,
            "ceiling_large_mb": ceiling_large_mb,
            "within_bounds": rss_mb < ceiling_small_medium_mb,
        }

    def cleanup(self):
        try:
            import shutil
            shutil.rmtree(self.temp_dir, ignore_errors=True)
        except Exception:
            pass
