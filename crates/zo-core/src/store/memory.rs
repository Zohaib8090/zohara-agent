//! Zohara Agent's long-term memory: what it remembers about the person across every chat, on the computer and on the phone.
//!
//! One file (`memory.json`) is shared by the app window, the phone service and the MCP tools, so what one learns the others
//! know. It holds short notes ("facts") the memory agent wrote after some chats, and one summary ("episode") per chat.
//!
//! Search is a vector search. Every note is turned into a vector by the local embedder (`embed`) and the closest ones to the
//! message are the ones the model is told about. The local embedder hashes words and word pieces into 512 numbers: it needs no
//! key and no network, it works offline and in any language, and it finds notes that share words or word pieces with the
//! message (it does not know that "car" and "vehicle" mean the same; a model-based embedder can be added later: every vector
//! remembers which embedder made it, so notes made with another one are simply made again).

use super::chats;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

pub const DIM: usize = 512;
/// The name of the local embedder; stored with a vector so a different embedder can replace it later.
pub const EMBEDDER: &str = "hash-512-v1";
const MAX_ITEMS: usize = 2000;
pub const MAX_TEXT: usize = 400;
/// A new note this close to an old one of the same kind updates the old one instead of piling up.
const SAME_AS: f32 = 0.60;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Item {
    pub id: String,
    pub text: String,
    /// fact, preference, person, project or episode
    pub kind: String,
    /// The chat it came from.
    #[serde(default)]
    pub chat: String,
    /// For an episode: the chat's title.
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub created: u64,
    #[serde(default)]
    pub updated: u64,
    #[serde(default)]
    pub pinned: bool,
    /// Where the note belongs: "" is the whole memory (every chat), otherwise the id of the project whose chats may read it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub scope: String,
    /// How often it was handed to the model.
    #[serde(default)]
    pub uses: u32,
    /// The vector, written only by an embedder other than the local one (the local one is cheap: it is made when needed).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vec: Vec<f32>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub embedder: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Store {
    /// Off: nothing is remembered and nothing is told to the model.
    #[serde(default = "yes")]
    pub enabled: bool,
    /// The memory agent runs after this many new messages of yours in a chat.
    #[serde(default = "three")]
    pub every: usize,
    /// By chat id: how many of your messages in it the memory agent has already read.
    #[serde(default)]
    pub read: HashMap<String, usize>,
    #[serde(default)]
    pub items: Vec<Item>,
    /// When `enabled` was last changed (the sync keeps the newest choice).
    #[serde(default)]
    pub state_updated: u64,
    /// Show the "Memory updated" chip above the message box when the memory agent changed something. This computer's own choice (not synced).
    #[serde(default = "yes")]
    pub show_updates: bool,
}

/// The memory agent never runs more often than every 2 messages of the person.
pub const MIN_EVERY: usize = 2;
pub const MAX_EVERY: usize = 20;

fn yes() -> bool {
    true
}
fn three() -> usize {
    3
}

impl Default for Store {
    fn default() -> Self {
        Store { enabled: true, every: 3, read: HashMap::new(), items: Vec::new(), state_updated: 0, show_updates: true }
    }
}

static LOCK: Mutex<()> = Mutex::new(());

fn path() -> PathBuf {
    crate::audit::data_dir().join("memory.json")
}

pub fn load() -> Store {
    std::fs::read_to_string(path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn save(s: &Store) {
    let p = path();
    let _ = std::fs::create_dir_all(p.parent().unwrap_or(&p));
    let tmp = p.with_extension("json.tmp");
    if let Ok(text) = serde_json::to_string(s) {
        if !crate::tenant::fits(&p, text.len()) {
            return;
        }
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, &p);
        }
    }
}

/// Reads, changes and writes the store as one step (no two changes in this program overwrite each other).
pub fn change<R>(f: impl FnOnce(&mut Store) -> R) -> R {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut s = load();
    let r = f(&mut s);
    save(&s);
    r
}

// ---------- the embedder ----------

