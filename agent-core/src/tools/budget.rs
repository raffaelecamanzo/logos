//! The bounded-dispatch primitive ([agent-core] "Budget primitives", ADR-41).
//!
//! A [`ToolBudget`] is a single per-run / per-agent **tool-call cap** — the
//! atomic counter the consumers compose into a richer structure: the chat
//! planner stacks a global budget over per-subagent budgets into the budget
//! *tree* (S-173), and the wiki queue-loop uses a single per-run bound
//! (ADR-42). This crate owns only the primitive, never the policy
//! (NFR-MA-02).
//!
//! [`BoundedDispatcher`] is the seam those loops dispatch through: it pairs a
//! `rig` [`ToolSet`] (one domain's tools, S-174) with a [`ToolBudget`] and
//! enforces two invariants on every step —
//!
//! 1. a tool **outside the registered set** is refused ([`DispatchError::ToolNotFound`]),
//!    so a least-privilege subagent cannot reach another domain's tools
//!    (S-174 acceptance);
//! 2. once the cap is reached the dispatch **halts honestly**
//!    ([`DispatchError::BudgetExhausted`]) — it never invokes the tool and
//!    never fabricates a result ([NFR-CC-04]).
//!
//! [agent-core]: ../../../docs/specs/architecture/components/agent-core.md
//! [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md

use std::sync::atomic::{AtomicUsize, Ordering};

use rig_core::tool::{ToolSet, ToolSetError};

/// A per-run / per-agent tool-call cap (ADR-41 budget primitive).
///
/// Cloneable counter semantics are deliberately **not** provided — share one
/// budget across a loop (or a budget tree) behind an `Arc`. `charge` is atomic,
/// so concurrent subagents drawing on the same budget can never overspend it.
#[derive(Debug)]
pub struct ToolBudget {
    /// The maximum number of tool calls this budget admits.
    max_calls: usize,
    /// Calls charged so far (monotonic, never decremented).
    used: AtomicUsize,
}

/// The honest halt state when a [`ToolBudget`] is spent ([NFR-CC-04]): names
/// the bound that was hit so the caller can report *which* limit halted the run
/// rather than looping unbounded or fabricating a result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("tool-call budget exhausted: {used}/{limit} calls used")]
pub struct BudgetExhausted {
    /// The cap that was reached.
    pub limit: usize,
    /// Calls charged at the point of refusal (== `limit`).
    pub used: usize,
}

impl ToolBudget {
    /// A budget admitting `max_calls` tool calls.
    pub fn new(max_calls: usize) -> Self {
        Self {
            max_calls,
            used: AtomicUsize::new(0),
        }
    }

    /// Reserve one call slot, returning the 1-based index of the charged call.
    ///
    /// Atomically refuses (without consuming a slot) once the cap is reached —
    /// the honest budget halt ([NFR-CC-04]).
    pub fn charge(&self) -> Result<usize, BudgetExhausted> {
        // `fetch_update` makes the check-and-increment a single atomic step, so
        // two subagents sharing this budget can never both pass the final slot.
        match self
            .used
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |used| {
                (used < self.max_calls).then_some(used + 1)
            }) {
            Ok(prev) => Ok(prev + 1),
            Err(used) => Err(BudgetExhausted {
                limit: self.max_calls,
                used,
            }),
        }
    }

    /// The configured cap.
    pub fn limit(&self) -> usize {
        self.max_calls
    }

    /// Calls charged so far.
    pub fn used(&self) -> usize {
        self.used.load(Ordering::SeqCst)
    }

    /// Calls still available before the cap.
    pub fn remaining(&self) -> usize {
        self.max_calls.saturating_sub(self.used())
    }

    /// Whether the next [`charge`](Self::charge) would be refused.
    pub fn is_exhausted(&self) -> bool {
        self.remaining() == 0
    }
}

/// Why a bounded dispatch did not produce a tool result.
#[derive(Debug, thiserror::Error)]
pub enum DispatchError {
    /// The requested tool is not in this dispatcher's set — a domain-partition
    /// violation (a subagent reaching outside its least-privilege subset,
    /// S-174), refused without charging the budget.
    #[error("tool {0:?} is not registered in this domain's tool set")]
    ToolNotFound(String),

