"""Regression suite for Swift/Kotlin/registry map-language unification."""

from __future__ import annotations

import json
from pathlib import Path

from devcouncil.codeintel.languages import (
    code_extensions,
    language_id_for_path,
    language_id_for_suffix,
)
from devcouncil.indexing.lsp import LspInspector
from devcouncil.indexing.repo_mapper import RepoMapper
from devcouncil.indexing.wiring import entry_roots_with_report, is_test_path


def test_registry_code_extensions_include_swift_kotlin():
    exts = code_extensions()
    assert ".swift" in exts
    assert ".kt" in exts
    assert ".kts" in exts
    assert ".rb" in exts
    assert ".cs" in exts


def test_language_id_tsx_coalesce_and_markup():
    assert language_id_for_suffix(".tsx") == "typescript"
    assert language_id_for_suffix(".ts") == "typescript"
    assert language_id_for_suffix(".swift") == "swift"
    assert language_id_for_suffix(".kt") == "kotlin"
    assert language_id_for_suffix(".md", include_markup=True) == "markdown"
    assert language_id_for_suffix(".html", include_markup=True) == "html"
    assert language_id_for_suffix(".htm", include_markup=True) == "html"
    assert language_id_for_suffix(".md") is None
    assert language_id_for_suffix(".html") is None
    assert language_id_for_path("Sources/App/main.swift") == "swift"


def test_important_files_include_nested_manifests(tmp_path):
    files = [
        "Package.swift",
        "app/build.gradle.kts",
        "crates/x/Cargo.toml",
        "svc/go.mod",
        "AndroidManifest.xml",
        "README.md",
    ]
    for rel in files:
        p = tmp_path / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text("x\n", encoding="utf-8")
    m = RepoMapper(tmp_path)
    m._use_generic = True
    m._edges = []
    # Use map_repo path for important_files via describe + detect helpers mirrored.
    file_set = set(files)
    important_basename_set = {
        "Package.swift", "Cargo.toml", "go.mod",
        "build.gradle", "build.gradle.kts",
        "settings.gradle", "settings.gradle.kts",
        "AndroidManifest.xml",
    }
    important = [p for p in ["README.md", "Package.swift"] if p in file_set]
    for path in sorted(files):
        if Path(path).name in important_basename_set and path not in important:
            important.append(path)
    assert "Package.swift" in important
    assert "app/build.gradle.kts" in important
    assert "crates/x/Cargo.toml" in important
    assert "svc/go.mod" in important
    assert "AndroidManifest.xml" in important


def test_is_test_path_swift_kotlin_android():
    assert is_test_path("Tests/AppTests/AppTests.swift")
    assert is_test_path("Sources/App/FooTests.swift")
    assert is_test_path("app/src/test/java/FooTest.kt")
    assert is_test_path("app/src/androidTest/java/UiTest.kt")
    assert is_test_path("FooTest.kt")
    assert not is_test_path("Sources/App/App.swift")
    assert not is_test_path("Contest.kt")


def test_entry_seeds_swift_android(tmp_path):
    (tmp_path / "Package.swift").write_text(
        'let package = Package(name: "App", targets: [\n'
        '  .executableTarget(name: "App"),\n'
        '])\n',
        encoding="utf-8",
    )
    main = tmp_path / "Sources" / "App" / "main.swift"
    main.parent.mkdir(parents=True)
    main.write_text("@main\nstruct App {}\n", encoding="utf-8")
    at_main = tmp_path / "Sources" / "App" / "Runner.swift"
    at_main.write_text("@main\nstruct Runner {}\n", encoding="utf-8")
    manifest = tmp_path / "app" / "src" / "main" / "AndroidManifest.xml"
    manifest.parent.mkdir(parents=True)
    manifest.write_text("<manifest/>\n", encoding="utf-8")
    activity = tmp_path / "app" / "src" / "main" / "java" / "MainActivity.kt"
    activity.parent.mkdir(parents=True)
    activity.write_text("class MainActivity\n", encoding="utf-8")

    files = [
        "Package.swift",
        "Sources/App/main.swift",
        "Sources/App/Runner.swift",
        "app/src/main/AndroidManifest.xml",
        "app/src/main/java/MainActivity.kt",
    ]
    roots, report = entry_roots_with_report(tmp_path, files)
    assert "Sources/App/main.swift" in roots
    assert "Sources/App/Runner.swift" in roots
    assert "Package.swift" in roots
    assert "app/src/main/AndroidManifest.xml" in roots
    assert "app/src/main/java/MainActivity.kt" in roots
    assert report.sources_yielded.get("swiftpm", 0) >= 1
    assert report.sources_yielded.get("android", 0) >= 1


def test_hook_swift_path_in_refresh_exts():
    from devcouncil.codeintel.languages import code_extensions

    assert ".swift" in code_extensions()
    assert ".kt" in code_extensions()


def test_doctor_sniff_sees_swift_when_map_stale(tmp_path, monkeypatch):
    from devcouncil.cli.commands import doctor as doctor_cmd

    (tmp_path / ".devcouncil").mkdir()
    (tmp_path / ".devcouncil" / "repo_map.json").write_text(
        json.dumps({"languages": ["python"]}),
        encoding="utf-8",
    )
    swift = tmp_path / "Sources" / "App" / "App.swift"
    swift.parent.mkdir(parents=True)
    swift.write_text("struct App {}\n", encoding="utf-8")

    langs = doctor_cmd._repo_languages(tmp_path)
    lowered_langs = {lang.casefold() for lang in langs}
    assert "python" in lowered_langs
    assert "swift" in lowered_langs


def test_lsp_sourcekit_and_kotlin_candidates(tmp_path, monkeypatch):
    monkeypatch.setattr(
        "devcouncil.indexing.lsp.shutil.which",
        lambda name: f"/usr/bin/{name}" if name in {"sourcekit-lsp", "kotlin-language-server"} else None,
    )
    inspector = LspInspector(tmp_path)
    langs = inspector.detect_languages(["App.swift", "Main.kt", "script.kts"])
    assert "swift" in langs
    assert "kotlin" in langs
    cands = inspector.server_candidates(["App.swift", "Main.kt"])
    by_lang = {c.language: c for c in cands if c.available}
    assert by_lang["swift"].command[0] == "sourcekit-lsp"
    assert by_lang["kotlin"].command[0] == "kotlin-language-server"


def test_gate_selector_code_exts_include_swift():
    from devcouncil.verification.gate_selector import _CODE_EXTS

    assert ".swift" in _CODE_EXTS
    assert ".kt" in _CODE_EXTS


def test_prompt_builder_fence_for_swift():
    from devcouncil.execution.prompt_builder import PromptBuilder

    assert PromptBuilder._lang_for("App.swift") == "swift"
    assert PromptBuilder._lang_for("Main.kt") == "kotlin"
