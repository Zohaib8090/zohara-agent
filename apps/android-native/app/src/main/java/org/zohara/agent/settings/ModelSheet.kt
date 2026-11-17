package org.zohara.agent.settings

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch
import org.zohara.agent.LocalAppContainer
import org.zohara.agent.core.design.ErrorLine
import org.zohara.agent.core.design.PillButton
import org.zohara.agent.core.design.PrimaryButton
import org.zohara.agent.core.design.SecondaryButton
import org.zohara.agent.core.design.ZoBottomSheet
import org.zohara.agent.core.design.ZoChip
import org.zohara.agent.core.design.ZoIcon
import org.zohara.agent.core.design.ZoIconButton
import org.zohara.agent.core.design.ZoIcons
import org.zohara.agent.core.design.ZoSpinner
import org.zohara.agent.core.design.ZoTextField
import org.zohara.agent.core.design.ZoTheme
import org.zohara.agent.core.net.ProviderInfo

/** What the sheet can do; the screen wires it to a [ModelSheetController], a screenshot test passes empty lambdas. */
class ModelSheetActions(
  val onProvider: (String) -> Unit = {},
  val onAddress: (String) -> Unit = {},
  val onAddressDone: () -> Unit = {},
  val onFilter: (String) -> Unit = {},
  val onChoose: (String) -> Unit = {},
  val onFetch: () -> Unit = {},
  val onTest: () -> Unit = {},
  val onSaveKey: (String) -> Unit = {},
  val onTogglePin: (String) -> Unit = {},
  val onEffort: (String) -> Unit = {},
  val onApiFormat: (String) -> Unit = {},
  val onSwitchFormat: (ModelLogic.ApiFormat) -> Unit = {},
  val onAddEndpoint: () -> Unit = {},
  val onAccount: (String) -> Unit = {},
  val onAddAccount: () -> Unit = {},
)

/**
 * The full model sheet as a bottom sheet (MODEL-04, 09..19, 23). Opened from "More models…" of the model menu and from Settings > Model and
 * profiles > Change. [onUsed] runs after "Use this model" worked.
 */
@Composable
fun ModelSheet(onDismiss: () -> Unit, onUsed: () -> Unit = {}) {
  val container = LocalAppContainer.current
  val account = rememberAccount()
  val restricted = account?.restricted ?: true
  val scope = rememberCoroutineScope()
  val ctl = remember {
    ModelSheetController(
      api = container.api, scope = scope, restricted = restricted, admin = account?.admin ?: false, cache = container.modelsCache,
      hiddenStore = HiddenStore(container.api, LocalSettings(container.app.getSharedPreferences("zo_settings_local", 0))),
      toast = { t -> scope.launch { container.toast.show(t) } },
      invalidate = { container.providersCache.clear(); container.profilesCache.clear(); container.modelsCache.clear() },
    )
  }
  DisposableEffect(ctl) { ctl.start(); onDispose { } }
  val s by ctl.state.collectAsState()
  var addEndpoint by remember { mutableStateOf(false) }
  var addAccount by remember { mutableStateOf(false) }
  val actions = remember(ctl) {
    ModelSheetActions(
      onProvider = ctl::selectProvider, onAddress = ctl::setAddress, onAddressDone = ctl::addressDone, onFilter = ctl::setFilter,
      onChoose = ctl::choose, onFetch = { ctl.fetchModels(force = true) }, onTest = ctl::testConnection, onSaveKey = { k -> ctl.saveKey(k) },
      onTogglePin = ctl::togglePin, onEffort = ctl::setEffort, onApiFormat = ctl::setApiFormat, onSwitchFormat = ctl::switchToFormat, onAddEndpoint = { addEndpoint = true },
      onAccount = { id -> if (id == ADD_ACCOUNT) addAccount = true else ctl.selectAccount(id) }, onAddAccount = { addAccount = true },
    )
  }
  ZoBottomSheet(
    title = "Model and API key", onDismiss = onDismiss,
    footer = {
      SecondaryButton("Close", onDismiss)
      PrimaryButton("Use this model", { ctl.use { onUsed(); onDismiss() } }, enabled = s.canUse, loading = s.using)
    },
  ) { ModelSheetBody(s, restricted, actions) }
  if (addEndpoint) {
    CustomProviderSheet(
      existing = null, otherNames = s.providers.filter { ModelLogic.isCustom(it) }.map { it.name }, restricted = restricted,
      onDismiss = { addEndpoint = false }, onDone = { id -> addEndpoint = false; ctl.start(select = id) },
    )
  }
  if (addAccount) {
    s.provider?.let { p ->
      AccountSheet(provider = p, existing = null, onDismiss = { addAccount = false }, onDone = { id -> addAccount = false; ctl.start(select = p.id, account = id) })
    }
  }
}

