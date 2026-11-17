package org.zohara.agent.settings

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.zohara.agent.core.net.AppCheck
import org.zohara.agent.core.net.AppsCopied
import org.zohara.agent.core.net.AppsCopiedServer
import org.zohara.agent.core.net.AppsPlanResp
import org.zohara.agent.core.net.MachineNew
import org.zohara.agent.core.net.MachineStatus
import org.zohara.agent.core.net.MachineStatusResp
import org.zohara.agent.core.net.ModelPlan
import org.zohara.agent.core.net.RunnerInfo
import org.zohara.agent.core.net.ZoJson

/** The words and the shapes of the Machines screens (docs/RUNNER.md). The JSON below is what the service really answers. */
class MachinesUiTest {
  private val when1: (Long) -> String = { "T$it" }

  @Test fun theStatusAnswerOfTheServiceDecodes() {
    val json = """{"machines":[{"apps_copied":{"at":1794904039679,"servers":[{"credentials":true,"name":"notion"}]},"commands":false,"files":false,"has_key":true,"installed":true,"last_sync":1,"ms":182,"name":"vm1","reachable":true,"runner":{"apps":1,"arch":"x86_64","enabled_tasks":2,"has_key":false,"has_model":true,"installed":true,"last_run":1794904054580,"last_status":"skipped","model":"claude / opus","next_run":null,"ok":true,"os":"linux","tasks":3,"timer_active":null,"timer_enabled":null,"version":"0.1.0"},"sync_error":""},{"name":"gone","reachable":false,"installed":false,"error":"Connection refused"}],"ok":true,"terminal":false}"""
    val r = ZoJson.decodeFromString<MachineStatusResp>(json)
    assertEquals(2, r.machines.size)
    val a = r.machines[0]
    assertTrue(a.reachable && a.installed && a.hasKey)
    assertEquals("0.1.0", a.runner?.version)
    assertEquals(2, a.runner?.enabledTasks)
    assertNull(a.runner?.timerActive)
    assertEquals("notion", a.appsCopied?.servers?.single()?.name)
    assertEquals("Connection refused", r.machines[1].error)
    assertNull(r.machines[1].runner)
  }

  @Test fun thePlansAndTheWizardAnswerDecode() {
    val plan = ZoJson.decodeFromString<AppsPlanResp>("""{"copied":null,"has_runner":true,"machine":"vm1","ok":true,"servers":[{"copied":true,"copied_with_credentials":true,"credentials":["variable NOTION_TOKEN"],"kind":"local","name":"notion","oauth":false,"runtime":"npx","target":"npx","warnings":["a"]}]}""")
    assertTrue(plan.hasRunner)
    assertEquals(listOf("variable NOTION_TOKEN"), plan.servers.single().credentials)
    assertTrue(plan.servers.single().copiedWithCredentials)
    val made = ZoJson.decodeFromString<MachineNew>("""{"name":"vm1","public_key":"ssh-ed25519 AAAA x","authorize_command":"mkdir -p ~/.ssh && echo 'k'","ok":true}""")
    assertEquals("ssh-ed25519 AAAA x", made.publicKey)
    assertTrue(made.authorizeCommand.startsWith("mkdir"))
    val mp = ZoJson.decodeFromString<ModelPlan>("""{"can":false,"confirm":"","model":"","needs_key":false,"ok":true,"provider":"","why":"No model is chosen here yet."}""")
    assertFalse(mp.can)
    assertEquals("No model is chosen here yet.", mp.why)
  }

  @Test fun theDotHasFourStates() {
    assertEquals(MachineDot.Wait, MachineText.dot(null))
    assertEquals(MachineDot.Bad, MachineText.dot(MachineStatus("a", reachable = false)))
    assertEquals(MachineDot.Warn, MachineText.dot(MachineStatus("a", reachable = true)))
    assertEquals(MachineDot.Ok, MachineText.dot(MachineStatus("a", reachable = true, installed = true)))
  }

  @Test fun theStateLineSaysWhatAPersonNeeds() {
    assertEquals("Checking…", MachineText.state(null, when1))
    assertEquals("Not reachable: Connection refused", MachineText.state(MachineStatus("a", error = "Connection refused"), when1))
    assertEquals("Reachable · no runner", MachineText.state(MachineStatus("a", reachable = true), when1))
    val ok = MachineStatus("a", reachable = true, installed = true, runner = RunnerInfo(version = "0.1.0", enabledTasks = 1, timerActive = false, lastRun = 5, lastStatus = "ok"))
    assertEquals("Reachable · runner 0.1.0 · timer off · 1 task · last run T5 (ok)", MachineText.state(ok, when1))
    assertEquals("Reachable · runner 0.1.0 · 2 tasks", MachineText.state(ok.copy(runner = RunnerInfo(version = "0.1.0", enabledTasks = 2)), when1))
  }

  @Test fun theNamedConfirmationIsTheServicesOwnSentence() {
    assertEquals("This copies the notion credentials to vm1.", MachineText.sentenceOf("Confirm exactly: This copies the notion credentials to vm1."))
    assertNull(MachineText.sentenceOf("There is no machine called \"x\"."))
    assertNull(MachineText.sentenceOf("Confirm exactly: "))
  }

  @Test fun checkAndCopiedTexts() {
    assertEquals("Everything the apps need is on the machine.", MachineText.missing(listOf(AppCheck("a", true))))
    assertEquals("a: Install node. b: Install uv.", MachineText.missing(listOf(AppCheck("a", false, "Install node."), AppCheck("b", false, "Install uv."), AppCheck("c", true))))
    assertTrue(MachineText.copied(MachineStatus("a")).startsWith("Copy your connected apps"))
    assertEquals("Copied: notion (with credentials), files", MachineText.copied(MachineStatus("a", appsCopied = AppsCopied(1, listOf(AppsCopiedServer("notion", true), AppsCopiedServer("files", false))))))
    assertTrue(MachineText.taskDone(true, "vm1").startsWith("Sent to the runner on vm1"))
    assertTrue(MachineText.taskDone(false, "vm1").contains("refused here"))
  }

  @Test fun theWizardChecksTheFirstStepBeforeAskingTheServer() {
    assertEquals("", MachineText.validateNew("vm-1", "203.0.113.7", "ubuntu", "22"))
    assertTrue(MachineText.validateNew("bad name", "h", "u", "22").contains("letters"))
    assertEquals("The port is a number from 1 to 65535.", MachineText.validateNew("a", "h", "u", "0"))
  }
}
