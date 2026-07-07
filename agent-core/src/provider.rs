//! Provider resolution over `rig` (ADR-41, [anthropic-api], [openai-compatible-api]).
//!
//! Two providers, both `rig` clients:
//!
//! - **Anthropic-native** — `rig`'s Anthropic Messages client (`tool_use` /
//!   `tool_result`), default endpoint `https://api.anthropic.com`.
//! - **OpenAI-compatible** — `rig`'s **Chat Completions** client (not the
//!   Responses API) pointed at a **configurable `base_url`** (default
//!   `https://openrouter.ai/api/v1`), so one client reaches OpenRouter, a local
//!   server, OpenAI itself, or any OpenAI-compatible gateway.
//!
//! Resolving a provider only *constructs* a client (an in-memory `reqwest`
//! handle plus stored config); it opens no connection. The first egress happens
//! later, on an explicit, consent-gated chat turn (NFR-SE-07) — never here.
//!
//! [anthropic-api]: docs/specs/architecture/integrations/anthropic-api.md
//! [openai-compatible-api]: docs/specs/architecture/integrations/openai-compatible-api.md

use anyhow::Context;
use rig_core::client::CompletionClient;
use rig_core::providers::{anthropic, openai};

use crate::retry::{RetryPolicy, RetryingModel};

/// The default OpenAI-compatible endpoint: **OpenRouter** ([FR-CF-06], ADR-41).
///
/// The headline use case is reaching open models through a gateway, not
/// committing to one vendor — so the OpenAI-compatible provider defaults here
/// rather than to `api.openai.com`.
pub const DEFAULT_OPENAI_BASE_URL: &str = "https://openrouter.ai/api/v1";

/// The default Anthropic Messages endpoint.
pub const DEFAULT_ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com";

/// `rig`'s native Anthropic client (the default `reqwest` HTTP backend).
pub type AnthropicClient = anthropic::Client;

/// `rig`'s OpenAI-compatible **Chat Completions** client (the default `reqwest`
/// HTTP backend). This is the `CompletionsClient`, not the Responses-API
/// `Client`, because gateways like OpenRouter speak Chat Completions.
pub type OpenAiCompatibleClient = openai::CompletionsClient;

/// Which provider family a chat/wiki agent talks to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// `rig`'s native Anthropic Messages provider.
    Anthropic,
    /// `rig`'s OpenAI-compatible (Chat Completions) provider, default OpenRouter.
    #[serde(rename = "openai")]
    OpenAiCompatible,
}

/// Resolved provider configuration: which family, the model id, the API key,
/// and an optional `base_url` override.
///
/// `base_url == None` selects the documented default for the kind
/// ([`DEFAULT_OPENAI_BASE_URL`] / [`DEFAULT_ANTHROPIC_BASE_URL`]); a `Some`
/// value overrides it (a local server, OpenAI, an Anthropic-compatible
/// gateway). The full `[chat]`-section parsing/secret-loading is S-169's job —
/// this struct is the in-memory shape the substrate resolves a `rig` client
/// from.
#[derive(Clone)]
pub struct ProviderConfig {
    /// The provider family.
    pub kind: ProviderKind,
    /// The model identifier passed to the provider (e.g. a Claude or an
    /// OpenRouter model slug).
    pub model: String,
    /// The API key (loaded from gitignored `secrets.toml` in the real flow;
    /// never echoed — NFR-SE-07).
    pub api_key: String,
    /// Optional endpoint override; `None` uses the kind's documented default.
    pub base_url: Option<String>,
}

/// Hand-written `Debug` that **redacts the API key** ([NFR-SE-07]: the key is
/// never echoed — not in any response body, log line, or rendered page). A
/// derived `Debug` would print `api_key` verbatim wherever a `ProviderConfig`
/// (or any struct embedding one) is `{:?}`-formatted — in a `tracing` event, an
/// `anyhow` context, or a test dump. We render presence + last-4 instead (the
/// same masked shape the Config editor uses, S-169), so the field stays
/// diagnosable without ever exposing the secret.
impl std::fmt::Debug for ProviderConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderConfig")
            .field("kind", &self.kind)
            .field("model", &self.model)
            .field("api_key", &MaskedKey(&self.api_key))
            .field("base_url", &self.base_url)
            .finish()
    }
}

/// A `Debug` wrapper that renders a secret as presence + last-4 only, never the
/// raw value ([NFR-SE-07]).
struct MaskedKey<'a>(&'a str);

impl std::fmt::Debug for MaskedKey<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.0.is_empty() {
            return f.write_str("<unset>");
        }
        let reversed_tail: String = self.0.chars().rev().take(4).collect();
        let last4: String = reversed_tail.chars().rev().collect();
        write!(f, "<set:…{last4}>")
    }
}

