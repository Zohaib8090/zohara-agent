//! The Machines popup, the add wizard and the per-machine window (docs/SSH.md, docs/RUNNER.md, docs/VM-MIGRATION-PLAN.md sections 2, 4, 5, 6).
//!
//! The same screens exist on the web page and in the Android app. Every button here calls one of the service's `machine_*` operations
//! (`zo_core::service::machine_call`), which do the work over SSH, so the three clients cannot disagree about what a button does.
//! Nothing here runs on the main thread for long: SSH calls go through `settings::background`. A credential is never shown; the sentence a
//! person must agree to before credentials are copied is shown word for word and sent back unchanged (the service refuses anything else).

use crate::settings::{background, row};
use crate::{hbox, lbl, primary, secondary, vbox, Ui};
use gtk4 as gtk;
use gtk::prelude::*;
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use zo_core::store::machines::{self, Machine};

fn call(op: &str, mut req: Value) -> Result<Value, String> {
    req["op"] = json!(op);
    zo_core::service::machine_call(op, &req)
}

fn ago(ms: u64) -> String {
    let now = zo_core::store::schedules::now_millis();
    let m = now.saturating_sub(ms) / 60_000;
    match m {
        0 => "just now".into(),
        1..=59 => format!("{m} min ago"),
        60..=2879 => format!("{} h ago", m / 60),
        _ => format!("{} days ago", m / 1440),
    }
}

/// One line about a machine from its status: what a person wants to know at a glance.
pub(crate) fn state_text(s: Option<&Value>) -> String {
    let Some(s) = s else { return "Checking…".into() };
    if s["reachable"] != json!(true) {
        let e = s["error"].as_str().unwrap_or("");
        return if e.is_empty() { "Not reachable".into() } else { format!("Not reachable: {}", e.chars().take(140).collect::<String>()) };
    }
    if s["installed"] != json!(true) {
        return "Reachable · no runner".into();
    }
    let r = &s["runner"];
    let n = r["enabled_tasks"].as_u64().unwrap_or(0);
    let mut t = format!("Reachable · runner {}", r["version"].as_str().unwrap_or("?"));
    if r["timer_active"] == json!(false) {
        t.push_str(" · timer off");
    }
    t.push_str(&format!(" · {n} task{}", if n == 1 { "" } else { "s" }));
    if let Some(l) = r["last_run"].as_u64() {
        t.push_str(&format!(" · last run {}", ago(l)));
        if let Some(st) = r["last_status"].as_str() {
            t.push_str(&format!(" ({st})"));
        }
    }
    t
}

/// The dot's colour as a style class: green (runner works), amber (reachable, no runner), red (not reachable), grey (not checked yet).
pub(crate) fn dot_class(s: Option<&Value>) -> &'static str {
    match s {
        None => "mdot-wait",
        Some(s) if s["reachable"] != json!(true) => "mdot-bad",
        Some(s) if s["installed"] == json!(true) => "mdot-ok",
        Some(_) => "mdot-warn",
    }
}

fn dot(s: Option<&Value>) -> gtk::Label {
    let d = lbl("●", "mdot");
    d.add_css_class(dot_class(s));
    d.set_tooltip_text(Some(&state_text(s)));
    d.set_valign(gtk::Align::Center);
    d
}

fn say_label() -> gtk::Label {
    let l = lbl("", "field-hint");
    l.set_wrap(true);
    l.set_xalign(0.0);
    l.set_max_width_chars(70);
    l
}

