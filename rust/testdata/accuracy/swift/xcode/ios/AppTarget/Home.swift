import CoreKit
struct Home {
    func render() -> String {
        _ = CoreKitEngine.run()             // X1 -> CoreKit Engine.swift CoreKitEngine.run
        let a = Theme.accent()              // X2 -> AppTarget/Theme.swift Theme.accent (own module shadows CoreKit.Theme)
        let b = sharedFormat()              // X3 -> Shared/Formatter.swift sharedFormat (same Xcode target, other folder)
        return a + b
    }
}
