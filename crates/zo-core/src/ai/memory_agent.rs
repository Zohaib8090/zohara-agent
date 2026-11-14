//! The memory agent: a background job that reads what was said in a chat after every few messages of the person and keeps the
//! long-term memory (`store::memory`) up to date: new notes about them, notes that changed or are no longer true, and one
//! summary of the chat so a new chat can be told about it. It asks the model that is chosen for chatting, with no tools, and
//! never blocks the chat: it runs on its own thread, one job at a time.

use super::agent::{self, Config};
use super::Selection;
use crate::store::chats::{self, Entry};
use crate::store::memory;
use serde_json::Value;
use std::collections::BTreeSet;
use std::sync::atomic::AtomicBool;

/// The tenants whose memory agent is busy right now ("" is the one person of a desktop). A job of one tenant never keeps another
/// tenant's from running.
static RUNNING: std::sync::Mutex<BTreeSet<String>> = std::sync::Mutex::new(BTreeSet::new());

fn tenant_key() -> String {
    crate::tenant::current().map(|t| t.id).unwrap_or_default()
}

/// Takes this tenant's place; false when its own job is already running.
fn begin() -> bool {
    RUNNING.lock().unwrap_or_else(|e| e.into_inner()).insert(tenant_key())
}

fn end(key: &str) {
    RUNNING.lock().unwrap_or_else(|e| e.into_inner()).remove(key);
}

#[cfg(test)]
pub(crate) fn __begin_for_test(id: &str) -> bool {
    RUNNING.lock().unwrap_or_else(|e| e.into_inner()).insert(id.to_string())
}

#[cfg(test)]
pub(crate) fn __end_for_test(id: &str) {
    end(id);
}

/// Is a memory job of the current tenant running?
pub fn is_running() -> bool {
    RUNNING.lock().unwrap_or_else(|e| e.into_inner()).contains(&tenant_key())
}

/// Is a memory job of any tenant running?
pub fn any_running() -> bool {
    !RUNNING.lock().unwrap_or_else(|e| e.into_inner()).is_empty()
}

/// A chat that was left alone this long is read even if it has fewer new messages than the memory agent waits for.
const IDLE_MS: u64 = 10 * 60 * 1000;

const SYSTEM: &str = "You are the memory of an AI assistant called Zohara Agent. You read a conversation between Zohara Agent and the person it works for, and keep a short list of what is worth remembering about that person for later conversations. \
You only write down what the person said or clearly showed. You never guess, and you never write down secrets (passwords, keys, tokens, card or ID numbers), and you do not even mention that a secret was shared. You answer with one JSON object and nothing else.";

fn user_count(entries: &[Entry]) -> usize {
    entries.iter().filter(|e| e.kind == "msg-user").count()
}

fn cut(s: &str, n: usize) -> String {
    if s.chars().count() > n {
        format!("{}…", s.chars().take(n).collect::<String>())
    } else {
        s.to_string()
    }
}

/// The part of the chat the memory agent reads: everything from a little before the first message it has not read yet.
fn transcript(entries: &[Entry], read: usize) -> String {
    let mut seen = 0usize;
    let mut start = 0usize;
    for (i, e) in entries.iter().enumerate() {
        if e.kind == "msg-user" {
            if seen + 1 == read.max(1) && read > 0 {
                start = i;
            }
            seen += 1;
        }
    }
    let mut out = String::new();
    for e in &entries[start..] {
        let who = match e.kind.as_str() {
            "msg-user" => "PERSON",
            "msg-ai" => "Zohara Agent",
            _ => continue,
        };
        out.push_str(&format!("{who}: {}\n", cut(&e.text, 1200)));
    }
    // the end of a long chat matters most
    let n = out.chars().count();
    if n > 9000 {
        out = format!("…{}", out.chars().skip(n - 9000).collect::<String>());
    }
    out
}

