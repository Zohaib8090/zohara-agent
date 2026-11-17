//! The VM side: the `zohara-agent-runner` program. It has its own folder (`~/.local/share/zohara-agent-runner`, or `$ZOHARA_RUNNER_HOME`)
//! with its own settings, task list, model, key file and copied connected apps, so it never mixes with a Zohara Agent that may also be
//! installed on that machine.
//!
//! Every command prints ONE line of JSON (`{"ok":true,...}` or `{"ok":false,"error":"..."}`) and the owner's computer reads it over SSH.
//! Secrets arrive only on standard input (never in an argument), are written with mode 0600 and are never printed or logged.
//!
//! Commands: `version`, `status`, `install-units`, `uninstall`, `run-due`, `tasks-set`, `results`, `config-set`, `config-clear`,
//! `apps-set`, `apps-clear`, `apps-check`.

use crate::approval::Approver;
use crate::scheduler::{self, RunOpts};
use crate::store::schedules::{self, Task};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, OnceLock};

const MARKER: &str = ".zohara-runner";
const MAX_TASKS: usize = 200;
/// Results kept on the VM until the owner's computer has fetched them.
const RESULTS_KEEP: usize = 200;

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

/// The runner's own folder.
pub fn base() -> PathBuf {
    match std::env::var_os("ZOHARA_RUNNER_HOME").filter(|v| !v.is_empty()) {
        Some(p) => PathBuf::from(p),
        None => home().join(".local/share/zohara-agent-runner"),
    }
}

pub fn bin_path() -> PathBuf {
    home().join(super::BIN_REL)
}

fn keys_path() -> PathBuf {
    base().join("keys.json")
}

fn results_path() -> PathBuf {
    base().join("results.jsonl")
}

/// The name of the systemd unit pair (only a test changes it).
fn unit_name() -> String {
    let n = std::env::var("ZOHARA_RUNNER_UNIT").unwrap_or_default();
    if !n.is_empty() && n.len() < 60 && n.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')) {
        n
    } else {
        "zohara-agent-runner".into()
    }
}

static ORIG_CONFIG: OnceLock<PathBuf> = OnceLock::new();

fn unit_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("ZOHARA_RUNNER_UNIT_DIR").filter(|v| !v.is_empty()) {
        return PathBuf::from(d);
    }
    ORIG_CONFIG.get().cloned().unwrap_or_else(|| home().join(".config")).join("systemd/user")
}

fn no_systemd() -> bool {
    std::env::var_os("ZOHARA_RUNNER_NO_SYSTEMD").is_some_and(|v| !v.is_empty())
}

/// Points the shared code (settings, task list, key file) at the runner's own folder. Called once, first thing, before any thread.
pub fn setup_env() -> Result<(), String> {
    use std::os::unix::fs::DirBuilderExt;
    let b = base();
    let orig = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| home().join(".config"));
    let _ = ORIG_CONFIG.set(orig);
    for d in [b.clone(), b.join("cfg"), b.join("data")] {
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&d).map_err(|e| format!("cannot make {}: {e}", d.display()))?;
    }
    let _ = std::fs::write(b.join(MARKER), b"zohara-agent-runner\n");
    std::env::set_var("XDG_CONFIG_HOME", b.join("cfg"));
    std::env::set_var("XDG_DATA_HOME", b.join("data"));
    std::env::set_var("ZOHARA_AGENT_KEYS_FILE", keys_path());
    Ok(())
}

fn write_private(path: &Path, data: &[u8]) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;
    let tmp = path.with_extension("tmp");
    {
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp).map_err(|e| e.to_string())?;
        f.write_all(data).map_err(|e| e.to_string())?;
    }
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

