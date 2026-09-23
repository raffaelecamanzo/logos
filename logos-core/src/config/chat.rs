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

use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::error::ConfigError;
use super::load_config_from_root;
use super::secrets::{load_secrets_from_root, MaskedSecret, Secrets};

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

/// Where one half of a [`ChatResolution`] came from ([FR-WS-30], [ADR-67]).
///
/// Origin is part of the result, never inferred by consumers: a surface states
/// *where* a value came from rather than merely that one exists ([NFR-CC-04]).
///
/// [FR-WS-30]: ../../../docs/specs/requirements/FR-WS-30.md
/// [ADR-67]: ../../../docs/specs/architecture/decisions/ADR-67.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatOrigin {
    /// Declared by the member root itself.
    Member,
    /// Inherited from the workspace root (reachable only when one was passed).
    Workspace,
    /// Declared by neither root: the configure-first state for this half.
    Unset,
}

/// The effective chat policy and credential for one member, with the origin of
/// each half ([FR-WS-30], [ADR-67]) — the output of [`resolve_chat`].
///
/// A **sibling** of the parsed document, never merged into it: the Config
/// editor keeps round-tripping only the member's literal bytes.
///
/// # The never-echo invariant ([NFR-SE-07])
/// The raw key is held for the turn path's egress call and reachable only
/// through [`api_key`](Self::api_key). It is `#[serde(skip)]` — serialization
/// carries the [`MaskedSecret`] in [`credential`](Self::credential) instead — and
/// `Debug` is **hand-written** to omit it, following
/// [`EffectiveWikiModel`](super::EffectiveWikiModel).
///
/// [NFR-SE-07]: ../../../docs/specs/requirements/NFR-SE-07.md
#[derive(Clone, PartialEq, Serialize)]
pub struct ChatResolution {
    /// The effective `[chat]` table: whichever root declared `model`, taken
    /// **whole** (no field is drawn from the other root's table). When neither
    /// declares `model` it is the member's own table.
    pub policy: ChatConfig,
    /// Where [`policy`](Self::policy) came from; [`ChatOrigin::Unset`] exactly
    /// when no root declares a (non-blank) `model`.
    pub policy_origin: ChatOrigin,
    /// The masked effective credential (presence + last-4).
    pub credential: MaskedSecret,
    /// Where the credential came from; [`ChatOrigin::Unset`] exactly when no
    /// root holds a (non-blank) key.
    pub credential_origin: ChatOrigin,
    /// The secret store the credential was resolved from — the raw key's only
    /// holder, never serialized.
    #[serde(skip)]
    secrets: Secrets,
}

impl ChatResolution {
    /// The raw effective API key, for the agent that dials; `None` when
    /// [`credential_origin`](Self::credential_origin) is [`ChatOrigin::Unset`].
    pub fn api_key(&self) -> Option<&str> {
        self.secrets.chat_api_key()
    }
}

/// `Debug` that **omits** the raw key ([NFR-SE-07]) — the masked
/// [`credential`](ChatResolution::credential) is the only credential rendered.
impl fmt::Debug for ChatResolution {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChatResolution")
            .field("policy", &self.policy)
            .field("policy_origin", &self.policy_origin)
            .field("credential", &self.credential)
            .field("credential_origin", &self.credential_origin)
            .finish_non_exhaustive()
    }
}

