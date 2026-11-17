//! The owner's side of the runner: install and remove it on a VM over SSH, ask how it is, send it the task list and the model, and fetch
//! what it did. Everything goes through `tools::ssh` (an argument list, never a local shell, the key only in a temporary file), so the
//! same limits and the same trust-on-first-use host keys apply as for every other SSH call.

use super::{sha256_hex, BIN_REL};
use crate::store::chats::{self, Chat, Entry};
use crate::store::machines::{self, Machine};
use crate::store::schedules::{self, Finished, Task};
use crate::tools::ssh::{self, quote, remote_path};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// What `remote` says when the program is not on the machine.
pub const NOT_INSTALLED: &str = "The runner is not installed on this machine.";

/// The only text a person has to type to agree to sending a model key to a machine is the machine's name; see `model_confirmation`.
pub fn model_confirmation(machine: &str, provider_name: &str) -> String {
    format!("This copies your {provider_name} API key to {machine}.")
}

/// What the owner's computer knows about the runner of one machine.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct RunnerEntry {
    pub installed: u64,
    #[serde(default)]
    pub version: String,
    /// The time of the newest result that was fetched.
    #[serde(default)]
    pub pulled: u64,
    /// The last time the lists were synced (ms).
    #[serde(default)]
    pub last_sync: u64,
    /// What was last sent (a hash), so that nothing is sent again when nothing changed.
    #[serde(default)]
    pub sig: String,
    #[serde(default)]
    pub last_error: String,
    /// May the runner run commands / write files without asking? (Everything else that would ask is refused.)
    #[serde(default)]
    pub commands: bool,
    #[serde(default)]
    pub files: bool,
}

fn file() -> PathBuf {
    crate::audit::config_dir().join("runners.json")
}

