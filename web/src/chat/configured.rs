//! The production [`ChatService`]: resolve the `[chat]` provider + key, build the
//! orchestrator over the real `rig` provider, and stream the turn ([S-170],
//! [ADR-40], [ADR-41], [FR-CF-06]).
//!
//! All agent logic lives in [`chat-agent`]/[`agent-core`] ([ADR-01]); this just
//! wires the config-resolved provider, the per-thread memory, and the sandbox into
//! the orchestrator, then hands off to the shared
//! [`run_orchestrated`](super::run_orchestrated) machinery. A missing provider
//! model or API key is the **configure-first** state ([FR-UI-18]): an honest
//! single-frame stream, not a crash ([NFR-CC-04]).
//!
//! # One seam, two readers ([ADR-67])
//! The policy and credential come from [`resolve_chat`] over the member root and
//! the already-resolved federation workspace root — the same resolution the config
//! read-model serializes for the tab — and the verdict is
//! [`turn_provider`](super::turn_provider)'s reading of its two origins. The turn
//! path does no model-or-key check of its own, so a member that inherits either
//! half from the workspace produces a turn, and the tab and the turn cannot
//! disagree about whether chat is usable.
//!
//! # One codebase, under any backing ([S-481], [ADR-71])
//! This is the member chat, and its roster is single-backing only: a service
//! built for a workspace member registers exactly the tools and preambles a
//! single root's does. Cross-service reach belongs to the workspace roster, which
//! the workspace chat serves on its own route (S-482).
//!
//! # Blocking setup is offloaded ([ADR-03])
//! Reading `config.toml`/`secrets.toml`, opening the `chat.db` stores, and walking
//! the sandbox root are synchronous filesystem/SQLite operations. Like every other
//! engine touch on the surface, they run on the blocking pool
//! (`tokio::task::spawn_blocking`) rather than the async I/O thread; only the
//! async orchestrator run stays on the runtime.
//!
//! [S-170]: ../../../docs/planning/journal.md#s-170-sse-streaming-and-intent-guarded-chat-post-routes
//! [ADR-01]: ../../../docs/specs/architecture/decisions/ADR-01.md
//! [ADR-03]: ../../../docs/specs/architecture/decisions/ADR-03.md
//! [ADR-40]: ../../../docs/specs/architecture/decisions/ADR-40.md
//! [ADR-41]: ../../../docs/specs/architecture/decisions/ADR-41.md
//! [ADR-67]: ../../../docs/specs/architecture/decisions/ADR-67.md
//! [ADR-71]: ../../../docs/specs/architecture/decisions/ADR-71.md
//! [S-481]: ../../../docs/planning/journal.md#s-481-a-workspace-roster-centred-on-the-workspace-and-the-member-roster-single-backing-only
//! [FR-CF-06]: ../../../docs/specs/requirements/FR-CF-06.md
//! [FR-UI-18]: ../../../docs/specs/requirements/FR-UI-18.md
//! [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
//! [`chat-agent`]: ../../../docs/specs/architecture/components/chat-agent.md
//! [`agent-core`]: ../../../docs/specs/architecture/components/agent-core.md

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent_core::rig::completion::CompletionModel;
use agent_core::{
    anthropic_completion_model, openai_compatible_completion_model, ProviderConfig, RetryPolicy,
};
use agent_core::Sandbox;
use chat_agent::{
    thread_window, BudgetTree, ChatRole, ChatStore, ConversationWindow, MemoryGrounding,
    MemoryStore, Orchestrator, Planner, SubagentRoster, SynthesizerGrounding,
};
use logos_core::config::{resolve_chat, ChatConfig, ChatProvider};
use logos_core::Engine;
use tokio::sync::mpsc::UnboundedSender;

use super::{
    resolution_fault, run_orchestrated, turn_provider, unbounded_chat_channel, ChatFrame,
    ChatService, ChatStream, TurnProvider, TurnTarget,
};

/// The production chat service over the live [`Engine`] and the `[chat]` policy +
/// `secrets.toml` key resolved against the member and workspace roots ([FR-CF-06],
/// [ADR-67]).
pub(crate) struct ConfiguredChatService {
    engine: Arc<Engine>,
    /// The federation's workspace root, or `None` in single-root mode — passed in
    /// from the backing that already resolved it, never discovered here.
    workspace_root: Option<PathBuf>,
}

impl ConfiguredChatService {
    /// Build the service over the shared engine and the already-resolved workspace
    /// root (`None` under single-root backing, where no second tier is consulted).
    pub(crate) fn new(engine: Arc<Engine>, workspace_root: Option<PathBuf>) -> Self {
        Self {
            engine,
            workspace_root,
        }
    }
}

/// The blocking-acquired pieces of a turn's setup that both chats share — the
/// member chat here and the workspace chat ([`super::workspace`], S-482):
/// produced on the blocking pool ([ADR-03]) and consumed by the async
/// orchestrator run. One shape and one constructor ([`prepare_turn`]), so the two
/// chats cannot drift on how a conversation is opened, recorded and bounded.
pub(super) struct TurnSetup {
    /// The root whose `.logos/chat.db` holds the conversation — where the
    /// question was recorded, and so where the scratchpad and the durable answer
    /// go ([`into_run`](Self::into_run)): one value, so the two cannot diverge.
    pub(super) store_root: PathBuf,
    pub(super) memory: Arc<MemoryStore>,
    pub(super) thread_id: i64,
    pub(super) turn: i64,
    pub(super) provider: ChatProvider,
    pub(super) model_id: String,
    pub(super) api_key: String,
    pub(super) base_url: String,
    pub(super) budget: BudgetTree,
    pub(super) temperature: Option<f64>,
    pub(super) max_tokens: Option<u64>,
    pub(super) retry: RetryPolicy,
    /// The thread's bounded prior turns, read before this turn's question was
    /// appended ([S-483]); empty on a thread's first turn.
    ///
    /// [S-483]: ../../../docs/planning/journal.md#s-483-follow-up-turns-see-prior-turns
    pub(super) history: ConversationWindow,
}

impl TurnSetup {
    /// Split the setup into what the roster runs on and what dials the provider,
    /// grounding the Synthesizer on the turn's memory and aiming the scratchpad
    /// and durable answer at the store the setup recorded the question in
    /// ([FR-UI-26] AC-2).
    ///
    /// [FR-UI-26]: ../../../docs/specs/requirements/FR-UI-26.md
    pub(super) fn into_run(self, question: String) -> (TurnRun, Dial) {
        let TurnSetup {
            store_root,
            memory,
            thread_id,
            turn,
            provider,
            model_id,
            api_key,
            base_url,
            budget,
            temperature,
            max_tokens,
            retry,
            history,
        } = self;
        let grounding: Arc<dyn SynthesizerGrounding> =
            Arc::new(MemoryGrounding::new(Arc::clone(&memory), thread_id, turn));
        let target = TurnTarget::new(store_root, thread_id, turn);
        (
            TurnRun { grounding, budget, temperature, max_tokens, question, history, memory, target },
            Dial { provider, model_id, api_key, base_url, retry },
        )
    }
}

