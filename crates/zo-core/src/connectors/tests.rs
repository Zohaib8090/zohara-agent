use super::add::{self, Input, Opts};
use super::*;
use crate::mcp_client::ServerConfig;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

const PAGE: &str = include_str!("../../data/registry-page.json");
const FEATURED_RECORDS: &str = include_str!("../../data/registry-featured.json");

fn page() -> Value {
    serde_json::from_str(PAGE).unwrap()
}

fn entry_of(id: &str) -> Entry {
    let p = parse_page(&page()).unwrap();
    p.entries.into_iter().find(|e| e.id == id).unwrap_or_else(|| panic!("{id} is not kept"))
}

// ---- the parser, on recorded registry JSON ----

#[test]
fn the_recorded_page_keeps_only_what_can_be_added() {
    let p = parse_page(&page()).unwrap();
    let ids: Vec<&str> = p.entries.iter().map(|e| e.id.as_str()).collect();
    // old versions of inference.sh (isLatest false) are dropped, its newest is kept once
    assert_eq!(ids.iter().filter(|i| **i == "ac.inference.sh/mcp").count(), 1);
    let inf = entry_of("ac.inference.sh/mcp");
    assert_eq!(inf.version, "2.0.1");
    assert_eq!(inf.remotes.len(), 1);
    // an entry with a title, a site and a remote
    let m = entry_of("ac.mizan/registry");
    assert_eq!((m.title.as_str(), m.publisher.as_str(), m.website.as_deref()), ("Mizan", "mizan.ac", Some("https://mizan.ac/capabilities")));
    // several remotes, of which only the streamable-http one is kept (the old SSE transport is not supported)
    assert_eq!(entry_of("com.notion/mcp").remotes.iter().map(|r| r.url.as_str()).collect::<Vec<_>>(), ["https://mcp.notion.com/mcp"]);
    // gone: only SSE, an address with a placeholder, deprecated
    for gone in ["io.github.Evozim/linear-broker", "ai.autorfp/mcp", "ai.agentlookups/greenlight"] {
        assert!(!ids.contains(&gone), "{gone} must not be listed");
    }
    // the synthetic bad addresses are never listed
    assert!(ids.iter().all(|i| !i.starts_with("test.invalid/")), "{ids:?}");
    assert!(p.skipped >= 9);
    assert_eq!(p.next.as_deref(), Some("com.notion/mcp:1.0.1"));
}

#[test]
fn headers_and_local_packages_are_read() {
    // a key the connector asks for
    let h = &entry_of("ae.easyvoice/tts").remotes[0].headers;
    assert_eq!(h.len(), 1);
    assert!(h[0].name == "Authorization" && h[0].secret && h[0].required && h[0].value.is_none());
    // an npm package with environment variables: pinned to the listed version, run with -y
    let l = entry_of("ai.2kw/mcp-server").local.unwrap();
    assert_eq!((l.runtime.as_str(), l.command.as_str()), ("npx", "npx -y @2kw/ai-mcp-server@8.0.3"));
    assert!(l.env.iter().any(|e| e.name == "AI_2KW_API_KEY" && e.secret && e.required));
    assert!(l.env.iter().any(|e| e.name == "AI_2KW_BASE_URL" && !e.required));
    // a pypi package with a literal argument
    let l = entry_of("ai.arsentev/contextburn").local.unwrap();
    assert_eq!(l.command, "uvx ai-contextburn@0.2.1 mcp".replace("ai-contextburn", &l.package));
    assert!(l.command.starts_with("uvx ") && l.command.ends_with(" mcp"), "{}", l.command);
    // an entry that has both a remote and a package is listed in both lists
    let both = entry_of("ai.agent-bev/bev-door");
    assert!(both.local.is_some() && !both.remotes.is_empty());
}

#[test]
fn packages_that_cannot_be_started_are_refused() {
    let rec = |pkg: Value| json!({"server": {"name": "x.test/p", "description": "d", "version": "1", "packages": [pkg]}, "_meta": {"io.modelcontextprotocol.registry/official": {"status": "active", "isLatest": true}}});
    let ok = json!({"registryType": "npm", "identifier": "@scope/pkg", "version": "1.2.3", "transport": {"type": "stdio"}});
    assert_eq!(Entry::parse(&rec(ok.clone())).unwrap().local.unwrap().command, "npx -y @scope/pkg@1.2.3");
    let bad = |f: &dyn Fn(&mut Value)| {
        let mut p = ok.clone();
        f(&mut p);
        assert!(Entry::parse(&rec(p)).is_none());
    };
    bad(&|p| p["registryType"] = json!("oci"));
    bad(&|p| p["registryType"] = json!("mcpb"));
    bad(&|p| p["identifier"] = json!("pkg; rm -rf ~"));
    bad(&|p| p["identifier"] = json!("--registry=https://evil.example"));
    bad(&|p| p["transport"] = json!({"type": "streamable-http"}));
    bad(&|p| p["runtimeArguments"] = json!([{"type": "positional", "value": "--flag"}]));
    bad(&|p| p["packageArguments"] = json!([{"type": "positional", "isRequired": true, "valueHint": "path"}]));
    bad(&|p| p["packageArguments"] = json!([{"type": "positional", "value": "a b; c"}]));
    bad(&|p| p["packageArguments"] = json!([{"type": "positional", "value": "{home}"}]));
    // a required argument that has a value, and an optional one without, are fine
    let mut p = ok.clone();
    p["packageArguments"] = json!([{"type": "named", "name": "--mode", "value": "stdio"}, {"type": "positional", "valueHint": "x"}]);
    assert_eq!(Entry::parse(&rec(p)).unwrap().local.unwrap().command, "npx -y @scope/pkg@1.2.3 --mode stdio");
    // an unpinned version ("latest") is not used as a pin
    let mut p = ok;
    p["version"] = json!("latest");
    assert_eq!(Entry::parse(&rec(p)).unwrap().local.unwrap().command, "npx -y @scope/pkg");
}

