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
//! # Cross-service reach under workspace serving ([S-431], [ADR-52])
//! A service built over a federated backing carries its [`XserviceBacking`], and
//! hands it to the roster ([`SubagentRoster::with_xservice`]) and the workspace
//! planner preamble to the orchestrator. Single-root serving supplies none, so its
//! roster, planner and turn are exactly what they were. Nothing here opens a
//! member: the backing is the registry the router already holds, and its engines
//! start only when a dispatched `xservice_*` call reaches them ([NFR-PE-10]).
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
//! [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
//! [S-431]: ../../../docs/planning/journal.md#s-431-the-chat-agents-tool-surface-is-workspace-aware
//! [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md
//! [FR-CF-06]: ../../../docs/specs/requirements/FR-CF-06.md
//! [FR-UI-18]: ../../../docs/specs/requirements/FR-UI-18.md
//! [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
//! [`chat-agent`]: ../../../docs/specs/architecture/components/chat-agent.md
//! [`agent-core`]: ../../../docs/specs/architecture/components/agent-core.md

use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent_core::rig::completion::CompletionModel;
use agent_core::{
    anthropic_completion_model, openai_compatible_completion_model, ProviderConfig, RetryPolicy,
};
use agent_core::{Sandbox, XserviceBacking};
use chat_agent::{
    BudgetTree, ChatRole, ChatStore, MemoryGrounding, MemoryStore, Orchestrator, Planner,
    SubagentRoster, SynthesizerGrounding,
};
use logos_core::config::{resolve_chat, ChatProvider};
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
    /// The federated query backing behind the Graph-Navigator's `xservice_*`
    /// tools ([S-431]); `None` under single-root backing, where the roster stays
    /// the eight graph tools ([ADR-52]).
    ///
    /// [S-431]: ../../../docs/planning/journal.md#s-431-the-chat-agents-tool-surface-is-workspace-aware
    /// [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
    xservice: Option<XserviceBacking>,
}

impl ConfiguredChatService {
    /// Build the service over the shared engine and the already-resolved workspace
    /// root (`None` under single-root backing, where no second tier is consulted).
    pub(crate) fn new(engine: Arc<Engine>, workspace_root: Option<PathBuf>) -> Self {
        Self {
            engine,
            workspace_root,
            xservice: None,
        }
    }

    /// Give the turn cross-service reach over `xservice` — what
    /// [`XserviceBacking::federated`] returned for the router's backing, so
    /// `None` (single-root) is a no-op ([S-431], [ADR-52]).
    ///
    /// [S-431]: ../../../docs/planning/journal.md#s-431-the-chat-agents-tool-surface-is-workspace-aware
    /// [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
    pub(crate) fn with_xservice(mut self, xservice: Option<XserviceBacking>) -> Self {
        self.xservice = xservice;
        self
    }
}

/// The blocking-acquired pieces of a turn's setup — produced on the blocking pool
/// ([ADR-03]) and consumed by the async orchestrator run.
struct ChatSetup {
    memory: Arc<MemoryStore>,
    sandbox: Arc<Sandbox>,
    thread_id: i64,
    turn: i64,
    provider: ChatProvider,
    model_id: String,
    api_key: String,
    base_url: String,
    budget: BudgetTree,
    temperature: Option<f64>,
    max_tokens: Option<u64>,
    retry: RetryPolicy,
}

/// Resolve the policy + credential through the seam, resolve/create the thread,
/// open memory, and open the source sandbox — the **blocking** half of a turn's
/// setup. Returns an honest configure-first / setup-fault message on failure
/// ([NFR-CC-04]); runs on the blocking pool ([ADR-03]).
///
/// The configure-first verdict is checked **before** any store is touched, so a
/// refused turn records no thread and no message.
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
    let TurnProvider { model_id, api_key } = turn_provider(root, workspace_root, &resolution)?;
    let chat = resolution.policy;

    // The thread the turn appends to (a new one when the caller gave none); the
    // scratchpad's foreign key requires the thread to exist first.
    let mut store =
        ChatStore::open(root).map_err(|e| format!("could not open the chat store: {e}"))?;
    let thread_id = match thread_id {
        Some(id) => id,
        None => store
            .create_thread_from_message(question)
            .map_err(|e| format!("could not create a chat thread: {e}"))?,
    };
    // The question is durable from the start; its ANSWER is appended by
    // `run_orchestrated` once the turn genuinely produces one, so a restored
    // conversation replays both halves ([FR-UI-26] AC-2).
    store
        .append_message(thread_id, ChatRole::User, question, &[])
        .map_err(|e| format!("could not record the user message: {e}"))?;
    drop(store);

    let memory =
        Arc::new(MemoryStore::open(root).map_err(|e| format!("could not open chat memory: {e}"))?);
    let turn = memory
        .next_turn(thread_id)
        .map_err(|e| format!("could not compute the turn ordinal: {e}"))?;

    let sandbox = Arc::new(
        Sandbox::from_root(root).map_err(|e| format!("could not open the source sandbox: {e}"))?,
    );

    let budget = BudgetTree::from(&chat);
    let temperature = chat.temperature;
    let max_tokens = chat.max_tokens.map(u64::from);
    // The bounded provider-retry policy the model decorator applies ([CR-060],
    // [S-240]); the wiki-agent inherits the same resolved keys.
    let retry = RetryPolicy::new(
        chat.max_provider_retries,
        u64::from(chat.provider_retry_base_ms),
    );

    Ok(ChatSetup {
        memory,
        sandbox,
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
    })
}

