//! The model chooser ("Model and API key"): provider, key, address, a searchable list of the provider's models with a star on each to
//! pin it to the model menu, and the custom-endpoint flow. Nothing here waits for the network on the window's own thread: the
//! lists are fetched in a worker thread and kept for a minute per provider and address.

use crate::{chat, hbox, lbl, primary, secondary, sheet, vbox, Ui};
use adw::prelude::*;
use gtk::glib;
use gtk4 as gtk;
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use zo_core::ai::providers::{Protocol, Provider};
use zo_core::ai::{accounts, agent, custom, keys, profiles, providers, Selection};
use zo_core::ai::ui_prefs as uiprefs;

// ---------------------------------------------------------------- the model lists, fetched off the window thread

static CACHE: Mutex<Option<HashMap<String, (Instant, Vec<String>)>>> = Mutex::new(None);
const CACHE_FOR: Duration = Duration::from_secs(60);

/// One list per provider, address and account (another account has another key, so maybe another list).
pub(crate) fn cache_key(p: &Provider, base: &str, account: &str) -> String {
    format!("{}|{}|{}", p.id, base.trim().trim_end_matches('/'), account)
}

fn cached(p: &Provider, base: &str, account: &str) -> Option<Vec<String>> {
    let g = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    g.as_ref()?.get(&cache_key(p, base, account)).filter(|(t, _)| t.elapsed() < CACHE_FOR).map(|(_, v)| v.clone())
}

fn remember(p: &Provider, base: &str, account: &str, list: &[String]) {
    let mut g = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    g.get_or_insert_with(HashMap::new).insert(cache_key(p, base, account), (Instant::now(), list.to_vec()));
}

/// The key to use for `p` and the account the sheet shows: the typed one, else the saved one of that account (the active profile's own
/// key only when the profile is for exactly this provider and account).
pub(crate) fn key_for(p: &Provider, account: &str, typed: Option<String>) -> Option<String> {
    typed.filter(|k| !k.trim().is_empty()).or_else(|| {
        let acct = (account != accounts::DEFAULT_ID && !account.is_empty()).then(|| account.to_string());
        let mut s = Selection::load();
        if let Some(id) = s.profile.clone() {
            if !profiles::load().iter().any(|x| x.id == id && x.provider == p.id && x.account == acct) {
                s.profile = None;
            }
        }
        s.provider = p.id.to_string();
        s.account = acct;
        profiles::get_key(&s, p.id)
    })
}

/// What went wrong, in words a person can act on.
pub(crate) fn friendly_error(e: &str) -> String {
    // the format hints and the sentences probe already made are final
    if e.starts_with("This server speaks") || e.starts_with("The server does not take") || e.starts_with("No API key") || e.starts_with("Nothing answers at") || e.contains(" refused the key") {
        return e.to_string();
    }
    let l = e.to_lowercase();
    if l.contains("answered 401") || l.contains("answered 403") {
        "The provider did not accept the key (401). Check it and try again.".into()
    } else if l.contains("answered 404") {
        "Nothing is listed at that address (404). It usually ends in /v1.".into()
    } else if l.contains("answered 429") {
        "The provider says too many requests (429). Wait a moment and retry.".into()
    } else if l.contains("timed out") || l.contains("timeout") {
        "The address did not answer in time. Check it, and that the server is running.".into()
    } else if l.contains("could not reach") {
        "Could not reach that address. Check it, and that the server is running.".into()
    } else if l.contains("not valid json") || l.contains("expected") && l.contains("json") {
        "That address answered, but not like a model server (the reply was not a model list).".into()
    } else {
        e.to_string()
    }
}

/// Fetches the model list in a worker thread and hands the result to `done` on the window's thread. A list fetched in the last
/// minute for the same provider, address and account is used as it is, unless `force`. `account` is the account (API key) of the provider
/// the sheet shows ("default" for the one every provider has).
pub(crate) fn fetch_models(p: &'static Provider, base: String, typed_key: Option<String>, account: String, force: bool, done: impl FnOnce(Result<Vec<String>, String>) + 'static) {
    if !force {
        if let Some(list) = cached(p, &base, &account) {
            glib::idle_add_local_once(move || done(Ok(list)));
            return;
        }
    }
    glib::spawn_future_local(async move {
        let (b2, a2) = (base.clone(), account.clone());
        let r = gtk::gio::spawn_blocking(move || {
            let key = key_for(p, &account, typed_key);
            if !p.local && !p.is_custom() && p.protocol != Protocol::ClaudeCode && key.is_none() {
                return Err("No API key yet. Paste it above, then press Retry.".to_string());
            }
            let b = if base.trim().is_empty() { p.base_url.to_string() } else { base };
            // probe::list checks the address and words the failure, including "this server speaks another format: set the API format ..."
            zo_core::ai::probe::list(p, &b, key.as_deref()).map_err(|e| friendly_error(&e))
        })
        .await
        .unwrap_or_else(|_| Err("The check stopped unexpectedly.".to_string()));
        if let Ok(list) = &r {
            remember(p, &b2, &a2, list);
        }
        done(r);
    });
}

/// "Test connection". With a model name it asks one tiny question in the provider's wire format and, when the server speaks another
/// one, says which API format to use; without a model it is the list call, reported as one friendly line.
pub(crate) fn test_connection(p: &'static Provider, base: String, typed_key: Option<String>, account: String, model: String, done: impl FnOnce(Result<String, String>) + 'static) {
    if model.trim().is_empty() {
        fetch_models(p, base, typed_key, account, true, move |r| {
            done(r.map(|l| if l.is_empty() { "Connected, but it listed no models.".to_string() } else { format!("Connected. {} models available.", l.len()) }));
        });
        return;
    }
    glib::spawn_future_local(async move {
        let r = gtk::gio::spawn_blocking(move || {
            let key = key_for(p, &account, typed_key);
            if !p.local && !p.is_custom() && p.protocol != Protocol::ClaudeCode && key.is_none() {
                return Err("No API key yet. Paste it above, then press Test.".to_string());
            }
            let b = if base.trim().is_empty() { p.base_url.to_string() } else { base };
            zo_core::ai::probe::test(p, &b, key, &model)
        })
        .await
        .unwrap_or_else(|_| Err("The check stopped unexpectedly.".to_string()));
        done(r);
    });
}

/// The line under the address of a custom provider: which wire format it speaks and what an address looks like.
pub(crate) fn endpoint_hint(format: &str) -> &'static str {
    match format {
        "responses" => "Any server that speaks the OpenAI Responses API (it answers at /v1/responses), for example an agent router: https://my-host.example/v1.",
        "claude" => "Any server that speaks the Claude format (it answers at /v1/messages): https://my-host.example/v1.",
        _ => "The address of any OpenAI-compatible server (it answers at /v1/chat/completions), for example http://192.168.1.20:8000/v1 or https://my-host.example/v1.",
    }
}

