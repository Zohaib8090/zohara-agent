package org.zohara.agent.settings

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.ui.Alignment
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.Role
import org.zohara.agent.core.design.DangerButton
import org.zohara.agent.core.design.ZoSwitch
import org.zohara.agent.core.net.AppCheck
import org.zohara.agent.core.net.AppsPlanResp
import org.zohara.agent.core.net.MachineNew
import org.zohara.agent.core.net.MachineStatus
import org.zohara.agent.core.net.ZoApi
import org.zohara.agent.nav.LocalNav
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.key
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
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
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch
import org.zohara.agent.LocalAppContainer
import org.zohara.agent.core.design.ErrorLine
import org.zohara.agent.core.design.PillButton
import org.zohara.agent.core.design.PrimaryButton
import org.zohara.agent.core.design.SecondaryButton
import org.zohara.agent.core.design.SectionHeader
import org.zohara.agent.core.design.SettingsCard
import org.zohara.agent.core.design.ZoBottomSheet
import org.zohara.agent.core.design.ZoIconButton
import org.zohara.agent.core.design.ZoIcons
import org.zohara.agent.core.design.ZoTextField
import org.zohara.agent.core.design.ZoTheme
import org.zohara.agent.core.net.ApiException
import org.zohara.agent.core.net.Machine

object MachineLogic {
  private val NAME = Regex("^[A-Za-z0-9._-]{1,40}$")

  /** "user@host:port · key saved · note" (SET-MAC-01). */
  fun describe(m: Machine): String =
    (if (m.user.isNotBlank()) m.user + "@" else "") + m.host + ":" + m.port + (if (m.hasKey) " · key saved" else " · no key") + (if (m.note.isNotBlank()) " · " + m.note else "")

  /** SET-MAC-02: "" when fine, else the sentence. The key is required (the server refuses a machine without it). */
  fun validate(name: String, host: String, user: String, port: String, key: String): String = when {
    !NAME.matches(name.trim()) -> "The name can have letters, digits, dots, underscores and dashes."
    host.isBlank() -> "Type the host: an address or a name."
    user.isBlank() -> "Type the user that logs in."
    (port.trim().toIntOrNull() ?: 0) !in 1..65535 -> "The port is a number from 1 to 65535."
    key.isBlank() -> "Paste the private key that this machine accepts."
    else -> ""
  }
}

/** The dot next to a machine: grey (not checked yet), green (the runner works), amber (reachable, no runner), red (not reachable). */
enum class MachineDot { Wait, Ok, Warn, Bad }

/** The words of the Machines screens, the same as the web page and the desktop window (docs/RUNNER.md). Pure, so they are tested without a screen. */
object MachineText {
  /** The service asks for a named confirmation by answering with this prefix and the sentence. */
  const val CONFIRM_PREFIX = "Confirm exactly: "

  /** The sentence a person must agree to, taken from the service's answer, or null when the error is something else. */
  fun sentenceOf(error: String): String? = if (error.startsWith(CONFIRM_PREFIX)) error.removePrefix(CONFIRM_PREFIX).trim().ifEmpty { null } else null

  fun dot(s: MachineStatus?): MachineDot = when {
    s == null -> MachineDot.Wait
    !s.reachable -> MachineDot.Bad
    s.installed -> MachineDot.Ok
    else -> MachineDot.Warn
  }

  /** "Reachable · runner 0.1.0 · 2 tasks · last run Nov 17 (ok)": what a person wants to know at a glance. */
  fun state(s: MachineStatus?, whenText: (Long) -> String = { formatWhen(it) }): String {
    if (s == null) return "Checking…"
    if (!s.reachable) return if (s.error.isNullOrBlank()) "Not reachable" else "Not reachable: " + s.error.take(140)
    if (!s.installed) return "Reachable · no runner"
    val r = s.runner
    val n = r?.enabledTasks ?: 0
    return buildString {
      append("Reachable · runner ").append(r?.version?.ifBlank { "?" } ?: "?")
      if (r?.timerActive == false) append(" · timer off")
      append(" · ").append(n).append(if (n == 1) " task" else " tasks")
      r?.lastRun?.let { append(" · last run ").append(whenText(it)); r.lastStatus?.let { st -> append(" ($st)") } }
    }
  }