fn prompt_for(title: &str, chat: &str, text: &str, existing_episode: Option<&str>, project: &str) -> String {
    // every note this chat can see (not only the closest ones), so that the model can merge into an old note instead of writing a new one
    let all = memory::load();
    let mut notes: Vec<&memory::Item> = all.items.iter().filter(|i| i.kind != "episode" && memory::visible(i, project)).collect();
    notes.sort_by(|a, b| b.updated.cmp(&a.updated));
    let known = if notes.is_empty() {
        "(nothing yet)".to_string()
    } else {
        notes.iter().take(60).map(|i| format!("[{}] ({}{}) {}", i.id, i.kind, if i.scope.is_empty() { "" } else { ", this project" }, i.text)).collect::<Vec<_>>().join("\n")
    };
    let name = if project.is_empty() { String::new() } else { crate::store::projects::Store::load().get(project).map(|p| p.name.clone()).unwrap_or_default() };
    let scope_rule = if name.is_empty() {
        String::new()
    } else {
        format!(
            "This chat belongs to the project \"{name}\". Notes marked \"this project\" are only told to chats of this project. For every added note also give \"scope\": \"project\" for a fact about this project's work (its goal, files, decisions, who is involved) or \"global\" for a fact about the person that matters in every chat (who they are, how they like things done, what they use). \
            A note about the person stays global even when it came up here.\n"
        )
    };
    format!(
        "Chat title: {title}\nChat id: {chat}\n\nWHAT IS ALREADY REMEMBERED (the id is in brackets):\n{known}\n\n{ep}{scope_rule}\
        THE CONVERSATION:\n{text}\n\n\
        Write the JSON object with these keys:\n\
        \"add\": a list of {{\"text\": \"...\", \"kind\": \"...\"}} for new things worth remembering. kind is one of: person (who they are: name, work, where they live, family, languages), preference (how they like things done or said, what they like), project (what they are building or working on, with names), fact (anything else lasting: their devices, accounts they use, decisions made). \
        One short sentence each, written in the third person (\"The person ...\"), at most 200 characters. Only things that will still matter in a month. Skip one-off requests, small talk, and anything already remembered.\n\
        KEEP THE LIST SHORT AND TIDY: before you add anything, look at the list above. If a remembered note is about the same subject (the same preference, the same project, the same person), do NOT add a new one: put it in \"update\" with that note's id and a text that joins the old and the new facts in one or two sentences (up to 300 characters). Only use \"add\" for a subject that has no note yet. If a new fact replaces an old one, update that note.\n\
        \"update\": a list of {{\"id\": \"...\", \"text\": \"...\"}} for remembered notes that this conversation changed or made more exact.\n\
        \"delete\": a list of ids of remembered notes that are no longer true or that the person asked to forget.\n\
        \"summary\": two or three sentences about what this whole chat was about and what came of it (what was done, decided, or left open), so that a later chat can be told about it. Use the earlier summary if there is one and add to it. Empty string if the chat has nothing of substance.\n\
        If there is nothing to change, answer {{\"add\": [], \"update\": [], \"delete\": [], \"summary\": \"\"}}.",
        ep = existing_episode.map(|e| format!("EARLIER SUMMARY OF THIS CHAT: {e}\n\n")).unwrap_or_default(),
    )
}

/// What the model answered, read leniently: the object may come inside a code fence or after a few words.
pub fn parse(answer: &str) -> Option<Value> {
    let a = answer.find('{')?;
    let b = answer.rfind('}')?;
    if b < a {
        return None;
    }
    serde_json::from_str(&answer[a..=b]).ok()
}

/// Applies the model's answer to the memory (a chat outside every project). Returns (added, updated, deleted).
pub fn apply(v: &Value, chat: &str, title: &str) -> (usize, usize, usize) {
    apply_in(v, chat, title, "")
}

/// Applies the model's answer for a chat of `project` ("" = none): new notes go to the project or to the whole memory (the model says
/// which; without a word, what is about the person stays global and the rest belongs to the project), and only notes this chat can see
/// are changed or removed. Returns (added, updated, deleted).
pub fn apply_in(v: &Value, chat: &str, title: &str, project: &str) -> (usize, usize, usize) {
    let (mut added, mut updated, mut deleted) = (0, 0, 0);
    for it in v.get("add").and_then(Value::as_array).into_iter().flatten() {
        let text = it.get("text").and_then(Value::as_str).unwrap_or("");
        if looks_secret(text) {
            continue;
        }
        let kind = it.get("kind").and_then(Value::as_str).unwrap_or("fact");
        let scope = if project.is_empty() {
            String::new()
        } else {
            match it.get("scope").and_then(Value::as_str) {
                Some("project") => project.to_string(),
                Some("global") => String::new(),
                _ => memory::default_scope(kind, project),
            }
        };
        if let Some((_, new)) = memory::remember_in(text, kind, chat, &scope) {
            if new {
                added += 1;
            } else {
                updated += 1;
            }
        }
    }
    for it in v.get("update").and_then(Value::as_array).into_iter().flatten() {
        let (id, text) = (it.get("id").and_then(Value::as_str).unwrap_or(""), it.get("text").and_then(Value::as_str).unwrap_or(""));
        if !looks_secret(text) && memory::update_text_in(id, text, project) {
            updated += 1;
        }
    }
    for id in v.get("delete").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
        if memory::forget_in(id, project) {
            deleted += 1;
        }
    }
    if let Some(s) = v.get("summary").and_then(Value::as_str) {
        if !s.trim().is_empty() && !looks_secret(s) {
            memory::set_episode(chat, title, s);
        }
    }
    (added, updated, deleted)
}