/// The three API formats a custom provider can speak: (id the service takes, what the person reads).
pub(crate) const FORMATS: [(&str, &str); 3] = [("chat", "OpenAI Chat (/v1/chat/completions)"), ("responses", "OpenAI Responses (/v1/responses)"), ("claude", "Claude format (/v1/messages)")];

/// The wire format of a provider as the picker words it (`chat`, `responses`, `claude`).
pub(crate) fn format_id(p: &Provider) -> &'static str {
    match p.protocol {
        Protocol::OpenAiResponses => "responses",
        Protocol::Anthropic => "claude",
        _ => "chat",
    }
}

/// The providers shown in the pickers: the ones that are not hidden, and always the one in use. The built-in ones come first, then the
/// custom providers the person named.
pub(crate) fn visible_providers(keep: &str) -> Vec<&'static Provider> {
    let hidden = uiprefs::hidden_providers();
    providers::listed().into_iter().filter(|p| p.id == keep || !hidden.iter().any(|h| h == p.id)).collect()
}

/// A provider where the person types the address: custom endpoints and local servers.
fn needs_url(p: &Provider) -> bool {
    p.is_custom() || (p.local && p.protocol != Protocol::ClaudeCode)
}

/// The pinned models of the active profile, when it belongs to this provider.
fn pins_of(sel: &Selection, p: &Provider) -> Vec<String> {
    sel.profile.as_deref().and_then(|id| profiles::load().into_iter().find(|x| x.id == id)).filter(|x| x.provider == p.id).map(|x| x.pinned).unwrap_or_default()
}

// ---------------------------------------------------------------- the model list widget

/// A searchable list of models with a star on each. Pinned ones come first, under their own header.
#[derive(Clone)]
struct ModelList {
    root: gtk::Box,
    list: gtk::ListBox,
    search: gtk::SearchEntry,
    banner: gtk::Box,
    spinner: gtk::Spinner,
    message: gtk::Label,
    retry: gtk::Button,
    count: gtk::Label,
    pinned: Rc<RefCell<Vec<String>>>,
    order: Rc<RefCell<HashMap<String, usize>>>,
    /// Lets an answer that arrives late (for a provider that is no longer shown) be dropped.
    generation: Rc<Cell<u32>>,
    /// The fill that is adding rows now (a newer fill stops an older one).
    fill_gen: Rc<Cell<u32>>,
    current: Rc<RefCell<String>>,
    on_pick: Rc<RefCell<Option<Rc<dyn Fn(&str)>>>>,
}

impl ModelList {
    fn new() -> ModelList {
        let root = vbox(8);
        let head = hbox(8);
        let title = lbl("Models", "field-label");
        title.set_hexpand(true);
        let count = lbl("", "field-hint");
        head.append(&title);
        head.append(&count);
        let reload = crate::icon_button("view-refresh-symbolic", "Load the list again", "icon-btn");
        head.append(&reload);
        root.append(&head);

        let banner = hbox(8);
        let spinner = gtk::Spinner::new();
        let message = lbl("", "field-hint");
        message.set_wrap(true);
        message.set_hexpand(true);
        let retry = secondary("Retry");
        banner.append(&spinner);
        banner.append(&message);
        banner.append(&retry);
        banner.set_visible(false);
        root.append(&banner);

        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some("Search models"));
        root.append(&search);

        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::None);
        list.set_activate_on_single_click(true);
        list.add_css_class("model-list");
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_child(Some(&list));
        scroll.set_min_content_height(200);
        scroll.set_max_content_height(200);
        scroll.set_hscrollbar_policy(gtk::PolicyType::Never);
        scroll.add_css_class("model-scroll");
        root.append(&scroll);

        let ml = ModelList {
            root,
            list: list.clone(),
            search: search.clone(),
            banner,
            spinner,
            message,
            retry,
            count,
            pinned: Rc::new(RefCell::new(Vec::new())),
            order: Rc::new(RefCell::new(HashMap::new())),
            generation: Rc::new(Cell::new(0)),
            fill_gen: Rc::new(Cell::new(0)),
            current: Rc::new(RefCell::new(String::new())),
            on_pick: Rc::new(RefCell::new(None)),
        };

        // filter by showing and hiding rows: typing never rebuilds the list
        {
            let search2 = search.clone();
            list.set_filter_func(move |row| {
                let q = search2.text().to_lowercase();
                q.is_empty() || row.widget_name().to_lowercase().contains(q.as_str())
            });
            let list2 = list.clone();
            search.connect_search_changed(move |_| list2.invalidate_filter());
        }
        // pinned first (in the order they were pinned), then the provider's own order
        {
            let (pinned, order) = (ml.pinned.clone(), ml.order.clone());
            list.set_sort_func(move |a, b| {
                let key = |r: &gtk::ListBoxRow| {
                    let name = r.widget_name();
                    let pin = pinned.borrow().iter().position(|m| m.as_str() == name.as_str());
                    match pin {
                        Some(i) => (0, i),
                        None => (1, order.borrow().get(name.as_str()).copied().unwrap_or(usize::MAX)),
                    }
                };
                key(a).cmp(&key(b)).into()
            });
        }
        {
            let pinned = ml.pinned.clone();
            list.set_header_func(move |row, before| {
                let is_pin = |r: &gtk::ListBoxRow| pinned.borrow().iter().any(|m| m.as_str() == r.widget_name().as_str());
                let any = !pinned.borrow().is_empty();
                let mine = is_pin(row);
                let theirs = before.map(is_pin);
                let header = match (before.is_none(), theirs) {
                    (true, _) if mine => Some("PINNED"),
                    (true, _) if any => Some("ALL MODELS"),
                    (false, Some(true)) if !mine => Some("ALL MODELS"),
                    _ => None,
                };
                match header {
                    Some(t) => row.set_header(Some(&lbl(t, "pop-head"))),
                    None => row.set_header(None::<&gtk::Widget>),
                }
            });
        }
        {
            let ml2 = ml.clone();
            list.connect_row_activated(move |_, row| {
                let name = row.widget_name().to_string();
                let f = ml2.on_pick.borrow().clone();
                if let Some(f) = f {
                    f(&name);
                }
            });
        }
        let _ = reload;
        ml
    }

    fn set_loading(&self, text: &str) {
        self.banner.set_visible(true);
        self.spinner.set_visible(true);
        self.spinner.start();
        self.message.set_text(text);
        self.retry.set_visible(false);
    }

    fn set_note(&self, text: &str, retry: bool) {
        self.spinner.stop();
        self.spinner.set_visible(false);
        self.banner.set_visible(!text.is_empty());
        self.message.set_text(text);
        self.retry.set_visible(retry);
    }

    fn mark_current(&self, model: &str) {
        *self.current.borrow_mut() = model.to_string();
        let mut c = self.list.first_child();
        while let Some(w) = c {
            if let Some(row) = w.downcast_ref::<gtk::ListBoxRow>() {
                if let Some(tick) = row.child().and_then(|b| b.last_child()) {
                    tick.set_visible(row.widget_name().as_str() == model);
                }
            }
            c = w.next_sibling();
        }
    }

    fn star_icon(on: bool) -> &'static str {
        if on { "starred-symbolic" } else { "non-starred-symbolic" }
    }

    /// Fills the list: the pinned models (even when the provider could not be asked) and the provider's list. Rows are added a
    /// batch at a time so a list of hundreds does not freeze the window.
    fn fill(&self, fetched: &[String]) {
        while let Some(c) = self.list.first_child() {
            self.list.remove(&c);
        }
        let mut names: Vec<String> = self.pinned.borrow().clone();
        for m in fetched {
            if !names.contains(m) {
                names.push(m.clone());
            }
        }
        *self.order.borrow_mut() = names.iter().enumerate().map(|(i, m)| (m.clone(), i)).collect();
        self.count.set_text(&if fetched.is_empty() { String::new() } else { format!("{} models", fetched.len()) });
        let ml = self.clone();
        let gen = self.fill_gen.get().wrapping_add(1);
        self.fill_gen.set(gen);
        let todo = Rc::new(RefCell::new(names.into_iter()));
        glib::idle_add_local(move || {
            if ml.fill_gen.get() != gen {
                return glib::ControlFlow::Break;
            }
            for _ in 0..60 {
                let Some(m) = todo.borrow_mut().next() else {
                    ml.list.invalidate_sort();
                    ml.list.invalidate_headers();
                    let cur = ml.current.borrow().clone();
                    ml.mark_current(&cur);
                    return glib::ControlFlow::Break;
                };
                ml.list.append(&ml.row(&m));
            }
            ml.list.invalidate_sort();
            ml.list.invalidate_headers();
            glib::ControlFlow::Continue
        });
    }

    fn row(&self, model: &str) -> gtk::ListBoxRow {
        let row = gtk::ListBoxRow::new();
        row.set_widget_name(model);
        row.add_css_class("model-row");
        let line = hbox(8);
        let star = crate::icon_button(Self::star_icon(self.pinned.borrow().iter().any(|m| m == model)), "Pin to the model menu", "icon-btn star-btn");
        {
            let (ml, model2, star2) = (self.clone(), model.to_string(), star.clone());
            star.connect_clicked(move |_| {
                let model = model2.clone();
                let now_pinned = {
                    let mut p = ml.pinned.borrow_mut();
                    if let Some(i) = p.iter().position(|m| *m == model) {
                        p.remove(i);
                        false
                    } else {
                        p.push(model.clone());
                        true
                    }
                };
                star2.set_icon_name(Self::star_icon(now_pinned));
                star2.set_tooltip_text(Some(if now_pinned { "Unpin from the model menu" } else { "Pin to the model menu" }));
                ml.list.invalidate_sort();
                ml.list.invalidate_headers();
            });
            star.set_tooltip_text(Some(if self.pinned.borrow().iter().any(|m| *m == model) { "Unpin from the model menu" } else { "Pin to the model menu" }));
        }
        line.append(&star);
        let name = lbl(&chat::nice_name(model), "pop-title");
        name.set_hexpand(true);
        name.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        line.append(&name);
        let tick = crate::icon("object-select-symbolic", 16);
        tick.add_css_class("pop-check");
        tick.set_visible(false);
        line.append(&tick);
        row.set_child(Some(&line));
        row
    }
}