/// What a configured turn dials: the provider family, its model and key, the
/// endpoint, and the bounded retry policy.
pub(super) struct Dial {
    provider: ChatProvider,
    model_id: String,
    api_key: String,
    base_url: String,
    retry: RetryPolicy,
}

/// The member chat's setup: the shared [`TurnSetup`] plus the member's source
/// sandbox.
struct ChatSetup {
    sandbox: Arc<Sandbox>,
    turn: TurnSetup,
}

/// Resolve the policy + credential through the seam, open the source sandbox,
/// resolve/create the thread, and open memory — the **blocking** half of a
/// turn's setup. Returns an honest configure-first / setup-fault message on
/// failure ([NFR-CC-04]); runs on the blocking pool ([ADR-03]).
///
/// The configure-first verdict and the sandbox (with its declared read roots)
/// are checked **before** any store is touched, so a refused turn records no
/// thread and no message.
fn build_setup(
    root: &Path,
    workspace_root: Option<&Path>,
    thread_id: Option<i64>,
    question: &str,
) -> Result<ChatSetup, String> {
    let resolution =
        resolve_chat(root, workspace_root).map_err(|e| resolution_fault("chat", &e))?;
    // Configure-first ([FR-UI-18]): an unset half is not an error, and the verdict
    // is the seam's origins — the same facts the tab reads ([ADR-67] §6).
    let provider = turn_provider(root, workspace_root, &resolution)?;

    // `[chat] read_roots` travel with the policy table, so they resolve against
    // the root that declared that table: the workspace root for an inherited
    // policy, else this member (sprint-79 HF-1) — the rule the workspace chat's
    // addressed source tools read through too. A missing entry fails the turn
    // by name — reported, never dropped — and, like the configure-first
    // verdict, before any store is touched, so it records no orphan thread.
    let sandbox = Arc::new(
        Sandbox::from_root(root)
            .and_then(|sandbox| {
                Ok(sandbox.with_chat_read_roots(root, workspace_root, &resolution)?)
            })
            .map_err(|e| format!("could not open the source sandbox: {e}"))?,
    );

    let turn = prepare_turn(root, provider, resolution.policy, thread_id, question)?;
    Ok(ChatSetup { sandbox, turn })
}

/// Open the conversation a turn appends to under `store_root`'s
/// `.logos/chat.db` — the thread (a new one when `thread_id` is `None`), its
/// bounded prior-turn window, the durable user message and the turn's memory —
/// and carry `chat`'s budget, sampling and retry keys beside the provider. The
/// part of a turn's setup both chats share; the caller has already passed the
/// configure-first verdict, so this is where the first store is touched.
pub(super) fn prepare_turn(
    store_root: &Path,
    provider: TurnProvider,
    chat: ChatConfig,
    thread_id: Option<i64>,
    question: &str,
) -> Result<TurnSetup, String> {
    let TurnProvider { model_id, api_key } = provider;
    // The thread the turn appends to (a new one when the caller gave none); the
    // scratchpad's foreign key requires the thread to exist first.
    let mut store =
        ChatStore::open(store_root).map_err(|e| format!("could not open the chat store: {e}"))?;
    // The prior turns come from the thread store, read BEFORE this turn's question
    // is appended so the window never contains the question it is answering
    // ([S-483]). A new thread has none; a deleted one has no messages either.
    let (thread_id, history) = match thread_id {
        Some(id) => {
            let window = thread_window(
                &store,
                id,
                question,
                chat.history_max_turns as usize,
                chat.history_max_chars as usize,
            )
            .map_err(|e| format!("could not read the conversation history: {e}"))?;
            (id, window)
        }
        None => (
            store
                .create_thread_from_message(question)
                .map_err(|e| format!("could not create a chat thread: {e}"))?,
            ConversationWindow::default(),
        ),
    };
    // The question is durable from the start; its ANSWER is appended by
    // `run_orchestrated` once the turn genuinely produces one, so a restored
    // conversation replays both halves ([FR-UI-26] AC-2).
    store
        .append_message(thread_id, ChatRole::User, question, &[])
        .map_err(|e| format!("could not record the user message: {e}"))?;
    drop(store);

    let memory = Arc::new(
        MemoryStore::open(store_root).map_err(|e| format!("could not open chat memory: {e}"))?,
    );
    let turn = memory
        .next_turn(thread_id)
        .map_err(|e| format!("could not compute the turn ordinal: {e}"))?;

    let budget = BudgetTree::from(&chat);
    let temperature = chat.temperature;
    let max_tokens = chat.max_tokens.map(u64::from);
    // The bounded provider-retry policy the model decorator applies ([CR-060],
    // [S-240]); the wiki-agent inherits the same resolved keys.
    let retry = RetryPolicy::new(
        chat.max_provider_retries,
        u64::from(chat.provider_retry_base_ms),
    );

    Ok(TurnSetup {
        store_root: store_root.to_path_buf(),
        memory,
        thread_id,
        turn,
        provider: chat.provider,
        model_id,
        api_key,
        base_url: chat.base_url,
        budget,
        temperature,
        max_tokens,
        retry,
        history,
    })
}

impl ChatService for ConfiguredChatService {
    fn start_turn(&self, question: String, thread_id: Option<i64>) -> ChatStream {
        let engine = Arc::clone(&self.engine);
        let setup_root = engine.root().to_path_buf();
        let workspace_root = self.workspace_root.clone();
        let setup_question = question.clone();
        spawn_configured_turn(question, move || {
            let ChatSetup { sandbox, turn } = build_setup(
                &setup_root,
                workspace_root.as_deref(),
                thread_id,
                &setup_question,
            )?;
            Ok((turn, MemberRoster { engine, sandbox }))
        })
    }
}

/// What a configured turn hands its roster once the provider model is built —
/// everything [`launch`] needs beside the model and the roster's own backing.
pub(super) struct TurnRun {
    pub(super) grounding: Arc<dyn SynthesizerGrounding>,
    pub(super) budget: BudgetTree,
    pub(super) temperature: Option<f64>,
    pub(super) max_tokens: Option<u64>,
    pub(super) question: String,
    pub(super) history: ConversationWindow,
    pub(super) memory: Arc<MemoryStore>,
    pub(super) target: TurnTarget,
}

/// A roster a configured turn can launch over whichever provider model the
/// `[chat]` policy names — the member roster ([`MemberRoster`]) or the
/// workspace roster (S-482). Generic over the model because the two provider
/// families are distinct concrete types, so each arm of
/// [`spawn_configured_turn`]'s provider match monomorphizes the launch.
pub(super) trait RosterLaunch: Send + 'static {
    /// Build the roster and orchestrator over `model` and run the streamed turn.
    fn launch<M>(
        self,
        model: M,
        run: TurnRun,
        tx: UnboundedSender<ChatFrame>,
    ) -> impl Future<Output = ()> + Send
    where
        M: CompletionModel + Clone + Send + Sync + 'static;
}

/// The member chat's roster backing: the member's engine and source sandbox.
struct MemberRoster {
    engine: Arc<Engine>,
    sandbox: Arc<Sandbox>,
}

