//! Turning one conversation into each provider's request, and each provider's answer back into one shape.
//! Pure functions, no network, so they are easy to test.

use super::providers::{Protocol, Provider};
use crate::tools::ToolDef;

/// A tool as the model is told about it: Zohara Agent's own, or one from an MCP server added in Settings.
#[derive(Clone, Debug)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub schema: Value,
}

/// Zohara Agent's own tools.
pub fn builtin_specs() -> Vec<ToolSpec> {
    crate::tools::active().into_iter().map(|d: &ToolDef| ToolSpec { name: d.name.into(), description: d.description.into(), schema: d.schema() }).collect()
}
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub args: Value,
}

/// A picture on disk that goes to the model along with a message or a tool result. Only the path is kept in a saved chat;
/// the file is read when a request is made.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImageRef {
    pub path: String,
    pub mime: String,
}

impl ImageRef {
    /// `None` unless the path is an existing picture of a kind the model APIs take.
    pub fn from_path(path: &str) -> Option<ImageRef> {
        let lower = path.to_lowercase();
        let mime = if lower.ends_with(".png") {
            "image/png"
        } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
            "image/jpeg"
        } else if lower.ends_with(".webp") {
            "image/webp"
        } else if lower.ends_with(".gif") {
            "image/gif"
        } else {
            return None;
        };
        let meta = std::fs::metadata(path).ok()?;
        (meta.is_file() && meta.len() <= MAX_IMAGE_BYTES).then(|| ImageRef { path: path.to_string(), mime: mime.to_string() })
    }
}

/// A picture that came with a message from the web app: its bytes (base64) are in the request itself. Never saved: after the answer
/// the turn becomes plain text with the picture's name (`agent::forget_inline_images`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InlineImage {
    pub mime: String,
    pub b64: String,
    pub name: String,
}

fn inline_pairs(imgs: &[InlineImage]) -> Vec<(String, String)> {
    // only the four kinds the model APIs take, whatever the turn says
    imgs.iter().filter(|i| matches!(i.mime.as_str(), "image/png" | "image/jpeg" | "image/webp" | "image/gif")).map(|i| (i.mime.clone(), i.b64.clone())).collect()
}

/// The largest picture that is sent (the model APIs refuse about 5 MB and more).
pub const MAX_IMAGE_BYTES: u64 = 4_500_000;

