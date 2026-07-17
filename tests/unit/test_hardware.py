from __future__ import annotations

import subprocess

from devcouncil import hardware


def test_platform_helpers_detect_macos_and_apple_silicon(monkeypatch) -> None:
    monkeypatch.setattr(hardware.platform, "system", lambda: "Darwin")
    monkeypatch.setattr(hardware.platform, "machine", lambda: "arm64")

    assert hardware.is_macos() is True
    assert hardware.is_apple_silicon() is True

    monkeypatch.setattr(hardware.platform, "machine", lambda: "x86_64")
    assert hardware.is_apple_silicon() is False


def test_mac_chip_brand_success_non_mac_and_failure(monkeypatch) -> None:
    monkeypatch.setattr(hardware.platform, "system", lambda: "Linux")
    assert hardware.mac_chip_brand() is None

    monkeypatch.setattr(hardware.platform, "system", lambda: "Darwin")
    monkeypatch.setattr(hardware.subprocess, "check_output", lambda *args, **kwargs: "Apple M3 Pro\n")
    assert hardware.mac_chip_brand() == "Apple M3 Pro"

    monkeypatch.setattr(
        hardware.subprocess,
        "check_output",
        lambda *args, **kwargs: (_ for _ in ()).throw(subprocess.SubprocessError("boom")),
    )
    assert hardware.mac_chip_brand() is None


def test_total_ram_gb_macos_linux_and_failure(monkeypatch) -> None:
    monkeypatch.setattr(hardware.platform, "system", lambda: "Darwin")
    monkeypatch.setattr(hardware.subprocess, "check_output", lambda *args, **kwargs: str(16 * 1024**3))
    assert hardware.total_ram_gb() == 16

    monkeypatch.setattr(hardware.platform, "system", lambda: "Linux")

    def fake_sysconf(name: str):
        return {"SC_PHYS_PAGES": 1024, "SC_PAGE_SIZE": 1024 * 1024}[name]

    monkeypatch.setattr(hardware.os, "sysconf", fake_sysconf)
    assert hardware.total_ram_gb() == 1

    monkeypatch.setattr(hardware.os, "sysconf", lambda name: (_ for _ in ()).throw(OSError("no sysconf")))
    assert hardware.total_ram_gb() is None


def test_nvidia_vram_gb_absent_success_empty_and_failure(monkeypatch) -> None:
    monkeypatch.setattr(hardware.shutil, "which", lambda command: None)
    assert hardware.nvidia_vram_gb() is None

    monkeypatch.setattr(hardware.shutil, "which", lambda command: "/usr/bin/nvidia-smi")
    monkeypatch.setattr(
        hardware.subprocess,
        "check_output",
        lambda *args, **kwargs: "8192\nbad\n24576\n\n",
    )
    assert hardware.nvidia_vram_gb() == 24

    monkeypatch.setattr(hardware.subprocess, "check_output", lambda *args, **kwargs: "bad\n")
    assert hardware.nvidia_vram_gb() is None

    monkeypatch.setattr(
        hardware.subprocess,
        "check_output",
        lambda *args, **kwargs: (_ for _ in ()).throw(subprocess.SubprocessError("boom")),
    )
    assert hardware.nvidia_vram_gb() is None


def test_recommend_ollama_model_uses_vram_before_ram_and_tiers(monkeypatch) -> None:
    assert hardware.recommend_ollama_model(ram_gb=None, vram_gb=None) == hardware.DEFAULT_OLLAMA_MODEL
    assert hardware.recommend_ollama_model(ram_gb=64, vram_gb=8) == "qwen2.5-coder:7b"
    assert hardware.recommend_ollama_model(ram_gb=64, vram_gb=None) == "qwen2.5-coder:32b"
    assert hardware.recommend_ollama_model(ram_gb=24, vram_gb=None) == "qwen2.5-coder:14b"
    assert hardware.recommend_ollama_model(ram_gb=8, vram_gb=None) == "qwen2.5-coder:7b"

    monkeypatch.setattr(hardware, "total_ram_gb", lambda: 48)
    assert hardware.recommend_ollama_model() == "qwen2.5-coder:32b"


def test_host_summary_labels(monkeypatch) -> None:
    apple = hardware.HostSummary(
        is_macos=True,
        is_apple_silicon=True,
        chip=None,
        ram_gb=32,
        vram_gb=None,
        recommended_ollama_model="qwen2.5-coder:14b",
    )
    assert apple.platform_label == "Apple Silicon"
    assert apple.chip_label == "Apple Silicon"
    assert apple.memory_label == "32 GB"

    intel = hardware.HostSummary(
        is_macos=True,
        is_apple_silicon=False,
        chip="Intel",
        ram_gb=None,
        vram_gb=None,
        recommended_ollama_model="qwen2.5-coder:7b",
    )
    assert intel.platform_label == "Mac (Intel)"
    assert intel.chip_label == "Intel"
    assert intel.memory_label == "unknown RAM"

    gpu = hardware.HostSummary(
        is_macos=False,
        is_apple_silicon=False,
        chip=None,
        ram_gb=64,
        vram_gb=12,
        recommended_ollama_model="qwen2.5-coder:7b",
    )
    assert gpu.chip_label == "discrete GPU host"
    assert gpu.memory_label == "12 GB VRAM (GPU)"

    monkeypatch.setattr(hardware.platform, "system", lambda: "")
    host = hardware.HostSummary(
        is_macos=False,
        is_apple_silicon=False,
        chip=None,
        ram_gb=None,
        vram_gb=None,
        recommended_ollama_model="qwen2.5-coder:7b",
    )
    assert host.platform_label == "Host"
    assert host.chip_label == "this host"


def test_describe_host_combines_detected_values(monkeypatch) -> None:
    monkeypatch.setattr(hardware, "total_ram_gb", lambda: 64)
    monkeypatch.setattr(hardware, "nvidia_vram_gb", lambda: 8)
    monkeypatch.setattr(hardware, "is_macos", lambda: False)
    monkeypatch.setattr(hardware, "is_apple_silicon", lambda: False)
    monkeypatch.setattr(hardware, "mac_chip_brand", lambda: None)

    summary = hardware.describe_host()

    assert summary.ram_gb == 64
    assert summary.vram_gb == 8
    assert summary.recommended_ollama_model == "qwen2.5-coder:7b"
