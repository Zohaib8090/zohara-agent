//! The list of Claude models, read from where it really lives instead of being typed in here.
//!
//! * With an Anthropic API key, the Models API (`GET /v1/models`) gives every model with its name, release date and, for each, which
//!   `effort` levels it supports (`capabilities.effort`). The list is kept in a cache file and refreshed once a day.
//! * Without one, Claude Code's own aliases (`sonnet`, `opus`, `haiku`: always the newest of each) are offered, plus any extra models
//!   Claude Code has written into its own cache.
//! * What Claude Desktop shows comes from Claude's account service, which a program cannot ask for you.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    /// When it was released (an ISO date as the API gives it); newest first is how lists are ordered.
    #[serde(default)]
    pub created: String,
    /// The effort levels it supports, when known (`None`: the API did not say).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub efforts: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// The size of its window in tokens, when the API said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_input_tokens: Option<u64>,
}

/// The effort levels a model can have, lowest first (the names the API and Claude Code use).
pub const EFFORT_ORDER: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

fn cache_file() -> std::path::PathBuf {
    crate::audit::data_dir().join("claude-models.json")
}

#[derive(Serialize, Deserialize, Default)]
struct Cache {
    fetched: u64,
    models: Vec<ModelInfo>,
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn load_cache() -> Cache {
    std::fs::read_to_string(cache_file()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn save_cache(models: &[ModelInfo]) {
    let _ = std::fs::create_dir_all(crate::audit::data_dir());
    if let Ok(s) = serde_json::to_string(&Cache { fetched: now(), models: models.to_vec() }) {
        let _ = std::fs::write(cache_file(), s);
    }
}

/// "claude-opus-4-8" -> "Opus 4.8"; "claude-fable-5-1[1m]" -> "Fable 5.1". Used when no name was given.
pub fn pretty_name(id: &str) -> String {
    let id = id.split('[').next().unwrap_or(id);
    let id = id.strip_prefix("claude-").unwrap_or(id);
    let mut family = String::new();
    let mut numbers: Vec<&str> = Vec::new();
    for part in id.split('-') {
        if part.chars().all(|c| c.is_ascii_digit()) && !part.is_empty() {
            // a date suffix such as 20251101 is not a version
            if part.len() <= 2 {
                numbers.push(part);
            }
        } else if numbers.is_empty() {
            if !family.is_empty() {
                family.push(' ');
            }
            family.push_str(part);
        }
    }
    let family: String = family.split(' ').map(|w| { let mut c = w.chars(); c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default() }).collect::<Vec<_>>().join(" ");
    if numbers.is_empty() { family } else { format!("{family} {}", numbers.join(".")) }
}

fn parse_item(m: &Value) -> Option<ModelInfo> {
    let id = m["id"].as_str()?.to_string();
    let name = m["display_name"].as_str().map(|n| n.strip_prefix("Claude ").unwrap_or(n).to_string()).unwrap_or_else(|| pretty_name(&id));
    let efforts = m["capabilities"]["effort"].as_object().filter(|e| e.get("supported") == Some(&Value::Bool(true))).map(|e| {
        EFFORT_ORDER.iter().filter(|l| e.get(**l).is_some_and(|v| v["supported"] == true)).map(|l| l.to_string()).collect::<Vec<_>>()
    });
    let efforts = match (m["capabilities"]["effort"].is_object(), efforts) {
        (true, Some(v)) => Some(v),
        (true, None) => Some(Vec::new()), // the model has no effort setting
        _ => None,
    };
    Some(ModelInfo { id, name, created: m["created_at"].as_str().unwrap_or("").to_string(), efforts, note: String::new(), max_input_tokens: m["max_input_tokens"].as_u64() })
}

/// Every model the key can see, newest first. Follows the pages of the list.
pub fn fetch_anthropic(base_url: &str, key: &str) -> Result<Vec<ModelInfo>, String> {
    // the same agent as every other call that sends a key: for a restricted account only public addresses, and no redirects
    let agent = super::agent::quick_agent(Duration::from_secs(20));
    let base = base_url.trim_end_matches('/');
    let mut out: Vec<ModelInfo> = Vec::new();
    let mut after: Option<String> = None;
    for _ in 0..10 {
        let mut req = agent.get(&format!("{base}/models")).query("limit", "100").set("x-api-key", key).set("anthropic-version", "2023-06-01");
        if let Some(a) = &after {
            req = req.query("after_id", a);
        }
        let v: Value = req.call().map_err(|e| format!("The model list could not be loaded: {e}"))?.into_json().map_err(|e| e.to_string())?;
        out.extend(v["data"].as_array().into_iter().flatten().filter_map(parse_item));
        match (v["has_more"].as_bool(), v["last_id"].as_str()) {
            (Some(true), Some(l)) => after = Some(l.to_string()),
            _ => break,
        }
    }
    out.sort_by(|a, b| b.created.cmp(&a.created));
    Ok(out)
}

/// Loads the list with the Anthropic key (if there is one) and keeps it. Returns what was loaded.
pub fn refresh(base_url: &str, key: &str) -> Result<Vec<ModelInfo>, String> {
    let list = fetch_anthropic(base_url, key)?;
    save_cache(&list);
    Ok(list)
}

/// Refreshes the cache when it is older than a day (does nothing without a key). For a background thread at start.
pub fn refresh_if_stale(base_url: &str, key: Option<&str>) {
    let c = load_cache();
    if let Some(k) = key {
        if c.models.is_empty() || now().saturating_sub(c.fetched) > 24 * 3600 {
            let _ = refresh(base_url, k);
        }
    }
}

/// What Claude Code wrote in its own cache about extra models (`~/.claude.json`, only that one entry; nothing about the login is read).
fn claude_code_extras() -> Vec<ModelInfo> {
    let path = std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".claude.json"));
    let Some(text) = path.and_then(|p| std::fs::read_to_string(p).ok()) else { return Vec::new() };
    let Ok(v) = serde_json::from_str::<Value>(&text) else { return Vec::new() };
    v["additionalModelOptionsCache"].as_array().into_iter().flatten().filter_map(|m| {
        let id = m["value"].as_str()?.split('[').next()?.to_string();
        Some(ModelInfo { name: pretty_name(&id), id, note: m["description"].as_str().unwrap_or("").to_string(), ..Default::default() })
    }).collect()
}

/// The models to offer: the cached Models API list; else Claude Code's aliases and its own extras.
pub fn claude_models() -> Vec<ModelInfo> {
    let cached = load_cache().models;
    if !cached.is_empty() {
        return cached;
    }
    let mut v: Vec<ModelInfo> = ["opus", "sonnet", "haiku"].iter().map(|a| ModelInfo { id: a.to_string(), name: format!("{} (latest)", pretty_name(a)), note: "Always the newest of its kind".into(), ..Default::default() }).collect();
    for e in claude_code_extras() {
        if !v.iter().any(|m| m.id == e.id) {
            v.push(e);
        }
    }
    v
}

/// How a model is named in the interface: the name the API gave, else made from its id.
pub fn display_name(id: &str) -> String {
    claude_models().into_iter().find(|m| m.id == id).map(|m| m.name).unwrap_or_else(|| pretty_name(id))
}

/// The effort levels to offer for a model: what the API said, or all five when it did not say; none when the model has no effort setting.
pub fn effort_levels(id: &str) -> Vec<String> {
    match claude_models().into_iter().find(|m| m.id == id).and_then(|m| m.efforts) {
        Some(levels) => levels,
        None => EFFORT_ORDER.iter().map(|l| l.to_string()).collect(),
    }
}

/// The family of a model, for choosing the newest of each: opus, sonnet, haiku, fable, or the id itself.
pub fn family(id: &str) -> String {
    let id = id.strip_prefix("claude-").unwrap_or(id);
    id.split('-').next().unwrap_or(id).to_string()
}

/// (main, more): the newest model of each family first, then all the others, each group newest first.
pub fn split_main(models: &[ModelInfo]) -> (Vec<ModelInfo>, Vec<ModelInfo>) {
    let mut seen: Vec<String> = Vec::new();
    let (mut main, mut more) = (Vec::new(), Vec::new());
    for m in models {
        let f = family(&m.id);
        if !seen.contains(&f) {
            seen.push(f);
            main.push(m.clone());
        } else {
            more.push(m.clone());
        }
    }
    (main, more)
}

// ---------------------------------------------------------------------------------------------------------------------------
// The Claude Code catalog: every model the subscription can use, kept in one data file (data/claude-models.json) so it can be
// updated without touching code. A copy named `claude-code-models.json` in the data folder, when it is valid, replaces it.
// ---------------------------------------------------------------------------------------------------------------------------

/// The curated list that ships with the program.
const CATALOG_JSON: &str = include_str!("../../data/claude-models.json");

/// What Claude Code does with "no model chosen": it picks by itself, so no `--model` is passed.
pub const DEFAULT_ID: &str = "default";

/// One entry of the catalog: an alias (`sonnet`, `opusplan`...) or a full model id.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct CatalogModel {
    pub id: String,
    pub label: String,
    /// "default", "alias" or "model".
    pub kind: String,
    /// opus, sonnet, haiku, fable; empty for entries that belong to no family (Default, opusplan).
    #[serde(default)]
    pub family: String,
    /// "5.5", "4.8"...; empty for aliases that follow the newest.
    #[serde(default)]
    pub version: String,
    /// The size of its window in tokens (0: not known).
    #[serde(default)]
    pub context: u64,
    /// When it was released, `YYYY-MM` or `YYYY-MM-DD` (empty: not known); newest first is how a family is ordered.
    #[serde(default)]
    pub released: String,
    /// Other names the program accepts for the same model.
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub note: String,
    /// true: a real call with this id was answered on the maintainer's laptop; false: taken from the docs, not tried.
    #[serde(default)]
    pub verified: bool,
    /// Set by `catalog()`: the newest model of its family.
    #[serde(default)]
    pub recommended: bool,
}

