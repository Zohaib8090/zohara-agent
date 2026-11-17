//! Adding a connector from the directory: two steps. [`preview`] checks everything and tries the server (its sign-in, its keys, its tool
//! list) and keeps the result for a quarter of an hour; [`commit`] saves it. Nothing is saved without a preview, a local program is
//! never started without the person naming it, and the address always comes from the registry's own record, never from the caller.

use super::{Entry, Kind, Local, Remote, Source, CACHE};
use crate::mcp_client::{self, ServerConfig};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Mutex;

/// How long a tried server waits to be saved.
pub const PENDING_SECS: u64 = 900;
const MAX_PENDING: usize = 8;

/// What the person answered in the form.
#[derive(Clone, Debug, Default)]
pub struct Input {
    pub id: String,
    pub kind: Option<Kind>,
    /// Which of the entry's addresses (it can list several); empty: the first.
    pub url: Option<String>,
    pub name: Option<String>,
    /// Values for the entry's header questions, by header name.
    pub headers: BTreeMap<String, String>,
    /// Values for a local program's environment questions.
    pub env: BTreeMap<String, String>,
    /// A local program: the exact command that will run, typed back by the person (the form shows it).
    pub confirm: Option<String>,
}

impl Input {
    pub fn from_json(v: &Value) -> Result<Input, String> {
        let text = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        let map = |k: &str| -> Result<BTreeMap<String, String>, String> {
            let mut m = BTreeMap::new();
            if let Some(o) = v.get(k) {
                for (a, b) in o.as_object().ok_or_else(|| format!("{k} must be an object."))? {
                    m.insert(a.clone(), b.as_str().ok_or_else(|| format!("The value of {a} must be text."))?.to_string());
                }
            }
            Ok(m)
        };
        Ok(Input {
            id: text("id").unwrap_or_default(),
            kind: match v.get("kind").and_then(Value::as_str) {
                None | Some("") | Some("remote") => Some(Kind::Remote),
                Some("local") => Some(Kind::Local),
                Some(_) => return Err("The kind is \"remote\" or \"local\".".into()),
            },
            url: text("url"),
            name: text("name"),
            headers: map("headers")?,
            env: map("env")?,
            confirm: text("confirm"),
        })
    }
}

#[derive(Clone, Debug)]
pub struct Preview {
    pub id: String,
    pub name: String,
    pub kind: Kind,
    /// The address, or the command that was run.
    pub target: String,
    pub tools: Vec<String>,
    pub publisher: String,
    pub trust: String,
}

impl Preview {
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id, "name": self.name, "kind": self.kind.as_str(), "target": self.target, "tools": self.tools,
            "publisher": self.publisher, "trust": self.trust,
            "notice": "These are the tools this connector offers. Every call still asks you first in Manual and Auto mode (a tool that only reads runs on its own in Auto mode); only Bypass mode skips the question.",
        })
    }
}

struct Pending {
    tenant: String,
    cfg: ServerConfig,
    preview: Preview,
    at: u64,
}

static PENDING: Mutex<Vec<Pending>> = Mutex::new(Vec::new());

fn tenant_id() -> String {
    crate::tenant::current().map(|t| t.id).unwrap_or_default()
}

/// A connector's address is public https: the shape (no user name, no local or numeric host) and, unless `allow_private` (tests only),
/// that the host resolves to public addresses.
pub fn check_public(url: &str, allow_private: bool) -> Result<(), String> {
    if allow_private {
        return Ok(());
    }
    crate::tools::web::public_https_url(url).map_err(|_| "Only public https addresses are added from the directory.".to_string())?;
    crate::tools::web::host_resolves_public(url)
}

fn program_exists(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(name).is_file()))
}

fn clean_value(name: &str, v: &str) -> Result<String, String> {
    let v = v.trim();
    if v.len() > 4000 || v.chars().any(char::is_control) {
        return Err(format!("The value for {name} is not valid (no line breaks, 4000 characters at most)."));
    }
    Ok(v.to_string())
}

/// Options that tests (and nothing else) change.
pub struct Opts<'a> {
    pub allow_private: bool,
    /// Where a sign-in page is shown (the person's browser). On the cloud server there is none and a sign-in is not started.
    pub open: &'a dyn Fn(&str),
}

fn default_name(e: &Entry) -> String {
    let n = mcp_client::clean_name(e.id.split('/').nth(1).unwrap_or(&e.id));
    // "mcp" alone says nothing: the publisher's name goes in front
    let first = e.id.split('/').next().unwrap_or("").rsplit('.').next().unwrap_or("");
    if n.len() < 4 || n.eq_ignore_ascii_case("mcp") { mcp_client::clean_name(&format!("{first}-{n}")) } else { n }
}

