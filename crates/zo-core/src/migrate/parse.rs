//! Readers for the files other tools keep their MCP servers and instructions in. Everything here is a pure function of text: the files
//! are fetched elsewhere (`source.rs`), so these are tested with fixtures that follow each tool's documented format.
//!
//! A server comes out as a [`Raw`]: exactly what the file says, secrets included. Deciding what is a secret and what may be shown or
//! copied happens in `mod.rs`.

use serde_json::Value;
use std::collections::BTreeMap;

/// One server as a file describes it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Raw {
    pub name: String,
    /// A program to start (`command` + `args`), or empty for a remote server.
    pub program: String,
    pub args: Vec<String>,
    /// The address of a remote server.
    pub url: String,
    pub env: BTreeMap<String, String>,
    pub headers: BTreeMap<String, String>,
    pub disabled: bool,
    /// Things worth telling the person (an older transport, a header whose value lives in an environment variable...).
    pub notes: Vec<String>,
    /// Header names whose value is not in the file but in an environment variable the file names (Codex `bearer_token_env_var`).
    pub header_from_env: BTreeMap<String, String>,
    /// The address came from a `url` key (not `httpUrl`).
    pub plain_url: bool,
}

/// Parses JSON that may have comments, trailing commas or a byte-order mark (VS Code and Zed settings are written that way).
pub fn jsonc(text: &str) -> Result<Value, String> {
    let t = text.trim_start_matches('\u{feff}');
    if let Ok(v) = serde_json::from_str::<Value>(t) {
        return Ok(v);
    }
    let cleaned = strip_jsonc(t);
    serde_json::from_str(&cleaned).map_err(|e| format!("That is not valid JSON: {e}"))
}

fn strip_jsonc(t: &str) -> String {
    let chars: Vec<char> = t.chars().collect();
    let mut out = String::with_capacity(t.len());
    let (mut i, mut in_str) = (0, false);
    while i < chars.len() {
        let c = chars[i];
        if in_str {
            out.push(c);
            if c == '\\' && i + 1 < chars.len() {
                out.push(chars[i + 1]);
                i += 1;
            } else if c == '"' {
                in_str = false;
            }
        } else if c == '"' {
            in_str = true;
            out.push(c);
        } else if c == '/' && chars.get(i + 1) == Some(&'/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        } else if c == '/' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i += 2;
            continue;
        } else {
            out.push(c);
        }
        i += 1;
    }
    // trailing commas before } or ]
    let o: Vec<char> = out.chars().collect();
    let mut res = String::with_capacity(o.len());
    let (mut j, mut s) = (0, false);
    while j < o.len() {
        let c = o[j];
        if s {
            res.push(c);
            if c == '\\' && j + 1 < o.len() {
                res.push(o[j + 1]);
                j += 1;
            } else if c == '"' {
                s = false;
            }
        } else if c == '"' {
            s = true;
            res.push(c);
        } else if c == ',' {
            let next = o[j + 1..].iter().find(|x| !x.is_whitespace());
            if !matches!(next, Some('}') | Some(']')) {
                res.push(c);
            }
        } else {
            res.push(c);
        }
        j += 1;
    }
    res
}

