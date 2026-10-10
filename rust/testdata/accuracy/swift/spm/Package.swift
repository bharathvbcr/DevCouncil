// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "Demo",
    platforms: [.macOS(.v14)],
    targets: [
        .target(name: "Core"),
        .target(name: "DemoUI", dependencies: ["Core"]),
        .executableTarget(name: "DemoMain", dependencies: ["Core"]),
        .testTarget(name: "CoreTests", dependencies: ["Core"]),
    ]
)
