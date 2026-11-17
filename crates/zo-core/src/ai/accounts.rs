//! Several named accounts (API keys) for one provider: "Groq 1", "Groq 2". Every provider that takes a key can have them, the custom
//! providers too. The key each one uses is in the key store under its own id, isolated from the others:
//!
//! * the Default account (the key a provider had before accounts existed) keeps the provider's own id, `groq`, so nothing is moved;
//! * another account is `<provider>:<account slug>` (`groq:groq-2`, `custom:home-ollama:work`).
//!
//! The list (names, optional own address, hidden) is kept in the picker preferences, and never holds a key. Keys only travel
//! between computers when key sync is on. An optional setting switches to the next account of the same provider when one hits its
//! rate limit (never to another provider).

use super::custom::{clean_name, is_slug, unique_slug};
use super::providers::{self, Provider};
use super::{keys, profiles, ui_prefs, Selection};
use serde::{Deserialize, Serialize};

/// The id of the Default account (the one every provider has).
pub const DEFAULT_ID: &str = "default";
/// The name of the Default account until the person renames it.
pub const DEFAULT_NAME: &str = "Default";
/// The most extra accounts one provider may have.
pub const MAX_PER_PROVIDER: usize = 20;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Account {
    /// The slug: part of the key id, never the name.
    pub id: String,
    pub name: String,
    /// An address of its own, when the provider allows one (its key only goes to the provider's own host); none = the provider's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
}

/// What the person changed about the Default account of a provider.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct DefaultAccount {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
}

/// One account as shown to a person.
#[derive(Clone, Debug, PartialEq)]
pub struct View {
    pub id: String,
    pub name: String,
    pub is_default: bool,
    pub hidden: bool,
    pub base_url: Option<String>,
    pub has_key: bool,
}

/// The key store id of an account (`None` or `default` is the provider's own).
pub fn key_id(provider_id: &str, account: Option<&str>) -> String {
    match account.filter(|a| !a.is_empty() && *a != DEFAULT_ID) {
        Some(a) => format!("{provider_id}:{a}"),
        None => provider_id.to_string(),
    }
}

/// The key of an account.
pub fn get_key(provider_id: &str, account: Option<&str>) -> Option<String> {
    keys::get(&key_id(provider_id, account))
}

fn takes(p: &str) -> Result<&'static Provider, String> {
    let prov = providers::find(p).ok_or("unknown provider")?;
    if !prov.takes_accounts() {
        return Err(format!("{} has no API key, so it has no accounts.", prov.name));
    }
    Ok(prov)
}

/// The extra accounts of a provider (not the Default one).
pub fn extra(provider_id: &str) -> Vec<Account> {
    ui_prefs::load().accounts.get(provider_id).cloned().unwrap_or_default()
}

/// The address of an account of its own (none for the Default one or one without).
pub fn address_of(provider_id: &str, account: &str) -> Option<String> {
    extra(provider_id).into_iter().find(|a| a.id == account).and_then(|a| a.base_url).filter(|u| !u.trim().is_empty())
}

/// All accounts of a provider, the Default first.
pub fn list(provider_id: &str) -> Vec<View> {
    let prefs = ui_prefs::load();
    let d = prefs.default_accounts.get(provider_id).cloned().unwrap_or_default();
    let mut out = vec![View {
        id: DEFAULT_ID.into(),
        name: d.name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| DEFAULT_NAME.into()),
        is_default: true,
        hidden: d.hidden,
        base_url: None,
        has_key: get_key(provider_id, None).is_some(),
    }];
    for a in prefs.accounts.get(provider_id).into_iter().flatten() {
        out.push(View { id: a.id.clone(), name: a.name.clone(), is_default: false, hidden: a.hidden, base_url: a.base_url.clone(), has_key: get_key(provider_id, Some(&a.id)).is_some() });
    }
    out
}

/// Does this account exist (the Default one always does)?
pub fn exists(provider_id: &str, account: &str) -> bool {
    account == DEFAULT_ID || extra(provider_id).iter().any(|a| a.id == account)
}

/// "Groq · Groq 1": how a picker names a provider with its account. Without a second account it is the provider's name.
pub fn label(p: &Provider, account: Option<&str>) -> String {
    let all = list(p.id);
    if all.len() < 2 {
        return p.name.to_string();
    }
    let id = account.unwrap_or(DEFAULT_ID);
    let name = all.iter().find(|a| a.id == id).map(|a| a.name.clone()).unwrap_or_else(|| DEFAULT_NAME.into());
    format!("{} · {}", p.name, name)
}

