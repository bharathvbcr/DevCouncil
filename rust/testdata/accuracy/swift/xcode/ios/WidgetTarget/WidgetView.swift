struct WidgetView {
    func render() -> String {
        let a = Theme.accent()              // X4 -> WidgetTarget/Theme.swift Theme.accent
        let b = sharedFormat()              // X5 -> Shared/Formatter.swift sharedFormat
        return a + b
    }
}
