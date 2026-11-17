package org.zohara.agent.settings

import android.content.SharedPreferences
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.async
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import org.zohara.agent.core.net.ApiException
import org.zohara.agent.core.net.ErrorKind
import org.zohara.agent.core.net.ModelsResp
import org.zohara.agent.core.net.ProfilesResp
import org.zohara.agent.core.net.ProviderInfo
import org.zohara.agent.core.net.ProvidersResp
import org.zohara.agent.core.net.StatusInfo
import org.zohara.agent.core.net.ZoApi
import org.zohara.agent.core.store.TtlCache

/** Small phone-only choices that have no server home (the fallback of the hidden providers when the server has no `settings_set`). */
class LocalSettings(private val prefs: SharedPreferences) {
  fun hiddenProviders(): List<String> = prefs.getStringSet("hidden_providers", emptySet()).orEmpty().sorted()
  fun setHiddenProviders(list: Collection<String>) { prefs.edit().putStringSet("hidden_providers", list.toSet()).apply() }
}

/**
 * MODEL-29: the providers the person hid. They live in the synced settings document (`settings_get`, `settings_set`); a server that does not
 * have the ops yet answers "unknown request", and then the choice stays on this phone.
 */
class HiddenStore(private val api: ZoApi, private val local: LocalSettings?) {
  /** false once the server said it has no `settings_set`: the choice is only on this phone */
  var onServer: Boolean = true
    private set

  suspend fun get(): List<String> = try {
    val r = api.settingsGet()
    onServer = true
    r.hiddenProviders
  } catch (e: ApiException) {
    if (e.kind != ErrorKind.UnknownOp) throw e
    onServer = false
    local?.hiddenProviders().orEmpty()
  }

  suspend fun set(list: List<String>) {
    if (onServer) {
      try { api.settingsSet(hiddenProviders = list); return } catch (e: ApiException) { if (e.kind != ErrorKind.UnknownOp) throw e; onServer = false }
    }
    local?.setHiddenProviders(list)
  }
}

data class ModelSheetState(
  val loading: Boolean = true,
  val loadError: String? = null,
  val providers: List<ProviderInfo> = emptyList(),
  val hiddenHint: String? = null,
  val providerId: String = "",
  val address: String = "",
  val addressError: String = "",
  val models: List<String> = emptyList(),
  val chosen: String = "",
  val filter: String = "",
  val fetching: Boolean = false,
  val fetchError: String = "",
  val efforts: List<String> = emptyList(),
  val effort: String = "",
  val pins: Set<String> = emptySet(),
  val testing: Boolean = false,
  val testText: String = "",
  val testOk: Boolean? = null,
  val savingKey: Boolean = false,
  val using: Boolean = false,
  val claudeLine: String? = null,
  /** the API format chosen for a hosted provider that takes one: "" is its normal format */
  val apiFormat: String = "",
  /** the account (API key) of the provider in use here; "default" is the one every provider has */
  val accountId: String = "default",
) {
  val provider: ProviderInfo? get() = providers.firstOrNull { it.id == providerId }
  /** the accounts the picker offers (not the hidden ones, except the one in use); empty for a provider with no key */
  val accounts: List<org.zohara.agent.core.net.AccountInfo> get() = provider?.takeIf { it.takesAccounts }?.let { ModelLogic.shownAccounts(it, accountId) }.orEmpty()
  /** the account name as a picker shows it: "Groq · Groq 1" */
  val accountLabel: String get() = provider?.let { ModelLogic.accountLabel(it, accountId) }.orEmpty()
  /** The format hint of the last test, when it names another API format to use ([ModelLogic.hintFormat]). */
  val hintedFormat: ModelLogic.ApiFormat? get() = if (testOk == false) provider?.let { ModelLogic.hintFormat(testText, ModelLogic.formatOf(it)) } else null
  val canUse: Boolean get() = chosen.isNotBlank() && providerId.isNotBlank() && !using
}

/**
 * The logic of the "Model and API key" sheet (MODEL-09..19, 23, 24). One per open sheet; [state] is what the screen shows.
 * Rules are those of the web2 sheet: the selection is live, "Use this model" saves it, the star pins to the active profile.
 */
