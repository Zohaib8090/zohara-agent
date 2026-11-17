//! The desktop's timer for scheduled tasks. The work itself (what a task is, how it runs, approvals, history, reminders, catch-up of
//! missed runs) is `zo_core::scheduler`, the same code the cloud server and "Test" use; this file only wakes up every 20 seconds, runs
//! what is due on a thread of its own and shows the result as a desktop notification (docs/SCHEDULED.md).

use adw::prelude::*;
use gtk4 as gtk;
use gtk::glib;
use libadwaita as adw;
use std::cell::Cell;
use std::rc::Rc;
use std::sync::atomic::AtomicBool;
use std::sync::{mpsc, Arc};
use std::time::Duration;
use zo_core::approval::AppApprover;
use zo_core::scheduler::{self, Notice};
use zo_core::store::schedules::{self, Task};

thread_local! {
    static RUNNING: Cell<bool> = const { Cell::new(false) };
}

/// A finished run, on its way from a worker thread to the main thread.
type Shown = (Task, Notice);

/// Shows a finished task: a desktop notification (the result in one line; a failure says why) and a refresh of the Scheduled page.
/// Main thread only.
pub fn show(task: &Task, n: &Notice, done: &dyn Fn()) {
    if let Some(app) = gtk::gio::Application::default() {
        let note = gtk::gio::Notification::new(&n.title);
        note.set_body(Some(&n.body));
        note.set_default_action("app.zo-show");
        if n.kind == "reminder" || !n.ok {
            note.set_priority(gtk::gio::NotificationPriority::High);
        }
        app.send_notification(Some(&format!("zo-task-{}", task.id)), &note);
    }
    done();
}

/// Registers how every finished run of this computer reaches the person (the timer, "Run now" in the window, and "Run now" from the
/// phone or the web through the service). The runner may call it from any thread; the notification is made on the main one.
pub fn register_notifier() {
    scheduler::set_notifier(Arc::new(|_tenant, task, notice| {
        let (task, notice): Shown = (task.clone(), notice.clone());
        glib::MainContext::default().invoke(move || {
            let f = crate::TASKS_DONE.with(|t| t.borrow().clone());
            show(&task, &notice, &|| {
                if let Some(f) = &f {
                    f();
                }
            });
        });
    }));
}

/// Starts "Run now" for a task of this computer (the result shows in its card and as a notification).
pub fn run_now(id: &str) -> Result<(), String> {
    scheduler::run_now_in_background(id, Arc::new(AppApprover), "scheduler", Arc::new(|t, o| scheduler::notify("", t, &o.notice)))
}

/// Looks for tasks that are due and runs them one after another in the background. `done` is called when they have finished.
pub fn check(done: Rc<dyn Fn()>) {
    if RUNNING.with(Cell::get) || schedules::due(schedules::now_millis()).is_empty() {
        return;
    }
    RUNNING.with(|r| r.set(true));
    let (tx, rx) = mpsc::channel::<()>();
    std::thread::spawn(move || {
        let cancel = Arc::new(AtomicBool::new(false));
        // a task that is late (the computer was off) runs once now and says "Missed 09:00"; the finished run reaches the person
        // through the registered notifier
        scheduler::run_due(&AppApprover, "scheduler", &cancel, &mut |task, out| scheduler::notify("", task, &out.notice));
        let _ = tx.send(());
    });
    glib::timeout_add_local(Duration::from_millis(500), move || match rx.try_recv() {
        Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
        _ => {
            RUNNING.with(|r| r.set(false));
            done();
            glib::ControlFlow::Break
        }
    });
}

/// Starts looking every 20 seconds (and once now, which runs what was missed while the computer was off).
pub fn start(done: Rc<dyn Fn()>) {
    register_notifier();
    let d = done.clone();
    glib::timeout_add_seconds_local(20, move || {
        check(d.clone());
        glib::ControlFlow::Continue
    });
    check(done);
}
