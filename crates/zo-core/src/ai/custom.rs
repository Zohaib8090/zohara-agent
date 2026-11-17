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
mod tests {
    use super::*;
    use crate::store::TEST_ENV_LOCK;
    use crate::tenant::testkit::*;

    #[test]
    fn slugs_are_safe_unique_and_never_the_name() {
        assert_eq!(slug("My router"), "my-router");
        assert_eq!(slug("  Work   Gateway!! "), "work-gateway");
        assert_eq!(slug("../../etc/passwd"), "etc-passwd");
        assert_eq!(slug("Ünïcode ok"), "n-code-ok");
        assert_eq!(slug("!!!"), "item");
        assert_eq!(slug(&"a".repeat(80)).len(), 30);
        assert!(!slug(&format!("{} b", "a".repeat(29))).ends_with('-'));
        for name in ["My router", "a/b", "x:y", "..", "Ünï", "&&&", "default"] {
            assert!(is_slug(&slug(name)), "{name}");
        }
        assert!(is_slug("groq-2") && !is_slug("") && !is_slug("-a") && !is_slug("a-") && !is_slug("A") && !is_slug("a:b") && !is_slug("a/b") && !is_slug("a b"));
        let taken = ["pool", "pool-2"];
        assert_eq!(unique_slug("Pool", |s| taken.contains(&s)), "pool-3");
        assert_eq!(unique_slug("Other", |s| taken.contains(&s)), "other");
    }

    #[test]
    fn names_are_one_to_forty_characters_without_control_characters() {
        assert_eq!(clean_name("  My router  ", "provider").unwrap(), "My router");
        assert!(clean_name(&"x".repeat(40), "provider").is_ok());
        for bad in ["", "   ", &"x".repeat(41), "a\nb", "a\u{7}b", "a\u{0}b"] {
            assert!(clean_name(bad, "provider").is_err(), "{bad:?}");
        }
        assert!(clean_name("Ünï Ñame 名前", "account").is_ok());
        assert_eq!(clean_name("", "account").unwrap_err(), "Give the account a name.");
    }

    fn entry(id: &str, name: &str, format: &str, url: &str) -> CustomProvider {
        CustomProvider { id: id.into(), name: name.into(), format: format.into(), base_url: url.into(), default_model: None }
    }

    #[test]
    fn a_list_of_custom_providers_is_checked_as_a_whole() {
        let _g = TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let ok = vec![entry("custom:a", "A", "chat", "http://127.0.0.1:1/v1"), entry("custom:b", "B", "claude", "https://example.com/v1")];
        assert!(check_all(&ok).is_ok());
        let mut dup_id = ok.clone();
        dup_id[1].id = "custom:a".into();
        assert!(check_all(&dup_id).unwrap_err().contains("same id"));
        let mut dup_name = ok.clone();
        dup_name[1].name = " a ".into();
        assert!(check_all(&dup_name).unwrap_err().contains("already a custom provider named"));
        for bad_id in ["custom:", "custom:A", "custom:a/b", "custom:../x", "openai", "custom:a:b", "custom:-a"] {
            let mut l = ok.clone();
            l[0].id = bad_id.into();
            assert!(check_all(&l).is_err(), "{bad_id}");
        }
        let mut l = ok.clone();
        l[0].format = "grpc".into();
        assert!(check_all(&l).is_err());
        for bad_url in ["", "ftp://x/v1", "not a url", "http://"] {
            let mut l = ok.clone();
            l[0].base_url = bad_url.into();
            assert!(check_all(&l).is_err(), "{bad_url:?}");
        }
        let many: Vec<CustomProvider> = (0..=MAX).map(|i| entry(&format!("custom:p{i}"), &format!("P{i}"), "chat", "http://127.0.0.1:1/v1")).collect();
        assert!(check_all(&many).unwrap_err().contains("At most"));
    }

    #[test]
    fn the_config_of_the_list_is_read_back_and_a_key_is_never_part_of_it() {
        let _g = TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let t = fresh(false);
        crate::tenant::scope(&t, || {
            let c = add("My router", "responses", " http://127.0.0.1:5/v1 ", Some(" gpt-5 ")).unwrap();
            assert_eq!((c.id.as_str(), c.name.as_str(), c.format.as_str(), c.base_url.as_str(), c.default_model.as_deref()), ("custom:my-router", "My router", "responses", "http://127.0.0.1:5/v1", Some("gpt-5")));
            keys::set(&c.id, "sk-secret-123").unwrap();
            let raw = std::fs::read_to_string(crate::audit::config_dir().join("ui-prefs.json")).unwrap_or_default();
            assert!(raw.contains("my-router") && !raw.contains("sk-secret-123"), "{raw}");
            // the provider as the agent sees it: format, address, default model
            let p = resolve("custom:my-router").unwrap();
            assert_eq!((p.protocol, p.base_url, p.models, p.name), (Protocol::OpenAiResponses, "http://127.0.0.1:5/v1", &["gpt-5"][..], "My router"));
            assert!(providers::find("custom:my-router").is_some() && providers::listed().iter().any(|p| p.id == "custom:my-router"));
            // a rename keeps the id, the key and the address
            let r = update("custom:my-router", Some("Renamed"), None, None, None).unwrap();
            assert_eq!((r.id.as_str(), r.name.as_str()), ("custom:my-router", "Renamed"));
            assert_eq!(keys::get("custom:my-router").as_deref(), Some("sk-secret-123"));
            // a new address drops the key typed for the old one; the same address (with a slash) keeps it
            update("custom:my-router", None, None, Some("http://127.0.0.1:5/v1/"), None).unwrap();
            assert_eq!(keys::get("custom:my-router").as_deref(), Some("sk-secret-123"));
            update("custom:my-router", None, None, Some("http://127.0.0.1:6/v1"), None).unwrap();
            assert_eq!(keys::get("custom:my-router"), None);
            // the same name makes a second provider with a different id... only when the names differ; the same name is refused
            assert!(add("renamed", "chat", "http://127.0.0.1:7/v1", None).is_err());
            let b = add("Renamed 2", "chat", "http://127.0.0.1:7/v1", None).unwrap();
            assert_eq!(b.id, "custom:renamed-2");
            assert!(draft("chat", "http://127.0.0.1:7/v1").is_ok() && draft("grpc", "http://127.0.0.1:7/v1").is_err() && draft("chat", "").is_err());
            remove("custom:my-router").unwrap();
            assert!(remove("custom:my-router").is_err());
            assert_eq!(list().len(), 1);
        });
    }

