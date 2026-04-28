from pathlib import Path
from devcouncil.indexing.repo_mapper import RepoMapper

def test_repo_mapper_basic():
    mapper = RepoMapper(Path("."))
    repo_map = mapper.map_repo("init")
    
    assert "python" in repo_map.languages
    assert "uv" in repo_map.package_managers
    assert "npm" in repo_map.package_managers
    assert "pyproject.toml" in repo_map.important_files
    assert "python" in repo_map.lsp["languages"]
    assert all("__pycache__" not in item["path"] for item in repo_map.candidate_files)
    # "init" should match some files if any contain it
    # Since we have src/devcouncil/cli/commands/init.py, it should match
    assert len(repo_map.candidate_files) > 0
