//! The `[chat]` config section — agentic-chat policy + the orchestrator budget
//! tree ([FR-CF-06], [ADR-40], [ADR-41]).
//!
//! `[chat]` is an **optional** `config.toml` section parsed under the same
//! `#[serde(deny_unknown_fields)]` discipline as the rest of the policy
//! ([FR-CF-01]): every key is optional with a documented default, and an unknown
//! key (or out-of-range value) fails loud at load (exit 2). It carries the
//! non-secret chat policy only — the **API key is never here**; it lives in the
//! gitignored [`secrets.toml`](super::secrets) ([NFR-SE-07]).
//!
//! # Why the provider lives in the core, not `agent-core`
//! Parsing `[chat]` is **policy**, not networking, so it belongs in the
//! default-tree [config component] alongside every other `config.toml` table —
//! it must be readable without the `ui` feature. The `agent-core` crate (which
//! owns the `rig` clients and its `reqwest` HTTP backend) is `ui`-only; if the
//! core depended on it, the HTTP client would leak into the default dependency
//! tree and break the byte-identical no-networking fitness function
//! ([NFR-SE-01]). So [`ChatProvider`] is a **core-local** enum whose serde names
//! match `agent-core`'s `ProviderKind` (`"anthropic"` / `"openai"`); the `ui`
//! layer bridges the two when it constructs a provider.
//!
//! [config component]: ../../../docs/specs/architecture/components/config.md
//! [FR-CF-01]: ../../../docs/specs/requirements/FR-CF-01.md
//! [FR-CF-06]: ../../../docs/specs/requirements/FR-CF-06.md
//! [NFR-SE-01]: ../../../docs/specs/requirements/NFR-SE-01.md
//! [NFR-SE-07]: ../../../docs/specs/requirements/NFR-SE-07.md
//! [ADR-40]: ../../../docs/specs/architecture/decisions/ADR-40.md
//! [ADR-41]: ../../../docs/specs/architecture/decisions/ADR-41.md

use serde::{Deserialize, Serialize};

use super::error::ConfigError;

/// The default OpenAI-compatible endpoint: **OpenRouter** ([FR-CF-06], [ADR-41]).
///
/// Kept byte-identical to `agent-core`'s `DEFAULT_OPENAI_BASE_URL` so the policy
/// default the editor renders is the endpoint the substrate dials. The headline
/// use case is reaching open models through a gateway, not committing to one
/// vendor — so the OpenAI-compatible provider defaults here rather than to
/// `api.openai.com`.
///
/// [FR-CF-06]: ../../../docs/specs/requirements/FR-CF-06.md
/// [ADR-41]: ../../../docs/specs/architecture/decisions/ADR-41.md
pub const DEFAULT_CHAT_BASE_URL: &str = "https://openrouter.ai/api/v1";

/// Global per-turn tool-call ceiling default — **48** ([ADR-41] budget tree).
pub const DEFAULT_MAX_TOOL_CALLS: u32 = 48;

/// Per-subagent tool-call cap default — **16** ([ADR-41] budget tree).
pub const DEFAULT_MAX_SUBAGENT_TOOL_CALLS: u32 = 16;

/// Max-replans default — **3** ([ADR-41] budget tree).
pub const DEFAULT_MAX_REPLANS: u32 = 3;

/// Provider-retry count default — **2** attempts beyond the first ([CR-060],
/// [S-240], [FR-CF-06]). `0` disables retries (a single attempt). Kept
/// byte-identical to `agent-core`'s `DEFAULT_MAX_PROVIDER_RETRIES` so the policy
/// the editor renders is the policy the retry decorator applies.
///
/// [CR-060]: ../../../docs/requests/CR-060-chat-resilience-recoverable-faults.md
pub const DEFAULT_MAX_PROVIDER_RETRIES: u32 = 2;

/// Provider-retry base backoff default — **200 ms** ([CR-060], [S-240]). The
/// first retry interval, doubled (with jitter) each subsequent retry. Kept
/// byte-identical to `agent-core`'s `DEFAULT_PROVIDER_RETRY_BASE_MS`.
pub const DEFAULT_PROVIDER_RETRY_BASE_MS: u32 = 200;

