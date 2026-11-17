package org.zohara.agent.settings

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.size
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.async
import kotlinx.coroutines.coroutineScope
import org.zohara.agent.LocalAppContainer
import org.zohara.agent.core.design.PillButton
import org.zohara.agent.core.design.PrimaryButton
import org.zohara.agent.core.design.SectionHeader
import org.zohara.agent.core.design.SettingsCard
import org.zohara.agent.core.design.SettingsRow
import org.zohara.agent.core.design.ZoIcon
import org.zohara.agent.core.design.ZoIconButton
import org.zohara.agent.core.design.ZoIcons
import org.zohara.agent.core.design.ZoSwitch
import org.zohara.agent.core.design.ZoTheme
import org.zohara.agent.core.net.AccountInfo
import org.zohara.agent.core.net.ApiException
import org.zohara.agent.core.net.CustomProviderInfo
import org.zohara.agent.core.net.ProviderInfo
import org.zohara.agent.core.net.ClaudeStatus
import org.zohara.agent.core.net.Profile
import org.zohara.agent.core.net.ProfilesResp
import org.zohara.agent.core.net.ProvidersResp
import org.zohara.agent.core.net.StatusInfo
import org.zohara.agent.core.net.ZoApi
import org.zohara.agent.nav.LocalNav
import org.zohara.agent.nav.Routes

/** Everything the Model page shows (SET-MOD). */
data class ModelPageData(
  val status: StatusInfo,
  val profiles: ProfilesResp,
  val providers: ProvidersResp,
  val hidden: List<String>,
  /** false: this server has no `settings_set`, the choice stays on this phone */
  val hiddenOnServer: Boolean,
  /** only for administrators */
  val claude: ClaudeStatus?,
)

suspend fun loadModelPage(api: ZoApi, hiddenStore: HiddenStore, restricted: Boolean): ModelPageData = coroutineScope {
  val st = async { api.status() }
  val pf = async { api.profiles() }
  val pv = async { api.providers() }
  val hd = async { hiddenStore.get() }
  val cc = async { if (restricted) null else try { api.claudeStatus().takeIf { it.admin } } catch (_: ApiException) { null } }
  val prov = pv.await()
  ModelPageData(st.await(), pf.await(), prov, (hd.await() + prov.hiddenProviders).distinct(), hiddenStore.onServer, cc.await())
}