    /// The tool-call budget was spent before this call — the honest halt
    /// ([NFR-CC-04]); the tool was not invoked.
    #[error(transparent)]
    BudgetExhausted(#[from] BudgetExhausted),

    /// The tool was dispatched but failed inside its own execution.
    #[error(transparent)]
    Tool(#[from] ToolSetError),

    /// A source tool refused a path that **escapes the project root / sandbox** —
    /// an [NFR-SE-04] containment violation ([`SandboxError::is_containment_refusal`]).
    ///
    /// Unlike a benign [`Tool`](Self::Tool) fault (a missing file, bad arguments,
    /// a transient failure) this is **turn-fatal**: the caller surfaces it as an
    /// honest refusal that names the escape and fabricates no answer, never
    /// routing around it ([NFR-CC-04], CR-063). It is detected **structurally** at
    /// the dispatch seam — a downcast to the typed [`SandboxError`] cause, not a
    /// match on the message text — so every source tool that consults the sandbox
    /// inherits it. Carries the refusal's message (which names the escape) for the
    /// honest error the caller renders.
    ///
    /// [SandboxError]: super::SandboxError
    /// [SandboxError::is_containment_refusal]: super::SandboxError::is_containment_refusal
    #[error("sandbox containment refusal: {0}")]
    Containment(String),
}

impl DispatchError {
    /// Classify a `rig` tool-call failure at the dispatch seam.
    ///
    /// A source-tool **sandbox containment refusal** (a path escaping the project
    /// root) becomes the turn-fatal [`Containment`](Self::Containment); every
    /// other tool fault — a missing file, malformed arguments, a transient
    /// failure — stays the recoverable [`Tool`](Self::Tool). Detection is
    /// **structural**: walk the error's `source()` chain and downcast to the typed
    /// [`SandboxError`](super::SandboxError) cause (`rig` type-erases the tool's
    /// error into a `Box<dyn Error>`, but the concrete `SandboxError` survives as
    /// the boxed payload), never a match on the message text (CR-063,
    /// [NFR-SE-04], [NFR-CC-04]).
    fn classify_tool_error(err: ToolSetError) -> Self {
        // Compute the refusal (if any) in a scope that only *borrows* `err`, so it
        // can be moved into `Tool(err)` afterwards.
        let refusal = {
            let mut cause: Option<&(dyn std::error::Error + 'static)> = Some(&err);
            loop {
                match cause {
                    Some(source) => {
                        if let Some(sandbox) = source.downcast_ref::<super::SandboxError>() {
                            // Found the typed sandbox cause: a containment refusal
                            // is turn-fatal; any other sandbox fault is benign and
                            // stays recoverable.
                            break sandbox
                                .is_containment_refusal()
                                .then(|| sandbox.to_string());
                        }
                        cause = source.source();
                    }
                    None => break None,
                }
            }
        };
        match refusal {
            Some(refusal) => DispatchError::Containment(refusal),
            None => DispatchError::Tool(err),
        }
    }
}

/// A `rig` [`ToolSet`] gated by a [`ToolBudget`] — the bounded dispatch seam
/// the planner/roster loops drive (S-173, S-174).
///
/// Holds the budget by shared reference so the same budget can gate several
/// dispatchers (e.g. each subagent's per-role dispatcher charging the one
/// global budget of the budget tree).
pub struct BoundedDispatcher<'b> {
    tools: ToolSet,
    budget: &'b ToolBudget,
}

impl<'b> BoundedDispatcher<'b> {
    /// Gate `tools` by `budget`.
    pub fn new(tools: ToolSet, budget: &'b ToolBudget) -> Self {
        Self { tools, budget }
    }

    /// The set of tools this dispatcher can reach (read-only).
    pub fn tools(&self) -> &ToolSet {
        &self.tools
    }

    /// The gating budget.
    pub fn budget(&self) -> &ToolBudget {
        self.budget
    }

    /// Dispatch one tool call by name with JSON-encoded `args`, charging the
    /// budget.
    ///
    /// Order matters: an unknown tool is refused *before* the budget is
    /// charged (a misroute should not spend the run's allowance), and the
    /// budget is charged *before* the tool runs (so the cap bounds attempts,
    /// not just successes). On a spent budget the tool is never invoked
    /// ([NFR-CC-04]).
    pub async fn dispatch(
        &self,
        name: &str,
        args: impl Into<String>,
    ) -> Result<String, DispatchError> {
        if !self.tools.contains(name) {
            return Err(DispatchError::ToolNotFound(name.to_string()));
        }
        self.budget.charge()?;
        // A tool failure is classified at this seam: a sandbox containment refusal
        // is turn-fatal (`Containment`), every other tool fault stays recoverable
        // (`Tool`) — CR-063, see [`DispatchError::classify_tool_error`].
        self.tools
            .call(name, args.into())
            .await
            .map_err(DispatchError::classify_tool_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn charge_counts_up_to_the_cap_then_halts_honestly() {
        let budget = ToolBudget::new(2);
        assert_eq!(budget.remaining(), 2);
        assert_eq!(budget.charge().unwrap(), 1);
        assert_eq!(budget.charge().unwrap(), 2);
        assert!(budget.is_exhausted());

        // The honest halt names the bound that was hit (NFR-CC-04).
        let err = budget.charge().unwrap_err();
        assert_eq!(err.limit, 2);
        assert_eq!(err.used, 2);
        // A refused charge consumes nothing.
        assert_eq!(budget.used(), 2);
    }

    #[test]
    fn zero_budget_refuses_immediately() {
        let budget = ToolBudget::new(0);
        assert!(budget.is_exhausted());
        assert!(budget.charge().is_err());
    }
}
