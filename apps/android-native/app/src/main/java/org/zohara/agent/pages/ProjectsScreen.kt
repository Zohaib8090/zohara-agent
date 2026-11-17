package org.zohara.agent.pages

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch
import org.zohara.agent.core.design.EmptyBox
import org.zohara.agent.core.design.ErrorLine
import org.zohara.agent.core.design.PrimaryButton
import org.zohara.agent.core.design.SecondaryButton
import org.zohara.agent.core.design.ZoBottomSheet
import org.zohara.agent.core.design.ZoIcon
import org.zohara.agent.core.design.ZoIcons
import org.zohara.agent.core.design.ZoTextField
import org.zohara.agent.core.design.ZoTheme

/**
 * Projects page. Parity rows: PROJ-01, 02, 05, 12 (the workspace rows PROJ-06..11 need service ops that do not exist: docs/PAGES.md).
 * The list is the `projects` text parsed into cards; "New project" asks for a name and a goal. A restricted account gets the server's own refusal.
 */
@Composable
fun ProjectsScreen() {
  val hub = rememberPagesHub()
  val holder = rememberHolder("pages-projects") { ProjectsHolder(hub.api, it) }
  val state by holder.state.collectAsState()
  val toast = rememberToastLambda()
  RefreshWhileVisible(null) { holder.refresh() }
  ProjectsContent(
    state, onRefresh = holder::refresh, onCreate = { n, g -> holder.create(n, g).also { if (it == null) toast("Project created") } },
    onDelete = { id -> holder.delete(id).also { if (it == null) toast("Project deleted") } },
  )
}

@Composable
fun ProjectsContent(
  state: Loadable<List<PagesFormat.Project>>,
  onRefresh: () -> Unit,
  onCreate: suspend (name: String, goal: String) -> String?,
  startWithSheet: Boolean = false,
  startWithProject: PagesFormat.Project? = null,
  /** Deletes a project and the memory notes kept for it; returns an error sentence or null. Null = no Delete button. */
  onDelete: (suspend (id: String) -> String?)? = null,
) {
  var creating by remember { mutableStateOf(startWithSheet) }
  var deleting by remember { mutableStateOf<PagesFormat.Project?>(null) }
  val deleteScope = rememberCoroutineScope()
  val askDelete: ((PagesFormat.Project) -> Unit)? = if (onDelete == null) null else { p -> deleting = p }
  var shown by remember { mutableStateOf(startWithProject) }
  var query by remember { mutableStateOf("") }
  val all = state.value
  val filtered = all?.let { PagesFormat.filterProjects(it, query) }
  val wide = org.zohara.agent.nav.isTwoPane()
  val list: @Composable () -> Unit = { ListPage(
    state = if (filtered != null && query.isNotBlank()) state.copy(value = filtered) else state,
    title = "Projects",
    onRefresh = onRefresh,
    headerAction = { PrimaryButton("New project", { creating = true }, icon = ZoIcons.Plus) },
    beforeList = if (all.isNullOrEmpty()) null else ({
      item(key = "project-search") {
        ZoTextField(query, { query = it }, placeholder = "Search projects", trailing = { ZoIcon(ZoIcons.Search) })
      }
    }),
    empty = {
      if (!all.isNullOrEmpty()) EmptyBox("Nothing matches that.", icon = ZoIcons.Search)
      else EmptyBox(
        PagesFormat.PROJECTS_EMPTY_TITLE, subtitle = PagesFormat.PROJECTS_EMPTY_TEXT, icon = ZoIcons.Folder,
        action = { PrimaryButton("New project", { creating = true }) },
      )
    },
  ) { p -> PageCard(ZoIcons.Folder, p.name, meta = p.meta, onClick = { shown = p }, selected = wide && shown?.id == p.id) } }
  if (wide) {
    // NAV-23: the opened project is shown beside the list instead of in a sheet
    org.zohara.agent.nav.TwoPane(list, detail = { shown?.let { p -> ProjectDetail(p, Modifier.padding(16.dp), onDelete = askDelete?.let { f -> { f(p) } }) } ?: PickAnItem("Pick a project to see it here") })
  } else {
    list()
    shown?.let { p -> ProjectSheet(p, onDelete = askDelete?.let { f -> { f(p) } }) { shown = null } }
  }
  deleting?.let { p ->
    org.zohara.agent.settings.ConfirmSheet(
      "Delete this project?", ProjectsLogic.deleteText(p.name), "Delete",
      onConfirm = { deleteScope.launch { onDelete?.invoke(p.id); if (shown?.id == p.id) shown = null } },
      onDismiss = { deleting = null },
    )
  }
  if (creating) NewProjectSheet(onCreate = onCreate, onDismiss = { creating = false })
}