/** Model and profiles. Parity rows: SET-MOD, MODEL-26..30, 36, 37, 18, 25. */
@Composable
fun SettingsModelScreen() {
  val container = LocalAppContainer.current
  val account = rememberAccount()
  val restricted = account?.restricted ?: true
  val doer = rememberDoer()
  val nav = LocalNav.current
  val store = remember { HiddenStore(container.api, LocalSettings(container.app.getSharedPreferences("zo_settings_local", 0))) }
  val loader = rememberLoader(restricted) { loadModelPage(container.api, store, restricted) }
  val load by loader.state.collectAsState()
  var sheet by remember { mutableStateOf(false) }
  var customSheet by remember { mutableStateOf<CustomSheetTarget?>(null) }
  var acctSheet by remember { mutableStateOf<AccountTarget?>(null) }
  var deleteCustom by remember { mutableStateOf<ProviderInfo?>(null) }
  var deleteAcct by remember { mutableStateOf<AccountTarget?>(null) }
  var rename by remember { mutableStateOf<Profile?>(null) }
  var delete by remember { mutableStateOf<Profile?>(null) }
  var newProfile by remember { mutableStateOf(false) }
  val invalidate = { container.providersCache.clear(); container.profilesCache.clear(); container.modelsCache.clear() }
  val refresh = { invalidate(); loader.load() }

  SettingsPage("Model and profiles") {
    LoadBox(load, onRetry = { loader.load() }) { d ->
      var hidden by remember(d) { mutableStateOf(d.hidden.toSet()) }
      ModelPageContent(
        d, restricted, hidden,
        ModelPageActions(
          onChange = { sheet = true },
          onUse = { p -> doer.run(onDone = refresh) { container.api.profileUse(p.id) } },
          onRename = { rename = it },
          onDelete = { delete = it },
          onNew = { newProfile = true },
          onAddEndpoint = { customSheet = CustomSheetTarget(null, customNames(d)) },
          onEditCustom = { customSheet = CustomSheetTarget(it, customNames(d)) },
          onTestCustom = { p -> doer.run { val r = container.api.customProviderTest(id = p.id, model = p.defaultModel); doer.say(r.message.ifBlank { if (r.works) "Connected." else "The test failed." }) } },
          onDeleteCustom = { deleteCustom = it },
          onAddAccount = { acctSheet = AccountTarget(it, null) },
          onEditAccount = { p, a -> acctSheet = AccountTarget(p, a) },
          onHideAccount = { p, a -> doer.run(onDone = refresh) { container.api.accountHide(p.id, a.id, !a.hidden) } },
          onDeleteAccount = { p, a -> deleteAcct = AccountTarget(p, a) },
          onRotate = { on -> doer.run(onDone = refresh) { container.api.settingsSet(rotateOnLimit = on); doer.say(if (on) "Will switch accounts at a rate limit" else "Will not switch accounts") } },
          onUnpin = { p, m -> doer.run(onDone = refresh) { container.api.profilePin(p, m, false) } },
          onToggleProvider = { id, shown ->
            val next = if (shown) hidden - id else hidden + id
            val before = hidden
            hidden = next
            doer.run(onFail = { hidden = before; doer.say(it) }, onDone = { invalidate() }) { store.set(next.toList()) }
            val name = d.providers.providers.firstOrNull { it.id == id }?.name ?: id
            doer.say(if (shown) "$name shown" else "$name hidden")
          },
          onOpenSync = { nav.settings(Routes.SETTINGS_SYNC) },
        ),
      )
    }
  }
  if (sheet) ModelSheet(onDismiss = { sheet = false }, onUsed = refresh)
  customSheet?.let { t ->
    CustomProviderSheet(
      existing = t.provider?.let { p -> CustomProviderInfo(p.id, p.name, p.format.ifBlank { "chat" }, p.baseUrl, p.defaultModel, p.hasKey) },
      otherNames = t.others, restricted = restricted, onDismiss = { customSheet = null }, onDone = { customSheet = null; refresh() },
    )
  }
  acctSheet?.let { t -> AccountSheet(provider = t.provider, existing = t.account, onDismiss = { acctSheet = null }, onDone = { acctSheet = null; refresh() }) }
  deleteCustom?.let { p ->
    ConfirmSheet("Delete ${p.name}?", "The provider, its API key, its accounts and the profiles that use it are removed from the server. Chats stay.", "Delete",
      { doer.run(onDone = refresh) { container.api.customProviderRemove(p.id) } }, { deleteCustom = null })
  }
  deleteAcct?.let { t ->
    val a = t.account ?: return@let
    ConfirmSheet("Delete ${t.provider.name} · ${a.name}?", "The account and its API key are removed from the server. Profiles that used it go back to the Default account.", "Delete",
      { doer.run(onDone = refresh) { container.api.accountRemove(t.provider.id, a.id) } }, { deleteAcct = null })
  }
  rename?.let { p ->
    InputSheet("Rename profile", "Save", { rename = null }, label = "Name", placeholder = "Work, Home, Free models…", initial = p.name, maxLength = 40) { name ->
      container.api.profileRename(p.id, name); refresh()
    }
  }
  if (newProfile) {
    InputSheet("New profile", "Create", { newProfile = false }, label = "Name", placeholder = "Work, Home, Free models…", hint = ModelLogic.NEW_PROFILE_HINT, maxLength = 40) { name ->
      container.api.profileCreate(name); refresh()
    }
  }
  delete?.let { p ->
    ConfirmSheet("Delete the profile \"${p.name}\"?", "Its saved choices and the API key it kept are removed from the server.", "Delete", { doer.run(onDone = refresh) { container.api.profileDelete(p.id) } }, { delete = null })
  }
}

