package org.zohara.agent.settings

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.jsonPrimitive
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment
import org.zohara.agent.core.net.ModelsResp
import org.zohara.agent.core.store.TtlCache

@RunWith(RobolectricTestRunner::class)
@org.robolectric.annotation.Config(sdk = [35])
class ModelControllerTest {
  private lateinit var fake: FakeServer
  private val toasts = java.util.concurrent.CopyOnWriteArrayList<String>()
  private var invalidated = 0
  private lateinit var local: LocalSettings

  private fun str(o: JsonObject, k: String) = o[k]?.jsonPrimitive?.content

  private val providersJson = """{"ok":true,"current":"anthropic","hidden_providers":["groq"],"providers":[
    {"id":"anthropic","name":"Anthropic (Claude)","has_key":true,"default_url":"https://api.anthropic.com/v1","base_url":"https://api.anthropic.com/v1"},
    {"id":"openai","name":"OpenAI","has_key":false,"default_url":"https://api.openai.com/v1","base_url":"https://api.openai.com/v1","format":"chat","takes_format":true,"api_format":"responses"},
    {"id":"groq","name":"Groq","has_key":false,"takes_accounts":true,"accounts":[
      {"id":"default","name":"Default","default":true,"has_key":true},{"id":"groq-2","name":"Groq 2","has_key":true},{"id":"groq-3","name":"Groq 3","has_key":false,"hidden":true}]},
    {"id":"ollama","name":"Ollama","local":true,"needs_url":true,"default_url":"http://127.0.0.1:11434/v1"},
    {"id":"custom","name":"Custom endpoint: OpenAI Chat (/v1/chat/completions)","needs_url":true,"custom":true,"format":"chat"},
    {"id":"custom-responses","name":"Custom endpoint: OpenAI Responses (/v1/responses)","needs_url":true,"custom":true,"format":"responses"},
    {"id":"custom-claude","name":"Custom endpoint: Claude format (/v1/messages)","needs_url":true,"custom":true,"format":"claude"},
    {"id":"custom:my-router","name":"My router","needs_url":true,"custom":true,"group":"custom","format":"chat","base_url":"https://router.example.com/v1","has_key":true,"takes_accounts":true,
      "accounts":[{"id":"default","name":"Default","default":true,"has_key":true}]}]}"""
  private val profilesJson = """{"ok":true,"active":"p1","profiles":[{"id":"p1","name":"Work","provider":"anthropic","model":"claude-sonnet-5","pinned":["claude-sonnet-5"]}]}"""

  @Before fun up() {
    fake = FakeServer()
    local = LocalSettings(RuntimeEnvironment.getApplication().getSharedPreferences("t", 0))
    fake.on("providers", providersJson)
    fake.on("profiles", profilesJson)
    fake.on("status", """{"ok":true,"mode":"Auto","model":"claude-sonnet-5","provider":"anthropic","ready":true,"version":"0.1.0"}""")
    fake.on("settings_get", """{"ok":true,"hidden_providers":["openrouter"],"provider_prefs":{},"updated":1}""")
    fake.on("models", """{"ok":true,"provider":"anthropic","models":["claude-sonnet-5","claude-haiku-5","claude-opus-5"],"current":"claude-sonnet-5","efforts":["low","high"],"effort":"high"}""")
  }
  @After fun down() { fake.shutdown() }

  private fun controller(restricted: Boolean = false, admin: Boolean = true, cache: TtlCache<String, ModelsResp> = TtlCache()) =
    ModelSheetController(
      fake.api, CoroutineScope(Dispatchers.Default), restricted, admin, cache, HiddenStore(fake.api, local), { toasts.add(it) }, { invalidated++ },
    )

  private fun waitFor(c: ModelSheetController, cond: (ModelSheetState) -> Boolean): ModelSheetState = runBlocking { withTimeout(8000) { c.state.first(cond) } }

