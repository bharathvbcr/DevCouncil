---
name: windows
title: Windows App Development Intake
description: Before writing Windows desktop code, retrieve current .NET/WinUI/WPF versions, supported targets, deprecations, recommended frameworks, and the right CLI/build tools — like a senior Windows engineer.
triggers:
  keywords: [windows, wpf, winui, winforms, uwp, win32, dotnet, ".net", csharp, "c#", xaml, maui, msix]
  globs: ["*.csproj", "*.sln", "*.xaml", "*.cs", "Directory.Build.props", "global.json", "*.vcxproj"]
---

# Windows App Development Intake

Do this **before** writing or changing Windows desktop code. Confirm against
Microsoft Learn, the .NET release notes, and the project files — the Windows app
stack has several overlapping UI frameworks and the right choice depends on targets.

## Establish current state first

1. **Toolchain & targets** — read `*.csproj` / `global.json` / `Directory.Build.props`:
   `TargetFramework(s)` (e.g. `net8.0-windows`), .NET SDK version, and the UI stack in
   use (WPF, WinUI 3 / Windows App SDK, WinForms, UWP, or .NET MAUI). Identify which one
   this file belongs to and stay in it.
2. **Latest .NET & runtime** — current LTS/STS .NET release and whether the project
   should target it; note Windows version / Windows App SDK minimums.
3. **Deprecations & migrations** — UWP is in maintenance; new desktop work generally
   targets WinUI 3 (Windows App SDK) or WPF on modern .NET. `.NET Framework` (4.x) is
   legacy — don't introduce it for new code. Note any deprecated APIs the change touches.
4. **Recommended frameworks** — packaging via MSIX; MVVM (e.g. CommunityToolkit.Mvvm);
   dependency injection via `Microsoft.Extensions.DependencyInjection`; async/await over
   blocking calls. Confirm current recommended packages and versions on NuGet.
5. **Guidelines** — Fluent design, accessibility (UI Automation), and packaging/signing
   requirements relevant to the change.

## Build & CLI tools

- `dotnet build` / `dotnet test` / `dotnet publish`; `msbuild` for full solutions.
- `winget` for tooling; `nuget`/`dotnet add package` for dependencies.
- Visual Studio diagnostics for profiling.

## What to record before coding

- Target framework(s), .NET SDK version, and which UI stack the change belongs to.
- Deprecated APIs/frameworks to avoid and their modern replacements.
- The build/test commands you will run (`dotnet test`, `dotnet build -c Release`) so
  the change is verifiable.

Don't migrate a project between UI frameworks (e.g. WinForms → WinUI) as a side
effect of an unrelated task.