/// The upper bound on `max_provider_retries`: a retry count above this can never
/// help and only amplifies load against the provider, so it fails loud at load.
const MAX_PROVIDER_RETRIES_CEILING: u32 = 10;

/// The upper bound on `temperature` — provider APIs reject anything above this,
/// so a misconfiguration fails loud at load rather than on the first turn.
const MAX_TEMPERATURE: f64 = 2.0;

fn default_chat_base_url() -> String {
    DEFAULT_CHAT_BASE_URL.to_string()
}

fn default_max_tool_calls() -> u32 {
    DEFAULT_MAX_TOOL_CALLS
}

fn default_max_subagent_tool_calls() -> u32 {
    DEFAULT_MAX_SUBAGENT_TOOL_CALLS
}

fn default_max_replans() -> u32 {
    DEFAULT_MAX_REPLANS
}

fn default_max_provider_retries() -> u32 {
    DEFAULT_MAX_PROVIDER_RETRIES
}

fn default_provider_retry_base_ms() -> u32 {
    DEFAULT_PROVIDER_RETRY_BASE_MS
}

/// Which provider family the chat agent talks to ([FR-CF-06]).
///
/// A **core-local** mirror of `agent-core`'s `ProviderKind` — same serde wire
/// names (`"anthropic"` / `"openai"`) so a `[chat] provider` value round-trips
/// to the substrate's enum, but defined here so `[chat]` parsing needs no
/// `ui`-only dependency (see the module docs).
///
/// [FR-CF-06]: ../../../docs/specs/requirements/FR-CF-06.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ChatProvider {
    /// The native Anthropic Messages provider (`tool_use` / `tool_result`).
    Anthropic,
    /// The OpenAI-compatible (Chat Completions) provider — default OpenRouter
    /// via [`DEFAULT_CHAT_BASE_URL`]. The default family, pairing with the
    /// OpenRouter `base_url` default ([FR-CF-06]).
    #[serde(rename = "openai")]
    #[default]
    OpenAi,
}

/// Optional **per-role model overrides** ([FR-CF-06], [ADR-41]).
///
/// The orchestrator roster is *fixed* ([ADR-41]) — a planner plus four
/// specialized subagents — so the override keys are an enumerated set under
/// `#[serde(deny_unknown_fields)]`: a typo'd role fails loud rather than being
/// silently ignored. Every role with no override falls back to the top-level
/// [`ChatConfig::model`] (see [`ChatConfig::model_for_role`]).
///
/// [FR-CF-06]: ../../../docs/specs/requirements/FR-CF-06.md
/// [ADR-41]: ../../../docs/specs/architecture/decisions/ADR-41.md
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatModelOverrides {
    /// Override for the planner `Agent`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planner: Option<String>,
    /// Override for the Graph-Navigator subagent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph_navigator: Option<String>,
    /// Override for the Governance-Analyst subagent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub governance_analyst: Option<String>,
    /// Override for the Source-Reader subagent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_reader: Option<String>,
    /// Override for the (tool-less) Synthesizer subagent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub synthesizer: Option<String>,
}

/// One orchestrator role addressable by a per-role model override ([ADR-41]).
///
/// The fixed roster: the planner and the four specialized subagents. Used by
/// [`ChatConfig::model_for_role`] to resolve the effective model for a role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatRole {
    /// The plan→act→observe→replan planner.
    Planner,
    /// The navigation-tools subagent.
    GraphNavigator,
    /// The governance-tools subagent.
    GovernanceAnalyst,
    /// The sandboxed `read`/`grep`/`glob` subagent.
    SourceReader,
    /// The tool-less final-answer subagent.
    Synthesizer,
}