/// A last guard: nothing that looks like a key or a password is kept, whatever the model wrote.
pub(crate) fn looks_secret(text: &str) -> bool {
    let t = text.to_lowercase();
    if ["password is", "passcode", "api key is", "secret key", "token is", "pin is"].iter().any(|k| t.contains(k)) {
        return true;
    }
    text.split_whitespace().any(|w| {
        let w = w.trim_matches(|c: char| !c.is_alphanumeric() && c != '-' && c != '_');
        w.len() >= 28 && w.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') && w.chars().any(|c| c.is_ascii_digit()) && w.chars().any(|c| c.is_ascii_alphabetic())
    })
}

/// Reads one chat now: returns what changed, or why it could not. `ask` sends one prompt to the model and returns its answer.
fn process(ask: &dyn Fn(&str) -> Result<String, String>, chat: &str, title: &str, entries: &[Entry], project: &str) -> Result<(usize, usize, usize), String> {
    let count = user_count(entries);
    let read = memory::load().read.get(chat).copied().unwrap_or(0);
    let text = transcript(entries, read);
    if text.trim().is_empty() {
        return Ok((0, 0, 0));
    }
    let episode = memory::load().items.into_iter().find(|i| i.kind == "episode" && i.chat == chat).map(|i| i.text);
    // a project that was deleted meanwhile is no project
    let project = if project.is_empty() || crate::store::projects::Store::load().get(project).is_some() { project } else { "" };
    let answer = ask(&prompt_for(title, chat, &text, episode.as_deref(), project))?;
    let v = parse(&answer).ok_or("The memory agent's answer was not JSON.")?;
    // what changed is written into the change log (the "Memory updated" feed) and announced
    let before = crate::store::memory_log::snapshot();
    let r = apply_in(&v, chat, title, project);
    memory::change(|s| {
        s.read.insert(chat.to_string(), count);
    });
    crate::store::memory_log::record_diff(&before, "engine", chat, title);
    Ok(r)
}

/// How to ask the chosen model one question (no tools). None when no model is set up.
fn asker() -> Option<Box<dyn Fn(&str) -> Result<String, String> + Send>> {
    let sel = Selection::load();
    if sel.provider == "claude-code" {
        if crate::tenant::is_restricted() {
            return None;
        }
        super::claude_code::binary()?;
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        Some(Box::new(move |prompt| super::claude_code::complete("haiku", SYSTEM, prompt, &cancel)))
    } else {
        if sel.provider.is_empty() {
            return None;
        }
        let cfg = Config::from_selection(&sel, SYSTEM).ok()?;
        let cancel = AtomicBool::new(false);
        Some(Box::new(move |prompt| agent::complete(&cfg, SYSTEM, prompt, &cancel)))
    }
}

fn edit_prompt(instruction: &str) -> String {
    let all = memory::load();
    let notes: Vec<String> = all.items.iter().filter(|i| i.kind != "episode").map(|i| format!("[{}] ({}) {}", i.id, i.kind, i.text)).collect();
    format!(
        "WHAT IS REMEMBERED (the id is in brackets):\n{}\n\nThe person now tells you how to change this list:\n{instruction}\n\n\
        Do exactly what they ask and nothing else. Answer with one JSON object with these keys:\n\
        \"add\": a list of {{\"text\": \"...\", \"kind\": \"person|preference|project|fact\"}} for new notes (third person, one or two short sentences).\n\
        \"update\": a list of {{\"id\": \"...\", \"text\": \"...\"}} for notes to rewrite. To join several notes into one, update one of them and delete the others.\n\
        \"delete\": a list of ids to remove.\n\
        \"say\": one short sentence telling the person what you changed.",
        if notes.is_empty() { "(nothing yet)".to_string() } else { notes.join("\n") }
    )
}

