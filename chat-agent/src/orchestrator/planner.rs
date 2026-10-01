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
use agent_core::{ToolDomain, WORKSPACE_TOOL_NAMES, XSERVICE_TOOL_NAMES};
use logos_core::federation::Federation;

use super::history::ConversationWindow;
use super::plan::{PlanStep, PlannerDecision};
use super::step::StepObservation;
use super::OrchestratorError;

/// The default planner system prompt: the roster, the JSON contract, and the
/// honest-grounding discipline. Overridable via [`Planner::with_preamble`].
pub const DEFAULT_PLANNER_PREAMBLE: &str = "\
You are the planner for Logos, a structural code-intelligence tool. You answer a \
user's question about THIS codebase by decomposing it into a short plan of steps, \
each handled by one specialized subagent, then replanning from their observations \
until the turn can be finalized.\n\n\
The subagent roles are:\n\
- graph_navigator: navigates the code graph (search, context, node, callers, \
callees, impact, explore).\n\
- governance_analyst: runs governance/quality read-models (scan, check_rules, \
hotspots, dsm, gate, evolution, health).\n\
- source_reader: reads source files within the project (read, grep, glob).\n\
- synthesizer: composes the final grounded answer from the gathered observations \
(no tools) — this is done for you when you finalize; you do NOT write the answer.\n\n\
Reply with EXACTLY ONE JSON object and nothing else. Either lay out the next steps:\n\
{\"action\":\"plan\",\"steps\":[{\"role\":\"graph_navigator\",\"instruction\":\"…\"}]}\n\
or finalize the turn — the Synthesizer then composes the user-facing answer from \
the observations, so your final decision carries NO answer text, only a `grounded` \
marker:\n\
{\"action\":\"final\",\"grounded\":true}\n\n\
Set \"grounded\": true when the answer makes any claim about THIS codebase — you \
MUST have gathered at least one observation first; a codebase answer is never given \
from prior knowledge alone. Set \"grounded\": false ONLY for a purely conversational \
turn that makes no codebase claim (a greeting, or a meta-question about this chat) — \
that answers directly with no tool steps. Ground every claim in the subagents' \
observations. Never invent a tool result.";