fn fnv(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn add(v: &mut [f32], feature: &str, weight: f32) {
    let h = fnv(feature.as_bytes());
    let slot = (h % DIM as u64) as usize;
    let sign = if (h >> 40) & 1 == 0 { 1.0 } else { -1.0 };
    v[slot] += sign * weight;
}

/// Words the search should not care about (they are in every message).
fn is_filler(w: &str) -> bool {
    matches!(
        w,
        "the" | "a" | "an" | "and" | "or" | "of" | "to" | "in" | "on" | "at" | "is" | "are" | "was" | "were" | "be" | "it" | "this" | "that" | "i" | "you" | "my" | "me" | "we" | "for" | "with" | "do" | "does" | "did" | "have" | "has"
            | "what" | "which" | "who" | "how" | "can" | "will" | "so" | "if" | "but" | "not" | "no" | "yes" | "your" | "about" | "from" | "as" | "by" | "ka" | "ki" | "ke" | "hai" | "hain" | "ko" | "se" | "aur"
    )
}

fn words(text: &str) -> Vec<String> {
    text.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_string).collect()
}

/// The local embedder: a unit vector of DIM numbers built from the words, their pairs and the word pieces of the text.
pub fn embed(text: &str) -> Vec<f32> {
    let mut v = vec![0f32; DIM];
    let ws: Vec<String> = words(text).into_iter().filter(|w| !is_filler(w)).collect();
    for (i, w) in ws.iter().enumerate() {
        add(&mut v, &format!("w:{w}"), 1.0);
        // word pieces, so "playing" and "played" meet
        let chars: Vec<char> = w.chars().collect();
        if chars.len() >= 4 {
            for k in 0..=(chars.len() - 3) {
                let piece: String = chars[k..k + 3].iter().collect();
                add(&mut v, &format!("t:{piece}"), 0.35);
            }
        }
        if let Some(next) = ws.get(i + 1) {
            add(&mut v, &format!("b:{w} {next}"), 0.6);
        }
    }
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
    v
}

pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

impl Item {
    /// The vector to compare with: the stored one when it came from the embedder being used, else the local one.
    fn vector(&self) -> Vec<f32> {
        // a vector made by another embedder cannot be compared with the local one: the local one is made again
        embed(&self.text_for_search())
    }

    fn text_for_search(&self) -> String {
        if self.title.is_empty() {
            self.text.clone()
        } else {
            format!("{} {}", self.title, self.text)
        }
    }
}

// ---------- reading ----------

pub struct Hit {
    pub item: Item,
    pub score: f32,
}