/** The "Add an account…" entry of the account drop-down. */
const val ADD_ACCOUNT = "+"

/** The content of the sheet (stateless: screenshot tested). */
@Composable
fun ModelSheetBody(s: ModelSheetState, restricted: Boolean, a: ModelSheetActions) {
  val c = ZoTheme.colors
  Column(Modifier.heightIn(max = 600.dp).verticalScroll(rememberScrollState())) {
    if (s.loading) {
      Text("Loading…", style = ZoTheme.type.cardMeta, color = c.dim, modifier = Modifier.padding(vertical = 16.dp))
      return@Column
    }
    if (s.loadError != null) { ErrorLine(s.loadError, Modifier.padding(vertical = 12.dp)); return@Column }

    Text("Provider", style = ZoTheme.type.fieldLabel, color = c.text, modifier = Modifier.padding(bottom = 6.dp))
    ProviderPicker(s, a.onProvider)
    s.claudeLine?.let { Text(it, style = ZoTheme.type.fieldHint, color = c.dimmer, modifier = Modifier.padding(top = 6.dp)) }
    s.hiddenHint?.let { Text(it, style = ZoTheme.type.fieldHint, color = c.dimmer, modifier = Modifier.padding(top = 6.dp)) }

    val p = s.provider
    if (p != null) {
      if (p.takesAccounts) AccountSection(s, p, a)
      if (p.takesFormat) ApiFormatSection(s, p, a.onApiFormat)
      AddressSection(s, p, restricted, a)
      if (!ModelLogic.isLocal(p)) KeySection(s, p, restricted, a)
      ModelListSection(s, a)
      if (s.efforts.isNotEmpty()) EffortPicker(s, a.onEffort)
    }
    Row(Modifier.padding(top = 16.dp)) { PillButton("Add a custom provider…", a.onAddEndpoint) }
  }
}

/** The account (API key) of the provider: "Groq · Groq 1". Every provider with a key has the Default account; "Add an account…" makes more. */
@Composable
private fun AccountSection(s: ModelSheetState, p: ProviderInfo, a: ModelSheetActions) {
  val options = s.accounts.map { it.id to (p.name.ifBlank { p.id } + " · " + it.name + if (it.hasKey) " ✓" else "") } + (ADD_ACCOUNT to "Add an account…")
  Column(Modifier.padding(top = 14.dp)) { FormatDropdown("Account", options, s.accountId, a.onAccount) }
}

@Composable
private fun ProviderPicker(s: ModelSheetState, onSelect: (String) -> Unit) {
  val c = ZoTheme.colors
  var open by remember { mutableStateOf(false) }
  Box {
    Row(
      Modifier.fillMaxWidth().clip(ZoTheme.shapes.field).background(c.input).border(1.dp, c.border, ZoTheme.shapes.field)
        .clickable(role = Role.DropdownList) { open = true }.padding(horizontal = 12.dp, vertical = 12.dp),
      verticalAlignment = Alignment.CenterVertically,
    ) {
      val cur = s.provider
      Text(cur?.let { ModelLogic.providerLabel(it) } ?: "Choose a provider…", style = ZoTheme.type.body.copy(fontSize = androidx.compose.ui.unit.TextUnit(14f, androidx.compose.ui.unit.TextUnitType.Sp)), color = if (cur == null) c.dimmer else c.text, modifier = Modifier.weight(1f), maxLines = 1, overflow = TextOverflow.Ellipsis)
      ZoIcon(ZoIcons.Down, tint = c.dim, size = 18.dp)
    }
    DropdownMenu(open, { open = false }, modifier = Modifier.background(c.popover).heightIn(max = 420.dp)) {
      for (g in ModelLogic.groups(s.providers)) {
        Text(g.label, style = ZoTheme.type.sectionHeader, color = c.dimmer, modifier = Modifier.padding(horizontal = 16.dp, vertical = 6.dp))
        for (p in g.items) {
          DropdownMenuItem(
            text = { Text(ModelLogic.providerLabel(p), style = ZoTheme.type.body, color = c.text) },
            trailingIcon = if (p.id == s.providerId) ({ ZoIcon(ZoIcons.Check, tint = c.accent, size = 16.dp) }) else null,
            onClick = { open = false; onSelect(p.id) },
          )
        }
      }
    }
  }
}

