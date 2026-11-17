//! The Scheduled page of the desktop: the list (next run, last result, on/off, Run now, Test, History, Edit), the form with the preview
//! of the next runs and a Test button, the history window and the Test result card. See docs/SCHEDULED.md.

use crate::{field, hbox, icon, icon_button, lbl, open_chat, primary, secondary, sheet, vbox, Ui};
use adw::prelude::*;
use gtk4 as gtk;
use gtk::glib;
use libadwaita as adw;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;
use zo_core::ai::agent::Event;
use zo_core::approval::AppApprover;
use zo_core::scheduler::{self, RunOpts};
use zo_core::store::{chats, machines, projects as model, schedules};
use zo_core::store::schedules::{Edit, Task};

const KINDS: [(&str, &str); 3] = [("task", "Task"), ("reminder", "Reminder (a notification only)"), ("browser", "Browser task (asks first)")];
const REPEATS: [(&str, &str); 7] = [("", "As the words say"), ("once", "Once"), ("hourly", "Every hour"), ("daily", "Every day"), ("weekdays", "Every weekday"), ("weekly", "Every week"), ("monthly", "Every month")];

fn kind_name(kind: &str) -> &'static str {
    KINDS.iter().find(|(k, _)| *k == kind).map(|(_, n)| n.split(" (").next().unwrap_or("Task")).unwrap_or("Task")
}

/// "Every weekday", "Every 3 days", "Once" for the list.
fn repeat_text(r: &str) -> String {
    match r {
        "once" => "Once".into(),
        "hourly" => "Every hour".into(),
        "daily" => "Every day".into(),
        "weekdays" => "Every weekday".into(),
        "weekly" => "Every week".into(),
        "monthly" => "Every month".into(),
        other => {
            let mut c = other.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        }
    }
}

fn place(t: &Task) -> String {
    match t.machine() {
        Some(m) => format!("runs from this computer on {m}"),
        None => "this computer".into(),
    }
}

/// The text of the "next runs" preview for what is typed in the form.
fn preview_text(when: &str, repeat: &str, custom: &str) -> Result<String, String> {
    let now = schedules::now_millis();
    let w = schedules::parse_schedule(when, None, now)?;
    let typed = custom.trim();
    let repeat = if !typed.is_empty() {
        schedules::normalize_repeat(typed)?
    } else if !repeat.is_empty() {
        repeat.to_string()
    } else {
        w.repeat.clone().unwrap_or_else(|| "once".into())
    };
    let runs = schedules::next_runs(w.at, &repeat, None, w.day, 3, now);
    if runs.is_empty() {
        return Err("That time is already past and the task does not repeat.".into());
    }
    Ok(format!("next runs: {}", runs.iter().map(|r| schedules::format_when(*r)).collect::<Vec<_>>().join(" · ")))
}

