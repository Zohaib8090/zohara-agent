package org.zohara.agent.core.net

import kotlinx.serialization.Serializable
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.decodeFromJsonElement
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonArray
import kotlinx.serialization.json.putJsonObject

// ---- Migrate (docs/MIGRATE.md): the `migrate_*` ops. What came from other people's files is only ever shown as text. ----

@Serializable data class MigrateNeed(val kind: String = "", val name: String = "", val from: String? = null, val inFile: Boolean = false)

@Serializable data class MigrateServer(
  val id: String, val tool: String = "", val toolName: String = "", val source: String = "", val scope: String = "", val folder: String = "",
  val name: String = "", val kind: String = "remote", val target: String = "", val disabled: Boolean = false,
  val needs: List<MigrateNeed> = emptyList(), val notes: List<String> = emptyList(),
  /** "" (new), "here" (a server with this name or command is already added) or "scan" (listed twice in what was found) */
  val duplicate: String = "", val duplicateOf: String = "",
)

@Serializable data class MigrateInstruction(
  val id: String, val tool: String = "", val toolName: String = "", val source: String = "", val scope: String = "global", val folder: String = "",
  val chars: Int = 0, val lines: Int = 0, val preview: String = "", val notes: Int = 0, val notesMore: Int = 0, val secretLines: Int = 0,
  /** the id of the project whose name is the folder's name */
  val suggest: String? = null,
)

@Serializable data class MigrateExtra(val id: String = "", val tool: String = "", val toolName: String = "", val kind: String = "", val name: String = "", val note: String = "")
@Serializable data class MigrateTool(val id: String, val name: String = "", val found: Boolean = false, val files: List<String> = emptyList())
@Serializable data class MigrateProject(val id: String, val name: String = "")

/** What this account may import: a restricted one has no MCP servers and no projects; the cloud server cannot start programs or keep keys. */
@Serializable data class MigrateCan(val servers: Boolean = true, val programs: Boolean = true, val projects: Boolean = true, val secrets: Boolean = false)

@Serializable data class MigrateScan(
  val origin: String = "", val kind: String = "", val tools: List<MigrateTool> = emptyList(), val servers: List<MigrateServer> = emptyList(),
  val instructions: List<MigrateInstruction> = emptyList(), val extras: List<MigrateExtra> = emptyList(), val notes: List<String> = emptyList(),
  val projects: List<MigrateProject> = emptyList(), val can: MigrateCan = MigrateCan(), val viaLink: Boolean = false,
)

@Serializable data class MigrateSkipped(val name: String = "", val source: String = "", val why: String = "")
@Serializable data class MigrateServersOut(val added: List<String> = emptyList(), val skipped: List<MigrateSkipped> = emptyList())
@Serializable data class MigrateInstructionsOut(val skipped: List<MigrateSkipped> = emptyList())
@Serializable data class MigrateNeeds(val server: String = "", val needs: List<MigrateNeed> = emptyList())
@Serializable data class MigrateResult(
  val message: String = "", val batch: String = "", val servers: MigrateServersOut = MigrateServersOut(),
  val instructions: MigrateInstructionsOut = MigrateInstructionsOut(), val needs: List<MigrateNeeds> = emptyList(),
)
@Serializable data class MigrateBatch(val id: String, val time: Long = 0, val origin: String = "", val summary: String = "")
@Serializable data class MigrateHistory(val batches: List<MigrateBatch> = emptyList())

/** One file to read: its name decides how it is read (`claude_desktop_config.json`, `CLAUDE.md`, `config.toml`...). */
data class MigrateFile(val name: String, val text: String)

/** Where the files are read: pasted text, a machine added in Settings (admins), or a linked computer (it asks its person first). */
sealed interface MigrateSource {
  data class Paste(val files: List<MigrateFile>) : MigrateSource
  data class Machine(val name: String) : MigrateSource
  data class Computer(val device: String, val name: String) : MigrateSource
}

private fun JsonObject.migrate(): MigrateScan = ZoJson.decodeFromJsonElement(this as JsonElement)

/** Reads the files of other tools and says what is in them. Nothing is changed. */
suspend fun ZoApi.migrateScan(src: MigrateSource): MigrateScan = when (src) {
  is MigrateSource.Paste -> op("migrate_scan") { put("source", "paste"); putFiles(src.files) }.migrate()
  is MigrateSource.Machine -> op("migrate_scan") { put("source", "machine"); put("machine", src.name) }.migrate()
  is MigrateSource.Computer -> desktopOp(src.device, buildJsonObject { put("op", "migrate_scan") }).migrate()
}

/** Imports what is ticked. Keys are never sent or asked for here. */
suspend fun ZoApi.migrateApply(src: MigrateSource, select: List<String>, targets: Map<String, String>): MigrateResult {
  val r = when (src) {
    is MigrateSource.Paste -> op("migrate_apply") { put("source", "paste"); putFiles(src.files); putSelection(select, targets) }
    is MigrateSource.Machine -> op("migrate_apply") { put("source", "machine"); put("machine", src.name); putSelection(select, targets) }
    is MigrateSource.Computer -> desktopOp(src.device, buildJsonObject { put("op", "migrate_apply"); putSelection(select, targets) })
  }
  return ZoJson.decodeFromJsonElement(r as JsonElement)
}

suspend fun ZoApi.migrateUndo(src: MigrateSource?, batch: String): String {
  val r = if (src is MigrateSource.Computer) desktopOp(src.device, buildJsonObject { put("op", "migrate_undo"); put("batch", batch) }) else op("migrate_undo") { put("batch", batch) }
  return (r["message"] as? JsonPrimitive)?.content.orEmpty()
}

suspend fun ZoApi.migrateHistory(): List<MigrateBatch> = ZoJson.decodeFromJsonElement<MigrateHistory>(op("migrate_history", true) as JsonElement).batches

private fun kotlinx.serialization.json.JsonObjectBuilder.putFiles(files: List<MigrateFile>) {
  putJsonArray("files") { files.forEach { f -> add(buildJsonObject { put("name", f.name); put("text", f.text) }) } }
}

private fun kotlinx.serialization.json.JsonObjectBuilder.putSelection(select: List<String>, targets: Map<String, String>) {
  putJsonArray("select") { select.forEach { add(JsonPrimitive(it)) } }
  putJsonObject("targets") { targets.forEach { (k, v) -> put(k, v) } }
}
