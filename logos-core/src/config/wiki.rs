//! The `[wiki]` config section — a dedicated wiki generation model, inheriting
//! `provider`/`base_url`/the `secrets.toml` key from `[chat]` ([FR-CF-07],
//! [ADR-42]), plus the revision-stale re-queue cadence dampener
//! ([FR-WK-17]).
//!
//! `[wiki]` is an **optional** `config.toml` section parsed under the same
//! `#[serde(deny_unknown_fields)]` discipline as the rest of the policy
//! ([FR-CF-01]): its keys are optional, and an unknown key (or an out-of-range
//! value) fails loud at load (exit 2). There is **no** separate wiki
//! `provider`, endpoint, or secret — [`WikiConfig::resolve`] inherits all three
//! from [`ChatConfig`]/[`Secrets`] ([FR-CF-06]) verbatim, so configuring a wiki
//! model needs no additional secret handling.
//!
//! [FR-CF-01]: ../../../docs/specs/requirements/FR-CF-01.md
//! [FR-CF-06]: ../../../docs/specs/requirements/FR-CF-06.md
//! [FR-CF-07]: ../../../docs/specs/requirements/FR-CF-07.md
//! [FR-WK-17]: ../../../docs/specs/requirements/FR-WK-17.md
//! [ADR-42]: ../../../docs/specs/architecture/decisions/ADR-42.md

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::chat::{ChatConfig, ChatOrigin, ChatProvider, ChatResolution};
use super::error::ConfigError;
use super::load_config_from_root;
use super::secrets::{MaskedSecret, Secrets};

/// The default revision-stale re-queue threshold ([FR-WK-17]): an
/// already-regenerated structured section re-queues once the graph revision has
/// advanced this many revisions past its built-at revision. `5` batches a
/// handful of graph-revision advances between regenerations of the anchorless
/// prose pages (the Overview/architecture/consolidated-doc singletons) — the
/// same "bound the churn a fast-moving signal produces" shape the watcher's
/// debounce/settle knobs apply to sync cadence. `1` is the documented minimum,
/// at which the behavior reduces exactly to "re-queue whenever
/// revision-pending" ([FR-WK-17] AC).
fn default_revision_stale_threshold() -> u64 {
    5
}

/// The parsed `[wiki]` section: an optional `model` override ([FR-CF-07]) and
/// the revision-stale re-queue cadence dampener ([FR-WK-17]).
///
/// An absent `[wiki]` deserialises to [`WikiConfig::default`] (`model: None`,
/// `revision_stale_threshold: None`), which resolves to the chat model and the
/// default dampening threshold respectively.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WikiConfig {
    /// The model identifier for wiki page synthesis, distinct from
    /// [`ChatConfig::model`]. Optional — an absent value falls back to the chat
    /// model ([`resolve`](Self::resolve)), or, under an inherited workspace chat
    /// policy, first to the workspace root's `model`
    /// ([`resolve_inherited`](Self::resolve_inherited)).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// The revision-delta dampening threshold ([FR-WK-17]): an
    /// already-regenerated agent-tier structured section (the Overview
    /// singletons and consolidated documentation category pages, [FR-WK-06])
    /// is re-surfaced on the regeneration work-list/queue only once the
    /// current graph revision has advanced at least this many revisions past
    /// the section's built-at revision — bounding regeneration cost for
    /// anchorless prose pages that would otherwise re-queue on every single
    /// revision advance. Optional; an absent value resolves to
    /// [`default_revision_stale_threshold`] via
    /// [`effective_revision_stale_threshold`](Self::effective_revision_stale_threshold).
    /// `1` is the documented minimum: it reduces the behavior exactly to
    /// "re-queue whenever revision-pending" (the pre-dampening behavior).
    /// Never affects `revision_stale_count` reporting ([FR-WK-06]) — dampening
    /// governs re-queue cadence only, so the count stays truthful regardless
    /// of this setting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision_stale_threshold: Option<u64>,
}

