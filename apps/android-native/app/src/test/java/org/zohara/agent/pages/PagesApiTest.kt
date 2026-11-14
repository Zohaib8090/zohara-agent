package org.zohara.agent.pages

import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import okhttp3.mockwebserver.MockResponse
import okhttp3.mockwebserver.MockWebServer
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.zohara.agent.core.net.ZoApi
import org.zohara.agent.core.net.ZoHttp
import org.zohara.agent.core.net.ZoJson

/** The real client against a mock server with the answers captured live (docs/API.md): the wire shapes the pages depend on. */
class PagesApiTest {
  private lateinit var server: MockWebServer
  private lateinit var api: PagesApi

  @Before fun up() {
    server = MockWebServer().also { it.start() }
    api = ZoPagesApi(ZoApi(ZoHttp.newClient({ _ -> "tok" }), { server.url("/").toString().trimEnd('/') }, retryDelayMs = { 0L }))
  }
  @After fun down() { server.shutdown() }

  private fun ok(body: String) = MockResponse().setHeader("Content-Type", "application/json").setBody(body)
  private fun body() = ZoJson.parseToJsonElement(server.takeRequest().body.readUtf8()).jsonObject

  @Test fun chatsLive() = runBlocking {
    server.enqueue(ok("""{"ok":true,"chats":[{"id":"c1794509120577","pinned":true,"title":"run it","updated":1794509131852}]}"""))
    val c = api.chats().single()
    assertEquals("run it", c.title); assertTrue(c.pinned); assertEquals(1794509131852, c.updated)
  }

  @Test fun searchLive() = runBlocking {
    server.enqueue(ok("""{"ok":true,"hits":[{"id":"c1","snippet":"Hello from the fake model.","title":"run it","updated":1794509131852}]}"""))
    val h = api.search("fake").single()
    assertEquals("Hello from the fake model.", h.snippet)
    assertEquals("fake", body()["query"]!!.jsonPrimitive.content)
  }

  @Test fun pinAndDeleteBodies() = runBlocking {
    server.enqueue(ok("""{"ok":true}""")); server.enqueue(ok("""{"ok":true}"""))
    api.pinChat("c1", true); api.deleteChat("c1")
    val pin = body(); val del = body()
    assertEquals("pin_chat", pin["op"]!!.jsonPrimitive.content); assertEquals("true", pin["on"]!!.jsonPrimitive.content)
    assertEquals("delete_chat", del["op"]!!.jsonPrimitive.content); assertEquals("c1", del["id"]!!.jsonPrimitive.content)
  }

  @Test fun projectsLiveTextBecomesCards() = runBlocking {
    server.enqueue(ok("""{"ok":true,"text":"p1794509093697 | Demo | g | 0 memory notes"}"""))
    assertEquals("Demo", PagesFormat.parseProjects(api.projects()).single().name)
  }

  @Test fun artifactsAndScheduledLive() = runBlocking {
    server.enqueue(ok("""{"ok":true,"items":[{"id":"a1","kind":"code","title":"x.rs","path":"","text":"fn main() {}","chat":"c1","created":1794509131852}]}"""))
    server.enqueue(ok("""{"ok":true,"tasks":[{"id":"t17945090995370","title":"t","prompt":"t","next_run":1794512699000,"repeat":"once","enabled":true}]}"""))
    val a = api.artifacts().single(); val t = api.scheduled().single()
    assertEquals("fn main() {}", a.text); assertEquals("c1", a.chat)
    assertEquals(1794512699000, t.nextRun); assertTrue(t.enabled)
  }

  @Test fun scheduleAddRewritesInAndSendsRepeat() = runBlocking {
    server.enqueue(ok("""{"ok":true,"message":"Scheduled \"t\" (once)."}"""))
    api.scheduleAdd("check disk", "in 2 hours", "every 90 minutes")
    val b = body()
    assertEquals("now + 2 hours", b["when"]!!.jsonPrimitive.content)
    assertEquals("every 90 minutes", b["repeat"]!!.jsonPrimitive.content)
    assertEquals("check disk", b["task"]!!.jsonPrimitive.content)
  }

  @Test fun activityLive() = runBlocking {
    server.enqueue(ok("""{"ok":true,"now_ms":1794509131887,"clients_seen":[{"last_ms":1794509131884,"source":"web"}],"items":[{"id":3,"kind":"answer","title":"hello","detail":"m1","source":"web","chat_id":"c1","started_ms":1794509131000,"can_stop":true}]}"""))
    val a = api.activity()
    assertEquals(887, a.nowMs - a.items[0].startedMs)
    assertEquals("c1", a.items[0].chatId); assertTrue(a.items[0].canStop)
    assertEquals("web", a.clientsSeen[0].source)
  }

  @Test fun refusedOpIsTheServersSentence() = runBlocking {
    server.enqueue(ok("""{"ok":false,"error":"Scheduled tasks are not available to this account."}"""))
    val h = ScheduledHolder(api, kotlinx.coroutines.CoroutineScope(kotlinx.coroutines.Dispatchers.Unconfined))
    h.refresh()
    val done = kotlinx.coroutines.withTimeout(5000) { h.state.first { !it.loading } }
    assertEquals("Scheduled tasks are not available to this account.", done.error)
  }

  // ---- the memory feed and project memory (docs/MEMORY.md) ----

  private val zo get() = ZoApi(ZoHttp.newClient({ _ -> "tok" }), { server.url("/").toString().trimEnd('/') }, retryDelayMs = { 0L })