/// The list of machines into `host` (used by the Machines page and the popup). The rows appear at once and get their dots when the
/// status calls (one SSH call per machine, all at the same time) are back. `again` redraws the list.
pub(crate) fn list_into(ui: &Rc<Ui>, host: &gtk::Box, again: Rc<dyn Fn()>) {
    while let Some(c) = host.first_child() {
        host.remove(&c);
    }
    let gen = {
        thread_local!(static GEN: Cell<u64> = const { Cell::new(0) });
        GEN.with(|g| {
            g.set(g.get() + 1);
            g.get()
        })
    };
    host.set_widget_name(&format!("mlist{gen}"));
    let list = machines::load();
    if list.is_empty() {
        let card = vbox(0);
        card.add_css_class("settings-card");
        card.append(&row("No machines yet", "Add a computer or a cloud server that Zohara Agent can reach over SSH.", None::<&gtk::Box>));
        host.append(&card);
        return;
    }
    let paint = {
        let (ui, host, again) = (ui.clone(), host.clone(), again.clone());
        move |status: Option<&Value>| {
            while let Some(c) = host.first_child() {
                host.remove(&c);
            }
            let card = vbox(0);
            card.add_css_class("settings-card");
            for m in machines::load() {
                let s = status.and_then(|v| v["machines"].as_array()).and_then(|a| a.iter().find(|x| x["name"] == json!(m.name)));
                let s = match (status, s) {
                    (Some(_), None) => Some(json!({"reachable": false, "error": "No answer."})),
                    (_, s) => s.cloned(),
                };
                card.append(&machine_row(&ui, &m, s.as_ref(), again.clone()));
            }
            host.append(&card);
        }
    };
    paint(None);
    let (host2, name) = (host.clone(), format!("mlist{gen}"));
    background(
        || call("machine_status", json!({})),
        move |r| {
            if host2.widget_name() != name || host2.root().is_none() {
                return;
            }
            if let Ok(v) = r {
                paint(Some(&v));
            }
        },
    );
}

fn machine_row(ui: &Rc<Ui>, m: &Machine, s: Option<&Value>, again: Rc<dyn Fn()>) -> gtk::Box {
    let r = hbox(12);
    r.add_css_class("settings-row");
    r.append(&dot(s));
    let col = vbox(3);
    col.set_hexpand(true);
    col.append(&lbl(&m.name, "pc-name"));
    let mut meta = format!("{}@{}:{}", m.user, m.host, m.port);
    if !m.note.is_empty() {
        meta.push_str(&format!("  ·  {}", m.note));
    }
    if !m.has_key() {
        meta.push_str("  ·  no key saved");
    }
    let meta = lbl(&meta, "pc-meta");
    meta.set_ellipsize(gtk::pango::EllipsizeMode::End);
    col.append(&meta);
    let st = say_label();
    st.set_text(&state_text(s));
    col.append(&st);
    r.append(&col);

    let test = secondary("Test");
    {
        let (name, st, again) = (m.name.clone(), st.clone(), again.clone());
        test.connect_clicked(move |b| {
            b.set_sensitive(false);
            st.set_text("Connecting…");
            let (n, b, st, again) = (name.clone(), b.clone(), st.clone(), again.clone());
            background(
                move || zo_core::tools::ssh::test(&n),
                move |r| {
                    st.set_text(&r.unwrap_or_else(|e| e));
                    b.set_sensitive(true);
                    let _ = &again;
                },
            );
        });
    }
    r.append(&test);
    let term = secondary("Terminal");
    {
        let (ui, name, st) = (ui.clone(), m.name.clone(), st.clone());
        term.connect_clicked(move |_| open_terminal(&ui, &name, &st));
    }
    r.append(&term);
    let manage = secondary("Manage…");
    {
        let (ui, name, again) = (ui.clone(), m.name.clone(), again);
        manage.connect_clicked(move |_| manage_sheet(&ui, &name, again.clone()));
    }
    r.append(&manage);
    r
}

/// Opens a terminal tab in the window that is an ssh session to the machine.
fn open_terminal(ui: &Rc<Ui>, name: &str, st: &gtk::Label) {
    match call("machine_terminal", json!({"name": name})) {
        Ok(_) => ui.dock.show("terminal"),
        Err(e) => st.set_text(&e),
    }
}

/// The popup next to the model pill on the chat screen.
pub(crate) fn popup(ui: &Rc<Ui>) {
    let (win, body, foot) = crate::sheet(ui, "Machines", 640);
    let host = vbox(0);
    body.append(&host);
    let load: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));
    let again: Rc<dyn Fn()> = {
        let (ui, host, load) = (ui.clone(), host.clone(), load.clone());
        Rc::new(move || {
            let again = load.borrow().clone();
            if let Some(a) = again {
                list_into(&ui, &host, a);
            }
        })
    };
    *load.borrow_mut() = Some(again.clone());
    let add = primary("Add machine…");
    {
        let (ui, again) = (ui.clone(), again.clone());
        add.connect_clicked(move |_| wizard(&ui, again.clone()));
    }
    let close = secondary("Close");
    {
        let w = win.clone();
        close.connect_clicked(move |_| w.close());
    }
    foot.append(&close);
    foot.append(&add);
    again();
    win.present();
}

