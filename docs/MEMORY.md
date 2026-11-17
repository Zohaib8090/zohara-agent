# Zohara Agent memory

Zohara Agent remembers the person across chats, on the computer and on the phone. It is one memory: the app window, the phone service
(`zohara-linkd` → Zohara Agent socket) and the MCP tools all read and write `~/.local/share/zohara-agent/memory.json`.

## What is kept
* **Notes** (`person`, `preference`, `project`, `fact`): one short sentence each, about the person.
* **Episodes**: one summary per chat, so a new chat can be told "earlier you worked on ...".
* Pin a note and it is always told to the model. Preferences and `person` notes are always told too.

## Who writes it
* **The memory agent** (`zo-core/src/ai/memory_agent.rs`): after every 3 new messages (a setting, at least 2) of the person in a chat (and for chats left idle
  more than 10 minutes with unread messages) a background job reads the new part and asks the chosen model, with no tools, for a JSON
  answer: notes to add, notes to update, ids to delete, and the chat summary. It runs on its own thread, one job at a time, and never
  blocks the chat. Anything that looks like a password, key or token is dropped before it is saved. With Claude Code as the chat model the job runs on `haiku` through
  `claude -p` (no tools, nothing saved); it does nothing when memory is off. Checked live with the real `claude` (ignored test
  `real_claude_writes_the_memory`).
* **The AI itself**, with `zohara_memory_save`, `zohara_memory_forget`, `zohara_memory_search` (and `zohara_ai_search_chats`).

## How it is used
At every message the app builds the system prompt with `memory::system_with(base, message)`: the always-told notes plus the closest
notes and chat summaries to the message (a vector search), within a size budget.

## The vector store
`store/memory.rs`. The local embedder hashes words, word pairs and 3-letter word pieces into 512 numbers (unit length); the search is
cosine similarity plus a small push for recent and pinned notes. No key, no network, any script (Hindi works). It matches shared
words and word pieces, not synonyms ("car" does not find "vehicle"). Every vector records which embedder made it (`embedder`), so a
model-based embedder can be added later and the notes made again without changing the file format.