  @Test fun startLoadsEverythingAndFetchesTheModelsOfTheCurrentProvider() {
    fake.on("/api/account/claude_status", """{"ok":true,"present":true,"source":"own","set_at":1794509000,"admin":true}""")
    val c = controller()
    c.start()
    val s = waitFor(c) { !it.loading && !it.fetching && it.models.isNotEmpty() }
    assertEquals("anthropic", s.providerId)
    assertEquals("claude-sonnet-5", s.chosen)
    assertEquals(setOf("claude-sonnet-5"), s.pins)
    assertEquals(listOf("low", "high"), s.efforts); assertEquals("high", s.effort)
    // hidden: the server's settings and the providers answer, merged; the one in use stays
    assertFalse(s.providers.any { it.id == "groq" })
    assertTrue(s.hiddenHint!!.startsWith("2 providers are hidden") || s.hiddenHint!!.startsWith("1 provider is hidden"))
    assertTrue(s.claudeLine!!.startsWith("Claude Code (your subscription) is ready on the server."))
    assertEquals("", s.address) // the normal address is not shown as a typed one
  }

  @Test fun restrictedAccountsNeverAskForClaudeAndGetNoLocalProviders() {
    val c = controller(restricted = true, admin = false)
    c.start()
    val s = waitFor(c) { !it.loading && !it.fetching }
    assertFalse("/api/account/claude_status" in fake.ops())
    assertFalse(s.providers.any { it.id == "ollama" })
    assertNull(s.claudeLine)
  }

  @Test fun aFailedLoadShowsTheSentence() {
    fake.answers.remove("providers")
    val c = controller()
    c.start()
    val s = waitFor(c) { !it.loading }
    assertTrue(s.loadError!!.isNotBlank())
  }

  @Test fun changingProviderListsItsModelsAndResetsTheChoice() {
    fake.on("models") { """{"ok":true,"provider":"${str(it, "provider")}","models":["gpt-4o","o3"],"current":""}""" }
    val c = controller(); c.start(); waitFor(c) { !it.loading && !it.fetching }
    c.selectProvider("openai")
    val s = waitFor(c) { it.providerId == "openai" && !it.fetching && it.models.isNotEmpty() }
    assertEquals(listOf("gpt-4o", "o3"), s.models)
    assertEquals("", s.chosen)
    assertEquals("openai", str(fake.body("models"), "provider"))
  }

  @Test fun aCustomEndpointWaitsForTheAddressBeforeFetching() {
    val c = controller(); c.start(); waitFor(c) { !it.loading && !it.fetching }
    val before = fake.ops().count { it == "models" }
    c.selectProvider("custom")
    Thread.sleep(300)
    assertEquals(before, fake.ops().count { it == "models" })
    assertEquals("", c.state.value.fetchError)
    // a bad address is reported at once and nothing is asked
    c.setAddress("my host"); c.addressDone()
    assertTrue(c.state.value.addressError.startsWith("That does not look like a web address"))
    c.setAddress("https://my-host.example/v1"); c.addressDone()
    waitFor(c) { !it.fetching && it.models.isNotEmpty() }
    assertEquals("https://my-host.example/v1", str(fake.body("models"), "base_url"))
  }

  @Test fun aFailedFetchKeepsTheFormAndOffersRetry() {
    fake.on("models", """{"ok":true,"provider":"anthropic","models":[],"error":"The provider did not accept the key (401). Check it and try again."}""")
    val c = controller(); c.start()
    val s = waitFor(c) { !it.loading && !it.fetching && it.fetchError.isNotEmpty() }
    assertEquals("The provider did not accept the key (401). Check it and try again.", s.fetchError)
    assertTrue(s.models.isEmpty())
    // Retry asks again and a good answer clears the error
    fake.on("models", """{"ok":true,"provider":"anthropic","models":["m1"],"current":""}""")
    c.fetchModels(force = true)
    val s2 = waitFor(c) { !it.fetching && it.models.isNotEmpty() }
    assertEquals("", s2.fetchError)
  }

