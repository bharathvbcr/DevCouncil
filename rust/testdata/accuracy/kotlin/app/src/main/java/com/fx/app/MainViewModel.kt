package com.fx.app

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import com.fx.core.data.LoadResult
import com.fx.core.data.Task
import com.fx.core.util.GetTasksUseCase
import dagger.hilt.android.lifecycle.HiltViewModel
import javax.inject.Inject
import kotlinx.coroutines.async
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.launch

// K02 ViewModel + StateFlow; K03 Hilt; K04 launch/async; K26 invoke
@HiltViewModel
class MainViewModel @Inject constructor(
    private val getTasks: GetTasksUseCase,
) : ViewModel() {
    private val _state = MutableStateFlow<LoadResult>(LoadResult.Loading)
    val state: StateFlow<LoadResult> = _state

    fun refresh() {
        viewModelScope.launch {
            _state.value = getTasks()                      // EXPECT refresh -> GetTasksUseCase.invoke
            val count = async { countDone(emptyList()) }   // EXPECT refresh -> MainViewModel.countDone
            count.await()
        }
    }

    // K09 when over sealed class
    fun describe(result: LoadResult): String = when (result) {
        is LoadResult.Ok -> summarize(result.tasks)        // EXPECT describe -> MainViewModel.summarize
        is LoadResult.Failed -> result.reason
        LoadResult.Loading -> "…"
    }

    private fun summarize(tasks: List<Task>) = tasks.size.toString()
    private suspend fun countDone(tasks: List<Task>) = tasks.count { it.done }
}