thread_local! {
    /// The project the chat being answered on this thread belongs to ("" = none). The tools that read and write notes use it.
    static PROJECT: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

/// While it lives, the memory tools on this thread work for chats of that project (see `current_project`).
pub struct ProjectGuard(String);

impl Drop for ProjectGuard {
    fn drop(&mut self) {
        let old = std::mem::take(&mut self.0);
        PROJECT.with(|p| *p.borrow_mut() = old);
    }
}

/// A project that is gone (deleted, or never here) counts as no project: a note must never be written for a project nobody can see.
pub fn project_guard(project: &str) -> ProjectGuard {
    let project = if project.is_empty() || super::projects::Store::load().get(project).is_some() { project } else { "" };
    ProjectGuard(PROJECT.with(|p| p.replace(project.to_string())))
}

pub fn current_project() -> String {
    PROJECT.with(|p| p.borrow().clone())
}

/// Can a chat of `project` ("" = a chat outside every project) see this note? Global notes are seen by everybody; a project's notes only
/// by the chats of that project.
pub fn visible(it: &Item, project: &str) -> bool {
    it.scope.is_empty() || (!project.is_empty() && it.scope == project)
}

/// The notes closest to `query`, best first. Notes that are pinned or recent get a small push; very weak matches are left out.
pub fn search(store: &Store, query: &str, limit: usize) -> Vec<Hit> {
    search_in(store, query, limit, "")
}

/// Like `search`, for a chat of `project`: its own notes and the global ones, never another project's.
pub fn search_in(store: &Store, query: &str, limit: usize, project: &str) -> Vec<Hit> {
    let q = embed(query);
    if q.iter().all(|x| *x == 0.0) {
        return Vec::new();
    }
    let now = chats::now_millis();
    let mut hits: Vec<Hit> = store
        .items
        .iter()
        .filter(|it| visible(it, project))
        .filter_map(|it| {
            let c = cosine(&q, &it.vector());
            if c < 0.12 {
                return None;
            }
            let days = now.saturating_sub(it.updated) as f32 / 86_400_000.0;
            let recent = 0.05 / (1.0 + days / 30.0);
            let pin = if it.pinned { 0.05 } else { 0.0 };
            Some(Hit { item: it.clone(), score: c + recent + pin })
        })
        .collect();
    hits.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    hits.truncate(limit);
    hits
}

fn date(ms: u64) -> String {
    // days since 1970-01-01 to a calendar date (Howard Hinnant's algorithm)
    let z = (ms / 86_400_000) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{:04}-{:02}-{:02}", if m <= 2 { y + 1 } else { y }, m, d)
}

fn line(it: &Item) -> String {
    if it.kind == "episode" {
        format!("- Earlier chat \"{}\" ({}): {}", it.title, date(it.updated), it.text)
    } else {
        format!("- {}", it.text)
    }
}

/// What the model is told about the person at the start of a message: the notes that always matter (pinned, preferences,
/// who they are) and the ones closest to this message, with the summaries of earlier chats that look related.
pub fn briefing(query: &str) -> String {
    briefing_in(query, "")
}

/// `briefing` for a chat of `project` ("" = none): the project's notes come with the global ones, no other project's.
pub fn briefing_in(query: &str, project: &str) -> String {
    let mut store = load();
    store.items.retain(|i| visible(i, project));
    if !store.enabled || store.items.is_empty() {
        return String::new();
    }
    let mut chosen: Vec<Item> = Vec::new();
    let mut used = 0usize;
    let mut pick = |it: &Item, chosen: &mut Vec<Item>, budget: usize| {
        if chosen.iter().any(|c| c.id == it.id) {
            return;
        }
        let n = line(it).chars().count();
        if used + n <= budget {
            used += n;
            chosen.push(it.clone());
        }
    };
    // always: pinned notes, then who the person is and what they like, newest first
    let mut always: Vec<&Item> = store.items.iter().filter(|i| i.kind != "episode" && (i.pinned || i.kind == "preference" || i.kind == "person")).collect();
    always.sort_by(|a, b| (b.pinned, b.updated).cmp(&(a.pinned, a.updated)));
    for it in always.into_iter().take(14) {
        pick(it, &mut chosen, 1400);
    }
    let always_count = chosen.len();
    for h in search_in(&store, query, 8, project) {
        pick(&h.item, &mut chosen, 3000);
    }
    if chosen.is_empty() {
        return String::new();
    }
    let ids: Vec<String> = chosen.iter().map(|c| c.id.clone()).collect();
    change(|s| {
        for it in s.items.iter_mut().filter(|i| ids.contains(&i.id)) {
            it.uses += 1;
        }
    });
    let (core, related) = chosen.split_at(always_count);
    let mut out = String::from(
        "WHAT YOU REMEMBER ABOUT THE PERSON\n\
        These notes were written from earlier chats with this person, on the computer and on the phone (it is one memory). Use them naturally, the way a person who knows them would: do not list them, \
        and do not say \"according to my memory\" unless they ask what you remember. Only what is written here is remembered: never pretend to remember anything else. \
        If the person says something that contradicts a note, trust them now and fix the note with zohara_memory_save (or zohara_memory_forget). \
        To look further back, use zohara_memory_search and zohara_ai_search_chats.\n",
    );
    if !core.is_empty() {
        out.push_str("\nAbout them:\n");
        out.push_str(&core.iter().map(line).collect::<Vec<_>>().join("\n"));
        out.push('\n');
    }
    if !related.is_empty() {
        out.push_str("\nRelated to this message:\n");
        out.push_str(&related.iter().map(line).collect::<Vec<_>>().join("\n"));
        out.push('\n');
    }
    out
}

/// `base`, then the briefing for `query` (when there are notes), then the personality in use (when there is one). The personality is LAST on
/// purpose: it must come after the abilities, the tool rules and the notes, so that it decides how everything is worded.
pub fn system_with(base: &str, query: &str) -> String {
    system_in(base, query, "")
}

/// `system_with` for a chat of `project`.
pub fn system_in(base: &str, query: &str, project: &str) -> String {
    join(&[base, &briefing_in(query, project), &super::personality::instructions()])
}

/// `base` followed by the personality only, with NO notes: for a restricted account, whose notes must reach the model only through
/// `zohara_memory_search` (which closes the web for the rest of the answer; notes placed in the prompt would not).
pub fn system_without_notes(base: &str) -> String {
    join(&[base, &super::personality::instructions()])
}

/// The briefing and the personality as one piece (personality last), for programs that take extra instructions (Claude Code).
pub fn extras(query: &str) -> String {
    extras_in(query, "")
}

pub fn extras_in(query: &str, project: &str) -> String {
    join(&[&briefing_in(query, project), &super::personality::instructions()])
}

fn join(parts: &[&str]) -> String {
    parts.iter().filter(|p| !p.trim().is_empty()).copied().collect::<Vec<_>>().join("\n\n")
}

// ---------- writing ----------

fn clean(text: &str) -> String {
    let t: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.chars().count() > MAX_TEXT {
        format!("{}…", t.chars().take(MAX_TEXT).collect::<String>())
    } else {
        t
    }
}

fn valid_kind(k: &str) -> &'static str {
    match k {
        "preference" => "preference",
        "person" => "person",
        "project" => "project",
        "episode" => "episode",
        _ => "fact",
    }
}

