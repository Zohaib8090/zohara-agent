package org.zohara.agent.settings

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.decodeFromJsonElement
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.zohara.agent.core.net.MigrateCan
import org.zohara.agent.core.net.MigrateFile
import org.zohara.agent.core.net.MigrateResult
import org.zohara.agent.core.net.MigrateScan
import org.zohara.agent.core.net.ZoJson

/**
 * The rules of Settings > Import, on the real answers of the server (recorded from `migrate_scan` and `migrate_apply` of zo-server with the
 * fixture files of zo-core's tests: Claude Desktop, Codex and CLAUDE.md; no key in them is real).
 */
class ImportLogicTest {
  private val scanJson = """{"can":{"programs":false,"projects":true,"secrets":false,"servers":true},"extras":[{"id":"xed8474ecb6","kind":"model","name":"gpt-5-codex","note":"Choose the model in Settings > Model.","tool":"codex","tool_name":"Codex CLI"}],"instructions":[{"chars":160,"folder":"","id":"ib847bb0ae9","lines":5,"notes":2,"notes_more":0,"preview":"- Always answer in short sentences. - Prefer Rust for command line tools.","scope":"global","secret_lines":1,"source":"CLAUDE.md","suggest":null,"tool":"claude-code","tool_name":"Claude Code CLI"}],"kind":"paste","notes":[],"origin":"what you pasted","placeholder":"YOUR_VALUE_HERE","projects":[{"id":"p1794889395608","name":"webapp"}],"servers":[{"disabled":false,"duplicate":"","duplicate_of":"","folder":"","id":"scca0244823","kind":"program","name":"github","needs":[{"in_file":true,"kind":"env","name":"GITHUB_PERSONAL_ACCESS_TOKEN"}],"notes":[],"plain_env":1,"scope":"user","source":"claude_desktop_config.json","target":"npx -y @modelcontextprotocol/server-github","tool":"claude-desktop","tool_name":"Claude Desktop"},{"disabled":false,"duplicate":"","duplicate_of":"","folder":"","id":"s1014a69b2f","kind":"remote","name":"linear","needs":[],"notes":["This server uses the older SSE transport; it may not connect here."],"plain_env":0,"scope":"user","source":"claude_desktop_config.json","target":"https://mcp.linear.app/sse","tool":"claude-desktop","tool_name":"Claude Desktop"},{"disabled":false,"duplicate":"here","duplicate_of":"notion","folder":"","id":"s25afb54abe","kind":"remote","name":"notion","needs":[],"notes":[],"plain_env":0,"scope":"user","source":"claude_desktop_config.json","target":"https://mcp.notion.com/mcp","tool":"claude-desktop","tool_name":"Claude Desktop"},{"disabled":false,"duplicate":"","duplicate_of":"","folder":"","id":"s7f1ba1659c","kind":"remote","name":"sentry","needs":[{"in_file":true,"kind":"header","name":"Authorization"}],"notes":[],"plain_env":0,"scope":"user","source":"claude_desktop_config.json","target":"https://mcp.sentry.dev/mcp","tool":"claude-desktop","tool_name":"Claude Desktop"},{"disabled":false,"duplicate":"","duplicate_of":"","folder":"","id":"s9b34531023","kind":"remote","name":"docs","needs":[{"from":"DOCS_TOKEN","in_file":false,"kind":"header","name":"Authorization"}],"notes":[],"plain_env":0,"scope":"user","source":"config.toml","target":"https://docs.example.com/mcp","tool":"codex","tool_name":"Codex CLI"}],"tools":[{"files":["CLAUDE.md"],"found":true,"id":"claude-code","instructions":1,"name":"Claude Code CLI","servers":0},{"files":["claude_desktop_config.json"],"found":true,"id":"claude-desktop","instructions":0,"name":"Claude Desktop","servers":4},{"files":["config.toml"],"found":true,"id":"codex","instructions":0,"name":"Codex CLI","servers":1}]}"""
  private val resultJson = """{"batch":"mg1794889395614","instructions":{"appended":[],"skipped":[]},"message":"2 servers added.","needs":[{"needs":[{"from":null,"kind":"header","name":"Authorization"}],"server":"sentry"}],"notes":{"added":0,"already":0},"secrets_copied":[],"servers":{"added":["linear","sentry"],"skipped":[]}}"""
  private fun scan(): MigrateScan = ZoJson.decodeFromJsonElement(Json.parseToJsonElement(scanJson) as JsonObject)
  private fun result(): MigrateResult = ZoJson.decodeFromJsonElement(Json.parseToJsonElement(resultJson) as JsonObject)

  @Test fun theServersAnswerIsReadInFull() {
    val s = scan()
    assertEquals("what you pasted", s.origin)
    assertEquals(listOf("github", "linear", "notion", "docs", "sentry").sorted(), s.servers.map { it.name }.sorted())
    val gh = s.servers.first { it.name == "github" }
    assertEquals("program", gh.kind)
    assertEquals(listOf("GITHUB_PERSONAL_ACCESS_TOKEN"), gh.needs.map { it.name })
    assertTrue(gh.needs.single().inFile)
    // a token that only names an environment variable
    val docs = s.servers.first { it.name == "docs" }
    assertEquals("DOCS_TOKEN", docs.needs.single().from)
    assertFalse(docs.needs.single().inFile)
    assertEquals(false, s.can.programs)
    assertEquals(false, s.can.secrets)
    assertEquals("webapp", s.projects.single().name)
    assertEquals("gpt-5-codex", s.extras.single().name)
    assertEquals(1, s.instructions.single().secretLines)
  }