#[derive(Deserialize)]
struct CatalogFile {
    #[serde(default)]
    updated: String,
    models: Vec<CatalogModel>,
}

/// Reads a catalog file's text. Every entry needs an id and a label, ids are unique, kinds are known.
pub fn parse_catalog(text: &str) -> Result<Vec<CatalogModel>, String> {
    let f: CatalogFile = serde_json::from_str(text).map_err(|e| format!("claude-models.json: {e}"))?;
    let _ = &f.updated;
    let mut seen = std::collections::HashSet::new();
    for m in &f.models {
        if m.id.trim().is_empty() || m.label.trim().is_empty() {
            return Err("claude-models.json: every model needs an id and a label".into());
        }
        if !["default", "alias", "model"].contains(&m.kind.as_str()) {
            return Err(format!("claude-models.json: {} has an unknown kind {:?}", m.id, m.kind));
        }
        if !seen.insert(m.id.clone()) {
            return Err(format!("claude-models.json: {} is listed twice", m.id));
        }
        if m.kind == "model" && (m.family.trim().is_empty() || m.version.trim().is_empty()) {
            return Err(format!("claude-models.json: {} needs a family and a version", m.id));
        }
    }
    if !f.models.iter().any(|m| m.kind != "model") || !f.models.iter().any(|m| m.kind == "model") {
        return Err("claude-models.json: needs at least one default or alias and one model".into());
    }
    let mut v = f.models;
    mark_recommended(&mut v);
    Ok(v)
}

