//! Named custom providers: a server whose address the person types, with a name of their own ("My router", "Home Ollama") and one of
//! the three API formats (`chat`, `responses`, `claude`). There can be as many as they want. Each is a provider with the id
//! `custom:<slug>`; its key lives in the key store under that same id (typed for that address only, and only synced when key sync is
//! on). The list is kept in the picker preferences (`ui_prefs`, so it travels with them), and never holds a key.
//!
//! Before names existed there were three fixed providers, `custom`, `custom-responses` and `custom-claude`. `ensure_migrated` turns
//! those the person used into named ones ("Custom endpoint", "Custom endpoint (Responses)", "Custom endpoint (Claude)") once, with
//! their address, key, profiles, hidden state and the current choice.

use super::providers::{self, Protocol, Provider};
use super::{keys, profiles, ui_prefs, Selection};
use serde::{Deserialize, Serialize};

/// The most custom providers one person may have.
pub const MAX: usize = 50;
/// The longest name.
pub const MAX_NAME: usize = 40;
/// The address a new custom provider starts with in a form.
pub const DEFAULT_URL: &str = "http://127.0.0.1:8000/v1";

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct CustomProvider {
    /// `custom:<slug>`: never the name, so a rename changes nothing that is stored.
    pub id: String,
    pub name: String,
    /// The API format: `chat`, `responses` or `claude`.
    pub format: String,
    pub base_url: String,
    /// The model to start with when this provider is chosen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
}

/// A name for a provider or an account: 1..40 characters, no control characters. Returns it trimmed.
pub fn clean_name(name: &str, what: &str) -> Result<String, String> {
    let n = name.trim();
    if n.is_empty() {
        return Err(format!("Give the {what} a name."));
    }
    if n.chars().count() > MAX_NAME {
        return Err(format!("The {what} name can have at most {MAX_NAME} characters."));
    }
    if n.chars().any(char::is_control) {
        return Err(format!("The {what} name cannot have control characters."));
    }
    Ok(n.to_string())
}

/// A name turned into the part of an id that is safe in a file name, a key id or an address: lowercase letters, digits and single
/// dashes, at most 30 characters; never empty.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let out: String = out.trim_end_matches('-').chars().take(30).collect();
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() { "item".into() } else { out }
}

/// Is this a slug as `slug` makes them (what an id may be made of)?
pub fn is_slug(s: &str) -> bool {
    !s.is_empty() && s.len() <= 40 && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') && !s.starts_with('-') && !s.ends_with('-')
}

/// The first of `slug`, `slug-2`, `slug-3`... that is not taken.
pub fn unique_slug(base: &str, taken: impl Fn(&str) -> bool) -> String {
    let base = slug(base);
    if !taken(&base) {
        return base;
    }
    (2..).map(|n| format!("{base}-{n}")).find(|s| !taken(s)).unwrap_or(base)
}

/// The address of a custom provider: an http(s) address; a restricted account only a public https one.
pub fn check_address(url: &str) -> Result<(), String> {
    let u = url.trim();
    if u.is_empty() {
        return Err("Type the address of the server.".into());
    }
    if u.chars().count() > 500 {
        return Err("The address is too long.".into());
    }
    super::probe::check_address(u)?;
    if crate::tenant::is_restricted() {
        let any = providers::find("openai").ok_or("no provider")?;
        super::agent::check_restricted_provider(any, u)?;
    }
    Ok(())
}

fn check_one(c: &CustomProvider) -> Result<(), String> {
    let id_slug = c.id.strip_prefix("custom:").unwrap_or("");
    if !is_slug(id_slug) {
        return Err("A custom provider has an id that is not allowed.".into());
    }
    clean_name(&c.name, "provider")?;
    if providers::format_name(&c.format) != Some(c.format.as_str()) {
        return Err("The API format is one of: chat, responses, claude.".into());
    }
    check_address(&c.base_url)?;
    if let Some(m) = &c.default_model {
        if m.chars().count() > 300 || m.chars().any(char::is_control) {
            return Err("The default model is not valid.".into());
        }
    }
    Ok(())
}

/// Checks a whole list (a person's edit or a synced document): count, ids, unique names and ids, formats and addresses.
pub fn check_all(list: &[CustomProvider]) -> Result<(), String> {
    if list.len() > MAX {
        return Err(format!("At most {MAX} custom providers."));
    }
    for (i, c) in list.iter().enumerate() {
        check_one(c)?;
        if list[..i].iter().any(|o| o.id == c.id) {
            return Err("Two custom providers have the same id.".into());
        }
        if list[..i].iter().any(|o| o.name.trim().eq_ignore_ascii_case(c.name.trim())) {
            return Err(format!("There is already a custom provider named \"{}\".", c.name.trim()));
        }
    }
    Ok(())
}

fn marker() -> std::path::PathBuf {
    crate::audit::config_dir().join("custom-providers-migrated")
}

/// The custom providers, oldest first.
pub fn list() -> Vec<CustomProvider> {
    ensure_migrated();
    ui_prefs::load().custom_providers
}

pub fn get(id: &str) -> Option<CustomProvider> {
    list().into_iter().find(|c| c.id == id)
}

