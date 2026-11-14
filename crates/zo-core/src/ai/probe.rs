//! Friendly answers about a provider for the screens that show them (desktop and web): the model list with a readable reason when it
//! cannot be loaded, and a connection test. Nothing here changes any setting.

use super::agent::{self, Config};
use super::wire;
use super::providers::{Protocol, Provider};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// How long the connection test waits for the provider.
pub const TEST_WAIT: Duration = Duration::from_secs(15);

/// The address must be an http(s) address without spaces; this is only a shape check (a restricted account has its own, stricter rule).
pub fn check_address(base_url: &str) -> Result<(), String> {
    let u = base_url.trim();
    let rest = u.strip_prefix("https://").or_else(|| u.strip_prefix("http://"));
    match rest {
        Some(r) if !r.is_empty() && r.len() <= 500 && !u.chars().any(|c| c.is_whitespace() || c.is_control()) && !r.starts_with('/') => Ok(()),
        _ => Err("The address must start with http:// or https:// and have no spaces (for example https://api.example.com/v1).".into()),
    }
}

/// A provider's error turned into a sentence a person can act on: no key, bad address, 401, timeout.
pub fn friendly(p: &Provider, base_url: &str, has_key: bool, err: &str) -> String {
    let low = err.to_lowercase();
    let name = p.name;
    if err.starts_with("A restricted account") || err.starts_with("Claude Code is not available") || err.starts_with("This server speaks") || err.starts_with("The server does not take") {
        return err.to_string();
    }
    if low.contains("answered 401") || low.contains("answered 403") {
        return if has_key || p.local {
            format!("{name} refused the key (401). Check that the key is right and belongs to {name}.")
        } else {
            format!("No API key for {name} yet: add it in the model settings, then load the models again.")
        };
    }
    if low.contains("answered 404") {
        return format!("Nothing answers at {}/models: check the address (it usually ends in /v1).", base_url.trim_end_matches('/'));
    }
    if low.contains("answered 429") {
        return format!("{name} says there are too many requests right now. Wait a moment and try again.");
    }
    if low.contains("answered 5") {
        return format!("{name} had a problem on its side ({}). Try again later.", err.split("answered ").nth(1).and_then(|r| r.get(..3)).unwrap_or("5xx"));
    }
    if low.contains("timed out") || low.contains("timeout") || low.contains("deadline") {
        return format!("{name} did not answer in time. Check the address and your connection.");
    }
    if low.contains("dns") || low.contains("lookup") || low.contains("resolve") {
        return "That address could not be found: check the host name.".into();
    }
    if low.contains("refused") {
        return "Nothing is listening at that address (connection refused): is the server running, and is the port right?".into();
    }
    if low.contains("could not reach") || low.contains("connection") || low.contains("tls") || low.contains("certificate") {
        return format!("Cannot reach {}: check the address and your connection.", base_url.trim_end_matches('/'));
    }
    if low.contains("expected") || low.contains("eof") || low.contains("invalid") && low.contains("json") {
        return format!("{} answered, but not like a model server (an OpenAI-compatible one lists its models at /models).", base_url.trim_end_matches('/'));
    }
    err.to_string()
}

/// The models a provider offers. A failure is a readable `Err`, never a panic or a raw error.
pub fn list(p: &Provider, base_url: &str, key: Option<&str>) -> Result<Vec<String>, String> {
    if p.protocol != Protocol::ClaudeCode {
        check_address(base_url)?;
    }
    agent::list_models(p, base_url, key).map_err(|e| friendly(p, base_url, key.is_some(), &e))
}

