---
name: ai-training
title: AI / ML Model Training Intake
description: Before writing model-training code, retrieve current framework versions, hardware/driver constraints, deprecations, dataset/eval practices, and the right tooling — like a senior ML engineer.
triggers:
  keywords: [train, training, model, pytorch, torch, tensorflow, keras, jax, huggingface, transformers, finetune, "fine-tune", dataset, llm, ml, cuda, embedding]
  globs: ["*.ipynb", "train.py", "trainer.py", "requirements.txt", "environment.yml", "pyproject.toml", "accelerate*.yaml", "deepspeed*.json"]
---

# AI / ML Model Training Intake

Do this **before** writing or changing model-training code. Frameworks, hardware
drivers, and recommended recipes change frequently, and silent version mismatches
waste expensive compute. Confirm against the framework docs and the project's
environment files.

## Establish current state first

1. **Framework & environment** — read `requirements.txt` / `environment.yml` /
   `pyproject.toml`: the framework (PyTorch, TensorFlow/Keras, JAX, Hugging Face
   `transformers`/`accelerate`/`peft`), its version, Python version, and the
   CUDA/cuDNN/ROCm requirement. Match the existing stack.
2. **Hardware & precision** — available accelerators (GPU/TPU), memory budget, and
   the precision strategy (fp32/fp16/bf16, mixed precision, quantization). Confirm the
   training approach fits the hardware (single-GPU vs distributed: DDP/FSDP/DeepSpeed).
3. **Deprecations & API shifts** — note moved/renamed APIs (e.g. `transformers`
   trainer/argument changes, optimizer/scheduler APIs, dataset loaders) and avoid
   patterns deprecated in the installed version.
4. **Recommended practices** — reproducibility (seed, deterministic flags, pinned
   versions), data pipeline (streaming vs in-memory, tokenization), checkpointing and
   resume, and evaluation (held-out split, the metric that actually matters, no leakage
   between train/val/test).
5. **Cost & safety** — estimate compute/time before launching long runs; log metrics
   (Weights & Biases / TensorBoard) and checkpoint so a crash isn't a total loss.

## Tools

- `python`/`accelerate launch`/`torchrun` for training entry points.
- Experiment tracking (W&B, MLflow, TensorBoard) and `nvidia-smi` for GPU monitoring.
- `datasets`/`dvc` for data, and a pinned environment (`uv`/`conda`/`pip-tools`).

## What to record before coding

- Framework + version, Python + CUDA versions, and the hardware/precision plan.
- Deprecated APIs to avoid and their current replacements.
- How the run is validated: the eval split, the metric, the checkpoint cadence, and a
  short smoke run (a few steps) before the full training job.

Start with a tiny smoke run to validate the pipeline end-to-end before committing to a
long, expensive training run.
