//! Running scheduled tasks (the list itself is `store::schedules`). One runner for every place a task can run: the desktop app's timer,
//! the cloud server's timer, "Run now", and "Test" from any client. See docs/SCHEDULED.md.
//!
//! * A **task** is run by the agent with its tools; a **reminder** is a notification only (no model, nothing is asked of anybody); a
//!   **browser task** is a task that may use the browser and web tools and always asks before it changes anything.
//! * The permission mode of a run is the task's own (auto or manual) or the person's saved one; it is never Bypass for a browser task,
//!   for a Test, or on the cloud server. Approvals are never skipped: whoever runs a task brings an `Approver`, and when nobody can answer
//!   (the cloud server) the answer is no.
//! * A **Test** is a normal run in a chat titled "Test: <task>". It does not change the task's schedule, last run or history.
//! * Which host runs a task: the desktop app runs tasks that are on "this computer" or on a machine; the cloud server runs tasks that are
//!   on "cloud" or on a machine (the agent then works on the machine through the SSH tools, from the host that owns the task).

use crate::ai::agent::{self, Config, Event};
use crate::ai::{claude_code, Selection};
use crate::approval::{Approver, NoQuestions};
use crate::policy::Mode;
use crate::store::chats::{self, Chat, Entry};
use crate::store::schedules::{self, Finished, Task};
use crate::tenant;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

/// What is shown to the person when a run ends (a desktop notification, a web push, an Android notification, the test result card).
#[derive(Clone, Debug, PartialEq)]
pub struct Notice {
    pub title: String,
    pub body: String,
    /// task | reminder
    pub kind: String,
    pub ok: bool,
    pub chat: Option<String>,
}

pub struct Outcome {
    /// ok | failed | skipped
    pub status: &'static str,
    /// The final answer, or why it failed or did not start.
    pub summary: String,
    pub tools: Vec<String>,
    pub ms: u64,
    /// The chat of the run, not saved yet (a reminder has none).
    pub chat: Option<Chat>,
    pub notice: Notice,
}

impl Outcome {
    pub fn ok(&self) -> bool {
        self.status == "ok"
    }
    /// What `schedule_test` sends as its last event.
    pub fn result_json(&self) -> Value {
        json!({
            "ok": self.ok(), "status": self.status, "summary": cut(&self.summary, 4000), "tools": self.tools, "ms": self.ms,
            "chat": self.chat.as_ref().map(|c| c.id.clone()),
            "notice": {"title": self.notice.title, "body": self.notice.body, "kind": self.notice.kind, "ok": self.notice.ok},
        })
    }
}

pub struct RunOpts<'a> {
    pub test: bool,
    /// "scheduled" or "manual".
    pub how: &'a str,
    /// The time the task was planned for, when this run is a catch-up.
    pub missed: Option<u64>,
    pub approver: &'a dyn Approver,
    pub cancel: &'a Arc<AtomicBool>,
    /// Where it shows in Activity: "scheduler", "phone", "web" ...
    pub source: &'a str,
}

fn cut(s: &str, max: usize) -> String {
    if s.chars().count() > max {
        format!("{}…", s.chars().take(max.saturating_sub(1)).collect::<String>().trim_end())
    } else {
        s.to_string()
    }
}

fn entry(kind: &str, text: impl Into<String>) -> Entry {
    Entry { kind: kind.into(), text: text.into(), ..Default::default() }
}

// ── who runs what ──

/// Does this process run `task`? The desktop runs "this computer" tasks, the cloud server runs "cloud" tasks; either runs a machine's.
pub fn host_runs(task: &Task) -> bool {
    match task.run_on() {
        "cloud" => tenant::multi_tenant(),
        "this" => !tenant::multi_tenant(),
        // machine:<name>: this host runs it, unless a runner on that machine does (docs/RUNNER.md)
        // (a browser task is the exception: the runner has no browser, so the host keeps running it)
        _ => task.kind() == "browser" || !task.machine().is_some_and(crate::runner::host::has_runner),
    }
}

/// "this computer", "the cloud server" or "<machine> (from this computer)".
pub fn place_text(task: &Task) -> String {
    match task.machine() {
        Some(m) if crate::runner::host::has_runner(m) && task.kind() != "browser" => format!("{m}, by the runner on it (it works while this computer is off)"),
        Some(m) => format!("{m}, worked on from {}", if tenant::multi_tenant() { "the cloud server" } else { "this computer" }),
        None if task.run_on() == "cloud" => "the cloud server".into(),
        None => "this computer".into(),
    }
}

/// When the next task of this person that this host runs is due (an asked-for run counts as now), for the server's index of who has
/// something to run. None when nothing of theirs is switched on.
pub fn next_due_here() -> Option<u64> {
    let now = schedules::now_millis();
    schedules::load().iter().filter(|t| t.enabled && host_runs(t)).map(|t| if t.run_requested { now } else { t.next_run }).min()
}

// ── the cloud policy and per-person limits ──

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloudTasks {
    /// Nobody runs tasks on the server.
    Off,
    /// Only the people the operator made administrators.
    Admins,
    /// Everybody who signs in (a person who is not an administrator is restricted: reminders and web search/fetch tasks).
    All,
}