/// Settings > Machines: the same list, plus the wizard.
pub(crate) fn page(ui: &Rc<Ui>, content: &gtk::Box) {
    let sec = vbox(0);
    sec.append(&lbl("Machines", "settings-head"));
    let top = vbox(0);
    top.add_css_class("settings-card");
    let add = secondary("Add machine");
    let host = vbox(0);
    let load: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));
    let again: Rc<dyn Fn()> = {
        let (ui, host, load) = (ui.clone(), host.clone(), load.clone());
        Rc::new(move || {
            let again = load.borrow().clone();
            if let Some(a) = again {
                list_into(&ui, &host, a);
            }
        })
    };
    *load.borrow_mut() = Some(again.clone());
    {
        let (ui, again) = (ui.clone(), again.clone());
        add.connect_clicked(move |_| wizard(&ui, again.clone()));
    }
    top.append(&row(
        "Let Zohara Agent work on another computer",
        "A cloud VM or a server you can reach with SSH. Zohara Agent makes a new key for it and shows you the one line to put on it. It can run commands there and read and write files, asking you first unless the mode is Bypass. The private key is kept in the system keyring.",
        Some(&add),
    ));
    sec.append(&top);
    let gap = vbox(0);
    gap.set_margin_top(12);
    gap.append(&host);
    sec.append(&gap);
    let note = vbox(0);
    note.add_css_class("settings-card");
    note.set_margin_top(12);
    note.append(&row(
        "Tasks that run while this computer is off",
        "Install the runner on a machine (Manage…) and its scheduled tasks keep running there, with their results reported back here. See docs/RUNNER.md.",
        None::<&gtk::Box>,
    ));
    sec.append(&note);
    content.append(&sec);
    again();
}

/// The add wizard: where it is -> a new key and the one line to put on the machine (or a one-time password, never kept) -> Test.
pub(crate) fn wizard(ui: &Rc<Ui>, done: Rc<dyn Fn()>) {
    let (win, body, foot) = crate::sheet(ui, "Add a machine", 600);
    let entry = |hint: &str| {
        let e = gtk::Entry::new();
        e.set_placeholder_text(Some(hint));
        e.add_css_class("field-input");
        e
    };
    body.append(&lbl("Zohara Agent makes a new key just for this machine. You put its public half on the machine, once.", "field-hint"));
    let (name, host, user, port, note) = (entry("cloud-vm"), entry("203.0.113.7  or  vm.example.com"), entry("ubuntu"), entry("22"), entry("what it is for (optional)"));
    port.set_text("22");
    body.append(&crate::field("Name (letters, digits, . _ -)", &name));
    body.append(&crate::field("Address", &host));
    body.append(&crate::field("User", &user));
    body.append(&crate::field("Port", &port));
    body.append(&crate::field("Note", &note));
    let status = say_label();
    body.append(&status);
    let cancel = secondary("Cancel");
    let next = primary("Next");
    foot.append(&cancel);
    foot.append(&next);
    {
        let w = win.clone();
        cancel.connect_clicked(move |_| w.close());
    }
    {
        let (w, ui2, body2, foot2, done) = (win.clone(), ui.clone(), body.clone(), foot.clone(), done);
        next.connect_clicked(move |b| {
            let req = json!({"name": name.text().trim(), "host": host.text().trim(), "user": user.text().trim(), "port": port.text().trim(), "note": note.text().trim()});
            b.set_sensitive(false);
            status.set_text("");
            let (b, status, w, ui3, body3, foot3, done) = (b.clone(), status.clone(), w.clone(), ui2.clone(), body2.clone(), foot2.clone(), done.clone());
            background(
                move || call("machine_new", req),
                move |r| match r {
                    Ok(v) => wizard_step2(&ui3, &w, &body3, &foot3, v, done),
                    Err(e) => {
                        status.set_text(&e);
                        b.set_sensitive(true);
                    }
                },
            );
        });
    }
    win.present();
}