fn read_keys() -> serde_json::Map<String, Value> {
    std::fs::read_to_string(keys_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn write_keys(m: &serde_json::Map<String, Value>) -> Result<(), String> {
    write_private(&keys_path(), Value::Object(m.clone()).to_string().as_bytes())
}

// ── systemd ──

fn systemctl(args: &[&str]) -> Result<String, String> {
    let mut c = Command::new("systemctl");
    c.arg("--user").args(args).stdin(Stdio::null());
    if std::env::var_os("XDG_RUNTIME_DIR").is_none() {
        // SAFETY: getuid cannot fail.
        let uid = unsafe { libc::getuid() };
        let dir = format!("/run/user/{uid}");
        if Path::new(&dir).exists() {
            c.env("XDG_RUNTIME_DIR", &dir);
        }
    }
    let out = c.output().map_err(|e| if e.kind() == std::io::ErrorKind::NotFound { "systemd is not available on this machine.".to_string() } else { e.to_string() })?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    if out.status.success() { Ok(text.trim().to_string()) } else { Err(text.trim().to_string()) }
}

fn unit_text(bin: &Path) -> (String, String) {
    let unit = unit_name();
    let mut env = format!("Environment=PATH={}/.local/bin:{}/.cargo/bin:/usr/local/bin:/usr/bin:/bin\n", home().display(), home().display());
    if let Some(h) = std::env::var_os("ZOHARA_RUNNER_HOME").filter(|v| !v.is_empty()) {
        env.push_str(&format!("Environment=ZOHARA_RUNNER_HOME={}\n", h.to_string_lossy()));
    }
    let service = format!(
        "[Unit]\nDescription=Zohara Agent runner: scheduled tasks on this machine\n\n[Service]\nType=oneshot\n{env}ExecStart={} run-due\nNice=10\nTimeoutStartSec=2h\n",
        bin.display()
    );
    let timer = format!(
        "[Unit]\nDescription=Look for Zohara Agent tasks that are due\n\n[Timer]\nOnCalendar=minutely\nPersistent=true\nAccuracySec=10s\nUnit={unit}.service\n\n[Install]\nWantedBy=timers.target\n"
    );
    (service, timer)
}

fn install_units() -> Result<Value, String> {
    let bin = bin_path();
    if !bin.exists() {
        return Err("The runner program is not in place yet.".into());
    }
    let (service, timer) = unit_text(&bin);
    let dir = unit_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot make {}: {e}", dir.display()))?;
    let unit = unit_name();
    std::fs::write(dir.join(format!("{unit}.service")), service).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(format!("{unit}.timer")), timer).map_err(|e| e.to_string())?;
    let mut linger = false;
    let mut note = String::new();
    if no_systemd() {
        note = "systemd was skipped (ZOHARA_RUNNER_NO_SYSTEMD).".into();
    } else {
        systemctl(&["daemon-reload"]).map_err(|e| format!("systemd refused: {e}"))?;
        systemctl(&["enable", "--now", &format!("{unit}.timer")]).map_err(|e| format!("could not start the timer: {e}"))?;
        // so that it keeps running while nobody is logged in; the person's own account is allowed to do this on most systems
        linger = std::env::var_os("ZOHARA_RUNNER_NO_LINGER").is_none() && Command::new("loginctl").arg("enable-linger").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success());
        if !linger {
            note = "The timer runs while you are logged in. To keep it running after you log out, run: sudo loginctl enable-linger $USER".into();
        }
    }
    Ok(json!({"ok": true, "linger": linger, "note": note}))
}

fn uninstall() -> Result<Value, String> {
    let unit = unit_name();
    if !no_systemd() {
        let _ = systemctl(&["disable", "--now", &format!("{unit}.timer")]);
        let _ = systemctl(&["stop", &format!("{unit}.service")]);
    }
    let dir = unit_dir();
    let mut removed = Vec::new();
    for f in [format!("{unit}.service"), format!("{unit}.timer")] {
        if std::fs::remove_file(dir.join(&f)).is_ok() {
            removed.push(f);
        }
    }
    if !no_systemd() {
        let _ = systemctl(&["daemon-reload"]);
    }
    // the runner's folder (task list, model, key file, copied apps, results) goes too, but only a folder that is marked as ours
    let b = base();
    if b.join(MARKER).exists() {
        std::fs::remove_dir_all(&b).map_err(|e| format!("could not remove {}: {e}", b.display()))?;
        removed.push("data".into());
    }
    if std::fs::remove_file(bin_path()).is_ok() {
        removed.push("program".into());
    }
    Ok(json!({"ok": true, "removed": removed}))
}