@Composable
private fun AddressSection(s: ModelSheetState, p: ProviderInfo, restricted: Boolean, a: ModelSheetActions) {
  val c = ZoTheme.colors
  var advanced by remember(p.id) { mutableStateOf(false) }
  val needs = ModelLogic.needsAddress(p)
  Column(Modifier.padding(top = 14.dp)) {
    if (!needs) {
      Row(
        Modifier.clickable(role = Role.Button) { advanced = !advanced }.padding(vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
      ) {
        Text("Advanced: address", style = ZoTheme.type.fieldLabel, color = c.text2)
        Spacer(Modifier.width(6.dp))
        ZoIcon(if (advanced) ZoIcons.Down else ZoIcons.Chevron, tint = c.dim, size = 16.dp)
      }
    }
    if (needs || advanced) {
      ZoTextField(
        s.address, a.onAddress, label = if (needs) "Address (endpoint URL)" else null, placeholder = p.defaultUrl.ifBlank { "https://" },
        hint = s.addressError.ifBlank { ModelLogic.addressHint(p, restricted) }, isError = s.addressError.isNotBlank(),
        keyboardType = androidx.compose.ui.text.input.KeyboardType.Uri, modifier = Modifier.fillMaxWidth(),
      )
      Row(Modifier.padding(top = 8.dp), verticalAlignment = Alignment.CenterVertically) {
        SecondaryButton("Test connection", a.onTest, enabled = !s.testing, loading = s.testing)
        Spacer(Modifier.width(10.dp))
        Text(
          s.testText, style = ZoTheme.type.fieldHint,
          color = when (s.testOk) { true -> c.ok; false -> c.danger; null -> c.dimmer }, modifier = Modifier.weight(1f),
        )
      }
      // the server said which format it speaks: one tap sets the API format of this provider to it
      s.hintedFormat?.takeIf { ModelLogic.isCustom(p) || p.takesFormat }?.let { f ->
        Row(Modifier.padding(top = 8.dp)) { PillButton("Use API format: ${f.name}", { a.onSwitchFormat(f) }, accent = true) }
      }
    }
  }
}

@Composable
private fun KeySection(s: ModelSheetState, p: ProviderInfo, restricted: Boolean, a: ModelSheetActions) {
  val c = ZoTheme.colors
  var key by remember(p.id, s.accountId) { mutableStateOf("") }
  Column(Modifier.padding(top = 14.dp)) {
    Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.padding(bottom = 6.dp)) {
      Text("API key", style = ZoTheme.type.fieldLabel, color = c.text)
      Spacer(Modifier.width(8.dp))
      val saved = ModelLogic.accountHasKey(p, s.accountId)
      ZoChip(if (saved) "Key saved" else "No key yet", color = if (saved) c.okChip else c.warn)
    }
    Row(verticalAlignment = Alignment.Top) {
      ZoTextField(key, { key = it }, placeholder = "Paste the key (a saved one is used when this stays empty)", password = true, modifier = Modifier.weight(1f))
      Spacer(Modifier.width(8.dp))
      PrimaryButton("Save", { a.onSaveKey(key); key = "" }, enabled = key.isNotBlank() && !s.savingKey, loading = s.savingKey)
    }
    Text(
      if (restricted) "Bring your own key: it is stored encrypted on our server and only used to call your provider for you."
      else "Keys stay on the server, stored encrypted. Empty keeps the saved one.",
      style = ZoTheme.type.fieldHint, color = c.dimmer, modifier = Modifier.padding(top = 5.dp),
    )
  }
}

