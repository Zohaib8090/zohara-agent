package org.zohara.agent.settings

import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch
import org.zohara.agent.LocalAppContainer
import org.zohara.agent.core.design.ErrorLine
import org.zohara.agent.core.design.PillButton
import org.zohara.agent.core.design.PrimaryButton
import org.zohara.agent.core.design.SecondaryButton
import org.zohara.agent.core.design.ZoBottomSheet
import org.zohara.agent.core.design.ZoTextField
import org.zohara.agent.core.design.ZoTheme
import org.zohara.agent.core.net.ApiException
import org.zohara.agent.core.net.ConnectorEntry
import org.zohara.agent.core.net.ConnectorPreview
import org.zohara.agent.core.net.McpServer

/** Pure rules of the connectors directory screens (unit tested). */
object ConnectorsLogic {
  const val RESTRICTED = "This account cannot add connectors. Accounts on the cloud server get web search, page reading and memory only; connectors (MCP servers) can run programs and reach your accounts, so they are for the app on your own computer."
  const val INTRO = "Connectors are tools from other services that the chat can use. They come from the public MCP registry, read by the Zohara Agent server. Only the ones marked Featured were looked at by Zohara Agent. Every call of a connector still asks you first unless the mode is Bypass."
  const val LOCAL_NOTE = "Programs that run on a computer (npm or Python packages) are added in the desktop app, after a confirmation that names what will run."
  val CATEGORIES = listOf("all" to "All", "dev" to "Developer", "docs" to "Documents", "chat" to "Chat", "data" to "Data", "finance" to "Finance", "media" to "Media", "search" to "Search", "other" to "Other")

  /** What a connector's row in Connected apps says under its name. */
  fun rowSubtitle(s: McpServer): String =
    if (s.addressChanged != null) "Address changed: the directory now lists ${s.addressChanged}. It is held until you decide. ${s.command}" else s.command

  /** The id of every connector that came from the directory (these show "Added"). */
  fun addedIds(servers: List<McpServer>): Set<String> = servers.mapNotNull { it.registry }.toSet()

  /** The headers the form must ask for: the ones the registry did not fix a value for. */
  fun asked(e: ConnectorEntry, url: String? = null) =
    (e.remotes.firstOrNull { url == null || it.url == url } ?: e.remotes.firstOrNull())?.headers.orEmpty().filter { it.value == null }

  /** "" when the form can be sent, else the sentence. */
  fun validate(name: String, e: ConnectorEntry, values: Map<String, String>, existing: List<McpServer>, url: String? = null): String {
    val n = name.trim()
    if (n.isEmpty()) return "Give it a name."
    if (existing.any { it.name.equals(n, ignoreCase = true) }) return "A server with that name is already added."
    for (h in asked(e, url)) if (h.required && values[h.name].isNullOrBlank()) return "${h.name} is needed."
    return ""
  }

  fun defaultName(e: ConnectorEntry): String = e.id.substringAfter('/').replace(Regex("[^A-Za-z0-9_-]"), "-").ifBlank { "connector" }

  /** What the person is shown after a successful try. */
  fun tried(p: ConnectorPreview): String =
    "It works. It offers these tools: ${if (p.tools.isEmpty()) "none" else p.tools.joinToString(", ")}. ${p.notice}"
}

/** Browse connectors (remote ones; the "Runs on this computer" list is for the desktop and is explained here). */
@Composable
fun BrowseConnectorsSheet(servers: List<McpServer>, onDismiss: () -> Unit, onAdded: () -> Unit) {
  val container = LocalAppContainer.current
  val scope = rememberCoroutineScope()
  var q by remember { mutableStateOf("") }
  var applied by remember { mutableStateOf("") }
  var category by remember { mutableStateOf("all") }
  var next by remember { mutableStateOf<String?>(null) }
  var note by remember { mutableStateOf("Loading…") }
  var picked by remember { mutableStateOf<ConnectorEntry?>(null) }
  var added by remember { mutableStateOf(ConnectorsLogic.addedIds(servers)) }
  val items = remember { mutableStateListOf<ConnectorEntry>() }
  var gen by remember { mutableStateOf(0) }

  fun load(more: Boolean) {
    val mine = if (more) gen else ++gen
    if (!more) { items.clear(); next = null }
    note = "Loading…"
    scope.launch {
      try {
        val r = container.api.connectors(applied, if (more) next else null, category)
        if (mine != gen) return@launch
        if (r.restricted) { note = r.message.ifBlank { ConnectorsLogic.RESTRICTED }; return@launch }
        items.addAll((r.featured + r.entries).filter { e -> items.none { it.id == e.id } })
        next = r.next
        note = if (items.isEmpty()) (r.error ?: "Nothing found.") else (r.error ?: "")
      } catch (e: ApiException) { if (mine == gen) note = e.text }
    }
  }
  LaunchedEffect(category) { load(false) }

  ZoBottomSheet("Browse connectors", onDismiss, footer = { SecondaryButton("Close", onDismiss) }) {
    Column(Modifier.heightIn(max = 560.dp).verticalScroll(rememberScrollState())) {
      Text(ConnectorsLogic.INTRO, style = ZoTheme.type.fieldHint, color = ZoTheme.colors.dimmer)
      ZoTextField(
        q, { q = it }, placeholder = "Search connectors", modifier = Modifier.fillMaxWidth().padding(top = 8.dp),
        keyboardOptions = androidx.compose.foundation.text.KeyboardOptions(imeAction = ImeAction.Search),
        keyboardActions = KeyboardActions(onSearch = { applied = q.trim(); load(false) }),
      )
      Row(Modifier.horizontalScroll(rememberScrollState()).padding(vertical = 8.dp)) {
        for ((k, label) in ConnectorsLogic.CATEGORIES) {
          PillButton(label, { category = k }, accent = category == k)
          Spacer(Modifier.size(6.dp))
        }
      }
      if (note.isNotEmpty()) Text(note, style = ZoTheme.type.fieldHint, color = ZoTheme.colors.dimmer, modifier = Modifier.padding(bottom = 6.dp))
      for (e in items) {
        val isAdded = e.id in added
        ActionRow(
          e.title, "${e.publisher} · ${e.trust}\n${e.description}",
          actions = { if (isAdded) PillButton("Added", {}, enabled = false) else PrimaryButton("Add", { picked = e }) },
        )
      }
      if (next != null) PillButton("Load more", { load(true) }, modifier = Modifier.padding(top = 8.dp))
      Text(ConnectorsLogic.LOCAL_NOTE, style = ZoTheme.type.fieldHint, color = ZoTheme.colors.dimmer, modifier = Modifier.padding(top = 12.dp))
    }
  }
  picked?.let { e ->
    AddConnectorSheet(e, servers, onDismiss = { picked = null }) { added = added + e.id; onAdded() }
  }
}

