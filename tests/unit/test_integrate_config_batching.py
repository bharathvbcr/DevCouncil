from devcouncil.cli.commands import integrate
from devcouncil.cli.commands.init import initialize_project


def test_record_configs_inside_batch_save_yaml_once(tmp_path, monkeypatch):
    initialize_project(tmp_path, quiet=True)

    saves: list[int] = []
    real_save = integrate._save_raw_config

    def counting_save(project_root, config):
        saves.append(1)
        real_save(project_root, config)

    monkeypatch.setattr(integrate, "_save_raw_config", counting_save)

    with integrate._batched_raw_config(tmp_path):
        integrate._record_cursor_config(tmp_path)
        integrate._record_opencode_config(tmp_path)
        integrate._record_antigravity_config(tmp_path)
        integrate._record_warp_config(tmp_path)
        integrate._record_aider_config(tmp_path)

    assert len(saves) == 1
    config = integrate._load_raw_config(tmp_path)
    integrations = config["integrations"]
    assert integrations["cursor"]["enabled"] is True
    assert integrations["opencode"]["enabled"] is True
    assert integrations["antigravity"]["enabled"] is True
    assert integrations["warp"]["enabled"] is True
    assert integrations["warp"]["run_mode"] == "local"
    assert integrations["aider"]["enabled"] is True


def test_record_config_outside_batch_still_saves_immediately(tmp_path):
    initialize_project(tmp_path, quiet=True)

    integrate._record_aider_config(tmp_path)

    config = integrate._load_raw_config(tmp_path)
    assert config["integrations"]["aider"]["enabled"] is True