/// The planner preamble of the **workspace** roster ([S-481], [FR-WS-34],
/// [ADR-71]): told it answers for the workspace `federation` names — the workspace
/// first, one member when the question names one — and given every member with
/// its declared kind ([`member_roster`]).
///
/// The role lines name each role's tools from the same name lists the workspace
/// roster registers them from ([`ToolDomain::tool_names`],
/// [`WORKSPACE_TOOL_NAMES`], [`XSERVICE_TOOL_NAMES`]), so the planner is never told
/// of a tool the roster does not carry. The JSON contract is
/// [`DEFAULT_PLANNER_PREAMBLE`]'s, worded for a workspace; the member planner's
/// preamble is untouched.
///
/// [S-481]: ../../../docs/planning/journal.md#s-481-a-workspace-roster-centred-on-the-workspace-and-the-member-roster-single-backing-only
/// [FR-WS-34]: ../../../docs/specs/requirements/FR-WS-34.md
/// [ADR-71]: ../../../docs/specs/architecture/decisions/ADR-71.md
pub fn workspace_planner_preamble(federation: &Federation) -> String {
    let workspace = WORKSPACE_TOOL_NAMES.join(", ");
    let xservice = XSERVICE_TOOL_NAMES.join(", ");
    let graph = ToolDomain::Graph.tool_names().join(", ");
    let governance = ToolDomain::Governance.tool_names().join(", ");
    let source = ToolDomain::Source.tool_names().join(", ");
    format!(
        "\
You are the planner for Logos, a structural code-intelligence tool. You answer a \
user's question about the multi-repository WORKSPACE \"{name}\" — for the workspace as \
a whole first, narrowing to one member when the question names one — by decomposing \
it into a short plan of steps, each handled by one specialized subagent, then \
replanning from their observations until the turn can be finalized.\n\n\
{roster}\n\n\
The subagent roles are:\n\
- workspace_analyst: reads the workspace as a whole ({workspace}) and across services \
({xservice}), every result repo-qualified. Route here any question about which members \
exist, how they connect, or what crosses repositories.\n\
- graph_navigator: navigates ONE member's code graph ({graph}).\n\
- governance_analyst: runs ONE member's governance/quality read-models ({governance}).\n\
- source_reader: reads source files within ONE member ({source}).\n\
- synthesizer: composes the final grounded answer from the gathered observations \
(no tools) — this is done for you when you finalize; you do NOT write the answer.\n\n\
graph_navigator, governance_analyst and source_reader each address one member: name \
that member in the step's instruction exactly as it is listed above. To look into \
several members, give each its own step. When the prompt opens with earlier turns of \
this conversation, read a follow-up against them: \"and for <member>?\" asks the earlier \
question again, of that member. An observation line marked UNRESOLVED or NOT CHECKED \
means a cross-service answer is incomplete: it must reach the answer as such, never as \
\"none\".\n\n\
Reply with EXACTLY ONE JSON object and nothing else. Either lay out the next steps:\n\
{{\"action\":\"plan\",\"steps\":[{{\"role\":\"workspace_analyst\",\"instruction\":\"…\"}}]}}\n\
or finalize the turn — the Synthesizer then composes the user-facing answer from \
the observations, so your final decision carries NO answer text, only a `grounded` \
marker:\n\
{{\"action\":\"final\",\"grounded\":true}}\n\n\
Set \"grounded\": true when the answer makes any claim about this workspace or any of \
its members — you MUST have gathered at least one observation first; such an answer \
is never given from prior knowledge alone. Set \"grounded\": false ONLY for a purely \
conversational turn that makes no workspace claim (a greeting, or a meta-question \
about this chat) — that answers directly with no tool steps. Ground every claim in \
the subagents' observations. Never invent a tool result.",
        name = federation.name,
        roster = member_roster(federation),
    )
}

/// The workspace's members, one line each with its declared kind
/// ([FR-WS-32]) — `no declared kind` where the manifest declares none — and the
/// default member marked; the block the workspace planner and Synthesizer
/// preambles carry ([FR-WS-34]).
///
/// The names are [`Member::name`](logos_core::federation::Member::name), the
/// value a repo-addressed tool's `repo` takes. Reading them touches the manifest's
/// resolution only: no member engine is started ([NFR-PE-10]).
///
/// [FR-WS-32]: ../../../docs/specs/requirements/FR-WS-32.md
/// [FR-WS-34]: ../../../docs/specs/requirements/FR-WS-34.md
/// [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md
pub fn member_roster(federation: &Federation) -> String {
    let mut roster = format!(
        "The workspace has {} member(s) — each name, exactly as written, is the `repo` a \
         member tool takes (name: declared kind):",
        federation.members.len()
    );
    for member in &federation.members {
        let kind = federation
            .member_kinds
            .get(&member.name)
            .map_or("no declared kind", |kind| kind.as_str());
        let _ = write!(roster, "\n- {}: {kind}", member.name);
        if federation.default.as_deref() == Some(member.name.as_str()) {
            roster.push_str(" (the default member)");
        }
    }
    roster
}

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

    /// Decide the next move: prompt the model with the thread's prior-turn
    /// `history`, the request and the scratchpad, and parse its reply into a
    /// [`PlannerDecision`].
    ///
    /// Surfaces a provider failure as [`OrchestratorError::Planner`] and an
    /// unparseable reply as [`OrchestratorError::PlanParse`] — both honest, never
    /// a fabricated plan ([NFR-CC-04]).
    pub async fn decide(
        &self,
        request: &str,
        scratchpad: &[(PlanStep, StepObservation)],
        correction: Option<&str>,
        history: &ConversationWindow,
    ) -> Result<PlannerDecision, OrchestratorError> {
        let prompt = render_prompt(request, scratchpad, correction, history);
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
/// `history`, when non-empty, leads the prompt as the thread's prior turns (oldest
/// first, [S-483]); an empty window adds nothing, so a thread's first turn renders
/// exactly as it did before windows existed.
///
/// [S-483]: ../../../docs/planning/journal.md#s-483-follow-up-turns-see-prior-turns
///
/// Real providers reason over this; the mock ignores it and returns its scripted
/// turn, so the orchestrated loop is exercised deterministically offline.
///
/// `correction`, when present, is a corrective directive appended last — the
/// orchestrator sets it to re-prompt a planner that finalized a codebase answer
/// prematurely (a `grounded` final over an empty scratchpad), telling it to gather
/// grounding first ([FR-UI-30], [NFR-CC-04]).
fn render_prompt(
    request: &str,
    scratchpad: &[(PlanStep, StepObservation)],
    correction: Option<&str>,
    history: &ConversationWindow,
) -> String {
    let mut prompt = if history.is_empty() {
        String::new()
    } else {
        format!("{}\n\n", history.render())
    };
    let _ = writeln!(prompt, "User question:\n{request}");
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
             finalize the turn.",
        );
    }
    if let Some(correction) = correction {
        let _ = write!(prompt, "\n\n{correction}");
    }
    prompt
}

