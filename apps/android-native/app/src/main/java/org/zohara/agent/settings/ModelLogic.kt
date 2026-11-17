package org.zohara.agent.settings

import java.net.URI
import org.zohara.agent.core.net.CatalogGroup
import org.zohara.agent.core.net.CatalogModel
import org.zohara.agent.core.net.ModelsResp
import org.zohara.agent.core.net.Profile
import org.zohara.agent.core.net.ProfilesResp
import org.zohara.agent.core.net.ProviderInfo

/** The pure rules of the model menu, the model sheet and the Model page (MODEL-*). No Android, no network: unit tested. */
object ModelLogic {
  private val LOCAL_IDS = setOf("ollama", "lmstudio", "llamacpp", "vllm", "sglang", "claude-code")
  private val MAJOR_IDS = setOf("anthropic", "openai", "gemini")

  /** The three "Custom endpoint" providers, one per wire format. An older server does not send `custom`, so the ids are known too. */
  private val CUSTOM_IDS = setOf("custom", "custom-responses", "custom-claude")

  fun isCustom(p: ProviderInfo): Boolean = p.custom || p.id in CUSTOM_IDS

  /** A custom endpoint is nothing without its address, whatever an older server says in `needs_url`. */
  fun needsAddress(p: ProviderInfo): Boolean = p.needsUrl || isCustom(p)

  /** One wire format: the id the server takes (`api_format`), its name and the path it is sent to. */
  data class ApiFormat(val id: String, val name: String, val path: String, val providerId: String) {
    val label: String get() = "$name ($path)"
  }

  val FORMATS = listOf(
    ApiFormat("chat", "OpenAI Chat", "/v1/chat/completions", "custom"),
    ApiFormat("responses", "OpenAI Responses", "/v1/responses", "custom-responses"),
    ApiFormat("claude", "Claude format", "/v1/messages", "custom-claude"),
  )

  fun formatById(id: String?): ApiFormat? = FORMATS.firstOrNull { it.id == id }

  /** The wire format of a provider: what the server says, else known from the id (Anthropic speaks Claude, the rest Chat). */
  fun formatOf(p: ProviderInfo): ApiFormat = formatById(p.format) ?: FORMATS.firstOrNull { it.providerId == p.id }
    ?: if (p.id == "anthropic") FORMATS[2] else FORMATS[0]

  /** The format choice shown for a hosted provider (null when it takes none): its saved one, else its normal one. */
  fun chosenFormat(p: ProviderInfo): ApiFormat? = if (p.takesFormat) formatById(p.apiFormat) ?: formatOf(p) else null

  /**
   * The format a server answer points to: "This server speaks the OpenAI Responses format: pick "Custom endpoint: OpenAI Responses"". Null when
   * the message names none or names the one in use ([current]).
   */
  fun hintFormat(message: String, current: ApiFormat?): ApiFormat? {
    val m = Regex("Custom endpoint: (OpenAI Chat|OpenAI Responses|Claude format)").find(message) ?: return null
    return FORMATS.firstOrNull { it.name == m.groupValues[1] }?.takeIf { it != current }
  }

  fun isLocal(p: ProviderInfo): Boolean = p.local || p.id in LOCAL_IDS

  /**
   * MODEL-10: the providers the person may be offered. Restricted accounts get no local providers and no Claude Code; hidden ones are left
   * out except the one in use ([keep]).
   */
  fun shownProviders(all: List<ProviderInfo>, hidden: Collection<String>, keep: String?, restricted: Boolean): List<ProviderInfo> =
    all.filter { !(restricted && isLocal(it)) && (it.id == keep || it.id !in hidden) }

  /** The list of the sheet: also no Claude Code unless this admin's server has the sign-in ([claudeReady], MODEL-10). */
  fun sheetProviders(all: List<ProviderInfo>, hidden: Collection<String>, keep: String?, restricted: Boolean, claudeReady: Boolean): List<ProviderInfo> =
    shownProviders(all, hidden, keep, restricted).filter { restricted || claudeReady || it.id != "claude-code" }

