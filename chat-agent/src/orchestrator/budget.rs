//! The **budget tree** — the three-level bound that halts a chat turn honestly
//! ([S-173], [ADR-41] "Budget tree", [NFR-CC-04]).
//!
//! [ADR-41] replaces the flat iteration cap with a three-level budget, all
//! configurable in `[chat]` ([FR-CF-06]):
//!
//! - a **global per-turn tool-call ceiling** (`max_tool_calls`, default 24);
//! - a **per-subagent tool-call cap** (`max_subagent_tool_calls`, default 8);
//! - a **max-replans** bound (`max_replans`, default 3).
//!
//! Each level composes agent-core's atomic [`ToolBudget`] primitive (S-167): the
//! global ceiling is one shared `ToolBudget`; each step draws a fresh
//! per-subagent `ToolBudget` from [`BudgetTree::new_subagent_budget`]. Hitting
//! any bound stops the turn and reports **which** one was reached
//! ([`BudgetBound`]) — the orchestrator never loops unbounded and never
//! fabricates a result ([NFR-CC-04]). The replan bound is enforced by the
//! orchestrator loop itself (it counts planning rounds); this type owns the two
//! tool-call levels and the `max_replans` value the loop reads.
//!
//! [S-173]: ../../../docs/planning/journal.md#s-173-planner-and-plan-act-observe-replan-orchestration-loop-with-budget-tree
//! [ADR-41]: ../../../docs/specs/architecture/decisions/ADR-41.md
//! [FR-CF-06]: ../../../docs/specs/requirements/FR-CF-06.md
//! [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md

use agent_core::ToolBudget;
use logos_core::config::ChatConfig;
use serde::Serialize;

/// Which bound of the budget tree halted the turn ([NFR-CC-04]).
///
/// The honest halt names the exact limit that was reached so the surface can
/// report it to the user (and the stream can carry it, S-170) rather than the
/// run looping unbounded or fabricating an answer. Serialized tagged so the SSE
/// seam renders a stable discriminant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, thiserror::Error)]
#[serde(tag = "bound", rename_all = "snake_case")]
pub enum BudgetBound {
    /// The global per-turn tool-call ceiling (`max_tool_calls`) was reached.
    #[error("global per-turn tool-call ceiling reached ({limit} calls)")]
    GlobalToolCalls {
        /// The configured global ceiling.
        limit: usize,
    },
    /// A subagent reached its per-subagent tool-call cap
    /// (`max_subagent_tool_calls`).
    #[error("per-subagent tool-call cap reached ({limit} calls)")]
    SubagentToolCalls {
        /// The configured per-subagent cap.
        limit: usize,
    },
    /// The planner exceeded the max-replans bound (`max_replans`).
    #[error("max replans reached ({limit} replans)")]
    Replans {
        /// The configured max-replans bound.
        limit: u32,
    },
}

/// The three-level tool-call + replan budget bounding one chat turn ([ADR-41]).
///
/// Owns the **global ceiling** (a shared [`ToolBudget`]) and the **per-subagent
/// cap** value (a fresh `ToolBudget` is minted per step); the **max-replans**
/// bound is a value the orchestrator loop reads to bound its planning rounds.
#[derive(Debug)]
pub struct BudgetTree {
    /// The shared global per-turn tool-call ceiling.
    global: ToolBudget,
    /// The per-subagent tool-call cap each step's budget is minted with.
    max_subagent_tool_calls: usize,
    /// The maximum number of replans the planner may perform.
    max_replans: u32,
}

impl BudgetTree {
    /// Build a budget tree from the three explicit bounds.
    pub fn new(max_tool_calls: usize, max_subagent_tool_calls: usize, max_replans: u32) -> Self {
        Self {
            global: ToolBudget::new(max_tool_calls),
            max_subagent_tool_calls,
            max_replans,
        }
    }

    /// The global per-turn tool-call ceiling.
    pub fn global_limit(&self) -> usize {
        self.global.limit()
    }

    /// Global tool calls charged so far this turn.
    pub fn global_used(&self) -> usize {
        self.global.used()
    }

    /// Global tool calls still available before the ceiling.
    pub fn global_remaining(&self) -> usize {
        self.global.remaining()
    }

    /// The per-subagent tool-call cap.
    pub fn max_subagent_tool_calls(&self) -> usize {
        self.max_subagent_tool_calls
    }

    /// The max-replans bound the orchestrator loop enforces.
    pub fn max_replans(&self) -> u32 {
        self.max_replans
    }

    /// Mint a fresh per-subagent [`ToolBudget`] for a step.
    ///
    /// Each step gets its own per-subagent budget (capped at
    /// [`max_subagent_tool_calls`](Self::max_subagent_tool_calls)); they all draw
    /// on the one shared global ceiling via [`charge_tool_call`](Self::charge_tool_call).
    pub fn new_subagent_budget(&self) -> ToolBudget {
        ToolBudget::new(self.max_subagent_tool_calls)
    }