impl ProviderConfig {
    /// Construct an OpenAI-compatible config with the default OpenRouter
    /// endpoint.
    pub fn openai_compatible(model: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            kind: ProviderKind::OpenAiCompatible,
            model: model.into(),
            api_key: api_key.into(),
            base_url: None,
        }
    }

    /// Construct an Anthropic-native config with the default endpoint.
    pub fn anthropic(model: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            kind: ProviderKind::Anthropic,
            model: model.into(),
            api_key: api_key.into(),
            base_url: None,
        }
    }

    /// Set an explicit `base_url` override (returns `self` for chaining).
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = Some(base_url.into());
        self
    }

    /// The effective endpoint for this config — the override if present, else
    /// the kind's documented default.
    pub fn effective_base_url(&self) -> &str {
        match (&self.base_url, self.kind) {
            (Some(url), _) => url.as_str(),
            (None, ProviderKind::OpenAiCompatible) => DEFAULT_OPENAI_BASE_URL,
            (None, ProviderKind::Anthropic) => DEFAULT_ANTHROPIC_BASE_URL,
        }
    }

    /// The documented API-root the kind's error/preflight messages suggest.
    fn example_base_url(&self) -> &'static str {
        match self.kind {
            ProviderKind::OpenAiCompatible => DEFAULT_OPENAI_BASE_URL,
            ProviderKind::Anthropic => DEFAULT_ANTHROPIC_BASE_URL,
        }
    }

    /// Validate the configuration **before** a turn opens any connection ([S-199],
    /// [FR-UI-24]): a model is set, an API key is present, and the effective
    /// `base_url` is a well-formed http(s) URL that does not already include the
    /// path rig appends. Each failure names the **specific** problem so the Chat
    /// surface can render an actionable message ([NFR-CC-04]).
    ///
    /// This is the **deterministic** half of the preflight — it opens no socket,
    /// so it runs offline and the API key never leaves the struct ([NFR-SE-07]).
    /// Reachability is not probed here: a genuinely unreachable endpoint surfaces
    /// honestly as a [transport error](crate::ProviderFailure) naming the endpoint
    /// when the turn's first call fails (the sprint risk note's "surfacing, not
    /// guaranteeing reachability").
    pub fn preflight(&self) -> Result<(), PreflightError> {
        if self.model.trim().is_empty() {
            return Err(PreflightError::MissingModel);
        }
        if self.api_key.trim().is_empty() {
            return Err(PreflightError::MissingApiKey);
        }

        let effective = self.effective_base_url();
        let parsed = url::Url::parse(effective).map_err(|e| PreflightError::MalformedBaseUrl {
            url: effective.to_string(),
            reason: e.to_string(),
            example: self.example_base_url(),
        })?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(PreflightError::MalformedBaseUrl {
                url: effective.to_string(),
                reason: "the scheme must be http or https".to_string(),
                example: self.example_base_url(),
            });
        }
        if parsed.host_str().is_none_or(str::is_empty) {
            return Err(PreflightError::MalformedBaseUrl {
                url: effective.to_string(),
                reason: "the URL has no host".to_string(),
                example: self.example_base_url(),
            });
        }

        // rig's OpenAI-compatible client appends `/chat/completions` to the
        // base_url ([openai-compatible-api]); a base_url that already ends in that
        // path would post to `…/chat/completions/chat/completions`. (rig handles a
        // trailing slash correctly, so only the embedded path is a problem.)
        if self.kind == ProviderKind::OpenAiCompatible
            && path_ends_with_chat_completions(parsed.path())
        {
            return Err(PreflightError::DoubleAppendedPath {
                url: effective.to_string(),
                example: self.example_base_url(),
            });
        }

        Ok(())
    }
}

/// Whether a URL path already ends with the `/chat/completions` segment rig
/// appends — tolerant of a trailing slash.
fn path_ends_with_chat_completions(path: &str) -> bool {
    path.trim_end_matches('/').ends_with("/chat/completions")
}

/// A configuration problem caught by [`ProviderConfig::preflight`] **before** the
/// turn runs ([S-199], [FR-UI-24]). Each variant's `Display` names the specific
/// problem and the fix; none echoes the API key ([NFR-SE-07]).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PreflightError {
    /// No model id is set.
    #[error(
        "Chat is not configured yet — no model is set. Choose a provider model in \
         the Config tab before starting a turn."
    )]
    MissingModel,

    /// No API key is present.
    #[error(
        "Chat is not configured yet — no API key is set. Add an API key in the \
         Config tab before starting a turn."
    )]
    MissingApiKey,

    /// The effective `base_url` is not a well-formed http(s) URL.
    #[error(
        "The configured base_url ({url}) is not a valid http(s) URL: {reason}. Set \
         it to the provider's API root (for example {example})."
    )]
    MalformedBaseUrl {
        /// The offending URL.
        url: String,
        /// Why it failed to parse / validate.
        reason: String,
        /// A valid example for the provider kind.
        example: &'static str,
    },

    /// The `base_url` already includes the `/chat/completions` path rig appends.
    #[error(
        "The configured base_url ({url}) should be the API root (for example \
         {example}); rig appends \"/chat/completions\" automatically, so including \
         it here would double up the request path."
    )]
    DoubleAppendedPath {
        /// The offending URL.
        url: String,
        /// A valid example for the provider kind.
        example: &'static str,
    },
}