impl ChatService for ConfiguredChatService {
    #[cfg(test)]
    fn cross_service_reach(&self) -> bool {
        self.xservice.is_some()
    }

    fn start_turn(&self, question: String, thread_id: Option<i64>) -> ChatStream {
        let (tx, rx) = unbounded_chat_channel();
        let engine = Arc::clone(&self.engine);
        let root = engine.root().to_path_buf();
        let workspace_root = self.workspace_root.clone();
        let xservice = self.xservice.clone();
        let setup_question = question.clone();

        let turn_root = root.clone();

        let handle = tokio::spawn(async move {
            // Blocking config/store/sandbox setup off the async executor thread
            // ([ADR-03]); a configure-first or setup fault is an honest single
            // `error` frame, never a crash ([NFR-CC-04]).
            let setup = match tokio::task::spawn_blocking(move || {
                build_setup(&root, workspace_root.as_deref(), thread_id, &setup_question)
            })
            .await
            {
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

            let ChatSetup {
                memory,
                sandbox,
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
            } = setup;
            let grounding: Arc<dyn SynthesizerGrounding> =
                Arc::new(MemoryGrounding::new(Arc::clone(&memory), thread_id, turn));
            // Where this turn's scratchpad AND its durable answer are written
            // ([FR-UI-26] AC-2) — the same `.logos/chat.db` the setup recorded the
            // user's question in.
            let target = TurnTarget::new(turn_root, thread_id, turn);

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
            // monomorphizes `launch`; both run the same orchestrated turn.
            match provider {
                ChatProvider::Anthropic => match anthropic_completion_model(&cfg, retry) {
                    Ok(model) => {
                        launch(engine, sandbox, xservice, model, grounding, budget, temperature,
                            max_tokens, question, memory, target, tx).await
                    }
                    Err(e) => {
                        let _ = tx.send(ChatFrame::Error(format!(
                            "could not build the Anthropic provider: {e}"
                        )));
                    }
                },
                ChatProvider::OpenAi => match openai_compatible_completion_model(&cfg, retry) {
                    Ok(model) => {
                        launch(engine, sandbox, xservice, model, grounding, budget, temperature,
                            max_tokens, question, memory, target, tx).await
                    }
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
}

/// Build the roster (grounded on the turn's memory) and orchestrator over `model`,
/// then run the streamed turn — generic over the provider model so both provider
/// families share one code path. The fixed roster shares the top-level model across
/// all four roles; per-role `[chat.models]` overrides ([FR-CF-06]) are a deferred
/// refinement.
///
/// Under a federated backing (`xservice` is `Some`) the Graph-Navigator gains the
/// `xservice_*` tools and the planner the workspace addendum that routes a
/// cross-repository question to them; otherwise both are exactly today's
/// ([S-431], [ADR-52]).
///
/// [S-431]: ../../../docs/planning/journal.md#s-431-the-chat-agents-tool-surface-is-workspace-aware
/// [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
#[allow(clippy::too_many_arguments)]
async fn launch<M>(
    engine: Arc<Engine>,
    sandbox: Arc<Sandbox>,
    xservice: Option<XserviceBacking>,
    model: M,
    grounding: Arc<dyn SynthesizerGrounding>,
    budget: BudgetTree,
    temperature: Option<f64>,
    max_tokens: Option<u64>,
    question: String,
    memory: Arc<MemoryStore>,
    target: TurnTarget,
    tx: UnboundedSender<ChatFrame>,
) where
    M: CompletionModel + Clone + Send + Sync + 'static,
{
    let roster = SubagentRoster::new(engine, sandbox, model.clone())
        .with_xservice(xservice)
        .with_temperature(temperature)
        .with_max_tokens(max_tokens)
        .with_synthesizer_grounding(grounding);
    // The roster names the planner preamble that matches its tools.
    let planner = Planner::with_preamble(model, roster.planner_preamble());
    let orchestrator = Orchestrator::with_planner(planner, roster, budget);
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
                if setup.model_id == WS_MODEL {
                    assert_eq!(
                        setup.api_key, WS_KEY,
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
        assert_eq!(setup.model_id, WS_MODEL);
        assert_eq!(setup.api_key, WS_KEY);
    }

    /// Per-half inheritance reaches the turn in the one allowed direction: a
    /// member's own model dials with the workspace's key.
    #[test]
    fn a_member_owned_policy_dials_with_the_inherited_workspace_key() {
        let e = estate(Half::Declared, Half::Absent, Half::Declared, Half::Declared);
        let setup = build_setup(&e.member, Some(&e.ws), None, "q").expect("a turn");
        assert_eq!(
            (setup.model_id.as_str(), setup.api_key.as_str()),
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
            (setup.model_id.as_str(), setup.api_key.as_str()),
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
            (setup.model_id.as_str(), setup.api_key.as_str()),
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
        assert_eq!(setup.model_id, WS_MODEL);
        assert_eq!(setup.api_key, WS_KEY);
        assert_eq!(setup.provider, ChatProvider::Anthropic);
        assert_eq!(setup.base_url, "https://workspace.example/v1");
        assert_eq!(setup.temperature, Some(0.3));
        assert_eq!(setup.max_tokens, Some(777));
        assert_eq!(
            (
                setup.budget.global_limit(),
                setup.budget.max_subagent_tool_calls(),
                setup.budget.max_replans()
            ),
            (11, 5, 2)
        );
        assert_eq!(setup.retry, RetryPolicy::new(4, 321));
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
            (setup.model_id.as_str(), setup.api_key.as_str()),
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

    /// [S-431]: `launch` — the seam every production turn goes through — hands
    /// the federated backing to the roster, so a turn over a workspace dispatches
    /// `xservice_*` and its observation carries the reading; the single-root twin
    /// gets no such tool, and the same scripted call is refused as out-of-domain.
    /// Mock provider throughout: nothing dials.
    ///
    /// [S-431]: ../../../docs/planning/journal.md#s-431-the-chat-agents-tool-surface-is-workspace-aware
    mod xservice_launch {
        use std::sync::Arc;

        use agent_core::{MockCompletionModel, MockTurn, Sandbox, XserviceBacking};
        use chat_agent::{
            BudgetTree, ChatStore, MemoryGrounding, MemoryStore, OrchestratorEvent, StepRole,
            SynthesizerGrounding,
        };
        use logos_core::federation::{
            Backing, ContractBridge, EngineRegistry, Federation, Member, RegistryMode,
        };
        use logos_core::Engine;
        use tempfile::TempDir;

        use super::super::launch;
        use crate::chat::{unbounded_chat_channel, ChatFrame, TurnTarget};

        /// A two-member workspace backing over empty member directories — enough
        /// to prove the wiring: the reading names the fan-out's member count.
        fn federated(root: &std::path::Path) -> Option<XserviceBacking> {
            let members = ["api", "web"]
                .into_iter()
                .map(|name| {
                    std::fs::create_dir_all(root.join(name)).unwrap();
                    Member { name: name.to_string(), root: root.join(name) }
                })
                .collect();
            let federation = Federation {
                name: "shop".to_string(),
                root: root.to_path_buf(),
                members,
                default: None,
                links: Vec::new(),
                governance: Default::default(),
                warm_concurrency: None,
            };
            let registry = EngineRegistry::<Engine>::new(federation, RegistryMode::Lazy);
            XserviceBacking::federated(
                Arc::new(Backing::Federated(Box::new(registry))),
                Arc::new(ContractBridge::new()),
            )
        }

        /// Run one scripted turn through `launch` — over a federated backing or a
        /// single root — and return the Graph-Navigator's observation.
        async fn navigator_observation(workspace: bool) -> (String, Vec<Option<String>>) {
            let tmp = TempDir::new().unwrap();
            let project = tmp.path().join("project");
            std::fs::create_dir_all(project.join("src")).unwrap();
            std::fs::write(project.join("src/lib.rs"), "pub fn alpha() {}\n").unwrap();
            let xservice = if workspace { federated(tmp.path()) } else { None };
            assert_eq!(xservice.is_some(), workspace);

            let engine = Arc::new(Engine::start(&project).expect("engine"));
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
                xservice,
                model,
                grounding,
                BudgetTree::new(48, 16, 3),
                None,
                None,
                "find alpha across services".to_string(),
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

        #[tokio::test]
        async fn a_workspace_turn_dispatches_xservice_through_launch() {
            let (observation, prompts) = navigator_observation(true).await;
            // The planner ran under the workspace preamble, the Synthesizer (the
            // last request) under its cross-service addendum.
            assert_eq!(
                prompts.first().cloned().flatten().as_deref(),
                Some(chat_agent::workspace_planner_preamble().as_str())
            );
            let synthesizer = prompts.last().cloned().flatten().unwrap_or_default();
            assert!(
                synthesizer.ends_with(chat_agent::SYNTHESIZER_XSERVICE_ADDENDUM),
                "{synthesizer}"
            );
            assert!(
                observation.contains("xservice_search \"alpha\" over 2 member(s)"),
                "{observation}"
            );
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