/** The custom provider the sheet edits (null: a new one). */
data class CustomSheetTarget(val provider: ProviderInfo?, val others: List<String> = emptyList())

private fun customNames(d: ModelPageData): List<String> = d.providers.providers.filter { ModelLogic.isCustom(it) }.map { it.name }

/** An account of a provider (null account: a new one). */
data class AccountTarget(val provider: ProviderInfo, val account: AccountInfo?)

class ModelPageActions(
  val onChange: () -> Unit = {},
  val onUse: (Profile) -> Unit = {},
  val onRename: (Profile) -> Unit = {},
  val onDelete: (Profile) -> Unit = {},
  val onNew: () -> Unit = {},
  val onAddEndpoint: () -> Unit = {},
  val onUnpin: (profileId: String, model: String) -> Unit = { _, _ -> },
  val onToggleProvider: (id: String, shown: Boolean) -> Unit = { _, _ -> },
  val onOpenSync: () -> Unit = {},
  val onEditCustom: (ProviderInfo) -> Unit = {},
  val onTestCustom: (ProviderInfo) -> Unit = {},
  val onDeleteCustom: (ProviderInfo) -> Unit = {},
  val onAddAccount: (ProviderInfo) -> Unit = {},
  val onEditAccount: (ProviderInfo, AccountInfo) -> Unit = { _, _ -> },
  val onHideAccount: (ProviderInfo, AccountInfo) -> Unit = { _, _ -> },
  val onDeleteAccount: (ProviderInfo, AccountInfo) -> Unit = { _, _ -> },
  val onRotate: (Boolean) -> Unit = {},
)

