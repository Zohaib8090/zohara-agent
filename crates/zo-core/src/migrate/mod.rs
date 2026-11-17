//! Migrate: bring the MCP servers and instructions a person already set up in other tools (Claude Code, Claude Desktop, Codex CLI,
//! Gemini CLI, Cursor, VS Code, Zed) into Zohara Agent. docs/VM-MIGRATION-PLAN.md section 3 is the specification; docs/MIGRATE.md says
//! what was built.
//!
//! Two steps, like the connectors directory:
//! * [`scan`] reads the known files (this computer, a machine over SSH, or text that was pasted) and says what it found. It is read-only
//!   and everything it reports is safe to show: no secret value is ever in the answer, only the NAME of what is needed.
//! * [`apply`] reads the same source again (so a secret never travels to a screen and back) and imports what the person ticked:
//!   servers are saved but not started, instruction files become memory notes or are appended to a project's instruction, nothing
//!   that exists is overwritten, and every import can be undone ([`undo`]).
//!
//! Secrets (values in `env` blocks, headers, arguments or addresses) are copied only for a server the person ticked "include secrets"
//! for AND confirmed by name; without that the server comes in with an empty placeholder. Values go to the key store (not the settings
//! file), never to a log, and never over the sync.

pub mod parse;
pub mod source;
#[cfg(test)]
mod tests;

use crate::mcp_client::{self, ServerConfig};
use crate::store::{chats, memory, memory_log, projects};
use parse::Raw;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use source::Reader;
use std::collections::{BTreeMap, HashMap, HashSet};

/// The tools that have detectors: (id, name).
pub const TOOLS: &[(&str, &str)] = &[("claude-code", "Claude Code CLI"), ("claude-desktop", "Claude Desktop"), ("codex", "Codex CLI"), ("gemini", "Gemini CLI"), ("cursor", "Cursor"), ("vscode", "VS Code"), ("zed", "Zed")];

pub fn tool_name(id: &str) -> &'static str {
    TOOLS.iter().find(|(i, _)| *i == id).map(|(_, n)| *n).unwrap_or("Pasted file")
}

/// What a file holds.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    ClaudeJson,
    Settings,
    ServersJson,
    Toml,
    Instruction,
}

/// The files of each tool, relative to the home folder (the same paths are tried on every system: Linux, macOS and Windows).
const FILES: &[(&str, &str, Kind)] = &[
    ("claude-code", ".claude.json", Kind::ClaudeJson),
    ("claude-code", ".claude/settings.json", Kind::Settings),
    ("claude-code", ".claude/CLAUDE.md", Kind::Instruction),
    ("claude-desktop", ".config/Claude/claude_desktop_config.json", Kind::ServersJson),
    ("claude-desktop", "Library/Application Support/Claude/claude_desktop_config.json", Kind::ServersJson),
    ("claude-desktop", "AppData/Roaming/Claude/claude_desktop_config.json", Kind::ServersJson),
    ("codex", ".codex/config.toml", Kind::Toml),
    ("codex", ".codex/AGENTS.md", Kind::Instruction),
    ("gemini", ".gemini/settings.json", Kind::Settings),
    ("gemini", ".gemini/GEMINI.md", Kind::Instruction),
    ("cursor", ".cursor/mcp.json", Kind::ServersJson),
    ("vscode", ".config/Code/User/mcp.json", Kind::ServersJson),
    ("vscode", "Library/Application Support/Code/User/mcp.json", Kind::ServersJson),
    ("vscode", ".config/Code/User/settings.json", Kind::ServersJson),
    ("zed", ".config/zed/settings.json", Kind::ServersJson),
    ("zed", "AppData/Roaming/Zed/settings.json", Kind::ServersJson),
];

/// Folders whose entry NAMES are listed (custom commands and skills of Claude Code); nothing in them is read.
const DIRS: &[(&str, &str, &str)] = &[("claude-code", ".claude/commands", "command"), ("claude-code", ".claude/skills", "skill")];

/// Files looked for in each project folder Claude Code knows: (tool, name, kind).
const PROJECT_FILES: &[(&str, &str, Kind)] = &[
    ("claude-code", ".mcp.json", Kind::ServersJson),
    ("claude-code", "CLAUDE.md", Kind::Instruction),
    ("codex", "AGENTS.md", Kind::Instruction),
    ("gemini", "GEMINI.md", Kind::Instruction),
    ("cursor", ".cursor/mcp.json", Kind::ServersJson),
    ("vscode", ".vscode/mcp.json", Kind::ServersJson),
];

const MAX_PROJECTS: usize = 40;
pub const MAX_SERVERS: usize = 300;
pub const MAX_NOTE_CHARS: usize = 380;
pub const MAX_NOTES_PER_FILE: usize = 60;
/// Most characters appended to a project's instruction from one file.
pub const MAX_INSTRUCTION_CHARS: usize = 20_000;
/// What a server gets in place of a secret that was not copied.
pub const PLACEHOLDER: &str = "YOUR_VALUE_HERE";

// ---------------------------------------------------------------------------------------------------------------------------
// what is a secret

fn words(name: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut prev_lower = false;
    for c in name.chars() {
        if !c.is_ascii_alphanumeric() {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            prev_lower = false;
            continue;
        }
        // camelCase boundary: apiKey -> api, key
        if c.is_ascii_uppercase() && prev_lower && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        prev_lower = c.is_ascii_lowercase();
        cur.push(c.to_ascii_lowercase());
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Does a variable, header or parameter NAME say it holds a secret?
pub fn secret_name(name: &str) -> bool {
    const EXACT: &[&str] = &["key", "keys", "token", "tokens", "secret", "secrets", "password", "passwd", "pwd", "pass", "auth", "authorization", "credential", "credentials", "cookie", "bearer", "pat", "dsn", "session", "signature", "sig", "jwt", "apikey", "passphrase"];
    words(name).iter().any(|w| EXACT.contains(&w.as_str()) || ["key", "token", "secret", "password"].iter().any(|s| w.len() > s.len() && w.ends_with(s)))
}

/// Does one word look like a key (a well-known prefix, or a long random-looking string)?
pub fn secret_word(w: &str) -> bool {
    let t = w.trim_matches(|c: char| !c.is_alphanumeric() && c != '-' && c != '_' && c != '.');
    const PREFIXES: &[&str] = &["sk-", "sk_", "ghp_", "gho_", "ghu_", "ghs_", "github_pat_", "glpat-", "xoxb-", "xoxp-", "xoxa-", "xapp-", "AKIA", "AIza", "ya29.", "eyJ", "npm_", "pat-", "shpat_", "rk_live", "pk_live", "sk_live"];
    if t.len() >= 16 && PREFIXES.iter().any(|p| t.starts_with(p)) {
        return true;
    }
    crate::ai::memory_agent::looks_secret(t)
}

/// A line of text that looks like it holds a secret (a key-like word, or "password is ...").
pub fn secret_line(l: &str) -> bool {
    // also the pieces inside punctuation: one-line JSON (`{"k":"ghp_..."}`) has no spaces around its values
    crate::ai::memory_agent::looks_secret(l) || l.split_whitespace().any(secret_word) || l.split(|c: char| !(c.is_alphanumeric() || matches!(c, '-' | '_' | '.'))).any(secret_word)
}

/// A value that is only a reference to an environment variable or an input (`${GITHUB_TOKEN}`, `${env:X}`, `${input:key}`, `$TOKEN`).
pub fn is_reference(v: &str) -> bool {
    let t = v.trim();
    // an optional scheme word (`Bearer `), then nothing but the reference
    let t = match t.split_once(' ') {
        Some((w, rest)) if matches!(w.to_ascii_lowercase().as_str(), "bearer" | "basic" | "token") => rest.trim_start(),
        _ => t,
    };
    if let Some(inner) = t.strip_prefix("${").and_then(|r| r.strip_suffix('}')) {
        return !inner.is_empty() && inner.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '-' | '.'));
    }
    t.starts_with('$') && t.len() > 1 && t[1..].chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// A value that is clearly not a secret (a flag, a number, a plain word, a path, a plain address, a media type).
fn harmless(v: &str) -> bool {
    let t = v.trim();
    if t.is_empty() {
        return true;
    }
    if t.len() > 200 || secret_word(t) {
        return false;
    }
    if matches!(t, "true" | "false" | "null") || t.parse::<f64>().is_ok() {
        return true;
    }
    if t.len() <= 24 && t.chars().next().is_some_and(|c| c.is_ascii_lowercase()) && t.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_') {
        return true;
    }
    if t.starts_with('/') || t.starts_with("~/") {
        return !t.contains(char::is_whitespace);
    }
    if let Some((a, b)) = t.split_once('/') {
        if !a.is_empty() && a.chars().all(|c| c.is_ascii_lowercase()) && b.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '+' | '-')) {
            return true;
        }
    }
    (t.starts_with("http://") || t.starts_with("https://")) && !t.contains('@') && !t.contains('?') && !t.split('/').skip(3).any(secret_word)
}

