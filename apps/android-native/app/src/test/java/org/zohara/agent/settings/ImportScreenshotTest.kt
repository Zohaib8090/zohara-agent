package org.zohara.agent.settings

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onRoot
import androidx.compose.ui.unit.dp
import com.github.takahirom.roborazzi.captureRoboImage
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.decodeFromJsonElement
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import org.zohara.agent.core.design.PrimaryButton
import org.zohara.agent.core.design.SecondaryButton
import org.zohara.agent.core.design.SheetContent
import org.zohara.agent.core.design.ZoAgentTheme
import org.zohara.agent.core.design.ZoTheme
import org.zohara.agent.core.net.MigrateInstruction
import org.zohara.agent.core.net.MigrateNeed
import org.zohara.agent.core.net.MigrateScan
import org.zohara.agent.core.net.MigrateServer
import org.zohara.agent.core.net.MigrateTool
import org.zohara.agent.core.net.MigrateProject
import org.zohara.agent.core.net.MigrateCan

/** Screenshots of the Import checklist (`./gradlew recordRoborazziDebug` writes app/src/test/screenshots/import_*.png). */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35], qualifiers = "w400dp-h2400dp-hdpi")
class ImportScreenshotTest {
  @get:Rule val rule = createComposeRule()

  private val scan = MigrateScan(
    origin = "this computer",
    tools = listOf(MigrateTool("claude-code", "Claude Code CLI", true), MigrateTool("cursor", "Cursor", true)),
    servers = listOf(
      MigrateServer("s1", "claude-code", "Claude Code CLI", "~/.claude.json", "user", "", "github", "program", "npx -y @modelcontextprotocol/server-github", false,
        needs = listOf(MigrateNeed("env", "GITHUB_PERSONAL_ACCESS_TOKEN", null, true))),
      MigrateServer("s2", "claude-code", "Claude Code CLI", "~/.claude.json", "user", "", "sentry", "remote", "https://mcp.sentry.dev/mcp", false,
        needs = listOf(MigrateNeed("header", "Authorization", "DOCS_TOKEN", false))),
      MigrateServer("s3", "claude-code", "Claude Code CLI", "~/.claude.json", "user", "", "notion", "remote", "https://mcp.notion.com/mcp", true, duplicate = "here", duplicateOf = "notion"),
      MigrateServer("s4", "cursor", "Cursor", "~/.cursor/mcp.json", "user", "", "zapier", "remote", "https://mcp.zapier.com/api/mcp/s/•••/mcp", false, notes = listOf("This server uses the older SSE transport; it may not connect here.")),
    ),
    instructions = listOf(
      MigrateInstruction("i1", "claude-code", "Claude Code CLI", "~/.claude/CLAUDE.md", "global", "", 1200, 40, "Always answer in short sentences. Prefer Rust for command line tools.", 12, 0, 1),
      MigrateInstruction("i2", "claude-code", "Claude Code CLI", "/home/dev/webapp/CLAUDE.md", "project", "/home/dev/webapp", 300, 9, "This project uses pnpm, never npm.", 4, 0, 0, suggest = "p1"),
    ),
    projects = listOf(MigrateProject("p1", "webapp")),
    can = MigrateCan(servers = true, programs = true, projects = true, secrets = false),
  )

  private fun shot(name: String, dark: Boolean, s: MigrateScan = scan) {
    rule.setContent {
      ZoAgentTheme(dark) {
        val c = ZoTheme.colors
        Column(Modifier.background(c.sheet).width(400.dp).padding(horizontal = 0.dp)) {
          SheetContent("Found in ${s.origin}", null, footer = { SecondaryButton("Close", {}); PrimaryButton("Import ticked", {}) }) {
            ReviewBody(s, ImportLogic.defaultSelection(s).toList(), mapOf("i1" to "notes", "i2" to "instruction:p1"), { _, _ -> }, { _, _ -> }, null)
          }
        }
      }
    }
    rule.onRoot().captureRoboImage("src/test/screenshots/import_${name}_${if (dark) "dark" else "light"}.png")
  }

  @Test fun reviewDark() = shot("review", true)
  @Test fun reviewLight() = shot("review", false)
  /** a restricted account: the servers cannot be ticked and there are no project targets */
  @Test fun reviewRestrictedDark() = shot("review_restricted", true, scan.copy(can = MigrateCan(servers = false, programs = false, projects = false)))
}