impl WikiConfig {
    /// Validate the `[wiki]` section: a present `model` must be non-blank (a
    /// blank override resolves to nothing meaningful, mirroring
    /// [`ChatConfig::base_url`]'s non-empty check); a present
    /// `revision_stale_threshold` must be at least `1` (`0` would classify a
    /// section built at the *current* revision as revision-stale, since a
    /// zero-delta comparison is trivially satisfied — silently dishonest
    /// rather than "always re-queue"). Either violation is a load-time
    /// [`ConfigError::InvalidValue`] (exit 2, [FR-CF-07] AC: "an out-of-range
    /// value is rejected … with no partial write").
    pub(crate) fn validate(&self) -> Result<(), ConfigError> {
        if let Some(model) = &self.model {
            if model.trim().is_empty() {
                return Err(ConfigError::InvalidValue {
                    key: "wiki.model".to_string(),
                    message: "must not be empty".to_string(),
                });
            }
        }
        if self.revision_stale_threshold == Some(0) {
            return Err(ConfigError::InvalidValue {
                key: "wiki.revision_stale_threshold".to_string(),
                message: "must be at least 1 ([FR-WK-17])".to_string(),
            });
        }
        Ok(())
    }

    /// Resolve the effective wiki generation policy: [`WikiConfig::model`] if
    /// set, else [`ChatConfig::model`] ([FR-CF-07] AC) — plus the `provider`,
    /// `base_url`, and API key **inherited** from `chat`/`secrets` verbatim (no
    /// separate wiki provider, endpoint, or secret, [ADR-42]).
    pub fn resolve(&self, chat: &ChatConfig, secrets: &Secrets) -> EffectiveWikiModel {
        self.resolve_parts(chat, None, secrets.chat_api_key(), false, false)
    }

    /// [`resolve`](Self::resolve) over the chat policy and credential the
    /// [`resolve_chat`](super::resolve_chat) seam resolved ([ADR-67], [ADR-42]):
    /// the inherited provider, endpoint, retry policy and key are the **effective**
    /// chat halves — the member's own, or the workspace's where the member leaves
    /// that half undeclared, and the workspace's key alone under an inherited
    /// policy ([ADR-67] §2).
    ///
    /// `self` is the member's `[wiki]` table and `workspace` the workspace root's
    /// (`None` single-root). The model resolves in three steps (Sprint 77 HF-1):
    ///
    /// 1. the member's own [`WikiConfig::model`];
    /// 2. else the workspace's [`WikiConfig::model`] — **only** when the member
    ///    inherits the chat policy half ([`ChatOrigin::Workspace`]). A member that
    ///    declares its own `[chat] model` owns its endpoint, and a model named for
    ///    the workspace's endpoint may not exist at the member's, so it never
    ///    receives the workspace wiki model;
    /// 3. else the effective chat model.
    ///
    /// [ADR-67]: ../../../docs/specs/architecture/decisions/ADR-67.md
    pub fn resolve_inherited(
        &self,
        workspace: Option<&WikiConfig>,
        chat: &ChatResolution,
    ) -> EffectiveWikiModel {
        let policy_inherited = chat.policy_origin == ChatOrigin::Workspace;
        let inherited_model = workspace
            .filter(|_| policy_inherited)
            .and_then(|w| w.model.as_deref());
        self.resolve_parts(
            &chat.policy,
            inherited_model,
            chat.api_key(),
            policy_inherited,
            chat.member_key_withheld,
        )
    }

    /// [`resolve_inherited`](Self::resolve_inherited) with the workspace root's
    /// `[wiki]` table read from `workspace_root` — the already-resolved federation
    /// root, never discovered here. The workspace `config.toml` is read **only**
    /// when the member inherits the chat policy half, the one case its `[wiki]`
    /// can apply; otherwise this performs no read at all, so single-root (`None`)
    /// behaviour is byte-identical to [`resolve_inherited`](Self::resolve_inherited)
    /// over no workspace table ([ADR-52]).
    ///
    /// # Errors
    /// The [`ConfigError`] [`load_config_from_root`] raises for the workspace
    /// root's `config.toml`.
    ///
    /// [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
    pub fn resolve_in_workspace(
        &self,
        chat: &ChatResolution,
        workspace_root: Option<&Path>,
    ) -> Result<EffectiveWikiModel, ConfigError> {
        let workspace = match (chat.policy_origin, workspace_root) {
            (ChatOrigin::Workspace, Some(root)) => Some(load_config_from_root(root)?.wiki),
            _ => None,
        };
        Ok(self.resolve_inherited(workspace.as_ref(), chat))
    }

