package com.fx.app

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

// K19 manifest-only component: no annotation, no Kotlin caller. EXPECT class live (manifest), onReceive live (override).
class BootReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        rescheduleAll()                                   // EXPECT onReceive -> BootReceiver.rescheduleAll
    }
    private fun rescheduleAll() = Unit
}