fn wizard_step2(_ui: &Rc<Ui>, win: &crate::Sheet, body: &gtk::Box, foot: &gtk::Box, made: Value, done: Rc<dyn Fn()>) {
    for b in [body, foot] {
        while let Some(c) = b.first_child() {
            b.remove(&c);
        }
    }
    let name = made["name"].as_str().unwrap_or("").to_string();
    let line = made["authorize_command"].as_str().unwrap_or("").to_string();
    body.append(&lbl("Put this key on the machine. Run this one line there (it appends the key to ~/.ssh/authorized_keys):", "field-hint"));
    let tv = gtk::TextView::new();
    tv.set_monospace(true);
    tv.set_editable(false);
    tv.set_wrap_mode(gtk::WrapMode::Char);
    tv.set_top_margin(8);
    tv.set_bottom_margin(8);
    tv.set_left_margin(10);
    tv.set_right_margin(10);
    tv.buffer().set_text(&line);
    let sc = gtk::ScrolledWindow::new();
    sc.set_child(Some(&tv));
    sc.set_min_content_height(110);
    sc.add_css_class("field-input");
    body.append(&sc);
    let copy = secondary("Copy the line");
    copy.set_halign(gtk::Align::Start);
    {
        let line = line.clone();
        copy.connect_clicked(move |b| b.clipboard().set_text(&line));
    }
    body.append(&copy);
    body.append(&lbl("Or, if the machine accepts a password, give it once. It is used one time to put the key there and is never stored.", "field-hint"));
    let pw = gtk::PasswordEntry::new();
    pw.set_show_peek_icon(true);
    pw.add_css_class("field-input");
    pw.set_hexpand(true);
    let usepw = secondary("Use it once");
    let prow = hbox(8);
    prow.append(&pw);
    prow.append(&usepw);
    body.append(&prow);
    let status = say_label();
    body.append(&status);
    {
        let (name, status, pw) = (name.clone(), status.clone(), pw.clone());
        usepw.connect_clicked(move |b| {
            let p = pw.text().to_string();
            if p.is_empty() {
                return;
            }
            pw.set_text("");
            b.set_sensitive(false);
            status.set_text("Connecting…");
            let (n, b, status) = (name.clone(), b.clone(), status.clone());
            background(
                move || call("machine_authorize", json!({"name": n, "password": p})),
                move |r| {
                    status.set_text(&match r {
                        Ok(v) => v["message"].as_str().unwrap_or("Done.").to_string(),
                        Err(e) => e,
                    });
                    b.set_sensitive(true);
                },
            );
        });
    }
    let test = primary("Test connection");
    {
        let (name, status) = (name.clone(), status.clone());
        test.connect_clicked(move |b| {
            b.set_sensitive(false);
            status.set_text("Connecting…");
            let (n, b, status) = (name.clone(), b.clone(), status.clone());
            background(
                move || zo_core::tools::ssh::test(&n),
                move |r| {
                    status.set_text(&r.unwrap_or_else(|e| e));
                    b.set_sensitive(true);
                },
            );
        });
    }
    let finish = secondary("Done");
    {
        let w = win.clone();
        finish.connect_clicked(move |_| {
            w.close();
            done();
        });
    }
    foot.append(&finish);
    foot.append(&test);
}

/// Shows the sentence a person agrees to word for word; "Confirm" is enabled only after the box is ticked, and the sentence goes back
/// to the service unchanged.
fn named_confirm(ui: &Rc<Ui>, title: &str, sentence: &str, ok: &str, run: impl Fn(&str) -> Result<String, String> + Send + Sync + 'static, finished: Rc<dyn Fn(String)>) {
    let (win, body, foot) = crate::sheet(ui, title, 520);
    body.append(&lbl("Your credentials are copied over SSH, straight from this computer to the machine, as files only you can read. You can revoke them on the machine afterwards.", "field-hint"));
    let check = gtk::CheckButton::with_label(sentence);
    body.append(&check);
    let status = say_label();
    body.append(&status);
    let cancel = secondary("Cancel");
    let go = primary(ok);
    go.set_sensitive(false);
    foot.append(&cancel);
    foot.append(&go);
    {
        let go = go.clone();
        check.connect_toggled(move |c| go.set_sensitive(c.is_active()));
    }
    {
        let w = win.clone();
        cancel.connect_clicked(move |_| w.close());
    }
    let run = Arc::new(run);
    let sentence = sentence.to_string();
    let win2 = win.clone();
    go.connect_clicked(move |b| {
        b.set_sensitive(false);
        status.set_text("Copying…");
        let (run, s, b, status, w, finished) = (run.clone(), sentence.clone(), b.clone(), status.clone(), win2.clone(), finished.clone());
        background(
            move || run(&s),
            move |r| match r {
                Ok(m) => {
                    w.close();
                    finished(m);
                }
                Err(e) => {
                    status.set_text(&e);
                    b.set_sensitive(true);
                }
            },
        );
    });
    win.present();
}
use std::sync::Arc;