  /** What "Check" found: the apps whose program is missing on the machine, each with what to do; or the good news. */
  fun missing(list: List<AppCheck>): String {
    val bad = list.filter { !it.present }
    return if (bad.isEmpty()) "Everything the apps need is on the machine." else bad.joinToString(" ") { "${it.name}: ${it.hint}" }
  }

  /** What the Connected apps row says about the copy that was made. */
  fun copied(s: MachineStatus?): String {
    val list = s?.appsCopied?.servers.orEmpty()
    return if (list.isEmpty()) "Copy your connected apps (MCP servers) to the machine so its tasks can use them without signing in again."
    else "Copied: " + list.joinToString(", ") { if (it.credentials) "${it.name} (with credentials)" else it.name }
  }

  /** The wizard's first step: "" when fine, else the sentence. (The service checks again.) */
  fun validateNew(name: String, host: String, user: String, port: String): String = MachineLogic.validate(name, host, user, port, "key")

  fun runnerText(installed: Boolean) =
    if (installed) "Installed. It runs this machine's own task list from a systemd timer, also when your computer is off. Its results come back here."
    else "A small program (zohara-agent-runner) that is copied to the machine over SSH, with its checksum checked. It lets scheduled tasks run there while your computer is off."

  fun taskDone(byRunner: Boolean, name: String) =
    if (byRunner) "Sent to the runner on $name. The answer comes back to you." else "Started from here over SSH. It asks you first, as always."
}

@Composable
private fun StatusDot(d: MachineDot) {
  val color = when (d) {
    MachineDot.Wait -> ZoTheme.colors.dimmer
    MachineDot.Ok -> Color(0xFF3FB57A)
    MachineDot.Warn -> Color(0xFFD9A441)
    MachineDot.Bad -> ZoTheme.colors.danger
  }
  Box(Modifier.size(9.dp).clip(CircleShape).background(color))
}

