package org.zohara.agent.settings

import android.content.Context
import android.net.Uri
import android.provider.OpenableColumns
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.zohara.agent.LocalAppContainer
import org.zohara.agent.core.design.ErrorLine
import org.zohara.agent.core.design.PillButton
import org.zohara.agent.core.design.PrimaryButton
import org.zohara.agent.core.design.SecondaryButton
import org.zohara.agent.core.design.SectionHeader
import org.zohara.agent.core.design.SettingsCard
import org.zohara.agent.core.design.ZoBottomSheet
import org.zohara.agent.core.design.ZoSpinner
import org.zohara.agent.core.design.ZoSwitch
import org.zohara.agent.core.design.ZoTextField
import org.zohara.agent.core.design.ZoTheme
import org.zohara.agent.core.net.ApiException
import org.zohara.agent.core.net.LinkDevice
import org.zohara.agent.core.net.Machine
import org.zohara.agent.core.net.MigrateBatch
import org.zohara.agent.core.net.MigrateFile
import org.zohara.agent.core.net.MigrateInstruction
import org.zohara.agent.core.net.MigrateNeed
import org.zohara.agent.core.net.MigrateResult
import org.zohara.agent.core.net.MigrateScan
import org.zohara.agent.core.net.MigrateServer
import org.zohara.agent.core.net.MigrateSource
import org.zohara.agent.core.net.migrateApply
import org.zohara.agent.core.net.migrateHistory
import org.zohara.agent.core.net.migrateScan
import org.zohara.agent.core.net.migrateUndo
import org.zohara.agent.link.DesktopOps

/** The pure rules of Settings > Import (docs/MIGRATE.md); the screens only draw them. Unit tested. */
object ImportLogic {
  /** The server reads at most this much text at once (its request limit is 1 MB). */
  const val MAX_BYTES = 900 * 1024
  const val MAX_FILES = 12
  const val INTRO = "Bring in the MCP servers and instruction files you already set up in Claude Code, Claude Desktop, Codex CLI, Gemini CLI, Cursor, VS Code and Zed. Nothing is imported until you tick it, and nothing that exists here is overwritten."
  const val PRIVACY = "Pasted text goes to the Zohara Agent server to be read and is not saved. Remove API keys and tokens from it first if you can. Keys are never copied from this app: an imported server gets an empty placeholder, and you add the key on your computer."
  const val NO_COMPUTER = "Link a computer first (Settings > My computers), or paste the files below."
  const val KEYS_NOT_COPIED = "A server is only saved, not started; the first time the chat uses it, it asks first. Keys are not copied from here: a server that needs one comes in with an empty placeholder."

  /** "" when the files can be sent, else the sentence. */
  fun validate(files: List<MigrateFile>): String = when {
    files.isEmpty() || files.all { it.text.isBlank() } -> "Paste some text first."
    files.size > MAX_FILES -> "At most $MAX_FILES files at once."
    files.sumOf { it.text.toByteArray().size } > MAX_BYTES -> "That is too much text at once (900 KB at most)."
    else -> ""
  }

  /** Why a server cannot be ticked, or null when it can. */
  fun blocked(s: MigrateServer, can: org.zohara.agent.core.net.MigrateCan): String? = when {
    !can.servers -> "MCP servers are not available to this account."
    s.kind == "program" && !can.programs -> "Programs run on a computer: add this one in the desktop app."
    s.duplicate == "here" -> "Already added as ${s.duplicateOf.ifBlank { s.name }} (kept as it is)."
    s.duplicate == "scan" -> "The same server is listed above; pick one."
    else -> null
  }

  /** The ids ticked at first: every server that can be imported and every instruction file. */
  fun defaultSelection(scan: MigrateScan): Set<String> =
    scan.servers.filter { blocked(it, scan.can) == null }.map { it.id }.toSet() + scan.instructions.map { it.id }

  /** (value, label) of the places an instruction file can go. A restricted account only has the memory. */
  fun targets(scan: MigrateScan): List<Pair<String, String>> {
    val out = mutableListOf("notes" to "Memory notes (everywhere)")
    if (scan.can.projects) for (p in scan.projects) {
      out += "instruction:${p.id}" to "Add to the instruction of project ${p.name}"
      out += "notes:${p.id}" to "Memory notes of project ${p.name}"
    }
    return out
  }

