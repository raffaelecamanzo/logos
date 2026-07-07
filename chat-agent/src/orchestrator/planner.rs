//! The LLM-driven planner — a `rig` `Agent` that decomposes a request into a
//! step plan and replans from observations ([S-173], [ADR-41]).
//!
//! The planner is a tool-less [`rig` `Agent`](agent_core::rig::agent::Agent)
//! built over any [`CompletionModel`] (the mock in offline tests, a real provider
//! in production). Each round the orchestrator calls [`Planner::decide`] with the
//! request and the per-turn scratchpad; the planner prompts the model and parses
//! its JSON reply into a [`PlannerDecision`] — a (re)plan or the final answer.
//!
//! A parse failure is surfaced honestly ([`OrchestratorError::PlanParse`]) rather
//! than guessed at — the orchestrator never fabricates a plan ([NFR-CC-04]).
//!
//! [S-173]: ../../../docs/planning/journal.md#s-173-planner-and-plan-act-observe-replan-orchestration-loop-with-budget-tree
//! [ADR-41]: ../../../docs/specs/architecture/decisions/ADR-41.md
//! [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md

use std::fmt::Write as _;

use agent_core::rig::agent::AgentBuilder;
use agent_core::rig::completion::{CompletionModel, Prompt};

use super::plan::{PlanStep, PlannerDecision};
use super::step::StepObservation;
use super::OrchestratorError;

/// The default planner system prompt: the roster, the JSON contract, and the
/// honest-grounding discipline. Overridable via [`Planner::with_preamble`].
pub const DEFAULT_PLANNER_PREAMBLE: &str = "\
You are the planner for Logos, a structural code-intelligence tool. You answer a \
user's question about THIS codebase by decomposing it into a short plan of steps, \
each handled by one specialized subagent, then replanning from their observations \
until you can give a final, grounded answer.\n\n\
The subagent roles are:\n\
- graph_navigator: navigates the code graph (search, context, node, callers, \
callees, impact, explore).\n\
- governance_analyst: runs governance/quality read-models (scan, check_rules, \
hotspots, test_gaps, dsm, gate, evolution, health).\n\
- source_reader: reads source files within the project (read, grep, glob).\n\
- synthesizer: composes the final grounded answer from the gathered observations \
(no tools).\n\n\
Reply with EXACTLY ONE JSON object and nothing else. Either lay out the next steps:\n\
{\"action\":\"plan\",\"steps\":[{\"role\":\"graph_navigator\",\"instruction\":\"…\"}]}\n\
or, once you have enough grounded observations, give the final answer:\n\
{\"action\":\"final\",\"answer\":\"…\"}\n\n\
Ground every claim in the subagents' observations. Never invent a tool result.";

/// The plan→act→observe→replan planner over a `rig` `Agent` ([ADR-41]).
///
/// Holds the completion model (cloned to build a fresh `Agent` per round; the
/// mock shares its scripted state across clones, so successive rounds consume
/// successive scripted turns) and the system preamble.
#[derive(Clone)]
pub struct Planner<M> {
    model: M,
    preamble: String,
}

impl<M> Planner<M>
where
    M: CompletionModel + Clone + 'static,
{
    /// A planner over `model` with the [`DEFAULT_PLANNER_PREAMBLE`].
    pub fn new(model: M) -> Self {
        Self {
            model,
            preamble: DEFAULT_PLANNER_PREAMBLE.to_string(),
        }
    }

    /// A planner over `model` with a custom system preamble.
    pub fn with_preamble(model: M, preamble: impl Into<String>) -> Self {
        Self {
            model,
            preamble: preamble.into(),
        }
    }

    /// Decide the next move: prompt the model with the request + scratchpad and
    /// parse its reply into a [`PlannerDecision`].
    ///
    /// Surfaces a provider failure as [`OrchestratorError::Planner`] and an
    /// unparseable reply as [`OrchestratorError::PlanParse`] — both honest, never
    /// a fabricated plan ([NFR-CC-04]).
    pub async fn decide(
        &self,
        request: &str,
        scratchpad: &[(PlanStep, StepObservation)],
    ) -> Result<PlannerDecision, OrchestratorError> {
        let prompt = render_prompt(request, scratchpad);
        // Build a fresh tool-less Agent per round; the same pattern S-166's
        // zero-egress test uses. The model is cloned (cheap; shared state for the
        // mock) so the planner can be consulted across replans.
        let agent = AgentBuilder::new(self.model.clone())
            .preamble(&self.preamble)
            .build();
        let raw = agent.prompt(prompt.as_str()).await.map_err(|e| {
            // Classify and carry the FULL source chain (transport vs HTTP-status vs
            // auth, with status/body where present) — never flatten with
            // `e.to_string()`, which would drop the legible root cause ([S-199],
            // [FR-UI-24]).
            OrchestratorError::Planner(agent_core::classify_provider_error(&e))
        })?;
        parse_decision(&raw)
            .map_err(|e| OrchestratorError::PlanParse(format!("{e}; planner said: {raw}")))
    }
}