// ---------------------------------------------------------------- the chooser

pub fn models_sheet(ui: &Rc<Ui>) {
    models_sheet_at(ui, None);
}

/// The model chooser, opened on `start` = (provider id, account id) when given (a provider or account that was just added).
pub fn models_sheet_at(ui: &Rc<Ui>, start: Option<(String, String)>) {
    let (win, body, foot) = sheet(ui, "Model and API key", 620);
    let sel = Selection::load();
    let start_provider = start.as_ref().map(|(p, _)| p.clone()).unwrap_or_else(|| sel.provider.clone());
    let shown = Rc::new(RefCell::new(visible_providers(&start_provider)));

    let names: Vec<&str> = shown.borrow().iter().map(|p| p.name).collect();
    let provider_dd = gtk::DropDown::from_strings(&names);
    provider_dd.set_selected(shown.borrow().iter().position(|p| p.id == start_provider).unwrap_or(0) as u32);
    body.append(&crate::field("Provider", &provider_dd));
    let hidden_n = providers::listed().len() - shown.borrow().len();
    if hidden_n > 0 {
        body.append(&lbl(&format!("{hidden_n} providers are hidden. Settings > Model and profiles has a switch for each."), "field-hint"));
    }

    // the account (API key) of the provider: "Groq · Groq 1"; every provider with a key has the Default one
    let acct_names = gtk::StringList::new(&[]);
    let acct_dd = gtk::DropDown::new(Some(acct_names.clone()), None::<gtk::Expression>);
    let acct_ids: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let acct_field = crate::field("Account", &acct_dd);
    body.append(&acct_field);
    // the account chosen for the provider shown ("default" = the one every provider has)
    let filling = Rc::new(Cell::new(false));
    let acct_now = Rc::new(RefCell::new(start.as_ref().map(|(_, a)| a.clone()).unwrap_or_else(|| sel.account.clone().unwrap_or_else(|| accounts::DEFAULT_ID.to_string()))));

    let key = gtk::PasswordEntry::new();
    key.set_show_peek_icon(true);
    key.add_css_class("field-input");
    let key_field = crate::field("API key", &key);
    body.append(&key_field);

    // the address: always shown for custom and local servers, under "Advanced" for the others
    let url = gtk::Entry::new();
    url.add_css_class("field-input");
    url.set_hexpand(true);
    let test = secondary("Test connection");
    let url_row = hbox(8);
    url_row.append(&url);
    url_row.append(&test);
    let url_field = crate::field("Address", &url_row);
    let url_hint = lbl("", "field-hint");
    url_hint.set_wrap(true);
    url_field.append(&url_hint);
    // Can the model look at pictures? Normally decided from the model's name.
    let vision_dd = gtk::DropDown::from_strings(&["Decide from the model's name (recommended)", "This model can see pictures", "This model cannot see pictures"]);
    vision_dd.set_selected(match sel.vision {
        None => 0,
        Some(true) => 1,
        Some(false) => 2,
    });
    url_field.append(&crate::field("Pictures", &vision_dd));

    let url_rev = gtk::Revealer::new();
    url_rev.set_child(Some(&url_field));
    let advanced = gtk::Button::with_label("Advanced: address and pictures");
    advanced.set_has_frame(false);
    advanced.set_halign(gtk::Align::Start);
    advanced.add_css_class("link-btn");
    {
        let r = url_rev.clone();
        advanced.connect_clicked(move |_| r.set_reveal_child(!r.reveals_child()));
    }
    body.append(&advanced);
    body.append(&url_rev);

    let model = gtk::Entry::new();
    model.add_css_class("field-input");
    model.set_placeholder_text(Some("Model name, or pick one below"));
    body.append(&crate::field("Model", &model));

    let ml = ModelList::new();
    body.append(&ml.root);

    let status = lbl("", "field-hint");
    status.set_wrap(true);
    status.set_xalign(0.0);
    body.append(&status);
    // Claude Code: who is logged in, with Log in and Log out (shown only for that provider)
    let (cc_block, cc_refresh) = crate::claude_ui::block(ui);
    body.append(&cc_block);

    let current = {
        let shown = shown.clone();
        move |dd: &gtk::DropDown| -> &'static Provider { shown.borrow()[(dd.selected() as usize).min(shown.borrow().len().saturating_sub(1))] }
    };

    // the account shown for the provider on screen
    let account_id = {
        let acct_now = acct_now.clone();
        move || acct_now.borrow().clone()
    };
    // fills the account drop-down for the provider on screen and shows it only when the provider takes accounts
    let fill_accounts = {
        let (acct_names, acct_ids, acct_field, acct_now, acct_dd, filling) = (acct_names.clone(), acct_ids.clone(), acct_field.clone(), acct_now.clone(), acct_dd.clone(), filling.clone());
        Rc::new(move |p: &'static Provider| {
            acct_field.set_visible(p.takes_accounts());
            if !p.takes_accounts() {
                return;
            }
            let all = accounts::list(p.id);
            let keep = acct_now.borrow().clone();
            let shown: Vec<&accounts::View> = all.iter().filter(|a| !a.hidden || a.id == keep).collect();
            let want = if shown.iter().any(|a| a.id == keep) { keep } else { accounts::DEFAULT_ID.to_string() };
            *acct_now.borrow_mut() = want.clone();
            filling.set(true);
            acct_names.splice(0, acct_names.n_items(), &[]);
            let mut ids = Vec::new();
            for a in &shown {
                acct_names.append(&format!("{} · {}{}", p.name, a.name, if a.has_key { " ✓" } else { "" }));
                ids.push(a.id.clone());
            }
            acct_names.append("Add an account…");
            ids.push("+".to_string());
            let at = ids.iter().position(|i| *i == want).unwrap_or(0) as u32;
            *acct_ids.borrow_mut() = ids;
            acct_dd.set_selected(at);
            filling.set(false);
        })
    };

    // typing a model name moves the tick
    {
        let ml = ml.clone();
        model.connect_changed(move |e| ml.mark_current(e.text().trim()));
    }
    // a click on a row picks that model
    {
        let model = model.clone();
        *ml.on_pick.borrow_mut() = Some(Rc::new(move |m: &str| model.set_text(m)));
    }

    // (re)load the list for what is on screen now
    let load: Rc<dyn Fn(bool)> = {
        let (provider_dd, url, key, ml, current, account_id) = (provider_dd.clone(), url.clone(), key.clone(), ml.clone(), current.clone(), account_id.clone());
        Rc::new(move |force: bool| {
            let p = current(&provider_dd);
            let gen = ml.generation.get().wrapping_add(1);
            ml.generation.set(gen);
            let base = url.text().trim().to_string();
            let typed = Some(key.text().to_string()).filter(|k| !k.trim().is_empty());
            ml.fill(&[]);
            ml.set_loading("Loading the model list…");
            let ml2 = ml.clone();
            let acct = if p.takes_accounts() { account_id() } else { accounts::DEFAULT_ID.to_string() };
            fetch_models(p, base, typed, acct, force, move |r| {
                if ml2.generation.get() != gen {
                    return;
                }
                match r {
                    Ok(list) if list.is_empty() => {
                        ml2.fill(&[]);
                        ml2.set_note("The provider returned no models. You can still type a model name above.", true);
                    }
                    Ok(list) => {
                        ml2.fill(&list);
                        ml2.set_note("", false);
                    }
                    Err(e) => {
                        ml2.fill(&[]);
                        ml2.set_note(&e, true);
                    }
                }
            });
        })
    };
    {
        let load = load.clone();
        ml.retry.connect_clicked(move |_| load(true));
    }
    {
        // the refresh icon in the list's heading
        let load = load.clone();
        if let Some(b) = ml.root.first_child().and_then(|h| h.last_child()).and_then(|b| b.downcast::<gtk::Button>().ok()) {
            b.connect_clicked(move |_| load(true));
        }
    }
    {
        let (load, key) = (load.clone(), key.clone());
        key.connect_activate(move |_| load(true));
    }
    {
        let (load, url) = (load.clone(), url.clone());
        url.connect_activate(move |_| load(true));
    }

    // test the connection
    {
        let (provider_dd, url, key, status, current, model, account_id) = (provider_dd.clone(), url.clone(), key.clone(), status.clone(), current.clone(), model.clone(), account_id.clone());
        let load = load.clone();
        test.connect_clicked(move |b| {
            let p = current(&provider_dd);
            let acct = if p.takes_accounts() { account_id() } else { accounts::DEFAULT_ID.to_string() };
            let typed = Some(key.text().to_string()).filter(|k| !k.trim().is_empty());
            status.set_text("Testing the connection…");
            b.set_sensitive(false);
            let (status, b, load) = (status.clone(), b.clone(), load.clone());
            test_connection(p, url.text().trim().to_string(), typed, acct, model.text().trim().to_string(), move |r| {
                b.set_sensitive(true);
                match r {
                    Ok(m) => {
                        status.set_text(&m);
                        load(false);
                    }
                    Err(e) => status.set_text(&e),
                }
            });
        });
    }

    // fill the fields for the provider that is selected
    let refresh = {
        let (key, key_field, url, url_hint, advanced, url_rev, model, status, ml, load) = (key.clone(), key_field.clone(), url.clone(), url_hint.clone(), advanced.clone(), url_rev.clone(), model.clone(), status.clone(), ml.clone(), load.clone());
        let sel = Rc::new(RefCell::new(sel.clone()));
        let current = current.clone();
        let (fill_accounts, acct_now) = (fill_accounts.clone(), acct_now.clone());
        let last_provider = Rc::new(RefCell::new(start_provider.clone()));
        Rc::new(move |dd: &gtk::DropDown| {
            let p = current(dd);
            let s = sel.borrow();
            // another provider starts on its Default account; the one first shown keeps the account it was opened on
            if *last_provider.borrow() != p.id {
                *acct_now.borrow_mut() = accounts::DEFAULT_ID.to_string();
                *last_provider.borrow_mut() = p.id.to_string();
            }
            fill_accounts(p);
            let acct = acct_now.borrow().clone();
            let base_pref = if p.takes_accounts() { accounts::address_of(p.id, &acct) } else { None };
            url.set_text(&base_pref.unwrap_or_else(|| s.base_url_for(p)));
            key_field.set_visible(!p.local);
            url_rev.set_visible(p.protocol != Protocol::ClaudeCode);
            url_rev.set_reveal_child(needs_url(p));
            advanced.set_visible(p.protocol != Protocol::ClaudeCode && !needs_url(p));
            url_hint.set_text(if p.is_custom() { endpoint_hint(format_id(p)) } else if needs_url(p) { "Where the server runs. The default is right unless you changed its port." } else { "Only change this for a proxy or a regional endpoint." });
            key.set_text("");
            key.set_placeholder_text(Some(if p.takes_accounts() && accounts::get_key(p.id, Some(&acct)).is_some() { "A key is saved for this account. Paste another to replace it" } else { "Paste the key (a saved one is used when this stays empty)" }));
            model.set_text(if s.provider == p.id && (!p.takes_accounts() || s.account.as_deref().unwrap_or(accounts::DEFAULT_ID) == acct) { &s.model } else { p.models.first().copied().unwrap_or("") });
            *ml.pinned.borrow_mut() = pins_of(&s, p);
            ml.search.set_text("");
            status.set_text(if p.protocol == Protocol::ClaudeCode {
                if zo_core::ai::claude_code::binary().is_some() { "Uses the Claude Code program you installed and signed in to yourself (claude auth login). Zohara Agent never sees your login. Experimental. The full model list is read with an Anthropic API key (add one under the Anthropic provider or in a profile); without one you get Claude Code's own names for the newest models." } else { "Claude Code is not installed. Install it with: npm install -g @anthropic-ai/claude-code, then sign in with: claude auth login" }
            } else if p.local { "Runs on this computer. No key, and nothing leaves it." } else if keys::available() { "The key is stored in the system keyring." } else { "The keyring tool (secret-tool) is missing, so keys cannot be saved." });
            cc_block.set_visible(p.protocol == Protocol::ClaudeCode);
            if p.protocol == Protocol::ClaudeCode {
                cc_refresh();
            }
            ml.mark_current(&model.text());
            load(false);
        })
    };
    let refresh: Rc<dyn Fn(&gtk::DropDown)> = refresh;
    refresh(&provider_dd);
    {
        let refresh = refresh.clone();
        provider_dd.connect_selected_notify(move |dd| refresh(dd));
    }
    // another account of the provider: its own key, its own address; "Add an account…" opens the form
    {
        let (refresh, provider_dd, acct_ids, acct_now, ui2, win2, current, filling) = (refresh.clone(), provider_dd.clone(), acct_ids.clone(), acct_now.clone(), ui.clone(), win.clone(), current.clone(), filling.clone());
        acct_dd.connect_selected_notify(move |dd| {
            if filling.get() {
                return;
            }
            let Some(id) = acct_ids.borrow().get(dd.selected() as usize).cloned() else { return };
            if id == acct_now.borrow().as_str() {
                return;
            }
            let p = current(&provider_dd);
            if id == "+" {
                // back to the account shown, then the form; the sheet opens again on the new account
                let back = acct_ids.borrow().iter().position(|i| *i == *acct_now.borrow()).unwrap_or(0) as u32;
                dd.set_selected(back);
                let (ui3, win3) = (ui2.clone(), win2.clone());
                account_sheet(&ui2, p, None, move |new_id| {
                    win3.close();
                    models_sheet_at(&ui3, Some((p.id.to_string(), new_id)));
                });
                return;
            }
            *acct_now.borrow_mut() = id;
            refresh(&provider_dd);
        });
    }

    // show which local servers are already running
    {
        let status = status.clone();
        glib::spawn_future_local(async move {
            let found = gtk::gio::spawn_blocking(|| agent::detect_local().iter().map(|p| p.name.to_string()).collect::<Vec<_>>()).await.unwrap_or_default();
            if !found.is_empty() {
                let t = status.text();
                status.set_text(&format!("{t}\nFound running here: {}", found.join(", ")));
            }
        });
    }

    let hint = lbl("Stars are saved with Save.", "field-hint");
    hint.set_hexpand(true);
    let add_custom = secondary("Add custom provider");
    {
        let (ui2, win2) = (ui.clone(), win.clone());
        add_custom.connect_clicked(move |_| {
            let (ui3, win3) = (ui2.clone(), win2.clone());
            custom_provider_sheet(&ui2, None, move |id| {
                win3.close();
                models_sheet_at(&ui3, Some((id, accounts::DEFAULT_ID.to_string())));
            });
        });
    }
    let cancel = secondary("Cancel");
    let save = primary("Save");
    foot.append(&hint);
    foot.append(&add_custom);
    foot.append(&cancel);
    foot.append(&save);
    {
        let w = win.clone();
        cancel.connect_clicked(move |_| w.close());
    }
    {
        let (w, ui2, provider_dd, key, url, model, status, ml, vision_dd, current, account_id) = (win.clone(), ui.clone(), provider_dd.clone(), key.clone(), url.clone(), model.clone(), status.clone(), ml.clone(), vision_dd.clone(), current.clone(), account_id.clone());
        save.connect_clicked(move |_| {
            let mut sel = Selection::load();
            sel.vision = match vision_dd.selected() {
                1 => Some(true),
                2 => Some(false),
                _ => None,
            };
            let p = current(&provider_dd);
            let acct = if p.takes_accounts() { Some(account_id()).filter(|a| a != accounts::DEFAULT_ID) } else { None };
            // a profile belongs to one provider and one account: choosing another one leaves the profile as it was
            if let Some(id) = sel.profile.clone() {
                if profiles::load().iter().any(|x| x.id == id && (x.provider != p.id || x.account != acct)) {
                    sel.profile = None;
                }
            }
            sel.provider = p.id.to_string();
            sel.account = acct.clone();
            let typed = key.text().to_string();
            if !typed.trim().is_empty() {
                // with a profile chosen the key belongs to that profile, else to the account in use
                if let Err(e) = keys::set(&profiles::key_id(&sel, p.id), &typed) {
                    status.set_text(&e);
                    return;
                }
            }
            sel.model = model.text().trim().to_string();
            let u = url.text().trim().to_string();
            let own = acct.as_deref().and_then(|a| accounts::address_of(p.id, a));
            if own.as_deref() == Some(u.as_str()) {
                // the address of this account itself: nothing to save about it here
            } else if p.is_custom() {
                // a custom provider's address is part of it
                if !u.is_empty() && u != p.base_url {
                    if let Err(e) = custom::update(p.id, None, None, Some(&u), None) {
                        status.set_text(&e);
                        return;
                    }
                }
            } else if u == p.base_url || u.is_empty() {
                sel.base_urls.remove(p.id);
            } else {
                sel.base_urls.insert(p.id.to_string(), u);
            }
            sel.save();
            profiles::update_active(&sel);
            // the stars: the active profile's pins become exactly what is starred (a profile is made when the first star is saved)
            let want = ml.pinned.borrow().clone();
            let have = pins_of(&sel, p);
            if !want.is_empty() || !have.is_empty() {
                let id = if want.is_empty() { sel.profile.clone() } else { profiles::ensure_active(&mut sel, &accounts::label(p, acct.as_deref())).ok() };
                if let Some(id) = id {
                    sel.save();
                    for m in have.iter().filter(|m| !want.contains(m)) {
                        profiles::unpin(&id, m);
                    }
                    for m in want.iter().filter(|m| !have.contains(m)) {
                        profiles::pin(&id, m);
                    }
                }
            }
            ui2.model_label.set_text(&chat::pill_text(&sel));
            ui2.pv_model_label.set_text(&chat::pill_text(&sel));
            w.close();
        });
    }
    // this chooser is tall: let it use most of the window (the list inside scrolls on its own)
    if let Some(sw) = body.ancestor(gtk::ScrolledWindow::static_type()).and_then(|w| w.downcast::<gtk::ScrolledWindow>().ok()) {
        let h = ui.window.height().max(ui.window.default_height()).max(600);
        sw.set_max_content_height((h as f64 * 0.8) as i32);
    }
    win.present();
}