fn env_is_secret(name: &str, value: &str) -> bool {
    !is_reference(value) && (secret_name(name) || !harmless(value))
}

// ---------------------------------------------------------------------------------------------------------------------------
// one server, prepared for showing or for saving

/// Something a server needs from the person that was not (or will not be) copied.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Need {
    /// "env", "header", "arg" or "address"
    pub kind: &'static str,
    pub name: String,
    /// What the file says it is read from, when it only names an environment variable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// A value is in the file (it can be copied when the person asks); false when the file only names a variable.
    pub in_file: bool,
}

/// A server as it would be saved.
#[derive(Clone, Debug, Default)]
pub struct Prepared {
    pub command: String,
    /// Plain environment values (and empty placeholders for the secret ones that were not copied).
    pub env: BTreeMap<String, String>,
    pub headers: BTreeMap<String, String>,
    /// Secret values that go to the key store (only when copying secrets was asked for).
    pub secret_env: BTreeMap<String, String>,
    pub secret_headers: BTreeMap<String, String>,
    pub needs: Vec<Need>,
    pub notes: Vec<String>,
    /// The command as shown: secret arguments and address parts hidden.
    pub shown: String,
}

/// The pieces of an address: (scheme and host part, path segments, query pairs, fragment).
fn split_url(u: &str) -> (String, Vec<String>, Vec<(String, String)>, String) {
    let (rest, frag) = u.split_once('#').map(|(a, b)| (a, b.to_string())).unwrap_or((u, String::new()));
    let (rest, query) = rest.split_once('?').map(|(a, b)| (a, Some(b))).unwrap_or((rest, None));
    let (scheme, after) = rest.split_once("://").map(|(a, b)| (format!("{a}://"), b)).unwrap_or((String::new(), rest));
    let (auth, path) = after.split_once('/').map(|(a, b)| (a.to_string(), b)).unwrap_or((after.to_string(), ""));
    let segs: Vec<String> = if path.is_empty() { Vec::new() } else { path.split('/').map(str::to_string).collect() };
    let q = query.map(|q| q.split('&').filter(|p| !p.is_empty()).map(|p| p.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())).unwrap_or((p.to_string(), String::new()))).collect()).unwrap_or_default();
    (format!("{scheme}{auth}"), segs, q, frag)
}

/// An address with its secret parts hidden (`shown`), or replaced by a placeholder (`import`). Returns (shown, import, names of the secret parts).
fn url_parts(u: &str) -> (String, String, Vec<String>) {
    let (head, segs, query, frag) = split_url(u);
    let mut needs = Vec::new();
    let (mut head_shown, mut head_imp) = (head.clone(), head.clone());
    if let Some((scheme, rest)) = head.split_once("://") {
        if let Some((_, host)) = rest.rsplit_once('@') {
            head_shown = format!("{scheme}://•••@{host}");
            head_imp = format!("{scheme}://{host}");
            needs.push("user name and password".to_string());
        }
    }
    let (mut s_shown, mut s_imp) = (Vec::new(), Vec::new());
    for (i, s) in segs.iter().enumerate() {
        // matrix parameters (`/mcp;key=abc`)
        if let Some((name, params)) = s.split_once(';') {
            let (mut shown, mut imp) = (name.to_string(), name.to_string());
            for kv in params.split(';').filter(|p| !p.is_empty()) {
                let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
                if secret_name(k) || secret_word(v) || secret_word(kv) {
                    needs.push(k.to_string());
                    shown.push_str(&format!(";{k}=•••"));
                    imp.push_str(&format!(";{k}={PLACEHOLDER}"));
                } else {
                    shown.push_str(&format!(";{kv}"));
                    imp.push_str(&format!(";{kv}"));
                }
            }
            s_shown.push(shown);
            s_imp.push(imp);
            continue;
        }
        if secret_word(s) {
            needs.push(format!("part {} of the path", i + 1));
            s_shown.push("•••".to_string());
            s_imp.push(PLACEHOLDER.to_string());
        } else {
            s_shown.push(s.clone());
            s_imp.push(s.clone());
        }
    }
    let (mut q_shown, mut q_imp) = (Vec::new(), Vec::new());
    for (k, v) in &query {
        if secret_name(k) || secret_word(v) {
            needs.push(k.clone());
            q_shown.push(format!("{k}=•••"));
            q_imp.push(format!("{k}={PLACEHOLDER}"));
        } else {
            q_shown.push(format!("{k}={v}"));
            q_imp.push(format!("{k}={v}"));
        }
    }
    // the fragment: `#access_token=abc` (or a bare token) is a secret like a query value
    let (mut f_shown, mut f_imp) = (frag.clone(), frag.clone());
    if !frag.is_empty() {
        let pairs: Vec<(&str, &str)> = frag.split('&').map(|p| p.split_once('=').unwrap_or((p, ""))).collect();
        if pairs.iter().any(|(k, v)| secret_name(k) || secret_word(v) || secret_word(k)) {
            let (mut sh, mut im) = (Vec::new(), Vec::new());
            for (k, v) in &pairs {
                if secret_name(k) || secret_word(v) || secret_word(k) {
                    needs.push(if v.is_empty() { "fragment".to_string() } else { k.to_string() });
                    sh.push(if v.is_empty() { "•••".to_string() } else { format!("{k}=•••") });
                    im.push(if v.is_empty() { PLACEHOLDER.to_string() } else { format!("{k}={PLACEHOLDER}") });
                } else {
                    sh.push(format!("{k}={v}"));
                    im.push(format!("{k}={v}"));
                }
            }
            f_shown = sh.join("&");
            f_imp = im.join("&");
        }
    }
    let build = |head: &str, segs: &[String], q: &[String], frag: &str| {
        let mut o = head.to_string();
        if !segs.is_empty() {
            o.push('/');
            o.push_str(&segs.join("/"));
        }
        if !q.is_empty() {
            o.push('?');
            o.push_str(&q.join("&"));
        }
        if !frag.is_empty() {
            o.push('#');
            o.push_str(&frag);
        }
        o
    };
    (build(&head_shown, &s_shown, &q_shown, &f_shown), build(&head_imp, &s_imp, &q_imp, &f_imp), needs)
}