fn to_provider(c: &CustomProvider) -> &'static Provider {
    let protocol = providers::format_protocol(&c.format).unwrap_or(Protocol::OpenAi);
    let models: &'static [&'static str] = match c.default_model.as_deref().filter(|m| !m.trim().is_empty()) {
        Some(m) => Box::leak(vec![providers::leak_str(m.trim())].into_boxed_slice()),
        None => &[],
    };
    providers::leak(Provider { id: providers::leak_str(&c.id), name: providers::leak_str(&c.name), protocol, base_url: providers::leak_str(c.base_url.trim()), local: false, models })
}

/// The provider of one custom entry (`custom:<slug>`); none when there is no such one.
pub fn resolve(id: &str) -> Option<&'static Provider> {
    get(id).map(|c| to_provider(&c))
}

/// All of them as providers.
pub fn providers() -> Vec<&'static Provider> {
    list().iter().map(to_provider).collect()
}

/// A custom provider that is not saved yet (a form's Test button): the format and the address as typed.
pub fn draft(format: &str, base_url: &str) -> Result<&'static Provider, String> {
    let protocol = providers::format_protocol(format).ok_or("The API format is one of: chat, responses, claude.")?;
    check_address(base_url)?;
    Ok(providers::leak(Provider { id: "custom:draft", name: "this server", protocol, base_url: providers::leak_str(base_url.trim()), local: false, models: &[] }))
}

/// Adds a custom provider. The key (if any) is the caller's to store under the returned id.
pub fn add(name: &str, format: &str, base_url: &str, default_model: Option<&str>) -> Result<CustomProvider, String> {
    let name = clean_name(name, "provider")?;
    let format = providers::format_name(format).ok_or("The API format is one of: chat, responses, claude.")?.to_string();
    let base_url = base_url.trim().to_string();
    let default_model = default_model.map(str::trim).filter(|m| !m.is_empty()).map(str::to_string);
    let mut made = None;
    ui_prefs::edit(|p| {
        let id = format!("custom:{}", unique_slug(&name, |s| p.custom_providers.iter().any(|c| c.id == format!("custom:{s}"))));
        let c = CustomProvider { id, name: name.clone(), format: format.clone(), base_url: base_url.clone(), default_model: default_model.clone() };
        p.custom_providers.push(c.clone());
        made = Some(c);
    })?;
    // `edit` returns the old preferences unchanged when its check refused nothing but also changed nothing; a refusal is an Err
    made.ok_or_else(|| "The custom provider could not be added.".to_string())
}

/// Changes a custom provider (`None` leaves a field as it is; an empty default model forgets it). The id never changes.
pub fn update(id: &str, name: Option<&str>, format: Option<&str>, base_url: Option<&str>, default_model: Option<&str>) -> Result<CustomProvider, String> {
    let name = name.map(|n| clean_name(n, "provider")).transpose()?;
    let format = format.map(|f| providers::format_name(f).map(str::to_string).ok_or("The API format is one of: chat, responses, claude.")).transpose()?;
    let old = get(id).ok_or("That custom provider is gone.")?;
    let mut out = None;
    ui_prefs::edit(|p| {
        if let Some(c) = p.custom_providers.iter_mut().find(|c| c.id == id) {
            if let Some(n) = &name {
                c.name = n.clone();
            }
            if let Some(f) = &format {
                c.format = f.clone();
            }
            if let Some(u) = base_url {
                c.base_url = u.trim().to_string();
            }
            if let Some(m) = default_model {
                c.default_model = Some(m.trim().to_string()).filter(|m| !m.is_empty());
            }
            out = Some(c.clone());
        }
    })?;
    let c = out.ok_or_else(|| "That custom provider is gone.".to_string())?;
    // a key typed for one address is never sent to another: a new address leaves the provider (and its accounts) without keys
    if old.base_url.trim().trim_end_matches('/') != c.base_url.trim().trim_end_matches('/') {
        keys::clear(&c.id);
        for a in super::accounts::extra(&c.id) {
            keys::clear(&super::accounts::key_id(&c.id, Some(&a.id)));
        }
    }
    Ok(c)
}

/// Removes a custom provider and everything that belongs to it: its key and its accounts' keys, its profiles (and their keys), its
/// accounts, its hidden state, and the current choice when it was the chosen one.
pub fn remove(id: &str) -> Result<(), String> {
    let c = get(id).ok_or("That custom provider is gone.")?;
    let accts: Vec<String> = ui_prefs::load().accounts.get(&c.id).map(|v| v.iter().map(|a| a.id.clone()).collect()).unwrap_or_default();
    ui_prefs::edit(|p| {
        p.custom_providers.retain(|x| x.id != c.id);
        p.accounts.remove(&c.id);
        p.default_accounts.remove(&c.id);
        p.hidden_providers.retain(|h| h != &c.id);
        p.provider_prefs.remove(&c.id);
    })?;
    keys::clear(&c.id);
    for a in accts {
        keys::clear(&format!("{}:{a}", c.id));
    }
    for pr in profiles::load().into_iter().filter(|p| p.provider == c.id) {
        profiles::delete(&pr.id);
    }
    let mut sel = Selection::load();
    let mut changed = sel.base_urls.remove(&c.id).is_some();
    sel.api_formats.remove(&c.id);
    if sel.provider == c.id {
        sel = Selection { base_urls: sel.base_urls, ..Default::default() };
        changed = true;
    }
    if changed {
        sel.save();
    }
    Ok(())
}