    /// The one spelling of the inheritance rule every resolver shares, so the
    /// single-root and two-tier paths cannot drift into two precedence rules.
    /// `inherited_model` is the workspace `[wiki].model`, already gated on an
    /// inherited chat policy by the caller; `None` on every single-root path.
    fn resolve_parts(
        &self,
        chat: &ChatConfig,
        inherited_model: Option<&str>,
        api_key: Option<&str>,
        chat_policy_inherited: bool,
        member_key_withheld: bool,
    ) -> EffectiveWikiModel {
        EffectiveWikiModel {
            model: self
                .model
                .clone()
                .or_else(|| inherited_model.map(str::to_string))
                .or_else(|| chat.model.clone()),
            provider: chat.provider,
            base_url: chat.base_url.clone(),
            api_key: api_key.map(str::to_string),
            max_provider_retries: chat.max_provider_retries,
            provider_retry_base_ms: chat.provider_retry_base_ms,
            chat_policy_inherited,
            member_key_withheld,
        }
    }

    /// The effective revision-stale re-queue threshold ([FR-WK-17]):
    /// [`revision_stale_threshold`](Self::revision_stale_threshold) if set,
    /// else [`default_revision_stale_threshold`]. [`validate`](Self::validate)
    /// has already rejected `0` at load, so this is always `>= 1`.
    pub fn effective_revision_stale_threshold(&self) -> u64 {
        self.revision_stale_threshold
            .unwrap_or_else(default_revision_stale_threshold)
    }
}

/// The resolved wiki generation policy ([FR-CF-07], [ADR-42]): the effective
/// model plus the `provider`/`base_url`/key **inherited** from `[chat]`/
/// `secrets.toml` verbatim — the input the wiki-agent's provider construction
/// needs, with no separate wiki provider/endpoint/secret to resolve.
///
/// Carries the raw API key ([`api_key`](Self::api_key)) at rest for the
/// wiki-agent's egress call — so, like [`Secrets`], its `Debug` is
/// **hand-written** to redact it ([NFR-SE-07]); it is not [`Serialize`], so it
/// can never be JSON-encoded into a response either.
///
/// [NFR-SE-07]: ../../../docs/specs/requirements/NFR-SE-07.md
#[derive(Clone, PartialEq, Eq)]
pub struct EffectiveWikiModel {
    /// The resolved model id, or `None` when no `[wiki].model` applies and no
    /// effective `[chat].model` is set (the configure-first state).
    pub model: Option<String>,
    /// The provider family, inherited from [`ChatConfig::provider`].
    pub provider: ChatProvider,
    /// The OpenAI-compatible endpoint, inherited from [`ChatConfig::base_url`].
    pub base_url: String,
    /// The chat API key, inherited from `secrets.toml`; `None` when unset.
    pub api_key: Option<String>,
    /// The bounded provider-retry count, inherited from
    /// [`ChatConfig::max_provider_retries`] ([CR-060], [S-240]): the wiki-agent
    /// reuses the resolved chat retry policy — there is no separate wiki policy.
    ///
    /// [CR-060]: ../../../docs/requests/CR-060-chat-resilience-recoverable-faults.md
    pub max_provider_retries: u32,
    /// The provider-retry base backoff in milliseconds, inherited from
    /// [`ChatConfig::provider_retry_base_ms`] ([CR-060], [S-240]).
    pub provider_retry_base_ms: u32,
    /// The `[chat]` policy is inherited from the workspace root
    /// ([`ChatOrigin::Workspace`]), so its endpoint takes the workspace key alone
    /// ([ADR-67] §2) — the configure-first text then names the workspace root,
    /// never a member key, as the missing key's source. `false` single-root.
    ///
    /// [ADR-67]: ../../../docs/specs/architecture/decisions/ADR-67.md
    pub chat_policy_inherited: bool,
    /// The member's own key is withheld from that endpoint
    /// ([`ChatResolution::member_key_withheld`]).
    pub member_key_withheld: bool,
}

