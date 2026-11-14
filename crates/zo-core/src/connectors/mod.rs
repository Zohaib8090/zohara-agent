//! The connectors directory (docs/CONNECTORS-DIRECTORY-PLAN.md, docs/MCP.md "Browse connectors"): remote MCP servers (and, in a second
//! list, local npm/pypi ones) to add with one click, read from the official MCP registry.
//!
//! * **Source**: `GET <registry>/v0/servers?limit=&version=latest&search=&cursor=` and `GET <registry>/v0/servers/<id>/versions/latest`,
//!   plain requests with no key and nothing about the person. The Zohara Agent server makes them and keeps the answers for an hour
//!   (stale answers are served when the registry is down); the clients ask the server (`GET /api/connectors`), so the registry never
//!   sees a person's address or search. A desktop that is not signed in asks the registry itself, with the same code and no identifier.
//! * **Kept**: status `active` and `isLatest`, and at least one `streamable-http` remote at a public `https://` address (shape only here;
//!   the host is looked up again before anything is added), or a stdio npm/pypi package with literal arguments. Everything a registry
//!   entry says is third-party text: it is cleaned and cut short, and no picture is loaded from it (an icon would tell its publisher who
//!   is browsing), so the clients draw a letter tile.
//! * **Featured**: a short hand-made list ([`FEATURED`]), shown first. Each one is asked for by its exact id at every refresh and shown only
//!   when the registry answers for it; the others say "Not reviewed by Zohara Agent".
//! * **Adding** is in [`add`]: try it (with the person's answers to the header and key questions), show what it can do, then save.
//! * **Address changed**: [`check_changes`] asks the registry about the connectors that were added from it; one whose address is no
//!   longer listed is held until the person decides.

pub mod add;

use crate::mcp_client;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::Read;
use std::sync::Mutex;
use std::time::Duration;

/// The official registry (no key, public).
pub const OFFICIAL: &str = "https://registry.modelcontextprotocol.io";
const MAX_BODY: u64 = 3_000_000;
/// How long an answer is kept, and how long a failed request is not repeated.
pub const CACHE_SECS: u64 = 3600;
pub const RETRY_SECS: u64 = 300;
/// Requests to the registry in a minute, for the whole process (a crowd of people cannot make the registry limit us).
const UPSTREAM_PER_MINUTE: usize = 40;
const MAX_CACHED: usize = 160;
/// Entries asked for per registry page, pages read for one answer, and how many kept entries are enough for one answer.
const UPSTREAM_PAGE: usize = 50;
const MAX_PAGES: usize = 4;
const WANT: usize = 24;

pub const NOT_REVIEWED: &str = "Not reviewed by Zohara Agent";
pub const FEATURED_LABEL: &str = "Featured by Zohara Agent";
pub const RESTRICTED_MESSAGE: &str = "This account cannot add connectors. Accounts on the cloud server get web search, page reading and memory only; connectors (MCP servers) can run programs and reach your accounts, so they are for the app on your own computer. You can still look at the list here.";

pub const CATEGORIES: [&str; 8] = ["dev", "docs", "chat", "data", "finance", "media", "search", "other"];

/// The featured connectors and their category. Every id was found in the live registry (one plain request each, 2026-11-14) and is asked
/// for again at every refresh: one the registry does not answer for is not shown. Slack, Asana, Intercom and Spotify were not in the
/// registry under their own names that day, so they are not here.
pub const FEATURED: [(&str, &str); 12] = [
    ("com.notion/mcp", "docs"),
    ("io.github.github/github-mcp-server", "dev"),
    ("app.linear/linear", "dev"),
    ("com.atlassian/atlassian-mcp-server", "dev"),
    ("io.github.getsentry/sentry-mcp", "dev"),
    ("com.stripe/mcp", "finance"),
    ("com.paypal.mcp/mcp", "finance"),
    ("com.cloudflare.mcp/mcp", "dev"),
    ("com.figma.mcp/mcp", "media"),
    ("com.zapier/mcp", "other"),
    ("com.vercel/vercel-mcp", "dev"),
    ("com.supabase/mcp", "data"),
];

// ---------------------------------------------------------------------------------------------------------------------------
// entries