fn base64_std(data: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(A[(n >> 18) as usize & 63] as char);
        out.push(A[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { A[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { A[n as usize & 63] as char } else { '=' });
    }
    out
}

/// (mime, base64) of the picture at `path`, for an MCP result; `None` if it is not a picture that can be sent.
pub fn encode_path(path: &str) -> Option<(String, String)> {
    encoded(&ImageRef::from_path(path)?)
}

/// (mime, base64) of a picture, read now; `None` if the file is gone or too large.
fn encoded(img: &ImageRef) -> Option<(String, String)> {
    // A saved chat comes from the client (sync), so its picture paths are not trusted: a restricted tenant never has a file read
    // from the server's disk, and for everyone else the kind comes from the file name, never from the saved mime.
    if crate::tenant::is_restricted() {
        return None;
    }
    let checked = ImageRef::from_path(&img.path)?;
    let data = std::fs::read(&checked.path).ok()?;
    (data.len() as u64 <= MAX_IMAGE_BYTES).then(|| (checked.mime, base64_std(&data)))
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolResult {
    pub id: String,
    pub name: String,
    pub content: String,
    pub is_error: bool,
    /// Pictures the tool produced (a screenshot): the model is shown them next to the text.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ImageRef>,
}

/// One step of a conversation, the same for every provider.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Turn {
    User(String),
    /// A message that comes with pictures.
    UserImages(String, Vec<ImageRef>),
    /// A message with pictures that were sent along as data (from the web app): they are used for this question only.
    UserInline(String, Vec<InlineImage>),
    Assistant { text: String, calls: Vec<ToolCall> },
    Results(Vec<ToolResult>),
}

/// What one request used, as the provider reported it. `input` is what was sent fresh; `cached` came from the provider's cache (cheaper
/// and quicker); `written` was put into the cache by this request (Anthropic). Together they are everything that was sent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cached: u64,
    pub written: u64,
}

impl Usage {
    pub fn total_input(&self) -> u64 {
        self.input + self.cached + self.written
    }

    fn is_empty(&self) -> bool {
        *self == Usage::default()
    }

    /// Takes what a piece of an answer says about usage (a later piece may only know some of the numbers).
    fn merge(&mut self, other: Usage) {
        self.input = self.input.max(other.input);
        self.output = self.output.max(other.output);
        self.cached = self.cached.max(other.cached);
        self.written = self.written.max(other.written);
    }
}

/// The usage numbers inside an answer (or a piece of one), in each provider's own words.
fn usage_in(protocol: Protocol, usage: &Value) -> Usage {
    let n = |v: &Value| v.as_u64().unwrap_or(0);
    match protocol {
        Protocol::Anthropic => Usage { input: n(&usage["input_tokens"]), output: n(&usage["output_tokens"]), cached: n(&usage["cache_read_input_tokens"]), written: n(&usage["cache_creation_input_tokens"]) },
        Protocol::OpenAi => {
            let cached = n(&usage["prompt_tokens_details"]["cached_tokens"]);
            Usage { input: n(&usage["prompt_tokens"]).saturating_sub(cached), output: n(&usage["completion_tokens"]), cached, written: 0 }
        }
        Protocol::OpenAiResponses => {
            // routers vary: the Responses names first, the chat names when that is what a router fills in
            let cached = n(&usage["input_tokens_details"]["cached_tokens"]).max(n(&usage["prompt_tokens_details"]["cached_tokens"]));
            let input = if usage["input_tokens"].is_null() { &usage["prompt_tokens"] } else { &usage["input_tokens"] };
            let output = if usage["output_tokens"].is_null() { &usage["completion_tokens"] } else { &usage["output_tokens"] };
            Usage { input: n(input).saturating_sub(cached), output: n(output), cached, written: 0 }
        }
        Protocol::Gemini => {
            let cached = n(&usage["cachedContentTokenCount"]);
            Usage { input: n(&usage["promptTokenCount"]).saturating_sub(cached), output: n(&usage["candidatesTokenCount"]), cached, written: 0 }
        }
        Protocol::ClaudeCode => Usage::default(),
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reply {
    pub text: String,
    pub calls: Vec<ToolCall>,
    pub usage: Option<Usage>,
}

pub struct Request {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Value,
}

fn has_params(t: &ToolSpec) -> bool {
    t.schema["properties"].as_object().is_some_and(|o| !o.is_empty())
}

pub fn build(p: &Provider, base_url: &str, key: Option<&str>, model: &str, system: &str, turns: &[Turn], tools: &[ToolSpec]) -> Request {
    let base = base_url.trim_end_matches('/');
    match p.protocol {
        Protocol::Anthropic => {
            let messages: Vec<Value> = turns.iter().map(|t| match t {
                Turn::User(s) => json!({ "role": "user", "content": s }),
                Turn::UserImages(s, imgs) => {
                    let mut c: Vec<Value> = imgs.iter().filter_map(encoded).map(|(m, d)| json!({ "type": "image", "source": { "type": "base64", "media_type": m, "data": d } })).collect();
                    c.push(json!({ "type": "text", "text": s }));
                    json!({ "role": "user", "content": c })
                }
                Turn::UserInline(s, imgs) => {
                    let mut c: Vec<Value> = inline_pairs(imgs).into_iter().map(|(m, d)| json!({ "type": "image", "source": { "type": "base64", "media_type": m, "data": d } })).collect();
                    c.push(json!({ "type": "text", "text": s }));
                    json!({ "role": "user", "content": c })
                }
                Turn::Assistant { text, calls } => {
                    let mut c = Vec::new();
                    if !text.is_empty() {
                        c.push(json!({ "type": "text", "text": text }));
                    }
                    for k in calls {
                        c.push(json!({ "type": "tool_use", "id": k.id, "name": k.name, "input": k.args }));
                    }
                    json!({ "role": "assistant", "content": c })
                }
                Turn::Results(rs) => json!({ "role": "user", "content": rs.iter().map(|r| {
                    let pics: Vec<Value> = r.images.iter().filter_map(encoded).map(|(m, d)| json!({ "type": "image", "source": { "type": "base64", "media_type": m, "data": d } })).collect();
                    let content = if pics.is_empty() { json!(r.content) } else { let mut c = vec![json!({ "type": "text", "text": r.content })]; c.extend(pics); json!(c) };
                    json!({ "type": "tool_result", "tool_use_id": r.id, "content": content, "is_error": r.is_error })
                }).collect::<Vec<_>>() }),
            }).collect();
            // top-level cache_control: the provider keeps the start of the conversation (tools, instructions, earlier messages) in its
            // cache, so a long chat is read at a fraction of the price and much faster
            let mut body = json!({ "model": model, "max_tokens": 4096, "messages": messages });
            // (only Anthropic itself is told to cache: another server may refuse a field it does not know)
            if !p.is_custom() {
                body["cache_control"] = json!({ "type": "ephemeral" });
            }
            if !system.is_empty() {
                body["system"] = json!(system);
            }
            if !tools.is_empty() {
                body["tools"] = json!(tools.iter().map(|t| json!({ "name": t.name, "description": t.description, "input_schema": t.schema.clone() })).collect::<Vec<_>>());
            }
            let mut headers = vec![("anthropic-version".to_string(), "2023-06-01".to_string())];
            if let Some(k) = key {
                headers.push(("x-api-key".into(), k.into()));
                // routers that speak the Claude format differ on where they look for the key: both are the same key, sent to the same address
                if p.is_custom() {
                    headers.push(("authorization".into(), format!("Bearer {k}")));
                }
            }
            Request { url: format!("{base}/messages"), headers, body }
        }
        Protocol::OpenAi => {
            let mut messages = Vec::new();
            if !system.is_empty() {
                messages.push(json!({ "role": "system", "content": system }));
            }
            for t in turns {
                match t {
                    Turn::User(s) => messages.push(json!({ "role": "user", "content": s })),
                    Turn::UserImages(s, imgs) => {
                        let mut c = vec![json!({ "type": "text", "text": s })];
                        c.extend(imgs.iter().filter_map(encoded).map(|(m, d)| json!({ "type": "image_url", "image_url": { "url": format!("data:{m};base64,{d}") } })));
                        messages.push(json!({ "role": "user", "content": c }));
                    }
                    Turn::UserInline(s, imgs) => {
                        let mut c = vec![json!({ "type": "text", "text": s })];
                        c.extend(inline_pairs(imgs).into_iter().map(|(m, d)| json!({ "type": "image_url", "image_url": { "url": format!("data:{m};base64,{d}") } })));
                        messages.push(json!({ "role": "user", "content": c }));
                    }
                    Turn::Assistant { text, calls } => {
                        let mut m = json!({ "role": "assistant", "content": if text.is_empty() { Value::Null } else { json!(text) } });
                        if !calls.is_empty() {
                            m["tool_calls"] = json!(calls.iter().map(|k| json!({
                                "id": k.id, "type": "function",
                                "function": { "name": k.name, "arguments": k.args.to_string() } })).collect::<Vec<_>>());
                        }
                        messages.push(m);
                    }
                    Turn::Results(rs) => {
                        for r in rs {
                            messages.push(json!({ "role": "tool", "tool_call_id": r.id, "content": r.content }));
                        }
                        // a tool message can only hold text, so the pictures a tool made follow as a message of their own
                        let mut pics: Vec<Value> = Vec::new();
                        for r in rs {
                            for (m, d) in r.images.iter().filter_map(encoded) {
                                pics.push(json!({ "type": "text", "text": format!("The picture returned by {}:", r.name) }));
                                pics.push(json!({ "type": "image_url", "image_url": { "url": format!("data:{m};base64,{d}") } }));
                            }
                        }
                        if !pics.is_empty() {
                            messages.push(json!({ "role": "user", "content": pics }));
                        }
                    }
                }
            }
            let mut body = json!({ "model": model, "messages": messages });
            if !tools.is_empty() {
                body["tools"] = json!(tools.iter().map(|t| json!({ "type": "function",
                    "function": { "name": t.name, "description": t.description, "parameters": t.schema.clone() } })).collect::<Vec<_>>());
            }
            let headers = key.map(|k| vec![("authorization".to_string(), format!("Bearer {k}"))]).unwrap_or_default();
            Request { url: format!("{base}/chat/completions"), headers, body }
        }
        Protocol::OpenAiResponses => {
            let mut input: Vec<Value> = Vec::new();
            let pic = |m: String, d: String| json!({ "type": "input_image", "image_url": format!("data:{m};base64,{d}") });
            for t in turns {
                match t {
                    Turn::User(s) => input.push(json!({ "type": "message", "role": "user", "content": [{ "type": "input_text", "text": s }] })),
                    Turn::UserImages(s, imgs) => {
                        let mut c = vec![json!({ "type": "input_text", "text": s })];
                        c.extend(imgs.iter().filter_map(encoded).map(|(m, d)| pic(m, d)));
                        input.push(json!({ "type": "message", "role": "user", "content": c }));
                    }
                    Turn::UserInline(s, imgs) => {
                        let mut c = vec![json!({ "type": "input_text", "text": s })];
                        c.extend(inline_pairs(imgs).into_iter().map(|(m, d)| pic(m, d)));
                        input.push(json!({ "type": "message", "role": "user", "content": c }));
                    }
                    Turn::Assistant { text, calls } => {
                        if !text.is_empty() {
                            input.push(json!({ "type": "message", "role": "assistant", "content": [{ "type": "output_text", "text": text }] }));
                        }
                        for k in calls {
                            input.push(json!({ "type": "function_call", "call_id": k.id, "name": k.name, "arguments": k.args.to_string() }));
                        }
                    }
                    Turn::Results(rs) => {
                        for r in rs {
                            input.push(json!({ "type": "function_call_output", "call_id": r.id, "output": r.content }));
                        }
                        // an output can only hold text, so the pictures a tool made follow as a message of their own
                        let mut pics: Vec<Value> = Vec::new();
                        for r in rs {
                            for (m, d) in r.images.iter().filter_map(encoded) {
                                pics.push(json!({ "type": "input_text", "text": format!("The picture returned by {}:", r.name) }));
                                pics.push(pic(m, d));
                            }
                        }
                        if !pics.is_empty() {
                            input.push(json!({ "type": "message", "role": "user", "content": pics }));
                        }
                    }
                }
            }
            let mut body = json!({ "model": model, "input": input, "store": false });
            if !system.is_empty() {
                body["instructions"] = json!(system);
            }
            if !tools.is_empty() {
                body["tools"] = json!(tools.iter().map(|t| json!({ "type": "function", "name": t.name, "description": t.description, "parameters": t.schema.clone(), "strict": false })).collect::<Vec<_>>());
            }
            let headers = key.map(|k| vec![("authorization".to_string(), format!("Bearer {k}"))]).unwrap_or_default();
            Request { url: format!("{base}/responses"), headers, body }
        }
        Protocol::Gemini => {
            let contents: Vec<Value> = turns.iter().flat_map(|t| -> Vec<Value> { match t {
                Turn::User(s) => vec![json!({ "role": "user", "parts": [{ "text": s }] })],
                Turn::UserImages(s, imgs) => {
                    let mut parts = vec![json!({ "text": s })];
                    parts.extend(imgs.iter().filter_map(encoded).map(|(m, d)| json!({ "inlineData": { "mimeType": m, "data": d } })));
                    vec![json!({ "role": "user", "parts": parts })]
                }
                Turn::UserInline(s, imgs) => {
                    let mut parts = vec![json!({ "text": s })];
                    parts.extend(inline_pairs(imgs).into_iter().map(|(m, d)| json!({ "inlineData": { "mimeType": m, "data": d } })));
                    vec![json!({ "role": "user", "parts": parts })]
                }
                Turn::Assistant { text, calls } => {
                    let mut parts = Vec::new();
                    if !text.is_empty() {
                        parts.push(json!({ "text": text }));
                    }
                    for k in calls {
                        let mut part = json!({ "functionCall": { "name": k.name, "args": k.args } });
                        // Gemini thinking models need their thoughtSignature handed back (kept after "~" in the id)
                        if let Some((_, sig)) = k.id.split_once('~') {
                            part["thoughtSignature"] = json!(sig);
                        }
                        parts.push(part);
                    }
                    vec![json!({ "role": "model", "parts": parts })]
                }
                Turn::Results(rs) => {
                    let mut out = vec![json!({ "role": "user", "parts": rs.iter().map(|r|
                        json!({ "functionResponse": { "name": r.name, "response": { "content": r.content, "is_error": r.is_error } } })).collect::<Vec<_>>() })];
                    // pictures cannot ride inside a function response, so they follow as a message of their own
                    let mut pics: Vec<Value> = Vec::new();
                    for r in rs {
                        for (m, d) in r.images.iter().filter_map(encoded) {
                            pics.push(json!({ "text": format!("The picture returned by {}:", r.name) }));
                            pics.push(json!({ "inlineData": { "mimeType": m, "data": d } }));
                        }
                    }
                    if !pics.is_empty() {
                        out.push(json!({ "role": "user", "parts": pics }));
                    }
                    out
                }
            } }).collect();
            let mut body = json!({ "contents": contents });
            if !system.is_empty() {
                body["systemInstruction"] = json!({ "parts": [{ "text": system }] });
            }
            if !tools.is_empty() {
                // Gemini refuses an empty parameter object, so tools without arguments leave it out
                body["tools"] = json!([{ "functionDeclarations": tools.iter().map(|t| {
                    let mut d = json!({ "name": t.name, "description": t.description });
                    if has_params(t) {
                        d["parameters"] = gemini_schema(&t.schema);
                    }
                    d
                }).collect::<Vec<_>>() }]);
            }
            let headers = key.map(|k| vec![("x-goog-api-key".to_string(), k.to_string())]).unwrap_or_default();
            Request { url: format!("{base}/models/{model}:generateContent"), headers, body }
        }
        Protocol::ClaudeCode => Request { url: String::new(), headers: Vec::new(), body: Value::Null },
    }
}

/// Gemini accepts only a small part of JSON Schema (an OpenAPI subset) and answers 400 to the rest
/// ("Unknown name additionalProperties"), so tools from MCP servers are reduced to what it understands.
/// Gemini puts a `thoughtSignature` beside a function call; it is kept in the call id after a "~" so it travels with the call.
fn with_signature(id: String, part: &Value) -> String {
    match part.get("thoughtSignature").or_else(|| part.get("thought_signature")).and_then(Value::as_str) {
        Some(sig) => format!("{id}~{sig}"),
        None => id,
    }
}

pub fn gemini_schema(v: &Value) -> Value {
    const KEEP: &[&str] = &["type", "format", "description", "nullable", "enum", "properties", "required", "items",
        "minItems", "maxItems", "minimum", "maximum", "anyOf", "title"];
    match v {
        Value::Object(m) => {
            let mut out = serde_json::Map::new();
            for (k, val) in m {
                if k == "const" {
                    out.insert("enum".into(), json!([val]));
                    continue;
                }
                if !KEEP.contains(&k.as_str()) {
                    continue;
                }
                match k.as_str() {
                    "properties" => {
                        let props = val.as_object().map(|o| o.iter().map(|(n, p)| (n.clone(), gemini_schema(p))).collect()).unwrap_or_default();
                        out.insert(k.clone(), Value::Object(props));
                    }
                    "type" if val.is_array() => {
                        // ["string","null"] becomes type string + nullable
                        let list = val.as_array().cloned().unwrap_or_default();
                        if let Some(t) = list.iter().find(|t| t.as_str() != Some("null")) {
                            out.insert("type".into(), t.clone());
                        }
                        if list.iter().any(|t| t.as_str() == Some("null")) {
                            out.insert("nullable".into(), json!(true));
                        }
                    }
                    "enum" | "required" | "type" | "format" | "description" | "title" | "nullable" | "minItems" | "maxItems"
                    | "minimum" | "maximum" => {
                        out.insert(k.clone(), val.clone());
                    }
                    _ => {
                        out.insert(k.clone(), gemini_schema(val));
                    }
                }
            }
            // an object or array without the parts Gemini insists on
            if out.get("type").and_then(Value::as_str) == Some("array") && !out.contains_key("items") {
                out.insert("items".into(), json!({ "type": "string" }));
            }
            Value::Object(out)
        }
        Value::Array(a) => Value::Array(a.iter().map(gemini_schema).collect()),
        other => other.clone(),
    }
}

/// Which wire format an answer body is in, judging by its shape (`None` when it is none of the three).
pub fn shape_of(v: &Value) -> Option<Protocol> {
    if v["choices"].is_array() || v["object"] == "chat.completion" {
        Some(Protocol::OpenAi)
    } else if v["output"].is_array() || v["object"] == "response" || v["output_text"].is_string() {
        Some(Protocol::OpenAiResponses)
    } else if (v["type"] == "message" && v["content"].is_array()) || v["stop_reason"].is_string() {
        Some(Protocol::Anthropic)
    } else {
        None
    }
}

/// The sentence that tells a person which provider to pick when the server answered in another wire format.
pub fn wrong_format_hint(speaks: Protocol) -> String {
    use super::providers::format_label;
    format!("This server speaks the {0} format: set the API format of this provider to {0}.", format_label(speaks))
}

/// Anything in an error that points at another wire format: the server says which endpoint it wants.
pub fn format_in_error(err: &str, asked: Protocol) -> Option<Protocol> {
    let low = err.to_lowercase();
    let responses = low.contains("/v1/responses") || low.contains("/responses") || low.contains("responses api") || low.contains("responses format") || low.contains("use the responses");
    let claude = low.contains("/v1/messages") || low.contains("/messages endpoint") || low.contains("anthropic format") || low.contains("claude format");
    let chat = low.contains("/chat/completions") || low.contains("chat completions");
    match asked {
        Protocol::OpenAi if responses => Some(Protocol::OpenAiResponses),
        Protocol::OpenAi if claude => Some(Protocol::Anthropic),
        Protocol::OpenAiResponses if chat && !responses => Some(Protocol::OpenAi),
        Protocol::OpenAiResponses if claude => Some(Protocol::Anthropic),
        Protocol::Anthropic if responses => Some(Protocol::OpenAiResponses),
        Protocol::Anthropic if chat => Some(Protocol::OpenAi),
        _ => None,
    }
}

/// A Responses-format `output` item list into text and calls. Tolerates what routers do: nulls, a plain-string message content,
/// `arguments` that is already an object, a missing `call_id`.
fn parse_responses(v: &Value, reply: &mut Reply) {
    for item in v["output"].as_array().into_iter().flatten() {
        match item["type"].as_str() {
            Some("message") => match &item["content"] {
                Value::String(s) => reply.text.push_str(s),
                Value::Array(parts) => {
                    for part in parts {
                        match part["type"].as_str() {
                            Some("output_text") | Some("text") => reply.text.push_str(part["text"].as_str().unwrap_or("")),
                            Some("refusal") => reply.text.push_str(part["refusal"].as_str().unwrap_or("")),
                            _ => {}
                        }
                    }
                }
                _ => {}
            },
            Some("function_call") => {
                let i = reply.calls.len();
                let args = match &item["arguments"] {
                    Value::String(raw) => serde_json::from_str(raw).ok().filter(Value::is_object),
                    o @ Value::Object(_) => Some(o.clone()),
                    _ => None,
                };
                reply.calls.push(ToolCall {
                    id: item["call_id"].as_str().or_else(|| item["id"].as_str()).filter(|s| !s.is_empty()).map(str::to_string).unwrap_or_else(|| format!("call_{i}")),
                    name: item["name"].as_str().unwrap_or("").into(),
                    args: args.unwrap_or_else(|| json!({})),
                });
            }
            // reasoning, web_search_call and the like: nothing to show or run
            _ => {}
        }
    }
    // some routers give the text only as one plain string
    if reply.text.is_empty() {
        if let Some(t) = v["output_text"].as_str() {
            reply.text = t.to_string();
        }
    }
}

pub fn parse(protocol: Protocol, v: &Value) -> Result<Reply, String> {
    if let Some(e) = v.get("error").filter(|e| !e.is_null()) {
        return Err(error_text(e));
    }
    // another wire format than the one asked for: say which provider fits
    let wrong = |expected: bool| if expected { None } else { shape_of(v).filter(|s| *s != protocol).map(wrong_format_hint) };
    let mut reply = Reply::default();
    match protocol {
        Protocol::ClaudeCode => return Err("Claude Code mode does not use a web protocol.".into()),
        Protocol::Anthropic => {
            if let Some(h) = wrong(v["content"].is_array()) {
                return Err(h);
            }
            for b in v["content"].as_array().ok_or("The answer had no content.")? {
                match b["type"].as_str() {
                    Some("text") => reply.text.push_str(b["text"].as_str().unwrap_or("")),
                    Some("tool_use") => reply.calls.push(ToolCall {
                        id: b["id"].as_str().unwrap_or("").into(),
                        name: b["name"].as_str().unwrap_or("").into(),
                        args: b.get("input").cloned().unwrap_or_else(|| json!({})),
                    }),
                    _ => {}
                }
            }
        }
        Protocol::OpenAi => {
            let m = &v["choices"][0]["message"];
            if m.is_null() {
                return Err(wrong(false).unwrap_or_else(|| "The answer had no message.".into()));
            }
            reply.text = m["content"].as_str().unwrap_or("").to_string();
            for (i, c) in m["tool_calls"].as_array().into_iter().flatten().enumerate() {
                let raw = c["function"]["arguments"].as_str().unwrap_or("{}");
                reply.calls.push(ToolCall {
                    id: c["id"].as_str().map(str::to_string).unwrap_or_else(|| format!("call_{i}")),
                    name: c["function"]["name"].as_str().unwrap_or("").into(),
                    args: serde_json::from_str(raw).ok().filter(Value::is_object).unwrap_or_else(|| json!({})),
                });
            }
        }
        Protocol::OpenAiResponses => {
            if !v["output"].is_array() && !v["output_text"].is_string() {
                return Err(wrong(false).unwrap_or_else(|| "The answer had no output.".into()));
            }
            parse_responses(v, &mut reply);
            // a stopped answer: what was written is kept; with nothing written the reason is the answer
            let status = v["status"].as_str().unwrap_or("completed");
            if reply.text.is_empty() && reply.calls.is_empty() {
                match status {
                    "failed" | "cancelled" => return Err(responses_failure(v)),
                    "incomplete" => return Err(format!("The model stopped before answering ({}).", v["incomplete_details"]["reason"].as_str().unwrap_or("incomplete"))),
                    _ => {}
                }
            }
        }
        Protocol::Gemini => {
            let parts = v["candidates"][0]["content"]["parts"].as_array();
            let Some(parts) = parts else {
                let why = v["candidates"][0]["finishReason"].as_str().or_else(|| v["promptFeedback"]["blockReason"].as_str());
                return Err(format!("Gemini gave no answer{}.", why.map(|w| format!(" ({w})")).unwrap_or_default()));
            };
            for (i, part) in parts.iter().enumerate() {
                if let Some(t) = part["text"].as_str() {
                    reply.text.push_str(t);
                }
                if let Some(fc) = part.get("functionCall") {
                    reply.calls.push(ToolCall {
                        id: with_signature(format!("call_{i}"), part),
                        name: fc["name"].as_str().unwrap_or("").into(),
                        args: fc.get("args").cloned().unwrap_or_else(|| json!({})),
                    });
                }
            }
        }
    }
    let usage = usage_in(protocol, if protocol == Protocol::Gemini { &v["usageMetadata"] } else { &v["usage"] });
    reply.usage = (!usage.is_empty()).then_some(usage);
    Ok(reply)
}

/// Why a Responses answer says it failed (`error` inside the response, or its status).
fn responses_failure(v: &Value) -> String {
    match v.get("error").filter(|e| !e.is_null()) {
        Some(e) => error_text(e),
        None => format!("The model's answer {}.", v["status"].as_str().unwrap_or("failed")),
    }
}

pub fn error_text(e: &Value) -> String {
    e["message"].as_str().or_else(|| e.as_str()).map(str::to_string).unwrap_or_else(|| e.to_string())
}

/// Names of the models in a provider's "list models" answer.
pub fn parse_models(protocol: Protocol, v: &Value) -> Vec<String> {
    let mut out: Vec<String> = match protocol {
        Protocol::Gemini => v["models"].as_array().into_iter().flatten()
            .filter(|m| m["supportedGenerationMethods"].as_array().map_or(true, |a| a.iter().any(|x| x == "generateContent")))
            .filter_map(|m| m["name"].as_str()).map(|n| n.trim_start_matches("models/").to_string()).collect(),
        _ => {
            // `{"data":[{"id":..}]}` is the usual shape; routers also send `{"models":[..]}` or a bare list, with `id` or `name`
            let list = v["data"].as_array().or_else(|| v["models"].as_array()).or_else(|| v.as_array());
            list.into_iter().flatten().filter_map(|m| m["id"].as_str().or_else(|| m["name"].as_str()).or_else(|| m.as_str())).map(str::to_string).collect()
        }
    };
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::providers::find;

    fn conversation() -> Vec<Turn> {
        vec![
            Turn::User("what is running?".into()),
            Turn::Assistant { text: "Let me look.".into(), calls: vec![ToolCall { id: "t1".into(), name: "zohara_process_list".into(), args: json!({}) }] },
            Turn::Results(vec![ToolResult { id: "t1".into(), name: "zohara_process_list".into(), content: "firefox".into(), is_error: false, images: Vec::new() }]),
        ]
    }

    #[test]
    fn a_restricted_tenant_never_has_a_server_file_read_as_a_picture() {
        let dir = std::env::temp_dir().join(format!("zo-wire-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let secret = dir.join("secret.txt");
        std::fs::write(&secret, "TOPSECRET").unwrap();
        let png = dir.join("a.png");
        std::fs::write(&png, [1u8, 2, 3]).unwrap();
        let evil = ImageRef { path: secret.to_string_lossy().into(), mime: "text/plain".into() };
        let good = ImageRef { path: png.to_string_lossy().into(), mime: "text/plain".into() };
        // not a picture by name: refused for everyone, and the saved mime is not trusted
        assert!(encoded(&evil).is_none());
        assert_eq!(encoded(&good).unwrap().0, "image/png");
        crate::tenant::test_multi(true);
        assert!(encoded(&good).is_none(), "restricted tenants get no file reads");
        let turns = [Turn::UserImages("look".into(), vec![good.clone()])];
        let r = build(find("gemini").unwrap(), "https://x/v1", Some("k"), "m", "", &turns, &[]);
        crate::tenant::test_multi(false);
        assert!(!r.body.to_string().contains("AQID"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn anthropic_request_carries_tool_use_and_result() {
        let r = build(find("anthropic").unwrap(), "https://x/v1/", Some("k"), "m", "sys", &conversation(), &builtin_specs());
        assert_eq!(r.url, "https://x/v1/messages");
        assert!(r.headers.contains(&("x-api-key".into(), "k".into())));
        assert_eq!(r.body["system"], "sys");
        assert_eq!(r.body["messages"][1]["content"][1]["type"], "tool_use");
        assert_eq!(r.body["messages"][2]["content"][0]["tool_use_id"], "t1");
        assert_eq!(r.body["tools"][0]["input_schema"]["type"], "object");
    }

    #[test]
    fn openai_request_uses_tool_role_and_string_arguments() {
        let r = build(find("openai").unwrap(), "https://x/v1", Some("k"), "m", "sys", &conversation(), &builtin_specs());
        assert_eq!(r.url, "https://x/v1/chat/completions");
        assert!(r.headers.contains(&("authorization".into(), "Bearer k".into())));
        assert_eq!(r.body["messages"][0]["role"], "system");
        assert_eq!(r.body["messages"][2]["tool_calls"][0]["function"]["arguments"], "{}");
        assert_eq!(r.body["messages"][3]["role"], "tool");
        assert_eq!(r.body["messages"][3]["tool_call_id"], "t1");
    }

    #[test]
    fn a_local_server_needs_no_key_header() {
        let r = build(find("ollama").unwrap(), "http://127.0.0.1:11434/v1", None, "qwen", "", &[Turn::User("hi".into())], &[]);
        assert!(r.headers.is_empty());
        assert!(r.body.get("tools").is_none());
    }

    #[test]
    fn gemini_thought_signature_comes_back_with_the_call() {
        let g = parse(Protocol::Gemini, &json!({"candidates":[{"content":{"parts":[{"functionCall":{"name":"a","args":{}},"thoughtSignature":"AbC+/="}]}}]})).unwrap();
        assert!(g.calls[0].id.ends_with("~AbC+/="));
        let req = build(find("gemini").unwrap(), "https://x/v1beta", None, "m", "", &[Turn::Assistant { text: String::new(), calls: g.calls }], &[]);
        assert_eq!(req.body["contents"][0]["parts"][0]["thoughtSignature"], "AbC+/=");
    }

    #[test]
    fn gemini_schema_drops_what_gemini_does_not_know() {
        let s = json!({"type":"object","additionalProperties":false,"$schema":"x",
            "properties":{"q":{"type":["string","null"],"default":"a","additionalProperties":false},
                          "additionalProperties":{"type":"object","additionalProperties":true,"properties":{}},
                          "l":{"type":"array"}},
            "required":["q"]});
        let g = gemini_schema(&s);
        assert!(g.get("additionalProperties").is_none() && g.get("$schema").is_none());
        assert_eq!(g["properties"]["q"]["type"], "string");
        assert_eq!(g["properties"]["q"]["nullable"], true);
        assert!(g["properties"]["q"].get("default").is_none());
        assert!(g["properties"]["additionalProperties"].get("additionalProperties").is_none());
        assert_eq!(g["properties"]["l"]["items"]["type"], "string");
        assert_eq!(g["required"][0], "q");
    }

    #[test]
    fn gemini_request_leaves_out_empty_parameters() {
        let r = build(find("gemini").unwrap(), "https://x/v1beta", Some("k"), "gemini-x", "sys", &conversation(), &builtin_specs());
        assert_eq!(r.url, "https://x/v1beta/models/gemini-x:generateContent");
        assert_eq!(r.body["contents"][1]["role"], "model");
        assert_eq!(r.body["contents"][2]["parts"][0]["functionResponse"]["name"], "zohara_process_list");
        let decls = r.body["tools"][0]["functionDeclarations"].as_array().unwrap();
        let ss = decls.iter().find(|d| d["name"] == "zohara_ss").unwrap();
        assert!(ss.get("parameters").is_none());
        let read = decls.iter().find(|d| d["name"] == "zohara_fs_read").unwrap();
        assert!(read.get("parameters").is_some());
    }

    #[test]
    fn answers_parse_into_text_and_calls() {
        let a = parse(Protocol::Anthropic, &json!({"content":[{"type":"text","text":"ok"},{"type":"tool_use","id":"a","name":"zohara_ss","input":{}}]})).unwrap();
        assert_eq!((a.text.as_str(), a.calls[0].name.as_str()), ("ok", "zohara_ss"));
        let o = parse(Protocol::OpenAi, &json!({"choices":[{"message":{"content":null,"tool_calls":[{"id":"b","function":{"name":"zohara_fs_read","arguments":"{\"path\":\"/etc/hostname\"}"}}]}}]})).unwrap();
        assert_eq!(o.calls[0].args["path"], "/etc/hostname");
        let g = parse(Protocol::Gemini, &json!({"candidates":[{"content":{"parts":[{"text":"hi"},{"functionCall":{"name":"zohara_system","args":{}}}]}}]})).unwrap();
        assert_eq!((g.text.as_str(), g.calls.len()), ("hi", 1));
    }

    #[test]
    fn provider_errors_become_messages() {
        assert_eq!(parse(Protocol::OpenAi, &json!({"error":{"message":"bad key"}})).unwrap_err(), "bad key");
        assert!(parse(Protocol::Gemini, &json!({"candidates":[{"finishReason":"SAFETY"}]})).unwrap_err().contains("SAFETY"));
        assert!(parse(Protocol::OpenAi, &json!({})).is_err());
    }

    #[test]
    fn model_lists_parse() {
        assert_eq!(parse_models(Protocol::OpenAi, &json!({"data":[{"id":"b"},{"id":"a"}]})), ["a", "b"]);
        assert_eq!(parse_models(Protocol::Gemini, &json!({"models":[{"name":"models/g1","supportedGenerationMethods":["generateContent"]},{"name":"models/emb","supportedGenerationMethods":["embedContent"]}]})), ["g1"]);
    }
}

// ---------------------------------------------------------------- streaming

/// Turns a request into its streaming form: the provider then sends the answer in small pieces as it writes it.
pub fn make_streaming(req: &mut Request, protocol: Protocol) {
    match protocol {
        Protocol::OpenAi => {
            req.body["stream"] = json!(true);
            // without this an OpenAI-style server does not say how many tokens were used
            req.body["stream_options"] = json!({ "include_usage": true });
        }
        Protocol::Anthropic | Protocol::OpenAiResponses => req.body["stream"] = json!(true),
        Protocol::Gemini => req.url = req.url.replace(":generateContent", ":streamGenerateContent?alt=sse"),
        Protocol::ClaudeCode => {}
    }
}

#[derive(Default)]
struct PartialCall {
    id: String,
    name: String,
    args: String,
}

/// Collects a streamed answer line by line. `feed` gives back each new piece of text as it arrives; `finish` gives the whole answer.
#[derive(Default)]
pub struct StreamAcc {
    text: String,
    calls: Vec<PartialCall>,
    /// Anthropic numbers its content blocks; this maps a block number to its place in `calls`.
    blocks: std::collections::HashMap<u64, usize>,
    /// Responses: the same for the ids of output items (some routers number them, some only name them).
    item_ids: std::collections::HashMap<String, usize>,
    /// Responses: the output items whose text arrived piece by piece (their "done" event must not add it a second time).
    streamed_items: std::collections::HashSet<u64>,
    /// A line that has not ended yet (`push`).
    pending: String,
    usage: Usage,
    /// The wire format the pieces turned out to be in, when it is not the one asked for.
    foreign: Option<Protocol>,
}

impl StreamAcc {
    /// Takes a piece of the stream as it came off the network, which may end in the middle of a line. Complete lines are fed; the rest
    /// waits for the next piece. Gives the new text.
    pub fn push(&mut self, protocol: Protocol, chunk: &str) -> Result<Option<String>, String> {
        self.pending.push_str(chunk);
        let mut out = String::new();
        while let Some(i) = self.pending.find('\n') {
            let line: String = self.pending.drain(..=i).collect();
            if let Some(d) = self.feed(protocol, &line)? {
                out.push_str(&d);
            }
        }
        Ok((!out.is_empty()).then_some(out))
    }

    /// A last line that came without its newline (the stream just ended).
    pub fn flush(&mut self, protocol: Protocol) -> Result<Option<String>, String> {
        let rest = std::mem::take(&mut self.pending);
        if rest.trim().is_empty() {
            return Ok(None);
        }
        self.feed(protocol, &rest)
    }

    pub fn feed(&mut self, protocol: Protocol, line: &str) -> Result<Option<String>, String> {
        // "data:" with or without the space after it; comment lines (": keep-alive") and "event:" lines carry nothing
        let Some(data) = line.trim().strip_prefix("data:") else { return Ok(None) };
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            return Ok(None);
        }
        // a server's own ping that is not JSON is not an error in the answer
        if !data.starts_with('{') {
            return Ok(None);
        }
        let v: Value = serde_json::from_str(data).map_err(|e| format!("A piece of the answer was not valid JSON: {e}"))?;
        if let Some(e) = v.get("error").filter(|e| !e.is_null()) {
            return Err(error_text(e));
        }
        let kind = v["type"].as_str().unwrap_or("");
        let shape = if kind.starts_with("response.") {
            Some(Protocol::OpenAiResponses)
        } else if matches!(kind, "message_start" | "content_block_start" | "content_block_delta" | "message_delta") {
            Some(Protocol::Anthropic)
        } else if v["choices"].is_array() {
            Some(Protocol::OpenAi)
        } else {
            None
        };
        if let Some(s) = shape.filter(|s| *s != protocol && protocol != Protocol::Gemini) {
            self.foreign = Some(s);
        }
        if protocol == Protocol::OpenAiResponses {
            return self.feed_responses(&v);
        }
        let mut delta = String::new();
        // token counts come with the first piece (Anthropic), the last (OpenAI) or every piece (Gemini)
        let piece = match protocol {
            Protocol::Anthropic => match v["type"].as_str() {
                Some("message_start") => Some(&v["message"]["usage"]),
                Some("message_delta") => Some(&v["usage"]),
                _ => None,
            },
            Protocol::OpenAi => v.get("usage").filter(|u| !u.is_null()),
            Protocol::Gemini => v.get("usageMetadata"),
            Protocol::ClaudeCode | Protocol::OpenAiResponses => None,
        };
        if let Some(u) = piece {
            self.usage.merge(usage_in(protocol, u));
        }
        match protocol {
            Protocol::OpenAi => {
                let d = &v["choices"][0]["delta"];
                if let Some(t) = d["content"].as_str() {
                    delta.push_str(t);
                }
                for tc in d["tool_calls"].as_array().into_iter().flatten() {
                    let idx = tc["index"].as_u64().unwrap_or(0) as usize;
                    while self.calls.len() <= idx {
                        self.calls.push(PartialCall::default());
                    }
                    let c = &mut self.calls[idx];
                    if let Some(id) = tc["id"].as_str() {
                        c.id = id.to_string();
                    }
                    if let Some(n) = tc["function"]["name"].as_str().filter(|n| !n.is_empty()) {
                        c.name = n.to_string();
                    }
                    if let Some(a) = tc["function"]["arguments"].as_str() {
                        c.args.push_str(a);
                    }
                }
            }
            Protocol::Anthropic => match v["type"].as_str() {
                Some("content_block_start") if v["content_block"]["type"] == "tool_use" => {
                    self.calls.push(PartialCall {
                        id: v["content_block"]["id"].as_str().unwrap_or("").into(),
                        name: v["content_block"]["name"].as_str().unwrap_or("").into(),
                        args: String::new(),
                    });
                    self.blocks.insert(v["index"].as_u64().unwrap_or(0), self.calls.len() - 1);
                }
                Some("content_block_delta") => {
                    let d = &v["delta"];
                    if let Some(t) = d["text"].as_str() {
                        delta.push_str(t);
                    }
                    if let (Some(p), Some(&at)) = (d["partial_json"].as_str(), self.blocks.get(&v["index"].as_u64().unwrap_or(0))) {
                        self.calls[at].args.push_str(p);
                    }
                }
                _ => {}
            },
            Protocol::Gemini => {
                for part in v["candidates"][0]["content"]["parts"].as_array().into_iter().flatten() {
                    if let Some(t) = part["text"].as_str() {
                        delta.push_str(t);
                    }
                    if let Some(fc) = part.get("functionCall") {
                        self.calls.push(PartialCall {
                            id: with_signature(String::new(), part),
                            name: fc["name"].as_str().unwrap_or("").into(),
                            args: fc.get("args").map(|a| a.to_string()).unwrap_or_default(),
                        });
                    }
                }
            }
            Protocol::ClaudeCode | Protocol::OpenAiResponses => {}
        }
        self.text.push_str(&delta);
        Ok((!delta.is_empty()).then_some(delta))
    }

    /// The place of the call an event is about: by output index, else by item id; a call the stream never announced is added.
    fn responses_call(&mut self, v: &Value, item: &Value) -> usize {
        let index = v["output_index"].as_u64();
        let id = item["id"].as_str().or_else(|| v["item_id"].as_str()).filter(|s| !s.is_empty());
        let found = index.and_then(|i| self.blocks.get(&i).copied()).or_else(|| id.and_then(|i| self.item_ids.get(i).copied()));
        if let Some(at) = found {
            return at;
        }
        self.calls.push(PartialCall::default());
        let at = self.calls.len() - 1;
        if let Some(i) = index {
            self.blocks.insert(i, at);
        }
        if let Some(i) = id {
            self.item_ids.insert(i.to_string(), at);
        }
        at
    }

    /// One event of a Responses stream (`response.created`, `response.output_text.delta`, `response.output_item.added`, ...).
    fn feed_responses(&mut self, v: &Value) -> Result<Option<String>, String> {
        let mut delta = String::new();
        match v["type"].as_str().unwrap_or("") {
            "response.output_text.delta" => {
                if let Some(t) = v["delta"].as_str() {
                    delta.push_str(t);
                    if let Some(i) = v["output_index"].as_u64() {
                        self.streamed_items.insert(i);
                    }
                }
            }
            "response.output_item.added" if v["item"]["type"] == "function_call" => {
                let item = &v["item"];
                let at = self.responses_call(v, item);
                let c = &mut self.calls[at];
                if let Some(id) = item["call_id"].as_str().filter(|s| !s.is_empty()).or_else(|| item["id"].as_str()) {
                    c.id = id.to_string();
                }
                if let Some(n) = item["name"].as_str().filter(|n| !n.is_empty()) {
                    c.name = n.to_string();
                }
                if let Some(a) = item["arguments"].as_str() {
                    c.args = a.to_string();
                }
            }
            "response.function_call_arguments.delta" => {
                let at = self.responses_call(v, &Value::Null);
                if let Some(d) = v["delta"].as_str() {
                    self.calls[at].args.push_str(d);
                }
            }
            "response.function_call_arguments.done" => {
                let at = self.responses_call(v, &Value::Null);
                if let Some(a) = v["arguments"].as_str().filter(|a| !a.is_empty()) {
                    self.calls[at].args = a.to_string();
                }
                if let Some(n) = v["name"].as_str().filter(|n| !n.is_empty()) {
                    self.calls[at].name = n.to_string();
                }
            }
            "response.output_item.done" => {
                let item = &v["item"];
                match item["type"].as_str() {
                    Some("function_call") => {
                        let at = self.responses_call(v, item);
                        let c = &mut self.calls[at];
                        if let Some(id) = item["call_id"].as_str().filter(|s| !s.is_empty()) {
                            c.id = id.to_string();
                        }
                        if let Some(n) = item["name"].as_str().filter(|n| !n.is_empty()) {
                            c.name = n.to_string();
                        }
                        if let Some(a) = item["arguments"].as_str().filter(|a| !a.is_empty()) {
                            c.args = a.to_string();
                        }
                    }
                    // a server that sends no text pieces gives the whole message here
                    Some("message") if !v["output_index"].as_u64().is_some_and(|i| self.streamed_items.contains(&i)) => {
                        let mut one = Reply::default();
                        parse_responses(&json!({ "output": [item] }), &mut one);
                        delta.push_str(&one.text);
                    }
                    _ => {}
                }
            }
            "response.completed" | "response.incomplete" | "response.done" => {
                let r = &v["response"];
                if !r["usage"].is_null() {
                    self.usage.merge(usage_in(Protocol::OpenAiResponses, &r["usage"]));
                }
                // nothing arrived piece by piece: the finished response has it all
                if self.text.is_empty() && self.calls.is_empty() && r["output"].is_array() {
                    let mut whole = Reply::default();
                    parse_responses(r, &mut whole);
                    delta.push_str(&whole.text);
                    self.calls.extend(whole.calls.into_iter().map(|c| PartialCall { id: c.id, name: c.name, args: c.args.to_string() }));
                }
            }
            "response.failed" => return Err(responses_failure(&v["response"])),
            "error" => return Err(v["message"].as_str().map(str::to_string).unwrap_or_else(|| error_text(v))),
            // response.created, response.in_progress, reasoning, content_part and the rest carry nothing to show
            _ => {}
        }
        self.text.push_str(&delta);
        Ok((!delta.is_empty()).then_some(delta))
    }

    /// The wire format the server actually spoke, when it was not the one the request was made for.
    pub fn foreign(&self) -> Option<Protocol> {
        self.foreign
    }

    pub fn finish(self) -> Reply {
        let calls = self.calls.into_iter().enumerate().map(|(i, c)| ToolCall {
            id: if c.id.is_empty() { format!("call_{i}") } else if c.id.starts_with('~') { format!("call_{i}{}", c.id) } else { c.id },
            name: c.name,
            args: serde_json::from_str(&c.args).ok().filter(Value::is_object).unwrap_or_else(|| json!({})),
        }).collect();
        Reply { text: self.text, calls, usage: (!self.usage.is_empty()).then_some(self.usage) }
    }
}

#[cfg(test)]
mod stream_tests {
    use super::*;

    fn run(protocol: Protocol, lines: &[&str]) -> (Vec<String>, Reply) {
        let mut acc = StreamAcc::default();
        let mut deltas = Vec::new();
        for l in lines {
            if let Some(d) = acc.feed(protocol, l).unwrap() {
                deltas.push(d);
            }
        }
        (deltas, acc.finish())
    }

    #[test]
    fn openai_text_and_split_tool_arguments() {
        let (d, r) = run(Protocol::OpenAi, &[
            r#"data: {"choices":[{"delta":{"content":"Hel"}}]}"#, "",
            r#"data: {"choices":[{"delta":{"content":"lo"}}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"zohara_shell","arguments":"{\"cmd\":"}}]}}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"ls\"}"}}]}}]}"#,
            "data: [DONE]",
        ]);
        assert_eq!(d, ["Hel", "lo"]);
        assert_eq!(r.text, "Hello");
        assert_eq!((r.calls[0].id.as_str(), r.calls[0].name.as_str(), r.calls[0].args["cmd"].as_str()), ("c1", "zohara_shell", Some("ls")));
    }

    #[test]
    fn anthropic_blocks() {
        let (d, r) = run(Protocol::Anthropic, &[
            "event: content_block_start",
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Look"}}"#,
            r#"data: {"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"t1","name":"zohara_fs_read"}}"#,
            r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}"#,
            r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"\"/etc/hostname\"}"}}"#,
        ]);
        assert_eq!(d, ["Look"]);
        assert_eq!((r.calls[0].id.as_str(), r.calls[0].args["path"].as_str()), ("t1", Some("/etc/hostname")));
    }

    #[test]
    fn gemini_chunks() {
        let (d, r) = run(Protocol::Gemini, &[
            r#"data: {"candidates":[{"content":{"parts":[{"text":"Hi "}]}}]}"#,
            r#"data: {"candidates":[{"content":{"parts":[{"text":"there"},{"functionCall":{"name":"zohara_system","args":{}}}]}}]}"#,
        ]);
        assert_eq!(d, ["Hi ", "there"]);
        assert_eq!((r.text.as_str(), r.calls[0].name.as_str(), r.calls[0].id.as_str()), ("Hi there", "zohara_system", "call_0"));
    }

    #[test]
    fn an_error_in_the_stream_is_reported_and_other_lines_are_ignored() {
        let mut acc = StreamAcc::default();
        assert_eq!(acc.feed(Protocol::OpenAi, ": keep-alive").unwrap(), None);
        assert_eq!(acc.feed(Protocol::OpenAi, "event: ping").unwrap(), None);
        assert_eq!(acc.feed(Protocol::OpenAi, r#"data: {"error":{"message":"overloaded"}}"#).unwrap_err(), "overloaded");
    }

    #[test]
    fn gemini_url_becomes_the_streaming_one() {
        let mut r = Request { url: "https://x/v1beta/models/m:generateContent".into(), headers: vec![], body: json!({}) };
        make_streaming(&mut r, Protocol::Gemini);
        assert_eq!(r.url, "https://x/v1beta/models/m:streamGenerateContent?alt=sse");
        let mut o = Request { url: "u".into(), headers: vec![], body: json!({}) };
        make_streaming(&mut o, Protocol::OpenAi);
        assert_eq!(o.body["stream"], true);
    }

    #[test]
    fn pictures_are_put_where_each_provider_expects_them() {
        let dir = std::env::temp_dir().join(format!("zo-wire-pic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.png");
        std::fs::write(&f, b"abc").unwrap();
        let img = ImageRef::from_path(&f.to_string_lossy()).unwrap();
        assert_eq!(img.mime, "image/png");
        assert_eq!(base64_std(b"abc"), "YWJj");
        assert_eq!(base64_std(b"ab"), "YWI=");
        assert!(ImageRef::from_path("/no/such/file.png").is_none() && ImageRef::from_path(&dir.join("x.txt").to_string_lossy()).is_none());
        let turns = vec![
            Turn::UserImages("look".into(), vec![img.clone()]),
            Turn::Assistant { text: String::new(), calls: vec![ToolCall { id: "t".into(), name: "zohara_ss".into(), args: json!({}) }] },
            Turn::Results(vec![ToolResult { id: "t".into(), name: "zohara_ss".into(), content: "shot".into(), is_error: false, images: vec![img] }]),
        ];
        let anth = build(crate::ai::providers::find("anthropic").unwrap(), "https://x", Some("k"), "m", "", &turns, &[]).body.to_string();
        assert!(anth.contains("\"type\":\"image\"") && anth.contains("\"media_type\":\"image/png\"") && anth.contains("\"data\":\"YWJj\""));
        let oa = build(crate::ai::providers::find("openai").unwrap(), "https://x", Some("k"), "m", "", &turns, &[]).body;
        assert!(oa["messages"].to_string().contains("data:image/png;base64,YWJj"));
        assert_eq!(oa["messages"].as_array().unwrap().last().unwrap()["role"], "user");
        let gem = build(crate::ai::providers::find("gemini").unwrap(), "https://x", Some("k"), "m", "", &turns, &[]).body;
        let contents = gem["contents"].as_array().unwrap();
        assert!(contents[0]["parts"][1]["inlineData"]["data"] == "YWJj");
        assert_eq!(contents.len(), 4, "the picture of a tool result follows its own message");
        assert!(contents[3]["parts"].to_string().contains("inlineData"));
        // a saved chat from before pictures still loads
        let old: Vec<Turn> = serde_json::from_str(r#"[{"User":"hi"},{"Results":[{"id":"1","name":"n","content":"c","is_error":false}]}]"#).unwrap();
        assert!(matches!(&old[1], Turn::Results(r) if r[0].images.is_empty()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pictures_that_came_in_the_request_are_mapped_for_every_provider_and_are_never_read_from_disk() {
        let img = |n: &str, m: &str| InlineImage { mime: m.into(), b64: "QUJD".into(), name: n.into() };
        let turns = vec![Turn::User("earlier".into()), Turn::Assistant { text: "ok".into(), calls: vec![] }, Turn::UserInline("what is this?".into(), vec![img("a.png", "image/png"), img("b.jpg", "image/jpeg"), img("evil", "text/html")])];
        // Anthropic: base64 sources, images first, then the text; a mime that is not a picture is left out
        let a = build(crate::ai::providers::find("anthropic").unwrap(), "https://x", Some("k"), "m", "", &turns, &[]).body;
        let c = &a["messages"][2]["content"];
        assert_eq!(c.as_array().unwrap().len(), 3);
        assert_eq!(c[0], json!({"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "QUJD"}}));
        assert_eq!(c[1]["source"]["media_type"], "image/jpeg");
        assert_eq!(c[2], json!({"type": "text", "text": "what is this?"}));
        // OpenAI-compatible: the text, then image_url data addresses
        let o = build(crate::ai::providers::find("openai").unwrap(), "https://x", Some("k"), "m", "", &turns, &[]).body;
        let c = &o["messages"][2]["content"];
        assert_eq!(c[0], json!({"type": "text", "text": "what is this?"}));
        assert_eq!(c[1], json!({"type": "image_url", "image_url": {"url": "data:image/png;base64,QUJD"}}));
        assert_eq!(c[2]["image_url"]["url"], "data:image/jpeg;base64,QUJD");
        assert_eq!(c.as_array().unwrap().len(), 3);
        // Gemini: inlineData parts after the text
        let g = build(crate::ai::providers::find("gemini").unwrap(), "https://x", Some("k"), "m", "", &turns, &[]).body;
        let parts = &g["contents"][2]["parts"];
        assert_eq!(parts[0], json!({"text": "what is this?"}));
        assert_eq!(parts[1], json!({"inlineData": {"mimeType": "image/png", "data": "QUJD"}}));
        assert_eq!(parts.as_array().unwrap().len(), 3);
        assert_eq!(g["contents"][2]["role"], "user");
        // a turn that came back from disk with a path-based picture is read for nobody who is restricted
        let dir = std::env::temp_dir().join(format!("zo-wire-restricted-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("secret.png");
        std::fs::write(&f, b"SECRET-BYTES").unwrap();
        let by_path = vec![Turn::UserImages("look".into(), vec![ImageRef::from_path(&f.to_string_lossy()).unwrap()])];
        assert!(build(crate::ai::providers::find("openai").unwrap(), "https://x", Some("k"), "m", "", &by_path, &[]).body.to_string().contains("data:image/png"));
        let t = crate::tenant::testkit::fresh(true);
        crate::tenant::scope(&t, || {
            let body = build(crate::ai::providers::find("openai").unwrap(), "https://x", Some("k"), "m", "", &by_path, &[]).body.to_string();
            assert!(!body.contains("image_url") && !body.contains("U0VDUkVU"), "{body}");
            // but a picture that is in the request itself is fine for them
            let body = build(crate::ai::providers::find("openai").unwrap(), "https://x", Some("k"), "m", "", &turns, &[]).body.to_string();
            assert!(body.contains("data:image/png;base64,QUJD"));
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn inline_pictures_are_dropped_from_the_conversation_after_the_answer_and_leave_their_names() {
        let img = |n: &str| InlineImage { mime: "image/png".into(), b64: "QUJD".into(), name: n.into() };
        let mut h = vec![Turn::User("a".into()), Turn::UserInline("b".into(), vec![img("x.png"), img("y.png")])];
        crate::ai::agent::forget_inline_images(&mut h);
        assert_eq!(h[1], Turn::User("b\n[image: x.png]\n[image: y.png]".into()));
        assert!(!serde_json::to_string(&h).unwrap().contains("QUJD"));
        // a request that fails because the model takes no pictures drops them too
        let mut h = vec![Turn::UserInline("b".into(), vec![img("x.png")])];
        assert!(crate::ai::agent::has_images(&h));
        crate::ai::agent::strip_images(&mut h);
        assert!(matches!(&h[0], Turn::User(t) if t.contains("1 picture(s) left out")));
        // the turn is saved and loaded like the others
        let t = Turn::UserInline("q".into(), vec![img("z.png")]);
        assert_eq!(serde_json::from_str::<Turn>(&serde_json::to_string(&t).unwrap()).unwrap(), t);
    }

    #[test]
    fn token_usage_is_read_from_each_provider_and_anthropic_is_asked_to_cache() {
        // streamed: Anthropic says the input at the start and the output at the end
        let mut acc = StreamAcc::default();
        acc.feed(Protocol::Anthropic, r#"data: {"type":"message_start","message":{"usage":{"input_tokens":12,"cache_read_input_tokens":900,"cache_creation_input_tokens":40,"output_tokens":1}}}"#).unwrap();
        acc.feed(Protocol::Anthropic, r#"data: {"type":"message_delta","usage":{"output_tokens":55}}"#).unwrap();
        let u = acc.finish().usage.unwrap();
        assert_eq!((u.input, u.cached, u.written, u.output, u.total_input()), (12, 900, 40, 55, 952));
        // OpenAI counts the cached tokens inside the prompt tokens
        let mut acc = StreamAcc::default();
        acc.feed(Protocol::OpenAi, r#"data: {"choices":[{"delta":{"content":"hi"}}],"usage":null}"#).unwrap();
        acc.feed(Protocol::OpenAi, r#"data: {"choices":[],"usage":{"prompt_tokens":1000,"completion_tokens":7,"prompt_tokens_details":{"cached_tokens":600}}}"#).unwrap();
        let u = acc.finish().usage.unwrap();
        assert_eq!((u.input, u.cached, u.output, u.total_input()), (400, 600, 7, 1000));
        // not streamed, Gemini
        let r = parse(Protocol::Gemini, &json!({ "candidates": [{ "content": { "parts": [{ "text": "x" }] } }], "usageMetadata": { "promptTokenCount": 50, "candidatesTokenCount": 3, "cachedContentTokenCount": 20 } })).unwrap();
        assert_eq!((r.usage.unwrap().total_input(), r.usage.unwrap().cached), (50, 20));
        assert!(parse(Protocol::OpenAi, &json!({ "choices": [{ "message": { "content": "x" } }] })).unwrap().usage.is_none());
        // Anthropic requests carry the top-level cache setting; streaming OpenAI asks for the usage numbers
        let req = build(crate::ai::providers::find("anthropic").unwrap(), "https://x", Some("k"), "m", "s", &[Turn::User("hi".into())], &[]);
        assert_eq!(req.body["cache_control"]["type"], "ephemeral");
        let mut oa = build(crate::ai::providers::find("openai").unwrap(), "https://x", Some("k"), "m", "s", &[Turn::User("hi".into())], &[]);
        make_streaming(&mut oa, Protocol::OpenAi);
        assert_eq!(oa.body["stream_options"]["include_usage"], true);
    }

    #[test]
    fn which_models_can_look_at_pictures() {
        use crate::ai::providers::{can_see, find};
        assert!(can_see(find("anthropic").unwrap(), "claude-sonnet-5-5"));
        assert!(can_see(find("gemini").unwrap(), "gemini-2.5-flash"));
        assert!(can_see(find("openai").unwrap(), "gpt-4o") && !can_see(find("openai").unwrap(), "gpt-3.5-turbo"));
        assert!(can_see(find("openrouter").unwrap(), "qwen/qwen2.5-vl-72b-instruct") && can_see(find("openrouter").unwrap(), "anthropic/claude-3.5-sonnet"));
        assert!(!can_see(find("openrouter").unwrap(), "cohere/north-mini-code:free") && !can_see(find("deepseek").unwrap(), "deepseek-chat"));
        assert!(!can_see(find("claude-code").unwrap(), "sonnet"));
    }
}

#[cfg(test)]
mod responses_tests {
    use super::*;
    use crate::ai::providers::find;

    fn spec(name: &str) -> ToolSpec {
        ToolSpec { name: name.into(), description: "does it".into(), schema: json!({"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}) }
    }

    fn req(turns: &[Turn], system: &str, tools: &[ToolSpec]) -> Request {
        build(find("custom:t-responses").unwrap(), "https://router.example/v1/", Some("sk-1"), "gpt-5", system, turns, tools)
    }

    // ---- request shape (golden)

    #[test]
    fn text_request_goes_to_responses_with_instructions_and_no_storage() {
        let r = req(&[Turn::User("hi".into())], "be brief", &[]);
        assert_eq!(r.url, "https://router.example/v1/responses");
        assert_eq!(r.headers, vec![("authorization".to_string(), "Bearer sk-1".to_string())]);
        assert_eq!(r.body, json!({
            "model": "gpt-5", "instructions": "be brief", "store": false,
            "input": [{"type": "message", "role": "user", "content": [{"type": "input_text", "text": "hi"}]}],
        }));
    }

    #[test]
    fn no_system_prompt_means_no_instructions_and_no_key_means_no_header() {
        let r = build(find("custom:t-responses").unwrap(), "http://127.0.0.1:1/v1", None, "m", "", &[Turn::User("x".into())], &[]);
        assert!(r.body.get("instructions").is_none() && r.body.get("tools").is_none() && r.headers.is_empty());
    }

    #[test]
    fn tool_call_and_result_are_function_call_items_joined_by_call_id() {
        let turns = vec![
            Turn::User("what is running?".into()),
            Turn::Assistant { text: "Let me look.".into(), calls: vec![ToolCall { id: "call_9".into(), name: "zohara_fs_read".into(), args: json!({"path": "/etc/hostname"}) }] },
            Turn::Results(vec![ToolResult { id: "call_9".into(), name: "zohara_fs_read".into(), content: "zo-box".into(), is_error: false, images: Vec::new() }]),
        ];
        let r = req(&turns, "s", &[spec("zohara_fs_read")]);
        assert_eq!(r.body["input"], json!([
            {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "what is running?"}]},
            {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "Let me look."}]},
            {"type": "function_call", "call_id": "call_9", "name": "zohara_fs_read", "arguments": "{\"path\":\"/etc/hostname\"}"},
            {"type": "function_call_output", "call_id": "call_9", "output": "zo-box"},
        ]));
        assert_eq!(r.body["tools"], json!([{
            "type": "function", "name": "zohara_fs_read", "description": "does it", "strict": false,
            "parameters": {"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]},
        }]));
    }

    #[test]
    fn an_assistant_turn_with_only_a_call_has_no_empty_message() {
        let turns = vec![Turn::Assistant { text: String::new(), calls: vec![ToolCall { id: "c".into(), name: "t".into(), args: json!({}) }] }];
        let r = req(&turns, "", &[]);
        assert_eq!(r.body["input"], json!([{"type": "function_call", "call_id": "c", "name": "t", "arguments": "{}"}]));
    }

    #[test]
    fn a_picture_goes_as_input_image_and_a_tool_picture_follows_as_its_own_message() {
        let png = InlineImage { mime: "image/png".into(), b64: "AAAA".into(), name: "a.png".into() };
        let bad = InlineImage { mime: "image/tiff".into(), b64: "BBBB".into(), name: "b.tif".into() };
        let r = req(&[Turn::UserInline("look".into(), vec![png, bad])], "", &[]);
        assert_eq!(r.body["input"][0]["content"], json!([
            {"type": "input_text", "text": "look"},
            {"type": "input_image", "image_url": "data:image/png;base64,AAAA"},
        ]));
        // a tool picture on disk
        let dir = std::env::temp_dir().join(format!("zo-resp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("s.png");
        std::fs::write(&f, b"png-bytes").unwrap();
        let turns = vec![Turn::Results(vec![ToolResult { id: "c1".into(), name: "zohara_ss".into(), content: "saved".into(), is_error: false, images: vec![ImageRef { path: f.to_string_lossy().into(), mime: "image/png".into() }] }])];
        let r = req(&turns, "", &[]);
        assert_eq!(r.body["input"][0], json!({"type": "function_call_output", "call_id": "c1", "output": "saved"}));
        assert_eq!(r.body["input"][1]["role"], "user");
        assert_eq!(r.body["input"][1]["content"][0]["text"], "The picture returned by zohara_ss:");
        assert!(r.body["input"][1]["content"][1]["image_url"].as_str().unwrap().starts_with("data:image/png;base64,"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_long_history_keeps_its_order() {
        let mut turns = Vec::new();
        for i in 0..30 {
            turns.push(Turn::User(format!("q{i}")));
            turns.push(Turn::Assistant { text: format!("a{i}"), calls: Vec::new() });
        }
        let r = req(&turns, "s", &[]);
        let input = r.body["input"].as_array().unwrap();
        assert_eq!(input.len(), 60);
        assert_eq!((input[0]["content"][0]["text"].clone(), input[59]["content"][0]["text"].clone()), (json!("q0"), json!("a29")));
        assert!(input.iter().step_by(2).all(|m| m["role"] == "user") && input.iter().skip(1).step_by(2).all(|m| m["role"] == "assistant"));
    }

    #[test]
    fn streaming_adds_stream_true_and_nothing_else() {
        let mut r = req(&[Turn::User("x".into())], "", &[]);
        make_streaming(&mut r, Protocol::OpenAiResponses);
        assert_eq!(r.body["stream"], true);
        assert!(r.body.get("stream_options").is_none());
    }

    #[test]
    fn the_claude_format_of_a_custom_endpoint_sends_the_key_both_ways_and_no_cache_field() {
        let r = build(find("custom:t-claude").unwrap(), "https://router.example/v1", Some("k"), "m", "s", &[Turn::User("hi".into())], &[]);
        assert_eq!(r.url, "https://router.example/v1/messages");
        assert!(r.headers.contains(&("x-api-key".to_string(), "k".to_string())) && r.headers.contains(&("authorization".to_string(), "Bearer k".to_string())));
        assert!(r.body.get("cache_control").is_none());
        // Anthropic itself is unchanged
        let a = build(find("anthropic").unwrap(), "https://api.anthropic.com/v1", Some("k"), "m", "s", &[Turn::User("hi".into())], &[]);
        assert_eq!(a.body["cache_control"]["type"], "ephemeral");
        assert!(!a.headers.iter().any(|(k, _)| k == "authorization"));
    }

    #[test]
    fn the_chat_format_of_a_custom_endpoint_is_what_it_was() {
        let r = build(find("custom:t-chat").unwrap(), "https://x/v1", Some("k"), "m", "s", &[Turn::User("hi".into())], &[]);
        assert_eq!(r.url, "https://x/v1/chat/completions");
        assert_eq!(r.body["messages"][0], json!({"role": "system", "content": "s"}));
    }

    // ---- parsing recorded and router-style bodies

    /// OpenAI's documented shape for a text answer.
    fn openai_text() -> Value {
        json!({
            "id": "resp_67ccd2bed1ec8190b14f964abc0542670bb6a6b452d3795b", "object": "response", "created_at": 1741476542, "status": "completed",
            "error": null, "incomplete_details": null, "instructions": null, "model": "gpt-4o-2024-08-06",
            "output": [
                {"type": "reasoning", "id": "rs_1", "summary": []},
                {"type": "message", "id": "msg_67ccd2bf17f0819081ff3bb2cf6508e6", "status": "completed", "role": "assistant",
                 "content": [{"type": "output_text", "text": "In a peaceful grove, a unicorn slept.", "annotations": []}]}
            ],
            "usage": {"input_tokens": 36, "input_tokens_details": {"cached_tokens": 0}, "output_tokens": 87, "output_tokens_details": {"reasoning_tokens": 0}, "total_tokens": 123},
        })
    }

    #[test]
    fn openai_documented_text_answer_parses_and_a_null_error_is_not_an_error() {
        let r = parse(Protocol::OpenAiResponses, &openai_text()).unwrap();
        assert_eq!(r.text, "In a peaceful grove, a unicorn slept.");
        assert!(r.calls.is_empty());
        assert_eq!(r.usage, Some(Usage { input: 36, output: 87, cached: 0, written: 0 }));
    }

    #[test]
    fn function_calls_come_out_with_call_id_name_and_arguments_from_a_json_string() {
        let v = json!({"object": "response", "status": "completed", "output": [
            {"type": "function_call", "id": "fc_1", "call_id": "call_abc", "name": "zohara_fs_read", "arguments": "{\"path\":\"/etc/hostname\"}", "status": "completed"},
            {"type": "function_call", "id": "fc_2", "call_id": "call_def", "name": "zohara_system", "arguments": ""},
        ]});
        let r = parse(Protocol::OpenAiResponses, &v).unwrap();
        assert_eq!(r.calls, vec![
            ToolCall { id: "call_abc".into(), name: "zohara_fs_read".into(), args: json!({"path": "/etc/hostname"}) },
            ToolCall { id: "call_def".into(), name: "zohara_system".into(), args: json!({}) },
        ]);
        assert_eq!(r.text, "");
    }

    #[test]
    fn router_variations_are_tolerated() {
        // output_text as a plain string and no output list at all
        let r = parse(Protocol::OpenAiResponses, &json!({"id": "x", "output_text": "hello"})).unwrap();
        assert_eq!(r.text, "hello");
        // message content as a plain string, null fields, missing usage, arguments already an object, no call_id (the item id is used)
        let v = json!({"object": "response", "status": null, "error": null, "usage": null, "output": [
            {"type": "message", "role": "assistant", "content": "plain"},
            {"type": "message", "role": "assistant", "content": null},
            {"type": "function_call", "id": "fc_77", "call_id": null, "name": "t", "arguments": {"a": 1}},
            {"type": "function_call", "name": "u", "arguments": null},
            {"type": "web_search_call", "id": "ws_1"},
        ]});
        let r = parse(Protocol::OpenAiResponses, &v).unwrap();
        assert_eq!(r.text, "plain");
        assert_eq!(r.calls, vec![
            ToolCall { id: "fc_77".into(), name: "t".into(), args: json!({"a": 1}) },
            ToolCall { id: "call_1".into(), name: "u".into(), args: json!({}) },
        ]);
        assert!(r.usage.is_none());
        // several parts, a refusal, a part of kind "text"
        let v = json!({"output": [{"type": "message", "content": [{"type": "output_text", "text": "a"}, {"type": "text", "text": "b"}, {"type": "refusal", "refusal": " no"}]}]});
        assert_eq!(parse(Protocol::OpenAiResponses, &v).unwrap().text, "ab no");
    }

    #[test]
    fn usage_with_cached_tokens_and_with_chat_style_names() {
        let v = json!({"output": [], "usage": {"input_tokens": 1000, "input_tokens_details": {"cached_tokens": 600}, "output_tokens": 7}});
        let u = parse(Protocol::OpenAiResponses, &v).unwrap().usage.unwrap();
        assert_eq!((u.input, u.cached, u.output, u.total_input()), (400, 600, 7, 1000));
        let v = json!({"output": [], "usage": {"prompt_tokens": 50, "completion_tokens": 3}});
        let u = parse(Protocol::OpenAiResponses, &v).unwrap().usage.unwrap();
        assert_eq!((u.input, u.output), (50, 3));
    }

    #[test]
    fn errors_failed_and_incomplete_answers() {
        assert_eq!(parse(Protocol::OpenAiResponses, &json!({"error": {"message": "bad key", "type": "invalid_request_error"}})).unwrap_err(), "bad key");
        let failed = json!({"object": "response", "status": "failed", "error": {"code": "server_error", "message": "The model overloaded"}, "output": []});
        assert_eq!(parse(Protocol::OpenAiResponses, &failed).unwrap_err(), "The model overloaded");
        let cancelled = json!({"object": "response", "status": "cancelled", "output": []});
        assert!(parse(Protocol::OpenAiResponses, &cancelled).unwrap_err().contains("cancelled"));
        // incomplete with nothing written: the reason; with text written: the text is kept
        let nothing = json!({"object": "response", "status": "incomplete", "incomplete_details": {"reason": "max_output_tokens"}, "output": []});
        assert!(parse(Protocol::OpenAiResponses, &nothing).unwrap_err().contains("max_output_tokens"));
        let some = json!({"object": "response", "status": "incomplete", "incomplete_details": {"reason": "max_output_tokens"}, "output": [{"type": "message", "content": [{"type": "output_text", "text": "half an ans"}]}]});
        assert_eq!(parse(Protocol::OpenAiResponses, &some).unwrap().text, "half an ans");
        assert!(parse(Protocol::OpenAiResponses, &json!({})).is_err());
    }

    #[test]
    fn a_server_in_another_format_is_named() {
        let chat = json!({"choices": [{"message": {"content": "hi"}}]});
        let responses = openai_text();
        let claude = json!({"type": "message", "role": "assistant", "content": [{"type": "text", "text": "hi"}], "stop_reason": "end_turn"});
        let e = parse(Protocol::OpenAi, &responses).unwrap_err();
        assert!(e.starts_with("This server speaks the OpenAI Responses format") && e.contains("set the API format of this provider to OpenAI Responses"), "{e}");
        let e = parse(Protocol::OpenAi, &claude).unwrap_err();
        assert!(e.contains("Claude format"), "{e}");
        let e = parse(Protocol::OpenAiResponses, &chat).unwrap_err();
        assert!(e.contains("OpenAI Chat"), "{e}");
        let e = parse(Protocol::Anthropic, &responses).unwrap_err();
        assert!(e.contains("OpenAI Responses"), "{e}");
        let e = parse(Protocol::OpenAiResponses, &claude).unwrap_err();
        assert!(e.contains("Claude format"), "{e}");
        // the right format still parses
        assert!(parse(Protocol::OpenAi, &chat).is_ok() && parse(Protocol::Anthropic, &claude).is_ok());
    }

    #[test]
    fn errors_that_name_another_endpoint_name_that_format() {
        assert_eq!(format_in_error("The provider answered 400: Use the /v1/responses endpoint for this model", Protocol::OpenAi), Some(Protocol::OpenAiResponses));
        assert_eq!(format_in_error("This model is only supported in v1/responses", Protocol::OpenAi), Some(Protocol::OpenAiResponses));
        assert_eq!(format_in_error("use /v1/messages", Protocol::OpenAi), Some(Protocol::Anthropic));
        assert_eq!(format_in_error("not supported on /responses, use /chat/completions", Protocol::OpenAiResponses), None); // names both: no guess
        assert_eq!(format_in_error("only /chat/completions", Protocol::OpenAiResponses), Some(Protocol::OpenAi));
        assert_eq!(format_in_error("The provider answered 401: bad key", Protocol::OpenAi), None);
    }

    #[test]
    fn model_lists_in_router_shapes() {
        assert_eq!(parse_models(Protocol::OpenAiResponses, &json!({"object": "list", "data": [{"id": "b"}, {"id": "a"}]})), ["a", "b"]);
        assert_eq!(parse_models(Protocol::OpenAi, &json!({"models": [{"name": "x"}, {"id": "w"}]})), ["w", "x"]);
        assert_eq!(parse_models(Protocol::OpenAi, &json!(["m2", "m1"])), ["m1", "m2"]);
    }

    // ---- streaming

    fn run(lines: &[&str]) -> (Vec<String>, Reply) {
        let mut acc = StreamAcc::default();
        let mut deltas = Vec::new();
        for l in lines {
            if let Some(d) = acc.feed(Protocol::OpenAiResponses, l).unwrap() {
                deltas.push(d);
            }
        }
        (deltas, acc.finish())
    }

    #[test]
    fn a_text_stream_is_rebuilt_from_deltas() {
        let (d, r) = run(&[
            r#"event: response.created"#,
            r#"data: {"type":"response.created","response":{"id":"resp_1","status":"in_progress","output":[]}}"#,
            r#"data: {"type":"response.output_item.added","output_index":0,"item":{"type":"message","id":"msg_1","role":"assistant","content":[]}}"#,
            r#"data: {"type":"response.content_part.added","output_index":0,"content_index":0,"part":{"type":"output_text","text":""}}"#,
            r#"data: {"type":"response.output_text.delta","output_index":0,"content_index":0,"delta":"Hel"}"#,
            r#"data: {"type":"response.output_text.delta","output_index":0,"content_index":0,"delta":"lo"}"#,
            r#"data: {"type":"response.output_text.done","output_index":0,"text":"Hello"}"#,
            r#"data: {"type":"response.output_item.done","output_index":0,"item":{"type":"message","id":"msg_1","content":[{"type":"output_text","text":"Hello"}]}}"#,
            r#"data: {"type":"response.completed","response":{"status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":"Hello"}]}],"usage":{"input_tokens":10,"output_tokens":2,"input_tokens_details":{"cached_tokens":4}}}}"#,
            "data: [DONE]",
        ]);
        assert_eq!(d, ["Hel", "lo"]);
        assert_eq!(r.text, "Hello", "the finished message is not added a second time");
        assert_eq!(r.usage, Some(Usage { input: 6, output: 2, cached: 4, written: 0 }));
    }

    #[test]
    fn a_tool_call_stream_collects_argument_fragments() {
        let (d, r) = run(&[
            r#"data: {"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"zohara_fs_read","arguments":""}}"#,
            r#"data: {"type":"response.function_call_arguments.delta","item_id":"fc_1","output_index":0,"delta":"{\"pa"}"#,
            r#"data: {"type":"response.function_call_arguments.delta","item_id":"fc_1","output_index":0,"delta":"th\":\"/etc/hostname\"}"}"#,
            r#"data: {"type":"response.function_call_arguments.done","item_id":"fc_1","output_index":0,"arguments":"{\"path\":\"/etc/hostname\"}"}"#,
            r#"data: {"type":"response.output_item.done","output_index":0,"item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"zohara_fs_read","arguments":"{\"path\":\"/etc/hostname\"}"}}"#,
            r#"data: {"type":"response.output_item.added","output_index":1,"item":{"type":"function_call","id":"fc_2","call_id":"call_2","name":"zohara_system"}}"#,
            r#"data: {"type":"response.output_item.done","output_index":1,"item":{"type":"function_call","id":"fc_2","call_id":"call_2","name":"zohara_system","arguments":"{}"}}"#,
            r#"data: {"type":"response.completed","response":{"status":"completed","output":[]}}"#,
        ]);
        assert!(d.is_empty());
        assert_eq!(r.calls, vec![
            ToolCall { id: "call_1".into(), name: "zohara_fs_read".into(), args: json!({"path": "/etc/hostname"}) },
            ToolCall { id: "call_2".into(), name: "zohara_system".into(), args: json!({}) },
        ]);
    }

    #[test]
    fn a_stream_without_item_added_events_still_builds_the_call() {
        let (_, r) = run(&[
            r#"data: {"type":"response.function_call_arguments.delta","item_id":"fc_9","delta":"{\"a\":"}"#,
            r#"data: {"type":"response.function_call_arguments.delta","item_id":"fc_9","delta":"1}"}"#,
            r#"data: {"type":"response.output_item.done","item":{"type":"function_call","id":"fc_9","call_id":"call_9","name":"t"}}"#,
        ]);
        assert_eq!(r.calls, vec![ToolCall { id: "call_9".into(), name: "t".into(), args: json!({"a": 1}) }]);
    }

    #[test]
    fn a_server_that_only_sends_the_finished_response_still_gives_the_answer() {
        let (d, r) = run(&[
            r#"data: {"type":"response.completed","response":{"status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":"all at once"}]},{"type":"function_call","call_id":"c1","name":"t","arguments":"{\"x\":2}"}],"usage":{"input_tokens":5,"output_tokens":1}}}"#,
        ]);
        assert_eq!(d, ["all at once"]);
        assert_eq!((r.text.as_str(), r.calls.len(), r.usage.unwrap().output), ("all at once", 1, 1));
        assert_eq!(r.calls[0].args, json!({"x": 2}));
    }

    #[test]
    fn a_message_without_text_pieces_comes_from_its_done_event() {
        let (d, r) = run(&[r#"data: {"type":"response.output_item.done","output_index":0,"item":{"type":"message","content":[{"type":"output_text","text":"whole"}]}}"#]);
        assert_eq!((d, r.text), (vec!["whole".to_string()], "whole".to_string()));
    }

    #[test]
    fn sse_quirks_are_tolerated() {
        let mut acc = StreamAcc::default();
        let p = Protocol::OpenAiResponses;
        // no space after "data:", comments, event lines, blank lines, a ping that is not JSON, [DONE] with and without a space
        assert_eq!(acc.feed(p, r#"data:{"type":"response.output_text.delta","delta":"a"}"#).unwrap(), Some("a".into()));
        for l in [": keep-alive", "event: ping", "", "data:", "data: ping", "data:[DONE]", "data: [DONE]", ": OPENROUTER PROCESSING", "retry: 100", "id: 7"] {
            assert_eq!(acc.feed(p, l).unwrap(), None, "{l:?}");
        }
        assert_eq!(acc.feed(p, "data: {\"type\":\"response.output_text.delta\",\"delta\":\"b\"}\r").unwrap(), Some("b".into()), "a carriage return at the end");
        assert_eq!(acc.finish().text, "ab");
    }

    #[test]
    fn chunks_split_in_the_middle_of_a_line_are_put_together() {
        let whole = "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"Hello\"}\n\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\" world\"}\n\ndata: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":3,\"output_tokens\":2}}}\n\ndata: [DONE]\n\n";
        for size in [1, 2, 3, 7, 13, 40, whole.len()] {
            let mut acc = StreamAcc::default();
            let mut got = String::new();
            let chars: Vec<char> = whole.chars().collect();
            for c in chars.chunks(size) {
                let s: String = c.iter().collect();
                if let Some(d) = acc.push(Protocol::OpenAiResponses, &s).unwrap() {
                    got.push_str(&d);
                }
            }
            assert_eq!(got, "Hello world", "chunk size {size}");
            let r = acc.finish();
            assert_eq!((r.text.as_str(), r.usage.map(|u| u.output)), ("Hello world", Some(2)), "chunk size {size}");
        }
        // the last line without its newline
        let mut acc = StreamAcc::default();
        assert_eq!(acc.push(Protocol::OpenAiResponses, "data: {\"type\":\"response.output_text.delta\",\"delta\":\"x\"}").unwrap(), None);
        assert_eq!(acc.flush(Protocol::OpenAiResponses).unwrap(), Some("x".into()));
    }

    #[test]
    fn failed_and_error_events_are_errors() {
        let mut acc = StreamAcc::default();
        let e = acc.feed(Protocol::OpenAiResponses, r#"data: {"type":"response.failed","response":{"status":"failed","error":{"code":"server_error","message":"overloaded"}}}"#).unwrap_err();
        assert_eq!(e, "overloaded");
        let e = acc.feed(Protocol::OpenAiResponses, r#"data: {"type":"error","code":"rate_limit_exceeded","message":"Slow down"}"#).unwrap_err();
        assert_eq!(e, "Slow down");
        let e = acc.feed(Protocol::OpenAiResponses, r#"data: {"error":{"message":"bad gateway"}}"#).unwrap_err();
        assert_eq!(e, "bad gateway");
        // an incomplete response keeps its text
        let (_, r) = run(&[
            r#"data: {"type":"response.output_text.delta","output_index":0,"delta":"cut o"}"#,
            r#"data: {"type":"response.incomplete","response":{"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"},"usage":{"input_tokens":4,"output_tokens":9}}}"#,
        ]);
        assert_eq!((r.text.as_str(), r.usage.unwrap().output), ("cut o", 9));
        // JSON that is truly broken still says so
        assert!(StreamAcc::default().feed(Protocol::OpenAiResponses, r#"data: {"type": "#).is_err());
    }

    #[test]
    fn chat_streams_keep_their_quirks_too() {
        let mut acc = StreamAcc::default();
        let p = Protocol::OpenAi;
        acc.feed(p, r#"data:{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"t","arguments":"{\"a\""}}]}}]}"#).unwrap();
        acc.feed(p, r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":":1}"}}]}}]}"#).unwrap();
        acc.feed(p, ": ping").unwrap();
        acc.feed(p, r#"data: {"choices":[],"usage":{"prompt_tokens":9,"completion_tokens":2}}"#).unwrap();
        acc.feed(p, "data:[DONE]").unwrap();
        let r = acc.finish();
        assert_eq!(r.calls, vec![ToolCall { id: "c1".into(), name: "t".into(), args: json!({"a": 1}) }]);
        assert_eq!(r.usage, Some(Usage { input: 9, output: 2, cached: 0, written: 0 }));
    }

    #[test]
    fn a_stream_in_another_format_is_noticed() {
        let mut acc = StreamAcc::default();
        acc.feed(Protocol::OpenAi, r#"data: {"type":"response.created","response":{}}"#).unwrap();
        acc.feed(Protocol::OpenAi, r#"data: {"type":"response.output_text.delta","delta":"hi"}"#).unwrap();
        assert_eq!(acc.foreign(), Some(Protocol::OpenAiResponses));
        let mut acc = StreamAcc::default();
        acc.feed(Protocol::OpenAiResponses, r#"data: {"choices":[{"delta":{"content":"hi"}}]}"#).unwrap();
        assert_eq!(acc.foreign(), Some(Protocol::OpenAi));
        let mut acc = StreamAcc::default();
        acc.feed(Protocol::OpenAi, r#"data: {"type":"message_start","message":{}}"#).unwrap();
        assert_eq!(acc.foreign(), Some(Protocol::Anthropic));
        // the right format sets nothing
        let mut acc = StreamAcc::default();
        acc.feed(Protocol::OpenAiResponses, r#"data: {"type":"response.created"}"#).unwrap();
        acc.feed(Protocol::Anthropic, r#"data: {"type":"message_start","message":{}}"#).unwrap();
        assert_eq!(acc.foreign(), None);
    }
}
