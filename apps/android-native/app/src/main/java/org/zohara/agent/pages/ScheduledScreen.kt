package org.zohara.agent.pages

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
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
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.emptyFlow
import kotlinx.coroutines.launch
import org.zohara.agent.core.design.EmptyBox
import org.zohara.agent.core.design.ErrorLine
import org.zohara.agent.core.design.PillButton
import org.zohara.agent.core.design.PrimaryButton
import org.zohara.agent.core.design.SecondaryButton
import org.zohara.agent.core.design.ZoBottomSheet
import org.zohara.agent.core.design.ZoIconButton
import org.zohara.agent.core.design.ZoIcons
import org.zohara.agent.core.design.ZoSwitch
import org.zohara.agent.core.design.ZoTextField
import org.zohara.agent.core.design.ZoTheme
import org.zohara.agent.core.net.ScheduleRun
import org.zohara.agent.core.net.ScheduledResp
import org.zohara.agent.core.net.ScheduledTask
import org.zohara.agent.core.net.SendEvent

/**
 * What the Scheduled page can do with a task besides listing it (docs/SCHEDULED.md); the defaults do nothing, so a screen test can leave out
 * what it does not look at. [onTest] streams the Test of a saved task (by id) or of the form (a draft that is not saved).
 */
class ScheduleActions(
  val onEnable: (ScheduledTask, Boolean) -> Unit = { _, _ -> },
  val onRunNow: (ScheduledTask) -> Unit = {},
  val onHistory: suspend (ScheduledTask) -> Result<List<ScheduleRun>> = { Result.success(emptyList()) },
  val onPreview: suspend (TaskForm) -> Result<String> = { Result.success("") },
  val onTest: (String?, TaskForm?) -> Flow<SendEvent> = { _, _ -> emptyFlow() },
  /** an approval asked during a Test: (stream, approval id, yes) */
  val onApprove: suspend (Long, Long, Boolean) -> Unit = { _, _, _ -> },
  val onStopTest: suspend (Long) -> Unit = {},
  val onOpenChat: (String) -> Unit = {},
)

/**
 * Scheduled tasks. Parity rows: SCHED-01..04, 08, 09 and Scheduled v2: on/off, Run now, Test, History, last result, kinds, where it runs, the
 * next-runs preview. Results of a scheduled run are normal chats named "Scheduled: <title>" (a Test: "Test: <title>"); they appear in Recents (SCHED-08).
 */
@Composable
fun ScheduledScreen() {
  val hub = rememberPagesHub()
  val holder = rememberHolder("pages-scheduled") { ScheduledHolder(hub.api, it) }
  val state by holder.state.collectAsState()
  val info by holder.info.collectAsState()
  val toast = rememberToastLambda()
  val scope = rememberCoroutineScope()
  val nav = org.zohara.agent.nav.LocalNav.current
  val api = org.zohara.agent.LocalAppContainer.current.api
  RefreshWhileVisible(15_000) { holder.refresh() }
  ScheduledContent(
    state, remember(state) { PagesClock(System.currentTimeMillis()) }, onRefresh = holder::refresh,
    onCancel = { id -> scope.launch { holder.cancel(id)?.let(toast) } },
    onAdd = { f -> holder.add(f).fold({ toast(if (f.editId == null) "Task created" else "Saved"); null }, { it.sentence() }) },
    info = info,
    actions = ScheduleActions(
      onEnable = { t, on -> scope.launch { holder.enable(t.id, on)?.let(toast) } },
      onRunNow = { t -> scope.launch { toast(holder.runNow(t.id) ?: "Started. The result shows here and as a notification.") } },
      onHistory = { t -> holder.history(t.id) },
      onPreview = { f -> holder.preview(f) },
      onTest = { id, f -> holder.test(id, f) },
      onApprove = { stream, id, yes -> api.replyApprove(stream, id, yes) },
      onStopTest = { stream -> api.replyStop(stream) },
      onOpenChat = { nav.openChat(it) },
    ),
  )
}

