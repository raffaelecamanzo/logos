//! The **workspace chat** service ([S-482], [FR-WS-34], [ADR-71]): the workspace
//! roster ([S-481]) served on its own route, configured from the workspace tier
//! alone and stored at the workspace root.
//!
//! It is the member chat's production turn with three things swapped, and
//! nothing else ([`spawn_configured_turn`] is the one turn body both run):
//!
//! - **the roster** — [`WorkspaceRoster`] over the surface's [`XserviceBacking`],
//!   its planner wired with [`WorkspaceRoster::planner_preamble`] exactly as the
//!   member chat wires its own ([`launch_workspace`]);
//! - **the config** — [`resolve_chat`] at the workspace root with **no** tier
//!   above it ([FR-WS-30] as clarified by [CR-155], [ADR-67]): the seam is called
//!   with no member half, so no member's `[chat]` ever drives this chat. Its
//!   origins are therefore relative to the workspace root (`member` means
//!   *declared there*), and an incomplete tier is the configure-first state
//!   naming the workspace root, the missing half and Workspace Config
//!   ([`workspace_turn_provider`]);
//! - **the store** — `<workspace root>/.logos/chat.db`. No member's `chat.db` is
//!   opened: the repo-addressed tools read member graphs and sources, never a
//!   member's conversation store. The workspace root's generated ignore rules
//!   cover the store (`federation::enable`'s `maintain_root_ignore`).
//!
//! The service exists only over a federated backing — [`XserviceBacking`] cannot
//! be minted under a single root — so a single-root serve has no workspace chat
//! and its routes answer `404` ([ADR-52]).
//!
//! [S-482]: ../../../docs/planning/journal.md#s-482-the-workspace-chat-is-its-own-service-route-and-store
//! [S-481]: ../../../docs/planning/journal.md#s-481-a-workspace-roster-centred-on-the-workspace-and-the-member-roster-single-backing-only
//! [FR-WS-34]: ../../../docs/specs/requirements/FR-WS-34.md
//! [FR-WS-30]: ../../../docs/specs/requirements/FR-WS-30.md
//! [CR-155]: ../../../docs/requests/CR-155-workspace-chat-narrows-to-a-member.md
//! [ADR-71]: ../../../docs/specs/architecture/decisions/ADR-71.md
//! [ADR-67]: ../../../docs/specs/architecture/decisions/ADR-67.md
//! [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md

use std::future::Future;
use std::path::{Path, PathBuf};

use agent_core::rig::completion::CompletionModel;
use agent_core::XserviceBacking;
use chat_agent::{Orchestrator, Planner, WorkspaceRoster};
use logos_core::config::{resolve_chat, ChatOrigin, ChatResolution};
use tokio::sync::mpsc::UnboundedSender;

use super::configured::{prepare_turn, spawn_configured_turn, RosterLaunch, TurnRun, TurnSetup};
use super::{
    resolution_fault, run_orchestrated, usable_provider, ChatFrame, ChatService, ChatStream,
    TurnProvider,
};

/// The production workspace chat service over the surface's federated query
/// backing ([S-482]).
///
/// [S-482]: ../../../docs/planning/journal.md#s-482-the-workspace-chat-is-its-own-service-route-and-store
pub(crate) struct WorkspaceChatService {
    xservice: XserviceBacking,
}

impl WorkspaceChatService {
    /// Serve the workspace roster over `xservice` — the surface's registry,
    /// bridge and build-dependency cache — rooted at its federation's root.
    pub(crate) fn new(xservice: XserviceBacking) -> Self {
        Self { xservice }
    }

    /// The workspace root: the config root this chat resolves at and the root
    /// whose `.logos/chat.db` holds its conversations. Taken from the registry
    /// that already resolved it, never discovered.
    fn workspace_root(&self) -> PathBuf {
        self.xservice.registry().federation().root.clone()
    }
}

impl ChatService for WorkspaceChatService {
    fn start_turn(&self, question: String, thread_id: Option<i64>) -> ChatStream {
        let root = self.workspace_root();
        let xservice = self.xservice.clone();
        let setup_root = root.clone();
        let setup_question = question.clone();
        spawn_configured_turn(root, question, move || {
            let turn = build_workspace_setup(&setup_root, thread_id, &setup_question)?;
            Ok((turn, WorkspaceLaunch(xservice)))
        })
    }
}

