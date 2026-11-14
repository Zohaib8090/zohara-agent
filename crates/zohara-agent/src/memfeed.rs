//! The memory feed on the desktop: the slim "Memory updated · View" chip above the message box, and the list of what changed with an
//! Undo for each item (see docs/MEMORY.md). The memory agent runs on its own thread; it tells this window through
//! `memory_log::subscribe`, and the window shows the chip on its own thread.

use crate::{hbox, icon_button, lbl, secondary, settings, vbox, Ui};
use adw::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::{Rc, Weak};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use zo_core::store::memory_log::{self, Change};

/// How long the chip stays before it fades by itself.
const SHOWN_MS: u64 = 12_000;

#[derive(Clone)]
struct Chip {
    rev: gtk::Revealer,
    text: gtk::Label,
}

thread_local! {
    static CHIPS: RefCell<Vec<Chip>> = const { RefCell::new(Vec::new()) };
    static UI: RefCell<Weak<Ui>> = const { RefCell::new(Weak::new()) };
    /// Counts the times a chip was shown: an old timer must not hide a newer showing.
    static SHOWING: Cell<u32> = const { Cell::new(0) };
    static SUB: RefCell<Option<memory_log::Subscription>> = const { RefCell::new(None) };
}

/// A new chip to put above a message box. Hidden until the memory agent changes something.
pub fn chip() -> gtk::Revealer {
    let rev = gtk::Revealer::new();
    rev.set_transition_type(gtk::RevealerTransitionType::Crossfade);
    rev.set_transition_duration(250);
    rev.set_halign(gtk::Align::Start);
    rev.set_reveal_child(false);
    let row = hbox(6);
    row.add_css_class("mem-chip");
    let dot = gtk::Image::from_icon_name("preferences-other-symbolic");
    dot.set_pixel_size(14);
    row.append(&dot);
    let text = lbl("Memory updated", "mem-chip-text");
    row.append(&text);
    row.append(&lbl("·", "mem-chip-text"));
    let view = gtk::Button::with_label("View");
    view.add_css_class("mem-chip-view");
    view.add_css_class("flat");
    view.set_tooltip_text(Some("See what changed in the memory"));
    row.append(&view);
    let go = icon_button("go-next-symbolic", "See what changed in the memory", "mem-chip-go");
    row.append(&go);
    let close = icon_button("window-close-symbolic", "Dismiss", "mem-chip-close");
    row.append(&close);
    rev.set_child(Some(&row));
    let c = Chip { rev: rev.clone(), text };
    for b in [&view, &go] {
        b.connect_clicked(|_| {
            hide();
            if let Some(ui) = UI.with(|u| u.borrow().upgrade()) {
                changes_sheet(&ui, true);
            }
        });
    }
    close.connect_clicked(|_| hide());
    CHIPS.with(|v| v.borrow_mut().push(c));
    rev
}

fn hide() {
    SHOWING.with(|s| s.set(s.get().wrapping_add(1)));
    CHIPS.with(|v| v.borrow().iter().for_each(|c| c.rev.set_reveal_child(false)));
}

fn show_chip(summary: &str) {
    // "Added 2 notes" reads as the detail of "Memory updated"
    let text = if summary.is_empty() { "Memory updated".to_string() } else { format!("Memory updated ({})", summary.to_lowercase()) };
    CHIPS.with(|v| {
        for c in v.borrow().iter() {
            c.text.set_text(&text);
            c.rev.set_reveal_child(true);
        }
    });
    let n = SHOWING.with(|s| {
        s.set(s.get().wrapping_add(1));
        s.get()
    });
    gtk::glib::timeout_add_local_once(Duration::from_millis(SHOWN_MS), move || {
        if SHOWING.with(Cell::get) == n {
            hide();
        }
    });
}

/// Starts listening for memory updates; call once, after the window exists.
pub fn start(ui: &Rc<Ui>) {
    UI.with(|u| *u.borrow_mut() = Rc::downgrade(ui));
    let queue: Arc<Mutex<VecDeque<String>>> = Arc::default();
    let q = queue.clone();
    let sub = memory_log::subscribe(move |ev| {
        if let Ok(mut q) = q.lock() {
            q.push_back(ev.get("summary").and_then(|s| s.as_str()).unwrap_or("").to_string());
        }
    });
    SUB.with(|s| *s.borrow_mut() = Some(sub));
    gtk::glib::timeout_add_local(Duration::from_millis(500), move || {
        let next = queue.lock().ok().and_then(|mut q| {
            let last = q.pop_back();
            q.clear();
            last
        });
        if let Some(summary) = next {
            show_chip(&summary);
        }
        gtk::glib::ControlFlow::Continue
    });
}