/// The list of tasks.
pub fn refresh(ui: &Rc<Ui>) {
    while let Some(c) = ui.sched_list.first_child() {
        ui.sched_list.remove(&c);
    }
    let tasks = schedules::load();
    if tasks.is_empty() {
        let empty = vbox(10);
        empty.add_css_class("empty-box");
        empty.set_margin_top(40);
        let t = lbl("Nothing is scheduled", "empty-title");
        t.set_halign(gtk::Align::Center);
        let d = lbl("Ask Zohara Agent in a chat to do something later (for example \"every weekday at 9, remind me to stand up\"), or press New task.", "empty-text");
        d.set_halign(gtk::Align::Center);
        d.set_wrap(true);
        d.set_justify(gtk::Justification::Center);
        d.set_max_width_chars(60);
        empty.append(&t);
        empty.append(&d);
        ui.sched_list.append(&empty);
        return;
    }
    let now = schedules::now_millis();
    for task in tasks {
        let card = hbox(12);
        card.add_css_class("project-card");
        let ic = icon(if task.kind() == "reminder" { "alarm-symbolic" } else { "emblem-system-symbolic" }, 18);
        ic.add_css_class("pc-icon");
        card.append(&ic);
        let col = vbox(3);
        col.set_hexpand(true);
        let title = lbl(&task.title, "pc-name");
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        col.append(&title);
        let running = scheduler::is_running(&task.id);
        let when = if running {
            format!("Running now · {}", kind_name(task.kind()))
        } else if task.enabled {
            let late = if task.next_run + schedules::MISSED_AFTER_MS < now { " (missed: runs once as soon as Zohara Agent is running)" } else { "" };
            format!("Next: {} · {} · {} · {}{late}", schedules::format_when(task.next_run), repeat_text(&task.repeat), kind_name(task.kind()), place(&task))
        } else {
            format!("Off · {} · {} · {}", repeat_text(&task.repeat), kind_name(task.kind()), place(&task))
        };
        let w = lbl(&when, "pc-meta");
        w.set_wrap(true);
        col.append(&w);
        let what = lbl(&task.prompt, "pc-meta");
        what.set_ellipsize(gtk::pango::EllipsizeMode::End);
        col.append(&what);
        if let (Some(status), Some(at)) = (&task.last_status, task.last_run) {
            let out = task.last_output.clone().unwrap_or_default();
            let l = lbl(&format!("Last run: {status} {}{}{}", model::relative(at), if out.is_empty() { "" } else { " — " }, out), "pc-meta");
            l.set_ellipsize(gtk::pango::EllipsizeMode::End);
            if status != "ok" {
                l.add_css_class("error");
            }
            col.append(&l);
        }
        card.append(&col);

        if let Some(chat_id) = task.last_chat.clone() {
            let open = icon_button("chat-message-new-symbolic", "Open the last result", "icon-btn");
            let ui2 = ui.clone();
            open.connect_clicked(move |_| {
                if let Some(c) = chats::load().into_iter().find(|c| c.id == chat_id) {
                    open_chat(&ui2, c);
                }
            });
            card.append(&open);
        }
        let test = icon_button("tools-check-spelling-symbolic", "Test: run it once now without changing its schedule", "icon-btn");
        let (ui2, t2) = (ui.clone(), task.clone());
        test.connect_clicked(move |_| test_sheet(&ui2, t2.clone()));
        card.append(&test);
        let run = icon_button("media-playback-start-symbolic", "Run it now", "icon-btn");
        run.set_sensitive(!running);
        let (ui2, id) = (ui.clone(), task.id.clone());
        run.connect_clicked(move |_| {
            if let Err(e) = crate::scheduler::run_now(&id) {
                toast(&ui2, &e);
            }
            // show "Running now" and, when it ends, the result
            refresh(&ui2);
        });
        card.append(&run);
        let hist = icon_button("document-open-recent-symbolic", "History: the last runs", "icon-btn");
        let (ui2, id) = (ui.clone(), task.id.clone());
        hist.connect_clicked(move |_| history_sheet(&ui2, &id));
        card.append(&hist);
        let edit = icon_button("document-edit-symbolic", "Edit", "icon-btn");
        let (ui2, t2) = (ui.clone(), task.clone());
        edit.connect_clicked(move |_| open_form(&ui2, Some(t2.clone()), None));
        card.append(&edit);
        let sw = gtk::Switch::new();
        sw.set_active(task.enabled);
        sw.set_valign(gtk::Align::Center);
        sw.set_tooltip_text(Some("On or off"));
        let (ui2, id) = (ui.clone(), task.id.clone());
        sw.connect_state_set(move |_, on| {
            schedules::set_enabled(&id, on);
            refresh(&ui2);
            glib::Propagation::Proceed
        });
        card.append(&sw);
        let del = icon_button("edit-delete-symbolic", "Remove", "icon-btn");
        let (ui2, id) = (ui.clone(), task.id.clone());
        del.connect_clicked(move |_| {
            schedules::remove(&id);
            refresh(&ui2);
        });
        card.append(&del);
        ui.sched_list.append(&card);
    }
}

/// A short message in a small window (what went wrong with a button).
fn toast(ui: &Rc<Ui>, text: &str) {
    let (win, body, foot) = sheet(ui, "Scheduled", 420);
    let l = lbl(text, "field-hint");
    l.set_wrap(true);
    body.append(&l);
    let ok = primary("OK");
    let w = win.clone();
    ok.connect_clicked(move |_| w.close());
    foot.append(&ok);
    win.present();
}