// ---------------------------------------------------------------- a custom provider (named), and an account of a provider

/// The fields of the "custom provider" form and what Save does with them. Built without a window, so the form can be tested.
#[derive(Clone)]
pub(crate) struct CustomForm {
    pub root: gtk::Box,
    pub name: gtk::Entry,
    pub format_dd: gtk::DropDown,
    pub url: gtk::Entry,
    pub key: gtk::PasswordEntry,
    pub model: gtk::Entry,
    pub status: gtk::Label,
    /// The provider being edited, none for a new one.
    pub existing: Option<custom::CustomProvider>,
}

impl CustomForm {
    pub(crate) fn new(existing: Option<custom::CustomProvider>) -> CustomForm {
        let root = vbox(10);
        let name = gtk::Entry::new();
        name.add_css_class("field-input");
        name.set_max_length(custom::MAX_NAME as i32);
        name.set_placeholder_text(Some("My router, Work gateway, Home Ollama"));
        root.append(&crate::field("Name", &name));
        let labels: Vec<&str> = FORMATS.iter().map(|f| f.1).collect();
        let format_dd = gtk::DropDown::from_strings(&labels);
        root.append(&crate::field("API format", &format_dd));
        let hint = lbl("", "field-hint");
        hint.set_wrap(true);
        hint.set_xalign(0.0);
        root.append(&hint);
        {
            let hint = hint.clone();
            let set = move |dd: &gtk::DropDown| hint.set_text(endpoint_hint(FORMATS[(dd.selected() as usize).min(2)].0));
            set(&format_dd);
            format_dd.connect_selected_notify(set);
        }
        let url = gtk::Entry::new();
        url.add_css_class("field-input");
        url.set_placeholder_text(Some("https://my-host.example/v1"));
        root.append(&crate::field("Address", &url));
        let key = gtk::PasswordEntry::new();
        key.set_show_peek_icon(true);
        key.add_css_class("field-input");
        let key_note = match &existing {
            Some(c) if keys::get(&c.id).is_some() => "API key (a key is saved: leave empty to keep it; a new address removes it)",
            Some(_) => "API key (none saved; leave empty if it needs none)",
            None => "API key (leave empty if it needs none)",
        };
        root.append(&crate::field(key_note, &key));
        let model = gtk::Entry::new();
        model.add_css_class("field-input");
        model.set_placeholder_text(Some("Model to start with (Test connection can list them)"));
        root.append(&crate::field("Model", &model));
        let status = lbl("", "field-hint");
        status.set_wrap(true);
        status.set_xalign(0.0);
        root.append(&status);
        if let Some(c) = &existing {
            name.set_text(&c.name);
            url.set_text(&c.base_url);
            model.set_text(c.default_model.as_deref().unwrap_or(""));
            format_dd.set_selected(FORMATS.iter().position(|f| f.0 == c.format).unwrap_or(0) as u32);
        }
        CustomForm { root, name, format_dd, url, key, model, status, existing }
    }

