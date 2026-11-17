//! Settings > Import: bring the MCP servers and instructions of other tools (Claude Code, Claude Desktop, Codex CLI, Gemini CLI, Cursor,
//! VS Code, Zed) into Zohara Agent. The work is `zo_core::migrate` (docs/MIGRATE.md); this is the checklist around it. Detection is
//! read-only; a key is copied only for a server that has its own tick and after a confirmation that names it; every import can be undone.

use crate::{hbox, lbl, secondary, vbox, Ui};
use adw::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;
use serde_json::Value;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;
use zo_core::migrate::{self, Origin, Request, Scan};

/// The page body: where to look, and the last imports with their Undo.
pub(crate) fn page(ui: &Rc<Ui>, content: &gtk::Box, redraw: Rc<dyn Fn()>) {
    let (sec, card) = crate::settings::section("Find other tools' settings");
    card.append(&crate::settings::row(
        "",
        "Looks for the MCP servers and instruction files you already set up in Claude Code, Claude Desktop, Codex CLI, Gemini CLI, Cursor, VS Code and Zed. Looking is read-only and nothing leaves this computer. You choose what to bring in; nothing that exists here is overwritten.",
        crate::settings::no_control(),
    ));
    let here = crate::primary("Detect");
    {
        let (ui, redraw) = (ui.clone(), redraw.clone());
        here.connect_clicked(move |b| detect(&ui, Origin::Computer, b.clone(), redraw.clone()));
    }
    card.append(&crate::settings::row("This computer", "Reads the known files in your home folder.", Some(&here)));

    let machines = zo_core::store::machines::load();
    let names: Vec<String> = machines.iter().map(|m| m.name.clone()).collect();
    let pick = gtk::DropDown::from_strings(&names.iter().map(String::as_str).collect::<Vec<_>>());
    let go = crate::primary("Detect");
    let box_ = hbox(6);
    box_.append(&pick);
    box_.append(&go);
    if names.is_empty() {
        pick.set_sensitive(false);
        go.set_sensitive(false);
    }
    {
        let (ui, redraw) = (ui.clone(), redraw.clone());
        go.connect_clicked(move |b| {
            if let Some(n) = names.get(pick.selected() as usize) {
                detect(&ui, Origin::Machine(n.clone()), b.clone(), redraw.clone());
            }
        });
    }
    card.append(&crate::settings::row(
        "A machine (read-only over SSH)",
        if machines.is_empty() { "Add a machine in Settings > Machines first." } else { "Reads the same files there with ordinary read commands. Nothing is written on the machine." },
        Some(&box_),
    ));

    let files = hbox(6);
    let open = secondary("Open files…");
    let paste = secondary("Paste…");
    files.append(&open);
    files.append(&paste);
    {
        let (ui, redraw) = (ui.clone(), redraw.clone());
        open.connect_clicked(move |_| {
            let dialog = gtk::FileDialog::builder().title("Open settings files").build();
            let (ui2, redraw2) = (ui.clone(), redraw.clone());
            dialog.open_multiple(Some(&ui.window), gtk::gio::Cancellable::NONE, move |res| {
                let Ok(list) = res else { return };
                let mut given = Vec::new();
                for i in 0..list.n_items() {
                    let Some(f) = list.item(i).and_then(|o| o.downcast::<gtk::gio::File>().ok()) else { continue };
                    let Some(path) = f.path() else { continue };
                    if let Ok(text) = std::fs::read_to_string(&path) {
                        given.push((path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), text));
                    }
                }
                review_given(&ui2, given, redraw2.clone());
            });
        });
    }
    {
        let (ui, redraw) = (ui.clone(), redraw.clone());
        paste.connect_clicked(move |_| paste_sheet(&ui, redraw.clone()));
    }
    card.append(&crate::settings::row("A file you have", "A claude_desktop_config.json, .mcp.json, config.toml, settings.json, CLAUDE.md or AGENTS.md. Pasted text is read on this computer only.", Some(&files)));
    content.append(&sec);

    // the last imports, each with an Undo
    let history = migrate::history();
    if let Some(list) = history["batches"].as_array().filter(|l| !l.is_empty()) {
        let (sec, card) = crate::settings::section("Imports you can undo");
        for b in list {
            let undo = secondary("Undo");
            let (ui2, redraw2, id) = (ui.clone(), redraw.clone(), b["id"].as_str().unwrap_or("").to_string());
            undo.connect_clicked(move |_| {
                let (ui3, redraw3, id2) = (ui2.clone(), redraw2.clone(), id.clone());
                crate::confirm_delete(&ui2, "Undo this import?", "The servers, memory notes and project text it added are taken back, unless you changed them since (those stay).", move || {
                    let msg = match migrate::undo(&id2) {
                        Ok(v) => v["message"].as_str().unwrap_or("Undone.").to_string(),
                        Err(e) => e,
                    };
                    say(&ui3, "Undo", &msg);
                    redraw3();
                });
            });
            let when = zo_core::store::projects::relative(b["time"].as_u64().unwrap_or(0));
            card.append(&crate::settings::row(b["summary"].as_str().unwrap_or("An import"), &format!("{} · {}", b["origin"].as_str().unwrap_or(""), when), Some(&undo)));
        }
        content.append(&sec);
    }
}

