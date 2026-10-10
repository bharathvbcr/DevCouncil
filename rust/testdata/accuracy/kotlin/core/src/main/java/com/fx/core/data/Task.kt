package com.fx.core.data

// K09 data class + sealed hierarchy
data class Task(val id: Long, val title: String, val done: Boolean = false) {
    fun complete(): Task = copy(done = true)
}

sealed class LoadResult {
    data class Ok(val tasks: List<Task>) : LoadResult()
    data class Failed(val reason: String) : LoadResult()
    object Loading : LoadResult()
}