@Composable
private fun StateLine(s: MachineStatus?) {
  Row(Modifier.padding(top = 4.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
    StatusDot(MachineText.dot(s))
    Text(MachineText.state(s), style = ZoTheme.type.cardMeta, color = ZoTheme.colors.dim)
  }
}

/** Machines (SSH), administrators only. Parity rows: SET-MAC-01..04, 07, and the runner screens of docs/RUNNER.md. */
@Composable
fun SettingsMachinesScreen() = AdminOnly { SettingsPage("Machines") { MachinesPanel() } }

/** The list of machines with their status, the add wizard and the per-machine window. On the Machines page and in the popup of the chat screen. */
@Composable
fun MachinesPanel() {
  val container = LocalAppContainer.current
  val nav = LocalNav.current
  val doer = rememberDoer()
  val scope = rememberCoroutineScope()
  val loader = rememberLoader { container.api.machines() }
  val load by loader.state.collectAsState()
  var status by remember { mutableStateOf<Map<String, MachineStatus>?>(null) }
  var wizard by remember { mutableStateOf(false) }
  var ownKey by remember { mutableStateOf(false) }
  var manage by remember { mutableStateOf<String?>(null) }
  var testing by remember { mutableStateOf<String?>(null) }
  val refreshStatus: () -> Unit = { scope.launch { status = try { container.api.machineStatus().associateBy { it.name } } catch (e: ApiException) { emptyMap() } } }
  val reload: () -> Unit = { loader.load(); refreshStatus() }
  LaunchedEffect(load is Load.Ready) { if (load is Load.Ready) refreshStatus() }
  LoadBox(load, { loader.load() }) { machines ->
    MachinesContent(
      machines, testing,
      onTest = { m -> testing = m.name; doer.run(onFail = { testing = null; doer.say(it) }) { doer.say(container.api.machineTest(m.name)); testing = null } },
      onRemove = { m -> manage = m.name }, onAdd = { wizard = true },
      status = status ?: emptyMap(), checked = status != null, onTerminal = { nav.terminal() }, onManage = { manage = it.name }, onOwnKey = { ownKey = true },
    )
  }
  if (wizard) MachineWizardSheet(container.api, onDismiss = { wizard = false; reload() })
  if (ownKey) AddMachineSheet(onDismiss = { ownKey = false }) { n, h, u, p, note, k -> container.api.machineAdd(n, h, u, p, note, k); reload() }
  manage?.let { name -> MachineManageSheet(container.api, name, onDismiss = { manage = null; reload() }) }
}

/** The popup that opens from the Machines pill next to the model pill (VM-MIGRATION-PLAN section 6). */
@Composable
fun MachinesSheet(onDismiss: () -> Unit) {
  ZoBottomSheet("Machines", onDismiss) {
    Column(Modifier.heightIn(max = 560.dp).verticalScroll(rememberScrollState())) { MachinesPanel() }
  }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
fun MachinesContent(
  machines: List<Machine>, testing: String?, onTest: (Machine) -> Unit, onRemove: (Machine) -> Unit, onAdd: () -> Unit,
  status: Map<String, MachineStatus> = emptyMap(), checked: Boolean = false, onTerminal: ((Machine) -> Unit)? = null,
  onManage: ((Machine) -> Unit)? = null, onOwnKey: (() -> Unit)? = null,
) {
  SectionHeader("Machines")
  SettingsCard {
    for (m in machines) {
      val s = if (checked) status[m.name] ?: MachineStatus(m.name, reachable = false, error = "No answer.") else null
      ActionRow(
        m.name, MachineLogic.describe(m),
        extra = { StateLine(s) },
        actions = {
          PillButton(if (testing == m.name) "Testing…" else "Test", { onTest(m) }, enabled = testing != m.name)
          onTerminal?.let { PillButton("Terminal", { it(m) }) }
          if (onManage != null) PillButton("Manage…", { onManage(m) })
          else ZoIconButton(ZoIcons.Trash, "Remove this machine", { onRemove(m) }, tint = ZoTheme.colors.danger, size = 36.dp)
        },
      )
    }
    ActionRow(
      "Add a machine", "Zohara Agent connects with SSH to run commands there. It makes a new key for the machine and shows you the one line to put on it. The private key is stored on the server and never shown.",
      divider = onOwnKey != null, actions = { PrimaryButton("Add machine…", onAdd) },
    )
    if (onOwnKey != null) ActionRow("Already have a key?", "Add a machine with a private key you made yourself (no passphrase).", divider = false, actions = { PillButton("Use my own key…", onOwnKey) })
  }
}

/** The add wizard: where it is -> the new key and the one line to put on the machine (or a one-time password, never kept) -> Test. */
@Composable
fun MachineWizardSheet(api: ZoApi, onDismiss: () -> Unit) {
  var name by remember { mutableStateOf("") }
  var host by remember { mutableStateOf("") }
  var user by remember { mutableStateOf("") }
  var port by remember { mutableStateOf("22") }
  var note by remember { mutableStateOf("") }
  var made by remember { mutableStateOf<MachineNew?>(null) }
  var password by remember { mutableStateOf("") }
  var busy by remember { mutableStateOf(false) }
  var error by remember { mutableStateOf<String?>(null) }
  var ok by remember { mutableStateOf<String?>(null) }
  val scope = rememberCoroutineScope()
  val clipboard = LocalClipboardManager.current
  val m = made
  ZoBottomSheet(
    "Add a machine", onDismiss,
    footer = {
      if (m == null) {
        SecondaryButton("Cancel", onDismiss)
        PrimaryButton("Next", {
          val bad = MachineText.validateNew(name, host, user, port)
          if (bad.isNotEmpty()) error = bad else {
            busy = true; error = null
            scope.launch { try { made = api.machineNew(name.trim(), host.trim(), user.trim(), port.trim().toInt(), note.trim()) } catch (e: ApiException) { error = e.text } finally { busy = false } }
          }
        }, loading = busy)
      } else {
        SecondaryButton("Done", onDismiss)
        PrimaryButton("Test connection", {
          busy = true; error = null; ok = null
          scope.launch { try { ok = api.machineTest(m.name) } catch (e: ApiException) { error = e.text } finally { busy = false } }
        }, loading = busy)
      }
    },
  ) {
    Column(Modifier.heightIn(max = 520.dp).verticalScroll(rememberScrollState())) {
      if (m == null) {
        Text("Zohara Agent makes a new key just for this machine. You put its public half on the machine, once.", style = ZoTheme.type.fieldHint, color = ZoTheme.colors.dimmer, modifier = Modifier.padding(bottom = 8.dp))
        ZoTextField(name, { name = it }, label = "Name", placeholder = "home-server", modifier = Modifier.fillMaxWidth())
        Spacer(Modifier.size(10.dp))
        ZoTextField(host, { host = it }, label = "Host", placeholder = "192.168.1.20 or example.com", keyboardType = androidx.compose.ui.text.input.KeyboardType.Uri, modifier = Modifier.fillMaxWidth())
        Spacer(Modifier.size(10.dp))
        ZoTextField(user, { user = it }, label = "User", modifier = Modifier.fillMaxWidth())
        Spacer(Modifier.size(10.dp))
        ZoTextField(port, { port = it.filter(Char::isDigit).take(5) }, label = "Port", keyboardType = androidx.compose.ui.text.input.KeyboardType.Number, modifier = Modifier.fillMaxWidth())
        Spacer(Modifier.size(10.dp))
        ZoTextField(note, { note = it }, label = "Note (optional)", modifier = Modifier.fillMaxWidth())
      } else {
        Text("Put this key on the machine. Run this one line there (it appends the key to ~/.ssh/authorized_keys):", style = ZoTheme.type.cardMeta, color = ZoTheme.colors.dim)
        Text(m.authorizeCommand, style = ZoTheme.type.code, color = ZoTheme.colors.codeFg, modifier = Modifier.padding(top = 8.dp).fillMaxWidth().clip(ZoTheme.shapes.code).background(ZoTheme.colors.input).padding(10.dp))
        PillButton("Copy the line", { clipboard.setText(androidx.compose.ui.text.AnnotatedString(m.authorizeCommand)); ok = "Copied." }, modifier = Modifier.padding(top = 8.dp))
        Text("Or, if the machine accepts a password, give it once. It is used one time to put the key there and is never stored.", style = ZoTheme.type.cardMeta, color = ZoTheme.colors.dim, modifier = Modifier.padding(top = 14.dp, bottom = 8.dp))
        ZoTextField(password, { password = it }, placeholder = "One-time password", password = true, modifier = Modifier.fillMaxWidth())
        PillButton("Use it once", {
          val p = password; password = ""
          if (p.isNotEmpty()) { busy = true; error = null; scope.launch { try { ok = api.machineAuthorize(m.name, p) } catch (e: ApiException) { error = e.text } finally { busy = false } } }
        }, enabled = !busy && password.isNotEmpty(), modifier = Modifier.padding(top = 8.dp))
        ok?.let { Text(it, style = ZoTheme.type.cardMeta, color = ZoTheme.colors.accent, modifier = Modifier.padding(top = 8.dp)) }
      }
      error?.let { ErrorLine(it, Modifier.padding(top = 8.dp)) }
    }
  }
}

/**
 * The sentence a person agrees to before credentials are copied, shown word for word with a switch. [run] gets the sentence back
 * unchanged; the service refuses any other text.
 */
@Composable
fun NamedConfirmSheet(title: String, sentence: String, okLabel: String, onDismiss: () -> Unit, run: suspend (String) -> String, onDone: (String) -> Unit) {
  var sure by remember { mutableStateOf(false) }
  var busy by remember { mutableStateOf(false) }
  var error by remember { mutableStateOf<String?>(null) }
  val scope = rememberCoroutineScope()
  ZoBottomSheet(
    title, onDismiss,
    footer = {
      SecondaryButton("Cancel", onDismiss)
      PrimaryButton(okLabel, {
        busy = true; error = null
        scope.launch { try { val m = run(sentence); onDismiss(); onDone(m) } catch (e: ApiException) { error = e.text } finally { busy = false } }
      }, enabled = sure && !busy, loading = busy)
    },
  ) {
    Text("Your credentials are copied over SSH, straight from the server to the machine, as files only you can read. You can revoke them on the machine afterwards.", style = ZoTheme.type.cardMeta, color = ZoTheme.colors.dim)
    Row(Modifier.padding(vertical = 12.dp).clickable(role = Role.Checkbox) { sure = !sure }, verticalAlignment = Alignment.Top, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
      ZoSwitch(sure, null)
      Text(sentence, style = ZoTheme.type.body, color = ZoTheme.colors.text, modifier = Modifier.weight(1f))
    }
    error?.let { ErrorLine(it) }
  }
}

/** The window of one machine: runner, model, connected apps, "run a task here", remove. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun MachineManageSheet(api: ZoApi, name: String, onDismiss: () -> Unit) {
  val scope = rememberCoroutineScope()
  var s by remember { mutableStateOf<MachineStatus?>(null) }
  var message by remember { mutableStateOf("") }
  var working by remember { mutableStateOf<String?>(null) }
  var confirm by remember { mutableStateOf<Triple<String, String, suspend (String) -> String>?>(null) }
  var appsPlan by remember { mutableStateOf<AppsPlanResp?>(null) }
  var removeAsk by remember { mutableStateOf(false) }
  var task by remember { mutableStateOf("") }
  val reload: () -> Unit = { scope.launch { s = try { api.machineStatus(name).firstOrNull() ?: s } catch (e: ApiException) { s } } }
  LaunchedEffect(name) { reload() }
  /** runs one operation, shows what it said, asks the machine again */
  val doIt: (String, suspend () -> String) -> Unit = { label, block ->
    working = label; message = ""
    scope.launch { try { message = block() } catch (e: ApiException) { message = e.text } finally { working = null }; reload() }
  }
  val installed = s?.installed == true
  ZoBottomSheet(name, onDismiss, footer = { SecondaryButton("Close", onDismiss) }) {
    Column(Modifier.heightIn(max = 560.dp).verticalScroll(rememberScrollState())) {
      SettingsCard {
        ActionRow("Status", extra = { StateLine(s) })
        ActionRow(
          "Runner", MachineText.runnerText(installed),
          actions = {
            PrimaryButton(if (working == "install") "Installing…" else if (installed) "Update" else "Install runner", { doIt("install") { api.runnerInstall(name).lastOrNull() ?: "Done." } }, enabled = working == null)
            if (installed) DangerButton("Remove runner", { doIt("uninstall") { api.runnerUninstall(name); "The runner is removed." } }, enabled = working == null, outlined = true)
          },
        )
        if (installed) {
          ActionRow("Let tasks run commands there", "Off: a task that needs to run a command is refused (nobody is there to ask).", actions = {
            ZoSwitch(s?.commands == true, { on -> scope.launch { try { api.runnerAllow(name, on, s?.files == true); s = s?.copy(commands = on) } catch (e: ApiException) { message = e.text } } })
          })
          ActionRow("Let tasks write files there", "Off: writing files is refused.", actions = {
            ZoSwitch(s?.files == true, { on -> scope.launch { try { api.runnerAllow(name, s?.commands == true, on); s = s?.copy(files = on) } catch (e: ApiException) { message = e.text } } })
          })
          val r = s?.runner
          ActionRow(
            "Model on this machine",
            if (r?.hasModel == true) r.model + (if (r.hasKey) " · key copied" else "") else "Tasks there need a model. Copying sends your API key for it over SSH, after you confirm.",
            actions = {
              PillButton("Copy model key…", {
                scope.launch {
                  try {
                    val p = api.machineModelPlan(name)
                    if (!p.can) message = p.why.ifBlank { "That cannot be copied." } else confirm = Triple("Copy the model key to $name", p.confirm) { c -> api.machineModelCopy(name, c) }
                  } catch (e: ApiException) { message = e.text }
                }
              })
              if (r?.hasModel == true) PillButton("Remove", { doIt("model") { api.machineModelClear(name); "Removed from the machine." } })
            },
          )
        }
        ActionRow("Connected apps", MachineText.copied(s), actions = {
          PillButton("Copy connected apps…", { scope.launch { try { appsPlan = api.machineAppsPlan(name) } catch (e: ApiException) { message = e.text } } })
          if (s?.appsCopied?.servers.orEmpty().isNotEmpty()) {
            PillButton("Check", { doIt("check") { MachineText.missing(api.machineAppsCheck(name)) } }, enabled = working == null)
            PillButton("Sync", {
              scope.launch {
                try { val r = api.machineAppsSync(name, ""); message = "In step: ${r.synced} app(s)."; reload() }
                catch (e: ApiException) { val sent = MachineText.sentenceOf(e.text); if (sent != null) confirm = Triple("Send the credentials again", sent) { c -> "In step: ${api.machineAppsSync(name, c).synced} app(s)." } else message = e.text }
              }
            })
            PillButton("Revoke on machine", { doIt("revoke") { api.machineAppsRevoke(name); "Deleted from the machine." } }, enabled = working == null)
          }
        })
        ActionRow("Run a task here", "One time, now.", extra = {
          ZoTextField(task, { task = it }, placeholder = "What should it do on $name?", singleLine = false, minLines = 2, modifier = Modifier.fillMaxWidth().padding(top = 8.dp))
        }, actions = {
          PrimaryButton("Run a task here", {
            val p = task.trim()
            if (p.isNotEmpty()) doIt("task") { val r = api.machineTaskRun(name, p); task = ""; MachineText.taskDone(r.byRunner, name) }
          }, enabled = working == null && task.isNotBlank())
        })
        ActionRow("Remove this machine", "Zohara Agent forgets it and its saved key. Nothing changes on the machine itself.", divider = false, actions = {
          DangerButton("Remove", { removeAsk = true }, outlined = true)
        })
      }
      if (message.isNotBlank()) Text(message, style = ZoTheme.type.cardMeta, color = ZoTheme.colors.dim, modifier = Modifier.padding(top = 10.dp))
    }
  }
  confirm?.let { (title, sentence, run) -> NamedConfirmSheet(title, sentence, "Copy", { confirm = null }, run) { m -> message = m; reload() } }
  appsPlan?.let { plan -> AppsCopySheet(api, name, plan, onDismiss = { appsPlan = null }, onNeedConfirm = { sentence, picks -> confirm = Triple("Copy credentials to $name", sentence) { c -> "Copied ${api.machineAppsCopy(name, picks, c).copied} app(s) with credentials." } }, onDone = { m -> message = m; reload() }) }
  if (removeAsk) ConfirmSheet("Remove $name?", "Zohara Agent forgets this machine and its saved key. Nothing changes on the machine itself (the runner stays unless you remove it first).", "Remove", {
    scope.launch { try { api.machineRemove(name); onDismiss() } catch (e: ApiException) { message = e.text } }
  }, { removeAsk = false })
}

/** Picks which apps to copy, and for which of them the credentials too. */
@Composable
fun AppsCopySheet(api: ZoApi, name: String, plan: AppsPlanResp, onDismiss: () -> Unit, onNeedConfirm: (String, List<Pair<String, Boolean>>) -> Unit, onDone: (String) -> Unit) {
  val copy = remember { mutableStateMapOf<String, Boolean>().apply { plan.servers.forEach { put(it.name, !it.copied) } } }
  val withCred = remember { mutableStateMapOf<String, Boolean>() }
  var busy by remember { mutableStateOf(false) }
  var error by remember { mutableStateOf<String?>(null) }
  val scope = rememberCoroutineScope()
  ZoBottomSheet(
    "Copy connected apps to $name", onDismiss,
    footer = {
      SecondaryButton("Cancel", onDismiss)
      PrimaryButton("Copy", {
        val picks = plan.servers.filter { copy[it.name] == true }.map { it.name to (withCred[it.name] == true) }
        if (picks.isEmpty()) error = "Pick at least one app." else {
          busy = true; error = null
          scope.launch {
            try { val r = api.machineAppsCopy(name, picks, ""); onDismiss(); onDone("Copied ${r.copied} app(s) to $name.") }
            catch (e: ApiException) {
              val sent = MachineText.sentenceOf(e.text)
              if (sent != null) { onDismiss(); onNeedConfirm(sent, picks) } else error = e.text
            } finally { busy = false }
          }
        }
      }, loading = busy)
    },
  ) {
    Column(Modifier.heightIn(max = 520.dp).verticalScroll(rememberScrollState())) {
      if (plan.servers.isEmpty()) Text("There are no connected apps to copy.", style = ZoTheme.type.cardMeta, color = ZoTheme.colors.dim)
      SettingsCard {
        plan.servers.forEachIndexed { i, sv ->
          val hasCred = sv.credentials.isNotEmpty() || sv.oauth
          val sub = (if (sv.kind == "remote") "Address: " else "Starts: ") + sv.target + (if (hasCred) " · needs " + (sv.credentials.joinToString(", ").ifEmpty { "a sign-in" }) else "") +
            (if (sv.copied) " · already copied" else "") + sv.warnings.joinToString("") { " — $it" }
          ActionRow(sv.name, sub, divider = i < plan.servers.size - 1, actions = {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
              ZoSwitch(copy[sv.name] == true, { copy[sv.name] = it }, label = "Copy ${sv.name}")
              Text("Copy it", style = ZoTheme.type.cardMeta, color = ZoTheme.colors.text2)
            }
            if (hasCred) Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
              ZoSwitch(withCred[sv.name] == true, { withCred[sv.name] = it }, label = "Copy the credentials of ${sv.name}")
              Text("with its credentials", style = ZoTheme.type.cardMeta, color = ZoTheme.colors.text2)
            }
          })
        }
      }
      error?.let { ErrorLine(it, Modifier.padding(top = 8.dp)) }
    }
  }
}