@OptIn(ExperimentalLayoutApi::class)
@Composable
fun ModelPageContent(d: ModelPageData, restricted: Boolean, hidden: Set<String>, a: ModelPageActions) {
  val c = ZoTheme.colors
  val provName = { id: String -> d.providers.providers.firstOrNull { it.id == id }?.name?.ifBlank { id } ?: id }

  SectionHeader("Model")
  SettingsCard {
    val active = d.profiles.profiles.firstOrNull { it.id == d.profiles.active }
    val line = if (d.status.ready || d.status.provider.isNotBlank()) {
      (d.providers.providers.firstOrNull { it.id == d.status.provider }?.let { ModelLogic.accountLabel(it, d.providers.currentAccount) } ?: provName(d.status.provider)) + " · " + d.status.model.ifBlank { "no model chosen" } + (active?.let { " · profile ${it.name}" } ?: "")
    } else "Nothing chosen yet"
    SettingsRow("Provider and model", subtitle = line, control = { PrimaryButton("Change", a.onChange) })
    SettingsRow("API keys", subtitle = "Keys stay on the server, stored encrypted.", divider = false)
  }

  SectionHeader("Profiles")
  SettingsCard {
    if (d.profiles.profiles.isEmpty()) {
      ActionRow(
        "No profiles yet",
        "A profile is a preset: a provider, a model, its address, how hard it thinks and the models you pinned. Switch between them in one tap from the model button.",
      )
    }
    for (p in d.profiles.profiles) {
      val on = d.profiles.active == p.id
      ActionRow(
        p.name, ModelLogic.profileLine(p, provName(p.provider), d.providers.providers.firstOrNull { it.id == p.provider }?.accounts?.takeIf { it.size > 1 }?.firstOrNull { it.id == p.account }?.name),
        extra = {
          ModelLogic.visionLine(p)?.let { Text(it, style = ZoTheme.type.fieldHint, color = c.dimmer, modifier = Modifier.padding(top = 4.dp)) }
          if (p.pinned.isNotEmpty()) {
            FlowRow(Modifier.padding(top = 8.dp), horizontalArrangement = Arrangement.spacedBy(6.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
              p.pinned.forEach { m -> PinChip(m) { a.onUnpin(p.id, m) } }
            }
          }
        },
        actions = {
          PillButton(if (on) "In use" else "Use", { a.onUse(p) }, enabled = !on, accent = !on)
          ZoIconButton(ZoIcons.Edit, "Rename this profile", { a.onRename(p) }, size = 36.dp)
          ZoIconButton(ZoIcons.Trash, "Delete this profile and its key", { a.onDelete(p) }, tint = c.danger, size = 36.dp)
        },
      )
    }
    ActionRow(
      if (d.profiles.profiles.isEmpty()) "" else "Add another", if (d.profiles.profiles.isEmpty()) null else "Save the current settings as a profile.", divider = false,
      actions = {
        PrimaryButton("New profile…", a.onNew)
      },
    )
  }
  Note("Profiles, pinned models and MCP addresses sync from your desktop when you are signed in there.")
  Row(Modifier.padding(start = 6.dp)) { PillButton("Open Desktop sync", a.onOpenSync) }

  CustomProvidersSection(d, a)
  AccountsSection(d, a)

  SectionHeader("Pinned models")
  SettingsCard {
    val rows = ModelLogic.pinRows(d.profiles, provName)
    if (rows.isEmpty()) {
      ActionRow("Nothing pinned yet", "Open Change, find a model and tap the star next to it. Pinned models show in the model menu, one tap away.", divider = false)
    }
    rows.forEachIndexed { i, r ->
      SettingsRow(
        r.model, subtitle = "${r.profileName} · ${r.provider}", divider = i != rows.lastIndex,
        control = { PillButton("Unpin", { a.onUnpin(r.profileId, r.model) }) },
      )
    }
  }

  SectionHeader("Providers in the model picker")
  SettingsCard {
    val shown = ModelLogic.shownProviders(d.providers.providers, emptyList(), null, restricted)
    shown.forEachIndexed { i, p ->
      val inUse = p.id == d.providers.current
      val pins = d.profiles.profiles.filter { it.provider == p.id }.sumOf { it.pinned.size }
      SettingsRow(
        p.name.ifBlank { p.id }, subtitle = ModelLogic.providerSub(p, pins, inUse), divider = i != shown.lastIndex,
        control = {
          Row(Modifier.semantics { contentDescription = "Show in the model picker: ${p.name}" }) {
            ZoSwitch(checked = p.id !in hidden, onCheckedChange = { a.onToggleProvider(p.id, it) }, enabled = !inUse)
          }
        },
      )
    }
  }
  Note(
    "Switch off the providers you never use so the pickers stay short. The one in use always shows. " +
      if (d.hiddenOnServer) "This choice follows you to your other devices when sync is on." else "On this server the choice is kept on this phone only.",
  )

  d.claude?.let { cc ->
    SectionHeader("Claude Code (your subscription)")
    ClaudeCodeCard(cc)
  }
}

/** "Custom providers": the named servers of the person's own, each with its API format and address. */
@Composable
fun CustomProvidersSection(d: ModelPageData, a: ModelPageActions) {
  val c = ZoTheme.colors
  val customs = d.providers.providers.filter { ModelLogic.isCustom(it) && it.id.startsWith("custom:") }
  SectionHeader("Custom providers")
  SettingsCard {
    for (p in customs) {
      ActionRow(
        p.name, ModelLogic.customLine(p),
        actions = {
          PillButton("Edit", { a.onEditCustom(p) })
          PillButton("Test", { a.onTestCustom(p) })
          ZoIconButton(ZoIcons.Trash, "Delete the custom provider ${p.name}", { a.onDeleteCustom(p) }, tint = c.danger, size = 36.dp)
        },
      )
    }
    ActionRow(
      if (customs.isEmpty()) "No custom providers yet" else "Add another",
      "Your own server or a router: give it a name, pick the API format (OpenAI Chat, OpenAI Responses or Claude), its address and a key. It shows in the model list under its name.",
      divider = false, actions = { PrimaryButton("Add custom provider…", a.onAddEndpoint) },
    )
  }
}

/** "API key accounts": several named accounts (keys) of one provider, and the switch for the rate limit. */
@Composable
fun AccountsSection(d: ModelPageData, a: ModelPageActions) {
  val c = ZoTheme.colors
  val keyed = d.providers.providers.filter { it.takesAccounts }
  SectionHeader("API key accounts")
  SettingsCard {
    if (keyed.isEmpty()) {
      ActionRow("No accounts here", "This server has no providers with an API key.", divider = false)
      return@SettingsCard
    }
    var chosen by remember { mutableStateOf(keyed.first().id) }
    val p = keyed.firstOrNull { it.id == chosen } ?: keyed.first()
    Column(Modifier.padding(horizontal = 16.dp, vertical = 12.dp)) {
      FormatDropdown("Provider", keyed.map { it.id to it.name.ifBlank { it.id } }, p.id) { chosen = it }
      Text("Several accounts (API keys) of one provider, for example Groq 1 and Groq 2. The key you have now is the Default account.", style = ZoTheme.type.fieldHint, color = c.dimmer, modifier = Modifier.padding(top = 6.dp))
    }
    for (acc in p.accounts) {
      ActionRow(
        p.name.ifBlank { p.id } + " · " + acc.name, ModelLogic.accountLine(acc),
        actions = {
          PillButton("Edit", { a.onEditAccount(p, acc) })
          PillButton(if (acc.hidden) "Show" else "Hide", { a.onHideAccount(p, acc) })
          if (!acc.default) ZoIconButton(ZoIcons.Trash, "Delete the account ${acc.name}", { a.onDeleteAccount(p, acc) }, tint = c.danger, size = 36.dp)
        },
      )
    }
    ActionRow("Add an account", null, actions = { PrimaryButton("Add account…", { a.onAddAccount(p) }) })
    SettingsRow(
      "When an account hits its rate limit, switch to the next account",
      subtitle = "On a rate limit (HTTP 429) or a quota error the next account of the same provider is tried once, and the chat says so. Never another provider. Off by default.",
      divider = false,
      control = {
        Row(Modifier.semantics { contentDescription = "Switch to the next account at a rate limit" }) {
          ZoSwitch(checked = d.providers.rotateOnLimit, onCheckedChange = a.onRotate)
        }
      },
    )
  }
}

/** LINK-CC: the Claude Code card for administrators. */
@Composable
fun ClaudeCodeCard(cc: ClaudeStatus) {
  SettingsCard {
    SettingsRow(if (cc.present) "Ready on the server" else "Not set up", subtitle = claudeLine(cc.present, cc.setAt), divider = false)
  }
}

@Composable
private fun PinChip(model: String, onUnpin: () -> Unit) {
  val c = ZoTheme.colors
  Row(
    Modifier.clip(ZoTheme.shapes.pill).border(1.dp, c.line12, ZoTheme.shapes.pill).padding(start = 8.dp),
    verticalAlignment = Alignment.CenterVertically,
  ) {
    ZoIcon(ZoIcons.Star, tint = c.gold, size = 14.dp)
    Spacer(Modifier.width(4.dp))
    Text(model, style = ZoTheme.type.fieldHint, color = c.text2)
    Box1(onUnpin, "Remove $model from the model menu")
  }
}

@Composable
private fun Box1(onClick: () -> Unit, description: String) {
  androidx.compose.foundation.layout.Box(
    Modifier.size(32.dp).clickable(role = Role.Button, onClick = onClick).semantics { contentDescription = description },
    contentAlignment = Alignment.Center,
  ) { ZoIcon(ZoIcons.Close, tint = ZoTheme.colors.dim, size = 14.dp) }
}