/// Where a note of this kind goes when nobody said: what is about the person (who they are, how they like things) stays global, the rest of
/// what is learned in a project's chat belongs to the project.
pub fn default_scope(kind: &str, project: &str) -> String {
    match valid_kind(kind) {
        "person" | "preference" | "episode" => String::new(),
        _ => project.to_string(),
    }
}

/// Adds a note to the whole memory, or updates the note it repeats. Returns (id, true if it was new).
pub fn remember(text: &str, kind: &str, chat: &str) -> Option<(String, bool)> {
    remember_in(text, kind, chat, "")
}

/// Adds a note to `scope` ("" = the whole memory, else a project id), or updates the note of the same scope it repeats.
pub fn remember_in(text: &str, kind: &str, chat: &str, scope: &str) -> Option<(String, bool)> {
    let text = clean(text);
    if text.chars().count() < 3 {
        return None;
    }
    let kind = valid_kind(kind);
    change(|s| {
        let v = embed(&text);
        let now = chats::now_millis();
        let same = s.items.iter().enumerate().filter(|(_, i)| i.kind == kind && i.kind != "episode" && i.scope == scope).map(|(n, i)| (n, cosine(&v, &i.vector()))).filter(|(_, c)| *c >= SAME_AS).max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        if let Some((n, _)) = same {
            let it = &mut s.items[n];
            it.text = text;
            it.updated = now;
            return Some((it.id.clone(), false));
        }
        let id = format!("m{}{:03}", now, s.items.len() % 1000);
        s.items.push(Item { id: id.clone(), text, kind: kind.into(), chat: chat.into(), created: now, updated: now, scope: scope.into(), ..Default::default() });
        prune(s);
        Some((id, true))
    })
}

/// Writes a note that was removed again (undo), with a new id. Returns the id.
pub fn restore(text: &str, kind: &str, chat: &str, scope: &str, pinned: bool) -> Option<String> {
    let text = clean(text);
    if text.chars().count() < 3 {
        return None;
    }
    let kind = valid_kind(kind);
    Some(change(|s| {
        let now = chats::now_millis();
        let id = format!("m{}{:03}", now, s.items.len() % 1000);
        s.items.push(Item { id: id.clone(), text, kind: kind.into(), chat: chat.into(), created: now, updated: now, pinned, scope: scope.into(), ..Default::default() });
        prune(s);
        id
    }))
}

/// Adds a note the person brought in (an import), unless the same text is already there in that scope. Some(true) = added, Some(false) = it was there.
pub fn add_unique(text: &str, kind: &str, scope: &str, pinned: bool) -> Option<bool> {
    let text = clean(text);
    if text.chars().count() < 3 {
        return None;
    }
    let kind = valid_kind(kind);
    if kind == "episode" {
        return None;
    }
    let dup = load().items.iter().any(|i| i.scope == scope && i.kind != "episode" && i.text.trim().eq_ignore_ascii_case(text.trim()));
    if dup {
        return Some(false);
    }
    restore(&text, kind, "", scope, pinned).map(|_| true)
}