class ModelSheetController(
  private val api: ZoApi,
  private val scope: CoroutineScope,
  private val restricted: Boolean,
  private val admin: Boolean,
  private val cache: TtlCache<String, ModelsResp>,
  private val hiddenStore: HiddenStore,
  private val toast: (String) -> Unit,
  /** forget the cached providers, profiles and model lists of the app */
  private val invalidate: () -> Unit,
) {
  private val _state = MutableStateFlow(ModelSheetState())
  val state: StateFlow<ModelSheetState> = _state

  private var prov = ProvidersResp()
  private var prof = ProfilesResp()
  private var status = StatusInfo()
  private var seq = 0
  private var dirty = false
  private var fetchJob: Job? = null

  /** Loads everything. [select] is a provider to show instead of the one in use (a provider or account the person just added). */
  fun start(select: String? = null, account: String? = null) {
    scope.launch {
      _state.update { it.copy(loading = true, loadError = null) }
      try {
        coroutineScope {
          val a = async { api.providers() }
          val b = async { api.profiles() }
          val c = async { api.status() }
          val d = async { hiddenStore.get() }
          val cc = async { if (admin && !restricted) try { api.claudeStatus() } catch (_: ApiException) { null } else null }
          prov = a.await(); prof = b.await(); status = c.await()
          val hidden = (d.await() + prov.hiddenProviders).distinct()
          val claude = cc.await()
          val list = ModelLogic.sheetProviders(prov.providers, hidden, prov.current, restricted, claude != null && claude.admin && claude.present)
          _state.update {
            it.copy(
              providers = list, providerId = (select ?: prov.current).takeIf { id -> list.any { p -> p.id == id } } ?: prov.current.takeIf { id -> list.any { p -> p.id == id } } ?: "",
              accountId = ModelLogic.accountFor(list.firstOrNull { p -> p.id == (select ?: prov.current) }, account ?: prov.currentAccount.takeIf { _ -> select == null }),
              hiddenHint = ModelLogic.hiddenHint(prov.providers, hidden, prov.current, restricted),
              chosen = status.model, pins = ModelLogic.activePins(prof).toSet(),
              claudeLine = if (claude != null && claude.admin) claudeLine(claude.present, claude.setAt) else null,
            )
          }
        }
        val p = _state.value.provider
        if (p != null) {
          _state.update { it.copy(address = ModelLogic.savedAddress(p, prov.current), chosen = if (p.id == prov.current) status.model else "") }
          fetchModels()
        }
        _state.update { it.copy(loading = false) }
      } catch (e: CancellationException) {
        throw e
      } catch (e: ApiException) {
        _state.update { it.copy(loading = false, loadError = e.text) }
      }
    }
  }

  // ---- provider and address

  fun selectProvider(id: String) {
    val p = _state.value.providers.firstOrNull { it.id == id } ?: return
    dirty = false
    seq++
    _state.update {
      it.copy(
        providerId = id, accountId = if (id == prov.current) ModelLogic.accountFor(p, prov.currentAccount) else "default",
        address = ModelLogic.savedAddress(p, prov.current), addressError = "", models = emptyList(),
        chosen = if (id == prov.current) status.model else "", filter = "", fetchError = "", fetching = false,
        efforts = emptyList(), effort = "", testText = "", testOk = null,
        apiFormat = if (p.takesFormat) p.apiFormat.orEmpty() else "",
      )
    }
    // a custom endpoint needs the address first; every other provider lists its models at once
    if (ModelLogic.needsAddress(p) && _state.value.address.isBlank()) return
    fetchModels()
  }

  /** The API format of a hosted provider (`""` = its normal one): the list, the test and "Use this model" ask in that format. */
  fun setApiFormat(id: String) {
    dirty = true
    seq++
    _state.update { it.copy(apiFormat = id, testText = "", testOk = null, models = emptyList(), fetchError = "") }
    fetchModels()
  }

  /** Another account (API key) of the provider: its own key is used for the list, the test and "Use this model". */
  fun selectAccount(id: String) {
    val p = _state.value.provider ?: return
    dirty = true
    seq++
    _state.update { it.copy(accountId = ModelLogic.accountFor(p, id), models = emptyList(), fetchError = "", testText = "", testOk = null, chosen = if (p.id == prov.current && ModelLogic.accountFor(p, id) == ModelLogic.accountFor(p, prov.currentAccount)) status.model else "") }
    fetchModels()
  }

  /**
   * "Use it" under a test hint: the server speaks another API format. A custom provider changes its own format (and keeps its name, address and
   * key); a hosted provider is told the format; an older server with the fixed Custom endpoints switches to the one of that format.
   */
  fun switchToFormat(f: ModelLogic.ApiFormat) {
    val s = _state.value; val p = s.provider ?: return
    when {
      p.takesFormat -> setApiFormat(f.id)
      p.id.startsWith("custom:") -> scope.launch {
        try {
          api.customProviderUpdate(p.id, format = f.id)
          invalidate()
          start(select = p.id)
        } catch (e: CancellationException) {
          throw e
        } catch (e: ApiException) {
          toast(e.text)
        }
      }
      else -> {
        val target = s.providers.firstOrNull { it.id == f.providerId } ?: return
        val address = s.address
        selectProvider(target.id)
        _state.update { it.copy(address = address) }
        if (ModelLogic.isCustom(p) && !target.hasKey) toast("Paste the key for ${target.name} (a key is kept per provider).")
        fetchModels()
      }
    }
  }

  /** The account name to send: none for the Default account and for a provider with no accounts. */
  private fun accountArg(p: ProviderInfo, id: String): String? = if (p.takesAccounts && id.isNotBlank() && id != "default") id else null

  private fun formatArg(p: ProviderInfo, id: String): String? = if (p.takesFormat && id.isNotEmpty()) id else null

  fun setAddress(text: String) { dirty = true; _state.update { it.copy(address = text, addressError = "", testText = "", testOk = null) } }

  /** called when the address field loses focus or the person presses done */
  fun addressDone() {
    val s = _state.value; val p = s.provider ?: return
    val bad = ModelLogic.validateAddress(s.address, ModelLogic.needsAddress(p), restricted)
    _state.update { it.copy(addressError = bad) }
    if (bad.isEmpty()) fetchModels()
  }

  fun setFilter(text: String) { _state.update { it.copy(filter = text) } }
  fun choose(model: String) { _state.update { it.copy(chosen = model) } }
  fun setEffort(level: String) { _state.update { it.copy(effort = level) } }

  // ---- model list (MODEL-14, 15, 16)

  fun fetchModels(force: Boolean = false) {
    val s = _state.value
    val p = s.provider
    if (p == null) { _state.update { it.copy(models = emptyList()) }; return }
    val bad = ModelLogic.validateAddress(s.address, ModelLogic.needsAddress(p), restricted)
    if (bad.isNotEmpty()) { _state.update { it.copy(models = emptyList(), fetchError = bad, fetching = false) }; return }
    val my = ++seq
    fetchJob?.cancel()
    _state.update { it.copy(fetching = true, fetchError = "") }
    val addr = s.address.trim()
    fetchJob = scope.launch {
      try {
        val acct = accountArg(p, s.accountId)
        val r = cache.getOrLoad(ModelLogic.cacheKey(p.id, addr, acct ?: "default") + "|" + (formatArg(p, s.apiFormat) ?: ""), force) { api.models(p.id, addr.ifBlank { null }, formatArg(p, s.apiFormat), acct) }
        if (my != seq) return@launch
        _state.update {
          val models = r.models
          val sameProvider = p.id == prov.current
          val chosen = when {
            it.chosen in models -> it.chosen
            it.chosen.isNotEmpty() && models.isEmpty() && sameProvider -> it.chosen
            sameProvider && r.current in models -> r.current
            else -> ""
          }
          it.copy(
            fetching = false, models = models, fetchError = ModelLogic.listMessage(r), chosen = chosen,
            address = if (r.baseUrl.isNotBlank() && !dirty && it.address.isBlank() && ModelLogic.needsAddress(p)) r.baseUrl else it.address,
            efforts = r.efforts, effort = r.effort.orEmpty(),
          )
        }
      } catch (e: CancellationException) {
        throw e
      } catch (e: ApiException) {
        if (my != seq) return@launch
        _state.update { it.copy(fetching = false, models = emptyList(), fetchError = e.text) }
      }
    }
  }

  // ---- key

  fun saveKey(key: String, onSaved: () -> Unit = {}) {
    val p = _state.value.provider ?: return
    val k = key.trim()
    if (k.isEmpty()) { toast("Paste the key first."); return }
    val acct = _state.value.accountId
    _state.update { it.copy(savingKey = true) }
    scope.launch {
      try {
        api.setKey(p.id, k, accountArg(p, acct))
        val keyed = { x: ProviderInfo -> if (x.id != p.id) x else x.copy(hasKey = x.hasKey || acct == "default", accounts = x.accounts.map { a -> if (a.id == acct) a.copy(hasKey = true) else a }) }
        prov = prov.copy(providers = prov.providers.map(keyed))
        _state.update { s -> s.copy(savingKey = false, providers = s.providers.map(keyed)) }
        invalidate()
        toast("Key saved")
        onSaved()
        fetchModels(force = true)
      } catch (e: CancellationException) {
        throw e
      } catch (e: ApiException) {
        _state.update { it.copy(savingKey = false) }
        toast(e.text)
      }
    }
  }

  // ---- test connection (MODEL-13)

  fun testConnection() {
    val s = _state.value; val p = s.provider ?: return
    val bad = ModelLogic.validateAddress(s.address, ModelLogic.needsAddress(p), restricted)
    if (bad.isNotEmpty()) { _state.update { it.copy(testOk = false, testText = bad) }; return }
    _state.update { it.copy(testing = true, testOk = null, testText = "Testing…") }
    scope.launch {
      try {
        val r = api.modelTest(p.id, s.chosen.ifBlank { null }, s.address.trim().ifBlank { null }, formatArg(p, s.apiFormat), accountArg(p, s.accountId))
        _state.update { it.copy(testing = false, testOk = r.works, testText = r.message.ifBlank { if (r.works) "Connected." else "The test failed." }) }
      } catch (e: CancellationException) {
        throw e
      } catch (e: ApiException) {
        _state.update { it.copy(testing = false, testOk = false, testText = if (e.kind == ErrorKind.UnknownOp) "This server cannot test connections yet." else e.text) }
      }
    }
  }

  // ---- pins (MODEL-19)

  fun togglePin(model: String) {
    val s = _state.value; val p = s.provider ?: return
    val on = model !in s.pins
    scope.launch {
      try {
        val active = prof.profiles.firstOrNull { it.id == prof.active }
        var id = prof.active
        var note = if (on) "Pinned" else "Unpinned"
        if (on && (active == null || active.provider != p.id)) {
          api.select(provider = p.id, model = model, baseUrl = s.address.trim(), account = accountArg(p, s.accountId))
          id = api.profileCreate(ModelLogic.accountLabel(p, s.accountId).ifBlank { p.id })
          note = "Pinned to a new profile, " + p.name
        }
        api.profilePin(id, model, on)
        invalidate()
        prof = api.profiles()
        _state.update { it.copy(pins = ModelLogic.activePins(prof).toSet()) }
        toast(note)
      } catch (e: CancellationException) {
        throw e
      } catch (e: ApiException) {
        toast(e.text)
      }
    }
  }

  // ---- use (MODEL-23)

  /** Saves the choice. [onUsed] runs when it worked (the sheet closes). */
  fun use(onUsed: () -> Unit) {
    val s = _state.value; val p = s.provider ?: return
    if (s.chosen.isBlank()) return
    val bad = ModelLogic.validateAddress(s.address, ModelLogic.needsAddress(p), restricted)
    if (bad.isNotEmpty()) { _state.update { it.copy(addressError = bad) }; toast(bad); return }
    _state.update { it.copy(using = true) }
    scope.launch {
      try {
        api.select(provider = p.id, model = s.chosen, effort = if (s.efforts.isNotEmpty()) s.effort else null, baseUrl = s.address.trim(),
          apiFormat = if (p.takesFormat) s.apiFormat else null, account = if (p.takesAccounts) s.accountId else null)
        invalidate()
        _state.update { it.copy(using = false) }
        toast("Using " + s.chosen)
        onUsed()
      } catch (e: CancellationException) {
        throw e
      } catch (e: ApiException) {
        _state.update { it.copy(using = false) }
        toast(e.text)
      }
    }
  }
}