/// Resolve `rig`'s native Anthropic client from the config.
///
/// Constructs the client only; opens no connection. Fails with an actionable
/// error if the key/endpoint cannot produce a valid client.
pub fn resolve_anthropic(config: &ProviderConfig) -> anyhow::Result<AnthropicClient> {
    anthropic::Client::builder()
        .api_key(config.api_key.clone())
        .base_url(config.effective_base_url())
        .build()
        .map_err(|e| anyhow::anyhow!(e))
        .context("failed to construct the rig Anthropic client")
}

/// Resolve `rig`'s OpenAI-compatible (Chat Completions) client from the config,
/// honoring the configurable `base_url` (default OpenRouter).
///
/// Constructs the client only; opens no connection.
pub fn resolve_openai_compatible(
    config: &ProviderConfig,
) -> anyhow::Result<OpenAiCompatibleClient> {
    openai::CompletionsClient::builder()
        .api_key(config.api_key.clone())
        .base_url(config.effective_base_url())
        .build()
        .map_err(|e| anyhow::anyhow!(e))
        .context("failed to construct the rig OpenAI-compatible client")
}

/// A `rig` completion model resolved from an OpenAI-compatible client, wrapped
/// with the bounded retry-with-backoff decorator ([`RetryingModel`], [CR-060],
/// [S-240]) — the handle an `Agent` is built from. Exposed so downstream agents
/// (chat-agent, wiki-agent) obtain a retry-aware model without re-deriving the
/// client/model wiring or the retry seam ([ADR-42]).
///
/// `retry` is the config-resolved policy (`[chat].max_provider_retries` /
/// `provider_retry_base_ms`); the wiki-agent inherits the same resolved policy.
///
/// [CR-060]: ../../docs/requests/CR-060-chat-resilience-recoverable-faults.md
/// [S-240]: ../../docs/planning/journal.md#s-240-provider-call-retry-with-backoff-in-agent-core
/// [ADR-42]: ../../docs/specs/architecture/decisions/ADR-42.md
pub fn openai_compatible_completion_model(
    config: &ProviderConfig,
    retry: RetryPolicy,
) -> anyhow::Result<RetryingModel<openai::completion::CompletionModel>> {
    let client = resolve_openai_compatible(config)?;
    Ok(RetryingModel::new(
        client.completion_model(&config.model),
        retry,
    ))
}

