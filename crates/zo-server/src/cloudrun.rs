//! The cloud runner: scheduled tasks that run on the server for the people who made them there (docs/SCHEDULED.md).
//!
//! The server keeps people's folders on its disk only while they are in use, so it cannot look through every folder every minute.
//! It keeps a small index instead: for each person who has something switched on for the cloud, the time it is next due (and
//! whether the person is an administrator). The index is in memory and, when there is a database, in the collection `sched_index`
//! (one small document per person: two numbers and a flag, never the text of a task), so that a restart finds it again.
//!
//! Every few seconds `tick` takes the people whose time has come, one thread each: the person's folder is loaded, one place of the
//! server's running answers is held (`ZO_MAX_RUNS`), and `zo_core::scheduler::run_due` runs what is due, sequentially, with every
//! approval answered "no" (nobody is there to answer). The limits of what a person may do on the server (kinds, how often, how many
//! tasks and runs a day, web tools only for people who are not administrators) are in `zo_core::scheduler`, so they hold for every
//! way a task can start. A person's data is only touched inside `tenant::scope` for that person.

use crate::push;
use crate::tenants::Refusal;
use crate::App;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use zo_core::approval::Deny;
use zo_core::scheduler::{self, Notice};

/// Where the index is kept in the database.
pub const COLLECTION: &str = "sched_index";

#[derive(Clone, Debug, PartialEq)]
struct Entry {
    /// When something of this person is due, in milliseconds since 1970.
    next: u64,
    admin: bool,
}

static INDEX: Mutex<BTreeMap<String, Entry>> = Mutex::new(BTreeMap::new());
/// People whose runs are going on now: one thread per person.
static BUSY: Mutex<Option<HashSet<String>>> = Mutex::new(None);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn now_ms() -> u64 {
    zo_core::store::schedules::now_millis()
}

/// Reads the index back from the database (at start).
pub fn load(app: &App) {
    let Some(db) = app.tenants.db() else { return };
    match db.list_docs(COLLECTION) {
        Ok(docs) => {
            let mut idx = lock(&INDEX);
            for (id, text) in docs {
                let v: Value = serde_json::from_str(&text).unwrap_or_default();
                if let Some(next) = v.get("next").and_then(Value::as_u64) {
                    idx.insert(id, Entry { next, admin: v.get("admin").and_then(Value::as_bool).unwrap_or(false) });
                }
            }
            eprintln!("zo-server: {} people have scheduled tasks on the server", idx.len());
        }
        Err(e) => eprintln!("zo-server: cannot read the index of scheduled tasks: {e}"),
    }
}

/// Records when somebody's next task is due (None: nothing is switched on, so they leave the index).
fn put(app: &App, tenant: &str, admin: bool, next: Option<u64>) {
    let changed = {
        let mut idx = lock(&INDEX);
        match next {
            Some(n) => idx.insert(tenant.to_string(), Entry { next: n, admin }) != Some(Entry { next: n, admin }),
            None => idx.remove(tenant).is_some(),
        }
    };
    if !changed {
        return;
    }
    if let Some(db) = app.tenants.db() {
        let r = match next {
            Some(n) => db.put_doc(COLLECTION, tenant, &json!({"next": n, "admin": admin}).to_string()),
            None => db.delete_doc(COLLECTION, tenant),
        };
        if let Err(e) = r {
            eprintln!("zo-server: cannot save the index of scheduled tasks: {e}");
        }
    }
}

/// When this person is next due: a task of theirs, or (an administrator with runners on machines) the next look at a runner.
fn next_wake(admin: bool) -> Option<u64> {
    let runner = if admin { zo_core::runner::host::next_tick() } else { None };
    match (scheduler::next_due_here(), runner) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// A person's tasks changed (made, edited, switched, removed, asked to run): look again at when they are next due. Called after the
/// scheduled-task operations, inside the request that made them.
pub fn refresh(app: &App, tenant: &zo_core::tenant::Tenant) {
    let next = zo_core::tenant::scope(tenant, || next_wake(!tenant.restricted));
    put(app, &tenant.id, !tenant.restricted, next);
}

/// A person's account was deleted: they leave the index.
pub fn forget(app: &App, tenant: &str) {
    put(app, tenant, false, None);
}

/// When the person is next due, if they are in the index.
#[cfg(test)]
pub fn due_at(tenant: &str) -> Option<u64> {
    lock(&INDEX).get(tenant).map(|e| e.next)
}

/// What a finished run says to the person's devices.
pub fn notify(push: &Arc<push::Hub>, tenant: &str, notice: &Notice) {
    push.notify_task(
        tenant,
        push::TaskNote { chat: notice.chat.as_deref().unwrap_or(""), title: &notice.title, body: &notice.body, ok: notice.ok, reminder: notice.kind == "reminder" },
    );
}

/// Registers the way finished runs reach people's devices.
pub fn start_notifier(app: &Arc<App>) {
    let push = app.push.clone();
    scheduler::set_notifier(Arc::new(move |tenant, _task, notice| notify(&push, tenant, notice)));
}

/// One pass: starts a thread for every person whose time has come. Returns how many were started.
pub fn tick(app: &Arc<App>) -> usize {
    let now = now_ms();
    let due: Vec<(String, bool)> = lock(&INDEX).iter().filter(|(_, e)| e.next <= now).map(|(id, e)| (id.clone(), e.admin)).collect();
    let mut started = 0;
    for (id, admin) in due {
        if !lock(&BUSY).get_or_insert_with(HashSet::new).insert(id.clone()) {
            continue; // their runs are going on
        }
        started += 1;
        let app = app.clone();
        std::thread::spawn(move || {
            run_person(&app, &id, admin);
            lock(&BUSY).get_or_insert_with(HashSet::new).remove(&id);
        });
    }
    started
}

/// Runs what is due for one person.
fn run_person(app: &Arc<App>, id: &str, admin: bool) {
    let lease = match app.tenants.acquire(id, admin, Instant::now()) {
        Ok(l) => l,
        Err(Refusal::Gone) => return put(app, id, admin, None), // the account is gone: nothing to run
        Err(_) => return,                                        // busy, over their rate or the storage is away: the next pass tries again
    };
    // one of the server's places for running answers, like a chat answer: when the server is full the task waits for the next pass
    let permit = match lease.permit_send() {
        Ok(p) => p,
        Err(_) => return,
    };
    let tenant = lease.tenant(!admin);
    let push = app.push.clone();
    let cancel = Arc::new(AtomicBool::new(false));
    let next = zo_core::tenant::scope(&tenant, || {
        let tid = tenant.id.clone();
        scheduler::run_due(&Deny, "scheduler", &cancel, &mut |_, out| notify(&push, &tid, &out.notice));
        if admin {
            // the runners on the administrator's machines: send the changed tasks, fetch what they did (docs/RUNNER.md)
            zo_core::runner::host::tick();
        }
        next_wake(admin)
    });
    put(app, id, admin, next);
    drop(permit);
    drop(lease);
}

/// Starts the timer: every 10 seconds, forever.
pub fn start(app: &Arc<App>) {
    load(app);
    start_notifier(app);
    let a = app.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_secs(10));
        tick(&a);
    });
}
