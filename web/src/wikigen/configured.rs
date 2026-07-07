//! The production [`WikiRunService`]: run the CR-062 deterministic presented
//! tier ([`Engine::wiki_materialize`], [FR-WK-20]) ahead of the LLM queue, then
//! resolve the effective wiki model ([`[wiki].model`], else `[chat].model`,
//! inheriting provider/key from `[chat]`) and drive the [`wiki-agent`]
//! generation pass, streaming its [`WikiProgress`] ([S-178], [ADR-42],
//! [FR-WK-18], [FR-CF-07]).
//!
//! All generation logic lives in [`wiki-agent`]/[`agent-core`] ([ADR-01]); this
//! just materializes the presented tier, resolves the config-driven provider off
//! the blocking pool, and hands it to
//! [`run_configured`](wiki_agent::run_configured), forwarding its progress to the
//! surface's SSE channel. Materializing runs unconditionally — before the
//! configure-first check — so the Summary tier grounds on already-present
//! Design/Specs pages even when the LLM half of the run is unconfigured. A
//! missing wiki/chat model or API key is the honest **configure-first** state
//! ([FR-UI-18]) — a single frame, not a crash ([NFR-CC-04]); an empty work-list
//! starts no run ([`run_configured`](wiki_agent::run_configured) returns
//! `Ran(None)`), emitting no progress.
//!
//! # Blocking setup is offloaded ([ADR-03])
//! Reading `config.toml`/`secrets.toml` are synchronous filesystem operations; like
//! every other engine touch on the surface (and the chat service's `build_setup`,
//! [`crate::chat`]), they run on the blocking pool (`tokio::task::spawn_blocking`)
//! rather than the async I/O thread. The queue read and each `wiki write` inside
//! the pass are already offloaded by the runner itself.
//!
//! [S-178]: ../../../docs/planning/journal.md#s-178-wiki-tab-trigger-background-generation-sse-streaming-and-first-use-consent
//! [ADR-01]: ../../../docs/specs/architecture/decisions/ADR-01.md
//! [ADR-03]: ../../../docs/specs/architecture/decisions/ADR-03.md
//! [ADR-42]: ../../../docs/specs/architecture/decisions/ADR-42.md
//! [FR-WK-18]: ../../../docs/specs/requirements/FR-WK-18.md
//! [FR-UI-18]: ../../../docs/specs/requirements/FR-UI-18.md
//! [FR-CF-07]: ../../../docs/specs/requirements/FR-CF-07.md
//! [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
//! [`[wiki].model`]: ../../../docs/specs/requirements/FR-CF-07.md
//! [`wiki-agent`]: ../../../docs/specs/architecture/components/wiki-agent.md
//! [`agent-core`]: ../../../docs/specs/architecture/components/agent-core.md

use std::path::Path;
use std::sync::Arc;

use logos_core::config::{load_config_from_root, load_secrets_from_root, EffectiveWikiModel};
use logos_core::Engine;
use wiki_agent::{run_configured, ConfiguredRun, DEFAULT_RUN_BUDGET};

use super::{spawn_run, WikiRunGuard, WikiRunService, WikiSink};

/// The production wiki-generation service over the live [`Engine`] and the on-disk
/// `[wiki]`/`[chat]` policy + `secrets.toml` key ([FR-CF-07]).
pub(crate) struct ConfiguredWikiRunService {
    engine: Arc<Engine>,
}

impl ConfiguredWikiRunService {
    /// Build the service over the shared engine.
    pub(crate) fn new(engine: Arc<Engine>) -> Self {
        Self { engine }
    }
}

/// Resolve the effective wiki model from the on-disk policy — the **blocking** half
/// of a run's setup ([ADR-03]). Returns an honest setup-fault message on a
/// config/secret read failure ([NFR-CC-04]); a missing model/key is **not** decided
/// here — it is [`run_configured`](wiki_agent::run_configured)'s configure-first
/// state, so the resolution stays a pure read.
///
/// The **secrets** read fault is surfaced with a **fixed** message that never
/// interpolates the underlying error ([NFR-SE-07]): a `secrets.toml` TOML-parse
/// error's `Display` embeds a snippet of the offending input line, which could be
/// the `api_key = "…"` line — echoing it into the SSE `error` frame the UI renders
/// verbatim would leak the raw key. The `config.toml` read carries no secret, so its
/// detailed error is kept for diagnosability.
fn resolve_effective_model(root: &Path) -> Result<EffectiveWikiModel, String> {
    let config = load_config_from_root(root)
        .map_err(|e| format!("could not read the wiki config: {e}"))?;
    let secrets = load_secrets_from_root(root).map_err(|_| {
        "could not read the wiki secret — check that .logos/secrets.toml is valid TOML".to_string()
    })?;
    Ok(config.effective_wiki_model(&secrets))
}