/// The workspace chat's blocking setup: the policy and credential from the
/// workspace tier alone, then the conversation at the workspace root
/// ([`prepare_turn`], shared with the member chat). A refused turn touches no
/// store, as the member chat's does not.
fn build_workspace_setup(
    workspace_root: &Path,
    thread_id: Option<i64>,
    question: &str,
) -> Result<TurnSetup, String> {
    // No tier above the workspace root: `None` is what makes "the workspace tier
    // alone" structural — no member root is passed, so no member is read.
    let resolution =
        resolve_chat(workspace_root, None).map_err(|e| resolution_fault("chat", &e))?;
    let provider = workspace_turn_provider(workspace_root, &resolution)?;
    prepare_turn(workspace_root, provider, resolution.policy, thread_id, question)
}

/// The workspace chat's readiness verdict ([FR-WS-34], [NFR-CC-04]): the
/// provider to dial, or the configure-first text a refused turn streams as its
/// single frame. The verdict is the member chat's ([`usable_provider`]); only
/// the wording differs — it names the workspace root, the missing half and
/// Workspace Config, and says that a member's own `[chat]` does not configure
/// this chat, since a member declaring one is the likeliest way to end up here.
///
/// [FR-WS-34]: ../../../docs/specs/requirements/FR-WS-34.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
pub(crate) fn workspace_turn_provider(
    workspace_root: &Path,
    resolution: &ChatResolution,
) -> Result<TurnProvider, String> {
    usable_provider(resolution).ok_or_else(|| configure_first_message(workspace_root, resolution))
}

/// The workspace configure-first frame text: the root, the absent half (or
/// both), a present half, and the Workspace Config action that fixes it.
fn configure_first_message(workspace_root: &Path, resolution: &ChatResolution) -> String {
    let declared = |origin: ChatOrigin| origin != ChatOrigin::Unset;
    let (absent, present, action) =
        match (declared(resolution.policy_origin), declared(resolution.credential_origin)) {
            (false, false) => (
                "neither a provider model nor an API key is declared",
                "",
                "Choose a provider model and add an API key",
            ),
            (false, true) => (
                "no provider model is declared",
                "; its API key is declared",
                "Choose a provider model",
            ),
            (true, _) => ("no API key is declared", "; its provider model is declared", "Add an API key"),
        };
    format!(
        "The workspace chat is not configured yet for the workspace root {} — {absent} \
         there{present}. A member's own [chat] does not configure the workspace chat. \
         {action} in Workspace Config before starting a turn.",
        workspace_root.display()
    )
}

/// The workspace chat's roster backing.
struct WorkspaceLaunch(XserviceBacking);

impl RosterLaunch for WorkspaceLaunch {
    fn launch<M>(
        self,
        model: M,
        run: TurnRun,
        tx: UnboundedSender<ChatFrame>,
    ) -> impl Future<Output = ()> + Send
    where
        M: CompletionModel + Clone + Send + Sync + 'static,
    {
        launch_workspace(self.0, model, run, tx)
    }
}

/// Build the workspace roster (grounded on the turn's memory) and orchestrator
/// over `model`, then run the streamed turn — the member chat's
/// `configured::launch` over [`WorkspaceRoster`] ([S-481]). The planner runs
/// under the workspace planner preamble, which names every member with its
/// declared kind, and is shown the thread's prior turns ([S-483]).
///
/// [S-481]: ../../../docs/planning/journal.md#s-481-a-workspace-roster-centred-on-the-workspace-and-the-member-roster-single-backing-only
/// [S-483]: ../../../docs/planning/journal.md#s-483-follow-up-turns-see-prior-turns
async fn launch_workspace<M>(
    xservice: XserviceBacking,
    model: M,
    run: TurnRun,
    tx: UnboundedSender<ChatFrame>,
) where
    M: CompletionModel + Clone + Send + Sync + 'static,
{
    let TurnRun { grounding, budget, temperature, max_tokens, question, history, memory, target } =
        run;
    let roster = WorkspaceRoster::new(xservice, model.clone())
        .with_temperature(temperature)
        .with_max_tokens(max_tokens)
        .with_synthesizer_grounding(grounding);
    let planner = Planner::with_preamble(model, roster.planner_preamble());
    let orchestrator = Orchestrator::with_planner(planner, roster, budget).with_history(history);
    run_orchestrated(orchestrator, question, memory, target, tx).await;
}

