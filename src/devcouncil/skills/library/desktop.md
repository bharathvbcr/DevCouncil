---
name: desktop
title: Cross-Platform Desktop (Electron / Tauri / Qt) Intake
description: Before changing desktop-app code, retrieve current framework versions, the process/IPC and security model, packaging/auto-update guidance, and the right build/run CLI commands across OSes — like a senior desktop engineer briefing themselves.
triggers:
  keywords: [electron, tauri, qt, qml, gtk, wxwidgets, "desktop app", "system tray", ipc, "main process", "renderer process", webview, "auto-update", "code signing", notarization]
  globs: ["tauri.conf.json", "*.qml", "*.pro", "CMakeLists.txt.user", "electron-builder.yml", "electron-builder.json", "forge.config.js", "*.desktop"]
---

# Cross-Platform Desktop (Electron / Tauri / Qt) Intake

Do this **before** writing or changing desktop-app code. Don't rely on training data — these
frameworks change their process and security models, and a desktop app must build, sign, and
behave correctly across Windows, macOS, and Linux. Confirm against the framework's current docs
and the project's own pinned versions.

## Establish current state first

1. **Framework & version in use** — Electron (version, `package.json`), Tauri (`tauri.conf.json`,
   Rust + WebView2/WKWebView), or Qt (version, Widgets vs QML). Match what's there.
2. **Process & IPC model** — main/renderer (Electron) or core/WebView (Tauri) boundaries, and the
   IPC surface. Keep the boundary explicit; validate everything crossing it.
3. **Security** — Electron: `contextIsolation` on, `nodeIntegration` off, a `contextBridge`
   preload, and a strict CSP; Tauri: the allowlist/capabilities scoped to the minimum. Never
   expose Node/shell/filesystem broadly to the renderer; treat loaded web content as untrusted.
4. **Platform integration** — file dialogs, tray, notifications, menus, deep links, and
   permissions differ per OS. Implement and test the ones this change touches on each target.
5. **Packaging & updates** — code signing (Windows Authenticode, macOS notarization), the
   installer format, and the auto-update channel. A change can silently break signing or updates.

## Build & CLI tools

- Electron: `npm`/`pnpm` scripts, `electron .`, `electron-builder`/`electron-forge make`, Spectron/
  Playwright for e2e.
- Tauri: `cargo tauri dev`/`cargo tauri build`, plus the frontend's own build.
- Qt: `qmake`/`cmake` + `make`/`ninja`, `windeployqt`/`macdeployqt`, `ctest`.

## What to record before coding

- The framework + version and the OS targets you must support.
- The IPC surface touched and the security settings (isolation/allowlist/CSP) that must hold.
- The platform-integration features changed and how they're tested per OS.
- The build/sign/package commands and tests that prove the change works and still ships.

Don't broaden the change beyond the task — no incidental framework upgrades or weakening the
security model for convenience (see the surgical-changes rule in core-engineering).
