package gaps

import android.app.Service

class PollService : Service() {
    fun label(): String = getString(1)
}