/// Marks which arguments of a program are secret (the value after `--api-key`, `--token=abc`, `KEY=abc`, a key-like word, a bearer header).
fn secret_args(args: &[String]) -> Vec<Option<String>> {
    let mut out: Vec<Option<String>> = vec![None; args.len()];
    let mut pending: Option<String> = None;
    for (i, a) in args.iter().enumerate() {
        let low = a.to_ascii_lowercase();
        if a.starts_with('-') {
            pending = None;
            if let Some((flag, val)) = a.split_once('=') {
                if secret_name(flag) || secret_word(val) {
                    out[i] = Some(flag.trim_start_matches('-').to_string());
                }
            } else if secret_name(a) {
                pending = Some(a.trim_start_matches('-').to_string());
            }
            continue;
        }
        if let Some(name) = pending.take() {
            out[i] = Some(name);
            continue;
        }
        if low.contains("bearer ") || low.starts_with("authorization:") || low.contains("api-key:") || low.contains("api_key:") || low.contains("token:") {
            out[i] = Some("header".into());
            continue;
        }
        // a connection string with a user name and password in it (postgresql://app:pw@host/db)
        if let Some((_, rest)) = a.split_once("://") {
            if rest.split('/').next().is_some_and(|auth| auth.contains('@')) {
                out[i] = Some("connection string".into());
                continue;
            }
        }
        if let Some((k, v)) = a.split_once('=') {
            if !k.contains(char::is_whitespace) && (secret_name(k) || secret_word(v)) && !is_reference(v) {
                out[i] = Some(k.to_string());
                continue;
            }
        }
        if secret_word(a) {
            out[i] = Some(format!("argument {}", i + 1));
        }
    }
    out
}

/// Prepares a server for showing (`copy` false: nothing secret in the result) or for saving (`copy` true: secret values included, to
/// be put in the key store by the caller).
pub fn prepare(raw: &Raw, copy: bool) -> Prepared {
    let mut p = Prepared { notes: raw.notes.clone(), ..Default::default() };
    if !raw.url.is_empty() {
        let (shown, import, names) = url_parts(&raw.url);
        for n in &names {
            p.needs.push(Need { kind: "address", name: n.clone(), from: None, in_file: true });
        }
        p.shown = shown;
        p.command = if copy { raw.url.trim().to_string() } else { import };
    } else {
        let marks = secret_args(&raw.args);
        let mut real = Vec::new();
        let mut hidden = Vec::new();
        let mut safe = Vec::new();
        for (a, m) in raw.args.iter().zip(&marks) {
            real.push(a.clone());
            match m {
                None => {
                    hidden.push(a.clone());
                    safe.push(a.clone());
                }
                Some(name) => {
                    p.needs.push(Need { kind: "arg", name: name.clone(), from: None, in_file: true });
                    let cut = |a: &str, v: &str| match a.split_once('=') {
                        Some((k, _)) => format!("{k}={v}"),
                        None => v.to_string(),
                    };
                    // `--api-key abc`: the flag stays, the next word is the value (already marked); `--api-key=abc` is one word
                    let wholly_value = !a.starts_with('-') && !a.contains('=') || a.to_ascii_lowercase().contains("bearer ") || a.to_ascii_lowercase().starts_with("authorization:");
                    hidden.push(if wholly_value { "•••".into() } else { cut(a, "•••") });
                    safe.push(if wholly_value { PLACEHOLDER.into() } else { cut(a, PLACEHOLDER) });
                }
            }
        }
        p.shown = mcp_client::join_command(&raw.program, &hidden);
        p.command = if copy { mcp_client::join_command(&raw.program, &real) } else { mcp_client::join_command(&raw.program, &safe) };
    }
    for (k, v) in &raw.env {
        if is_reference(v) {
            p.needs.push(Need { kind: "env", name: k.clone(), from: Some(v.trim().to_string()), in_file: false });
            p.env.insert(k.clone(), String::new());
        } else if env_is_secret(k, v) {
            p.needs.push(Need { kind: "env", name: k.clone(), from: None, in_file: true });
            if copy && !v.is_empty() {
                p.secret_env.insert(k.clone(), v.clone());
            } else {
                p.env.insert(k.clone(), String::new());
            }
        } else {
            p.env.insert(k.clone(), v.clone());
        }
    }
    for (k, v) in &raw.headers {
        if is_reference(v) {
            p.needs.push(Need { kind: "header", name: k.clone(), from: Some(v.trim().to_string()), in_file: false });
        } else if secret_name(k) || !harmless(v) {
            p.needs.push(Need { kind: "header", name: k.clone(), from: None, in_file: true });
            if copy && !v.is_empty() {
                p.secret_headers.insert(k.clone(), v.clone());
            }
        } else {
            p.headers.insert(k.clone(), v.clone());
        }
    }
    for (h, var) in &raw.header_from_env {
        p.needs.push(Need { kind: "header", name: h.clone(), from: Some(var.clone()), in_file: false });
    }
    if !p.needs.is_empty() && !copy {
        p.notes.push("Secrets were not copied: add them in Settings after the import.".into());
    }
    p
}

// ---------------------------------------------------------------------------------------------------------------------------
// the scan

#[derive(Clone, Debug)]
pub struct Server {
    pub id: String,
    pub tool: &'static str,
    pub source: String,
    /// "user", "project" (a folder's own file), or "local" (kept by Claude Code for one folder)
    pub scope: String,
    pub folder: String,
    pub raw: Raw,
}

#[derive(Clone, Debug)]
pub struct Instruction {
    pub id: String,
    pub tool: &'static str,
    pub source: String,
    /// "" for the tool's own (global) file, else the project folder
    pub folder: String,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct Extra {
    pub id: String,
    pub tool: &'static str,
    /// "command", "skill" or "model"
    pub kind: &'static str,
    pub name: String,
    pub note: String,
}

#[derive(Clone, Debug, Default)]
pub struct Scan {
    pub origin: String,
    pub kind: String,
    /// (tool id, files that were found, shown)
    pub found: Vec<(&'static str, Vec<String>)>,
    pub servers: Vec<Server>,
    pub instructions: Vec<Instruction>,
    pub extras: Vec<Extra>,
    pub notes: Vec<String>,
}

fn hash_id(prefix: &str, parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p.as_bytes());
        h.update([0]);
    }
    format!("{prefix}{}", h.finalize().iter().take(5).map(|b| format!("{b:02x}")).collect::<String>())
}

fn tool_static(id: &str) -> &'static str {
    TOOLS.iter().find(|(i, _)| *i == id).map(|(i, _)| *i).unwrap_or("pasted")
}