fn name_taken(provider_id: &str, name: &str, except: Option<&str>) -> bool {
    list(provider_id).iter().any(|a| Some(a.id.as_str()) != except && a.name.trim().eq_ignore_ascii_case(name.trim()))
}

/// Adds an account. The key (if any) is the caller's to store with `key_id`.
pub fn add(provider_id: &str, name: &str, base_url: Option<&str>) -> Result<Account, String> {
    let prov = takes(provider_id)?;
    let name = clean_name(name, "account")?;
    if name_taken(provider_id, &name, None) {
        return Err(format!("{} already has an account named \"{name}\".", prov.name));
    }
    let base_url = base_url.map(str::trim).filter(|u| !u.is_empty()).map(str::to_string);
    if let Some(u) = &base_url {
        providers::check_base_url(prov, u)?;
    }
    let mut made = None;
    ui_prefs::edit(|p| {
        let v = p.accounts.entry(provider_id.to_string()).or_default();
        let id = unique_slug(&name, |s| s == DEFAULT_ID || v.iter().any(|a| a.id == s));
        let a = Account { id, name: name.clone(), base_url: base_url.clone(), hidden: false };
        v.push(a.clone());
        made = Some(a);
    })?;
    made.ok_or_else(|| "The account could not be added.".to_string())
}

/// Renames an account (the Default one too).
pub fn rename(provider_id: &str, account: &str, name: &str) -> Result<(), String> {
    let prov = takes(provider_id)?;
    let name = clean_name(name, "account")?;
    if name_taken(provider_id, &name, Some(account)) {
        return Err(format!("{} already has an account named \"{name}\".", prov.name));
    }
    if !exists(provider_id, account) {
        return Err("That account is gone.".into());
    }
    ui_prefs::edit(|p| {
        if account == DEFAULT_ID {
            let d = p.default_accounts.entry(provider_id.to_string()).or_default();
            d.name = Some(name.clone()).filter(|n| n != DEFAULT_NAME);
        } else if let Some(a) = p.accounts.get_mut(provider_id).and_then(|v| v.iter_mut().find(|a| a.id == account)) {
            a.name = name.clone();
        }
    })?;
    Ok(())
}

/// Changes the own address of an account (empty forgets it). Not for the Default one: its address is the provider's.
pub fn set_address(provider_id: &str, account: &str, base_url: &str) -> Result<(), String> {
    let prov = takes(provider_id)?;
    if account == DEFAULT_ID {
        return Err("The Default account uses the provider's address: change that one in the provider's settings.".into());
    }
    let u = base_url.trim();
    providers::check_base_url(prov, u)?;
    if !exists(provider_id, account) {
        return Err("That account is gone.".into());
    }
    ui_prefs::edit(|p| {
        if let Some(a) = p.accounts.get_mut(provider_id).and_then(|v| v.iter_mut().find(|a| a.id == account)) {
            a.base_url = Some(u.to_string()).filter(|u| !u.is_empty());
        }
    })?;
    Ok(())
}

pub fn set_hidden(provider_id: &str, account: &str, hidden: bool) -> Result<(), String> {
    takes(provider_id)?;
    if !exists(provider_id, account) {
        return Err("That account is gone.".into());
    }
    ui_prefs::edit(|p| {
        if account == DEFAULT_ID {
            let d = p.default_accounts.entry(provider_id.to_string()).or_default();
            d.hidden = hidden;
            if *d == DefaultAccount::default() {
                p.default_accounts.remove(provider_id);
            }
        } else if let Some(a) = p.accounts.get_mut(provider_id).and_then(|v| v.iter_mut().find(|a| a.id == account)) {
            a.hidden = hidden;
        }
    })?;
    Ok(())
}

/// Deletes an extra account and its key. The Default account stays (a provider always has one).
pub fn remove(provider_id: &str, account: &str) -> Result<(), String> {
    takes(provider_id)?;
    if account == DEFAULT_ID {
        return Err("The Default account cannot be deleted. Rename it, or hide it.".into());
    }
    if !extra(provider_id).iter().any(|a| a.id == account) {
        return Err("That account is gone.".into());
    }
    ui_prefs::edit(|p| {
        if let Some(v) = p.accounts.get_mut(provider_id) {
            v.retain(|a| a.id != account);
            if v.is_empty() {
                p.accounts.remove(provider_id);
            }
        }
    })?;
    keys::clear(&key_id(provider_id, Some(account)));
    // a chosen account that is gone falls back to the Default one
    let mut sel = Selection::load();
    if sel.provider == provider_id && sel.account.as_deref() == Some(account) {
        sel.account = None;
        sel.save();
    }
    let mut all = profiles::load();
    let mut changed = false;
    for p in all.iter_mut().filter(|p| p.provider == provider_id && p.account.as_deref() == Some(account)) {
        p.account = None;
        changed = true;
    }
    if changed {
        profiles::save(&all);
    }
    Ok(())
}