/// The History window: the last runs of a task, newest first, each with its result and a link to its chat.
fn history_sheet(ui: &Rc<Ui>, id: &str) {
    let Some(task) = schedules::get(id) else { return };
    let (win, body, foot) = sheet(ui, &format!("History: {}", task.title), 620);
    if task.history.is_empty() {
        body.append(&lbl("It has not run yet.", "field-hint"));
    }
    for run in &task.history {
        let row = hbox(10);
        row.add_css_class("project-card");
        let col = vbox(3);
        col.set_hexpand(true);
        let missed = run.missed.map(|m| format!(" · missed {}", schedules::format_when(m))).unwrap_or_default();
        let head = lbl(&format!("{} · {}{} · {} s{}", run.status, model::relative(run.time), missed, run.ms / 1000, if run.how == "manual" { " · Run now" } else { "" }), "pc-name");
        col.append(&head);
        let s = lbl(&run.summary, "pc-meta");
        s.set_wrap(true);
        s.set_max_width_chars(70);
        col.append(&s);
        row.append(&col);
        if let Some(chat_id) = run.chat.clone() {
            let open = secondary("Open chat");
            let (ui2, w) = (ui.clone(), win.clone());
            open.connect_clicked(move |_| {
                if let Some(c) = chats::load().into_iter().find(|c| c.id == chat_id) {
                    w.close();
                    open_chat(&ui2, c);
                }
            });
            row.append(&open);
        }
        body.append(&row);
    }
    let close = primary("Close");
    let w = win.clone();
    close.connect_clicked(move |_| w.close());
    foot.append(&close);
    win.present();
}

enum Msg {
    Line(String),
    Done(serde_json::Value, String),
    Failed(String),
}

/// Test: runs the task once now in test mode (a normal chat "Test: <title>"; its schedule, last run and history are not changed;
/// every change still asks first) and shows what it does as it goes, then a result card.
pub fn test_sheet(ui: &Rc<Ui>, task: Task) {
    let (win, body, foot) = sheet(ui, &format!("Test: {}", task.title), 620);
    body.append(&lbl(if task.kind() == "reminder" { "A reminder shows the notification it would send. Nothing is changed." } else { "It runs once now. Its schedule is not changed, and anything that changes something still asks first." }, "field-hint"));
    let log = lbl("Starting…", "pc-meta");
    log.set_wrap(true);
    log.set_selectable(true);
    log.set_max_width_chars(80);
    body.append(&log);
    let card = vbox(6);
    card.add_css_class("project-card");
    card.set_visible(false);
    body.append(&card);
    let stop = secondary("Stop");
    let close = primary("Close");
    foot.append(&stop);
    foot.append(&close);
    {
        let w = win.clone();
        close.connect_clicked(move |_| w.close());
    }
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let c = cancel.clone();
        stop.connect_clicked(move |b| {
            c.store(true, Ordering::SeqCst);
            b.set_sensitive(false);
        });
    }
    let (tx, rx) = mpsc::channel::<Msg>();
    {
        let cancel = cancel.clone();
        std::thread::spawn(move || {
            let o = RunOpts { test: true, how: "manual", missed: None, approver: &AppApprover, cancel: &cancel, source: "scheduler" };
            let t2 = tx.clone();
            let out = scheduler::run_and_record(&task, &o, &mut |e| {
                let line = match e {
                    Event::ToolStart { name, summary } => Some(format!("▸ {name} {}", summary.lines().nth(1).unwrap_or(""))),
                    Event::ToolDone { ok, output, .. } => Some(format!("{} {}", if ok { "✓" } else { "✗" }, output.chars().take(200).collect::<String>())),
                    _ => None,
                };
                if let Some(l) = line {
                    let _ = t2.send(Msg::Line(l));
                }
            });
            let _ = match out {
                Some(o) => tx.send(Msg::Done(o.result_json(), format!("{}: {}", o.notice.title, o.notice.body))),
                None => tx.send(Msg::Failed("That task is running already.".into())),
            };
        });
    }
    let lines = Rc::new(RefCell::new(Vec::<String>::new()));
    let stop2 = stop.clone();
    glib::timeout_add_local(Duration::from_millis(150), move || {
        loop {
            match rx.try_recv() {
                Ok(Msg::Line(l)) => {
                    lines.borrow_mut().push(l);
                    log.set_text(&lines.borrow().join("\n"));
                }
                Ok(Msg::Failed(e)) => {
                    log.set_text(&e);
                    stop2.set_visible(false);
                    return glib::ControlFlow::Break;
                }
                Ok(Msg::Done(r, notice)) => {
                    stop2.set_visible(false);
                    log.set_visible(!lines.borrow().is_empty());
                    let ok = r["ok"].as_bool().unwrap_or(false);
                    let head = lbl(&format!("{} · {:.1} s · {}", if ok { "Success" } else { "Failed" }, r["ms"].as_u64().unwrap_or(0) as f64 / 1000.0, match r["tools"].as_array() {
                        Some(t) if !t.is_empty() => format!("tools: {}", t.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", ")),
                        _ => "no tools used".to_string(),
                    }), "pc-name");
                    if !ok {
                        head.add_css_class("error");
                    }
                    card.append(&head);
                    let ans = lbl(r["summary"].as_str().unwrap_or(""), "pc-meta");
                    ans.set_wrap(true);
                    ans.set_selectable(true);
                    ans.set_max_width_chars(80);
                    card.append(&ans);
                    if r["notice"]["kind"] == "reminder" {
                        card.append(&lbl(&format!("The notification: {notice}"), "pc-meta"));
                    }
                    if r["chat"].is_string() {
                        card.append(&lbl("The run is saved as a chat called \"Test: …\".", "field-hint"));
                    }
                    card.set_visible(true);
                    return glib::ControlFlow::Break;
                }
                Err(mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
                Err(_) => return glib::ControlFlow::Break,
            }
        }
    });
    win.present();
}

