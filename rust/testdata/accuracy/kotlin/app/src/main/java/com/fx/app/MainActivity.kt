package com.fx.app

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.viewModels
import com.fx.core.data.Task
import com.fx.core.util.formatDuration
import com.fx.core.util.formatDuration as fmt
import com.fx.core.util.isOverdue
import com.fx.core.util.label
import com.fx.core.util.slugify
import com.fx.core.util.every
import com.fx.core.util.castOrNull
import com.fx.legacy.LegacyMath
import dagger.hilt.android.AndroidEntryPoint

// K19 Activity; K14 delegation; K11 scope functions; K13 ctor; K15 import/alias
@AndroidEntryPoint
class MainActivity : ComponentActivity() {
    private val vm: MainViewModel by viewModels()                // K14
    private val clock by lazy { Clock.create() }                 // K14 + K07  EXPECT <file/field init> -> Clock.create

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent { AppNavGraph() }                             // EXPECT onCreate -> AppNavGraph
        vm.refresh()                                             // EXPECT onCreate -> MainViewModel.refresh (receiver typed by delegated property)
        val task = Task(1, "Write tests")                        // EXPECT onCreate -> Task (ctor, K13)
        task.let { it.complete() }                               // EXPECT onCreate -> Task.complete (K11 `it`)
        task.run { complete() }                                  // EXPECT onCreate -> Task.complete (K11 `this`)
        with(task) { isOverdue(5) }                              // EXPECT onCreate -> Task.isOverdue (K05 via with)
        task.isOverdue(10)                                       // EXPECT onCreate -> Task.isOverdue (K05)
        "A B".slugify()                                          // EXPECT onCreate -> String.slugify (K05 stdlib receiver)
        println(task.label)                                      // EXPECT reference -> Task.label (K06)
        fmt(3)                                                   // EXPECT onCreate -> formatDuration (K15 alias)
        formatDuration(4)                                        // EXPECT onCreate -> formatDuration (K15)
        1 every 3                                                // EXPECT onCreate -> Int.every (K25)
        castOrNull<Task>(task)                                   // EXPECT onCreate -> castOrNull (K24)
        clock.elapsed(10).also { log(it) }                       // EXPECT onCreate -> Clock.elapsed ; onCreate -> MainActivity.log
        buildPage { section("intro") }                           // EXPECT onCreate -> buildPage ; onCreate -> Page.section (K12)
        LegacyMath.twice(2)                                      // EXPECT onCreate -> LegacyMath.twice (K23 Java)
    }

    private fun log(value: Long) = println(value)
}