/// Checks the accounts of a whole preferences document (a person's edit or a synced one).
pub fn check_all(p: &ui_prefs::Prefs) -> Result<(), String> {
    let mut total = 0;
    for (prov, list) in &p.accounts {
        total += list.len();
        if list.len() > MAX_PER_PROVIDER || total > 400 {
            return Err("Too many accounts.".into());
        }
        if prov.is_empty() || prov.len() > 80 || !prov.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':')) {
            return Err("An account names a provider that is not allowed.".into());
        }
        for (i, a) in list.iter().enumerate() {
            if !is_slug(&a.id) || a.id == DEFAULT_ID {
                return Err("An account has an id that is not allowed.".into());
            }
            clean_name(&a.name, "account")?;
            if a.name.trim().eq_ignore_ascii_case(p.default_accounts.get(prov).and_then(|d| d.name.as_deref()).unwrap_or(DEFAULT_NAME)) {
                return Err(format!("There is already an account named \"{}\".", a.name.trim()));
            }
            if list[..i].iter().any(|o| o.id == a.id || o.name.trim().eq_ignore_ascii_case(a.name.trim())) {
                return Err(format!("There is already an account named \"{}\".", a.name.trim()));
            }
            if let Some(u) = a.base_url.as_deref().filter(|u| !u.trim().is_empty()) {
                super::probe::check_address(u)?;
                if crate::tenant::is_restricted() {
                    let any = providers::find("openai").ok_or("no provider")?;
                    super::agent::check_restricted_provider(any, u)?;
                }
            }
        }
    }
    for (prov, d) in &p.default_accounts {
        if prov.is_empty() || prov.len() > 80 {
            return Err("An account names a provider that is not allowed.".into());
        }
        if let Some(n) = &d.name {
            clean_name(n, "account")?;
        }
    }
    Ok(())
}

// ---- switching to the next account when one hits its rate limit ----

/// Is the "switch to the next account when one hits its limit" setting on? (Off unless the person turned it on.)
pub fn rotate_on_limit() -> bool {
    ui_prefs::load().rotate_on_limit
}

pub fn set_rotate_on_limit(on: bool) -> Result<(), String> {
    ui_prefs::edit(|p| p.rotate_on_limit = on).map(|_| ())
}

/// Does this error say the account is out of requests (HTTP 429, or a quota / rate limit message)?
pub fn is_limit_error(err: &str) -> bool {
    let low = err.to_lowercase();
    low.contains("answered 429")
        || low.contains("rate limit")
        || low.contains("rate_limit")
        || low.contains("quota")
        || low.contains("too many requests")
        || low.contains("resource_exhausted")
        || low.contains("insufficient_quota")
}

/// The name of the account that uses `key` (the Default one when none matches).
pub fn name_for_key(provider_id: &str, key: Option<&str>) -> String {
    let all = list(provider_id);
    all.iter().find(|a| key.is_some() && get_key(provider_id, (!a.is_default).then_some(a.id.as_str())).as_deref() == key).unwrap_or(&all[0]).name.clone()
}

/// The account to try after the one that uses `current_key`: the next one of the same provider (in the order of the list, round
/// the end) that has a key other than `current_key` and is not hidden. None when there is no other. Never another provider.
pub fn next_after(provider_id: &str, current_key: Option<&str>) -> Option<(View, String)> {
    let all = list(provider_id);
    let cur = all.iter().position(|a| current_key.is_some() && get_key(provider_id, (!a.is_default).then_some(a.id.as_str())).as_deref() == current_key).unwrap_or(0);
    for step in 1..all.len() {
        let a = &all[(cur + step) % all.len()];
        if a.hidden {
            continue;
        }
        if let Some(k) = get_key(provider_id, (!a.is_default).then_some(a.id.as_str())) {
            if Some(k.as_str()) != current_key {
                return Some((a.clone(), k));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_ids_are_per_account() {
        assert_eq!(key_id("groq", None), "groq");
        assert_eq!(key_id("groq", Some("default")), "groq");
        assert_eq!(key_id("groq", Some("groq-2")), "groq:groq-2");
        assert_eq!(key_id("custom:home", Some("work")), "custom:home:work");
    }

    #[test]
    fn limit_errors_are_recognised() {
        assert!(is_limit_error("The provider answered 429: slow down"));
        assert!(is_limit_error("You exceeded your current quota"));
        assert!(!is_limit_error("The provider answered 401: bad key"));
        assert!(!is_limit_error("Cannot reach the server"));
    }
}