impl WikiRunService for ConfiguredWikiRunService {
    fn start_run(&self, guard: WikiRunGuard, sink: WikiSink) {
        let engine = Arc::clone(&self.engine);
        let root = engine.root().to_path_buf();

        spawn_run(guard, sink, move |sink| async move {
            // The deterministic presented tier runs FIRST (FR-WK-20, FR-WK-18,
            // CR-062): in SRS mode this (re)assembles the Design/Specs pages from
            // `docs/specs/**` and sweeps reconciliation orphans before the LLM
            // queue is ever touched, so the Summary tier grounds on already-
            // present pages; outside SRS mode it is a no-op. Runs regardless of
            // whether a model/key is configured below — presentation is a pure
            // local-FS read + `wiki.db` write, no LLM/network ([NFR-SE-01]).
            {
                let engine = Arc::clone(&engine);
                match tokio::task::spawn_blocking(move || engine.wiki_materialize()).await {
                    Ok(Ok(_)) => {}
                    Ok(Err(e)) => {
                        sink.error(format!("wiki materialize failed: {e}"));
                        return;
                    }
                    Err(_join) => {
                        sink.error("the wiki materialize task failed unexpectedly");
                        return;
                    }
                }
            }

            // Blocking config/secret read off the async executor thread ([ADR-03]);
            // a read fault is an honest single `error` frame, never a crash
            // ([NFR-CC-04]).
            let effective =
                match tokio::task::spawn_blocking(move || resolve_effective_model(&root)).await {
                    Ok(Ok(effective)) => effective,
                    Ok(Err(message)) => {
                        sink.error(message);
                        return;
                    }
                    Err(_join) => {
                        sink.error("the wiki setup task failed unexpectedly");
                        return;
                    }
                };

            // Drive the runner, forwarding each per-page event onto the SSE channel.
            // `run_configured` owns the configure-first guard, the pre-send
            // preflight, provider construction, and the queue loop — the surface
            // holds no generation logic ([ADR-01]). The first outbound call is the
            // consent-gated generation turn ([NFR-SE-07]).
            match run_configured(engine, effective, DEFAULT_RUN_BUDGET, sink.as_progress_fn()).await
            {
                // Configure-first: no model/key resolved — the honest, no-egress
                // state the surface renders ([FR-UI-18], [NFR-CC-04]).
                Ok(ConfiguredRun::ConfigureFirst(message)) => sink.configure_first(message),
                // A run happened (or the work-list was empty, `Ran(None)`): the
                // per-page progress already streamed through the sink; nothing more
                // to emit.
                Ok(ConfiguredRun::Ran(_)) => {}
                // A malformed endpoint (preflight), a provider-client construction
                // failure, or an infrastructure fault inside the run — honest, never
                // a fabricated page ([NFR-CC-04]). The classified cause is carried
                // verbatim; the API key is never in it ([NFR-SE-07]).
                Err(e) => sink.error(format!("wiki generation failed: {e}")),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_effective_model;
    use tempfile::TempDir;

    /// [NFR-SE-07] regression guard: a malformed `secrets.toml` — whose raw TOML
    /// parse error would embed the offending `api_key` line — must surface a fixed
    /// fault message that never echoes the key. This locks the fix independently of
    /// the `toml` crate's error-snippet formatting.
    #[test]
    fn secrets_read_fault_never_echoes_the_key() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join(".logos")).unwrap();
        std::fs::write(
            dir.path().join(".logos/secrets.toml"),
            "[chat]\napi_key = \"sk-LEAKME-DEADBEEF\" not valid toml here\n",
        )
        .unwrap();

        let err = resolve_effective_model(dir.path())
            .expect_err("a malformed secrets.toml is a read fault");
        assert!(
            !err.contains("sk-LEAKME-DEADBEEF"),
            "the secrets read fault must never echo the key (NFR-SE-07): {err}",
        );
        assert!(
            err.contains("secrets.toml"),
            "the fault still names the offending file for the user: {err}",
        );
    }
}