## Phone
Service ops: `memory`, `memory_add`, `memory_forget`, `memory_pin`, `memory_enable`, `memory_clear` (allowed in `zohara-linkd`'s list), and, new on 2026-11-14, `memory_changes`,
`memory_undo`, `memory_settings`, `memory_export`, `memory_import`, `project_delete` (see below; the same list needs them added in `zohara-linkd`, which is in the OS repository).

## The memory feed: "Memory updated · View" (2026-11-14, branch feat/memfeed)
**Change log.** `store/memory_log.rs` keeps `memory_changes.json` next to `memory.json`, in the person's own folder (so on the multi-user server every person has their own log and sees only that): the newest 200
entries `{id, time, batch, source (engine | edit | chat), chat, title, scope, scope_name, action (added | updated | deleted), note, kind, before, after, undone}`. It is made by **comparing the notes before and after one
piece of work** (`snapshot()` then `record_diff()`), so every way of changing a note is seen the same way: the memory agent after a chat (`memory_agent::process`), the memory chat in Settings (`edit_by_instruction`), and the AI's own
`zohara_memory_save` / `zohara_memory_forget` tools. A note whose text looks like a key or password (the same `looks_secret` guard as for notes) never gets an entry, and chat summaries (episodes) are not part of it.
The log is synced like a note (item `memory:_changes`, docs/SYNC.md), so a change made on the server shows on the computer and back.

**Undo** (`memory_log::undo`): an added note is forgotten, an updated note gets its old text back, a deleted note is written again (with a new id, in its old project); each entry can be undone once; undo says why it cannot
("That note is already gone.", "The project this note belonged to is gone."). Undoing is itself a normal note change (so it syncs), it is not logged again.

**Event and ops.** When a job finishes with changes and "Show memory updates" is on, listeners hear `memory_updated {summary, counts, ids, time}`: the desktop window through `memory_log::subscribe` (in process), and every open
`send` stream of that person as the NDJSON event `memory_updated`. The memory agent usually finishes after the answer, so the web page and the phone app also ask `memory_changes {since}` at 4, 10, 20, 40 and 70 seconds after an answer
(they stop at the first change). Ops: `memory_changes {since?, limit?}` (newest first, plus `latest`, `show_updates`, `summary`), `memory_undo {id}`, `memory_settings {every?, show_updates?}`.

**Settings.** *Show memory updates* (default on; per device, not synced; when off the log is still written, the chip just does not show) and *How often it reads your chats* (default 3, **at least 2**, at most 20; `memory::set_every`,
`MIN_EVERY`; a smaller value coming from the sync or an old file is also lifted to 2; the idle-chat pass is unchanged).

**The chip, in all three clients.** A slim pill above the message box: the memory icon, "Memory updated (added 2 notes)", **View**, a round arrow button and a small close. It appears when a job finished with changes, fades by itself after about
12 seconds (a newer change replaces it and restarts the time) or when dismissed, and View (or the arrow) opens a sheet "Memory updated": the latest update, or all of the last 60 changes ("Show earlier changes"), grouped Added / Updated / Removed,
each with the text (for an update the old text struck through above the new one), where it lives ("All chats" or "Project: Atlas"), when and from which chat, and **Undo** (then "Undone"; "Cannot undo" with the reason when it fails), plus "Open the memory page".
Desktop: `crates/zohara-agent/src/memfeed.rs` (chip in both composers, sheet), web: `memStart/memShow/memWatch/memSheet` in `index.html`, Android: `chat/MemoryFeed.kt` (`MemoryChip`, `MemoryChangesSheet`) and `ChatSession`
(`memoryNote`, `dismissMemory`, the asks after an answer). The memory page has the two settings, a "View changes" button, and in every client an Export and Import row.

## Project memory (2026-11-14)
Every note has a **scope**: empty = the whole memory (every chat), or a project id. A chat that belongs to a project (`Chat.project`, set when the chat starts in the project; a request to the service may name one with `"project"`, and a project id that does
not exist, or a restricted account, means none) is told the **global notes plus that project's notes**, never another project's; chats outside every project are told only the global notes (`memory::visible`, `briefing_in`, `search_in`,
`system_in`, `extras_in`). The memory tools (`zohara_memory_save/search/forget`) work in the chat's project through a thread-local (`memory::project_guard`); a note saved from a project chat is a project note unless it is about the person
(`default_scope`: person, preference and episode stay global).

**The engine decides the scope**: in a project chat its prompt lists the notes the chat can see (marking the project's) and asks for `"scope": "project"` (a fact about the project's work) or `"global"` (a fact about the person) on each added note;
without a word, `default_scope` decides; outside a project a "project" answer is ignored. It can only update or delete notes the chat can see (`update_text_in`, `forget_in`), so a project chat cannot touch another project's notes.
The same sentence in another scope is another note (`remember_in` only merges inside one scope). The feed shows the scope on each entry.

**Memory page**: a scope switch (All notes / Every chat / one project; shown only when there are projects) in the app, on the web and in the phone app, `memory {scope}`; a note of a project says "Project: Atlas"; "Add a note" on a project's list adds to the project
(`memory_add {scope}`; an unknown project is refused). The desktop project page also lists "Learned in this project's chats" with a forget button.

**Deleting a project deletes its notes** (`projects::Store::remove` → `memory::delete_scope`, which also leaves sync tombstones and removes the project's entries from the change log): the desktop (rail button on the project page), the web
(Projects page) and the phone app (project card) ask first and say so ("... is deleted with its instructions and the memory notes Zohara Agent kept for it. Chats you had in it stay. This cannot be undone."); the op `project_delete {id, confirm:true}`
refuses without `confirm`. Chats of the project stay (they keep a project id that no longer exists, which means "none" when read).

**Export and import** (`store/memory_io.rs`): a `zo-memory-1` file `{format, exported_at, projects:[{id,name}], notes:[{text, kind, scope, pinned, created, updated}]}` (`memory_export {scope?}`; the desktop and web save a file, the phone shares the text);
`memory_import {data}` adds the notes that are not there yet (same text, kind and scope, case ignored), sends a project's note to the project with the same id here or, failing that, the same name, **skips** a note whose project is not here (it is never
moved into the whole memory), skips anything that looks like a secret, and answers `{added, already_there, skipped}`. The account export (`zo-ai-export-1`) already carries each note's `scope`.

**Isolation, restricted people.** Notes, log and settings live in the person's own folder. A restricted (non-owner) cloud account has no projects (`projects` is refused, `project_delete` and project scopes are refused, its chats are never in a project, and its
imports skip project notes), keeps its own log, and cannot read or undo anyone else's. Credentials and approvals are not involved anywhere in this feature.