/// `Debug` that **redacts** the key ([NFR-SE-07]) — presence + last-4 only, the
/// same defense [`ChatSecrets`](super::secrets::ChatSecrets) uses.
impl std::fmt::Debug for EffectiveWikiModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EffectiveWikiModel")
            .field("model", &self.model)
            .field("provider", &self.provider)
            .field("base_url", &self.base_url)
            .field("api_key", &MaskedSecret::from_key(self.api_key.as_deref()))
            .field("max_provider_retries", &self.max_provider_retries)
            .field("provider_retry_base_ms", &self.provider_retry_base_ms)
            .field("chat_policy_inherited", &self.chat_policy_inherited)
            .field("member_key_withheld", &self.member_key_withheld)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    /// [FR-CF-07] AC: an absent `[wiki]` section is all-defaults (`model: None`).
    #[test]
    fn absent_wiki_section_is_all_defaults() {
        let cfg: Config = toml::from_str("").unwrap();
        assert_eq!(cfg.wiki, WikiConfig::default());
        assert!(cfg.wiki.model.is_none());
        assert!(cfg.wiki.revision_stale_threshold.is_none());
    }

    /// [FR-WK-17] AC: an absent `revision_stale_threshold` resolves to the
    /// documented non-trivial default, not the minimum.
    #[test]
    fn effective_revision_stale_threshold_defaults_to_five() {
        assert_eq!(WikiConfig::default().effective_revision_stale_threshold(), 5);
    }

    /// An explicit `revision_stale_threshold` wins over the default.
    #[test]
    fn effective_revision_stale_threshold_honors_explicit_value() {
        let cfg = WikiConfig {
            revision_stale_threshold: Some(12),
            ..Default::default()
        };
        assert_eq!(cfg.effective_revision_stale_threshold(), 12);
    }

    /// [FR-WK-17] AC: `0` is rejected — it would make a page built at the
    /// *current* revision read revision-stale (a zero-delta trivially satisfies
    /// `>= 0`), which is dishonest, not merely "no dampening".
    #[test]
    fn zero_revision_stale_threshold_is_rejected() {
        let cfg = WikiConfig {
            revision_stale_threshold: Some(0),
            ..Default::default()
        };
        assert!(matches!(
            cfg.validate(),
            Err(ConfigError::InvalidValue { ref key, .. }) if key == "wiki.revision_stale_threshold"
        ));
    }

    /// [FR-WK-17] AC: `1`, the documented minimum, validates — it is the value
    /// that reduces dampening to "re-queue whenever revision-pending".
    #[test]
    fn minimum_revision_stale_threshold_of_one_validates() {
        let cfg = WikiConfig {
            revision_stale_threshold: Some(1),
            ..Default::default()
        };
        assert!(cfg.validate().is_ok());
        assert_eq!(cfg.effective_revision_stale_threshold(), 1);
    }

    /// `#[serde(deny_unknown_fields)]`: an unknown `[wiki]` key fails loud
    /// ([FR-CF-07] AC, the [FR-CF-01] discipline).
    #[test]
    fn unknown_wiki_key_is_rejected() {
        let err = toml::from_str::<Config>("[wiki]\nbogus = 1\n").unwrap_err();
        assert!(
            err.to_string().contains("bogus") || err.to_string().contains("unknown"),
            "unknown key error should name the field: {err}"
        );
    }

    /// A blank `model` is a load-time `InvalidValue` (exit 2) naming
    /// `wiki.model` ([FR-CF-07] AC: "an out-of-range value is rejected").
    #[test]
    fn blank_model_is_rejected() {
        let cfg = WikiConfig {
            model: Some("   ".to_string()),
            ..Default::default()
        };
        assert!(matches!(
            cfg.validate(),
            Err(ConfigError::InvalidValue { ref key, .. }) if key == "wiki.model"
        ));
    }

    /// A non-blank `model` validates.
    #[test]
    fn non_blank_model_passes() {
        let cfg = WikiConfig {
            model: Some("anthropic/claude-haiku".to_string()),
            ..Default::default()
        };
        assert!(cfg.validate().is_ok());
    }

    /// [FR-CF-07] AC: explicit `[wiki].model` wins over `[chat].model`, and the
    /// two stay independent.
    #[test]
    fn resolve_prefers_explicit_wiki_model() {
        let wiki = WikiConfig {
            model: Some("wiki/model".to_string()),
            ..Default::default()
        };
        let chat = ChatConfig {
            model: Some("chat/model".to_string()),
            ..Default::default()
        };
        let resolved = wiki.resolve(&chat, &Secrets::default());
        assert_eq!(resolved.model.as_deref(), Some("wiki/model"));
    }

    /// [FR-CF-07] AC: an omitted `[wiki].model` falls back to `[chat].model`.
    #[test]
    fn resolve_falls_back_to_chat_model_when_wiki_model_absent() {
        let wiki = WikiConfig::default();
        let chat = ChatConfig {
            model: Some("chat/model".to_string()),
            ..Default::default()
        };
        let resolved = wiki.resolve(&chat, &Secrets::default());
        assert_eq!(resolved.model.as_deref(), Some("chat/model"));
    }

    /// With neither `[wiki].model` nor `[chat].model` set, resolution is `None`
    /// (the configure-first state).
    #[test]
    fn resolve_is_none_when_neither_model_is_set() {
        let resolved = WikiConfig::default().resolve(&ChatConfig::default(), &Secrets::default());
        assert!(resolved.model.is_none());
    }

    /// [FR-CF-07] AC: the wiki always reuses the `[chat]` `provider`/`base_url`
    /// and the `secrets.toml` key — there is no separate wiki provider,
    /// endpoint, or secret.
    #[test]
    fn resolve_inherits_provider_base_url_and_key_from_chat() {
        let wiki = WikiConfig {
            model: Some("wiki/model".to_string()),
            ..Default::default()
        };
        let chat = ChatConfig {
            provider: ChatProvider::Anthropic,
            base_url: "https://api.anthropic.com".to_string(),
            ..Default::default()
        };
        let mut secrets = Secrets::default();
        secrets.chat.api_key = Some("sk-secret-1234".to_string());

        let resolved = wiki.resolve(&chat, &secrets);
        assert_eq!(resolved.provider, ChatProvider::Anthropic);
        assert_eq!(resolved.base_url, "https://api.anthropic.com");
        assert_eq!(resolved.api_key.as_deref(), Some("sk-secret-1234"));
    }

    /// [CR-060]/[S-240] AC: the wiki-agent inherits the resolved `[chat]`
    /// provider-retry policy — there is no separate wiki retry policy.
    #[test]
    fn resolve_inherits_the_provider_retry_policy_from_chat() {
        let wiki = WikiConfig {
            model: Some("wiki/model".to_string()),
            ..Default::default()
        };
        let chat = ChatConfig {
            max_provider_retries: 4,
            provider_retry_base_ms: 125,
            ..Default::default()
        };
        let resolved = wiki.resolve(&chat, &Secrets::default());
        assert_eq!(resolved.max_provider_retries, 4);
        assert_eq!(resolved.provider_retry_base_ms, 125);
    }

    // ── The two-tier `[wiki].model` rule (Sprint 77 HF-1, [ADR-67]) ──────────

    /// A workspace root declaring a whole `[chat]` table, a key and a
    /// `[wiki].model`, with one member nested inside it — the real layout, so a
    /// resolution that walked up the tree would find the tier.
    struct Estate {
        _dir: tempfile::TempDir,
        workspace: std::path::PathBuf,
        member: std::path::PathBuf,
    }

    const WS_CHAT_MODEL: &str = "workspace/chat";
    const WS_WIKI_MODEL: &str = "workspace/wiki";
    const WS_BASE_URL: &str = "https://workspace.example/v1";

    impl Estate {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let workspace = dir.path().to_path_buf();
            let member = workspace.join("svc");
            std::fs::create_dir_all(member.join(".logos")).unwrap();
            let e = Estate { _dir: dir, workspace, member };
            e.workspace_config(&format!(
                "[chat]\nmodel = \"{WS_CHAT_MODEL}\"\nbase_url = \"{WS_BASE_URL}\"\n\
                 [wiki]\nmodel = \"{WS_WIKI_MODEL}\"\n"
            ));
            std::fs::write(
                e.workspace.join(".logos/secrets.toml"),
                "[chat]\napi_key = \"sk-workspace-ws42\"\n",
            )
            .unwrap();
            e
        }

        fn workspace_config(&self, text: &str) {
            std::fs::create_dir_all(self.workspace.join(".logos")).unwrap();
            std::fs::write(self.workspace.join(".logos/config.toml"), text).unwrap();
        }

        fn member_config(&self, text: &str) {
            std::fs::write(self.member.join(".logos/config.toml"), text).unwrap();
        }

        /// The production composition: the member's `[wiki]` over the chat
        /// resolution, with the workspace root handed in (or not).
        fn resolve(&self, workspace_root: Option<&std::path::Path>) -> EffectiveWikiModel {
            let member = crate::config::load_config_from_root(&self.member).unwrap().wiki;
            let chat = crate::config::resolve_chat(&self.member, workspace_root).unwrap();
            member.resolve_in_workspace(&chat, workspace_root).unwrap()
        }
    }

    /// (1) The member's own `[wiki].model` wins over the workspace's, even under
    /// an inherited chat policy.
    #[test]
    fn the_member_wiki_model_wins_over_the_workspace_wiki_model() {
        let e = Estate::new();
        e.member_config("[wiki]\nmodel = \"member/wiki\"\n");
        let r = e.resolve(Some(&e.workspace));
        assert_eq!(r.model.as_deref(), Some("member/wiki"));
        assert_eq!(r.base_url, WS_BASE_URL, "the endpoint is still the inherited one");
    }

    /// (2) A member inheriting the chat policy half inherits the workspace
    /// `[wiki].model` — over the effective chat model — and dials the
    /// workspace endpoint with the workspace key.
    #[test]
    fn a_member_inheriting_the_chat_policy_inherits_the_workspace_wiki_model() {
        let e = Estate::new();
        let r = e.resolve(Some(&e.workspace));
        assert_eq!(r.model.as_deref(), Some(WS_WIKI_MODEL));
        assert_eq!(r.base_url, WS_BASE_URL);
        assert_eq!(r.api_key.as_deref(), Some("sk-workspace-ws42"));
        assert!(r.chat_policy_inherited);
    }

    /// (3) A member owning its `[chat]` model owns its endpoint, so it must NOT
    /// receive the workspace wiki model (a model named for the workspace's
    /// endpoint may not exist at the member's): its wiki model is its own chat
    /// model.
    #[test]
    fn a_member_owning_its_chat_policy_never_receives_the_workspace_wiki_model() {
        let e = Estate::new();
        e.member_config(
            "[chat]\nmodel = \"member/chat\"\nbase_url = \"https://member.example/v1\"\n",
        );
        let r = e.resolve(Some(&e.workspace));
        assert_eq!(r.model.as_deref(), Some("member/chat"));
        assert_eq!(r.base_url, "https://member.example/v1");
        assert!(!r.chat_policy_inherited);
    }

    /// (4) Neither root declares a `[wiki].model`: the effective chat model —
    /// here the inherited workspace one.
    #[test]
    fn with_no_wiki_model_anywhere_the_effective_chat_model_is_used() {
        let e = Estate::new();
        e.workspace_config(&format!("[chat]\nmodel = \"{WS_CHAT_MODEL}\"\n"));
        let r = e.resolve(Some(&e.workspace));
        assert_eq!(r.model.as_deref(), Some(WS_CHAT_MODEL));
    }

    /// Neither root declares a `[chat] model` (policy `unset`): the workspace
    /// `[wiki].model` is not inherited — the rule is keyed on an inherited
    /// policy, not on an absent member one.
    #[test]
    fn an_unset_chat_policy_does_not_inherit_the_workspace_wiki_model() {
        let e = Estate::new();
        e.workspace_config(&format!("[wiki]\nmodel = \"{WS_WIKI_MODEL}\"\n"));
        let r = e.resolve(Some(&e.workspace));
        assert_eq!(r.model, None);
    }

    /// (5) No workspace root ([ADR-52]): the enclosing tier is never consulted —
    /// the result equals the single-root resolver's over the member alone.
    ///
    /// [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
    #[test]
    fn without_a_workspace_root_the_resolution_is_the_single_root_one() {
        for member in ["", "[chat]\nmodel = \"member/chat\"\n", "[wiki]\nmodel = \"member/wiki\"\n"] {
            let e = Estate::new();
            e.member_config(member);
            let config = crate::config::load_config_from_root(&e.member).unwrap();
            let secrets = crate::config::load_secrets_from_root(&e.member).unwrap();
            assert_eq!(e.resolve(None), config.effective_wiki_model(&secrets), "{member:?}");
        }
    }

    /// [`WikiConfig::resolve_inherited`] holds the policy gate on its own, not only
    /// through [`WikiConfig::resolve_in_workspace`]'s read gate: handed a
    /// workspace table directly, it applies it under an inherited policy and
    /// ignores it under an owned or unset one.
    #[test]
    fn resolve_inherited_applies_a_workspace_table_only_under_an_inherited_policy() {
        let workspace = WikiConfig {
            model: Some(WS_WIKI_MODEL.to_string()),
            ..Default::default()
        };
        for (member, expected) in [
            ("", Some(WS_WIKI_MODEL)),
            ("[chat]\nmodel = \"member/chat\"\n", Some("member/chat")),
        ] {
            let e = Estate::new();
            e.member_config(member);
            let chat = crate::config::resolve_chat(&e.member, Some(&e.workspace)).unwrap();
            let r = WikiConfig::default().resolve_inherited(Some(&workspace), &chat);
            assert_eq!(r.model.as_deref(), expected, "{member:?}");
        }
        let e = Estate::new();
        e.workspace_config("");
        let unset = crate::config::resolve_chat(&e.member, Some(&e.workspace)).unwrap();
        assert_eq!(unset.policy_origin, ChatOrigin::Unset);
        let r = WikiConfig::default().resolve_inherited(Some(&workspace), &unset);
        assert_eq!(r.model, None, "an unset policy inherits no workspace wiki model");
    }

    /// The workspace `config.toml` is read only when its `[wiki]` can apply: a
    /// member owning its policy resolves even over a workspace file that no
    /// longer parses, because nothing reads it.
    #[test]
    fn the_workspace_config_is_not_read_unless_the_policy_is_inherited() {
        let e = Estate::new();
        e.member_config("[chat]\nmodel = \"member/chat\"\n");
        let chat = crate::config::resolve_chat(&e.member, Some(&e.workspace)).unwrap();
        e.workspace_config("[wiki]\nmodle = 1\n");
        let r = WikiConfig::default()
            .resolve_in_workspace(&chat, Some(&e.workspace))
            .expect("no read of the broken workspace file");
        assert_eq!(r.model.as_deref(), Some("member/chat"));
    }

    /// [NFR-SE-07]: `Debug` never renders the raw key, only presence + last-4.
    #[test]
    fn debug_never_leaks_the_raw_key() {
        let resolved = EffectiveWikiModel {
            model: Some("wiki/model".to_string()),
            provider: ChatProvider::OpenAi,
            base_url: "https://openrouter.ai/api/v1".to_string(),
            api_key: Some("sk-secret-DEADBEEF".to_string()),
            max_provider_retries: 2,
            provider_retry_base_ms: 200,
            chat_policy_inherited: false,
            member_key_withheld: false,
        };
        let dbg = format!("{resolved:?}");
        assert!(
            !dbg.contains("sk-secret-DEADBEEF"),
            "Debug must never render the raw key (NFR-SE-07): {dbg}"
        );
        assert!(dbg.contains("BEEF"), "Debug shows last-4 for diagnosability: {dbg}");
    }
}
