//! Tests only: a model server on this computer that answers by path, so a test can make a server that speaks one wire format only.

use serde_json::Value;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// What a handler answers: status, content type, and the body in pieces (each piece is flushed on its own, so a stream arrives split).
pub struct Answer {
    pub status: u16,
    pub ctype: &'static str,
    pub pieces: Vec<String>,
}

impl Answer {
    pub fn json(status: u16, body: Value) -> Answer {
        Answer { status, ctype: "application/json", pieces: vec![body.to_string()] }
    }

    pub fn sse(pieces: Vec<String>) -> Answer {
        Answer { status: 200, ctype: "text/event-stream", pieces }
    }
}

pub struct Mock {
    /// Ends in `/v1`.
    pub url: String,
    /// Every request seen: (method, path, body, authorization header, x-api-key header).
    pub seen: Arc<Mutex<Vec<(String, String, Value, String, String)>>>,
    stop: Arc<AtomicBool>,
}

impl Drop for Mock {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl Mock {
    pub fn start(handler: impl Fn(&str, &str, &Value) -> Answer + Send + 'static) -> Mock {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (seen2, stop2) = (seen.clone(), stop.clone());
        std::thread::spawn(move || {
            while !stop2.load(Ordering::SeqCst) {
                let Ok((mut s, _)) = listener.accept() else {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                    continue;
                };
                s.set_nonblocking(false).unwrap();
                let mut buf = Vec::new();
                let mut tmp = [0u8; 8192];
                let (head_end, len) = loop {
                    let n = s.read(&mut tmp).unwrap_or(0);
                    if n == 0 {
                        break (buf.len(), 0);
                    }
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&buf[..i]).to_lowercase();
                        break (i + 4, head.lines().find_map(|l| l.strip_prefix("content-length:")).and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(0));
                    }
                };
                while buf.len() < head_end + len {
                    let n = s.read(&mut tmp).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                }
                let head = String::from_utf8_lossy(&buf[..head_end.min(buf.len())]).to_string();
                let mut first = head.lines().next().unwrap_or("").split_whitespace();
                let (method, path) = (first.next().unwrap_or("").to_string(), first.next().unwrap_or("").to_string());
                let header = |name: &str| head.lines().find_map(|l| l.to_lowercase().starts_with(&format!("{name}:")).then(|| l[name.len() + 1..].trim().to_string())).unwrap_or_default();
                let (auth, xkey) = (header("authorization"), header("x-api-key"));
                let body: Value = serde_json::from_slice(&buf[head_end.min(buf.len())..]).unwrap_or(Value::Null);
                let a = handler(&method, &path, &body);
                seen2.lock().unwrap().push((method, path, body, auth, xkey));
                let _ = write!(s, "HTTP/1.1 {} X\r\ncontent-type: {}\r\nconnection: close\r\n", a.status, a.ctype);
                if a.pieces.len() <= 1 {
                    let body = a.pieces.first().cloned().unwrap_or_default();
                    let _ = write!(s, "content-length: {}\r\n\r\n{body}", body.len());
                } else {
                    let _ = write!(s, "\r\n");
                    for p in a.pieces {
                        let _ = s.write_all(p.as_bytes());
                        let _ = s.flush();
                        std::thread::sleep(std::time::Duration::from_millis(15));
                    }
                }
            }
        });
        Mock { url, seen, stop }
    }

    pub fn paths(&self) -> Vec<String> {
        self.seen.lock().unwrap().iter().map(|(m, p, ..)| format!("{m} {p}")).collect()
    }
}