/// The window of one machine: runner, model, connected apps, "run a task here", remove.
pub(crate) fn manage_sheet(ui: &Rc<Ui>, name: &str, changed: Rc<dyn Fn()>) {
    let (win, body, foot) = crate::sheet(ui, name, 680);
    let holder = vbox(12);
    body.append(&holder);
    let msg = say_label();
    body.append(&msg);
    let close = secondary("Close");
    {
        let (w, changed) = (win.clone(), changed.clone());
        close.connect_clicked(move |_| {
            w.close();
            changed();
        });
    }
    foot.append(&close);
    let m = Rc::new(Manage { ui: ui.clone(), name: name.to_string(), holder, msg, win: win.clone(), changed, status: RefCell::new(None) });
    m.draw();
    m.reload();
    win.present();
}

struct Manage {
    ui: Rc<Ui>,
    name: String,
    holder: gtk::Box,
    msg: gtk::Label,
    win: crate::Sheet,
    changed: Rc<dyn Fn()>,
    status: RefCell<Option<Value>>,
}

impl Manage {
    /// Asks the machine again (one SSH call) and draws.
    fn reload(self: &Rc<Self>) {
        let (n, me) = (self.name.clone(), self.clone());
        background(
            move || call("machine_status", json!({"name": n})),
            move |r| {
                if let Ok(v) = r {
                    *me.status.borrow_mut() = v["machines"].get(0).cloned();
                }
                me.draw();
            },
        );
    }

    /// A button that runs one operation in the background and then asks the machine again.
    fn op_button(self: &Rc<Self>, label: &str, danger: bool, op: &'static str, extra: Value, ok_text: &'static str) -> gtk::Button {
        let b = if danger { secondary(label) } else { primary(label) };
        if danger {
            b.add_css_class("danger");
        }
        let me = self.clone();
        b.connect_clicked(move |b| {
            b.set_sensitive(false);
            me.msg.set_text("Working…");
            let mut req = extra.clone();
            req["name"] = json!(me.name);
            let me2 = me.clone();
            background(
                move || call(op, req),
                move |r| {
                    me2.msg.set_text(&match r {
                        Ok(v) => match v["steps"].as_array().and_then(|s| s.last()).and_then(Value::as_str) {
                            Some(last) => last.to_string(),
                            None => ok_text.to_string(),
                        },
                        Err(e) => e,
                    });
                    me2.reload();
                },
            );
        });
        b
    }

