from devcouncil.indexing.repo_mapper import RepoMapper

def test_go_import_edges(tmp_path, monkeypatch):
    mapper = RepoMapper(tmp_path)
    
    # Write a mock go.mod file
    go_mod = tmp_path / "go.mod"
    go_mod.write_text("module my/go/mod\n", encoding="utf-8")
    
    # Write some mock .go files
    go_a = tmp_path / "src" / "a.go"
    go_a.parent.mkdir(parents=True, exist_ok=True)
    go_a.write_text("""package main
import "my/go/mod/src/sub"
""", encoding="utf-8")
    
    go_sub = tmp_path / "src" / "sub" / "sub.go"
    go_sub.parent.mkdir(parents=True, exist_ok=True)
    go_sub.write_text("package sub\n", encoding="utf-8")
    
    file_set = {"go.mod", "src/a.go", "src/sub/sub.go"}
    files = list(file_set)
    
    # Mock extract_go_import_specs to return ["my/go/mod/src/sub"]
    from devcouncil.indexing import ts_imports
    monkeypatch.setattr(ts_imports, "extract_go_import_specs", lambda src: ["my/go/mod/src/sub"])
    
    edges = mapper._go_import_edges(files, file_set)
    assert len(edges) > 0


def test_rust_import_edges(tmp_path, monkeypatch):
    mapper = RepoMapper(tmp_path)
    
    # Mock tree_sitter_available to True
    from devcouncil.indexing import ts_imports
    monkeypatch.setattr(ts_imports, "tree_sitter_available", lambda: True)
    
    # Write mock .rs files
    lib_rs = tmp_path / "src" / "lib.rs"
    lib_rs.parent.mkdir(parents=True, exist_ok=True)
    lib_rs.write_text("mod helper;\n", encoding="utf-8")
    
    helper_rs = tmp_path / "src" / "helper.rs"
    helper_rs.write_text("pub fn run() {}\n", encoding="utf-8")
    
    file_set = {"src/lib.rs", "src/helper.rs"}
    files = list(file_set)
    
    # Mock extract_rust_import_refs to return list of dicts
    monkeypatch.setattr(ts_imports, "extract_rust_import_refs", lambda src: [{"kind": "mod", "name": "helper"}])
    
    edges = mapper._rust_import_edges(files, file_set)
    assert ("src/lib.rs", "src/helper.rs") in edges


def test_js_import_edges(tmp_path, monkeypatch):
    mapper = RepoMapper(tmp_path)
    
    # Write mock js/ts files
    a_ts = tmp_path / "src" / "a.ts"
    a_ts.parent.mkdir(parents=True, exist_ok=True)
    a_ts.write_text('import { b } from "./b";\n', encoding="utf-8")
    
    b_ts = tmp_path / "src" / "b.ts"
    b_ts.write_text("export const b = 1;\n", encoding="utf-8")
    
    file_set = {"src/a.ts", "src/b.ts"}
    files = list(file_set)
    
    edges = mapper._js_import_edges(files, file_set)
    assert ("src/a.ts", "src/b.ts") in edges
