package org.zohara.agent.pages

import kotlinx.coroutines.CancellationException
import org.zohara.agent.core.net.ActivityResp
import org.zohara.agent.core.net.ApiErrors
import org.zohara.agent.core.net.ApiException
import org.zohara.agent.core.net.ArtifactItem
import org.zohara.agent.core.net.ChatDetail
import org.zohara.agent.core.net.ChatSummary
import org.zohara.agent.core.net.ScheduleRun
import org.zohara.agent.core.net.ScheduledResp
import org.zohara.agent.core.net.ScheduledTask
import org.zohara.agent.core.net.SendEvent
import kotlinx.coroutines.flow.Flow
import kotlinx.serialization.json.put
import org.zohara.agent.core.net.SearchHit
import org.zohara.agent.core.net.ZoApi

/** The ops the pages use. An interface so the unit tests can use a fake; [ZoPagesApi] is the real one. */
interface PagesApi {
  suspend fun chats(): List<ChatSummary>
  suspend fun chat(id: String): ChatDetail
  suspend fun deleteChat(id: String)
  suspend fun pinChat(id: String, on: Boolean)
  suspend fun search(query: String): List<SearchHit>
  suspend fun projects(): String
  suspend fun projectCreate(name: String, goal: String): String
  suspend fun artifacts(): List<ArtifactItem>
  suspend fun scheduled(): List<ScheduledTask>
  suspend fun scheduleAdd(task: String, whenText: String, repeat: String?): String
  suspend fun scheduleCancel(id: String)
  // Scheduled v2 (docs/SCHEDULED.md)
  suspend fun scheduledInfo(): ScheduledResp
  /** makes a task, or changes the one in [f]`.editId`; [tz] is the phone's time zone */
  suspend fun scheduleSave(f: TaskForm, tz: String): ScheduledTask?
  suspend fun scheduleEnable(id: String, on: Boolean): ScheduledTask?
  suspend fun scheduleRunNow(id: String)
  suspend fun scheduleHistory(id: String): List<ScheduleRun>
  /** "next runs: ..." for the form, or an [ApiException] with the sentence when the time cannot be read */
  suspend fun schedulePreview(f: TaskForm, tz: String): String
  /** Test: [id] of a saved task, or the form as a draft; the events of the run (SendEvent.Test, Delta, ToolStart, Approval, Notify, Result, Done) */
  fun scheduleTest(id: String?, f: TaskForm?, tz: String): Flow<SendEvent>
  suspend fun activity(): ActivityResp
  suspend fun activityStop(id: Long)
  suspend fun terminalRun(cmd: String): String
}

class ZoPagesApi(private val api: ZoApi) : PagesApi {
  override suspend fun chats() = api.chats()
  override suspend fun chat(id: String) = api.chat(id)
  override suspend fun deleteChat(id: String) = api.deleteChat(id)
  override suspend fun pinChat(id: String, on: Boolean) = api.pinChat(id, on)
  override suspend fun search(query: String) = api.search(query)
  override suspend fun projects() = api.projects()
  override suspend fun projectCreate(name: String, goal: String) = api.projectCreate(name, goal)
  override suspend fun artifacts() = api.artifacts()
  override suspend fun scheduled() = api.scheduled()
  override suspend fun scheduleAdd(task: String, whenText: String, repeat: String?) = api.scheduleAdd(task, whenText, repeat)
  override suspend fun scheduleCancel(id: String) = api.scheduleCancel(id)
  override suspend fun scheduledInfo() = api.scheduledInfo()
  override suspend fun scheduleSave(f: TaskForm, tz: String) = api.scheduleSave(
    f.editId, f.prompt.trim(), f.whenText.takeIf { it.isNotBlank() }, PagesFormat.repeatValue(f.repeatChoice, f.ownInterval).takeIf { it.isNotBlank() },
    f.title.trim(), f.kind, f.runOn, tz,
  )
  override suspend fun scheduleEnable(id: String, on: Boolean) = api.scheduleEnable(id, on)
  override suspend fun scheduleRunNow(id: String) = api.scheduleRunNow(id)
  override suspend fun scheduleHistory(id: String) = api.scheduleHistory(id).history
  override suspend fun schedulePreview(f: TaskForm, tz: String) = api.schedulePreview(f.whenText, PagesFormat.repeatValue(f.repeatChoice, f.ownInterval).takeIf { it.isNotBlank() }, tz).text
  override fun scheduleTest(id: String?, f: TaskForm?, tz: String): Flow<SendEvent> = api.scheduleTest(
    id,
    f?.let {
      kotlinx.serialization.json.buildJsonObject {
        put("task", it.prompt.trim()); put("kind", it.kind); put("run_on", it.runOn)
        if (it.title.isNotBlank()) put("title", it.title.trim())
      }
    },
    tz,
  )
  override suspend fun activity() = api.activity()
  override suspend fun activityStop(id: Long) = api.activityStop(id)
  override suspend fun terminalRun(cmd: String) = api.terminalRun(cmd)
}

/**
 * A list that is loaded from the server: [value] is the last good answer (kept while it is reloaded and when a reload fails), [loading] is true
 * while a call runs, [error] is the sentence of the last failure. The screen shows "Loading…" only without a value, and the error row only
 * without a value (otherwise the old list stays and the pull-to-refresh indicator spins).
 */
data class Loadable<T>(val value: T? = null, val loading: Boolean = false, val error: String? = null) {
  val isFirstLoad: Boolean get() = value == null && error == null
  val refreshing: Boolean get() = loading && value != null
}

/** The sentence for a person of any failure of a call; cancellation is not a failure. */
internal fun Throwable.sentence(): String = when (this) {
  is ApiException -> text
  else -> ApiErrors.GENERIC
}

internal inline fun <T> attempt(block: () -> T): Result<T> = try {
  Result.success(block())
} catch (e: CancellationException) {
  throw e
} catch (e: Exception) {
  Result.failure(e)
}