#[test]
fn third_party_text_is_cleaned_and_status_and_ids_are_checked() {
    assert_eq!(clean_text("a\u{0}b\n\n  c\td", 50), "a b c d");
    assert_eq!(clean_text(&"x".repeat(100), 10).chars().count(), 10);
    assert!(clean_text(&"x".repeat(100), 10).ends_with('…'));
    let rec = |status: &str, latest: bool, name: &str, url: &str| json!({"server": {"name": name, "title": "T\u{7}itle", "description": "d", "version": "1", "remotes": [{"type": "streamable-http", "url": url}]}, "_meta": {"io.modelcontextprotocol.registry/official": {"status": status, "isLatest": latest}}});
    assert!(Entry::parse(&rec("active", true, "a.b/c", "https://a.example.com/mcp")).is_some());
    assert_eq!(Entry::parse(&rec("active", true, "a.b/c", "https://a.example.com/mcp")).unwrap().title, "T itle");
    assert!(Entry::parse(&rec("deprecated", true, "a.b/c", "https://a.example.com/mcp")).is_none());
    assert!(Entry::parse(&rec("active", false, "a.b/c", "https://a.example.com/mcp")).is_none());
    for bad in ["nope", "a/b/c", "a b/c", "../x/y", "a.b/c;d", "/c", "a.b/"] {
        assert!(Entry::parse(&rec("active", true, bad, "https://a.example.com/mcp")).is_none(), "{bad}");
    }
    assert!(Entry::parse(&json!({"server": {}})).is_none());
    // a record without the status block is not trusted
    assert!(Entry::parse(&json!({"server": {"name": "a.b/c", "remotes": [{"type": "streamable-http", "url": "https://a.example.com/mcp"}]}})).is_none());
    assert!(parse_page(&json!({"nope": 1})).is_err());
    assert_eq!(publisher_of("com.notion/mcp"), "notion.com");
    assert_eq!(publisher_of("app.linear/linear"), "linear.app");
    assert_eq!(publisher_of("io.github.github/github-mcp-server"), "github.com/github");
    assert_eq!(publisher_of("com.cloudflare.mcp/mcp"), "mcp.cloudflare.com");
}

#[test]
fn categories_come_from_words_not_from_pieces_of_words() {
    assert_eq!(category_of("Pay with Stripe: payments and invoices"), "finance");
    assert_eq!(category_of("Slack messages and email"), "chat");
    assert_eq!(category_of("Spotify and YouTube music"), "media");
    assert_eq!(category_of("Notes and wiki pages"), "docs");
    assert_eq!(category_of("Search the web"), "search");
    assert_eq!(category_of("Postgres database queries"), "data");
    assert_eq!(category_of("GitHub repos and issues"), "dev");
    assert_eq!(category_of("A mysterious thing"), "other");
    // "cipher" is not "ci"; "bankrupt" would only match by its start, and only words longer than four letters may do that
    assert_eq!(category_of("cipher"), "other");
}

#[test]
fn bad_addresses_in_a_registry_page_are_never_listed_and_the_guard_refuses_them_too() {
    for (url, why) in [("http://example.com/mcp", "plain http"), ("https://localhost:8443/mcp", "localhost"), ("https://10.0.0.5/mcp", "private"), ("https://127.0.0.1/mcp", "loopback"), ("https://169.254.169.254/mcp", "metadata"), ("https://user:pw@example.com/mcp", "credentials"), ("https://0x7f.1/mcp", "odd spelling"), ("https://127.1/mcp", "short loopback"), ("https://[::1]/mcp", "ipv6 loopback")] {
        let rec = json!({"server": {"name": "x.test/y", "description": "d", "version": "1", "remotes": [{"type": "streamable-http", "url": url}]}, "_meta": {"io.modelcontextprotocol.registry/official": {"status": "active", "isLatest": true}}});
        assert!(Entry::parse(&rec).is_none(), "{why}: {url}");
        assert!(add::check_public(url, false).is_err(), "{why}: {url}");
    }
    assert!(add::check_public("https://93.184.216.34/mcp", false).is_ok(), "a public address literal passes without a name lookup");
    assert!(add::check_public("https://[2606:2800:220:1:248:1893:25c8:1946]/mcp", false).is_ok());
}

// ---- the query ----