  @Test fun noModelsAndNoErrorSaysTheProviderReturnedNone() {
    fake.on("models", """{"ok":true,"provider":"anthropic","models":[]}""")
    val c = controller(); c.start()
    assertEquals("The provider returned no models. You can still type a model name above.", waitFor(c) { !it.loading && !it.fetching }.fetchError)
  }

  @Test fun modelListsAreCachedAndForcedOnRetry() {
    val cache = TtlCache<String, ModelsResp>()
    val c = controller(cache = cache); c.start(); waitFor(c) { !it.loading && !it.fetching && it.models.isNotEmpty() }
    assertEquals(1, fake.ops().count { it == "models" })
    c.fetchModels(); waitFor(c) { !it.fetching }; Thread.sleep(100)
    assertEquals(1, fake.ops().count { it == "models" })
    c.fetchModels(force = true); waitFor(c) { !it.fetching }; Thread.sleep(100)
    assertEquals(2, fake.ops().count { it == "models" })
  }

  @Test fun useThisModelSendsSelectWithEffortAndAddress() {
    fake.on("select", """{"ok":true,"provider":"anthropic","model":"claude-opus-5"}""")
    val c = controller(); c.start(); waitFor(c) { !it.loading && !it.fetching && it.models.isNotEmpty() }
    c.choose("claude-opus-5"); c.setEffort("low")
    var used = false
    c.use { used = true }
    waitFor(c) { !it.using }; Thread.sleep(100)
    assertTrue(used)
    val b = fake.body("select")
    assertEquals("anthropic", str(b, "provider")); assertEquals("claude-opus-5", str(b, "model")); assertEquals("low", str(b, "effort")); assertEquals("", str(b, "base_url"))
    assertTrue(toasts.contains("Using claude-opus-5")); assertTrue(invalidated > 0)
  }

  @Test fun useIsRefusedWithABadAddressForARestrictedAccount() {
    fake.on("select", """{"ok":true}""")
    val c = controller(restricted = true, admin = false); c.start(); waitFor(c) { !it.loading && !it.fetching }
    c.selectProvider("custom"); c.setAddress("http://192.168.1.20:8000/v1"); c.choose("m")
    c.use { }
    Thread.sleep(200)
    assertFalse("select" in fake.ops())
    assertEquals("Only https addresses work here.", c.state.value.addressError)
  }

  @Test fun serverRefusalOfSelectIsShownAsAToast() {
    fake.on("select", """{"ok":false,"error":"A restricted account can only use a model server with a public https address."}""")
    val c = controller(); c.start(); waitFor(c) { !it.loading && !it.fetching && it.models.isNotEmpty() }
    c.use { }
    waitFor(c) { !it.using }; Thread.sleep(100)
    assertTrue(toasts.contains("A restricted account can only use a model server with a public https address."))
  }

  @Test fun savingAKeySendsItOnceAndMarksTheProvider() {
    fake.on("set_key", """{"ok":true}""")
    val c = controller(); c.start(); waitFor(c) { !it.loading && !it.fetching }
    c.selectProvider("openai"); waitFor(c) { it.providerId == "openai" && !it.fetching }
    c.saveKey("  sk-secret  ")
    waitFor(c) { it.provider?.hasKey == true && !it.savingKey }
    assertEquals("sk-secret", str(fake.body("set_key"), "key")); assertEquals("openai", str(fake.body("set_key"), "provider"))
    assertTrue(toasts.contains("Key saved"))
    assertTrue(c.state.value.toString().contains("sk-secret").not()) // the key is never kept in the state
  }

  @Test fun anEmptyKeyIsNotSent() {
    val c = controller(); c.start(); waitFor(c) { !it.loading && !it.fetching }
    c.saveKey("   ")
    assertEquals("Paste the key first.", toasts.last())
    assertFalse("set_key" in fake.ops())
  }