/// A `rig` completion model resolved from the native Anthropic client, wrapped
/// with the bounded retry-with-backoff decorator ([`RetryingModel`], [CR-060],
/// [S-240]). See [`openai_compatible_completion_model`] for the `retry` contract.
pub fn anthropic_completion_model(
    config: &ProviderConfig,
    retry: RetryPolicy,
) -> anyhow::Result<RetryingModel<anthropic::completion::CompletionModel>> {
    let client = resolve_anthropic(config)?;
    Ok(RetryingModel::new(
        client.completion_model(&config.model),
        retry,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openai_compatible_defaults_to_openrouter() {
        let cfg = ProviderConfig::openai_compatible("some/model", "sk-test");
        assert_eq!(cfg.effective_base_url(), "https://openrouter.ai/api/v1");
    }

    #[test]
    fn base_url_override_wins() {
        let cfg =
            ProviderConfig::openai_compatible("m", "k").with_base_url("http://127.0.0.1:1234/v1");
        assert_eq!(cfg.effective_base_url(), "http://127.0.0.1:1234/v1");
    }

    #[test]
    fn anthropic_defaults_to_its_endpoint() {
        let cfg = ProviderConfig::anthropic("claude", "sk-test");
        assert_eq!(cfg.effective_base_url(), "https://api.anthropic.com");
    }

    #[test]
    fn debug_redacts_the_api_key() {
        let cfg = ProviderConfig::openai_compatible("m", "sk-secret-abcd1234");
        let rendered = format!("{cfg:?}");
        assert!(
            !rendered.contains("sk-secret-abcd1234"),
            "the raw key must never appear in Debug output (NFR-SE-07): {rendered}",
        );
        // Presence + last-4 is rendered so the field stays diagnosable.
        assert!(
            rendered.contains("<set:…1234>"),
            "masked key shows last-4: {rendered}"
        );

        let unset = ProviderConfig::anthropic("claude", "");
        assert!(
            format!("{unset:?}").contains("<unset>"),
            "empty key renders <unset>"
        );
    }

    #[test]
    fn preflight_accepts_a_well_formed_config() {
        let cfg = ProviderConfig::openai_compatible("openai/gpt-4o", "sk-test");
        assert!(cfg.preflight().is_ok());
        let anthropic = ProviderConfig::anthropic("claude-3-5-sonnet", "sk-ant-test");
        assert!(anthropic.preflight().is_ok());
    }

    #[test]
    fn preflight_rejects_an_empty_model() {
        let cfg = ProviderConfig::openai_compatible("   ", "sk-test");
        assert_eq!(cfg.preflight(), Err(PreflightError::MissingModel));
    }

    #[test]
    fn preflight_rejects_a_missing_key() {
        let cfg = ProviderConfig::openai_compatible("m", "");
        assert_eq!(cfg.preflight(), Err(PreflightError::MissingApiKey));
    }

    #[test]
    fn preflight_rejects_a_malformed_base_url() {
        let cfg = ProviderConfig::openai_compatible("m", "k").with_base_url("not a url");
        assert!(matches!(
            cfg.preflight(),
            Err(PreflightError::MalformedBaseUrl { .. })
        ));
    }

    #[test]
    fn preflight_rejects_a_non_http_scheme() {
        let cfg = ProviderConfig::openai_compatible("m", "k").with_base_url("ftp://example.com/v1");
        assert!(matches!(
            cfg.preflight(),
            Err(PreflightError::MalformedBaseUrl { .. })
        ));
    }

    #[test]
    fn preflight_catches_a_double_appended_chat_completions_path() {
        // A base_url that already includes the path rig appends would post to
        // `…/chat/completions/chat/completions`.
        let cfg = ProviderConfig::openai_compatible("m", "k")
            .with_base_url("https://openrouter.ai/api/v1/chat/completions");
        assert!(matches!(
            cfg.preflight(),
            Err(PreflightError::DoubleAppendedPath { .. })
        ));
        // Tolerant of a trailing slash.
        let trailing = ProviderConfig::openai_compatible("m", "k")
            .with_base_url("https://openrouter.ai/api/v1/chat/completions/");
        assert!(matches!(
            trailing.preflight(),
            Err(PreflightError::DoubleAppendedPath { .. })
        ));
    }

    #[test]
    fn preflight_rejects_a_base_url_with_no_host() {
        // A scheme with an empty authority — `url` rejects this as an empty host
        // for the special http(s) schemes, so it surfaces as MalformedBaseUrl with
        // an actionable message rather than reaching the provider (S-199 review).
        let cfg = ProviderConfig::openai_compatible("m", "k").with_base_url("https://");
        assert!(matches!(
            cfg.preflight(),
            Err(PreflightError::MalformedBaseUrl { .. })
        ));
    }

    #[test]
    fn preflight_double_append_guard_is_openai_only() {
        // The `/chat/completions` double-append guard is OpenAI-compatible-specific
        // (rig's Anthropic client appends `/v1/messages`, not `/chat/completions`),
        // so an Anthropic base_url carrying that path must NOT trip the guard
        // (S-199 review).
        let anthropic = ProviderConfig::anthropic("claude", "sk-ant")
            .with_base_url("https://gateway.example.com/chat/completions");
        assert!(
            anthropic.preflight().is_ok(),
            "the double-append guard must not fire for the Anthropic kind"
        );
    }

    #[test]
    fn preflight_allows_the_api_root_that_rig_appends_to() {
        // The correct shape: the API root, no `/chat/completions` — rig appends it.
        let cfg = ProviderConfig::openai_compatible("m", "k")
            .with_base_url("https://openrouter.ai/api/v1");
        assert!(cfg.preflight().is_ok());
        // A trailing slash on the root is fine (rig normalizes it).
        let trailing = ProviderConfig::openai_compatible("m", "k")
            .with_base_url("https://openrouter.ai/api/v1/");
        assert!(trailing.preflight().is_ok());
    }

    #[test]
    fn preflight_never_echoes_the_api_key() {
        // Every preflight message must be safe to render verbatim (NFR-SE-07).
        let cfg = ProviderConfig::openai_compatible("m", "super-secret-key")
            .with_base_url("ftp://example.com");
        let message = cfg.preflight().unwrap_err().to_string();
        assert!(!message.contains("super-secret-key"), "{message}");
    }

    #[test]
    fn provider_kind_serde_uses_openai_alias() {
        let k: ProviderKind = serde_json::from_str("\"openai\"").unwrap();
        assert_eq!(k, ProviderKind::OpenAiCompatible);
        let a: ProviderKind = serde_json::from_str("\"anthropic\"").unwrap();
        assert_eq!(a, ProviderKind::Anthropic);
    }
}
