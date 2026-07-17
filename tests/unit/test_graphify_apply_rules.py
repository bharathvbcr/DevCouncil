"""External corpus integration removed — corpus config lives in config.yaml only."""


def test_graphify_module_removed():
    import importlib.util

    spec = importlib.util.find_spec("devcouncil.integrations.graphify")
    assert spec is None
