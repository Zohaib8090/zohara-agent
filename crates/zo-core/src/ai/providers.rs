//! The providers Zohara Agent can talk to. A few wire protocols cover all of them; most are the OpenAI-compatible one (chat completions).

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Protocol {
    /// Anthropic Messages API (Claude).
    Anthropic,
    /// OpenAI chat completions (`/chat/completions`), also spoken by most hosts and every local server.
    OpenAi,
    /// OpenAI Responses (`/responses`): what some agent routers and newer OpenAI models speak.
    OpenAiResponses,
    /// Google Gemini generateContent.
    Gemini,
    /// Not a web protocol: the Claude Code program the person installed and signed in to.
    ClaudeCode,
}

#[derive(Clone)]
pub struct Provider {
    pub id: &'static str,
    pub name: &'static str,
    pub protocol: Protocol,
    /// Default base URL; the person can change it in the settings.
    pub base_url: &'static str,
    /// Runs on this computer: no key, works offline.
    pub local: bool,
    /// Suggestions shown before the live model list is fetched.
    pub models: &'static [&'static str],
}

const fn p(id: &'static str, name: &'static str, protocol: Protocol, base_url: &'static str, models: &'static [&'static str]) -> Provider {
    Provider { id, name, protocol, base_url, local: false, models }
}

const fn local(id: &'static str, name: &'static str, base_url: &'static str) -> Provider {
    Provider { id, name, protocol: Protocol::OpenAi, base_url, local: true, models: &[] }
}

use Protocol::*;

static ALL: &[Provider] = &[
    p("anthropic", "Anthropic (Claude)", Anthropic, "https://api.anthropic.com/v1",
        &["claude-sonnet-5-5", "claude-opus-5-5", "claude-haiku-5-5", "claude-fable-5-1"]),
    p("openai", "OpenAI", OpenAi, "https://api.openai.com/v1", &[]),
    p("gemini", "Google Gemini", Gemini, "https://generativelanguage.googleapis.com/v1beta", &[]),
    p("openrouter", "OpenRouter", OpenAi, "https://openrouter.ai/api/v1", &[]),
    p("deepseek", "DeepSeek", OpenAi, "https://api.deepseek.com/v1", &[]),
    p("fireworks", "Fireworks AI", OpenAi, "https://api.fireworks.ai/inference/v1", &[]),
    p("groq", "Groq", OpenAi, "https://api.groq.com/openai/v1", &[]),
    p("mistral", "Mistral", OpenAi, "https://api.mistral.ai/v1", &[]),
    p("together", "Together AI", OpenAi, "https://api.together.xyz/v1", &[]),
    p("cerebras", "Cerebras", OpenAi, "https://api.cerebras.ai/v1", &[]),
    p("perplexity", "Perplexity", OpenAi, "https://api.perplexity.ai/v1", &[]),
    p("novita", "NovitaAI", OpenAi, "https://api.novita.ai/openai/v1", &[]),
    p("gmi", "GMI Cloud", OpenAi, "https://api.gmi-serving.com/v1", &[]),
    p("stepfun", "StepFun", OpenAi, "https://api.stepfun.com/v1", &[]),
    p("kimi", "Kimi / Moonshot", OpenAi, "https://api.moonshot.ai/v1", &[]),
    p("zai", "z.ai / GLM", OpenAi, "https://api.z.ai/api/paas/v4", &[]),
    p("qwen", "Qwen (Alibaba DashScope)", OpenAi, "https://dashscope-intl.aliyuncs.com/compatible-mode/v1", &[]),
    p("minimax", "MiniMax", OpenAi, "https://api.minimax.io/v1", &[]),
    p("nvidia", "NVIDIA Build", OpenAi, "https://integrate.api.nvidia.com/v1", &[]),
    p("huggingface", "Hugging Face", OpenAi, "https://router.huggingface.co/v1", &[]),
    local("ollama", "Ollama (this computer)", "http://127.0.0.1:11434/v1"),
    local("lmstudio", "LM Studio (this computer)", "http://127.0.0.1:1234/v1"),
    local("llamacpp", "llama.cpp (this computer)", "http://127.0.0.1:8080/v1"),
    local("vllm", "vLLM (this computer)", "http://127.0.0.1:8000/v1"),
    local("sglang", "SGLang (this computer)", "http://127.0.0.1:30000/v1"),
    Provider { id: "claude-code", name: "Claude Code (your subscription, experimental)", protocol: ClaudeCode, base_url: "", local: true, models: &["sonnet", "opus", "haiku"] },
];

/// The wire formats a person can pick by name (the "API format" of a provider): `chat`, `responses` and `claude`.
pub fn format_protocol(name: &str) -> Option<Protocol> {
    match name.trim().to_ascii_lowercase().as_str() {
        "chat" | "openai" | "chat_completions" => Some(OpenAi),
        "responses" | "openai_responses" => Some(OpenAiResponses),
        "claude" | "messages" | "anthropic" => Some(Anthropic),
        _ => None,
    }
}

/// The spelling stored and shown for a format name (`None` for anything that is not one).
pub fn format_name(name: &str) -> Option<&'static str> {
    Some(match format_protocol(name)? {
        OpenAi => "chat",
        OpenAiResponses => "responses",
        _ => "claude",
    })
}

