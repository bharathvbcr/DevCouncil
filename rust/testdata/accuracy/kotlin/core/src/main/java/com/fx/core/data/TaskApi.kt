package com.fx.core.data

import retrofit2.Retrofit
import retrofit2.http.GET

// K17 Retrofit interface
interface TaskApi {
    @GET("tasks")
    suspend fun fetchRemote(): List<Task>
}

fun buildApi(retrofit: Retrofit): TaskApi = retrofit.create(TaskApi::class.java)
