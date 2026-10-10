import SwiftUI
import Core

// C21 @main App entry
@main
struct DemoApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) var appDelegate
    var body: some Scene {
        WindowGroup { MainView() }               // -> MainView
    }
}

struct MainView: View {
    var body: some View { Text(greetAll().joined()) }   // C24 cross-module -> Core greetAll
}

// C20 AppKit delegate: framework-called methods
final class AppDelegate: NSObject, NSApplicationDelegate, EngineDelegate {
    let engine = Engine()
    func applicationDidFinishLaunching(_ notification: Notification) {   // framework-called, not dead
        engine.delegate = self
        engine.run()                              // -> Engine.run
    }
    func engineDidFinish(_ engine: Engine) {}     // called via delegate?.engineDidFinish
}

// C24 same-named type in two modules: DemoMain.Theme vs Core has none; DemoUI has none.
struct Theme { static func accent() -> String { "blue" } }
