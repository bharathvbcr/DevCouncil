package com.fx.app

import com.fx.core.data.TaskApi
import com.fx.core.data.TaskDao
import com.fx.core.data.TaskRepository
import com.fx.core.data.TaskRepositoryImpl
import com.fx.core.data.buildApi
import dagger.Module
import dagger.Provides
import dagger.hilt.InstallIn
import dagger.hilt.components.SingletonComponent
import retrofit2.Retrofit

// K03 Hilt @Module @Provides: provider is called by generated code only.
@Module
@InstallIn(SingletonComponent::class)
object RepoModule {
    @Provides
    fun provideRepo(dao: TaskDao, api: TaskApi): TaskRepository = TaskRepositoryImpl(dao, api)  // EXPECT provideRepo -> TaskRepositoryImpl (ctor)

    @Provides
    fun provideApi(retrofit: Retrofit): TaskApi = buildApi(retrofit)   // EXPECT provideApi -> buildApi (cross-module top-level)
}