/// The New / Edit window.
pub fn open_form(ui: &Rc<Ui>, existing: Option<Task>, project: Option<String>) {
    let editing = existing.is_some();
    let (win, body, foot) = sheet(ui, if editing { "Edit scheduled task" } else { "New scheduled task" }, 580);
    let cur = existing.clone().unwrap_or_default();

    let kind = gtk::DropDown::from_strings(&KINDS.map(|(_, n)| n));
    kind.set_selected(KINDS.iter().position(|(k, _)| *k == cur.kind()).unwrap_or(0) as u32);
    body.append(&field("Kind", &kind));
    let what_label = lbl("What should Zohara Agent do?", "field-label");
    let what = gtk::Entry::new();
    what.set_placeholder_text(Some("e.g. check my disk space and tell me if it is above 90%"));
    what.add_css_class("field-input");
    what.set_text(&cur.prompt);
    let what_box = vbox(7);
    what_box.append(&what_label);
    what_box.append(&what);
    body.append(&what_box);
    let title = gtk::Entry::new();
    title.set_placeholder_text(Some("a short name for the list (optional)"));
    title.add_css_class("field-input");
    title.set_text(&cur.title);
    body.append(&field("Name", &title));
    let when = gtk::Entry::new();
    when.set_placeholder_text(Some(if editing { "leave empty to keep the time" } else { "tomorrow 9am, in 2 hours, every weekday at 9, on the 1st of each month" }));
    when.add_css_class("field-input");
    body.append(&field("When", &when));
    let repeat = gtk::DropDown::from_strings(&REPEATS.map(|(_, n)| n));
    repeat.set_selected(REPEATS.iter().position(|(r, _)| !r.is_empty() && *r == cur.repeat).unwrap_or(0) as u32);
    body.append(&field("Repeat", &repeat));
    let custom = gtk::Entry::new();
    custom.set_placeholder_text(Some("or your own: every 3 days, every 90 minutes, every 2 months"));
    custom.add_css_class("field-input");
    if !cur.repeat.is_empty() && !REPEATS.iter().any(|(r, _)| *r == cur.repeat) {
        custom.set_text(&cur.repeat);
    }
    body.append(&custom);
    let machine_list = machines::load();
    let mut run_on_ids = vec![String::new()];
    let mut run_on_names = vec!["This computer".to_string()];
    for m in &machine_list {
        run_on_ids.push(format!("machine:{}", m.name));
        run_on_names.push(format!("{} (added machine, worked on through SSH from this computer)", m.name));
    }
    let run_on = gtk::DropDown::from_strings(&run_on_names.iter().map(String::as_str).collect::<Vec<_>>());
    run_on.set_selected(run_on_ids.iter().position(|i| *i == cur.run_on).unwrap_or(0) as u32);
    body.append(&field("Run on", &run_on));
    let run_note = lbl("This computer: Zohara Agent must be running (it can run in the tray). If it was off at the time, the task runs once when it is back and says it was missed.", "field-hint");
    run_note.set_wrap(true);
    body.append(&run_note);
    let preview = lbl(&if editing { format!("next run: {}", schedules::format_when(cur.next_run)) } else { String::new() }, "field-hint");
    preview.set_wrap(true);
    body.append(&preview);
    let error = lbl("", "field-hint");
    error.add_css_class("error");
    error.set_wrap(true);
    body.append(&error);

    let update_preview = {
        let (when, repeat, custom, preview, what_label, kind, editing_next) = (when.clone(), repeat.clone(), custom.clone(), preview.clone(), what_label.clone(), kind.clone(), cur.next_run);
        Rc::new(move || {
            what_label.set_text(if KINDS[kind.selected() as usize].0 == "reminder" { "What should the reminder say?" } else { "What should Zohara Agent do?" });
            let t = when.text();
            if t.trim().is_empty() {
                preview.set_text(&if editing { format!("next run: {}", schedules::format_when(editing_next)) } else { String::new() });
                return;
            }
            preview.set_text(&match preview_text(&t, REPEATS[repeat.selected() as usize].0, &custom.text()) {
                Ok(s) => s,
                Err(e) => format!("I can't use that time yet: {e}"),
            });
        })
    };
    {
        let u = update_preview.clone();
        when.connect_changed(move |_| u());
        let u = update_preview.clone();
        custom.connect_changed(move |_| u());
        let u = update_preview.clone();
        repeat.connect_selected_notify(move |_| u());
        let u = update_preview.clone();
        kind.connect_selected_notify(move |_| u());
    }
    update_preview();

    let cancel = secondary("Cancel");
    let test = secondary("Test");
    test.set_tooltip_text(Some("Run it once now without saving or changing anything"));
    let save = primary(if editing { "Save" } else { "Create task" });
    foot.append(&cancel);
    foot.append(&test);
    foot.append(&save);
    {
        let w = win.clone();
        cancel.connect_clicked(move |_| w.close());
    }
    // the fields as one task: (kind, prompt, title, run_on, repeat, first time, day)
    let read = {
        let (kind, what, title, when, repeat, custom, run_on, ids) = (kind.clone(), what.clone(), title.clone(), when.clone(), repeat.clone(), custom.clone(), run_on.clone(), run_on_ids.clone());
        Rc::new(move || -> Result<(String, String, String, String, String, Option<(u64, Option<u32>)>), String> {
            let prompt = what.text().trim().to_string();
            if prompt.is_empty() {
                return Err("Say what the task is.".into());
            }
            let first = if when.text().trim().is_empty() {
                None
            } else {
                Some(schedules::parse_schedule(&when.text(), None, schedules::now_millis())?)
            };
            let typed = custom.text().trim().to_string();
            let chosen = REPEATS[repeat.selected() as usize].0;
            let repeat_id = if !typed.is_empty() {
                typed
            } else if !chosen.is_empty() {
                chosen.to_string()
            } else {
                first.as_ref().and_then(|w| w.repeat.clone()).unwrap_or_default()
            };
            Ok((
                KINDS[kind.selected() as usize].0.to_string(),
                prompt,
                title.text().trim().to_string(),
                ids[run_on.selected() as usize].clone(),
                repeat_id,
                first.map(|w| (w.at, w.day)),
            ))
        })
    };
    {
        let (read, ui2, error) = (read.clone(), ui.clone(), error.clone());
        test.connect_clicked(move |_| match read() {
            Ok((kind, prompt, title, run_on, _, _)) => {
                error.set_text("");
                let title = if title.is_empty() { prompt.chars().take(40).collect() } else { title };
                test_sheet(&ui2, Task { id: "draft".into(), title, prompt, kind, run_on, repeat: "once".into(), enabled: true, ..Default::default() });
            }
            Err(e) => error.set_text(&e),
        });
    }
    {
        let (w, ui2) = (win.clone(), ui.clone());
        save.connect_clicked(move |_| {
            let (kind, prompt, title, run_on, repeat_id, first) = match read() {
                Ok(x) => x,
                Err(e) => return error.set_text(&e),
            };
            let result = match &existing {
                Some(t) => schedules::update(
                    &t.id,
                    Edit {
                        title: Some(title),
                        prompt: Some(prompt),
                        next_run: first.map(|f| f.0),
                        repeat: if repeat_id.is_empty() { None } else { Some(repeat_id) },
                        kind: Some(kind),
                        run_on: Some(run_on),
                        day: first.map(|f| f.1),
                        ..Default::default()
                    },
                )
                .map(|_| ()),
                None => match first {
                    None => Err("Say when, for example \"tomorrow 9am\" or \"every weekday at 9\".".into()),
                    Some((at, day)) => schedules::add_new(schedules::New {
                        title,
                        prompt,
                        next_run: at,
                        repeat: if repeat_id.is_empty() { "once".into() } else { repeat_id },
                        project: project.clone(),
                        kind,
                        run_on,
                        day,
                        ..Default::default()
                    })
                    .map(|_| ()),
                },
            };
            match result {
                Ok(()) => {
                    w.close();
                    refresh(&ui2);
                }
                Err(e) => error.set_text(&e),
            }
        });
    }
    win.present();
}