@Composable
fun ScheduledContent(
  state: Loadable<List<ScheduledTask>>,
  clock: PagesClock,
  onRefresh: () -> Unit,
  onCancel: (String) -> Unit,
  onAdd: suspend (TaskForm) -> String?,
  startWithSheet: Boolean = false,
  startWithConfirm: ScheduledTask? = null,
  info: ScheduledResp = ScheduledResp(),
  actions: ScheduleActions = ScheduleActions(),
) {
  var creating by remember { mutableStateOf(startWithSheet) }
  var editing by remember { mutableStateOf<ScheduledTask?>(null) }
  var cancelling by remember { mutableStateOf(startWithConfirm) }
  var shown by remember { mutableStateOf<ScheduledTask?>(null) }
  var history by remember { mutableStateOf<ScheduledTask?>(null) }
  var test by remember { mutableStateOf<Pair<String?, TaskForm?>?>(null) }
  var testTitle by remember { mutableStateOf("") }
  val wide = org.zohara.agent.nav.isTwoPane()
  val startTest = { id: String?, f: TaskForm?, title: String -> test = id to f; testTitle = title }
  val list: @Composable () -> Unit = { ListPage(
    state = state,
    title = "Scheduled",
    onRefresh = onRefresh,
    headerAction = { PrimaryButton("New task", { creating = true }, icon = ZoIcons.Plus) },
    empty = { EmptyBox(PagesFormat.SCHEDULED_EMPTY_TITLE, subtitle = PagesFormat.SCHEDULED_EMPTY_TEXT, icon = ZoIcons.Alarm) },
  ) { t ->
    PageCard(
      ZoIcons.Alarm, PagesFormat.taskTitle(t), meta = PagesFormat.taskMeta(t, clock.now, clock.locale, clock.zone), metaLines = 3,
      extra = PagesFormat.taskLast(t, clock.now, clock.zone) ?: t.prompt.takeIf { it.isNotBlank() && it != PagesFormat.taskTitle(t) },
      onClick = { shown = t }, selected = wide && shown?.id == t.id,
      trailing = {
        Row(verticalAlignment = Alignment.CenterVertically) {
          ZoSwitch(t.enabled, { actions.onEnable(t, it) }, label = "On or off: ${PagesFormat.taskTitle(t)}")
          ZoIconButton(ZoIcons.Trash, "Cancel this task", { cancelling = t }, size = 36.dp, iconSize = 18.dp)
        }
      },
    )
  } }
  val detail: @Composable (ScheduledTask) -> Unit = { t ->
    TaskDetail(
      t, clock,
      onTest = { startTest(t.id, null, PagesFormat.taskTitle(t)) },
      onRunNow = { actions.onRunNow(t) },
      onHistory = { history = t },
      onEdit = { editing = t },
      onCancel = { cancelling = t },
      onOpenChat = actions.onOpenChat,
    )
  }
  if (wide) {
    // NAV-23: the task that was tapped is described beside the list
    org.zohara.agent.nav.TwoPane(list, detail = {
      shown?.let { t -> Column(Modifier.padding(16.dp)) { detail(state.value?.firstOrNull { it.id == t.id } ?: t) } } ?: PickAnItem("Pick a task to see it here")
    })
  } else {
    list()
    shown?.let { t ->
      val cur = state.value?.firstOrNull { it.id == t.id } ?: t
      ZoBottomSheet(PagesFormat.taskTitle(cur), onDismiss = { shown = null }) { detail(cur) }
    }
  }
  cancelling?.let { t ->
    ConfirmSheet(
      PagesFormat.CANCEL_TASK_TITLE, PagesFormat.CANCEL_TASK_TEXT, PagesFormat.CANCEL_TASK_BUTTON,
      onConfirm = { cancelling = null; shown = null; onCancel(t.id) }, onDismiss = { cancelling = null }, dismissLabel = "Keep it",
    )
  }
  if (creating) TaskSheet(null, info, onAdd, actions, onTest = { f -> startTest(null, f, f.title.ifBlank { f.prompt.take(40) }) }, onDismiss = { creating = false })
  editing?.let { t -> TaskSheet(t, info, onAdd, actions, onTest = { f -> startTest(null, f, f.title.ifBlank { f.prompt.take(40) }) }, onDismiss = { editing = null }) }
  history?.let { t -> HistorySheet(t, clock, actions, onDismiss = { history = null }) }
  test?.let { (id, f) -> TestSheet(testTitle, id, f, actions, onDismiss = { test = null }) }
}