@Composable
private fun ModelListSection(s: ModelSheetState, a: ModelSheetActions) {
  val c = ZoTheme.colors
  Column(Modifier.padding(top = 14.dp)) {
    Row(verticalAlignment = Alignment.CenterVertically) {
      Text("Model", style = ZoTheme.type.fieldLabel, color = c.text, modifier = Modifier.weight(1f))
      ZoIconButton(ZoIcons.Refresh, "Load the list again", a.onFetch, size = 32.dp, iconSize = 18.dp, enabled = !s.fetching)
    }
    Spacer(Modifier.size(6.dp))
    ZoTextField(
      s.filter, a.onFilter, placeholder = if (s.models.isEmpty() && !s.fetching) "Model name, or pick one below" else "Search models",
      modifier = Modifier.fillMaxWidth(),
    )
    val pins = s.pins
    // MODEL-08: Claude Code lists the newest model of each family and keeps the older ones under "More models" (a search shows all)
    val grouped = s.providerId == "claude-code" && s.filter.isBlank()
    val split = if (grouped) ModelLogic.splitMain(s.models) else s.models to emptyList()
    val items = ModelLogic.orderModels(split.first, pins, s.filter)
    val older = split.second
    var showOlder by remember(s.providerId) { mutableStateOf(false) }
    val custom = ModelLogic.customOffer(s.models, s.filter)
    Spacer(Modifier.size(8.dp))
    if (s.fetching) {
      Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.padding(vertical = 10.dp)) {
        ZoSpinner(size = 16.dp)
        Spacer(Modifier.width(8.dp))
        Text("Fetching the models list…", style = ZoTheme.type.fieldHint, color = c.dimmer)
      }
    } else {
      LazyColumn(
        Modifier.fillMaxWidth().heightIn(max = 260.dp).clip(ZoTheme.shapes.field).border(1.dp, c.line, ZoTheme.shapes.field),
      ) {
        items(items, key = { it }) { m -> ModelRow(m, m == s.chosen, m in pins, a) }
        if (older.isNotEmpty()) {
          item(key = "more-models") {
            Row(
              Modifier.fillMaxWidth().clickable(role = Role.Button) { showOlder = !showOlder }.padding(horizontal = 14.dp, vertical = 12.dp),
              verticalAlignment = Alignment.CenterVertically,
            ) {
              Text("More models", style = ZoTheme.type.body, color = c.text2, modifier = Modifier.weight(1f))
              ZoIcon(if (showOlder) ZoIcons.Down else ZoIcons.Chevron, tint = c.dim, size = 16.dp)
            }
          }
          if (showOlder) items(older, key = { "older:$it" }) { m -> ModelRow(m, m == s.chosen, m in pins, a) }
        }
        if (custom.isNotEmpty()) item(key = "custom:$custom") { CustomRow(custom, custom == s.chosen, a.onChoose) }
        if (items.isEmpty() && custom.isEmpty()) item(key = "none") {
          Text(
            if (s.models.isNotEmpty()) "No model matches your search." else "No models yet.",
            style = ZoTheme.type.cardMeta, color = c.dim, modifier = Modifier.padding(14.dp),
          )
        }
      }
      if (s.fetchError.isNotBlank()) {
        Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.padding(top = 8.dp)) {
          Text(s.fetchError, style = ZoTheme.type.fieldHint, color = c.danger, modifier = Modifier.weight(1f))
          Spacer(Modifier.width(8.dp))
          PillButton("Retry", a.onFetch)
        }
      } else if (s.models.isNotEmpty()) {
        Text("${s.models.size} models. Tap the star to pin one to the model menu.", style = ZoTheme.type.fieldHint, color = c.dimmer, modifier = Modifier.padding(top = 6.dp))
      }
    }
  }
}

@Composable
private fun ModelRow(name: String, chosen: Boolean, pinned: Boolean, a: ModelSheetActions) {
  val c = ZoTheme.colors
  Row(
    Modifier.fillMaxWidth().clickable(role = Role.RadioButton) { a.onChoose(name) }.semantics { selected = chosen }
      .background(if (chosen) c.active else androidx.compose.ui.graphics.Color.Transparent).padding(start = 14.dp),
    verticalAlignment = Alignment.CenterVertically,
  ) {
    Text(name, style = ZoTheme.type.body, color = c.text, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f).padding(vertical = 10.dp))
    if (chosen) ZoIcon(ZoIcons.Check, tint = c.accent, size = 16.dp)
    ZoIconButton(
      ZoIcons.Star, if (pinned) "Unpin from the model menu" else "Pin to the model menu", { a.onTogglePin(name) },
      tint = if (pinned) c.gold else c.dimmer, size = 40.dp, iconSize = 18.dp,
    )
  }
}

