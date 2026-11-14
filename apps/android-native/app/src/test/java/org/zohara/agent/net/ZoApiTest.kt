package org.zohara.agent.core.net

import kotlinx.coroutines.flow.toList
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import okhttp3.OkHttpClient
import okhttp3.mockwebserver.MockResponse
import okhttp3.mockwebserver.MockWebServer
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Before
import org.junit.Test

class ZoApiTest {
  private lateinit var server: MockWebServer
  private lateinit var api: ZoApi

  @Before fun up() {
    server = MockWebServer().also { it.start() }
    val http = ZoHttp.newClient({ _ -> "tok" })
    api = ZoApi(http, { server.url("/").toString().trimEnd('/') }, retryDelayMs = { 0L })
  }
  @After fun down() { server.shutdown() }

  private fun ok(body: String) = MockResponse().setHeader("Content-Type", "application/json").setBody(body)

  @Test fun opPostsTheOpAndDecodes() = runBlocking {
    server.enqueue(ok("""{"ok":true,"chats":[{"id":"a","title":"One","pinned":true,"updated":5}]}"""))
    val chats = api.chats()
    assertEquals("a", chats[0].id)
    val req = server.takeRequest()
    assertEquals("/api/op", req.path)
    assertEquals("Bearer tok", req.getHeader("Authorization"))
    assertEquals("chats", ZoJson.parseToJsonElement(req.body.readUtf8()).jsonObject["op"]!!.jsonPrimitive.content)
  }

  @Test fun okFalseBecomesAnException() = runBlocking {
    server.enqueue(ok("""{"ok":false,"error":"unknown request: model_test"}"""))
    try { api.modelTest(); fail() } catch (e: ApiException) { assertEquals(ErrorKind.UnknownOp, e.kind) }
  }

  @Test fun rateLimitThrowsWithRetryAfter() = runBlocking {
    server.enqueue(MockResponse().setResponseCode(429).setHeader("Retry-After", "3").setBody("{}"))
    try { api.deleteChat("x"); fail() } catch (e: ApiException) {
      assertEquals(ErrorKind.RateLimited, e.kind)
      assertEquals(3, e.retryAfterSec)
    }
  }

  @Test fun readOnlyCallRetriesOnceOn503() = runBlocking {
    server.enqueue(MockResponse().setResponseCode(503).setBody("{}"))
    server.enqueue(ok("""{"ok":true,"chats":[]}"""))
    assertEquals(0, api.chats().size)
    assertEquals(2, server.requestCount)
  }

  @Test fun listenerSeesErrors() = runBlocking {
    var seen: ErrorKind? = null
    api.errorListener = { seen = it.kind }
    server.enqueue(MockResponse().setResponseCode(403).setBody("""{"error":"t","terms_required":true,"version":"v9"}"""))
    try { api.chats(); fail() } catch (_: ApiException) {}
    assertEquals(ErrorKind.TermsRequired, seen)
  }

  @Test fun sendStreamsEventsAndEndsAfterDone() = runBlocking {
    val body = listOf(
      """{"event":"stream","id":1}""", """{"event":"chat","id":"c","title":"T"}""", """{"event":"delta","text":"He"}""",
      """{"event":"delta","text":"llo"}""", """{"event":"usage","input":1,"output":2}""", """{"event":"done"}""", """{"event":"delta","text":"after done"}""",
    ).joinToString("\n", postfix = "\n")
    server.enqueue(MockResponse().setHeader("Content-Type", "application/x-ndjson").setBody(body))
    val events = api.send(SendRequest("hi", chat = "c", keepRunning = true, mode = "auto")).toList()
    assertEquals(listOf("Stream", "Chat", "Delta", "Delta", "Usage", "Done"), events.map { it::class.simpleName })
    val sent = ZoJson.parseToJsonElement(server.takeRequest().body.readUtf8()).jsonObject
    assertEquals("hi", sent["text"]!!.jsonPrimitive.content)
    assertEquals("true", sent["keep_running"]!!.jsonPrimitive.content)
    assertEquals("auto", sent["mode"]!!.jsonPrimitive.content)
  }

  @Test fun sendWithoutDoneReportsConnectionLost() = runBlocking {
    server.enqueue(MockResponse().setBody("""{"event":"delta","text":"x"}""" + "\n"))
    val events = api.send(SendRequest("hi")).toList()
    assertTrue(events.last() is SendEvent.ConnectionLost)
  }

