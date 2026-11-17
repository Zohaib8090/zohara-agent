# Providers and wire formats

Zohara Agent talks to a model server in one of four wire formats. The provider list (`{"op":"providers"}`) is the single source: the
desktop app, the web app and the Android app all read it from the service, so a provider added in `crates/zo-core/src/ai/providers.rs`
shows up everywhere.

| Format | Endpoint (relative to the address) | Provider id | Notes |
|---|---|---|---|
| OpenAI Chat | `POST {base}/chat/completions` | `custom` (and every hosted OpenAI-compatible provider: OpenRouter, Groq, Ollama, ...) | the default; models from `GET {base}/models` |
| OpenAI Responses | `POST {base}/responses` | `custom-responses` | for agent routers and servers that only speak the Responses API; models from `GET {base}/models` |
| Claude format | `POST {base}/messages` | `custom-claude` (and `anthropic`, which is the real Anthropic API) | key sent as `x-api-key` and as a bearer token; `anthropic-version: 2023-06-01` |
| Gemini | `POST {base}/models/{model}:generateContent` | `gemini` | |

The three **Custom endpoint** providers have an editable address and a key typed for that address (the key is stored under the
provider's own id, so each endpoint has its own). Their labels are:

* `custom`: "Custom endpoint: OpenAI Chat (/v1/chat/completions)"
* `custom-responses`: "Custom endpoint: OpenAI Responses (/v1/responses)"
* `custom-claude`: "Custom endpoint: Claude format (/v1/messages)"

Code that needs "is this a custom endpoint" asks `Provider::is_custom()`, never compares the id with `"custom"`.
A restricted account (the public service) gets the same rules for all three: only a public https address, no private or local
host (`agent::check_restricted_provider`).

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

When a request to a custom endpoint fails because of the format, the error says so in one sentence:

* a body or stream in another shape: "This server speaks the OpenAI Responses format: pick "Custom endpoint: OpenAI Responses" (or set the
  API format to OpenAI Responses)." (the same for the Claude and Chat shapes);
* an error that names another endpoint (`/v1/responses`, `/v1/messages`, "use the Responses API"): the same sentence, with the server's own
  words in brackets;
* 404, 405, 501 or "unsupported" without a name: "The server does not take /chat/completions (...). It may speak another API format: try ...".

`model_test` goes one step further: when the first format fails in one of those ways it tries the other two (8 seconds each) and, if one
answers, returns `{"works": false, "message": "This server speaks the ... format: pick ..."}`. A refused key (401), a busy server (429, 5xx) or
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
