---
name: android
title: Android App Development Intake
description: Before writing Android code, retrieve current SDK/AGP/Kotlin/Compose versions, platform behavior changes, deprecations, recommended Jetpack libraries, and the right CLI/build tools — like a senior Android engineer briefing themselves.
triggers:
  keywords: [android, kotlin, jetpack, compose, gradle, agp, apk, aab, espresso, room, hilt]
  globs: ["build.gradle", "build.gradle.kts", "settings.gradle", "settings.gradle.kts", "AndroidManifest.xml", "*.kt", "gradle/libs.versions.toml"]
---

# Android App Development Intake

Do this **before** writing or changing Android code. Don't rely on training data —
Android moves fast and the answer that was right a year ago is often deprecated.
Confirm against `developer.android.com`, the AGP/Kotlin/Compose release notes, and
the project's own version catalog.

## Establish current state first

1. **Toolchain versions in use** — read `gradle/libs.versions.toml`, `build.gradle[.kts]`,
   and `gradle-wrapper.properties`: `compileSdk` / `targetSdk` / `minSdk`, Android
   Gradle Plugin (AGP), Kotlin, Gradle, Jetpack Compose / Compose Compiler, and core
   Jetpack libraries.
2. **Latest platform & API level** — current stable Android release, its behavior
   changes, and the newest `targetSdk` Google Play will require. Note anything that
   affects this task.
3. **Deprecations & removals** — APIs deprecated or removed between the project's
   `targetSdk` and current (e.g. background-execution limits, storage/scoped-storage
   changes, notification/permission model, `AsyncTask`, `onBackPressed`, implicit
   intents). List the ones this change touches and their replacements.
4. **Recommended libraries** — prefer current Jetpack: Compose (+ adaptive layouts),
   Navigation, Room, WorkManager, Hilt, DataStore (not SharedPreferences for new code),
   Lifecycle/ViewModel. Confirm the recommended artifact and version.
5. **Guidelines** — Material 3, adaptive/large-screen & foldable support, edge-to-edge,
   the runtime permission model, and predictive back.

## Build & CLI tools

- `gradlew`/`gradlew.bat` for builds; `adb` for device/log work; `R8` for shrink/obfuscate.
- The `android` command-line tool and `sdkmanager`/`avdmanager` for SDK and emulator setup.
- Android Studio for profiling (Perfetto traces) and Compose preview.

## What to record before coding

- Effective `minSdk`/`targetSdk` and the version catalog entries you will use.
- Deprecated APIs to avoid and their modern replacements, with migration steps.
- The build/test commands you will run (`gradlew :app:assembleDebug`, `:app:testDebugUnitTest`,
  connected/instrumented tests) so the change is verifiable.

If the project mixes Views and Compose, match the file you're editing; don't migrate
unrelated screens as a side effect (see the surgical-changes rule in core-engineering).
