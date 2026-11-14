package org.zohara.agent.net

import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import okhttp3.mockwebserver.MockResponse
import okhttp3.mockwebserver.MockWebServer
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.zohara.agent.core.net.ApiException
import org.zohara.agent.core.net.ConnectorEntry
import org.zohara.agent.core.net.ConnectorHeader
import org.zohara.agent.core.net.ConnectorRemote
import org.zohara.agent.core.net.McpServer
import org.zohara.agent.core.net.ZoApi
import org.zohara.agent.core.net.ZoHttp
import org.zohara.agent.core.net.ZoJson
import org.zohara.agent.settings.ConnectorsLogic

/** The connectors directory on the phone (docs/MCP.md "Browse connectors"): the ops it sends, what it reads, and the form rules. */
class ConnectorsApiTest {
  private lateinit var server: MockWebServer
  private lateinit var api: ZoApi

  @Before fun up() {
    server = MockWebServer().also { it.start() }
    api = ZoApi(ZoHttp.newClient({ _ -> "tok" }), { server.url("/").toString().trimEnd('/') }, retryDelayMs = { 0L })
  }
  @After fun down() { server.shutdown() }

  private fun ok(body: String) = MockResponse().setHeader("Content-Type", "application/json").setBody(body)
  private fun sent() = ZoJson.parseToJsonElement(server.takeRequest().body.readUtf8()).jsonObject

  // a trimmed answer of the real server (recorded from a sandbox server reading the recorded registry pages)
  private val answer = """{"checked_at":1794664965,"entries":[],"error":null,"featured":[
    {"category":"finance","description":"MCP server integrating with Stripe","featured":true,"id":"com.stripe/mcp","kind":"remote","local":null,"publisher":"stripe.com","remotes":[{"headers":[],"url":"https://mcp.stripe.com"}],"title":"Stripe","trust":"Featured by Zohara Agent","version":"0.2.4","website":null},
    {"category":"finance","description":"PayPal MCP server","featured":true,"id":"com.paypal.mcp/mcp","kind":"remote","local":null,"publisher":"mcp.paypal.com","remotes":[{"headers":[{"description":"OAuth Bearer token","name":"Authorization","required":true,"secret":true}],"url":"https://mcp.paypal.com/mcp"}],"title":"Paypal","trust":"Featured by Zohara Agent","version":"1.0.0","website":null}],
    "next":"abc:1","ok":true,"restricted":false,"source":"registry","stale":false}"""

  @Test fun listAsksTheConnectorsOpAndReadsTheAnswer() = runBlocking {
    server.enqueue(ok(answer))
    val r = api.connectors(q = " stripe ", cursor = "c:1", category = "finance")
    val b = sent()
    assertEquals("connectors", b["op"]!!.jsonPrimitive.content)
    assertEquals("stripe", b["q"]!!.jsonPrimitive.content)
    assertEquals("c:1", b["cursor"]!!.jsonPrimitive.content)
    assertEquals("finance", b["category"]!!.jsonPrimitive.content)
    assertEquals("remote", b["kind"]!!.jsonPrimitive.content)
    assertEquals(listOf("com.stripe/mcp", "com.paypal.mcp/mcp"), r.featured.map { it.id })
    assertEquals("Featured by Zohara Agent", r.featured[0].trust)
    assertEquals("abc:1", r.next)
    assertTrue(r.featured[1].remotes[0].headers[0].secret)
    assertFalse(r.restricted)
  }

  @Test fun aRestrictedAccountGetsTheExplanation() = runBlocking {
    server.enqueue(ok("""{"ok":true,"restricted":true,"message":"This account cannot add connectors.","entries":[],"featured":[]}"""))
    val r = api.connectors()
    assertTrue(r.restricted)
    assertEquals("This account cannot add connectors.", r.message)
    assertTrue(r.entries.isEmpty())
  }

