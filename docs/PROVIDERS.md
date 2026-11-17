# Providers and wire formats

Zohara Agent talks to a model server in one of four wire formats. The provider list (`{"op":"providers"}`) is the single source: the
desktop app, the web app and the Android app all read it from the service, so a provider added in `crates/zo-core/src/ai/providers.rs`
shows up everywhere.

| Format | Endpoint (relative to the address) | Provider | Notes |
|---|---|---|---|
| OpenAI Chat | `POST {base}/chat/completions` | a custom provider with format `chat` (and every hosted OpenAI-compatible provider: OpenRouter, Groq, Ollama, ...) | the default; models from `GET {base}/models` |
| OpenAI Responses | `POST {base}/responses` | a custom provider with format `responses` | for agent routers and servers that only speak the Responses API; models from `GET {base}/models` |
| Claude format | `POST {base}/messages` | a custom provider with format `claude` (and `anthropic`, which is the real Anthropic API) | key sent as `x-api-key` and as a bearer token; `anthropic-version: 2023-06-01` |
| Gemini | `POST {base}/models/{model}:generateContent` | `gemini` | |

## Custom providers (named, as many as you want)

A **custom provider** is a server whose address the person types. When adding one they give it a **name** ("My router", "Work gateway",
"Home Ollama") and pick the API format (`chat`, `responses` or `claude`); they can add as many as they like (at most 50). Each has:

| Field | Rules |
|---|---|
| `id` | `custom:<slug>`, made from the name once (`My router` becomes `custom:my-router`; a second `My-Router` gets `custom:my-router-2`). The slug is lowercase letters, digits and single dashes, 1 to 30 characters. It is never the name itself and a rename changes nothing that is stored (profiles, keys, pins, the current choice). It is the only part that ever reaches a file name, a key id or a request. |
| `name` | 1 to 40 characters, no control characters, unique among the custom providers (any letter case). |
| `format` | `chat`, `responses` or `claude`. |
| `base_url` | An http(s) address. A key typed for it is only ever sent to it: changing the address removes the saved key (and the keys of its accounts). A restricted account gets only a public https address (`agent::check_restricted_provider`). |
| `default_model` | Optional: the model offered when the provider is chosen. |
| key | In the key store under the provider id (`custom:my-router`); never in the list. |

The list (never a key) lives in the picker preferences (`ui-prefs.json`, key `custom_providers`), so it travels with them (see
`docs/SYNC.md`). They appear in every picker as their own entries, grouped under **Custom providers**, and can be renamed, edited,
tested, deleted (the key, the accounts, the profiles that use them and the current choice go with it, after a confirm), hidden and shown
(the same switch as the other providers), pinned per provider, and used by profiles. The active selection and the profiles store the
provider id (`custom:<slug>`).

Code asks `Provider::is_custom()` (true for ids that start with `custom:`), never compares with `"custom"`. The code is in
`crates/zo-core/src/ai/custom.rs`.

### Service ops

| Op | Request | Answer |
|---|---|---|
| `custom_provider_list` | `{}` | `{"custom_providers":[{"id","name","format","base_url","default_model","has_key"}],"formats":[...],"max":50}` |
| `custom_provider_add` | `{"name","format","base_url","key"?,"default_model"?}` | `{"id","custom_provider":{...}}`; refused with a sentence for a bad name, format or address (a restricted account: public https only). The key is stored under the new id; if that fails the provider is not kept. |
| `custom_provider_update` | `{"id","name"?,"format"?,"base_url"?,"key"?,"default_model"?}` | `{"id","custom_provider"}`; an empty `key` keeps the saved one; a new address removes it |
| `custom_provider_test` | `{"id"?,"format"?,"base_url"?,"key"?,"model"?}` | `{"works":bool,"message"}`. A saved provider (`id`) or one still being typed (`format` and `base_url`). The typed key (else the saved one, only when the address is unchanged) goes only to that address. With `model`, one small real request is made; a wrong format says "This server speaks the ... format: set the API format of this provider to ...". |
| `custom_provider_remove` | `{"id","confirm":true}` | `{}`; without `confirm` it is refused |
| `providers` | `{}` | also lists the custom providers (`custom: true`, `group: "custom"`, `format`, `base_url`) |