/** One task, with everything that can be done with it. */
@OptIn(androidx.compose.foundation.layout.ExperimentalLayoutApi::class)
@Composable
private fun TaskDetail(
  t: ScheduledTask, clock: PagesClock,
  onTest: () -> Unit, onRunNow: () -> Unit, onHistory: () -> Unit, onEdit: () -> Unit, onCancel: () -> Unit, onOpenChat: (String) -> Unit,
) {
  Text(PagesFormat.taskTitle(t), style = ZoTheme.type.sheetTitle, color = ZoTheme.colors.text)
  Text(PagesFormat.taskMeta(t, clock.now, clock.locale, clock.zone), style = ZoTheme.type.cardMeta, color = ZoTheme.colors.dim, modifier = Modifier.padding(top = 4.dp, bottom = 10.dp))
  if (t.prompt.isNotBlank()) Text(t.prompt, style = ZoTheme.type.body, color = ZoTheme.colors.text)
  PagesFormat.taskLast(t, clock.now, clock.zone)?.let {
    Text(it, style = ZoTheme.type.cardMeta, color = if (t.lastStatus == "ok") ZoTheme.colors.dim else ZoTheme.colors.danger, modifier = Modifier.padding(top = 10.dp))
  }
  FlowRow(Modifier.padding(top = 14.dp), horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
    PillButton("Test", onTest)
    PillButton("Run now", onRunNow, enabled = !t.running)
    PillButton("History", onHistory)
    PillButton("Edit", onEdit)
    t.lastChat?.let { c -> PillButton("Last result", { onOpenChat(c) }) }
    SecondaryButton("Cancel this task", onCancel)
  }
}

/** SCHED-04 and Scheduled v2: what, when (plain words), repeat, name, kind and where it runs, with "next runs" before saving and Test. */
@OptIn(androidx.compose.foundation.layout.ExperimentalLayoutApi::class)
@Composable
fun TaskSheet(
  task: ScheduledTask?,
  info: ScheduledResp,
  onAdd: suspend (TaskForm) -> String?,
  actions: ScheduleActions,
  onTest: (TaskForm) -> Unit,
  onDismiss: () -> Unit,
) {
  val kinds = PagesFormat.KIND_CHOICES.filter { info.kinds.isEmpty() || it.second in info.kinds }
  var form by remember {
    mutableStateOf(
      if (task == null) TaskForm(runOn = info.runOn.firstOrNull()?.id.orEmpty())
      else TaskForm(
        prompt = task.prompt, whenText = "", repeatChoice = if (PagesFormat.REPEAT_CHOICES_V2.any { it.second == task.repeat }) task.repeat else "",
        ownInterval = if (PagesFormat.REPEAT_CHOICES_V2.any { it.second == task.repeat }) "" else task.repeat,
        title = task.title, kind = task.kind, runOn = task.runOn, editId = task.id,
      ),
    )
  }
  var busy by remember { mutableStateOf(false) }
  var error by remember { mutableStateOf<String?>(null) }
  var preview by remember { mutableStateOf<String?>(null) }
  var previewBad by remember { mutableStateOf(false) }
  val scope = rememberCoroutineScope()
  val c = ZoTheme.colors
  // the next runs, looked up a moment after the last key
  LaunchedEffect(form.whenText, form.repeatChoice, form.ownInterval) {
    if (form.whenText.isBlank()) { preview = null; return@LaunchedEffect }
    kotlinx.coroutines.delay(350)
    actions.onPreview(form).fold({ preview = it.ifBlank { null }; previewBad = false }, { preview = "I can't use that time yet: ${it.sentence()}"; previewBad = true })
  }
  ZoBottomSheet(
    title = if (task == null) "New scheduled task" else "Edit scheduled task",
    onDismiss = onDismiss,
    footer = {
      SecondaryButton("Cancel", onDismiss)
      SecondaryButton("Test", { onTest(form) }, enabled = form.prompt.isNotBlank())
      PrimaryButton(if (task == null) "Create task" else "Save", {
        busy = true; error = null
        scope.launch {
          val e = onAdd(form)
          busy = false
          if (e == null) onDismiss() else error = e
        }
      }, enabled = form.valid && !busy, loading = busy)
    },
  ) {
    ZoTextField(
      form.prompt, { form = form.copy(prompt = it) }, label = if (form.kind == "reminder") "What should the reminder say?" else "What should Zohara Agent do?",
      placeholder = "e.g. check my disk space and tell me if it is above 90%", singleLine = false, minLines = 2,
    )
    ZoTextField(
      form.whenText, { form = form.copy(whenText = it) }, label = "When",
      placeholder = if (task == null) "tomorrow 9am, in 2 hours, every weekday at 9" else "leave empty to keep the time",
      modifier = Modifier.padding(top = 12.dp),
    )
    preview?.let { Text(it, style = ZoTheme.type.cardMeta, color = if (previewBad) c.danger else c.dim, modifier = Modifier.padding(top = 6.dp)) }
    Text("Repeat", style = ZoTheme.type.fieldLabel, color = c.text, modifier = Modifier.padding(top = 12.dp, bottom = 6.dp))
    FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
      for ((label, value) in PagesFormat.REPEAT_CHOICES_V2) PillButton(label, { form = form.copy(repeatChoice = value) }, accent = form.repeatChoice == value && form.ownInterval.isBlank())
    }
    ZoTextField(
      form.ownInterval, { form = form.copy(ownInterval = it) }, placeholder = "or your own: every 3 days, every 90 minutes, every 2 months",
      hint = "Your own interval wins over the choice above. At least 5 minutes.", modifier = Modifier.padding(top = 10.dp),
    )
    ZoTextField(form.title, { form = form.copy(title = it) }, label = "Name", placeholder = "a short name for the list (optional)", modifier = Modifier.padding(top = 12.dp))
    if (kinds.size > 1) {
      Text("Kind", style = ZoTheme.type.fieldLabel, color = c.text, modifier = Modifier.padding(top = 12.dp, bottom = 6.dp))
      FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        for ((label, value) in kinds) PillButton(label, { form = form.copy(kind = value) }, accent = form.kind == value)
      }
      Text(
        when (form.kind) {
          "reminder" -> "A notification at the time. No model is used."
          "browser" -> "May use the browser and web tools. It asks before it changes, buys or sends anything."
          else -> "The agent does it with its tools."
        },
        style = ZoTheme.type.cardMeta, color = c.dim, modifier = Modifier.padding(top = 4.dp),
      )
    }
    if (info.runOn.size > 1) {
      Text("Run on", style = ZoTheme.type.fieldLabel, color = c.text, modifier = Modifier.padding(top = 12.dp, bottom = 6.dp))
      FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        for (p in info.runOn) PillButton(p.label, { form = form.copy(runOn = p.id) }, accent = form.runOn == p.id)
      }
      info.runOn.firstOrNull { it.id == form.runOn }?.note?.takeIf { it.isNotBlank() }?.let { Text(it, style = ZoTheme.type.cardMeta, color = c.dim, modifier = Modifier.padding(top = 4.dp)) }
    } else info.runOn.firstOrNull()?.note?.takeIf { it.isNotBlank() }?.let {
      Text("Runs on: ${info.runOn.first().label}. $it", style = ZoTheme.type.cardMeta, color = c.dim, modifier = Modifier.padding(top = 12.dp))
    }
    error?.let { ErrorLine(it, Modifier.padding(top = 10.dp)) }
  }
}