#[test]
fn what_a_client_sends_is_checked() {
    let q = Query::parse(Some("  github "), Some("a.b/c:1.0.0"), Some("local"), Some("dev")).unwrap();
    assert_eq!((q.q.as_str(), q.cursor.as_deref(), q.kind, q.category.as_deref()), ("github", Some("a.b/c:1.0.0"), Kind::Local, Some("dev")));
    let d = Query::parse(None, None, None, Some("all")).unwrap();
    assert_eq!((d.kind, d.category, d.cursor), (Kind::Remote, None, None));
    assert!(Query::parse(Some(&"x".repeat(81)), None, None, None).is_err());
    assert!(Query::parse(Some("a\nb"), None, None, None).is_err());
    assert!(Query::parse(None, Some("a b"), None, None).is_err());
    assert!(Query::parse(None, Some("a&cursor=b"), None, None).is_err());
    assert!(Query::parse(None, Some(&"a".repeat(301)), None, None).is_err());
    assert!(Query::parse(None, None, Some("both"), None).is_err());
    assert!(Query::parse(None, None, None, Some("../x")).is_err());
    let q = Query::from_query_string("q=hello%20world%26x&kind=remote&category=docs&cursor=a.b%2Fc%3A1").unwrap();
    assert_eq!((q.q.as_str(), q.category.as_deref(), q.cursor.as_deref()), ("hello world&x", Some("docs"), Some("a.b/c:1")));
    assert_eq!(Query::from_query_string(&q.to_query_string()).unwrap().q, q.q);
    assert_eq!(pct("a b&c/d"), "a%20b%26c%2Fd");
}

// ---- a fake registry ----

#[derive(Default)]
struct Reg {
    /// Request lines, newest last.
    seen: StdMutex<Vec<String>>,
    down: AtomicBool,
    hits: AtomicUsize,
    /// Ids the featured lookups do not find.
    missing: StdMutex<Vec<String>>,
    /// Another record to answer for an id (the registry changed it).
    replace: StdMutex<Vec<(String, Value)>>,
}

/// A registry on this computer: `/v0/servers` answers the recorded page (a second page when a cursor is given), `/v0/servers/<id>/versions/latest`
/// answers the recorded featured records.
fn fake_registry() -> (String, Arc<Reg>) {
    let reg = Arc::new(Reg::default());
    let r2 = reg.clone();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let featured: Value = serde_json::from_str(FEATURED_RECORDS).unwrap();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut buf = Vec::new();
            let mut byte = [0u8; 1];
            while !buf.ends_with(b"\r\n\r\n") && stream.read(&mut byte).unwrap_or(0) == 1 {
                buf.push(byte[0]);
            }
            let head = String::from_utf8_lossy(&buf).to_string();
            let target = head.split_whitespace().nth(1).unwrap_or("").to_string();
            r2.seen.lock().unwrap().push(target.clone());
            r2.hits.fetch_add(1, Ordering::SeqCst);
            let reply = |status: &str, ctype: &str, text: String| format!("HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}", text.len());
            let out = if r2.down.load(Ordering::SeqCst) {
                reply("503 Service Unavailable", "text/plain", "down".into())
            } else if let Some(rest) = target.strip_prefix("/v0/servers/") {
                let id = unpct(rest.strip_suffix("/versions/latest").unwrap_or(rest));
                let replaced = r2.replace.lock().unwrap().iter().find(|(i, _)| *i == id).map(|(_, v)| v.clone());
                let rec = replaced.or_else(|| featured.get(&id).cloned());
                match rec.filter(|_| !r2.missing.lock().unwrap().contains(&id)) {
                    Some(v) => reply("200 OK", "application/json", v.to_string()),
                    None => reply("404 Not Found", "application/json", "{}".into()),
                }
            } else if target.starts_with("/v0/servers?") {
                let mut p = page();
                if target.contains("cursor=") {
                    // the second page: one more entry and no more pages
                    p["servers"] = json!([p["servers"][4].clone()]);
                    p["metadata"] = json!({"count": 1});
                }
                reply("200 OK", "application/json", p.to_string())
            } else {
                reply("404 Not Found", "text/plain", String::new())
            };
            let _ = stream.write_all(out.as_bytes());
        }
    });
    (format!("http://{addr}"), reg)
}

#[test]
fn the_list_has_the_featured_connectors_first_and_filters_work() {
    let (base, reg) = fake_registry();
    let (src, cache) = (Source::at(&base), Cache::new());
    let b = browse(&src, &cache, 1000, &Query::parse(None, None, None, None).unwrap()).unwrap();
    // all twelve featured answer; they come in the list's order and carry their mark and label
    assert_eq!(b.featured.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), FEATURED.iter().map(|(i, _)| *i).collect::<Vec<_>>());
    assert!(b.featured.iter().all(|e| e.featured && !e.remotes.is_empty()));
    assert_eq!(b.featured[0].to_json(Kind::Remote)["trust"], FEATURED_LABEL);
    assert_eq!(b.featured.iter().find(|e| e.id == "com.stripe/mcp").unwrap().category, "finance");
    // Notion is in the registry page too, but is listed once (as featured); the others say they were not reviewed
    assert!(b.entries.iter().all(|e| e.id != "com.notion/mcp"));
    assert!(b.entries.iter().all(|e| !e.featured && e.to_json(Kind::Remote)["trust"] == NOT_REVIEWED));
    assert!(b.entries.iter().any(|e| e.id == "ae.easyvoice/tts"));
    assert!(b.entries.iter().all(|e| !e.remotes.is_empty()), "the remote list has remote entries only");
    assert_eq!(b.next.as_deref(), None, "the fake registry's second page ends the list");
    assert!(!b.stale && b.error.is_none() && b.checked_at == Some(1000));
    // the Local list: packages only, no featured
    let l = browse(&src, &cache, 1000, &Query::parse(None, None, Some("local"), None).unwrap()).unwrap();
    assert!(l.featured.is_empty());
    let ids: Vec<&str> = l.entries.iter().map(|e| e.id.as_str()).collect();
    assert!(ids.contains(&"ai.2kw/mcp-server") && ids.contains(&"ai.arsentev/contextburn") && ids.contains(&"ai.agent-bev/bev-door"), "{ids:?}");
    assert!(!ids.contains(&"ac.mizan/registry"));
    assert!(l.entries.iter().all(|e| e.local.is_some()));
    // a category and a search word
    let d = browse(&src, &cache, 1000, &Query::parse(None, None, None, Some("finance")).unwrap()).unwrap();
    assert!(d.featured.iter().all(|e| e.category == "finance") && d.featured.len() == 2);
    let q = browse(&src, &cache, 1000, &Query::parse(Some("linear"), None, None, None).unwrap()).unwrap();
    assert_eq!(q.featured.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["app.linear/linear"]);
    assert!(reg.seen.lock().unwrap().iter().any(|t| t.contains("search=linear") && t.contains("version=latest")), "the word is passed to the registry's search");
    // a page that is not the first has no featured list
    let c = browse(&src, &cache, 1000, &Query::parse(None, Some("a.b/c:1"), None, None).unwrap()).unwrap();
    assert!(c.featured.is_empty());
    assert!(reg.seen.lock().unwrap().iter().any(|t| t.contains("cursor=a.b%2Fc%3A1")));
}