  @Test fun testConnectionShowsTheServersMessage() {
    fake.on("model_test", """{"ok":true,"works":true,"message":"Connected. 3 models available."}""")
    val c = controller(); c.start(); waitFor(c) { !it.loading && !it.fetching }
    c.testConnection()
    val s = waitFor(c) { !it.testing && it.testOk != null }
    assertEquals(true, s.testOk); assertEquals("Connected. 3 models available.", s.testText)
    assertEquals("claude-sonnet-5", str(fake.body("model_test"), "model"))
    fake.on("model_test", """{"ok":true,"works":false,"message":"The provider did not accept the key (401)."}""")
    c.testConnection()
    assertEquals(false, waitFor(c) { !it.testing && it.testOk == false }.testOk)
  }

  @Test fun testConnectionOnAnOldServerSaysSo() {
    fake.answers.remove("model_test")
    val c = controller(); c.start(); waitFor(c) { !it.loading && !it.fetching }
    c.testConnection()
    assertEquals("This server cannot test connections yet.", waitFor(c) { !it.testing && it.testOk == false }.testText)
  }

  @Test fun pinningToAnotherProviderCreatesAProfileFirst() {
    fake.on("select", """{"ok":true}""")
    fake.on("profile_create", """{"ok":true,"id":"p9"}""")
    fake.on("profile_pin", """{"ok":true}""")
    fake.on("models") { """{"ok":true,"provider":"openai","models":["gpt-4o"],"current":""}""" }
    val c = controller(); c.start(); waitFor(c) { !it.loading && !it.fetching }
    c.selectProvider("openai"); waitFor(c) { it.providerId == "openai" && !it.fetching && it.models.isNotEmpty() }
    c.togglePin("gpt-4o")
    Thread.sleep(500)
    val ops = fake.ops()
    val i = ops.indexOf("select")
    assertTrue(i >= 0)
    assertEquals(listOf("select", "profile_create", "profile_pin"), ops.drop(i).take(3))
    assertEquals("OpenAI", str(fake.body("profile_create"), "name"))
    assertEquals("p9", str(fake.body("profile_pin"), "id")); assertEquals(true, fake.body("profile_pin")["on"]?.jsonPrimitive?.booleanOrNull)
    assertTrue(toasts.contains("Pinned to a new profile, OpenAI"))
  }

  @Test fun pinningOnTheActiveProfileJustPinsAndUnpinning() {
    fake.on("profile_pin", """{"ok":true}""")
    val c = controller(); c.start(); waitFor(c) { !it.loading && !it.fetching && it.models.isNotEmpty() }
    c.togglePin("claude-haiku-5"); Thread.sleep(400)
    assertFalse("profile_create" in fake.ops())
    assertEquals("p1", str(fake.body("profile_pin"), "id"))
    assertTrue(toasts.contains("Pinned"))
    c.togglePin("claude-sonnet-5"); Thread.sleep(400) // already pinned: this is an unpin
    assertEquals(false, fake.body("profile_pin")["on"]?.jsonPrimitive?.booleanOrNull)
    assertTrue(toasts.contains("Unpinned"))
  }

  // ---- hidden providers (MODEL-29)
  @Test fun hiddenProvidersGoToTheServer() = runBlocking {
    fake.on("settings_set", """{"ok":true,"hidden_providers":["groq"],"provider_prefs":{},"updated":2}""")
    val store = HiddenStore(fake.api, local)
    assertEquals(listOf("openrouter"), store.get())
    store.set(listOf("groq"))
    assertEquals(listOf("groq"), fake.body("settings_set")["hidden_providers"]!!.let { (it as kotlinx.serialization.json.JsonArray).map { e -> e.jsonPrimitive.content } })
    assertTrue(store.onServer); assertEquals(emptyList<String>(), local.hiddenProviders())
  }