#[derive(Clone, Debug, PartialEq)]
pub struct Header {
    pub name: String,
    pub description: String,
    pub secret: bool,
    pub required: bool,
    /// A fixed value the registry gives (public, so not asked for).
    pub value: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Remote {
    pub url: String,
    pub headers: Vec<Header>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EnvVar {
    pub name: String,
    pub description: String,
    pub secret: bool,
    pub required: bool,
    pub default: Option<String>,
}

/// A server that runs on this computer: `npx -y <package>@<version>` or `uvx <package>@<version>`.
#[derive(Clone, Debug, PartialEq)]
pub struct Local {
    pub runtime: String,
    pub package: String,
    pub version: Option<String>,
    pub command: String,
    pub env: Vec<EnvVar>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub id: String,
    pub title: String,
    pub description: String,
    pub version: String,
    pub website: Option<String>,
    pub publisher: String,
    pub category: String,
    pub featured: bool,
    pub remotes: Vec<Remote>,
    pub local: Option<Local>,
}

/// Third-party text made safe to show: no control characters, one space between words, cut at `max` characters.
pub fn clean_text(s: &str, max: usize) -> String {
    let t: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ");
    if t.chars().count() <= max {
        return t;
    }
    let mut cut: String = t.chars().take(max.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

/// A registry id: `reverse.dns.name/server`.
pub fn valid_id(id: &str) -> bool {
    let Some((ns, name)) = id.split_once('/') else { return false };
    id.len() <= 200 && !ns.is_empty() && !name.is_empty() && !name.contains('/') && ns.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-') && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// Where the id comes from, as a domain: `com.notion/mcp` is `notion.com`, `io.github.user/x` is `github.com/user`.
pub fn publisher_of(id: &str) -> String {
    let ns = id.split('/').next().unwrap_or("");
    if let Some(user) = ns.strip_prefix("io.github.").or_else(|| ns.strip_prefix("com.github.")) {
        return format!("github.com/{user}");
    }
    ns.split('.').rev().collect::<Vec<_>>().join(".")
}

fn https_url(u: &str, max: usize) -> Option<String> {
    let u = u.trim();
    if u.len() > max || u.contains('{') || crate::tools::web::public_https_url(u).is_err() {
        return None;
    }
    Some(u.to_string())
}

/// The first category whose words appear in the text (finance, chat, media, docs, search, data, dev), else `other`.
pub fn category_of(text: &str) -> &'static str {
    let t = text.to_lowercase();
    let words: Vec<&str> = t.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let has = |keys: &[&str]| keys.iter().any(|k| words.iter().any(|w| w == k || (k.len() > 4 && w.starts_with(k))));
    if has(&["payment", "payments", "stripe", "paypal", "finance", "financial", "bank", "banking", "invoice", "invoices", "crypto", "trading", "stock", "stocks", "accounting", "billing", "wallet"]) {
        "finance"
    } else if has(&["chat", "slack", "message", "messages", "messaging", "email", "mail", "discord", "telegram", "whatsapp", "sms", "calendar"]) {
        "chat"
    } else if has(&["music", "video", "image", "images", "audio", "spotify", "youtube", "photo", "photos", "design", "figma", "podcast", "game", "games", "speech", "voice", "tts"]) {
        "media"
    } else if has(&["docs", "documentation", "notes", "wiki", "document", "documents", "notion", "pdf", "knowledge", "markdown", "obsidian"]) {
        "docs"
    } else if has(&["search", "web", "crawl", "scrape", "scraping", "research", "browser", "news", "serp"]) {
        "search"
    } else if has(&["database", "databases", "sql", "analytics", "data", "postgres", "postgresql", "mysql", "warehouse", "csv", "spreadsheet", "dataset", "datasets"]) {
        "data"
    } else if has(&["github", "git", "code", "developer", "developers", "api", "sdk", "repo", "repository", "deploy", "devops", "cloud", "build", "issue", "issues", "ticket", "project", "tasks", "task", "ci"]) {
        "dev"
    } else {
        "other"
    }
}

fn header_of(v: &Value) -> Option<Header> {
    let name = v.get("name")?.as_str()?.trim();
    // a header name is a token: no spaces, no colon
    if name.is_empty() || name.len() > 80 || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
        return None;
    }
    let value = v.get("value").and_then(Value::as_str).map(str::to_string);
    Some(Header {
        name: name.to_string(),
        description: clean_text(v.get("description").and_then(Value::as_str).unwrap_or(""), 200),
        // a header that carries a credential is always treated as a secret (kept in the key store, never shown again), whatever the registry says
        secret: v.get("isSecret").and_then(Value::as_bool).unwrap_or(false) || { let n = name.to_ascii_lowercase(); n == "authorization" || n.contains("key") || n.contains("token") || n.contains("secret") },
        required: v.get("isRequired").and_then(Value::as_bool).unwrap_or(false),
        value,
    })
}

fn remote_of(v: &Value) -> Option<Remote> {
    if v.get("type").and_then(Value::as_str) != Some("streamable-http") {
        return None; // the old HTTP+SSE transport is not supported (docs/MCP.md)
    }
    let url = https_url(v.get("url")?.as_str()?, 500)?;
    let mut headers = Vec::new();
    for h in v.get("headers").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]) {
        // a header that needs a variable filled in (a value with `{...}`) is more than the form can ask: the entry is skipped
        if h.get("value").and_then(Value::as_str).is_some_and(|x| x.contains('{')) || h.get("variables").is_some() {
            return None;
        }
        headers.push(header_of(h)?);
    }
    if headers.len() > 8 || v.get("variables").is_some() {
        return None;
    }
    Some(Remote { url, headers })
}

fn arg_ok(a: &str) -> bool {
    !a.is_empty() && a.len() <= 200 && a.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'/' | b'=' | b'@' | b'+' | b',' | b'~' | b'-'))
}