/// What the person has to pick for a wire format, in a sentence ("set the API format to OpenAI Responses").
pub fn format_label(protocol: Protocol) -> &'static str {
    match protocol {
        OpenAiResponses => "OpenAI Responses",
        Anthropic => "Claude format",
        _ => "OpenAI Chat",
    }
}

/// The same provider speaking another wire format (the "API format" of a compatible host). Variants are made once and live as long as
/// the program; the id, address and key stay the provider's own.
fn variants() -> &'static [Provider] {
    static V: std::sync::OnceLock<Vec<Provider>> = std::sync::OnceLock::new();
    V.get_or_init(|| {
        let mut out = Vec::new();
        for p in ALL.iter().filter(|p| p.protocol == OpenAi && !p.local && !p.is_custom()) {
            for proto in [OpenAiResponses, Anthropic] {
                out.push(Provider { protocol: proto, ..p.clone() });
            }
        }
        out
    })
}

/// The built-in providers. The named custom providers the person added (see `custom`) are not in it: `listed` has them too.
pub fn all() -> &'static [Provider] {
    ALL
}

/// Every provider the person can pick now: the built-in ones, then the custom providers they named.
pub fn listed() -> Vec<&'static Provider> {
    ALL.iter().chain(super::custom::providers()).collect()
}

/// A provider made at run time (a named custom one). It lives as long as the program; the same one is returned for the same
/// content, so a provider that is edited leaves one small leftover and nothing grows while it stays as it is.
pub(crate) fn leak(p: Provider) -> &'static Provider {
    static CACHE: std::sync::Mutex<Vec<&'static Provider>> = std::sync::Mutex::new(Vec::new());
    let mut c = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(f) = c.iter().find(|q| q.id == p.id && q.name == p.name && q.protocol == p.protocol && q.base_url == p.base_url && q.models == p.models) {
        return f;
    }
    let l: &'static Provider = Box::leak(Box::new(p));
    c.push(l);
    l
}