  @Test fun anOldServerKeepsTheChoiceOnThePhone() = runBlocking {
    fake.answers.remove("settings_get")
    val store = HiddenStore(fake.api, local)
    assertEquals(emptyList<String>(), store.get())
    assertFalse(store.onServer)
    store.set(listOf("groq", "openai"))
    assertEquals(listOf("groq", "openai"), local.hiddenProviders())
    assertEquals(listOf("groq", "openai"), store.get())
    assertFalse("settings_set" in fake.ops())
  }

  // ---- add or edit a custom provider
  private fun provider(c: CustomProviderController, id: String?, name: String, f: ModelLogic.ApiFormat, url: String, key: String, model: String): String? {
    var got: String? = null
    c.save(id, name, f, url, key, model) { got = it }
    return runCatching { runBlocking { withTimeout(5000) { while (got == null) kotlinx.coroutines.delay(20) } }; got }.getOrNull()
  }

  @Test fun addingACustomProviderSendsTheNameFormatAddressKeyAndModelInOneCall() {
    fake.on("custom_provider_add", """{"ok":true,"id":"custom:home-gpu","custom_provider":{"id":"custom:home-gpu","name":"Home GPU","format":"responses","base_url":"http://192.168.1.20:8000/v1"}}""")
    val c = CustomProviderController(fake.api, CoroutineScope(Dispatchers.Default), false, { toasts.add(it) }, { invalidated++ })
    assertEquals("custom:home-gpu", provider(c, null, " Home GPU ", ModelLogic.FORMATS[1], "http://192.168.1.20:8000/v1", " sk-x ", " llama3 "))
    assertEquals(listOf("custom_provider_add"), fake.ops())
    val b = fake.body("custom_provider_add")
    assertEquals(listOf("Home GPU", "responses", "http://192.168.1.20:8000/v1", "sk-x", "llama3"), listOf("name", "format", "base_url", "key", "default_model").map { str(b, it) })
    assertTrue(toasts.contains("Added Home GPU")); assertTrue(invalidated > 0)
  }

  @Test fun editingACustomProviderUpdatesItAndAnEmptyKeyKeepsTheSavedOne() {
    fake.on("custom_provider_update", """{"ok":true,"id":"custom:my-router"}""")
    val c = CustomProviderController(fake.api, CoroutineScope(Dispatchers.Default), false, { toasts.add(it) }, { invalidated++ })
    assertEquals("custom:my-router", provider(c, "custom:my-router", "Fast router", ModelLogic.FORMATS[2], "https://router.example.com/v2", "", ""))
    val b = fake.body("custom_provider_update")
    assertEquals(listOf("custom:my-router", "Fast router", "claude", "https://router.example.com/v2"), listOf("id", "name", "format", "base_url").map { str(b, it) })
    assertFalse("an empty key is not sent, so the saved one stays", "key" in b)
    assertTrue(toasts.contains("Saved Fast router"))
  }

  @Test fun theNameAndTheAddressAreCheckedBeforeAnythingIsSent() {
    val c = CustomProviderController(fake.api, CoroutineScope(Dispatchers.Default), true, { }, { }, otherNames = listOf("My router"))
    val f = ModelLogic.FORMATS[0]
    c.save(null, "x", f, "", "", "") { }
    assertEquals("Type the address first.", c.state.value.error)
    c.save(null, "x", f, "192.168.1.1/v1", "", "") { }
    assertEquals("The address should start with http:// or https://", c.state.value.error)
    c.save(null, "", f, "https://api.example.com/v1", "", "") { }
    assertEquals("Give the provider a name.", c.state.value.error)
    c.save(null, "my ROUTER", f, "https://api.example.com/v1", "", "") { }
    assertEquals("There is already a provider named \"my ROUTER\".", c.state.value.error)
    c.save(null, "a".repeat(41), f, "https://api.example.com/v1", "", "") { }
    assertEquals("The provider name can have at most 40 characters.", c.state.value.error)
    c.save(null, "bad\nname", f, "https://api.example.com/v1", "", "") { }
    assertEquals("The provider name cannot have control characters.", c.state.value.error)
    assertTrue(fake.ops().isEmpty())
  }

