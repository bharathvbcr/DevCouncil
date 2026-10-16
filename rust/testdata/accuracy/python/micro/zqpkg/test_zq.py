import pytest


class ZqEnv:
    def launch(self):
        return 1


@pytest.fixture
def zq_env() -> ZqEnv:
    return ZqEnv()


def test_zq_launches(zq_env):
    assert zq_env.launch() == 1