pub(crate) fn leak_str(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

/// The host part of an http(s) address, lowercase, without user name or port (`""` when there is none).
fn host_of(url: &str) -> String {
    let rest = url.trim().split("://").nth(1).unwrap_or("");
    let auth = rest.split(['/', '?', '#']).next().unwrap_or("");
    let auth = auth.rsplit('@').next().unwrap_or("");
    let host = if auth.starts_with('[') { auth.split(']').next().unwrap_or("") } else { auth.split(':').next().unwrap_or("") };
    host.to_ascii_lowercase()
}

impl Provider {
    /// A custom provider the person named (`custom:<slug>`): any address and API format, with a key typed for that address.
    pub fn is_custom(&self) -> bool {
        self.id.starts_with("custom:")
    }

    /// May the person add several named accounts (API keys) for it? Every provider with a key can; a local server and Claude Code have none.
    pub fn takes_accounts(&self) -> bool {
        !self.local && self.protocol != Protocol::ClaudeCode
    }

    /// This provider speaking `format` (a name from `format_protocol`), when it is a host that can: a hosted provider of the chat kind.
    /// A custom endpoint's format is its own id and a local server only speaks chat, so those, and a name nobody knows, give `self`.
    pub fn with_format(&'static self, format: Option<&str>) -> &'static Provider {
        let Some(proto) = format.and_then(format_protocol) else { return self };
        if proto == self.protocol {
            return self;
        }
        variants().iter().find(|v| v.id == self.id && v.protocol == proto).unwrap_or(self)
    }

    /// The provider that speaks `protocol` at the same kind of address: for a custom endpoint the sibling with that format, for a
    /// hosted compatible provider the same one with its API format changed. `None` when there is none.
    pub fn sibling(&'static self, protocol: Protocol) -> Option<&'static Provider> {
        if self.protocol == protocol {
            return Some(self);
        }
        if self.is_custom() {
            // the same custom provider speaking another format (used to say which format fits when a test fails)
            return Some(leak(Provider { protocol, ..self.clone() }));
        }
        variants().iter().find(|v| v.id == self.id && v.protocol == protocol)
    }

    /// May the person pick a wire format for it (the "API format" hint)? Custom endpoints are the format already.
    pub fn takes_format(&self) -> bool {
        !self.is_custom() && !self.local && ALL.iter().any(|p| p.id == self.id && p.protocol == OpenAi)
    }

    /// May this provider's API key be sent to `base`? Only to the provider's own host: a different address is for a Custom endpoint,
    /// which has a key of its own. A local server and a custom endpoint may be anywhere (their key is typed for that address).
    pub fn key_fits(&self, base: &str) -> bool {
        let base = base.trim();
        base.is_empty() || self.local || self.is_custom() || self.protocol == Protocol::ClaudeCode || (!host_of(base).is_empty() && host_of(base) == host_of(self.base_url))
    }
}

pub fn find(id: &str) -> Option<&'static Provider> {
    ALL.iter().find(|p| p.id == id).or_else(|| {
        #[cfg(test)]
        if let Some(f) = id.strip_prefix("custom:t-").filter(|f| format_protocol(f).is_some()) {
            return Some(test_custom(f));
        }
        if id.starts_with("custom:") { super::custom::resolve(id) } else { None }
    })
}

/// A custom provider for tests that need one without saving anything: `custom:t-chat`, `custom:t-responses`, `custom:t-claude`.
#[cfg(test)]
pub(crate) fn test_custom(format: &str) -> &'static Provider {
    let protocol = format_protocol(format).expect("a format");
    leak(Provider { id: leak_str(&format!("custom:t-{format}")), name: "Test endpoint", protocol, base_url: "http://127.0.0.1:8000/v1", local: false, models: &[] })
}

/// An address a person typed for a provider: empty (the provider's default), or an http(s) address; a restricted account only a
/// public https one. The key is only sent to the provider's own host (see `key_fits`).
pub fn check_base_url(p: &Provider, u: &str) -> Result<(), String> {
    let u = u.trim();
    if u.is_empty() || p.protocol == Protocol::ClaudeCode {
        return Ok(());
    }
    super::probe::check_address(u)?;
    if !p.key_fits(u) {
        return Err(format!("{} can only be used at its own address (its key would be sent there). For another address add a custom provider.", p.name));
    }
    if crate::tenant::is_restricted() {
        super::agent::check_restricted_provider(p, u)?;
    }
    Ok(())
}

/// Can this model look at pictures? Anthropic and Gemini models can; OpenAI's, apart from a few old and small ones. For the many
/// other hosts it depends on the model, so its name decides. A model that is not recognised is treated as text-only, and the
/// person can say otherwise in the model settings (`Selection::vision`).
pub fn can_see(p: &Provider, model: &str) -> bool {
    let m = model.to_lowercase();
    match p.id {
        "claude-code" => false,
        "anthropic" => true,
        "gemini" => !(m.contains("embedding") || m.contains("aqa")),
        "openai" => !["gpt-3.5", "o1-mini", "o3-mini", "gpt-oss", "embedding", "whisper", "tts", "davinci", "babbage", "moderation", "instruct"].iter().any(|bad| m.contains(bad)),
        _ => [
            "vision", "-vl", "vl-", "_vl", "gpt-4o", "gpt-4.1", "gpt-4.5", "gpt-5", "claude", "gemini", "gemma-3", "gemma3", "llama-3.2", "llama3.2",
            "llama-4", "llama4", "pixtral", "llava", "minicpm-v", "internvl", "glm-4v", "glm-4.5v", "glm-4.6v", "kimi-vl", "step-1v", "step-3",
            "mistral-small-3", "mistral-medium", "grok-4", "nova-", "phi-4-multimodal", "molmo", "omni", "multimodal",
        ].iter().any(|k| m.contains(k)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_providers_are_named_and_not_in_the_built_in_list() {
        let c = test_custom("chat");
        let r = test_custom("responses");
        let m = test_custom("claude");
        assert_eq!((c.protocol, r.protocol, m.protocol), (Protocol::OpenAi, Protocol::OpenAiResponses, Protocol::Anthropic));
        assert!([c, r, m].iter().all(|p| p.is_custom() && !p.local && p.models.is_empty() && !p.takes_format() && p.takes_accounts()));
        assert_eq!(all().iter().filter(|p| p.is_custom()).count(), 0, "the built-in list has no custom provider: the person adds them");
        assert!(!all().iter().any(|p| p.id == "custom" || p.id.starts_with("custom-")));
        assert!(!find("openai").unwrap().is_custom() && !find("ollama").unwrap().is_custom() && !find("anthropic").unwrap().is_custom());
        assert!(find("custom").is_none() && find("custom:nope").is_none());
    }

    #[test]
    fn who_can_have_accounts() {
        assert!(find("groq").unwrap().takes_accounts() && find("openai").unwrap().takes_accounts() && find("anthropic").unwrap().takes_accounts() && find("gemini").unwrap().takes_accounts());
        assert!(!find("ollama").unwrap().takes_accounts() && !find("claude-code").unwrap().takes_accounts());
    }

    #[test]
    fn a_custom_endpoint_key_goes_to_any_address_and_the_others_only_to_their_own_host() {
        for id in ["custom:t-chat", "custom:t-responses", "custom:t-claude"] {
            assert!(find(id).unwrap().key_fits("https://router.example/v1"), "{id}");
        }
        assert!(find("openai").unwrap().key_fits("https://api.openai.com/v2") && !find("openai").unwrap().key_fits("https://router.example/v1"));
        assert!(!find("anthropic").unwrap().key_fits("https://router.example/v1"));
    }

    #[test]
    fn api_format_names() {
        assert_eq!(format_name("Responses"), Some("responses"));
        assert_eq!(format_name(" claude "), Some("claude"));
        assert_eq!(format_name("messages"), Some("claude"));
        assert_eq!(format_name("chat"), Some("chat"));
        assert_eq!(format_name("grpc"), None);
        assert_eq!(format_label(Protocol::OpenAiResponses), "OpenAI Responses");
    }

    #[test]
    fn a_hosted_provider_can_be_told_its_api_format_and_keeps_its_id_and_address() {
        let o = find("openai").unwrap();
        assert!(o.takes_format() && find("openrouter").unwrap().takes_format());
        assert!(!find("ollama").unwrap().takes_format() && !find("anthropic").unwrap().takes_format() && !find("gemini").unwrap().takes_format() && !find("claude-code").unwrap().takes_format());
        let r = o.with_format(Some("responses"));
        assert_eq!((r.id, r.protocol, r.base_url, r.name), ("openai", Protocol::OpenAiResponses, o.base_url, o.name));
        assert_eq!(o.with_format(Some("claude")).protocol, Protocol::Anthropic);
        assert!(std::ptr::eq(o.with_format(Some("chat")), o) && std::ptr::eq(o.with_format(None), o) && std::ptr::eq(o.with_format(Some("nonsense")), o));
        // the key still only fits the provider's own host
        assert!(r.key_fits("https://api.openai.com/v1") && !r.key_fits("https://router.example/v1"));
        // a local server and a custom endpoint do not change format this way
        let l = find("ollama").unwrap();
        assert!(std::ptr::eq(l.with_format(Some("responses")), l));
        let c = find("custom:t-chat").unwrap();
        assert!(std::ptr::eq(c.with_format(Some("responses")), c));
    }

    #[test]
    fn siblings_are_the_provider_for_another_format() {
        let c = find("custom:t-chat").unwrap();
        let sib = c.sibling(Protocol::OpenAiResponses).unwrap();
        assert_eq!((sib.id, sib.protocol, sib.base_url), ("custom:t-chat", Protocol::OpenAiResponses, c.base_url), "the same custom provider, speaking another format");
        assert_eq!(find("custom:t-responses").unwrap().sibling(Protocol::Anthropic).unwrap().protocol, Protocol::Anthropic);
        assert_eq!(find("custom:t-claude").unwrap().sibling(Protocol::OpenAi).unwrap().protocol, Protocol::OpenAi);
        let o = find("openai").unwrap();
        let s = o.sibling(Protocol::OpenAiResponses).unwrap();
        assert_eq!((s.id, s.protocol), ("openai", Protocol::OpenAiResponses));
        assert!(find("gemini").unwrap().sibling(Protocol::OpenAiResponses).is_none());
    }

    #[test]
    fn custom_endpoints_judge_pictures_by_model_name_like_other_hosts() {
        for id in ["custom:t-chat", "custom:t-responses", "custom:t-claude"] {
            let p = find(id).unwrap();
            assert!(can_see(p, "gpt-5") && !can_see(p, "tiny-text-model"), "{id}");
        }
    }
}