  /** "N providers are hidden. Settings > Model and profiles has a switch for each." or null when none is hidden. */
  fun hiddenHint(all: List<ProviderInfo>, hidden: Collection<String>, keep: String?, restricted: Boolean): String? {
    val n = all.count { !(restricted && isLocal(it)) && it.id != keep && it.id in hidden }
    if (n == 0) return null
    return (if (n == 1) "1 provider is hidden." else "$n providers are hidden.") + " Settings > Model and profiles has a switch for each."
  }

  data class Group(val label: String, val items: List<ProviderInfo>)

  /** MODEL-09: Popular Providers, Cloud & Open-Weight, Local Models (This Computer), Custom Endpoint. Empty groups are left out. */
  fun groups(list: List<ProviderInfo>): List<Group> = listOf(
    Group("Popular Providers", list.filter { it.id in MAJOR_IDS }),
    Group("Cloud & Open-Weight", list.filter { it.id !in MAJOR_IDS && !isCustom(it) && !isLocal(it) }),
    Group("Local Models (This Computer)", list.filter { it.id !in MAJOR_IDS && !isCustom(it) && isLocal(it) }),
    Group("Custom Endpoint", list.filter { isCustom(it) }),
  ).filter { it.items.isNotEmpty() }

  /** The name of a provider in the picker: a tick after the ones that have a key (or need none). */
  fun providerLabel(p: ProviderInfo): String = p.name.ifBlank { p.id } + if (p.hasKey || isLocal(p)) " ✓" else ""

  fun hasKeyOrNone(p: ProviderInfo): Boolean = p.hasKey || isLocal(p)

  /** MODEL-12: the address the person sees prefilled: the saved one, only when it is not the normal one. */
  fun savedAddress(p: ProviderInfo?, currentProvider: String): String =
    if (p != null && p.id == currentProvider && p.baseUrl.isNotBlank() && p.baseUrl != p.defaultUrl) p.baseUrl else ""

  /** MODEL-12: the hint under the address field. */
  fun addressHint(p: ProviderInfo?, restricted: Boolean): String = when {
    p == null -> ""
    isCustom(p) -> "The address of a server that speaks ${formatOf(p).name}, for example http://192.168.1.20:8000/v1 or https://my-host.example/v1. " +
      "The app adds ${formatOf(p).path.removePrefix("/v1")}." + if (restricted) " Only public https addresses work here." else ""
    isLocal(p) || needsAddress(p) -> "Where the server runs. The default is right unless you changed its port."
    else -> "Only change this for a proxy or a regional endpoint." + if (p.defaultUrl.isNotBlank()) " Normal address: ${p.defaultUrl}" else ""
  }

  /** The sentence under the API format choice of a hosted provider. */
  fun formatHint(p: ProviderInfo): String =
    "The wire format this provider is asked in. Leave it on ${formatOf(p).name} unless the provider says otherwise."

  /** MODEL-12/MODEL-25: "" when the address is fine, else the sentence to show. Restricted accounts: a public https address only. */
  fun validateAddress(address: String, needs: Boolean, restricted: Boolean): String {
    val u = address.trim()
    if (u.isEmpty()) return if (needs) "Enter the address of the endpoint, for example https://api.example.com/v1" else ""
    val uri = try { URI(u) } catch (_: Exception) { null }
    val scheme = uri?.scheme?.lowercase()
    val host = uri?.host?.lowercase()
    if (uri == null || scheme == null || host.isNullOrEmpty()) return "That does not look like a web address. It should start with https://"
    if (restricted) {
      if (scheme != "https") return "Only https addresses work here."
      if (isPrivateHost(host)) return "Use a public address. Local and private addresses are not allowed on this server."
    } else if (scheme != "http" && scheme != "https") return "The address must start with http:// or https://"
    return ""
  }

  private val PRIVATE = Regex("^(0\\.|10\\.|127\\.|169\\.254\\.|192\\.168\\.|172\\.(1[6-9]|2\\d|3[01])\\.)")

  fun isPrivateHost(host: String): Boolean =
    host == "localhost" || host.startsWith("[") || !host.contains('.') || PRIVATE.containsMatchIn(host) ||
      host.endsWith(".local") || host.endsWith(".internal") || host.endsWith(".lan")