/// The operator's switch `ZO_CLOUD_TASKS` = `all` (default) | `admins` | `off`.
pub fn cloud_policy() -> CloudTasks {
    cloud_policy_from(std::env::var("ZO_CLOUD_TASKS").ok().as_deref())
}

fn cloud_policy_from(v: Option<&str>) -> CloudTasks {
    match v.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        Some("off" | "none" | "0" | "false") => CloudTasks::Off,
        Some("admins" | "admin" | "owner") => CloudTasks::Admins,
        _ => CloudTasks::All,
    }
}

/// May the person of this thread have tasks on the cloud server?
pub fn cloud_allowed_here() -> bool {
    if !tenant::multi_tenant() {
        return false;
    }
    match cloud_policy() {
        CloudTasks::Off => false,
        CloudTasks::Admins => tenant::current().is_some_and(|t| !t.restricted),
        CloudTasks::All => tenant::current().is_some(),
    }
}

/// What one person may have on the cloud server.
#[derive(Clone, Debug, PartialEq)]
pub struct Limits {
    pub max_tasks: usize,
    /// The shortest time between two runs.
    pub min_repeat_ms: u64,
    pub runs_per_day: u32,
    pub kinds: &'static [&'static str],
    /// Machines (SSH) may be used from the cloud.
    pub machines: bool,
}

pub fn limits() -> Limits {
    if tenant::multi_tenant() && tenant::is_restricted() {
        Limits { max_tasks: 5, min_repeat_ms: 3_600_000, runs_per_day: 24, kinds: &["reminder", "task"], machines: false }
    } else if tenant::multi_tenant() {
        Limits { max_tasks: 30, min_repeat_ms: schedules::MIN_EVERY_MS, runs_per_day: 200, kinds: &["reminder", "task", "browser"], machines: true }
    } else {
        Limits { max_tasks: 500, min_repeat_ms: schedules::MIN_EVERY_MS, runs_per_day: u32::MAX, kinds: &["reminder", "task", "browser"], machines: true }
    }
}

/// Checks a task that is about to be kept (made or edited) against this host and this person, and returns the `run_on` to keep.
/// `existing` is how many tasks the person already has (the one being edited not counted).
pub fn check_for_host(kind: &str, run_on: &str, repeat: &str, existing: usize) -> Result<String, String> {
    let lim = limits();
    let multi = tenant::multi_tenant();
    if multi && !cloud_allowed_here() {
        return Err("Scheduled tasks on the server are not available to this account.".into());
    }
    let kind = if kind.is_empty() { "task" } else { kind };
    if !lim.kinds.contains(&kind) {
        return Err(if kind == "browser" { "Browser tasks need your own computer; this account only has reminders and web search tasks on the server.".into() } else { "That kind of task is not available to this account.".into() });
    }
    let run_on = match run_on.trim() {
        "" | "this" if multi => "cloud".to_string(),
        "" | "this" => String::new(),
        "cloud" if multi => "cloud".to_string(),
        "cloud" => return Err("A task for the cloud server is made on the server (the web page or the phone app), not in this list.".into()),
        m if m.starts_with("machine:") => {
            if multi && !lim.machines {
                return Err("Machines (SSH) are not available to this account.".into());
            }
            m.to_string()
        }
        _ => return Err("Run on must be this computer, the cloud server or machine:<name>.".into()),
    };
    if multi {
        if existing >= lim.max_tasks {
            return Err(format!("You can have {} scheduled tasks on the server. Remove one first.", lim.max_tasks));
        }
        let too_often = match schedules::normalize_repeat(repeat) {
            Ok(r) => match r.as_str() {
                "hourly" => 3_600_000 < lim.min_repeat_ms,
                other => other.split_whitespace().nth(1).and_then(|n| n.parse::<u64>().ok()).zip(other.split_whitespace().nth(2)).is_some_and(|(n, u)| {
                    let step = n * match u.trim_end_matches('s') {
                        "minute" => 60_000,
                        "hour" => 3_600_000,
                        _ => u64::MAX / 1_000_000,
                    };
                    step < lim.min_repeat_ms
                }),
            },
            Err(e) => return Err(e),
        };
        if too_often {
            return Err(format!("On the server a task can repeat at most every {} minutes.", lim.min_repeat_ms / 60_000));
        }
    }
    Ok(run_on)
}

fn usage_path() -> std::path::PathBuf {
    crate::audit::data_dir().join("schedule_usage.json")
}

fn today() -> u64 {
    schedules::now_millis() / 86_400_000
}