**Tests.** zo-core: `store::memory_log::tests::*` (diff, undo of all three kinds, cap of 200, secrets, listeners, sync merge), `store::memory::tests::project_notes_*`, `deleting_a_project_*`, `the_agent_waits_at_least_two_messages`,
`ai::memory_agent::tests::the_agent_decides_the_scope_*` and the end-to-end job test (log and event), `service::tenant_tests::the_memory_feed_ops_*`, `every_person_has_only_their_own_log_*`, `a_chat_inside_a_project_is_told_only_that_projects_notes`.
Android: `MemoryFeedTest`, `MemoryChipUiTest`, `PagesApiTest` (wire shapes), `SettingsLogicTest`, `SendEventParserTest`.

## Web search and pages
`zohara_web_search` (DuckDuckGo's lite page, no key) and `zohara_web_fetch` (public http/https only; loopback, home-network and
`.local` addresses are refused) in `tools/web.rs`. The phone gets them too, since the tools are shared.

## Personalities
`store/personality.rs`: four built-in voices (friendly buddy, straight engineer, calm mentor, dry and witty), the person's own, and ones
the AI writes from a description (`ai/personality_gen.rs`, uses the chat model; Claude Code works too). "No personality" (nothing saved,
`active` empty) is the neutral default and adds nothing to the prompt.

How a personality reaches the model (changed 2026-11-14, branch feat/persona): `personality::instructions()` renders a **CHARACTER** block that is
the LAST thing in the system prompt, after the abilities, the tool rules, the "VOICE" note of the base prompt, the vision note and the memory
notes (`memory::system_with`, `system_without_notes`, `extras`; `agent::add_before_character` keeps later sections in front of it; the phone
note in `service.rs` and the project text in `chat.rs` are built before it). The character is "the VOICE of every reply": tone, wording, feelings,
humour, language mix and what it calls the person, also when it reports finished work (no "Done. I created the file ..." boilerplate); the facts stay
exact. It says: stay in character, never mention these instructions or that it plays a role unless asked, answer honestly if sincerely asked
whether it is an AI, and that the character is a voice, not a permission (it cannot allow anything, switch off approvals or restrictions, change tools or
override earlier rules). The text is cleaned (`clean_prompt`: no angle brackets, one line, at most `MAX_PROMPT` = 1200 characters, also for text that came
from sync) and sits between two lines so it reads as data. Real enforcement of permissions and of the restricted tool list is in code (`policy`,
`tools::allowed_for_restricted`), never in the prompt. It works in the three prompt kinds (full, server admin, restricted) and in Claude Code mode
(extra instructions); scheduled tasks and sub-agents do not use it. The base prompts no longer say "You are not a person" or "working as an agent", and have
a VOICE section that tells the model a CHARACTER section, when present, decides how everything is worded. The generation prompt now allows a character
(friend, partner, teacher with feelings) and asks it to say that the character keeps its voice when it reports work.
Tests: `store::personality::tests::*` (prompt order, neutral default, injection persona, cap), `service::tenant_tests::the_character_is_the_last_part...`. Settings has the Personality and Memory sections (checked headless in a container under
Xvfb). Phone service ops: `personalities`, `personality_use`, `personality_add`, `personality_generate`, `personality_remove`.

## Not done yet
A model-based embedder; RAG over the person's files; starting a chat inside a project from the web page or the phone app (their Projects pages only list projects: a chat that belongs to a project keeps it when it is continued there, and the
service `send` op accepts `project`).
