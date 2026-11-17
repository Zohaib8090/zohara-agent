//! The memory change log: what the memory agent (and the memory chat in Settings) added, changed or removed, with the text before and
//! after, so the person can see it ("Memory updated · View") and undo it. See docs/MEMORY.md.
//!
//! One small file (`memory_changes.json`, the newest `KEEP` entries) next to `memory.json`, in the person's own folder: on a
//! multi-user server every person has their own log and sees only that. It never holds a secret: a change whose text looks like a key or
//! a password (the same guard that keeps such text out of the notes) is not written down. The chat summaries ("episodes") are not part
//! of it, only the notes.
//!
//! The log is made by comparing the notes before and after one piece of work (`record_diff`), so every way of changing notes is
//! seen the same way. Listeners (the desktop window, the service's open streams) hear about a finished batch through `subscribe`.

use super::chats;
use super::memory::{self, Item};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// The log keeps this many entries (one per note changed).
pub const KEEP: usize = 200;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Change {
    pub id: String,
    pub time: u64,
    /// Entries made by one piece of work share a batch.
    #[serde(default)]
    pub batch: String,
    /// "engine" (the memory agent after a chat) or "edit" (the memory chat in Settings).
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub chat: String,
    #[serde(default)]
    pub title: String,
    /// "" = the whole memory, else the project id.
    #[serde(default)]
    pub scope: String,
    /// The project's name when it was written (the project may be gone later).
    #[serde(default)]
    pub scope_name: String,
    /// "added", "updated" or "deleted".
    pub action: String,
    /// The note's id (a note that was deleted and brought back has a new id; see `note`).
    #[serde(default)]
    pub note: String,
    /// The note's kind: person, preference, project or fact.
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub before: String,
    #[serde(default)]
    pub after: String,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub undone: bool,
}

#[derive(Default, Serialize, Deserialize)]
struct Log {
    #[serde(default)]
    changes: Vec<Change>,
    /// When the log last changed (an entry written, undone or removed); the sync keeps the newest log.
    #[serde(default)]
    updated: u64,
}

static LOCK: Mutex<()> = Mutex::new(());

fn path() -> PathBuf {
    crate::audit::data_dir().join("memory_changes.json")
}

fn load() -> Log {
    std::fs::read_to_string(path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn save(l: &Log) {
    let p = path();
    let _ = std::fs::create_dir_all(p.parent().unwrap_or(&p));
    let tmp = p.with_extension("json.tmp");
    if let Ok(text) = serde_json::to_string(l) {
        if !crate::tenant::fits(&p, text.len()) {
            return;
        }
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, &p);
        }
    }
}

fn edit<R>(f: impl FnOnce(&mut Log) -> R) -> R {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut l = load();
    let r = f(&mut l);
    l.updated = chats::now_millis().max(l.updated + 1);
    save(&l);
    r
}

/// The notes (not the chat summaries) by id, for comparing before and after.
pub fn snapshot() -> HashMap<String, Item> {
    memory::load().items.into_iter().filter(|i| i.kind != "episode").map(|i| (i.id.clone(), i)).collect()
}

/// What happened between `before` and now: the entries to write, oldest first. A note counts as updated when its text changed.
fn diff(before: &HashMap<String, Item>, after: &HashMap<String, Item>, source: &str, chat: &str, title: &str) -> Vec<Change> {
    let now = chats::now_millis();
    let batch = format!("b{now}");
    let projects: BTreeMap<String, String> = super::projects::Store::load().projects.into_iter().map(|p| (p.id, p.name)).collect();
    let mut out: Vec<Change> = Vec::new();
    let mut n = 0usize;
    let mut push = |action: &str, it: &Item, b: &str, a: &str, out: &mut Vec<Change>| {
        n += 1;
        out.push(Change {
            id: format!("g{now}{n:03}"),
            time: now,
            batch: batch.clone(),
            source: source.into(),
            chat: chat.into(),
            title: title.chars().take(60).collect(),
            scope: it.scope.clone(),
            scope_name: projects.get(&it.scope).cloned().unwrap_or_default(),
            action: action.into(),
            note: it.id.clone(),
            kind: it.kind.clone(),
            before: b.into(),
            after: a.into(),
            pinned: it.pinned,
            undone: false,
        });
    };
    let mut deleted_first: Vec<&Item> = Vec::new();
    let mut ids: Vec<&String> = after.keys().collect();
    ids.sort();
    for id in ids {
        let a = &after[id];
        match before.get(id) {
            None => push("added", a, "", &a.text, &mut out),
            // the same id with another birth time or kind is another note that was given the id of one that went (ids come from the clock)
            Some(b) if b.created != a.created || b.kind != a.kind => {
                deleted_first.push(b);
                push("added", a, "", &a.text, &mut out);
            }
            Some(b) if b.text != a.text => push("updated", a, &b.text, &a.text, &mut out),
            _ => {}
        }
    }
    let mut gone: Vec<&Item> = before.iter().filter(|(k, _)| !after.contains_key(*k)).map(|(_, v)| v).chain(deleted_first).collect();
    gone.sort_by(|a, b| a.id.cmp(&b.id).then(a.created.cmp(&b.created)));
    for b in gone {
        push("deleted", b, &b.text, "", &mut out);
    }
    // never a secret, whatever the model or the note held
    out.retain(|c| !crate::ai::memory_agent::looks_secret(&c.before) && !crate::ai::memory_agent::looks_secret(&c.after));
    out
}

