//! The configured production entry: resolve the effective wiki model, build the
//! real `rig` provider, and run the generation pass ([FR-WK-18], [FR-CF-07],
//! [ADR-42]).
//!
//! [`run_configured`] consumes the S-176 [`EffectiveWikiModel`] (produced by
//! [`Config::effective_wiki_model`](logos_core::config::Config::effective_wiki_model)):
//! `[wiki].model` if set, else `[chat].model`, with `provider`/`base_url`/the API
//! key inherited from `[chat]`/`secrets.toml`. It mirrors the chat surface's
//! provider bridge (`web/src/chat/configured.rs`): a missing model or key is the
//! honest **configure-first** state ([FR-UI-18], [NFR-CC-04]) — not a crash — and
//! the deterministic pre-send preflight ([FR-UI-24]) catches a malformed endpoint
//! before any connection opens. All agent logic lives in [`WikiAgent`] ([ADR-01]);
//! this only wires the config-resolved provider into it.
//!
//! [FR-WK-18]: ../../../docs/specs/requirements/FR-WK-18.md
//! [FR-CF-07]: ../../../docs/specs/requirements/FR-CF-07.md
//! [FR-UI-18]: ../../../docs/specs/requirements/FR-UI-18.md
//! [FR-UI-24]: ../../../docs/specs/requirements/FR-UI-24.md
//! [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
//! [ADR-01]: ../../../docs/specs/architecture/decisions/ADR-01.md
//! [ADR-42]: ../../../docs/specs/architecture/decisions/ADR-42.md

use std::sync::Arc;

use agent_core::{
    anthropic_completion_model, openai_compatible_completion_model, ProviderConfig, RetryPolicy,
};
use anyhow::Result;
use logos_core::config::{ChatProvider, EffectiveWikiModel};
use logos_core::Engine;

use crate::{WikiAgent, WikiProgress, WikiRunOutcome};

/// The result of a configured generation attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfiguredRun {
    /// No provider model and/or API key is set — the honest configure-first
    /// state ([FR-UI-18], [NFR-CC-04]). Carries the message the surface shows; no
    /// run started, no outbound call made.
    ConfigureFirst(String),
    /// A run was attempted. `None` when the work-list was empty (no run started);
    /// `Some` with the outcome when a run happened.
    Ran(Option<WikiRunOutcome>),
}

/// Run a generation pass over the effective wiki model ([FR-CF-07], [FR-WK-18]).
///
/// Resolves the provider from `effective`, applies the configure-first guard and
/// the pre-send preflight, then hands the constructed `rig` provider to a
/// [`WikiAgent`] whose system prompt is the embedded `logos-wiki` skill body
/// ([FR-WK-08]) and whose generator label is the resolved model id ([FR-WK-02]).
///
/// The first outbound call is the (consent-gated, [NFR-SE-07]) generation turn;
/// provider *construction* is egress-free. `budget` bounds the pass ([ADR-42]).
///
/// # Errors
/// Returns an `Err` on a malformed endpoint (preflight), a provider-client
/// construction failure, or an infrastructure failure inside the run
/// ([`WikiAgent::run`]). Configure-first and per-page/budget halts are **not**
/// errors — they surface via [`ConfiguredRun`] / the progress stream.
pub async fn run_configured(
    engine: Arc<Engine>,
    effective: EffectiveWikiModel,
    budget: usize,
    sink: impl Fn(WikiProgress),
) -> Result<ConfiguredRun> {
    // Configure-first ([FR-UI-18]): no resolved model id (neither `[wiki].model`
    // nor `[chat].model`) is not an error.
    let Some(model_id) = effective.model.clone() else {
        return Ok(ConfiguredRun::ConfigureFirst(
            "Wiki generation is not configured — choose a wiki or chat model in the \
             Config tab before generating."
                .to_string(),
        ));
    };
    // Likewise a missing/blank inherited key ([FR-CF-06]).
    let api_key = match effective.api_key.clone() {
        Some(key) if !key.trim().is_empty() => key,
        _ => {
            return Ok(ConfiguredRun::ConfigureFirst(
                "Wiki generation is not configured — add an API key in the Config tab \
                 before generating."
                    .to_string(),
            ))
        }
    };

    // The embedded skill body is the single source of page-shaping guidance
    // ([FR-WK-08]) — the wiki-agent's system prompt.
    let preamble = logos_core::wiki::rendered_skill();

    // Resolve the provider config (provider/base_url inherited from `[chat]`,
    // [FR-CF-07]), then run the deterministic pre-send preflight ([FR-UI-24]): a
    // malformed endpoint is an honest error naming the problem, never a crash or
    // an echoed key ([NFR-SE-07]).
    let cfg = match effective.provider {
        ChatProvider::Anthropic => ProviderConfig::anthropic(model_id.clone(), api_key),
        ChatProvider::OpenAi => {
            ProviderConfig::openai_compatible(model_id.clone(), api_key)
                .with_base_url(effective.base_url.clone())
        }
    };
    cfg.preflight()?;

    // The wiki-agent inherits the resolved `[chat]` provider-retry policy verbatim
    // ([CR-060], [S-240], [ADR-42]) — no separate wiki retry policy.
    let retry = RetryPolicy::new(
        effective.max_provider_retries,
        u64::from(effective.provider_retry_base_ms),
    );

    // Build the concrete provider model (egress-free construction) and run the
    // pass. The two providers are distinct concrete model types, so each arm
    // monomorphizes the generic runner; both run the same generation loop.
    let outcome = match effective.provider {
        ChatProvider::Anthropic => {
            let model = anthropic_completion_model(&cfg, retry)?;
            WikiAgent::new(model, preamble, model_id)
                .with_budget(budget)
                .run(engine, sink)
                .await?
        }
        ChatProvider::OpenAi => {
            let model = openai_compatible_completion_model(&cfg, retry)?;
            WikiAgent::new(model, preamble, model_id)
                .with_budget(budget)
                .run(engine, sink)
                .await?
        }
    };
    Ok(ConfiguredRun::Ran(outcome))
}