static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub fn load() -> BTreeMap<String, RunnerEntry> {
    std::fs::read_to_string(file()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn change<R>(f: impl FnOnce(&mut BTreeMap<String, RunnerEntry>) -> R) -> R {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut m = load();
    let r = f(&mut m);
    let _ = std::fs::create_dir_all(crate::audit::config_dir());
    let tmp = file().with_extension("json.tmp");
    if let Ok(s) = serde_json::to_string_pretty(&m) {
        if std::fs::write(&tmp, s).is_ok() {
            let _ = std::fs::rename(&tmp, file());
        }
    }
    r
}

/// Is a runner installed on this machine (so that it, not this computer, runs the machine's tasks)?
pub fn has_runner(machine: &str) -> bool {
    load().contains_key(machine)
}

/// Forgets what is known about a machine's runner (the machine was removed).
pub fn forget(machine: &str) {
    change(|m| m.remove(machine));
}

// ── talking to the runner ──

fn remote_cmd(args: &[&str]) -> Result<String, String> {
    let mut s = remote_path(&format!("~/{BIN_REL}"))?;
    for a in args {
        s.push(' ');
        s.push_str(&quote(a));
    }
    Ok(s)
}

/// Runs a runner command on the machine and reads its one line of JSON. `Err` carries the runner's own error text.
pub fn call(name: &str, args: &[&str], stdin: Option<&[u8]>, timeout: u64) -> Result<Value, String> {
    let d = ssh::run_on(name, &remote_cmd(args)?, stdin, timeout, 4_000_000)?;
    if let Some(e) = ssh::failure(&d, name) {
        return Err(e);
    }
    if d.code == 127 {
        return Err(NOT_INSTALLED.into());
    }
    let last = d.stdout.lines().rev().find(|l| l.trim_start().starts_with('{')).unwrap_or("");
    let v: Value = serde_json::from_str(last).map_err(|_| {
        let why = d.stderr.trim();
        if why.is_empty() { "The runner answered with something that is not understood.".to_string() } else { format!("The runner could not start: {}", why.lines().last().unwrap_or("")) }
    })?;
    if v["ok"].as_bool() == Some(false) {
        return Err(v["error"].as_str().unwrap_or("The runner reported an error.").to_string());
    }
    Ok(v)
}

/// Is there such a machine? (For the other parts of the runner.)
pub fn status_exists(name: &str) -> Result<(), String> {
    must(name).map(|_| ())
}

fn must(name: &str) -> Result<Machine, String> {
    machines::get(name).ok_or_else(|| format!("There is no machine called \"{name}\"."))
}

/// Is the machine reachable, and what is its runner doing? One SSH call.
pub fn status(name: &str) -> Value {
    let t0 = std::time::Instant::now();
    let entry = load().get(name).cloned();
    let mut v = match call(name, &["status"], None, 30) {
        Ok(r) => json!({"reachable": true, "installed": true, "runner": r}),
        Err(e) if e == NOT_INSTALLED => json!({"reachable": true, "installed": false}),
        Err(e) => json!({"reachable": false, "installed": entry.is_some(), "error": e}),
    };
    v["name"] = json!(name);
    v["ms"] = json!(t0.elapsed().as_millis() as u64);
    if let Some(e) = entry {
        v["commands"] = json!(e.commands);
        v["files"] = json!(e.files);
        v["last_sync"] = json!(e.last_sync);
        v["sync_error"] = json!(e.last_error);
    }
    v["apps_copied"] = json!(super::apps::copied(name));
    v
}

// ── install and remove ──

/// Where the runner program is found on this computer.
pub fn find_binary() -> Result<PathBuf, String> {
    let mut tried = Vec::new();
    let mut cands: Vec<PathBuf> = Vec::new();
    if let Some(p) = std::env::var_os("ZOHARA_RUNNER_BINARY").filter(|v| !v.is_empty()) {
        cands.push(p.into());
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(d) = exe.parent() {
            cands.push(d.join("zohara-agent-runner"));
        }
    }
    for p in ["/usr/lib/zohara-agent/zohara-agent-runner", "/usr/bin/zohara-agent-runner", "/usr/local/bin/zohara-agent-runner"] {
        cands.push(p.into());
    }
    for c in cands {
        if c.is_file() {
            return Ok(c);
        }
        tried.push(c.display().to_string());
    }
    Err("The runner program (zohara-agent-runner) was not found on this computer. It is installed together with Zohara Agent; set ZOHARA_RUNNER_BINARY to its path if it is somewhere else.".into())
}

fn arch_of(uname_m: &str) -> &str {
    match uname_m.trim() {
        "arm64" | "aarch64" => "aarch64",
        "amd64" | "x86_64" => "x86_64",
        other => other,
    }
}

/// Installs (or updates) the runner on a machine: checks it, copies the program with a checksum, starts the systemd user timer and
/// sends the machine's tasks. Returns what was done, step by step.
pub fn install(name: &str) -> Result<Vec<String>, String> {
    let m = must(name)?;
    m.validate()?;
    let mut steps = Vec::new();
    let bin = find_binary()?;
    // 1. the machine: Linux, the same kind of processor, systemd for the user
    let d = ssh::run_on(name, "uname -sm; if [ -n \"$ZOHARA_RUNNER_NO_SYSTEMD\" ] || command -v systemctl >/dev/null 2>&1; then echo systemd; else echo nosystemd; fi", None, 30, 2_000)?;
    if let Some(e) = ssh::failure(&d, name) {
        return Err(e);
    }
    let mut lines = d.stdout.lines();
    let uname = lines.next().unwrap_or("").to_string();
    let (os, arch) = uname.split_once(' ').unwrap_or((&uname, ""));
    if os != "Linux" {
        return Err(format!("The runner is for Linux machines; this one says \"{uname}\"."));
    }
    if arch_of(arch) != std::env::consts::ARCH {
        return Err(format!("This machine has a {arch} processor, but the runner program on this computer is for {}. A runner for {arch} is needed.", std::env::consts::ARCH));
    }
    if lines.next() != Some("systemd") {
        return Err("This machine has no systemd, which the runner uses to wake itself up. Nothing was installed.".into());
    }
    steps.push(format!("Checked {name}: Linux, {arch}, systemd."));
    // 2. copy the program, check it, put it in place
    let data = std::fs::read(&bin).map_err(|e| format!("cannot read the runner program: {e}"))?;
    let sum = sha256_hex(&data);
    let tmp = remote_path(&format!("~/{BIN_REL}.new"))?;
    let dir = remote_path("~/.local/bin")?;
    let d = ssh::run_on(name, &format!("mkdir -p {dir} && cat > {tmp}"), Some(&data), 600, 2_000)?;
    if let Some(e) = ssh::failure(&d, name) {
        return Err(e);
    }
    if d.code != 0 {
        return Err(format!("Could not copy the runner: {}", d.stderr.trim()));
    }
    let d = ssh::run_on(name, &format!("sha256sum {tmp}"), None, 60, 2_000)?;
    let got = d.stdout.split_whitespace().next().unwrap_or("").to_string();
    if got != sum {
        let _ = ssh::run_on(name, &format!("rm -f {tmp}"), None, 30, 2_000);
        return Err("The copy on the machine does not match (the checksum is different), so it was not installed.".into());
    }
    let final_path = remote_path(&format!("~/{BIN_REL}"))?;
    let d = ssh::run_on(name, &format!("chmod 0755 {tmp} && mv -f {tmp} {final_path}"), None, 30, 2_000)?;
    if d.code != 0 {
        return Err(format!("Could not put the runner in place: {}", d.stderr.trim()));
    }
    steps.push(format!("Copied the runner ({} KB) and checked its checksum.", data.len() / 1024));
    // 3. it must start there (an old system can lack what the program needs)
    let v = call(name, &["version"], None, 30).map_err(|e| format!("The runner was copied but does not start on that machine: {e}"))?;
    let version = v["version"].as_str().unwrap_or("").to_string();
    // 4. the timer
    let u = call(name, &["install-units"], None, 60)?;
    steps.push("Started the systemd user timer (it looks for due tasks every minute).".into());
    if let Some(n) = u["note"].as_str().filter(|n| !n.is_empty()) {
        steps.push(n.to_string());
    }
    change(|r| {
        let e = r.entry(name.to_string()).or_default();
        e.installed = schedules::now_millis();
        e.version = version;
        e.sig.clear();
    });
    crate::audit::log("runner_install", &json!({"machine": name, "sha256": sum}), "ok");
    // 5. the tasks that belong to this machine
    match sync(name) {
        Ok(n) => steps.push(format!("Sent {n} task(s) to the runner.")),
        Err(e) => steps.push(format!("The runner is installed, but sending its tasks failed: {e}")),
    }
    Ok(steps)
}

/// Removes the runner from the machine: the timer, its folder (its tasks, key file and copied apps) and the program. What it had
/// done is fetched first. Tasks of that machine go back to being run from this computer.
pub fn uninstall(name: &str) -> Result<(), String> {
    must(name)?;
    let _ = pull(name);
    match call(name, &["uninstall"], None, 60) {
        Ok(_) => {}
        Err(e) if e == NOT_INSTALLED => {}
        Err(e) => return Err(e),
    }
    // the program may be gone already; this also removes a half-done copy
    let _ = ssh::run_on(name, &format!("rm -f {} {}", remote_path(&format!("~/{BIN_REL}"))?, remote_path(&format!("~/{BIN_REL}.new"))?), None, 30, 2_000);
    forget(name);
    super::apps::forget(name);
    crate::audit::log("runner_uninstall", &json!({"machine": name}), "ok");
    Ok(())
}

// ── what the runner may do, the model, the tasks ──

/// What the runner may do without asking.
pub fn set_allow(name: &str, commands: bool, files: bool) -> Result<(), String> {
    if !has_runner(name) {
        return Err(NOT_INSTALLED.into());
    }
    change(|r| {
        if let Some(e) = r.get_mut(name) {
            e.commands = commands;
            e.files = files;
            e.sig.clear();
        }
    });
    sync(name).map(|_| ())
}

fn allow_of(e: &RunnerEntry) -> Vec<&'static str> {
    let mut v = Vec::new();
    if e.commands {
        v.push("zohara_shell");
    }
    if e.files {
        v.extend(["zohara_fs_write", "zohara_fs_mkdir"]);
    }
    v
}

/// The tasks this machine's runner runs: the ones set to run on it, except browser tasks (it has no browser).
fn tasks_for(name: &str) -> Vec<Task> {
    let on = format!("machine:{name}");
    schedules::load().into_iter().filter(|t| t.run_on == on && t.kind() != "browser").collect()
}

/// Fetches what the runner did, then sends it the current tasks. Returns how many tasks were sent.
pub fn sync(name: &str) -> Result<usize, String> {
    let Some(entry) = load().get(name).cloned() else { return Err(NOT_INSTALLED.into()) };
    let r = (|| {
        pull(name)?;
        let tasks = tasks_for(name);
        let body = json!({"tasks": tasks, "allow": allow_of(&entry)});
        let sig = sha256_hex(body.to_string().as_bytes());
        let cur = load().get(name).cloned().unwrap_or_default();
        if cur.sig != sig {
            call(name, &["tasks-set"], Some(body.to_string().as_bytes()), 60)?;
            change(|r| {
                if let Some(e) = r.get_mut(name) {
                    e.sig = sig;
                }
            });
        }
        Ok(tasks.len())
    })();
    let err = r.as_ref().err().cloned().unwrap_or_default();
    change(|m| {
        if let Some(e) = m.get_mut(name) {
            e.last_sync = schedules::now_millis();
            e.last_error = err;
        }
    });
    r
}

/// Sends the model choice and the key of the chosen provider to the machine, for its tasks. Needs the sentence from
/// `model_confirmation`, which names the machine and the provider.
pub fn push_model(name: &str, confirm: &str) -> Result<String, String> {
    must(name)?;
    if !has_runner(name) {
        return Err(NOT_INSTALLED.into());
    }
    let sel = crate::ai::Selection::load();
    if sel.provider == "claude-code" {
        return Err("Claude Code cannot be copied: it needs its own login on the machine. Choose a model that works with an API key first.".into());
    }
    let p = crate::ai::providers::find(&sel.provider).ok_or("No model is chosen here yet.")?;
    let want = model_confirmation(name, p.name);
    if confirm != want {
        return Err(format!("Confirm exactly: {want}"));
    }
    let mut keys = serde_json::Map::new();
    if !p.local {
        let k = crate::ai::profiles::get_key(&sel, p.id).ok_or_else(|| format!("There is no API key for {} here to copy.", p.name))?;
        keys.insert(p.id.to_string(), json!(k));
    }
    let body = json!({"selection": sel, "keys": keys});
    drop(keys);
    call(name, &["config-set"], Some(body.to_string().as_bytes()), 60)?;
    // only the fact is logged, never the key
    crate::audit::log("runner_copy_model", &json!({"machine": name, "provider": p.id, "model": sel.model}), "ok");
    Ok(format!("{} ({}) is set on {name}.", p.name, if sel.model.is_empty() { "its default model" } else { &sel.model }))
}

/// Removes the model choice and key from the machine.
pub fn clear_model(name: &str) -> Result<(), String> {
    must(name)?;
    call(name, &["config-clear"], None, 60)?;
    crate::audit::log("runner_clear_model", &json!({"machine": name}), "ok");
    Ok(())
}

// ── results ──

fn entry_kind(k: &str) -> &'static str {
    match k {
        "msg-user" => "msg-user",
        "msg-ai" => "msg-ai",
        "msg-tool-out" => "msg-tool-out",
        "msg-error" => "msg-error",
        _ => "msg-tool",
    }
}

