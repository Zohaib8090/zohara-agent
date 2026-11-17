# Migrate: bring settings in from other tools (Settings > Import)

Built 2026-11-17 from `docs/VM-MIGRATION-PLAN.md` section 3. Code: `crates/zo-core/src/migrate/` (`mod.rs`, `parse.rs`, `source.rs`, `tests.rs`),
the desktop page `crates/zohara-agent/src/migrate_ui.rs`, the website page `pageImport` in `crates/zo-server/web/index.html`, and the Android
screen `settings/SettingsImportScreen.kt` with `core/net/MigrateApi.kt`.

## What it reads

Read-only. The same detectors run on this computer, on a machine added in Settings > Machines (over SSH), or on text that was pasted or uploaded.

| Tool | Files (relative to the home folder; the Linux, macOS and Windows places are all tried) | What is taken |
|---|---|---|
| Claude Code CLI | `.claude.json` (user servers, and the "local" servers of each project folder), `.claude/settings.json`, `.claude/CLAUDE.md`, `.claude/commands/`, `.claude/skills/`; in every project folder Claude Code knows (at most 40, inside the home folder): `.mcp.json`, `CLAUDE.md` | servers; the instruction files; command and skill NAMES and the model, listed for reference only |
| Claude Desktop | `.config/Claude/claude_desktop_config.json`, `Library/Application Support/Claude/...`, `AppData/Roaming/Claude/...` | servers |
| Codex CLI | `.codex/config.toml` (`[mcp_servers.x]`: `command/args/env`, or `url` with `http_headers`, `env_http_headers`, `bearer_token_env_var`, `enabled`), `.codex/AGENTS.md`; project `AGENTS.md` | servers; instruction files; the model |
| Gemini CLI | `.gemini/settings.json` (`mcpServers` with `command`, `httpUrl` or `url`), `.gemini/GEMINI.md`; project `GEMINI.md` | servers; instruction files; the model |
| Cursor | `.cursor/mcp.json`; project `.cursor/mcp.json` | servers |
| VS Code | `.config/Code/User/mcp.json` (`servers`, `inputs`), `Library/Application Support/Code/User/mcp.json`, `settings.json` (`mcp.servers`); project `.vscode/mcp.json` | servers |
| Zed | `.config/zed/settings.json` (`context_servers`: both the `{"command": {"path", "args", "env"}}` and the `{"source": "custom", "command", "args"}` shapes, and remote `url`/`headers`; servers that come from an extension are skipped) | servers |

JSON may have comments and trailing commas (VS Code and Zed write it that way); TOML is read with the `toml` crate. Files are read from the person's own
home folder only: a symbolic link that leads out of it is not followed, anything that is not a regular file is skipped, 8 MB at most per file. Over SSH one
read-only command per round prints the files between random markers (`cat`-like: `head -c`, `ls`; no redirection and no writing; the marker is random per run
so a file cannot fake the end of the output); the project folders named inside a remote file are only asked for when they are plain absolute paths.