impl RosterLaunch for MemberRoster {
    fn launch<M>(
        self,
        model: M,
        run: TurnRun,
        tx: UnboundedSender<ChatFrame>,
    ) -> impl Future<Output = ()> + Send
    where
        M: CompletionModel + Clone + Send + Sync + 'static,
    {
        let TurnRun { grounding, budget, temperature, max_tokens, question, history, memory, target } =
            run;
        launch(
            self.engine,
            self.sandbox,
            model,
            grounding,
            budget,
            temperature,
            max_tokens,
            question,
            history,
            memory,
            target,
            tx,
        )
    }
}

/// Spawn a configured turn: run `setup` (config resolution, sandbox, stores) on
/// the blocking pool ([ADR-03]), then build the provider the resolved policy
/// names, preflight it, and launch the roster `setup` returned — the shared
/// production turn body of both chats. The turn's scratchpad and durable answer
/// are written to the store `setup` recorded the question in
/// ([`TurnSetup::store_root`]): the member root, or the workspace root for the
/// workspace chat. A configure-first or setup fault is an honest single `error`
/// frame, never a crash ([NFR-CC-04]).
pub(super) fn spawn_configured_turn<L, F>(question: String, setup: F) -> ChatStream
where
    L: RosterLaunch,
    F: FnOnce() -> Result<(TurnSetup, L), String> + Send + 'static,
{
    let (tx, rx) = unbounded_chat_channel();
    let handle = tokio::spawn(async move {
        // Blocking config/store/sandbox setup off the async executor thread
        // ([ADR-03]); a configure-first or setup fault is an honest single
        // `error` frame, never a crash ([NFR-CC-04]).
        let (setup, roster) = match tokio::task::spawn_blocking(setup).await {
            Ok(Ok(setup)) => setup,
            Ok(Err(message)) => {
                let _ = tx.send(ChatFrame::Error(message));
                return;
            }
            Err(_join) => {
                let _ = tx.send(ChatFrame::Error(
                    "the chat setup task failed unexpectedly".to_string(),
                ));
                return;
            }
        };

        // The scratchpad AND the durable answer go to the same `.logos/chat.db`
        // the setup recorded the user's question in ([FR-UI-26] AC-2).
        let (run, Dial { provider, model_id, api_key, base_url, retry }) = setup.into_run(question);

        // Resolve the provider config, then run the deterministic pre-send
        // preflight ([S-199], [FR-UI-24]): a model is set, the key is present,
        // and the `base_url` is well-formed and does not already carry rig's
        // appended `/chat/completions` path. A misconfiguration is an honest
        // single frame naming the specific problem — never a crash, a
        // fabricated answer ([NFR-CC-04]), or an echoed key ([NFR-SE-07]).
        // (An unreachable-but-well-formed endpoint is not probed here; it
        // surfaces honestly as a transport error naming the endpoint when the
        // turn's first call fails — the story's "surface, don't guarantee
        // reachability" scope.)
        let cfg = match provider {
            ChatProvider::Anthropic => ProviderConfig::anthropic(model_id, api_key),
            ChatProvider::OpenAi => {
                ProviderConfig::openai_compatible(model_id, api_key).with_base_url(base_url)
            }
        };
        if let Err(e) = cfg.preflight() {
            let _ = tx.send(ChatFrame::Error(e.to_string()));
            return;
        }

        // Provider client construction + orchestrator wiring are non-blocking;
        // the first egress is the consent-gated turn ([NFR-SE-07]). The two
        // providers are distinct concrete model types, so each arm
        // monomorphizes the roster's launch; both run the same orchestrated turn.
        match provider {
            ChatProvider::Anthropic => match anthropic_completion_model(&cfg, retry) {
                Ok(model) => roster.launch(model, run, tx).await,
                Err(e) => {
                    let _ = tx.send(ChatFrame::Error(format!(
                        "could not build the Anthropic provider: {e}"
                    )));
                }
            },
            ChatProvider::OpenAi => match openai_compatible_completion_model(&cfg, retry) {
                Ok(model) => roster.launch(model, run, tx).await,
                Err(e) => {
                    let _ = tx.send(ChatFrame::Error(format!(
                        "could not build the OpenAI-compatible provider: {e}"
                    )));
                }
            },
        }
    });

    ChatStream::from_spawn(rx, handle)
}

/// Build the roster (grounded on the turn's memory) and orchestrator over `model`,
/// then run the streamed turn — generic over the provider model so both provider
/// families share one code path. The fixed roster shares the top-level model across
/// all four roles; per-role `[chat.models]` overrides ([FR-CF-06]) are a deferred
/// refinement.
///
/// The roster is the member roster, single-backing under any backing ([S-481]).
///
/// [S-481]: ../../../docs/planning/journal.md#s-481-a-workspace-roster-centred-on-the-workspace-and-the-member-roster-single-backing-only
#[allow(clippy::too_many_arguments)]
async fn launch<M>(
    engine: Arc<Engine>,
    sandbox: Arc<Sandbox>,
    model: M,
    grounding: Arc<dyn SynthesizerGrounding>,
    budget: BudgetTree,
    temperature: Option<f64>,
    max_tokens: Option<u64>,
    question: String,
    history: ConversationWindow,
    memory: Arc<MemoryStore>,
    target: TurnTarget,
    tx: UnboundedSender<ChatFrame>,
) where
    M: CompletionModel + Clone + Send + Sync + 'static,
{
    let roster = SubagentRoster::new(engine, sandbox, model.clone())
        .with_temperature(temperature)
        .with_max_tokens(max_tokens)
        .with_synthesizer_grounding(grounding);
    // The roster names the planner preamble that matches its tools.
    let planner = Planner::with_preamble(model, roster.planner_preamble());
    let orchestrator = Orchestrator::with_planner(planner, roster, budget).with_history(history);
    run_orchestrated(orchestrator, question, memory, target, tx).await;
}

#[cfg(test)]
mod tests {
    //! The turn path reads the seam ([S-449], [ADR-67] §6): its verdict is the
    //! tab's verdict, over the whole two-tier fixture matrix.
    //!
    //! Every fixture is on disk with the member nested **inside** the workspace
    //! directory (the real layout), so a resolution that walked up the tree would
    //! find the tier — the `None` rows prove it is never consulted.
    //!
    //! [S-449]: ../../../docs/planning/journal.md#s-449-the-chat-turn-path-reads-the-same-resolution-the-gate-reads
    //! [ADR-67]: ../../../docs/specs/architecture/decisions/ADR-67.md

    use std::fs;
    use std::path::{Path, PathBuf};

    use logos_core::config::resolve_chat;
    use tempfile::TempDir;

    use super::build_setup;

    const WS_MODEL: &str = "workspace/model";
    const WS_KEY: &str = "sk-workspace-key-ws42";
    const MEMBER_MODEL: &str = "member/model";
    const MEMBER_KEY: &str = "sk-member-key-mb77";

    /// A member's declaration of one half: absent, blank, or declared.
    #[derive(Debug, Clone, Copy, PartialEq)]
    enum Half {
        Absent,
        Blank,
        Declared,
    }