/// Parse the planner's reply into a [`PlannerDecision`].
///
/// Three passes, most-specific first ([CR-086]):
/// 1. the trimmed reply verbatim;
/// 2. the outermost `{…}` slice — models sometimes wrap the JSON in prose or a
///    code fence;
/// 3. a **control-char repair fallback** — escape raw control characters occurring
///    **inside** JSON string values and reparse once. Now that no answer prose
///    rides the JSON, the only remaining free-form field is a `plan` step's
///    `instruction`; a model that emits a raw newline there would otherwise trip
///    the same RFC 8259 "control character in string" failure the reroute fixed for
///    the answer. This is defense-in-depth for that residual field, not the primary
///    fix.
///
/// A genuinely unparseable reply returns the serde error for an honest
/// [`OrchestratorError::PlanParse`].
///
/// [CR-086]: ../../../docs/requests/CR-086-chat-planner-answer-synthesizer-reroute.md
fn parse_decision(raw: &str) -> Result<PlannerDecision, serde_json::Error> {
    let trimmed = raw.trim();
    if let Ok(decision) = serde_json::from_str::<PlannerDecision>(trimmed) {
        return Ok(decision);
    }
    // The outermost object slice, or the trimmed reply itself when it has no braces
    // (a pure-prose reply — repair leaves it unparseable, an honest error).
    let candidate = outermost_json_object(trimmed).unwrap_or(trimmed);
    match serde_json::from_str::<PlannerDecision>(candidate) {
        Ok(decision) => Ok(decision),
        Err(err) => {
            // Last resort: escape raw control chars inside string values, reparse
            // once. On failure surface the pre-repair error — it names the original
            // fault, not an artifact of the repaired text.
            let repaired = escape_control_chars_in_strings(candidate);
            serde_json::from_str::<PlannerDecision>(&repaired).map_err(|_| err)
        }
    }
}

/// The substring from the first `{` to the last `}`, if both are present — the
/// outermost JSON object embedded in a reply.
fn outermost_json_object(s: &str) -> Option<&str> {
    let start = s.find('{')?;
    let end = s.rfind('}')?;
    (start < end).then(|| &s[start..=end])
}