fn pick_remote<'a>(e: &'a Entry, url: Option<&str>) -> Result<&'a Remote, String> {
    match url.map(str::trim).filter(|u| !u.is_empty()) {
        Some(u) => e.remotes.iter().find(|r| r.url == u).ok_or_else(|| "That address is not one the directory lists for this connector.".to_string()),
        None => e.remotes.first().ok_or_else(|| "This connector has no remote address.".to_string()),
    }
}

/// Step one: everything is checked and the server is tried. On a failure nothing is left behind (no key, no sign-in).
pub fn preview(src: &Source, now: u64, input: &Input, opts: &Opts) -> Result<Preview, String> {
    if crate::tenant::is_restricted() {
        return Err(super::RESTRICTED_MESSAGE.into());
    }
    let entry = super::lookup(src, &CACHE, now, input.id.trim()).ok_or("That connector is not in the directory right now, so it cannot be added.")?;
    preview_entry(&entry, now, input, opts)
}

/// [`preview`] for an entry the registry has just given (split off so the tests can use an entry at a local address).
pub(crate) fn preview_entry(entry: &Entry, now: u64, input: &Input, opts: &Opts) -> Result<Preview, String> {
    if crate::tenant::is_restricted() {
        return Err(super::RESTRICTED_MESSAGE.into());
    }
    let kind = input.kind.unwrap_or(Kind::Remote);
    if kind == Kind::Local && crate::tenant::multi_tenant() {
        return Err("Programs that run on a computer are added in the desktop app, not on the cloud server.".into());
    }
    let name = mcp_client::clean_name(input.name.as_deref().map(str::trim).filter(|n| !n.is_empty()).unwrap_or(&default_name(entry)));
    if name.is_empty() || name.len() > 60 {
        return Err("The name needs letters or digits (60 characters at most).".into());
    }
    if mcp_client::load_servers().iter().any(|s| s.name == name) {
        return Err(format!("A server named \"{name}\" is already added."));
    }
    let mut cfg = ServerConfig { name: name.clone(), enabled: true, registry: Some(entry.id.clone()), ..Default::default() };
    // secrets to put in the key store once everything else is checked: (id, value)
    let mut secrets: Vec<(String, String)> = Vec::new();
    match kind {
        Kind::Remote => {
            let r = pick_remote(entry, input.url.as_deref())?.clone();
            check_public(&r.url, opts.allow_private)?;
            cfg.command = r.url.clone();
            for given in input.headers.keys() {
                if !r.headers.iter().any(|h| h.name.eq_ignore_ascii_case(given)) {
                    return Err(format!("This connector does not ask for a header named {given}."));
                }
            }
            for h in &r.headers {
                if let Some(v) = &h.value {
                    cfg.headers.insert(h.name.clone(), v.clone());
                    continue;
                }
                let typed = input.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(&h.name)).map(|(_, v)| clean_value(&h.name, v)).transpose()?.filter(|v| !v.is_empty());
                match typed {
                    None if h.required => return Err(format!("{} is needed: {}", h.name, if h.description.is_empty() { "type its value" } else { &h.description })),
                    None => {}
                    Some(mut v) => {
                        // an Authorization key typed without its scheme is a bearer token
                        if h.name.eq_ignore_ascii_case("authorization") && !v.contains(' ') {
                            v = format!("Bearer {v}");
                        }
                        if h.secret {
                            secrets.push((mcp_client::header_id(&name, &h.name), v));
                            cfg.secret_headers.push(h.name.clone());
                        } else {
                            cfg.headers.insert(h.name.clone(), v);
                        }
                    }
                }
            }
        }
        Kind::Local => {
            let l: Local = entry.local.clone().ok_or("This connector has no program to run here.")?;
            if input.confirm.as_deref().map(str::trim) != Some(l.command.as_str()) {
                return Err(format!("Confirm by name first: adding this runs `{}` on this computer. It is third-party code that can do what you can do here.", l.command));
            }
            if !program_exists(l.command.split_whitespace().next().unwrap_or("")) {
                return Err(format!("This needs {} on this computer (the \"{}\" program), and it was not found.", if l.runtime == "npx" { "Node.js" } else { "Python with uv" }, l.runtime));
            }
            for given in input.env.keys() {
                if !l.env.iter().any(|e| &e.name == given) {
                    return Err(format!("This connector does not ask for {given}."));
                }
            }
            for e in &l.env {
                let v = input.env.get(&e.name).map(|v| clean_value(&e.name, v)).transpose()?.filter(|v| !v.is_empty()).or_else(|| e.default.clone());
                match v {
                    Some(v) => {
                        cfg.env.insert(e.name.clone(), v);
                    }
                    None if e.required => return Err(format!("{} is needed: {}", e.name, if e.description.is_empty() { "type its value" } else { &e.description })),
                    None => {}
                }
            }
            cfg.command = l.command.clone();
        }
    }
    for (id, v) in &secrets {
        if let Err(e) = crate::ai::keys::set(id, v) {
            mcp_client::clear_secrets(&cfg);
            return Err(e);
        }
    }
    let tried = try_server(&cfg, kind, opts);
    let tools = match tried {
        Ok(t) => t,
        Err(e) => {
            mcp_client::clear_secrets(&cfg);
            return Err(e);
        }
    };
    let p = Preview { id: entry.id.clone(), name: name.clone(), kind, target: cfg.command.clone(), tools, publisher: entry.publisher.clone(), trust: if entry.featured { super::FEATURED_LABEL } else { super::NOT_REVIEWED }.to_string() };
    let tenant = tenant_id();
    let mut all = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    // old previews (nobody saved them) are dropped with what they stored
    let (keep, gone): (Vec<_>, Vec<_>) = std::mem::take(&mut *all).into_iter().partition(|p| now.saturating_sub(p.at) < PENDING_SECS && !(p.tenant == tenant && p.cfg.name == name));
    *all = keep;
    for g in gone {
        // a try under the same name used the same key-store ids as the one that was just made: they now belong to the new one
        if !(g.tenant == tenant && g.cfg.name == name) {
            mcp_client::clear_secrets(&g.cfg);
        }
    }
    while all.iter().filter(|p| p.tenant == tenant).count() >= MAX_PENDING {
        if let Some(i) = all.iter().position(|p| p.tenant == tenant) {
            let g = all.remove(i);
            mcp_client::clear_secrets(&g.cfg);
        }
    }
    all.push(Pending { tenant, cfg, preview: p.clone(), at: now });
    Ok(p)
}