/// Rewrites the note about one chat (one episode per chat).
pub fn set_episode(chat: &str, title: &str, summary: &str) {
    let summary = clean(summary);
    if summary.is_empty() {
        return;
    }
    change(|s| {
        let now = chats::now_millis();
        if let Some(it) = s.items.iter_mut().find(|i| i.kind == "episode" && i.chat == chat) {
            it.text = summary;
            it.title = title.chars().take(60).collect();
            it.updated = now;
        } else {
            let id = format!("e{}{:03}", now, s.items.len() % 1000);
            s.items.push(Item { id, text: summary, kind: "episode".into(), chat: chat.into(), title: title.chars().take(60).collect(), created: now, updated: now, ..Default::default() });
            prune(s);
        }
    });
}

fn prune(s: &mut Store) {
    while s.items.len() > MAX_ITEMS {
        // the oldest unpinned summary goes first, then the oldest unpinned note
        let pick = s.items.iter().enumerate().filter(|(_, i)| !i.pinned).min_by_key(|(_, i)| (i.kind != "episode", i.updated)).map(|(n, _)| n);
        match pick {
            Some(n) => {
                s.items.remove(n);
            }
            None => break,
        }
    }
}

/// Rewrites one note (the memory agent found it changed). Returns false when there is no such note.
pub fn update_text(id: &str, text: &str) -> bool {
    update_text_checked(id, text, None)
}

/// Like `update_text`, for the memory agent working on a chat of `project`: a note that chat cannot see is not touched.
pub fn update_text_in(id: &str, text: &str, project: &str) -> bool {
    update_text_checked(id, text, Some(project))
}

fn update_text_checked(id: &str, text: &str, project: Option<&str>) -> bool {
    let text = clean(text);
    if text.chars().count() < 3 {
        return false;
    }
    change(|s| match s.items.iter_mut().find(|i| i.id == id && i.kind != "episode" && project.is_none_or(|p| visible(i, p))) {
        Some(i) => {
            i.text = text;
            i.updated = chats::now_millis();
            true
        }
        None => false,
    })
}

pub fn forget(id: &str) -> bool {
    forget_checked(id, None)
}

/// Like `forget`, for the memory agent working on a chat of `project`: a note that chat cannot see stays.
pub fn forget_in(id: &str, project: &str) -> bool {
    forget_checked(id, Some(project))
}

fn forget_checked(id: &str, project: Option<&str>) -> bool {
    let (gone, updated) = change(|s| {
        let updated = s.items.iter().find(|i| i.id == id).map_or(0, |i| i.updated);
        let before = s.items.len();
        s.items.retain(|i| i.id != id || project.is_some_and(|p| !visible(i, p)));
        (s.items.len() != before, updated)
    });
    if gone {
        crate::sync::tombstone("memory", id, updated);
    }
    gone
}

/// Forgets the notes that match `words` best (used when the model is asked to forget something), returning what went.
pub fn forget_matching(words: &str) -> Vec<String> {
    forget_matching_in(words, "")
}

/// `forget_matching` for a chat of `project`: only notes it can see.
pub fn forget_matching_in(words: &str, project: &str) -> Vec<String> {
    let store = load();
    let hits = search_in(&store, words, 3, project);
    let gone: Vec<Item> = hits.into_iter().filter(|h| h.score > 0.45).map(|h| h.item).collect();
    for g in &gone {
        forget_in(&g.id, project);
    }
    gone.into_iter().map(|g| g.text).collect()
}

pub fn set_pinned(id: &str, on: bool) {
    change(|s| {
        if let Some(i) = s.items.iter_mut().find(|i| i.id == id) {
            i.pinned = on;
        }
    });
}

pub fn clear_all() {
    super::memory_log::clear();
    let items: Vec<(String, u64)> = change(|s| {
        s.read.clear();
        s.items.drain(..).map(|i| (i.id, i.updated)).collect()
    });
    for (id, updated) in items {
        crate::sync::tombstone("memory", &id, updated);
    }
}

