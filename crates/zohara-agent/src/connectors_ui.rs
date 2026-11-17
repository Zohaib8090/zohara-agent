//! Settings > Connected apps > "Browse connectors": the directory of remote MCP servers (docs/MCP.md "Browse connectors"). The list comes
//! from `zo_core::connectors` (through the Zohara Agent server when signed in); adding runs the same try-then-save steps as the web page
//! and the phone app (`connectors::add`), so an address is only ever the registry's own, public https, and a key goes to the key store.

use crate::{hbox, lbl, secondary, vbox, Ui};
use gtk4 as gtk;
use gtk::prelude::*;
use serde_json::Value;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::mpsc;
use zo_core::connectors::{self, add, Kind, Query, Source};
use zo_core::mcp_client;

const CATEGORY_NAMES: [&str; 9] = ["All", "Developer", "Documents", "Chat", "Data", "Finance", "Media", "Search", "Other"];
const CATEGORY_KEYS: [&str; 9] = ["all", "dev", "docs", "chat", "data", "finance", "media", "search", "other"];

struct State {
    q: String,
    category: String,
    kind: Kind,
    cursor: Option<String>,
    /// Bumps with every new search, so an answer to an old one is dropped.
    gen: u64,
}

/// Opens the directory. `changed` runs on the main thread after something was added (the list behind is drawn again).
pub(crate) fn browse_sheet(ui: &Rc<Ui>, changed: Rc<dyn Fn()>) {
    let (win, body, foot) = crate::sheet(ui, "Browse connectors", 680);
    body.append(&lbl("Connectors are tools from other services (Notion, GitHub, Linear and more) that the chat can use. They come from the public MCP registry. Only the ones marked Featured were looked at by Zohara Agent; every call of a connector still asks you first unless the mode is Bypass.", "field-hint"));
    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some("Search connectors"));
    search.add_css_class("field-input");
    let cats = gtk::DropDown::from_strings(&CATEGORY_NAMES);
    let remote_tab = secondary("Remote");
    let local_tab = secondary("Runs on this computer");
    let bar = hbox(8);
    search.set_hexpand(true);
    bar.append(&search);
    bar.append(&cats);
    body.append(&bar);
    let tabs = hbox(8);
    tabs.append(&remote_tab);
    tabs.append(&local_tab);
    body.append(&tabs);
    let note = lbl("", "field-hint");
    note.set_wrap(true);
    note.set_xalign(0.0);
    body.append(&note);
    let list = vbox(10);
    body.append(&list);
    let more = secondary("Load more");
    more.set_visible(false);
    body.append(&more);
    let close = secondary("Close");
    foot.append(&close);
    {
        let w = win.clone();
        close.connect_clicked(move |_| w.close());
    }
    let st = Rc::new(RefCell::new(State { q: String::new(), category: "all".into(), kind: Kind::Remote, cursor: None, gen: 0 }));
    let ctx = Rc::new(Ctx { ui: ui.clone(), list, note, more: more.clone(), st: st.clone(), changed });
    let reload: Rc<dyn Fn()> = {
        let ctx = ctx.clone();
        Rc::new(move || load(&ctx, false))
    };
    {
        let (st, reload) = (st.clone(), reload.clone());
        search.connect_activate(move |e| {
            st.borrow_mut().q = e.text().trim().to_string();
            reload();
        });
    }
    {
        let (st, reload) = (st.clone(), reload.clone());
        cats.connect_selected_notify(move |d| {
            st.borrow_mut().category = CATEGORY_KEYS.get(d.selected() as usize).copied().unwrap_or("all").to_string();
            reload();
        });
    }
    for (b, kind) in [(&remote_tab, Kind::Remote), (&local_tab, Kind::Local)] {
        let (st, reload) = (st.clone(), reload.clone());
        let (a, c) = (remote_tab.clone(), local_tab.clone());
        b.connect_clicked(move |_| {
            st.borrow_mut().kind = kind;
            for (btn, k) in [(&a, Kind::Remote), (&c, Kind::Local)] {
                if k == kind { btn.add_css_class("primary") } else { btn.remove_css_class("primary") }
            }
            reload();
        });
    }
    remote_tab.add_css_class("primary");
    {
        let ctx = ctx.clone();
        more.connect_clicked(move |_| load(&ctx, true));
    }
    win.present();
    load(&ctx, false);
}

