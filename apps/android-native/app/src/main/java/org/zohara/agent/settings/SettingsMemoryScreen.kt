package org.zohara.agent.settings

import android.content.Intent
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch
import org.zohara.agent.LocalAppContainer
import org.zohara.agent.core.design.DangerButton
import org.zohara.agent.core.design.PillButton
import org.zohara.agent.core.design.PrimaryButton
import org.zohara.agent.core.design.SecondaryButton
import org.zohara.agent.core.design.SectionHeader
import org.zohara.agent.core.design.SettingsCard
import org.zohara.agent.core.design.ZoBottomSheet
import org.zohara.agent.core.design.ZoIconButton
import org.zohara.agent.core.design.ZoIcons
import org.zohara.agent.core.design.ZoSwitch
import org.zohara.agent.core.design.ZoTextField
import org.zohara.agent.core.design.ZoTheme
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import org.zohara.agent.chat.MemoryChangesSheet
import org.zohara.agent.core.net.ApiException
import org.zohara.agent.core.net.ErrorKind
import org.zohara.agent.core.net.MemoryItem
import org.zohara.agent.core.net.MemoryResp

/** The rules of the Memory page (SET-MEM-01, 03). */
object MemoryLogic {
  const val GROUP_LIMIT = 80
  val GROUPS = listOf("person" to "About you", "preference" to "Preferences", "project" to "Projects", "fact" to "Facts", "episode" to "Chat summaries")

  data class Group(val kind: String, val label: String, val shown: List<MemoryItem>)

  /** Pinned first, then newest; at most [GROUP_LIMIT] per group; empty groups are left out. */
  fun groups(items: List<MemoryItem>): List<Group> {
    val sorted = items.sortedWith(compareByDescending<MemoryItem> { it.pinned }.thenByDescending { it.updated })
    return GROUPS.mapNotNull { (k, label) ->
      val mine = sorted.filter { it.kind == k }
      if (mine.isEmpty()) null else Group(k, label, mine.take(GROUP_LIMIT))
    }
  }

  /** How many are kept but not listed. */
  fun omitted(items: List<MemoryItem>): Int = groups(items).sumOf { g -> items.count { it.kind == g.kind } - g.shown.size }

  fun counts(r: MemoryResp): Pair<Int, Int> { val summaries = r.items.count { it.kind == "episode" }; return (r.items.size - summaries) to summaries }

  /** The notes the page lists for [scope]: "all", "global" (every chat) or a project id. */
  fun inScope(items: List<MemoryItem>, scope: String): List<MemoryItem> = when (scope) {
    "all", "" -> items
    "global" -> items.filter { it.scope.isEmpty() }
    else -> items.filter { it.scope == scope }
  }

  /** The scope switch: (value, label); empty when the person has no project (nothing to switch). */
  fun scopes(r: MemoryResp): List<Pair<String, String>> =
    if (r.projects.isEmpty()) emptyList() else listOf("all" to "All notes", "global" to "Every chat") + r.projects.map { it.id to "Project: ${it.name}" }

  /** A scope that is no longer there (the project was deleted) falls back to all notes. */
  fun validScope(r: MemoryResp, scope: String): String = if (scope == "all" || scope == "global" || r.projects.any { it.id == scope }) scope else "all"

  /** The scope a new note goes to: a project's page adds to the project, the rest to the whole memory. */
  fun addScope(scope: String): String = if (scope == "all" || scope == "global") "" else scope

  /** The second line of a note: its text, and the project when it belongs to one. */
  fun noteText(i: MemoryItem): String = if (i.scope.isEmpty()) i.text else "${i.text}\nProject: ${i.scopeName.ifEmpty { "(gone)" }}"

  /** Every = how many messages of yours the agent waits for; at least [MemoryResp.minEvery]. */
  fun clampEvery(n: Int, min: Int): Int = n.coerceIn(min, 20)

  const val EXPORT_NAME = "zohara-memory.json"

