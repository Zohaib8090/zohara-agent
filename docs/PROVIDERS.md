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

## Tests

`cargo test -p zo-core ai::` covers request shapes (golden), parsing of recorded and router-style bodies, streaming reassembly (including
split chunks), the hints, the agent loop against mock servers that speak only `/v1/responses` (streaming and not) and only `/v1/messages`,
provider-list and restricted-account checks. `ai/mock.rs` is the path-aware mock server used by those tests.