    fn draw(self: &Rc<Self>) {
        while let Some(c) = self.holder.first_child() {
            self.holder.remove(&c);
        }
        let s = self.status.borrow().clone();
        let card = vbox(0);
        card.add_css_class("settings-card");
        let top = hbox(12);
        top.add_css_class("settings-row");
        top.append(&dot(s.as_ref()));
        let col = vbox(3);
        col.append(&lbl("Status", "pc-name"));
        let t = lbl(&state_text(s.as_ref()), "pc-meta");
        t.set_wrap(true);
        t.set_max_width_chars(60);
        col.append(&t);
        top.append(&col);
        card.append(&top);

        let installed = s.as_ref().is_some_and(|s| s["installed"] == json!(true));
        let runner = s.as_ref().map(|s| s["runner"].clone()).unwrap_or(Value::Null);
        let btns = hbox(6);
        btns.append(&self.op_button(if installed { "Update" } else { "Install runner" }, false, "machine_runner_install", json!({}), "Done."));
        if installed {
            btns.append(&self.op_button("Remove runner", true, "machine_runner_uninstall", json!({}), "The runner is removed."));
        }
        card.append(&row(
            "Runner",
            if installed {
                "Installed. It runs this machine's own task list from a systemd timer, also when this computer is off. Its results come back here."
            } else {
                "A small program (zohara-agent-runner) that is copied to the machine over SSH, with its checksum checked. It lets scheduled tasks run there while this computer is off."
            },
            Some(&btns),
        ));
        if installed {
            for (key, title, sub) in [
                ("commands", "Let tasks run commands there", "Off: a task that needs to run a command is refused (nobody is there to ask)."),
                ("files", "Let tasks write files there", "Off: writing files is refused."),
            ] {
                let sw = gtk::Switch::new();
                sw.set_active(s.as_ref().is_some_and(|s| s[key] == json!(true)));
                let me = self.clone();
                sw.connect_state_set(move |sw, on| {
                    let cur = me.status.borrow().clone().unwrap_or(Value::Null);
                    let want = |k: &str| if k == key { on } else { cur[k] == json!(true) };
                    match call("machine_runner_allow", json!({"name": me.name, "commands": want("commands"), "files": want("files")})) {
                        Ok(_) => {
                            if let Some(st) = me.status.borrow_mut().as_mut() {
                                st[key] = json!(on);
                            }
                            me.msg.set_text("");
                            sw.set_state(on);
                        }
                        Err(e) => me.msg.set_text(&e),
                    }
                    gtk::glib::Propagation::Stop
                });
                card.append(&row(title, sub, Some(&sw)));
            }
            let mb = hbox(6);
            let copy = secondary("Copy model key…");
            {
                let me = self.clone();
                copy.connect_clicked(move |_| me.model_copy());
            }
            mb.append(&copy);
            let has_model = runner["has_model"] == json!(true);
            if has_model {
                mb.append(&self.op_button("Remove", true, "machine_model_clear", json!({}), "Removed from the machine."));
            }
            let sub = if has_model {
                format!("{}{}", runner["model"].as_str().unwrap_or(""), if runner["has_key"] == json!(true) { " · key copied" } else { "" })
            } else {
                "Tasks there need a model. Copying sends your API key for it over SSH, after you confirm.".to_string()
            };
            card.append(&row("Model on this machine", &sub, Some(&mb)));
        }
        let copied: Vec<(String, bool)> = s
            .as_ref()
            .and_then(|s| s["apps_copied"]["servers"].as_array())
            .map(|a| a.iter().map(|x| (x["name"].as_str().unwrap_or("").to_string(), x["credentials"] == json!(true))).collect())
            .unwrap_or_default();
        let ab = hbox(6);
        let copy_apps = secondary("Copy connected apps…");
        {
            let me = self.clone();
            copy_apps.connect_clicked(move |_| me.apps_copy());
        }
        ab.append(&copy_apps);
        if !copied.is_empty() {
            let check = secondary("Check");
            {
                let me = self.clone();
                check.connect_clicked(move |b| {
                    b.set_sensitive(false);
                    me.msg.set_text("Checking…");
                    let (n, me2, b) = (me.name.clone(), me.clone(), b.clone());
                    background(
                        move || call("machine_apps_check", json!({"name": n})),
                        move |r| {
                            me2.msg.set_text(&match r {
                                Ok(v) => {
                                    let bad: Vec<String> = v["servers"].as_array().map(|a| a.iter().filter(|x| x["present"] != json!(true)).map(|x| format!("{}: {}", x["name"].as_str().unwrap_or(""), x["hint"].as_str().unwrap_or(""))).collect()).unwrap_or_default();
                                    if bad.is_empty() { "Everything the apps need is on the machine.".to_string() } else { bad.join(" ") }
                                }
                                Err(e) => e,
                            });
                            b.set_sensitive(true);
                        },
                    );
                });
            }
            ab.append(&check);
            let sync = secondary("Sync");
            {
                let me = self.clone();
                sync.connect_clicked(move |_| me.apps_sync());
            }
            ab.append(&sync);
            ab.append(&self.op_button("Revoke on machine", true, "machine_apps_revoke", json!({}), "Deleted from the machine."));
        }
        let sub = if copied.is_empty() {
            "Copy your connected apps (MCP servers) to the machine so its tasks can use them without signing in again.".to_string()
        } else {
            format!("Copied: {}", copied.iter().map(|(n, c)| if *c { format!("{n} (with credentials)") } else { n.clone() }).collect::<Vec<_>>().join(", "))
        };
        card.append(&row("Connected apps", &sub, Some(&ab)));

        // run a task here
        let task = gtk::Entry::new();
        task.set_placeholder_text(Some(&format!("What should it do on {}?", self.name)));
        task.add_css_class("field-input");
        task.set_hexpand(true);
        let run = primary("Run a task here");
        {
            let (me, task) = (self.clone(), task.clone());
            run.connect_clicked(move |b| {
                let p = task.text().trim().to_string();
                if p.is_empty() {
                    return;
                }
                b.set_sensitive(false);
                me.msg.set_text("Starting…");
                let (n, me2, b, task) = (me.name.clone(), me.clone(), b.clone(), task.clone());
                background(
                    move || call("machine_task_run", json!({"name": n, "prompt": p})),
                    move |r| {
                        me2.msg.set_text(&match r {
                            Ok(v) if v["by_runner"] == json!(true) => format!("Sent to the runner on {}. The answer comes back to you.", me2.name),
                            Ok(_) => "Started from here over SSH. It asks you first, as always.".to_string(),
                            Err(e) => e,
                        });
                        task.set_text("");
                        b.set_sensitive(true);
                    },
                );
            });
        }
        let tr = vbox(8);
        tr.set_hexpand(true);
        tr.append(&task);
        tr.append(&run);
        let trow = hbox(14);
        trow.add_css_class("settings-row");
        let tcol = vbox(3);
        tcol.set_hexpand(true);
        tcol.append(&lbl("Run a task here", "pc-name"));
        tcol.append(&lbl("One time, now.", "pc-meta"));
        tcol.append(&tr);
        trow.append(&tcol);
        card.append(&trow);

        let del = secondary("Remove");
        del.add_css_class("danger");
        {
            let me = self.clone();
            del.connect_clicked(move |_| {
                let me2 = me.clone();
                let name = me.name.clone();
                crate::confirm_delete(&me.ui, &format!("Remove \"{name}\"?"), "Zohara Agent forgets this machine and its private key. Nothing changes on the machine itself (the runner stays unless you remove it first).", move || {
                    let _ = machines::remove(&name);
                    me2.win.close();
                    (me2.changed)();
                });
            });
        }
        card.append(&row("Remove this machine", "Zohara Agent forgets it and its saved key. Nothing changes on the machine itself.", Some(&del)));
        self.holder.append(&card);
    }