fn local_of(v: &Value) -> Option<Local> {
    let kind = v.get("registryType").and_then(Value::as_str)?;
    let (runtime, program) = match kind {
        "npm" => ("npx", "npx"),
        "pypi" => ("uvx", "uvx"),
        _ => return None, // docker images, .mcpb bundles, cargo and nuget crates are not started from here
    };
    if v.pointer("/transport/type").and_then(Value::as_str) != Some("stdio") || v.get("runtimeArguments").is_some_and(|a| a.as_array().is_none_or(|a| !a.is_empty())) {
        return None;
    }
    let package = v.get("identifier")?.as_str()?.trim();
    let name_ok = if kind == "npm" {
        let bare = package.strip_prefix('@').and_then(|r| r.split_once('/').map(|(a, b)| (a, b))).map(|(a, b)| (a.to_string(), b.to_string()));
        let part = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-'));
        match bare {
            Some((scope, name)) => part(&scope) && part(&name),
            None => part(package),
        }
    } else {
        !package.is_empty() && package.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    };
    if !name_ok || package.len() > 120 {
        return None;
    }
    let version = v.get("version").and_then(Value::as_str).map(str::trim).filter(|x| !x.is_empty() && x.len() <= 40 && *x != "latest" && x.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'+' | b'-' | b'_'))).map(str::to_string);
    let mut args: Vec<String> = Vec::new();
    for a in v.get("packageArguments").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]) {
        let value = a.get("value").and_then(Value::as_str);
        match (value, a.get("isRequired").and_then(Value::as_bool).unwrap_or(false)) {
            (Some(x), _) if x.contains('{') => return None,
            (Some(x), _) => {
                if a.get("type").and_then(Value::as_str) == Some("named") {
                    let n = a.get("name").and_then(Value::as_str)?;
                    if !arg_ok(n) {
                        return None;
                    }
                    args.push(n.to_string());
                }
                if !arg_ok(x) {
                    return None;
                }
                args.push(x.to_string());
            }
            (None, true) => return None, // wants an answer the form does not ask
            (None, false) => {}
        }
    }
    let spec = match &version {
        Some(ver) => format!("{package}@{ver}"),
        None => package.to_string(),
    };
    let mut words: Vec<String> = Vec::new();
    if runtime == "npx" {
        words.push("-y".into());
    }
    words.push(spec);
    words.extend(args);
    let mut env = Vec::new();
    for e in v.get("environmentVariables").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]) {
        let name = e.get("name")?.as_str()?.trim();
        if name.is_empty() || name.len() > 80 || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            return None;
        }
        // `{placeholders}` inside a value are not filled in here: such a variable is simply not offered
        let default = e.get("default").and_then(Value::as_str).filter(|d| !d.contains('{') && d.len() <= 300 && !d.chars().any(char::is_control)).map(str::to_string);
        env.push(EnvVar { name: name.to_string(), description: clean_text(e.get("description").and_then(Value::as_str).unwrap_or(""), 200), secret: e.get("isSecret").and_then(Value::as_bool).unwrap_or(false), required: e.get("isRequired").and_then(Value::as_bool).unwrap_or(false), default });
    }
    if env.len() > 16 {
        return None;
    }
    Some(Local { runtime: runtime.into(), package: package.to_string(), version, command: mcp_client::join_command(program, &words), env })
}