    /// A workspace directory with a member nested at `<ws>/svc`.
    struct Estate {
        _tmp: TempDir,
        ws: PathBuf,
        member: PathBuf,
    }

    fn write(root: &Path, file: &str, body: &str) {
        fs::create_dir_all(root.join(".logos")).unwrap();
        fs::write(root.join(".logos").join(file), body).unwrap();
    }

    fn policy(model: Half, value: &str) -> Option<String> {
        match model {
            Half::Absent => None,
            Half::Blank => Some("[chat]\nmodel = \"\"\n".to_string()),
            Half::Declared => Some(format!("[chat]\nmodel = \"{value}\"\n")),
        }
    }

    fn secret(key: Half, value: &str) -> Option<String> {
        match key {
            Half::Absent => None,
            Half::Blank => Some("[chat]\napi_key = \"   \"\n".to_string()),
            Half::Declared => Some(format!("[chat]\napi_key = \"{value}\"\n")),
        }
    }

    fn estate(member_model: Half, member_key: Half, ws_model: Half, ws_key: Half) -> Estate {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path().to_path_buf();
        let member = ws.join("svc");
        fs::create_dir_all(member.join(".logos")).unwrap();
        fs::write(member.join("lib.rs"), "pub fn alpha() {}\n").unwrap();
        if let Some(body) = policy(member_model, MEMBER_MODEL) {
            write(&member, "config.toml", &body);
        }
        if let Some(body) = secret(member_key, MEMBER_KEY) {
            write(&member, "secrets.toml", &body);
        }
        if let Some(body) = policy(ws_model, WS_MODEL) {
            write(&ws, "config.toml", &body);
        }
        if let Some(body) = secret(ws_key, WS_KEY) {
            write(&ws, "secrets.toml", &body);
        }
        Estate {
            _tmp: tmp,
            ws,
            member,
        }
    }

    /// The tab's verdict: read off the **serialized** resolution the config
    /// read-model carries — the two origins, never the file contents.
    fn tab_verdict(member: &Path, ws: Option<&Path>) -> bool {
        let json = serde_json::to_value(resolve_chat(member, ws).expect("resolution")).unwrap();
        json["policy_origin"] != "unset" && json["credential_origin"] != "unset"
    }

    /// The turn's verdict: does `build_setup` produce a turn? A refusal must be the
    /// configure-first state, never a setup fault. A produced turn must also hold
    /// the trust boundary ([ADR-67] §2): a turn dialling the inherited workspace
    /// model dials the workspace key, never the member's.
    fn turn_verdict(member: &Path, ws: Option<&Path>) -> bool {
        match build_setup(member, ws, None, "what is here?") {
            Ok(setup) => {
                if setup.turn.model_id == WS_MODEL {
                    assert_eq!(
                        setup.turn.api_key, WS_KEY,
                        "a member key reached the workspace endpoint"
                    );
                }
                true
            }
            Err(message) => {
                assert!(
                    message.starts_with("Chat is not configured yet"),
                    "a refused turn is configure-first, not a fault: {message}",
                );
                false
            }
        }
    }

    /// AC-1: a member declaring **neither** half, under a workspace declaring
    /// both, produces a turn — dialling the workspace's model with its key.
    #[test]
    fn a_member_declaring_neither_half_under_a_workspace_declaring_both_produces_a_turn() {
        let e = estate(Half::Absent, Half::Absent, Half::Declared, Half::Declared);
        let setup = build_setup(&e.member, Some(&e.ws), None, "what is here?")
            .expect("an inheriting member produces a turn");
        assert_eq!(setup.turn.model_id, WS_MODEL);
        assert_eq!(setup.turn.api_key, WS_KEY);
    }

    /// Per-half inheritance reaches the turn in the one allowed direction: a
    /// member's own model dials with the workspace's key.
    #[test]
    fn a_member_owned_policy_dials_with_the_inherited_workspace_key() {
        let e = estate(Half::Declared, Half::Absent, Half::Declared, Half::Declared);
        let setup = build_setup(&e.member, Some(&e.ws), None, "q").expect("a turn");
        assert_eq!(
            (setup.turn.model_id.as_str(), setup.turn.api_key.as_str()),
            (MEMBER_MODEL, WS_KEY)
        );
    }

    /// HF-1 ([ADR-67] §2): the mirror is closed. A member holding only its own
    /// key, under a workspace declaring both halves, dials the workspace model
    /// with the **workspace** key — its own key never reaches that endpoint.
    #[test]
    fn a_member_key_is_never_dialled_with_the_inherited_workspace_policy() {
        let e = estate(Half::Absent, Half::Declared, Half::Declared, Half::Declared);
        let setup = build_setup(&e.member, Some(&e.ws), None, "q").expect("a turn");
        assert_eq!(
            (setup.turn.model_id.as_str(), setup.turn.api_key.as_str()),
            (WS_MODEL, WS_KEY)
        );
    }

    /// HF-1: under a workspace declaring the policy but no key, the member's own
    /// key does not fill the gap — the turn is refused, and the refusal names
    /// the withheld key and the action that lets the member use it.
    #[test]
    fn a_member_key_under_a_keyless_workspace_policy_is_refused_and_named() {
        let e = estate(Half::Absent, Half::Declared, Half::Declared, Half::Absent);
        let m = build_setup(&e.member, Some(&e.ws), None, "q")
            .err()
            .expect("no key may be dialled with the workspace policy");
        let ws = e.ws.display().to_string();
        assert!(
            m.contains(&format!("no API key is declared by the workspace root {ws}")),
            "{m}"
        );
        assert!(
            m.contains(&format!("the provider model is inherited from the workspace root {ws}")),
            "{m}"
        );
        assert!(
            m.contains(
                "this member's own API key is not used with the inherited workspace endpoint"
            ),
            "{m}"
        );
        assert!(
            m.contains("setting a [chat] model on this member makes it use its own key"),
            "{m}"
        );
        assert!(m.contains("Choose a provider model in the Config tab"), "{m}");
        assert!(!m.contains(MEMBER_KEY) && !m.contains("mb77"), "never echoes the key: {m}");
        assert!(!tab_verdict(&e.member, Some(&e.ws)), "and the gate agrees");

        // Owning the policy is what makes the member use its own key.
        write(&e.member, "config.toml", &format!("[chat]\nmodel = \"{MEMBER_MODEL}\"\n"));
        let setup = build_setup(&e.member, Some(&e.ws), None, "q").expect("a turn");
        assert_eq!(
            (setup.turn.model_id.as_str(), setup.turn.api_key.as_str()),
            (MEMBER_MODEL, MEMBER_KEY)
        );
    }