@Composable
private fun CustomRow(text: String, chosen: Boolean, onChoose: (String) -> Unit) {
  val c = ZoTheme.colors
  Row(Modifier.fillMaxWidth().clickable(role = Role.RadioButton) { onChoose(text) }.background(if (chosen) c.active else androidx.compose.ui.graphics.Color.Transparent).padding(horizontal = 14.dp, vertical = 12.dp), verticalAlignment = Alignment.CenterVertically) {
    Text("Use “$text” as the model name", style = ZoTheme.type.body, color = c.text, maxLines = 2, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
    if (chosen) ZoIcon(ZoIcons.Check, tint = c.accent, size = 16.dp)
  }
}

@Composable
private fun EffortPicker(s: ModelSheetState, onEffort: (String) -> Unit) {
  val c = ZoTheme.colors
  var open by remember { mutableStateOf(false) }
  Column(Modifier.padding(top = 14.dp)) {
    Text("Effort", style = ZoTheme.type.fieldLabel, color = c.text, modifier = Modifier.padding(bottom = 6.dp))
    Box {
      Row(
        Modifier.fillMaxWidth().clip(ZoTheme.shapes.field).background(c.input).border(1.dp, c.border, ZoTheme.shapes.field)
          .clickable(role = Role.DropdownList) { open = true }.padding(horizontal = 12.dp, vertical = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
      ) {
        Text(ModelLogic.effortLabel(s.effort), style = ZoTheme.type.body, color = c.text, modifier = Modifier.weight(1f))
        ZoIcon(ZoIcons.Down, tint = c.dim, size = 18.dp)
      }
      DropdownMenu(open, { open = false }, modifier = Modifier.background(c.popover)) {
        for (lvl in listOf("") + s.efforts) {
          DropdownMenuItem(
            text = { Text(ModelLogic.effortLabel(lvl), style = ZoTheme.type.body, color = c.text) },
            trailingIcon = if (lvl == s.effort) ({ ZoIcon(ZoIcons.Check, tint = c.accent, size = 16.dp) }) else null,
            onClick = { open = false; onEffort(lvl) },
          )
        }
      }
    }
    Text("How hard it thinks before answering", style = ZoTheme.type.fieldHint, color = c.dimmer, modifier = Modifier.padding(top = 5.dp))
  }
}

/** The "API format" of a hosted OpenAI-compatible provider: its normal format, or another one it is told to speak. */
@Composable
private fun ApiFormatSection(s: ModelSheetState, p: ProviderInfo, onFormat: (String) -> Unit) {
  val normal = ModelLogic.formatOf(p)
  val options = listOf("" to "Default: ${normal.label}") + ModelLogic.FORMATS.map { it.id to it.label }
  Column(Modifier.padding(top = 14.dp)) {
    FormatDropdown("API format", options, s.apiFormat, onFormat)
    Text(ModelLogic.formatHint(p), style = ZoTheme.type.fieldHint, color = ZoTheme.colors.dimmer, modifier = Modifier.padding(top = 5.dp))
  }
}

/** A drop-down of (id, label) pairs, shown like the provider and effort fields. */
@Composable
fun FormatDropdown(label: String, options: List<Pair<String, String>>, selected: String, onSelect: (String) -> Unit) {
  val c = ZoTheme.colors
  var open by remember { mutableStateOf(false) }
  Text(label, style = ZoTheme.type.fieldLabel, color = c.text, modifier = Modifier.padding(bottom = 6.dp))
  Box {
    Row(
      Modifier.fillMaxWidth().clip(ZoTheme.shapes.field).background(c.input).border(1.dp, c.border, ZoTheme.shapes.field)
        .clickable(role = Role.DropdownList) { open = true }.padding(horizontal = 12.dp, vertical = 12.dp),
      verticalAlignment = Alignment.CenterVertically,
    ) {
      Text(options.firstOrNull { it.first == selected }?.second ?: selected, style = ZoTheme.type.body, color = c.text, modifier = Modifier.weight(1f))
      ZoIcon(ZoIcons.Down, tint = c.dim, size = 18.dp)
    }
    DropdownMenu(open, { open = false }, modifier = Modifier.background(c.popover)) {
      for ((id, text) in options) {
        DropdownMenuItem(
          text = { Text(text, style = ZoTheme.type.body, color = c.text) },
          trailingIcon = if (id == selected) ({ ZoIcon(ZoIcons.Check, tint = c.accent, size = 16.dp) }) else null,
          onClick = { open = false; onSelect(id) },
        )
      }
    }
  }
}

// ================================================================================================================ custom providers and accounts

/** Add (existing null) or edit a custom provider: name, API format, address, key and a model to start with. [onDone] gets its id. */
@Composable
fun CustomProviderSheet(existing: org.zohara.agent.core.net.CustomProviderInfo?, otherNames: Collection<String>, restricted: Boolean, onDismiss: () -> Unit, onDone: (String) -> Unit) {
  val container = LocalAppContainer.current
  val scope = rememberCoroutineScope()
  val ctl = remember {
    CustomProviderController(
      container.api, scope, restricted, toast = { t -> scope.launch { container.toast.show(t) } },
      invalidate = { container.providersCache.clear(); container.profilesCache.clear(); container.modelsCache.clear() },
      otherNames = otherNames.filter { it != existing?.name },
    )
  }
  val st by ctl.state.collectAsState()
  var format by remember { mutableStateOf(ModelLogic.formatById(existing?.format) ?: ModelLogic.FORMATS[0]) }
  var name by remember { mutableStateOf(existing?.name.orEmpty()) }
  var address by remember { mutableStateOf(existing?.baseUrl.orEmpty()) }
  var key by remember { mutableStateOf("") }
  var model by remember { mutableStateOf(existing?.defaultModel.orEmpty()) }
  ZoBottomSheet(
    title = if (existing == null) "Add a custom provider" else "Edit custom provider", onDismiss = onDismiss,
    footer = {
      SecondaryButton("Cancel", onDismiss)
      PrimaryButton(if (existing == null) "Add provider" else "Save", { ctl.save(existing?.id, name, format, address, key, model, onDone) }, loading = st.busy)
    },
  ) {
    EndpointForm(
      name, { name = it }, address, { address = it }, key, { key = it }, model, { model = it },
      st.error, st.testText, st.testOk, st.testing, onTest = { ctl.test(existing?.id, format, address, key, model) },
      format = format, onFormat = { format = it }, keyHint = if (existing != null) (if (existing.hasKey) "Key saved. Empty keeps it; a new address removes it." else "No key saved") else "Only if the server needs one",
    )
  }
}

/**
 * Add (existing null) or edit an account of [provider]: a name, its API key, and (only for a provider that allows it) an address of its own.
 * [onDone] gets the id of the account.
 */
@Composable
fun AccountSheet(provider: ProviderInfo, existing: org.zohara.agent.core.net.AccountInfo?, onDismiss: () -> Unit, onDone: (String) -> Unit) {
  val container = LocalAppContainer.current
  val scope = rememberCoroutineScope()
  val ctl = remember { AccountController(container.api, invalidate = { container.providersCache.clear(); container.profilesCache.clear(); container.modelsCache.clear() }) }
  var name by remember { mutableStateOf(existing?.name.orEmpty()) }
  var key by remember { mutableStateOf("") }
  var address by remember { mutableStateOf(existing?.baseUrl.orEmpty()) }
  var error by remember { mutableStateOf("") }
  var busy by remember { mutableStateOf(false) }
  var testText by remember { mutableStateOf("") }
  var testOk by remember { mutableStateOf<Boolean?>(null) }
  var testing by remember { mutableStateOf(false) }
  val takesAddress = !ModelLogic.isCustom(provider) && existing?.default != true
  val others = provider.accounts.filter { it.id != existing?.id }.map { it.name }
  ZoBottomSheet(
    title = if (existing == null) "Add an account" else "Edit account", onDismiss = onDismiss,
    footer = {
      SecondaryButton("Cancel", onDismiss)
      PrimaryButton(if (existing == null) "Add account" else "Save", {
        busy = true
        scope.launch {
          val msg = if (existing == null) ctl.add(provider.id, name, key, address, others)
          else ctl.edit(provider.id, existing.id, name, key, if (takesAddress) address else null, others)
          busy = false
          if (msg.isEmpty()) onDone(existing?.id ?: accountIdFor(provider, name)) else error = msg
        }
      }, loading = busy)
    },
  ) {
    val c = ZoTheme.colors
    Column(Modifier.heightIn(max = 520.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(10.dp)) {
      Text("Another API key of ${provider.name.ifBlank { provider.id }}, for example a second account. Its key is kept on its own and only used for this account.", style = ZoTheme.type.fieldHint, color = c.dimmer)
      ZoTextField(name, { name = it }, label = "Name", placeholder = "${provider.name.ifBlank { provider.id }} 1", modifier = Modifier.fillMaxWidth())
      ZoTextField(key, { key = it }, label = "API key", password = true, placeholder = if (existing == null) "Paste the key" else if (existing.hasKey) "Key saved. Empty keeps it" else "No key yet", modifier = Modifier.fillMaxWidth())
      if (takesAddress) ZoTextField(address, { address = it }, label = "Address (optional)", placeholder = provider.defaultUrl.ifBlank { "https://" }, hint = "Only if this account uses another address of ${provider.name}.", keyboardType = androidx.compose.ui.text.input.KeyboardType.Uri, modifier = Modifier.fillMaxWidth())
      if (existing != null) {
        Row(verticalAlignment = Alignment.CenterVertically) {
          SecondaryButton("Test connection", {
            testing = true; testOk = null; testText = "Testing…"
            scope.launch { val (ok, m) = ctl.test(provider.id, existing.id); testing = false; testOk = ok; testText = m }
          }, enabled = !testing, loading = testing)
          Spacer(Modifier.width(10.dp))
          Text(testText, style = ZoTheme.type.fieldHint, color = when (testOk) { true -> c.ok; false -> c.danger; null -> c.dimmer }, modifier = Modifier.weight(1f))
        }
      }
      if (error.isNotBlank()) ErrorLine(error)
    }
  }
}

/** The id the server gives an account named [name]: a slug, made unique per provider (the sheet then shows that account). */
fun accountIdFor(provider: ProviderInfo, name: String): String = ModelLogic.slug(name).let { base ->
  val taken = provider.accounts.map { it.id }.toSet() + "default"
  if (base !in taken) base else generateSequence(2) { it + 1 }.map { "$base-$it" }.first { it !in taken }
}

@Composable
fun EndpointForm(
  name: String, onName: (String) -> Unit, address: String, onAddress: (String) -> Unit, key: String, onKey: (String) -> Unit,
  model: String, onModel: (String) -> Unit, error: String, testText: String, testOk: Boolean?, testing: Boolean, onTest: () -> Unit,
  format: ModelLogic.ApiFormat = ModelLogic.FORMATS[0], onFormat: (ModelLogic.ApiFormat) -> Unit = {}, keyHint: String = "Only if the server needs one",
) {
  val c = ZoTheme.colors
  Column(Modifier.heightIn(max = 520.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(10.dp)) {
    Text("Any server that speaks one of the three formats: vLLM, LM Studio, an agent router, a company gateway, a proxy. Give it a name; it shows in the model list under that name.", style = ZoTheme.type.fieldHint, color = c.dimmer)
    FormatDropdown("API format", ModelLogic.FORMATS.map { it.id to it.label }, format.id) { id -> ModelLogic.formatById(id)?.let(onFormat) }
    Text("The app adds ${format.path.removePrefix("/v1")} to the address. Not sure? Test connection says which one the server speaks.", style = ZoTheme.type.fieldHint, color = c.dimmer)
    ZoTextField(name, onName, label = "Name", placeholder = "My router, Work gateway, Home Ollama", modifier = Modifier.fillMaxWidth())
    ZoTextField(address, onAddress, label = "Address", placeholder = "https://my-host.example/v1", keyboardType = androidx.compose.ui.text.input.KeyboardType.Uri, modifier = Modifier.fillMaxWidth())
    ZoTextField(key, onKey, label = "Key", password = true, placeholder = keyHint, modifier = Modifier.fillMaxWidth())
    ZoTextField(model, onModel, label = "Model", placeholder = "Model name (Test connection can list them)", modifier = Modifier.fillMaxWidth())
    Row(verticalAlignment = Alignment.CenterVertically) {
      SecondaryButton("Test connection", onTest, enabled = !testing, loading = testing)
      Spacer(Modifier.width(10.dp))
      Text(testText, style = ZoTheme.type.fieldHint, color = when (testOk) { true -> c.ok; false -> c.danger; null -> c.dimmer }, modifier = Modifier.weight(1f))
    }
    if (testOk == false) ModelLogic.hintFormat(testText, format)?.let { f -> Row { PillButton("Use API format: ${f.name}", { onFormat(f) }, accent = true) } }
    if (error.isNotBlank()) ErrorLine(error)
  }
}
