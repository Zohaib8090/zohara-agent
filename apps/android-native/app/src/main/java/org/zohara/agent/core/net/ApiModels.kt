package org.zohara.agent.core.net

import kotlinx.serialization.ExperimentalSerializationApi
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonNamingStrategy

/**
 * Response shapes of docs API.md. Property names are camelCase; the JSON is snake_case ([ZoJson] maps them).
 * Every field has a default so a server that leaves one out (older server, restricted account) never breaks the app.
 */
@OptIn(ExperimentalSerializationApi::class)
val ZoJson: Json = Json {
  ignoreUnknownKeys = true
  isLenient = true
  coerceInputValues = true
  explicitNulls = false
  encodeDefaults = false
  namingStrategy = JsonNamingStrategy.SnakeCase
}

// ---- status, chats ----
@Serializable data class StatusInfo(val mode: String = "", val model: String = "", val provider: String = "", val ready: Boolean = false, val version: String = "")
@Serializable data class ChatSummary(val id: String, val pinned: Boolean = false, val title: String = "", val updated: Long = 0)
@Serializable data class ChatsResp(val chats: List<ChatSummary> = emptyList())
@Serializable data class AttachInfo(val kind: String = "", val name: String = "", val size: Long = 0, val thumb: String? = null)
@Serializable data class ChatEntry(val kind: String = "", val text: String = "", val attach: List<AttachInfo> = emptyList())
@Serializable data class TakeInfo(val count: Int = 1, val current: Int = 0, val nth: Int = 0)
@Serializable data class ChatDetail(val id: String = "", val title: String = "", val entries: List<ChatEntry> = emptyList(), val takes: List<TakeInfo> = emptyList())
@Serializable data class SearchHit(val id: String, val snippet: String = "", val title: String = "", val updated: Long = 0)
@Serializable data class SearchResp(val hits: List<SearchHit> = emptyList())
@Serializable data class LinkPreview(val title: String = "", val description: String = "", val host: String = "")
@Serializable data class ModeResp(val mode: String = "")
@Serializable data class ActivityItem(
  val id: Long = 0, val kind: String = "", val title: String = "", val detail: String = "", val source: String = "",
  val chatId: String? = null, val startedMs: Long = 0, val canStop: Boolean = false,
)
@Serializable data class ClientSeen(val lastMs: Long = 0, val source: String = "")
@Serializable data class ActivityResp(val nowMs: Long = 0, val clientsSeen: List<ClientSeen> = emptyList(), val items: List<ActivityItem> = emptyList())
@Serializable data class DeletedCount(val deleted: Int = 0)
@Serializable data class TextResp(val output: String = "", val message: String = "", val text: String = "")

// ---- models ----
@Serializable data class ProviderInfo(
  val id: String, val name: String = "", val local: Boolean = false, val hasKey: Boolean = false, val needsUrl: Boolean = false,
  val defaultUrl: String = "", val baseUrl: String = "", val hidden: Boolean = false,
  /** true for the three "Custom endpoint" providers (chat, responses, claude); older servers do not send it, see [org.zohara.agent.settings.ModelLogic.isCustom] */
  val custom: Boolean = false,
  /** the wire format the provider speaks: chat, responses or claude ("" from an older server) */
  val format: String = "",
  /** a hosted OpenAI-compatible provider that may be told another API format (select {api_format}) */
  val takesFormat: Boolean = false,
  /** the saved API format of such a provider, null for its normal one */
  val apiFormat: String? = null,
)
@Serializable data class ProvidersResp(val current: String = "", val hiddenProviders: List<String> = emptyList(), val providers: List<ProviderInfo> = emptyList())
@Serializable data class ModelsResp(
  val provider: String = "", val models: List<String> = emptyList(), val current: String = "", val efforts: List<String> = emptyList(),
  val effort: String? = null, val baseUrl: String = "", val error: String? = null,
)
@Serializable data class ModelTestResp(val works: Boolean = false, val message: String = "")
@Serializable data class SelectResp(val provider: String = "", val model: String = "", val effort: String? = null, val baseUrl: String = "", val apiFormat: String? = null)
@Serializable data class Profile(
  val id: String, val name: String = "", val provider: String = "", val model: String = "", val effort: String? = null,
  val pinned: List<String> = emptyList(), val baseUrl: String = "", val vision: Boolean? = null, val updated: Long = 0,
)
@Serializable data class ProfilesResp(val active: String = "", val profiles: List<Profile> = emptyList())
@Serializable data class IdResp(val id: String = "")
@Serializable data class ProfileUseResp(val provider: String = "", val model: String = "", val effort: String? = null, val baseUrl: String = "")
@Serializable data class ProviderPref(val order: Int? = null)
@Serializable data class SettingsResp(val hiddenProviders: List<String> = emptyList(), val providerPrefs: Map<String, ProviderPref> = emptyMap(), val updated: Long = 0)