    pub(crate) fn format(&self) -> &'static str {
        FORMATS[(self.format_dd.selected() as usize).min(2)].0
    }

    /// The key to test with: the typed one, else the saved one when the address is the one it was saved for.
    fn test_key(&self) -> Option<String> {
        let typed = Some(self.key.text().to_string()).filter(|k| !k.trim().is_empty());
        typed.or_else(|| {
            let c = self.existing.as_ref()?;
            (c.base_url.trim().trim_end_matches('/') == self.url.text().trim().trim_end_matches('/')).then(|| keys::get(&c.id)).flatten()
        })
    }

    /// Tests what is typed, saving nothing.
    pub(crate) fn test(&self, done: impl FnOnce() + 'static) {
        let base = self.url.text().trim().to_string();
        let p = match custom::draft(self.format(), &base) {
            Ok(p) => p,
            Err(e) => {
                self.status.set_text(&e);
                return;
            }
        };
        self.status.set_text("Testing the connection…");
        let (status, key) = (self.status.clone(), self.test_key());
        let model_name = self.model.text().trim().to_string();
        test_connection(p, base, key, accounts::DEFAULT_ID.to_string(), model_name, move |r| {
            status.set_text(&r.unwrap_or_else(|e| friendly_error(&e)));
            done();
        });
    }

    /// Adds or saves the provider and its key. Returns its id; on a refusal the sentence to show.
    pub(crate) fn save(&self) -> Result<String, String> {
        let (name, url, model) = (self.name.text().trim().to_string(), self.url.text().trim().to_string(), self.model.text().trim().to_string());
        let typed = self.key.text().trim().to_string();
        match &self.existing {
            None => {
                let c = custom::add(&name, self.format(), &url, Some(&model))?;
                if !typed.is_empty() {
                    if let Err(e) = keys::set(&c.id, &typed) {
                        let _ = custom::remove(&c.id);
                        return Err(e);
                    }
                }
                Ok(c.id)
            }
            Some(old) => {
                let c = custom::update(&old.id, Some(&name), Some(self.format()), Some(&url), Some(&model))?;
                if !typed.is_empty() {
                    keys::set(&c.id, &typed)?;
                }
                Ok(c.id)
            }
        }
    }
}