#[cfg(test)]
mod tests {
    //! The workspace chat reads the workspace tier alone, stores at the workspace
    //! root, and runs the workspace roster's turn through the launch path every
    //! production turn takes ([S-482]). Mock provider throughout: nothing dials.
    //!
    //! [S-482]: ../../../docs/planning/journal.md#s-482-the-workspace-chat-is-its-own-service-route-and-store

    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use agent_core::{MockCompletionModel, MockTurn, XserviceBacking};
    use chat_agent::{ChatRole, ChatStore, OrchestratorEvent};
    use logos_core::federation::{
        Backing, ContractBridge, EngineRegistry, Federation, Member, RegistryMode,
    };
    use logos_core::Engine;
    use tempfile::TempDir;

    use super::{build_workspace_setup, launch_workspace};
    use crate::chat::{unbounded_chat_channel, ChatFrame};

    const WS_MODEL: &str = "workspace/model";
    const WS_KEY: &str = "sk-workspace-only-ws42";
    const MEMBER_MODEL: &str = "member/model";
    const MEMBER_KEY: &str = "sk-member-complete-mb77";

    /// A workspace root with two members nested inside it; `svc` declares a
    /// **complete** `[chat]` of its own (model, key, and window keys that differ
    /// from any the workspace sets), so a resolution that consulted it would show.
    struct Estate {
        _tmp: TempDir,
        ws: PathBuf,
        svc: PathBuf,
    }

    fn write(root: &Path, file: &str, body: &str) {
        fs::create_dir_all(root.join(".logos")).unwrap();
        fs::write(root.join(".logos").join(file), body).unwrap();
    }

    fn estate() -> Estate {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path().to_path_buf();
        let svc = ws.join("svc");
        fs::create_dir_all(ws.join("web")).unwrap();
        write(
            &svc,
            "config.toml",
            &format!("[chat]\nmodel = \"{MEMBER_MODEL}\"\nhistory_max_turns = 6\n"),
        );
        write(&svc, "secrets.toml", &format!("[chat]\napi_key = \"{MEMBER_KEY}\"\n"));
        Estate { _tmp: tmp, ws, svc }
    }

    fn refusal(e: &Estate) -> String {
        build_workspace_setup(&e.ws, None, "q")
            .err()
            .expect("an incomplete workspace tier refuses the turn")
    }

    /// AC-2: a member's complete `[chat]` under an empty workspace tier is the
    /// configure-first state — naming the workspace root, both missing halves and
    /// Workspace Config — never a turn dialling the member's model or key.
    #[test]
    fn a_complete_member_chat_under_an_empty_workspace_tier_is_configure_first() {
        let e = estate();
        let m = refusal(&e);
        let ws = e.ws.display().to_string();
        assert!(
            m.starts_with(&format!(
                "The workspace chat is not configured yet for the workspace root {ws} — \
                 neither a provider model nor an API key is declared there."
            )),
            "{m}"
        );
        assert!(m.contains("A member's own [chat] does not configure the workspace chat."), "{m}");
        assert!(
            m.ends_with("Choose a provider model and add an API key in Workspace Config before starting a turn."),
            "{m}"
        );
        assert!(!m.contains(MEMBER_MODEL) && !m.contains("mb77"), "nothing of the member's: {m}");
        assert!(
            !e.ws.join(".logos/chat.db").exists(),
            "a refused turn opens no store at the workspace root"
        );
        assert!(!e.svc.join(".logos/chat.db").exists(), "nor at a member");
    }