fn string_map(v: &Value) -> BTreeMap<String, String> {
    v.as_object()
        .map(|o| {
            o.iter()
                .filter_map(|(k, v)| match v {
                    Value::String(s) => Some((k.clone(), s.clone())),
                    Value::Number(n) => Some((k.clone(), n.to_string())),
                    Value::Bool(b) => Some((k.clone(), b.to_string())),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn string_list(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

/// One server definition in the JSON shapes of Claude Code, Claude Desktop, Cursor, VS Code, Gemini CLI and Zed.
fn def_json(name: &str, d: &Value) -> Option<Raw> {
    let obj = d.as_object()?;
    let mut r = Raw { name: name.to_string(), ..Default::default() };
    let kind = obj.get("type").and_then(Value::as_str).unwrap_or("").to_ascii_lowercase();
    let http = obj.get("httpUrl").and_then(Value::as_str);
    let url = http.or_else(|| obj.get("url").and_then(Value::as_str)).or_else(|| obj.get("serverUrl").and_then(Value::as_str));
    match (url, obj.get("command")) {
        (Some(u), _) if !u.trim().is_empty() => {
            r.url = u.trim().to_string();
            // Zohara Agent speaks the streamable HTTP transport; the older SSE one needs a different client
            if kind == "sse" {
                r.notes.push("This server uses the older SSE transport; it may not connect here.".into());
            }
            // Gemini CLI: `url` is an SSE address, `httpUrl` the streamable one (mod.rs adds the note for that tool)
            r.plain_url = http.is_none();
        }
        // Zed's older form: "command": {"path": "npx", "args": [...], "env": {...}}
        (_, Some(Value::Object(c))) => {
            r.program = c.get("path").or_else(|| c.get("command")).and_then(Value::as_str)?.to_string();
            r.args = c.get("args").map(string_list).unwrap_or_default();
            r.env = c.get("env").map(string_map).unwrap_or_default();
        }
        (_, Some(Value::String(c))) if !c.trim().is_empty() => {
            r.program = c.trim().to_string();
            r.args = obj.get("args").map(string_list).unwrap_or_default();
        }
        _ => return None,
    }
    if let Some(e) = obj.get("env") {
        for (k, v) in string_map(e) {
            r.env.entry(k).or_insert(v);
        }
    }
    if let Some(h) = obj.get("headers").or_else(|| obj.get("http_headers")) {
        r.headers = string_map(h);
    }
    r.disabled = obj.get("disabled").and_then(Value::as_bool).unwrap_or(false) || obj.get("enabled").and_then(Value::as_bool) == Some(false) || obj.get("isActive").and_then(Value::as_bool) == Some(false);
    if obj.get("source").and_then(Value::as_str) == Some("extension") {
        return None;
    }
    Some(r)
}

/// A map of name -> definition.
fn defs_json(map: &Value) -> Vec<Raw> {
    map.as_object().map(|o| o.iter().filter_map(|(n, d)| def_json(n, d)).collect()).unwrap_or_default()
}

/// Servers of a JSON document, wherever the tool keeps them: `mcpServers` (Claude Code and Desktop, Cursor, Gemini), `servers`
/// (VS Code mcp.json), `mcp.servers` (VS Code settings), `context_servers` (Zed). Servers of a Claude Code *project* are read apart
/// (`claude_projects`) because they belong to a folder.
pub fn servers_json(doc: &Value) -> Vec<Raw> {
    let mut out = Vec::new();
    for key in ["mcpServers", "servers", "context_servers"] {
        if let Some(m) = doc.get(key).filter(|m| m.is_object()) {
            out.extend(defs_json(m));
        }
    }
    if let Some(m) = doc.get("mcp").and_then(|m| m.get("servers")).filter(|m| m.is_object()) {
        out.extend(defs_json(m));
    }
    // VS Code mcp.json lists `inputs` (prompted values) that servers refer to as ${input:id}
    // a file that is one server itself ({"command": ..} or {"url": ..}) is accepted too
    if out.is_empty() {
        if let Some(r) = def_json("server", doc) {
            out.push(r);
        }
    }
    // a bare {"name": {...}} map, as in a README snippet
    if out.is_empty() {
        if let Some(o) = doc.as_object() {
            let looks = o.values().all(|v| v.is_object()) && !o.is_empty();
            if looks {
                out.extend(defs_json(doc));
            }
        }
    }
    out
}

/// Claude Code's `~/.claude.json`: (project folder, its local-scope servers) for every project that has any, plus the folders of all
/// projects (they may hold a `.mcp.json` and a CLAUDE.md).
pub fn claude_projects(doc: &Value) -> (Vec<String>, Vec<(String, Vec<Raw>)>) {
    let mut folders = Vec::new();
    let mut with_servers = Vec::new();
    if let Some(p) = doc.get("projects").and_then(Value::as_object) {
        for (path, v) in p {
            if path.is_empty() || path.len() > 400 || path.chars().any(char::is_control) {
                continue;
            }
            folders.push(path.clone());
            if let Some(m) = v.get("mcpServers") {
                let s = defs_json(m);
                if !s.is_empty() {
                    with_servers.push((path.clone(), s));
                }
            }
        }
    }
    (folders, with_servers)
}

/// The `model` a JSON settings file names (Claude Code: a string; Gemini CLI: a string or `{"name": ..}`).
pub fn model_json(doc: &Value) -> Option<String> {
    let m = doc.get("model")?;
    m.as_str().map(str::to_string).or_else(|| m.get("name").and_then(Value::as_str).map(str::to_string)).filter(|s| !s.is_empty() && s.len() < 100)
}

/// Codex CLI's `config.toml`: `[mcp_servers.NAME]` tables, and the `model` and `model_provider` at the top.
pub fn servers_toml(text: &str) -> Result<(Vec<Raw>, Option<String>), String> {
    let doc: toml::Table = text.parse().map_err(|e: toml::de::Error| format!("That is not valid TOML: {}", e.message()))?;
    let mut out = Vec::new();
    if let Some(t) = doc.get("mcp_servers").and_then(toml::Value::as_table) {
        for (name, d) in t {
            let Some(d) = d.as_table() else { continue };
            let mut r = Raw { name: name.clone(), ..Default::default() };
            let strs = |k: &str| -> Vec<String> { d.get(k).and_then(toml::Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default() };
            let map = |k: &str| -> BTreeMap<String, String> { d.get(k).and_then(toml::Value::as_table).map(|t| t.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string()))).collect()).unwrap_or_default() };
            if let Some(u) = d.get("url").and_then(toml::Value::as_str) {
                r.url = u.to_string();
                r.headers = map("http_headers");
                // a header whose value is read from an environment variable: the file only names the variable
                for (h, var) in map("env_http_headers") {
                    r.header_from_env.insert(h, var);
                }
                if let Some(var) = d.get("bearer_token_env_var").and_then(toml::Value::as_str) {
                    r.header_from_env.insert("Authorization".into(), var.to_string());
                }
            } else if let Some(c) = d.get("command").and_then(toml::Value::as_str) {
                r.program = c.to_string();
                r.args = strs("args");
                r.env = map("env");
                for v in strs("env_vars") {
                    r.notes.push(format!("It also uses the environment variable {v} of the shell it was started from."));
                }
            } else {
                continue;
            }
            r.disabled = d.get("enabled").and_then(toml::Value::as_bool) == Some(false);
            out.push(r);
        }
    }
    let model = doc.get("model").and_then(toml::Value::as_str).map(str::to_string);
    let provider = doc.get("model_provider").and_then(toml::Value::as_str);
    let model = match (model, provider) {
        (Some(m), Some(p)) => Some(format!("{m} (provider {p})")),
        (Some(m), None) => Some(m),
        _ => None,
    };
    Ok((out, model))
}

/// Cuts a markdown instruction file into short notes: one per bullet or paragraph line, without headings, code blocks and links-only
/// lines. Returns (notes, lines left out because they looked like a secret).
pub fn instruction_notes(text: &str, max_chars: usize, max_notes: usize) -> (Vec<String>, usize, usize) {
    let mut notes = Vec::new();
    let (mut secret, mut more) = (0usize, 0usize);
    let mut in_code = false;
    for raw in text.lines() {
        let l = raw.trim();
        if l.starts_with("```") || l.starts_with("~~~") {
            in_code = !in_code;
            continue;
        }
        if in_code || l.is_empty() || l.starts_with('#') || l.starts_with("---") || l.starts_with("<!--") || l.starts_with('|') {
            continue;
        }
        let mut l = l.trim_start_matches(|c: char| matches!(c, '-' | '*' | '+' | '>' | ' ')).trim();
        // numbered list
        if let Some(rest) = l.split_once(". ").filter(|(n, _)| !n.is_empty() && n.len() <= 3 && n.chars().all(|c| c.is_ascii_digit())) {
            l = rest.1.trim();
        }
        let l = l.replace("**", "").replace('`', "");
        if l.chars().count() < 12 {
            continue;
        }
        if super::secret_line(&l) {
            secret += 1;
            continue;
        }
        if notes.len() >= max_notes {
            more += 1;
            continue;
        }
        let cut: String = if l.chars().count() > max_chars { l.chars().take(max_chars - 1).collect::<String>() + "…" } else { l };
        notes.push(cut);
    }
    (notes, secret, more)
}
