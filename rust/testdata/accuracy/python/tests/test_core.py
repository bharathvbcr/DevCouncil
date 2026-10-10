from pkg.models import build_config
from pkg.util import split


def test_engine(engine):
    """C23 fixture injection by parameter; C24 test discovery entry point."""
    assert engine.step() >= 0


def test_split():
    """C20 import-scoped call to a name another module also declares."""
    assert split("a,b") == ["a", "b"]


def test_config():
    assert build_config().validate()


class TestGroup:
    def test_method(self):
        assert split("x") == ["x"]