/// Reads one file's text into what it holds.
fn read_file(scan: &mut Scan, tool: &'static str, shown: &str, kind: Kind, text: &str, folder: &str, projects_out: &mut Vec<String>) {
    let add_servers = |scan: &mut Scan, list: Vec<Raw>, scope: &str, folder: &str| {
        for mut raw in list {
            // an address that is not http(s) is not a remote server (and must never be run as a program)
            if !raw.url.is_empty() && !mcp_client::is_url(&raw.url) {
                scan.notes.push(format!("{shown}: server \"{}\" has an address that is not http(s); it was left out.", raw.name.chars().take(60).collect::<String>()));
                continue;
            }
            if tool == "gemini" && raw.plain_url && !raw.url.is_empty() {
                raw.notes.push("Gemini CLI's `url` is an older SSE address; it may not connect here.".into());
            }
            let id = hash_id("s", &[tool, shown, scope, folder, &raw.name]);
            if scan.servers.iter().any(|s| s.id == id) {
                continue;
            }
            scan.servers.push(Server { id, tool, source: shown.to_string(), scope: scope.to_string(), folder: folder.to_string(), raw });
        }
    };
    match kind {
        Kind::Instruction => {
            if text.trim().is_empty() {
                return;
            }
            let id = hash_id("i", &[tool, shown, folder]);
            scan.instructions.push(Instruction { id, tool, source: shown.to_string(), folder: folder.to_string(), text: text.to_string() });
        }
        Kind::Toml => match parse::servers_toml(text) {
            Ok((list, model)) => {
                add_servers(scan, list, if folder.is_empty() { "user" } else { "project" }, folder);
                if let Some(m) = model {
                    scan.extras.push(Extra { id: hash_id("x", &[tool, shown, "model"]), tool, kind: "model", name: m, note: "Choose the model in Settings > Model.".into() });
                }
            }
            Err(e) => scan.notes.push(format!("{shown}: {e}")),
        },
        Kind::ClaudeJson | Kind::Settings | Kind::ServersJson => match parse::jsonc(text) {
            Ok(doc) => {
                if kind == Kind::ClaudeJson {
                    let top = parse::servers_json(&json!({"mcpServers": doc.get("mcpServers").cloned().unwrap_or(Value::Null)}));
                    add_servers(scan, top, "user", "");
                    let (folders, locals) = parse::claude_projects(&doc);
                    projects_out.extend(folders);
                    for (f, list) in locals {
                        add_servers(scan, list, "local", &f);
                    }
                } else {
                    let scope = if folder.is_empty() { "user" } else { "project" };
                    // a settings.json also names the model
                    add_servers(scan, parse::servers_json(&doc), scope, folder);
                    if kind == Kind::Settings || tool == "gemini" {
                        if let Some(m) = parse::model_json(&doc) {
                            scan.extras.push(Extra { id: hash_id("x", &[tool, shown, "model"]), tool, kind: "model", name: m, note: "Choose the model in Settings > Model.".into() });
                        }
                    }
                }
            }
            Err(e) => scan.notes.push(format!("{shown}: {e}")),
        },
    }
}

fn is_clean_folder(p: &str) -> bool {
    p.starts_with('/') && p.len() <= 400 && !p.contains("..") && !p.chars().any(char::is_control)
}

/// Looks for the files of every tool in `reader` and says what is there.
pub fn scan(reader: &dyn Reader, kind: &str) -> Result<Scan, String> {
    let mut sc = Scan { origin: reader.label(), kind: kind.into(), ..Default::default() };
    let files: Vec<String> = FILES.iter().map(|f| f.1.to_string()).collect();
    let dirs: Vec<String> = DIRS.iter().map(|d| d.1.to_string()).collect();
    let got = reader.fetch(&files, &dirs)?;
    for f in &got.too_big {
        sc.notes.push(format!("{}: bigger than 8 MB, so it was not read.", reader.shown(f)));
    }
    let mut found: BTreeMap<&'static str, Vec<String>> = BTreeMap::new();
    let mut folders: Vec<String> = Vec::new();
    for (tool, path, k) in FILES {
        let Some(text) = got.files.get(*path) else { continue };
        let shown = reader.shown(path);
        found.entry(tool_static(tool)).or_default().push(shown.clone());
        read_file(&mut sc, tool_static(tool), &shown, *k, text, "", &mut folders);
    }
    for (tool, path, what) in DIRS {
        let Some(names) = got.dirs.get(*path) else { continue };
        let shown = reader.shown(path);
        found.entry(tool_static(tool)).or_default().push(shown.clone());
        for n in names.iter().filter(|n| !n.starts_with('.')).take(parse_cap()) {
            let name = n.strip_suffix(".md").unwrap_or(n).to_string();
            let (kindname, note) = if *what == "command" { ("command", "Zohara Agent has no custom commands; listed for reference.") } else { ("skill", "Skills are not imported; listed for reference.") };
            sc.extras.push(Extra { id: hash_id("x", &[tool, path, n]), tool: tool_static(tool), kind: kindname, name: name.chars().take(60).collect(), note: note.into() });
        }
    }
    // the folders of the projects Claude Code knows: their own server file and instruction files
    folders.retain(|f| is_clean_folder(f));
    folders.sort();
    folders.dedup();
    if folders.len() > MAX_PROJECTS {
        sc.notes.push(format!("Claude Code knows {} project folders; the first {MAX_PROJECTS} were looked at.", folders.len()));
        folders.truncate(MAX_PROJECTS);
    }
    if !folders.is_empty() {
        let pf: Vec<String> = folders.iter().flat_map(|f| PROJECT_FILES.iter().map(move |p| format!("{f}/{}", p.1))).collect();
        let got = reader.fetch(&pf, &[])?;
        for f in &got.too_big {
            sc.notes.push(format!("{f}: bigger than 8 MB, so it was not read."));
        }
        for f in &folders {
            for (tool, name, k) in PROJECT_FILES {
                let path = format!("{f}/{name}");
                let Some(text) = got.files.get(&path) else { continue };
                found.entry(tool_static(tool)).or_default().push(path.clone());
                let mut none = Vec::new();
                read_file(&mut sc, tool_static(tool), &path, *k, text, f, &mut none);
            }
        }
    }
    for (id, _) in TOOLS {
        let files = found.remove(id).unwrap_or_default();
        sc.found.push((tool_static(id), files));
    }
    sc.servers.truncate(MAX_SERVERS);
    Ok(sc)
}

fn parse_cap() -> usize {
    60
}

/// What was pasted or uploaded: the name decides how it is read (`CLAUDE.md`, `config.toml`, `claude_desktop_config.json`...), and
/// when the name says nothing, the content does.
pub fn scan_given(files: &[(String, String)]) -> Result<Scan, String> {
    if files.is_empty() || files.len() > 12 {
        return Err("Give between 1 and 12 files.".into());
    }
    let mut sc = Scan { origin: "what you pasted".into(), kind: "paste".into(), ..Default::default() };
    let mut found: BTreeMap<&'static str, Vec<String>> = BTreeMap::new();
    for (name, text) in files {
        let n = name.to_ascii_lowercase();
        let short: String = name.chars().filter(|c| !c.is_control()).take(80).collect();
        let trimmed = text.trim_start_matches('\u{feff}').trim();
        if trimmed.is_empty() {
            continue;
        }
        let json_like = trimmed.starts_with('{') || trimmed.starts_with('[');
        // a name that says nothing: the keys of the file do
        let by_content: &'static str = if json_like {
            match parse::jsonc(trimmed) {
                Ok(d) if d.get("context_servers").is_some() => "zed",
                Ok(d) if d.get("servers").is_some() || d.get("inputs").is_some() => "vscode",
                _ => "pasted",
            }
        } else {
            "pasted"
        };
        let tool: &'static str = if n.contains("claude_desktop") {
            "claude-desktop"
        } else if n.contains(".claude.json") || n.contains(".mcp.json") || n.contains("claude.md") || n.contains("claude/settings") {
            "claude-code"
        } else if n.contains("agents.md") || n.ends_with(".toml") {
            "codex"
        } else if n.contains("gemini") {
            "gemini"
        } else if n.contains("cursor") {
            "cursor"
        } else if n.contains("zed") {
            "zed"
        } else if n.contains("vscode") || n.contains("/code/") || n == "mcp.json" {
            "vscode"
        } else {
            by_content
        };
        // a pasted text with no useful name (`pasted.txt`) that is really a JSON file is read as one
        let json_text = json_like && (n.ends_with(".txt") || !n.contains('.')) && parse::jsonc(trimmed).is_ok();
        let kind = if !json_text && (n.ends_with(".md") || n.ends_with(".txt") || n.ends_with(".mdc") || (!json_like && !n.ends_with(".toml") && !looks_toml(trimmed))) {
            Kind::Instruction
        } else if n.ends_with(".toml") || (!json_like && looks_toml(trimmed)) {
            Kind::Toml
        } else if parse::jsonc(trimmed).ok().is_some_and(|d| d.get("projects").is_some_and(Value::is_object) && n.contains("claude")) {
            Kind::ClaudeJson
        } else {
            Kind::ServersJson
        };
        // JSON that is not a server file at all is said so instead of being silently empty
        let before = (sc.servers.len(), sc.instructions.len(), sc.notes.len());
        let mut folders = Vec::new();
        read_file(&mut sc, tool, &short, kind, trimmed, "", &mut folders);
        if (sc.servers.len(), sc.instructions.len(), sc.notes.len()) == before {
            sc.notes.push(format!("{short}: nothing to import was found in it."));
        } else {
            found.entry(tool).or_default().push(short);
        }
    }
    for id in TOOLS.iter().map(|t| t.0).chain(std::iter::once("pasted")) {
        if let Some(f) = found.remove(id) {
            sc.found.push((tool_static(id), f));
        }
    }
    sc.servers.truncate(MAX_SERVERS);
    Ok(sc)
}