/// The memory chat in Settings: the person says what to change ("join my preferences into one note", "forget the laptop model"),
/// the model edits the notes. Returns a sentence about what was done.
pub fn edit_by_instruction(instruction: &str) -> Result<String, String> {
    if instruction.trim().chars().count() < 3 {
        return Err("Say what to change.".into());
    }
    let ask = asker().ok_or("Choose a model first (Settings, Model and profiles).")?;
    let _activity = crate::activity::start(crate::activity::Kind::MemoryJob, "Editing memory", instruction, "desktop", None, None);
    let answer = ask(&edit_prompt(instruction))?;
    let v = parse(&answer).ok_or("The model's answer could not be read. Try again.")?;
    let before = crate::store::memory_log::snapshot();
    let (a, u, d) = apply(&v, "", "");
    crate::store::memory_log::record_diff(&before, "edit", "", "");
    let say = v.get("say").and_then(Value::as_str).unwrap_or("").trim().to_string();
    Ok(if say.is_empty() { format!("{a} added, {u} changed, {d} removed.") } else { format!("{say} ({a} added, {u} changed, {d} removed)") })
}

/// Call after each answer is saved. When the person has written `every` new messages in this chat (or other chats were left idle with unread
/// messages), a background job brings the memory up to date. Does nothing when memory is off or no model is set up. With Claude Code
/// as the model, the small and quick `haiku` does the reading.
pub fn after_turn(chat_id: &str, title: &str, entries: &[Entry]) {
    after_turn_in(chat_id, title, entries, "");
}

