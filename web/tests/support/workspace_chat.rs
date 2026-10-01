//! The workspace-chat fixture the `/chat` route suites run their cases against
//! a second time ([S-482]): a real two-member workspace on disk, the workspace
//! router over it, and a scripted mock-provider [`ChatService`] driving the real
//! workspace roster, so `POST /workspace/chat` is exercised end-to-end with zero
//! egress exactly as `web::router_with_chat` exercises `/chat`.
//!
//! Included by `#[path]` from each suite that needs it; not every suite uses
//! every item.
//!
//! [S-482]: ../../../docs/planning/journal.md#s-482-the-workspace-chat-is-its-own-service-route-and-store

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use agent_core::{MockCompletionModel, MockTurn, XserviceBacking};
use chat_agent::{
    BudgetTree, ChatStore, MemoryGrounding, MemoryStore, Orchestrator, Planner, WorkspaceRoster,
};
use logos_core::federation::{discover, Backing, ContractBridge, EngineRegistry, RegistryMode};
use logos_core::Engine;
use tempfile::TempDir;
use web::chat::{spawn_turn, ChatService, ChatStream, TurnTarget};
use web::IntentToken;

/// Run `git` in `cwd` with an isolated identity and no auto-maintenance (a
/// detached maintenance run would race a test that snapshots the tree).
pub fn sh_git(cwd: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["-c", "user.email=dev@logos", "-c", "user.name=Logos Dev"])
        .args(["-c", "maintenance.auto=false", "-c", "gc.auto=0"])
        .args(["-c", "core.excludesFile=/dev/null"])
        .args(args)
        .output()
        .expect("git is on PATH");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `git status --porcelain` at `dir`.
pub fn porcelain(dir: &Path) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["-c", "core.excludesFile=/dev/null", "status", "--porcelain"])
        .output()
        .expect("git is on PATH");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A committed git repo — `discover` keeps only members that are git roots.
pub fn init_repo(dir: &Path) {
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/lib.rs"), "pub fn alpha() {}\n").unwrap();
    sh_git(dir, &["init", "-q", "-b", "main"]);
    sh_git(dir, &["add", "."]);
    sh_git(dir, &["commit", "-q", "-m", "init"]);
}

/// A two-member workspace (`api`, the default, and `web`) declaring no chat
/// configuration anywhere.
pub fn workspace() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    init_repo(&root.join("api"));
    init_repo(&root.join("web"));
    std::fs::write(
        root.join("logos.workspace.toml"),
        "[workspace]\nname = \"shop\"\nmembers = [\"api\", \"web\"]\ndefault = \"api\"\n",
    )
    .unwrap();
    tmp
}

/// The lazy member registry over the workspace at `root`.
pub fn registry(root: &Path) -> EngineRegistry<Engine> {
    let federation = discover(root)
        .expect("discovery succeeds")
        .expect("a workspace");
    EngineRegistry::<Engine>::new(federation, RegistryMode::Lazy)
}

/// The production workspace router over `root`, with its configured services.
pub fn router(root: &Path, intent: &IntentToken) -> axum::Router {
    web::workspace_router_with_intent(registry(root), intent.clone())
        .expect("the workspace router builds")
}

/// A [`ChatService`] running the **real** workspace roster and orchestrator over
/// the offline mock provider: one Workspace-Analyst step observing `observation`,
/// then the Synthesizer answering `answer` — persisted, as production persists,
/// to `<workspace root>/.logos/chat.db` through the genuine [`spawn_turn`].
pub struct ScriptedWorkspaceChat {
    pub xservice: XserviceBacking,
    pub root: PathBuf,
    pub observation: &'static str,
    pub answer: &'static str,
}

impl ScriptedWorkspaceChat {
    /// The scripted service over its own lazy registry of the workspace at `root`.
    pub fn over(root: &Path, observation: &'static str, answer: &'static str) -> Self {
        let backing = Arc::new(Backing::Federated(Box::new(registry(root))));
        let xservice = XserviceBacking::federated(backing, Arc::new(ContractBridge::new()))
            .expect("a federated backing");
        Self { xservice, root: root.to_path_buf(), observation, answer }
    }
}

impl ChatService for ScriptedWorkspaceChat {
    fn start_turn(&self, question: String, thread_id: Option<i64>) -> ChatStream {
        let mut store = ChatStore::open(&self.root).expect("open the workspace store");
        let thread = thread_id
            .unwrap_or_else(|| store.create_thread_from_message(&question).expect("thread"));
        drop(store);
        let memory = Arc::new(MemoryStore::open(&self.root).expect("open memory"));
        let turn = memory.next_turn(thread).expect("turn");

        // One model backs the planner and every role, consumed in order:
        // plan → the analyst's observation → final → the Synthesizer's answer.
        let model = MockCompletionModel::new([
            MockTurn::text(
                r#"{"action":"plan","steps":[{"role":"workspace_analyst","instruction":"gather grounding"}]}"#,
            ),
            MockTurn::text(self.observation),
            MockTurn::text(r#"{"action":"final","grounded":true}"#),
            MockTurn::text(self.answer),
        ]);
        let grounding = Arc::new(MemoryGrounding::new(Arc::clone(&memory), thread, turn));
        let roster = WorkspaceRoster::new(self.xservice.clone(), model.clone())
            .with_synthesizer_grounding(grounding);
        let planner = Planner::with_preamble(model, roster.planner_preamble());
        let orchestrator = Orchestrator::with_planner(planner, roster, BudgetTree::new(24, 8, 3));
        spawn_turn(orchestrator, question, memory, TurnTarget::new(self.root.clone(), thread, turn))
    }
}

/// A workspace router whose workspace chat is a [`ScriptedWorkspaceChat`] —
/// `/workspace/chat`'s counterpart of `router_with_chat` for `/chat`.
pub fn scripted_router(
    observation: &'static str,
    answer: &'static str,
) -> (TempDir, axum::Router, IntentToken) {
    let tmp = workspace();
    let intent = IntentToken::generate();
    let service: Arc<dyn ChatService> =
        Arc::new(ScriptedWorkspaceChat::over(tmp.path(), observation, answer));
    let router = web::workspace_router_with_chat(registry(tmp.path()), intent.clone(), service)
        .expect("the workspace router builds");
    (tmp, router, intent)
}