/// Fetches the results the runner has not given yet: each becomes a chat (so it can be read), a line in the task's history and a
/// notification. Returns how many.
pub fn pull(name: &str) -> Result<usize, String> {
    let after = load().get(name).map(|e| e.pulled).unwrap_or(0);
    let v = call(name, &["results", "--after", &after.to_string()], None, 120)?;
    let list = v["results"].as_array().cloned().unwrap_or_default();
    let mut newest = after;
    let mut n = 0;
    for r in &list {
        let time = r["time"].as_u64().unwrap_or(0);
        if time <= newest {
            continue;
        }
        newest = time;
        let id = r["task"].as_str().unwrap_or("");
        let Some(task) = schedules::get(id) else { continue };
        let status = match r["status"].as_str() {
            Some("ok") => "ok",
            Some("skipped") => "skipped",
            _ => "failed",
        };
        let summary = r["summary"].as_str().unwrap_or("").to_string();
        let mut entries = vec![Entry { kind: "msg-tool-out".into(), text: format!("This ran on {name}, by its runner, at its planned time."), ..Default::default() }];
        for e in r["entries"].as_array().cloned().unwrap_or_default() {
            entries.push(Entry { kind: entry_kind(e["kind"].as_str().unwrap_or("")).into(), text: e["text"].as_str().unwrap_or("").chars().take(1500).collect(), ..Default::default() });
        }
        let chat = Chat { id: format!("c{time}r"), title: format!("Scheduled on {name}: {}", task.title), updated: time, entries, ..Default::default() };
        let chat_id = chat.id.clone();
        chats::upsert(chat);
        let missed = r["missed"].as_u64();
        let how = if r["how"].as_str() == Some("manual") { "manual" } else { "scheduled" };
        schedules::finish(id, time, Finished { status: status.into(), summary: summary.clone(), chat: Some(chat_id.clone()), ms: r["ms"].as_u64().unwrap_or(0), missed, how: how.into() });
        let shown = task.title.clone();
        let notice = crate::scheduler::Notice {
            title: format!("{shown} {}", if status == "ok" { "finished" } else if status == "skipped" { "did not run" } else { "failed" }),
            body: summary.chars().take(200).collect(),
            kind: "task".into(),
            ok: status == "ok",
            chat: Some(chat_id),
        };
        let tenant = crate::tenant::current().map(|t| t.id).unwrap_or_default();
        crate::scheduler::notify(&tenant, &task, &notice);
        n += 1;
    }
    if newest > after {
        change(|r| {
            if let Some(e) = r.get_mut(name) {
                e.pulled = newest;
            }
        });
    }
    Ok(n)
}

