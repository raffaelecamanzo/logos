//! `wiki-agent` — the `ui`-gated, single-purpose **in-process wiki generator**
//! ([CR-047], [FR-WK-18], [ADR-42]).
//!
//! A single tool-less `rig` [`Agent`](agent_core::rig::agent::Agent) on the
//! shared [`agent-core`] substrate that loops the deterministic FR-WK-13
//! generation queue ([`Engine::wiki_generate`](logos_core::Engine::wiki_generate)),
//! synthesizes each drifted/absent page using the embedded `logos-wiki` skill
//! body ([`wiki::rendered_skill`](logos_core::wiki::rendered_skill), [FR-WK-08])
//! **as its system prompt**, and persists each page via the unchanged `wiki
//! write` contract ([`Engine::wiki_write`](logos_core::Engine::wiki_write),
//! [FR-WK-02]) — write-time anchors, HEAD, and built-at-revision — so dual-axis
//! freshness ([FR-WK-03], [FR-WK-12]) is preserved.
//!
//! It deliberately is **not** the chat planner / subagent roster / multi-step
//! memory ([chat-agent], [ADR-41]): wiki generation is a bounded,
//! deterministically-ordered batch, so this is a single queue-driven loop that
//! **auto-continues** across per-chunk budget slices — re-reading the deterministic
//! FR-WK-13 queue between chunks ([NFR-RA-06]) — until the work-list drains,
//! bounded by a **hard safety ceiling** on total pages per run
//! ([`DEFAULT_RUN_CEILING`], [ADR-42], [CR-056]). An empty work-list starts no run
//! and makes no model call ([NFR-CC-04]); a ceiling or provider halt is reported
//! honestly, and pages already written persist (each
//! [`wiki write`](logos_core::Engine::wiki_write) is atomic).
//!
//! # Offline carve-out ([NFR-SE-01], [ADR-40])
//!
//! The substrate's `rig`/`reqwest` HTTP client is confined to [`agent-core`], and
//! this crate — like [chat-agent] — reaches the `logos` binary only through the
//! `web` adapter under the non-default `ui` cargo feature. The **default**
//! `logos` tree links no HTTP client and the byte-identical no-networking-crate
//! fitness function (`logos-core/tests/no_network_deps.rs`) is unchanged;
//! `tests/carve_out.rs` locks the ui-vs-default boundary and
//! `tests/generation.rs` proves a full mock-`CompletionModel` pass dials nothing.
//!
//! [CR-047]: ../../../docs/requests/CR-047-internal-wiki-generation-on-agent-substrate.md
//! [CR-056]: ../../../docs/requests/CR-056-wiki-generation-usability.md
//! [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
//! [FR-WK-18]: ../../../docs/specs/requirements/FR-WK-18.md
//! [FR-WK-02]: ../../../docs/specs/requirements/FR-WK-02.md
//! [FR-WK-03]: ../../../docs/specs/requirements/FR-WK-03.md
//! [FR-WK-08]: ../../../docs/specs/requirements/FR-WK-08.md
//! [FR-WK-12]: ../../../docs/specs/requirements/FR-WK-12.md
//! [NFR-SE-01]: ../../../docs/specs/requirements/NFR-SE-01.md
//! [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
//! [ADR-40]: ../../../docs/specs/architecture/decisions/ADR-40.md
//! [ADR-41]: ../../../docs/specs/architecture/decisions/ADR-41.md
//! [ADR-42]: ../../../docs/specs/architecture/decisions/ADR-42.md
//! [`agent-core`]: ../../../docs/specs/architecture/components/agent-core.md
//! [chat-agent]: ../../../docs/specs/architecture/components/chat-agent.md

#![forbid(unsafe_code)]

mod agent;
mod configured;
mod grounding;

pub use agent::{
    WikiAgent, WikiProgress, WikiRunOutcome, DEFAULT_GROUNDING_BUDGET, DEFAULT_RUN_BUDGET,
    DEFAULT_RUN_CEILING, DEFAULT_SYNTHESIS_TIMEOUT,
};
pub use configured::{run_configured, ConfiguredRun};