/// "5.5" > "4.8" > "4"; compares the numbers, not the text.
fn version_key(v: &str) -> Vec<u32> {
    v.split('.').map(|p| p.parse().unwrap_or(0)).collect()
}

/// Sets `recommended` on the newest model of each family (full ids only; aliases and Default are never marked).
fn mark_recommended(v: &mut [CatalogModel]) {
    for i in 0..v.len() {
        v[i].recommended = false;
    }
    let mut best: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (i, m) in v.iter().enumerate() {
        if m.kind != "model" {
            continue;
        }
        let better = best.get(&m.family).map_or(true, |&j| (version_key(&m.version), &m.released) > (version_key(&v[j].version), &v[j].released));
        if better {
            best.insert(m.family.clone(), i);
        }
    }
    for i in best.into_values() {
        v[i].recommended = true;
    }
}

/// The catalog: the person's own copy in the data folder when it is valid, else the one that ships with the program.
pub fn catalog() -> Vec<CatalogModel> {
    catalog_from(std::fs::read_to_string(crate::audit::data_dir().join("claude-code-models.json")).ok().as_deref())
}

/// The catalog from a copy's text when it is valid, else the shipped one.
fn catalog_from(own: Option<&str>) -> Vec<CatalogModel> {
    own.and_then(|t| parse_catalog(t).ok()).unwrap_or_else(|| parse_catalog(CATALOG_JSON).unwrap_or_default())
}

/// The order families are shown in: the most capable first.
pub const FAMILY_ORDER: [&str; 4] = ["fable", "opus", "sonnet", "haiku"];