/// How many runs this person has had today on this host.
pub fn runs_today() -> u32 {
    let v: Value = std::fs::read_to_string(usage_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    if v.get("day").and_then(Value::as_u64) == Some(today()) { v.get("runs").and_then(Value::as_u64).unwrap_or(0) as u32 } else { 0 }
}

fn count_run() {
    let n = runs_today() + 1;
    if let Some(d) = usage_path().parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(usage_path(), json!({"day": today(), "runs": n}).to_string());
}

// ── running ──

static RUNNING: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

fn key(task: &str) -> String {
    format!("{}/{task}", tenant::current().map(|t| t.id).unwrap_or_default())
}

/// Is this task being run right now?
pub fn is_running(task: &str) -> bool {
    RUNNING.lock().unwrap_or_else(|e| e.into_inner()).contains(&key(task))
}

/// Holds a task's place in the running set; dropping it frees the place.
pub struct Slot(String);

impl Slot {
    /// None when the task is already running.
    pub fn take(task: &str) -> Option<Slot> {
        let k = key(task);
        let fresh = RUNNING.lock().unwrap_or_else(|e| e.into_inner()).insert(k.clone());
        // not `then_some(Slot(k))`: a Slot that is made and dropped at once would lock the set again
        if fresh { Some(Slot(k)) } else { None }
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        RUNNING.lock().unwrap_or_else(|e| e.into_inner()).remove(&self.0);
    }
}

/// The permission mode of a run.
pub fn mode_for(task: &Task, test: bool) -> Mode {
    let chosen = task.mode.as_deref().and_then(Mode::parse_known);
    let mut mode = chosen.unwrap_or_else(Mode::load);
    // a browser task, a test and anything on the server asks first for everything that changes something: never Bypass
    if mode == Mode::Bypass && (task.kind() == "browser" || test || tenant::multi_tenant()) {
        mode = Mode::Manual;
    }
    if task.kind() == "browser" && chosen.is_none() {
        mode = Mode::Manual; // ask-first is the default of a browser task
    }
    mode
}

fn kind_note(task: &Task, test: bool, missed: Option<u64>) -> String {
    let mut s = String::from("This is a scheduled task. It runs by itself at the planned time and the person may not be watching. Do the task, then say briefly what you did and what the result was.");
    if task.kind() == "browser" {
        s.push_str(" It is a browser task: you may use the browser tools (zohara_browser_*) and the web tools to open pages, read, fill in and collect what is asked, and then report back here. Never buy, pay, book, order, send a message or e-mail, post, submit a form that sends something, or delete anything unless the person's approval comes through the approval step; if approval is not given, stop and say what you would have done.");
    }
    if let Some(m) = task.machine() {
        s.push_str(&format!(" The work is on the machine named \"{m}\": use zohara_ssh_run, zohara_ssh_read_file and zohara_ssh_write_file with that machine name, and do not change the computer you run on. If a command says the machine is not reachable or not added, say so in your answer; do not try to work around it."));
    }
    if test {
        s.push_str(" This run is a TEST of the task: do what the task says, but keep any change small and reversible, and say clearly at the end what a real run would do differently, if anything.");
    }
    if let Some(m) = missed {
        s.push_str(&format!(" The task was planned for {} but could not run then (the computer was off); it is running now, once. Say that in your first sentence.", schedules::format_when_in(m, task.tz.as_deref())));
    }
    s
}

fn failed(task: &Task, status: &'static str, why: &str, ms: u64, test: bool) -> Outcome {
    let title = if test { format!("Test: {}", task.title) } else { task.title.clone() };
    Outcome {
        status,
        summary: why.to_string(),
        tools: Vec::new(),
        ms,
        chat: None,
        notice: Notice { title: format!("{title} {}", if status == "skipped" { "did not run" } else { "failed" }), body: cut(why, 200), kind: "task".into(), ok: false, chat: None },
    }
}

/// Runs a task now and returns what came of it. Nothing is saved: see `run_and_record`. `on` gets the events as they happen.
pub fn execute(task: &Task, o: &RunOpts, on: &mut dyn FnMut(Event)) -> Outcome {
    let t0 = std::time::Instant::now();
    let now = schedules::now_millis();
    let restricted = tenant::is_restricted();
    let missed_text = o.missed.map(|m| format!("Missed {}", schedules::format_when_in(m, task.tz.as_deref())));
    // a reminder: the notification itself, no model
    if task.kind() == "reminder" {
        let body = match &missed_text {
            Some(m) => format!("{} ({m})", task.prompt),
            None => task.prompt.clone(),
        };
        return Outcome {
            status: "ok",
            summary: task.prompt.clone(),
            tools: Vec::new(),
            ms: t0.elapsed().as_millis() as u64,
            chat: None,
            notice: Notice { title: if o.test { format!("Test: {}", task.title) } else { task.title.clone() }, body: cut(&body, 300), kind: "reminder".into(), ok: true, chat: None },
        };
    }
    let lim = limits();
    if !lim.kinds.contains(&task.kind()) {
        return failed(task, "skipped", "This kind of task is not available to this account.", 0, o.test);
    }
    if let Some(m) = task.machine() {
        if restricted {
            return failed(task, "skipped", "Machines (SSH) are not available to this account.", 0, o.test);
        }
        if !crate::store::machines::load().iter().any(|x| x.name == m) {
            return failed(task, "skipped", &format!("The machine \"{m}\" is not added. Add it under Settings > Machines, or pick another place to run this task."), 0, o.test);
        }
    }
    let sel = Selection::load();
    if sel.provider.is_empty() {
        return failed(task, "skipped", "No model is chosen. Choose one in Zohara Agent (or add your key on the web) and the task can run.", 0, o.test);
    }
    if restricted && sel.provider == "claude-code" && !claude_code::allowed_here() {
        return failed(task, "skipped", "Claude Code is not available to this account. Choose another model.", 0, o.test);
    }
    let mode = mode_for(task, o.test);
    let note = kind_note(task, o.test, o.missed);
    let base = if restricted { crate::store::memory::system_without_notes(agent::system_prompt()) } else { crate::store::memory::system_with(agent::system_prompt(), &task.prompt) };
    let system = agent::add_before_character(&base, &note);
    let title = if o.test { format!("Test: {}", task.title) } else { format!("Scheduled: {}", task.title) };
    let mut entries = vec![entry("msg-user", task.prompt.clone())];
    if let Some(m) = &missed_text {
        entries.push(entry("msg-tool-out", format!("{m}: the computer was off. Running it now, once.")));
    }
    let mut history = Vec::new();
    let mut session = None;
    let mut tools: Vec<String> = Vec::new();
    let mut last_text = String::new();
    let activity = crate::activity::start(
        if sel.provider == "claude-code" { crate::activity::Kind::ClaudeCode } else { crate::activity::Kind::Scheduled },
        &title,
        &sel.model,
        o.source,
        None,
        Some(o.cancel.clone()),
    );
    let result = {
        let mut handle = |e: Event| {
            match &e {
                Event::Text(t) => {
                    entries.push(entry("msg-ai", t.clone()));
                    last_text = t.clone();
                }
                Event::ToolStart { name, summary } => {
                    if !tools.contains(name) {
                        tools.push(name.clone());
                    }
                    entries.push(entry("msg-tool", format!("▸ {name}  {}", summary.lines().nth(1).unwrap_or(""))));
                }
                Event::ToolDone { ok, output, .. } => {
                    let short: String = output.chars().take(300).collect();
                    entries.push(entry(if *ok { "msg-tool-out" } else { "msg-error" }, format!("{} {short}", if *ok { "✓" } else { "✗" })));
                }
                Event::TextDelta(_) | Event::Usage(_) | Event::Steps(_) | Event::Stuck { .. } => {}
            }
            on(e);
        };
        if sel.provider == "claude-code" {
            let model = if sel.model.is_empty() { "sonnet".to_string() } else { sel.model.clone() };
            claude_code::run_in_mode(Some(mode), &model, sel.effort.as_deref(), &mut session, &task.prompt, &system, o.cancel, &mut handle)
        } else {
            match Config::from_selection(&sel, &system) {
                Ok(cfg) => agent::run(&cfg, &mut history, &task.prompt, mode, o.approver, &NoQuestions, o.cancel, &mut handle),
                Err(e) => Err(e),
            }
        }
    };
    drop(activity);
    let mut error = None;
    if let Err(e) = result {
        let stopped = e == agent::STOPPED;
        let msg = if stopped { "Stopped.".to_string() } else { e };
        entries.push(entry("msg-error", msg.clone()));
        error = Some(msg);
    }
    agent::forget_inline_images(&mut history);
    let ms = t0.elapsed().as_millis() as u64;
    let chat = Chat { id: format!("c{now}"), title, updated: now, entries, history, session, ..Default::default() };
    let chat_id = chat.id.clone();
    let (status, summary) = match &error {
        Some(e) => ("failed", e.clone()),
        None if last_text.trim().is_empty() => ("ok", "Done.".to_string()),
        None => ("ok", last_text.trim().to_string()),
    };
    let shown = if o.test { format!("Test: {}", task.title) } else { task.title.clone() };
    let notice = Notice {
        title: if status == "ok" { format!("{shown} finished") } else { format!("{shown} failed") },
        body: cut(&summary, 200),
        kind: "task".into(),
        ok: status == "ok",
        chat: Some(chat_id),
    };
    Outcome { status, summary, tools, ms, chat: Some(chat), notice }
}

/// Runs a task and keeps what came of it: the chat (always, so a test is there to read) and, unless it is a test, the task's last
/// result and history and its next time. Returns None when the task is already running.
pub fn run_and_record(task: &Task, o: &RunOpts, on: &mut dyn FnMut(Event)) -> Option<Outcome> {
    let _slot = Slot::take(&task.id)?;
    let mut out = execute(task, o, on);
    if let Some(chat) = out.chat.take() {
        let id = chat.id.clone();
        chats::upsert(chat);
        out.notice.chat = Some(id);
        out.chat = chats::load().into_iter().find(|c| c.id == out.notice.chat.clone().unwrap_or_default());
    }
    if !o.test {
        if out.status != "skipped" {
            count_run();
        }
        schedules::finish(
            &task.id,
            schedules::now_millis(),
            Finished { status: out.status.into(), summary: out.summary.clone(), chat: out.notice.chat.clone(), ms: out.ms, missed: o.missed, how: o.how.into() },
        );
    }
    Some(out)
}

/// Runs every task of this host that is due, one after another. `shown` gets each result, to notify the person. Returns how many ran.
pub fn run_due(approver: &dyn Approver, source: &str, cancel: &Arc<AtomicBool>, shown: &mut dyn FnMut(&Task, &Outcome)) -> usize {
    let now = schedules::now_millis();
    let mut n = 0;
    for task in schedules::due(now) {
        if !host_runs(&task) || is_running(&task.id) {
            continue;
        }
        if tenant::multi_tenant() && runs_today() >= limits().runs_per_day {
            // the person's runs for today are used up: it waits for tomorrow (the time is moved on so it is not tried every minute)
            schedules::finish(&task.id, now, Finished { status: "skipped".into(), summary: "The daily limit of runs on the server is reached, so this run was skipped.".into(), chat: None, ms: 0, missed: None, how: "scheduled".into() });
            continue;
        }
        let how = if task.run_requested { "manual" } else { "scheduled" };
        let o = RunOpts { test: false, how, missed: schedules::missed_time(&task, now), approver, cancel, source };
        if let Some(out) = run_and_record(&task, &o, &mut |_| {}) {
            shown(&task, &out);
            n += 1;
        }
    }
    n
}

/// Starts "Run now" in the background and returns at once; the result shows in the task (last result, history) when it ends.
pub fn run_now_in_background(id: &str, approver: Arc<dyn Approver + Send + Sync>, source: &'static str, shown: Arc<dyn Fn(&Task, &Outcome) + Send + Sync>) -> Result<(), String> {
    let task = schedules::get(id).ok_or("That task is gone.")?;
    // a task that a runner on its machine owns: the runner is asked to run it now, and its result is fetched when it is there
    if task.machine().is_some_and(crate::runner::host::has_runner) && !host_runs(&task) {
        let id = id.to_string();
        tenant::spawn(move || {
            if let Err(e) = crate::runner::host::run_task_now(&id) {
                schedules::finish(&id, schedules::now_millis(), Finished { status: "failed".into(), summary: e, chat: None, ms: 0, missed: None, how: "manual".into() });
            }
        });
        return Ok(());
    }
    if !host_runs(&task) {
        return Err(format!("This task runs on {}, not here.", place_text(&task)));
    }
    if is_running(id) {
        return Err("That task is running already.".into());
    }
    let id = id.to_string();
    tenant::spawn(move || {
        let Some(task) = schedules::get(&id) else { return };
        let cancel = Arc::new(AtomicBool::new(false));
        let o = RunOpts { test: false, how: "manual", missed: None, approver: &*approver, cancel: &cancel, source };
        if let Some(out) = run_and_record(&task, &o, &mut |_| {}) {
            shown(&task, &out);
        }
    });
    Ok(())
}

/// Who shows a finished task to the person: `(tenant id, task, notice)`. The desktop app shows a desktop notification; the cloud server
/// sends a web push or an Android notification. Whoever starts a host registers one; without it nothing is shown (the result is in the
/// task and its history all the same).
pub type Notifier = Arc<dyn Fn(&str, &Task, &Notice) + Send + Sync>;

static NOTIFIER: Mutex<Option<Notifier>> = Mutex::new(None);

pub fn set_notifier(n: Notifier) {
    *NOTIFIER.lock().unwrap_or_else(|e| e.into_inner()) = Some(n);
}

/// Shows a finished run (never a test) through the registered notifier.
pub fn notify(tenant_id: &str, task: &Task, notice: &Notice) {
    let n = NOTIFIER.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(n) = n {
        n(tenant_id, task, notice);
    }
}

/// The short text of a notification of a finished run, for places that only have a title and one line.
pub fn notice_json(n: &Notice) -> Value {
    json!({"title": n.title, "body": n.body, "kind": n.kind, "ok": n.ok, "chat": n.chat})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approval::Deny;
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A tiny model server: each request gets the next answer of the list (the last one repeats).
    fn model(answers: Vec<Value>) -> (String, Arc<AtomicUsize>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let hits = Arc::new(AtomicUsize::new(0));
        let h = hits.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { return };
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                loop {
                    let n = s.read(&mut tmp).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&buf[..i]).to_lowercase();
                        let len = head.lines().find_map(|l| l.strip_prefix("content-length:")).and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(0);
                        while buf.len() < i + 4 + len {
                            let n = s.read(&mut tmp).unwrap_or(0);
                            if n == 0 {
                                break;
                            }
                            buf.extend_from_slice(&tmp[..n]);
                        }
                        break;
                    }
                }
                let k = h.fetch_add(1, Ordering::SeqCst);
                let body = answers[k.min(answers.len() - 1)].to_string();
                let _ = write!(s, "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len());
            }
        });
        (url, hits)
    }

    fn env(name: &str, f: impl FnOnce(&std::path::Path)) {
        let _g = crate::store::TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("zo-runner-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("XDG_CONFIG_HOME", dir.join("config"));
        std::env::set_var("XDG_DATA_HOME", dir.join("data"));
        f(&dir);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn choose(url: String) {
        let mut sel = Selection { provider: "ollama".into(), model: "m".into(), ..Default::default() };
        sel.base_urls.insert("ollama".into(), url);
        sel.save();
    }

    fn task(kind: &str, prompt: &str) -> Task {
        schedules::add_new(schedules::New { title: "Morning check".into(), prompt: prompt.into(), next_run: 1_000, repeat: "daily".into(), kind: kind.into(), ..Default::default() }).unwrap()
    }

    fn opts<'a>(test: bool, cancel: &'a Arc<AtomicBool>) -> RunOpts<'a> {
        RunOpts { test, how: if test { "manual" } else { "scheduled" }, missed: None, approver: &Deny, cancel, source: "test" }
    }

    #[test]
    fn a_task_runs_leaves_a_chat_and_moves_to_its_next_time() {
        env("run", |_| {
            let (url, _) = model(vec![json!({"choices":[{"message":{"content":"All is well."}}]})]);
            choose(url);
            let t = task("task", "check that all is well");
            let cancel = Arc::new(AtomicBool::new(false));
            let out = run_and_record(&t, &RunOpts { missed: Some(1_000), ..opts(false, &cancel) }, &mut |_| {}).unwrap();
            assert_eq!((out.status, out.summary.as_str()), ("ok", "All is well."));
            let chat = chats::load().into_iter().find(|c| c.title == "Scheduled: Morning check").expect("the chat is saved");
            assert!(chat.entries.iter().any(|e| e.kind == "msg-ai" && e.text == "All is well."));
            assert!(chat.entries.iter().any(|e| e.text.starts_with("Missed ")), "a catch-up says what was missed");
            let g = schedules::get(&t.id).unwrap();
            assert_eq!((g.last_status.as_deref(), g.last_output.as_deref(), g.history.len()), (Some("ok"), Some("All is well."), 1));
            assert_eq!(g.last_chat, Some(chat.id));
            assert!(g.next_run > schedules::now_millis(), "a daily task moved to its next time");
            assert_eq!(g.history[0].missed, Some(1_000));
            assert_eq!(runs_today(), 1);
        });
    }

    #[test]
    fn a_test_run_changes_nothing_in_the_task() {
        env("test", |_| {
            let (url, _) = model(vec![json!({"choices":[{"message":{"content":"Test answer."}}]})]);
            choose(url);
            let t = task("task", "do it");
            let before = schedules::get(&t.id).unwrap();
            let cancel = Arc::new(AtomicBool::new(false));
            let mut seen = Vec::new();
            let out = run_and_record(&t, &opts(true, &cancel), &mut |e| seen.push(matches!(e, Event::Text(_)))).unwrap();
            assert!(out.ok() && seen.contains(&true));
            assert_eq!(schedules::get(&t.id).unwrap(), before, "schedule, last run and history are untouched");
            let chat = chats::load().into_iter().find(|c| c.title == "Test: Morning check").expect("the test chat is there to read");
            assert!(chat.entries.iter().any(|e| e.text == "Test answer."));
            assert_eq!(runs_today(), 0, "a test does not use up the day's runs");
            assert!(out.notice.title.starts_with("Test: "));
        });
    }

    #[test]
    fn a_test_never_skips_an_approval_even_in_bypass() {
        env("approve", |dir| {
            let target = dir.join("written.txt");
            let call = json!({"choices":[{"message":{"content":null,"tool_calls":[{"id":"1","type":"function","function":{"name":"zohara_fs_write","arguments":json!({"path": target.to_string_lossy(), "content": "x"}).to_string()}}]}}]});
            let (url, hits) = model(vec![call, json!({"choices":[{"message":{"content":"I could not write it."}}]})]);
            choose(url);
            Mode::Bypass.save();
            let t = task("task", "write a file");
            assert_eq!(mode_for(&t, true), Mode::Manual, "a test is never Bypass");
            let cancel = Arc::new(AtomicBool::new(false));
            let mut tool_results = Vec::new();
            let out = run_and_record(&t, &opts(true, &cancel), &mut |e| {
                if let Event::ToolDone { ok, output, .. } = e {
                    tool_results.push((ok, output));
                }
            })
            .unwrap();
            assert!(hits.load(Ordering::SeqCst) >= 2, "the model saw the refusal and answered");
            assert!(!target.exists(), "nobody approved, so nothing was written");
            assert!(tool_results.iter().any(|(ok, _)| !ok), "{tool_results:?}");
            assert!(out.tools.contains(&"zohara_fs_write".to_string()));
            Mode::Auto.save();
        });
    }

    #[test]
    fn browser_tasks_ask_first_and_modes_are_never_bypass_where_nobody_can_answer() {
        env("modes", |_| {
            Mode::Bypass.save();
            let b = task("browser", "look at a page");
            assert_eq!(mode_for(&b, false), Mode::Manual);
            let mut plain = task("task", "x");
            assert_eq!(mode_for(&plain, false), Mode::Bypass, "the person's own mode, as before");
            plain.mode = Some("manual".into());
            assert_eq!(mode_for(&plain, false), Mode::Manual);
            Mode::Auto.save();
            assert_eq!(mode_for(&b, false), Mode::Manual);
            let mut t = task("browser", "y");
            t.mode = Some("auto".into());
            assert_eq!(mode_for(&t, false), Mode::Auto, "the person may choose auto for a browser task; clicks still ask in Auto");
            assert!(kind_note(&b, false, None).contains("Never buy, pay"));
            assert!(kind_note(&plain, true, None).contains("TEST"));
        });
    }

    #[test]
    fn a_reminder_needs_no_model_and_shows_its_notification() {
        env("remind", |_| {
            // no model chosen at all
            let t = task("reminder", "call mom");
            let cancel = Arc::new(AtomicBool::new(false));
            let out = run_and_record(&t, &RunOpts { missed: Some(1_000), ..opts(false, &cancel) }, &mut |_| {}).unwrap();
            assert_eq!(out.status, "ok");
            assert_eq!((out.notice.kind.as_str(), out.notice.title.as_str()), ("reminder", "Morning check"));
            assert!(out.notice.body.starts_with("call mom") && out.notice.body.contains("Missed"), "{}", out.notice.body);
            assert!(out.chat.is_none() && chats::load().is_empty(), "a reminder leaves no chat");
            let g = schedules::get(&t.id).unwrap();
            assert_eq!(g.last_status.as_deref(), Some("ok"));
            let test = run_and_record(&t, &opts(true, &cancel), &mut |_| {}).unwrap();
            assert!(test.notice.title.starts_with("Test: ") && test.notice.body == "call mom");
        });
    }

    #[test]
    fn a_run_that_cannot_start_says_why_and_a_failure_is_recorded() {
        env("fail", |_| {
            let t = task("task", "do it");
            let cancel = Arc::new(AtomicBool::new(false));
            let out = run_and_record(&t, &opts(false, &cancel), &mut |_| {}).unwrap();
            assert_eq!(out.status, "skipped");
            assert!(out.summary.contains("No model"), "{}", out.summary);
            assert_eq!(schedules::get(&t.id).unwrap().last_status.as_deref(), Some("skipped"));
            // a machine that is not added
            choose("http://127.0.0.1:9/v1".into());
            let m = schedules::add_new(schedules::New { prompt: "x".into(), next_run: 1, repeat: "once".into(), run_on: "machine:ghost".into(), ..Default::default() }).unwrap();
            let out = run_and_record(&m, &opts(false, &cancel), &mut |_| {}).unwrap();
            assert!(out.summary.contains("ghost") && out.summary.contains("not added"), "{}", out.summary);
            // an unreachable model is a failure with the reason
            let t2 = task("task", "again");
            let out = run_and_record(&t2, &opts(false, &cancel), &mut |_| {}).unwrap();
            assert_eq!(out.status, "failed");
            assert!(!out.summary.is_empty() && !out.notice.ok && out.notice.title.ends_with("failed"));
        });
    }

    #[test]
    fn a_task_is_never_run_twice_at_once_and_due_tasks_are_run_by_their_host() {
        env("due", |_| {
            let (url, hits) = model(vec![json!({"choices":[{"message":{"content":"ok"}}]})]);
            choose(url);
            let t = task("task", "a");
            let _slot = Slot::take(&t.id).unwrap();
            assert!(is_running(&t.id) && Slot::take(&t.id).is_none());
            let cancel = Arc::new(AtomicBool::new(false));
            assert!(run_and_record(&t, &opts(false, &cancel), &mut |_| {}).is_none(), "already running");
            assert_eq!(run_due(&Deny, "test", &cancel, &mut |_, _| {}), 0, "a running task is skipped by the timer");
            drop(_slot);
            let cloud = schedules::add_new(schedules::New { prompt: "c".into(), next_run: 1, repeat: "once".into(), ..Default::default() }).unwrap();
            // a task that belongs to the cloud is not run by a desktop (change it by hand: a desktop cannot make one)
            let mut shown = Vec::new();
            let n = run_due(&Deny, "test", &cancel, &mut |t, o| shown.push((t.id.clone(), o.status)));
            assert_eq!(n, 2, "both desktop tasks were due: {shown:?}");
            assert_eq!(hits.load(Ordering::SeqCst), 2);
            assert!(schedules::get(&cloud.id).unwrap().last_run.is_some());
            assert!(host_runs(&t) && !host_runs(&Task { run_on: "cloud".into(), ..t.clone() }) && host_runs(&Task { run_on: "machine:x".into(), ..t.clone() }));
        });
    }

    #[test]
    fn run_now_runs_in_the_background_and_keeps_the_schedule() {
        env("runnow", |_| {
            let (url, _) = model(vec![json!({"choices":[{"message":{"content":"Ran now."}}]})]);
            choose(url);
            let t = schedules::add_new(schedules::New { prompt: "later".into(), next_run: schedules::now_millis() + 5 * 3_600_000, repeat: "daily".into(), ..Default::default() }).unwrap();
            let (tx, rx) = std::sync::mpsc::channel();
            let shown: Arc<dyn Fn(&Task, &Outcome) + Send + Sync> = Arc::new(move |_, o| {
                let _ = tx.send(o.summary.clone());
            });
            run_now_in_background(&t.id, Arc::new(Deny), "test", shown).unwrap();
            assert_eq!(rx.recv_timeout(std::time::Duration::from_secs(20)).unwrap(), "Ran now.");
            let g = schedules::get(&t.id).unwrap();
            assert_eq!(g.next_run, t.next_run, "the planned time is still ahead");
            assert_eq!((g.history.len(), g.history[0].how.as_str()), (1, "manual"));
            assert!(run_now_in_background("nope", Arc::new(Deny), "test", Arc::new(|_, _| {})).is_err());
        });
    }

    #[test]
    fn the_cloud_switch_and_the_limits_follow_the_person() {
        assert_eq!(cloud_policy_from(None), CloudTasks::All);
        assert_eq!(cloud_policy_from(Some("admins")), CloudTasks::Admins);
        assert_eq!(cloud_policy_from(Some("OFF")), CloudTasks::Off);
        env("limits", |dir| {
            // on a computer: this computer or a machine; the cloud is not made here
            assert_eq!(check_for_host("task", "", "daily", 0).unwrap(), "");
            assert_eq!(check_for_host("browser", "machine:vm", "daily", 0).unwrap(), "machine:vm");
            assert!(check_for_host("task", "cloud", "daily", 0).is_err());
            assert!(check_for_host("task", "mars", "daily", 0).is_err());
            // on the server, a restricted person
            tenant::test_multi(true);
            let t = tenant::Tenant::open_in(&dir.join("tenants"), &"a".repeat(32), true).unwrap();
            tenant::scope(&t, || {
                assert_eq!(limits().max_tasks, 5);
                assert_eq!(check_for_host("reminder", "", "daily", 0).unwrap(), "cloud");
                assert_eq!(check_for_host("task", "this", "weekdays", 4).unwrap(), "cloud");
                assert!(check_for_host("browser", "cloud", "daily", 0).unwrap_err().contains("own computer"));
                assert!(check_for_host("task", "machine:vm", "daily", 0).is_err(), "no SSH for a restricted person");
                assert!(check_for_host("task", "cloud", "daily", 5).unwrap_err().contains("5 scheduled tasks"));
                assert!(check_for_host("task", "cloud", "every 30 minutes", 0).is_err(), "at most hourly");
                assert!(check_for_host("task", "cloud", "hourly", 0).is_ok());
                assert!(check_for_host("task", "cloud", "every 2 hours", 0).is_ok());
            });
            // an administrator
            let a = tenant::Tenant::open_in(&dir.join("tenants"), &"b".repeat(32), false).unwrap();
            tenant::scope(&a, || {
                assert!(check_for_host("browser", "", "every 10 minutes", 0).is_ok());
                assert!(check_for_host("task", "machine:vm", "daily", 0).is_ok());
                assert!(check_for_host("task", "cloud", "every 2 minutes", 0).is_err());
            });
            tenant::test_multi(false);
        });
    }

    #[test]
    fn the_daily_run_count_is_kept_per_day() {
        env("usage", |_| {
            assert_eq!(runs_today(), 0);
            count_run();
            count_run();
            assert_eq!(runs_today(), 2);
            std::fs::write(usage_path(), json!({"day": today() - 1, "runs": 99}).to_string()).unwrap();
            assert_eq!(runs_today(), 0, "yesterday's count is gone");
        });
    }

    #[test]
    fn a_restricted_persons_task_on_the_server_gets_the_web_tools_only() {
        let _g = crate::store::TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        use crate::tenant::testkit::*;
        let t = fresh(true);
        let target = std::env::temp_dir().join(format!("zo-restricted-write-{}.txt", std::process::id()));
        let _ = std::fs::remove_file(&target);
        let (url, seen) = fake_model(vec![tool_reply(&[("zohara_fs_write", json!({"path": target.to_string_lossy(), "content": "x"}))]), chat_reply("I could not write.")]);
        use_model(&t, &url);
        crate::tenant::test_multi(true);
        let results = tenant::scope(&t, || {
            let task = schedules::add_new(schedules::New { prompt: "write a file".into(), next_run: 1_000, repeat: "daily".into(), run_on: "cloud".into(), ..Default::default() }).unwrap();
            assert!(host_runs(&task));
            let cancel = Arc::new(AtomicBool::new(false));
            let mut tool_results = Vec::new();
            let o = RunOpts { test: false, how: "scheduled", missed: None, approver: &Deny, cancel: &cancel, source: "test" };
            let out = run_and_record(&task, &o, &mut |e| {
                if let Event::ToolDone { ok, output, .. } = e {
                    tool_results.push((ok, output));
                }
            })
            .unwrap();
            assert_eq!(out.status, "ok", "{}", out.summary);
            tool_results
        });
        crate::tenant::test_multi(false);
        let bodies = seen.join().unwrap();
        assert!(!target.exists(), "a person who is not an owner has no file tools on the server");
        assert!(results.iter().any(|(ok, _)| !ok), "{results:?}");
        // the model was only offered the web and memory tools
        let offered: Vec<String> = bodies[0]["tools"].as_array().unwrap().iter().filter_map(|t| t["function"]["name"].as_str().map(String::from)).collect();
        assert!(offered.iter().all(|n| n.starts_with("zohara_web_") || n.starts_with("zohara_memory_") || n == "zohara_ask_user"), "{offered:?}");
    }
}