/// Name, API format, address, key and a model: adds (or edits) a custom provider. `done` gets its id.
pub fn custom_provider_sheet(ui: &Rc<Ui>, existing: Option<custom::CustomProvider>, done: impl Fn(String) + 'static) {
    let (win, body, foot) = sheet(ui, if existing.is_some() { "Edit custom provider" } else { "Add a custom provider" }, 520);
    let form = CustomForm::new(existing.clone());
    body.append(&form.root);
    let test = secondary("Test connection");
    let cancel = secondary("Cancel");
    let save = primary(if existing.is_some() { "Save" } else { "Add provider" });
    foot.append(&test);
    foot.append(&cancel);
    foot.append(&save);
    {
        let w = win.clone();
        cancel.connect_clicked(move |_| w.close());
    }
    {
        let form = form.clone();
        test.connect_clicked(move |b| {
            b.set_sensitive(false);
            let b2 = b.clone();
            form.test(move || b2.set_sensitive(true));
        });
    }
    {
        let (w, form) = (win.clone(), form.clone());
        save.connect_clicked(move |_| match form.save() {
            Ok(id) => {
                w.close();
                done(id);
            }
            Err(e) => form.status.set_text(&e),
        });
    }
    win.present();
    form.name.grab_focus();
}

/// The fields of the "account" form: a name, its key, and (for a provider that allows it) an address of its own.
#[derive(Clone)]
pub(crate) struct AccountForm {
    pub root: gtk::Box,
    pub name: gtk::Entry,
    pub key: gtk::PasswordEntry,
    pub url: gtk::Entry,
    pub status: gtk::Label,
    pub provider: &'static Provider,
    pub existing: Option<accounts::View>,
}

