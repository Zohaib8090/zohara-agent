//! `GET /api/connectors?q=&cursor=&kind=remote|local&category=`: the connectors directory for the web page, the Android app and the
//! desktop app (docs/MCP.md "Browse connectors"). The registry is asked here (one request an hour per kind of question, stale answers
//! when it is down: `zo_core::connectors`), never by a client, so the registry never sees a person's address or search.
//!
//! Who: anybody signed in (a person, or a device credential of any kind). A restricted account gets the explanation and an empty list.
//! Rates: per person, 30 a minute; the registry itself is asked at most 40 times a minute for the whole server.

use crate::tenants::Bucket;
use crate::{App, Fail, Who};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;
use zo_core::connectors;

const PER_MIN: u32 = 30;

fn buckets() -> &'static Mutex<HashMap<String, Bucket>> {
    static B: std::sync::OnceLock<Mutex<HashMap<String, Bucket>>> = std::sync::OnceLock::new();
    B.get_or_init(|| Mutex::new(HashMap::new()))
}

fn rate(tenant: &str) -> Result<(), Fail> {
    let mut g = buckets().lock().unwrap_or_else(|e| e.into_inner());
    if g.len() > 5000 {
        g.clear();
    }
    g.entry(tenant.to_string()).or_insert_with(|| Bucket::new(PER_MIN, Instant::now())).take(Instant::now()).map_err(|w| Fail { status: 429, error: "Too many requests. Wait a moment.".into(), retry: Some(w) })
}

/// The answer for one request. `query` is the part of the address after `?`.
pub fn handle(_app: &App, query: &str, who: &Who) -> Result<Value, Fail> {
    rate(&who.tenant)?;
    let q = connectors::Query::from_query_string(query).map_err(|e| Fail::new(400, e))?;
    // restricted: everybody but an admin (the same rule as the tenant the other routes build)
    if !who.admin {
        return Ok(connectors::restricted_json());
    }
    connectors::browse(&connectors::Source::from_env(), &connectors::CACHE, connectors::now_secs(), &q).map(|b| b.to_json(q.kind, "registry")).map_err(|e| Fail::new(502, e))
}

pub fn is_path(path: &str) -> bool {
    path == "/api/connectors"
}
