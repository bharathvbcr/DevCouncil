---
name: mobile-cross-platform
title: Cross-Platform Mobile (Flutter / React Native) Intake
description: Before changing Flutter or React Native code, retrieve current SDK/framework versions, platform-channel and native-module guidance, deprecations, and the right build/run CLI commands for both iOS and Android — like a senior cross-platform mobile engineer briefing themselves.
triggers:
  keywords: [flutter, dart, "react native", "react-native", expo, "cross-platform mobile", cupertino]
  globs: ["pubspec.yaml", "pubspec.lock", "*.dart", "metro.config.js", "app.config.js", "app.config.ts", "react-native.config.js", "expo.json"]
---

# Cross-Platform Mobile (Flutter / React Native) Intake

Do this **before** writing or changing Flutter/React Native code. Don't rely on training
data — these frameworks and their native toolchains move quickly, and a change that builds
on one platform can break the other. Confirm against the framework's current docs and the
project's own pinned versions.

## Establish current state first

1. **Framework & SDK versions in use** — Flutter/Dart SDK from `pubspec.yaml` and
   `.fvm`/`flutter --version`; React Native / Expo from `package.json` and the RN/Expo release
   notes. Note the JS engine (Hermes) and architecture (new arch / Fabric / TurboModules).
2. **Both native platforms** — iOS (CocoaPods/SPM, min iOS, Xcode) and Android (Gradle/AGP,
   min/target SDK). A dependency or native change must build and run on **both**.
3. **Deprecations & breaking changes** — between the pinned versions and current (Flutter API
   removals, RN bridge → JSI/TurboModules, deprecated Expo modules). List what this change touches.
4. **State & navigation** — match the project's existing approach (Riverpod/Bloc/Provider for
   Flutter; Redux/Zustand/Context + React Navigation/Expo Router for RN); don't introduce a new one.
5. **Native interop** — if the change needs platform code, use the current channel/module API
   (Flutter platform channels / Pigeon; RN TurboModules/native modules) and implement both sides.

## Build & CLI tools

- Flutter: `flutter pub get`, `flutter run`, `flutter build apk|ios`, `flutter test`,
  `flutter analyze`, `dart format`.
- React Native / Expo: `npm`/`yarn`/`pnpm`, `npx react-native run-android|run-ios`,
  `pod install` (iOS), `npx expo start`, `eas build`, `npm test`.

## What to record before coding

- The framework/SDK versions and the iOS/Android minimums you must support.
- Deprecated APIs to avoid and their modern replacements, with migration steps.
- The state/navigation libraries already in use, and any native-module work needed on both sides.
- The build/test/analyze commands you will run on both platforms to prove the change works.

If the project mixes platform-specific code, match the file you're editing; don't migrate
unrelated screens as a side effect (see the surgical-changes rule in core-engineering).