@Composable
fun AddMachineSheet(onDismiss: () -> Unit, submit: suspend (String, String, String, Int, String, String) -> Unit) {
  var name by remember { mutableStateOf("") }
  var host by remember { mutableStateOf("") }
  var user by remember { mutableStateOf("") }
  var port by remember { mutableStateOf("22") }
  var note by remember { mutableStateOf("") }
  var key by remember { mutableStateOf("") }
  var busy by remember { mutableStateOf(false) }
  var error by remember { mutableStateOf<String?>(null) }
  val scope = rememberCoroutineScope()
  val clipboard = LocalClipboardManager.current
  ZoBottomSheet(
    "Add a machine", onDismiss,
    footer = {
      SecondaryButton("Cancel", onDismiss)
      PrimaryButton("Add", {
        val bad = MachineLogic.validate(name, host, user, port, key)
        if (bad.isNotEmpty()) { error = bad } else {
          busy = true; error = null
          scope.launch { try { submit(name.trim(), host.trim(), user.trim(), port.trim().toInt(), note.trim(), key.trim()); onDismiss() } catch (e: ApiException) { error = e.text } finally { busy = false } }
        }
      }, loading = busy)
    },
  ) {
    Column(Modifier.heightIn(max = 520.dp).verticalScroll(rememberScrollState())) {
      ZoTextField(name, { name = it }, label = "Name", placeholder = "home-server", modifier = Modifier.fillMaxWidth())
      androidx.compose.foundation.layout.Spacer(Modifier.size(10.dp))
      ZoTextField(host, { host = it }, label = "Host", placeholder = "192.168.1.20 or example.com", keyboardType = androidx.compose.ui.text.input.KeyboardType.Uri, modifier = Modifier.fillMaxWidth())
      androidx.compose.foundation.layout.Spacer(Modifier.size(10.dp))
      ZoTextField(user, { user = it }, label = "User", modifier = Modifier.fillMaxWidth())
      androidx.compose.foundation.layout.Spacer(Modifier.size(10.dp))
      ZoTextField(port, { port = it.filter(Char::isDigit).take(5) }, label = "Port", keyboardType = androidx.compose.ui.text.input.KeyboardType.Number, modifier = Modifier.fillMaxWidth())
      androidx.compose.foundation.layout.Spacer(Modifier.size(10.dp))
      ZoTextField(note, { note = it }, label = "Note (optional)", modifier = Modifier.fillMaxWidth())
      androidx.compose.foundation.layout.Spacer(Modifier.size(10.dp))
      ZoTextField(key, { key = it }, label = "Private key", placeholder = "-----BEGIN OPENSSH PRIVATE KEY-----", singleLine = false, minLines = 5, mono = true, modifier = Modifier.fillMaxWidth())
      Text("From -----BEGIN to -----END. It must not need a passphrase.", style = ZoTheme.type.fieldHint, color = ZoTheme.colors.dimmer, modifier = Modifier.padding(top = 5.dp))
      PillButton("Paste key", { key = clipboard.getText()?.text.orEmpty() }, modifier = Modifier.padding(top = 8.dp))
      error?.let { ErrorLine(it, Modifier.padding(top = 8.dp)) }
    }
  }
}