// ---- memory, personalities ----
@Serializable data class MemoryItem(val id: String, val kind: String = "fact", val text: String = "", val title: String = "", val chat: String = "", val pinned: Boolean = false, val updated: Long = 0, val scope: String = "", val scopeName: String = "")
@Serializable data class MemoryProject(val id: String, val name: String = "")
@Serializable data class MemoryResp(
  val enabled: Boolean = true, val every: Int = 3, val items: List<MemoryItem> = emptyList(),
  val minEvery: Int = 2, val showUpdates: Boolean = true, val projects: List<MemoryProject> = emptyList(),
)
/** One entry of the memory change log (`memory_changes`): [action] is added, updated or deleted; [scope] is "" for the whole memory, else a project id. */
@Serializable data class MemoryChange(
  val id: String, val time: Long = 0, val batch: String = "", val source: String = "", val chat: String = "", val title: String = "",
  val scope: String = "", val scopeName: String = "", val action: String = "", val note: String = "", val kind: String = "",
  val before: String = "", val after: String = "", val undone: Boolean = false,
)
@Serializable data class MemoryChangesResp(
  val changes: List<MemoryChange> = emptyList(), val latest: Long = 0, val running: Boolean = false, val showUpdates: Boolean = true, val summary: String = "",
)
@Serializable data class MemorySettingsResp(val every: Int = 3, val minEvery: Int = 2, val showUpdates: Boolean = true)
@Serializable data class MemoryImportResp(val added: Int = 0, val alreadyThere: Int = 0, val skipped: Int = 0)
@Serializable data class Personality(val id: String, val name: String = "", val prompt: String = "", val generated: Boolean = false, val builtin: Boolean = false)
@Serializable data class PersonalitiesResp(val active: String = "", val items: List<Personality> = emptyList())
@Serializable data class PersonalityGenerated(val id: String = "", val name: String = "", val prompt: String = "")

// ---- projects, artifacts, schedules, machines, MCP ----
@Serializable data class ProjectsResp(val text: String = "")
@Serializable data class ProjectDeleteResp(val notesRemoved: Int = 0)
@Serializable data class ArtifactItem(val id: String = "", val kind: String = "", val title: String = "", val path: String = "", val text: String? = null, val chat: String = "", val created: Long = 0)
@Serializable data class ArtifactsResp(val items: List<ArtifactItem> = emptyList())
/**
 * A scheduled task (docs/SCHEDULED.md). [kind] is task, reminder or browser; [runOn] is "this", "cloud" or "machine:<name>"; [lastStatus] is ok, failed or
 * skipped. Every field after [enabled] is new and optional, so an older server still gives a task that shows.
 */