/// Render the planner's user prompt from the request and the scratchpad so far.
///
/// Real providers reason over this; the mock ignores it and returns its scripted
/// turn, so the orchestrated loop is exercised deterministically offline.
fn render_prompt(request: &str, scratchpad: &[(PlanStep, StepObservation)]) -> String {
    let mut prompt = format!("User question:\n{request}\n");
    if scratchpad.is_empty() {
        prompt.push_str("\nNo observations yet — produce the initial plan.");
    } else {
        prompt.push_str("\nObservations so far:\n");
        for (i, (step, obs)) in scratchpad.iter().enumerate() {
            // `writeln!` to a String is infallible; the import is `std::fmt::Write`.
            let _ = writeln!(
                prompt,
                "{}. [{:?}] {} -> {}",
                i + 1,
                step.role,
                step.instruction,
                obs.summary
            );
        }
        prompt.push_str(
            "\nEither plan the next steps or, if these observations are enough, \
             give the final answer.",
        );
    }
    prompt
}

/// Parse the planner's reply into a [`PlannerDecision`].
///
/// Models sometimes wrap the JSON in prose or a code fence; we parse the trimmed
/// reply first, then fall back to the outermost `{…}` slice. A genuinely
/// unparseable reply returns the serde error for an honest [`OrchestratorError::PlanParse`].
fn parse_decision(raw: &str) -> Result<PlannerDecision, serde_json::Error> {
    let trimmed = raw.trim();
    match serde_json::from_str::<PlannerDecision>(trimmed) {
        Ok(decision) => Ok(decision),
        Err(first_err) => match outermost_json_object(trimmed) {
            Some(slice) => serde_json::from_str::<PlannerDecision>(slice),
            None => Err(first_err),
        },
    }
}

/// The substring from the first `{` to the last `}`, if both are present — the
/// outermost JSON object embedded in a reply.
fn outermost_json_object(s: &str) -> Option<&str> {
    let start = s.find('{')?;
    let end = s.rfind('}')?;
    (start < end).then(|| &s[start..=end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestrator::plan::StepRole;

    #[test]
    fn parses_a_bare_json_decision() {
        let d = parse_decision(r#"{"action":"final","answer":"ok"}"#).unwrap();
        assert_eq!(
            d,
            PlannerDecision::Final {
                answer: "ok".to_string()
            }
        );
    }

    #[test]
    fn parses_json_wrapped_in_a_code_fence() {
        let d = parse_decision("```json\n{\"action\":\"final\",\"answer\":\"ok\"}\n```").unwrap();
        assert_eq!(
            d,
            PlannerDecision::Final {
                answer: "ok".to_string()
            }
        );
    }

    #[test]
    fn an_unparseable_reply_is_an_error_not_a_guess() {
        assert!(parse_decision("I cannot answer that.").is_err());
    }

    #[test]
    fn renders_observations_into_the_replan_prompt() {
        let scratchpad = vec![(
            PlanStep::new(StepRole::GraphNavigator, "find Engine"),
            StepObservation::new("Engine has 3 callers"),
        )];
        let prompt = render_prompt("who calls Engine?", &scratchpad);
        assert!(prompt.contains("who calls Engine?"));
        assert!(prompt.contains("Engine has 3 callers"));
    }
}
