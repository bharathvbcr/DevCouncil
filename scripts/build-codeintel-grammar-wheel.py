"""Stage a complete language-pack cache for a platform-specific grammar wheel."""

from __future__ import annotations

import argparse
import hashlib
import json
import platform
import shutil
from pathlib import Path

import tree_sitter_language_pack as pack
from packaging.tags import sys_tags

_CANONICAL_ASSET_ALIASES = {
    "csharp": "c_sharp",
    "vb": "vb_dotnet",
}


def _stage_canonical_aliases(cache: Path) -> None:
    """Give release IDs physical filenames discoverable without a registry download."""

    for canonical, packaged in _CANONICAL_ASSET_ALIASES.items():
        matches = sorted(cache.glob(f"*tree_sitter_{packaged}.*"))
        if len(matches) != 1:
            raise RuntimeError(
                f"expected one staged asset for {canonical} ({packaged}), found {len(matches)}"
            )
        source = matches[0]
        target = source.with_name(source.name.replace(
            f"tree_sitter_{packaged}",
            f"tree_sitter_{canonical}",
        ))
        if not target.exists():
            shutil.copy2(source, target)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--cache-dir", type=Path, default=None)
    parser.add_argument(
        "--prefetch",
        action="store_true",
        help="Explicitly download the required grammars into --cache-dir before staging.",
    )
    parser.add_argument(
        "--assets",
        type=Path,
        default=Path("packages/codeintel-grammars/src/devcouncil_codeintel_grammars/assets"),
    )
    ns = parser.parse_args()
    from devcouncil.codeintel.languages import LANGUAGE_SPECS

    required = sorted({
        grammar
        for spec in LANGUAGE_SPECS
        for grammar in (spec.grammar, *spec.embedded)
    })
    source = (ns.cache_dir or Path(str(pack.cache_dir()))).expanduser().resolve()
    if ns.cache_dir is not None:
        source.mkdir(parents=True, exist_ok=True)
        pack.configure(pack.PackConfig(cache_dir=str(source)))
    if ns.prefetch:
        pack.prefetch(required)
    assets = ns.assets.expanduser().resolve()
    available = sorted(pack.available_languages())

    missing = sorted(set(required) - set(available))
    if missing:
        print("Grammar cache is incomplete; explicitly prefetch before building:")
        for language in missing:
            print(f"  - {language}")
        return 1

    cache_target = assets / "cache"
    if source == cache_target or cache_target in source.parents:
        parser.error("--cache-dir must not be inside the staged assets directory")
    if cache_target.exists():
        shutil.rmtree(cache_target)
    if not source.is_dir():
        parser.error(f"grammar cache does not exist: {source}")
    assets.mkdir(parents=True, exist_ok=True)
    shutil.copytree(source, cache_target)
    _stage_canonical_aliases(cache_target)
    checksums = {
        path.relative_to(assets).as_posix(): hashlib.sha256(path.read_bytes()).hexdigest()
        for path in sorted(cache_target.rglob("*"))
        if path.is_file()
    }
    manifest = {
        "pack_version": getattr(pack, "__version__", "unknown"),
        "platform": {
            "system": platform.system(),
            "machine": platform.machine(),
            "python": platform.python_version(),
            "wheel_tag": f"py3-none-{next(sys_tags()).platform}",
        },
        "languages": available,
        "required_grammars": required,
        "sha256": checksums,
    }
    (assets / "manifest.json").write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    print(f"Staged {len(checksums)} grammar assets for {len(available)} languages at {assets}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