/// The parsed `[chat]` section — agentic-chat policy + the budget tree.
///
/// Every field defaults, so an absent `[chat]` deserialises to
/// [`ChatConfig::default`] (the documented defaults) and a partial `[chat]`
/// fills the omitted keys with theirs ([FR-CF-06] AC). The non-secret policy
/// only — the key is in [`secrets.toml`](super::secrets).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatConfig {
    /// The provider family (`"anthropic"` | `"openai"`); default
    /// [`ChatProvider::OpenAi`].
    #[serde(default)]
    pub provider: ChatProvider,

    /// The model identifier passed to the provider (a Claude id or an
    /// OpenRouter model slug). Optional — an absent model is the configure-first
    /// signal the Chat view ([FR-UI-18]) reads as "not yet usable".
    ///
    /// [FR-UI-18]: ../../../docs/specs/requirements/FR-UI-18.md
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// The OpenAI-compatible endpoint ([FR-CF-06]); default
    /// [`DEFAULT_CHAT_BASE_URL`] (OpenRouter). Applies to the `openai` provider;
    /// the `anthropic` provider uses its own native endpoint.
    #[serde(default = "default_chat_base_url")]
    pub base_url: String,

    /// Maximum tokens to request per completion. Optional — `None` lets the
    /// provider/`rig` apply its own default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,

    /// Sampling temperature in `[0.0, 2.0]`. Optional — `None` lets the
    /// provider apply its own default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,

    /// Budget tree: the **global per-turn tool-call ceiling** ([ADR-41]);
    /// default [`DEFAULT_MAX_TOOL_CALLS`] (48).
    #[serde(default = "default_max_tool_calls")]
    pub max_tool_calls: u32,

    /// Budget tree: the **per-subagent tool-call cap** ([ADR-41]); default
    /// [`DEFAULT_MAX_SUBAGENT_TOOL_CALLS`] (16).
    #[serde(default = "default_max_subagent_tool_calls")]
    pub max_subagent_tool_calls: u32,

    /// Budget tree: the **max replans** ([ADR-41]); default
    /// [`DEFAULT_MAX_REPLANS`] (3). `0` disables replanning (a single plan pass).
    #[serde(default = "default_max_replans")]
    pub max_replans: u32,

    /// Bounded provider-retry: the number of transient-fault retries **beyond**
    /// the first attempt ([CR-060], [S-240]); default
    /// [`DEFAULT_MAX_PROVIDER_RETRIES`] (2). `0` disables retries (a single
    /// attempt). Bounded above by an internal ceiling so a misconfiguration can
    /// never amplify load against the provider. The wiki-agent inherits this via
    /// [`EffectiveWikiModel`](super::EffectiveWikiModel).
    ///
    /// [CR-060]: ../../../docs/requests/CR-060-chat-resilience-recoverable-faults.md
    #[serde(default = "default_max_provider_retries")]
    pub max_provider_retries: u32,

    /// Bounded provider-retry: the base exponential-backoff delay in
    /// milliseconds ([CR-060], [S-240]); default
    /// [`DEFAULT_PROVIDER_RETRY_BASE_MS`] (200). Must be ≥ 1 — a zero base delay
    /// would busy-retry, so it is rejected at load.
    #[serde(default = "default_provider_retry_base_ms")]
    pub provider_retry_base_ms: u32,

    /// Optional per-role model overrides (`[chat.models]`, [FR-CF-06]).
    #[serde(default)]
    pub models: ChatModelOverrides,
}

impl Default for ChatConfig {
    fn default() -> Self {
        ChatConfig {
            provider: ChatProvider::default(),
            model: None,
            base_url: default_chat_base_url(),
            max_tokens: None,
            temperature: None,
            max_tool_calls: default_max_tool_calls(),
            max_subagent_tool_calls: default_max_subagent_tool_calls(),
            max_replans: default_max_replans(),
            max_provider_retries: default_max_provider_retries(),
            provider_retry_base_ms: default_provider_retry_base_ms(),
            models: ChatModelOverrides::default(),
        }
    }
}

impl ChatConfig {
    /// The effective model for `role`: its per-role override if set, else the
    /// top-level [`model`](Self::model) ([FR-CF-06], [ADR-41]).
    ///
    /// Returns `None` only when neither the role override nor the top-level
    /// model is set — the configure-first state.
    pub fn model_for_role(&self, role: ChatRole) -> Option<&str> {
        let override_for = match role {
            ChatRole::Planner => &self.models.planner,
            ChatRole::GraphNavigator => &self.models.graph_navigator,
            ChatRole::GovernanceAnalyst => &self.models.governance_analyst,
            ChatRole::SourceReader => &self.models.source_reader,
            ChatRole::Synthesizer => &self.models.synthesizer,
        };
        override_for
            .as_deref()
            .or(self.model.as_deref())
    }