A server that uses the older SSE transport (`"type": "sse"`, or Gemini's `url`) is listed with a note: Zohara Agent speaks streamable HTTP.

## What the screens see (never a secret)

`migrate_scan` answers with names, addresses with their secret parts hidden, which variables are needed, and a short preview of instruction files. A value is a
secret when its NAME says so (`TOKEN`, `KEY`, `SECRET`, `PASSWORD`, `Authorization`, `Cookie`...), when it looks like a key (well-known prefixes such as `ghp_`,
`sk-`, `xoxb-`; long random strings), or when it is not plainly harmless (a flag, a number, a plain word, a path, a plain address, a media type are harmless). Also
hidden: the user name and password of an address, secret parts of a path (`/s/<token>/mcp`) or query (`?key=`), the value after `--api-key`, a connection
string with a password, a `Bearer` header in an argument. A value that is only a reference (`${GITHUB_TOKEN}`, `${env:X}`, `${input:key}`, `$TOKEN`) is never
copied; it is named ("needs GITHUB_TOKEN (from ${GITHUB_TOKEN})").

## Import (`migrate_apply`)

The same source is read again (so a secret never travels to a screen and back), and what the person ticked is imported:

* **Servers** are saved, not started (no third-party program runs at import time). A disabled server stays disabled. A server whose name or command is already
  added is skipped: **nothing is ever overwritten** (rename or remove the existing one first). Two ticked servers with one name: the first wins.
* **Secrets** become an empty placeholder (an env variable with an empty value; a header is left out; a secret argument or address part is replaced by
  `YOUR_VALUE_HERE`) and the result names what is still to fill in. They are copied only when the person ticks "Include the key" for that server (desktop only) AND
  confirms: the dialog lists every server and variable name, and the request must carry the same names back (`confirm`), or it is refused. The values go to
  the key store, not the settings file: environment values as `ServerConfig::secret_env` (key id `mcp-<server>-env-<NAME>`, added to the program's environment
  when it starts), header values as `secret_headers` (as the connectors directory does). Never logged, never in the sync (the sync only carries remote servers
  without secrets, and `secret_env`/`secret_headers` are not part of it), removed by Undo. Without a key store (no `secret-tool`) a server with ticked keys is skipped, not imported with plain-text keys.
* **Instruction files** become memory notes (one per rule: bullets and paragraphs, up to 380 characters and 60 notes per file; headings, code blocks and
  lines that look like a secret are left out), kind `preference` in the whole memory or `project` in one project, or are **appended** to a project's
  instruction under "## Imported from ..." (never replacing what is there; the same text is not appended twice; lines that look like keys are dropped). The
  notes show up in the Memory page and its "Memory updated" feed (source `import`).
* **Undo** (`migrate_undo {batch}`): takes back the servers it added (only if still as imported), the notes it added (only if not edited) and the text it
  appended (only if the instruction still ends with it), and clears the keys it stored. The last 10 imports are kept in `migrate-log.json` (0600, per person;
  it holds names, commands without keys, note texts, appended text; no secret).

Not imported, only listed: custom commands and skills of Claude Code, and the model/provider of Claude Code, Codex and Gemini (choose the model in Settings >
Model).

## Where the files are read

| Client | Sources |
|---|---|
| Desktop (Settings > Import) | This computer; a machine ("Detect on <VM>", read-only over SSH); open files or paste. Keys can be copied here (per server tick + named confirmation). |
| Website | Paste or upload (read by the server, not stored, at most 900 KB); "From <computer>" through the desktop link; a machine for an unrestricted account. No keys. |
| Android | The same as the website (upload through the system file picker). No keys. |

**Through the desktop link** (`migrate_scan`, `migrate_apply`, `migrate_undo` are control operations like `fs_read`, docs/LINK.md): the desktop asks its person
on its own screen first, every time, with a dialog built from what it found (names of tools, servers and files), not from the page's text. It answers with names and previews, never a key, and
`migrate_apply` never copies keys (the job parser drops `secrets` and `confirm`). The server whitelists the three ops and only lets lists of short ids
(`select`, `targets`, `batch`) through.

**On the cloud server** the server's own disk is not the person's: `source: "computer"` is refused. A **restricted** account can import instruction files as
memory notes only (no servers, no projects, no machines, no keys); an unrestricted (admin) account can also import remote servers and read machines, but
not programs ("add this one in the desktop app") and never keys. Every person's imports, notes and history are their own (tests check that another
person cannot see or undo them).

## Ops

| Op | Request | Answer |
|---|---|---|
| `migrate_scan` | `{source: "computer"\|"machine"\|"paste", machine?, files?: [{name, text}]}` (at most 12 files, 2 MB each, 4 MB in all) | `{origin, kind, tools:[{id,name,found,files,servers,instructions}], servers:[{id,tool,tool_name,source,scope,folder,name,kind,target,disabled,needs:[{kind,name,from?,in_file}],notes,duplicate,duplicate_of}], instructions:[{id,...,chars,lines,preview,notes,notes_more,secret_lines,suggest}], extras, notes, projects, can:{servers,programs,projects,secrets}}` |
| `migrate_apply` | the same source, plus `select:[ids]`, `targets:{instruction id: "notes"\|"notes:<project>"\|"instruction:<project>"}`, `secrets:[server ids]`, `confirm:[server names]` | `{message, batch, servers:{added,skipped:[{name,why}]}, notes:{added,already,lines_left_out,more_than_kept}, instructions:{appended,skipped}, needs:[{server,needs}], secrets_copied:[{server,names}]}` |
| `migrate_undo` | `{batch}` | `{message, left_alone}` |
| `migrate_history` | `{}` | `{batches:[{id,time,origin,summary,servers,notes,instructions}]}` |

Item ids are stable hashes of tool, file, scope and name, so `migrate_apply` can find the items again.

## Tests

`cargo test -p zo-core migrate` (fixtures in `migrate/tests.rs`, one per tool, written in each tool's documented format with invented tokens; checks that
no secret appears in anything a screen gets or on disk, key copying only with the tick and the typed names, nothing overwritten, undo, restricted and
admin tenants and isolation, SSH script and marker parsing, home-folder jail), `cargo test -p zo-core link::` (the link route), `cargo test -p zo-server link`
(the server's whitelist), Android `ImportLogicTest` on recorded server answers and `ImportScreenshotTest`.

## Known limits

* Only the files listed above; a tool's other stores (Cursor's rules UI, Claude Code plugins, hooks) are not read.
* Env and header secrets use the key store; a secret that is part of an argument or address (kept only when keys are copied) is saved in `mcp-servers.json` (0600), as for a server added by hand.
* Detection over SSH assumes a POSIX shell and a Linux-style home; a Windows machine is not supported.
* Over the link there is no "Detect on <VM>" (the machine list lives on the desktop); use the desktop page.