// ── status ──

fn last_result() -> Option<Value> {
    std::fs::read_to_string(results_path()).ok()?.lines().rev().find_map(|l| serde_json::from_str::<Value>(l).ok())
}

fn status() -> Result<Value, String> {
    let tasks = schedules::load();
    let enabled: Vec<&Task> = tasks.iter().filter(|t| t.enabled).collect();
    let sel = crate::ai::Selection::load();
    let provider = crate::ai::providers::find(&sel.provider);
    let has_key = provider.is_some_and(|p| p.local || crate::ai::profiles::get_key(&sel, p.id).is_some());
    let (mut timer_enabled, mut timer_active) = (Value::Null, Value::Null);
    if !no_systemd() {
        let unit = format!("{}.timer", unit_name());
        timer_enabled = json!(systemctl(&["is-enabled", &unit]).is_ok_and(|s| s.trim() == "enabled"));
        timer_active = json!(systemctl(&["is-active", &unit]).is_ok_and(|s| s.trim() == "active"));
    }
    let last = last_result();
    Ok(json!({
        "ok": true, "installed": true, "version": super::VERSION, "os": std::env::consts::OS, "arch": std::env::consts::ARCH,
        "timer_enabled": timer_enabled, "timer_active": timer_active,
        "tasks": tasks.len(), "enabled_tasks": enabled.len(), "next_run": enabled.iter().map(|t| t.next_run).min(),
        "last_run": last.as_ref().and_then(|l| l["time"].as_u64()), "last_status": last.as_ref().and_then(|l| l["status"].as_str().map(str::to_string)),
        "apps": crate::mcp_client::load_servers().len(),
        "model": if sel.provider.is_empty() { String::new() } else { format!("{}{}{}", sel.provider, if sel.model.is_empty() { "" } else { " / " }, sel.model) },
        "has_model": !sel.provider.is_empty(), "has_key": has_key,
    }))
}

// ── running what is due ──

/// What the runner may do without asking: the tools its owner named for this machine. Everything else that would ask is refused,
/// because nobody is there to answer.
static ALLOW: Mutex<Vec<String>> = Mutex::new(Vec::new());

struct RunnerApprover;

impl Approver for RunnerApprover {
    fn approve(&self, tool: &str, _summary: &str) -> bool {
        ALLOW.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|t| t == tool)
    }
}

fn allow_path() -> PathBuf {
    base().join("allow.json")
}