    /// Validate the `[chat]` section: every numeric/range key must be in bounds,
    /// or it is a load-time [`ConfigError::InvalidValue`] (exit 2, [FR-CF-06] AC:
    /// "an out-of-range value is rejected … with no partial write").
    ///
    /// Bounds:
    /// - `base_url` must be non-empty (an empty endpoint would dial nowhere);
    /// - `max_tool_calls` ≥ 1 (the global ceiling must admit at least one call);
    /// - `max_subagent_tool_calls` in `[1, max_tool_calls]` (a per-subagent cap
    ///   above the global ceiling is meaningless — the global bound wins first);
    /// - `max_tokens`, if set, ≥ 1;
    /// - `temperature`, if set, in `[0.0, 2.0]`;
    /// - `max_provider_retries` ≤ [`MAX_PROVIDER_RETRIES_CEILING`] (a higher
    ///   count only amplifies load; `0` is valid — retries disabled);
    /// - `provider_retry_base_ms` ≥ 1 (a zero base delay would busy-retry).
    ///
    /// `max_replans` needs no check: `0` is valid (a single plan pass, no replan)
    /// and `u32` has no negative form.
    pub(crate) fn validate(&self) -> Result<(), ConfigError> {
        let invalid = |key: &str, message: String| ConfigError::InvalidValue {
            key: format!("chat.{key}"),
            message,
        };

        if self.base_url.trim().is_empty() {
            return Err(invalid("base_url", "must not be empty".to_string()));
        }
        if self.max_tool_calls < 1 {
            return Err(invalid(
                "max_tool_calls",
                "must be at least 1 (the global per-turn tool-call ceiling)".to_string(),
            ));
        }
        if self.max_subagent_tool_calls < 1 {
            return Err(invalid(
                "max_subagent_tool_calls",
                "must be at least 1 (the per-subagent tool-call cap)".to_string(),
            ));
        }
        if self.max_subagent_tool_calls > self.max_tool_calls {
            return Err(invalid(
                "max_subagent_tool_calls",
                format!(
                    "{} exceeds max_tool_calls ({}); a per-subagent cap above the global \
                     ceiling can never bind",
                    self.max_subagent_tool_calls, self.max_tool_calls
                ),
            ));
        }
        if let Some(max_tokens) = self.max_tokens {
            if max_tokens < 1 {
                return Err(invalid("max_tokens", "must be at least 1".to_string()));
            }
        }
        if let Some(temperature) = self.temperature {
            if !(0.0..=MAX_TEMPERATURE).contains(&temperature) {
                return Err(invalid(
                    "temperature",
                    format!("{temperature} is outside the valid range [0.0, {MAX_TEMPERATURE}]"),
                ));
            }
        }
        if self.max_provider_retries > MAX_PROVIDER_RETRIES_CEILING {
            return Err(invalid(
                "max_provider_retries",
                format!(
                    "{} exceeds the maximum of {}; a higher retry count only amplifies load \
                     against the provider",
                    self.max_provider_retries, MAX_PROVIDER_RETRIES_CEILING
                ),
            ));
        }
        if self.provider_retry_base_ms < 1 {
            return Err(invalid(
                "provider_retry_base_ms",
                "must be at least 1 (a zero base delay would busy-retry)".to_string(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    /// [FR-CF-06] AC: an absent `[chat]` section is all-defaults — OpenRouter
    /// `base_url`, the documented budget-tree defaults, the `openai` provider.
    #[test]
    fn absent_chat_section_is_all_defaults() {
        let cfg: Config = toml::from_str("").unwrap();
        assert_eq!(cfg.chat, ChatConfig::default());
        assert_eq!(cfg.chat.provider, ChatProvider::OpenAi);
        assert_eq!(cfg.chat.base_url, "https://openrouter.ai/api/v1");
        assert_eq!(cfg.chat.max_tool_calls, 48);
        assert_eq!(cfg.chat.max_subagent_tool_calls, 16);
        assert_eq!(cfg.chat.max_replans, 3);
        assert_eq!(cfg.chat.max_provider_retries, 2);
        assert_eq!(cfg.chat.provider_retry_base_ms, 200);
        assert!(cfg.chat.model.is_none());
        assert!(cfg.chat.max_tokens.is_none());
        assert!(cfg.chat.temperature.is_none());
    }

    /// A partial `[chat]` fills omitted keys with their documented defaults
    /// ([FR-CF-06] AC) — here only `model` is given.
    #[test]
    fn partial_chat_section_fills_defaults() {
        let cfg: Config = toml::from_str("[chat]\nmodel = \"anthropic/claude\"\n").unwrap();
        assert_eq!(cfg.chat.model.as_deref(), Some("anthropic/claude"));
        // Everything else is still the default.
        assert_eq!(cfg.chat.base_url, DEFAULT_CHAT_BASE_URL);
        assert_eq!(cfg.chat.max_tool_calls, DEFAULT_MAX_TOOL_CALLS);
        assert_eq!(cfg.chat.provider, ChatProvider::OpenAi);
    }

    /// The provider serde wire names match `agent-core`'s `ProviderKind`:
    /// `"anthropic"` and `"openai"` (the alias for the OpenAI-compatible family).
    #[test]
    fn provider_serde_wire_names() {
        let anthropic: Config =
            toml::from_str("[chat]\nprovider = \"anthropic\"\n").unwrap();
        assert_eq!(anthropic.chat.provider, ChatProvider::Anthropic);
        let openai: Config = toml::from_str("[chat]\nprovider = \"openai\"\n").unwrap();
        assert_eq!(openai.chat.provider, ChatProvider::OpenAi);
    }

    /// `#[serde(deny_unknown_fields)]`: an unknown `[chat]` key fails loud
    /// ([FR-CF-06] AC, the [FR-CF-01] discipline).
    #[test]
    fn unknown_chat_key_is_rejected() {
        let err = toml::from_str::<Config>("[chat]\nbogus = 1\n").unwrap_err();
        assert!(
            err.to_string().contains("bogus") || err.to_string().contains("unknown"),
            "unknown key error should name the field: {err}"
        );
    }

    /// An unknown per-role override key under `[chat.models]` also fails loud.
    #[test]
    fn unknown_role_override_is_rejected() {
        let err =
            toml::from_str::<Config>("[chat.models]\narchitect = \"m\"\n").unwrap_err();
        assert!(
            err.to_string().contains("architect") || err.to_string().contains("unknown"),
            "unknown role override should be rejected: {err}"
        );
    }

    /// Per-role overrides win over the top-level model; an un-overridden role
    /// falls back to it ([FR-CF-06], [ADR-41]).
    #[test]
    fn model_for_role_prefers_override_then_top_level() {
        let cfg = ChatConfig {
            model: Some("default/model".to_string()),
            models: ChatModelOverrides {
                planner: Some("smart/planner".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        // Overridden role uses its override.
        assert_eq!(cfg.model_for_role(ChatRole::Planner), Some("smart/planner"));
        // Un-overridden roles fall back to the top-level model.
        assert_eq!(
            cfg.model_for_role(ChatRole::Synthesizer),
            Some("default/model")
        );
        assert_eq!(
            cfg.model_for_role(ChatRole::GraphNavigator),
            Some("default/model")
        );
    }

    /// With neither override nor top-level model, a role resolves to `None` (the
    /// configure-first state).
    #[test]
    fn model_for_role_is_none_when_unset() {
        let cfg = ChatConfig::default();
        assert_eq!(cfg.model_for_role(ChatRole::Planner), None);
    }

    /// Each out-of-range value is a load-time `InvalidValue` (exit 2) naming the
    /// `chat.<key>` ([FR-CF-06] AC). Validation runs through the whole `Config`.
    #[test]
    fn out_of_range_values_are_rejected() {
        let bad = |chat: ChatConfig| Config {
            chat,
            ..Default::default()
        }
        .validate();

        // Empty base_url.
        assert!(matches!(
            bad(ChatConfig { base_url: "  ".to_string(), ..Default::default() }),
            Err(ConfigError::InvalidValue { ref key, .. }) if key == "chat.base_url"
        ));
        // Zero global ceiling.
        assert!(matches!(
            bad(ChatConfig { max_tool_calls: 0, ..Default::default() }),
            Err(ConfigError::InvalidValue { ref key, .. }) if key == "chat.max_tool_calls"
        ));
        // Zero per-subagent cap.
        assert!(matches!(
            bad(ChatConfig { max_subagent_tool_calls: 0, ..Default::default() }),
            Err(ConfigError::InvalidValue { ref key, .. }) if key == "chat.max_subagent_tool_calls"
        ));
        // Per-subagent cap above the global ceiling.
        assert!(matches!(
            bad(ChatConfig { max_tool_calls: 4, max_subagent_tool_calls: 5, ..Default::default() }),
            Err(ConfigError::InvalidValue { ref key, .. }) if key == "chat.max_subagent_tool_calls"
        ));
        // Zero max_tokens.
        assert!(matches!(
            bad(ChatConfig { max_tokens: Some(0), ..Default::default() }),
            Err(ConfigError::InvalidValue { ref key, .. }) if key == "chat.max_tokens"
        ));
        // Temperature above the ceiling.
        assert!(matches!(
            bad(ChatConfig { temperature: Some(2.5), ..Default::default() }),
            Err(ConfigError::InvalidValue { ref key, .. }) if key == "chat.temperature"
        ));
        // Negative temperature.
        assert!(matches!(
            bad(ChatConfig { temperature: Some(-0.1), ..Default::default() }),
            Err(ConfigError::InvalidValue { ref key, .. }) if key == "chat.temperature"
        ));
    }

    /// A well-formed `[chat]` (including the boundary temperature values and the
    /// per-subagent cap equal to the global ceiling) validates.
    #[test]
    fn valid_chat_section_passes() {
        let cfg = ChatConfig {
            provider: ChatProvider::Anthropic,
            model: Some("anthropic/claude-sonnet".to_string()),
            base_url: "https://api.anthropic.com".to_string(),
            max_tokens: Some(4096),
            temperature: Some(2.0),
            max_tool_calls: 8,
            max_subagent_tool_calls: 8,
            max_replans: 0,
            max_provider_retries: DEFAULT_MAX_PROVIDER_RETRIES,
            provider_retry_base_ms: DEFAULT_PROVIDER_RETRY_BASE_MS,
            models: ChatModelOverrides::default(),
        };
        assert!(cfg.validate().is_ok());
    }

    /// [CR-060]/[FR-CF-06] AC: the provider-retry keys default to the documented
    /// values (2 retries, 200 ms) when absent, and honor explicit values.
    #[test]
    fn provider_retry_keys_default_and_honor_explicit_values() {
        // Absent → documented defaults.
        let defaulted: Config = toml::from_str("[chat]\nmodel = \"m\"\n").unwrap();
        assert_eq!(defaulted.chat.max_provider_retries, 2);
        assert_eq!(defaulted.chat.provider_retry_base_ms, 200);

        // Explicit values are honored, including `0` retries (disables retrying).
        let explicit: Config = toml::from_str(
            "[chat]\nmax_provider_retries = 0\nprovider_retry_base_ms = 50\n",
        )
        .unwrap();
        assert_eq!(explicit.chat.max_provider_retries, 0);
        assert_eq!(explicit.chat.provider_retry_base_ms, 50);
    }

    /// [CR-060]/[FR-CF-06] AC: `provider_retry_base_ms = 0` and an out-of-range
    /// `max_provider_retries` are rejected at load (exit 2), each naming its key.
    #[test]
    fn provider_retry_out_of_range_values_are_rejected() {
        let bad = |chat: ChatConfig| Config {
            chat,
            ..Default::default()
        }
        .validate();

        // A zero base delay would busy-retry.
        assert!(matches!(
            bad(ChatConfig { provider_retry_base_ms: 0, ..Default::default() }),
            Err(ConfigError::InvalidValue { ref key, .. }) if key == "chat.provider_retry_base_ms"
        ));
        // A retry count above the ceiling only amplifies load.
        assert!(matches!(
            bad(ChatConfig { max_provider_retries: MAX_PROVIDER_RETRIES_CEILING + 1, ..Default::default() }),
            Err(ConfigError::InvalidValue { ref key, .. }) if key == "chat.max_provider_retries"
        ));
        // The ceiling itself, and a zero count, are valid.
        assert!(bad(ChatConfig {
            max_provider_retries: MAX_PROVIDER_RETRIES_CEILING,
            ..Default::default()
        })
        .is_ok());
        assert!(bad(ChatConfig { max_provider_retries: 0, ..Default::default() }).is_ok());
    }
}