fn say(ui: &Rc<Ui>, title: &str, msg: &str) {
    let d = adw::AlertDialog::new(Some(title), Some(msg));
    d.add_response("ok", "OK");
    d.present(Some(&ui.window));
}

/// Scans in the background and opens the checklist.
fn detect(ui: &Rc<Ui>, origin: Origin, button: gtk::Button, redraw: Rc<dyn Fn()>) {
    let label = button.label().map(|l| l.to_string()).unwrap_or_default();
    button.set_sensitive(false);
    button.set_label("Looking…");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(origin.scan());
    });
    let ui = ui.clone();
    gtk::glib::timeout_add_local(std::time::Duration::from_millis(150), move || match rx.try_recv() {
        Ok(res) => {
            button.set_sensitive(true);
            button.set_label(&label);
            match res {
                Ok(scan) => review(&ui, scan, redraw.clone()),
                Err(e) => say(&ui, "Nothing was read", &e),
            }
            gtk::glib::ControlFlow::Break
        }
        Err(mpsc::TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
        Err(_) => {
            button.set_sensitive(true);
            button.set_label(&label);
            gtk::glib::ControlFlow::Break
        }
    });
}

fn paste_sheet(ui: &Rc<Ui>, redraw: Rc<dyn Fn()>) {
    let (win, body, foot) = crate::sheet(ui, "Paste a settings file", 620);
    body.append(&lbl("Paste the text of one file: Claude Desktop's claude_desktop_config.json, a .mcp.json, Codex's config.toml, Cursor's mcp.json, VS Code's mcp.json, a Zed settings.json, or a CLAUDE.md / AGENTS.md / GEMINI.md. Nothing is imported until you tick it on the next screen.", "field-hint"));
    let name = gtk::Entry::new();
    name.add_css_class("field-input");
    name.set_placeholder_text(Some("File name (helps to recognise it), for example claude_desktop_config.json"));
    body.append(&name);
    let view = gtk::TextView::new();
    view.set_monospace(true);
    view.set_wrap_mode(gtk::WrapMode::WordChar);
    view.add_css_class("field-input");
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_child(Some(&view));
    scroll.set_min_content_height(220);
    body.append(&scroll);
    let cancel = secondary("Cancel");
    let go = crate::primary("Look at it");
    foot.append(&cancel);
    foot.append(&go);
    {
        let w = win.clone();
        cancel.connect_clicked(move |_| w.close());
    }
    let (ui2, win2) = (ui.clone(), win.clone());
    go.connect_clicked(move |_| {
        let buf = view.buffer();
        let text = buf.text(&buf.start_iter(), &buf.end_iter(), false).to_string();
        let n = name.text().trim().to_string();
        win2.close();
        review_given(&ui2, vec![(if n.is_empty() { "pasted.txt".into() } else { n }, text)], redraw.clone());
    });
    win.present();
}

fn review_given(ui: &Rc<Ui>, given: Vec<(String, String)>, redraw: Rc<dyn Fn()>) {
    match migrate::scan_given(&given) {
        Ok(scan) => review(ui, scan, redraw),
        Err(e) => say(ui, "Nothing was read", &e),
    }
}

/// One ticked line of the checklist.
struct ServerRow {
    id: String,
    name: String,
    tick: gtk::CheckButton,
    /// The tick for copying its keys, when the file holds some.
    keys: Option<(gtk::CheckButton, Vec<String>)>,
}

struct InstrRow {
    id: String,
    tick: gtk::CheckButton,
    target: gtk::DropDown,
    /// What each entry of `target` means: "notes", "notes:<id>" or "instruction:<id>".
    values: Vec<String>,
}