fn allowed_tools() -> Vec<String> {
    std::fs::read_to_string(allow_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

/// Tools a person may let the runner use without asking. Removing files, stopping processes and anything else is never on the list.
pub const ALLOWABLE: [&str; 4] = ["zohara_shell", "zohara_fs_write", "zohara_fs_mkdir", "zohara_ai_add_memory"];

fn cut(s: &str, max: usize) -> String {
    if s.chars().count() > max { format!("{}…", s.chars().take(max.saturating_sub(1)).collect::<String>()) } else { s.to_string() }
}

fn append_result(v: &Value) {
    let p = results_path();
    let mut lines: Vec<String> = std::fs::read_to_string(&p).unwrap_or_default().lines().map(str::to_string).collect();
    lines.push(v.to_string());
    if lines.len() > RESULTS_KEEP {
        let n = lines.len() - RESULTS_KEEP;
        lines.drain(..n);
    }
    let _ = write_private(&p, format!("{}\n", lines.join("\n")).as_bytes());
}

fn run_due() -> Result<Value, String> {
    use std::os::unix::io::AsRawFd;
    // one run at a time: the timer may fire again while a long task is still going
    let lock = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(base().join("run.lock")).map_err(|e| e.to_string())?;
    // SAFETY: flock on a descriptor we own; released when `lock` is dropped.
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Ok(json!({"ok": true, "ran": 0, "busy": true}));
    }
    *ALLOW.lock().unwrap_or_else(|e| e.into_inner()) = allowed_tools();
    let cancel = Arc::new(AtomicBool::new(false));
    let now = schedules::now_millis();
    let mut ran = 0;
    for task in schedules::due(now) {
        if scheduler::is_running(&task.id) {
            continue;
        }
        let how = if task.run_requested { "manual" } else { "scheduled" };
        let o = RunOpts { test: false, how, missed: schedules::missed_time(&task, now), approver: &RunnerApprover, cancel: &cancel, source: "runner" };
        let Some(out) = scheduler::run_and_record(&task, &o, &mut |_| {}) else { continue };
        let entries: Vec<Value> = out.chat.as_ref().map(|c| c.entries.iter().take(60).map(|e| json!({"kind": e.kind, "text": cut(&e.text, 1200)})).collect()).unwrap_or_default();
        append_result(&json!({
            "task": task.id, "title": task.title, "time": schedules::now_millis(), "status": out.status, "summary": cut(&out.summary, 4000),
            "tools": out.tools, "ms": out.ms, "how": how, "missed": o.missed, "kind": task.kind(), "entries": entries,
        }));
        ran += 1;
    }
    Ok(json!({"ok": true, "ran": ran}))
}

fn results(args: &[String]) -> Result<Value, String> {
    let after = args.iter().position(|a| a == "--after").and_then(|i| args.get(i + 1)).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
    let all: Vec<Value> = std::fs::read_to_string(results_path()).unwrap_or_default().lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let list: Vec<Value> = all.into_iter().filter(|r| r["time"].as_u64().unwrap_or(0) > after).take(40).collect();
    Ok(json!({"ok": true, "results": list}))
}

// ── what the owner's computer sends ──

fn read_stdin() -> Result<Value, String> {
    let mut s = String::new();
    std::io::stdin().take(super::MAX_STDIN as u64 + 1).read_to_string(&mut s).map_err(|e| e.to_string())?;
    if s.len() > super::MAX_STDIN {
        return Err("The request is too large.".into());
    }
    serde_json::from_str(&s).map_err(|_| "The request is not valid JSON.".to_string())
}

/// A task as the runner keeps it: it runs here ("this"), asks nothing of anybody, never in Bypass. The run state (next time, last result,
/// history) stays the runner's own when it is ahead of the copy it was sent.
pub fn clean_task(mut t: Task, old: Option<&Task>) -> Result<Task, String> {
    if t.id.is_empty() || t.id.len() > 64 || !t.id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':' | '.')) {
        return Err("A task has a bad id.".into());
    }
    if t.prompt.trim().is_empty() || t.prompt.chars().count() > schedules::MAX_PROMPT_CHARS {
        return Err("A task has no text, or too much.".into());
    }
    t.title = cut(&t.title, schedules::MAX_TITLE_CHARS);
    t.run_on = "this".into();
    t.project = None;
    t.mode = Some(if t.mode.as_deref() == Some("auto") { "auto".into() } else { "manual".into() });
    if t.kind == "browser" {
        return Err(format!("The task \"{}\" is a browser task: a runner has no browser.", t.title));
    }
    if let Some(o) = old {
        let same = o.repeat == t.repeat && o.tz == t.tz && o.day == t.day;
        if same && o.last_run.is_some() && o.next_run > t.next_run {
            t.next_run = o.next_run;
            t.enabled = t.enabled && o.enabled;
        }
        if same && o.last_run.is_some() {
            t.last_run = o.last_run;
            t.last_chat = o.last_chat.clone();
            t.last_status = o.last_status.clone();
            t.last_output = o.last_output.clone();
            t.history = o.history.clone();
        }
    }
    Ok(t)
}