fn looks_toml(t: &str) -> bool {
    t.lines().any(|l| l.trim_start().starts_with("[mcp_servers") || l.trim_start().starts_with("model =") || l.trim_start().starts_with("model="))
}

// ---------------------------------------------------------------------------------------------------------------------------
// what the screens see

fn same_command(a: &str, b: &str) -> bool {
    mcp_client::split_command(a) == mcp_client::split_command(b)
}

fn project_by_folder(folder: &str, projs: &[(String, String)]) -> Option<String> {
    let base = folder.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_lowercase();
    if base.is_empty() {
        return None;
    }
    projs.iter().find(|(_, n)| n.to_lowercase() == base).map(|(id, _)| id.clone())
}

impl Scan {
    /// The answer for a screen: names, addresses with their secret parts hidden, which variables are needed, never a secret value.
    pub fn view(&self) -> Value {
        let have = mcp_client::load_servers();
        let restricted = crate::tenant::is_restricted();
        let projs = crate::store::memory_io::projects();
        let mut taken: HashSet<String> = HashSet::new();
        let mut seen_cmds: Vec<(String, String)> = Vec::new();
        let servers: Vec<Value> = self
            .servers
            .iter()
            .map(|s| {
                let p = prepare(&s.raw, false);
                let name = mcp_client::clean_name(&s.raw.name);
                let remote = mcp_client::is_url(&s.raw.url);
                let mut dup = String::new();
                let mut dup_of = String::new();
                if have.iter().any(|h| h.name == name) {
                    dup = "here".into();
                    dup_of = name.clone();
                } else if let Some(h) = have.iter().find(|h| same_command(&h.command, &p.command) || same_command(&h.command, &prepare(&s.raw, true).command)) {
                    dup = "here".into();
                    dup_of = h.name.clone();
                } else if taken.contains(&name) {
                    dup = "scan".into();
                } else if let Some((_, n)) = seen_cmds.iter().find(|(c, _)| same_command(c, &p.command)) {
                    dup = "scan".into();
                    dup_of = n.clone();
                }
                taken.insert(name.clone());
                seen_cmds.push((p.command.clone(), name.clone()));
                json!({
                    "id": s.id, "tool": s.tool, "tool_name": tool_name(s.tool), "source": s.source,
                    "scope": s.scope, "folder": s.folder, "name": name, "kind": if remote { "remote" } else { "program" },
                    "target": p.shown, "disabled": s.raw.disabled,
                    "needs": p.needs.iter().map(|n| serde_json::to_value(n).unwrap_or(Value::Null)).collect::<Vec<_>>(),
                    "plain_env": p.env.values().filter(|v| !v.is_empty()).count(),
                    "notes": p.notes.iter().filter(|n| !n.starts_with("Secrets were not copied")).collect::<Vec<_>>(),
                    "duplicate": dup, "duplicate_of": dup_of,
                })
            })
            .collect();
        let instructions: Vec<Value> = self
            .instructions
            .iter()
            .map(|i| {
                let (notes, secret, more) = parse::instruction_notes(&i.text, MAX_NOTE_CHARS, MAX_NOTES_PER_FILE);
                let preview: String = i.text.lines().map(|l| l.trim().trim_start_matches(|c: char| matches!(c, '-' | '*' | '+' | '>' | ' '))).filter(|l| !l.is_empty() && !l.starts_with('#') && !secret_line(l)).take(3).collect::<Vec<_>>().join(" ").chars().take(180).collect();
                json!({
                    "id": i.id, "tool": i.tool, "tool_name": tool_name(i.tool), "source": i.source,
                    "scope": if i.folder.is_empty() { "global" } else { "project" }, "folder": i.folder,
                    "chars": i.text.chars().count(), "lines": i.text.lines().count(), "preview": preview,
                    "notes": notes.len(), "notes_more": more, "secret_lines": secret,
                    "suggest": if restricted { None } else { project_by_folder(&i.folder, &projs) },
                })
            })
            .collect();
        json!({
            "origin": self.origin, "kind": self.kind,
            "tools": self.found.iter().map(|(t, files)| json!({
                "id": t, "name": tool_name(t), "found": !files.is_empty(), "files": files,
                "servers": self.servers.iter().filter(|s| s.tool == *t).count(),
                "instructions": self.instructions.iter().filter(|s| s.tool == *t).count(),
            })).collect::<Vec<_>>(),
            "servers": servers,
            "instructions": instructions,
            "extras": self.extras.iter().map(|e| json!({"id": e.id, "tool": e.tool, "tool_name": tool_name(e.tool), "kind": e.kind, "name": e.name, "note": e.note})).collect::<Vec<_>>(),
            "notes": self.notes,
            "projects": projs.iter().map(|(id, n)| json!({"id": id, "name": n})).collect::<Vec<_>>(),
            // what this account may import (a restricted account has no MCP servers and no projects)
            "can": {"servers": !restricted && !(crate::tenant::multi_tenant() && self.servers.iter().all(|s| s.raw.url.is_empty())), "programs": !crate::tenant::multi_tenant(), "projects": !restricted, "secrets": !crate::tenant::multi_tenant()},
            "placeholder": PLACEHOLDER,
        })
    }

    /// Names of the selected items, for a dialog that asks the person to agree (built here, never from what a page says).
    /// What the person is asked to approve: for every ticked item, where it goes and what it will do (addresses and commands with
    /// secret parts hidden).
    pub fn describe_for_approval(&self, req: &Request) -> Vec<String> {
        let projs = crate::store::memory_io::projects();
        let mut out = Vec::new();
        for s in &self.servers {
            if req.select.contains(&s.id) {
                let p = prepare(&s.raw, false);
                let what = if mcp_client::is_url(&s.raw.url) { format!("connects to {}", p.shown) } else { format!("runs the program `{}`", p.shown) };
                out.push(format!("server {} ({}): {what}; saved {}", mcp_client::clean_name(&s.raw.name), tool_name(s.tool), if s.raw.disabled { "switched off" } else { "switched on, it starts when a chat uses it" }));
            }
        }
        for i in &self.instructions {
            if req.select.contains(&i.id) {
                let target = req.targets.get(&i.id).map(String::as_str).unwrap_or("notes");
                let (kind, pid) = target.split_once(':').unwrap_or((target, ""));
                let pname = projs.iter().find(|(id, _)| id == pid).map(|(_, n)| n.as_str());
                let place = match (kind, pname) {
                    ("instruction", Some(n)) => format!("added to the end of the instruction of project \"{n}\""),
                    ("notes", Some(n)) => format!("saved as memory notes of project \"{n}\""),
                    _ => "saved as memory notes".to_string(),
                };
                out.push(format!("instructions {} ({}): {place}", i.source, tool_name(i.tool)));
            }
        }
        out
    }