    /// Each missing half is named on its own, with the present one stated.
    #[test]
    fn a_half_declared_workspace_tier_names_the_missing_half() {
        let e = estate();
        write(&e.ws, "config.toml", &format!("[chat]\nmodel = \"{WS_MODEL}\"\n"));
        let m = refusal(&e);
        assert!(
            m.contains("— no API key is declared there; its provider model is declared."),
            "{m}"
        );
        assert!(m.contains("Add an API key in Workspace Config"), "{m}");

        fs::remove_file(e.ws.join(".logos/config.toml")).unwrap();
        write(&e.ws, "secrets.toml", &format!("[chat]\napi_key = \"{WS_KEY}\"\n"));
        let m = refusal(&e);
        assert!(
            m.contains("— no provider model is declared there; its API key is declared."),
            "{m}"
        );
        assert!(m.contains("Choose a provider model in Workspace Config"), "{m}");
        assert!(!m.contains(WS_KEY) && !m.contains("ws42"), "never echoes the key: {m}");
    }

    /// AC-2 and AC-3: a complete workspace tier dials its own table and key — the
    /// member's complete `[chat]` notwithstanding — and the conversation lands in
    /// `<workspace root>/.logos/chat.db`, with no member store created.
    #[test]
    fn a_complete_workspace_tier_dials_its_own_table_and_stores_at_the_root() {
        let e = estate();
        write(
            &e.ws,
            "config.toml",
            &format!("[chat]\nmodel = \"{WS_MODEL}\"\nbase_url = \"https://workspace.example/v1\"\n"),
        );
        write(&e.ws, "secrets.toml", &format!("[chat]\napi_key = \"{WS_KEY}\"\n"));
        let setup = build_workspace_setup(&e.ws, None, "which services exist?").expect("a turn");
        assert_eq!((setup.model_id.as_str(), setup.api_key.as_str()), (WS_MODEL, WS_KEY));
        assert_eq!(setup.base_url, "https://workspace.example/v1");

        let store = ChatStore::open(&e.ws).expect("the workspace store");
        let messages = store.messages(setup.thread_id).expect("messages");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content, "which services exist?");
        assert!(!e.svc.join(".logos/chat.db").exists(), "no member store is created");
    }

    /// [S-483]'s window on the workspace chat takes its bounds from the
    /// **workspace** tier: the workspace keeps one prior turn while the member
    /// keeps six, so a third turn sees exactly one and states one omitted.
    ///
    /// [S-483]: ../../../docs/planning/journal.md#s-483-follow-up-turns-see-prior-turns
    #[test]
    fn the_prior_turn_window_is_bounded_by_the_workspace_tiers_keys() {
        let e = estate();
        write(
            &e.ws,
            "config.toml",
            &format!("[chat]\nmodel = \"{WS_MODEL}\"\nhistory_max_turns = 1\n"),
        );
        write(&e.ws, "secrets.toml", &format!("[chat]\napi_key = \"{WS_KEY}\"\n"));
        let first = build_workspace_setup(&e.ws, None, "first question").expect("turn 1");
        let thread = first.thread_id;
        let answer = |text: &str| {
            ChatStore::open(&e.ws)
                .unwrap()
                .append_message(thread, ChatRole::Assistant, text, &[])
                .unwrap();
        };
        answer("first answer");
        build_workspace_setup(&e.ws, Some(thread), "second question").expect("turn 2");
        answer("second answer");
        let third = build_workspace_setup(&e.ws, Some(thread), "third question").expect("turn 3");
        assert_eq!(third.history.turns().len(), 1, "{:?}", third.history);
        assert_eq!(third.history.turns()[0].user, "second question");
        assert_eq!(third.history.omitted(), 1);
    }

    /// A lazy two-member federation over `ws` (`svc`, `web`), as the serve's
    /// registry holds it.
    fn xservice(ws: &Path) -> XserviceBacking {
        let federation = Federation {
            name: "shop".to_string(),
            root: ws.to_path_buf(),
            members: ["svc", "web"]
                .into_iter()
                .map(|name| Member { name: name.to_string(), root: ws.join(name) })
                .collect(),
            default: None,
            links: Vec::new(),
            governance: Default::default(),
            warm_concurrency: None,
            member_kinds: Default::default(),
        };
        let backing = Arc::new(Backing::Federated(Box::new(EngineRegistry::<Engine>::new(
            federation,
            RegistryMode::Lazy,
        ))));
        XserviceBacking::federated(backing, Arc::new(ContractBridge::new())).expect("federated")
    }

    /// One scripted workspace turn: plan one Workspace-Analyst step, observe it,
    /// finalize, answer `answer`.
    fn scripted(answer: &str) -> MockCompletionModel {
        MockCompletionModel::new([
            MockTurn::text(
                r#"{"action":"plan","steps":[{"role":"workspace_analyst","instruction":"list the members"}]}"#,
            ),
            MockTurn::text("two members: svc and web."),
            MockTurn::text(r#"{"action":"final","grounded":true}"#),
            MockTurn::text(answer),
        ])
    }

    /// Run one turn of `thread` (a new one when `None`) through the setup and the
    /// launch path — what `start_turn` does once the provider is built — and
    /// return the frames it streamed.
    async fn turn(
        ws: &Path,
        xs: &XserviceBacking,
        model: MockCompletionModel,
        thread: Option<i64>,
        question: &str,
    ) -> (i64, Vec<ChatFrame>) {
        let setup = build_workspace_setup(ws, thread, question).expect("a configured tier");
        let thread_id = setup.thread_id;
        let (run, _dial) = setup.into_run(ws.to_path_buf(), question.to_string());
        let (tx, mut rx) = unbounded_chat_channel();
        launch_workspace(xs.clone(), model, run, tx).await;
        let mut frames = Vec::new();
        while let Ok(frame) = rx.try_recv() {
            frames.push(frame);
        }
        (thread_id, frames)
    }

    /// The launch path wires the workspace roster as S-481 wired it by hand
    /// (`chat-agent`'s
    /// `a_workspace_turn_runs_the_planner_and_synthesizer_under_the_workspace_preambles`):
    /// the planner under the workspace planner preamble, the analyst under its
    /// own, the Synthesizer under the workspace one. And turn 2's planner prompt
    /// carries turn 1 — its question and the answer the launch path persisted
    /// ([S-483]).
    ///
    /// [S-483]: ../../../docs/planning/journal.md#s-483-follow-up-turns-see-prior-turns
    #[tokio::test]
    async fn the_launch_path_runs_the_workspace_roster_and_turn_two_sees_turn_one() {
        let e = estate();
        write(&e.ws, "config.toml", &format!("[chat]\nmodel = \"{WS_MODEL}\"\n"));
        write(&e.ws, "secrets.toml", &format!("[chat]\napi_key = \"{WS_KEY}\"\n"));
        let xs = xservice(&e.ws);
        let federation = xs.registry().federation().clone();

        let one = scripted("ANSWER-ONE: svc and web");
        let (thread, frames) = turn(&e.ws, &xs, one.clone(), None, "which members are there?").await;
        assert!(
            frames.iter().any(|f| matches!(
                f,
                ChatFrame::Event(OrchestratorEvent::FinalAnswer { answer }) if answer.starts_with("ANSWER-ONE")
            )),
            "{frames:?}"
        );
        let prompts = one.system_prompts();
        assert_eq!(
            prompts.first().cloned().flatten(),
            Some(chat_agent::workspace_planner_preamble(&federation))
        );
        let analyst = prompts.get(1).cloned().flatten().unwrap_or_default();
        assert!(analyst.starts_with(chat_agent::WORKSPACE_ANALYST_PREAMBLE), "{analyst}");
        assert_eq!(
            prompts.last().cloned().flatten(),
            Some(chat_agent::workspace_synthesizer_preamble(&federation))
        );
        let first_planner_prompt = one.user_prompts().first().cloned().flatten().unwrap_or_default();
        assert!(!first_planner_prompt.contains("ANSWER-ONE"), "turn 1 has no window");

        let two = scripted("ANSWER-TWO");
        let (same, _) = turn(&e.ws, &xs, two.clone(), Some(thread), "and web alone?").await;
        assert_eq!(same, thread);
        let planner_prompt = two.user_prompts().first().cloned().flatten().unwrap_or_default();
        assert!(planner_prompt.contains("which members are there?"), "{planner_prompt}");
        assert!(planner_prompt.contains("ANSWER-ONE: svc and web"), "{planner_prompt}");
        assert_eq!(xs.registry().resident_count(), 0, "neither turn opened a member");
        assert!(!e.svc.join(".logos/chat.db").exists(), "no member store is touched");
    }
}