impl Entry {
    /// One `{server, _meta}` record of the registry. None when it is not to be shown: not active, not the latest version, no
    /// usable remote and no usable local package, or an id that is not an id.
    pub fn parse(rec: &Value) -> Option<Entry> {
        let meta = rec.pointer("/_meta/io.modelcontextprotocol.registry~1official")?;
        if meta.get("status").and_then(Value::as_str) != Some("active") || meta.get("isLatest").and_then(Value::as_bool) != Some(true) {
            return None;
        }
        let s = rec.get("server")?;
        let id = s.get("name")?.as_str()?.trim().to_string();
        if !valid_id(&id) {
            return None;
        }
        let mut remotes: Vec<Remote> = Vec::new();
        for r in s.get("remotes").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]) {
            if let Some(r) = remote_of(r) {
                if !remotes.iter().any(|x| x.url == r.url) {
                    remotes.push(r);
                }
            }
        }
        remotes.truncate(6);
        let local = s.get("packages").and_then(Value::as_array).and_then(|p| p.iter().find_map(local_of));
        if remotes.is_empty() && local.is_none() {
            return None;
        }
        // the part after the slash; "mcp" (or another name too short to say anything) is replaced by the publisher's own name
        let mut short = id.split('/').nth(1).unwrap_or(&id).to_string();
        if short.len() < 4 || short.eq_ignore_ascii_case("mcp") {
            // the organisation in the reverse-DNS id: com.notion -> notion, com.paypal.mcp -> paypal, io.github.user -> user
            let labels: Vec<&str> = id.split('/').next().unwrap_or("").split('.').collect();
            let word = if labels.len() >= 3 && labels[0] == "io" && labels[1] == "github" { labels[2] } else { labels.get(1).copied().unwrap_or(labels[0]) }.to_string();
            let mut cs = word.chars();
            short = cs.next().map(|c| c.to_uppercase().collect::<String>() + cs.as_str()).unwrap_or(short);
        }
        let title = Some(clean_text(s.get("title").and_then(Value::as_str).unwrap_or(""), 80)).filter(|t| !t.is_empty()).unwrap_or(short);
        let description = clean_text(s.get("description").and_then(Value::as_str).unwrap_or(""), 400);
        let category = category_of(&format!("{id} {title} {description}")).to_string();
        Some(Entry {
            publisher: publisher_of(&id),
            version: clean_text(s.get("version").and_then(Value::as_str).unwrap_or(""), 40),
            website: s.get("websiteUrl").and_then(Value::as_str).and_then(|u| https_url(u, 300)),
            title,
            description,
            category,
            featured: false,
            remotes,
            local,
            id,
        })
    }

    /// The JSON clients get. `kind` is the list this entry is shown in ("remote" or "local").
    pub fn to_json(&self, kind: Kind) -> Value {
        let header = |h: &Header| {
            let mut j = json!({"name": h.name, "description": h.description, "secret": h.secret, "required": h.required});
            if let Some(v) = &h.value {
                j["value"] = json!(v);
            }
            j
        };
        json!({
            "id": self.id,
            "title": self.title,
            "description": self.description,
            "version": self.version,
            "website": self.website,
            "publisher": self.publisher,
            "category": self.category,
            "featured": self.featured,
            "trust": if self.featured { FEATURED_LABEL } else { NOT_REVIEWED },
            "kind": kind.as_str(),
            "remotes": self.remotes.iter().map(|r| json!({"url": r.url, "headers": r.headers.iter().map(header).collect::<Vec<_>>()})).collect::<Vec<_>>(),
            "local": self.local.as_ref().map(|l| json!({
                "runtime": l.runtime, "package": l.package, "version": l.version, "command": l.command,
                "needs": if l.runtime == "npx" { "Node.js" } else { "Python (uv)" },
                "env": l.env.iter().map(|e| json!({"name": e.name, "description": e.description, "secret": e.secret, "required": e.required, "default": e.default})).collect::<Vec<_>>(),
            })),
        })
    }
}

// ---------------------------------------------------------------------------------------------------------------------------
// reading a registry page

pub struct Page {
    pub entries: Vec<Entry>,
    pub next: Option<String>,
    /// Records that were left out (old versions, deprecated, nothing usable).
    pub skipped: usize,
}

/// A page of `GET /v0/servers`. Entries seen twice (the same id) are kept once.
pub fn parse_page(v: &Value) -> Result<Page, String> {
    let list = v.get("servers").and_then(Value::as_array).ok_or("The directory answered, but not in the form expected.")?;
    let mut entries: Vec<Entry> = Vec::new();
    let mut skipped = 0;
    for rec in list {
        match Entry::parse(rec) {
            Some(e) if !entries.iter().any(|x| x.id == e.id) => entries.push(e),
            _ => skipped += 1,
        }
    }
    let next = v.pointer("/metadata/nextCursor").and_then(Value::as_str).filter(|c| cursor_ok(c)).map(str::to_string);
    Ok(Page { entries, next, skipped })
}

