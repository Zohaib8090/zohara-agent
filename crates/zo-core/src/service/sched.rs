//! The scheduled-task operations of the service (docs/SCHEDULED.md): list, add, edit, on/off, run now, history, the preview of the next
//! runs, and Test (the only one that streams). They are separate from the rest of the service so the two stay easy to read.

use super::*;
use crate::scheduler::{self, Outcome, RunOpts};
use crate::store::schedules::{self, Task};

/// The operations that are answered here (Test is a stream and is started by `serve_request`).
pub fn handles(op: &str) -> bool {
    matches!(op, "scheduled" | "schedule_add" | "schedule_update" | "schedule_enable" | "schedule_cancel" | "schedule_history" | "schedule_preview" | "schedule_run_now")
}

fn text<'a>(req: &'a Value, k: &str) -> Option<&'a str> {
    req.get(k).and_then(Value::as_str).map(str::trim)
}

fn tz_of(req: &Value) -> Option<String> {
    schedules::clean_tz(text(req, "tz"))
}

/// A task as the clients get it: the stored fields, whether it is running now, and where it runs, in words.
fn task_json(t: &Task) -> Value {
    let mut v = schedules::to_json(t);
    v["running"] = json!(scheduler::is_running(&t.id));
    v["place"] = json!(scheduler::place_text(t));
    // a task that belongs to the other host is listed but this host does not run it
    v["runs_here"] = json!(scheduler::host_runs(t));
    v
}

fn run_on_options() -> Vec<Value> {
    let lim = scheduler::limits();
    let mut out = Vec::new();
    if tenant::multi_tenant() {
        out.push(json!({"id": "cloud", "label": "Cloud server", "note": "Runs on the Zohara Agent server, even when your computer is off."}));
    } else {
        out.push(json!({"id": "this", "label": "This computer", "note": "Needs Zohara Agent to be running (it can run in the tray)."}));
    }
    if lim.machines && !tenant::is_restricted() {
        for m in crate::store::machines::load() {
            out.push(json!({"id": format!("machine:{}", m.name), "label": format!("{} (added machine)", m.name), "note": if crate::runner::host::has_runner(&m.name) { format!("The runner on {} runs it, even when {} is off (browser tasks still run from {}).", m.name, if tenant::multi_tenant() { "the cloud server" } else { "this computer" }, if tenant::multi_tenant() { "the cloud server" } else { "this computer" }) } else { format!("Runs from {} and works on {} through SSH.", if tenant::multi_tenant() { "the cloud server" } else { "this computer" }, m.name) }}));
        }
    }
    out
}

fn list() -> Value {
    let lim = scheduler::limits();
    json!({
        "tasks": schedules::load().iter().map(task_json).collect::<Vec<_>>(),
        "host": if tenant::multi_tenant() { "cloud" } else { "desktop" },
        "run_on": run_on_options(),
        "kinds": lim.kinds,
        "limits": {"max_tasks": lim.max_tasks, "min_repeat_minutes": lim.min_repeat_ms / 60_000, "runs_per_day": if lim.runs_per_day == u32::MAX { Value::Null } else { json!(lim.runs_per_day) }, "runs_today": scheduler::runs_today()},
        "now": schedules::now_millis(),
    })
}

/// What the fields of a request say about when and how often.
struct Timing {
    at: u64,
    repeat: String,
    day: Option<u32>,
}

fn timing(req: &Value, tz: Option<&str>, default_repeat: &str) -> Result<Timing, String> {
    let now = schedules::now_millis();
    let (at, said, day) = match (req.get("next_run").and_then(Value::as_u64), text(req, "when").filter(|w| !w.is_empty())) {
        (_, Some(w)) => {
            let w = schedules::parse_schedule(w, tz, now)?;
            (w.at, w.repeat, w.day)
        }
        (Some(ms), None) => (ms, None, None),
        (None, None) => return Err("Say when, for example \"tomorrow 9am\", \"in 2 hours\" or \"every weekday at 9\".".into()),
    };
    // a repeat in its own field wins over one the time words carried
    let repeat = match text(req, "repeat").filter(|r| !r.is_empty()) {
        Some(r) => schedules::normalize_repeat(r)?,
        None => said.unwrap_or_else(|| default_repeat.to_string()),
    };
    // "every monday" and the like give the first run itself; a repeat given on top of a plain time moves on from that time
    Ok(Timing { at, repeat, day })
}

