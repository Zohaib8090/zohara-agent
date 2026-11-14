package org.zohara.agent.chat

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.ui.Modifier
import androidx.compose.runtime.getValue
import androidx.compose.runtime.setValue
import androidx.compose.ui.test.assertCountEquals
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onAllNodesWithText
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import org.zohara.agent.core.design.ZoAgentTheme
import org.zohara.agent.core.design.ZoTheme
import org.zohara.agent.core.net.ApiException
import org.zohara.agent.core.net.ErrorKind
import org.zohara.agent.core.net.MemoryChange

/** The chip above the message box and the list behind it, drawn and clicked. */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35], qualifiers = "w411dp-h900dp-xhdpi")
class MemoryChipUiTest {
  @get:Rule val rule = createComposeRule()

  private fun ch(id: String, action: String, before: String = "", after: String = "", undone: Boolean = false, scope: String = "", scopeName: String = "") =
    MemoryChange(id = id, time = 1000, batch = "b1", action = action, before = before, after = after, undone = undone, scope = scope, scopeName = scopeName, title = "Phone")

  @Test fun theChipIsNotThereWithoutANoteAndShowsViewDismissAndTheArrowWithOne() {
    var note by androidx.compose.runtime.mutableStateOf<MemoryNote?>(null)
    var viewed = 0
    var dismissed = 0
    rule.setContent { ZoAgentTheme(true) { Box(Modifier.background(ZoTheme.colors.bg)) { MemoryChip(note, { viewed++ }, { dismissed++ }) } } }
    rule.onAllNodesWithText("View").assertCountEquals(0)
    note = MemoryNote("Added 2 notes", 1)
    rule.waitForIdle()
    rule.onNodeWithText("Memory updated (added 2 notes)").assertExists()
    rule.onNodeWithText("View").performClick()
    rule.onNodeWithContentDescription(MemoryFeed.SEE).performClick()
    assertEquals(2, viewed)
    rule.onNodeWithContentDescription("Dismiss").performClick()
    assertEquals(1, dismissed)
  }

  @Test fun theListIsGroupedShowsBeforeAndAfterAndUndoRunsOnce() {
    val asked = mutableListOf<String>()
    rule.setContent {
      ZoAgentTheme(true) {
        Box(Modifier.background(ZoTheme.colors.sheet)) {
          MemoryChangesContent(
            listOf(ch("g1", "added", after = "Builds Zohara OS", scope = "p1", scopeName = "Atlas"), ch("g2", "updated", before = "Uses an A22", after = "Uses an A26"), ch("g3", "deleted", before = "Lives in Karachi"), ch("g4", "added", after = "Old one", undone = true)),
            now = 1000 + 5 * 60_000, onUndo = { id -> asked += id; if (id == "g2") throw ApiException(ErrorKind.Rejected, "That note is gone, so its old text cannot be put back.") },
          )
        }
      }
    }
    for (t in listOf("Added", "Updated", "Removed", "Builds Zohara OS", "Uses an A22", "Uses an A26", "Lives in Karachi", MemoryFeed.HINT)) rule.onNodeWithText(t).assertExists()
    rule.onNodeWithText("Project: Atlas · 5 min ago · from \"Phone\"").assertExists()
    rule.onNodeWithText("Undone").assertExists()
    assertEquals(3, rule.onAllNodesWithText("Undo").fetchSemanticsNodes().size)
    rule.onAllNodesWithText("Undo")[1].performClick()
    rule.waitForIdle()
    assertEquals(listOf("g2"), asked)
    rule.onNodeWithText("Cannot undo").assertExists()
    rule.onNodeWithText("That note is gone, so its old text cannot be put back.").assertExists()
    assertTrue(rule.onAllNodesWithText("Undo").fetchSemanticsNodes().size == 2)
  }

  @Test fun anEmptyListSaysSo() {
    rule.setContent { ZoAgentTheme(true) { Box { MemoryChangesContent(emptyList(), 0, onUndo = {}) } } }
    rule.onNodeWithText(MemoryFeed.EMPTY).assertExists()
  }
}