  @Test fun aServerRefusalIsShownInTheForm() {
    fake.answers["custom_provider_add"] = { """{"ok":false,"error":"Only public https addresses work for this account."}""" }
    val c = CustomProviderController(fake.api, CoroutineScope(Dispatchers.Default), false, { }, { })
    c.save(null, "Mine", ModelLogic.FORMATS[0], "http://127.0.0.1:8000/v1", "", "") { }
    val s = runBlocking { withTimeout(5000) { c.state.first { it.error.isNotBlank() && !it.busy } } }
    assertEquals("Only public https addresses work for this account.", s.error)
  }

  @Test fun testingACustomProviderSendsWhatIsTypedAndSavesNothing() {
    fake.on("custom_provider_test", """{"ok":true,"works":true,"message":"Connected to this server: 2 models available."}""")
    val c = CustomProviderController(fake.api, CoroutineScope(Dispatchers.Default), false, { }, { })
    c.test(null, ModelLogic.FORMATS[1], "https://my-host.example/v1", "sk-typed", "m1")
    runBlocking { withTimeout(5000) { c.state.first { it.testOk != null } } }
    assertEquals("Connected to this server: 2 models available.", c.state.value.testText)
    assertEquals(listOf("custom_provider_test"), fake.ops())
    val b = fake.body("custom_provider_test")
    assertEquals(listOf("responses", "https://my-host.example/v1", "sk-typed", "m1"), listOf("format", "base_url", "key", "model").map { str(b, it) })
    assertFalse("id" in b)
    // a saved provider is tested by id, with its saved key when none is typed
    fake.requests.clear()
    c.test("custom:my-router", ModelLogic.FORMATS[0], "https://router.example.com/v1", "", "")
    runBlocking { withTimeout(5000) { c.state.first { it.testOk != null } } }
    assertEquals("custom:my-router", str(fake.body("custom_provider_test"), "id")); assertFalse("key" in fake.body("custom_provider_test"))
  }

  @Test fun aTestThatAnswersWithTheFormatHintOffersTheOtherFormatAndTheNamedProviderChangesItsOwnFormat() {
    fake.on("model_test", """{"ok":true,"works":false,"message":"This server speaks the OpenAI Responses format: set the API format of this provider to OpenAI Responses."}""")
    fake.on("custom_provider_update", """{"ok":true,"id":"custom:my-router"}""")
    val c = controller(); c.start(); waitFor(c) { !it.loading && !it.fetching }
    c.selectProvider("custom:my-router")
    c.testConnection()
    val s = waitFor(c) { it.testOk == false }
    assertEquals("responses", s.hintedFormat?.id)
    c.switchToFormat(s.hintedFormat!!)
    runBlocking { withTimeout(5000) { while ("custom_provider_update" !in fake.ops()) kotlinx.coroutines.delay(20) } }
    assertEquals(listOf("custom:my-router", "responses"), listOf("id", "format").map { str(fake.body("custom_provider_update"), it) })
    runBlocking { withTimeout(5000) { while (invalidated == 0 || fake.ops().count { it == "providers" } < 2) kotlinx.coroutines.delay(20) } }   // the sheet loads again
    waitFor(c) { !it.loading && it.providerId == "custom:my-router" }
  }

  @Test fun anOldServerWithTheFixedCustomEndpointsStillSwitchesToTheEndpointOfThatFormat() {
    fake.on("model_test", """{"ok":true,"works":false,"message":"This server speaks the OpenAI Responses format: set the API format of this provider to OpenAI Responses."}""")
    val c = controller(); c.start(); waitFor(c) { !it.loading && !it.fetching }
    c.selectProvider("custom"); c.setAddress("https://router.example.com/v1")
    c.testConnection()
    val s = waitFor(c) { it.testOk == false }
    c.switchToFormat(s.hintedFormat!!)
    val t = waitFor(c) { it.providerId == "custom-responses" }
    assertEquals("https://router.example.com/v1", t.address)
  }