/// Writes what changed since `before` into the log and tells the listeners. Returns the entries written (empty when nothing changed).
pub fn record_diff(before: &HashMap<String, Item>, source: &str, chat: &str, title: &str) -> Vec<Change> {
    let changes = diff(before, &snapshot(), source, chat, title);
    if changes.is_empty() {
        return changes;
    }
    edit(|l| {
        l.changes.extend(changes.iter().cloned());
        let extra = l.changes.len().saturating_sub(KEEP);
        l.changes.drain(..extra);
    });
    if memory::load().show_updates {
        publish(&changes);
    }
    changes
}

/// Entries newer than `since` (milliseconds; 0 = all), newest first, at most `limit`.
pub fn list(since: u64, limit: usize) -> Vec<Change> {
    let mut v: Vec<Change> = load().changes.into_iter().filter(|c| c.time > since).collect();
    v.reverse();
    v.truncate(limit.clamp(1, KEEP));
    v
}

/// The time of the newest entry (0 when there is none).
pub fn latest() -> u64 {
    load().changes.iter().map(|c| c.time).max().unwrap_or(0)
}

pub fn clear() {
    if !load().changes.is_empty() {
        edit(|l| l.changes.clear());
    }
}

/// The log forgets what it holds about a project's notes (the project was deleted, or its notes were).
pub fn forget_scope(project: &str) {
    if load().changes.iter().any(|c| c.scope == project) {
        edit(|l| l.changes.retain(|c| c.scope != project));
    }
}

// ---------- the sync (the log travels as one small item, `memory:_changes`; see docs/SYNC.md) ----------

/// When the log last changed (0 = never): its time in the sync manifest.
pub fn updated() -> u64 {
    load().updated
}

/// The whole log as one document for the sync.
pub fn sync_doc() -> Value {
    let l = load();
    json!({"changes": l.changes, "updated": l.updated})
}

/// Takes in the other side's log if it is newer than ours: the entries of both are kept (an entry undone on either side counts as
/// undone), the newest `KEEP` stay. Entries with a secret in them are dropped. Returns whether anything was taken in.
pub fn sync_import(doc: &Value) -> Result<bool, String> {
    let up = doc.get("updated").and_then(Value::as_u64).filter(|u| *u > 0).ok_or("The memory log has no time.")?;
    let mut theirs: Vec<Change> = serde_json::from_value(doc.get("changes").cloned().unwrap_or(Value::Null)).map_err(|e| format!("The memory log is not valid: {e}"))?;
    if theirs.len() > KEEP * 2 {
        return Err("That memory log is too long.".into());
    }
    let cut = |s: &mut String, n: usize| {
        if s.chars().count() > n {
            *s = s.chars().take(n).collect();
        }
    };
    for c in &mut theirs {
        for f in [&mut c.id, &mut c.batch, &mut c.source, &mut c.chat, &mut c.scope, &mut c.note, &mut c.kind, &mut c.action] {
            cut(f, 80);
        }
        for f in [&mut c.title, &mut c.scope_name] {
            cut(f, 120);
        }
        cut(&mut c.before, 2000);
        cut(&mut c.after, 2000);
    }
    theirs.retain(|c| !c.id.is_empty() && ["added", "updated", "deleted"].contains(&c.action.as_str()) && !crate::ai::memory_agent::looks_secret(&c.before) && !crate::ai::memory_agent::looks_secret(&c.after));
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut l = load();
    if l.updated >= up {
        return Ok(false);
    }
    let known: std::collections::HashSet<String> = l.changes.iter().map(|c| c.id.clone()).collect();
    let fresh: Vec<Change> = theirs.iter().filter(|c| !known.contains(&c.id)).cloned().collect();
    for mine in &mut l.changes {
        if theirs.iter().any(|t| t.id == mine.id && t.undone) {
            mine.undone = true;
        }
    }
    l.changes.extend(fresh.iter().cloned());
    l.changes.sort_by(|a, b| a.time.cmp(&b.time).then(a.id.cmp(&b.id)));
    let extra = l.changes.len().saturating_sub(KEEP);
    l.changes.drain(..extra);
    l.updated = up;
    save(&l);
    drop(_g);
    // a change that was just made on the other side (the server's agent, or the computer's) shows the chip here too
    let now = chats::now_millis();
    let recent: Vec<Change> = fresh.into_iter().filter(|c| c.time + 120_000 > now).collect();
    if !recent.is_empty() && memory::load().show_updates {
        publish(&recent);
    }
    Ok(true)
}

