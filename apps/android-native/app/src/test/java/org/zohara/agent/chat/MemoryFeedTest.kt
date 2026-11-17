package org.zohara.agent.chat

import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.zohara.agent.core.net.MemoryChange
import org.zohara.agent.core.net.MemoryChangesResp
import org.zohara.agent.core.net.SendEvent

/** The "Memory updated · View" chip: how the session decides to show it (event on the stream, asking after the answer) and the sheet's rules. */
@OptIn(ExperimentalCoroutinesApi::class)
class MemoryFeedTest {
  private fun TestScope.make(): Pair<ChatSession, FakeBackend> { val be = FakeBackend(); return Pair(ChatSession(be, backgroundScope, FakeEnv()), be) }

  private fun change(id: String, action: String, batch: String = "b1", time: Long = 1000, before: String = "", after: String = "", scope: String = "", scopeName: String = "") =
    MemoryChange(id = id, time = time, batch = batch, action = action, before = before, after = after, scope = scope, scopeName = scopeName)

  @Test fun theChipComesFromTheStreamEventAndFadesAfterAbout12Seconds() = runTest(UnconfinedTestDispatcher()) {
    val (s, be) = make()
    val ch = be.newStream()
    assertTrue(s.submit("remember I like tea"))
    ch.send(SendEvent.Chat("c1", "tea"))
    ch.send(SendEvent.MemoryUpdated("Added 1 note", 5000))
    assertEquals(MemoryNote("Added 1 note", 5000), s.state.value.memoryNote)
    advanceTimeBy(ChatSession.MEMORY_CHIP_MS - 1)
    assertTrue(s.state.value.memoryNote != null)
    advanceTimeBy(2)
    assertNull(s.state.value.memoryNote)
    ch.close(); advanceUntilIdle()
  }

  @Test fun aNewerUpdateReplacesTheChipAndTheOldTimerDoesNotHideIt() = runTest(UnconfinedTestDispatcher()) {
    val (s, be) = make()
    val ch = be.newStream()
    s.submit("hi")
    ch.send(SendEvent.MemoryUpdated("Added 1 note", 1))
    advanceTimeBy(8_000)
    ch.send(SendEvent.MemoryUpdated("Added 2 notes", 2))
    advanceTimeBy(8_000) // the first timer is over, the second is not
    assertEquals("Added 2 notes", s.state.value.memoryNote?.summary)
    advanceTimeBy(5_000)
    assertNull(s.state.value.memoryNote)
    ch.close(); advanceUntilIdle()
  }

  @Test fun dismissingHidesTheChipAtOnce() = runTest(UnconfinedTestDispatcher()) {
    val (s, be) = make()
    val ch = be.newStream()
    s.submit("hi")
    ch.send(SendEvent.MemoryUpdated("Added 1 note", 1))
    s.dismissMemory()
    assertNull(s.state.value.memoryNote)
    ch.close(); advanceUntilIdle()
  }

  @Test fun afterTheAnswerTheSessionAsksUntilTheAgentHasChangedSomething() = runTest(UnconfinedTestDispatcher()) {
    val (s, be) = make()
    be.chats["c1"] = detail("c1", "tea", entry("msg-user", "hi"), entry("msg-ai", "ok"))
    // the first ask is the baseline taken when the message goes out, then two asks that find nothing, then the change
    be.memoryResults += MemoryChangesResp(latest = 100)
    be.memoryResults += MemoryChangesResp(latest = 100)
    be.memoryResults += MemoryChangesResp(latest = 100)
    be.memoryResults += MemoryChangesResp(changes = listOf(change("g1", "added", time = 200, after = "Likes tea")), latest = 200, summary = "Added 1 note")
    val ch = be.newStream()
    s.submit("hi")
    ch.send(SendEvent.Chat("c1", "tea")); ch.send(SendEvent.Delta("ok")); ch.send(SendEvent.Done); ch.close()
    assertNull(s.state.value.memoryNote)
    advanceTimeBy(4_001)
    assertNull(s.state.value.memoryNote)
    advanceTimeBy(6_000)
    assertNull(s.state.value.memoryNote)
    advanceTimeBy(10_000)
    assertEquals(MemoryNote("Added 1 note", 200), s.state.value.memoryNote)
    assertEquals("later asks only want what is newer than the baseline", listOf(0L, 100L, 100L, 100L), be.memoryAsks)
    // it stops asking once it found something
    advanceTimeBy(60_000)
    assertEquals(4, be.memoryAsks.size)
  }