  // ---- accounts in the model sheet
  /** The sample server hides Groq; the account tests need it in the list. */
  private fun showGroq() { fake.on("providers", providersJson.replace("\"hidden_providers\":[\"groq\"]", "\"hidden_providers\":[]")); fake.on("settings_get", """{"ok":true,"hidden_providers":[],"provider_prefs":{},"updated":1}""") }

  @Test fun theSheetOffersTheAccountsOfAProviderAndAsksWithTheOneChosen() {
    showGroq()
    fake.on("select", """{"ok":true}"""); fake.on("model_test", """{"ok":true,"works":true,"message":"ok"}"""); fake.on("set_key", """{"ok":true}""")
    val c = controller(); c.start(); waitFor(c) { !it.loading && !it.fetching }
    c.selectProvider("groq")
    var s = waitFor(c) { it.providerId == "groq" }
    assertEquals("default", s.accountId)
    assertEquals(listOf("default", "groq-2"), s.accounts.map { it.id })   // Groq 3 is hidden
    assertEquals("Groq · Default", s.accountLabel)
    waitFor(c) { !it.fetching }
    assertFalse("the Default account sends no account", "account" in fake.body("models"))
    c.selectAccount("groq-2")
    s = waitFor(c) { it.accountId == "groq-2" && !it.fetching }
    assertEquals("Groq · Groq 2", s.accountLabel)
    assertEquals("groq-2", str(fake.body("models"), "account"))
    c.choose("m1"); c.testConnection(); waitFor(c) { it.testOk == true }
    assertEquals("groq-2", str(fake.body("model_test"), "account"))
    c.saveKey("gk-new")
    runBlocking { withTimeout(5000) { while ("set_key" !in fake.ops()) kotlinx.coroutines.delay(20) } }
    assertEquals(listOf("groq", "gk-new", "groq-2"), listOf("provider", "key", "account").map { str(fake.body("set_key"), it) })
    c.use { }
    runBlocking { withTimeout(5000) { while ("select" !in fake.ops()) kotlinx.coroutines.delay(20) } }
    assertEquals(listOf("groq", "m1", "groq-2"), listOf("provider", "model", "account").map { str(fake.body("select"), it) })
    // an account that the server does not know falls back to the Default one
    c.selectAccount("gone"); assertEquals("default", c.state.value.accountId)
    // another provider starts on its Default account
    c.selectProvider("anthropic"); assertEquals("default", c.state.value.accountId)
  }

  @Test fun theKeyIsPerAccountSoSavingOneDoesNotMarkTheOthers() {
    showGroq()
    fake.on("set_key", """{"ok":true}""")
    val c = controller(); c.start(); waitFor(c) { !it.loading && !it.fetching }
    c.selectProvider("groq"); c.selectAccount("groq-3")   // hidden, but the person may still pick it by an old selection
    assertEquals("groq-3", c.state.value.accountId)
    assertFalse(ModelLogic.accountHasKey(c.state.value.provider!!, "groq-3"))
    c.saveKey("k3")
    val s = waitFor(c) { !it.savingKey && ModelLogic.accountHasKey(it.provider!!, "groq-3") }
    assertTrue(s.provider!!.accounts.first { it.id == "groq-2" }.hasKey)
    assertFalse("the provider's own flag is the Default account's", s.provider!!.hasKey)
  }

  @Test fun pinningWithAnotherAccountMakesAProfileNamedWithTheAccount() {
    showGroq()
    fake.on("select", """{"ok":true}"""); fake.on("profile_create", """{"ok":true,"id":"p9"}"""); fake.on("profile_pin", """{"ok":true}""")
    val c = controller(); c.start(); waitFor(c) { !it.loading && !it.fetching }
    c.selectProvider("groq"); c.selectAccount("groq-2"); waitFor(c) { !it.fetching }
    c.togglePin("m1")
    runBlocking { withTimeout(5000) { while ("profile_pin" !in fake.ops()) kotlinx.coroutines.delay(20) } }
    assertEquals("groq-2", str(fake.body("select"), "account"))
    assertEquals("Groq · Groq 2", str(fake.body("profile_create"), "name"))
  }