fn cursor_ok(c: &str) -> bool {
    !c.is_empty() && c.len() <= 300 && c.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'/' | b'@' | b'+' | b'-' | b'=' | b'~'))
}

// ---------------------------------------------------------------------------------------------------------------------------
// asking the registry, and the cache

#[derive(Clone, Debug)]
pub struct Source {
    pub base: String,
}

impl Source {
    pub fn official() -> Source {
        Source { base: OFFICIAL.into() }
    }

    pub fn at(base: &str) -> Source {
        Source { base: base.trim().trim_end_matches('/').to_string() }
    }

    /// The official registry, or `ZO_CONNECTORS_REGISTRY_URL` (an http(s) address: a mirror, and the way tests use a fake registry).
    pub fn from_env() -> Source {
        match std::env::var("ZO_CONNECTORS_REGISTRY_URL").ok().map(|b| b.trim().to_string()).filter(|b| b.starts_with("http://") || b.starts_with("https://")) {
            Some(b) => Source::at(&b),
            None => Source::official(),
        }
    }
}

/// Percent-encoding of one query value or path part.
pub fn pct(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn fetch(url: &str) -> Result<Value, String> {
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(15)).redirects(0).user_agent("Zohara-Agent-Connectors").build();
    match agent.get(url).set("Accept", "application/json").call() {
        Ok(r) => {
            if !r.content_type().to_ascii_lowercase().contains("json") {
                return Err("The directory did not answer with JSON.".into());
            }
            let mut body = String::new();
            r.into_reader().take(MAX_BODY + 1).read_to_string(&mut body).map_err(|_| "The directory's answer could not be read.".to_string())?;
            if body.len() as u64 > MAX_BODY {
                return Err("The directory's answer is too big.".into());
            }
            serde_json::from_str(&body).map_err(|_| "The directory's answer is not JSON.".to_string())
        }
        Err(ureq::Error::Status(404, _)) => Err("not found".into()),
        Err(ureq::Error::Status(code, _)) => Err(format!("The directory answered {code}.")),
        Err(_) => Err("The directory could not be reached.".into()),
    }
}

struct Inner {
    good: BTreeMap<String, (Value, u64)>,
    failed: BTreeMap<String, (u64, String)>,
    /// When the last requests to the registry were made (seconds).
    sent: Vec<u64>,
}

/// One hour of memory for what the registry said, per request address. A request that failed is not repeated for five minutes; the last
/// good answer is served instead (marked stale). At most [`UPSTREAM_PER_MINUTE`] requests a minute are made, whoever asks.
pub struct Cache {
    inner: Mutex<Inner>,
}

pub struct Got {
    pub value: Result<Value, String>,
    pub stale: bool,
    pub checked_at: Option<u64>,
}

impl Cache {
    pub const fn new() -> Cache {
        Cache { inner: Mutex::new(Inner { good: BTreeMap::new(), failed: BTreeMap::new(), sent: Vec::new() }) }
    }

    pub fn get(&self, key: &str, now: u64, fetch: impl FnOnce() -> Result<Value, String>) -> Got {
        {
            let mut s = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((v, at)) = s.good.get(key) {
                if now.saturating_sub(*at) < CACHE_SECS {
                    return Got { value: Ok(v.clone()), stale: false, checked_at: Some(*at) };
                }
            }
            let stale = s.good.get(key).cloned();
            let refuse = if let Some((at, e)) = s.failed.get(key).filter(|(at, _)| now.saturating_sub(*at) < RETRY_SECS) {
                let _ = at;
                Some(e.clone())
            } else {
                s.sent.retain(|t| now.saturating_sub(*t) < 60);
                (s.sent.len() >= UPSTREAM_PER_MINUTE).then(|| "The directory is being asked too often right now. Try again in a minute.".to_string())
            };
            if let Some(e) = refuse {
                return match stale {
                    Some((v, at)) => Got { value: Ok(v), stale: true, checked_at: Some(at) },
                    None => Got { value: Err(e), stale: false, checked_at: None },
                };
            }
            s.sent.push(now);
        }
        // the registry is asked without the lock: a slow answer does not hold up the other requests
        let res = fetch();
        let mut s = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        match res {
            Ok(v) => {
                s.failed.remove(key);
                if s.good.len() >= MAX_CACHED && !s.good.contains_key(key) {
                    if let Some(oldest) = s.good.iter().min_by_key(|(_, (_, at))| *at).map(|(k, _)| k.clone()) {
                        s.good.remove(&oldest);
                    }
                }
                s.good.insert(key.to_string(), (v.clone(), now));
                Got { value: Ok(v), stale: false, checked_at: Some(now) }
            }
            Err(e) => {
                if s.failed.len() >= MAX_CACHED {
                    s.failed.clear();
                }
                s.failed.insert(key.to_string(), (now, e.clone()));
                match s.good.get(key) {
                    Some((v, at)) => Got { value: Ok(v.clone()), stale: true, checked_at: Some(*at) },
                    None => Got { value: Err(e), stale: false, checked_at: None },
                }
            }
        }
    }
}

