---
name: ios
title: iOS / Apple Platform Development Intake
description: Before writing iOS/macOS code, retrieve current Swift/SwiftUI/Xcode versions, deployment targets, deprecations, recommended frameworks, and the right CLI/build tools — like a senior Apple-platform engineer.
triggers:
  keywords: [ios, ipados, macos, swift, swiftui, uikit, xcode, cocoapods, spm, swiftpm, combine]
  globs: ["*.swift", "*.xcodeproj", "*.xcworkspace", "Podfile", "Package.swift", "*.entitlements", "Info.plist"]
---

# iOS / Apple Platform Development Intake

Do this **before** writing or changing Swift/iOS code. Confirm against
`developer.apple.com`, the Xcode/Swift release notes, and the project's own settings —
Apple deprecates APIs aggressively and gates features on OS version.

## Establish current state first

1. **Toolchain & targets** — read the Xcode project / `Package.swift` / `Podfile`:
   Swift language version, Xcode version, iOS/macOS **deployment target**, and the
   dependency manager (Swift Package Manager vs CocoaPods — prefer SPM for new work).
2. **Latest OS & SDK** — the current iOS/macOS release, its SDK, and notable
   behavior/privacy changes (App Tracking Transparency, privacy manifests, required
   reason APIs). Note what gates this task.
3. **Deprecations & availability** — APIs deprecated or unavailable below the
   deployment target. Use `@available` / `if #available` correctly rather than
   assuming an API exists. Common shifts: UIKit → SwiftUI patterns, `NSUserActivity`,
   completion handlers → `async/await`, Combine → Swift Concurrency / Observation.
4. **Recommended frameworks** — SwiftUI (+ Observation `@Observable`), Swift
   Concurrency (`async/await`, actors), SwiftData vs Core Data, the current navigation
   API. Confirm the recommended approach for the deployment target.
5. **Guidelines** — Human Interface Guidelines, accessibility (Dynamic Type,
   VoiceOver), and App Store / privacy requirements relevant to the change.

## Build & CLI tools

- `xcodebuild` for builds/tests/archives; `xcrun simctl` for simulators.
- `swift build` / `swift test` for SwiftPM targets; `swiftformat`/`swiftlint` if configured.
- Instruments for profiling.

## What to record before coding

- Swift version, deployment target, and dependency-manager choice.
- Deprecated/unavailable APIs to avoid and their `@available`-guarded replacements.
- The build/test commands you will run (`xcodebuild test -scheme … -destination …`,
  or `swift test`) so the change is verifiable.

Match the paradigm of the file you're editing (UIKit vs SwiftUI); don't rewrite
unrelated screens as a side effect.
