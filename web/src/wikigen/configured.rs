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
//! # The pass names its own surface ([FR-OB-13], [CR-139])
//! The materialize call below runs inside the `serve --ui` process, so without a
//! scope it inherits that process's surface and Logos's own generator is counted
//! as a developer browsing the dashboard. [`in_surface`] installs
//! [`Surface::WikiGen`] inside the `spawn_blocking`, where the engine — and so
//! the telemetry event — actually runs.
//!
//! **What the scope reaches, and what it does not.** It is thread-scoped, so it
//! covers exactly the engine call in the closure it wraps. The LLM half of the
//! run, [`run_configured`](wiki_agent::run_configured), owns its own
//! `spawn_blocking` hops inside `wiki-agent` (`wiki-agent/src/agent.rs` —
//! `wiki_generate`, `wiki_read`, and the per-page grounding/write calls), and
//! those threads carry no scope from here. `web/tests/wikigen_enumeration.rs`
//! records that boundary as a declared, classified site rather than leaving it
//! assumed.
//!
//! # The inherited chat halves come from the seam ([ADR-67], [ADR-42])
//! The wiki agent has no provider, endpoint or key of its own: it inherits them
//! from `[chat]`. Those inherited halves are read through
//! [`resolve_chat`] over the member root and the already-resolved federation
//! workspace root — the same resolution the chat turn and the tab read — so a
//! member that inherits its credential (or its whole chat policy) from the
//! workspace gets wiki generation as well as chat. The member's own
//! `[wiki].model` still wins over the effective chat model, exactly as before;
//! the `[wiki]` table itself is read from the member alone.
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
//! [ADR-67]: ../../../docs/specs/architecture/decisions/ADR-67.md
//! [FR-WK-18]: ../../../docs/specs/requirements/FR-WK-18.md
//! [FR-UI-18]: ../../../docs/specs/requirements/FR-UI-18.md
//! [FR-CF-07]: ../../../docs/specs/requirements/FR-CF-07.md
//! [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
//! [NFR-OO-02]: ../../../docs/specs/requirements/NFR-OO-02.md
//! [FR-OB-13]: ../../../docs/specs/requirements/FR-OB-13.md
//! [CR-139]: ../../../docs/requests/CR-139-the-wiki-generation-pass-names-its-own-surface.md
//! [`[wiki].model`]: ../../../docs/specs/requirements/FR-CF-07.md
//! [`wiki-agent`]: ../../../docs/specs/architecture/components/wiki-agent.md
//! [`agent-core`]: ../../../docs/specs/architecture/components/agent-core.md

use std::path::{Path, PathBuf};
use std::sync::Arc;

use logos_core::config::{load_config_from_root, resolve_chat, EffectiveWikiModel};
use logos_core::observability::{in_surface, Surface};
use logos_core::Engine;
use wiki_agent::{run_configured, ConfiguredRun, DEFAULT_RUN_BUDGET};

use super::{spawn_run, WikiRunGuard, WikiRunService, WikiSink};
use crate::chat::resolution_fault;

/// The production wiki-generation service over the live [`Engine`], the member's
/// `[wiki]` policy, and the `[chat]` policy + `secrets.toml` key resolved against
/// the member and workspace roots ([FR-CF-07], [ADR-67]).
pub(crate) struct ConfiguredWikiRunService {
    engine: Arc<Engine>,
    /// The federation's workspace root, or `None` in single-root mode — passed in
    /// from the backing that already resolved it, never discovered here.
    workspace_root: Option<PathBuf>,
}

impl ConfiguredWikiRunService {
    /// Build the service over the shared engine and the already-resolved workspace
    /// root (`None` under single-root backing, where no second tier is consulted).
    pub(crate) fn new(engine: Arc<Engine>, workspace_root: Option<PathBuf>) -> Self {
        Self {
            engine,
            workspace_root,
        }
    }
}