  fun defaultTarget(scan: MigrateScan, i: MigrateInstruction): String {
    val want = i.suggest?.let { "instruction:$it" }
    return if (want != null && targets(scan).any { it.first == want }) want else "notes"
  }

  fun needText(n: MigrateNeed): String = if (n.from.isNullOrBlank()) n.name else "${n.name} (from ${n.from})"

  fun serverTitle(s: MigrateServer): String = s.name + if (s.kind == "program") "  (runs a program)" else "  (remote)"

  fun serverMeta(s: MigrateServer): String = buildString {
    append(s.source)
    if (s.folder.isNotBlank()) append(" · for the folder ${s.folder}")
    if (s.disabled) append(" · switched off there (stays off)")
  }

  fun instructionMeta(i: MigrateInstruction): String = buildString {
    append("${i.lines} lines · ${i.notes} note${if (i.notes == 1) "" else "s"}")
    if (i.notesMore > 0) append(" (${i.notesMore} more rules are not imported)")
    if (i.secretLines > 0) append(" · ${i.secretLines} line(s) that look like keys are left out")
  }

  fun foundTools(scan: MigrateScan): String {
    val names = scan.tools.filter { it.found }.map { it.name }
    return if (names.isEmpty()) "None of the supported tools left settings there." else "Settings of: ${names.joinToString(", ")}."
  }

  /** The lines of the result sheet. */
  fun resultLines(r: MigrateResult): List<String> = buildList {
    r.servers.skipped.forEach { add("Left out ${it.name}: ${it.why}") }
    r.instructions.skipped.forEach { add("Left out ${it.source}: ${it.why}") }
    r.needs.forEach { n -> add("${n.server} still needs: ${n.needs.joinToString(", ") { it.name }}") }
  }

  /** The sentence for an error of a relayed call (a computer that said no, was off or too slow). */
  fun explain(e: ApiException): String = DesktopOps.explain(e).message ?: e.text
}