// ---- the three fixed providers of older versions ----

const OLD: [(&str, &str, &str); 3] = [("custom", "chat", "Custom endpoint"), ("custom-responses", "responses", "Custom endpoint (Responses)"), ("custom-claude", "claude", "Custom endpoint (Claude)")];

thread_local! {
    static MIGRATING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
static MIGRATION: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Turns the old fixed custom providers the person used into named ones, once per configuration folder.
pub fn ensure_migrated() {
    if MIGRATING.with(|m| m.get()) || marker().exists() {
        return;
    }
    let _g = MIGRATION.lock().unwrap_or_else(|e| e.into_inner());
    if marker().exists() {
        return;
    }
    MIGRATING.with(|m| m.set(true));
    migrate();
    MIGRATING.with(|m| m.set(false));
    let _ = std::fs::create_dir_all(crate::audit::config_dir());
    let _ = std::fs::write(marker(), "1");
}

fn migrate() {
    let mut sel = Selection::load();
    let mut all_profiles = profiles::load();
    let prefs = ui_prefs::load();
    let mut sel_changed = false;
    let mut profiles_changed = false;
    for (old, format, name) in OLD {
        let used = sel.provider == old
            || sel.base_urls.contains_key(old)
            || all_profiles.iter().any(|p| p.provider == old)
            || prefs.hidden_providers.iter().any(|h| h == old)
            || prefs.provider_prefs.contains_key(old)
            || keys::get(old).is_some();
        if !used {
            continue;
        }
        let url = sel.base_urls.get(old).map(|u| u.trim().to_string()).filter(|u| !u.is_empty()).unwrap_or_else(|| DEFAULT_URL.to_string());
        // an address that does not pass today's checks is kept as typed would be lost: fall back to the default one but keep the old in the name
        let mut entry = CustomProvider { id: String::new(), name: name.to_string(), format: format.to_string(), base_url: url, default_model: None };
        let mut new_id = String::new();
        let r = ui_prefs::edit(|p| {
            entry.id = format!("custom:{}", unique_slug(name, |s| p.custom_providers.iter().any(|c| c.id == format!("custom:{s}"))));
            new_id = entry.id.clone();
            if check_one(&entry).is_err() {
                entry.base_url = DEFAULT_URL.to_string();
            }
            p.custom_providers.push(entry.clone());
            if let Some(i) = p.hidden_providers.iter().position(|h| h == old) {
                p.hidden_providers[i] = new_id.clone();
            }
            if let Some(v) = p.provider_prefs.remove(old) {
                p.provider_prefs.insert(new_id.clone(), v);
            }
        });
        if r.is_err() || new_id.is_empty() {
            continue;
        }
        if let Some(k) = keys::get(old) {
            if keys::set(&new_id, &k).is_ok() {
                keys::clear(old);
            }
        }
        for p in all_profiles.iter_mut().filter(|p| p.provider == old) {
            let old_key = format!("{old}@{}", p.id);
            if let Some(k) = keys::get(&old_key) {
                if keys::set(&format!("{new_id}@{}", p.id), &k).is_ok() {
                    keys::clear(&old_key);
                }
            }
            p.provider = new_id.clone();
            p.base_url = None;
            profiles_changed = true;
        }
        if sel.provider == old {
            sel.provider = new_id.clone();
        }
        sel.base_urls.remove(old);
        sel_changed = true;
    }
    if profiles_changed {
        profiles::save(&all_profiles);
    }
    if sel_changed {
        sel.save();
    }
}

/// A profile or setting from a computer that still has the three fixed providers names `custom`, `custom-responses` or
/// `custom-claude`: this is the named provider that stands in for it here (made, with the default name, when there is none yet).
/// `None` for any other id. The address and key of the old one are not part of it: the profile's own address (if any) is kept.
pub fn legacy_instance(old: &str, address: Option<&str>) -> Option<Result<String, String>> {
    let (_, format, name) = OLD.iter().find(|(o, _, _)| *o == old)?;
    ensure_migrated();
    if let Some(c) = ui_prefs::load().custom_providers.into_iter().find(|c| c.name == *name && c.format == *format) {
        return Some(Ok(c.id));
    }
    let url = address.map(str::trim).filter(|u| !u.is_empty()).unwrap_or(DEFAULT_URL);
    Some(add(name, format, url, None).map(|c| c.id))
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    /// A custom provider for tests, without a check of the address being run twice: returns its id.
    pub fn add(name: &str, format: &str, url: &str) -> String {
        add_with_key(name, format, url, None)
    }

    pub fn add_with_key(name: &str, format: &str, url: &str, key: Option<&str>) -> String {
        let c = super::add(name, format, url, None).unwrap();
        if let Some(k) = key {
            keys::set(&c.id, k).unwrap();
        }
        c.id
    }
}