struct Ctx {
    ui: Rc<Ui>,
    list: gtk::Box,
    note: gtk::Label,
    more: gtk::Button,
    st: Rc<RefCell<State>>,
    changed: Rc<dyn Fn()>,
}

fn load(ctx: &Rc<Ctx>, append: bool) {
    let (q, gen) = {
        let mut s = ctx.st.borrow_mut();
        if !append {
            s.cursor = None;
            s.gen += 1;
        }
        let q = Query { q: s.q.clone(), cursor: s.cursor.clone(), kind: s.kind, category: Some(s.category.clone()).filter(|c| c != "all") };
        (q, s.gen)
    };
    if !append {
        while let Some(c) = ctx.list.first_child() {
            ctx.list.remove(&c);
        }
    }
    ctx.more.set_visible(false);
    ctx.note.set_text("Loading…");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(connectors::browse_for_client(&q));
    });
    let ctx = ctx.clone();
    gtk::glib::timeout_add_local(std::time::Duration::from_millis(120), move || match rx.try_recv() {
        Ok(res) => {
            if ctx.st.borrow().gen != gen {
                return gtk::glib::ControlFlow::Break;
            }
            match res {
                Err(e) => ctx.note.set_text(&e),
                Ok(v) => show(&ctx, &v),
            }
            gtk::glib::ControlFlow::Break
        }
        Err(mpsc::TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
        Err(_) => gtk::glib::ControlFlow::Break,
    });
}

fn show(ctx: &Rc<Ctx>, v: &Value) {
    if v["restricted"].as_bool() == Some(true) {
        ctx.note.set_text(v["message"].as_str().unwrap_or(connectors::RESTRICTED_MESSAGE));
        return;
    }
    let mut note = v["error"].as_str().unwrap_or("").to_string();
    let entries: Vec<&Value> = v["featured"].as_array().into_iter().chain(v["entries"].as_array()).flatten().collect();
    if entries.is_empty() && ctx.list.first_child().is_none() {
        note = if note.is_empty() { "Nothing found.".into() } else { note };
    }
    ctx.note.set_text(&note);
    let added: Vec<String> = mcp_client::load_servers().into_iter().filter_map(|s| s.registry).collect();
    for e in entries {
        ctx.list.append(&card(ctx, e, added.iter().any(|a| Some(a.as_str()) == e["id"].as_str())));
    }
    let next = v["next"].as_str().map(str::to_string);
    ctx.more.set_visible(next.is_some());
    ctx.st.borrow_mut().cursor = next;
}

fn card(ctx: &Rc<Ctx>, e: &Value, added: bool) -> gtk::Box {
    let r = hbox(12);
    r.add_css_class("settings-row");
    // a letter instead of the publisher's picture: loading a picture would tell its publisher who is browsing
    let letter = lbl(&e["title"].as_str().unwrap_or("?").chars().next().unwrap_or('?').to_uppercase().to_string(), "pc-name");
    letter.set_width_chars(3);
    r.append(&letter);
    let col = vbox(3);
    col.set_hexpand(true);
    col.append(&lbl(e["title"].as_str().unwrap_or(""), "pc-name"));
    let trust = e["trust"].as_str().unwrap_or("");
    let meta = lbl(&format!("{}  ·  {}", e["publisher"].as_str().unwrap_or(""), trust), "pc-meta");
    meta.set_xalign(0.0);
    col.append(&meta);
    let d = lbl(e["description"].as_str().unwrap_or(""), "field-hint");
    d.set_wrap(true);
    d.set_xalign(0.0);
    col.append(&d);
    r.append(&col);
    let b = if added { secondary("Added") } else { crate::primary("Add") };
    b.set_valign(gtk::Align::Center);
    b.set_sensitive(!added);
    let (ctx2, e2) = (ctx.clone(), e.clone());
    b.connect_clicked(move |_| add_sheet(&ctx2, &e2));
    r.append(&b);
    r
}

