//! Tests of the Migrate page's core, on fixtures written the way each tool documents its files. None of the tokens below is real.

use super::source::{Local, Ssh};
use super::*;
use crate::store::TEST_ENV_LOCK;
use serde_json::json;
use std::path::{Path, PathBuf};

const GH_TOKEN: &str = "ghp_AbCdEfGhIjKlMnOpQrStUvWxYz0123456789";
const SENTRY: &str = "sntrys_eyJpYXQiOjE2ODg4OTk5OTksInVybCI6Imh0dHBz";
const PG_PW: &str = "hunter2pass";
const SLACK: &str = concat!("xo", "xb-123456789012-1234567890123-AbCdEfGhIjKlMnOpQrStUvWx");
const FIGMA: &str = "figd_AbCdEfGhIjKlMnOpQrStUvWxYz012345";
const ALL_SECRETS: &[&str] = &[GH_TOKEN, SENTRY, PG_PW, SLACK, FIGMA, "tvly-AbCdEf0123456789AbCdEf012345", "zapier_AbCdEf0123456789AbCdEf0123"];

/// A throw-away home folder holding the config files of every tool.
struct Fixture {
    home: PathBuf,
}

impl Fixture {
    fn new(tag: &str) -> Fixture {
        let home = std::env::temp_dir().join(format!("zo-migrate-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        let home = std::fs::canonicalize(home).unwrap();
        let f = Fixture { home };
        f.fill();
        f
    }

    fn put(&self, rel: &str, text: &str) {
        let p = self.home.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    fn fill(&self) {
        let h = self.home.display().to_string();
        // Claude Code: user servers, a local-scope server of one project, project folders with .mcp.json and CLAUDE.md
        self.put(".claude.json", &format!(r#"{{
  "numStartups": 212,
  "installMethod": "native",
  "mcpServers": {{
    "github": {{"type": "stdio", "command": "npx", "args": ["-y", "@modelcontextprotocol/server-github"], "env": {{"GITHUB_PERSONAL_ACCESS_TOKEN": "{GH_TOKEN}", "LOG_LEVEL": "info"}}}},
    "sentry": {{"type": "http", "url": "https://mcp.sentry.dev/mcp", "headers": {{"Authorization": "Bearer {SENTRY}", "X-Region": "eu"}}}},
    "linear": {{"type": "sse", "url": "https://mcp.linear.app/sse"}}
  }},
  "projects": {{
    "{h}/webapp": {{"allowedTools": [], "hasTrustDialogAccepted": true, "mcpServers": {{"postgres": {{"command": "npx", "args": ["-y", "@modelcontextprotocol/server-postgres", "postgresql://app:{PG_PW}@localhost:5432/appdb"]}}}}}},
    "{h}/notes": {{"mcpServers": {{}}}},
    "/etc/elsewhere": {{"mcpServers": {{}}}}
  }}
}}"#));
        self.put(".claude/settings.json", r#"{"model": "opus", "permissions": {"allow": ["Bash(git status)"]}, "hooks": {}}"#);
        self.put(".claude/CLAUDE.md", "# My rules\n\n- Always answer in short sentences.\n- Prefer Rust for command line tools.\n- My deploy key is sk-live-AbCdEf0123456789AbCdEf0123456789 (never paste it).\n\n```sh\nrm -rf nothing-here-in-code-blocks\n```\n1. Run the tests before saying a task is done.\n");
        self.put(".claude/commands/review.md", "Review the diff.");
        self.put(".claude/commands/deploy.md", "Deploy it.");
        self.put(".claude/skills/pdf/SKILL.md", "---\nname: pdf\n---\n");
        self.put("webapp/.mcp.json", r#"{"mcpServers": {"figma": {"type": "http", "url": "https://mcp.figma.com/mcp", "headers": {"X-Figma-Token": "figd_AbCdEfGhIjKlMnOpQrStUvWxYz012345"}}}}"#);
        self.put("webapp/CLAUDE.md", "# Webapp\n\nThis project uses pnpm, never npm.\nThe API lives in services/api and is written in Go.\n");
        // Claude Desktop (Linux path)
        self.put(".config/Claude/claude_desktop_config.json", &r#"{"mcpServers": {"filesystem": {"command": "npx", "args": ["-y", "@modelcontextprotocol/server-filesystem", "/home/dev/Documents"]}, "slack": {"command": "npx", "args": ["-y", "@modelcontextprotocol/server-slack"], "env": {"SLACK_BOT_TOKEN": "@@SLACK@@", "SLACK_TEAM_ID": "T01234567"}}}, "globalShortcut": "Ctrl+Space"}"#.replace("@@SLACK@@", SLACK));
        // Codex CLI
        self.put(".codex/config.toml", "model = \"gpt-5-codex\"\nmodel_provider = \"openai\"\napproval_policy = \"on-request\"\n\n[mcp_servers.context7]\ncommand = \"npx\"\nargs = [\"-y\", \"@upstash/context7-mcp\"]\nenv = { CONTEXT7_API_KEY = \"tvly-AbCdEf0123456789AbCdEf012345\" }\n\n[mcp_servers.docs]\nurl = \"https://docs.example.com/mcp\"\nbearer_token_env_var = \"DOCS_TOKEN\"\n\n[mcp_servers.old]\ncommand = \"old-server\"\nenabled = false\n");
        self.put(".codex/AGENTS.md", "# Codex rules\n\n* Keep commit messages under 72 characters.\n");
        // Gemini CLI
        self.put(".gemini/settings.json", r#"{"theme": "Default", "model": {"name": "gemini-2.5-pro"}, "mcpServers": {"remote-http": {"httpUrl": "https://gem.example.com/mcp", "headers": {"Authorization": "Bearer ${GEM_TOKEN}"}}, "remote-sse": {"url": "https://gem.example.com/sse"}, "local": {"command": "python3", "args": ["-m", "my_server"], "env": {"API_KEY": "$GEM_KEY"}, "trust": false}}}"#);
        self.put(".gemini/GEMINI.md", "# Gemini\n\nUse metric units in every answer you give.\n");
        // Cursor: environment references
        self.put(".cursor/mcp.json", r#"{"mcpServers": {"zapier": {"url": "https://mcp.zapier.com/api/mcp/s/zapier_AbCdEf0123456789AbCdEf0123/mcp"}, "brave": {"command": "npx", "args": ["-y", "@modelcontextprotocol/server-brave-search"], "env": {"BRAVE_API_KEY": "${env:BRAVE_API_KEY}"}}}}"#);
        // VS Code: JSON with comments, trailing commas and prompted inputs
        self.put(".config/Code/User/mcp.json", "// MCP servers\n{\n  \"inputs\": [{\"type\": \"promptString\", \"id\": \"ctx-key\", \"description\": \"Key\", \"password\": true}],\n  \"servers\": {\n    /* a remote one */\n    \"context\": {\"type\": \"http\", \"url\": \"https://ctx.example.com/mcp\", \"headers\": {\"CONTEXT_API_KEY\": \"${input:ctx-key}\"}},\n    \"time\": {\"type\": \"stdio\", \"command\": \"uvx\", \"args\": [\"mcp-server-time\"],},\n  },\n}\n");
        // Zed: both shapes of a custom server, a remote one, and an extension that cannot be imported
        self.put(".config/zed/settings.json", "// Zed settings\n{\n  \"theme\": \"One Dark\",\n  \"context_servers\": {\n    \"mcp-old\": {\"command\": {\"path\": \"npx\", \"args\": [\"-y\", \"some-mcp\"], \"env\": {\"MODE\": \"production\"}}},\n    \"mcp-new\": {\"source\": \"custom\", \"command\": \"uvx\", \"args\": [\"another-mcp\"], \"env\": {}},\n    \"remote-zed\": {\"url\": \"https://zed.example.com/mcp\", \"headers\": {}},\n    \"from-ext\": {\"source\": \"extension\", \"settings\": {}},\n  },\n}\n");
    }

    fn scan(&self) -> Scan {
        scan(&Local::at(&self.home), "computer").unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

fn server<'a>(sc: &'a Scan, name: &str) -> &'a Server {
    sc.servers.iter().find(|s| s.raw.name == name).unwrap_or_else(|| panic!("no server {name}: {:?}", sc.servers.iter().map(|s| &s.raw.name).collect::<Vec<_>>()))
}

/// Makes this test's config, data and key files private to it, and runs `f` as a desktop (no tenant).
fn sandbox<R>(tag: &str, f: impl FnOnce(&Path) -> R) -> R {
    let _g = TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!("zo-migrate-sb-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("XDG_CONFIG_HOME", dir.join("config"));
    std::env::set_var("XDG_DATA_HOME", dir.join("data"));
    std::env::set_var("ZOHARA_AGENT_KEYS_FILE", dir.join("keys.json"));
    let r = f(&dir);
    for k in ["XDG_CONFIG_HOME", "XDG_DATA_HOME", "ZOHARA_AGENT_KEYS_FILE"] {
        std::env::remove_var(k);
    }
    let _ = std::fs::remove_dir_all(&dir);
    r
}

// ---- the formats ----

#[test]
fn json_with_comments_and_trailing_commas_is_read_and_urls_survive() {
    let v = parse::jsonc("// c\n{\n \"a\": \"https://x.example/a//b\", /* x */ \"b\": [1, 2,],\n}\n").unwrap();
    assert_eq!(v["a"], "https://x.example/a//b");
    assert_eq!(v["b"], json!([1, 2]));
    assert!(parse::jsonc("{nope").is_err());
}

#[test]
fn every_tool_is_detected_with_its_servers_and_files() {
    let fx = Fixture::new("all");
    let sc = fx.scan();
    let found: Vec<(&str, usize)> = sc.found.iter().map(|(t, f)| (*t, f.len())).collect();
    for (id, _) in TOOLS {
        assert!(found.iter().any(|(t, n)| t == id && *n > 0), "{id} not detected: {found:?}");
    }
    let names: Vec<&str> = sc.servers.iter().map(|s| s.raw.name.as_str()).collect();
    for want in ["github", "sentry", "linear", "postgres", "figma", "filesystem", "slack", "context7", "docs", "old", "remote-http", "remote-sse", "local", "zapier", "brave", "context", "time", "mcp-old", "mcp-new", "remote-zed"] {
        assert!(names.contains(&want), "{want} missing in {names:?}");
    }
    // a Zed server from an extension cannot be imported
    assert!(!names.contains(&"from-ext"));
    // the project folder outside the home folder is not read
    assert!(sc.instructions.iter().all(|i| i.folder.is_empty() || i.folder.starts_with(fx.home.to_str().unwrap())));
}

#[test]
fn claude_code_scopes_and_extras() {
    let fx = Fixture::new("cc");
    let sc = fx.scan();
    assert_eq!(server(&sc, "github").scope, "user");
    let pg = server(&sc, "postgres");
    assert_eq!(pg.scope, "local");
    assert!(pg.folder.ends_with("/webapp"));
    assert_eq!(server(&sc, "figma").scope, "project");
    assert!(server(&sc, "linear").raw.notes.iter().any(|n| n.contains("SSE")));
    // custom commands and skills are listed by name only; the model is shown, not imported
    let ex: Vec<(&str, &str)> = sc.extras.iter().filter(|e| e.tool == "claude-code").map(|e| (e.kind, e.name.as_str())).collect();
    assert!(ex.contains(&("command", "review")) && ex.contains(&("command", "deploy")) && ex.contains(&("skill", "pdf")) && ex.contains(&("model", "opus")), "{ex:?}");
    // the instruction files: the global one and the project's
    let i: Vec<(&str, bool)> = sc.instructions.iter().map(|i| (i.tool, i.folder.is_empty())).collect();
    assert!(i.contains(&("claude-code", true)) && i.contains(&("claude-code", false)) && i.contains(&("codex", true)) && i.contains(&("gemini", true)));
}

#[test]
fn codex_toml_gemini_cursor_vscode_zed_shapes() {
    let fx = Fixture::new("shapes");
    let sc = fx.scan();
    let c7 = server(&sc, "context7");
    assert_eq!((c7.raw.program.as_str(), c7.raw.args.len()), ("npx", 2));
    assert!(c7.raw.env.contains_key("CONTEXT7_API_KEY"));
    let docs = server(&sc, "docs");
    assert_eq!(docs.raw.url, "https://docs.example.com/mcp");
    assert_eq!(docs.raw.header_from_env.get("Authorization").map(String::as_str), Some("DOCS_TOKEN"));
    assert!(server(&sc, "old").raw.disabled);
    assert!(sc.extras.iter().any(|e| e.tool == "codex" && e.name == "gpt-5-codex (provider openai)"));
    // Gemini: httpUrl is the streamable address; a plain url is SSE
    assert!(server(&sc, "remote-http").raw.notes.is_empty() && !server(&sc, "remote-http").raw.plain_url);
    assert!(server(&sc, "remote-sse").raw.plain_url);
    let v = sc.view();
    let note = |n: &str| v["servers"].as_array().unwrap().iter().find(|s| s["name"] == n).unwrap()["notes"].to_string();
    assert!(note("remote-sse").contains("SSE") && !note("remote-http").contains("SSE"));
    assert!(sc.extras.iter().any(|e| e.tool == "gemini" && e.name == "gemini-2.5-pro"));
    // VS Code (comments, trailing commas) and Zed (both shapes)
    assert_eq!(server(&sc, "time").raw.program, "uvx");
    assert_eq!(server(&sc, "context").raw.url, "https://ctx.example.com/mcp");
    assert_eq!((server(&sc, "mcp-old").raw.program.as_str(), server(&sc, "mcp-old").raw.env["MODE"].as_str()), ("npx", "production"));
    assert_eq!(server(&sc, "mcp-new").raw.program, "uvx");
    assert_eq!(server(&sc, "remote-zed").raw.url, "https://zed.example.com/mcp");
}

// ---- secrets ----

#[test]
fn what_the_screens_get_never_holds_a_secret() {
    let fx = Fixture::new("view");
    let sc = fx.scan();
    let shown = sc.view().to_string();
    for s in ALL_SECRETS {
        assert!(!shown.contains(s), "the view leaks {s}");
    }
    for name in ["GITHUB_PERSONAL_ACCESS_TOKEN", "Authorization", "SLACK_BOT_TOKEN", "CONTEXT7_API_KEY", "connection string"] {
        assert!(shown.contains(name), "{name} should be named as needed");
    }
    // non-secret values are not "needed": a log level, a team id? (a team id is not obviously harmless, so it counts)
    let gh = prepare(&server(&sc, "github").raw, false);
    assert_eq!(gh.env.get("LOG_LEVEL").map(String::as_str), Some("info"));
    assert_eq!(gh.env.get("GITHUB_PERSONAL_ACCESS_TOKEN").map(String::as_str), Some(""));
    // an address with a secret in its path or query is shown without it
    let z = prepare(&server(&sc, "zapier").raw, false);
    assert!(z.shown.contains("•••") && !z.shown.contains("zapier_AbCdEf") && z.command.contains(PLACEHOLDER));
    let pg = prepare(&server(&sc, "postgres").raw, false);
    assert!(!pg.shown.contains(PG_PW) && !pg.command.contains(PG_PW) && pg.command.contains(PLACEHOLDER));
    // references to environment variables are never copied, only named
    let brave = prepare(&server(&sc, "brave").raw, true);
    assert!(brave.secret_env.is_empty() && brave.needs.iter().any(|n| n.name == "BRAVE_API_KEY" && n.from.as_deref() == Some("${env:BRAVE_API_KEY}")));
}

#[test]
fn secret_names_and_values() {
    for n in ["GITHUB_TOKEN", "apiKey", "X-API-Key", "Authorization", "slack_bot_token", "DB_PASSWORD", "ANTHROPIC_API_KEY", "client-secret"] {
        assert!(secret_name(n), "{n}");
    }
    for n in ["PATH", "NODE_ENV", "LOG_LEVEL", "HOME", "Content-Type", "KEYBOARD", "X-Region"] {
        assert!(!secret_name(n), "{n}");
    }
    assert!(secret_word("ghp_AbCdEfGhIjKlMnOpQrStUvWxYz0123456789") && secret_word("sk-ant-api03-AbCdEf0123456789xyz") && !secret_word("hello") && !secret_word("@modelcontextprotocol/server-github"));
    assert!(is_reference("${GITHUB_TOKEN}") && is_reference("Bearer ${env:X}") && is_reference("$TOKEN") && !is_reference("abc$def"));
    assert!(harmless("info") && harmless("production") && harmless("/home/dev/docs") && harmless("application/json") && harmless("https://api.example.com/v1") && harmless("8080") && !harmless("https://u:p@x.com") && !harmless("T01234567"));
}

#[test]
fn argument_secrets_are_found() {
    let a = |v: &[&str]| secret_args(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
    assert_eq!(a(&["-y", "pkg", "--api-key", "abcdef"]), vec![None, None, None, Some("api-key".into())]);
    assert_eq!(a(&["--token=abcdef", "--port", "80"]), vec![Some("token".into()), None, None]);
    assert!(a(&["mcp-remote", "https://x.example/sse", "--header", "Authorization: Bearer abc"])[3].is_some());
    assert!(a(&["API_KEY=abc123"])[0].is_some());
    assert_eq!(a(&["-y", "@scope/server-name", "--stdio"]), vec![None, None, None]);
}

// ---- applying ----

fn req(select: &[&str]) -> Request {
    Request { select: select.iter().map(|s| s.to_string()).collect(), ..Default::default() }
}

fn ids_of(sc: &Scan, names: &[&str]) -> Vec<String> {
    names.iter().map(|n| server(sc, n).id.clone()).collect()
}

#[test]
fn importing_without_secrets_leaves_placeholders_and_saves_nothing_secret() {
    sandbox("nosec", |dir| {
        let fx = Fixture::new("nosec");
        let sc = fx.scan();
        let ids = ids_of(&sc, &["github", "sentry", "postgres", "zapier", "slack", "old"]);
        let out = apply_scan(&sc, &Request { select: ids, ..Default::default() }).unwrap();
        assert_eq!(out["servers"]["added"].as_array().unwrap().len(), 6, "{out}");
        let saved = mcp_client::load_servers();
        let gh = saved.iter().find(|s| s.name == "github").unwrap();
        assert_eq!(gh.env.get("GITHUB_PERSONAL_ACCESS_TOKEN").map(String::as_str), Some(""));
        assert_eq!(gh.env.get("LOG_LEVEL").map(String::as_str), Some("info"));
        let sentry = saved.iter().find(|s| s.name == "sentry").unwrap();
        assert!(!sentry.headers.contains_key("Authorization") && sentry.headers.get("X-Region").map(String::as_str) == Some("eu"));
        assert!(!saved.iter().find(|s| s.name == "old").unwrap().enabled, "a disabled server stays off");
        // not a single secret on disk: not in the server list, not in the undo log, not in the key file
        let mut all = String::new();
        for e in walk(dir) {
            all.push_str(&std::fs::read_to_string(e).unwrap_or_default());
        }
        for s in ALL_SECRETS {
            assert!(!all.contains(s), "{s} was written to disk");
        }
        // what is still to fill in is named
        assert!(out["needs"].to_string().contains("GITHUB_PERSONAL_ACCESS_TOKEN") && out["needs"].to_string().contains("Authorization"));
    });
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(walk(&p));
        } else {
            out.push(p);
        }
    }
    out
}

#[test]
fn keys_are_copied_only_with_the_tick_and_the_names_typed_back() {
    sandbox("sec", |_| {
        let fx = Fixture::new("sec");
        let sc = fx.scan();
        let ids = ids_of(&sc, &["github", "sentry"]);
        let mut r = Request { select: ids.clone(), secrets: ids_of(&sc, &["github"]), ..Default::default() };
        // no confirmation, and the wrong one
        assert!(apply_scan(&sc, &r).unwrap_err().contains("Confirm by name"));
        r.confirm = vec!["sentry".into()];
        assert!(apply_scan(&sc, &r).unwrap_err().contains("github"));
        assert!(mcp_client::load_servers().is_empty(), "nothing was saved by the refused tries");
        // keys for a server that is not ticked
        let bad = Request { select: ids_of(&sc, &["sentry"]), secrets: ids_of(&sc, &["github"]), confirm: vec!["github".into()], ..Default::default() };
        assert!(apply_scan(&sc, &bad).is_err());
        // the right way
        r.confirm = vec!["github".into()];
        let out = apply_scan(&sc, &r).unwrap();
        assert_eq!(out["secrets_copied"][0]["server"], "github", "{out}");
        let saved = mcp_client::load_servers();
        let gh = saved.iter().find(|s| s.name == "github").unwrap();
        assert_eq!(gh.secret_env, vec!["GITHUB_PERSONAL_ACCESS_TOKEN".to_string()]);
        assert!(!gh.env.contains_key("GITHUB_PERSONAL_ACCESS_TOKEN"), "the value is not in the settings file");
        assert_eq!(crate::ai::keys::get(&mcp_client::env_id("github", "GITHUB_PERSONAL_ACCESS_TOKEN")).as_deref(), Some(GH_TOKEN));
        // sentry was not given its key
        let sentry = saved.iter().find(|s| s.name == "sentry").unwrap();
        assert!(sentry.secret_headers.is_empty() && crate::ai::keys::get(&mcp_client::header_id("sentry", "Authorization")).is_none());
        // the settings file is private and holds no key
        let file = std::fs::read_to_string(crate::audit::config_dir().join("mcp-servers.json")).unwrap();
        assert!(!file.contains(GH_TOKEN) && !file.contains(SENTRY));
        // undo takes the key out of the store too
        let batch = out["batch"].as_str().unwrap().to_string();
        undo(&batch).unwrap();
        assert!(mcp_client::load_servers().is_empty());
        assert!(crate::ai::keys::get(&mcp_client::env_id("github", "GITHUB_PERSONAL_ACCESS_TOKEN")).is_none());
    });
}

#[test]
fn header_keys_go_to_the_key_store_and_are_sent_to_the_server() {
    sandbox("hdr", |_| {
        let fx = Fixture::new("hdr");
        let sc = fx.scan();
        let id = server(&sc, "sentry").id.clone();
        apply_scan(&sc, &Request { select: vec![id.clone()], secrets: vec![id], confirm: vec!["sentry".into()], ..Default::default() }).unwrap();
        let s = mcp_client::load_servers().into_iter().find(|s| s.name == "sentry").unwrap();
        assert_eq!(s.secret_headers, vec!["Authorization".to_string()]);
        assert!(s.headers.get("Authorization").is_none() && s.headers.get("X-Region").map(String::as_str) == Some("eu"));
        assert_eq!(crate::ai::keys::get(&mcp_client::header_id("sentry", "Authorization")).unwrap(), format!("Bearer {SENTRY}"));
    });
}

#[test]
fn nothing_is_overwritten_and_duplicates_are_skipped() {
    sandbox("dup", |_| {
        let fx = Fixture::new("dup");
        let sc = fx.scan();
        let mut mine = ServerConfig { name: "github".into(), command: "my-own-github".into(), ..Default::default() };
        mine.env.insert("X".into(), "1".into());
        mcp_client::save_servers(&[mine.clone()]);
        let ids = ids_of(&sc, &["github", "linear"]);
        let out = apply_scan(&sc, &Request { select: ids.clone(), ..Default::default() }).unwrap();
        assert_eq!(out["servers"]["added"], json!(["linear"]));
        assert!(out["servers"]["skipped"][0]["why"].as_str().unwrap().contains("already added"));
        assert_eq!(mcp_client::load_servers().iter().find(|s| s.name == "github").unwrap().command, "my-own-github");
        // the same again: the same server is not added twice (by name, or by command under another name)
        let again = apply_scan(&sc, &Request { select: ids, ..Default::default() }).unwrap();
        assert_eq!(again["servers"]["added"], json!([]));
        assert_eq!(mcp_client::load_servers().len(), 2);
        // the view says which ones are here already
        let v = sc.view();
        let dup = |n: &str| v["servers"].as_array().unwrap().iter().find(|s| s["name"] == n).unwrap()["duplicate"].clone();
        assert_eq!((dup("github"), dup("linear"), dup("sentry")), (json!("here"), json!("here"), json!("")));
    });
}

#[test]
fn undo_takes_back_servers_notes_and_instruction_text_but_leaves_what_was_changed() {
    sandbox("undo", |_| {
        let fx = Fixture::new("undo");
        let sc = fx.scan();
        let mut st = projects::Store::load();
        let pid = st.add("webapp".into(), "the web app".into());
        st.get_mut(&pid).unwrap().instructions = "Be careful.".into();
        st.save();
        let global = sc.instructions.iter().find(|i| i.tool == "claude-code" && i.folder.is_empty()).unwrap().id.clone();
        let proj = sc.instructions.iter().find(|i| i.tool == "claude-code" && !i.folder.is_empty()).unwrap().id.clone();
        let mut r = Request { select: [ids_of(&sc, &["linear", "time"]), vec![global, proj.clone()]].concat(), ..Default::default() };
        r.targets.insert(proj, format!("instruction:{pid}"));
        let out = apply_scan(&sc, &r).unwrap();
        assert_eq!(out["servers"]["added"].as_array().unwrap().len(), 2, "{out}");
        assert!(out["notes"]["added"].as_u64().unwrap() >= 3, "{out}");
        assert_eq!(out["instructions"]["appended"][0]["project"], "webapp");
        let instr = projects::Store::load().get(&pid).unwrap().instructions.clone();
        assert!(instr.starts_with("Be careful.") && instr.contains("pnpm, never npm"), "{instr}");
        // the history lists it
        assert_eq!(history()["batches"][0]["id"], out["batch"]);
        // the person edits the server "time" meanwhile: undo leaves it
        mcp_client::update_servers(|all| all.iter_mut().find(|s| s.name == "time").unwrap().command = "uvx other".into());
        let batch = out["batch"].as_str().unwrap().to_string();
        let u = undo(&batch).unwrap();
        assert!(u["message"].as_str().unwrap().contains("Left alone") && u["message"].as_str().unwrap().contains("time"), "{u}");
        let left: Vec<String> = mcp_client::load_servers().into_iter().map(|s| s.name).collect();
        assert_eq!(left, vec!["time".to_string()]);
        assert_eq!(projects::Store::load().get(&pid).unwrap().instructions, "Be careful.");
        assert!(memory::load().items.iter().all(|i| !i.text.contains("Prefer Rust")), "the notes went");
        assert!(undo(&batch).is_err(), "an import is undone once");
    });
}

#[test]
fn instruction_files_become_notes_without_code_headings_or_secrets() {
    sandbox("notes", |_| {
        let fx = Fixture::new("notes");
        let sc = fx.scan();
        let global = sc.instructions.iter().find(|i| i.tool == "claude-code" && i.folder.is_empty()).unwrap();
        let (notes, secret, _) = parse::instruction_notes(&global.text, MAX_NOTE_CHARS, MAX_NOTES_PER_FILE);
        assert_eq!(notes, vec!["Always answer in short sentences.", "Prefer Rust for command line tools.", "Run the tests before saying a task is done."]);
        assert_eq!(secret, 1, "the line with a deploy key is left out");
        let out = apply_scan(&sc, &req(&[&global.id])).unwrap();
        assert_eq!(out["notes"]["added"], 3);
        let items = memory::load().items;
        assert!(items.iter().any(|i| i.text == "Prefer Rust for command line tools." && i.kind == "preference" && i.scope.is_empty()));
        assert!(items.iter().all(|i| !i.text.contains("sk-live")));
        // the same file again adds nothing
        let again = apply_scan(&sc, &req(&[&global.id])).unwrap();
        assert_eq!((again["notes"]["added"].clone(), again["notes"]["already"].clone()), (json!(0), json!(3)));
        // the "Memory updated" feed lists the import
        assert!(memory_log::list(0, 50).iter().any(|c| c.source == "import"));
    });
}

#[test]
fn a_project_instruction_is_extended_not_replaced_and_secret_lines_stay_out() {
    sandbox("instr", |_| {
        let fx = Fixture::new("instr");
        let sc = fx.scan();
        let global = sc.instructions.iter().find(|i| i.tool == "claude-code" && i.folder.is_empty()).unwrap().id.clone();
        let mut st = projects::Store::load();
        let pid = st.add("Rules".into(), String::new());
        st.save();
        let mut r = req(&[&global]);
        r.targets.insert(global.clone(), format!("instruction:{pid}"));
        let out = apply_scan(&sc, &r).unwrap();
        assert_eq!(out["instructions"]["appended"][0]["lines_left_out"], 1);
        let text = projects::Store::load().get(&pid).unwrap().instructions.clone();
        assert!(text.contains("Always answer in short sentences.") && !text.contains("sk-live"), "{text}");
        // the same text is not appended twice
        let again = apply_scan(&sc, &r).unwrap();
        assert!(again["instructions"]["skipped"][0]["why"].as_str().unwrap().contains("already"));
        // a project that does not exist
        r.targets.insert(global, "instruction:nope".into());
        assert!(apply_scan(&sc, &r).unwrap()["instructions"]["skipped"][0]["why"].as_str().unwrap().contains("no such project"));
    });
}

#[test]
fn unknown_or_empty_selections_are_refused() {
    sandbox("bad", |_| {
        let fx = Fixture::new("bad");
        let sc = fx.scan();
        assert!(apply_scan(&sc, &req(&[])).is_err());
        assert!(apply_scan(&sc, &req(&["s0000000000"])).unwrap_err().contains("not in what was found"));
        let r = Request::from_json(&json!({"select": ["a"], "targets": {"a": "notes\n"}}));
        assert!(r.is_err());
    });
}

// ---- who may do what ----

#[test]
fn a_restricted_account_can_import_notes_only_and_is_told_so() {
    let _g = TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let t = crate::tenant::testkit::fresh(true);
    crate::tenant::test_multi(true);
    let fx = Fixture::new("restr");
    let given = vec![("CLAUDE.md".to_string(), "# Rules\n\n- Always answer in short sentences.\n".to_string()), ("claude_desktop_config.json".to_string(), r#"{"mcpServers": {"x": {"url": "https://x.example/mcp"}}}"#.to_string())];
    crate::tenant::scope(&t, || {
        let sc = scan_given(&given).unwrap();
        let v = sc.view();
        assert_eq!((v["can"]["servers"].clone(), v["can"]["projects"].clone(), v["can"]["secrets"].clone()), (json!(false), json!(false), json!(false)));
        assert_eq!(v["projects"], json!([]));
        // a server is refused outright
        let sid = sc.servers[0].id.clone();
        assert!(apply_scan(&sc, &req(&[&sid])).unwrap_err().contains("not available"));
        // notes are allowed, in the person's own memory
        let iid = sc.instructions[0].id.clone();
        assert_eq!(apply_scan(&sc, &req(&[&iid])).unwrap()["notes"]["added"], 1);
        // a project target is refused
        let mut r = req(&[&iid]);
        r.targets.insert(iid.clone(), "instruction:p1".into());
        assert!(apply_scan(&sc, &r).unwrap()["instructions"]["skipped"][0]["why"].as_str().unwrap().contains("no such project"));
        // the server's own disk and machines are not reachable
        for src in [json!({"source": "computer"}), json!({"source": "machine", "machine": "vm"})] {
            assert!(Origin::from_request(&src).is_err());
        }
        assert!(Origin::from_request(&json!({"source": "paste", "files": [{"name": "a.md", "text": "x"}]})).is_ok());
    });
    // another person's memory is untouched
    let other = crate::tenant::testkit::fresh(true);
    crate::tenant::scope(&other, || assert!(memory::load().items.iter().all(|i| !i.text.contains("short sentences"))));
    crate::tenant::test_multi(false);
    drop(fx);
}

#[test]
fn an_unrestricted_server_account_cannot_import_programs_or_keys() {
    let _g = TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let t = crate::tenant::testkit::fresh(false);
    crate::tenant::test_multi(true);
    let given = vec![("mcp.json".to_string(), r#"{"servers": {"local": {"command": "npx", "args": ["x"]}, "remote": {"type": "http", "url": "https://r.example/mcp", "headers": {"Authorization": "Bearer abc"}}}}"#.to_string())];
    crate::tenant::scope(&t, || {
        let sc = scan_given(&given).unwrap();
        let ids: Vec<String> = sc.servers.iter().map(|s| s.id.clone()).collect();
        let out = apply_scan(&sc, &Request { select: ids.clone(), ..Default::default() }).unwrap();
        assert_eq!(out["servers"]["added"], json!(["remote"]), "{out}");
        assert!(out["servers"]["skipped"][0]["why"].as_str().unwrap().contains("desktop app"));
        let r = Request { select: ids.clone(), secrets: vec![ids[1].clone()], confirm: vec!["remote".into()], ..Default::default() };
        assert!(apply_scan(&sc, &r).unwrap_err().contains("not copied to the cloud server"));
    });
    crate::tenant::test_multi(false);
}

// ---- pasted files ----

#[test]
fn pasted_files_are_recognised_by_name_and_by_content() {
    let sc = scan_given(&[
        ("claude_desktop_config.json".into(), r#"{"mcpServers": {"a": {"command": "x"}}}"#.into()),
        ("config.toml".into(), "[mcp_servers.b]\ncommand = \"y\"\n".into()),
        ("settings.json".into(), r#"{"context_servers": {"c": {"command": {"path": "z"}}}}"#.into()),
        ("CLAUDE.md".into(), "# t\n\nKeep answers brief, please.\n".into()),
        ("notes.txt".into(), "Something else that is a long enough line.".into()),
        ("empty.json".into(), r#"{"theme": "dark"}"#.into()),
        ("broken.json".into(), "{nope".into()),
    ])
    .unwrap();
    let tools: Vec<&str> = sc.servers.iter().map(|s| s.tool).collect();
    assert_eq!(tools, vec!["claude-desktop", "codex", "zed"]);
    assert_eq!(sc.instructions.len(), 2);
    assert_eq!(sc.notes.len(), 2, "files with nothing to import are reported: {:?}", sc.notes);
    assert!(scan_given(&[]).is_err());
    // the website names an unnamed paste "pasted.txt": JSON in it is still read as JSON, and no key shows in a preview
    let sc = scan_given(&[("pasted.txt".into(), r#"{"mcpServers":{"g":{"command":"npx","env":{"GITHUB_PERSONAL_ACCESS_TOKEN":"ghp_FAKEFAKEFAKEFAKEFAKEFAKE1234"}}}}"#.into())]).unwrap();
    assert_eq!(sc.servers.len(), 1, "{:?}", sc.notes);
    assert!(secret_line(r#"{"k":"ghp_FAKEFAKEFAKEFAKEFAKEFAKE1234"}"#));
    assert!(!secret_line("Prefer small commits and clear names."));
}

#[test]
fn paste_requests_are_limited() {
    let big = "x".repeat(2 * 1024 * 1024 + 1);
    assert!(Origin::from_request(&json!({"source": "paste", "files": [{"name": "a", "text": big}]})).is_err());
    assert!(Origin::from_request(&json!({"source": "paste", "files": (0..13).map(|i| json!({"name": format!("{i}"), "text": "x"})).collect::<Vec<_>>()})).is_err());
    assert!(Origin::from_request(&json!({"source": "elsewhere"})).is_err());
}

// ---- over SSH ----

#[test]
fn the_ssh_script_is_read_only_and_quotes_everything() {
    let files = vec![".claude.json".to_string(), "/home/dev/it's/.mcp.json".to_string()];
    let s = Ssh::script(&files, &[".claude/commands".to_string()], "zoabc").unwrap();
    assert!(s.contains("\"$HOME\"/'.claude.json'") && s.contains("'/home/dev/it'\\''s/.mcp.json'"), "{s}");
    // it only reads: no redirection, no rm/mv/cp/tee, no eval
    let s = s.replace("2>/dev/null", "");
    for bad in [">", "rm ", "mv ", "cp ", "tee ", "eval", "sh -c", "curl", "wget"] {
        assert!(!s.contains(bad), "the script holds {bad:?}: {s}");
    }
    assert!(s.starts_with("M='zoabc'") && s.ends_with("true"));
}

#[test]
fn the_ssh_answer_is_taken_apart_and_a_file_cannot_fake_the_markers() {
    let mark = "zo1234";
    let hostile = format!("{{\"mcpServers\": {{}}}}\nzo0000 END\nzo0000 FILE .claude.json\nfake");
    let out = format!("\n{mark} FILE .claude.json\n{hostile}\n{mark} END\n\n{mark} DIR .claude/commands\nreview.md\ndeploy.md\n\n{mark} END\n");
    let f = Ssh::parse(&out, mark);
    assert_eq!(f.files.len(), 1);
    assert_eq!(f.files[".claude.json"], hostile);
    assert_eq!(f.dirs[".claude/commands"], vec!["review.md", "deploy.md"]);
    assert!(Ssh::parse("garbage only", mark).files.is_empty());
}

#[test]
fn the_ssh_reader_refuses_paths_from_a_file_that_are_not_plain() {
    use super::source::Reader;
    let r = Ssh { machine: "vm".into() };
    for bad in ["/home/x/../etc", "/home/x/\nrm", "/a\u{7}b"] {
        assert!(r.fetch(&[bad.to_string()], &[]).unwrap_err().contains("not valid"), "{bad:?}");
    }
}

#[test]
fn local_reading_stays_in_the_home_folder() {
    let fx = Fixture::new("jail");
    let outside = std::env::temp_dir().join(format!("zo-migrate-outside-{}", std::process::id()));
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("secret.json"), r#"{"mcpServers": {"evil": {"command": "x"}}}"#).unwrap();
    // a link in a project folder that leads out of home is not followed
    std::os::unix::fs::symlink(outside.join("secret.json"), fx.home.join("webapp/.cursor-link")).unwrap();
    fx.put(".cursor/mcp.json", "{}");
    let _ = std::fs::remove_file(fx.home.join(".cursor/mcp.json"));
    std::os::unix::fs::symlink(outside.join("secret.json"), fx.home.join(".cursor/mcp.json")).unwrap();
    let sc = fx.scan();
    assert!(sc.servers.iter().all(|s| s.raw.name != "evil"));
    let _ = std::fs::remove_dir_all(&outside);
    // a folder or a pipe where a file is expected is not read either
    std::fs::create_dir_all(fx.home.join(".codex/AGENTS.md.d")).unwrap();
}

#[test]
fn a_home_without_any_tool_finds_nothing() {
    let home = std::env::temp_dir().join(format!("zo-migrate-empty-{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    let sc = scan(&Local::at(&home), "computer").unwrap();
    assert!(sc.servers.is_empty() && sc.instructions.is_empty() && sc.found.iter().all(|(_, f)| f.is_empty()));
    assert_eq!(sc.found.len(), TOOLS.len());
    let _ = std::fs::remove_dir_all(home);
}