/** The last runs of a task, newest first, each with what it said; a run that left a chat can be opened. */
@Composable
private fun HistorySheet(t: ScheduledTask, clock: PagesClock, actions: ScheduleActions, onDismiss: () -> Unit) {
  var runs by remember { mutableStateOf<List<ScheduleRun>?>(null) }
  var error by remember { mutableStateOf<String?>(null) }
  LaunchedEffect(t.id) { actions.onHistory(t).fold({ runs = it }, { error = it.sentence() }) }
  ZoBottomSheet("History: ${PagesFormat.taskTitle(t)}", onDismiss = onDismiss, footer = { PrimaryButton("Close", onDismiss) }) {
    when {
      error != null -> ErrorLine(error!!)
      runs == null -> Text("Loading…", style = ZoTheme.type.cardMeta, color = ZoTheme.colors.dim)
      runs!!.isEmpty() -> Text("It has not run yet.", style = ZoTheme.type.cardMeta, color = ZoTheme.colors.dim)
      else -> for (r in runs!!) Column(Modifier.padding(bottom = 12.dp)) {
        Text(PagesFormat.runLine(r, clock.now, clock.locale, clock.zone), style = ZoTheme.type.cardName, color = if (r.status == "ok") ZoTheme.colors.text else ZoTheme.colors.danger)
        if (r.summary.isNotBlank()) Text(r.summary, style = ZoTheme.type.cardMeta, color = ZoTheme.colors.dim)
        r.chat?.let { c -> PillButton("Open chat", { onDismiss(); actions.onOpenChat(c) }, modifier = Modifier.padding(top = 6.dp)) }
      }
    }
  }
}

/**
 * Test: the task runs once now in test mode (a chat "Test: <title>"; its schedule, last run and history are not changed; every change still asks first).
 * The output shows as it comes, then a result card: success or failed, the time, the tools, what it said; a reminder shows its notification.
 */