#[test]
fn a_featured_connector_the_registry_does_not_answer_for_is_not_shown() {
    let (base, reg) = fake_registry();
    reg.missing.lock().unwrap().push("com.stripe/mcp".into());
    // the registry answers for linear with a record of another connector: not trusted
    reg.replace.lock().unwrap().push(("app.linear/linear".into(), serde_json::from_str::<Value>(FEATURED_RECORDS).unwrap()["com.figma.mcp/mcp"].clone()));
    let b = browse(&Source::at(&base), &Cache::new(), 5, &Query::parse(None, None, None, None).unwrap()).unwrap();
    let ids: Vec<&str> = b.featured.iter().map(|e| e.id.as_str()).collect();
    assert!(!ids.contains(&"com.stripe/mcp") && !ids.contains(&"app.linear/linear"), "{ids:?}");
    assert_eq!(ids.len(), 10);
    // a deprecated featured connector is gone too
    let mut dep: Value = serde_json::from_str::<Value>(FEATURED_RECORDS).unwrap()["com.zapier/mcp"].clone();
    dep["_meta"]["io.modelcontextprotocol.registry/official"]["status"] = json!("deprecated");
    reg.replace.lock().unwrap().push(("com.zapier/mcp".into(), dep));
    let b = browse(&Source::at(&base), &Cache::new(), 5, &Query::parse(None, None, None, None).unwrap()).unwrap();
    assert!(b.featured.iter().all(|e| e.id != "com.zapier/mcp"));
}

#[test]
fn the_answer_is_kept_for_an_hour_and_an_old_one_is_served_when_the_registry_is_down() {
    let (base, reg) = fake_registry();
    let (src, cache) = (Source::at(&base), Cache::new());
    let q = Query::parse(None, None, None, None).unwrap();
    browse(&src, &cache, 1000, &q).unwrap();
    let after_first = reg.hits.load(Ordering::SeqCst);
    assert!(after_first >= 13, "one page and twelve featured lookups, got {after_first}");
    browse(&src, &cache, 1000 + CACHE_SECS - 1, &q).unwrap();
    assert_eq!(reg.hits.load(Ordering::SeqCst), after_first, "nothing is asked again inside the hour");
    // an hour later the registry is down: the old list is served, marked old, with the reason
    reg.down.store(true, Ordering::SeqCst);
    let t = 1000 + CACHE_SECS + 10;
    let b = browse(&src, &cache, t, &q).unwrap();
    assert!(b.stale && b.error.is_some() && b.checked_at == Some(1000), "{:?} {:?}", b.stale, b.error);
    assert_eq!(b.featured.len(), 12);
    let hits = reg.hits.load(Ordering::SeqCst);
    // and it is not asked again for five minutes
    browse(&src, &cache, t + RETRY_SECS - 1, &q).unwrap();
    assert_eq!(reg.hits.load(Ordering::SeqCst), hits, "a failure is not repeated at once");
    // back up: the next try after the wait gets a fresh list
    reg.down.store(false, Ordering::SeqCst);
    let b = browse(&src, &cache, t + RETRY_SECS + 1, &q).unwrap();
    assert!(!b.stale && b.checked_at == Some(t + RETRY_SECS + 1));
    // nothing cached and the registry down: an error a person can read
    let (base2, reg2) = fake_registry();
    reg2.down.store(true, Ordering::SeqCst);
    let e = browse(&Source::at(&base2), &Cache::new(), 1, &q).err().unwrap();
    assert!(e.contains("directory"), "{e}");
    // a registry that cannot be reached at all
    let e = browse(&Source::at("http://127.0.0.1:1"), &Cache::new(), 1, &q).err().unwrap();
    assert!(e.contains("could not be reached"), "{e}");
}