    fn model_copy(self: &Rc<Self>) {
        let (n, me) = (self.name.clone(), self.clone());
        background(
            move || call("machine_model_plan", json!({"name": n})),
            move |r| match r {
                Err(e) => me.msg.set_text(&e),
                Ok(p) if p["can"] != json!(true) => me.msg.set_text(p["why"].as_str().unwrap_or("That cannot be copied.")),
                Ok(p) => {
                    let name = me.name.clone();
                    let me2 = me.clone();
                    named_confirm(
                        &me.ui,
                        &format!("Copy the model key to {}", me.name),
                        p["confirm"].as_str().unwrap_or(""),
                        "Copy key",
                        move |c| call("machine_model_copy", json!({"name": name, "confirm": c})).map(|v| v["message"].as_str().unwrap_or("Done.").to_string()),
                        Rc::new(move |m| {
                            me2.msg.set_text(&m);
                            me2.reload();
                        }),
                    );
                }
            },
        );
    }

    fn apps_sync(self: &Rc<Self>) {
        // the service answers with the sentence to confirm when credentials travel again
        let (n, me) = (self.name.clone(), self.clone());
        background(
            move || call("machine_apps_sync", json!({"name": n, "confirm": ""})),
            move |r| match r {
                Ok(v) => {
                    me.msg.set_text(&format!("In step: {} app(s).", v["synced"].as_u64().unwrap_or(0)));
                    me.reload();
                }
                Err(e) => match e.strip_prefix("Confirm exactly: ") {
                    Some(sentence) => {
                        let (name, me2) = (me.name.clone(), me.clone());
                        named_confirm(
                            &me.ui,
                            "Send the credentials again",
                            sentence,
                            "Sync",
                            move |c| call("machine_apps_sync", json!({"name": name, "confirm": c})).map(|v| format!("In step: {} app(s).", v["synced"].as_u64().unwrap_or(0))),
                            Rc::new(move |m| {
                                me2.msg.set_text(&m);
                                me2.reload();
                            }),
                        );
                    }
                    None => me.msg.set_text(&e),
                },
            },
        );
    }

    fn apps_copy(self: &Rc<Self>) {
        let (n, me) = (self.name.clone(), self.clone());
        background(
            move || call("machine_apps_plan", json!({"name": n})),
            move |r| match r {
                Err(e) => me.msg.set_text(&e),
                Ok(p) => me.apps_sheet(p),
            },
        );
    }