@Composable
private fun TestSheet(title: String, id: String?, form: TaskForm?, actions: ScheduleActions, onDismiss: () -> Unit) {
  val lines = remember { mutableStateListOf<String>() }
  var answer by remember { mutableStateOf("") }
  var stream by remember { mutableStateOf(0L) }
  var finished by remember { mutableStateOf(false) }
  var result by remember { mutableStateOf<SendEvent.Result?>(null) }
  var notice by remember { mutableStateOf<SendEvent.Notify?>(null) }
  var failure by remember { mutableStateOf<String?>(null) }
  val asks = remember { mutableStateListOf<SendEvent.Approval>() }
  val scope = rememberCoroutineScope()
  LaunchedEffect(id, form) {
    try {
      actions.onTest(id, form).collect { e ->
        when (e) {
          is SendEvent.Stream -> stream = e.id
          is SendEvent.Test -> lines += "Test of a ${e.kind.ifBlank { "task" }}, on ${e.place.ifBlank { "this computer" }}"
          is SendEvent.Delta -> answer += e.text
          is SendEvent.Text -> if (answer.isEmpty()) answer = e.text
          is SendEvent.ToolStart -> { lines += "▸ " + e.summary.lines().take(2).joinToString(" ").ifBlank { e.name }; answer = "" }
          is SendEvent.ToolDone -> lines += (if (e.ok) "✓ " else "✗ ") + e.output.take(200)
          is SendEvent.Approval -> asks += e
          is SendEvent.Notify -> notice = e
          is SendEvent.Result -> { result = e; answer = "" }
          is SendEvent.Error -> failure = e.message
          is SendEvent.ConnectionLost -> failure = e.reason
          else -> {}
        }
      }
    } catch (e: kotlinx.coroutines.CancellationException) {
      throw e
    } catch (e: Exception) {
      failure = (e as? org.zohara.agent.core.net.ApiException)?.text ?: org.zohara.agent.core.net.ApiErrors.GENERIC
    }
    finished = true
  }
  ZoBottomSheet(
    "Test: $title", onDismiss = onDismiss,
    footer = {
      if (!finished) SecondaryButton("Stop", { scope.launch { actions.onStopTest(stream) } }, enabled = stream != 0L)
      PrimaryButton("Close", onDismiss)
    },
  ) {
    Text("It runs once now. Its schedule is not changed, and anything that changes something still asks first.", style = ZoTheme.type.cardMeta, color = ZoTheme.colors.dim)
    if (lines.isEmpty() && !finished) Text("Starting…", style = ZoTheme.type.cardMeta, color = ZoTheme.colors.dim, modifier = Modifier.padding(top = 8.dp))
    for (l in lines) Text(l, style = ZoTheme.type.cardMeta, color = ZoTheme.colors.dim, modifier = Modifier.padding(top = 4.dp))
    if (answer.isNotBlank()) Text(answer, style = ZoTheme.type.body, color = ZoTheme.colors.text, modifier = Modifier.padding(top = 8.dp))
    for (a in asks) {
      var decided by remember(a.id) { mutableStateOf<Boolean?>(null) }
      Column(Modifier.padding(top = 10.dp)) {
        Text("Zohara Agent wants to: ${a.summary.ifBlank { a.tool }}", style = ZoTheme.type.body, color = ZoTheme.colors.text)
        if (decided == null) Row(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.padding(top = 6.dp)) {
          PrimaryButton("Allow", { decided = true; scope.launch { actions.onApprove(stream, a.id, true) } })
          SecondaryButton("Deny", { decided = false; scope.launch { actions.onApprove(stream, a.id, false) } })
        } else Text(if (decided == true) "Allowed" else "Denied", style = ZoTheme.type.cardMeta, color = ZoTheme.colors.dim)
      }
    }
    notice?.takeIf { it.kind == "reminder" }?.let {
      Text("The notification:", style = ZoTheme.type.cardMeta, color = ZoTheme.colors.dim, modifier = Modifier.padding(top = 10.dp))
      Text(it.title, style = ZoTheme.type.cardName, color = ZoTheme.colors.text)
      Text(it.body, style = ZoTheme.type.cardMeta, color = ZoTheme.colors.dim)
    }
    result?.let { r ->
      val head = (if (r.ok) "Success" else if (r.status == "skipped") "Could not start" else "Failed") + " · " + "%.1f s".format(r.ms / 1000.0) + " · " +
        (if (r.tools.isEmpty()) "no tools used" else "tools: " + r.tools.joinToString(", "))
      Text(head, style = ZoTheme.type.cardName, color = if (r.ok) ZoTheme.colors.text else ZoTheme.colors.danger, modifier = Modifier.padding(top = 12.dp))
      if (r.summary.isNotBlank()) Text(r.summary, style = ZoTheme.type.body, color = ZoTheme.colors.text, modifier = Modifier.padding(top = 4.dp))
      r.chat?.let { c -> PillButton("Open the test chat", { onDismiss(); actions.onOpenChat(c) }, modifier = Modifier.padding(top = 8.dp)) }
    }
    failure?.let { ErrorLine(it, Modifier.padding(top = 10.dp)) }
  }
}