#[test]
fn the_registry_is_asked_at_most_forty_times_a_minute_whoever_asks() {
    let cache = Cache::new();
    let calls = AtomicUsize::new(0);
    for i in 0..60 {
        let got = cache.get(&format!("k{i}"), 100, || {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(json!(i))
        });
        if i < UPSTREAM_PER_MINUTE {
            assert!(got.value.is_ok(), "{i}");
        } else {
            assert!(got.value.is_err(), "{i}");
        }
    }
    assert_eq!(calls.load(Ordering::SeqCst), UPSTREAM_PER_MINUTE);
    // a minute later there is room again, and answers already held are still given
    assert!(cache.get("later", 161, || Ok(json!(1))).value.is_ok());
    assert!(cache.get("k0", 161, || panic!("held")).value.is_ok());
}

#[test]
fn a_registry_that_is_too_big_or_not_json_is_refused() {
    let serve = |ctype: &'static str, body: String| -> String {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let a = l.local_addr().unwrap();
        std::thread::spawn(move || {
            for mut s in l.incoming().flatten() {
                let mut b = [0u8; 2048];
                let _ = s.read(&mut b);
                let _ = s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes());
            }
        });
        format!("http://{a}/x")
    };
    assert!(fetch(&serve("text/html", "<html>".into())).unwrap_err().contains("JSON"));
    assert!(fetch(&serve("application/json", "{not json".into())).unwrap_err().contains("not JSON"));
    assert!(fetch(&serve("application/json", format!("[\"{}\"]", "x".repeat(MAX_BODY as usize)))).unwrap_err().contains("too big"));
    assert_eq!(fetch(&serve("application/json", "{\"a\":1}".into())).unwrap()["a"], 1);
}

// ---- adding ----

/// A stand-in remote MCP server: answers initialize, tools/list ("search") and tools/call (echoes the X-Api-Key header).
fn fake_mcp(need_key: bool) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut buf = Vec::new();
            let mut byte = [0u8; 1];
            while !buf.ends_with(b"\r\n\r\n") && stream.read(&mut byte).unwrap_or(0) == 1 {
                buf.push(byte[0]);
            }
            let head = String::from_utf8_lossy(&buf).to_string();
            let header = |name: &str| head.lines().find_map(|l| l.to_lowercase().strip_prefix(&format!("{name}:")).map(|v| v.trim().to_string())).unwrap_or_default();
            let mut body = vec![0u8; header("content-length").parse::<usize>().unwrap_or(0)];
            let _ = stream.read_exact(&mut body);
            let v: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            let id = v["id"].clone();
            let reply = |status: &str, text: String| format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}", text.len());
            let out = if need_key && header("x-api-key") != "k-123" {
                reply("401 Unauthorized", String::new())
            } else if id.is_null() {
                reply("202 Accepted", String::new())
            } else {
                match v["method"].as_str().unwrap_or("") {
                    "initialize" => reply("200 OK", json!({"jsonrpc": "2.0", "id": id, "result": {"protocolVersion": "2025-06-18", "capabilities": {"tools": {}}}}).to_string()),
                    "tools/list" => reply("200 OK", json!({"jsonrpc": "2.0", "id": id, "result": {"tools": [{"name": "search", "description": "x", "annotations": {"readOnlyHint": true}, "inputSchema": {"type": "object", "properties": {}}}]}}).to_string()),
                    _ => reply("200 OK", json!({"jsonrpc": "2.0", "id": id, "result": {"content": [{"type": "text", "text": format!("key={}", header("x-api-key"))}], "isError": false}}).to_string()),
                }
            };
            let _ = stream.write_all(out.as_bytes());
        }
    });
    format!("http://{addr}/mcp")
}

fn sandbox(name: &str) -> (std::sync::MutexGuard<'static, ()>, std::path::PathBuf) {
    let g = crate::store::TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!("zo-conn-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("XDG_DATA_HOME", dir.join("data"));
    std::env::set_var("XDG_CONFIG_HOME", dir.join("config"));
    std::env::set_var("ZOHARA_AGENT_KEYS_FILE", dir.join("keys.json"));
    mcp_client::TEST_ALLOW_PRIVATE.store(true, Ordering::SeqCst);
    (g, dir)
}

fn local_entry(url: &str, headers: Vec<Header>) -> Entry {
    Entry { id: "test.example/fake".into(), title: "Fake".into(), description: "d".into(), version: "1".into(), website: None, publisher: "example.test".into(), category: "other".into(), featured: false, remotes: vec![Remote { url: url.into(), headers }], local: None }
}

fn input(id: &str) -> Input {
    Input { id: id.into(), ..Default::default() }
}

fn no_open(_: &str) {}

fn tests_opts() -> Opts<'static> {
    Opts { allow_private: true, open: &no_open }
}