  /** MODEL-25 validation of the add-endpoint sheet. */
  fun validateNewEndpoint(address: String, restricted: Boolean): String {
    val u = address.trim()
    if (u.isEmpty()) return "Type the address first."
    if (!u.startsWith("http://", true) && !u.startsWith("https://", true)) return "The address should start with http:// or https://"
    return validateAddress(u, true, restricted)
  }

  /** MODEL-14: the models matching [filter], the pinned ones first. */
  fun orderModels(models: List<String>, pins: Set<String>, filter: String): List<String> {
    val q = filter.trim().lowercase()
    val hit = models.filter { q.isEmpty() || it.lowercase().contains(q) }
    return hit.filter { it in pins } + hit.filter { it !in pins }
  }

  /** MODEL-26: the hint of the "New profile" sheet (the desktop's words) */
  const val NEW_PROFILE_HINT = "Keeps the provider, model, address, effort and the API key that is in use now."

  /** The family of a Claude model id: opus, sonnet, haiku, fable, or the id itself (zo-core `models::family`). */
  fun family(id: String): String = id.removePrefix("claude-").substringBefore('-')

  /**
   * MODEL-08: (main, more) for the Claude Code list, as `models::split_main`: the first (newest) model of each family is main, the older
   * ones go under "More models". Only Claude Code has such a group; other providers keep a flat list.
   */
  fun splitMain(models: List<String>): Pair<List<String>, List<String>> {
    val seen = mutableSetOf<String>()
    val main = mutableListOf<String>(); val more = mutableListOf<String>()
    for (m in models) if (seen.add(family(m))) main += m else more += m
    return main to more
  }

  // ---- Claude Code catalog (the list comes from the server: zo-core data/claude-models.json) ----

  /** A row of the grouped Claude Code list: a group title or one model. */
  sealed interface CatalogRow {
    data class Header(val title: String) : CatalogRow
    data class Item(val model: CatalogModel) : CatalogRow
  }

  /** The window as people say it: 200000 -> "200K window", 1000000 -> "1M window"; "" when not known. */
  fun windowLabel(tokens: Long): String = when {
    tokens <= 0 -> ""
    tokens >= 1_000_000 -> "${tokens / 1_000_000}M window"
    else -> "${tokens / 1000}K window"
  }

  /** The grey line under a model name: its id (not for Default), its note and its window. */
  fun catalogSub(m: CatalogModel): String =
    listOf(if (m.kind == "default") "" else m.id, m.note, windowLabel(m.context)).filter { it.isNotBlank() }.joinToString(" · ")

  /** The rows of the grouped list for a search: groups with no match are left out; the search looks at the label, the id and the aliases. */
  fun catalogRows(catalog: List<CatalogGroup>, filter: String): List<CatalogRow> {
    val q = filter.trim().lowercase()
    return catalog.flatMap { g ->
      val hit = g.models.filter { q.isEmpty() || (it.label + " " + it.id + " " + it.aliases.joinToString(" ")).lowercase().contains(q) }
      if (hit.isEmpty()) emptyList() else listOf<CatalogRow>(CatalogRow.Header(g.title)) + hit.map { CatalogRow.Item(it) }
    }
  }

  /** Every id of the catalog. */
  fun catalogIds(catalog: List<CatalogGroup>): Set<String> = catalog.flatMap { g -> g.models.map { it.id } }.toSet()

  /** The id typed by hand ("Other model id…"): offered when it is not one of the catalog's or the list's, and looks like an id. */
  fun typedModelId(catalog: List<CatalogGroup>, models: List<String>, filter: String): String {
    val q = filter.trim()
    if (q.isEmpty() || !validModelId(q)) return ""
    return if (q in catalogIds(catalog) || models.any { it.equals(q, ignoreCase = true) }) "" else q
  }

  /** The same rule as the server (zo-core `models::valid_model_id`): letters, digits and `. _ - : /`, an optional `[1m]`, at most 100 characters. */
  fun validModelId(id: String): Boolean {
    val t = id.trim()
    val core = t.removeSuffix("[1m]")
    return core.isNotEmpty() && t.length <= 100 && core.all { it.isLetterOrDigit() && it.code < 128 || it in "._-:/" }
  }