/** Reads the files the person picked (at most [ImportLogic.MAX_FILES], [ImportLogic.MAX_BYTES] in all). */
private suspend fun readPicked(context: Context, uris: List<Uri>): Result<List<MigrateFile>> = withContext(Dispatchers.IO) {
  runCatching {
    var total = 0
    uris.take(ImportLogic.MAX_FILES).map { u ->
      val name = context.contentResolver.query(u, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { c -> if (c.moveToFirst()) c.getString(0) else null } ?: "file"
      val bytes = context.contentResolver.openInputStream(u)?.use { it.readNBytes(ImportLogic.MAX_BYTES + 1) } ?: ByteArray(0)
      total += bytes.size
      if (total > ImportLogic.MAX_BYTES) throw IllegalStateException("That is too much text at once (900 KB at most).")
      MigrateFile(name, bytes.toString(Charsets.UTF_8))
    }
  }
}

private class Found(val src: MigrateSource, val scan: MigrateScan)
private class Done(val src: MigrateSource, val result: MigrateResult)

@Composable
fun SettingsImportScreen() {
  val container = LocalAppContainer.current
  val account = rememberAccount()
  val restricted = account?.restricted != false
  val scope = rememberCoroutineScope()
  val doer = rememberDoer()
  val context = LocalContext.current
  var busy by remember { mutableStateOf<String?>(null) }
  var paste by remember { mutableStateOf(false) }
  var found by remember { mutableStateOf<Found?>(null) }
  var done by remember { mutableStateOf<Done?>(null) }
  var undo by remember { mutableStateOf<MigrateBatch?>(null) }
  val loader = rememberLoader(restricted) {
    val devices: List<LinkDevice> = if (restricted) emptyList() else runCatching { container.api.linkStatus() }.getOrDefault(emptyList())
    val machines: List<Machine> = if (restricted) emptyList() else runCatching { container.api.machines() }.getOrDefault(emptyList())
    val history = runCatching { container.api.migrateHistory() }.getOrDefault(emptyList())
    Triple(devices.filter { it.online }, machines, history)
  }
  val load by loader.state.collectAsState()

  fun look(src: MigrateSource, what: String) {
    busy = what
    scope.launch {
      try {
        found = Found(src, container.api.migrateScan(src))
      } catch (e: ApiException) {
        doer.say(ImportLogic.explain(e))
      } finally { busy = null }
    }
  }
  val picker = rememberLauncherForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { uris ->
    if (uris.isEmpty()) return@rememberLauncherForActivityResult
    scope.launch {
      readPicked(context, uris).onSuccess { files ->
        val bad = ImportLogic.validate(files)
        if (bad.isNotEmpty()) doer.say(bad) else look(MigrateSource.Paste(files), "Reading…")
      }.onFailure { doer.say(it.message ?: "Those files could not be read.") }
    }
  }

  SettingsPage("Import") {
    LoadBox(load, { loader.load() }) { (online, machines, history) ->
      Text(ImportLogic.INTRO, style = ZoTheme.type.fieldHint, color = ZoTheme.colors.dimmer, modifier = Modifier.padding(bottom = 8.dp))
      SectionHeader("Find other tools' settings")
      SettingsCard {
        if (!restricted) {
          if (online.isEmpty()) ActionRow("From my computer", ImportLogic.NO_COMPUTER)
          for (d in online) ActionRow(
            "From ${d.name.ifBlank { "my computer" }}",
            "Reads the settings files of Claude Code, Cursor and the others on that computer. It asks you first and sends back only names and short previews, never a key.",
            actions = { PrimaryButton("Detect", { look(MigrateSource.Computer(d.id, d.name), "Waiting for your approval on your computer…") }, enabled = busy == null) },
          )
          for (m in machines) ActionRow(
            "Machine ${m.name}", "Reads the same files there with ordinary read commands (read-only over SSH). Nothing is written on the machine.",
            actions = { PrimaryButton("Detect", { look(MigrateSource.Machine(m.name), "Reading the files on ${m.name}…") }, enabled = busy == null) },
          )
        }
        ActionRow(
          "Paste or upload files",
          "A claude_desktop_config.json, .mcp.json, config.toml, settings.json, mcp.json, CLAUDE.md, AGENTS.md or GEMINI.md. The text is read by the server and not kept.",
          divider = false,
          actions = {
            PillButton("Paste…", { paste = true }, enabled = busy == null)
            Spacer(Modifier.size(6.dp))
            PillButton("Upload files…", { picker.launch(arrayOf("*/*")) }, enabled = busy == null)
          },
        )
      }
      busy?.let { Row(Modifier.padding(top = 10.dp), verticalAlignment = Alignment.CenterVertically) { ZoSpinner(size = 16.dp); Spacer(Modifier.size(8.dp)); Text(it, style = ZoTheme.type.fieldHint, color = ZoTheme.colors.dim) } }
      Text(ImportLogic.PRIVACY, style = ZoTheme.type.fieldHint, color = ZoTheme.colors.dimmer, modifier = Modifier.padding(top = 10.dp))
      if (history.isNotEmpty()) {
        SectionHeader("Imports you can undo")
        SettingsCard {
          history.forEachIndexed { i, b ->
            ActionRow(b.summary.ifBlank { "An import" }, b.origin, divider = i < history.lastIndex, actions = { PillButton("Undo", { undo = b }) })
          }
        }
      }
    }
  }
  if (paste) PasteSheet(onDismiss = { paste = false }) { files -> paste = false; look(MigrateSource.Paste(files), "Reading…") }
  found?.let { f ->
    ReviewSheet(f.scan, f.src, onDismiss = { found = null }) { r -> found = null; done = Done(f.src, r); loader.load() }
  }
  done?.let { d ->
    ResultSheet(d.result, onUndo = {
      scope.launch {
        try { doer.say(container.api.migrateUndo(d.src, d.result.batch)); done = null; loader.load() } catch (e: ApiException) { doer.say(ImportLogic.explain(e)) }
      }
    }, onDismiss = { done = null })
  }
  undo?.let { b ->
    ConfirmSheet("Undo this import?", "What it added is taken back, unless you changed it since (those stay).", "Undo", {
      doer.run(onDone = { loader.load() }) { doer.say(container.api.migrateUndo(null, b.id)) }
    }, { undo = null })
  }
}

@Composable
private fun PasteSheet(onDismiss: () -> Unit, onLook: (List<MigrateFile>) -> Unit) {
  var name by remember { mutableStateOf("") }
  var text by remember { mutableStateOf("") }
  var error by remember { mutableStateOf<String?>(null) }
  ZoBottomSheet(
    "Paste a settings file", onDismiss,
    footer = {
      SecondaryButton("Cancel", onDismiss)
      PrimaryButton("Look at it", {
        val files = listOf(MigrateFile(name.trim().ifBlank { "pasted.txt" }, text))
        val bad = ImportLogic.validate(files)
        if (bad.isNotEmpty()) error = bad else onLook(files)
      })
    },
  ) {
    Column(Modifier.heightIn(max = 520.dp).verticalScroll(rememberScrollState())) {
      Text("Paste the text of one file. Nothing is imported until you tick it on the next screen.", style = ZoTheme.type.fieldHint, color = ZoTheme.colors.dimmer)
      ZoTextField(name, { name = it.take(80) }, label = "File name (helps to recognise it)", placeholder = "claude_desktop_config.json", modifier = Modifier.fillMaxWidth().padding(top = 8.dp))
      ZoTextField(text, { text = it }, label = "Text", singleLine = false, minLines = 8, mono = true, modifier = Modifier.fillMaxWidth().padding(top = 8.dp))
      error?.let { ErrorLine(it, Modifier.padding(top = 8.dp)) }
    }
  }
}

/** The checklist: what was found, ticked by default where it can be imported. */
@Composable
private fun ReviewSheet(scan: MigrateScan, src: MigrateSource, onDismiss: () -> Unit, onDone: (MigrateResult) -> Unit) {
  val container = LocalAppContainer.current
  val scope = rememberCoroutineScope()
  val ticked = remember { mutableStateListOf<String>().also { it.addAll(ImportLogic.defaultSelection(scan)) } }
  val targets = remember { mutableStateMapOf<String, String>().also { m -> scan.instructions.forEach { m[it.id] = ImportLogic.defaultTarget(scan, it) } } }
  var busy by remember { mutableStateOf(false) }
  var error by remember { mutableStateOf<String?>(null) }
  fun toggle(id: String, on: Boolean) { if (on) { if (id !in ticked) ticked.add(id) } else ticked.remove(id) }
  val nothing = scan.servers.isEmpty() && scan.instructions.isEmpty()
  ZoBottomSheet(
    "Found in ${scan.origin}", onDismiss,
    footer = {
      SecondaryButton("Close", onDismiss)
      PrimaryButton(if (busy && src is MigrateSource.Computer) "Waiting for your computer…" else "Import ticked", {
        if (ticked.isEmpty()) { error = "Tick something first."; return@PrimaryButton }
        busy = true; error = null
        scope.launch {
          try {
            onDone(container.api.migrateApply(src, ticked.toList(), targets.filterKeys { it in ticked }))
          } catch (e: ApiException) { error = ImportLogic.explain(e) } finally { busy = false }
        }
      }, enabled = !nothing, loading = busy)
    },
  ) {
    ReviewBody(scan, ticked, targets, ::toggle, { id, v -> targets[id] = v }, error)
  }
}

/** The checklist without any state holder, so a screenshot test can draw exactly what the person sees. */
@Composable
fun ReviewBody(scan: MigrateScan, ticked: List<String>, targets: Map<String, String>, onToggle: (String, Boolean) -> Unit, onTarget: (String, String) -> Unit, error: String?) {
  val nothing = scan.servers.isEmpty() && scan.instructions.isEmpty()
  Column(Modifier.heightIn(max = 560.dp).verticalScroll(rememberScrollState())) {
    Text(ImportLogic.foundTools(scan), style = ZoTheme.type.fieldHint, color = ZoTheme.colors.dimmer)
    scan.notes.take(12).forEach { Text(it.take(300), style = ZoTheme.type.fieldHint, color = ZoTheme.colors.dimmer) }
    if (scan.servers.isNotEmpty()) {
      Text("MCP servers", style = ZoTheme.type.cardName, color = ZoTheme.colors.text, modifier = Modifier.padding(top = 12.dp))
      Text(ImportLogic.KEYS_NOT_COPIED, style = ZoTheme.type.fieldHint, color = ZoTheme.colors.dimmer)
      if (!scan.can.servers) ErrorLine("MCP servers are not available to this account. Only the instructions below can be imported.", Modifier.padding(top = 6.dp))
      var last = ""
      for (s in scan.servers.take(300)) {
        if (s.toolName != last) { last = s.toolName; Text(s.toolName, style = ZoTheme.type.cardMeta, color = ZoTheme.colors.dim, modifier = Modifier.padding(top = 10.dp)) }
        val why = ImportLogic.blocked(s, scan.can)
        Row(Modifier.padding(top = 6.dp), verticalAlignment = Alignment.Top) {
          ZoSwitch(s.id in ticked, { onToggle(s.id, it) }, enabled = why == null, label = "Import ${s.name}")
          Column(Modifier.weight(1f).padding(start = 8.dp)) {
            Text(ImportLogic.serverTitle(s), style = ZoTheme.type.cardName, color = ZoTheme.colors.text)
            Text(s.target.take(300), style = ZoTheme.type.cardMeta, color = ZoTheme.colors.dim)
            Text(ImportLogic.serverMeta(s).take(300), style = ZoTheme.type.fieldHint, color = ZoTheme.colors.dimmer)
            why?.takeIf { scan.can.servers }?.let { Text(it, style = ZoTheme.type.fieldHint, color = ZoTheme.colors.dimmer) }
            s.notes.take(3).forEach { Text(it.take(200), style = ZoTheme.type.fieldHint, color = ZoTheme.colors.dimmer) }
            if (s.needs.isNotEmpty()) Text("Needs: ${s.needs.take(10).joinToString(", ") { ImportLogic.needText(it) }}. It comes in with an empty placeholder.", style = ZoTheme.type.fieldHint, color = ZoTheme.colors.dimmer)
          }
        }
      }
    }
    if (scan.instructions.isNotEmpty()) {
      Text("Instructions", style = ZoTheme.type.cardName, color = ZoTheme.colors.text, modifier = Modifier.padding(top = 14.dp))
      Text("An instruction file becomes memory notes (one per rule)${if (scan.can.projects) ", or is added to the end of a project's instruction" else ""}. Lines that look like keys are left out.", style = ZoTheme.type.fieldHint, color = ZoTheme.colors.dimmer)
      for (i in scan.instructions.take(60)) {
        val opts = ImportLogic.targets(scan)
        Row(Modifier.padding(top = 6.dp), verticalAlignment = Alignment.Top) {
          ZoSwitch(i.id in ticked, { onToggle(i.id, it) }, label = "Import ${i.source}")
          Column(Modifier.weight(1f).padding(start = 8.dp)) {
            Text("${i.source.take(120)} (${i.toolName})", style = ZoTheme.type.cardName, color = ZoTheme.colors.text)
            Text(ImportLogic.instructionMeta(i), style = ZoTheme.type.cardMeta, color = ZoTheme.colors.dim)
            if (i.preview.isNotBlank()) Text("“${i.preview.take(180)}”", style = ZoTheme.type.fieldHint, color = ZoTheme.colors.dimmer)
            if (opts.size > 1) Row(Modifier.horizontalScroll(rememberScrollState()).padding(top = 6.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
              for ((v, label) in opts) PillButton(label, { onTarget(i.id, v) }, accent = targets[i.id] == v)
            }
          }
        }
      }
    }
    if (scan.extras.isNotEmpty()) {
      Text("Found, but not imported", style = ZoTheme.type.cardName, color = ZoTheme.colors.text, modifier = Modifier.padding(top = 14.dp))
      Text(scan.extras.take(80).joinToString("\n") { "${it.toolName} ${it.kind}: ${it.name}" }, style = ZoTheme.type.fieldHint, color = ZoTheme.colors.dimmer)
    }
    if (nothing) Text("There is nothing to import here.", style = ZoTheme.type.fieldHint, color = ZoTheme.colors.dimmer, modifier = Modifier.padding(top = 10.dp))
    error?.let { ErrorLine(it, Modifier.padding(top = 8.dp)) }
  }
}


@Composable
private fun ResultSheet(r: MigrateResult, onUndo: () -> Unit, onDismiss: () -> Unit) {
  ZoBottomSheet(
    "Imported", onDismiss,
    footer = {
      if (r.batch.isNotBlank()) SecondaryButton("Undo", onUndo)
      PrimaryButton("Done", onDismiss)
    },
  ) {
    Column(Modifier.heightIn(max = 480.dp).verticalScroll(rememberScrollState())) {
      Text(r.message.ifBlank { "Done." }.take(300), style = ZoTheme.type.cardName, color = ZoTheme.colors.text)
      for (l in ImportLogic.resultLines(r).take(60)) Text(l.take(300), style = ZoTheme.type.fieldHint, color = ZoTheme.colors.dimmer, modifier = Modifier.padding(top = 4.dp))
    }
  }
}