/// Tries the server: starts it, signs in when it asks (in the person's browser, where there is one), lists its tools.
fn try_server(cfg: &ServerConfig, kind: Kind, opts: &Opts) -> Result<Vec<String>, String> {
    // a key was typed into the form: a "no" from the server means the key was refused, not that a sign-in page is wanted
    if kind == Kind::Remote && !cfg.secret_headers.is_empty() {
        return mcp_client::test(cfg).map_err(|e| if e.starts_with(mcp_client::NEEDS_SIGN_IN) || e.contains("refused") { "The server refused the key you typed. Check it and try again.".to_string() } else { e });
    }
    if kind == Kind::Remote && crate::tenant::multi_tenant() {
        // the cloud server has no browser for the person: a sign-in page cannot be completed there
        return mcp_client::test(cfg).map_err(|e| if e.starts_with(mcp_client::NEEDS_SIGN_IN) { "This connector needs a sign-in in a browser, which does not work on the cloud server. Add it in the desktop app (which signs in on your own computer), or use one that takes a key.".to_string() } else { e });
    }
    mcp_client::connect(cfg, None, opts.open)
}

/// Step two: saves what [`preview`] tried (the person has seen its tools and said yes).
pub fn commit(name: &str, now: u64) -> Result<String, String> {
    if crate::tenant::is_restricted() {
        return Err(super::RESTRICTED_MESSAGE.into());
    }
    let tenant = tenant_id();
    let p = {
        let mut all = PENDING.lock().unwrap_or_else(|e| e.into_inner());
        let i = all.iter().position(|p| p.tenant == tenant && p.cfg.name == name && now.saturating_sub(p.at) < PENDING_SECS).ok_or("Nothing is waiting to be added under that name (it may have waited too long): try it again first.")?;
        all.remove(i)
    };
    let mut servers = mcp_client::load_servers();
    if servers.iter().any(|s| s.name == p.cfg.name) {
        mcp_client::clear_secrets(&p.cfg);
        return Err(format!("A server named \"{}\" is already added.", p.cfg.name));
    }
    servers.push(p.cfg.clone());
    mcp_client::save_servers(&servers);
    Ok(format!("Added \"{}\". Its tools ({}) are yours from the next message on.", p.cfg.name, p.preview.tools.join(", ")))
}

/// The person said no (or closed the window): what the preview stored is removed.
pub fn cancel(name: &str) {
    let tenant = tenant_id();
    let mut all = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(i) = all.iter().position(|p| p.tenant == tenant && p.cfg.name == name) {
        let g = all.remove(i);
        mcp_client::clear_secrets(&g.cfg);
    }
}