fn dim(l: &gtk::Label) {
    l.set_wrap(true);
    l.set_xalign(0.0);
}

/// The checklist of what was found.
fn review(ui: &Rc<Ui>, scan: Scan, redraw: Rc<dyn Fn()>) {
    let view = scan.view();
    let (win, body, foot) = crate::sheet(ui, &format!("Found on {}", view["origin"].as_str().unwrap_or("")), 720);
    let can_servers = view["can"]["servers"].as_bool() != Some(false);
    let tools: Vec<&Value> = view["tools"].as_array().into_iter().flatten().collect();
    let found: Vec<String> = tools.iter().filter(|t| t["found"].as_bool() == Some(true)).map(|t| t["name"].as_str().unwrap_or("").to_string()).collect();
    let nothing = view["servers"].as_array().map(Vec::is_empty).unwrap_or(true) && view["instructions"].as_array().map(Vec::is_empty).unwrap_or(true);
    let l = lbl(
        &if found.is_empty() { "None of the supported tools left settings there.".to_string() } else { format!("Settings of: {}.", found.join(", ")) },
        "field-hint",
    );
    dim(&l);
    body.append(&l);
    for n in view["notes"].as_array().into_iter().flatten() {
        let l = lbl(n.as_str().unwrap_or(""), "field-hint");
        dim(&l);
        body.append(&l);
    }
    let servers: Rc<RefCell<Vec<ServerRow>>> = Rc::new(RefCell::new(Vec::new()));
    let instrs: Rc<RefCell<Vec<InstrRow>>> = Rc::new(RefCell::new(Vec::new()));

    let list: Vec<&Value> = view["servers"].as_array().into_iter().flatten().collect();
    if !list.is_empty() {
        body.append(&lbl("MCP servers", "field-label"));
        let l = lbl("Servers are only saved, not started; the first time the chat uses one it asks first, like every other tool. Keys are not copied unless you tick that for the server.", "field-hint");
        dim(&l);
        body.append(&l);
        let mut by_tool: Vec<(&str, Vec<&Value>)> = Vec::new();
        for s in &list {
            let t = s["tool_name"].as_str().unwrap_or("");
            match by_tool.iter_mut().find(|(n, _)| *n == t) {
                Some((_, v)) => v.push(s),
                None => by_tool.push((t, vec![s])),
            }
        }
        for (tool, items) in by_tool {
            body.append(&lbl(tool, "pc-meta"));
            for s in items {
                let dup = s["duplicate"].as_str().unwrap_or("");
                let name = s["name"].as_str().unwrap_or("").to_string();
                let tick = gtk::CheckButton::with_label(&format!("{name}   ({})", if s["kind"] == "remote" { "remote" } else { "runs a program" }));
                tick.set_active(dup.is_empty() && can_servers);
                tick.set_sensitive(dup.is_empty() && can_servers);
                let col = vbox(3);
                col.set_margin_start(26);
                col.set_margin_bottom(6);
                let target = lbl(s["target"].as_str().unwrap_or(""), "settings-code");
                dim(&target);
                target.set_selectable(true);
                col.append(&target);
                let scope = match s["scope"].as_str() {
                    Some("project") | Some("local") => format!("{} · for the folder {}", s["source"].as_str().unwrap_or(""), s["folder"].as_str().filter(|f| !f.is_empty()).unwrap_or("")),
                    _ => s["source"].as_str().unwrap_or("").to_string(),
                };
                let mut meta = scope;
                if s["disabled"].as_bool() == Some(true) {
                    meta.push_str(" · switched off there (stays off)");
                }
                match dup {
                    "here" => meta.push_str(&format!(" · already added as {} (kept as it is)", s["duplicate_of"].as_str().unwrap_or(""))),
                    "scan" => meta.push_str(" · the same server is listed above; pick one"),
                    _ => {}
                }
                let m = lbl(&meta, "pc-meta");
                dim(&m);
                col.append(&m);
                for n in s["notes"].as_array().into_iter().flatten() {
                    let l = lbl(n.as_str().unwrap_or(""), "field-hint");
                    dim(&l);
                    col.append(&l);
                }
                let needs: Vec<&Value> = s["needs"].as_array().into_iter().flatten().collect();
                let mut keys = None;
                if !needs.is_empty() {
                    let names: Vec<String> = needs.iter().map(|n| match n["from"].as_str() {
                        Some(f) => format!("{} (from {f})", n["name"].as_str().unwrap_or("")),
                        None => n["name"].as_str().unwrap_or("").to_string(),
                    }).collect();
                    let l = lbl(&format!("Needs: {}. Without a key copied, it comes in with an empty placeholder to fill in later.", names.join(", ")), "field-hint");
                    dim(&l);
                    col.append(&l);
                    let in_file: Vec<String> = needs.iter().filter(|n| n["in_file"].as_bool() == Some(true)).map(|n| n["name"].as_str().unwrap_or("").to_string()).collect();
                    if !in_file.is_empty() && view["can"]["secrets"].as_bool() != Some(false) {
                        let k = gtk::CheckButton::with_label(&format!("Include the key{}: {}", if in_file.len() == 1 { "" } else { "s" }, in_file.join(", ")));
                        k.set_sensitive(false);
                        col.append(&k);
                        let k2 = k.clone();
                        tick.connect_toggled(move |t| {
                            k2.set_sensitive(t.is_active());
                            if !t.is_active() {
                                k2.set_active(false);
                            }
                        });
                        keys = Some((k, in_file));
                    }
                }
                body.append(&tick);
                body.append(&col);
                servers.borrow_mut().push(ServerRow { id: s["id"].as_str().unwrap_or("").to_string(), name, tick, keys });
            }
        }
    }

    let ilist: Vec<&Value> = view["instructions"].as_array().into_iter().flatten().collect();
    if !ilist.is_empty() {
        body.append(&lbl("Instructions", "field-label"));
        let l = lbl("An instruction file becomes memory notes (one per rule, shown in the Memory page and its updates feed), or is added to the end of a project's instruction. Lines that look like keys are left out.", "field-hint");
        dim(&l);
        body.append(&l);
        let projects: Vec<(String, String)> = view["projects"].as_array().into_iter().flatten().map(|p| (p["id"].as_str().unwrap_or("").to_string(), p["name"].as_str().unwrap_or("").to_string())).collect();
        for i in ilist {
            let tick = gtk::CheckButton::with_label(&format!("{} ({})", i["source"].as_str().unwrap_or(""), i["tool_name"].as_str().unwrap_or("")));
            tick.set_active(true);
            body.append(&tick);
            let col = vbox(4);
            col.set_margin_start(26);
            col.set_margin_bottom(6);
            let preview = i["preview"].as_str().unwrap_or("");
            let m = lbl(&format!("{} lines · {} note{}{}{}{}", i["lines"], i["notes"], if i["notes"] == 1 { "" } else { "s" }, if i["notes_more"].as_u64().unwrap_or(0) > 0 { format!(" (the first {}; {} more rules are not imported)", i["notes"], i["notes_more"]) } else { String::new() }, if i["secret_lines"].as_u64().unwrap_or(0) > 0 { format!(" · {} line(s) that look like keys are left out", i["secret_lines"]) } else { String::new() }, if preview.is_empty() { String::new() } else { format!("\n“{preview}”") }), "pc-meta");
            dim(&m);
            col.append(&m);
            let mut labels = vec!["Memory notes (everywhere)".to_string()];
            let mut values = vec!["notes".to_string()];
            for (id, name) in &projects {
                labels.push(format!("Add to the instruction of project {name}"));
                values.push(format!("instruction:{id}"));
                labels.push(format!("Memory notes of project {name}"));
                values.push(format!("notes:{id}"));
            }
            let dd = gtk::DropDown::from_strings(&labels.iter().map(String::as_str).collect::<Vec<_>>());
            if let Some(sid) = i["suggest"].as_str() {
                if let Some(p) = values.iter().position(|v| *v == format!("instruction:{sid}")) {
                    dd.set_selected(p as u32);
                }
            }
            col.append(&dd);
            body.append(&col);
            instrs.borrow_mut().push(InstrRow { id: i["id"].as_str().unwrap_or("").to_string(), tick, target: dd, values });
        }
    }

    let extras: Vec<&Value> = view["extras"].as_array().into_iter().flatten().collect();
    if !extras.is_empty() {
        body.append(&lbl("Found, but not imported", "field-label"));
        let text: Vec<String> = extras.iter().map(|e| format!("{} {}: {}", e["tool_name"].as_str().unwrap_or(""), e["kind"].as_str().unwrap_or(""), e["name"].as_str().unwrap_or(""))).collect();
        let l = lbl(&text.join("\n"), "field-hint");
        dim(&l);
        body.append(&l);
    }
    if nothing {
        let l = lbl("There is nothing to import here.", "field-hint");
        dim(&l);
        body.append(&l);
    }

    let status = lbl("", "field-hint");
    dim(&status);
    body.append(&status);
    let cancel = secondary("Close");
    let go = crate::primary("Import ticked");
    go.set_sensitive(!nothing);
    foot.append(&cancel);
    foot.append(&go);
    {
        let w = win.clone();
        cancel.connect_clicked(move |_| w.close());
    }
    let scan = Rc::new(scan);
    let (ui2, win2) = (ui.clone(), win.clone());
    go.connect_clicked(move |_| {
        let mut req = Request::default();
        let mut key_names: Vec<(String, Vec<String>)> = Vec::new();
        for s in servers.borrow().iter() {
            if !s.tick.is_active() {
                continue;
            }
            req.select.push(s.id.clone());
            if let Some((k, names)) = &s.keys {
                if k.is_active() {
                    req.secrets.push(s.id.clone());
                    key_names.push((s.name.clone(), names.clone()));
                }
            }
        }
        for i in instrs.borrow().iter() {
            if i.tick.is_active() {
                req.select.push(i.id.clone());
                if let Some(v) = i.values.get(i.target.selected() as usize) {
                    req.targets.insert(i.id.clone(), v.clone());
                }
            }
        }
        if req.select.is_empty() {
            status.set_text("Tick something first.");
            return;
        }
        let run = {
            let (scan, ui3, win3, redraw3, status) = (scan.clone(), ui2.clone(), win2.clone(), redraw.clone(), status.clone());
            move |req: Request| match migrate::apply_scan(&scan, &req) {
                Ok(out) => {
                    win3.close();
                    say(&ui3, "Imported", &summary(&out));
                    redraw3();
                }
                Err(e) => status.set_text(&e),
            }
        };
        if key_names.is_empty() {
            run(req);
            return;
        }
        // keys: an explicit confirmation that names every server and what is copied
        let list: Vec<String> = key_names.iter().map(|(s, n)| format!("{s}: {}", n.join(", "))).collect();
        let d = adw::AlertDialog::new(Some("Copy keys?"), Some(&format!("These keys are read from the other tool's files and saved in this computer's key store (not in the settings file, never in a log, never synced):\n\n{}\n\nOnly the servers listed get their keys.", list.join("\n"))));
        d.add_responses(&[("cancel", "Cancel"), ("copy", "Copy these keys")]);
        d.set_response_appearance("copy", adw::ResponseAppearance::Suggested);
        d.set_default_response(Some("cancel"));
        d.set_close_response("cancel");
        let confirm: Vec<String> = key_names.iter().map(|(s, _)| s.clone()).collect();
        d.connect_response(None, move |_, id| {
            if id == "copy" {
                let mut r = req.clone();
                r.confirm = confirm.clone();
                run(r);
            }
        });
        d.present(Some(&ui2.window));
    });
    win.present();
}

