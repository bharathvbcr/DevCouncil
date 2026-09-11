---
name: game-dev
title: Game Development (Unity / Unreal / Godot) Intake
description: Before changing game code, retrieve current engine versions, the frame-loop and performance budget, asset/scene conventions, and the right build/play CLI commands — like a senior game engineer briefing themselves on the engine and project.
triggers:
  keywords: [unity, unreal, "unreal engine", godot, gameplay, gamedev, "game engine", "game loop", shader, hlsl, glsl, ecs, dots, "frame rate", physics, collider, gdscript]
  globs: ["*.unity", "*.uproject", "*.uplugin", "project.godot", "*.gd", "*.tscn", "*.tres", "Assets", "ProjectSettings"]
---

# Game Development (Unity / Unreal / Godot) Intake

Do this **before** writing or changing game code. Don't rely on training data — engine APIs
and recommended patterns change between major versions, and gameplay code runs every frame so
small mistakes show up as stutter or crashes. Confirm against the engine's current docs and the
project's own version and conventions.

## Establish current state first

1. **Engine & version in use** — Unity (`ProjectSettings/ProjectVersion.txt`, render pipeline
   URP/HDRP/Built-in, input system), Unreal (`.uproject` engine association, Blueprint vs C++),
   or Godot (`project.godot`, Godot 3 vs 4, GDScript vs C#/GDExtension). Match it exactly.
2. **The frame loop & budget** — what runs per-frame (`Update`/`Tick`/`_process`) vs fixed-step
   physics (`FixedUpdate`/`_physics_process`). Keep per-frame work cheap; respect the target
   frame rate and platform budget (mobile/console/VR are tighter).
3. **Allocations & GC** — avoid per-frame allocations (no `new`/LINQ in `Update`, pool objects);
   GC spikes cause hitches. In Unreal, mind UObject lifetime/GC and `TWeakObjectPtr`.
4. **Scenes, prefabs & assets** — follow the project's scene/prefab/asset layout; serialized
   fields and `.meta`/GUID references are easy to break. Don't reformat scene/asset files by hand.
5. **Determinism & netcode** — if gameplay is networked or replay-based, keep simulation
   deterministic and authority/replication correct.

## Build & CLI tools

- Unity: batch mode `Unity -batchmode -runTests`/`-buildTarget`, the Test Runner, the Profiler.
- Unreal: `UnrealBuildTool`/`RunUAT BuildCookRun`, Automation tests, Unreal Insights.
- Godot: `godot --headless --export-release`, `godot --headless --run-tests` / GUT.

## What to record before coding

- The engine + version, render pipeline / scripting backend, and target platform budget.
- The per-frame vs fixed-step work the change adds, and how you keep it allocation-light.
- The scenes/prefabs/assets touched and any serialized references at risk.
- The build/play/test commands (and a profiler check) that prove performance and correctness.

Don't broaden the change beyond the task — no incidental engine upgrades or scene-wide edits
(see the surgical-changes rule in core-engineering).