    /// An inheriting member dials with the workspace's **whole** `[chat]` table
    /// ([ADR-67] §3): provider, endpoint, sampling, budget tree and retry policy all
    /// come from the root that declared `model` — none from the member's own
    /// model-less table, whose every value here differs from the workspace's.
    ///
    /// [ADR-67]: ../../../docs/specs/architecture/decisions/ADR-67.md
    #[test]
    fn an_inheriting_member_dials_with_the_whole_workspace_table() {
        use agent_core::RetryPolicy;
        use logos_core::config::ChatProvider;

        let e = estate(Half::Absent, Half::Absent, Half::Absent, Half::Declared);
        write(
            &e.ws,
            "config.toml",
            &format!(
                "[chat]\nprovider = \"anthropic\"\nmodel = \"{WS_MODEL}\"\n\
                 base_url = \"https://workspace.example/v1\"\ntemperature = 0.3\n\
                 max_tokens = 777\nmax_tool_calls = 11\nmax_subagent_tool_calls = 5\n\
                 max_replans = 2\nmax_provider_retries = 4\nprovider_retry_base_ms = 321\n"
            ),
        );
        write(
            &e.member,
            "config.toml",
            "[chat]\nprovider = \"openai\"\nbase_url = \"https://member.example/v1\"\n\
             temperature = 0.9\nmax_tokens = 99\nmax_tool_calls = 40\n\
             max_subagent_tool_calls = 20\nmax_replans = 6\nmax_provider_retries = 1\n\
             provider_retry_base_ms = 50\n",
        );

        let setup = build_setup(&e.member, Some(&e.ws), None, "q").expect("a turn");
        assert_eq!(setup.turn.model_id, WS_MODEL);
        assert_eq!(setup.turn.api_key, WS_KEY);
        assert_eq!(setup.turn.provider, ChatProvider::Anthropic);
        assert_eq!(setup.turn.base_url, "https://workspace.example/v1");
        assert_eq!(setup.turn.temperature, Some(0.3));
        assert_eq!(setup.turn.max_tokens, Some(777));
        assert_eq!(
            (
                setup.turn.budget.global_limit(),
                setup.turn.budget.max_subagent_tool_calls(),
                setup.turn.budget.max_replans()
            ),
            (11, 5, 2)
        );
        assert_eq!(setup.turn.retry, RetryPolicy::new(4, 321));
    }

    /// [S-483]: a follow-up turn on an existing thread is set up with the thread's
    /// bounded prior turns, read from the thread store BEFORE the new question is
    /// appended (so the window never holds the question it is answering) and bounded
    /// by the resolved `[chat]` window keys; a first turn has none.
    ///
    /// [S-483]: ../../../docs/planning/journal.md#s-483-follow-up-turns-see-prior-turns
    #[test]
    fn a_follow_up_setup_carries_the_threads_prior_turns_bounded_by_the_chat_keys() {
        use chat_agent::{ChatRole, ChatStore};

        let e = estate(Half::Declared, Half::Declared, Half::Declared, Half::Declared);
        write(
            &e.member,
            "config.toml",
            &format!("[chat]\nmodel = \"{MEMBER_MODEL}\"\nhistory_max_turns = 2\n"),
        );

        let first = build_setup(&e.member, None, None, "first question").expect("a turn");
        assert!(first.turn.history.is_empty(), "a thread's first turn has no window");
        let thread = first.turn.thread_id;
        let mut store = ChatStore::open(&e.member).unwrap();
        store.append_message(thread, ChatRole::Assistant, "first answer", &[]).unwrap();
        for (q, a) in [("second question", "second answer"), ("third question", "third answer")] {
            store.append_message(thread, ChatRole::User, q, &[]).unwrap();
            store.append_message(thread, ChatRole::Assistant, a, &[]).unwrap();
        }
        drop(store);

        let followup =
            build_setup(&e.member, None, Some(thread), "fourth question").expect("a follow-up");
        let text = followup.turn.history.render();
        assert_eq!(followup.turn.history.omitted(), 1, "history_max_turns = 2 keeps two of three: {text}");
        assert!(text.contains("second question") && text.contains("third answer"), "{text}");
        assert!(!text.contains("first question"), "the oldest turn went first: {text}");
        assert!(!text.contains("fourth question"), "the window never holds the live question: {text}");

        // `history_max_chars` bounds the window too (and the turn key, left at its
        // default of 6 here, no longer binds): "fourth question" (15 characters, never
        // answered) fits a 30-character ceiling, the 27-character third turn does not.
        write(
            &e.member,
            "config.toml",
            &format!("[chat]\nmodel = \"{MEMBER_MODEL}\"\nhistory_max_chars = 30\n"),
        );
        let capped =
            build_setup(&e.member, None, Some(thread), "fifth question").expect("a follow-up");
        let text = capped.turn.history.render();
        assert_eq!(capped.turn.history.omitted(), 3, "history_max_chars = 30 keeps one of four: {text}");
        assert!(text.contains("fourth question") && !text.contains("third question"), "{text}");
    }

    /// Sprint-79 HF-1: `[chat] read_roots` resolve against the root that
    /// declared the effective table. A member-owned table resolves its entry
    /// against the member; an inherited one against the WORKSPACE root — the
    /// same entry names a different directory, and each turn gets its own.
    #[test]
    fn read_roots_resolve_against_the_root_that_declared_the_policy() {
        let e = estate(Half::Declared, Half::Declared, Half::Declared, Half::Declared);
        fs::create_dir_all(e.member.join("member-docs")).unwrap();
        fs::create_dir_all(e.ws.join("member-docs")).unwrap();
        let body = |model: &str| {
            format!("[chat]\nmodel = \"{model}\"\nread_roots = [\"member-docs\"]\n")
        };
        write(&e.member, "config.toml", &body(MEMBER_MODEL));
        write(&e.ws, "config.toml", &body(WS_MODEL));

        let owned = build_setup(&e.member, Some(&e.ws), None, "q").expect("a turn");
        assert_eq!(
            owned.sandbox.read_roots(),
            [e.member.join("member-docs").canonicalize().unwrap()],
            "a member-owned table resolves against the member"
        );

        write(&e.member, "config.toml", "[chat]\n");
        let inherited = build_setup(&e.member, Some(&e.ws), None, "q").expect("a turn");
        assert_eq!(inherited.turn.model_id, WS_MODEL);
        assert_eq!(
            inherited.sandbox.read_roots(),
            [e.ws.join("member-docs").canonicalize().unwrap()],
            "an inherited table resolves against the workspace root that declared it"
        );
    }