#[test]
fn a_connector_is_tried_shown_and_only_then_saved_and_its_key_is_not_in_the_file() {
    let (_g, dir) = sandbox("add");
    let url = fake_mcp(true);
    let header = Header { name: "X-Api-Key".into(), description: "Your key".into(), secret: true, required: true, value: None };
    let e = local_entry(&url, vec![header]);
    let opts = tests_opts();
    // the key is needed: the answer names it, and nothing is stored
    let err = add::preview_entry(&e, 100, &input(&e.id), &opts).unwrap_err();
    assert!(err.starts_with("X-Api-Key is needed"), "{err}");
    // a header the connector does not ask for is refused (no way to send other headers through this form)
    let mut i = input(&e.id);
    i.headers.insert("X-Evil".into(), "1".into());
    assert!(add::preview_entry(&e, 100, &i, &opts).unwrap_err().contains("does not ask for a header named X-Evil"));
    // a wrong key: the server says no, nothing is kept
    let mut i = input(&e.id);
    i.headers.insert("x-api-key".into(), "wrong".into());
    let err = add::preview_entry(&e, 100, &i, &opts).unwrap_err();
    assert!(err.contains("refused") || err.contains("sign"), "{err}");
    assert!(crate::ai::keys::get(&mcp_client::header_id("fake", "X-Api-Key")).is_none(), "a failed try leaves no key behind");
    assert!(add::commit("fake", 101).is_err(), "nothing waits after a failed try");
    // the right key: the tools are shown, nothing is saved yet
    let mut i = input(&e.id);
    i.headers.insert("X-Api-Key".into(), "k-123".into());
    let p = add::preview_entry(&e, 100, &i, &opts).unwrap();
    assert_eq!((p.name.as_str(), p.tools.as_slice(), p.trust.as_str()), ("fake", ["search".to_string()].as_slice(), NOT_REVIEWED));
    assert!(p.to_json()["notice"].as_str().unwrap().contains("still asks you first"));
    assert!(mcp_client::load_servers().is_empty(), "the preview saves nothing");
    // saving: only with what was tried, and only in time
    assert!(add::commit("other", 101).is_err());
    assert!(add::commit("fake", 100 + add::PENDING_SECS + 1).unwrap_err().contains("Nothing is waiting"));
    assert!(add::preview_entry(&e, 100, &i, &opts).is_ok());
    let msg = add::commit("fake", 200).unwrap();
    assert!(msg.contains("search"), "{msg}");
    assert!(add::commit("fake", 201).is_err(), "it can only be saved once");
    let saved = mcp_client::load_servers();
    assert_eq!(saved.len(), 1);
    assert_eq!((saved[0].registry.as_deref(), saved[0].command.as_str(), saved[0].secret_headers.as_slice()), (Some("test.example/fake"), url.as_str(), ["X-Api-Key".to_string()].as_slice()));
    assert!(saved[0].headers.is_empty());
    // the key is in the key store only: not in the server list file, and the server still works with it
    let file = std::fs::read_to_string(dir.join("config/zohara-agent/mcp-servers.json")).unwrap_or_else(|_| std::fs::read_dir(dir.join("config")).map(|_| String::new()).unwrap());
    assert!(!file.contains("k-123"), "the key must not be in the server list: {file}");
    let mut ext = mcp_client::External::start_with(&saved);
    assert!(ext.problems.is_empty(), "{:?}", ext.problems);
    assert_eq!(ext.execute("fake__search", &json!({}), crate::Mode::Auto, &crate::approval::Deny).unwrap(), "key=k-123");
    // the same name cannot be added twice
    assert!(add::preview_entry(&e, 300, &i, &opts).unwrap_err().contains("already added"));
    // removing it forgets the key
    mcp_client::clear_secrets(&saved[0]);
    assert!(crate::ai::keys::get(&mcp_client::header_id("fake", "X-Api-Key")).is_none());
}

#[test]
fn cancelling_a_try_removes_what_it_stored() {
    let (_g, _dir) = sandbox("cancel");
    let url = fake_mcp(true);
    let e = local_entry(&url, vec![Header { name: "X-Api-Key".into(), description: String::new(), secret: true, required: true, value: None }]);
    let mut i = input(&e.id);
    i.headers.insert("X-Api-Key".into(), "k-123".into());
    add::preview_entry(&e, 10, &i, &tests_opts()).unwrap();
    assert!(crate::ai::keys::get(&mcp_client::header_id("fake", "X-Api-Key")).is_some());
    add::cancel("fake");
    assert!(crate::ai::keys::get(&mcp_client::header_id("fake", "X-Api-Key")).is_none());
    assert!(add::commit("fake", 11).is_err());
}

#[test]
fn an_authorization_key_without_its_scheme_is_a_bearer_token_and_fixed_headers_are_sent() {
    let (_g, _dir) = sandbox("auth");
    let url = fake_mcp(false);
    let e = local_entry(&url, vec![Header { name: "Authorization".into(), description: String::new(), secret: true, required: true, value: None }, Header { name: "X-Client".into(), description: String::new(), secret: false, required: false, value: Some("zohara".into()) }]);
    let mut i = input(&e.id);
    i.name = Some("my conn!".into());
    i.headers.insert("Authorization".into(), "abc123".into());
    let p = add::preview_entry(&e, 10, &i, &tests_opts()).unwrap();
    assert_eq!(p.name, "my_conn");
    assert_eq!(crate::ai::keys::get(&mcp_client::header_id("my_conn", "Authorization")).as_deref(), Some("Bearer abc123"));
    add::commit("my_conn", 11).unwrap();
    let s = &mcp_client::load_servers()[0];
    assert_eq!(s.headers.get("X-Client").map(String::as_str), Some("zohara"), "a fixed public value is kept in the file");
    // a value with a line break is refused
    let mut i = input(&e.id);
    i.name = Some("b".into());
    i.headers.insert("Authorization".into(), "x\r\nX-Other: 1".into());
    assert!(add::preview_entry(&e, 10, &i, &tests_opts()).unwrap_err().contains("not valid"));
}