  @Test fun theAccountControllerChecksNamesAndKeysAndSendsTheOps() = runBlocking {
    fake.on("account_add", """{"ok":true,"id":"groq-4","accounts":[]}"""); fake.on("account_rename", """{"ok":true,"accounts":[]}""")
    fake.on("account_hide", """{"ok":true,"accounts":[]}"""); fake.on("account_remove", """{"ok":true,"accounts":[]}""")
    fake.on("account_test", """{"ok":true,"works":false,"message":"The provider answered 401: bad key"}""")
    var cleared = 0
    val ctl = AccountController(fake.api) { cleared++ }
    assertEquals("Give the account a name.", ctl.add("groq", " ", "k", "", listOf("Default")))
    assertEquals("There is already a account named \"default\".", ctl.add("groq", "default", "k", "", listOf("Default")))
    assertEquals("Paste the account's API key.", ctl.add("groq", "Groq 4", " ", "", listOf("Default")))
    assertTrue(fake.ops().isEmpty())
    assertEquals("", ctl.add("groq", " Groq 4 ", " k4 ", "", listOf("Default", "Groq 2")))
    assertEquals(listOf("groq", "Groq 4", "k4"), listOf("provider", "name", "key").map { str(fake.body("account_add"), it) })
    assertFalse("no address typed, none sent", "base_url" in fake.body("account_add"))
    assertEquals("", ctl.edit("groq", "groq-2", "Work", "", null, listOf("Default")))
    assertFalse("key" in fake.body("account_rename")); assertEquals("groq-2", str(fake.body("account_rename"), "id"))
    assertEquals("", ctl.hide("groq", "groq-2", true)); assertEquals(true, fake.body("account_hide")["hidden"]?.jsonPrimitive?.booleanOrNull)
    assertEquals("", ctl.remove("groq", "groq-2")); assertEquals(true, fake.body("account_remove")["confirm"]?.jsonPrimitive?.booleanOrNull)
    assertEquals(false to "The provider answered 401: bad key", ctl.test("groq", "groq-2"))
    assertTrue(cleared >= 4)
    fake.answers["account_remove"] = { """{"ok":false,"error":"That account is gone."}""" }
    assertEquals("That account is gone.", ctl.remove("groq", "groq-2"))
  }

  @Test fun theApiFormatOfAHostedProviderIsShownAskedAndSavedAndACustomOneSendsNone() {
    fake.on("select", """{"ok":true}"""); fake.on("model_test", """{"ok":true,"works":true,"message":"ok"}""")
    val c = controller(); c.start(); waitFor(c) { !it.loading && !it.fetching }
    c.selectProvider("openai")
    assertEquals("responses", c.state.value.apiFormat)
    waitFor(c) { !it.fetching }
    assertEquals("responses", str(fake.body("models"), "api_format"))
    c.setApiFormat("claude"); waitFor(c) { !it.fetching && it.models.isNotEmpty() }
    assertEquals("claude", str(fake.body("models"), "api_format"))
    c.choose("gpt-4o"); c.use { }
    runBlocking { withTimeout(5000) { while ("select" !in fake.ops()) kotlinx.coroutines.delay(20) } }
    assertEquals("claude", str(fake.body("select"), "api_format"))
    c.setApiFormat(""); waitFor(c) { !it.fetching }
    assertNull("the normal format asks nothing special", fake.body("models")["api_format"])
    c.selectProvider("custom-claude"); c.setAddress("https://r.example.com/v1"); c.addressDone(); waitFor(c) { !it.fetching && it.models.isNotEmpty() }
    assertNull(fake.body("models")["api_format"])
    assertEquals("custom-claude", str(fake.body("models"), "provider"))
  }
}