pub fn to_json(c: &Change) -> Value {
    json!({
        "id": c.id, "time": c.time, "batch": c.batch, "source": c.source, "chat": c.chat, "title": c.title,
        "scope": c.scope, "scope_name": c.scope_name, "action": c.action, "note": c.note, "kind": c.kind,
        "before": c.before, "after": c.after, "undone": c.undone,
    })
}

/// "Added 2 notes, updated 1" for a batch.
pub fn summary(changes: &[Change]) -> String {
    let count = |a: &str| changes.iter().filter(|c| c.action == a).count();
    let part = |n: usize, verb: &str| match n {
        0 => None,
        1 => Some(format!("{verb} 1 note")),
        n => Some(format!("{verb} {n} notes")),
    };
    let parts: Vec<String> = [part(count("added"), "Added"), part(count("updated"), "updated"), part(count("deleted"), "removed")].into_iter().flatten().collect();
    parts.join(", ")
}

pub fn counts(changes: &[Change]) -> Value {
    let count = |a: &str| changes.iter().filter(|c| c.action == a).count();
    json!({"added": count("added"), "updated": count("updated"), "deleted": count("deleted")})
}

/// Undoes one entry: an added note is forgotten, an updated one gets its old text back, a deleted one is written again.
/// Returns a sentence about what was done.
pub fn undo(id: &str) -> Result<String, String> {
    let c = load().changes.into_iter().find(|c| c.id == id).ok_or("That change is not in the list any more.")?;
    if c.undone {
        return Err("That change was already undone.".into());
    }
    let say = match c.action.as_str() {
        "added" => {
            if !memory::forget(&c.note) {
                return Err("That note is already gone.".into());
            }
            "The note was removed again."
        }
        "updated" => {
            if !memory::update_text(&c.note, &c.before) {
                return Err("That note is gone, so its old text cannot be put back.".into());
            }
            "The note has its earlier text again."
        }
        "deleted" => {
            if c.before.trim().is_empty() {
                return Err("The old text is not kept, so it cannot be brought back.".into());
            }
            // a project that was deleted meanwhile does not get its notes back
            if !c.scope.is_empty() && super::projects::Store::load().get(&c.scope).is_none() {
                return Err("The project this note belonged to is gone.".into());
            }
            memory::restore(&c.before, &c.kind, &c.chat, &c.scope, c.pinned).ok_or("The note could not be written again.")?;
            "The note is back."
        }
        _ => return Err("Unknown change.".into()),
    };
    edit(|l| {
        if let Some(x) = l.changes.iter_mut().find(|x| x.id == id) {
            x.undone = true;
        }
    });
    Ok(say.into())
}

// ---------- who is listening ----------

type Listener = Arc<dyn Fn(&Value) + Send + Sync>;

static LISTENERS: Mutex<Vec<(u64, String, Listener)>> = Mutex::new(Vec::new());
static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Keeps a listener registered until it is dropped.
pub struct Subscription(u64);

impl Drop for Subscription {
    fn drop(&mut self) {
        LISTENERS.lock().unwrap_or_else(|e| e.into_inner()).retain(|(n, _, _)| *n != self.0);
    }
}

fn tenant_key() -> String {
    crate::tenant::current().map(|t| t.id).unwrap_or_default()
}