#[test]
fn an_address_that_is_not_the_connectors_own_or_not_public_is_refused() {
    let (_g, _dir) = sandbox("ssrf");
    mcp_client::TEST_ALLOW_PRIVATE.store(false, Ordering::SeqCst);
    let strict = Opts { allow_private: false, open: &no_open };
    for url in ["https://localhost/mcp", "https://127.0.0.1/mcp", "https://10.1.2.3/mcp", "https://169.254.169.254/latest", "http://example.com/mcp"] {
        let e = local_entry(url, vec![]);
        let err = add::preview_entry(&e, 1, &input(&e.id), &strict).unwrap_err();
        assert!(err.contains("public https"), "{url}: {err}");
    }
    let e = local_entry("https://93.184.216.34/mcp", vec![]);
    let mut i = input(&e.id);
    i.url = Some("https://evil.example/mcp".into());
    assert!(add::preview_entry(&e, 1, &i, &strict).unwrap_err().contains("not one the directory lists"));
    // a server from the directory is only connected to at a public address, also when it is started later
    let url = fake_mcp(false);
    let cfg = ServerConfig { name: "dir".into(), command: url, enabled: true, registry: Some("a.b/c".into()), ..Default::default() };
    assert!(mcp_client::test(&cfg).is_err(), "a directory server at a local address does not connect");
    let plain = ServerConfig { registry: None, ..cfg };
    assert!(mcp_client::test(&plain).is_ok(), "one added by hand still can (that is the person's own choice)");
}

#[test]
fn a_local_program_needs_its_command_typed_back_and_its_environment_answers() {
    let (_g, dir) = sandbox("local");
    let script = dir.join("server.sh");
    std::fs::write(&script, "#!/bin/sh\nwhile IFS= read -r line; do\n id=$(printf '%s' \"$line\" | sed -n 's/.*\"id\":\\([0-9]*\\).*/\\1/p')\n case \"$line\" in\n *'\"method\":\"initialize\"'*) echo '{\"jsonrpc\":\"2.0\",\"id\":'$id',\"result\":{\"protocolVersion\":\"2025-03-26\",\"capabilities\":{\"tools\":{}}}}';;\n *'\"method\":\"tools/list\"'*) echo '{\"jsonrpc\":\"2.0\",\"id\":'$id',\"result\":{\"tools\":[{\"name\":\"echo\",\"description\":\"e\",\"inputSchema\":{\"type\":\"object\"}}]}}';;\n esac\ndone\n").unwrap();
    let command = format!("sh {}", script.display());
    let mut e = local_entry("https://x.example.com/mcp", vec![]);
    e.remotes.clear();
    e.local = Some(Local { runtime: "npx".into(), package: "p".into(), version: None, command: command.clone(), env: vec![EnvVar { name: "API_KEY".into(), description: "the key".into(), secret: true, required: true, default: None }, EnvVar { name: "MODE".into(), description: String::new(), secret: false, required: false, default: Some("fast".into()) }] });
    let mut i = Input { id: e.id.clone(), kind: Some(Kind::Local), ..Default::default() };
    // not started without the confirmation, and the answer names what would run
    let err = add::preview_entry(&e, 1, &i, &tests_opts()).unwrap_err();
    assert!(err.starts_with("Confirm by name first") && err.contains(&command), "{err}");
    i.confirm = Some("sh /somewhere/else.sh".into());
    assert!(add::preview_entry(&e, 1, &i, &tests_opts()).unwrap_err().starts_with("Confirm by name first"));
    i.confirm = Some(command.clone());
    assert!(add::preview_entry(&e, 1, &i, &tests_opts()).unwrap_err().starts_with("API_KEY is needed"));
    i.env.insert("NOPE".into(), "1".into());
    assert!(add::preview_entry(&e, 1, &i, &tests_opts()).unwrap_err().contains("does not ask for NOPE"));
    i.env.remove("NOPE");
    i.env.insert("API_KEY".into(), "secret-1".into());
    let p = add::preview_entry(&e, 1, &i, &tests_opts()).unwrap();
    assert_eq!((p.kind, p.tools.as_slice(), p.target.as_str()), (Kind::Local, ["echo".to_string()].as_slice(), command.as_str()));
    add::commit(&p.name, 2).unwrap();
    let s = &mcp_client::load_servers()[0];
    assert_eq!((s.env.get("API_KEY").map(String::as_str), s.env.get("MODE").map(String::as_str)), (Some("secret-1"), Some("fast")), "the default is filled in");
    // a missing runtime is reported before anything is started
    let mut e2 = e.clone();
    e2.id = "test.example/other".into();
    e2.local.as_mut().unwrap().command = "definitely-not-installed-xyz -y p".into();
    i.id = e2.id.clone();
    i.confirm = Some("definitely-not-installed-xyz -y p".into());
    i.name = Some("other".into());
    assert!(add::preview_entry(&e2, 1, &i, &tests_opts()).unwrap_err().contains("was not found"));
}

#[test]
fn a_restricted_account_cannot_try_or_save_or_change_a_connector() {
    let _g = crate::store::TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let t = crate::tenant::testkit::fresh(true);
    crate::tenant::scope(&t, || {
        let e = local_entry("https://93.184.216.34/mcp", vec![]);
        assert_eq!(add::preview_entry(&e, 1, &input(&e.id), &tests_opts()).unwrap_err(), RESTRICTED_MESSAGE);
        assert_eq!(add::commit("x", 1).unwrap_err(), RESTRICTED_MESSAGE);
        assert_eq!(accept_change("x").unwrap_err(), RESTRICTED_MESSAGE);
        // the list is not asked for: the answer is an explanation
        let v = browse_here(&Query::parse(None, None, None, None).unwrap()).unwrap();
        assert_eq!((v["restricted"].as_bool(), v["entries"].as_array().map(Vec::len), v["message"].as_str()), (Some(true), Some(0), Some(RESTRICTED_MESSAGE)));
    });
}