    fn profile(id: &str, provider: &str) -> profiles::Profile {
        profiles::Profile { id: id.into(), name: format!("Profile {id}"), provider: provider.into(), model: "m".into(), ..Default::default() }
    }

    #[test]
    fn the_old_single_custom_provider_becomes_one_named_instance_with_nothing_lost() {
        let _g = TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let t = fresh(false);
        crate::tenant::scope(&t, || {
            let mut sel = Selection { provider: "custom".into(), model: "mm".into(), effort: Some("high".into()), ..Default::default() };
            sel.base_urls.insert("custom".into(), "http://127.0.0.1:7001/v1".into());
            sel.save();
            keys::set("custom", "k-old").unwrap();
            keys::set("custom@p1", "k-profile").unwrap();
            profiles::save(&[profile("p1", "custom"), profile("p2", "openai")]);
            ui_prefs::set_hidden(vec!["custom".into(), "ollama".into()]).unwrap();
            ui_prefs::set_provider_pref("custom", serde_json::json!({"pinned": ["mm"]})).unwrap();
            let l = list();
            assert_eq!(l.len(), 1, "{l:?}");
            let c = &l[0];
            assert_eq!((c.id.as_str(), c.name.as_str(), c.format.as_str(), c.base_url.as_str()), ("custom:custom-endpoint", "Custom endpoint", "chat", "http://127.0.0.1:7001/v1"));
            // the choice, the key, the profile (and its key), the hidden state and the pins moved over; the old ids are gone
            let sel = Selection::load();
            assert_eq!((sel.provider.as_str(), sel.model.as_str(), sel.effort.as_deref()), ("custom:custom-endpoint", "mm", Some("high")));
            assert!(!sel.base_urls.contains_key("custom"));
            assert_eq!((keys::get("custom:custom-endpoint").as_deref(), keys::get("custom")), (Some("k-old"), None));
            assert_eq!((keys::get("custom:custom-endpoint@p1").as_deref(), keys::get("custom@p1")), (Some("k-profile"), None));
            let all = profiles::load();
            assert_eq!((all[0].provider.as_str(), all[1].provider.as_str()), ("custom:custom-endpoint", "openai"));
            let prefs = ui_prefs::load();
            assert_eq!(prefs.hidden_providers, ["custom:custom-endpoint", "ollama"]);
            assert_eq!(prefs.provider_prefs.get("custom:custom-endpoint").unwrap()["pinned"][0], "mm");
            assert!(!prefs.provider_prefs.contains_key("custom"));
            // it does not run twice, and an old id typed again later is not turned into a second instance
            assert_eq!(list().len(), 1);
            keys::set("custom", "stray").unwrap();
            assert_eq!(list().len(), 1);
        });
        // a person who never used the custom provider gets none
        let t2 = fresh(false);
        crate::tenant::scope(&t2, || assert!(list().is_empty()));
    }

    #[test]
    fn the_other_two_old_formats_are_migrated_with_their_own_names_and_a_bad_address_falls_back() {
        let _g = TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let t = fresh(false);
        crate::tenant::scope(&t, || {
            let mut sel = Selection { provider: "custom-claude".into(), model: "m".into(), ..Default::default() };
            sel.base_urls.insert("custom-claude".into(), "http://127.0.0.1:7003/v1".into());
            sel.base_urls.insert("custom-responses".into(), "ftp://not-allowed/v1".into());
            sel.save();
            keys::set("custom-responses", "k-r").unwrap();
            let l = list();
            let by = |id: &str| l.iter().find(|c| c.id == id).unwrap_or_else(|| panic!("{id} in {l:?}")).clone();
            assert_eq!(by("custom:custom-endpoint-claude").format, "claude");
            assert_eq!(by("custom:custom-endpoint-claude").base_url, "http://127.0.0.1:7003/v1");
            assert_eq!(by("custom:custom-endpoint-responses").format, "responses");
            assert_eq!(by("custom:custom-endpoint-responses").base_url, DEFAULT_URL);
            assert_eq!(Selection::load().provider, "custom:custom-endpoint-claude");
            assert_eq!(keys::get("custom:custom-endpoint-responses").as_deref(), Some("k-r"));
        });
    }

    #[test]
    fn a_synced_profile_from_a_computer_with_the_old_providers_lands_on_the_named_instance() {
        let _g = TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let t = fresh(false);
        crate::tenant::scope(&t, || {
            assert!(legacy_instance("openai", None).is_none());
            let id = legacy_instance("custom-responses", Some("http://127.0.0.1:7010/v1")).unwrap().unwrap();
            assert_eq!(id, "custom:custom-endpoint-responses");
            assert_eq!(legacy_instance("custom-responses", None).unwrap().unwrap(), id, "the same one the next time");
            assert_eq!(list().len(), 1);
        });
    }
}
