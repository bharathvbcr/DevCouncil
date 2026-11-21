package gaps

import android.app.Service

open class BaseSvc : Service() {
    fun inheritedHelper(): Int = 1
}

class ChildSvc : BaseSvc() {
    fun go(): Int = inheritedHelper()
}