/// A real, tiny test: with a model, one short answer is asked for (15 seconds at most); without one, the model list is loaded.
/// Ok is what to tell the person; Err is the friendly reason it failed.
pub fn test(p: &'static Provider, base_url: &str, key: Option<String>, model: &str) -> Result<String, String> {
    if p.protocol == Protocol::ClaudeCode {
        return Ok("Claude Code runs on this computer with your own sign-in; there is no address to test here.".into());
    }
    check_address(base_url)?;
    let has_key = key.is_some();
    if model.trim().is_empty() {
        let n = list(p, base_url, key.as_deref())?.len();
        return Ok(format!("Connected to {}: {n} model{} available.", p.name, if n == 1 { "" } else { "s" }));
    }
    let first = try_answer(p, base_url, key.clone(), model, TEST_WAIT);
    match first {
        Ok(_) => Ok(format!("{} answered with {}: it works.", p.name, model.trim())),
        Err(e) => {
            // a server that does not understand this wire format may well speak another one: try those, and say which provider fits
            if (p.is_custom() || p.takes_format()) && may_be_other_format(&e) {
                for proto in [Protocol::OpenAi, Protocol::OpenAiResponses, Protocol::Anthropic].into_iter().filter(|x| *x != p.protocol) {
                    if let Some(other) = p.sibling(proto) {
                        if try_answer(other, base_url, key.clone(), model, ALTERNATE_WAIT).is_ok() {
                            return Err(wire::wrong_format_hint(proto));
                        }
                    }
                }
            }
            Err(friendly(p, base_url, has_key, &e))
        }
    }
}

/// How long each of the other wire formats is tried for when the first one failed.
const ALTERNATE_WAIT: Duration = Duration::from_secs(8);

/// Would another wire format perhaps work? Yes when the server answered "no such endpoint" or in a shape that is not the one asked for;
/// no for a refused key, a busy server or one that cannot be reached.
fn may_be_other_format(err: &str) -> bool {
    if err.starts_with("This server speaks") || err.starts_with("The server does not take") || err.starts_with("The answer had no") || err.starts_with("The answer was not valid JSON") {
        return true;
    }
    let status: u16 = err.strip_prefix("The provider answered ").and_then(|r| r.get(..3)).and_then(|n| n.parse().ok()).unwrap_or(0);
    matches!(status, 400 | 404 | 405 | 415 | 422 | 501)
}