    /// Charge **only** the global ceiling.
    ///
    /// Exposed so a future subagent loop (S-174) can charge the global ceiling
    /// itself before dispatching through agent-core's
    /// [`BoundedDispatcher`](agent_core::BoundedDispatcher) — which charges the
    /// per-subagent budget separately. Returns the global bound on exhaustion,
    /// never invoking a tool ([NFR-CC-04]).
    pub fn charge_global(&self) -> Result<(), BudgetBound> {
        self.global
            .charge()
            .map(|_| ())
            .map_err(|e| BudgetBound::GlobalToolCalls { limit: e.limit })
    }

    /// Charge one tool call against **both** the global ceiling and a step's
    /// per-subagent budget, reporting the **first** bound reached.
    ///
    /// The global ceiling is the outer bound, so it is reported first: a turn that
    /// has spent its global allowance halts as [`BudgetBound::GlobalToolCalls`]
    /// even if the current subagent still had per-cap room. On a spent budget the
    /// caller must **not** invoke the tool — the honest halt ([NFR-CC-04]).
    ///
    /// **Charge order:** the global ceiling is *checked* first (so it reports
    /// first), but the per-subagent budget is *charged* first. This ordering
    /// matters because the per-subagent budget is step-local and discarded after
    /// the step, whereas the global ceiling is the shared, turn-long counter. By
    /// charging the discarded budget first, a refusal can never leave the shared
    /// global counter over-incremented: if the subagent charge fails the global is
    /// untouched, and if the global charge fails only the step-local subagent slot
    /// (thrown away) was spent. agent-core's `ToolBudget::charge` is atomic, so
    /// this also stays correct if S-174 ever shares one budget across concurrent
    /// subagents.
    pub fn charge_tool_call(&self, subagent: &ToolBudget) -> Result<(), BudgetBound> {
        // Report the outer (global) bound first if it is already spent — without
        // touching either counter.
        if self.global.is_exhausted() {
            return Err(BudgetBound::GlobalToolCalls {
                limit: self.global.limit(),
            });
        }
        // Charge the step-local per-subagent budget first: if it is exhausted the
        // charge fails atomically and the shared global counter is left untouched.
        subagent
            .charge()
            .map_err(|e| BudgetBound::SubagentToolCalls { limit: e.limit })?;
        // The subagent slot is reserved; now charge the shared global ceiling. A
        // failure here (only possible under concurrent global drain) spends a
        // step-local slot that is discarded with the step — never the global.
        self.global
            .charge()
            .map_err(|e| BudgetBound::GlobalToolCalls { limit: e.limit })?;
        Ok(())
    }
}

/// Build a [`BudgetTree`] from the parsed `[chat]` budget-tree params ([FR-CF-06],
/// [ADR-41]). The `u32` config fields widen to the `usize` the `ToolBudget`
/// primitive counts in.
impl From<&ChatConfig> for BudgetTree {
    fn from(cfg: &ChatConfig) -> Self {
        BudgetTree::new(
            cfg.max_tool_calls as usize,
            cfg.max_subagent_tool_calls as usize,
            cfg.max_replans,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_chat_config_maps_the_documented_defaults() {
        let tree = BudgetTree::from(&ChatConfig::default());
        assert_eq!(tree.global_limit(), 48);
        assert_eq!(tree.max_subagent_tool_calls(), 16);
        assert_eq!(tree.max_replans(), 3);
    }

    #[test]
    fn charge_tool_call_reports_subagent_bound_when_its_cap_binds() {
        // Global high, per-subagent low: a single step exhausts its own cap.
        let tree = BudgetTree::new(24, 1, 3);
        let sub = tree.new_subagent_budget();
        assert!(tree.charge_tool_call(&sub).is_ok());
        assert_eq!(
            tree.charge_tool_call(&sub),
            Err(BudgetBound::SubagentToolCalls { limit: 1 }),
        );
        // The honest halt charged nothing extra on the global ceiling.
        assert_eq!(tree.global_used(), 1);
    }

    #[test]
    fn charge_tool_call_reports_global_bound_first_across_steps() {
        // Global low, per-subagent high: the global ceiling binds first.
        let tree = BudgetTree::new(1, 8, 3);
        let sub_a = tree.new_subagent_budget();
        assert!(tree.charge_tool_call(&sub_a).is_ok());
        // A fresh step still cannot proceed — the global ceiling is spent.
        let sub_b = tree.new_subagent_budget();
        assert_eq!(
            tree.charge_tool_call(&sub_b),
            Err(BudgetBound::GlobalToolCalls { limit: 1 }),
        );
    }

    #[test]
    fn charge_global_reports_the_global_bound() {
        let tree = BudgetTree::new(1, 8, 3);
        assert!(tree.charge_global().is_ok());
        assert_eq!(
            tree.charge_global(),
            Err(BudgetBound::GlobalToolCalls { limit: 1 }),
        );
    }

    #[test]
    fn budget_bound_serializes_with_a_stable_tag() {
        let json = serde_json::to_value(BudgetBound::Replans { limit: 3 }).unwrap();
        assert_eq!(json, serde_json::json!({ "bound": "replans", "limit": 3 }));
    }
}