  /** MODEL-14/17: typed text that is not in the list is offered as "Use “x” as the model name". */
  fun customOffer(models: List<String>, filter: String): String {
    val q = filter.trim()
    return if (q.isNotEmpty() && models.none { it.equals(q, ignoreCase = true) }) q else ""
  }

  /** The pinned models of the profile in use. */
  fun activePins(prof: ProfilesResp): List<String> = prof.profiles.firstOrNull { it.id == prof.active }?.pinned.orEmpty()

  /** MODEL-07: the label of an effort level. */
  fun effortLabel(level: String?): String = when (level?.lowercase()) {
    null, "" -> "Default"
    "low" -> "Low"
    "medium" -> "Medium"
    "high" -> "High"
    "xhigh" -> "Extra high"
    "max" -> "Max"
    else -> level.replaceFirstChar { it.uppercase() }
  }

  /** MODEL-37: "<provider> · <model or 'no model yet'>[ · N pinned]". */
  fun profileLine(p: Profile, providerName: String): String {
    val base = providerName.ifBlank { p.provider } + " · " + p.model.ifBlank { "no model yet" }
    return if (p.pinned.isNotEmpty()) base + " · ${p.pinned.size} pinned" else base
  }

  /** MODEL-18: the read-only picture setting of a profile, or null when the server did not send it. */
  fun visionLine(p: Profile): String? = when (p.vision) {
    true -> "Pictures: this model can see pictures (set on the desktop)"
    false -> "Pictures: this model cannot see pictures (set on the desktop)"
    null -> null
  }

  /** MODEL-29: the sub line of a provider switch. */
  fun providerSub(p: ProviderInfo, pins: Int, inUse: Boolean, claudeSignedIn: Boolean = true): String {
    val base = when {
      p.id == "claude-code" -> "Uses your Claude Code sign-in"
      isLocal(p) -> "Runs on this computer, no key"
      isCustom(p) && p.hasKey -> "Key saved · ${formatOf(p).label}"
      isCustom(p) -> "No key yet · ${formatOf(p).label}"
      p.hasKey -> "Key saved"
      else -> "No key yet"
    }
    return base + (if (pins > 0) " · $pins pinned" else "") + (if (inUse) " · in use, always shown" else "")
  }

  /** The pins of every profile, for MODEL-28. */
  data class PinRow(val profileId: String, val profileName: String, val provider: String, val model: String)

  fun pinRows(prof: ProfilesResp, providerName: (String) -> String): List<PinRow> =
    prof.profiles.flatMap { p -> p.pinned.map { PinRow(p.id, p.name, providerName(p.provider), it) } }

  /** MODEL-01/06: the groups of the model menu: one per profile, with its pinned models (or its one model). */
  data class MenuGroup(val profileId: String, val header: String, val provider: String, val models: List<String>)

  fun menuGroups(prof: ProfilesResp, providerName: (String) -> String): List<MenuGroup> = prof.profiles.mapNotNull { p ->
    val ms = if (p.pinned.isNotEmpty()) p.pinned else if (p.model.isNotBlank()) listOf(p.model) else emptyList()
    if (ms.isEmpty()) null else MenuGroup(p.id, p.name + " · " + providerName(p.provider).ifBlank { p.provider }, p.provider, ms)
  }

  /** MODEL-02: true when the current model is not one of the models the menu lists, so "In use now" is shown. */
  fun needsInUseNow(groups: List<MenuGroup>, activeProfile: String, currentModel: String): Boolean =
    currentModel.isNotBlank() && groups.none { it.profileId == activeProfile && currentModel in it.models }

  fun cacheKey(provider: String, baseUrl: String) = provider + "|" + baseUrl.trim()

  /** Did the answer of `models` carry a list we can use (an empty list with no error means the provider returned none). */
  fun listMessage(r: ModelsResp): String = when {
    !r.error.isNullOrBlank() -> r.error
    r.models.isEmpty() -> "The provider returned no models. You can still type a model name above."
    else -> ""
  }
}
