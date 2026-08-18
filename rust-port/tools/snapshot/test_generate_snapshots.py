import tempfile
import unittest
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
import generate_snapshots as snapshots


class SnapshotContractTests(unittest.TestCase):
    def test_language_fixture_matrix_covers_frozen_registry(self) -> None:
        expected = {snapshots.fixture_slug(spec.name) for spec in snapshots.LANGUAGE_SPECS}
        fixture_root = snapshots.REPO_ROOT / "rust-port" / "testdata" / "fixtures" / "languages"
        actual = {path.name for path in fixture_root.iterdir() if path.is_dir()}
        self.assertEqual(actual, expected)

    def test_build_failure_never_writes_authoritative_empty_snapshot(self) -> None:
        def fail_build(_fixture: Path):
            raise RuntimeError("parser crashed")

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            fixture = root / "fixture"
            output = root / "golden"
            fixture.mkdir()

            with self.assertRaises(snapshots.SnapshotError):
                snapshots.generate_fixture_snapshot(fixture, output, builder=fail_build)
            self.assertFalse(output.exists())

    def test_normalization_uses_canonical_graph_identity_fields(self) -> None:
        node = {
            "id": "pkg/mod.py::run",
            "kind": "function",
            "path": "pkg/mod.py",
            "line": 3,
            "name": "run",
            "exported": True,
        }
        edge = {
            "source": "pkg/mod.py::run",
            "target": "pkg/lib.py::work",
            "kind": "calls",
            "confidence": "extracted",
        }
        dead = {"id": "pkg/mod.py::unused", "confidence": "inferred"}

        self.assertEqual(snapshots.normalize_nodes([node]), [node])
        self.assertEqual(snapshots.normalize_edges([edge]), [edge])
        self.assertEqual(snapshots.normalize_dead([dead]), [dead])

    def test_fixture_parse_cache_is_removed_after_snapshot(self) -> None:
        def build_with_cache(fixture: Path):
            cache = fixture / ".devcouncil" / "cache"
            cache.mkdir(parents=True)
            (cache / "transient.json").write_text("{}", encoding="utf-8")
            return {"nodes": [], "edges": [], "dead": []}

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            fixture = root / "fixture"
            fixture.mkdir()
            snapshots.generate_fixture_snapshot(
                fixture,
                root / "golden",
                builder=build_with_cache,
            )
            self.assertFalse((fixture / ".devcouncil" / "cache").exists())


if __name__ == "__main__":
    unittest.main()