### Older saved settings

Before names existed there were three fixed providers: `custom`, `custom-responses` and `custom-claude`. The first time the new version
reads the configuration it turns the ones the person used (a saved address, key, profile, pins, hidden state or the current choice) into
named ones, once per configuration folder: "Custom endpoint", "Custom endpoint (Responses)" and "Custom endpoint (Claude)", ids
`custom:custom-endpoint`, `custom:custom-endpoint-responses` and `custom:custom-endpoint-claude`. Their address, key, profiles (and the
profiles' keys), pins, hidden state and the current choice move over; an address that today's rules refuse falls back
to `http://127.0.0.1:8000/v1` (which a restricted account refuses too: that provider is then not moved over, its key stays where it was, and it is tried again at the next start). A person who never used them gets none. A profile that arrives by sync from a computer with the old
providers lands on the instance made for it.

## Several accounts (API keys) per provider

Every provider with a key (Groq, OpenAI, Anthropic, Gemini, OpenRouter, Mistral, ..., and the custom ones) can have several named
accounts: "Groq 1", "Groq 2". A local server and Claude Code have none (`Provider::takes_accounts()`). Code: `ai/accounts.rs`.

* The key a provider had before accounts existed is the account **Default** (id `default`). It can be renamed and hidden, not deleted.
* The key store id is per account and the accounts are isolated from each other: the Default account keeps the provider's own id
  (`groq`), another account is `<provider>:<account slug>` (`groq:groq-2`, `custom:home-ollama:work`). A profile's own key is
  `<provider>@<profile>` as before.
* An account has a name (same rules as above, unique per provider), an optional address of its own (only where the provider allows it:
  a hosted provider's account may use another address of the same host, a restricted account only a public https one), and a hidden flag.
  The list lives next to the custom providers in the picker preferences (`accounts`, `default_accounts`), never with a key.
* The pickers show "Groq · Groq 1" (just "Groq" while it has one account). The active selection (`account` in `ai.json`), profiles
  (`account`) and pins store provider and account id; deleting an account sends them back to the Default account.
* Keys are never synced unless key sync is on (`docs/SYNC.md`); an account's key id may travel then, like a provider's.

### Service ops

| Op | Request | Answer |
|---|---|---|
| `account_list` | `{"provider"}` | `{"provider","accounts":[{"id","name","label","default","hidden","base_url","has_key","active"}]}` (Default first) |
| `account_add` | `{"provider","name","key","base_url"?}` | `{"id","accounts"}` (the key is stored under `key_id`; a failure leaves nothing behind) |
| `account_rename` | `{"provider","id","name","key"?,"base_url"?}` | `{"accounts"}`; also changes the key / own address (not for the Default account) |
| `account_hide` | `{"provider","id","hidden"}` | `{"accounts"}` |
| `account_remove` | `{"provider","id","confirm":true}` | `{"accounts"}`; removes the key; the Default account cannot be deleted |
| `account_test` | `{"provider","id","model"?}` | `{"works","message"}`, with that account's key |
| `select`, `models`, `model_test`, `set_key` | `"account": "<id>"` | use (or save the key of) that account; `select` stores it |
| `providers` | | `current_account`, `rotate_on_limit`, and per provider `takes_accounts` and `accounts` |
| `settings_set` | `{"rotate_on_limit":true}` | the setting below |

### When an account hits its rate limit

Setting `rotate_on_limit` (default **off**): "When an account hits its rate limit (HTTP 429 or a quota error), switch to the next
account of the same provider and retry once". The next account is the following one in the list (round the end) that has a key other
than the one that failed and is not hidden. The retry is made once, never to another provider, and the chat shows a small notice
("Switched to Groq 2 (Groq 1 hit its limit)"; the stream event `notice`, `Event::Notice` in the core). Only 429 and quota/rate-limit
errors count; an error like 401 is never retried.

Restricted accounts of the public service keep their own rules: their keys are per person, in their own encrypted store, and custom
and account addresses must be public https.

## What goes over the wire for Responses

Request (`wire::build`, protocol `OpenAiResponses`):

* `instructions` = the system prompt, `store: false`, `stream: true`.
* `input` items: user turns are `{"type":"message","role":"user","content":[{"type":"input_text"|"input_image"}]}`, assistant text is a
  message with `output_text`, a tool call is `{"type":"function_call","call_id","name","arguments"}` (arguments is a JSON string), a
  tool result is `{"type":"function_call_output","call_id","output"}`; a picture a tool made follows as a user message.
* `tools`: flat `{"type":"function","name","description","parameters","strict":false}`.

Answer (`wire::parse`, `wire::StreamAcc`): `output[]` items (`message` with `output_text` parts, `function_call`, `reasoning` ignored),
`usage.input_tokens` / `output_tokens` / `input_tokens_details.cached_tokens`; errors, `failed` and `incomplete` statuses. Streams are read
from `response.output_text.delta`, `response.output_item.added/done`, `response.function_call_arguments.delta/done`,
`response.completed/incomplete/failed` and `error`. Quirks that are tolerated: `data:` without a space, `[DONE]`, comment lines,
`event:` lines, chunks that end in the middle of a line, a usage-only last chunk, a server that sends only the final
`response.completed`, a server that sends JSON (or the pieces under a wrong content type) although a stream was asked for, `null` fields,
`output_text` as a plain string.

## "API format" hint for hosted providers

A hosted OpenAI-compatible provider (for example `openai`) can be told to use another format: `{"op":"select","api_format":"responses"}`
(`chat`, `responses` or `claude`; an empty text takes the default again). It is saved in `ai.json` as `api_formats` and shown in
`providers` as `api_format` (`takes_format` says whether the provider has the choice). The key still only goes to the provider's own
host. `models` and `model_test` accept the same `api_format` to try one without saving it.

## Friendly errors and the automatic hint

When a request to a custom provider fails because of the format, the error says so in one sentence:

* a body or stream in another shape: "This server speaks the OpenAI Responses format: set the API format of this provider to OpenAI
  Responses." (the same for the Claude and Chat shapes);
* an error that names another endpoint (`/v1/responses`, `/v1/messages`, "use the Responses API"): the same sentence, with the server's own
  words in brackets;
* 404, 405, 501 or "unsupported" without a name: "The server does not take /chat/completions (...). It may speak another API format: try ...".

`model_test` goes one step further: when the first format fails in one of those ways it tries the other two (8 seconds each) and, if one
answers, returns `{"works": false, "message": "This server speaks the ... format: set the API format ..."}`. A refused key (401), a busy server (429, 5xx) or
a server that cannot be reached is never retried in other formats.

## Claude Code: every model a subscription can use

With the Anthropic subscription login (provider `claude-code`, the installed `claude` program) the model choice is not limited to
`sonnet`, `opus` and `haiku`. The list lives in one data file, `crates/zo-core/data/claude-models.json`, compiled into the program; to
update it without a release, put a valid copy named `claude-code-models.json` in the data folder (it replaces the shipped one while it
parses; a broken copy is ignored). Each entry has `id`, `label`, `kind` (`default`, `alias` or `model`), `family` (fable, opus, sonnet,
haiku), `version`, `context` (window in tokens, 0 when not known), `released`, `aliases`, `note` and `verified`.

What is in it:

* **Default (what Claude Code picks)**: id `default`. No `--model` is passed; the program chooses for the plan.
* **Shortcuts** (aliases the installed program accepts): `sonnet`, `opus`, `haiku`, `fable`, `opusplan` (Opus while planning, Sonnet
  otherwise) and the long-context `sonnet[1m]` and `opus[1m]`.
* **Full ids by family, newest first**: Fable 5.2 `claude-fable-5-2`, 5.1; Opus 5.5 `claude-opus-5-5`, 4.8, 4.7, 4.6, 4.5; Sonnet 5.5
  `claude-sonnet-5-5`, 4.6, 4.5; Haiku 5.5 `claude-haiku-5-5`, 4.5. The newest of each family carries **Recommended** (computed from
  `version`, never stored). Shortcuts and Default are never marked.
* **Other model id...**: any id typed by hand (letters, digits and `. _ - : /`, an optional `[1m]`, at most 100 characters; the service
  refuses anything else with a sentence). It is passed to `--model` as typed.

`verified: true` means a real one-line call with that id (`claude -p "say ok" --model <id> --output-format json`) was answered on the
maintainer's laptop on the date at the top of the file, and the id the program reported matched. The file's `not_listed` section names
ids that were tried and left out (one the program silently answers with another model, one it refuses). Ids with `verified: false`
are taken from the documentation and are marked that way: update them when they are tried.

**Which model answered.** The `claude` stream names the model in its start line and in every message. The agent loop sends it as the
event `model`; the service sends `{"event":"model","model":"claude-opus-5-5","label":"Opus 5.5","differs":true}` (once per answer, again
only when it changes) and `differs` is true when it is not the choice (a shortcut or Default resolved to a real model). Desktop shows
"Answered by Opus 5.5" under the answer, the web page and Android do the same; nothing is shown when they agree.

**Context bar.** `context::window_for` reads the window from the same file for Claude Code (a typed id ending in `[1m]` is 1M).

**In the three clients.** The `models` op answers for `claude-code` with `models` (ids, catalog order) and `catalog`
(`[{"title":"Shortcuts","models":[...]}, {"title":"Fable",...}, ...]`, newest first, with `recommended`), so desktop, web and Android show
the same list, grouped by family. Pins, profiles and `select {model}` work with these ids like with any other model; the pill and the
pinned list show the label ("Opus 5.5"), not the id.

Tests: `cargo test -p zo-core ai::models` (file schema: ids unique, kinds known, every full id has a family, version, window and a
release date of an allowed shape, the Claude 5 ids are present; recommended and newest-first ordering; bad files refused; labels, windows,
the `--model` argument, typed ids, the answering model), `ai::claude_code` (the stream names the model once and again on a change),
`service::tenant_tests::claude_code_models_carry_the_catalog_and_a_typed_id_is_checked`, the desktop list tests in `modelpick.rs`, Android
`ModelLogicTest`, `SendEventParserTest`, `ChatSessionTest`, `GapCloseSettingsTest` (Compose, Robolectric), and the web sheet checked in the
in-app browser against a sandbox `zo-server`.

## Tests

`cargo test -p zo-core ai::` covers request shapes (golden), parsing of recorded and router-style bodies, streaming reassembly (including
split chunks), the hints, the agent loop against mock servers that speak only `/v1/responses` (streaming and not) and only `/v1/messages`,
provider-list and restricted-account checks. `ai/mock.rs` is the path-aware mock server used by those tests.

The named providers and the accounts have their own tests: `ai::custom::tests` (slugs, names, list checks, the old `custom*` migration,
no key in the list), `service::tenant_tests` (the ops above incl. key isolation, restricted rules, the account test with its own key, the
rate-limit rotation: off, on, once, never another provider, hidden and key-less accounts skipped), `sync::tests` (which key ids may travel,
the instances and accounts in the settings document, secret addresses stay home, restricted refusals, a profile of an old provider).
The desktop forms are tested under Xvfb in the `zs-ai-img` container (`cargo test -p zohara-agent -- --ignored --test-threads=1`), the
Android screens with JUnit/Robolectric/Compose tests (`CustomProvidersAndAccountsUiTest`, `ModelControllerTest`, `ModelLogicTest`).
