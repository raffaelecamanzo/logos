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
//! [FR-CF-06]: ../../../docs/specs/requirements/FR-CF-06.md
//! [FR-UI-18]: ../../../docs/specs/requirements/FR-UI-18.md
//! [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
//! [`chat-agent`]: ../../../docs/specs/architecture/components/chat-agent.md
//! [`agent-core`]: ../../../docs/specs/architecture/components/agent-core.md

use std::path::Path;
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
use logos_core::config::{load_config_from_root, load_secrets_from_root, ChatProvider};
use logos_core::Engine;
use tokio::sync::mpsc::UnboundedSender;

use super::{run_orchestrated, unbounded_chat_channel, ChatFrame, ChatService, ChatStream};

/// The longest a thread title derived from the first user message may run.
const THREAD_TITLE_MAX: usize = 60;

/// The production chat service over the live [`Engine`] and the on-disk `[chat]`
/// policy + `secrets.toml` key ([FR-CF-06]).
pub(crate) struct ConfiguredChatService {
    engine: Arc<Engine>,
}

impl ConfiguredChatService {
    /// Build the service over the shared engine.
    pub(crate) fn new(engine: Arc<Engine>) -> Self {
        Self { engine }
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

/// Read config + secret, resolve/create the thread, open memory, and open the
/// source sandbox — the **blocking** half of a turn's setup. Returns an honest
/// configure-first / setup-fault message on failure ([NFR-CC-04]); runs on the
/// blocking pool ([ADR-03]).
fn build_setup(root: &Path, thread_id: Option<i64>, question: &str) -> Result<ChatSetup, String> {
    let config =
        load_config_from_root(root).map_err(|e| format!("could not read the chat config: {e}"))?;
    let chat = config.chat;

    // Configure-first ([FR-UI-18]): no model id and/or no key is not an error.
    let Some(model_id) = chat.model.clone() else {
        return Err(
            "Chat is not configured yet — choose a provider model in the Config tab before \
             starting a turn."
                .to_string(),
        );
    };
    let secrets =
        load_secrets_from_root(root).map_err(|e| format!("could not read the chat secret: {e}"))?;
    let api_key = match secrets.chat_api_key() {
        Some(key) if !key.trim().is_empty() => key.to_string(),
        _ => {
            return Err(
                "Chat is not configured yet — add an API key in the Config tab before starting \
                 a turn."
                    .to_string(),
            )
        }
    };

    // The thread the turn appends to (a new one when the caller gave none); the
    // scratchpad's foreign key requires the thread to exist first.
    let mut store =
        ChatStore::open(root).map_err(|e| format!("could not open the chat store: {e}"))?;
    let thread_id = match thread_id {
        Some(id) => id,
        None => store
            .create_thread(&thread_title(question))
            .map_err(|e| format!("could not create a chat thread: {e}"))?,
    };
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
        let setup_question = question.clone();

        let handle = tokio::spawn(async move {
            // Blocking config/store/sandbox setup off the async executor thread
            // ([ADR-03]); a configure-first or setup fault is an honest single
            // `error` frame, never a crash ([NFR-CC-04]).
            let setup = match tokio::task::spawn_blocking(move || {
                build_setup(&root, thread_id, &setup_question)
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
                            max_tokens, question, memory, thread_id, turn, tx).await
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
                            max_tokens, question, memory, thread_id, turn, tx).await
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
    thread_id: i64,
    turn: i64,
    tx: UnboundedSender<ChatFrame>,
) where
    M: CompletionModel + Clone + Send + Sync + 'static,
{
    let roster = SubagentRoster::new(engine, sandbox, model.clone())
        .with_temperature(temperature)
        .with_max_tokens(max_tokens)
        .with_synthesizer_grounding(grounding);
    let orchestrator = Orchestrator::new(model, roster, budget);
    run_orchestrated(orchestrator, question, memory, thread_id, turn, tx).await;
}

/// A single-line thread title from the first user message, truncated on a char
/// boundary to [`THREAD_TITLE_MAX`].
fn thread_title(question: &str) -> String {
    let line = question.lines().next().unwrap_or("").trim();
    let title: String = line.chars().take(THREAD_TITLE_MAX).collect();
    if title.is_empty() {
        "New chat".to_string()
    } else {
        title
    }
}