  @Test fun sendRefusedThrowsOnCollect() = runBlocking {
    server.enqueue(MockResponse().setResponseCode(429).setHeader("Retry-After", "2").setBody("{}"))
    try { api.send(SendRequest("hi")).toList(); fail() } catch (e: ApiException) { assertEquals(ErrorKind.RateLimited, e.kind) }
  }

  @Test fun replyBodies() = runBlocking {
    server.enqueue(ok("""{"ok":true}"""))
    assertTrue(api.replyApprove(5, 9, true))
    val a = ZoJson.parseToJsonElement(server.takeRequest().body.readUtf8()).jsonObject
    assertEquals("/api/reply", "/api/reply")
    assertEquals("approve", a["op"]!!.jsonPrimitive.content)
    assertEquals("5", a["stream"]!!.jsonPrimitive.content)
    server.enqueue(ok("""{"ok":true}"""))
    api.replyAnswer(5, 9, listOf(QuestionAnswer(listOf("A", "B"), "other\u0007 text")))
    val b = ZoJson.parseToJsonElement(server.takeRequest().body.readUtf8()).jsonObject
    assertEquals("answer", b["op"]!!.jsonPrimitive.content)
    assertTrue(b.toString().contains("other"))
    assertTrue(!b.toString().contains("\\u0007"))
    // "You decide" is sent as decide:true and nothing else is set
    server.enqueue(ok("""{"ok":true}"""))
    api.replyAnswer(5, 9, listOf(QuestionAnswer(emptyList(), null, decide = true), QuestionAnswer(listOf("A"))))
    val c = ZoJson.parseToJsonElement(server.takeRequest().body.readUtf8()).jsonObject["answers"]!!.jsonArray
    assertEquals("true", c[0].jsonObject["decide"]!!.jsonPrimitive.content)
    assertEquals(0, c[0].jsonObject["selected"]!!.jsonArray.size)
    assertTrue(c[1].jsonObject["decide"] == null)
  }

  // ---- Scheduled v2 (docs/SCHEDULED.md)

  private fun sent() = ZoJson.parseToJsonElement(server.takeRequest().body.readUtf8()).jsonObject

  @Test fun scheduledListHasTheNewFieldsAndAnOldServerStillWorks() = runBlocking {
    server.enqueue(ok("""{"ok":true,"host":"cloud","kinds":["reminder","task"],"run_on":[{"id":"cloud","label":"Cloud server","note":"n"}],"limits":{"max_tasks":5,"min_repeat_minutes":60,"runs_per_day":24,"runs_today":2},"tasks":[{"id":"t1","title":"Stretch","prompt":"Stand up","next_run":5,"repeat":"weekdays","enabled":true,"kind":"reminder","run_on":"cloud","tz":"UTC","last_run":4,"last_chat":null,"last_status":"ok","last_output":"Stand up","history_count":3,"running":false,"place":"the cloud server"}]}"""))
    val r = api.scheduledInfo()
    assertEquals("cloud", r.host); assertEquals(5, r.limits.maxTasks); assertEquals("cloud", r.runOn[0].id)
    val t = r.tasks.single()
    assertEquals("reminder", t.kind); assertEquals("ok", t.lastStatus); assertEquals(3, t.historyCount); assertEquals("the cloud server", t.place)
    server.enqueue(ok("""{"ok":true,"tasks":[{"id":"t2","title":"","prompt":"p","next_run":1,"repeat":"once","enabled":true}]}"""))
    val old = api.scheduled().single()
    assertEquals("task", old.kind); assertEquals(null, old.lastStatus)
  }