    /// The member chat's half of the review's fixture (sprint-impl-84 Decision 2,
    /// [NFR-SE-04]): a member declaring `read_roots = ["../shared-docs"]` but no
    /// `model`, under a workspace declaring `model`, inherits the workspace table
    /// **whole** — its own `read_roots` go with the rest of it — so its chat
    /// refuses `docs/guide.md` as a containment. Once the member owns its policy
    /// the same path is readable. The workspace chat's addressed `read` is pinned
    /// to the same verdict by `agent-core/tests/addressed_workspace_tools.rs`'s
    /// `the_addressed_read_roots_are_the_members_effective_ones_as_its_chat_resolves_them`.
    ///
    /// [NFR-SE-04]: ../../../docs/specs/requirements/NFR-SE-04.md
    #[cfg(unix)]
    #[test]
    fn an_inherited_policy_drops_the_members_own_read_roots_from_the_member_chat() {
        let e = estate(Half::Absent, Half::Absent, Half::Declared, Half::Declared);
        fs::create_dir_all(e.ws.join("shared-docs")).unwrap();
        fs::write(e.ws.join("shared-docs/guide.md"), "a guide\n").unwrap();
        std::os::unix::fs::symlink("../shared-docs", e.member.join("docs")).unwrap();
        write(&e.member, "config.toml", "[chat]\nread_roots = [\"../shared-docs\"]\n");

        let inherited = build_setup(&e.member, Some(&e.ws), None, "q").expect("a turn");
        assert_eq!(inherited.turn.model_id, WS_MODEL);
        let refused = inherited.sandbox.resolve("docs/guide.md").expect_err("refused");
        assert!(refused.is_containment_refusal(), "{refused}");

        write(
            &e.member,
            "config.toml",
            &format!("[chat]\nmodel = \"{MEMBER_MODEL}\"\nread_roots = [\"../shared-docs\"]\n"),
        );
        let owned = build_setup(&e.member, Some(&e.ws), None, "q").expect("a turn");
        owned.sandbox.resolve("docs/guide.md").expect("an owned table's read root is readable");
    }

    /// Sprint-79 HF-1: a declared read root that does not exist fails the turn
    /// with a setup message naming the entry — reported, never silently dropped
    /// into a turn that quietly cannot see the docs.
    #[test]
    fn a_missing_read_root_fails_the_turn_by_name() {
        let e = estate(Half::Declared, Half::Declared, Half::Absent, Half::Absent);
        write(
            &e.member,
            "config.toml",
            &format!("[chat]\nmodel = \"{MEMBER_MODEL}\"\nread_roots = [\"../no-such-docs\"]\n"),
        );
        let message = build_setup(&e.member, None, None, "q")
            .err()
            .expect("a missing read root refuses the turn");
        assert!(message.starts_with("could not open the source sandbox"), "{message}");
        assert!(message.contains("../no-such-docs"), "names the entry: {message}");
        assert!(message.contains("does not exist"), "{message}");
        assert!(
            !e.member.join(".logos/chat.db").exists(),
            "the refused turn opened no chat store, so it recorded no orphan thread"
        );
    }

    /// AC-2: tab verdict == turn verdict, asserted as ONE equality over the whole
    /// matrix — every member shape (absent / blank / declared, per half) × every
    /// workspace shape × with and without the workspace root passed. The matrix
    /// holds the HF-1 rows (a member key alone under a workspace declaring both
    /// halves, and under one declaring only the policy), and every turn it
    /// produces is checked against the trust boundary in `turn_verdict`.
    #[test]
    fn tab_verdict_equals_turn_verdict_over_the_whole_fixture_matrix() {
        let shapes = [Half::Absent, Half::Blank, Half::Declared];
        let ws_shapes = [Half::Absent, Half::Declared];
        let mut tab = Vec::new();
        let mut turn = Vec::new();
        for mm in shapes {
            for mk in shapes {
                for wm in ws_shapes {
                    for wk in ws_shapes {
                        for pass_ws in [false, true] {
                            let e = estate(mm, mk, wm, wk);
                            let ws = pass_ws.then_some(e.ws.as_path());
                            let label =
                                format!("member({mm:?},{mk:?}) ws({wm:?},{wk:?}) pass={pass_ws}");
                            tab.push((label.clone(), tab_verdict(&e.member, ws)));
                            turn.push((label, turn_verdict(&e.member, ws)));
                        }
                    }
                }
            }
        }
        assert_eq!(tab.len(), 72, "the whole matrix ran");
        assert_eq!(turn, tab, "the turn and the tab cannot disagree");
        // Non-degenerate: the matrix holds both verdicts, including inherited turns.
        let turns = tab.iter().filter(|(_, v)| *v).count();
        assert!(
            turns > 0 && turns < tab.len(),
            "{turns} of {} rows produce a turn",
            tab.len()
        );
    }

    /// Known consequence (a) of [S-447]: a blank `model` is undeclared. The turn
    /// used to dial with `Some("")` while the gate said configure-first; now a
    /// blank member model is refused alone, and does not shadow a workspace model.
    ///
    /// [S-447]: ../../../docs/planning/journal.md#s-447-the-chat-policy-and-its-credential-resolve-through-one-seam
    #[test]
    fn a_blank_model_is_undeclared_on_the_turn_path_as_on_the_gate() {
        let e = estate(Half::Blank, Half::Declared, Half::Absent, Half::Absent);
        let message = build_setup(&e.member, None, None, "q")
            .err()
            .expect("a blank model is configure-first, never a dial with an empty model");
        assert!(
            message.contains("no provider model is declared"),
            "{message}"
        );
        assert!(!tab_verdict(&e.member, None), "and the gate agrees");

        // The workspace policy it inherits takes the workspace key (HF-1): a
        // blank model does not make the member's key eligible for that endpoint.
        let e = estate(Half::Blank, Half::Declared, Half::Declared, Half::Declared);
        let setup = build_setup(&e.member, Some(&e.ws), None, "q").expect("a turn");
        assert_eq!(
            (setup.turn.model_id.as_str(), setup.turn.api_key.as_str()),
            (WS_MODEL, WS_KEY),
            "a blank member model does not shadow the workspace's"
        );
    }

    /// AC-3 ([NFR-CC-04]): a refused turn names the root it inspected, the absent
    /// half and the origin of any present half — the facts the tab's state names.
    ///
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    #[test]
    fn a_refused_turn_names_the_root_the_absent_half_and_the_present_origin() {
        // Policy absent everywhere; the key inherited from the workspace.
        let e = estate(Half::Absent, Half::Absent, Half::Absent, Half::Declared);
        let m = build_setup(&e.member, Some(&e.ws), None, "q")
            .err()
            .expect("refused");
        let member = e.member.display().to_string();
        let ws = e.ws.display().to_string();
        assert!(
            m.contains(&format!("for {member}")),
            "names the member root: {m}"
        );
        assert!(
            m.contains("no provider model is declared"),
            "names the absent half: {m}"
        );
        assert!(
            m.contains(&format!(
                "the API key is inherited from the workspace root {ws}"
            )),
            "names the present half's origin: {m}",
        );
        assert!(
            m.contains("Choose a provider model in the Config tab"),
            "{m}"
        );
        assert!(!m.contains(WS_KEY), "never echoes the key: {m}");

        // Policy the member's own; the key absent everywhere.
        let e = estate(Half::Declared, Half::Absent, Half::Absent, Half::Absent);
        let m = build_setup(&e.member, Some(&e.ws), None, "q")
            .err()
            .expect("refused");
        let member = e.member.display().to_string();
        assert!(
            m.contains("no API key is declared"),
            "names the absent half: {m}"
        );
        assert!(
            m.contains(&format!("the provider model is declared by {member}")),
            "names the present half's origin: {m}",
        );
        assert!(
            m.contains(&format!("or by the workspace root {}", e.ws.display())),
            "{m}"
        );
        assert!(m.contains("Add an API key in the Config tab"), "{m}");

        // Single root, neither half: names the root and both halves, and no tier.
        let e = estate(Half::Absent, Half::Absent, Half::Declared, Half::Declared);
        let m = build_setup(&e.member, None, None, "q")
            .err()
            .expect("refused");
        assert!(m.contains(&format!("for {}", e.member.display())), "{m}");
        assert!(
            m.contains("neither a provider model nor an API key is declared"),
            "{m}"
        );
        assert!(
            !m.contains("workspace"),
            "single-root consults no second tier: {m}"
        );
    }