/**
 * The sheet "Add a custom provider" / "Edit custom provider": a name of the person's own ("My router", "Home Ollama"), the API format (Chat,
 * Responses or Claude), the address, a key and a model to start with. Test uses what is typed without saving anything.
 */
class CustomProviderController(
  private val api: ZoApi,
  private val scope: CoroutineScope,
  private val restricted: Boolean,
  private val toast: (String) -> Unit,
  private val invalidate: () -> Unit,
  /** the names of the custom providers that exist (the one being edited left out), for the "already named" check */
  private val otherNames: Collection<String> = emptyList(),
) {
  data class State(val error: String = "", val testText: String = "", val testOk: Boolean? = null, val busy: Boolean = false, val testing: Boolean = false)

  private val _state = MutableStateFlow(State())
  val state: StateFlow<State> = _state

  /** [id] is the provider being edited (the saved key is used when none is typed), null for a new one. */
  fun test(id: String?, format: ModelLogic.ApiFormat, address: String, key: String, model: String) {
    val bad = ModelLogic.validateNewEndpoint(address, restricted)
    if (bad.isNotEmpty()) { _state.update { it.copy(error = bad) }; return }
    _state.update { it.copy(error = "", testing = true, testOk = null, testText = "Testing…") }
    scope.launch {
      try {
        val r = api.customProviderTest(id, format.id, address.trim(), key.trim().ifBlank { null }, model.trim().ifBlank { null })
        _state.update { it.copy(testing = false, testOk = r.works, testText = r.message.ifBlank { if (r.works) "Connected." else "The test failed." }) }
      } catch (e: CancellationException) {
        throw e
      } catch (e: ApiException) {
        _state.update { it.copy(testing = false, testOk = false, testText = if (e.kind == ErrorKind.UnknownOp) "This server cannot test connections yet." else e.text) }
      }
    }
  }

  /** Adds (id null) or saves the provider. [onDone] gets its id. */
  fun save(id: String?, name: String, format: ModelLogic.ApiFormat, address: String, key: String, model: String, onDone: (String) -> Unit) {
    val badName = ModelLogic.validateName(name, "provider", otherNames)
    if (badName.isNotEmpty()) { _state.update { it.copy(error = badName) }; return }
    val bad = ModelLogic.validateNewEndpoint(address, restricted)
    if (bad.isNotEmpty()) { _state.update { it.copy(error = bad) }; return }
    _state.update { it.copy(error = "", busy = true) }
    scope.launch {
      try {
        val r = if (id == null) api.customProviderAdd(name.trim(), format.id, address.trim(), key.trim(), model.trim())
        else api.customProviderUpdate(id, name.trim(), format.id, address.trim(), key.trim().ifBlank { null }, model.trim())
        invalidate()
        _state.update { it.copy(busy = false) }
        toast(if (id == null) "Added " + name.trim() else "Saved " + name.trim())
        onDone(r.id.ifBlank { id.orEmpty() })
      } catch (e: CancellationException) {
        throw e
      } catch (e: ApiException) {
        _state.update { it.copy(busy = false, error = e.text) }
      }
    }
  }
}

