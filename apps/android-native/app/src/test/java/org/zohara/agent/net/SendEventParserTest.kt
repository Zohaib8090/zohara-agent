package org.zohara.agent.core.net

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class SendEventParserTest {
  private fun p(s: String) = SendEventParser.parse(s)

  @Test fun everyEvent() {
    assertEquals(SendEvent.Stream(7, null, null), p("""{"event":"stream","id":7}"""))
    assertEquals(SendEvent.Stream(8, "desktop", "dev1"), p("""{"event":"stream","id":8,"target":"desktop","device":"dev1"}"""))
    assertEquals(SendEvent.Chat("c1", "Hello"), p("""{"event":"chat","id":"c1","title":"Hello"}"""))
    assertEquals(SendEvent.Delta("Hi"), p("""{"event":"delta","text":"Hi"}"""))
    assertEquals(SendEvent.Text("Hi there"), p("""{"event":"text","text":"Hi there"}"""))
    assertEquals(SendEvent.Steps(7), p("""{"event":"steps","n":7}"""))
    assertEquals(SendEvent.Stuck(8, "read_file"), p("""{"event":"stuck","n":8,"tool":"read_file"}"""))
    assertEquals(SendEvent.Model("claude-opus-5-5", "Opus 5.5", true), p("""{"event":"model","model":"claude-opus-5-5","label":"Opus 5.5","differs":true}"""))
    assertEquals(SendEvent.Model("", "", false), p("""{"event":"model"}"""))
    assertEquals(SendEvent.Steps(0), p("""{"event":"steps"}"""))
    assertEquals(SendEvent.ToolStart("bash", "ls"), p("""{"event":"tool_start","name":"bash","summary":"ls"}"""))
    assertEquals(SendEvent.ToolDone("bash", true, "a\nb"), p("""{"event":"tool_done","name":"bash","ok":true,"output":"a\nb"}"""))
    assertEquals(SendEvent.Approval(3, "bash", "rm x", false), p("""{"event":"approval","id":3,"tool":"bash","summary":"rm x"}"""))
    assertEquals(SendEvent.Usage(10, 20), p("""{"event":"usage","input":10,"output":20}"""))
    assertEquals(SendEvent.Stopped, p("""{"event":"stopped"}"""))
    assertEquals(SendEvent.Done, p("""{"event":"done"}"""))
    assertEquals(SendEvent.Ping, p("""{"event":"ping"}"""))
  }

  @Test fun memoryUpdatedEvent() {
    assertEquals(SendEvent.MemoryUpdated("Added 2 notes, updated 1 note", 1794509131852), p("""{"event":"memory_updated","summary":"Added 2 notes, updated 1 note","counts":{"added":2,"updated":1,"deleted":0},"ids":["g1","g2","g3"],"time":1794509131852}"""))
    assertEquals(SendEvent.MemoryUpdated("", 0), p("""{"event":"memory_updated"}"""))
  }

  @Test fun questionCard() {
    val ev = p("""{"event":"question","id":4,"questions":[{"question":"Which?","header":"Pick","multi_select":true,"options":[{"label":"A","description":"first"},{"label":"B"}]}]}""")
    ev as SendEvent.Question
    assertEquals(4L, ev.id)
    assertEquals("Which?", ev.questions[0].question)
    assertTrue(ev.questions[0].multiSelect)
    assertEquals(listOf("A", "B"), ev.questions[0].options.map { it.label })
    assertEquals("first", ev.questions[0].options[0].description)
  }

  @Test fun errorUsesMessageOrFriendlyCode() {
    assertEquals(SendEvent.Error("x", "Boom"), p("""{"event":"error","error":"x","message":"Boom"}"""))
    val e = p("""{"event":"error","error":"desktop_offline"}""") as SendEvent.Error
    assertEquals("desktop_offline", e.code)
    assertTrue(e.message.contains("not reachable"))
  }

  @Test fun badLinesAreIgnored() {
    assertNull(p(""))
    assertNull(p("   "))
    assertNull(p("not json"))
    assertNull(p("[1,2]"))
    assertNull(p("""{"no_event":1}"""))
    assertEquals(SendEvent.Unknown("future"), p("""{"event":"future","a":1}"""))
  }

  @Test fun unicodeAndEscapes() {
    assertEquals(SendEvent.Delta("नमस्ते 🌍 \"q\" \\ é"), p("""{"event":"delta","text":"नमस्ते 🌍 \"q\" \\ é"}"""))
  }

  // ---- the line buffer ----
  @Test fun partialLinesAcrossChunks() {
    val b = NdjsonLineBuffer()
    assertEquals(emptyList<String>(), b.feed("""{"event":"del""".toByteArray()))
    assertEquals(listOf("""{"event":"delta","text":"a"}"""), b.feed("""ta","text":"a"}""".toByteArray() + "\n{\"ev".toByteArray()))
    assertEquals(listOf("""{"event":"done"}"""), b.feed("""ent":"done"}""".toByteArray() + "\n".toByteArray()))
  }

  @Test fun multibyteCharacterCutInTwo() {
    val line = """{"event":"delta","text":"日本語🌍"}""" + "\n"
    val bytes = line.toByteArray()
    for (cut in 1 until bytes.size) {
      val b = NdjsonLineBuffer()
      val out = b.feed(bytes.copyOfRange(0, cut)) + b.feed(bytes.copyOfRange(cut, bytes.size))
      assertEquals("cut at $cut", 1, out.size)
      assertEquals(SendEvent.Delta("日本語🌍"), SendEventParser.parse(out[0]))
    }
  }

  @Test fun manyLinesInOneChunkAndCrlf() {
    val b = NdjsonLineBuffer()
    val out = b.feed("{\"event\":\"ping\"}\r\n\n{\"event\":\"done\"}\n".toByteArray())
    assertEquals(listOf("""{"event":"ping"}""", """{"event":"done"}"""), out)
  }

  @Test fun largeLine() {
    val big = "x".repeat(3 * 1024 * 1024)
    val line = """{"event":"tool_done","name":"n","ok":true,"output":"$big"}"""
    val b = NdjsonLineBuffer()
    val bytes = (line + "\n").toByteArray()
    val out = ArrayList<String>()
    var i = 0
    while (i < bytes.size) { val n = minOf(8192, bytes.size - i); out += b.feed(bytes.copyOfRange(i, i + n)); i += n }
    assertEquals(1, out.size)
    assertEquals(big.length, (SendEventParser.parse(out[0]) as SendEvent.ToolDone).output.length)
  }

  @Test fun lastLineWithoutNewlineComesFromFinish() {
    val b = NdjsonLineBuffer()
    assertEquals(emptyList<String>(), b.feed("""{"event":"done"}""".toByteArray()))
    assertEquals(listOf("""{"event":"done"}"""), b.finish())
  }

  @Test fun testOfAScheduledTaskHasItsOwnEvents() {
    assertEquals(SendEvent.Test("Stretch", "reminder", "the cloud server"), p("""{"event":"test","title":"Stretch","kind":"reminder","place":"the cloud server"}"""))
    assertEquals(SendEvent.Notify("Test: Stretch", "Stand up", "reminder", true), p("""{"event":"notify","title":"Test: Stretch","body":"Stand up","kind":"reminder","ok":true}"""))
    val r = p("""{"event":"result","result":{"ok":false,"status":"skipped","summary":"No model is chosen.","tools":["zohara_web_search"],"ms":1500,"chat":"c9"}}""")
    assertEquals(SendEvent.Result(false, "skipped", "No model is chosen.", listOf("zohara_web_search"), 1500, "c9"), r)
    assertEquals(SendEvent.Result(false, "", "", emptyList(), 0, null), p("""{"event":"result"}"""))
  }
}