/// What the import did, in words.
fn summary(out: &Value) -> String {
    let mut s = vec![out["message"].as_str().unwrap_or("Done.").to_string()];
    for k in out["servers"]["skipped"].as_array().into_iter().flatten() {
        s.push(format!("Left out {}: {}", k["name"].as_str().unwrap_or(""), k["why"].as_str().unwrap_or("")));
    }
    for k in out["instructions"]["skipped"].as_array().into_iter().flatten() {
        s.push(format!("Left out {}: {}", k["source"].as_str().unwrap_or(""), k["why"].as_str().unwrap_or("")));
    }
    for k in out["secrets_copied"].as_array().into_iter().flatten() {
        s.push(format!("Keys copied for {}: {}", k["server"].as_str().unwrap_or(""), k["names"].as_array().into_iter().flatten().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")));
    }
    for k in out["needs"].as_array().into_iter().flatten() {
        let names: Vec<String> = k["needs"].as_array().into_iter().flatten().map(|n| n["name"].as_str().unwrap_or("").to_string()).collect();
        s.push(format!("{} still needs: {}", k["server"].as_str().unwrap_or(""), names.join(", ")));
    }
    if !out["batch"].as_str().unwrap_or("").is_empty() {
        s.push("You can undo this import in Settings > Import.".into());
    }
    s.join("\n")
}
