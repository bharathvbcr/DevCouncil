"""Host hardware detection used to size local (Ollama) models.

DevCouncil runs the council roles against whatever LLM provider is configured.
For the local ``ollama`` provider the model has to fit in host memory. On Apple
Silicon Macs that memory is *unified* (shared by CPU/GPU/OS), so total RAM is the
ceiling. On a host with a discrete GPU (NVIDIA), Ollama offloads to VRAM, so the
*VRAM* is the practical ceiling instead — a 64 GB box with an 8 GB GPU should not be
told to run a model that only fits in system RAM. This module exposes small, pure
helpers so ``dev doctor`` and ``dev setup`` can recommend a model that will actually
run instead of a one-size-fits-all default, on macOS, Linux and Windows.

Everything here is best-effort and stdlib-only: detection failures return
``None`` rather than raising, so callers degrade to the static default.
"""

from __future__ import annotations

import os
import platform
import shutil
import subprocess
from dataclasses import dataclass

# Recommended Ollama context window for DevCouncil's large planning prompts.
# Kept in sync with the value surfaced by `dev doctor`.
RECOMMENDED_NUM_CTX = 16384

# Fallback model when the host is unknown or has little memory. Matches the
# static defaults in ``llm/model_defaults.yaml``.
DEFAULT_OLLAMA_MODEL = "qwen2.5-coder:7b"

# RAM (GiB) -> recommended qwen2.5-coder size. Highest matching tier wins.
# Sizes are chosen so the quantized weights plus a 16k context comfortably fit
# alongside the OS in Apple Silicon unified memory.
_OLLAMA_MODEL_TIERS: tuple[tuple[float, str], ...] = (
    (48.0, "qwen2.5-coder:32b"),
    (24.0, "qwen2.5-coder:14b"),
    (0.0, "qwen2.5-coder:7b"),
)


def is_macos() -> bool:
    return platform.system() == "Darwin"


def is_apple_silicon() -> bool:
    """True on Apple-Silicon (arm64) Macs."""
    return is_macos() and platform.machine() == "arm64"


def mac_chip_brand() -> str | None:
    """Marketing CPU string on macOS (e.g. ``Apple M3 Pro``), else None."""
    if not is_macos():
        return None
    try:
        out = subprocess.check_output(
            ["sysctl", "-n", "machdep.cpu.brand_string"],
            text=True,
            timeout=5,
        ).strip()
        return out or None
    except Exception:
        return None


def total_ram_gb() -> float | None:
    """Total physical RAM in GiB, or None if it cannot be determined."""
    try:
        if is_macos():
            out = subprocess.check_output(
                ["sysctl", "-n", "hw.memsize"], text=True, timeout=5
            ).strip()
            return int(out) / (1024**3)
        # POSIX (Linux): pages * page size.
        pages = os.sysconf("SC_PHYS_PAGES")
        page_size = os.sysconf("SC_PAGE_SIZE")
        return (pages * page_size) / (1024**3)
    except (OSError, ValueError, AttributeError, subprocess.SubprocessError):
        return None


def nvidia_vram_gb() -> float | None:
    """Total VRAM (GiB) of the largest NVIDIA GPU via ``nvidia-smi``, else ``None``.

    On hosts with a discrete NVIDIA GPU this is the real ceiling for Ollama, since the
    model is offloaded to VRAM. Returns ``None`` when ``nvidia-smi`` is absent (no GPU,
    Apple Silicon, or an unsupported vendor), so callers fall back to total RAM."""
    if not shutil.which("nvidia-smi"):
        return None
    try:
        out = subprocess.check_output(
            ["nvidia-smi", "--query-gpu=memory.total", "--format=csv,noheader,nounits"],
            text=True,
            timeout=5,
            stderr=subprocess.DEVNULL,
        ).strip()
    except (OSError, subprocess.SubprocessError):
        return None
    sizes_mib: list[float] = []
    for line in out.splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            sizes_mib.append(float(line))
        except ValueError:
            continue
    if not sizes_mib:
        return None
    return max(sizes_mib) / 1024.0  # MiB -> GiB


def recommend_ollama_model(ram_gb: float | None = None, vram_gb: float | None = None) -> str:
    """Largest qwen2.5-coder size expected to run on the host.

    When a discrete GPU's ``vram_gb`` is known it is the ceiling (Ollama offloads to
    VRAM); otherwise total system RAM is used (correct for unified-memory Macs and
    CPU-only hosts)."""
    if ram_gb is None:
        ram_gb = total_ram_gb()
    effective = vram_gb if vram_gb is not None else ram_gb
    if effective is None:
        return DEFAULT_OLLAMA_MODEL
    for floor, model in _OLLAMA_MODEL_TIERS:
        if effective >= floor:
            return model
    return DEFAULT_OLLAMA_MODEL


@dataclass(frozen=True)
class HostSummary:
    """A snapshot of the host relevant to local-model sizing."""

    is_macos: bool
    is_apple_silicon: bool
    chip: str | None
    ram_gb: float | None
    recommended_ollama_model: str
    vram_gb: float | None = None

    @property
    def ram_label(self) -> str:
        return f"{self.ram_gb:.0f} GB" if self.ram_gb is not None else "unknown RAM"

    @property
    def vram_label(self) -> str | None:
        return f"{self.vram_gb:.0f} GB VRAM" if self.vram_gb is not None else None

    @property
    def chip_label(self) -> str:
        if self.chip:
            return self.chip
        if self.is_apple_silicon:
            return "Apple Silicon"
        if self.vram_gb is not None:
            return "discrete GPU host"
        return "this host"

    @property
    def platform_label(self) -> str:
        """Human label for the sizing row across OSes."""
        if self.is_macos:
            return "Apple Silicon" if self.is_apple_silicon else "Mac (Intel)"
        system = platform.system()
        return system or "Host"

    @property
    def memory_label(self) -> str:
        """Memory ceiling used for the recommendation (VRAM if discrete GPU, else RAM)."""
        vram = self.vram_label
        return f"{vram} (GPU)" if vram else self.ram_label


def describe_host() -> HostSummary:
    ram = total_ram_gb()
    vram = nvidia_vram_gb()
    return HostSummary(
        is_macos=is_macos(),
        is_apple_silicon=is_apple_silicon(),
        chip=mac_chip_brand(),
        ram_gb=ram,
        vram_gb=vram,
        recommended_ollama_model=recommend_ollama_model(ram, vram),
    )
