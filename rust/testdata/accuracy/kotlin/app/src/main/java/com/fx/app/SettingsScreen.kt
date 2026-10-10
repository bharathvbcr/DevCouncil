package com.fx.app

// K33 (added for the dead-policy card, second pass): a component registered in
// AndroidManifest.xml whose supertype is the app's own `BaseScreen`, so the
// Android-supertype rule cannot recognise it. The manifest is what registers it.
open class BaseScreen {
    open fun show() {}
}

class SettingsScreen : BaseScreen()
