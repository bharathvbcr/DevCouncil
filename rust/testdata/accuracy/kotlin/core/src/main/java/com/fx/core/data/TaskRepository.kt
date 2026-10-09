package com.fx.core.data

import javax.inject.Inject
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

// K10 interface with default method + implementation
interface TaskRepository {
    suspend fun load(): LoadResult
    suspend fun refresh(): LoadResult = load()          // EXPECT refresh -> TaskRepository.load (implicit this)
}

// K03 @Inject constructor; K04 withContext; K16/K17 call through typed ctor params
class TaskRepositoryImpl @Inject constructor(
    private val dao: TaskDao,
    private val api: TaskApi,
) : TaskRepository {
    override suspend fun load(): LoadResult = withContext(Dispatchers.IO) {
        val remote = api.fetchRemote()                    // EXPECT load -> TaskApi.fetchRemote
        val local = dao.all()                             // EXPECT load -> TaskDao.all
        LoadResult.Ok(remote + local)                     // EXPECT load -> LoadResult.Ok (ctor)
    }
}
