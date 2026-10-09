package com.fx.core.util

import com.fx.core.data.Task

// K15 top-level function in a file NOT named for it
fun formatDuration(minutes: Int): String = "${minutes}m"

// K05 extension function on a project type and on a stdlib type
fun Task.isOverdue(now: Long): Boolean = id < now
fun String.slugify(): String = lowercase().replace(' ', '-')

// K06 extension property
val Task.label: String
    get() = title.slugify()                               // EXPECT Task.label(getter) -> String.slugify

// K25 infix function
infix fun Int.every(step: Int): IntProgression = this..step

// K24 inline reified
inline fun <reified T> castOrNull(value: Any?): T? = value as? T

// K26 operator invoke use case
class GetTasksUseCase(private val repo: com.fx.core.data.TaskRepository) {
    suspend operator fun invoke() = repo.load()            // EXPECT invoke -> TaskRepository.load
}