impl AccountForm {
    pub(crate) fn new(provider: &'static Provider, existing: Option<accounts::View>) -> AccountForm {
        let root = vbox(10);
        root.append(&lbl(&format!("Another API key of {}, for example a second account. Its key is kept on its own and only used for this account.", provider.name), "field-hint"));
        let name = gtk::Entry::new();
        name.add_css_class("field-input");
        name.set_max_length(custom::MAX_NAME as i32);
        name.set_placeholder_text(Some(&format!("{} 1", provider.name)));
        root.append(&crate::field("Name", &name));
        let key = gtk::PasswordEntry::new();
        key.set_show_peek_icon(true);
        key.add_css_class("field-input");
        root.append(&crate::field(if existing.as_ref().is_some_and(|a| a.has_key) { "API key (saved: leave empty to keep it)" } else { "API key" }, &key));
        let url = gtk::Entry::new();
        url.add_css_class("field-input");
        url.set_placeholder_text(Some(provider.base_url));
        let own_address = !provider.is_custom() && !existing.as_ref().is_some_and(|a| a.is_default);
        let url_field = crate::field("Address (optional)", &url);
        url_field.append(&lbl(&format!("Only if this account uses another address of {}.", provider.name), "field-hint"));
        url_field.set_visible(own_address);
        root.append(&url_field);
        let status = lbl("", "field-hint");
        status.set_wrap(true);
        status.set_xalign(0.0);
        root.append(&status);
        if let Some(a) = &existing {
            name.set_text(&a.name);
            url.set_text(a.base_url.as_deref().unwrap_or(""));
        }
        AccountForm { root, name, key, url, status, provider, existing }
    }

    /// Adds or saves the account and its key. Returns its id; on a refusal the sentence to show.
    pub(crate) fn save(&self) -> Result<String, String> {
        let (name, typed, url) = (self.name.text().trim().to_string(), self.key.text().trim().to_string(), self.url.text().trim().to_string());
        let pid = self.provider.id;
        match &self.existing {
            None => {
                if typed.is_empty() {
                    return Err("Paste the account's API key.".into());
                }
                let a = accounts::add(pid, &name, Some(&url))?;
                if let Err(e) = keys::set(&accounts::key_id(pid, Some(&a.id)), &typed) {
                    let _ = accounts::remove(pid, &a.id);
                    return Err(e);
                }
                Ok(a.id)
            }
            Some(old) => {
                accounts::rename(pid, &old.id, &name)?;
                if !old.is_default && !self.provider.is_custom() {
                    accounts::set_address(pid, &old.id, &url)?;
                }
                if !typed.is_empty() {
                    keys::set(&accounts::key_id(pid, Some(&old.id)), &typed)?;
                }
                Ok(old.id.clone())
            }
        }
    }

    /// Tests the account with its own key (the key typed here is not used: Save first to test a new one).
    pub(crate) fn test(&self, done: impl FnOnce() + 'static) {
        let Some(a) = self.existing.clone() else { return };
        self.status.set_text("Testing the connection…");
        let status = self.status.clone();
        let base = accounts::address_of(self.provider.id, &a.id).unwrap_or_else(|| Selection::load().base_url_for(self.provider));
        test_connection(self.provider, base, None, a.id.clone(), String::new(), move |r| {
            status.set_text(&r.unwrap_or_else(|e| friendly_error(&e)));
            done();
        });
    }
}