  fun explainer(r: MemoryResp): String {
    val (notes, summaries) = counts(r)
    return "$notes notes about you and $summaries chat summaries, shared with the computer and the phone. After every ${r.every} messages of yours, Zohara Agent reads the chat in the background and keeps what is worth remembering. Never passwords or keys."
  }
}

/** Memory. Parity rows: SET-MEM-01..05. */
@Composable
fun SettingsMemoryScreen() {
  val container = LocalAppContainer.current
  val doer = rememberDoer()
  val loader = rememberLoader { container.api.memory() }
  val load by loader.state.collectAsState()
  var chat by remember { mutableStateOf(false) }
  var add by remember { mutableStateOf(false) }
  var edit by remember { mutableStateOf<MemoryItem?>(null) }
  var forgetAll by remember { mutableStateOf(false) }
  var changes by remember { mutableStateOf(false) }
  var scope by remember { mutableStateOf("all") }
  val context = androidx.compose.ui.platform.LocalContext.current
  val importer = androidx.activity.compose.rememberLauncherForActivityResult(androidx.activity.result.contract.ActivityResultContracts.OpenDocument()) { uri ->
    if (uri != null) doer.run(onDone = { loader.load() }) {
      val text = withContext(Dispatchers.IO) { context.contentResolver.openInputStream(uri)?.use { it.readBytes().toString(Charsets.UTF_8) }.orEmpty() }
      val data = try { Json.parseToJsonElement(text) as? JsonObject } catch (_: Exception) { null } ?: throw ApiException(ErrorKind.Rejected, "That file is not a memory export.")
      val r = container.api.memoryImport(data)
      doer.say("Imported ${r.added} note${if (r.added == 1) "" else "s"}. ${r.alreadyThere} were already here, ${r.skipped} were left out.")
    }
  }
  SettingsPage("Memory") {
    LoadBox(load, { loader.load() }) { m0 ->
      val m = m0.copy(items = MemoryLogic.inScope(m0.items, MemoryLogic.validScope(m0, scope)))
      var enabled by remember(m0) { mutableStateOf(m0.enabled) }
      var showUpdates by remember(m0) { mutableStateOf(m0.showUpdates) }
      var every by remember(m0) { mutableStateOf(m0.every) }
      MemoryContent(
        m, enabled,
        onEnabled = { on -> enabled = on; doer.run(onFail = { enabled = !on; doer.say(it) }) { container.api.memoryEnable(on) } },
        onChat = { chat = true },
        onEdit = { edit = it },
        onPin = { i -> doer.run(onDone = { loader.load() }) { container.api.memoryPin(i.id, !i.pinned) } },
        onForget = { i -> doer.run(onDone = { loader.load() }) { container.api.memoryForget(i.id) } },
        onAdd = { add = true },
        onForgetAll = { forgetAll = true },
        feed = MemoryFeedSettings(
          showUpdates = showUpdates,
          onShowUpdates = { on -> showUpdates = on; doer.run(onFail = { showUpdates = !on; doer.say(it) }) { container.api.memorySettings(showUpdates = on) } },
          every = every,
          onEvery = { n ->
            val k = MemoryLogic.clampEvery(n, m0.minEvery)
            every = k
            doer.run(onFail = { every = m0.every; doer.say(it) }) { every = container.api.memorySettings(every = k).every }
          },
          onChanges = { changes = true },
          scope = MemoryLogic.validScope(m0, scope),
          scopes = MemoryLogic.scopes(m0),
          onScope = { scope = it },
          onExport = {
            doer.run {
              val json = Json { prettyPrint = true }.encodeToString(JsonObject.serializer(), JsonObject(container.api.memoryExport().filterKeys { it != "ok" }))
              context.startActivity(
                Intent.createChooser(Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_SUBJECT, MemoryLogic.EXPORT_NAME).putExtra(Intent.EXTRA_TEXT, json), null).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
              )
            }
          },
          onImport = { importer.launch(arrayOf("application/json", "text/plain", "*/*")) },
        ),
      )
    }
  }
  if (changes) MemoryChangesSheet({ changes = false }, latestOnly = false)
  if (chat) MemoryChatSheet(onDismiss = { chat = false; loader.load() })
  if (add) InputSheet("Add a note", "Add", { add = false }, label = "What should Zohara Agent remember?", placeholder = "I prefer short answers", minLines = 4) { t -> container.api.memoryAdd(t, scope = MemoryLogic.addScope(scope)); loader.load() }
  edit?.let { i ->
    InputSheet("Edit note", "Save", { edit = null }, label = "What should Zohara Agent remember?", placeholder = "I prefer short answers", initial = i.text, minLines = 4) { t -> container.api.memoryUpdate(i.id, t); loader.load() }
  }
  if (forgetAll) ConfirmSheet("Forget everything?", "All notes and chat summaries are removed, on every device. Saved chats stay.", "Forget", { doer.run(onDone = { loader.load() }) { container.api.memoryClear() } }, { forgetAll = false })
}