    /// Every reachable refusal shape, pinned **whole**: each (policy origin,
    /// credential origin) pair that refuses, with and without a workspace root
    /// passed, names exactly its root, its absent half, the origin of its present
    /// half and the action that fixes it ([NFR-CC-04]). A mislabelled origin or a
    /// wrong action cannot survive an equality the way it survives `contains`.
    ///
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    #[test]
    fn every_refusal_shape_names_exactly_its_facts() {
        use Half::{Absent, Declared};
        // (member model, member key, ws model, ws key, pass ws, absent, present, action,
        //  the inherited-policy member-key note)
        let rows = [
            (
                Absent,
                Absent,
                Absent,
                Absent,
                true,
                "neither a provider model nor an API key is declared",
                None,
                "Choose a provider model and add an API key",
                None,
            ),
            (
                Absent,
                Declared,
                Absent,
                Absent,
                true,
                "no provider model is declared",
                Some(("API key", false)),
                "Choose a provider model",
                None,
            ),
            (
                Absent,
                Absent,
                Absent,
                Declared,
                true,
                "no provider model is declared",
                Some(("API key", true)),
                "Choose a provider model",
                None,
            ),
            (
                Declared,
                Absent,
                Absent,
                Absent,
                true,
                "no API key is declared",
                Some(("provider model", false)),
                "Add an API key",
                None,
            ),
            // HF-1: under an inherited policy only the workspace root can supply
            // the key — a member key would not be used, so none is advised alone.
            (
                Absent,
                Absent,
                Declared,
                Absent,
                true,
                "no API key is declared",
                Some(("provider model", true)),
                "Choose a provider model and add an API key",
                Some(
                    "an API key added to this member is not used with the inherited \
                     workspace endpoint — it is used once this member declares its own \
                     [chat] model",
                ),
            ),
            // HF-1: the same shape with a member key, which is withheld.
            (
                Absent,
                Declared,
                Declared,
                Absent,
                true,
                "no API key is declared",
                Some(("provider model", true)),
                "Choose a provider model",
                Some(
                    "this member's own API key is not used with the inherited workspace \
                     endpoint — setting a [chat] model on this member makes it use its own key",
                ),
            ),
            (
                Absent,
                Absent,
                Declared,
                Declared,
                false,
                "neither a provider model nor an API key is declared",
                None,
                "Choose a provider model and add an API key",
                None,
            ),
            (
                Absent,
                Declared,
                Declared,
                Declared,
                false,
                "no provider model is declared",
                Some(("API key", false)),
                "Choose a provider model",
                None,
            ),
            (
                Declared,
                Absent,
                Declared,
                Declared,
                false,
                "no API key is declared",
                Some(("provider model", false)),
                "Add an API key",
                None,
            ),
        ];
        for (mm, mk, wm, wk, pass_ws, absent, present, action, note) in rows {
            let e = estate(mm, mk, wm, wk);
            let ws = pass_ws.then_some(e.ws.as_path());
            let member = e.member.display();
            // Under an inherited policy only the workspace root is named as a place
            // the key could come from (HF-1).
            let looked = match (ws, note) {
                (Some(ws), Some(_)) => format!(" by the workspace root {}", ws.display()),
                (Some(ws), None) => {
                    format!(" by this member or by the workspace root {}", ws.display())
                }
                (None, _) => String::new(),
            };
            let note = note.map(|n| format!("; {n}")).unwrap_or_default();
            let present = match present {
                None => String::new(),
                Some((half, true)) => {
                    format!(
                        "; the {half} is inherited from the workspace root {}",
                        e.ws.display()
                    )
                }
                Some((half, false)) => format!("; the {half} is declared by {member}"),
            };
            let expected = format!(
                "Chat is not configured yet for {member} — {absent}{looked}{present}{note}. \
                 {action} in the Config tab before starting a turn."
            );
            let actual = build_setup(&e.member, ws, None, "q")
                .err()
                .expect("refused");
            assert_eq!(
                actual, expected,
                "member({mm:?},{mk:?}) ws({wm:?},{wk:?}) pass={pass_ws}"
            );
        }
    }

    /// A refused turn is decided before any store is touched: it records no
    /// thread and no message.
    #[test]
    fn a_refused_turn_opens_no_chat_store() {
        let e = estate(Half::Absent, Half::Absent, Half::Absent, Half::Absent);
        assert!(build_setup(&e.member, Some(&e.ws), None, "q").is_err());
        assert!(
            !e.member.join(".logos/chat.db").exists(),
            "no chat.db for a refused turn"
        );
    }

    /// A key pasted into `config.toml` — the line `deny_unknown_fields` rejects —
    /// is never echoed by the parse fault, at the member root or at the workspace
    /// root an inheriting member reads ([NFR-SE-07]); the fault still names the
    /// file and the position.
    ///
    /// [NFR-SE-07]: ../../../docs/specs/requirements/NFR-SE-07.md
    #[test]
    fn a_config_toml_parse_fault_never_echoes_a_pasted_key_at_either_root() {
        let pasted = "[chat]\nmodel = \"m\"\napi_key = \"sk-LEAKME-CONFIG\"\n";

        let e = estate(Half::Absent, Half::Absent, Half::Absent, Half::Declared);
        write(&e.member, "config.toml", pasted);
        let member_fault = build_setup(&e.member, None, None, "q")
            .err()
            .expect("a fault");

        let e2 = estate(Half::Absent, Half::Absent, Half::Absent, Half::Declared);
        write(&e2.ws, "config.toml", pasted);
        let ws_fault = build_setup(&e2.member, Some(&e2.ws), None, "q")
            .err()
            .expect("a fault");

        for (m, file) in [
            (&member_fault, e.member.join(".logos/config.toml")),
            (&ws_fault, e2.ws.join(".logos/config.toml")),
        ] {
            assert!(!m.contains("sk-LEAKME-CONFIG"), "never echoes the key: {m}");
            assert!(
                m.starts_with("could not read the chat configuration"),
                "a config fault, not a secret fault: {m}"
            );
            assert!(
                m.contains(&file.display().to_string()),
                "names the file: {m}"
            );
            assert!(m.contains("line 3"), "names the position: {m}");
        }
    }

    /// A fault that quotes no file content keeps its detail: an invalid value in
    /// the workspace policy an inheriting member relies on names the key, under the
    /// configuration wording — never reclassified as a secret fault.
    #[test]
    fn a_validation_fault_keeps_its_detail() {
        let e = estate(Half::Absent, Half::Absent, Half::Absent, Half::Declared);
        write(
            &e.ws,
            "config.toml",
            "[chat]\nmodel = \"m\"\nbase_url = \"\"\n",
        );
        let m = build_setup(&e.member, Some(&e.ws), None, "q")
            .err()
            .expect("a fault");
        assert!(
            m.starts_with("could not read the chat configuration: "),
            "{m}"
        );
        assert!(m.contains("chat.base_url"), "names the offending key: {m}");
    }

