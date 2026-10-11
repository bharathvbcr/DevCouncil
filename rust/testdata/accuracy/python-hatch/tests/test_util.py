from rootpkg.util import split


def test_split():
    assert split("a b") == ["a", "b"]
