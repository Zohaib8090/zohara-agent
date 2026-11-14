package org.zohara.agent.chat

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch
import org.zohara.agent.LocalAppContainer
import org.zohara.agent.core.design.ErrorLine
import org.zohara.agent.core.design.SecondaryButton
import org.zohara.agent.core.design.ZoBottomSheet
import org.zohara.agent.core.design.ZoIcon
import org.zohara.agent.core.design.ZoIconButton
import org.zohara.agent.core.design.ZoIcons
import org.zohara.agent.core.design.ZoTheme
import org.zohara.agent.core.net.ApiException
import org.zohara.agent.core.net.MemoryChange

/** The rules of the memory feed ("Memory updated · View", docs/MEMORY.md): texts, grouping and times. */
object MemoryFeed {
  const val VIEW = "View"
  const val SEE = "See what changed in the memory"
  const val EMPTY = "Nothing has changed in the memory yet. When Zohara Agent learns something about you, it is listed here, and you can undo it."
  const val HINT = "This is what Zohara Agent changed in its memory. Undo puts a note back the way it was."

  /** "Added 2 notes" reads as the detail of "Memory updated". */
  fun chipText(summary: String): String = if (summary.isBlank()) "Memory updated" else "Memory updated (${summary.lowercase()})"

  /** The latest update only (the entries of the newest batch), or everything the list holds. [changes] is newest first. */
  fun shown(changes: List<MemoryChange>, latestOnly: Boolean): List<MemoryChange> {
    if (!latestOnly || changes.isEmpty()) return changes
    val batch = changes.first().batch
    return changes.filter { it.batch == batch }
  }

  val GROUPS = listOf("added" to "Added", "updated" to "Updated", "deleted" to "Removed")

  fun groups(list: List<MemoryChange>): List<Pair<String, List<MemoryChange>>> =
    GROUPS.mapNotNull { (action, title) -> list.filter { it.action == action }.takeIf { it.isNotEmpty() }?.let { title to it } }

  /** Where the note lives: every chat, or one project. */
  fun where(c: MemoryChange): String = when {
    c.scope.isEmpty() -> "All chats"
    c.scopeName.isEmpty() -> "A project"
    else -> "Project: ${c.scopeName}"
  }

  fun ago(now: Long, time: Long): String {
    val secs = (now - time).coerceAtLeast(0) / 1000
    return when {
      secs < 60 -> "just now"
      secs < 3600 -> "${secs / 60} min ago"
      secs < 86_400 -> "${secs / 3600} h ago"
      else -> "${secs / 86_400} days ago"
    }
  }

  /** The second line of an entry: where, when and from which chat. */
  fun sub(c: MemoryChange, now: Long): String = "${where(c)} · ${ago(now, c.time)}" + if (c.title.isNotEmpty()) " · from \"${c.title}\"" else ""
}

/** The slim chip above the message box: "Memory updated · View" with a round arrow button. Nothing is drawn while [note] is null. */
@Composable
fun MemoryChip(note: MemoryNote?, onView: () -> Unit, onDismiss: () -> Unit, modifier: Modifier = Modifier) {
  if (note == null) return
  val c = ZoTheme.colors
  Row(
    modifier.padding(start = 4.dp, bottom = 6.dp)
      .clip(CircleShape).background(c.raise).border(BorderStroke(1.dp, c.line8), CircleShape)
      .padding(start = 12.dp, end = 4.dp, top = 3.dp, bottom = 3.dp)
      .semantics { contentDescription = MemoryFeed.chipText(note.summary) },
    verticalAlignment = Alignment.CenterVertically,
    horizontalArrangement = Arrangement.spacedBy(6.dp),
  ) {
    ZoIcon(ZoIcons.Memory, tint = c.accent, size = 14.dp)
    Text(MemoryFeed.chipText(note.summary), style = ZoTheme.type.cardMeta, color = c.text2)
    Text("·", style = ZoTheme.type.cardMeta, color = c.text2)
    Text(
      MemoryFeed.VIEW, style = ZoTheme.type.cardMeta, color = c.text,
      modifier = Modifier.clip(CircleShape).clickable(role = Role.Button, onClick = onView).heightIn(min = 32.dp).padding(horizontal = 6.dp, vertical = 6.dp),
    )
    Box(
      Modifier.size(32.dp).clip(CircleShape).background(c.accent).clickable(role = Role.Button, onClick = onView).semantics { contentDescription = MemoryFeed.SEE },
      contentAlignment = Alignment.Center,
    ) { ZoIcon(ZoIcons.Arrow, tint = c.onAccent, size = 16.dp) }
    ZoIconButton(ZoIcons.Close, "Dismiss", onDismiss, size = 32.dp, iconSize = 14.dp)
  }
}