// ---- address changes ----

fn saved(name: &str, id: &str, url: &str) -> mcp_client::ServerConfig {
    mcp_client::ServerConfig { name: name.into(), command: url.into(), enabled: true, registry: Some(id.into()), headers: [("X-Api-Key".to_string(), "old-key".to_string())].into(), secret_headers: vec!["X-Api-Key".into()], ..Default::default() }
}

#[test]
fn a_connector_whose_address_changed_is_held_until_the_person_decides() {
    let (_g, _dir) = sandbox("changed");
    let (base, reg) = fake_registry();
    let (src, cache) = (Source::at(&base), Cache::new());
    crate::ai::keys::set(&mcp_client::header_id("notion", "X-Api-Key"), "old-secret").unwrap();
    crate::ai::keys::set(&mcp_client::token_id("notion"), "old-token").unwrap();
    mcp_client::save_servers(&[
        saved("notion", "com.notion/mcp", "https://mcp.notion.com/mcp"), // the registry lists it
        saved("moved", "com.figma.mcp/mcp", "https://old.example.com/mcp"), // the registry lists another address
        mcp_client::ServerConfig { name: "byhand".into(), command: "https://by.example.com/mcp".into(), enabled: true, ..Default::default() },
        mcp_client::ServerConfig { name: "local".into(), command: "npx -y x".into(), enabled: true, registry: Some("com.stripe/mcp".into()), ..Default::default() },
    ]);
    let found = check_changes(&src, &cache, 10);
    assert_eq!(found, [Changed { name: "moved".into(), old: "https://old.example.com/mcp".into(), new: "https://mcp.figma.com/mcp".into() }]);
    let all = mcp_client::load_servers();
    let get = |n: &str| all.iter().find(|s| s.name == n).unwrap().clone();
    assert!(get("moved").held && get("moved").address_changed.as_deref() == Some("https://mcp.figma.com/mcp"));
    assert!(!get("notion").held && get("notion").address_changed.is_none(), "an unchanged address stays as it is");
    assert!(!get("byhand").held && !get("local").held, "only remote servers from the directory are checked");
    // a held server is not started
    assert!(mcp_client::External::start_with(&all.iter().filter(|s| s.name == "moved").cloned().collect::<Vec<_>>()).problems.is_empty());
    // an unreachable registry changes nothing (and does not release anything)
    reg.down.store(true, Ordering::SeqCst);
    assert!(check_changes(&src, &Cache::new(), 20_000).is_empty());
    assert!(mcp_client::load_servers().iter().find(|s| s.name == "moved").unwrap().held);
    // the person agrees: the new address is used, and nothing saved for the old one goes to it
    let msg = accept_change_with("moved", true).unwrap();
    assert!(msg.contains("https://mcp.figma.com/mcp") && msg.contains("forgotten"), "{msg}");
    let s = mcp_client::load_servers().into_iter().find(|s| s.name == "moved").unwrap();
    assert_eq!((s.command.as_str(), s.held, s.enabled, s.address_changed.clone()), ("https://mcp.figma.com/mcp", false, true, None));
    assert!(s.headers.is_empty() && s.secret_headers.is_empty());
    assert!(accept_change_with("moved", true).unwrap_err().contains("has not changed"));
    assert!(accept_change_with("byhand", true).is_err() && accept_change_with("nothere", true).is_err());
    // a new address that is not public is refused even when the person says yes
    let mut all = mcp_client::load_servers();
    all.iter_mut().find(|s| s.name == "notion").unwrap().address_changed = Some("https://10.0.0.1/mcp".into());
    mcp_client::save_servers(&all);
    assert!(accept_change_with("notion", false).unwrap_err().contains("public https"));
}

#[test]
fn keeping_the_old_address_is_the_switch_and_a_registry_that_lists_it_again_releases_the_flag() {
    let (_g, _dir) = sandbox("keep");
    let (base, _reg) = fake_registry();
    let (src, cache) = (Source::at(&base), Cache::new());
    let mut s = saved("notion", "com.notion/mcp", "https://mcp.notion.com/mcp");
    s.address_changed = Some("https://elsewhere.example.com/mcp".into());
    s.held = true;
    mcp_client::save_servers(&[s]);
    // the registry lists the saved address (again): the flag goes, the hold is the person's to lift
    assert!(check_changes(&src, &cache, 5).is_empty());
    let s = &mcp_client::load_servers()[0];
    assert!(s.address_changed.is_none() && s.held);
}

#[test]
fn a_title_that_says_nothing_is_replaced_by_the_organisations_name() {
    let mk = |id: &str| Entry::parse(&json!({"server": {"name": id, "description": "d", "version": "1", "remotes": [{"type": "streamable-http", "url": "https://93.184.216.34/mcp"}]}, "_meta": {"io.modelcontextprotocol.registry/official": {"status": "active", "isLatest": true}}})).unwrap().title;
    assert_eq!(mk("com.notion/mcp"), "Notion");
    assert_eq!(mk("com.paypal.mcp/mcp"), "Paypal");
    assert_eq!(mk("io.github.someone/mcp"), "Someone");
    assert_eq!(mk("com.example/real-name"), "real-name");
}