@Serializable data class ScheduledTask(
  val id: String, val title: String = "", val prompt: String = "", val nextRun: Long = 0, val repeat: String = "once", val enabled: Boolean = true,
  val kind: String = "task", val runOn: String = "", val tz: String? = null, val lastRun: Long? = null, val lastChat: String? = null,
  val lastStatus: String? = null, val lastOutput: String? = null, val historyCount: Int = 0, val running: Boolean = false, val place: String = "",
)
@Serializable data class ScheduleRunOn(val id: String = "", val label: String = "", val note: String = "")
@Serializable data class ScheduleLimits(val maxTasks: Int = 0, val minRepeatMinutes: Int = 0, val runsPerDay: Int? = null, val runsToday: Int = 0)
@Serializable data class ScheduledResp(
  val tasks: List<ScheduledTask> = emptyList(), val host: String = "", val runOn: List<ScheduleRunOn> = emptyList(),
  val kinds: List<String> = emptyList(), val limits: ScheduleLimits = ScheduleLimits(), val now: Long = 0,
)
@Serializable data class ScheduledOne(val task: ScheduledTask? = null, val message: String = "", val started: Boolean = false)
/** one run in a task's history: [how] is "scheduled" or "manual"; [missed] is the time it should have run when the computer was off */
@Serializable data class ScheduleRun(val time: Long = 0, val status: String = "", val summary: String = "", val chat: String? = null, val ms: Long = 0, val missed: Long? = null, val how: String = "")
@Serializable data class ScheduleHistoryResp(val task: ScheduledTask? = null, val history: List<ScheduleRun> = emptyList())
@Serializable data class SchedulePreviewRun(val at: Long = 0, val label: String = "")
@Serializable data class SchedulePreview(val repeat: String = "", val runs: List<SchedulePreviewRun> = emptyList(), val text: String = "")
@Serializable data class Machine(val name: String, val host: String = "", val user: String = "", val port: Int = 22, val note: String = "", val hasKey: Boolean = false)
@Serializable data class MachinesResp(val machines: List<Machine> = emptyList())
// ---- machines: status, runner, copy of connected apps (docs/RUNNER.md, docs/VM-MIGRATION-PLAN.md sections 2, 4, 6): the `machine_*` ops ----
/** What the runner on a machine says about itself (`machine_status`.runner). */
@Serializable data class RunnerInfo(
  val version: String = "", val timerActive: Boolean? = null, val enabledTasks: Int = 0, val tasks: Int = 0, val lastRun: Long? = null,
  val lastStatus: String? = null, val hasModel: Boolean = false, val model: String = "", val hasKey: Boolean = false,
)
@Serializable data class AppsCopiedServer(val name: String, val credentials: Boolean = false)
@Serializable data class AppsCopied(val at: Long = 0, val servers: List<AppsCopiedServer> = emptyList())
/** One machine: reachable now? runner installed? [error] is the sentence when it is not reachable. */
@Serializable data class MachineStatus(
  val name: String, val reachable: Boolean = false, val installed: Boolean = false, val error: String? = null, val runner: RunnerInfo? = null,
  val commands: Boolean = false, val files: Boolean = false, val appsCopied: AppsCopied? = null, val hasKey: Boolean = false,
)
@Serializable data class MachineStatusResp(val machines: List<MachineStatus> = emptyList())
/** The wizard's answer: the public key line and the one command that puts it on the machine. The private key never leaves the server. */
@Serializable data class MachineNew(val name: String = "", val publicKey: String = "", val authorizeCommand: String = "")
@Serializable data class ModelPlan(val provider: String = "", val model: String = "", val can: Boolean = false, val why: String = "", val needsKey: Boolean = false, val confirm: String = "")
@Serializable data class AppPlan(
  val name: String, val kind: String = "", val target: String = "", val credentials: List<String> = emptyList(), val oauth: Boolean = false,
  val warnings: List<String> = emptyList(), val copied: Boolean = false, val copiedWithCredentials: Boolean = false,
)
@Serializable data class AppsPlanResp(val machine: String = "", val hasRunner: Boolean = false, val servers: List<AppPlan> = emptyList())
@Serializable data class AppCheck(val name: String = "", val present: Boolean = false, val hint: String = "")
@Serializable data class AppsCheckResp(val servers: List<AppCheck> = emptyList())
@Serializable data class AppsCopyResp(val copied: Int = 0, val credentials: Int = 0, val synced: Int = 0, val removed: List<String> = emptyList())
@Serializable data class RunTaskResp(val task: String = "", val byRunner: Boolean = false)
@Serializable data class StepsResp(val steps: List<String> = emptyList())
@Serializable data class McpServer(val name: String, val command: String = "", val enabled: Boolean = true, val held: Boolean = false, val registry: String? = null, val addressChanged: String? = null)
// ---- connectors directory (docs/MCP.md "Browse connectors"): `connectors` op and the connector_* ops ----
@Serializable data class ConnectorHeader(val name: String, val description: String = "", val secret: Boolean = false, val required: Boolean = false, val value: String? = null)
@Serializable data class ConnectorRemote(val url: String, val headers: List<ConnectorHeader> = emptyList())
@Serializable data class ConnectorEntry(
  val id: String, val title: String = "", val description: String = "", val version: String = "", val website: String? = null,
  val publisher: String = "", val category: String = "other", val featured: Boolean = false, val trust: String = "", val kind: String = "remote",
  val remotes: List<ConnectorRemote> = emptyList(),
)
@Serializable data class ConnectorsResp(
  val restricted: Boolean = false, val message: String = "", val entries: List<ConnectorEntry> = emptyList(), val featured: List<ConnectorEntry> = emptyList(),
  val next: String? = null, val stale: Boolean = false, val error: String? = null,
)
@Serializable data class ConnectorPreview(val id: String = "", val name: String, val kind: String = "remote", val target: String = "", val tools: List<String> = emptyList(), val publisher: String = "", val trust: String = "", val notice: String = "")
@Serializable data class McpResp(val servers: List<McpServer> = emptyList())