impl Default for Cache {
    fn default() -> Self {
        Cache::new()
    }
}

/// The cache of this process (the server's, or the desktop's own).
pub static CACHE: Cache = Cache::new();

pub fn now_secs() -> u64 {
    crate::update::now_secs()
}

// ---------------------------------------------------------------------------------------------------------------------------
// what a client asks

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Remote,
    Local,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Remote => "remote",
            Kind::Local => "local",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Query {
    pub q: String,
    pub cursor: Option<String>,
    pub kind: Kind,
    pub category: Option<String>,
}

impl Query {
    /// Checks what a client sent (text from a web address or a request line): nothing is passed on to the registry unchecked.
    pub fn parse(q: Option<&str>, cursor: Option<&str>, kind: Option<&str>, category: Option<&str>) -> Result<Query, String> {
        let q = q.unwrap_or("").trim().to_string();
        if q.chars().count() > 80 || q.chars().any(char::is_control) {
            return Err("The search text is too long (80 characters at most).".into());
        }
        let cursor = cursor.map(str::trim).filter(|c| !c.is_empty()).map(str::to_string);
        if cursor.as_deref().is_some_and(|c| !cursor_ok(c)) {
            return Err("That page marker is not valid.".into());
        }
        let kind = match kind.map(str::trim).unwrap_or("") {
            "" | "remote" => Kind::Remote,
            "local" => Kind::Local,
            _ => return Err("The kind is \"remote\" or \"local\".".into()),
        };
        let category = match category.map(str::trim).unwrap_or("") {
            "" | "all" => None,
            c if CATEGORIES.contains(&c) => Some(c.to_string()),
            _ => return Err("That category is not known.".into()),
        };
        Ok(Query { q, cursor, kind, category })
    }

    /// From the query part of a request address (`q=a%20b&cursor=...`).
    pub fn from_query_string(qs: &str) -> Result<Query, String> {
        let mut m: BTreeMap<String, String> = BTreeMap::new();
        for pair in qs.split('&').filter(|p| !p.is_empty()) {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            m.insert(unpct(k), unpct(v));
        }
        Query::parse(m.get("q").map(String::as_str), m.get("cursor").map(String::as_str), m.get("kind").map(String::as_str), m.get("category").map(String::as_str))
    }

    pub fn to_query_string(&self) -> String {
        let mut p = vec![format!("kind={}", self.kind.as_str())];
        if !self.q.is_empty() {
            p.push(format!("q={}", pct(&self.q)));
        }
        if let Some(c) = &self.cursor {
            p.push(format!("cursor={}", pct(c)));
        }
        if let Some(c) = &self.category {
            p.push(format!("category={}", pct(c)));
        }
        p.join("&")
    }
}

