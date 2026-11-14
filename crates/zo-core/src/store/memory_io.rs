//! Memory export and import (a JSON file of the notes, with the project each belongs to), used by every client. See docs/MEMORY.md.

use super::{chats, memory, projects};
use serde_json::{json, Value};
use std::collections::HashMap;

/// The projects of this person as (id, name); a restricted account has none.
pub fn projects() -> Vec<(String, String)> {
    if crate::tenant::is_restricted() {
        return Vec::new();
    }
    projects::Store::load().projects.into_iter().map(|p| (p.id, p.name)).collect()
}

/// The notes as a `zo-memory-1` document. `scope`: "" or "all" = every note, "global" = the whole memory only, or a project id. The chat
/// summaries are not part of it (they belong to chats, which are exported with the account).
pub fn export(scope: &str) -> Value {
    let notes: Vec<Value> = memory::load()
        .items
        .into_iter()
        .filter(|i| i.kind != "episode")
        .filter(|i| match scope {
            "" | "all" => true,
            "global" => i.scope.is_empty(),
            p => i.scope == p,
        })
        .map(|i| json!({"text": i.text, "kind": i.kind, "scope": i.scope, "pinned": i.pinned, "created": i.created, "updated": i.updated}))
        .collect();
    json!({
        "format": "zo-memory-1",
        "exported_at": chats::now_millis(),
        "projects": projects().iter().map(|(id, n)| json!({"id": id, "name": n})).collect::<Vec<_>>(),
        "notes": notes,
    })
}

/// Brings notes from an export in. A note of a project goes to the project with that id here, or with the same name; a note whose
/// project is not here is skipped (it is never moved into the whole memory). Nothing that looks like a secret comes in. Returns
/// (added, already there, skipped).
pub fn import(data: &Value) -> Result<(usize, usize, usize), String> {
    let notes = data.get("notes").and_then(Value::as_array).ok_or("That is not a memory export.")?;
    if notes.len() > 2000 {
        return Err("That file has too many notes.".into());
    }
    let here = projects();
    let theirs: HashMap<String, String> = data
        .get("projects")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|p| Some((p.get("id")?.as_str()?.to_string(), p.get("name")?.as_str()?.to_lowercase())))
        .collect();
    let (mut added, mut same, mut skipped) = (0, 0, 0);
    for n in notes {
        let text = n.get("text").and_then(Value::as_str).unwrap_or("");
        let scope = n.get("scope").and_then(Value::as_str).unwrap_or("");
        let target = if scope.is_empty() {
            Some(String::new())
        } else if here.iter().any(|(id, _)| id == scope) {
            Some(scope.to_string())
        } else {
            theirs.get(scope).and_then(|name| here.iter().find(|(_, h)| h.to_lowercase() == *name)).map(|(id, _)| id.clone())
        };
        let Some(target) = target else {
            skipped += 1;
            continue;
        };
        if crate::ai::memory_agent::looks_secret(text) {
            skipped += 1;
            continue;
        }
        match memory::add_unique(text, n.get("kind").and_then(Value::as_str).unwrap_or("fact"), &target, n.get("pinned").and_then(Value::as_bool).unwrap_or(false)) {
            Some(true) => added += 1,
            Some(false) => same += 1,
            None => skipped += 1,
        }
    }
    Ok((added, same, skipped))
}