/// Resolve the effective wiki model — the member's `[wiki]` table over the chat
/// halves [`resolve_chat`] resolved — the **blocking** half of a run's setup
/// ([ADR-03]). Returns an honest setup-fault message on a config/secret read
/// failure ([NFR-CC-04]); a missing model/key is **not** decided here — it is
/// [`run_configured`](wiki_agent::run_configured)'s configure-first state, so the
/// resolution stays a pure read.
///
/// A **parse** fault of either file, at either root, is surfaced by file and
/// position only ([NFR-SE-07], [`resolution_fault`]): its `Display` embeds a
/// snippet of the offending input line, which could be an `api_key = "…"` line —
/// in `secrets.toml`, or pasted into `config.toml` by mistake — and echoing it into
/// the SSE `error` frame the UI renders verbatim would leak the raw key.
fn resolve_effective_model(
    root: &Path,
    workspace_root: Option<&Path>,
) -> Result<EffectiveWikiModel, String> {
    let config = load_config_from_root(root).map_err(|e| resolution_fault("wiki", &e))?;
    let chat = resolve_chat(root, workspace_root).map_err(|e| resolution_fault("wiki", &e))?;
    Ok(config.wiki.resolve_inherited(&chat))
}

impl WikiRunService for ConfiguredWikiRunService {
    fn start_run(&self, guard: WikiRunGuard, sink: WikiSink) {
        let engine = Arc::clone(&self.engine);
        let root = engine.root().to_path_buf();
        let workspace_root = self.workspace_root.clone();

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
                // The pass names its own surface ([FR-OB-13], [CR-139]). Entered
                // INSIDE the `spawn_blocking` closure, not around the `await`:
                // `in_surface` scopes per thread and the blocking pool is where
                // the engine — and so the telemetry event — actually runs. Same
                // shape as `crate::bridge` and `agent-core`'s `in_chat_surface`,
                // and for the same reason: resolution happens once at this
                // adapter boundary, never per engine call ([NFR-OO-02]).
                match tokio::task::spawn_blocking(move || {
                    in_surface(Surface::WikiGen, || engine.wiki_materialize())
                })
                .await
                {
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
            let effective = match tokio::task::spawn_blocking(move || {
                resolve_effective_model(&root, workspace_root.as_deref())
            })
            .await
            {
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
    use std::fs;
    use std::path::Path;

    use super::resolve_effective_model;
    use tempfile::TempDir;

    fn write(root: &Path, file: &str, body: &str) {
        fs::create_dir_all(root.join(".logos")).unwrap();
        fs::write(root.join(".logos").join(file), body).unwrap();
    }

    /// A workspace declaring the whole chat policy and the key, with an empty
    /// member nested at `<ws>/svc`.
    fn inheriting_estate() -> (TempDir, std::path::PathBuf) {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "config.toml",
            "[chat]\nprovider = \"anthropic\"\nmodel = \"workspace/chat\"\n\
             base_url = \"https://workspace.example/v1\"\nmax_provider_retries = 7\n\
             provider_retry_base_ms = 321\n",
        );
        write(
            tmp.path(),
            "secrets.toml",
            "[chat]\napi_key = \"sk-workspace-ws42\"\n",
        );
        let member = tmp.path().join("svc");
        fs::create_dir_all(member.join(".logos")).unwrap();
        (tmp, member)
    }

    /// [ADR-42] under [ADR-67]: a member inheriting its chat halves from the
    /// workspace gets wiki generation too — the effective model, provider,
    /// endpoint, retry policy and key are the workspace's.
    ///
    /// [ADR-42]: ../../../docs/specs/architecture/decisions/ADR-42.md
    /// [ADR-67]: ../../../docs/specs/architecture/decisions/ADR-67.md
    #[test]
    fn an_inheriting_member_resolves_the_workspace_chat_halves() {
        let (tmp, member) = inheriting_estate();
        let effective = resolve_effective_model(&member, Some(tmp.path())).expect("resolves");
        assert_eq!(effective.model.as_deref(), Some("workspace/chat"));
        assert_eq!(effective.api_key.as_deref(), Some("sk-workspace-ws42"));
        assert_eq!(effective.base_url, "https://workspace.example/v1");
        assert_eq!(effective.max_provider_retries, 7);
        // Both differ from the defaults, so neither can be satisfied by a table
        // that was not the workspace's.
        assert_eq!(
            effective.provider,
            logos_core::config::ChatProvider::Anthropic
        );
        assert_eq!(effective.provider_retry_base_ms, 321);
    }

    /// The member's own `[wiki].model` still wins over the (inherited) chat model;
    /// only the chat halves are two-tier.
    #[test]
    fn the_member_wiki_model_still_wins_over_the_inherited_chat_model() {
        let (tmp, member) = inheriting_estate();
        write(&member, "config.toml", "[wiki]\nmodel = \"member/wiki\"\n");
        let effective = resolve_effective_model(&member, Some(tmp.path())).expect("resolves");
        assert_eq!(effective.model.as_deref(), Some("member/wiki"));
        assert_eq!(
            effective.api_key.as_deref(),
            Some("sk-workspace-ws42"),
            "key inherited"
        );
    }

    /// HF-1 ([ADR-67] §2): wiki generation holds the same trust boundary as the
    /// turn. A member's own key never goes to the inherited workspace endpoint —
    /// the workspace key does, and with none there, no key at all — while a
    /// member owning its `[chat]` policy uses its own key.
    ///
    /// [ADR-67]: ../../../docs/specs/architecture/decisions/ADR-67.md
    #[test]
    fn a_member_key_never_reaches_the_inherited_workspace_endpoint() {
        let member_key = "[chat]\napi_key = \"sk-member-mb77\"\n";

        let (tmp, member) = inheriting_estate();
        write(&member, "secrets.toml", member_key);
        let effective = resolve_effective_model(&member, Some(tmp.path())).expect("resolves");
        assert_eq!(effective.base_url, "https://workspace.example/v1");
        assert_eq!(effective.api_key.as_deref(), Some("sk-workspace-ws42"));

        fs::remove_file(tmp.path().join(".logos/secrets.toml")).unwrap();
        let effective = resolve_effective_model(&member, Some(tmp.path())).expect("resolves");
        assert_eq!(effective.model.as_deref(), Some("workspace/chat"));
        assert_eq!(effective.api_key, None, "the withheld member key is not dialled");
        // …and the run's configure-first text is told why (the policy is inherited,
        // the member's key withheld), so it does not advise adding a key.
        assert!(effective.chat_policy_inherited && effective.member_key_withheld);

        write(&member, "config.toml", "[chat]\nmodel = \"member/chat\"\n");
        let effective = resolve_effective_model(&member, Some(tmp.path())).expect("resolves");
        assert_eq!(effective.api_key.as_deref(), Some("sk-member-mb77"));
        assert!(!effective.chat_policy_inherited && !effective.member_key_withheld);
    }

    /// Single-root (`None`): the enclosing workspace files are never consulted.
    #[test]
    fn single_root_consults_no_second_tier() {
        let (_tmp, member) = inheriting_estate();
        let effective = resolve_effective_model(&member, None).expect("resolves");
        assert_eq!(effective.model, None);
        assert_eq!(effective.api_key, None);
    }

    /// A key pasted into `config.toml` is never echoed by the wiki's parse fault —
    /// neither from the member's own `config.toml` nor from the workspace's.
    #[test]
    fn a_config_toml_parse_fault_never_echoes_a_pasted_key_at_either_root() {
        let pasted = "[chat]\nmodel = \"m\"\napi_key = \"sk-LEAKME-CONFIG\"\n";

        let (tmp, member) = inheriting_estate();
        write(&member, "config.toml", pasted);
        let member_fault = resolve_effective_model(&member, Some(tmp.path())).expect_err("fault");

        let (tmp2, member2) = inheriting_estate();
        write(tmp2.path(), "config.toml", pasted);
        let ws_fault = resolve_effective_model(&member2, Some(tmp2.path())).expect_err("fault");

        for (err, file) in [
            (&member_fault, member.join(".logos/config.toml")),
            (&ws_fault, tmp2.path().join(".logos/config.toml")),
        ] {
            assert!(
                !err.contains("sk-LEAKME-CONFIG"),
                "never echoes the key: {err}"
            );
            assert!(
                err.starts_with("could not read the wiki configuration"),
                "{err}"
            );
            assert!(
                err.contains(&file.display().to_string()),
                "names the file: {err}"
            );
        }
    }

    /// An invalid **workspace** `secrets.toml` the member relies on fails loud with
    /// the fixed message, naming the workspace file and never echoing the key.
    #[test]
    fn a_workspace_secrets_fault_never_echoes_the_key() {
        let (tmp, member) = inheriting_estate();
        write(
            tmp.path(),
            "secrets.toml",
            "[chat]\napi_key = \"sk-LEAKME-WORKSPACE\" not valid toml here\n",
        );
        let err = resolve_effective_model(&member, Some(tmp.path())).expect_err("a read fault");
        assert!(
            !err.contains("sk-LEAKME-WORKSPACE"),
            "never echoes the key: {err}"
        );
        assert!(
            err.contains(&tmp.path().join(".logos/secrets.toml").display().to_string()),
            "names the workspace file: {err}",
        );
    }

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

        let err = resolve_effective_model(dir.path(), None)
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