/// Add (or edit) an account of `p`. `done` gets the id of the account.
pub fn account_sheet(ui: &Rc<Ui>, p: &'static Provider, existing: Option<accounts::View>, done: impl Fn(String) + 'static) {
    let (win, body, foot) = sheet(ui, if existing.is_some() { "Edit account" } else { "Add an account" }, 480);
    let form = AccountForm::new(p, existing.clone());
    body.append(&form.root);
    let test = secondary("Test connection");
    test.set_visible(existing.is_some());
    let cancel = secondary("Cancel");
    let save = primary(if existing.is_some() { "Save" } else { "Add account" });
    foot.append(&test);
    foot.append(&cancel);
    foot.append(&save);
    {
        let w = win.clone();
        cancel.connect_clicked(move |_| w.close());
    }
    {
        let form = form.clone();
        test.connect_clicked(move |b| {
            b.set_sensitive(false);
            let b2 = b.clone();
            form.test(move || b2.set_sensitive(true));
        });
    }
    {
        let (w, form) = (win.clone(), form.clone());
        save.connect_clicked(move |_| match form.save() {
            Ok(id) => {
                w.close();
                done(id);
            }
            Err(e) => form.status.set_text(&e),
        });
    }
    win.present();
    form.name.grab_focus();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_three_api_formats_are_offered_with_their_own_lines() {
        let ids: Vec<&str> = FORMATS.iter().map(|f| f.0).collect();
        assert_eq!(ids, ["chat", "responses", "claude"]);
        let hints: Vec<&str> = ids.iter().map(|i| endpoint_hint(i)).collect();
        assert!(hints[0].contains("/v1/chat/completions") && hints[1].contains("/v1/responses") && hints[2].contains("/v1/messages"));
        assert_eq!(hints.iter().collect::<std::collections::HashSet<_>>().len(), 3);
        // the labels agree with what the core calls the formats
        for (id, label) in FORMATS {
            assert_eq!(providers::format_name(id), Some(id));
            assert!(label.contains(providers::format_label(providers::format_protocol(id).unwrap())), "{label}");
        }
    }

    #[test]
    fn a_list_is_cached_per_provider_address_and_account() {
        let p = providers::find("groq").unwrap();
        let (a, b) = (cache_key(p, "https://x/v1/", "default"), cache_key(p, "https://x/v1", "groq-2"));
        assert_ne!(a, b, "another account has another key, so another list");
        assert_eq!(cache_key(p, "https://x/v1/", "default"), cache_key(p, "https://x/v1", "default"), "a trailing slash is the same address");
        assert_ne!(cache_key(p, "https://x/v1", "default"), cache_key(providers::find("openai").unwrap(), "https://x/v1", "default"));
    }

    #[test]
    fn format_hints_from_the_core_are_not_reworded() {
        let h = zo_core::ai::wire::wrong_format_hint(Protocol::OpenAiResponses);
        assert_eq!(friendly_error(&format!("{h} (The provider answered 404)")), format!("{h} (The provider answered 404)"));
    }

    #[test]
    fn the_formats_a_provider_speaks_are_named_for_the_picker() {
        assert_eq!(format_id(providers::find("anthropic").unwrap()), "claude");
        assert_eq!(format_id(providers::find("openai").unwrap()), "chat");
        assert_eq!(format_id(providers::find("groq").unwrap()), "chat");
    }

    /// A private configuration for a test that saves: its own folders and a key file, so nothing of the person is touched.
    fn private_config(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("zo-gtk-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("XDG_CONFIG_HOME", dir.join("config"));
        std::env::set_var("XDG_DATA_HOME", dir.join("data"));
        std::env::set_var("ZOHARA_AGENT_KEYS_FILE", dir.join("keys.json"));
        dir
    }

    /// GTK starts on one thread only, so the two form checks are one test.
    #[test]
    #[ignore = "needs a display (run under xvfb)"]
    fn the_provider_and_account_forms_save_what_is_typed_with_their_own_keys() {
        gtk::init().expect("a display");
        the_custom_provider_form_adds_edits_and_keeps_the_key_with_the_provider();
        the_account_form_adds_accounts_with_their_own_keys_and_checks_names();
    }

    fn the_custom_provider_form_adds_edits_and_keeps_the_key_with_the_provider() {
        let dir = private_config("custom-form");
        let f = CustomForm::new(None);
        assert_eq!(f.format(), "chat");
        assert!(f.save().is_err(), "a name is needed");
        f.name.set_text("My router");
        f.url.set_text("http://127.0.0.1:8123/v1");
        f.format_dd.set_selected(1);
        f.key.set_text("sk-form-1");
        f.model.set_text("gpt-5");
        assert_eq!(f.format(), "responses");
        let id = f.save().unwrap();
        assert_eq!(id, "custom:my-router");
        let c = custom::get(&id).unwrap();
        assert_eq!((c.name.as_str(), c.format.as_str(), c.base_url.as_str(), c.default_model.as_deref()), ("My router", "responses", "http://127.0.0.1:8123/v1", Some("gpt-5")));
        assert_eq!(keys::get(&id).as_deref(), Some("sk-form-1"));
        // a second provider of the same name is refused; another name gets its own key
        let g = CustomForm::new(None);
        g.name.set_text("my ROUTER");
        g.url.set_text("http://127.0.0.1:8124/v1");
        assert!(g.save().unwrap_err().contains("already a custom provider named"));
        g.name.set_text("Home Ollama");
        g.format_dd.set_selected(2);
        g.key.set_text("sk-form-2");
        let id2 = g.save().unwrap();
        assert_eq!((keys::get(&id).as_deref(), keys::get(&id2).as_deref()), (Some("sk-form-1"), Some("sk-form-2")));
        // the edit form starts from what is saved, a rename keeps the id and the key, a new address drops the key
        let e = CustomForm::new(custom::get(&id));
        assert_eq!((e.name.text().as_str(), e.url.text().as_str(), e.format(), e.model.text().as_str()), ("My router", "http://127.0.0.1:8123/v1", "responses", "gpt-5"));
        e.name.set_text("Fast router");
        assert_eq!(e.save().unwrap(), id);
        assert_eq!(keys::get(&id).as_deref(), Some("sk-form-1"));
        e.url.set_text("http://127.0.0.1:8999/v1");
        e.save().unwrap();
        assert_eq!(keys::get(&id), None);
        assert_eq!(keys::get(&id2).as_deref(), Some("sk-form-2"));
        // a bad address says so and saves nothing new
        let b = CustomForm::new(None);
        b.name.set_text("Bad");
        b.url.set_text("not-an-address");
        assert!(b.save().is_err());
        assert_eq!(custom::list().len(), 2);
        let _ = std::fs::remove_dir_all(dir);
    }

    fn the_account_form_adds_accounts_with_their_own_keys_and_checks_names() {
        let dir = private_config("account-form");
        let groq = providers::find("groq").unwrap();
        keys::set("groq", "gk-default").unwrap();
        let f = AccountForm::new(groq, None);
        assert!(f.save().is_err(), "a name is needed");
        f.name.set_text("Groq 1");
        assert!(f.save().unwrap_err().contains("API key"), "and a key");
        f.key.set_text("gk-one");
        let id = f.save().unwrap();
        assert_eq!(id, "groq-1");
        assert_eq!((keys::get("groq").as_deref(), keys::get("groq:groq-1").as_deref()), (Some("gk-default"), Some("gk-one")));
        let g = AccountForm::new(groq, None);
        g.name.set_text("groq 1");
        g.key.set_text("gk-two");
        assert!(g.save().unwrap_err().contains("already has an account named"));
        assert_eq!(keys::get("groq:groq-1").as_deref(), Some("gk-one"), "a refused account changes no key");
        // an account of another provider has its own key; a rename keeps the key
        let o = AccountForm::new(providers::find("openai").unwrap(), None);
        o.name.set_text("Groq 1");
        o.key.set_text("ok-one");
        o.save().unwrap();
        assert_eq!((keys::get("openai:groq-1").as_deref(), keys::get("groq:groq-1").as_deref()), (Some("ok-one"), Some("gk-one")));
        let view = accounts::list("groq").into_iter().find(|a| a.id == "groq-1");
        let e = AccountForm::new(groq, view);
        assert_eq!(e.name.text().as_str(), "Groq 1");
        e.name.set_text("Work");
        assert_eq!(e.save().unwrap(), "groq-1");
        assert_eq!(keys::get("groq:groq-1").as_deref(), Some("gk-one"));
        assert_eq!(accounts::list("groq").iter().map(|a| a.name.clone()).collect::<Vec<_>>(), ["Default", "Work"]);
        // the Default account is renamed through the same form
        let d = AccountForm::new(groq, accounts::list("groq").into_iter().next());
        d.name.set_text("Main");
        d.save().unwrap();
        assert_eq!(accounts::list("groq")[0].name, "Main");
        assert_eq!(key_for(groq, "groq-1", None).as_deref(), Some("gk-one"));
        assert_eq!(key_for(groq, "default", None).as_deref(), Some("gk-default"));
        assert_eq!(key_for(groq, "default", Some("typed".into())).as_deref(), Some("typed"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
