//! `chat-agent` — the `ui`-gated agentic-chat component ([ADR-40], [ADR-41]).
//!
//! This crate owns the chat orchestration policy and conversation state the
//! thin [`web`] adapter must not ([ADR-01]). It is built incrementally across
//! Sprint 30:
//!
//! - **[S-168]:** the [`db`] conversation store — `.logos/chat.db` (threads +
//!   messages + tool-call/tool-result traces), its read/write paths, and a
//!   Clear-history wipe.
//! - **This story ([S-173]):** the [`orchestrator`] core — the LLM-driven
//!   [`Planner`](orchestrator::Planner) (a `rig` `Agent`), the
//!   plan→act→observe→replan loop ([`Orchestrator`](orchestrator::Orchestrator)),
//!   the [budget tree](orchestrator::BudgetTree) that halts honestly, and the
//!   plan/step-transition [events](orchestrator::OrchestratorEvent) the SSE seam
//!   (S-170) will stream.
//! - **[S-174]:** the fixed
//!   [subagent roster](orchestrator::SubagentRoster) — Graph-Navigator,
//!   Governance-Analyst, Source-Reader, and the tool-less Synthesizer — the real
//!   [`StepExecutor`](orchestrator::StepExecutor) the loop dispatches each plan
//!   step to, each a `rig`-`Agent`-shaped unit least-privileged to one
//!   [`agent-core`] tool domain (S-167).
//! - **[S-175]:** the [`memory`] store — the per-turn
//!   [scratchpad](memory::MemoryStore) (plan + per-subagent observations +
//!   findings) and per-thread working/conversation memory over the same
//!   `chat.db` (migration 2), with a [`ScratchpadSink`](memory::ScratchpadSink)
//!   that persists the orchestrator's events as they stream. No embeddings/RAG.
//!   The Synthesizer's grounding of the final answer from this memory is
//!   composed at the SSE seam (S-170).
//!
//! Like [`agent-core`], it compiles into the `logos` binary **only** under the
//! non-default `ui` cargo feature (it is reached solely through the `ui`-only
//! [`web`] adapter), so the default binary links none of it and stays provably
//! offline ([NFR-SE-01], [ADR-40]). The store itself pulls in no networking
//! crate (`rusqlite` is the same bundled SQLite the default tree already uses),
//! so the carve-out fitness functions are unaffected either way.
//!
//! [ADR-01]: ../../docs/specs/architecture/decisions/ADR-01.md
//! [ADR-40]: ../../docs/specs/architecture/decisions/ADR-40.md
//! [ADR-41]: ../../docs/specs/architecture/decisions/ADR-41.md
//! [NFR-SE-01]: ../../docs/specs/requirements/NFR-SE-01.md
//! [S-168]: ../../docs/planning/journal.md#s-168-chat-persistence-store-threads-messages-and-clear-history
//! [S-173]: ../../docs/planning/journal.md#s-173-planner-and-plan-act-observe-replan-orchestration-loop-with-budget-tree
//! [S-174]: ../../docs/planning/journal.md#s-174-specialized-subagent-roster-on-rig
//! [S-175]: ../../docs/planning/journal.md#s-175-multi-step-agent-memory-store-scratchpad-and-working-memory

#![forbid(unsafe_code)]

pub mod db;
pub mod memory;
pub mod orchestrator;

pub use db::{db_path, latest_version, ChatMessage, ChatRole, ChatStore, ChatThread, ToolTrace};
pub use memory::{MemoryGrounding, MemoryStore, ScratchpadEntry, ScratchpadSink};
pub use orchestrator::{
    BudgetBound, BudgetTree, CapturingSink, EventSink, FanOut, Orchestrator, OrchestratorError,
    OrchestratorEvent, PlanStep, Planner, PlannerDecision, RoleModels, StepContext, StepError,
    StepExecutor, StepObservation, StepRole, SubagentRoster, SynthesizerGrounding, TurnOutcome,
    GOVERNANCE_ANALYST_PREAMBLE, GRAPH_NAVIGATOR_PREAMBLE, SOURCE_READER_PREAMBLE,
    SYNTHESIZER_PREAMBLE,
};