fn add(req: &Value) -> Result<Value, String> {
    let tz = tz_of(req);
    let t = timing(req, tz.as_deref(), "once")?;
    let kind = text(req, "kind").unwrap_or("task");
    let existing = schedules::load().len();
    let run_on = scheduler::check_for_host(kind, text(req, "run_on").unwrap_or(""), &t.repeat, existing)?;
    let prompt = text(req, "task").or_else(|| text(req, "prompt")).unwrap_or("");
    let task = schedules::add_new(schedules::New {
        title: text(req, "title").unwrap_or("").into(),
        prompt: prompt.into(),
        next_run: t.at,
        repeat: t.repeat,
        project: text(req, "project").filter(|p| !p.is_empty()).map(String::from),
        kind: kind.into(),
        mode: text(req, "mode").map(String::from),
        run_on,
        tz,
        day: t.day,
    })?;
    Ok(json!({
        "task": task_json(&task),
        "message": format!("Scheduled \"{}\" for {} ({}). It is on the Scheduled page, id {}.", task.title, schedules::format_when_in(task.next_run, task.tz.as_deref()), task.repeat, task.id),
    }))
}

fn update(req: &Value) -> Result<Value, String> {
    let id = text(req, "id").unwrap_or("");
    let cur = schedules::get(id).ok_or("That task is gone.")?;
    let tz = tz_of(req).or(cur.tz.clone());
    let mut e = schedules::Edit::default();
    e.title = text(req, "title").map(String::from);
    e.prompt = text(req, "task").or_else(|| text(req, "prompt")).map(String::from);
    e.kind = text(req, "kind").map(String::from);
    e.mode = text(req, "mode").map(String::from);
    e.tz = text(req, "tz").map(String::from);
    if text(req, "when").is_some_and(|w| !w.is_empty()) || req.get("next_run").is_some() {
        let t = timing(req, tz.as_deref(), &cur.repeat)?;
        e.next_run = Some(t.at);
        e.repeat = Some(t.repeat);
        e.day = Some(t.day);
    } else if let Some(r) = text(req, "repeat").filter(|r| !r.is_empty()) {
        e.repeat = Some(r.into());
    }
    let repeat = e.repeat.clone().unwrap_or(cur.repeat.clone());
    let kind = e.kind.clone().unwrap_or(cur.kind().to_string());
    let want_on = text(req, "run_on").map(String::from).unwrap_or(cur.run_on.clone());
    e.run_on = Some(scheduler::check_for_host(&kind, &want_on, &repeat, schedules::load().len().saturating_sub(1))?);
    let task = schedules::update(id, e)?;
    Ok(json!({"task": task_json(&task)}))
}

fn preview(req: &Value) -> Result<Value, String> {
    let tz = tz_of(req);
    let now = schedules::now_millis();
    let t = timing(req, tz.as_deref(), "once")?;
    let day = t.day;
    let runs = schedules::next_runs(t.at, &t.repeat, tz.as_deref(), day, 3, now);
    if runs.is_empty() {
        return Err("That time is already past and the task does not repeat.".into());
    }
    let label = |ms: u64| schedules::format_when_in(ms, tz.as_deref());
    Ok(json!({
        "repeat": t.repeat,
        "runs": runs.iter().map(|r| json!({"at": r, "label": label(*r)})).collect::<Vec<_>>(),
        "text": format!("next run: {}", runs.iter().map(|r| label(*r)).collect::<Vec<_>>().join(" · ")),
    }))
}

/// How a run's end reaches the person: a notice through whatever the host has registered (desktop notification, web push).
pub(super) fn notify_hook() -> Arc<dyn Fn(&Task, &Outcome) + Send + Sync> {
    let tenant_id = tenant::current().map(|t| t.id).unwrap_or_default();
    Arc::new(move |t: &Task, o: &Outcome| scheduler::notify(&tenant_id, t, &o.notice))
}

