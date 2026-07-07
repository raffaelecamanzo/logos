//! `agent-core` — the shared, `ui`-gated **`rig` agent substrate** (CR-046,
//! CR-047, [ADR-41], [ADR-42]).
//!
//! This crate owns the provider-agnostic foundation that both the chat-agent
//! and the wiki-agent build on: the **dual-provider clients** reached via
//! `rig` (native Anthropic + OpenAI-compatible with a configurable `base_url`),
//! and a **mock `CompletionModel`** for offline tests. It carries **no
//! orchestration policy** — the planner/subagent roster and the wiki queue-loop
//! live in their respective agent components ([ADR-01], NFR-MA-02).
//!
//! # Offline carve-out (NFR-SE-01, NFR-SE-07, [ADR-40])
//!
//! `rig` and its `reqwest` HTTP client are the only outbound-network machinery
//! in the workspace. They are confined here, and this crate reaches the `logos`
//! binary only through the `web` adapter, which links solely under the
//! non-default `ui` cargo feature. The result:
//!
//! - the **default** `logos` dependency tree links **no** HTTP client — the
//!   byte-identical no-networking-crate fitness function
//!   (`logos-core/tests/no_network_deps.rs`) is unchanged;
//! - the **`ui`** build links `rig`, and a mock-`CompletionModel` round-trip
//!   proves the offline path dials nothing (`tests/zero_egress.rs`);
//! - the carve-out boundary itself — `rig`/`reqwest` present under `ui`, absent
//!   by default — is locked by `tests/carve_out.rs`.
//!
//! [ADR-40]: the `ui`-gated outbound carve-out.
//! [ADR-41]: the `rig` decision + provider/mock strategy.
//! [ADR-42]: the extraction of this shared substrate.
//! [ADR-01]: thin-adapter discipline (no orchestration policy here).

#![forbid(unsafe_code)]

/// Re-export of the underlying `rig` framework so downstream agent crates
/// (chat-agent, wiki-agent) depend on one pinned `rig` through this substrate
/// rather than each taking a direct, independently-versioned dependency.
pub use rig_core as rig;

mod mock;
mod provider;
mod provider_error;
mod retry;
pub mod tools;

pub use mock::{MockCompletionModel, MockRawResponse, MockTurn};
pub use provider::{
    anthropic_completion_model, openai_compatible_completion_model, resolve_anthropic,
    resolve_openai_compatible, AnthropicClient, OpenAiCompatibleClient, PreflightError,
    ProviderConfig, ProviderKind, DEFAULT_ANTHROPIC_BASE_URL, DEFAULT_OPENAI_BASE_URL,
};
pub use provider_error::{classify_provider_error, ProviderErrorKind, ProviderFailure};
pub use retry::{
    RetryPolicy, RetryingModel, DEFAULT_MAX_PROVIDER_RETRIES, DEFAULT_PROVIDER_RETRY_BASE_MS,
};
pub use tools::{
    governance_toolset, graph_toolset, source_toolset, BoundedDispatcher, BudgetExhausted,
    DispatchError, Sandbox, SandboxError, ToolBudget, ToolCallError, ToolDomain,
};