  @Test fun memoryChangesLiveShapeAndTheSinceAsk() = runBlocking {
    server.enqueue(ok("""{"ok":true,"changes":[{"id":"g1794509131852001","time":1794509131852,"batch":"b1794509131852","source":"engine","chat":"c1","title":"Phone","scope":"p1","scope_name":"Atlas","action":"updated","note":"m1","kind":"fact","before":"Uses an A22","after":"Uses an A26","undone":false}],"latest":1794509131852,"running":false,"show_updates":true,"summary":"updated 1 note"}"""))
    val r = zo.memoryChanges(since = 1794509000000, limit = 60)
    val c = r.changes.single()
    assertEquals(listOf("updated", "Uses an A22", "Uses an A26", "Atlas", "p1"), listOf(c.action, c.before, c.after, c.scopeName, c.scope))
    assertEquals(1794509131852, r.latest); assertEquals("updated 1 note", r.summary); assertTrue(r.showUpdates)
    val b = body()
    assertEquals("memory_changes", b["op"]!!.jsonPrimitive.content); assertEquals("1794509000000", b["since"]!!.jsonPrimitive.content); assertEquals("60", b["limit"]!!.jsonPrimitive.content)
  }

  @Test fun memoryUndoSettingsAndAddWithAScope() = runBlocking {
    server.enqueue(ok("""{"ok":true,"message":"The note has its earlier text again."}"""))
    assertEquals("The note has its earlier text again.", zo.memoryUndo("g1"))
    assertEquals(listOf("memory_undo", "g1"), body().let { listOf(it["op"]!!.jsonPrimitive.content, it["id"]!!.jsonPrimitive.content) })
    server.enqueue(ok("""{"ok":true,"every":2,"min_every":2,"show_updates":false}"""))
    val st = zo.memorySettings(every = 1, showUpdates = false)
    assertEquals(Triple(2, 2, false), Triple(st.every, st.minEvery, st.showUpdates))
    val sb = body()
    assertEquals(listOf("memory_settings", "1", "false"), listOf(sb["op"]!!.jsonPrimitive.content, sb["every"]!!.jsonPrimitive.content, sb["show_updates"]!!.jsonPrimitive.content))
    server.enqueue(ok("""{"ok":true,"new":true}""")); server.enqueue(ok("""{"ok":true,"new":true}"""))
    zo.memoryAdd("The atlas release ships on Friday", "project", scope = "p1"); zo.memoryAdd("Likes tea")
    val withScope = body(); val without = body()
    assertEquals("p1", withScope["scope"]!!.jsonPrimitive.content)
    assertTrue("scope" !in without)
  }

  @Test fun memoryListAsksForAScopeAndReadsProjectsAndNotesScopes() = runBlocking {
    server.enqueue(ok("""{"ok":true,"enabled":true,"every":3,"min_every":2,"show_updates":true,"projects":[{"id":"p1","name":"Atlas"}],"items":[{"id":"m1","text":"Ships Friday","kind":"project","title":"","chat":"","pinned":false,"updated":5,"scope":"p1","scope_name":"Atlas"},{"id":"m2","text":"Likes tea","kind":"preference","title":"","chat":"","pinned":false,"updated":4,"scope":"","scope_name":""}]}"""))
    val m = zo.memory("p1")
    assertEquals("p1", body()["scope"]!!.jsonPrimitive.content)
    assertEquals(listOf("Atlas"), m.projects.map { it.name })
    assertEquals(listOf("p1", ""), m.items.map { it.scope }); assertEquals("Atlas", m.items[0].scopeName)
    // an older server without the new fields still reads
    server.enqueue(ok("""{"ok":true,"enabled":true,"every":3,"items":[{"id":"m1","text":"x","kind":"fact"}]}"""))
    val old = zo.memory()
    assertTrue("scope" !in body())
    assertEquals(Triple(2, true, 0), Triple(old.minEvery, old.showUpdates, old.projects.size))
  }

  @Test fun memoryExportImportAndProjectDelete() = runBlocking {
    server.enqueue(ok("""{"ok":true,"format":"zo-memory-1","exported_at":5,"projects":[{"id":"p1","name":"Atlas"}],"notes":[{"text":"Ships Friday","kind":"project","scope":"p1","pinned":false}]}"""))
    val ex = zo.memoryExport()
    assertEquals("zo-memory-1", ex["format"]!!.jsonPrimitive.content)
    server.enqueue(ok("""{"ok":true,"added":1,"already_there":2,"skipped":3}"""))
    val r = zo.memoryImport(ex)
    assertEquals(Triple(1, 2, 3), Triple(r.added, r.alreadyThere, r.skipped))
    server.takeRequest()
    assertEquals("zo-memory-1", body()["data"]!!.jsonObject["format"]!!.jsonPrimitive.content)
    server.enqueue(ok("""{"ok":true,"notes_removed":4}"""))
    assertEquals(4, api.projectDelete("p1"))
    val d = body()
    assertEquals(listOf("project_delete", "p1", "true"), listOf(d["op"]!!.jsonPrimitive.content, d["id"]!!.jsonPrimitive.content, d["confirm"]!!.jsonPrimitive.content))
    // the server refuses without the confirm; the app always sends it
    server.enqueue(ok("""{"ok":false,"error":"There is no such project."}"""))
    val e = try { api.projectDelete("p404"); null } catch (e: org.zohara.agent.core.net.ApiException) { e.text }
    assertEquals("There is no such project.", e)
  }
}
