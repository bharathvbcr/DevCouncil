"""Mark grammar artifacts as platform-specific without tying them to one ABI."""

import hashlib
import json
from pathlib import Path

from hatchling.builders.hooks.plugin.interface import BuildHookInterface
from packaging.tags import sys_tags

__all__ = ["CustomBuildHook"]


class CustomBuildHook(BuildHookInterface):
    def initialize(self, version: str, build_data: dict) -> None:
        del version
        assets = Path(self.root) / "src" / "devcouncil_codeintel_grammars" / "assets"
        manifest_path = assets / "manifest.json"
        if not manifest_path.is_file():
            raise RuntimeError(
                "grammar assets are not staged; run scripts/build-codeintel-grammar-wheel.py first"
            )
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        checksums = dict(manifest.get("sha256") or {})
        required = set(manifest.get("required_grammars") or [])
        available = set(manifest.get("languages") or [])
        missing = sorted(required - available)
        failed = [
            rel
            for rel, digest in checksums.items()
            if not (assets / rel).is_file()
            or hashlib.sha256((assets / rel).read_bytes()).hexdigest() != digest
        ]
        if not checksums or not required or missing or failed:
            raise RuntimeError(
                "invalid grammar assets: "
                f"missing={missing}, failed_checksums={failed}, asset_count={len(checksums)}"
            )
        platform_tag = next(sys_tags()).platform
        build_data["tag"] = f"py3-none-{platform_tag}"
        build_data["pure_python"] = False
        build_data.setdefault("force_include", {})[str(assets)] = (
            "devcouncil_codeintel_grammars/assets"
        )