/** The sheet behind the chip and behind "Recent changes" in Settings: what the memory agent changed, with Undo for each. */
@Composable
fun MemoryChangesSheet(onDismiss: () -> Unit, latestOnly: Boolean = true, onOpenPage: (() -> Unit)? = null, clock: () -> Long = System::currentTimeMillis) {
  val container = LocalAppContainer.current
  var changes by remember { mutableStateOf<List<MemoryChange>?>(null) }
  var error by remember { mutableStateOf<String?>(null) }
  var only by remember { mutableStateOf(latestOnly) }
  var reload by remember { mutableStateOf(0) }
  LaunchedEffect(reload) {
    try { changes = container.api.memoryChanges(0, 60).changes; error = null } catch (e: CancellationException) { throw e } catch (e: ApiException) { error = e.text }
  }
  ZoBottomSheet(
    "Memory updated", onDismiss,
    footer = {
      SecondaryButton(if (only) "Show earlier changes" else "Show only the latest", { only = !only })
      if (onOpenPage != null) SecondaryButton("Open the memory page", { onDismiss(); onOpenPage() })
    },
  ) {
    val err = error
    val list = changes
    if (err != null) ErrorLine(err)
    else if (list == null) Text("Loading…", style = ZoTheme.type.cardMeta, color = ZoTheme.colors.dim)
    else MemoryChangesContent(MemoryFeed.shown(list, only), clock(), onUndo = { id -> container.api.memoryUndo(id); reload++ })
  }
}

/** The list itself (no state of its own but the buttons), so it can be drawn in a test. [onUndo] throws [ApiException] when the change cannot be undone. */
@Composable
fun MemoryChangesContent(list: List<MemoryChange>, now: Long, onUndo: suspend (String) -> Unit) {
  val c = ZoTheme.colors
  val scope = rememberCoroutineScope()
  val failed = remember { mutableStateMapOf<String, String>() }
  Column(Modifier.heightIn(max = 520.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(6.dp)) {
    if (list.isEmpty()) {
      Text(MemoryFeed.EMPTY, style = ZoTheme.type.cardMeta, color = c.dim)
      return@Column
    }
    Text(MemoryFeed.HINT, style = ZoTheme.type.cardMeta, color = c.dim)
    for ((title, rows) in MemoryFeed.groups(list)) {
      Text(title, style = ZoTheme.type.sectionHeader, color = c.dimmer, modifier = Modifier.padding(top = 8.dp))
      for (ch in rows) {
        Row(
          Modifier.clip(ZoTheme.shapes.navItem).border(BorderStroke(1.dp, c.line), ZoTheme.shapes.navItem).padding(horizontal = 12.dp, vertical = 10.dp),
          verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp),
        ) {
          Column(Modifier.weight(1f)) {
            if (ch.action == "updated") Text(ch.before, style = ZoTheme.type.cardMeta, color = c.dim, textDecoration = TextDecoration.LineThrough)
            Text(if (ch.action == "deleted") ch.before else ch.after, style = ZoTheme.type.body, color = c.text)
            Text(MemoryFeed.sub(ch, now), style = ZoTheme.type.cardMeta, color = c.dimmer, modifier = Modifier.padding(top = 2.dp))
          }
          val bad = failed[ch.id]
          SecondaryButton(
            if (ch.undone) "Undone" else if (bad != null) "Cannot undo" else "Undo",
            { scope.launch { try { onUndo(ch.id) } catch (e: CancellationException) { throw e } catch (e: ApiException) { failed[ch.id] = e.text } } },
            enabled = !ch.undone && bad == null,
          )
        }
        failed[ch.id]?.let { ErrorLine(it) }
      }
    }
  }
}
