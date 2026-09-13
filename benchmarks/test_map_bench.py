"""Executable benchmark provenance and native workspace regression checks."""
import hashlib
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import map_bench


class NativeWorkspaceTests(unittest.TestCase):
    def test_current_release_precedes_an_unrelated_path_binary(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            binary = root / "rust/target/release/devmap"
            binary.parent.mkdir(parents=True)
            binary.write_text("#!/bin/sh\nexit 0\n")
            binary.chmod(0o755)
            with patch.object(map_bench, "REPO_ROOT", root), patch.object(
                map_bench.shutil, "which", return_value="/unrelated/devmap"
            ):
                self.assertEqual(map_bench.find_devmap(), str(binary))

    def test_peak_rss_helper_is_the_current_native_helper(self):
        self.assertEqual(map_bench.PEAK_RSS_HELPER,
                         map_bench.REPO_ROOT / "rust/tools/peak_rss.sh")
        self.assertTrue(map_bench.PEAK_RSS_HELPER.is_file())

    def test_identity_binds_exact_binary_bytes(self):
        with tempfile.TemporaryDirectory() as temp:
            binary = Path(temp) / "devmap"
            content = b"#!/bin/sh\nprintf 'devmap-test\\n'\n"
            binary.write_bytes(content)
            binary.chmod(0o755)
            identity = map_bench.binary_identity(str(binary))
            self.assertEqual(identity["sha256"], hashlib.sha256(content).hexdigest())
            self.assertEqual(identity["version"], "devmap-test")

    def test_failed_version_probe_cannot_be_valid_provenance(self):
        with tempfile.TemporaryDirectory() as temp:
            binary = Path(temp) / "devmap"
            binary.write_text("#!/bin/sh\nprintf 'not a version\\n'\nexit 7\n")
            binary.chmod(0o755)
            with self.assertRaises(map_bench.BenchError):
                map_bench.binary_identity(str(binary))


if __name__ == "__main__":
    unittest.main()