/// Resolve the effective chat policy and credential for `member_root`, with
/// the origin of each half ([FR-WS-30], [ADR-67]) — the **one seam** the Chat
/// tab's usability gate and the turn path both read, so they cannot disagree.
///
/// - **Per half, member wins.** The policy half and the credential half resolve
///   independently; the member's declaration wins wherever it makes one.
/// - **The policy half is atomic, keyed on `model`** — the only `[chat]` field
///   without a default, hence the only sound discriminator on the parsed
///   document. A member declaring a non-blank `model` owns its whole table;
///   otherwise the workspace table is inherited **entire** if it declares one.
///   A blank `model` counts as undeclared, as a blank key does
///   ([`Secrets::chat_api_key`]).
/// - **`workspace_root` is taken, never discovered.** The caller passes the
///   already-resolved federation root; nothing here parses a manifest or walks
///   up the tree. With `None` this performs exactly the pre-existing two reads
///   (`<member>/.logos/config.toml` and `<member>/.logos/secrets.toml`) and no
///   [`ChatOrigin::Workspace`] is reachable ([ADR-52]).
/// - **The workspace tier is read lazily**, per half, only when the member does
///   not declare that half — so a member that declares both never reads it.
///
/// # Errors
/// A present-but-invalid file at either root fails loud through the ordinary
/// loaders ([`load_config_from_root`] / [`load_secrets_from_root`]), whose
/// [`ConfigError`] names the offending path.
///
/// [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
pub fn resolve_chat(
    member_root: &Path,
    workspace_root: Option<&Path>,
) -> Result<ChatResolution, ConfigError> {
    resolve_chat_with(
        member_root,
        workspace_root,
        |root| load_config_from_root(root).map(|config| config.chat),
        load_secrets_from_root,
    )
}

/// [`resolve_chat`] over injected loaders — the seam its read-count contract is
/// tested through.
fn resolve_chat_with(
    member_root: &Path,
    workspace_root: Option<&Path>,
    mut load_policy: impl FnMut(&Path) -> Result<ChatConfig, ConfigError>,
    mut load_secrets: impl FnMut(&Path) -> Result<Secrets, ConfigError>,
) -> Result<ChatResolution, ConfigError> {
    let (policy, policy_origin) = resolve_half(
        member_root,
        workspace_root,
        &mut load_policy,
        |policy: &ChatConfig| {
            policy
                .model
                .as_deref()
                .is_some_and(|m| !m.trim().is_empty())
        },
    )?;
    let (secrets, credential_origin) = resolve_half(
        member_root,
        workspace_root,
        &mut load_secrets,
        |secrets: &Secrets| secrets.chat_api_key().is_some(),
    )?;
    Ok(ChatResolution {
        policy,
        policy_origin,
        credential: secrets.chat_key_masked(),
        credential_origin,
        secrets,
    })
}

