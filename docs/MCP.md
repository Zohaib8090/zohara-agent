# MCP in Zohara Agent: what other apps do, and what Zohara Agent does

Written 2026-11-08 from the MCP specification (version 2026-07-28), Claude's connector documentation and the ChatGPT/Codex MCP
documentation. Zohara Agent is an **MCP client** (it uses other people's servers: `crates/zo-core/src/mcp_client.rs`, `mcp_oauth.rs`) and an
**MCP server** (`zo-mcp`: lets Claude Code or any client control this computer).

## How the apps connect

| | Claude (web, Desktop, mobile) | ChatGPT / Codex | Zohara Agent |
|---|---|---|---|
| Local program (stdio) | Claude Desktop: `claude_desktop_config.json` (`command`, `args`, `env`) or a `.mcpb` extension | `command`, `args`, `env`, `cwd` | `command` + `env`; "Paste a config" reads the same JSON |
| Remote server | Streamable HTTP, by URL ("custom connectors") | Streamable HTTP (older HTTP+SSE also), by URL | Streamable HTTP, by URL |
| Static key | `Authorization` / `X-API-Key` headers | bearer token, `http_headers` | `headers` |
| Sign-in | OAuth 2.0 with PKCE | OAuth 2.0 with PKCE | OAuth 2.0 with PKCE |

## OAuth and the redirect address

The redirect address belongs to the **app that signs in**, not to the server:

* Claude's hosted apps use one fixed address, `https://claude.ai/api/mcp/auth_callback` (also `claude.com`).
* ChatGPT uses `https://chatgpt.com/connector_platform_oauth_redirect`.
* A desktop program (Claude Code, Codex, Zohara Agent) opens a small web address **on your own computer**, `http://127.0.0.1:<port>/callback`
  (RFC 8252), and the browser sends the answer there. Servers should accept any port for these.

How a client gets known to a server (newest first in the 2026-07-28 spec): a **Client ID Metadata Document** (the client's id is an
https address that describes it), a client **registered by hand** (id and secret typed in), or **Dynamic Client Registration**
(RFC 7591, now marked old but still the most common). Zohara Agent does registered-by-hand and Dynamic Client Registration. A metadata
document needs a public https address for Zohara Agent; that is not set up yet.

The steps Zohara Agent follows: the server answers 401 with a `WWW-Authenticate` header that names the page describing it (RFC 9728) → that
page names the authorization server → its metadata (RFC 8414) gives the addresses → register → open the browser with PKCE and the
`resource` of the server (RFC 8707) → catch the code on the local address → trade it for tokens, which are kept in the keyring.
Scopes: the ones you typed, else the ones the 401 named, else everything the server lists, plus `offline_access` when allowed.
Access tokens are renewed shortly before they run out (Claude renews 5 minutes before) and again on a 401.

## What Zohara Agent supports

* Tools, including servers that hand out their tool list in pages. A tool with `readOnlyHint` runs without asking in Auto mode, the
  rest ask (like Codex's "writes" mode).
* Resources and prompts: through four tools per server (`zo_resources_list`, `zo_resources_read`, `zo_prompts_list`, `zo_prompts_get`).
* A server's own `instructions` are given to the model.
* Remote: `MCP-Protocol-Version` and `Mcp-Session-Id` headers, a new session when the server forgot the old one, the session closed
  at the end, replies as JSON or as an event stream, a site address without `/mcp` (it is found), custom headers, a pasted token.
* Sign-in: automatic registration, or a client id and secret you registered; a fixed local port; your own scopes; token in the form
  or in a Basic header; `invalid_grant` is reported as "sign in again".
* Adding: by hand, by pasting the JSON other apps use (`mcpServers`, `servers`, a bare server), or by asking the AI
  (`zohara_ai_add_mcp_server`, `zohara_ai_add_mcp_config`).

## Not supported yet

* The old HTTP+SSE transport (servers that only have a `/sse` address).
* Client ID Metadata Documents; step-up sign-in when a call is refused for a missing scope (403 `insufficient_scope`).
* Server-initiated requests: sampling (the server asks the model), roots, elicitation (the server asks you a question), and
  notifications such as "the tool list changed" (the list is read again at the start of each message, so this only matters
  inside one message).
* Showing resources and prompts in the interface (the model can use them; there is no `@` picker or `/` list yet).
* Extensions in the `.mcpb` format.

## Import from other tools

Settings > Import finds the MCP servers (and instruction files) that Claude Code, Claude Desktop, Codex CLI, Gemini CLI, Cursor, VS Code and Zed already have and brings the ticked ones in: docs/MIGRATE.md. A server whose key is kept in the key store (`secret_env`, `secret_headers` of `ServerConfig`) never has it in `mcp-servers.json`.

## Browse connectors (the directory)

Settings > Connected apps > **Browse connectors** (desktop, website and Android app) lists remote MCP servers to add with one click. Built
2026-11-14 from `docs/CONNECTORS-DIRECTORY-PLAN.md`; code in `crates/zo-core/src/connectors/` (`mod.rs`: registry, parser, filter, cache;
`add.rs`: the Add steps), `crates/zo-server/src/connectors_api.rs`, `crates/zohara-agent/src/connectors_ui.rs`, the page in
`crates/zo-server/web/index.html` (`connectorsBrowse`), and `SettingsConnectors.kt` on Android.

**Source.** The official MCP registry, `https://registry.modelcontextprotocol.io/v0/servers?limit=50&version=latest[&search=][&cursor=]`
and `/v0/servers/<id>/versions/latest`: plain GETs, no key, nothing about the person, JSON only, at most 3 MB, no redirects, 15 s.
Shown: status `active`, `isLatest`, and a `streamable-http` remote at a public `https://` address (the old SSE type is not supported), or an
npm/pypi package with literal arguments (second list). Titles and descriptions are third-party text: control characters removed, cut
short. No icon is loaded from an entry (the publisher would learn who is browsing): the clients draw a letter. Category comes from words
in the id, title and description (dev, docs, chat, data, finance, media, search, other).

**Cache and limits.** The Zohara Agent server keeps each answer 1 hour; a failed request is not repeated for 5 minutes, and the last good
answer is served meanwhile (`stale: true`, with a sentence). At most 40 requests a minute go to the registry for the whole process, and 30
a minute per person reach the server's `GET /api/connectors?q=&cursor=&kind=remote|local&category=` (any sign-in; a restricted account gets
`{"restricted":true,"message":...,"entries":[]}`). The website and the desktop use the endpoint / the `connectors` op; the Android app uses
the `connectors` op (it only posts). A desktop that is signed in asks the server (the registry never sees its address); **a desktop
that is not signed in asks the registry itself with the same plain GET and no identifier.** `ZO_CONNECTORS_REGISTRY_URL` points the
server (or the desktop) at a mirror; tests use a fake registry on 127.0.0.1.

**Featured.** `FEATURED` in `connectors/mod.rs` (Notion, GitHub, Linear, Atlassian, Sentry, Stripe, PayPal, Cloudflare, Figma, Zapier,
Vercel, Supabase). Each id was found in the live registry on 2026-11-14 and is asked for again at every refresh; one that the registry does
not answer for is not shown. Slack, Asana, Intercom and Spotify were not in the registry under their own names that day, so they are not
listed (add them to `FEATURED` when they appear). Featured ones are labelled "Featured by Zohara Agent"; every other entry says "Not
reviewed by Zohara Agent".

**Add** is two steps (ops `connector_preview`, then `connector_add`; `connector_cancel` drops a try). The address always comes from the
registry's own record, never from the caller; it must be public https (same guard as model servers, and the host is looked up: a name
that resolves to a private address is refused, and a server from the directory is connected to with an agent that re-checks at connect time
and follows no redirect). Keys the entry asks for (headers such as `Authorization: Bearer ...`, shown with the header's name; a bare token
for `Authorization` becomes a bearer token) go to the key store, never into `mcp-servers.json`. The server is tried (a sign-in opens the
person's browser on the desktop; on the cloud server a sign-in cannot finish, so only key-based ones work there), the tool names are
shown with the notice that every call still asks in Manual and Auto mode, and only a second yes saves it. Nothing is left behind after a
failed try or Cancel. A saved connector remembers its registry id (`registry`), which marks it "Added" and is never synced.

**Local (npm/pypi) entries** are listed in "Runs on this computer" with the runtime they need. Adding writes `npx -y <package>@<version>` or
`uvx <package>@<version>` only after the person ticks a confirmation naming the exact command (the op needs the command typed back as
`confirm`), and only when `npx`/`uvx` exist. They are never added on the cloud server, and the website and the phone only explain this.

**Address changed.** Once per desktop run (and by the `connector_check` op) the directory is asked about every connector added from it. One
whose saved address is no longer listed is held (not started) and shows "Address changed: the directory now lists ...". "Use the new
address" forgets the old sign-in and keys (they must not go to the new address) and switches it on; switching it on as it is keeps the old
address. A server that arrives by sync with another address is treated the same way (it already came in held, without keys).

**Restricted accounts** on the cloud get no MCP; the browse screen says why and shows no Add. `connector_*` ops are refused for them.

**Tests.** `cargo test -p zo-core --lib connectors` (recorded registry JSON in `crates/zo-core/data/registry-page.json` and
`registry-featured.json`: entries with several remotes, only packages, deprecated, old versions, bad addresses; the filter, cache and
fallback, the rate limit, big/non-JSON answers, SSRF refusals, the Add flow against a fake MCP server, the restricted account, address
changes), `cargo test -p zo-server connectors_directory`, `ConnectorsApiTest` on Android.
