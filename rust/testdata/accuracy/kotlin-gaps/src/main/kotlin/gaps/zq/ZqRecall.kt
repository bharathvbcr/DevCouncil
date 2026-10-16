package gaps.zq

import java.io.File

private fun String.zqShout(): String = this.uppercase()

private fun File.zqLabel(prefix: String): String = prefix + name

private fun List<String>.zqTotal(): Int = size

private fun <T> zqQuery(map: (File) -> T?): T? = map(File("x"))

fun zqNames(): List<String> = listOf("a")

fun zqLocalFromStdlib(title: String): String {
    val folded = title.lowercase()
    return folded.zqShout()
}

fun zqLambdaParameter(): String? = zqQuery { f -> f.zqLabel("p") }

fun zqIfExpression(flag: Boolean): Int {
    val files = if (flag) zqNames() else emptyList()
    return files.zqTotal()
}

fun Int.zqShout(): Int = this * 2
