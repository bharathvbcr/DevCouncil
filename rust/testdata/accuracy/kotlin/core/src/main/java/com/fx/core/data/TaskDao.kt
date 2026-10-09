package com.fx.core.data

import androidx.room.Dao
import androidx.room.Query

// K16 Room DAO: interface method with no body; implementation is generated.
@Dao
interface TaskDao {
    @Query("SELECT * FROM task")
    suspend fun all(): List<Task>

    @Query("DELETE FROM task")
    suspend fun clear()
}