  @Test fun tryingSendsTheKeysByHeaderNameAndSavingSendsOnlyTheName() = runBlocking {
    server.enqueue(ok("""{"ok":true,"id":"com.paypal.mcp/mcp","name":"mcp","kind":"remote","target":"https://mcp.paypal.com/mcp","tools":["a","b"],"publisher":"mcp.paypal.com","trust":"Featured by Zohara Agent","notice":"Every call still asks you first."}"""))
    val p = api.connectorPreview("com.paypal.mcp/mcp", "pp", "https://mcp.paypal.com/mcp", mapOf("Authorization" to "Bearer k"))
    val b = sent()
    assertEquals("connector_preview", b["op"]!!.jsonPrimitive.content)
    assertEquals("Bearer k", b["headers"]!!.jsonObject["Authorization"]!!.jsonPrimitive.content)
    assertEquals("pp", b["name"]!!.jsonPrimitive.content)
    assertEquals(listOf("a", "b"), p.tools)
    assertTrue(ConnectorsLogic.tried(p).startsWith("It works. It offers these tools: a, b."))
    server.enqueue(ok("""{"ok":true,"message":"Added \"mcp\"."}"""))
    assertEquals("Added \"mcp\".", api.connectorAdd("mcp"))
    val c = sent()
    assertEquals(setOf("op", "name"), c.keys)
    server.enqueue(ok("""{"ok":true}"""))
    api.connectorCancel("mcp")
    assertEquals("connector_cancel", sent()["op"]!!.jsonPrimitive.content)
  }

  @Test fun aRefusedTryBecomesAnException() = runBlocking {
    server.enqueue(ok("""{"ok":false,"error":"The server refused the key you typed. Check it and try again."}"""))
    try { api.connectorPreview("x.y/z", "z"); org.junit.Assert.fail() } catch (e: ApiException) { assertTrue(e.text.contains("refused the key")) }
  }

  @Test fun anAddressChangeIsReadAndAccepted() = runBlocking {
    server.enqueue(ok("""{"ok":true,"servers":[{"name":"n","command":"https://old.example/mcp","enabled":false,"held":true,"registry":"a.b/c","address_changed":"https://new.example/mcp"}]}"""))
    val s = api.mcpList()[0]
    assertEquals("mcp_list", sent()["op"]!!.jsonPrimitive.content)
    assertEquals("https://new.example/mcp", s.addressChanged)
    assertTrue(ConnectorsLogic.rowSubtitle(s).startsWith("Address changed: the directory now lists https://new.example/mcp."))
    assertEquals(setOf("a.b/c"), ConnectorsLogic.addedIds(listOf(s, McpServer("by-hand", "https://x.y/mcp"))))
    server.enqueue(ok("""{"ok":true,"message":"now uses it"}"""))
    api.connectorAcceptChange("n")
    assertEquals("connector_accept_change", sent()["op"]!!.jsonPrimitive.content)
    assertNull(McpServer("by-hand", "https://x.y/mcp").addressChanged)
  }

  @Test fun formRules() {
    val e = ConnectorEntry("com.paypal.mcp/mcp", "Paypal", remotes = listOf(ConnectorRemote("https://mcp.paypal.com/mcp", listOf(ConnectorHeader("Authorization", required = true, secret = true), ConnectorHeader("X-Fixed", value = "1")))))
    assertEquals(listOf("Authorization"), ConnectorsLogic.asked(e).map { it.name })
    assertEquals("Give it a name.", ConnectorsLogic.validate(" ", e, emptyMap(), emptyList()))
    assertEquals("A server with that name is already added.", ConnectorsLogic.validate("PP", e, emptyMap(), listOf(McpServer("pp"))))
    assertEquals("Authorization is needed.", ConnectorsLogic.validate("pp", e, emptyMap(), emptyList()))
    assertEquals("", ConnectorsLogic.validate("pp", e, mapOf("Authorization" to "Bearer k"), emptyList()))
    assertEquals("mcp", ConnectorsLogic.defaultName(e))
  }
}
