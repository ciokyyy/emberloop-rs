//! Fixed provider endpoints shared by harness configuration and proxy routing.
//!
//! `KEY_PROVIDERS` lists the API-key providers pi-rust bundles. `OAUTH_PROVIDERS` adds
//! subscription endpoints that must also cross the proxy, but are never advertised as
//! key providers. Credentials are resolved by the harness, not this registry.

/// One provider endpoint; the registry determines its authentication kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderEndpoint {
    pub id: &'static str,
    pub name: &'static str,
    /// pi's default `baseUrl` for the provider, as in its bundled catalogue.
    pub base_url: &'static str,
}

impl ProviderEndpoint {
    /// `(origin, path)` of `base_url`, e.g. `("https://api.groq.com", "/openai/v1")`.
    pub fn split(&self) -> (&'static str, &'static str) {
        let rest = self
            .base_url
            .split_once("://")
            .map_or(0, |(scheme, _)| scheme.len() + 3);
        match self.base_url[rest..].find('/') {
            Some(slash) => self.base_url.split_at(rest + slash),
            None => (self.base_url, ""),
        }
    }

    /// The `baseUrl` pi gets in `models.json` for a proxy at `proxy_url`.
    pub fn proxied_base_url(&self, proxy_url: &str) -> String {
        format!(
            "{}/x/{}{}",
            proxy_url.trim_end_matches('/'),
            self.id,
            self.split().1
        )
    }
}

macro_rules! providers {
    ($(($id:literal, $name:literal, $base:literal)),+ $(,)?) => {
        pub const KEY_PROVIDERS: &[ProviderEndpoint] = &[
            $(ProviderEndpoint { id: $id, name: $name, base_url: $base }),+
        ];
    };
}

providers![
    ("anthropic", "Anthropic", "https://api.anthropic.com"),
    ("ant-ling", "Ant Ling", "https://api.ant-ling.com/v1"),
    ("baseten", "Baseten", "https://inference.baseten.co/v1"),
    ("cerebras", "Cerebras", "https://api.cerebras.ai/v1"),
    ("deepseek", "DeepSeek", "https://api.deepseek.com"),
    (
        "fireworks",
        "Fireworks",
        "https://api.fireworks.ai/inference"
    ),
    ("groq", "Groq", "https://api.groq.com/openai/v1"),
    (
        "huggingface",
        "Hugging Face",
        "https://router.huggingface.co/v1"
    ),
    (
        "kimi-coding",
        "Kimi For Coding",
        "https://api.kimi.com/coding/v1"
    ),
    ("minimax", "MiniMax", "https://api.minimax.io/anthropic"),
    (
        "minimax-cn",
        "MiniMax CN",
        "https://api.minimaxi.com/anthropic"
    ),
    ("mistral", "Mistral", "https://api.mistral.ai"),
    ("moonshotai", "Moonshot AI", "https://api.moonshot.ai/v1"),
    (
        "moonshotai-cn",
        "Moonshot AI CN",
        "https://api.moonshot.cn/v1"
    ),
    ("nvidia", "NVIDIA", "https://integrate.api.nvidia.com/v1"),
    ("openai", "OpenAI", "https://api.openai.com/v1"),
    ("openrouter", "OpenRouter", "https://openrouter.ai/api/v1"),
    (
        "qwen-token-plan",
        "Qwen Token Plan",
        "https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1"
    ),
    (
        "qwen-token-plan-cn",
        "Qwen Token Plan CN",
        "https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1"
    ),
    (
        "qwen-token-plan-individual",
        "Qwen Token Plan Individual",
        "https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1"
    ),
    ("together", "Together", "https://api.together.ai/v1"),
    (
        "vercel-ai-gateway",
        "Vercel AI Gateway",
        "https://ai-gateway.vercel.sh"
    ),
    ("xai", "xAI", "https://api.x.ai/v1"),
    ("xiaomi", "Xiaomi", "https://api.xiaomimimo.com/v1"),
    (
        "xiaomi-token-plan-ams",
        "Xiaomi Token Plan AMS",
        "https://token-plan-ams.xiaomimimo.com/v1"
    ),
    (
        "xiaomi-token-plan-cn",
        "Xiaomi Token Plan CN",
        "https://token-plan-cn.xiaomimimo.com/v1"
    ),
    (
        "xiaomi-token-plan-sgp",
        "Xiaomi Token Plan SGP",
        "https://token-plan-sgp.xiaomimimo.com/v1"
    ),
    ("zai", "Z.AI", "https://api.z.ai/api/coding/paas/v4"),
    (
        "zai-coding-cn",
        "Z.AI Coding CN",
        "https://open.bigmodel.cn/api/coding/paas/v4"
    ),
];

/// OAuth endpoints are proxied, but must never appear in the API-key provider list.
pub const OAUTH_PROVIDERS: &[ProviderEndpoint] = &[ProviderEndpoint {
    id: "openai-codex",
    name: "ChatGPT",
    base_url: "https://chatgpt.com/backend-api",
}];

pub fn proxy_provider(id: &str) -> Option<&'static ProviderEndpoint> {
    key_provider(id).or_else(|| OAUTH_PROVIDERS.iter().find(|provider| provider.id == id))
}

pub fn key_provider(id: &str) -> Option<&'static ProviderEndpoint> {
    KEY_PROVIDERS.iter().find(|provider| provider.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_url_safe() {
        for (index, provider) in KEY_PROVIDERS.iter().enumerate() {
            assert!(provider
                .id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'));
            assert!(provider.base_url.starts_with("https://"), "{}", provider.id);
            assert!(!KEY_PROVIDERS[..index]
                .iter()
                .any(|other| other.id == provider.id));
        }
    }

    #[test]
    fn base_url_splits_into_origin_and_path() {
        let groq = key_provider("groq").unwrap();
        assert_eq!(groq.split(), ("https://api.groq.com", "/openai/v1"));
        let anthropic = key_provider("anthropic").unwrap();
        assert_eq!(anthropic.split(), ("https://api.anthropic.com", ""));
    }

    #[test]
    fn the_proxied_base_url_keeps_pi_path() {
        let groq = key_provider("groq").unwrap();
        assert_eq!(
            groq.proxied_base_url("http://127.0.0.1:1/p/tok/"),
            "http://127.0.0.1:1/p/tok/x/groq/openai/v1"
        );
        assert!(key_provider("opencode-go").is_none());
    }
    #[test]
    fn codex_oauth_has_a_proxy_target_but_is_not_an_api_key_provider() {
        let endpoint = proxy_provider("openai-codex").unwrap();
        assert_eq!(endpoint.split(), ("https://chatgpt.com", "/backend-api"));
        assert_eq!(
            endpoint.proxied_base_url("http://127.0.0.1:1/proxy/tok"),
            "http://127.0.0.1:1/proxy/tok/x/openai-codex/backend-api"
        );
        assert!(key_provider("openai-codex").is_none());
    }
}