  @Test fun scheduleSaveSendsEveryFieldAndTheTimeZone() = runBlocking {
    server.enqueue(ok("""{"ok":true,"task":{"id":"t9","prompt":"p","kind":"reminder"}}"""))
    api.scheduleSave(null, "Stand up", "every weekday at 9", null, "Stretch", "reminder", "cloud", "Asia/Karachi")
    val b = sent()
    assertEquals("schedule_add", b["op"]!!.jsonPrimitive.content)
    assertEquals("every weekday at 9", b["when"]!!.jsonPrimitive.content)
    assertEquals("reminder", b["kind"]!!.jsonPrimitive.content); assertEquals("cloud", b["run_on"]!!.jsonPrimitive.content)
    assertEquals("Asia/Karachi", b["tz"]!!.jsonPrimitive.content); assertEquals("Stretch", b["title"]!!.jsonPrimitive.content)
    assertTrue("repeat" !in b)
    server.enqueue(ok("""{"ok":true,"task":{"id":"t9","prompt":"p"}}"""))
    api.scheduleSave("t9", "new", null, "daily", null, "task", "", "UTC")
    val u = sent()
    assertEquals("schedule_update", u["op"]!!.jsonPrimitive.content); assertEquals("t9", u["id"]!!.jsonPrimitive.content)
    assertTrue("when" !in u); assertEquals("daily", u["repeat"]!!.jsonPrimitive.content)
  }

  @Test fun enableRunNowHistoryAndPreview() = runBlocking {
    server.enqueue(ok("""{"ok":true,"task":{"id":"t1","enabled":false}}"""))
    assertEquals(false, api.scheduleEnable("t1", false)!!.enabled)
    val e = sent(); assertEquals("schedule_enable", e["op"]!!.jsonPrimitive.content); assertEquals("false", e["on"]!!.jsonPrimitive.content)
    server.enqueue(ok("""{"ok":true,"started":true}"""))
    api.scheduleRunNow("t1")
    assertEquals("schedule_run_now", sent()["op"]!!.jsonPrimitive.content)
    server.enqueue(ok("""{"ok":true,"task":{"id":"t1"},"history":[{"time":9,"status":"failed","summary":"why","chat":"c1","ms":2000,"missed":3,"how":"scheduled"}]}"""))
    val h = api.scheduleHistory("t1").history.single()
    assertEquals("failed", h.status); assertEquals(3L, h.missed); assertEquals("c1", h.chat)
    assertEquals("schedule_history", sent()["op"]!!.jsonPrimitive.content)
    server.enqueue(ok("""{"ok":true,"repeat":"weekdays","runs":[{"at":1,"label":"Mon 16 Nov, 09:00"}],"text":"next run: Mon 16 Nov, 09:00"}"""))
    assertEquals("next run: Mon 16 Nov, 09:00", api.schedulePreview("every weekday at 9", null, "UTC").text)
    val p = sent(); assertEquals("schedule_preview", p["op"]!!.jsonPrimitive.content); assertEquals("UTC", p["tz"]!!.jsonPrimitive.content)
    server.enqueue(ok("""{"ok":false,"error":"I couldn't read \"soon\" as a time."}"""))
    try { api.schedulePreview("soon", null, "UTC"); fail() } catch (x: ApiException) { assertTrue(x.text.contains("soon")) }
  }

  @Test fun testStreamsFromItsOwnRouteAndEndsAtDone() = runBlocking {
    server.enqueue(MockResponse().setHeader("Content-Type", "application/x-ndjson").setBody(
      """{"event":"stream","id":4}""" + "\n" + """{"event":"test","title":"Stretch","kind":"reminder","place":"the cloud server"}""" + "\n" +
        """{"event":"notify","title":"Test: Stretch","body":"Stand up","kind":"reminder","ok":true}""" + "\n" +
        """{"event":"result","result":{"ok":true,"status":"ok","summary":"Stand up","tools":[],"ms":10}}""" + "\n" + """{"event":"done"}""" + "\n",
    ))
    val events = api.scheduleTest("t1", null, "UTC").toList()
    assertEquals(listOf("Stream", "Test", "Notify", "Result", "Done"), events.map { it::class.simpleName })
    val req = server.takeRequest()
    assertEquals("/api/schedule/test", req.path)
    val b = ZoJson.parseToJsonElement(req.body.readUtf8()).jsonObject
    assertEquals("t1", b["id"]!!.jsonPrimitive.content); assertEquals("UTC", b["tz"]!!.jsonPrimitive.content)
    // a draft is sent as `draft`
    server.enqueue(MockResponse().setHeader("Content-Type", "application/x-ndjson").setBody("""{"event":"done"}""" + "\n"))
    api.scheduleTest(null, kotlinx.serialization.json.buildJsonObject { put("task", kotlinx.serialization.json.JsonPrimitive("x")) }, "UTC").toList()
    assertEquals("x", sent()["draft"]!!.jsonObject["task"]!!.jsonPrimitive.content)
  }
}
