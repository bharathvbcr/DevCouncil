package com.fx.app

import android.app.Application
import dagger.hilt.android.HiltAndroidApp

// K19 Application entry point (annotated + manifest-declared). EXPECT live.
@HiltAndroidApp
class FxApp : Application() {
    override fun onCreate() {
        super.onCreate()                                  // EXPECT no self-edge
        Registry.register("app")                          // EXPECT onCreate -> Registry.register
    }
}