    /// Known consequence (b) of [S-447], kept: the seam always reads the member's
    /// `secrets.toml`, so an invalid one with no model reports the parse fault
    /// (fail loud) rather than "not configured" — and never echoes the key.
    ///
    /// [S-447]: ../../../docs/planning/journal.md#s-447-the-chat-policy-and-its-credential-resolve-through-one-seam
    #[test]
    fn an_invalid_member_secrets_toml_with_no_model_fails_loud_without_the_key() {
        let e = estate(Half::Absent, Half::Absent, Half::Absent, Half::Absent);
        write(
            &e.member,
            "secrets.toml",
            "[chat]\napi_key = \"sk-LEAKME-DEADBEEF\" not valid toml here\n",
        );
        let m = build_setup(&e.member, None, None, "q")
            .err()
            .expect("a read fault");
        assert!(
            m.starts_with("could not read the chat secret"),
            "a fault, not configure-first: {m}"
        );
        assert!(m.contains("secrets.toml"), "names the file: {m}");
        assert!(
            !m.contains("sk-LEAKME-DEADBEEF"),
            "never echoes the key (NFR-SE-07): {m}"
        );
    }

    /// [S-481]: `launch` — the seam every production turn goes through — builds
    /// the member roster, which is single-backing under any backing. A turn over
    /// a workspace member's engine (resolved through the federated registry, as
    /// `chat_for` resolves a `?repo=` member) runs under exactly the single-root
    /// preambles, and a scripted `xservice_*` call is refused as out-of-domain in
    /// both. Mock provider throughout: nothing dials. S-431's federated half of
    /// this pair moved to the workspace roster
    /// (`chat-agent/tests/xservice_roster.rs`,
    /// `a_workspace_turn_runs_the_planner_and_synthesizer_under_the_workspace_preambles`).
    ///
    /// [S-481]: ../../../docs/planning/journal.md#s-481-a-workspace-roster-centred-on-the-workspace-and-the-member-roster-single-backing-only
    mod member_launch {
        use std::sync::Arc;

        use agent_core::{MockCompletionModel, MockTurn, Sandbox};
        use chat_agent::{
            BudgetTree, ChatStore, ConversationWindow, MemoryGrounding, MemoryStore,
            OrchestratorEvent, StepRole, SynthesizerGrounding,
        };
        use logos_core::federation::{EngineRegistry, Federation, Member, RegistryMode};
        use logos_core::Engine;
        use tempfile::TempDir;

        use super::super::launch;
        use crate::chat::{unbounded_chat_channel, ChatFrame, TurnTarget};

        /// `project`'s engine as a member `api` of a two-member workspace hands it
        /// out — through the lazy registry, not a direct start.
        fn member_engine(root: &std::path::Path, project: &std::path::Path) -> Arc<Engine> {
            std::fs::create_dir_all(root.join("web")).unwrap();
            let federation = Federation {
                name: "shop".to_string(),
                root: root.to_path_buf(),
                members: vec![
                    Member { name: "api".to_string(), root: project.to_path_buf() },
                    Member { name: "web".to_string(), root: root.join("web") },
                ],
                default: None,
                links: Vec::new(),
                governance: Default::default(),
                warm_concurrency: None,
                member_kinds: Default::default(),
            };
            EngineRegistry::<Engine>::new(federation, RegistryMode::Lazy)
                .engine_for("api")
                .expect("api resolves")
        }

        /// Run one scripted turn through `launch` — over a workspace member's
        /// engine or a single root's — and return the Graph-Navigator's
        /// observation and every system prompt the model was sent.
        async fn navigator_observation(workspace: bool) -> (String, Vec<Option<String>>) {
            let tmp = TempDir::new().unwrap();
            let project = tmp.path().join("api");
            std::fs::create_dir_all(project.join("src")).unwrap();
            std::fs::write(project.join("src/lib.rs"), "pub fn alpha() {}\n").unwrap();

            let engine = if workspace {
                member_engine(tmp.path(), &project)
            } else {
                Arc::new(Engine::start(&project).expect("engine"))
            };
            let sandbox = Arc::new(Sandbox::new(&project, std::iter::empty()).expect("sandbox"));
            let mut store = ChatStore::open(&project).expect("chat store");
            let thread = store.create_thread_from_message("q").expect("thread");
            drop(store);
            let memory = Arc::new(MemoryStore::open(&project).expect("memory"));
            let turn = memory.next_turn(thread).expect("turn");
            let grounding: Arc<dyn SynthesizerGrounding> =
                Arc::new(MemoryGrounding::new(Arc::clone(&memory), thread, turn));

            // One model backs the planner and all four roles, consumed in order:
            // plan → the navigator's tool call → its summary → final → answer.
            let model = MockCompletionModel::new([
                MockTurn::text(
                    r#"{"action":"plan","steps":[{"role":"graph_navigator","instruction":"find alpha across services"}]}"#,
                ),
                MockTurn::tool_call("x1", "xservice_search", serde_json::json!({ "query": "alpha" })),
                MockTurn::text("searched."),
                MockTurn::text(r#"{"action":"final","grounded":true}"#),
                MockTurn::text("answer"),
            ]);
            let (tx, mut rx) = unbounded_chat_channel();
            let recorder = model.clone();
            launch(
                engine,
                sandbox,
                model,
                grounding,
                BudgetTree::new(48, 16, 3),
                None,
                None,
                "find alpha across services".to_string(),
                ConversationWindow::default(),
                memory,
                TurnTarget::new(project, thread, turn),
                tx,
            )
            .await;

            let mut observation = None;
            while let Ok(frame) = rx.try_recv() {
                if let ChatFrame::Event(OrchestratorEvent::StepObserved {
                    role: StepRole::GraphNavigator,
                    summary,
                    ..
                }) = frame
                {
                    observation = Some(summary);
                }
            }
            (observation.expect("the navigator step was observed"), recorder.system_prompts())
        }

        /// Re-targeted from S-431's `a_workspace_turn_dispatches_xservice_through_launch`.
        #[tokio::test]
        async fn a_workspace_member_turn_through_launch_has_no_xservice_tool() {
            assert_eq!(navigator_observation(true).await, navigator_observation(false).await);
        }

        #[tokio::test]
        async fn a_single_root_turn_through_launch_has_no_xservice_tool() {
            let (observation, prompts) = navigator_observation(false).await;
            assert_eq!(
                prompts.first().cloned().flatten().as_deref(),
                Some(chat_agent::orchestrator::DEFAULT_PLANNER_PREAMBLE)
            );
            assert_eq!(
                prompts.last().cloned().flatten().as_deref(),
                Some(chat_agent::SYNTHESIZER_PREAMBLE)
            );
            assert_eq!(observation, "searched.", "no reading: the call was out-of-domain");
        }
    }
}
