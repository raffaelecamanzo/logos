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
use agent_core::Sandbox;
use chat_agent::{
    BudgetTree, ChatRole, ChatStore, MemoryGrounding, MemoryStore, Orchestrator, SubagentRoster,
    SynthesizerGrounding,
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
    fn start_turn(&self, question: String, thread_id: Option<i64>) -> ChatStream {
        let (tx, rx) = unbounded_chat_channel();
        let engine = Arc::clone(&self.engine);
        let root = engine.root().to_path_buf();
        let workspace_root = self.workspace_root.clone();
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
                        launch(engine, sandbox, model, grounding, budget, temperature,
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
                        launch(engine, sandbox, model, grounding, budget, temperature,
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
    let orchestrator = Orchestrator::new(model, roster, budget);
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
    /// configure-first state, never a setup fault.
    fn turn_verdict(member: &Path, ws: Option<&Path>) -> bool {
        match build_setup(member, ws, None, "what is here?") {
            Ok(_) => true,
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

    /// Per-half inheritance reaches the turn: a member's own model with the
    /// workspace's key, and the mirror.
    #[test]
    fn each_half_is_dialled_from_the_root_that_declares_it() {
        let e = estate(Half::Declared, Half::Absent, Half::Declared, Half::Declared);
        let setup = build_setup(&e.member, Some(&e.ws), None, "q").expect("a turn");
        assert_eq!(
            (setup.model_id.as_str(), setup.api_key.as_str()),
            (MEMBER_MODEL, WS_KEY)
        );

        let e = estate(Half::Absent, Half::Declared, Half::Declared, Half::Declared);
        let setup = build_setup(&e.member, Some(&e.ws), None, "q").expect("a turn");
        assert_eq!(
            (setup.model_id.as_str(), setup.api_key.as_str()),
            (WS_MODEL, MEMBER_KEY)
        );
    }

    /// AC-2: tab verdict == turn verdict, asserted as ONE equality over the whole
    /// matrix — every member shape (absent / blank / declared, per half) × every
    /// workspace shape × with and without the workspace root passed.
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

        let e = estate(Half::Blank, Half::Declared, Half::Declared, Half::Absent);
        let setup = build_setup(&e.member, Some(&e.ws), None, "q").expect("a turn");
        assert_eq!(
            setup.model_id, WS_MODEL,
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
}