fn ago(ms: u64) -> String {
    let secs = zo_core::store::chats::now_millis().saturating_sub(ms) / 1000;
    match secs {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", secs / 60),
        3600..=86_399 => format!("{} h ago", secs / 3600),
        _ => format!("{} days ago", secs / 86_400),
    }
}

fn where_to(c: &Change) -> String {
    if c.scope.is_empty() {
        "All chats".to_string()
    } else if c.scope_name.is_empty() {
        "A project".to_string()
    } else {
        format!("Project: {}", c.scope_name)
    }
}

/// The list of what changed: the latest update (`latest_only`) or the last 60 changes, grouped as added, updated and removed, each with Undo.
pub fn changes_sheet(ui: &Rc<Ui>, latest_only: bool) {
    let (win, body, foot) = crate::sheet(ui, "Memory updated", 620);
    let only_latest = Rc::new(Cell::new(latest_only));
    let fill: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::default();
    let more = secondary("Show earlier changes");
    let page = secondary("Open the memory page");
    let close = secondary("Close");
    foot.append(&more);
    foot.append(&page);
    foot.append(&close);
    {
        let w = win.clone();
        close.connect_clicked(move |_| w.close());
    }
    {
        let (w, ui2) = (win.clone(), ui.clone());
        page.connect_clicked(move |_| {
            w.close();
            settings::open_page(&ui2, "memory");
        });
    }
    let f: Rc<dyn Fn()> = {
        let (body, more, only, fill) = (body.clone(), more.clone(), only_latest.clone(), fill.clone());
        Rc::new(move || {
            while let Some(c) = body.first_child() {
                body.remove(&c);
            }
            let all = memory_log::list(0, 60);
            let shown: Vec<Change> = if only.get() { all.first().map(|f| all.iter().filter(|c| c.batch == f.batch).cloned().collect()).unwrap_or_default() } else { all };
            more.set_label(if only.get() { "Show earlier changes" } else { "Show only the latest" });
            if shown.is_empty() {
                let e = lbl("Nothing has changed in the memory yet. When Zohara Agent learns something about you, it is listed here, and you can undo it.", "sheet-empty");
                e.set_wrap(true);
                e.set_justify(gtk::Justification::Center);
                body.append(&e);
                return;
            }
            let hint = lbl("This is what Zohara Agent changed in its memory. Undo puts a note back the way it was.", "field-hint");
            hint.set_wrap(true);
            hint.set_xalign(0.0);
            body.append(&hint);
            for (action, title) in [("added", "Added"), ("updated", "Updated"), ("deleted", "Removed")] {
                let mine: Vec<&Change> = shown.iter().filter(|c| c.action == action).collect();
                if mine.is_empty() {
                    continue;
                }
                let h = lbl(title, "pc-meta");
                h.set_xalign(0.0);
                h.set_margin_top(6);
                body.append(&h);
                for c in mine {
                    body.append(&change_row(c, &fill));
                }
            }
        })
    };
    *fill.borrow_mut() = Some(f.clone());
    {
        let (only, f) = (only_latest.clone(), f.clone());
        more.connect_clicked(move |_| {
            only.set(!only.get());
            f();
        });
    }
    f();
    win.present();
}

fn change_row(c: &Change, fill: &Rc<RefCell<Option<Rc<dyn Fn()>>>>) -> gtk::Box {
    let row = hbox(10);
    row.add_css_class("sheet-item");
    let col = vbox(3);
    col.set_hexpand(true);
    let mk = |t: &str, class: &str| {
        let l = lbl(t, class);
        l.set_wrap(true);
        l.set_xalign(0.0);
        l.set_selectable(true);
        l
    };
    match c.action.as_str() {
        "updated" => {
            col.append(&mk(&format!("Before: {}", c.before), "sheet-item-sub"));
            col.append(&mk(&c.after, "sheet-item-name"));
        }
        "deleted" => col.append(&mk(&c.before, "sheet-item-name")),
        _ => col.append(&mk(&c.after, "sheet-item-name")),
    }
    let from = if c.title.is_empty() { String::new() } else { format!(" · from \"{}\"", c.title) };
    col.append(&mk(&format!("{} · {}{}", where_to(c), ago(c.time), from), "sheet-item-sub"));
    row.append(&col);
    let undo = secondary(if c.undone { "Undone" } else { "Undo" });
    undo.set_valign(gtk::Align::Center);
    undo.set_sensitive(!c.undone);
    let (id, fill) = (c.id.clone(), fill.clone());
    undo.connect_clicked(move |b| {
        if let Err(e) = memory_log::undo(&id) {
            b.set_label("Cannot undo");
            b.set_tooltip_text(Some(&e));
            b.set_sensitive(false);
            return;
        }
        let f = fill.borrow().clone();
        if let Some(f) = f {
            f();
        }
    });
    row.append(&undo);
    row
}