pub fn handle(op: &str, req: &Value) -> Result<Value, String> {
    let id = text(req, "id").unwrap_or("");
    match op {
        "scheduled" => Ok(list()),
        "schedule_cancel" => {
            if schedules::remove(id) { Ok(json!({})) } else { Err("That task is gone.".into()) }
        }
        "schedule_add" => add(req),
        "schedule_update" => update(req),
        "schedule_enable" => {
            let on = req.get("on").and_then(Value::as_bool).ok_or("Say on or off.")?;
            if !schedules::set_enabled(id, on) {
                return Err("That task is gone.".into());
            }
            Ok(json!({"task": schedules::get(id).as_ref().map(task_json)}))
        }
        "schedule_history" => {
            let t = schedules::get(id).ok_or("That task is gone.")?;
            Ok(json!({"task": task_json(&t), "history": t.history}))
        }
        "schedule_preview" => preview(req),
        "schedule_run_now" => {
            let t = schedules::get(id).ok_or("That task is gone.")?;
            if t.kind() != "reminder" && tenant::multi_tenant() && !scheduler::cloud_allowed_here() {
                return Err("Scheduled tasks on the server are not available to this account.".into());
            }
            // where nobody can answer an approval the answer is no; on a computer the person's own app asks
            let approver: Arc<dyn Approver + Send + Sync> = if tenant::multi_tenant() { Arc::new(crate::approval::Deny) } else { Arc::new(crate::approval::AppApprover) };
            scheduler::run_now_in_background(id, approver, "phone", notify_hook())?;
            Ok(json!({"started": true, "task": task_json(&schedules::get(id).unwrap_or(t))}))
        }
        _ => Err("Unknown scheduled-task operation.".into()),
    }
}