pub fn unpct(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() && b.get(i + 1).zip(b.get(i + 2)).is_some_and(|(h, l)| h.is_ascii_hexdigit() && l.is_ascii_hexdigit()) => {
                out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap_or(b'?'));
                i += 2;
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

pub struct Browse {
    pub entries: Vec<Entry>,
    pub featured: Vec<Entry>,
    pub next: Option<String>,
    pub stale: bool,
    pub checked_at: Option<u64>,
    /// Why the answer is old, when it is.
    pub error: Option<String>,
}

impl Browse {
    pub fn to_json(&self, kind: Kind, source: &str) -> Value {
        json!({
            "ok": true,
            "restricted": false,
            "source": source,
            "entries": self.entries.iter().map(|e| e.to_json(kind)).collect::<Vec<_>>(),
            "featured": self.featured.iter().map(|e| e.to_json(kind)).collect::<Vec<_>>(),
            "next": self.next,
            "stale": self.stale,
            "checked_at": self.checked_at,
            "error": self.error,
        })
    }
}

/// What a restricted account is told instead of a list.
pub fn restricted_json() -> Value {
    json!({"ok": true, "restricted": true, "message": RESTRICTED_MESSAGE, "entries": [], "featured": [], "next": null, "stale": false, "checked_at": null, "error": null, "source": "none"})
}

fn page_url(src: &Source, q: &Query, cursor: Option<&str>) -> String {
    let mut u = format!("{}/v0/servers?limit={UPSTREAM_PAGE}&version=latest", src.base);
    if !q.q.is_empty() {
        u.push_str(&format!("&search={}", pct(&q.q)));
    }
    if let Some(c) = cursor {
        u.push_str(&format!("&cursor={}", pct(c)));
    }
    u
}

fn matches_text(e: &Entry, q: &str) -> bool {
    let q = q.to_lowercase();
    q.is_empty() || e.id.to_lowercase().contains(&q) || e.title.to_lowercase().contains(&q) || e.description.to_lowercase().contains(&q)
}

fn shown(e: &Entry, q: &Query) -> bool {
    let has = match q.kind {
        Kind::Remote => !e.remotes.is_empty(),
        Kind::Local => e.local.is_some(),
    };
    has && q.category.as_deref().is_none_or(|c| e.category == c)
}

/// One connector by its exact id, as the registry lists it now (None: not there, not active, nothing usable in it, or the registry
/// answered for another id). Featured ones get their mark and category.
pub fn lookup(src: &Source, cache: &Cache, now: u64, id: &str) -> Option<Entry> {
    if !valid_id(id) {
        return None;
    }
    let url = format!("{}/v0/servers/{}/versions/latest", src.base, pct(id));
    let got = cache.get(&url, now, || fetch(&url));
    let mut e = Entry::parse(&got.value.ok()?)?;
    if e.id != id {
        return None;
    }
    if let Some((_, cat)) = FEATURED.iter().find(|(f, _)| *f == id) {
        e.featured = true;
        e.category = (*cat).to_string();
    }
    Some(e)
}

/// The featured connectors the registry answers for now, in the list's order.
pub fn featured(src: &Source, cache: &Cache, now: u64) -> Vec<Entry> {
    std::thread::scope(|s| {
        let jobs: Vec<_> = FEATURED.iter().map(|(id, _)| s.spawn(move || lookup(src, cache, now, id))).collect();
        jobs.into_iter().filter_map(|j| j.join().ok().flatten()).collect()
    })
}

/// One answer for a client: the featured list (first page of an unfiltered remote list only) and the entries that pass the filter.
pub fn browse(src: &Source, cache: &Cache, now: u64, q: &Query) -> Result<Browse, String> {
    let mut out = Browse { entries: Vec::new(), featured: Vec::new(), next: None, stale: false, checked_at: None, error: None };
    let mut cursor = q.cursor.clone();
    for n in 0..MAX_PAGES {
        let url = page_url(src, q, cursor.as_deref());
        let got = cache.get(&url, now, || fetch(&url));
        let value = match got.value {
            Ok(v) => v,
            Err(e) if n == 0 => return Err(if e.starts_with("The directory") { e } else { format!("The directory could not be read ({e}).") }),
            Err(e) => {
                out.error = Some(e);
                break;
            }
        };
        out.stale |= got.stale;
        out.checked_at = match (out.checked_at, got.checked_at) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        let page = parse_page(&value)?;
        for e in page.entries {
            if shown(&e, q) && !out.entries.iter().any(|x| x.id == e.id) {
                out.entries.push(e);
            }
        }
        cursor = page.next;
        if cursor.is_none() || out.entries.len() >= WANT {
            break;
        }
    }
    out.next = cursor;
    if q.cursor.is_none() && q.kind == Kind::Remote {
        out.featured = featured(src, cache, now).into_iter().filter(|e| matches_text(e, &q.q) && q.category.as_deref().is_none_or(|c| e.category == c)).collect();
        out.entries.retain(|e| !out.featured.iter().any(|f| f.id == e.id));
        // a registry entry of a featured id that is not the featured one (it was asked for again above) is not marked
    }
    if out.stale && out.error.is_none() {
        out.error = Some("The directory could not be reached just now; this is the list from earlier.".into());
    }
    Ok(out)
}

/// The list for this process: the official registry (or the mirror from the environment), through [`CACHE`].
pub fn browse_here(q: &Query) -> Result<Value, String> {
    if crate::tenant::is_restricted() {
        return Ok(restricted_json());
    }
    browse(&Source::from_env(), &CACHE, now_secs(), q).map(|b| b.to_json(q.kind, "registry"))
}

/// The list for a client program: through the Zohara Agent server when the person is signed in on this computer (the registry then
/// never sees this computer), else the registry itself with a plain request. On the server itself, the registry.
pub fn browse_for_client(q: &Query) -> Result<Value, String> {
    if crate::tenant::multi_tenant() {
        return browse_here(q);
    }
    let Some(cred) = crate::login::credential_of(crate::login::Client::Desktop).filter(|_| crate::login::signed_in_quick().is_some()) else { return browse_here(q) };
    let base = crate::login::server_url()?;
    let url = format!("{}/api/connectors?{}", base.trim_end_matches('/'), q.to_query_string());
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(30)).redirects(0).user_agent(&format!("zohara-agent/{}", env!("CARGO_PKG_VERSION"))).build();
    match agent.get(&url).set("Authorization", &format!("Bearer {cred}")).set("Accept", "application/json").call() {
        Ok(r) => {
            let mut body = String::new();
            r.into_reader().take(MAX_BODY).read_to_string(&mut body).map_err(|_| "The server's answer could not be read.".to_string())?;
            let mut v: Value = serde_json::from_str(&body).map_err(|_| "The server's answer is not JSON.".to_string())?;
            v["source"] = json!("server");
            Ok(v)
        }
        Err(ureq::Error::Status(code, r)) => {
            let msg = r.into_string().ok().and_then(|b| serde_json::from_str::<Value>(&b).ok()).and_then(|v| v.get("error").and_then(Value::as_str).map(str::to_string));
            Err(msg.unwrap_or_else(|| format!("The server answered {code}.")))
        }
        Err(_) => Err("The Zohara Agent server could not be reached, so the list could not be loaded.".into()),
    }
}

