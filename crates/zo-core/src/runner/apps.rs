//! Copying connected apps (MCP servers) to a VM (docs/VM-MIGRATION-PLAN.md section 4). The copy goes from this computer to the VM's
//! runner over SSH, never through the cloud server.
//!
//! * Without credentials only the definition goes: the name, the address or command, the names of the variables and headers it needs,
//!   with empty values.
//! * With credentials (API keys, header values, a stored sign-in, a client secret) the person must confirm a sentence that names the
//!   machine and the apps (`confirmation`); the request is refused if it does not match. The files on the VM are mode 0600, the
//!   audit log records which apps went where and never a value, and "Revoke" deletes them there.

use super::host::{self, NOT_INSTALLED};
use crate::mcp_client::{self, ServerConfig};
use crate::store::schedules::now_millis;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct CopiedServer {
    pub name: String,
    pub credentials: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
struct Copied {
    servers: Vec<CopiedServer>,
    at: u64,
}

fn file() -> std::path::PathBuf {
    crate::audit::config_dir().join("vm-apps.json")
}

fn load() -> BTreeMap<String, Copied> {
    std::fs::read_to_string(file()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn save(m: &BTreeMap<String, Copied>) {
    let _ = std::fs::create_dir_all(crate::audit::config_dir());
    let tmp = file().with_extension("json.tmp");
    if let Ok(s) = serde_json::to_string_pretty(m) {
        if std::fs::write(&tmp, s).is_ok() {
            let _ = std::fs::rename(&tmp, file());
        }
    }
}

/// What was copied to a machine (names only), or null.
pub fn copied(machine: &str) -> Value {
    load().get(machine).map(|c| json!({"at": c.at, "servers": c.servers})).unwrap_or(Value::Null)
}

pub fn forget(machine: &str) {
    let mut m = load();
    if m.remove(machine).is_some() {
        save(&m);
    }
}

/// The apps that can be copied: switched on, and not waiting for the person's say-so here.
fn candidates() -> Vec<ServerConfig> {
    mcp_client::load_servers().into_iter().filter(|s| s.enabled && !s.held).collect()
}

/// The labels (never values) of what an app needs to sign in.
fn credential_labels(s: &ServerConfig) -> Vec<String> {
    let mut v: Vec<String> = s.env.keys().map(|k| format!("variable {k}")).collect();
    v.extend(s.headers.keys().map(|k| format!("header {k}")));
    v.extend(s.secret_headers.iter().map(|k| format!("header {k}")));
    if crate::ai::keys::get(&mcp_client::token_id(&s.name)).is_some() {
        v.push("a saved sign-in".into());
    }
    if s.client_secret.as_deref().is_some_and(|c| !c.is_empty()) {
        v.push("a client secret".into());
    }
    v
}

fn runtime_of(s: &ServerConfig) -> String {
    if mcp_client::is_url(&s.command) {
        return String::new();
    }
    mcp_client::split_command(&s.command).into_iter().next().unwrap_or_default()
}

/// What can be copied, for the dialog: no values, only names.
pub fn plan(machine: &str) -> Result<Value, String> {
    host::status_exists(machine)?;
    let done = load().get(machine).cloned().unwrap_or_default();
    let servers: Vec<Value> = candidates()
        .iter()
        .map(|s| {
            let labels = credential_labels(s);
            let oauth = crate::ai::keys::get(&mcp_client::token_id(&s.name)).is_some();
            let program = runtime_of(s);
            let local_path = program.starts_with('/') || program.starts_with("~/");
            let mut warn = Vec::new();
            if oauth {
                warn.push(format!("Signed in with a login. A copy of the login on {machine} is a second live copy: \"Revoke on {machine}\" deletes it there, and revoking at the provider ends both."));
            }
            if local_path {
                warn.push("It starts a program by a path on this computer; check that the same path exists on the machine.".to_string());
            }
            let c = done.servers.iter().find(|c| c.name == s.name);
            json!({
                "name": s.name, "kind": if mcp_client::is_url(&s.command) { "remote" } else { "local" },
                "target": if mcp_client::is_url(&s.command) { s.command.clone() } else { program.clone() },
                "credentials": labels, "oauth": oauth, "runtime": program, "warnings": warn,
                "copied": c.is_some(), "copied_with_credentials": c.is_some_and(|c| c.credentials),
            })
        })
        .collect();
    Ok(json!({"machine": machine, "has_runner": host::has_runner(machine), "servers": servers, "copied": copied(machine)}))
}

fn names_list(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [a] => a.clone(),
        [a @ .., z] => format!("{} and {z}", a.join(", ")),
    }
}

/// The sentence a person agrees to before credentials are copied: it names the apps and the machine.
pub fn confirmation(machine: &str, apps: &[String]) -> String {
    let mut v = apps.to_vec();
    v.sort();
    format!("This copies the {} credentials to {machine}.", names_list(&v))
}

/// The definition of an app as it is sent: with the credentials, or with only their names.
fn payload_for(s: &ServerConfig, with_credentials: bool, keys: &mut serde_json::Map<String, Value>) -> ServerConfig {
    let mut c = s.clone();
    c.held = false;
    c.registry = None;
    c.address_changed = None;
    c.updated = 0;
    if with_credentials {
        if let Some(t) = crate::ai::keys::get(&mcp_client::token_id(&s.name)) {
            keys.insert(mcp_client::token_id(&s.name), json!(t));
        }
        for h in &s.secret_headers {
            if let Some(v) = crate::ai::keys::get(&mcp_client::header_id(&s.name, h)) {
                keys.insert(mcp_client::header_id(&s.name, h), json!(v));
            }
        }
    } else {
        for v in c.env.values_mut() {
            v.clear();
        }
        c.headers.clear();
        c.secret_headers.clear();
        c.client_secret = None;
    }
    c
}

fn send(machine: &str, servers: &[(ServerConfig, bool)], replace: bool) -> Result<usize, String> {
    let mut keys = serde_json::Map::new();
    let list: Vec<ServerConfig> = servers.iter().map(|(s, creds)| payload_for(s, *creds, &mut keys)).collect();
    let body = json!({"servers": list, "keys": keys, "replace": replace});
    drop(keys);
    let r = host::call(machine, &["apps-set"], Some(body.to_string().as_bytes()), 60)?;
    Ok(r["servers"].as_u64().unwrap_or(0) as usize)
}

/// Copies the chosen apps. `picks` is `(name, with credentials)`. When any pick has credentials, `confirm` must be `confirmation(...)`
/// for exactly those apps and this machine.
pub fn copy(machine: &str, picks: &[(String, bool)], confirm: &str) -> Result<Value, String> {
    host::status_exists(machine)?;
    if !host::has_runner(machine) {
        return Err(format!("{NOT_INSTALLED} Install it first: the copied apps live in its own settings."));
    }
    if picks.is_empty() {
        return Err("Pick at least one app.".into());
    }
    let all = candidates();
    let mut chosen: Vec<(ServerConfig, bool)> = Vec::new();
    for (name, creds) in picks {
        let s = all.iter().find(|s| &s.name == name).ok_or_else(|| format!("There is no app called \"{name}\" that can be copied."))?;
        // asking for credentials of an app that has none is not a credential copy
        let has = !credential_labels(s).is_empty();
        chosen.push((s.clone(), *creds && has));
    }
    let with: Vec<String> = chosen.iter().filter(|(_, c)| *c).map(|(s, _)| s.name.clone()).collect();
    if !with.is_empty() {
        let want = confirmation(machine, &with);
        if confirm != want {
            return Err(format!("Confirm exactly: {want}"));
        }
    }
    let n = send(machine, &chosen, false)?;
    let mut reg = load();
    let e = reg.entry(machine.to_string()).or_default();
    for (s, c) in &chosen {
        e.servers.retain(|x| x.name != s.name);
        e.servers.push(CopiedServer { name: s.name.clone(), credentials: *c });
    }
    e.servers.sort_by(|a, b| a.name.cmp(&b.name));
    e.at = now_millis();
    save(&reg);
    // which apps went where, never a value
    crate::audit::log("runner_copy_apps", &json!({"machine": machine, "apps": chosen.iter().map(|(s, _)| s.name.clone()).collect::<Vec<_>>(), "credentials": with}), "ok");
    Ok(json!({"copied": chosen.len(), "on_machine": n, "credentials": with.len()}))
}

/// Deletes the copied apps and their credentials on the machine.
pub fn revoke(machine: &str) -> Result<(), String> {
    host::status_exists(machine)?;
    match host::call(machine, &["apps-clear"], None, 60) {
        Ok(_) => {}
        Err(e) if e == NOT_INSTALLED => {} // nothing there to remove
        Err(e) => return Err(e),
    }
    forget(machine);
    crate::audit::log("runner_revoke_apps", &json!({"machine": machine}), "ok");
    Ok(())
}

/// Do the programs the copied apps start exist on the machine? (Nothing is installed: the answer says what is missing.)
pub fn check(machine: &str) -> Result<Value, String> {
    host::status_exists(machine)?;
    let r = host::call(machine, &["apps-check"], None, 60)?;
    let list: Vec<Value> = r["servers"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|mut s| {
            let present = s["present"].as_bool().unwrap_or(false);
            let rt = s["runtime"].as_str().unwrap_or("").to_string();
            s["hint"] = json!(if present { String::new() } else { missing_hint(&rt) });
            s
        })
        .collect();
    Ok(json!({"servers": list}))
}

fn missing_hint(program: &str) -> String {
    let base = program.rsplit('/').next().unwrap_or(program);
    match base {
        "npx" | "node" | "npm" => "Node.js is not installed on the machine. Install it there (for example: sudo apt install nodejs npm) and check again.".into(),
        "uvx" | "uv" => "uv is not installed on the machine. Install it there (see docs.astral.sh/uv) and check again.".into(),
        "python" | "python3" | "pipx" => "Python is not installed on the machine, or not under that name.".into(),
        "" => "The app has no program to start.".into(),
        other => format!("{other} is not on the machine."),
    }
}

/// Brings the machine's copy in step with this computer: changed apps are sent again, removed ones are removed. Apps that were copied
/// with credentials need the confirmation again, because their credentials travel again.
pub fn sync(machine: &str, confirm: &str) -> Result<Value, String> {
    host::status_exists(machine)?;
    let done = load().get(machine).cloned().ok_or("Nothing was copied to this machine yet.")?;
    let all = candidates();
    let mut chosen: Vec<(ServerConfig, bool)> = Vec::new();
    let mut removed = Vec::new();
    for c in &done.servers {
        match all.iter().find(|s| s.name == c.name) {
            Some(s) => chosen.push((s.clone(), c.credentials && !credential_labels(s).is_empty())),
            None => removed.push(c.name.clone()),
        }
    }
    let with: Vec<String> = chosen.iter().filter(|(_, c)| *c).map(|(s, _)| s.name.clone()).collect();
    if !with.is_empty() {
        let want = confirmation(machine, &with);
        if confirm != want {
            return Err(format!("Confirm exactly: {want}"));
        }
    }
    send(machine, &chosen, true)?;
    let mut reg = load();
    if let Some(e) = reg.get_mut(machine) {
        e.servers = chosen.iter().map(|(s, c)| CopiedServer { name: s.name.clone(), credentials: *c }).collect();
        e.at = now_millis();
    }
    save(&reg);
    crate::audit::log("runner_sync_apps", &json!({"machine": machine, "apps": chosen.iter().map(|(s, _)| s.name.clone()).collect::<Vec<_>>(), "credentials": with, "removed": removed}), "ok");
    Ok(json!({"synced": chosen.len(), "removed": removed, "credentials": with.len()}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_confirmation_names_the_apps_and_the_machine() {
        assert_eq!(confirmation("vm-1", &["Notion".into(), "GitHub".into()]), "This copies the GitHub and Notion credentials to vm-1.");
        assert_eq!(confirmation("vm-1", &["Notion".into()]), "This copies the Notion credentials to vm-1.");
        assert_eq!(confirmation("vm-1", &["c".into(), "a".into(), "b".into()]), "This copies the a, b and c credentials to vm-1.");
    }

    #[test]
    fn without_credentials_no_value_leaves() {
        let mut s = ServerConfig { name: "n".into(), command: "npx -y server".into(), enabled: true, ..Default::default() };
        s.env.insert("API_KEY".into(), "sk-secret".into());
        s.headers.insert("Authorization".into(), "Bearer secret".into());
        s.secret_headers.push("X-Key".into());
        s.client_secret = Some("cs".into());
        let mut keys = serde_json::Map::new();
        let c = payload_for(&s, false, &mut keys);
        let text = serde_json::to_string(&c).unwrap();
        assert!(!text.contains("secret") && !text.contains("cs\""), "{text}");
        assert_eq!(c.env.get("API_KEY").map(String::as_str), Some(""), "the name stays, the value does not");
        assert!(keys.is_empty());
    }

    #[test]
    fn missing_runtimes_are_explained() {
        assert!(missing_hint("npx").contains("Node.js"));
        assert!(missing_hint("/usr/bin/uvx").contains("uv"));
        assert!(missing_hint("foo").contains("foo"));
    }
}