/// Resolve one half against the two-tier chain: the member's value if it
/// `declares`, else the workspace's if it `declares`, else the member's value
/// as [`ChatOrigin::Unset`]. The workspace root is read only on a member miss.
fn resolve_half<T>(
    member_root: &Path,
    workspace_root: Option<&Path>,
    load: &mut impl FnMut(&Path) -> Result<T, ConfigError>,
    declares: impl Fn(&T) -> bool,
) -> Result<(T, ChatOrigin), ConfigError> {
    let member = load(member_root)?;
    if declares(&member) {
        return Ok((member, ChatOrigin::Member));
    }
    if let Some(workspace_root) = workspace_root {
        let workspace = load(workspace_root)?;
        if declares(&workspace) {
            return Ok((workspace, ChatOrigin::Workspace));
        }
    }
    Ok((member, ChatOrigin::Unset))
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

/// [`resolve_chat`] over real on-disk roots ([FR-WS-30], [ADR-67], S-447).
#[cfg(test)]
mod resolution_tests {
    use std::cell::RefCell;
    use std::fs;
    use std::path::PathBuf;

    use super::*;

    /// A member's key and the workspace's key — distinct last-4s, so a test can
    /// tell which root a masked credential came from.
    const MEMBER_KEY: &str = "sk-member-secret-AAAA1111";
    const WORKSPACE_KEY: &str = "sk-workspace-secret-DEADBEEF";

    /// The workspace `[chat]` table: differs from a member's in `base_url` and
    /// in a budget knob, so a field-wise merge cannot pass for a whole-table one.
    const WORKSPACE_CHAT: &str = "[chat]\nmodel = \"workspace/model\"\n\
                                  base_url = \"https://workspace.example/v1\"\n\
                                  max_tool_calls = 30\n";
    const MEMBER_CHAT: &str = "[chat]\nmodel = \"member/model\"\n";

    /// A workspace directory with one member nested inside it — the real
    /// layout, so a resolution that walked up the tree would find the tier.
    struct Estate {
        _dir: tempfile::TempDir,
        workspace: PathBuf,
        member: PathBuf,
    }

    impl Estate {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let workspace = dir.path().to_path_buf();
            let member = workspace.join("svc-a");
            fs::create_dir_all(member.join(".logos")).unwrap();
            fs::create_dir_all(workspace.join(".logos")).unwrap();
            Estate {
                _dir: dir,
                workspace,
                member,
            }
        }

        fn policy(root: &Path, text: &str) {
            fs::write(root.join(".logos/config.toml"), text).unwrap();
        }

        fn key(root: &Path, key: &str) {
            fs::write(
                root.join(".logos/secrets.toml"),
                format!("[chat]\napi_key = \"{key}\"\n"),
            )
            .unwrap();
        }

        fn resolve(&self) -> ChatResolution {
            resolve_chat(&self.member, Some(&self.workspace)).unwrap()
        }

        fn resolve_single_root(&self) -> ChatResolution {
            resolve_chat(&self.member, None).unwrap()
        }
    }

    fn parsed(text: &str) -> ChatConfig {
        toml::from_str::<crate::config::Config>(text).unwrap().chat
    }

    fn origins(r: &ChatResolution) -> (ChatOrigin, ChatOrigin) {
        (r.policy_origin, r.credential_origin)
    }

    /// Member-only: the member declares both halves, the workspace neither.
    #[test]
    fn member_only_resolves_both_halves_from_the_member() {
        let e = Estate::new();
        Estate::policy(&e.member, MEMBER_CHAT);
        Estate::key(&e.member, MEMBER_KEY);

        let r = e.resolve();
        assert_eq!(origins(&r), (ChatOrigin::Member, ChatOrigin::Member));
        assert_eq!(r.policy, parsed(MEMBER_CHAT));
        assert_eq!(r.api_key(), Some(MEMBER_KEY));
    }

    /// Workspace-only: a member declaring neither half inherits both, and both
    /// origins report the workspace ([FR-WS-30] AC 1).
    #[test]
    fn workspace_only_is_inherited_whole_and_reported_as_workspace() {
        let e = Estate::new();
        Estate::policy(&e.workspace, WORKSPACE_CHAT);
        Estate::key(&e.workspace, WORKSPACE_KEY);

        let r = e.resolve();
        assert_eq!(origins(&r), (ChatOrigin::Workspace, ChatOrigin::Workspace));
        assert_eq!(r.policy, parsed(WORKSPACE_CHAT));
        assert_eq!(r.api_key(), Some(WORKSPACE_KEY));
        assert_eq!(r.credential.last4.as_deref(), Some("BEEF"));
    }

    /// Both declare both: the member wins on each half.
    #[test]
    fn both_declaring_resolves_to_the_member_on_each_half() {
        let e = Estate::new();
        Estate::policy(&e.member, MEMBER_CHAT);
        Estate::key(&e.member, MEMBER_KEY);
        Estate::policy(&e.workspace, WORKSPACE_CHAT);
        Estate::key(&e.workspace, WORKSPACE_KEY);

        let r = e.resolve();
        assert_eq!(origins(&r), (ChatOrigin::Member, ChatOrigin::Member));
        assert_eq!(r.policy, parsed(MEMBER_CHAT));
        assert_eq!(r.api_key(), Some(MEMBER_KEY));
    }

    /// Neither declares anything: both halves are unset, the policy is the
    /// member's (default) table and no credential is present.
    #[test]
    fn neither_declaring_is_unset_on_both_halves() {
        let e = Estate::new();

        let r = e.resolve();
        assert_eq!(origins(&r), (ChatOrigin::Unset, ChatOrigin::Unset));
        assert_eq!(r.policy, ChatConfig::default());
        assert!(r.api_key().is_none());
        assert!(!r.credential.present);
    }

    /// The shape the reported defect needs, asserted explicitly: a member that
    /// overrides the model while declaring no credential inherits the
    /// workspace credential, and the two origins differ ([FR-WS-30] AC 3).
    #[test]
    fn member_overrides_policy_while_inheriting_the_credential() {
        let e = Estate::new();
        Estate::policy(&e.member, MEMBER_CHAT);
        Estate::policy(&e.workspace, WORKSPACE_CHAT);
        Estate::key(&e.workspace, WORKSPACE_KEY);

        let r = e.resolve();
        assert_eq!(origins(&r), (ChatOrigin::Member, ChatOrigin::Workspace));
        assert_eq!(r.policy.model.as_deref(), Some("member/model"));
        assert_eq!(r.api_key(), Some(WORKSPACE_KEY));
        assert_eq!(r.credential.last4.as_deref(), Some("BEEF"));
    }

    /// The mirror image: a member holding its own key but no policy inherits the
    /// workspace policy and keeps its own credential.
    #[test]
    fn member_keeps_its_credential_while_inheriting_the_policy() {
        let e = Estate::new();
        Estate::key(&e.member, MEMBER_KEY);
        Estate::policy(&e.workspace, WORKSPACE_CHAT);
        Estate::key(&e.workspace, WORKSPACE_KEY);

        let r = e.resolve();
        assert_eq!(origins(&r), (ChatOrigin::Workspace, ChatOrigin::Member));
        assert_eq!(r.policy, parsed(WORKSPACE_CHAT));
        assert_eq!(r.api_key(), Some(MEMBER_KEY));
    }

    /// [ADR-67] §3: a member declaring `model` draws **no field** from the
    /// workspace table — the workspace's `base_url` and `max_tool_calls` stay
    /// behind, so a field-wise merge fails here.
    #[test]
    fn a_member_declaring_model_draws_no_field_from_the_workspace_table() {
        let e = Estate::new();
        Estate::policy(&e.member, MEMBER_CHAT);
        Estate::policy(&e.workspace, WORKSPACE_CHAT);

        let r = e.resolve();
        assert_eq!(r.policy_origin, ChatOrigin::Member);
        assert_eq!(r.policy.base_url, DEFAULT_CHAT_BASE_URL);
        assert_eq!(r.policy.max_tool_calls, DEFAULT_MAX_TOOL_CALLS);
        assert_eq!(r.policy, parsed(MEMBER_CHAT));
    }

    /// [ADR-67] §3: a member declaring no `model` inherits the workspace table
    /// **entire** — its own `base_url` and budget knob are not kept, so a
    /// field-wise merge fails here too.
    #[test]
    fn a_member_declaring_no_model_inherits_the_workspace_table_entire() {
        let e = Estate::new();
        Estate::policy(
            &e.member,
            "[chat]\nbase_url = \"https://member.example/v1\"\nmax_replans = 1\n",
        );
        Estate::policy(&e.workspace, WORKSPACE_CHAT);

        let r = e.resolve();
        assert_eq!(r.policy_origin, ChatOrigin::Workspace);
        assert_eq!(r.policy.base_url, "https://workspace.example/v1");
        assert_eq!(r.policy.max_tool_calls, 30);
        assert_eq!(r.policy.max_replans, DEFAULT_MAX_REPLANS);
        assert_eq!(r.policy, parsed(WORKSPACE_CHAT));
    }

    /// A blank `model` is undeclared, like a blank key: it does not shadow the
    /// workspace table.
    #[test]
    fn a_blank_member_model_does_not_shadow_the_workspace_policy() {
        let e = Estate::new();
        Estate::policy(&e.member, "[chat]\nmodel = \"  \"\n");
        Estate::policy(&e.workspace, WORKSPACE_CHAT);

        let r = e.resolve();
        assert_eq!(r.policy_origin, ChatOrigin::Workspace);
        assert_eq!(r.policy, parsed(WORKSPACE_CHAT));
    }

    /// A workspace table with no `model` declares no policy: the member's own
    /// table stands, unset.
    #[test]
    fn a_workspace_table_without_model_is_not_inherited() {
        let e = Estate::new();
        Estate::policy(&e.member, "[chat]\nmax_replans = 1\n");
        Estate::policy(
            &e.workspace,
            "[chat]\nbase_url = \"https://workspace.example/v1\"\n",
        );

        let r = e.resolve();
        assert_eq!(r.policy_origin, ChatOrigin::Unset);
        assert_eq!(r.policy.max_replans, 1);
        assert_eq!(r.policy.base_url, DEFAULT_CHAT_BASE_URL);
    }

    /// [ADR-52]: with no workspace root the resolution equals the pre-existing
    /// two reads over every fixture shape, and only `member`/`unset` origins
    /// are reachable — even though the member sits inside a directory whose
    /// `.logos/` declares both halves (no up-tree walk).
    #[test]
    fn no_workspace_root_is_the_pre_existing_two_reads_over_the_matrix() {
        let shapes: [(Option<&str>, Option<&str>); 4] = [
            (Some(MEMBER_CHAT), Some(MEMBER_KEY)),
            (Some(MEMBER_CHAT), None),
            (None, Some(MEMBER_KEY)),
            (None, None),
        ];
        for (policy, key) in shapes {
            let e = Estate::new();
            Estate::policy(&e.workspace, WORKSPACE_CHAT);
            Estate::key(&e.workspace, WORKSPACE_KEY);
            if let Some(text) = policy {
                Estate::policy(&e.member, text);
            }
            if let Some(k) = key {
                Estate::key(&e.member, k);
            }

            let r = e.resolve_single_root();
            let shape = format!("policy={policy:?} key={key:?}");
            assert_eq!(
                r.policy,
                load_config_from_root(&e.member).unwrap().chat,
                "{shape}"
            );
            assert_eq!(
                r.api_key(),
                load_secrets_from_root(&e.member).unwrap().chat_api_key(),
                "{shape}"
            );
            assert_ne!(r.policy_origin, ChatOrigin::Workspace, "{shape}");
            assert_ne!(r.credential_origin, ChatOrigin::Workspace, "{shape}");
            assert_eq!(
                r.policy_origin,
                if policy.is_some() {
                    ChatOrigin::Member
                } else {
                    ChatOrigin::Unset
                },
                "{shape}"
            );
            assert_eq!(
                r.credential_origin,
                if key.is_some() {
                    ChatOrigin::Member
                } else {
                    ChatOrigin::Unset
                },
                "{shape}"
            );
        }
    }

    /// Every read the resolution makes, as `(loader, root)` in call order.
    fn reads_of(
        member: &Path,
        workspace: Option<&Path>,
        declares: bool,
    ) -> Vec<(&'static str, PathBuf)> {
        let log = RefCell::new(Vec::new());
        let policy = |root: &Path| {
            log.borrow_mut().push(("config", root.to_path_buf()));
            Ok(if declares && root == member {
                parsed(MEMBER_CHAT)
            } else {
                ChatConfig::default()
            })
        };
        let secrets = |root: &Path| {
            log.borrow_mut().push(("secrets", root.to_path_buf()));
            let mut s = Secrets::default();
            if declares && root == member {
                s.chat.api_key = Some(MEMBER_KEY.to_string());
            }
            Ok(s)
        };
        resolve_chat_with(member, workspace, policy, secrets).unwrap();
        log.into_inner()
    }

    /// [ADR-52] by construction: `None` performs exactly the two member reads,
    /// declared or not; a workspace root adds a read only for a half the member
    /// leaves undeclared, and never any path but the one passed.
    #[test]
    fn read_set_is_two_member_reads_plus_only_the_workspace_halves_needed() {
        let member = Path::new("/estate/svc-a");
        let ws = Path::new("/estate");
        let two = vec![
            ("config", member.to_path_buf()),
            ("secrets", member.to_path_buf()),
        ];

        assert_eq!(reads_of(member, None, true), two);
        assert_eq!(reads_of(member, None, false), two);
        assert_eq!(reads_of(member, Some(ws), true), two);
        assert_eq!(
            reads_of(member, Some(ws), false),
            vec![
                ("config", member.to_path_buf()),
                ("config", ws.to_path_buf()),
                ("secrets", member.to_path_buf()),
                ("secrets", ws.to_path_buf()),
            ]
        );
    }

    /// An invalid workspace file fails loud, naming the workspace path, when
    /// the member leaves that half to it.
    #[test]
    fn an_invalid_workspace_policy_fails_loud_when_it_is_needed() {
        let e = Estate::new();
        Estate::policy(&e.workspace, "[chat]\nbogus = 1\n");

        let err = resolve_chat(&e.member, Some(&e.workspace)).unwrap_err();
        assert!(
            matches!(err, ConfigError::Parse { ref path, .. } if path.starts_with(&e.workspace) && !path.starts_with(&e.member))
        );
        assert_eq!(err.exit_code(), 2);
    }

    /// [NFR-SE-07]: neither `Debug` nor the serialized resolution carries the
    /// raw key — only the masked presence + last-4 — for an inherited and for
    /// a member-declared credential alike.
    #[test]
    fn debug_and_serialization_never_carry_the_raw_key() {
        let e = Estate::new();
        Estate::policy(&e.workspace, WORKSPACE_CHAT);
        Estate::key(&e.workspace, WORKSPACE_KEY);
        let inherited = e.resolve();

        let e2 = Estate::new();
        Estate::policy(&e2.member, MEMBER_CHAT);
        Estate::key(&e2.member, MEMBER_KEY);
        let declared = e2.resolve();

        for (r, raw, last4) in [
            (&inherited, WORKSPACE_KEY, "BEEF"),
            (&declared, MEMBER_KEY, "1111"),
        ] {
            assert_eq!(r.api_key(), Some(raw), "the fixture really holds the key");
            let dbg = format!("{r:?}");
            let alt = format!("{r:#?}");
            let json = serde_json::to_string(r).unwrap();
            let value = serde_json::to_value(r).unwrap();
            for rendered in [&dbg, &alt, &json] {
                assert!(
                    !rendered.contains(raw),
                    "raw key leaked (NFR-SE-07): {rendered}"
                );
            }
            assert!(dbg.contains(last4), "Debug shows last-4: {dbg}");
            assert_eq!(value["credential"]["last4"], last4);
            assert_eq!(value["credential"]["present"], true);
        }
        let value = serde_json::to_value(&inherited).unwrap();
        assert_eq!(value["policy_origin"], "workspace");
        assert_eq!(value["credential_origin"], "workspace");
    }

    /// A blank member key is undeclared, like a blank `model`: it does not
    /// shadow the workspace credential, and origin and `api_key()` agree.
    #[test]
    fn a_blank_member_key_does_not_shadow_the_workspace_credential() {
        let e = Estate::new();
        Estate::policy(&e.member, MEMBER_CHAT);
        Estate::key(&e.member, "   ");
        Estate::key(&e.workspace, WORKSPACE_KEY);

        let r = e.resolve();
        assert_eq!(r.credential_origin, ChatOrigin::Workspace);
        assert_eq!(r.api_key(), Some(WORKSPACE_KEY));
        assert_eq!(r.credential.last4.as_deref(), Some("BEEF"));
    }

    /// A padded key dials and masks as its trimmed value — the same key the
    /// pre-existing `chat_api_key` read yields.
    #[test]
    fn a_padded_key_is_trimmed_for_dialling_and_masking() {
        let e = Estate::new();
        Estate::key(&e.member, "  sk-padded-WXYZ  ");

        let r = e.resolve_single_root();
        assert_eq!(r.credential_origin, ChatOrigin::Member);
        assert_eq!(r.api_key(), Some("sk-padded-WXYZ"));
        assert_eq!(r.credential.last4.as_deref(), Some("WXYZ"));
    }
}