/// Registers `f` for the memory updates of the current person (on the desktop there is one; on a server, the tenant that is working on
/// this thread). `f` gets `{"event":"memory_updated","summary":..,"counts":{..},"ids":[..],"time":..}` and may be called from another thread.
pub fn subscribe(f: impl Fn(&Value) + Send + Sync + 'static) -> Subscription {
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    LISTENERS.lock().unwrap_or_else(|e| e.into_inner()).push((n, tenant_key(), Arc::new(f)));
    Subscription(n)
}

fn publish(changes: &[Change]) {
    let key = tenant_key();
    let event = json!({
        "event": "memory_updated",
        "summary": summary(changes),
        "counts": counts(changes),
        "ids": changes.iter().map(|c| c.id.clone()).collect::<Vec<_>>(),
        "time": changes.iter().map(|c| c.time).max().unwrap_or(0),
    });
    let ls: Vec<Listener> = LISTENERS.lock().unwrap_or_else(|e| e.into_inner()).iter().filter(|(_, k, _)| *k == key).map(|(_, _, f)| f.clone()).collect();
    for f in ls {
        f(&event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::TEST_ENV_LOCK;

    fn sandbox(tag: &str) -> (std::sync::MutexGuard<'static, ()>, PathBuf) {
        let g = TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("zo-mlog-{tag}-{}-{}", std::process::id(), chats::now_millis()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("XDG_DATA_HOME", &dir);
        std::env::set_var("XDG_CONFIG_HOME", dir.join("config"));
        (g, dir)
    }

    #[test]
    fn a_batch_is_logged_with_before_and_after_and_can_be_undone() {
        let (_g, dir) = sandbox("undo");
        let old = memory::remember("Uses a Samsung A22 as the test phone", "fact", "c0").unwrap().0;
        let gone = memory::remember("Lives in Karachi", "person", "c0").unwrap().0;
        let before = snapshot();
        memory::update_text(&old, "Uses a Samsung A26 as the test phone");
        memory::forget(&gone);
        let (new, _) = memory::remember("Builds Zohara OS", "project", "c1").unwrap();
        let ch = record_diff(&before, "engine", "c1", "Phone");
        assert_eq!(ch.len(), 3, "{ch:?}");
        let by = |a: &str| ch.iter().find(|c| c.action == a).unwrap().clone();
        assert_eq!((by("updated").before.as_str(), by("updated").after.as_str()), ("Uses a Samsung A22 as the test phone", "Uses a Samsung A26 as the test phone"));
        assert_eq!(by("deleted").before, "Lives in Karachi");
        assert_eq!(by("added").after, "Builds Zohara OS");
        assert_eq!(summary(&ch), "Added 1 note, updated 1 note, removed 1 note");
        assert_eq!(list(0, 50).len(), 3);
        assert_eq!(list(0, 50)[0].chat, "c1");

        // undo each of the three
        assert!(undo(&by("added").id).is_ok());
        assert!(!memory::load().items.iter().any(|i| i.id == new));
        assert!(undo(&by("updated").id).is_ok());
        assert!(memory::load().items.iter().any(|i| i.text.contains("A22")));
        assert!(undo(&by("deleted").id).is_ok());
        let back = memory::load().items.into_iter().find(|i| i.text == "Lives in Karachi").expect("the deleted note is back");
        assert_eq!(back.kind, "person");
        // not twice
        assert!(undo(&by("added").id).is_err());
        assert!(list(0, 50).iter().all(|c| c.undone));
        assert!(undo("g404").is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn nothing_changed_is_nothing_logged_and_the_log_is_capped() {
        let (_g, dir) = sandbox("cap");
        memory::clear_all();
        let before = snapshot();
        assert!(record_diff(&before, "engine", "c", "t").is_empty());
        assert!(list(0, 10).is_empty());
        for n in 0..(KEEP + 20) {
            let before = snapshot();
            // written straight into the file: similar texts would be merged by `remember`
            memory::change(|s| s.items.push(Item { id: format!("n{n:04}"), text: format!("Note number {n}"), kind: "fact".into(), ..Default::default() }));
            record_diff(&before, "engine", "c", "t");
            // a different time for each entry
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(list(0, 1000).len(), KEEP);
        assert!(list(0, 1000)[0].after.contains(&format!("{}", KEEP + 19)), "newest first");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_secret_never_reaches_the_log() {
        let (_g, dir) = sandbox("secret");
        memory::clear_all();
        let before = snapshot();
        // a note that got into the memory some other way (an old file, the sync) must still stay out of the feed
        memory::remember("The wifi password is hunter22", "fact", "c").unwrap();
        memory::remember("Likes green tea", "preference", "c").unwrap();
        let ch = record_diff(&before, "engine", "c", "t");
        assert_eq!(ch.len(), 1, "{ch:?}");
        assert_eq!(ch[0].after, "Likes green tea");
        assert!(!std::fs::read_to_string(path()).unwrap().contains("hunter22"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn listeners_hear_a_finished_batch_unless_updates_are_off() {
        let (_g, dir) = sandbox("event");
        memory::clear_all();
        let seen: Arc<Mutex<Vec<Value>>> = Arc::default();
        let s2 = seen.clone();
        let sub = subscribe(move |v| s2.lock().unwrap().push(v.clone()));
        let before = snapshot();
        memory::remember("Plays the guitar", "fact", "c").unwrap();
        record_diff(&before, "engine", "c", "t");
        {
            let got = seen.lock().unwrap();
            assert_eq!(got.len(), 1);
            assert_eq!(got[0]["event"], "memory_updated");
            assert_eq!(got[0]["summary"], "Added 1 note");
            assert_eq!(got[0]["counts"]["added"], 1);
        }
        memory::set_show_updates(false);
        let before = snapshot();
        memory::remember("Allergic to peanuts", "fact", "c").unwrap();
        record_diff(&before, "engine", "c", "t");
        assert_eq!(seen.lock().unwrap().len(), 1, "no event when updates are off");
        assert_eq!(list(0, 10).len(), 2, "but it is still in the log");
        drop(sub);
        memory::set_show_updates(true);
        let before = snapshot();
        memory::remember("Drives a blue Suzuki Alto", "fact", "c").unwrap();
        record_diff(&before, "engine", "c", "t");
        assert_eq!(seen.lock().unwrap().len(), 1, "a dropped subscription hears nothing");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_log_travels_by_sync_and_the_entries_of_both_sides_are_kept() {
        let (_g, dir) = sandbox("sync");
        memory::clear_all();
        let before = snapshot();
        memory::remember("Plays the guitar", "fact", "c").unwrap();
        record_diff(&before, "engine", "c1", "Music");
        let mine = list(0, 10);
        assert_eq!(mine.len(), 1);
        // it is an item of the sync manifest, and a document
        let manifest = crate::sync::local_side();
        assert_eq!(manifest.items["memory:_changes"], updated());
        let doc = crate::sync::get_doc("memory", "_changes").unwrap();
        assert_eq!(doc["changes"].as_array().unwrap().len(), 1);
        assert!(crate::sync::apply_delete("memory", "_changes", 5).is_err(), "the log is never deleted by the other side");

        // the other side has an entry of its own, knows ours as undone, and also sent one with a secret and one that is not valid
        let now = chats::now_millis();
        let theirs = |id: &str, after: &str, action: &str| json!({"id": id, "time": now, "action": action, "after": after, "kind": "fact"});
        let mut undone = serde_json::to_value(&mine[0]).unwrap();
        undone["undone"] = json!(true);
        let remote = json!({"updated": updated() + 5000, "changes": [undone, theirs("gX1", "Likes tea", "added"), theirs("gX2", "The wifi password is hunter22", "added"), theirs("gX3", "x", "explode")]});
        let heard: Arc<Mutex<Vec<Value>>> = Arc::default();
        let h2 = heard.clone();
        let _sub = subscribe(move |v| h2.lock().unwrap().push(v.clone()));
        assert!(crate::sync::put_doc("memory", "_changes", &remote).unwrap());
        let all = list(0, 10);
        assert_eq!(all.len(), 2, "{all:?}");
        assert!(all.iter().find(|c| c.id == mine[0].id).unwrap().undone, "undone on the other side counts here too");
        assert!(all.iter().any(|c| c.after == "Likes tea"));
        assert!(!std::fs::read_to_string(path()).unwrap().contains("hunter22"));
        assert_eq!(heard.lock().unwrap().len(), 1, "a change just made on the other side shows the chip here");
        // an older log changes nothing
        assert!(!crate::sync::put_doc("memory", "_changes", &json!({"updated": 5, "changes": []})).unwrap());
        assert_eq!(list(0, 10).len(), 2);
        assert!(crate::sync::put_doc("memory", "_changes", &json!({"changes": []})).is_err(), "no time");
        let _ = std::fs::remove_dir_all(dir);
    }
}