/** Add, rename, test, hide and delete the accounts (API keys) of a provider. Every call says what went wrong in words, or "" when it worked. */
class AccountController(
  private val api: ZoApi,
  private val invalidate: () -> Unit,
) {
  private suspend fun run(f: suspend () -> Unit): String = try { f(); invalidate(); "" } catch (e: CancellationException) { throw e } catch (e: ApiException) { e.text }

  suspend fun add(provider: String, name: String, key: String, address: String, existing: Collection<String>): String {
    val bad = ModelLogic.validateName(name, "account", existing)
    if (bad.isNotEmpty()) return bad
    if (key.isBlank()) return "Paste the account's API key."
    return run { api.accountAdd(provider, name.trim(), key.trim(), address.trim().ifBlank { null }) }
  }

  suspend fun edit(provider: String, accountId: String, name: String, key: String, address: String?, existing: Collection<String>): String {
    val bad = ModelLogic.validateName(name, "account", existing)
    if (bad.isNotEmpty()) return bad
    return run { api.accountRename(provider, accountId, name.trim(), key.trim().ifBlank { null }, address?.trim()) }
  }

  suspend fun hide(provider: String, accountId: String, hidden: Boolean): String = run { api.accountHide(provider, accountId, hidden) }
  suspend fun remove(provider: String, accountId: String): String = run { api.accountRemove(provider, accountId) }

  /** Tests the account with its own key. Returns (works, message). */
  suspend fun test(provider: String, accountId: String, model: String? = null): Pair<Boolean, String> = try {
    val r = api.accountTest(provider, accountId, model)
    r.works to r.message.ifBlank { if (r.works) "Connected." else "The test failed." }
  } catch (e: CancellationException) {
    throw e
  } catch (e: ApiException) {
    false to e.text
  }
}