// ---------------------------------------------------------------------------------------------------------------------------
// address changes

#[derive(Clone, Debug, PartialEq)]
pub struct Changed {
    pub name: String,
    pub old: String,
    pub new: String,
}

fn same_address(a: &str, b: &str) -> bool {
    a.trim().trim_end_matches('/').eq_ignore_ascii_case(b.trim().trim_end_matches('/'))
}

/// Asks the registry about every connector that was added from it. One whose saved address is no longer among the addresses the
/// registry lists is held (it is not started and gets no header or token) and remembers the new address, until the person decides
/// ([`accept_change`], or the switch in the list, which keeps the old address). A registry that cannot be reached changes nothing.
pub fn check_changes(src: &Source, cache: &Cache, now: u64) -> Vec<Changed> {
    let mut all = mcp_client::load_servers();
    let mut found = Vec::new();
    let mut dirty = false;
    for s in all.iter_mut().filter(|s| s.registry.is_some() && mcp_client::is_url(&s.command)) {
        let Some(e) = lookup(src, cache, now, s.registry.as_deref().unwrap_or("")) else { continue };
        if e.remotes.is_empty() || e.remotes.iter().any(|r| same_address(&r.url, &s.command)) {
            if s.address_changed.is_some() && e.remotes.iter().any(|r| same_address(&r.url, &s.command)) {
                s.address_changed = None; // the registry lists the saved address again
                dirty = true;
            }
            continue;
        }
        let new = e.remotes[0].url.clone();
        if s.address_changed.as_deref() != Some(&new) || !s.held {
            s.address_changed = Some(new.clone());
            s.held = true;
            dirty = true;
        }
        found.push(Changed { name: s.name.clone(), old: s.command.clone(), new });
    }
    if dirty {
        mcp_client::save_servers(&all);
    }
    found
}

/// The person agrees to the new address of a connector (the "Address changed" button). The address is checked like a new one; what was
/// saved for the old address (sign-in, header values, a registered client) is forgotten, because it must not go to the new one: the
/// person signs in or types the keys again. The server is switched on again.
pub fn accept_change(name: &str) -> Result<String, String> {
    accept_change_with(name, false)
}

pub(crate) fn accept_change_with(name: &str, allow_private: bool) -> Result<String, String> {
    if crate::tenant::is_restricted() {
        return Err(RESTRICTED_MESSAGE.into());
    }
    let mut all = mcp_client::load_servers();
    let s = all.iter_mut().find(|s| s.name == name && s.registry.is_some()).ok_or("There is no connector with that name.")?;
    let new = s.address_changed.clone().ok_or("The address of this connector has not changed.")?;
    add::check_public(&new, allow_private)?;
    let old_cfg = s.clone();
    mcp_client::clear_secrets(&old_cfg);
    s.command = new.clone();
    s.address_changed = None;
    s.held = false;
    s.enabled = true;
    s.headers.clear();
    s.secret_headers.clear();
    s.client_id = None;
    s.client_secret = None;
    mcp_client::save_servers(&all);
    Ok(format!("\"{name}\" now uses {new}. Its sign-in and keys were forgotten: sign in again, or add the key again if it needs one."))
}

#[cfg(test)]
mod tests;
