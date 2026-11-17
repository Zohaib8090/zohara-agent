package org.zohara.agent.core.net

import java.io.IOException
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.channels.awaitClose
import kotlinx.coroutines.flow.channelFlow
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonObjectBuilder
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.decodeFromJsonElement
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonArray
import kotlinx.serialization.json.putJsonObject
import okhttp3.Call
import okhttp3.Callback
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import okhttp3.Response
import java.util.concurrent.TimeUnit

/** Body of `POST /api/send` (API.md section 3). */
data class SendRequest(
  val text: String,
  val chat: String? = null,
  /** re-ask from user message number `edit` (0-based); empty [text] asks the old text again */
  val edit: Int? = null,
  val attachments: List<Attachment> = emptyList(),
  /** `manual`, `auto` or `bypass` for this question only */
  val mode: String? = null,
  val keepRunning: Boolean = false,
  /** `desktop` runs it on the linked computer [device] */
  val target: String? = null,
  val device: String? = null,
)

/** One answer to a question card: the labels picked and an optional free text (`other`, at most 2000 characters), or [decide]: "You decide", Zohara Agent picks itself (then nothing else is set). */
data class QuestionAnswer(val selected: List<String>, val other: String? = null, val decide: Boolean = false)

/**
 * The typed client of the Zohara Agent server: one function per op of API.md. Every failure is an [ApiException] with a sentence for the person.
 * [baseUrl] is read on each call (the server address can change in Advanced settings).
 */
