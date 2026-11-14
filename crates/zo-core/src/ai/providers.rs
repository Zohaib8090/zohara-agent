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
    // any other OpenAI-compatible server: the person types the address
    // The three wire formats of a server whose address the person types. The key typed for the address is sent only there.
    Provider { id: "custom", name: "Custom endpoint: OpenAI Chat (/v1/chat/completions)", protocol: OpenAi, base_url: "http://127.0.0.1:8000/v1", local: false, models: &[] },
    Provider { id: "custom-responses", name: "Custom endpoint: OpenAI Responses (/v1/responses)", protocol: OpenAiResponses, base_url: "http://127.0.0.1:8000/v1", local: false, models: &[] },
    Provider { id: "custom-claude", name: "Custom endpoint: Claude format (/v1/messages)", protocol: Anthropic, base_url: "http://127.0.0.1:8000/v1", local: false, models: &[] },
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

/// What the person has to pick for a wire format, in a sentence ("pick Custom endpoint: OpenAI Responses").
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

pub fn all() -> &'static [Provider] {
    ALL
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
    /// One of the three "Custom endpoint" providers (chat, responses, claude): any address, with a key typed for that address.
    pub fn is_custom(&self) -> bool {
        self.id == "custom" || self.id.starts_with("custom-")
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
            return ALL.iter().find(|p| p.is_custom() && p.protocol == protocol);
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
    ALL.iter().find(|p| p.id == id)
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
    fn the_three_custom_endpoints_have_their_labels_and_formats() {
        let c = find("custom").unwrap();
        let r = find("custom-responses").unwrap();
        let m = find("custom-claude").unwrap();
        assert_eq!(c.name, "Custom endpoint: OpenAI Chat (/v1/chat/completions)");
        assert_eq!(r.name, "Custom endpoint: OpenAI Responses (/v1/responses)");
        assert_eq!(m.name, "Custom endpoint: Claude format (/v1/messages)");
        assert_eq!((c.protocol, r.protocol, m.protocol), (Protocol::OpenAi, Protocol::OpenAiResponses, Protocol::Anthropic));
        assert!([c, r, m].iter().all(|p| p.is_custom() && !p.local && p.models.is_empty() && p.takes_format() == false));
        assert_eq!(all().iter().filter(|p| p.is_custom()).count(), 3);
        assert!(!find("openai").unwrap().is_custom() && !find("ollama").unwrap().is_custom() && !find("anthropic").unwrap().is_custom());
    }

    #[test]
    fn a_custom_endpoint_key_goes_to_any_address_and_the_others_only_to_their_own_host() {
        for id in ["custom", "custom-responses", "custom-claude"] {
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
        let c = find("custom").unwrap();
        assert!(std::ptr::eq(c.with_format(Some("responses")), c));
    }

    #[test]
    fn siblings_are_the_provider_for_another_format() {
        let c = find("custom").unwrap();
        assert_eq!(c.sibling(Protocol::OpenAiResponses).unwrap().id, "custom-responses");
        assert_eq!(find("custom-responses").unwrap().sibling(Protocol::Anthropic).unwrap().id, "custom-claude");
        assert_eq!(find("custom-claude").unwrap().sibling(Protocol::OpenAi).unwrap().id, "custom");
        let o = find("openai").unwrap();
        let s = o.sibling(Protocol::OpenAiResponses).unwrap();
        assert_eq!((s.id, s.protocol), ("openai", Protocol::OpenAiResponses));
        assert!(find("gemini").unwrap().sibling(Protocol::OpenAiResponses).is_none());
    }

    #[test]
    fn custom_endpoints_judge_pictures_by_model_name_like_other_hosts() {
        for id in ["custom", "custom-responses", "custom-claude"] {
            let p = find(id).unwrap();
            assert!(can_see(p, "gpt-5") && !can_see(p, "tiny-text-model"), "{id}");
        }
    }
}