/// `{"op":"schedule_test","id":...}` or `{"op":"schedule_test","draft":{task fields}}`: runs the task once now in test mode and streams
/// the events like `send` does (`delta`, `text`, `tool_start`, `tool_done`, `approval`), then `notify` (what a reminder would show),
/// `result` and `done`. The task itself is not changed. Approvals are asked of the client as in a chat; Stop ends the run.
pub fn test_stream(req: &Value, mut reader: BufReader<UnixStream>, out: Arc<Mutex<UnixStream>>) {
    let fail = |why: &str| {
        line(&out, &json!({"event": "error", "error": why}));
        line(&out, &json!({"event": "done"}));
    };
    let task = match (text(req, "id").filter(|i| !i.is_empty()), req.get("draft").filter(|d| d.is_object())) {
        (Some(id), _) => match schedules::get(id) {
            Some(t) => t,
            None => return fail("That task is gone."),
        },
        (None, Some(d)) => {
            let kind = text(d, "kind").unwrap_or("task");
            let prompt = text(d, "task").or_else(|| text(d, "prompt")).unwrap_or("");
            if prompt.is_empty() {
                return fail(if kind == "reminder" { "Say what to remind you of." } else { "Say what Zohara Agent should do." });
            }
            let tz = schedules::clean_tz(text(d, "tz"));
            let run_on = match scheduler::check_for_host(kind, text(d, "run_on").unwrap_or(""), "once", 0) {
                Ok(r) => r,
                Err(e) => return fail(&e),
            };
            let mode = match text(d, "mode").filter(|m| !m.is_empty()) {
                Some(m) => match crate::policy::Mode::parse_known(m) {
                    Some(crate::policy::Mode::Auto) => Some("auto".to_string()),
                    Some(crate::policy::Mode::Manual) => Some("manual".to_string()),
                    _ => return fail("A task runs in Auto or Manual mode."),
                },
                None => None,
            };
            let title = text(d, "title").filter(|t| !t.is_empty()).map(String::from).unwrap_or_else(|| prompt.chars().take(48).collect());
            Task { id: "draft".into(), title, prompt: prompt.chars().take(schedules::MAX_PROMPT_CHARS).collect(), kind: kind.into(), mode, run_on, tz, repeat: "once".into(), enabled: true, ..Default::default() }
        }
        _ => return fail("Say which task to test (id) or give the draft."),
    };
    if tenant::multi_tenant() && !scheduler::cloud_allowed_here() {
        return fail("Scheduled tasks on the server are not available to this account.");
    }
    // the run limit of a restricted tenant applies to a test as to an answer
    if tenant::is_restricted() {
        if let Err(e) = tenant::check_room() {
            return fail(&e);
        }
    }
    let cancel = Arc::new(AtomicBool::new(false));
    let pending: Arc<Mutex<HashMap<u64, mpsc::Sender<bool>>>> = Arc::new(Mutex::new(HashMap::new()));
    let gone = Arc::new(AtomicBool::new(false));
    {
        let (cancel, pending, gone) = (cancel.clone(), pending.clone(), gone.clone());
        tenant::spawn(move || {
            let mut buf = String::new();
            loop {
                buf.clear();
                match reader.read_line(&mut buf) {
                    Ok(0) | Err(_) => {
                        cancel.store(true, Ordering::SeqCst);
                        gone.store(true, Ordering::SeqCst);
                        pending.lock().unwrap().clear();
                        return;
                    }
                    Ok(_) => {
                        let Ok(v) = serde_json::from_str::<Value>(&buf) else { continue };
                        match v.get("op").and_then(Value::as_str) {
                            Some("approve") => {
                                let id = v.get("id").and_then(Value::as_u64).unwrap_or(u64::MAX);
                                if let Some(tx) = pending.lock().unwrap().get(&id) {
                                    let _ = tx.send(v.get("yes").and_then(Value::as_bool).unwrap_or(false));
                                }
                            }
                            Some("stop") => cancel.store(true, Ordering::SeqCst),
                            _ => {}
                        }
                    }
                }
            }
        });
    }
    // on the server nobody can answer for a person who has gone; here the client is asked like in a chat
    let approver = RemoteApprover { out: out.clone(), pending, next: Arc::new(AtomicU64::new(1)), gone };
    line(&out, &json!({"event": "test", "title": task.title, "kind": task.kind(), "place": scheduler::place_text(&task)}));
    let o = RunOpts { test: true, how: "manual", missed: None, approver: &approver, cancel: &cancel, source: client_source(req) };
    let sink = out.clone();
    let result = scheduler::run_and_record(&task, &o, &mut |e| match e {
        Event::TextDelta(d) => {
            line(&sink, &json!({"event": "delta", "text": d}));
        }
        Event::Text(t) => {
            line(&sink, &json!({"event": "text", "text": t}));
        }
        Event::ToolStart { name, summary } => {
            line(&sink, &json!({"event": "tool_start", "name": name, "summary": summary}));
        }
        Event::ToolDone { name, ok, output } => {
            let cut: String = output.chars().take(1500).collect();
            line(&sink, &json!({"event": "tool_done", "name": name, "ok": ok, "output": cut}));
        }
        Event::Usage(u) => {
            line(&sink, &json!({"event": "usage", "input": u.input, "output": u.output}));
        }
        Event::Steps(n) => {
            line(&sink, &json!({"event": "steps", "n": n}));
        }
        Event::Model(_) => {}
        Event::Stuck { n, tool } => {
            line(&sink, &json!({"event": "stuck", "n": n, "tool": tool}));
        }
        Event::Notice(t) => {
            line(&sink, &json!({"event": "notice", "text": t}));
        }
    });
    match result {
        Some(r) => {
            // what a reminder would show is part of the result; a test never sends the real notification
            line(&out, &json!({"event": "notify", "title": r.notice.title, "body": r.notice.body, "kind": r.notice.kind, "ok": r.notice.ok}));
            line(&out, &json!({"event": "result", "result": r.result_json()}));
        }
        None => {
            line(&out, &json!({"event": "error", "error": "That task is running already."}));
        }
    }
    line(&out, &json!({"event": "done"}));
    let _ = out.lock().unwrap().shutdown(std::net::Shutdown::Both);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::TEST_ENV_LOCK;
    use crate::tenant::testkit::*;

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        let g = TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        env_once();
        g
    }

    /// One request over a real socket pair to `serve`; every line that comes back.
    fn talk(req: Value, multi: bool) -> Vec<Value> {
        let (client, server) = UnixStream::pair().unwrap();
        client.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
        std::thread::spawn(move || {
            crate::tenant::test_multi(multi);
            serve(server)
        });
        let mut w = client.try_clone().unwrap();
        writeln!(w, "{req}").unwrap();
        BufReader::new(client).lines().map_while(Result::ok).filter_map(|l| serde_json::from_str(&l).ok()).collect()
    }

    fn as_tenant(mut req: Value, t: &crate::tenant::Tenant) -> Value {
        req["tenant"] = json!({"id": t.id, "restricted": t.restricted});
        req
    }

    fn desktop_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("zo-sched-svc-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("XDG_CONFIG_HOME", dir.join("config"));
        std::env::set_var("XDG_DATA_HOME", dir.join("data"));
        dir
    }

    #[test]
    fn add_edit_switch_history_and_cancel_through_the_service() {
        let _g = lock();
        let dir = desktop_dir("ops");
        let r = talk(json!({"op": "scheduled"}), false);
        assert_eq!((r[0]["ok"].clone(), r[0]["tasks"].as_array().unwrap().len(), r[0]["host"].clone()), (json!(true), 0, json!("desktop")));
        assert_eq!(r[0]["run_on"][0]["id"], "this");
        // the preview does not save anything
        let p = talk(json!({"op": "schedule_preview", "when": "every weekday at 9", "tz": "UTC"}), false);
        assert_eq!(p[0]["ok"], true, "{p:?}");
        assert_eq!((p[0]["repeat"].clone(), p[0]["runs"].as_array().unwrap().len()), (json!("weekdays"), 3));
        assert!(p[0]["text"].as_str().unwrap().starts_with("next run: "));
        assert_eq!(schedules::load().len(), 0);
        let bad = talk(json!({"op": "schedule_preview", "when": "whenever"}), false);
        assert_eq!(bad[0]["ok"], false);
        // add (the words carry the repeat), with the old field names too
        let a = talk(json!({"op": "schedule_add", "task": "stand up", "when": "every weekday at 9", "kind": "reminder", "tz": "Asia/Karachi", "title": "Standup"}), false);
        assert_eq!(a[0]["ok"], true, "{a:?}");
        let t = &a[0]["task"];
        assert_eq!((t["repeat"].clone(), t["kind"].clone(), t["run_on"].clone(), t["tz"].clone()), (json!("weekdays"), json!("reminder"), json!("this"), json!("Asia/Karachi")));
        assert!(a[0]["message"].as_str().unwrap().contains("Scheduled \"Standup\""));
        let id = t["id"].as_str().unwrap().to_string();
        let old = talk(json!({"op": "schedule_add", "task": "tidy", "when": "in 2 hours"}), false);
        assert_eq!((old[0]["ok"].clone(), old[0]["task"]["repeat"].clone()), (json!(true), json!("once")), "\"in 2 hours\" is accepted now");
        assert_eq!(talk(json!({"op": "schedule_add", "task": "x", "when": "in 2 hours", "kind": "shell"}), false)[0]["ok"], false);
        assert_eq!(talk(json!({"op": "schedule_add", "task": "x", "when": "in 2 hours", "run_on": "cloud"}), false)[0]["ok"], false, "a computer cannot make a cloud task");
        assert_eq!(talk(json!({"op": "schedule_add", "task": "x", "when": "in 2 hours", "mode": "bypass"}), false)[0]["ok"], false);
        // edit
        let u = talk(json!({"op": "schedule_update", "id": id, "title": "Daily standup", "when": "every day at 10", "mode": "manual", "kind": "task"}), false);
        assert_eq!(u[0]["ok"], true, "{u:?}");
        assert_eq!((u[0]["task"]["title"].clone(), u[0]["task"]["repeat"].clone(), u[0]["task"]["mode"].clone(), u[0]["task"]["kind"].clone()), (json!("Daily standup"), json!("daily"), json!("manual"), json!("task")));
        assert_eq!(talk(json!({"op": "schedule_update", "id": "nope", "title": "x"}), false)[0]["ok"], false);
        // on and off
        let off = talk(json!({"op": "schedule_enable", "id": id, "on": false}), false);
        assert_eq!(off[0]["task"]["enabled"], false);
        assert_eq!(talk(json!({"op": "schedule_enable", "id": id, "on": true}), false)[0]["task"]["enabled"], true);
        assert_eq!(talk(json!({"op": "schedule_enable", "id": id}), false)[0]["ok"], false);
        assert_eq!(talk(json!({"op": "schedule_enable", "id": "nope", "on": true}), false)[0]["ok"], false);
        // history (empty so far) and the list carries the result fields
        let h = talk(json!({"op": "schedule_history", "id": id}), false);
        assert_eq!(h[0]["history"].as_array().unwrap().len(), 0);
        let l = talk(json!({"op": "scheduled"}), false);
        assert_eq!(l[0]["tasks"].as_array().unwrap().len(), 2);
        assert!(l[0]["tasks"][0].get("last_status").is_some() && l[0]["tasks"][0]["running"] == false && l[0]["tasks"][0]["runs_here"] == true);
        // cancel
        assert_eq!(talk(json!({"op": "schedule_cancel", "id": id}), false)[0]["ok"], true);
        assert_eq!(talk(json!({"op": "schedule_cancel", "id": id}), false)[0]["ok"], false);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_now_starts_in_the_background_and_the_result_shows_in_the_task() {
        let _g = lock();
        let dir = desktop_dir("runnow");
        let a = talk(json!({"op": "schedule_add", "task": "drink water", "when": "in 5 hours", "kind": "reminder", "repeat": "daily"}), false);
        let id = a[0]["task"]["id"].as_str().unwrap().to_string();
        let planned = a[0]["task"]["next_run"].as_u64().unwrap();
        let r = talk(json!({"op": "schedule_run_now", "id": id}), false);
        assert_eq!((r[0]["ok"].clone(), r[0]["started"].clone()), (json!(true), json!(true)), "{r:?}");
        let t0 = std::time::Instant::now();
        loop {
            let h = talk(json!({"op": "schedule_history", "id": id}), false);
            if !h[0]["history"].as_array().unwrap().is_empty() {
                assert_eq!(h[0]["history"][0]["status"], "ok");
                assert_eq!(h[0]["history"][0]["how"], "manual");
                assert_eq!(h[0]["task"]["last_output"], "drink water");
                assert_eq!(h[0]["task"]["next_run"].as_u64().unwrap(), planned, "Run now does not use up the planned time");
                break;
            }
            assert!(t0.elapsed() < Duration::from_secs(20), "the run never finished");
            std::thread::sleep(Duration::from_millis(100));
        }
        assert_eq!(talk(json!({"op": "schedule_run_now", "id": "nope"}), false)[0]["ok"], false);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_streams_the_run_and_leaves_the_task_alone() {
        let _g = lock();
        let dir = desktop_dir("test");
        let (url, _h) = fake_model(vec![chat_reply("Checked: all fine.")]);
        let mut sel = crate::ai::Selection { provider: "ollama".into(), model: "m".into(), ..Default::default() };
        sel.base_urls.insert("ollama".into(), url);
        sel.save();
        let a = talk(json!({"op": "schedule_add", "task": "check things", "when": "tomorrow 9am", "repeat": "daily"}), false);
        let id = a[0]["task"]["id"].as_str().unwrap().to_string();
        let before = schedules::get(&id).unwrap();
        let r = talk(json!({"op": "schedule_test", "id": id}), false);
        let kinds: Vec<&str> = r.iter().map(|l| l["event"].as_str().unwrap_or("")).collect();
        assert_eq!(kinds.first(), Some(&"test"), "{kinds:?}");
        assert!(kinds.contains(&"text") && kinds.contains(&"notify") && kinds.contains(&"result"), "{kinds:?}");
        assert_eq!(kinds.last(), Some(&"done"));
        let res = r.iter().find(|l| l["event"] == "result").unwrap();
        assert_eq!((res["result"]["ok"].clone(), res["result"]["summary"].clone(), res["result"]["status"].clone()), (json!(true), json!("Checked: all fine."), json!("ok")));
        assert!(res["result"]["chat"].is_string() && res["result"]["ms"].is_number() && res["result"]["tools"].is_array());
        assert_eq!(schedules::get(&id).unwrap(), before, "a test changes nothing in the task");
        assert!(crate::store::chats::load().iter().any(|c| c.title == "Test: check things"));
        // a draft that was never saved; a reminder shows the notification it would send
        let r = talk(json!({"op": "schedule_test", "draft": {"task": "call mom", "kind": "reminder", "title": "Call"}}), false);
        let n = r.iter().find(|l| l["event"] == "notify").expect("a reminder test shows its notification");
        assert_eq!((n["title"].clone(), n["body"].clone(), n["kind"].clone()), (json!("Test: Call"), json!("call mom"), json!("reminder")));
        assert_eq!(schedules::load().len(), 1, "the draft was not saved");
        // mistakes are errors on the stream, ending with done
        for bad in [json!({"op": "schedule_test"}), json!({"op": "schedule_test", "id": "nope"}), json!({"op": "schedule_test", "draft": {"task": ""}}), json!({"op": "schedule_test", "draft": {"task": "x", "mode": "bypass"}})] {
            let r = talk(bad.clone(), false);
            assert!(r.iter().any(|l| l["error"].is_string()) && r.last().unwrap()["event"] == "done", "{bad}: {r:?}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_restricted_person_has_reminders_and_web_tasks_only_with_their_own_list_and_limits() {
        let _g = lock();
        let me = fresh(true);
        let other = fresh(true);
        let admin = fresh(false);
        let ask = |req: Value, t: &crate::tenant::Tenant| talk(as_tenant(req, t), true);
        // allowed: the list, a reminder, a task (it will have the web tools only), cloud is where they run
        assert_eq!(ask(json!({"op": "scheduled"}), &me)[0]["host"], "cloud");
        let r = ask(json!({"op": "schedule_add", "task": "call mom", "when": "tomorrow 9am", "kind": "reminder", "run_on": "this"}), &me);
        assert_eq!((r[0]["ok"].clone(), r[0]["task"]["run_on"].clone()), (json!(true), json!("cloud")), "{r:?}");
        let r = ask(json!({"op": "schedule_add", "task": "find news about Rust", "when": "every day at 7", "tz": "Europe/Berlin"}), &me);
        assert_eq!(r[0]["ok"], true, "{r:?}");
        // refused: browser tasks, machines, too-frequent repeats
        for bad in [
            json!({"op": "schedule_add", "task": "x", "when": "tomorrow", "kind": "browser"}),
            json!({"op": "schedule_add", "task": "x", "when": "tomorrow", "run_on": "machine:vm"}),
            json!({"op": "schedule_add", "task": "x", "when": "tomorrow", "repeat": "every 10 minutes"}),
            json!({"op": "schedule_test", "draft": {"task": "x", "kind": "browser"}}),
        ] {
            let r = ask(bad.clone(), &me);
            assert!(r.iter().any(|l| l["ok"] == false || l["event"] == "error"), "{bad}: {r:?}");
        }
        // five at most
        for i in 0..3 {
            assert_eq!(ask(json!({"op": "schedule_add", "task": format!("r{i}"), "when": "tomorrow", "kind": "reminder"}), &me)[0]["ok"], true);
        }
        let r = ask(json!({"op": "schedule_add", "task": "one too many", "when": "tomorrow", "kind": "reminder"}), &me);
        assert!(r[0]["error"].as_str().unwrap().contains("5 scheduled tasks"), "{r:?}");
        assert_eq!(ask(json!({"op": "scheduled"}), &me)[0]["tasks"].as_array().unwrap().len(), 5);
        // per-person data: nobody else sees or changes them
        assert_eq!(ask(json!({"op": "scheduled"}), &other)[0]["tasks"].as_array().unwrap().len(), 0);
        assert_eq!(ask(json!({"op": "scheduled"}), &admin)[0]["tasks"].as_array().unwrap().len(), 0);
        let mine = ask(json!({"op": "scheduled"}), &me)[0]["tasks"][0]["id"].as_str().unwrap().to_string();
        assert_eq!(ask(json!({"op": "schedule_cancel", "id": mine}), &other)[0]["ok"], false);
        assert_eq!(ask(json!({"op": "schedule_enable", "id": mine, "on": false}), &other)[0]["ok"], false);
        assert_eq!(ask(json!({"op": "schedule_history", "id": mine}), &other)[0]["ok"], false);
        // the administrator has every kind
        assert_eq!(ask(json!({"op": "schedule_add", "task": "look at a page", "when": "tomorrow", "kind": "browser"}), &admin)[0]["ok"], true);
        // a reminder of a restricted person runs without any model and notifies through the registered notifier
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s2 = seen.clone();
        scheduler::set_notifier(Arc::new(move |tenant, t, n| s2.lock().unwrap().push((tenant.to_string(), t.title.clone(), n.body.clone()))));
        let r = ask(json!({"op": "schedule_run_now", "id": mine}), &me);
        assert_eq!(r[0]["ok"], true, "{r:?}");
        let t0 = std::time::Instant::now();
        while seen.lock().unwrap().is_empty() && t0.elapsed() < Duration::from_secs(20) {
            std::thread::sleep(Duration::from_millis(50));
        }
        let got = seen.lock().unwrap().clone();
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].0, me.id, "the notice is for the person it belongs to");
        assert_eq!(ask(json!({"op": "schedule_history", "id": mine}), &other)[0]["ok"], false);
        assert_eq!(ask(json!({"op": "schedule_history", "id": mine}), &me)[0]["history"].as_array().unwrap().len(), 1);
    }
}