  @Test fun noKeyValueIsInWhatTheScreenGets() {
    for (secret in listOf("ghp_AbCd", "sntrys_", "sk-live")) assertFalse(secret, scanJson.contains(secret))
  }

  @Test fun aProgramOnTheCloudServerAndADuplicateCannotBeTicked() {
    val s = scan()
    val gh = s.servers.first { it.name == "github" }
    assertEquals("Programs run on a computer: add this one in the desktop app.", ImportLogic.blocked(gh, s.can))
    assertNull(ImportLogic.blocked(s.servers.first { it.name == "sentry" }, s.can))
    val dup = s.servers.first { it.name == "notion" }.copy(duplicate = "here", duplicateOf = "notion")
    assertEquals("Already added as notion (kept as it is).", ImportLogic.blocked(dup, s.can))
    assertEquals("The same server is listed above; pick one.", ImportLogic.blocked(dup.copy(duplicate = "scan"), s.can))
    assertEquals("MCP servers are not available to this account.", ImportLogic.blocked(dup.copy(duplicate = ""), MigrateCan(servers = false)))
  }

  @Test fun theFirstSelectionIsEverythingThatCanBeImported() {
    val s = scan()
    val sel = ImportLogic.defaultSelection(s)
    assertFalse(s.servers.first { it.name == "github" }.id in sel)
    assertTrue(s.servers.first { it.name == "sentry" }.id in sel)
    assertTrue(s.instructions.single().id in sel)
    // a restricted account: no servers at all, the instructions stay
    val r = s.copy(can = MigrateCan(servers = false, programs = false, projects = false))
    assertEquals(setOf(s.instructions.single().id), ImportLogic.defaultSelection(r))
  }

  @Test fun anInstructionFileCanGoToMemoryOrToAProject() {
    val s = scan()
    assertEquals(listOf("notes", "instruction:p1794889395608", "notes:p1794889395608"), ImportLogic.targets(s).map { it.first })
    assertEquals("Add to the instruction of project webapp", ImportLogic.targets(s)[1].second)
    assertEquals("notes", ImportLogic.defaultTarget(s, s.instructions.single()))
    val suggested = s.instructions.single().copy(suggest = "p1794889395608")
    assertEquals("instruction:p1794889395608", ImportLogic.defaultTarget(s, suggested))
    // an account without projects only has the memory, and a suggestion it cannot use falls back
    val r = s.copy(can = MigrateCan(projects = false))
    assertEquals(listOf("notes"), ImportLogic.targets(r).map { it.first })
    assertEquals("notes", ImportLogic.defaultTarget(r, suggested))
  }

  @Test fun theResultListsWhatWasLeftOutAndWhatIsStillNeeded() {
    val r = result()
    assertEquals("2 servers added.", r.message)
    assertTrue(r.batch.startsWith("mg"))
    assertEquals(listOf("linear", "sentry"), r.servers.added.sorted())
    assertEquals(listOf("sentry still needs: Authorization"), ImportLogic.resultLines(r))
    val skipped = r.copy(servers = r.servers.copy(skipped = listOf(org.zohara.agent.core.net.MigrateSkipped(name = "github", why = "A server with this name is already added (nothing was overwritten)."))))
    assertEquals("Left out github: A server with this name is already added (nothing was overwritten).", ImportLogic.resultLines(skipped).first())
  }

  @Test fun filesAreCheckedBeforeTheyAreSent() {
    assertEquals("Paste some text first.", ImportLogic.validate(emptyList()))
    assertEquals("Paste some text first.", ImportLogic.validate(listOf(MigrateFile("a", "  "))))
    assertEquals("", ImportLogic.validate(listOf(MigrateFile("a.md", "- rule"))))
    assertTrue(ImportLogic.validate(listOf(MigrateFile("big", "x".repeat(ImportLogic.MAX_BYTES + 1)))).contains("too much"))
    assertTrue(ImportLogic.validate((1..13).map { MigrateFile("f$it", "x") }).contains("At most 12"))
  }

  @Test fun needsAndLabelsReadWell() {
    val s = scan()
    assertEquals("Authorization (from DOCS_TOKEN)", ImportLogic.needText(s.servers.first { it.name == "docs" }.needs.single()))
    assertEquals("github  (runs a program)", ImportLogic.serverTitle(s.servers.first { it.name == "github" }))
    assertEquals("Settings of: Claude Code CLI, Claude Desktop, Codex CLI.", ImportLogic.foundTools(s))
    assertEquals("None of the supported tools left settings there.", ImportLogic.foundTools(s.copy(tools = emptyList())))
    assertNotNull(ImportLogic.INTRO)
    assertEquals("5 lines · 2 notes · 1 line(s) that look like keys are left out", ImportLogic.instructionMeta(s.instructions.single()))
  }
}