  @Test fun updatesSwitchedOffNeverShowTheChip() = runTest(UnconfinedTestDispatcher()) {
    val (s, be) = make()
    be.chats["c1"] = detail("c1", "t", entry("msg-user", "hi"), entry("msg-ai", "ok"))
    be.memoryResults += MemoryChangesResp(latest = 1)
    be.memoryResults += MemoryChangesResp(changes = listOf(change("g1", "added", time = 9, after = "x")), latest = 9, showUpdates = false, summary = "Added 1 note")
    val ch = be.newStream()
    s.submit("hi")
    ch.send(SendEvent.Chat("c1", "t")); ch.send(SendEvent.Done); ch.close()
    advanceTimeBy(5_000)
    assertNull(s.state.value.memoryNote)
    advanceTimeBy(70_000)
    assertEquals("it found the change, stayed quiet and stopped", 2, be.memoryAsks.size)
  }

  @Test fun aBackendWithoutALogNeverShowsTheChip() = runTest(UnconfinedTestDispatcher()) {
    val (s, be) = make()
    be.chats["c1"] = detail("c1", "t", entry("msg-user", "hi"), entry("msg-ai", "ok"))
    val ch = be.newStream()
    s.submit("hi")
    ch.send(SendEvent.Chat("c1", "t")); ch.send(SendEvent.Done); ch.close()
    advanceTimeBy(100_000)
    assertNull(s.state.value.memoryNote)
  }

  // ---- the rules of the chip and the sheet ----

  @Test fun theChipSaysWhatChanged() {
    assertEquals("Memory updated", MemoryFeed.chipText(""))
    assertEquals("Memory updated (added 2 notes, updated 1 note)", MemoryFeed.chipText("Added 2 notes, updated 1 note"))
  }

  @Test fun theSheetShowsTheLatestBatchOrEverythingGroupedAddedUpdatedRemoved() {
    val all = listOf(
      change("g5", "deleted", batch = "b2", before = "Old laptop"), change("g4", "added", batch = "b2", after = "New laptop"),
      change("g3", "updated", batch = "b1", before = "A", after = "B"), change("g2", "added", batch = "b1", after = "C"),
    )
    assertEquals(listOf("g5", "g4"), MemoryFeed.shown(all, true).map { it.id })
    assertEquals(4, MemoryFeed.shown(all, false).size)
    assertTrue(MemoryFeed.shown(emptyList(), true).isEmpty())
    assertEquals(listOf("Added", "Updated", "Removed"), MemoryFeed.groups(all).map { it.first })
    assertEquals(listOf("g4", "g2"), MemoryFeed.groups(all)[0].second.map { it.id })
    assertEquals(listOf("Removed"), MemoryFeed.groups(listOf(all[0])).map { it.first })
  }

  @Test fun anEntryTellsWhereAndWhenAndFromWhichChat() {
    assertEquals("All chats", MemoryFeed.where(change("g", "added")))
    assertEquals("Project: Atlas", MemoryFeed.where(change("g", "added", scope = "p1", scopeName = "Atlas")))
    assertEquals("A project", MemoryFeed.where(change("g", "added", scope = "p1")))
    assertEquals("just now", MemoryFeed.ago(10_000, 5_000))
    assertEquals("3 min ago", MemoryFeed.ago(200_000, 20_000))
    assertEquals("2 h ago", MemoryFeed.ago(7_300_000, 0))
    assertEquals("2 days ago", MemoryFeed.ago(2 * 86_400_000L + 5, 0))
    assertEquals("a time ahead of the clock is not negative", "just now", MemoryFeed.ago(1, 99_999))
    assertEquals("Project: Atlas · 3 min ago · from \"Plan\"", MemoryFeed.sub(change("g", "added", time = 20_000, scope = "p1", scopeName = "Atlas").copy(title = "Plan"), 200_000))
  }
}