/// How many messages of the person the memory agent waits for (at least `MIN_EVERY`, at most `MAX_EVERY`). Returns what was kept.
pub fn set_every(n: usize) -> usize {
    let n = n.clamp(MIN_EVERY, MAX_EVERY);
    change(|s| {
        s.every = n;
        // the sync takes the newest choice
        s.state_updated = chats::now_millis();
    });
    n
}

pub fn set_show_updates(on: bool) {
    change(|s| s.show_updates = on);
}

/// Removes every note that belongs to the project (it was deleted). Returns how many went.
pub fn delete_scope(project: &str) -> usize {
    if project.is_empty() {
        return 0;
    }
    let gone: Vec<(String, u64)> = change(|s| {
        let gone = s.items.iter().filter(|i| i.scope == project).map(|i| (i.id.clone(), i.updated)).collect::<Vec<_>>();
        s.items.retain(|i| i.scope != project);
        gone
    });
    for (id, updated) in &gone {
        crate::sync::tombstone("memory", id, *updated);
    }
    // the change log must not keep the text of notes that were deleted with their project
    super::memory_log::forget_scope(project);
    gone.len()
}

pub fn set_enabled(on: bool) {
    change(|s| {
        s.enabled = on;
        s.state_updated = chats::now_millis();
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::TEST_ENV_LOCK;

    fn sandbox() -> (std::sync::MutexGuard<'static, ()>, PathBuf) {
        let g = TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("zo-mem-{}-{}", std::process::id(), chats::now_millis()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("XDG_DATA_HOME", &dir);
        (g, dir)
    }

    #[test]
    fn the_embedder_finds_shared_words_and_pieces() {
        let q = embed("which music app do I use");
        let near = embed("The person listens to music on Spotify");
        let far = embed("The laptop has 16 GB of memory");
        assert!(cosine(&q, &near) > cosine(&q, &far));
        let a = embed("he is playing guitar");
        let b = embed("played the guitar yesterday");
        assert!(cosine(&a, &b) > 0.2, "word pieces should meet: {}", cosine(&a, &b));
        assert!((cosine(&a, &a) - 1.0).abs() < 1e-4);
    }

    #[test]
    fn works_in_other_scripts() {
        let a = embed("मुझे चाय पसंद है");
        let b = embed("चाय पसंद");
        let c = embed("कार की सर्विस");
        assert!(cosine(&a, &b) > cosine(&a, &c));
    }

    #[test]
    fn remember_search_update_forget() {
        let (_g, dir) = sandbox();
        let (id, new) = remember("The person's name is Zohaib and he builds Zohara OS", "person", "c1").unwrap();
        assert!(new);
        remember("Likes short answers without long explanations", "preference", "c1").unwrap();
        remember("The laptop runs Arch Linux with KDE Plasma", "fact", "c2").unwrap();
        // the same thing said again updates, it does not pile up
        let (id2, new2) = remember("The person's name is Zohaib and he builds Zohara OS and Zohara Link", "person", "c3").unwrap();
        assert!(!new2);
        assert_eq!(id, id2);
        assert_eq!(load().items.len(), 3);

        let hits = search(&load(), "what operating system is the laptop running", 3);
        assert!(hits[0].item.text.contains("Arch"), "{:?}", hits.iter().map(|h| &h.item.text).collect::<Vec<_>>());

        let b = briefing("how do I update the laptop");
        assert!(b.contains("WHAT YOU REMEMBER"));
        assert!(b.contains("short answers"), "preferences are always told: {b}");
        assert!(b.contains("Arch Linux"), "{b}");

        assert!(forget(&id));
        assert_eq!(load().items.len(), 2);
        set_enabled(false);
        assert!(briefing("anything").is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn one_episode_per_chat_and_dates() {
        let (_g, dir) = sandbox();
        set_episode("c9", "Spotify on the phone", "Set up the Spotify MCP server and played a song from the phone.");
        set_episode("c9", "Spotify on the phone", "Set up the Spotify MCP server; the A22 played the song.");
        let s = load();
        assert_eq!(s.items.iter().filter(|i| i.kind == "episode").count(), 1);
        let b = briefing("what did we do with spotify");
        assert!(b.contains("Earlier chat \"Spotify on the phone\""), "{b}");
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(1_762_819_200_000), "2025-11-11");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn forget_matching_only_removes_close_notes() {
        let (_g, dir) = sandbox();
        remember("Allergic to peanuts", "fact", "c1").unwrap();
        remember("Drives a blue Suzuki Alto", "fact", "c1").unwrap();
        let gone = forget_matching("allergic to peanuts");
        assert_eq!(gone.len(), 1);
        assert_eq!(load().items.len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn project_notes_are_seen_only_by_chats_of_that_project() {
        let (_g, dir) = sandbox();
        remember("Likes short answers without long explanations", "preference", "c0").unwrap();
        remember_in("The atlas project uses metric units and a Postgres database", "project", "c1", "pa").unwrap();
        remember_in("The harbor project is written in Rust with a GTK window", "project", "c2", "pb").unwrap();
        // inside project A: its own notes and the global ones, never B's
        let a = briefing_in("which database does the atlas project use, and the harbor project", "pa");
        assert!(a.contains("Postgres") && a.contains("short answers"), "{a}");
        assert!(!a.contains("harbor") && !a.contains("GTK"), "{a}");
        let b = briefing_in("which database does the atlas project use, and the harbor project", "pb");
        assert!(b.contains("GTK") && !b.contains("Postgres"), "{b}");
        // outside every project: only the global ones
        let none = briefing("atlas harbor database GTK Postgres");
        assert!(!none.contains("Postgres") && !none.contains("GTK"), "{none}");
        assert!(search(&load(), "Postgres atlas database", 8).is_empty());
        assert_eq!(search_in(&load(), "Postgres atlas database", 8, "pa").len(), 1);
        assert!(search_in(&load(), "Postgres atlas database", 8, "pb").is_empty());
        // the same sentence in another scope is another note, not an update
        let (_, new) = remember_in("The atlas project uses metric units and a Postgres database", "project", "c3", "pb").unwrap();
        assert!(new);
        assert_eq!(load().items.len(), 4);
        // forgetting and rewriting from a chat of another project does nothing
        let id = load().items.iter().find(|i| i.scope == "pa").unwrap().id.clone();
        assert!(!forget_in(&id, "pb") && !forget_in(&id, ""));
        assert!(!update_text_in(&id, "Changed by the wrong project", "pb"));
        assert!(forget_matching_in("atlas Postgres database", "").is_empty());
        assert!(update_text_in(&id, "The atlas project uses imperial units", "pa"));
        assert!(forget_in(&id, "pa"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_guard_ignores_a_project_that_is_gone() {
        let (_g, dir) = sandbox();
        let mut p = super::super::projects::Store::default();
        p.projects.push(super::super::projects::Project::new("Atlas", ""));
        p.projects[0].id = "pa".into();
        p.save();
        {
            let _a = project_guard("pa");
            assert_eq!(current_project(), "pa");
            let _b = project_guard("p404");
            assert_eq!(current_project(), "", "a deleted project is no project");
        }
        assert_eq!(current_project(), "", "the guard puts back what was there");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn deleting_a_project_removes_its_notes_and_only_those() {
        let (_g, dir) = sandbox();
        remember("Likes green tea", "preference", "c0").unwrap();
        remember_in("The atlas project ships on Friday", "project", "c1", "pa").unwrap();
        remember_in("The harbor project ships in March", "project", "c2", "pb").unwrap();
        let mut p = super::super::projects::Store::default();
        p.projects.push(super::super::projects::Project::new("Atlas", ""));
        p.projects[0].id = "pa".into();
        assert_eq!(p.remove("pa"), Some(1));
        assert_eq!(p.remove("pa"), None);
        let left: Vec<String> = load().items.into_iter().map(|i| i.text).collect();
        assert_eq!(left.len(), 2);
        assert!(!left.iter().any(|t| t.contains("atlas")));
        assert_eq!(delete_scope(""), 0, "no scope means nothing is deleted");
        assert_eq!(load().items.len(), 2);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_agent_waits_at_least_two_messages() {
        let (_g, dir) = sandbox();
        assert_eq!(set_every(1), 2);
        assert_eq!(load().every, 2);
        assert_eq!(set_every(0), 2);
        assert_eq!(set_every(5), 5);
        assert_eq!(set_every(500), MAX_EVERY);
        assert_eq!(Store::default().every, 3);
        let _ = std::fs::remove_dir_all(dir);
    }
}