/// Escape raw ASCII control characters (U+0000–U+001F) that appear **inside** JSON
/// string literals, leaving structural control whitespace between tokens untouched
/// ([CR-086] repair fallback). A single left-to-right pass tracks whether the
/// cursor is inside a `"…"` string (honoring `\`-escapes), so a literal newline in
/// a value becomes `\n`, a tab `\t`, a carriage return `\r`, and any other control
/// char its `\u00XX` escape — the exact transformation that makes an otherwise
/// RFC 8259-illegal reply parseable, without altering already-valid JSON.
fn escape_control_chars_in_strings(raw: &str) -> String {
    use std::fmt::Write as _;

    let mut out = String::with_capacity(raw.len());
    let mut in_string = false;
    // Inside a string, the previous char was an unescaped `\` — so this char is the
    // escaped member of a `\x` pair and is copied verbatim (never treated as a
    // string terminator or a control char to re-escape).
    let mut escaped = false;

    for c in raw.chars() {
        if !in_string {
            if c == '"' {
                in_string = true;
            }
            out.push(c);
            continue;
        }
        if escaped {
            out.push(c);
            escaped = false;
            continue;
        }
        match c {
            '\\' => {
                out.push(c);
                escaped = true;
            }
            '"' => {
                out.push(c);
                in_string = false;
            }
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestrator::plan::StepRole;
    use logos_core::federation::{Member, MemberKind};

    /// A federation of `members` (name, declared kind) under `root`; building it
    /// opens nothing.
    fn federation(members: &[(&str, Option<MemberKind>)], default: Option<&str>) -> Federation {
        let root = std::path::PathBuf::from("/ws");
        Federation {
            name: "shop".to_string(),
            root: root.clone(),
            members: members
                .iter()
                .map(|(name, _)| Member { name: name.to_string(), root: root.join(name) })
                .collect(),
            default: default.map(str::to_string),
            links: Vec::new(),
            governance: Default::default(),
            warm_concurrency: None,
            member_kinds: members
                .iter()
                .filter_map(|(name, kind)| kind.map(|kind| (name.to_string(), kind)))
                .collect(),
        }
    }

    /// [S-481]: the workspace planner preamble names each role's tools from the
    /// lists the workspace roster registers them from — so it can never offer a
    /// tool the roster lacks — while the member planner's preamble names neither
    /// the Workspace-Analyst nor any workspace or cross-service tool.
    ///
    /// [S-481]: ../../../docs/planning/journal.md#s-481-a-workspace-roster-centred-on-the-workspace-and-the-member-roster-single-backing-only
    #[test]
    fn the_workspace_planner_names_every_registered_tool_and_the_member_planner_none() {
        let preamble = workspace_planner_preamble(&federation(&[("api", None)], None));
        let tools = WORKSPACE_TOOL_NAMES
            .iter()
            .chain(XSERVICE_TOOL_NAMES)
            .chain(ToolDomain::ALL.iter().flat_map(|d| d.tool_names()));
        for tool in tools {
            assert!(preamble.contains(tool), "{tool}: {preamble}");
        }
        for absent in ["workspace_analyst", "xservice", "workspace_"] {
            assert!(!DEFAULT_PLANNER_PREAMBLE.contains(absent), "{absent}");
        }
    }

    /// Both planner preambles teach the same two JSON decisions, and each
    /// example parses as the decision it names — the contract cannot drift
    /// between the two rosters' planners.
    #[test]
    fn both_planner_preambles_teach_decisions_the_parser_reads() {
        let workspace = workspace_planner_preamble(&federation(&[("api", None)], None));
        for preamble in [DEFAULT_PLANNER_PREAMBLE, workspace.as_str()] {
            let examples: Vec<&str> = preamble
                .lines()
                .filter(|line| line.starts_with("{\"action\""))
                .collect();
            assert_eq!(examples.len(), 2, "{preamble}");
            assert!(matches!(
                parse_decision(&examples[0].replace('…', "x")),
                Ok(PlannerDecision::Plan { .. })
            ));
            assert_eq!(
                parse_decision(examples[1]).unwrap(),
                PlannerDecision::Final { grounded: true }
            );
        }
    }

    /// The member roster block: one line per member in manifest order, its kind
    /// or `no declared kind`, the default marked.
    #[test]
    fn the_member_roster_lists_each_member_with_its_kind_and_marks_the_default() {
        let roster = member_roster(&federation(
            &[("web", None), ("api", Some(MemberKind::Mock)), ("docs", Some(MemberKind::Documentation))],
            Some("web"),
        ));
        assert_eq!(
            roster,
            "The workspace has 3 member(s) — each name, exactly as written, is the `repo` a \
             member tool takes (name: declared kind):\n\
             - web: no declared kind (the default member)\n\
             - api: mock\n\
             - docs: documentation"
        );
    }

    #[test]
    fn parses_a_bare_json_decision() {
        let d = parse_decision(r#"{"action":"final","grounded":true}"#).unwrap();
        assert_eq!(d, PlannerDecision::Final { grounded: true });
    }

    #[test]
    fn parses_json_wrapped_in_a_code_fence() {
        let d = parse_decision("```json\n{\"action\":\"final\",\"grounded\":false}\n```").unwrap();
        assert_eq!(d, PlannerDecision::Final { grounded: false });
    }

    #[test]
    fn an_unparseable_reply_is_an_error_not_a_guess() {
        assert!(parse_decision("I cannot answer that.").is_err());
    }

    #[test]
    fn repairs_raw_control_chars_inside_a_string_field() {
        // The residual free-form field — a plan step's `instruction` — may carry
        // raw newlines / control chars a model failed to escape. Strict JSON rejects
        // them (RFC 8259); the repair fallback escapes them in place and reparses
        // ([CR-086] regression). A bare `serde_json::from_str` on this input fails.
        let raw = "{\"action\":\"plan\",\"steps\":[{\"role\":\"synthesizer\",\
                   \"instruction\":\"summarize:\n- line one\n- line two\ttabbed\"}]}";
        assert!(
            serde_json::from_str::<PlannerDecision>(raw).is_err(),
            "the raw reply is genuinely control-char-illegal without repair"
        );
        let d = parse_decision(raw).expect("the control-char repair fallback reparses");
        match d {
            PlannerDecision::Plan { steps } => {
                assert_eq!(steps.len(), 1);
                assert_eq!(steps[0].role, StepRole::Synthesizer);
                // The literal newlines/tab survive as real characters in the field.
                assert!(steps[0].instruction.contains("- line one\n- line two"));
                assert!(steps[0].instruction.contains("\ttabbed"));
            }
            other => panic!("expected a plan, got {other:?}"),
        }
    }

    #[test]
    fn repair_leaves_control_whitespace_between_tokens_alone() {
        // Newlines/tabs OUTSIDE string values are valid JSON whitespace — the repair
        // must not touch them, only re-escape control chars inside string literals.
        let raw = "{\n\t\"action\": \"final\",\n\t\"grounded\": true\n}";
        assert_eq!(
            parse_decision(raw).unwrap(),
            PlannerDecision::Final { grounded: true }
        );
    }

    #[test]
    fn escape_control_chars_preserves_existing_backslash_escapes() {
        // An already-valid `\n` escape (backslash + n) must pass through unchanged —
        // the repair only touches RAW control bytes, never re-escapes an escape.
        let already_valid = r#"{"action":"plan","steps":[{"role":"source_reader","instruction":"a\nb"}]}"#;
        assert_eq!(escape_control_chars_in_strings(already_valid), already_valid);
    }

    #[test]
    fn renders_observations_into_the_replan_prompt() {
        let scratchpad = vec![(
            PlanStep::new(StepRole::GraphNavigator, "find Engine"),
            StepObservation::new("Engine has 3 callers"),
        )];
        let prompt = render_prompt("who calls Engine?", &scratchpad, None, &ConversationWindow::default());
        assert!(prompt.contains("who calls Engine?"));
        assert!(prompt.contains("Engine has 3 callers"));
    }

    #[test]
    fn a_correction_is_appended_to_the_prompt() {
        // The forced-grounding re-prompt ([FR-UI-30]): the corrective directive is
        // appended so a real planner sees why its premature finalize was refused.
        let prompt = render_prompt(
            "what does Engine do?",
            &[],
            Some("GATHER GROUNDING FIRST"),
            &ConversationWindow::default(),
        );
        assert!(prompt.contains("what does Engine do?"));
        assert!(prompt.ends_with("GATHER GROUNDING FIRST"));
    }
}