/** PROJ-05: Name (required) and Goal. */
@Composable
fun NewProjectSheet(onCreate: suspend (String, String) -> String?, onDismiss: () -> Unit) {
  var name by remember { mutableStateOf("") }
  var goal by remember { mutableStateOf("") }
  var busy by remember { mutableStateOf(false) }
  var error by remember { mutableStateOf<String?>(null) }
  val scope = rememberCoroutineScope()
  ZoBottomSheet(
    title = "New project",
    onDismiss = onDismiss,
    footer = {
      SecondaryButton("Cancel", onDismiss)
      PrimaryButton("Create project", {
        busy = true; error = null
        scope.launch {
          val e = onCreate(name, goal)
          busy = false
          if (e == null) onDismiss() else error = e
        }
      }, enabled = name.isNotBlank() && !busy, loading = busy)
    },
  ) {
    ZoTextField(name, { name = it }, label = "Name", placeholder = "Name your project")
    ZoTextField(goal, { goal = it }, label = "Goal", placeholder = "What is it for?", modifier = Modifier.padding(top = 12.dp))
    error?.let { ErrorLine(it, Modifier.padding(top = 10.dp)) }
  }
}

/**
 * Opening a project (PROJ-06): the desktop's workspace keeps instructions, context files, memory and per-project tasks in the desktop's own store;
 * the service has no op for them, so on the phone a card opens what the list knows, and says where the rest is.
 */
@Composable
fun ProjectSheet(p: PagesFormat.Project, onDelete: (() -> Unit)? = null, onDismiss: () -> Unit) {
  ZoBottomSheet(
    title = p.name, onDismiss = onDismiss,
    footer = {
      if (onDelete != null) org.zohara.agent.core.design.DangerButton("Delete project", onDelete, outlined = true)
      SecondaryButton("Close", onDismiss)
    },
  ) { ProjectDetailBody(p) }
}

/** The words of the pages about deleting a project. */
object ProjectsLogic {
  /** The notes the memory agent kept for a project go with it; the chats stay. */
  fun deleteText(name: String) = "\"$name\" is deleted with its instructions and the memory notes Zohara Agent kept for it. Chats you had in it stay. This cannot be undone."
}

/** The opened project in the right pane of the two-pane layout (NAV-23). */
@Composable
fun ProjectDetail(p: PagesFormat.Project, modifier: Modifier = Modifier, onDelete: (() -> Unit)? = null) {
  Column(modifier) {
    Text(p.name, style = ZoTheme.type.sheetTitle, color = ZoTheme.colors.text, modifier = Modifier.padding(bottom = 10.dp))
    ProjectDetailBody(p)
    if (onDelete != null) org.zohara.agent.core.design.DangerButton("Delete project", onDelete, Modifier.padding(top = 14.dp), outlined = true)
  }
}

@Composable
fun ProjectDetailBody(p: PagesFormat.Project) {
  val c = ZoTheme.colors
  run {
    Column {
      Text(
        p.goal.ifBlank { "Zohara Agent remembers the same knowledge every time you talk to it in this project." },
        style = ZoTheme.type.body, color = c.text,
      )
      if (p.memoryNotes.isNotBlank()) Text(p.memoryNotes, style = ZoTheme.type.cardMeta, color = c.dim, modifier = Modifier.padding(top = 8.dp))
      Text(
        "Instructions, context files, project memory and chats inside a project are kept in the desktop app.",
        style = ZoTheme.type.cardMeta, color = c.dimmer, modifier = Modifier.padding(top = 12.dp),
      )
    }
  }
}
