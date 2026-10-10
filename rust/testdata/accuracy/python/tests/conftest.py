import pytest

from pkg.core import Engine


@pytest.fixture
def engine():
    """C23 fixture; injected by parameter name."""
    return make_engine()


def make_engine():
    return Engine(5)


@pytest.fixture
def unused_fixture():
    return 1