/// What the person answers before a connector is tried: its name, the keys it asks for, and, for a program that runs here, the typed-back
/// confirmation. Then it is tried (its tools are listed), and only after a second yes it is saved.
fn add_sheet(ctx: &Rc<Ctx>, e: &Value) {
    let local = ctx.st.borrow().kind == Kind::Local;
    let title = e["title"].as_str().unwrap_or("Connector").to_string();
    let (win, body, foot) = crate::sheet(&ctx.ui, &format!("Add {title}"), 560);
    body.append(&lbl(&format!("{}  ·  {}", e["publisher"].as_str().unwrap_or(""), e["trust"].as_str().unwrap_or("")), "pc-meta"));
    if let Some(w) = e["website"].as_str() {
        body.append(&lbl(w, "field-hint"));
    }
    let name = gtk::Entry::new();
    name.set_text(&mcp_client::clean_name(e["id"].as_str().and_then(|i| i.split('/').nth(1)).unwrap_or("connector")));
    name.add_css_class("field-input");
    body.append(&crate::field("Name", &name));
    // the entry's own questions: (kind, name, widget)
    let mut asks: Vec<(bool, String, gtk::Editable)> = Vec::new();
    let mut url_pick: Option<(gtk::DropDown, Vec<String>)> = None;
    let mut confirm: Option<(gtk::CheckButton, String)> = None;
    if local {
        let l = &e["local"];
        let cmd = l["command"].as_str().unwrap_or("").to_string();
        body.append(&lbl(&format!("Needs {} on this computer.", l["needs"].as_str().unwrap_or("Node.js")), "field-hint"));
        let c = lbl(&cmd, "settings-code");
        c.set_selectable(true);
        c.set_wrap(true);
        body.append(&c);
        for v in l["env"].as_array().into_iter().flatten() {
            let en: gtk::Editable = if v["secret"].as_bool() == Some(true) { gtk::PasswordEntry::new().upcast() } else { gtk::Entry::new().upcast() };
            en.add_css_class("field-input");
            if let Some(d) = v["default"].as_str() {
                en.set_text(d);
            }
            body.append(&crate::field(&format!("{}{}", v["name"].as_str().unwrap_or(""), if v["required"].as_bool() == Some(true) { " (needed)" } else { "" }), &en));
            asks.push((false, v["name"].as_str().unwrap_or("").to_string(), en));
        }
        let ck = gtk::CheckButton::with_label(&format!("I understand that this runs {cmd} on this computer. It is third-party code and can do what I can do here."));
        body.append(&ck);
        confirm = Some((ck, cmd));
    } else {
        let remotes: Vec<&Value> = e["remotes"].as_array().into_iter().flatten().collect();
        let urls: Vec<String> = remotes.iter().filter_map(|r| r["url"].as_str().map(str::to_string)).collect();
        if urls.len() > 1 {
            let dd = gtk::DropDown::from_strings(&urls.iter().map(String::as_str).collect::<Vec<_>>());
            body.append(&crate::field("Address", &dd));
            url_pick = Some((dd, urls.clone()));
        } else if let Some(u) = urls.first() {
            body.append(&lbl(&format!("Address: {u}"), "field-hint"));
        }
        // headers of the first remote (a form for each: it names the header, e.g. Authorization: Bearer …)
        for h in remotes.first().and_then(|r| r["headers"].as_array()).into_iter().flatten().filter(|h| h.get("value").is_none()) {
            let en: gtk::Editable = if h["secret"].as_bool() == Some(true) { gtk::PasswordEntry::new().upcast() } else { gtk::Entry::new().upcast() };
            en.add_css_class("field-input");
            let n = h["name"].as_str().unwrap_or("");
            body.append(&crate::field(&format!("Header {n}{}", if h["required"].as_bool() == Some(true) { " (needed)" } else { "" }), &en));
            if let Some(d) = h["description"].as_str().filter(|d| !d.is_empty()) {
                body.append(&lbl(d, "field-hint"));
            }
            asks.push((true, n.to_string(), en));
        }
        body.append(&lbl("It is tried first. If it asks you to sign in, your browser opens.", "field-hint"));
    }
    let status = lbl("", "field-hint");
    status.set_wrap(true);
    status.set_xalign(0.0);
    body.append(&status);
    let (links, show_link) = crate::settings::link_row();
    body.append(&links);
    let cancel = secondary("Cancel");
    let go = crate::primary("Try it");
    foot.append(&cancel);
    foot.append(&go);
    let pending: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    {
        let (w, pending) = (win.clone(), pending.clone());
        cancel.connect_clicked(move |_| {
            if let Some(n) = pending.borrow_mut().take() {
                add::cancel(&n);
            }
            w.close();
        });
    }
    let id = e["id"].as_str().unwrap_or("").to_string();
    let (ctx2, win2) = (ctx.clone(), win.clone());
    go.connect_clicked(move |b| {
        // the second step: save what was tried
        if let Some(n) = pending.borrow().clone() {
            match add::commit(&n, connectors::now_secs()) {
                Ok(_) => {
                    pending.borrow_mut().take();
                    win2.close();
                    (ctx2.changed)();
                    load(&ctx2, false);
                }
                Err(e) => status.set_text(&e),
            }
            return;
        }
        let mut input = add::Input { id: id.clone(), kind: Some(if local { Kind::Local } else { Kind::Remote }), name: Some(name.text().trim().to_string()), ..Default::default() };
        if let Some((dd, urls)) = &url_pick {
            input.url = urls.get(dd.selected() as usize).cloned();
        }
        let (mut h, mut env): (BTreeMap<String, String>, BTreeMap<String, String>) = Default::default();
        for (is_header, n, en) in &asks {
            let t = en.text().to_string();
            if !t.trim().is_empty() {
                if *is_header { h.insert(n.clone(), t) } else { env.insert(n.clone(), t) };
            }
        }
        input.headers = h;
        input.env = env;
        if let Some((ck, cmd)) = &confirm {
            if ck.is_active() {
                input.confirm = Some(cmd.clone());
            }
        }
        status.set_text("Trying it… if it asks you to sign in, your browser opens.");
        b.set_sensitive(false);
        let (tx, rx) = mpsc::channel();
        let (url_tx, url_rx) = mpsc::channel::<String>();
        std::thread::spawn(move || {
            let open = move |u: &str| {
                let _ = url_tx.send(u.to_string());
                mcp_client::open_in_browser(u);
            };
            let opts = add::Opts { allow_private: false, open: &open };
            let _ = tx.send(add::preview(&Source::from_env(), connectors::now_secs(), &input, &opts));
        });
        let (status, b, pending, show_link) = (status.clone(), b.clone(), pending.clone(), show_link.clone());
        gtk::glib::timeout_add_local(std::time::Duration::from_millis(150), move || {
            while let Ok(u) = url_rx.try_recv() {
                show_link(&u);
            }
            match rx.try_recv() {
                Ok(Ok(p)) => {
                    let tools = if p.tools.is_empty() { "no tools".to_string() } else { p.tools.join(", ") };
                    status.set_text(&format!("It works. It offers these tools: {tools}.\n{}\nPress Add connector to keep it.", p.to_json()["notice"].as_str().unwrap_or("")));
                    *pending.borrow_mut() = Some(p.name);
                    b.set_label("Add connector");
                    b.set_sensitive(true);
                    gtk::glib::ControlFlow::Break
                }
                Ok(Err(e)) => {
                    status.set_text(&e);
                    b.set_sensitive(true);
                    gtk::glib::ControlFlow::Break
                }
                Err(mpsc::TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
                Err(_) => {
                    b.set_sensitive(true);
                    gtk::glib::ControlFlow::Break
                }
            }
        });
    });
    win.present();
}