/// The catalog for a picker: Default and the aliases first, then each family with the newest model first.
/// Entries are `(group title, models)`; groups with no model are left out.
pub fn catalog_groups(c: &[CatalogModel]) -> Vec<(String, Vec<CatalogModel>)> {
    let mut out: Vec<(String, Vec<CatalogModel>)> = Vec::new();
    let top: Vec<CatalogModel> = c.iter().filter(|m| m.kind == "default").cloned().chain(c.iter().filter(|m| m.kind == "alias").cloned()).collect();
    if !top.is_empty() {
        out.push(("Shortcuts".into(), top));
    }
    let mut fams: Vec<String> = FAMILY_ORDER.iter().map(|s| s.to_string()).collect();
    for m in c.iter().filter(|m| m.kind == "model") {
        if !fams.contains(&m.family) {
            fams.push(m.family.clone());
        }
    }
    for f in fams {
        let mut g: Vec<CatalogModel> = c.iter().filter(|m| m.kind == "model" && m.family == f).cloned().collect();
        g.sort_by(|a, b| (version_key(&b.version), &b.released).cmp(&(version_key(&a.version), &a.released)));
        if !g.is_empty() {
            out.push((pretty_family(&f), g));
        }
    }
    out
}

fn pretty_family(f: &str) -> String {
    let mut c = f.chars();
    c.next().map(|x| x.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

/// The catalog as the clients receive it (`models` answer for Claude Code): groups with their models.
pub fn catalog_json() -> Value {
    let groups = catalog_groups(&catalog());
    serde_json::json!(groups.into_iter().map(|(title, models)| serde_json::json!({ "title": title, "models": models })).collect::<Vec<_>>())
}

/// Finds an entry by its id or by one of its other names (case-insensitive; a `[1m]` ending is part of the id).
pub fn catalog_find(id: &str) -> Option<CatalogModel> {
    let q = id.trim().to_lowercase();
    catalog().into_iter().find(|m| m.id.to_lowercase() == q || m.aliases.iter().any(|a| a.to_lowercase() == q))
}

/// The label for a Claude Code model id: from the catalog, else made from the id ("claude-opus-4-9" -> "Opus 4.9").
pub fn catalog_label(id: &str) -> String {
    catalog_find(id).map(|m| m.label).unwrap_or_else(|| pretty_name(id))
}

/// The window size of a Claude Code model: from the catalog; a `[1m]` ending means a million tokens; None when not known.
pub fn catalog_context(id: &str) -> Option<u64> {
    context_in(&catalog(), id)
}

/// `catalog_context` for a given catalog. An alias with no window of its own (`sonnet`, `opus`, `opusplan`, Default) has the window of
/// the model it resolves to: the recommended (newest) model of its family; Default and `opusplan` resolve through Opus.
fn context_in(c: &[CatalogModel], id: &str) -> Option<u64> {
    let q = id.trim().to_lowercase();
    if let Some(e) = c.iter().find(|m| m.id.to_lowercase() == q || m.aliases.iter().any(|a| a.to_lowercase() == q)) {
        if e.context > 0 {
            return Some(e.context);
        }
        if e.kind != "model" {
            let fam = if e.family.is_empty() { "opus" } else { e.family.as_str() };
            if let Some(n) = c.iter().find(|m| m.kind == "model" && m.recommended && m.family == fam).map(|m| m.context).filter(|n| *n > 0) {
                return Some(n);
            }
        }
    }
    if id.to_lowercase().ends_with("[1m]") {
        return Some(1_000_000);
    }
    None
}

/// What is passed to `--model`: nothing for Default, an id typed by hand as it is.
pub fn model_argument(id: &str) -> Option<String> {
    let t = id.trim();
    if t.is_empty() || t.eq_ignore_ascii_case(DEFAULT_ID) { None } else { Some(t.to_string()) }
}

/// A model id typed by hand must look like one: letters, digits and `. _ - : /` with an optional `[1m]`, at most 100 characters.
pub fn valid_model_id(id: &str) -> bool {
    let t = id.trim();
    let core = t.strip_suffix("[1m]").unwrap_or(t);
    // not a dash first: the value goes after `--model`, where the claude program would read it as an option
    !core.is_empty() && t.len() <= 100 && core.chars().next().is_some_and(|c| c.is_ascii_alphanumeric()) && core.chars().all(|c| c.is_ascii_alphanumeric() || "._-:/".contains(c))
}

/// "Answered by Opus 5.5" when the model the program reports is not the one chosen (a shortcut or Default resolves to something);
/// None when it is the same one.
pub fn answered_by(chosen: &str, reported: &str) -> Option<String> {
    let r = reported.trim();
    if r.is_empty() {
        return None;
    }
    let base = |s: &str| s.split('[').next().unwrap_or(s).to_lowercase();
    if base(chosen) == base(r) {
        return None;
    }
    Some(catalog_label(r))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn names_are_made_from_ids_and_the_api_item_is_read() {
        assert_eq!(pretty_name("claude-opus-4-8"), "Opus 4.8");
        assert_eq!(pretty_name("claude-fable-5-1[1m]"), "Fable 5.1");
        assert_eq!(pretty_name("claude-haiku-4-5-20251001"), "Haiku 4.5");
        assert_eq!(pretty_name("sonnet"), "Sonnet");
        let m = parse_item(&json!({ "id": "claude-opus-5-5", "display_name": "Claude Opus 5.5", "created_at": "2026-09-01T00:00:00Z",
            "capabilities": { "effort": { "supported": true, "low": { "supported": true }, "medium": { "supported": true }, "high": { "supported": true }, "xhigh": { "supported": false }, "max": { "supported": true } } } })).unwrap();
        assert_eq!((m.name.as_str(), m.efforts.clone().unwrap()), ("Opus 5.5", vec!["low".to_string(), "medium".into(), "high".into(), "max".into()]));
        let none = parse_item(&json!({ "id": "claude-haiku-4-5", "capabilities": { "effort": { "supported": false } } })).unwrap();
        assert_eq!(none.efforts, Some(vec![]));
        assert_eq!(parse_item(&json!({ "id": "x" })).unwrap().efforts, None);
    }

    #[test]
    fn the_newest_of_each_family_comes_first() {
        let m = |id: &str| ModelInfo { id: id.into(), name: pretty_name(id), ..Default::default() };
        let all = vec![m("claude-opus-5-5"), m("claude-sonnet-5-5"), m("claude-opus-5"), m("claude-haiku-5-5"), m("claude-opus-4-8")];
        let (main, more) = split_main(&all);
        assert_eq!(main.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5"]);
        assert_eq!(more.len(), 2);
    }

    #[test]
    fn the_shipped_catalog_is_valid_and_complete() {
        let c = parse_catalog(CATALOG_JSON).expect("the shipped claude-models.json is valid");
        let ids: Vec<&str> = c.iter().map(|m| m.id.as_str()).collect();
        for want in ["default", "sonnet", "opus", "haiku", "claude-fable-5-2", "claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5", "claude-opus-4-8", "claude-opus-4-7", "claude-opus-4-6"] {
            assert!(ids.contains(&want), "{want} missing");
        }
        for m in &c {
            assert!(!m.label.is_empty() && !m.note.is_empty(), "{} needs a label and a note", m.id);
            if m.kind == "model" {
                assert!(FAMILY_ORDER.contains(&m.family.as_str()) && !m.version.is_empty() && m.context > 0 && [0, 7, 10].contains(&m.released.len()), "{} is incomplete", m.id);
                assert!(m.id.starts_with("claude-"), "{}", m.id);
            }
        }
    }

    #[test]
    fn the_newest_of_each_family_is_recommended_and_groups_run_newest_first() {
        let c = parse_catalog(CATALOG_JSON).unwrap();
        let rec: Vec<&str> = c.iter().filter(|m| m.recommended).map(|m| m.id.as_str()).collect();
        for want in ["claude-fable-5-2", "claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5"] {
            assert!(rec.contains(&want), "{want} should be recommended, got {rec:?}");
        }
        assert_eq!(rec.len(), 4);
        let g = catalog_groups(&c);
        assert_eq!(g[0].0, "Shortcuts");
        assert_eq!(g[0].1[0].id, "default");
        let opus = g.iter().find(|(t, _)| t == "Opus").unwrap();
        assert_eq!(opus.1[0].id, "claude-opus-5-5");
        assert!(opus.1.windows(2).all(|w| version_key(&w[0].version) >= version_key(&w[1].version)));
        assert!(g.iter().position(|(t, _)| t == "Fable").unwrap() < g.iter().position(|(t, _)| t == "Haiku").unwrap());
    }

    #[test]
    fn a_bad_catalog_is_refused() {
        assert!(parse_catalog("{").is_err());
        assert!(parse_catalog(r#"{"models":[{"id":"a","label":"A","kind":"x"}]}"#).unwrap_err().contains("unknown kind"));
        assert!(parse_catalog(r#"{"models":[{"id":"a","label":"A","kind":"alias"},{"id":"a","label":"B","kind":"alias"}]}"#).unwrap_err().contains("twice"));
        assert!(parse_catalog(r#"{"models":[{"id":"","label":"A","kind":"alias"}]}"#).is_err());
        // an empty list, no model, or a model with no family or version would replace the shipped list with nothing useful
        assert!(parse_catalog(r#"{"models":[]}"#).is_err());
        assert!(parse_catalog(r#"{"models":[{"id":"a","label":"A","kind":"alias"}]}"#).is_err());
        assert!(parse_catalog(r#"{"models":[{"id":"a","label":"A","kind":"alias"},{"id":"claude-x","label":"X","kind":"model"}]}"#).unwrap_err().contains("family"));
    }

    #[test]
    fn a_copy_of_the_catalog_replaces_the_shipped_one_only_when_it_is_valid() {
        let own = r#"{"models":[{"id":"mine","label":"Mine","kind":"alias"},{"id":"claude-opus-9","label":"Opus 9","kind":"model","family":"opus","version":"9","context":1000000}]}"#;
        assert!(catalog_from(Some(own)).iter().any(|m| m.id == "mine"));
        let shipped: Vec<String> = catalog_from(None).into_iter().map(|m| m.id).collect();
        assert!(shipped.contains(&"claude-opus-5-5".to_string()));
        for bad in ["{", r#"{"models":[]}"#, r#"{"models":[{"id":"a","label":"A","kind":"alias"}]}"#] {
            assert_eq!(catalog_from(Some(bad)).into_iter().map(|m| m.id).collect::<Vec<_>>(), shipped, "{bad} must fall back");
        }
    }

    #[test]
    fn an_alias_has_the_window_of_the_model_it_resolves_to() {
        let c = parse_catalog(CATALOG_JSON).unwrap();
        for a in ["default", "sonnet", "opus", "haiku", "fable", "opusplan"] {
            let want = context_in(&c, match a { "default" | "opusplan" => "claude-opus-5-5", "sonnet" => "claude-sonnet-5-5", "opus" => "claude-opus-5-5", "haiku" => "claude-haiku-5-5", _ => "claude-fable-5-2" });
            assert!(want.is_some());
            assert_eq!(context_in(&c, a), want, "{a}");
        }
    }

    #[test]
    fn ids_labels_windows_and_the_argument_follow_the_catalog() {
        assert_eq!(model_argument("default"), None);
        assert_eq!(model_argument(""), None);
        assert_eq!(model_argument(" claude-opus-5-5 ").as_deref(), Some("claude-opus-5-5"));
        assert_eq!(catalog_label("claude-opus-5-5"), "Opus 5.5");
        assert_eq!(catalog_label("claude-opus-9-9"), "Opus 9.9");
        assert_eq!(catalog_context("claude-opus-4-5"), Some(200_000));
        assert_eq!(catalog_context("claude-whatever-1[1m]"), Some(1_000_000));
        assert_eq!(catalog_context("claude-whatever-1"), None);
        assert!(valid_model_id("claude-opus-4-9") && valid_model_id("sonnet[1m]") && valid_model_id("vendor/model:tag"));
        assert!(!valid_model_id("--help") && !valid_model_id("-p") && !valid_model_id("-x[1m]"));
        assert!(!valid_model_id("") && !valid_model_id("a b") && !valid_model_id("a;rm -rf") && !valid_model_id(&"x".repeat(101)));
    }

    #[test]
    fn the_answering_model_is_shown_only_when_it_differs() {
        assert_eq!(answered_by("claude-opus-5-5", "claude-opus-5-5"), None);
        assert_eq!(answered_by("opus[1m]", "claude-opus-5-5[1m]"), Some("Opus 5.5".into()));
        assert_eq!(answered_by("sonnet", "claude-sonnet-5-5"), Some("Sonnet 5.5".into()));
        assert_eq!(answered_by("default", "claude-haiku-5-5"), Some("Haiku 5.5".into()));
        assert_eq!(answered_by("sonnet", ""), None);
    }
}
