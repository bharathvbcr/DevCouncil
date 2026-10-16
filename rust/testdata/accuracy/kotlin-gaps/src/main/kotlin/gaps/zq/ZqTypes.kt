package gaps.zq

interface ZqRepo {
    fun zqFind(id: Int): String
}

class ZqRepoImpl : ZqRepo {
    override fun zqFind(id: Int): String = id.toString()
}

class ZqService {
    fun zqServe(): String = "ok"
}

class ZqHolder {
    private fun zqBuild() = ZqService()

    private val svc = zqBuild()

    fun zqGo(): String = svc.zqServe()
}

fun <T> zqEntry(type: Class<T>): T = throw IllegalStateException(type.name)

fun <reified T> zqMock(): T = throw IllegalStateException()

fun zqUseMock(): String {
    val repo = zqMock<ZqRepo>()
    return repo.zqFind(1)
}

fun zqUseEntry(): String {
    val entry = zqEntry(ZqRepo::class.java)
    return entry.zqFind(2)
}