    fn apps_sheet(self: &Rc<Self>, plan: Value) {
        let servers = plan["servers"].as_array().cloned().unwrap_or_default();
        if servers.is_empty() {
            self.msg.set_text("There are no connected apps to copy.");
            return;
        }
        let (win, body, foot) = crate::sheet(&self.ui, &format!("Copy connected apps to {}", self.name), 640);
        let card = vbox(0);
        card.add_css_class("settings-card");
        let mut picks: Vec<(String, bool, gtk::CheckButton, gtk::CheckButton)> = Vec::new();
        for sv in &servers {
            let name = sv["name"].as_str().unwrap_or("").to_string();
            let labels: Vec<String> = sv["credentials"].as_array().map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
            let has_cred = !labels.is_empty() || sv["oauth"] == json!(true);
            let mut sub = format!("{} {}", if sv["kind"] == json!("remote") { "Address:" } else { "Starts:" }, sv["target"].as_str().unwrap_or(""));
            if has_cred {
                sub.push_str(&format!(" · needs {}", if labels.is_empty() { "a sign-in".to_string() } else { labels.join(", ") }));
            }
            if sv["copied"] == json!(true) {
                sub.push_str(" · already copied");
            }
            for w in sv["warnings"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                sub.push_str(&format!(" — {w}"));
            }
            let a = gtk::CheckButton::with_label("Copy it");
            a.set_active(sv["copied"] != json!(true));
            let c = gtk::CheckButton::with_label("with its credentials");
            c.set_visible(has_cred);
            let col = vbox(4);
            col.append(&a);
            col.append(&c);
            card.append(&row(&name, &sub, Some(&col)));
            picks.push((name, has_cred, a, c));
        }
        body.append(&card);
        let status = say_label();
        body.append(&status);
        let cancel = secondary("Cancel");
        let go = primary("Copy");
        foot.append(&cancel);
        foot.append(&go);
        {
            let w = win.clone();
            cancel.connect_clicked(move |_| w.close());
        }
        let me = self.clone();
        let w = win.clone();
        go.connect_clicked(move |b| {
            let chosen: Vec<Value> = picks.iter().filter(|p| p.2.is_active()).map(|p| json!({"name": p.0, "credentials": p.1 && p.3.is_active()})).collect();
            if chosen.is_empty() {
                status.set_text("Pick at least one app.");
                return;
            }
            b.set_sensitive(false);
            status.set_text("Copying…");
            let (n, me2, b, status, w) = (me.name.clone(), me.clone(), b.clone(), status.clone(), w.clone());
            let c1 = chosen.clone();
            background(
                move || call("machine_apps_copy", json!({"name": n, "servers": c1, "confirm": ""})),
                move |r| match r {
                    Ok(v) => {
                        w.close();
                        me2.msg.set_text(&format!("Copied {} app(s){}.", v["copied"].as_u64().unwrap_or(0), if v["credentials"].as_u64().unwrap_or(0) > 0 { " with credentials" } else { "" }));
                        me2.reload();
                    }
                    Err(e) => match e.strip_prefix("Confirm exactly: ") {
                        Some(sentence) => {
                            let (name, me3) = (me2.name.clone(), me2.clone());
                            w.close();
                            named_confirm(
                                &me2.ui,
                                &format!("Copy credentials to {}", me2.name),
                                sentence,
                                "Copy",
                                move |c| call("machine_apps_copy", json!({"name": name, "servers": chosen, "confirm": c})).map(|v| format!("Copied {} app(s) with credentials.", v["copied"].as_u64().unwrap_or(0))),
                                Rc::new(move |m| {
                                    me3.msg.set_text(&m);
                                    me3.reload();
                                }),
                            );
                        }
                        None => {
                            status.set_text(&e);
                            b.set_sensitive(true);
                        }
                    },
                },
            );
        });
        win.present();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_state_line_tells_what_a_person_needs() {
        assert_eq!(state_text(None), "Checking…");
        assert_eq!(state_text(Some(&json!({"reachable": false, "error": "Connection refused"}))), "Not reachable: Connection refused");
        assert_eq!(state_text(Some(&json!({"reachable": true, "installed": false}))), "Reachable · no runner");
        let ok = json!({"reachable": true, "installed": true, "runner": {"version": "0.1.0", "enabled_tasks": 1, "timer_active": false, "last_status": "ok"}});
        assert_eq!(state_text(Some(&ok)), "Reachable · runner 0.1.0 · timer off · 1 task");
    }

    #[test]
    fn the_dot_has_four_states() {
        assert_eq!(dot_class(None), "mdot-wait");
        assert_eq!(dot_class(Some(&json!({"reachable": false}))), "mdot-bad");
        assert_eq!(dot_class(Some(&json!({"reachable": true, "installed": false}))), "mdot-warn");
        assert_eq!(dot_class(Some(&json!({"reachable": true, "installed": true}))), "mdot-ok");
    }
}
