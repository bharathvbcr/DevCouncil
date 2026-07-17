"""External sync integration removed — native mapping only."""


def test_gitnexus_module_removed():
    import importlib.util

    spec = importlib.util.find_spec("devcouncil.integrations.gitnexus")
    assert spec is None