fn tasks_set(v: &Value) -> Result<Value, String> {
    let list = v["tasks"].as_array().ok_or("There is no task list.")?;
    if list.len() > MAX_TASKS {
        return Err("Too many tasks.".into());
    }
    let mut incoming: Vec<Task> = Vec::new();
    for t in list {
        incoming.push(serde_json::from_value(t.clone()).map_err(|e| format!("A task is not valid: {e}"))?);
    }
    let old = schedules::load();
    let mut out = Vec::new();
    for t in incoming {
        let o = old.iter().find(|o| o.id == t.id);
        out.push(clean_task(t, o)?);
    }
    let n = out.len();
    schedules::replace_all(|_| out);
    let allow: Vec<String> = v["allow"].as_array().map(|a| a.iter().filter_map(Value::as_str).filter(|t| ALLOWABLE.contains(t)).map(str::to_string).collect()).unwrap_or_default();
    write_private(&allow_path(), serde_json::to_string(&allow).map_err(|e| e.to_string())?.as_bytes())?;
    Ok(json!({"ok": true, "tasks": n, "allow": allow}))
}

fn config_set(v: &Value) -> Result<Value, String> {
    let mut sel: crate::ai::Selection = serde_json::from_value(v["selection"].clone()).map_err(|_| "The model choice is not valid.".to_string())?;
    if sel.provider == "claude-code" {
        return Err("Claude Code needs its own login on the machine; choose a model that works with an API key.".into());
    }
    let p = crate::ai::providers::find(&sel.provider).ok_or("That provider is not known here.")?;
    sel.profile = None;
    sel.base_urls.retain(|k, _| k == p.id);
    sel.api_formats.retain(|k, _| k == p.id);
    let mut keys = serde_json::Map::new();
    if let Some(m) = v["keys"].as_object() {
        for (id, val) in m {
            let val = val.as_str().unwrap_or("");
            if crate::ai::providers::find(id).is_none() || val.is_empty() || val.len() > 4096 || val.chars().any(char::is_control) {
                return Err("A key in the request is not valid.".into());
            }
            keys.insert(id.clone(), json!(val));
        }
    }
    sel.save();
    let mut all = read_keys();
    for (k, val) in keys {
        all.insert(k, val);
    }
    write_keys(&all)?;
    Ok(json!({"ok": true}))
}

fn config_clear() -> Result<Value, String> {
    let mut all = read_keys();
    let ids: Vec<String> = all.keys().filter(|k| !k.starts_with("mcp-")).cloned().collect();
    for k in ids {
        all.remove(&k);
    }
    write_keys(&all)?;
    crate::ai::Selection::default().save();
    Ok(json!({"ok": true}))
}

fn apps_set(v: &Value) -> Result<Value, String> {
    use crate::mcp_client::{self, ServerConfig};
    let list = v["servers"].as_array().ok_or("There is no server list.")?;
    if list.len() > 64 {
        return Err("Too many servers.".into());
    }
    let mut incoming: Vec<ServerConfig> = Vec::new();
    for s in list {
        let mut c: ServerConfig = serde_json::from_value(s.clone()).map_err(|e| format!("A server is not valid: {e}"))?;
        if c.name.trim().is_empty() || c.command.trim().is_empty() {
            return Err("A server has no name or no address.".into());
        }
        c.held = false;
        c.registry = None;
        c.address_changed = None;
        incoming.push(c);
    }
    let mut keys = serde_json::Map::new();
    if let Some(m) = v["keys"].as_object() {
        for (id, val) in m {
            let val = val.as_str().unwrap_or("");
            if !id.starts_with("mcp-") || id.len() > 160 || val.is_empty() || val.len() > 20_000 {
                return Err("A secret in the request is not valid.".into());
            }
            keys.insert(id.clone(), json!(val));
        }
    }
    let mut all = if v["replace"].as_bool().unwrap_or(false) { Vec::new() } else { mcp_client::load_servers() };
    for c in incoming {
        all.retain(|x| x.name != c.name);
        all.push(c);
    }
    all.sort_by(|a, b| a.name.cmp(&b.name));
    let n = all.len();
    mcp_client::save_servers_as_is(&all);
    let mut ks = read_keys();
    if v["replace"].as_bool().unwrap_or(false) {
        ks.retain(|k, _| !k.starts_with("mcp-"));
    }
    for (k, val) in keys {
        ks.insert(k, val);
    }
    write_keys(&ks)?;
    Ok(json!({"ok": true, "servers": n}))
}