/** The feed and project parts of the page: the chip switch, how often the agent reads, the list of changes, the scope switch, export and import. */
data class MemoryFeedSettings(
  val showUpdates: Boolean = true, val onShowUpdates: (Boolean) -> Unit = {}, val every: Int = 3, val onEvery: (Int) -> Unit = {},
  val onChanges: () -> Unit = {}, val scope: String = "all", val scopes: List<Pair<String, String>> = emptyList(), val onScope: (String) -> Unit = {},
  val onExport: () -> Unit = {}, val onImport: () -> Unit = {},
)

@Composable
fun MemoryContent(
  m: MemoryResp, enabled: Boolean, onEnabled: (Boolean) -> Unit, onChat: () -> Unit, onEdit: (MemoryItem) -> Unit, onPin: (MemoryItem) -> Unit,
  onForget: (MemoryItem) -> Unit, onAdd: () -> Unit, onForgetAll: () -> Unit, feed: MemoryFeedSettings = MemoryFeedSettings(showUpdates = m.showUpdates, every = m.every),
) {
  SectionHeader("Memory")
  SettingsCard {
    org.zohara.agent.core.design.SettingsRow(
      "Remember things between chats", subtitle = MemoryLogic.explainer(m),
      control = { ZoSwitch(enabled, onEnabled) },
    )
    ActionRow(
      "Edit by talking",
      "Tell Zohara Agent what to change, in your own words: \"join my preferences into one note\", \"forget the old laptop\", \"my name is spelled Zohaib\". This chat only edits the notes.",
      actions = { PrimaryButton("Chat about memory…", onChat) },
    )
    org.zohara.agent.core.design.SettingsRow(
      "Show memory updates", subtitle = "When Zohara Agent changes the memory, a small \"Memory updated · View\" note shows above the message box. View lists what changed and lets you undo it.",
      control = { ZoSwitch(feed.showUpdates, feed.onShowUpdates) },
    )
    ActionRow(
      "How often it reads your chats",
      "After this many messages of yours in a chat (at least ${m.minEvery}, normally 3). Fewer is more up to date, more is cheaper.",
      actions = {
        PillButton("−", { feed.onEvery(feed.every - 1) }, enabled = feed.every > m.minEvery)
        Text("${feed.every}", style = ZoTheme.type.body, color = ZoTheme.colors.text, modifier = Modifier.padding(horizontal = 6.dp))
        PillButton("+", { feed.onEvery(feed.every + 1) }, enabled = feed.every < 20)
      },
    )
    ActionRow("Recent changes", "What the memory agent added, updated or removed lately, with Undo for each.", actions = { PillButton("View changes", feed.onChanges) })
    if (feed.scopes.isNotEmpty()) {
      ActionRow(
        "Show", "Notes for every chat, and the notes Zohara Agent keeps for one project (only chats in that project are told them).",
        extra = {
          Row(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(vertical = 6.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            for ((v, label) in feed.scopes) PillButton(label, { feed.onScope(v) }, accent = v == feed.scope)
          }
        },
      )
    }
    for (g in MemoryLogic.groups(m.items)) {
      Text(g.label, style = ZoTheme.type.sectionHeader, color = ZoTheme.colors.dimmer, modifier = Modifier.padding(start = 16.dp, top = 14.dp, bottom = 2.dp))
      for (i in g.shown) {
        ActionRow(
          if (g.kind == "episode") i.title else "", MemoryLogic.noteText(i),
          actions = {
            if (g.kind != "episode") ZoIconButton(ZoIcons.Edit, "Edit this note", { onEdit(i) }, size = 36.dp)
            ZoIconButton(
              ZoIcons.Star, if (i.pinned) "Always tell the AI (pinned)" else "Pin: always tell the AI", { onPin(i) },
              tint = if (i.pinned) ZoTheme.colors.gold else ZoTheme.colors.icon, size = 36.dp,
            )
            ZoIconButton(ZoIcons.Trash, "Forget this", { onForget(i) }, tint = ZoTheme.colors.danger, size = 36.dp)
          },
        )
      }
    }
    val more = MemoryLogic.omitted(m.items)
    if (more > 0) ActionRow("…", "$more more are kept; the AI can search all of them.")
    ActionRow(
      "Your notes",
      "Pin a note to make sure the AI always knows it. Delete one that is wrong. You can also just tell Zohara Agent: \"remember that ...\" or \"forget that ...\".",
      divider = false,
      actions = { PillButton("Add a note…", onAdd); DangerButton("Forget everything", onForgetAll, outlined = true) },
    )
    ActionRow(
      "Export and import",
      "A file with all your notes, including each project's. Importing adds what is not there yet; a project's notes go to the project with the same name, or are left out.",
      divider = false,
      actions = { PillButton("Export…", feed.onExport); PillButton("Import…", feed.onImport) },
    )
  }
}

/** SET-MEM-02: a small chat that only edits the notes. */
@Composable
fun MemoryChatSheet(onDismiss: () -> Unit) {
  val container = LocalAppContainer.current
  val scope = rememberCoroutineScope()
  class Turn(val you: String, reply: String) { var reply by mutableStateOf(reply) }
  val log = remember { mutableStateListOf<Turn>() }
  var text by remember { mutableStateOf("") }
  var busy by remember { mutableStateOf(false) }
  val send = {
    val t = text.trim()
    if (t.isNotEmpty() && !busy) {
      text = ""
      val turn = Turn(t, "Working on it…")
      log.add(turn)
      busy = true
      scope.launch {
        try { turn.reply = container.api.memoryEdit(t).ifBlank { "Done." } } catch (e: ApiException) { turn.reply = e.text } finally { busy = false }
      }
    }
  }
  ZoBottomSheet(
    "Chat about memory", onDismiss,
    footer = { SecondaryButton("Done", onDismiss); PrimaryButton("Send", send, enabled = !busy && text.isNotBlank()) },
  ) {
    Column(Modifier.heightIn(max = 460.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
      Text("Say what to change. Zohara Agent edits the notes and tells you what it did.", style = ZoTheme.type.cardMeta, color = ZoTheme.colors.dim)
      for (t in log) {
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
          Text(t.you, style = ZoTheme.type.body, color = ZoTheme.colors.text, modifier = Modifier.padding(8.dp).let { it })
        }
        Text(t.reply, style = ZoTheme.type.cardMeta, color = ZoTheme.colors.dim, modifier = Modifier.padding(horizontal = 8.dp))
      }
      ZoTextField(text, { text = it }, placeholder = "For example: join all my preferences into one note", modifier = Modifier.fillMaxWidth().padding(top = 6.dp))
    }
  }
}