/// Looks at every runner now and then (a few minutes apart), or at once when its tasks changed. Called from the desktop's timer; it
/// can take a while when a machine does not answer, so call it from a thread.
pub fn tick() {
    let now = schedules::now_millis();
    for (name, e) in load() {
        let changed = {
            let body = json!({"tasks": tasks_for(&name), "allow": allow_of(&e)});
            sha256_hex(body.to_string().as_bytes()) != e.sig
        };
        if changed || now.saturating_sub(e.last_sync) >= 5 * 60_000 {
            let _ = sync(&name);
        }
    }
}

/// "Run a task here": makes a one-time task on the machine that is due now. With a runner the runner is told to look at once and the
/// result comes back through `pull`; without one the caller runs it from this computer through the SSH tools.
pub fn run_here(name: &str, prompt: &str, title: &str) -> Result<(Task, bool), String> {
    must(name)?;
    let t = schedules::add_new(schedules::New {
        title: title.to_string(),
        prompt: prompt.to_string(),
        next_run: schedules::now_millis(),
        repeat: "once".into(),
        kind: "task".into(),
        run_on: format!("machine:{name}"),
        ..Default::default()
    })?;
    if !has_runner(name) {
        return Ok((t, false));
    }
    sync(name)?;
    trigger(name)?;
    Ok((t, true))
}

/// Tells the runner to look for due tasks right now (it does not wait for its timer). Returns at once.
pub fn trigger(name: &str) -> Result<(), String> {
    let d = ssh::run_on(name, &format!("setsid nohup {} run-due >/dev/null 2>&1 </dev/null &", remote_path(&format!("~/{BIN_REL}"))?), None, 30, 2_000)?;
    match ssh::failure(&d, name) {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// "Run now" for a task that a runner owns: the runner is asked to run it at once.
pub fn run_task_now(task_id: &str) -> Result<(), String> {
    let t = schedules::get(task_id).ok_or("That task is gone.")?;
    let name = t.machine().ok_or("That task is not on a machine.")?.to_string();
    if !has_runner(&name) {
        return Err(NOT_INSTALLED.into());
    }
    schedules::request_run(task_id);
    sync(&name)?;
    trigger(&name)
}