/** The two steps: try it (keys go to the server, which keeps them in its key store), read the tools, then save. */
@Composable
fun AddConnectorSheet(e: ConnectorEntry, existing: List<McpServer>, onDismiss: () -> Unit, onAdded: () -> Unit) {
  val container = LocalAppContainer.current
  val scope = rememberCoroutineScope()
  var name by remember { mutableStateOf(ConnectorsLogic.defaultName(e)) }
  var url by remember { mutableStateOf(e.remotes.firstOrNull()?.url) }
  val values = remember { mutableStateOf(mapOf<String, String>()) }
  var busy by remember { mutableStateOf(false) }
  var tried by remember { mutableStateOf<ConnectorPreview?>(null) }
  var error by remember { mutableStateOf<String?>(null) }
  val cancel = {
    tried?.let { p -> scope.launch { runCatching { container.api.connectorCancel(p.name) } } }
    onDismiss()
  }
  ZoBottomSheet(
    "Add ${e.title}", cancel,
    footer = {
      SecondaryButton("Cancel", cancel)
      PrimaryButton(if (tried == null) "Try it" else "Add connector", {
        val t = tried
        if (t != null) {
          busy = true; error = null
          scope.launch { try { container.api.connectorAdd(t.name); tried = null; onAdded(); onDismiss() } catch (x: ApiException) { error = x.text } finally { busy = false } }
        } else {
          val bad = ConnectorsLogic.validate(name, e, values.value, existing, url)
          if (bad.isNotEmpty()) { error = bad } else {
            busy = true; error = null
            scope.launch { try { tried = container.api.connectorPreview(e.id, name.trim(), url, values.value.filterValues { it.isNotBlank() }) } catch (x: ApiException) { error = x.text } finally { busy = false } }
          }
        }
      }, loading = busy)
    },
  ) {
    Column(Modifier.heightIn(max = 480.dp).verticalScroll(rememberScrollState())) {
      Text("${e.publisher} · ${e.trust}", style = ZoTheme.type.cardMeta, color = ZoTheme.colors.dim)
      ZoTextField(name, { name = it }, label = "Name", enabled = tried == null, modifier = Modifier.fillMaxWidth().padding(top = 8.dp))
      if (e.remotes.size > 1) {
        for (r in e.remotes) PillButton(r.url, { url = r.url }, accent = url == r.url, enabled = tried == null)
      } else Text("Address: ${e.remotes.firstOrNull()?.url.orEmpty()}", style = ZoTheme.type.fieldHint, color = ZoTheme.colors.dimmer, modifier = Modifier.padding(top = 6.dp))
      for (h in ConnectorsLogic.asked(e, url)) {
        ZoTextField(
          values.value[h.name].orEmpty(), { v -> values.value = values.value + (h.name to v) },
          label = "Header ${h.name}${if (h.required) " (needed)" else ""}", hint = h.description.ifBlank { null }, password = h.secret, enabled = tried == null,
          modifier = Modifier.fillMaxWidth().padding(top = 8.dp),
        )
      }
      Text(tried?.let { ConnectorsLogic.tried(it) } ?: if (busy) "Trying it…" else "It is tried first. The tools it offers are listed before it is saved. If it needs a sign-in in a browser, add it in the desktop app.", style = ZoTheme.type.fieldHint, color = ZoTheme.colors.dimmer, modifier = Modifier.padding(top = 8.dp))
      error?.let { ErrorLine(it, Modifier.padding(top = 8.dp)) }
    }
  }
}