/// `after_turn` for a chat that belongs to `project` ("" = none): the notes the job writes may belong to the project.
pub fn after_turn_in(chat_id: &str, title: &str, entries: &[Entry], project: &str) {
    let store = memory::load();
    if !store.enabled || chat_id.is_empty() {
        return;
    }
    let due_here = user_count(entries).saturating_sub(store.read.get(chat_id).copied().unwrap_or(0)) >= store.every.max(memory::MIN_EVERY);
    let now = chats::now_millis();
    let idle: Vec<chats::Chat> = chats::load()
        .into_iter()
        .filter(|c| c.id != chat_id && now.saturating_sub(c.updated) > IDLE_MS && user_count(&c.entries) > store.read.get(&c.id).copied().unwrap_or(0))
        .take(2)
        .collect();
    if !due_here && idle.is_empty() {
        return;
    }
    let Some(ask) = asker() else { return };
    if !begin() {
        return;
    }
    let key = tenant_key();
    let (id, title, entries, project) = (chat_id.to_string(), title.to_string(), entries.to_vec(), project.to_string());
    // registered here (not on the thread) so it shows from this moment, and it ends with the thread whatever happens
    let activity = crate::activity::start(crate::activity::Kind::MemoryJob, "Updating memory", &title, "background", Some(&id), None);
    crate::tenant::spawn(move || {
        let _activity = activity;
        if due_here {
            let _ = process(&*ask, &id, &title, &entries, &project);
        }
        for c in idle {
            let _ = process(&*ask, &c.id, &c.title, &c.entries, &c.project);
        }
        end(&key);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::TEST_ENV_LOCK;

    fn e(kind: &str, text: &str) -> Entry {
        Entry { kind: kind.into(), text: text.into(), ..Default::default() }
    }

    #[test]
    fn reads_only_what_is_new() {
        let es = vec![e("msg-user", "one"), e("msg-ai", "a1"), e("msg-user", "two"), e("msg-ai", "a2"), e("msg-user", "three"), e("msg-tool", "x"), e("msg-ai", "a3")];
        let all = transcript(&es, 0);
        assert!(all.contains("PERSON: one") && all.contains("Zohara Agent: a3") && !all.contains("x\n"));
        // two messages were read: start with a little context, at the second one
        let later = transcript(&es, 2);
        assert!(!later.contains("PERSON: one") && later.contains("PERSON: two") && later.contains("PERSON: three"), "{later}");
    }

    #[test]
    fn parses_an_answer_in_a_fence() {
        let v = parse("Here you go:\n```json\n{\"add\": [], \"summary\": \"x\"}\n```").unwrap();
        assert_eq!(v["summary"], "x");
        assert!(parse("no json here").is_none());
    }

    #[test]
    fn applies_the_answer_and_keeps_secrets_out() {
        let _g = TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("zo-magent-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("XDG_DATA_HOME", &dir);
        memory::clear_all();
        let old = memory::remember("Uses a Samsung A22 as the test phone", "fact", "c0").unwrap().0;
        let gone = memory::remember("Lives in Karachi", "person", "c0").unwrap().0;
        let v: Value = serde_json::json!({
            "add": [{"text": "Builds Zohara OS, an Arch based system with KDE", "kind": "project"}, {"text": "The wifi password is hunter22", "kind": "fact"}, {"text": "token sk-abcdEFGH1234567890abcdEFGH1234567890", "kind": "fact"}],
            "update": [{"id": old, "text": "Uses a Samsung A26 as the test phone"}],
            "delete": [gone],
            "summary": "Fixed the Profiles list on the phone and set up memory."
        });
        let (a, u, d) = apply(&v, "c1", "Profiles");
        assert_eq!((a, u, d), (1, 1, 1));
        let s = memory::load();
        assert!(s.items.iter().any(|i| i.text.contains("A26")));
        assert!(!s.items.iter().any(|i| i.text.contains("hunter22") || i.text.contains("sk-abcd")));
        assert!(!s.items.iter().any(|i| i.text.contains("Karachi")));
        assert_eq!(s.items.iter().filter(|i| i.kind == "episode").count(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_agent_decides_the_scope_and_cannot_touch_another_projects_notes() {
        let _g = TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("zo-magent-scope-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("XDG_DATA_HOME", &dir);
        let other = memory::remember_in("The harbor project ships in March", "project", "c9", "pb").unwrap().0;
        let global = memory::remember("Lives in Karachi", "person", "c0").unwrap().0;
        let v: Value = serde_json::json!({
            "add": [
                {"text": "The atlas project uses a Postgres database", "kind": "project", "scope": "project"},
                {"text": "The person is called Zohaib and writes Rust", "kind": "person", "scope": "global"},
                {"text": "The atlas project is checked on a Samsung phone", "kind": "fact"},
                {"text": "Prefers answers in short sentences", "kind": "preference"}
            ],
            "update": [{"id": other, "text": "Changed from the wrong project"}, {"id": global, "text": "Lives in Lahore"}],
            "delete": [other],
        });
        let (a, u, d) = apply_in(&v, "c1", "Atlas", "pa");
        assert_eq!((a, u, d), (4, 1, 0), "the other project's note is neither rewritten nor deleted");
        let s = memory::load();
        let scope_of = |t: &str| s.items.iter().find(|i| i.text.contains(t)).unwrap_or_else(|| panic!("{t}")).scope.clone();
        assert_eq!(scope_of("Postgres"), "pa");
        assert_eq!(scope_of("Zohaib"), "", "the model said it is about the person");
        assert_eq!(scope_of("Samsung"), "pa", "no word from the model: a fact in a project chat belongs to the project");
        assert_eq!(scope_of("short sentences"), "", "a preference stays global without a word");
        assert_eq!(scope_of("harbor"), "pb");
        assert!(s.items.iter().any(|i| i.text == "The harbor project ships in March"));
        assert!(s.items.iter().any(|i| i.text == "Lives in Lahore"), "a global note can be corrected from a project chat");
        // outside every project the model's "project" scope is ignored: nothing lands in a project that is not there
        let v2: Value = serde_json::json!({"add": [{"text": "The person keeps bees on a farm", "kind": "fact", "scope": "project"}]});
        assert_eq!(apply_in(&v2, "c2", "t", ""), (1, 0, 0));
        assert_eq!(memory::load().items.iter().find(|i| i.text.contains("bees")).unwrap().scope, "");
        // the prompt tells the model about the project's notes and the global ones, never another project's
        let p = prompt_for("Atlas", "c1", "PERSON: hi\n", None, "pa");
        assert!(p.contains("Postgres") && p.contains("Lahore") && !p.contains("harbor"), "{p}");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A model server that answers one request with `content` and hands back what it was asked.
    fn model(content: &str) -> (String, std::sync::mpsc::Receiver<String>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let (tx, rx) = std::sync::mpsc::channel();
        let body = serde_json::json!({"choices": [{"message": {"content": content}}]}).to_string();
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = Vec::new();
            let mut tmp = [0u8; 4096];
            loop {
                let n = s.read(&mut tmp).unwrap();
                buf.extend_from_slice(&tmp[..n]);
                if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&buf[..i]).to_lowercase();
                    let len = head.lines().find_map(|l| l.strip_prefix("content-length:")).and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(0);
                    while buf.len() < i + 4 + len {
                        let n = s.read(&mut tmp).unwrap();
                        buf.extend_from_slice(&tmp[..n]);
                    }
                    break;
                }
            }
            let _ = tx.send(String::from_utf8_lossy(&buf).to_string());
            write!(s, "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len()).unwrap();
        });
        (url, rx)
    }

    #[test]
    fn after_three_messages_the_agent_writes_the_memory_and_the_next_chat_is_told() {
        let _g = TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("zo-magent-e2e-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("XDG_CONFIG_HOME", dir.join("config"));
        std::env::set_var("XDG_DATA_HOME", dir.join("data"));
        let answer = r#"```json
{"add":[{"text":"The person is building Zohara OS and a phone app called Zohara Link","kind":"project"},{"text":"Prefers answers in short sentences","kind":"preference"}],"update":[],"delete":[],"summary":"Tested the Zohara Link phone app and fixed the Profiles list."}
```"#;
        let (url, asked) = model(answer);
        let mut sel = super::super::Selection { provider: "ollama".into(), model: "m".into(), ..Default::default() };
        sel.base_urls.insert("ollama".into(), url);
        sel.save();

        let es = vec![e("msg-user", "test the phone app"), e("msg-ai", "ok"), e("msg-user", "profiles are empty"), e("msg-ai", "fixed"), e("msg-user", "I am building Zohara OS, keep answers short"), e("msg-ai", "noted")];
        let heard: std::sync::Arc<std::sync::Mutex<Vec<Value>>> = Default::default();
        let h2 = heard.clone();
        let _sub = crate::store::memory_log::subscribe(move |v| h2.lock().unwrap().push(v.clone()));
        // two messages: the agent now runs after every 2 messages at the soonest (default 3)
        after_turn("c1", "Phone app", &es[..4]);
        assert!(!is_running());
        // the third: the agent runs
        after_turn("c1", "Phone app", &es);
        for _ in 0..100 {
            if !is_running() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let request = asked.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert!(request.contains("I am building Zohara OS, keep answers short"), "the agent reads the conversation");
        let st = memory::load();
        assert_eq!(st.read.get("c1"), Some(&3));
        assert!(st.items.iter().any(|i| i.kind == "project" && i.text.contains("Zohara Link")), "{:?}", st.items);
        assert!(st.items.iter().any(|i| i.kind == "episode" && i.chat == "c1"));
        // what it did is in the change log and was announced
        let log = crate::store::memory_log::list(0, 10);
        assert_eq!(log.len(), 2, "{log:?}");
        assert!(log.iter().all(|c| c.action == "added" && c.source == "engine" && c.chat == "c1" && c.title == "Phone app"));
        let got = heard.lock().unwrap();
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!((got[0]["summary"].as_str(), got[0]["counts"]["added"].as_u64()), (Some("Added 2 notes"), Some(2)));
        drop(got);

        // a new chat on the phone or the computer is told, without being asked
        let sys = memory::system_with("BASE", "how is my phone app going");
        assert!(sys.starts_with("BASE") && sys.contains("short sentences") && sys.contains("Earlier chat \"Phone app\""), "{sys}");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Needs the real `claude` program, signed in: `cargo test -p zo-core real_claude -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn real_claude_writes_the_memory() {
        let _g = TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("zo-magent-real-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("XDG_CONFIG_HOME", dir.join("config"));
        std::env::set_var("XDG_DATA_HOME", dir.join("data"));
        super::super::Selection { provider: "claude-code".into(), model: "haiku".into(), ..Default::default() }.save();
        let es = vec![
            e("msg-user", "Hi, I'm Zohaib. I'm building an Arch based OS called Zohara OS with KDE Plasma."),
            e("msg-ai", "Nice to meet you, Zohaib. How can I help with Zohara OS?"),
            e("msg-user", "I test my phone app on a Samsung A26. Please keep your answers short, I don't like long explanations."),
            e("msg-ai", "Got it, short answers."),
            e("msg-user", "The wifi password is hunter22 by the way. Can you check the Profiles list in the phone app?"),
            e("msg-ai", "Sure, checking."),
        ];
        after_turn("real1", "Phone app testing", &es);
        for _ in 0..240 {
            std::thread::sleep(std::time::Duration::from_millis(500));
            if !is_running() {
                break;
            }
        }
        let st = memory::load();
        for i in &st.items {
            println!("[{}] {} :: {}", i.kind, i.title, i.text);
        }
        println!("read = {:?}", st.read);
        assert!(st.read.get("real1") == Some(&3), "the agent did not finish");
        assert!(!st.items.is_empty());
        assert!(!st.items.iter().any(|i| i.text.contains("hunter22")));
        println!("{}", memory::briefing("what phone do I test on"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