fn apps_clear() -> Result<Value, String> {
    let _ = std::fs::remove_file(crate::audit::config_dir().join("mcp-servers.json"));
    let mut ks = read_keys();
    ks.retain(|k, _| !k.starts_with("mcp-"));
    write_keys(&ks)?;
    Ok(json!({"ok": true}))
}

/// Is `program` found on this machine, the way the timer's service would find it?
pub fn program_present(program: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let ok = |p: &Path| p.is_file() && std::fs::metadata(p).is_ok_and(|m| m.permissions().mode() & 0o111 != 0);
    if program.contains('/') {
        return ok(Path::new(program));
    }
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    for d in [home().join(".local/bin"), home().join(".cargo/bin"), "/usr/local/bin".into(), "/usr/bin".into(), "/bin".into()] {
        dirs.push(d);
    }
    dirs.iter().any(|d| ok(&d.join(program)))
}

fn apps_check() -> Result<Value, String> {
    let list: Vec<Value> = crate::mcp_client::load_servers()
        .iter()
        .map(|s| {
            if crate::mcp_client::is_url(&s.command) {
                return json!({"name": s.name, "kind": "remote", "runtime": "", "present": true});
            }
            let program = crate::mcp_client::split_command(&s.command).into_iter().next().unwrap_or_default();
            json!({"name": s.name, "kind": "local", "runtime": program, "present": program_present(&program)})
        })
        .collect();
    Ok(json!({"ok": true, "servers": list}))
}

/// The entry point of the `zohara-agent-runner` program. Returns the exit code.
pub fn main(args: &[String]) -> i32 {
    let cmd = args.first().map(String::as_str).unwrap_or("");
    if matches!(cmd, "" | "help" | "--help" | "-h") {
        println!("zohara-agent-runner {}: runs Zohara Agent's scheduled tasks on this machine. It is driven by Zohara Agent over SSH.\nCommands: version status install-units uninstall run-due tasks-set results config-set config-clear apps-set apps-clear apps-check", super::VERSION);
        return 0;
    }
    if cmd == "version" || cmd == "--version" {
        println!("{}", json!({"ok": true, "version": super::VERSION, "os": std::env::consts::OS, "arch": std::env::consts::ARCH}));
        return 0;
    }
    let r = setup_env().and_then(|_| match cmd {
        "status" => status(),
        "install-units" => install_units(),
        "uninstall" => uninstall(),
        "run-due" => run_due(),
        "results" => results(&args[1..]),
        "tasks-set" => read_stdin().and_then(|v| tasks_set(&v)),
        "config-set" => read_stdin().and_then(|v| config_set(&v)),
        "config-clear" => config_clear(),
        "apps-set" => read_stdin().and_then(|v| apps_set(&v)),
        "apps-clear" => apps_clear(),
        "apps-check" => apps_check(),
        other => Err(format!("unknown command: {other}")),
    });
    match r {
        Ok(v) => {
            println!("{v}");
            0
        }
        Err(e) => {
            println!("{}", json!({"ok": false, "error": e}));
            1
        }
    }
}