/// One tiny question, `wait` at most.
fn try_answer(p: &'static Provider, base_url: &str, key: Option<String>, model: &str, wait: Duration) -> Result<String, String> {
    let cfg = Config { provider: p, base_url: base_url.to_string(), model: model.trim().to_string(), key, system: "Reply with the single word: ok".into(), vision: false };
    let cancel = Arc::new(AtomicBool::new(false));
    let (tx, rx) = std::sync::mpsc::channel();
    let c2 = cancel.clone();
    crate::tenant::spawn(move || {
        let _ = tx.send(agent::complete(&cfg, &cfg.system, "ok?", &c2));
    });
    match rx.recv_timeout(wait) {
        Ok(r) => r,
        Err(_) => {
            cancel.store(true, Ordering::SeqCst);
            Err(format!("{} did not answer within {} seconds.", p.name, wait.as_secs()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::providers;

    #[test]
    fn errors_become_sentences_a_person_can_act_on() {
        let custom = providers::find("custom").unwrap();
        let o = providers::find("openai").unwrap();
        assert!(friendly(o, "https://x/v1", false, "The provider answered 401: bad").contains("No API key"));
        assert!(friendly(o, "https://x/v1", true, "The provider answered 401: bad").contains("refused the key"));
        assert!(friendly(custom, "https://x/v1/", true, "The provider answered 404: nope").contains("https://x/v1/models"));
        assert!(friendly(o, "https://x", true, "Could not reach the provider: https://x/models: Dns Failed: no such host").contains("host name"));
        assert!(friendly(o, "https://x", true, "Could not reach the provider: Connection refused").contains("connection refused"));
        assert!(friendly(o, "https://x", true, "Could not reach the provider: timed out reading response").contains("in time"));
        assert!(friendly(o, "https://x", true, "The provider answered 503: x").contains("503"));
        assert!(check_address("ftp://x").is_err() && check_address("https://").is_err() && check_address("https://a b").is_err());
        assert!(check_address("http://127.0.0.1:8000/v1").is_ok() && check_address(" https://api.example.com/v1 ").is_ok());
    }

    use crate::ai::mock::{Answer, Mock};
    use serde_json::json;

    fn text_answer(t: &str) -> Value {
        json!({"object": "response", "status": "completed", "output": [{"type": "message", "content": [{"type": "output_text", "text": t}]}]})
    }

    use serde_json::Value;

    /// A server that answers only the one path, in the one format.
    fn only(path_end: &'static str, ok: Value) -> Mock {
        Mock::start(move |method, path, _| {
            if method == "POST" && path.ends_with(path_end) {
                Answer::json(200, ok.clone())
            } else if method == "GET" && path.ends_with("/models") {
                Answer::json(200, json!({"data": [{"id": "m1"}]}))
            } else {
                Answer::json(404, json!({"error": {"message": "Not Found"}}))
            }
        })
    }

    #[test]
    fn friendly_leaves_the_format_hints_alone() {
        let c = providers::find("custom").unwrap();
        let hint = "This server speaks the OpenAI Responses format: pick \"Custom endpoint: OpenAI Responses\" (or set the API format to OpenAI Responses).";
        assert_eq!(friendly(c, "https://x/v1", true, hint), hint);
        let e = "The server does not take /chat/completions (The provider answered 404: Not Found). It may speak another API format: try \"Custom endpoint: OpenAI Responses\".";
        assert_eq!(friendly(c, "https://x/v1", true, e), e);
    }

    #[test]
    fn the_test_of_a_chat_endpoint_that_speaks_responses_says_which_provider_to_pick() {
        let m = only("/v1/responses", text_answer("ok"));
        let c = providers::find("custom").unwrap();
        let e = test(c, &m.url, Some("k".into()), "m").unwrap_err();
        assert!(e.starts_with("This server speaks the OpenAI Responses format") && e.contains("Custom endpoint: OpenAI Responses"), "{e}");
        // the right provider works
        let r = providers::find("custom-responses").unwrap();
        assert!(test(r, &m.url, Some("k".into()), "m").unwrap().contains("it works"));
        // and a server that speaks the Claude format is found from the Responses provider
        let claude = only("/v1/messages", json!({"type": "message", "role": "assistant", "content": [{"type": "text", "text": "ok"}], "stop_reason": "end_turn"}));
        let e = test(r, &claude.url, Some("k".into()), "m").unwrap_err();
        assert!(e.contains("Claude format") && e.contains("Custom endpoint: Claude format"), "{e}");
        // a hosted provider with an API format hint gets the same help
        let o = providers::find("openai").unwrap();
        let e = test(o, &m.url, Some("k".into()), "m").unwrap_err();
        assert!(e.contains("OpenAI Responses"), "{e}");
    }

    #[test]
    fn the_test_does_not_guess_formats_when_the_key_is_refused_or_nothing_speaks() {
        let c = providers::find("custom").unwrap();
        let m = Mock::start(|_, _, _| Answer::json(401, json!({"error": {"message": "bad key"}})));
        let e = test(c, &m.url, Some("k".into()), "m").unwrap_err();
        assert!(e.contains("refused the key") && !e.contains("speaks"), "{e}");
        assert_eq!(m.paths().len(), 1, "a refused key is not tried again in other formats: {:?}", m.paths());
        // nothing speaks any format: the plain failure, after every format was tried once
        let m = Mock::start(|_, _, _| Answer::json(404, json!({"error": {"message": "Not Found"}})));
        let e = test(c, &m.url, Some("k".into()), "m").unwrap_err();
        assert!(e.contains("does not take /chat/completions") && !e.contains("This server speaks"), "{e}");
        assert_eq!(m.paths(), ["POST /v1/chat/completions", "POST /v1/responses", "POST /v1/messages"]);
    }

    #[test]
    fn the_model_list_of_every_format_is_read_from_models() {
        let m = only("/nothing", Value::Null);
        for id in ["custom", "custom-responses", "custom-claude"] {
            let p = providers::find(id).unwrap();
            assert_eq!(list(p, &m.url, Some("k")).unwrap(), ["m1"], "{id}");
        }
        let seen = m.seen.lock().unwrap();
        assert!(seen.iter().all(|(meth, path, ..)| meth == "GET" && path == "/v1/models"));
        // the Claude-format list carries the key in both places, the others as a bearer only
        assert_eq!((seen[2].3.as_str(), seen[2].4.as_str()), ("Bearer k", "k"));
        assert_eq!((seen[1].3.as_str(), seen[1].4.as_str()), ("Bearer k", ""));
    }
}