class ZoApi(
  private val http: OkHttpClient,
  private val baseUrl: () -> String,
  val json: kotlinx.serialization.json.Json = ZoJson,
  /** waits before a read-only retry; tests set it to 0 */
  private val retryDelayMs: (Int?) -> Long = { sec -> if (sec == null) 1000L else minOf(sec, 10) * 1000L },
) {
  private val jsonType = "application/json; charset=utf-8".toMediaType()
  private val opHttp: OkHttpClient by lazy { http.newBuilder().readTimeout(ZoHttp.OP_READ_SECONDS, TimeUnit.SECONDS).build() }
  private val streamHttp: OkHttpClient by lazy { http.newBuilder().readTimeout(ZoHttp.STREAM_SILENCE_SECONDS, TimeUnit.SECONDS).callTimeout(0, TimeUnit.SECONDS).build() }

  // ------------------------------------------------------------ plumbing

  /** Called for every error of every call (the session turns terms_required, 401, blocked, deleted and offline into screens and banners). */
  @Volatile var errorListener: ((ApiException) -> Unit)? = null
  /** Called after every call that worked (clears the offline banner). */
  @Volatile var successListener: (() -> Unit)? = null

  private fun report(e: ApiException): ApiException { errorListener?.invoke(e); return e }

  private fun url(path: String) = ZoHttp.normalizeServer(baseUrl()) + path

  private fun request(path: String, body: JsonObject): Request =
    Request.Builder().url(url(path)).post(body.toString().toRequestBody(jsonType)).header("Accept", "application/json").build()

  private suspend fun Call.await(): Response = suspendCancellableCoroutine { cont ->
    cont.invokeOnCancellation { runCatching { cancel() } }
    enqueue(object : Callback {
      override fun onFailure(call: Call, e: IOException) { if (cont.isActive) cont.resumeWithException(e) }
      override fun onResponse(call: Call, response: Response) { cont.resume(response) { _, _, _ -> response.close() } }
    })
  }

  /** POST [body] to [path]; returns the JSON object of the answer, or throws. HTTP 200 with `ok:false` is an error too (NET-02). */
  suspend fun post(path: String, body: JsonObject = JsonObject(emptyMap()), readOnly: Boolean = false): JsonObject =
    try {
      withRetry(readOnly) { postOnce(path, body) }.also { successListener?.invoke() }
    } catch (e: ApiException) {
      throw report(e)
    }

  private suspend fun postOnce(path: String, body: JsonObject): JsonObject {
    val response = try {
      opHttp.newCall(request(path, body)).await()
    } catch (e: CancellationException) {
      throw e
    } catch (e: Exception) {
      throw ApiErrors.fromThrowable(e)
    }
    val text = try {
      withContext(Dispatchers.IO) { response.use { it.body?.string().orEmpty() } }
    } catch (e: IOException) {
      throw ApiErrors.fromThrowable(e)
    }
    if (!response.isSuccessful) throw ApiErrors.fromHttp(response.code, text, response.header("Retry-After"))
    val obj = try {
      json.parseToJsonElement(text) as? JsonObject ?: throw ApiException(ErrorKind.Server, ApiErrors.GENERIC, response.code)
    } catch (e: ApiException) {
      throw e
    } catch (e: Exception) {
      throw ApiException(ErrorKind.Server, ApiErrors.GENERIC, response.code, cause = e)
    }
    if ((obj["ok"] as? JsonPrimitive)?.booleanOrNull == false) {
      throw ApiErrors.fromOpFailure((obj["error"] as? JsonPrimitive)?.contentOrNull.orEmpty())
    }
    return obj
  }

  private suspend fun <T> withRetry(readOnly: Boolean, block: suspend () -> T): T {
    try {
      return block()
    } catch (e: ApiException) {
      if (!readOnly) throw e
      when {
        e.kind == ErrorKind.Offline -> delay(retryDelayMs(null))
        (e.status == 429 || e.status == 503) -> delay(retryDelayMs(e.retryAfterSec ?: 5))
        else -> throw e
      }
      return block()
    }
  }

  private suspend inline fun <reified T> opAs(name: String, readOnly: Boolean = false, noinline fields: JsonObjectBuilder.() -> Unit = {}): T =
    json.decodeFromJsonElement<T>(op(name, readOnly, fields))

  /** `POST /api/op {op, ...}`. */
  suspend fun op(name: String, readOnly: Boolean = false, fields: JsonObjectBuilder.() -> Unit = {}): JsonObject =
    post("/api/op", buildJsonObject { put("op", name); fields() }, readOnly)

  private suspend inline fun <reified T> postAs(path: String, readOnly: Boolean = false, noinline fields: JsonObjectBuilder.() -> Unit = {}): T =
    json.decodeFromJsonElement<T>(post(path, buildJsonObject(fields), readOnly))

  // ------------------------------------------------------------ 4.1 status, chats

  suspend fun status(): StatusInfo = opAs("status", true)
  suspend fun chats(): List<ChatSummary> = opAs<ChatsResp>("chats", true).chats
  suspend fun chat(id: String): ChatDetail = opAs("chat", true) { put("id", id) }
  /** the other take of user message [nth]: [delta] -1 older, +1 newer */
  suspend fun switchTake(id: String, nth: Int, delta: Int): ChatDetail = opAs("switch") { put("id", id); put("nth", nth); put("delta", delta) }
  suspend fun deleteChat(id: String) { op("delete_chat") { put("id", id) } }
  suspend fun deleteAllChats(): Int = opAs<DeletedCount>("delete_all_chats").deleted
  suspend fun pinChat(id: String, on: Boolean) { op("pin_chat") { put("id", id); put("on", on) } }
  suspend fun search(query: String): List<SearchHit> = opAs<SearchResp>("search", true) { put("query", query) }.hits
  suspend fun compact(id: String) { op("compact") { put("id", id) } }
  suspend fun linkPreview(url: String): LinkPreview = opAs("link_preview") { put("url", url) }
  suspend fun terminalRun(cmd: String): String = opAs<TextResp>("terminal_run") { put("cmd", cmd) }.output
  /** the saved mode (`Manual`, `Auto`, `Bypass`); with [set] (`manual`, `auto`, `bypass`) it also changes it */
  suspend fun mode(set: String? = null): String = opAs<ModeResp>("mode") { if (set != null) put("set", set) }.mode
  suspend fun activity(): ActivityResp = opAs("activity", true)
  suspend fun activityStop(id: Long) { op("activity_stop") { put("id", id) } }
  suspend fun syncStatus(): JsonObject = op("sync_status", true)

  // ------------------------------------------------------------ 4.2 models, keys, profiles

  suspend fun providers(): ProvidersResp = opAs("providers", true)
  suspend fun models(provider: String? = null, baseUrl: String? = null, apiFormat: String? = null, account: String? = null): ModelsResp = opAs("models") {
    if (account != null) put("account", account)
    if (provider != null) put("provider", provider)
    if (baseUrl != null) put("base_url", baseUrl)
    if (apiFormat != null) put("api_format", apiFormat)
  }
  suspend fun modelTest(provider: String? = null, model: String? = null, baseUrl: String? = null, apiFormat: String? = null, account: String? = null): ModelTestResp = opAs("model_test") {
    if (account != null) put("account", account)
    if (provider != null) put("provider", provider)
    if (model != null) put("model", model)
    if (baseUrl != null) put("base_url", baseUrl)
    if (apiFormat != null) put("api_format", apiFormat)
  }
  /** [apiFormat]: chat, responses or claude for a hosted OpenAI-compatible provider; "" goes back to its normal format. */
  suspend fun select(provider: String? = null, model: String? = null, effort: String? = null, baseUrl: String? = null, apiFormat: String? = null, account: String? = null): SelectResp = opAs("select") {
    if (account != null) put("account", account)
    if (apiFormat != null) put("api_format", apiFormat)
    if (provider != null) put("provider", provider)
    if (model != null) put("model", model)
    if (effort != null) put("effort", effort)
    if (baseUrl != null) put("base_url", baseUrl)
  }
  /** The key is sent once and never logged or kept. */
  suspend fun setKey(provider: String, key: String, account: String? = null) { op("set_key") { put("provider", provider); put("key", key); if (account != null) put("account", account) } }

  // named custom providers: [format] is chat, responses or claude; an empty [key] keeps the saved one
  suspend fun customProviderAdd(name: String, format: String, baseUrl: String, key: String = "", defaultModel: String = ""): CustomProviderResp = opAs("custom_provider_add") {
    put("name", name); put("format", format); put("base_url", baseUrl); put("key", key); put("default_model", defaultModel)
  }
  suspend fun customProviderUpdate(id: String, name: String? = null, format: String? = null, baseUrl: String? = null, key: String? = null, defaultModel: String? = null): CustomProviderResp = opAs("custom_provider_update") {
    put("id", id)
    if (name != null) put("name", name)
    if (format != null) put("format", format)
    if (baseUrl != null) put("base_url", baseUrl)
    if (key != null) put("key", key)
    if (defaultModel != null) put("default_model", defaultModel)
  }
  /** Tests a saved provider ([id]) or one still being typed ([format] and [baseUrl]); the typed [key] goes only to that address. */
  suspend fun customProviderTest(id: String? = null, format: String? = null, baseUrl: String? = null, key: String? = null, model: String? = null): ModelTestResp = opAs("custom_provider_test") {
    if (id != null) put("id", id)
    if (format != null) put("format", format)
    if (baseUrl != null) put("base_url", baseUrl)
    if (key != null) put("key", key)
    if (model != null) put("model", model)
  }
  suspend fun customProviderRemove(id: String) { op("custom_provider_remove") { put("id", id); put("confirm", true) } }

  // several API key accounts per provider
  suspend fun accountAdd(provider: String, name: String, key: String, baseUrl: String? = null): AccountsResp = opAs("account_add") {
    put("provider", provider); put("name", name); put("key", key); if (baseUrl != null) put("base_url", baseUrl)
  }
  suspend fun accountRename(provider: String, id: String, name: String, key: String? = null, baseUrl: String? = null): AccountsResp = opAs("account_rename") {
    put("provider", provider); put("id", id); put("name", name); if (key != null) put("key", key); if (baseUrl != null) put("base_url", baseUrl)
  }
  suspend fun accountHide(provider: String, id: String, hidden: Boolean): AccountsResp = opAs("account_hide") { put("provider", provider); put("id", id); put("hidden", hidden) }
  suspend fun accountRemove(provider: String, id: String): AccountsResp = opAs("account_remove") { put("provider", provider); put("id", id); put("confirm", true) }
  suspend fun accountTest(provider: String, id: String, model: String? = null): ModelTestResp = opAs("account_test") {
    put("provider", provider); put("id", id); if (model != null) put("model", model)
  }
  suspend fun profiles(): ProfilesResp = opAs("profiles", true)
  suspend fun profileCreate(name: String): String = opAs<IdResp>("profile_create") { put("name", name) }.id
  suspend fun profileUse(id: String): ProfileUseResp = opAs("profile_use") { put("id", id) }
  suspend fun profileUpdate(id: String, baseUrl: String? = null, effort: String? = null) {
    op("profile_update") { put("id", id); if (baseUrl != null) put("base_url", baseUrl); if (effort != null) put("effort", effort) }
  }
  suspend fun profileRename(id: String, name: String) { op("profile_rename") { put("id", id); put("name", name) } }
  suspend fun profileDelete(id: String) { op("profile_delete") { put("id", id) } }
  suspend fun profilePin(id: String, model: String, on: Boolean) { op("profile_pin") { put("id", id); put("model", model); put("on", on) } }
  suspend fun settingsGet(): SettingsResp = opAs("settings_get", true)
  suspend fun settingsSet(hiddenProviders: List<String>? = null, providerPrefs: Map<String, ProviderPref>? = null, rotateOnLimit: Boolean? = null): SettingsResp = opAs("settings_set") {
    if (rotateOnLimit != null) put("rotate_on_limit", rotateOnLimit)
    if (hiddenProviders != null) putJsonArray("hidden_providers") { hiddenProviders.forEach { add(JsonPrimitive(it)) } }
    if (providerPrefs != null) putJsonObject("provider_prefs") { providerPrefs.forEach { (k, v) -> putJsonObject(k) { if (v.order != null) put("order", v.order) } } }
  }

  // ------------------------------------------------------------ 4.3 memory, personalities

  /** [scope]: "all" (default), "global" (notes for every chat) or a project id */
  suspend fun memory(scope: String = "all"): MemoryResp = opAs("memory", true) { if (scope != "all") put("scope", scope) }
  /** [scope] "" = the whole memory, else a project id */
  suspend fun memoryAdd(text: String, kind: String = "fact", scope: String = "") { op("memory_add") { put("text", text); put("kind", kind); if (scope.isNotEmpty()) put("scope", scope) } }
  /** What the memory agent changed, newest first; only entries newer than [since] (ms). */
  suspend fun memoryChanges(since: Long = 0, limit: Int = 60): MemoryChangesResp = opAs("memory_changes", true) { if (since > 0) put("since", since); put("limit", limit) }
  /** Undoes one change; returns the sentence to show. */
  suspend fun memoryUndo(id: String): String = opAs<TextResp>("memory_undo") { put("id", id) }.message
  suspend fun memorySettings(every: Int? = null, showUpdates: Boolean? = null): MemorySettingsResp = opAs("memory_settings") {
    if (every != null) put("every", every)
    if (showUpdates != null) put("show_updates", showUpdates)
  }
  /** The `zo-memory-1` document with every note and the project of each. */
  suspend fun memoryExport(): JsonObject = op("memory_export", true)
  suspend fun memoryImport(data: JsonObject): MemoryImportResp = opAs("memory_import") { put("data", data) }
  suspend fun memoryUpdate(id: String, text: String) { op("memory_update") { put("id", id); put("text", text) } }
  suspend fun memoryPin(id: String, on: Boolean) { op("memory_pin") { put("id", id); put("on", on) } }
  suspend fun memoryForget(id: String) { op("memory_forget") { put("id", id) } }
  suspend fun memoryEnable(on: Boolean) { op("memory_enable") { put("on", on) } }
  suspend fun memoryClear() { op("memory_clear") }
  suspend fun memoryEdit(text: String): String = opAs<TextResp>("memory_edit") { put("text", text) }.message
  suspend fun personalities(): PersonalitiesResp = opAs("personalities", true)
  /** `""` selects Plain */
  suspend fun personalityUse(id: String) { op("personality_use") { put("id", id) } }
  suspend fun personalityAdd(name: String, prompt: String): String = opAs<IdResp>("personality_add") { put("name", name); put("prompt", prompt) }.id
  suspend fun personalityGenerate(description: String): PersonalityGenerated = opAs("personality_generate") { put("description", description) }
  suspend fun personalityRemove(id: String) { op("personality_remove") { put("id", id) } }

  // ------------------------------------------------------------ 4.4 projects, artifacts, schedules, machines, MCP

  suspend fun projects(): String = opAs<ProjectsResp>("projects", true).text
  suspend fun projectCreate(name: String, goal: String = ""): String = opAs<TextResp>("project_create") { put("name", name); put("goal", goal) }.message
  /** Deletes a project and the memory notes kept for it (the caller has asked the person first). Returns how many notes went. */
  suspend fun projectDelete(id: String): Int = opAs<ProjectDeleteResp>("project_delete") { put("id", id); put("confirm", true) }.notesRemoved
  suspend fun artifacts(): List<ArtifactItem> = opAs<ArtifactsResp>("artifacts", true).items
  suspend fun scheduled(): List<ScheduledTask> = scheduledInfo().tasks
  /** the list with where tasks can run, the kinds and the limits of this account (docs/SCHEDULED.md) */
  suspend fun scheduledInfo(): ScheduledResp = opAs("scheduled", true)
  /** [`when`] is plain words ("tomorrow 9am", "in 2 hours", "every weekday at 9"); a leading "in " is still rewritten to "now + " for servers older than 2026-11-14 */
  suspend fun scheduleAdd(task: String, `when`: String, repeat: String? = null, title: String? = null): String = opAs<TextResp>("schedule_add") {
    put("task", task); put("when", fixWhen(`when`))
    if (!repeat.isNullOrBlank()) put("repeat", repeat)
    if (!title.isNullOrBlank()) put("title", title)
  }.message
  /** a task with every field: [kind] task | reminder | browser, [runOn] "" | "cloud" | "machine:<name>"; [tz] the phone's time zone so "9am" is the person's 9am */
  suspend fun scheduleSave(
    id: String?, task: String, `when`: String?, repeat: String?, title: String?, kind: String, runOn: String, tz: String,
  ): ScheduledTask? = opAs<ScheduledOne>(if (id == null) "schedule_add" else "schedule_update") {
    if (id != null) put("id", id)
    put("task", task); put("kind", kind); put("run_on", runOn); put("tz", tz)
    if (!`when`.isNullOrBlank()) put("when", `when`.trim())
    if (!repeat.isNullOrBlank()) put("repeat", repeat)
    if (title != null) put("title", title)
  }.task
  suspend fun scheduleEnable(id: String, on: Boolean): ScheduledTask? = opAs<ScheduledOne>("schedule_enable") { put("id", id); put("on", on) }.task
  /** starts the task now; its result shows in the task (last result, history) and as a notification */
  suspend fun scheduleRunNow(id: String) { op("schedule_run_now") { put("id", id) } }
  suspend fun scheduleHistory(id: String): ScheduleHistoryResp = opAs("schedule_history", true) { put("id", id) }
  /** "next runs: ..." for what is typed in the form; an error sentence when the time cannot be read */
  suspend fun schedulePreview(`when`: String, repeat: String?, tz: String): SchedulePreview = opAs("schedule_preview", true) {
    put("when", `when`.trim()); put("tz", tz)
    if (!repeat.isNullOrBlank()) put("repeat", repeat)
  }
  /** Test: runs the task once now in test mode, streamed like a send (`test`, `delta`, `tool_start`, `approval`, `notify`, `result`, `done`); for a saved task [id], else the [draft] */
  fun scheduleTest(id: String?, draft: JsonObject?, tz: String): Flow<SendEvent> =
    stream("/api/schedule/test", buildJsonObject {
      if (id != null) put("id", id) else if (draft != null) put("draft", draft)
      put("tz", tz); put("client", "phone")
    })
  suspend fun scheduleCancel(id: String) { op("schedule_cancel") { put("id", id) } }
  suspend fun machines(): List<Machine> = opAs<MachinesResp>("machines", true).machines
  suspend fun machineAdd(name: String, host: String, user: String, port: Int, note: String, key: String) {
    op("machine_add") { put("name", name); put("host", host); put("user", user); put("port", port); put("note", note); put("key", key) }
  }
  suspend fun machineTest(name: String): String = opAs<TextResp>("machine_test") { put("name", name) }.message
  suspend fun machineRemove(name: String, force: Boolean = false) { op("machine_remove") { put("name", name); if (force) put("force", true) else put("uninstall_runner", true) } }
  // docs/RUNNER.md: status, the add wizard, the runner, the model and the connected apps on a machine. Administrators only (the server refuses the rest).
  /** every machine (or just [name]) with reachable / runner installed; one SSH call each, all at the same time */
  suspend fun machineStatus(name: String? = null): List<MachineStatus> = opAs<MachineStatusResp>("machine_status", true) { if (name != null) put("name", name) }.machines
  /** a new machine with a NEW key made for Zohara Agent: the public line and the command to put it on the machine */
  suspend fun machineNew(name: String, host: String, user: String, port: Int, note: String): MachineNew =
    opAs("machine_new") { put("name", name); put("host", host); put("user", user); put("port", port); put("note", note) }
  /** puts the key on the machine with a one-time password: used once, never kept, never logged */
  suspend fun machineAuthorize(name: String, password: String): String = opAs<TextResp>("machine_authorize") { put("name", name); put("password", password) }.message
  suspend fun runnerInstall(name: String): List<String> = opAs<StepsResp>("machine_runner_install") { put("name", name) }.steps
  suspend fun runnerUninstall(name: String) { op("machine_runner_uninstall") { put("name", name) } }
  suspend fun runnerAllow(name: String, commands: Boolean, files: Boolean) { op("machine_runner_allow") { put("name", name); put("commands", commands); put("files", files) } }
  suspend fun machineModelPlan(name: String): ModelPlan = opAs("machine_model_plan", true) { put("name", name) }
  /** [confirm] is the sentence from [machineModelPlan], word for word: the service refuses anything else */
  suspend fun machineModelCopy(name: String, confirm: String): String = opAs<TextResp>("machine_model_copy") { put("name", name); put("confirm", confirm) }.message
  suspend fun machineModelClear(name: String) { op("machine_model_clear") { put("name", name) } }
  suspend fun machineAppsPlan(name: String): AppsPlanResp = opAs("machine_apps_plan", true) { put("name", name) }
  /** [servers] are (app name, copy its credentials too); with credentials the service wants the named sentence in [confirm] and says it when it is missing */
  suspend fun machineAppsCopy(name: String, servers: List<Pair<String, Boolean>>, confirm: String): AppsCopyResp = opAs("machine_apps_copy") {
    put("name", name); put("confirm", confirm)
    putJsonArray("servers") { servers.forEach { (n, c) -> add(buildJsonObject { put("name", n); put("credentials", c) }) } }
  }
  suspend fun machineAppsRevoke(name: String) { op("machine_apps_revoke") { put("name", name) } }
  suspend fun machineAppsCheck(name: String): List<AppCheck> = opAs<AppsCheckResp>("machine_apps_check") { put("name", name) }.servers
  suspend fun machineAppsSync(name: String, confirm: String): AppsCopyResp = opAs("machine_apps_sync") { put("name", name); put("confirm", confirm) }
  suspend fun machineTaskRun(name: String, prompt: String): RunTaskResp = opAs("machine_task_run") { put("name", name); put("prompt", prompt) }
  suspend fun mcpList(): List<McpServer> = opAs<McpResp>("mcp_list", true).servers
  suspend fun mcpAdd(name: String, address: String, token: String? = null): String = opAs<TextResp>("mcp_add") {
    put("name", name); put("address", address); if (!token.isNullOrBlank()) put("token", token)
  }.message
  suspend fun mcpImport(config: String): String = opAs<TextResp>("mcp_import") { put("config", config) }.message
  suspend fun mcpToggle(name: String, enabled: Boolean) { op("mcp_toggle") { put("name", name); put("enabled", enabled) } }
  suspend fun mcpRemove(name: String) { op("mcp_remove") { put("name", name) } }

  // connectors directory: the server reads the public MCP registry (cached an hour); the phone never asks the registry itself
  suspend fun connectors(q: String = "", cursor: String? = null, category: String = "all", kind: String = "remote"): ConnectorsResp = opAs("connectors", true) {
    put("kind", kind); put("category", category)
    if (q.isNotBlank()) put("q", q.trim())
    if (!cursor.isNullOrBlank()) put("cursor", cursor)
  }
  /** Step one of adding: the server tries the connector (keys in [headers] by header name) and lists its tools. Nothing is saved yet. */
  suspend fun connectorPreview(id: String, name: String, url: String? = null, headers: Map<String, String> = emptyMap()): ConnectorPreview = opAs("connector_preview") {
    put("id", id); put("kind", "remote"); put("name", name)
    if (!url.isNullOrBlank()) put("url", url)
    putJsonObject("headers") { headers.forEach { (k, v) -> put(k, v) } }
  }
  /** Step two: saves what [connectorPreview] tried. */
  suspend fun connectorAdd(name: String): String = opAs<TextResp>("connector_add") { put("name", name) }.message
  suspend fun connectorCancel(name: String) { op("connector_cancel") { put("name", name) } }
  suspend fun connectorAcceptChange(name: String): String = opAs<TextResp>("connector_accept_change") { put("name", name) }.message

  /** 4.5: an op relayed to a linked computer. Returns the computer's own answer. */
  suspend fun desktopOp(device: String, inner: JsonObject): JsonObject {
    val r = op("desktop_op") { put("device", device); put("inner", inner) }
    return (r["result"] as? JsonObject) ?: r
  }

  // ------------------------------------------------------------ 5 account

  suspend fun whoami(): Whoami = postAs("/api/account/whoami", true)
  suspend fun acceptTerms(version: String): String = postAs<VersionResp>("/api/account/accept_terms") { put("version", version) }.version
  suspend fun devicesList(): List<AccountDevice> = postAs<DevicesResp>("/api/account/devices_list", true).devices
  suspend fun deviceRevoke(id: String) { post("/api/account/device_revoke", buildJsonObject { put("id", id) }) }
  /** the whole export as JSON text, ready to save as `zohara-agent-export-YYYY-MM-DD.json` (without `ok`) */
  suspend fun exportAccount(): String = JsonObject(post("/api/account/export").filterKeys { it != "ok" }).toString()
  /** irreversible; the caller signs out afterwards */
  suspend fun deleteAccount(): DeleteAccountResp = postAs("/api/account/delete") { put("confirm", "DELETE") }
  suspend fun pushConfig(): PushConfig = postAs("/api/account/push_config", true)
  suspend fun pushSubscribe(token: String, label: String, showTitle: Boolean = false): String =
    postAs<IdResp>("/api/account/push_subscribe") { put("kind", "fcm"); put("token", token); put("label", label); put("show_title", showTitle) }.id
  suspend fun pushUnsubscribe(id: String) { post("/api/account/push_unsubscribe", buildJsonObject { put("id", id) }) }
  suspend fun pushList(): List<PushSubscription> = postAs<PushListResp>("/api/account/push_list", true).subscriptions
  suspend fun claudeStatus(): ClaudeStatus = postAs("/api/account/claude_status", true)

  // ------------------------------------------------------------ 6, 7, 8 devices, link, sync

  suspend fun deviceLookup(userCode: String): DeviceLookup = postAs("/api/auth/device_lookup") { put("user_code", userCode) }
  suspend fun deviceApprove(userCode: String, approve: Boolean) { post("/api/auth/device_approve", buildJsonObject { put("user_code", userCode); put("approve", approve) }) }
  suspend fun linkStatus(): List<LinkDevice> = postAs<LinkStatusResp>("/api/link/status", true).devices
  suspend fun syncManifest(): JsonObject = post("/api/sync/manifest", readOnly = true)

  // ------------------------------------------------------------ 3.5 reply

  /** `POST /api/reply`. true when the line reached the answer; false when the stream is not yours or is over (404). */
  suspend fun reply(stream: Long, fields: JsonObjectBuilder.() -> Unit): Boolean {
    return try {
      post("/api/reply", buildJsonObject { put("stream", stream); fields() })
      true
    } catch (e: ApiException) {
      if (e.status == 404) false else throw e
    }
  }
  suspend fun replyStop(stream: Long): Boolean = reply(stream) { put("op", "stop") }
  suspend fun replyApprove(stream: Long, id: Long, yes: Boolean): Boolean = reply(stream) { put("op", "approve"); put("id", id); put("yes", yes) }
  suspend fun replyAnswer(stream: Long, id: Long, answers: List<QuestionAnswer>): Boolean = reply(stream) {
    put("op", "answer"); put("id", id)
    putJsonArray("answers") {
      answers.forEach { a ->
        add(buildJsonObject {
          putJsonArray("selected") { a.selected.forEach { add(JsonPrimitive(it)) } }
          put("other", a.other?.let { JsonPrimitive(sanitizeOther(it)) } ?: JsonNull)
          if (a.decide) put("decide", true)
        })
      }
    }
  }
  suspend fun replySkipQuestion(stream: Long, id: Long): Boolean = reply(stream) { put("op", "answer"); put("id", id); put("cancelled", true) }

  // ------------------------------------------------------------ 3 send (NDJSON stream)

  /** The JSON body of a send; public so tests can look at it. */
  fun sendBody(r: SendRequest): JsonObject = buildJsonObject {
    put("text", r.text)
    if (r.chat != null) put("chat", r.chat)
    if (r.edit != null) put("edit", r.edit)
    if (r.attachments.isNotEmpty()) put("attachments", Attachments.toJson(r.attachments))
    if (r.mode != null) put("mode", r.mode)
    if (r.keepRunning) put("keep_running", true)
    if (r.target != null) put("target", r.target)
    if (r.device != null) put("device", r.device)
    put("client", "phone")
  }

  /**
   * Sends a message and streams the answer. The flow ends after `done`, `error` + `done`, or [SendEvent.ConnectionLost] (no `done` came).
   * A refused request (400, 401, 403, 413, 429) throws an [ApiException] when the flow is collected. Cancelling the collector closes the
   * connection, which stops the answer on the server unless `keep_running` was set.
   */
  fun send(r: SendRequest): Flow<SendEvent> = stream("/api/send", sendBody(r))

  /** The NDJSON stream of one request (a send, or the Test of a scheduled task); see [send]. */
  private fun stream(path: String, body: JsonObject): Flow<SendEvent> = channelFlow {
    val call = streamHttp.newCall(request(path, body).newBuilder().header("Accept", "application/x-ndjson").build())
    val reader = launch(Dispatchers.IO) {
      var sawDone = false
      try {
        val response = try {
          call.execute()
        } catch (e: IOException) {
          if (isActive) throw ApiErrors.fromThrowable(e) else return@launch
        }
        response.use { resp ->
          if (!resp.isSuccessful) throw ApiErrors.fromHttp(resp.code, resp.body?.string().orEmpty(), resp.header("Retry-After"))
          successListener?.invoke()
          val source = resp.body?.source() ?: throw ApiException(ErrorKind.Server, ApiErrors.GENERIC, resp.code)
          val lines = NdjsonLineBuffer()
          val chunk = ByteArray(8192)
          loop@ while (isActive) {
            val n = try { source.read(chunk) } catch (e: IOException) {
              if (isActive) send(SendEvent.ConnectionLost(LOST)); sawDone = true; break@loop
            }
            if (n < 0) break
            for (line in lines.feed(chunk, n)) {
              val ev = SendEventParser.parse(line) ?: continue
              send(ev)
              if (ev is SendEvent.Done) { sawDone = true; break@loop }
            }
          }
          if (!sawDone && isActive) {
            for (line in lines.finish()) {
              val ev = SendEventParser.parse(line) ?: continue
              send(ev); if (ev is SendEvent.Done) sawDone = true
            }
            if (!sawDone) send(SendEvent.ConnectionLost(LOST))
          }
        }
        close()
      } catch (e: CancellationException) {
        throw e
      } catch (e: ApiException) {
        close(report(e))
      } catch (e: Exception) {
        close(report(ApiErrors.fromThrowable(e)))
      }
    }
    awaitClose { call.cancel(); reader.cancel() }
  }

  companion object {
    const val LOST = "The connection was lost. The answer may still be finishing: reopen the chat in a moment."

    /** "in 2 hours" -> "now + 2 hours" (the server parses `when` with GNU date) */
    fun fixWhen(w: String): String {
      val t = w.trim()
      return if (t.lowercase().startsWith("in ")) "now + " + t.substring(3).trim() else t
    }

    /** question answers: control characters removed and at most 2000 characters (INTERACTION.md) */
    fun sanitizeOther(s: String): String = s.filter { !it.isISOControl() || it == '\n' || it == '\t' }.take(2000)
  }
}
