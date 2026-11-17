package org.zohara.agent.settings

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onAllNodesWithContentDescription
import androidx.compose.ui.test.onAllNodesWithText
import androidx.compose.ui.test.onFirst
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performScrollTo
import androidx.compose.ui.unit.dp
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
import org.zohara.agent.core.net.AccountInfo
import org.zohara.agent.core.net.ProfilesResp
import org.zohara.agent.core.net.ProvidersResp
import org.zohara.agent.core.net.StatusInfo

/** The Settings sections for named custom providers and API key accounts, and the account drop-down of the model sheet. */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [35], qualifiers = "w400dp-h2400dp-hdpi")
class CustomProvidersAndAccountsUiTest {
  @get:Rule val rule = createComposeRule()

  private fun themed(content: @Composable () -> Unit) = rule.setContent {
    ZoAgentTheme(true) { Column(Modifier.background(ZoTheme.colors.bg).width(400.dp).padding(horizontal = 16.dp).verticalScroll(rememberScrollState())) { content() } }
  }

  private fun acct(id: String, name: String, def: Boolean = false, hidden: Boolean = false, key: Boolean = true, active: Boolean = false) = AccountInfo(id, name, def, hidden, null, key, active)
  private val groq = provider("groq", "Groq", hasKey = true).copy(takesAccounts = true, accounts = listOf(acct("default", "Default", true, active = true), acct("groq-2", "Groq 2", key = false)))
  private val router = provider("custom:my-router", "My router", needsUrl = true, custom = true, format = "responses", baseUrl = "https://router.example.com/v1", hasKey = true)
    .copy(takesAccounts = true, accounts = listOf(acct("default", "Default", true)), group = "custom")
  private val home = provider("custom:home-ollama", "Home Ollama", needsUrl = true, custom = true, format = "claude", baseUrl = "http://192.168.1.20:11434/v1").copy(takesAccounts = true, accounts = listOf(acct("default", "Default", true, key = false)))

  private fun data(rotate: Boolean = false) = ModelPageData(
    StatusInfo(ready = true, provider = "groq", model = "llama"), ProfilesResp("", emptyList()),
    ProvidersResp(current = "groq", currentAccount = "groq-2", rotateOnLimit = rotate, providers = listOf(groq, router, home)), emptyList(), true, null,
  )

  @Test fun theModelLineNamesTheProviderAndTheAccountInUse() {
    themed { ModelPageContent(data(), false, emptySet(), ModelPageActions()) }
    rule.onNodeWithText("Groq · Groq 2 · llama").assertIsDisplayed()
  }

  @Test fun customProvidersAreListedWithTheirFormatAddressAndKeyAndHaveEditTestDelete() {
    val log = mutableListOf<String>()
    themed {
      ModelPageContent(
        data(), false, emptySet(),
        ModelPageActions(onEditCustom = { log += "edit:" + it.id }, onTestCustom = { log += "test:" + it.id }, onDeleteCustom = { log += "delete:" + it.id }, onAddEndpoint = { log += "add" }),
      )
    }
    rule.onNodeWithText("Custom providers").performScrollTo().assertIsDisplayed()
    rule.onNodeWithText("OpenAI Responses (/v1/responses) · https://router.example.com/v1 · key saved").performScrollTo().assertIsDisplayed()
    rule.onNodeWithText("Claude format (/v1/messages) · http://192.168.1.20:11434/v1 · no key").assertIsDisplayed()
    rule.onAllNodesWithText("Edit").onFirst().performScrollTo().performClick()
    rule.onAllNodesWithText("Test").onFirst().performScrollTo().performClick()
    rule.onNodeWithContentDescription("Delete the custom provider Home Ollama").performScrollTo().performClick()
    rule.onNodeWithText("Add custom provider…").performScrollTo().performClick()
    assertEquals(listOf("edit:custom:my-router", "test:custom:my-router", "delete:custom:home-ollama", "add"), log)
  }

  @Test fun anEmptyListSaysSoAndOffersTheForm() {
    val d = data().copy(providers = ProvidersResp(providers = listOf(groq)))
    themed { ModelPageContent(d, false, emptySet(), ModelPageActions()) }
    rule.onNodeWithText("No custom providers yet").performScrollTo().assertIsDisplayed()
  }

  @Test fun theAccountsOfAProviderAreListedAndTheRateLimitSwitchIsOffByDefault() {
    val log = mutableListOf<String>()
    themed {
      ModelPageContent(
        data(), false, emptySet(),
        ModelPageActions(
          onEditAccount = { p, a -> log += "edit:${p.id}/${a.id}" }, onHideAccount = { p, a -> log += "hide:${p.id}/${a.id}" },
          onDeleteAccount = { p, a -> log += "delete:${p.id}/${a.id}" }, onAddAccount = { log += "add:" + it.id }, onRotate = { log += "rotate:$it" },
        ),
      )
    }
    rule.onNodeWithText("API key accounts").performScrollTo().assertIsDisplayed()
    // the first provider with a key is Groq: its Default account and Groq 2
    rule.onNodeWithText("Groq · Default").performScrollTo().assertIsDisplayed()
    rule.onNodeWithText("Key saved · in use").assertIsDisplayed()
    rule.onNodeWithText("Groq · Groq 2").assertIsDisplayed()
    rule.onNodeWithText("No key").assertIsDisplayed()
    // the Default account cannot be deleted, another one can
    assertTrue(rule.onAllNodesWithText("x").fetchSemanticsNodes().isEmpty())
    rule.onNodeWithContentDescription("Delete the account Groq 2").performScrollTo().performClick()
    rule.onAllNodesWithText("Hide").onFirst().performScrollTo().performClick()
    rule.onNodeWithText("Add account…").performScrollTo().performClick()
    rule.onNodeWithContentDescription("Switch to the next account at a rate limit").performScrollTo().performClick()
    assertEquals(listOf("delete:groq/groq-2", "hide:groq/default", "add:groq", "rotate:true"), log)
    assertTrue(rule.onAllNodesWithContentDescription("Delete the account Default").fetchSemanticsNodes().isEmpty())
  }

  @Test fun theSheetShowsTheAccountDropdownWithAddAnAccount() {
    val state = ModelSheetState(loading = false, providers = listOf(groq, router), providerId = "groq", accountId = "groq-2", chosen = "m")
    val log = mutableListOf<String>()
    themed { ModelSheetBody(state, false, ModelSheetActions(onAccount = { log += it })) }
    rule.onNodeWithText("Account").assertIsDisplayed()
    rule.onNodeWithText("Groq · Groq 2").performClick()
    rule.onNodeWithText("Add an account…").performClick()
    rule.onNodeWithText("Add a custom provider…").assertIsDisplayed()
    assertEquals(listOf(ADD_ACCOUNT), log)
  }

  @Test fun aProviderWithNoKeyHasNoAccountDropdown() {
    val state = ModelSheetState(loading = false, providers = listOf(provider("ollama", "Ollama", local = true, needsUrl = true)), providerId = "ollama")
    themed { ModelSheetBody(state, false, ModelSheetActions()) }
    assertTrue(rule.onAllNodesWithText("Account").fetchSemanticsNodes().isEmpty())
  }

  @Test fun theKeyChipFollowsTheAccountInUse() {
    val state = ModelSheetState(loading = false, providers = listOf(groq), providerId = "groq", accountId = "groq-2")
    themed { ModelSheetBody(state, false, ModelSheetActions()) }
    rule.onNodeWithText("No key yet").assertIsDisplayed()   // Groq 2 has none, though the Default account has
  }
}