// ---- account ----
@Serializable data class TermsState(val version: String = "", val accepted: Boolean = false)
@Serializable data class Limits(val requestsPerMin: Int = 0, val concurrentSends: Int = 0, val messageChars: Int? = null, val dataBytes: Long? = null, val toolSteps: Int? = null, val answerSecs: Int? = null)
@Serializable data class Whoami(
  val admin: Boolean = false, val restricted: Boolean = true, val email: String = "", val tenant: String = "",
  val limits: Limits = Limits(), val dataBytes: Long = 0, val terms: TermsState = TermsState(),
)
@Serializable data class VersionResp(val version: String = "")
@Serializable data class AccountDevice(val id: String, val name: String = "", val kind: String = "", val scope: String = "", val created: Long = 0, val lastUsed: Long = 0)
@Serializable data class DevicesResp(val devices: List<AccountDevice> = emptyList())
@Serializable data class DeleteAccountResp(val loginRemoved: Boolean = true)
@Serializable data class PushConfig(val vapidPublicKey: String = "", val fcm: Boolean = false)
@Serializable data class PushSubscription(val id: String, val kind: String = "", val label: String = "", val created: Long = 0)
@Serializable data class PushListResp(val subscriptions: List<PushSubscription> = emptyList())
@Serializable data class ClaudeStatus(val present: Boolean = false, val source: String = "none", val setAt: Long = 0, val admin: Boolean = false)
@Serializable data class DeviceLookup(val name: String = "", val kind: String = "", val requestedAt: Long = 0, val ip: String = "", val sameNetwork: Boolean = true, val email: String = "")
@Serializable data class LinkCaps(val claudeCode: Boolean = false, val approvalsFromWeb: Boolean = false, val version: String = "", val os: String = "")
@Serializable data class LinkDevice(val id: String, val name: String = "", val online: Boolean = false, val lastSeen: Long = 0, val caps: LinkCaps = LinkCaps())
@Serializable data class LinkStatusResp(val devices: List<LinkDevice> = emptyList())
