"""Unit tests for hardware detection helpers."""

from __future__ import annotations

import subprocess
from unittest.mock import patch

from devcouncil import hardware


def test_platform_helpers():
    assert isinstance(hardware.is_macos(), bool)
    assert isinstance(hardware.is_apple_silicon(), bool)


def test_mac_chip_brand_and_ram_paths():
    with patch.object(hardware, "is_macos", return_value=False):
        assert hardware.mac_chip_brand() is None
    with patch.object(hardware, "is_macos", return_value=True):
        with patch("subprocess.check_output", return_value="Apple M3 Pro\n"):
            assert hardware.mac_chip_brand() == "Apple M3 Pro"
        with patch("subprocess.check_output", side_effect=OSError("no")):
            assert hardware.mac_chip_brand() is None

    with patch.object(hardware, "is_macos", return_value=True):
        with patch("subprocess.check_output", return_value=str(16 * 1024**3)):
            assert hardware.total_ram_gb() == 16.0
    with patch.object(hardware, "is_macos", return_value=False):
        with patch("os.sysconf", side_effect=[1024, 4096]):
            assert hardware.total_ram_gb() == (1024 * 4096) / (1024**3)
    with patch.object(hardware, "is_macos", return_value=True):
        with patch("subprocess.check_output", side_effect=ValueError("bad")):
            assert hardware.total_ram_gb() is None


def test_nvidia_vram_gb_paths():
    with patch("shutil.which", return_value=None):
        assert hardware.nvidia_vram_gb() is None
    with patch("shutil.which", return_value="/usr/bin/nvidia-smi"):
        with patch("subprocess.check_output", return_value="8192\n4096\n"):
            assert abs(hardware.nvidia_vram_gb() - 8.0) < 0.01
        with patch("subprocess.check_output", return_value="not-a-number\n"):
            assert hardware.nvidia_vram_gb() is None
        with patch(
            "subprocess.check_output",
            side_effect=subprocess.SubprocessError("fail"),
        ):
            assert hardware.nvidia_vram_gb() is None


def test_recommend_and_host_summary_labels():
    assert hardware.recommend_ollama_model(ram_gb=None, vram_gb=None) in {
        hardware.DEFAULT_OLLAMA_MODEL,
        "qwen2.5-coder:14b",
        "qwen2.5-coder:32b",
        "qwen2.5-coder:7b",
    }
    assert hardware.recommend_ollama_model(ram_gb=64, vram_gb=8) == "qwen2.5-coder:7b"
    assert hardware.recommend_ollama_model(ram_gb=64, vram_gb=None) == "qwen2.5-coder:32b"
    assert hardware.recommend_ollama_model(ram_gb=30, vram_gb=None) == "qwen2.5-coder:14b"
    with patch.object(hardware, "total_ram_gb", return_value=None):
        assert hardware.recommend_ollama_model() == hardware.DEFAULT_OLLAMA_MODEL

    summary = hardware.HostSummary(
        is_macos=True,
        is_apple_silicon=True,
        chip="Apple M2",
        ram_gb=32.0,
        recommended_ollama_model="qwen2.5-coder:14b",
        vram_gb=None,
    )
    assert summary.ram_label == "32 GB"
    assert summary.vram_label is None
    assert summary.chip_label == "Apple M2"
    assert summary.platform_label == "Apple Silicon"
    assert summary.memory_label == "32 GB"

    gpu = hardware.HostSummary(
        is_macos=False,
        is_apple_silicon=False,
        chip=None,
        ram_gb=64.0,
        recommended_ollama_model="qwen2.5-coder:7b",
        vram_gb=8.0,
    )
    assert gpu.vram_label == "8 GB VRAM"
    assert gpu.chip_label == "discrete GPU host"
    assert gpu.memory_label.endswith("(GPU)")
    assert gpu.platform_label

    unknown = hardware.HostSummary(
        is_macos=False,
        is_apple_silicon=False,
        chip=None,
        ram_gb=None,
        recommended_ollama_model=hardware.DEFAULT_OLLAMA_MODEL,
    )
    assert unknown.ram_label == "unknown RAM"
    assert unknown.chip_label == "this host"

    with patch.object(hardware, "total_ram_gb", return_value=16.0):
        with patch.object(hardware, "nvidia_vram_gb", return_value=None):
            with patch.object(hardware, "mac_chip_brand", return_value=None):
                host = hardware.describe_host()
                assert host.ram_gb == 16.0
                assert host.recommended_ollama_model