    pub fn describe(&self, select: &[String]) -> Vec<String> {
        let mut out = Vec::new();
        for s in &self.servers {
            if select.contains(&s.id) {
                out.push(format!("server {} ({})", mcp_client::clean_name(&s.raw.name), tool_name(s.tool)));
            }
        }
        for i in &self.instructions {
            if select.contains(&i.id) {
                out.push(format!("instructions {} ({})", i.source, tool_name(i.tool)));
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------------------------------------------------------
// apply and undo

#[derive(Clone, Debug, Default)]
pub struct Request {
    pub select: Vec<String>,
    /// Ids of servers whose secrets are copied too.
    pub secrets: Vec<String>,
    /// The names of those servers, typed back (the explicit confirmation).
    pub confirm: Vec<String>,
    /// Instruction id -> "notes", "notes:<project id>" or "instruction:<project id>".
    pub targets: HashMap<String, String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Batch {
    id: String,
    time: u64,
    origin: String,
    servers: Vec<(String, String)>,
    notes: Vec<(String, String)>,
    appended: Vec<(String, String)>,
    summary: String,
}

#[derive(Default, Serialize, Deserialize)]
struct Log {
    batches: Vec<Batch>,
}

const KEEP_BATCHES: usize = 10;

/// A one-way mark of a command, to tell on undo whether the server is still as it was imported.
fn fingerprint(command: &str) -> String {
    let mut h = Sha256::new();
    h.update(command.as_bytes());
    format!("sha256:{}", h.finalize().iter().map(|b| format!("{b:02x}")).collect::<String>())
}
static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn log_file() -> std::path::PathBuf {
    crate::audit::config_dir().join("migrate-log.json")
}

fn load_log() -> Log {
    std::fs::read_to_string(log_file()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn save_log(l: &Log) {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let _ = std::fs::create_dir_all(crate::audit::config_dir());
    let tmp = log_file().with_extension("json.tmp");
    if let Ok(s) = serde_json::to_string(l) {
        let ok = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp).and_then(|mut f| f.write_all(s.as_bytes())).is_ok();
        if ok {
            let _ = std::fs::rename(&tmp, log_file());
        }
    }
}

/// The last imports, newest first, that can still be undone.
pub fn history() -> Value {
    let l = load_log();
    json!({"batches": l.batches.iter().rev().map(|b| json!({"id": b.id, "time": b.time, "origin": b.origin, "summary": b.summary, "servers": b.servers.len(), "notes": b.notes.len(), "instructions": b.appended.len()})).collect::<Vec<_>>()})
}

fn list_names(v: &[String]) -> String {
    v.join(", ")
}

/// Imports what `req` selects from `reader`. The result says what was done and what was left out and why.
pub fn apply(reader: &dyn Reader, kind: &str, given: Option<&[(String, String)]>, req: &Request) -> Result<Value, String> {
    let sc = match given {
        Some(f) => scan_given(f)?,
        None => scan(reader, kind)?,
    };
    apply_scan(&sc, req)
}

pub fn apply_scan(sc: &Scan, req: &Request) -> Result<Value, String> {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if req.select.is_empty() {
        return Err("Nothing was ticked.".into());
    }
    if req.select.len() > 400 {
        return Err("Too many items were ticked.".into());
    }
    let known: HashSet<&str> = sc.servers.iter().map(|s| s.id.as_str()).chain(sc.instructions.iter().map(|i| i.id.as_str())).collect();
    if let Some(bad) = req.select.iter().find(|i| !known.contains(i.as_str())) {
        return Err(format!("{bad} is not in what was found (it may have changed: look again)."));
    }
    let chosen_servers: Vec<&Server> = sc.servers.iter().filter(|s| req.select.contains(&s.id)).collect();
    let chosen_instr: Vec<&Instruction> = sc.instructions.iter().filter(|i| req.select.contains(&i.id)).collect();
    let restricted = crate::tenant::is_restricted();
    if restricted && !chosen_servers.is_empty() {
        return Err("MCP servers are not available to this account.".into());
    }
    // secrets: only for ticked servers, only where the person confirmed by name, never on a shared server
    let mut copy_ids: HashSet<&str> = HashSet::new();
    if !req.secrets.is_empty() {
        if crate::tenant::multi_tenant() {
            return Err("Keys are not copied to the cloud server. Add the key in the desktop app, where it stays on your computer.".into());
        }
        for id in &req.secrets {
            let s = chosen_servers.iter().find(|s| &s.id == id).ok_or("Keys can only be copied for a server that is ticked.")?;
            copy_ids.insert(&s.id);
        }
        let mut want: Vec<String> = chosen_servers.iter().filter(|s| copy_ids.contains(s.id.as_str())).map(|s| mcp_client::clean_name(&s.raw.name)).collect();
        want.sort();
        want.dedup();
        let mut got: Vec<String> = req.confirm.iter().map(|n| n.trim().to_string()).collect();
        got.sort();
        got.dedup();
        if want != got {
            return Err(format!("Confirm by name first: the keys of {} will be copied into this computer's key store.", list_names(&want)));
        }
    }
    let mut out = json!({"servers": {"added": [], "skipped": []}, "notes": {"added": 0, "already": 0}, "instructions": {"appended": [], "skipped": []}, "needs": [], "secrets_copied": []});
    let mut batch = Batch { id: format!("mg{}", chats::now_millis()), time: chats::now_millis(), origin: sc.origin.clone(), ..Default::default() };
    let push = |out: &mut Value, a: &str, b: &str, v: Value| {
        if let Some(arr) = out[a][b].as_array_mut() {
            arr.push(v);
        }
    };

    // servers
    if !chosen_servers.is_empty() {
        let mut plan: Vec<(ServerConfig, Prepared, Vec<(String, String)>, bool)> = Vec::new();
        let mut names_in_batch: HashSet<String> = HashSet::new();
        let have = mcp_client::load_servers();
        for s in &chosen_servers {
            let name = mcp_client::clean_name(&s.raw.name);
            let skip = |out: &mut Value, why: &str| push(out, "servers", "skipped", json!({"name": name, "why": why}));
            if name.is_empty() || name.chars().count() > 60 {
                skip(&mut out, "The name has no letters or digits, or is too long.");
                continue;
            }
            let remote = mcp_client::is_url(&s.raw.url);
            if !s.raw.url.is_empty() && !remote {
                skip(&mut out, "The address is not an http(s) address.");
                continue;
            }
            if !remote && crate::tenant::multi_tenant() {
                skip(&mut out, "Programs that run on a computer are added in the desktop app, not on the cloud server.");
                continue;
            }
            let want_secrets = copy_ids.contains(s.id.as_str());
            let p = prepare(&s.raw, want_secrets);
            if have.iter().any(|h| h.name == name) {
                skip(&mut out, "A server with this name is already added (nothing was overwritten).");
                continue;
            }
            if let Some(h) = have.iter().find(|h| same_command(&h.command, &p.command)) {
                skip(&mut out, &format!("The same server is already added as \"{}\".", h.name));
                continue;
            }
            if !names_in_batch.insert(name.clone()) {
                skip(&mut out, "Another ticked server has the same name; tick only one of them.");
                continue;
            }
            // the key-store values to write, as (id, value)
            let mut secrets: Vec<(String, String)> = Vec::new();
            let mut cfg = ServerConfig { name: name.clone(), command: p.command.clone(), enabled: !s.raw.disabled, env: p.env.clone(), headers: p.headers.clone(), ..Default::default() };
            if want_secrets && (!p.secret_env.is_empty() || !p.secret_headers.is_empty()) {
                if !crate::ai::keys::available() {
                    skip(&mut out, "There is no key store on this computer to keep the keys in (install a keyring such as GNOME Keyring or KWallet), so the server was not imported with its keys.");
                    continue;
                }
                for (k, v) in &p.secret_env {
                    secrets.push((mcp_client::env_id(&name, k), v.clone()));
                    cfg.secret_env.push(k.clone());
                }
                for (k, v) in &p.secret_headers {
                    secrets.push((mcp_client::header_id(&name, k), v.clone()));
                    cfg.secret_headers.push(k.clone());
                }
            }
            plan.push((cfg, p, secrets, want_secrets));
        }
        for (cfg, p, secrets, want_secrets) in plan {
            let mut failed = None;
            let mut written: Vec<String> = Vec::new();
            for (id, v) in &secrets {
                match crate::ai::keys::set(id, v) {
                    Ok(()) => written.push(id.clone()),
                    Err(e) => {
                        failed = Some(e);
                        break;
                    }
                }
            }
            if let Some(e) = failed {
                for id in &written {
                    crate::ai::keys::clear(id);
                }
                push(&mut out, "servers", "skipped", json!({"name": cfg.name, "why": format!("The keys could not be saved ({e}).")}));
                continue;
            }
            let name = cfg.name.clone();
            let command = cfg.command.clone();
            // the list is changed under its lock; a name that appeared meanwhile is not overwritten
            let added = mcp_client::update_servers(|all| {
                if all.iter().any(|x| x.name == cfg.name) {
                    return false;
                }
                all.push(cfg.clone());
                true
            });
            if !added {
                for id in &written {
                    crate::ai::keys::clear(id);
                }
                push(&mut out, "servers", "skipped", json!({"name": name, "why": "A server with this name was added meanwhile."}));
                continue;
            }
            // the log keeps a fingerprint, not the command: an argument or an address may hold a key that was copied
            batch.servers.push((name.clone(), fingerprint(&command)));
            push(&mut out, "servers", "added", json!(name));
            if want_secrets && p.needs.iter().any(|n| n.in_file) {
                let names: Vec<&str> = p.needs.iter().filter(|n| n.in_file).map(|n| n.name.as_str()).collect();
                if let Some(arr) = out["secrets_copied"].as_array_mut() {
                    arr.push(json!({"server": name, "names": names}));
                }
            }
            // what is still the person's to fill in: everything that was not copied
            let copied = |n: &Need| want_secrets && n.in_file && match n.kind {
                "env" => p.secret_env.contains_key(&n.name),
                "header" => p.secret_headers.contains_key(&n.name),
                _ => true,
            };
            let missing: Vec<&Need> = p.needs.iter().filter(|n| !copied(n)).collect();
            if !missing.is_empty() {
                if let Some(arr) = out["needs"].as_array_mut() {
                    arr.push(json!({"server": name, "needs": missing.iter().map(|n| json!({"kind": n.kind, "name": n.name, "from": n.from})).collect::<Vec<_>>()}));
                }
            }
        }
    }

    // instructions
    let projs = crate::store::memory_io::projects();
    for i in &chosen_instr {
        let target = req.targets.get(&i.id).map(String::as_str).unwrap_or("notes");
        let label = format!("{} ({})", i.source, tool_name(i.tool));
        let skip = |out: &mut Value, why: &str| push(out, "instructions", "skipped", json!({"source": label, "why": why}));
        let (kind_t, pid) = match target.split_once(':') {
            Some((k, p)) => (k, p),
            None => (target, ""),
        };
        if !matches!(kind_t, "notes" | "instruction") || (kind_t == "instruction" && pid.is_empty()) {
            skip(&mut out, "The place to put it is not valid.");
            continue;
        }
        if !pid.is_empty() && (restricted || !projs.iter().any(|(id, _)| id == pid)) {
            skip(&mut out, "There is no such project.");
            continue;
        }
        if kind_t == "instruction" {
            let mut kept = Vec::new();
            let mut dropped = 0;
            for l in i.text.lines() {
                if secret_line(l) {
                    dropped += 1;
                } else {
                    kept.push(l);
                }
            }
            let mut body = kept.join("\n").trim().to_string();
            if body.chars().count() > MAX_INSTRUCTION_CHARS {
                body = body.chars().take(MAX_INSTRUCTION_CHARS).collect::<String>() + "\n…";
            }
            let block = format!("## Imported from {}\n\n{body}", label);
            let mut st = projects::Store::load();
            let Some(p) = st.get_mut(pid) else {
                skip(&mut out, "There is no such project.");
                continue;
            };
            if p.instructions.contains(body.trim()) {
                skip(&mut out, "Its text is already in the project's instruction.");
                continue;
            }
            let appended = if p.instructions.trim().is_empty() { block.clone() } else { format!("\n\n{block}") };
            p.instructions = if p.instructions.trim().is_empty() { block.clone() } else { format!("{}{appended}", p.instructions.trim_end()) };
            p.updated = projects::now_millis();
            let pname = p.name.clone();
            st.save();
            batch.appended.push((pid.to_string(), appended));
            push(&mut out, "instructions", "appended", json!({"source": label, "project": pname, "lines_left_out": dropped}));
        } else {
            if restricted {
                if let Err(e) = crate::tenant::check_room() {
                    skip(&mut out, &e);
                    continue;
                }
            }
            let (notes, secret, more) = parse::instruction_notes(&i.text, MAX_NOTE_CHARS, MAX_NOTES_PER_FILE);
            let (kind, scope) = if pid.is_empty() { ("preference", "") } else { ("project", pid) };
            let before = memory_log::snapshot();
            let (mut added, mut already) = (0usize, 0usize);
            for n in &notes {
                match memory::add_unique(n, kind, scope, false) {
                    Some(true) => added += 1,
                    _ => already += 1,
                }
            }
            let changes = memory_log::record_diff(&before, "import", "", "");
            for c in changes.iter().filter(|c| c.action == "added") {
                batch.notes.push((c.note.clone(), c.after.clone()));
            }
            // the log drops entries that look secret; the notes were filtered above, so these are all the ones that were added
            if let Some(o) = out["notes"].as_object_mut() {
                let a = o["added"].as_u64().unwrap_or(0) + added as u64;
                let b = o["already"].as_u64().unwrap_or(0) + already as u64;
                o.insert("added".into(), json!(a));
                o.insert("already".into(), json!(b));
                o.insert("lines_left_out".into(), json!(o.get("lines_left_out").and_then(Value::as_u64).unwrap_or(0) + secret as u64));
                o.insert("more_than_kept".into(), json!(o.get("more_than_kept").and_then(Value::as_u64).unwrap_or(0) + more as u64));
            }
        }
    }

    let n_added = out["servers"]["added"].as_array().map(Vec::len).unwrap_or(0);
    let n_skipped = out["servers"]["skipped"].as_array().map(Vec::len).unwrap_or(0);
    let notes_added = out["notes"]["added"].as_u64().unwrap_or(0);
    let appended = out["instructions"]["appended"].as_array().map(Vec::len).unwrap_or(0);
    let mut parts = Vec::new();
    if n_added > 0 || n_skipped > 0 {
        parts.push(format!("{n_added} server{} added{}", if n_added == 1 { "" } else { "s" }, if n_skipped > 0 { format!(", {n_skipped} left out") } else { String::new() }));
    }
    if notes_added > 0 || out["notes"]["already"].as_u64().unwrap_or(0) > 0 {
        parts.push(format!("{notes_added} memory note{} added", if notes_added == 1 { "" } else { "s" }));
    }
    if appended > 0 {
        parts.push(format!("{appended} project instruction{} extended", if appended == 1 { "" } else { "s" }));
    }
    let mut summary = if parts.is_empty() { "Nothing was imported.".to_string() } else { format!("{}.", parts.join("; ")) };
    if !memory::load().enabled && notes_added > 0 {
        summary.push_str(" Memory is switched off in Settings, so the notes are used once you switch it on.");
    }
    batch.summary = summary.clone();
    out["message"] = json!(summary);
    let can_undo = !(batch.servers.is_empty() && batch.notes.is_empty() && batch.appended.is_empty());
    out["batch"] = json!(if can_undo { batch.id.clone() } else { String::new() });
    if can_undo {
        let mut l = load_log();
        l.batches.push(batch);
        let extra = l.batches.len().saturating_sub(KEEP_BATCHES);
        l.batches.drain(..extra);
        save_log(&l);
    }
    Ok(out)
}

/// Takes an import back: the servers it added (if they were not changed since), the notes it added (if not edited since) and the
/// text it appended to project instructions (if the instruction still ends with it). What was changed by hand since is left alone.
pub fn undo(batch_id: &str) -> Result<Value, String> {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut l = load_log();
    let i = l.batches.iter().position(|b| b.id == batch_id).ok_or("That import is not in the list any more (only the last few can be undone).")?;
    let b = l.batches.remove(i);
    let (mut servers, mut kept) = (0usize, Vec::new());
    for (name, command) in &b.servers {
        let removed = mcp_client::update_servers(|all| {
            let Some(pos) = all.iter().position(|s| &s.name == name && (&s.command == command || &fingerprint(&s.command) == command)) else { return None };
            Some(all.remove(pos))
        });
        match removed {
            Some(cfg) => {
                mcp_client::clear_secrets(&cfg);
                servers += 1;
            }
            None => kept.push(format!("{name} (changed or removed since)")),
        }
    }
    let before = memory_log::snapshot();
    let mut notes = 0usize;
    for (id, text) in &b.notes {
        let same = memory::load().items.iter().any(|x| &x.id == id && x.text.trim() == text.trim());
        if same && memory::forget(id) {
            notes += 1;
        } else {
            kept.push(format!("a note (edited or removed since): {}", text.chars().take(40).collect::<String>()));
        }
    }
    if notes > 0 {
        memory_log::record_diff(&before, "import-undo", "", "");
    }
    let mut instr = 0usize;
    for (pid, text) in &b.appended {
        let mut st = projects::Store::load();
        let ok = match st.get_mut(pid) {
            Some(p) if p.instructions.ends_with(text.as_str()) => {
                let cut = p.instructions.len() - text.len();
                p.instructions.truncate(cut);
                p.updated = projects::now_millis();
                true
            }
            _ => false,
        };
        if ok {
            st.save();
            instr += 1;
        } else {
            kept.push("a project instruction (changed since)".into());
        }
    }
    save_log(&l);
    let mut msg = format!("Undone: {servers} server{}, {notes} note{}, {instr} project instruction{}.", if servers == 1 { "" } else { "s" }, if notes == 1 { "" } else { "s" }, if instr == 1 { "" } else { "s" });
    if !kept.is_empty() {
        msg.push_str(&format!(" Left alone: {}.", kept.join("; ")));
    }
    Ok(json!({"message": msg, "left_alone": kept}))
}

// ---------------------------------------------------------------------------------------------------------------------------
// entry points for the service ops

/// The source an op names: this computer, a machine, or text that was handed over.
pub enum Origin {
    Computer,
    Machine(String),
    Given(Vec<(String, String)>),
}

impl Origin {
    /// From the request of a client: `{"source": "computer"|"machine"|"paste", "machine": "...", "files": [{"name","text"}]}`.
    pub fn from_request(req: &Value) -> Result<Origin, String> {
        let src = req.get("source").and_then(Value::as_str).unwrap_or("computer");
        match src {
            "computer" => {
                if crate::tenant::multi_tenant() {
                    return Err("The server cannot see your computer. Paste or upload the files, or use \"Import from my computer\" (the desktop link).".into());
                }
                Ok(Origin::Computer)
            }
            "machine" => {
                if crate::tenant::is_restricted() {
                    return Err("Machines (SSH) are not available to this account.".into());
                }
                let name = req.get("machine").and_then(Value::as_str).unwrap_or("").trim().to_string();
                if name.is_empty() {
                    return Err("Say which machine.".into());
                }
                Ok(Origin::Machine(name))
            }
            "paste" => {
                let files = req.get("files").and_then(Value::as_array).ok_or("Give the files to read.")?;
                if files.len() > 12 {
                    return Err("At most 12 files at once.".into());
                }
                let mut out = Vec::new();
                let mut total = 0usize;
                for f in files {
                    let name = f.get("name").and_then(Value::as_str).unwrap_or("pasted.txt").to_string();
                    let text = f.get("text").and_then(Value::as_str).ok_or("A file has no text.")?.to_string();
                    total += text.len();
                    if text.len() > 2 * 1024 * 1024 || total > 4 * 1024 * 1024 {
                        return Err("That is too much text (2 MB per file, 4 MB in all).".into());
                    }
                    out.push((name.chars().take(120).collect(), text));
                }
                Ok(Origin::Given(out))
            }
            _ => Err("The source is one of: computer, machine, paste.".into()),
        }
    }

    pub fn scan(&self) -> Result<Scan, String> {
        match self {
            Origin::Computer => scan(&source::Local::new()?, "computer"),
            Origin::Machine(m) => scan(&source::Ssh { machine: m.clone() }, "machine"),
            Origin::Given(f) => scan_given(f),
        }
    }
}

impl Request {
    pub fn from_json(v: &Value) -> Result<Request, String> {
        let list = |k: &str| -> Result<Vec<String>, String> {
            match v.get(k) {
                None | Some(Value::Null) => Ok(Vec::new()),
                Some(Value::Array(a)) if a.len() <= 400 => a.iter().map(|x| x.as_str().filter(|s| s.len() <= 80 && !s.chars().any(char::is_control)).map(str::to_string).ok_or_else(|| format!("{k} must be a list of short texts."))).collect(),
                _ => Err(format!("{k} must be a list of short texts.")),
            }
        };
        let mut targets = HashMap::new();
        if let Some(o) = v.get("targets").and_then(Value::as_object) {
            if o.len() > 400 {
                return Err("Too many targets.".into());
            }
            for (k, t) in o {
                let t = t.as_str().filter(|t| t.len() <= 80 && !t.chars().any(char::is_control)).ok_or("A target is not valid.")?;
                targets.insert(k.clone(), t.to_string());
            }
        }
        Ok(Request { select: list("select")?, secrets: list("secrets")?, confirm: list("confirm")?, targets })
    }
}
