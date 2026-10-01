//! The fixed rosters of specialized subagents ([S-174], [ADR-41], [ADR-71],
//! [chat-agent]).
//!
//! S-173 built the plan→act→observe→replan loop and routed each [`PlanStep`] to a
//! [`StepExecutor`] behind a trait. This module holds the real executors — two
//! fixed rosters, each role a `rig`-`Agent`-shaped unit: a [`CompletionModel`] + a
//! system preamble + **one fixed** least-privilege tool subset from agent-core
//! (S-167) — a single domain for every role but the Workspace-Analyst, whose
//! subset is agent-core's workspace and `xservice_*` sets together ([ADR-71] §5).
//!
//! The **member roster** ([`SubagentRoster`]) answers for one codebase, under any
//! backing:
//!
//! - **Graph-Navigator** — the 8 graph tools (`search`/`context`/`node`/
//!   `callers`/`callees`/`impact`/`explore`/`affected`);
//! - **Governance-Analyst** — the 8 governance tools (`scan`/`check_rules`/
//!   `hotspots`/`dsm`/`gate`/`evolution`/`doc_gaps`/`health`);
//! - **Source-Reader** — the 3 sandboxed source tools (`read`/`grep`/`glob`),
//!   project-root-confined ([NFR-SE-04]);
//! - **Synthesizer** — **tool-less**; composes the final grounded answer from the
//!   turn's observations and makes **zero** tool calls.
//!
//! The **workspace roster** ([`WorkspaceRoster`], [S-481]) answers for a
//! workspace, narrowing to one member when a question names one:
//!
//! - **Workspace-Analyst** — the 5 workspace read-model tools
//!   (`workspace_status`/`workspace_reachability`/`workspace_check`/
//!   `xservice_build_deps`/`workspace_roster`) followed by the 4 read-only
//!   `xservice_*` tools (`xservice_route_providers`/`xservice_callers`/
//!   `xservice_impact`/`xservice_search`, [FR-WS-29]), under its own
//!   per-subagent cap: a step of its own draws its own [`ToolBudget`];
//! - **Graph-Navigator**, **Governance-Analyst** and **Source-Reader** — the
//!   same tools **repo-addressed** ([S-480]): each takes a required `repo` and
//!   resolves that member's engine, or sandbox, per call through the registry;
//! - **Synthesizer** — tool-less, as above.
//!
//! # Least privilege + the budget tree
//!
//! A tool-bearing subagent drives its own bounded tool loop: it prompts its model,
//! and for every tool call the model requests it dispatches through agent-core's
//! [`BoundedDispatcher`] over **this step's** per-subagent
//! [`ToolBudget`](agent_core::ToolBudget) ([`StepContext::subagent_budget`]). The
//! dispatcher enforces the domain partition — a tool **outside** the subagent's
//! subset is refused with [`DispatchError::ToolNotFound`] and **charges nothing**
//! (S-174 acceptance) — while the per-subagent cap and the shared global ceiling
//! ([`StepContext::budget_tree`]) bound the turn and halt it **honestly**, naming
//! the bound, never fabricating a result ([NFR-CC-04]).
//!
//! The loop is driven at the [`CompletionModel`] level rather than through `rig`'s
//! built-in multi-turn tool loop precisely so every tool call passes through the
//! budget tree; `rig`'s own loop would dispatch tools internally and bypass it.
//!
//! # Cross-service reach is the workspace roster's ([S-481], [ADR-71])
//!
//! The member roster has no federated branch: it takes no query backing, so its
//! registered tools, its preambles and every observation it produces are
//! byte-for-byte what a single root's were before [S-431] — whichever backing the
//! engine it is handed came from. Cross-service reach lives on the workspace
//! roster alone. There, every `xservice_*` and workspace read-model result's
//! deterministic `reading` line — repo-qualified, and carrying the unresolved
//! residue as `UNRESOLVED` when a cross-service answer is empty over a non-zero
//! residue ([BR-53]) — is appended **verbatim** to the step's observation
//! (`with_readings`), so the planner and the Synthesizer read the qualification
//! whether or not the model's own summary repeats it.
//!
//! [S-480]: ../../../docs/planning/journal.md#s-480-agent-tools-address-a-named-member-and-the-workspace-read-models-become-tools
//! [S-481]: ../../../docs/planning/journal.md#s-481-a-workspace-roster-centred-on-the-workspace-and-the-member-roster-single-backing-only
//! [ADR-71]: ../../../docs/specs/architecture/decisions/ADR-71.md
//! [S-174]: ../../../docs/planning/journal.md#s-174-specialized-subagent-roster-on-rig
//! [S-431]: ../../../docs/planning/journal.md#s-431-the-chat-agents-tool-surface-is-workspace-aware
//! [FR-WS-29]: ../../../docs/specs/requirements/FR-WS-29.md
//! [BR-53]: ../../../docs/specs/software-spec.md#327-workspace-federation
//! [ADR-41]: ../../../docs/specs/architecture/decisions/ADR-41.md
//! [chat-agent]: ../../../docs/specs/architecture/components/chat-agent.md
//! [NFR-SE-04]: ../../../docs/specs/requirements/NFR-SE-04.md
//! [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md

use std::sync::Arc;

use agent_core::rig::completion::{AssistantContent, CompletionModel};
use agent_core::rig::message::{Message, ToolCall};
use agent_core::{
    addressed_toolset, governance_toolset, graph_toolset, source_toolset, workspace_reading,
    workspace_toolset, xservice_reading, xservice_toolset, BoundedDispatcher, DispatchError,
    Sandbox, ToolBudget, ToolDomain, XserviceBacking,
};
use logos_core::federation::Federation;
use logos_core::Engine;

use super::plan::{PlanStep, StepRole};
use super::planner::member_roster;
use super::step::{StepContext, StepError, StepExecutor, StepObservation};

/// How many **consecutive** tool errors (a tool that ran and failed, or an
/// out-of-domain tool request) a tool-bearing subagent may accumulate before its
/// step soft-closes ([CR-060] Layer 2, [FR-UI-28]).
///
/// Feeding a tool error back as a self-correcting observation risks an unbounded
/// self-correction loop; this streak cap bounds it. The streak resets on **any**
/// successful dispatch, so it counts only a *run* of failures with no progress in
/// between — a subagent that recovers is never penalized for an earlier stumble.
/// A `ToolNotFound` misroute charges no budget, so for a pure `ToolNotFound` loop
/// this cap is the **only** thing that terminates the step (the budget bounds
/// never bite). Kept a small module constant rather than a config key: [CR-060]
/// scopes new `[chat]` keys to the S-240 retry policy and adds no tool-error knob.
///
/// [CR-060]: ../../../docs/requests/CR-060-chat-resilience-recoverable-faults.md
/// [FR-UI-28]: ../../../docs/specs/requirements/FR-UI-28.md
const MAX_CONSECUTIVE_TOOL_ERRORS: usize = 3;

/// System preamble for the **Graph-Navigator** subagent.
///
/// Steers it toward the breadth-efficient `context` tool over several
/// `search`/`node` calls ([S-182], [CR-048]) — a cheap complement to the
/// [S-181] soft cap that reduces how often a step is bounded at all by
/// economizing tool calls up front rather than gracefully closing out after
/// the fact. The running per-step budget status is appended dynamically by
/// [`budget_aware_preamble`] before every model round.
///
/// [S-182]: ../../../docs/planning/journal.md#s-182-budget-aware-subagent-preambles
/// [CR-048]: ../../../docs/requests/CR-048-soft-per-subagent-budget-cap.md
/// [S-181]: ../../../docs/planning/journal.md#s-181-soft-per-subagent-budget-cap-with-graceful-summarization-and-answer-on-halt
pub const GRAPH_NAVIGATOR_PREAMBLE: &str = "\
You are the Graph-Navigator subagent of Logos, a structural code-intelligence \
tool. You answer one step of a larger plan by navigating THIS codebase's code \
graph with your tools: search, context, node, callers, callees, impact, explore, \
affected. Call the tools you need to gather grounded facts, then reply with a \
concise plain-text summary of what you found. Ground every claim in a tool \
result — never invent a symbol, edge, or count. When a step is broad — it spans \
multiple symbols, or asks for a neighborhood/overview rather than one known \
symbol — prefer a single `context` call (one ranked multi-symbol bundle) over \
several separate `search`/`node` calls; reach for `search`/`node` once you already \
know the specific symbol you need.";

/// System preamble for the **Governance-Analyst** subagent.
///
/// The running per-step budget status is appended dynamically by
/// [`budget_aware_preamble`] before every model round ([S-182], [CR-048]).
///
/// [S-182]: ../../../docs/planning/journal.md#s-182-budget-aware-subagent-preambles
/// [CR-048]: ../../../docs/requests/CR-048-soft-per-subagent-budget-cap.md
pub const GOVERNANCE_ANALYST_PREAMBLE: &str = "\
You are the Governance-Analyst subagent of Logos, a structural code-intelligence \
tool. You answer one step of a larger plan by running THIS codebase's \
governance/quality read-models with your tools: scan, check_rules, hotspots, \
dsm, gate, evolution, doc_gaps, health. Call the tools you need, then \
reply with a concise plain-text summary of the grounded signal. Never fabricate a \
metric or a verdict.";

/// System preamble for the **Source-Reader** subagent.
///
/// The running per-step budget status is appended dynamically by
/// [`budget_aware_preamble`] before every model round ([S-182], [CR-048]).
///
/// [S-182]: ../../../docs/planning/journal.md#s-182-budget-aware-subagent-preambles
/// [CR-048]: ../../../docs/requests/CR-048-soft-per-subagent-budget-cap.md
pub const SOURCE_READER_PREAMBLE: &str = "\
You are the Source-Reader subagent of Logos, a structural code-intelligence tool. \
You answer one step of a larger plan by reading source files WITHIN this project \
with your sandboxed tools: read, grep, glob. They are confined to the project \
root. Call the tools you need, then reply with a concise plain-text summary \
grounded in the file contents you read. Never invent file contents.";

/// System preamble for the (tool-less) **Synthesizer** subagent.
///
/// The final sentence is HF-2: Mermaid's `sequenceDiagram` grammar treats a bare
/// `;` as a statement separator, so a model-written Note or message containing
/// one gets torn into an invalid second statement and the whole diagram is
/// rejected client-side. `web/ui/src/views/wiki/mermaid.ts` (`renderMermaidIn`)
/// also repairs it before render, but steering the model away from writing it
/// in the first place is cheaper than relying on that alone.
pub const SYNTHESIZER_PREAMBLE: &str = "\
You are the Synthesizer subagent of Logos, a structural code-intelligence tool. \
You have NO tools. Using only the observations the other subagents have already \
gathered (provided in your instruction), compose the final, grounded answer to \
the user's question in clear prose. Ground every claim in those observations; if \
they are insufficient, say so honestly rather than inventing facts. In a Mermaid \
sequence diagram, never put a bare ';' in message or note text — use '#59;' or \
rephrase, and use '<br/>' for line breaks.";

/// System preamble for the workspace roster's **Workspace-Analyst** ([S-481],
/// [FR-WS-34]): the workspace read-models, then the cross-service tools, whose
/// usage guidance it carries from the member Graph-Navigator's retired
/// cross-service addendum ([S-431]).
///
/// It names no member: the roster tool reads them, and the planner names the one
/// a step is about. The running per-step budget status is appended dynamically by
/// [`budget_aware_preamble`] before every model round.
///
/// [S-481]: ../../../docs/planning/journal.md#s-481-a-workspace-roster-centred-on-the-workspace-and-the-member-roster-single-backing-only
/// [S-431]: ../../../docs/planning/journal.md#s-431-the-chat-agents-tool-surface-is-workspace-aware
/// [FR-WS-34]: ../../../docs/specs/requirements/FR-WS-34.md
pub const WORKSPACE_ANALYST_PREAMBLE: &str = "\
You are the Workspace-Analyst subagent of Logos, a structural code-intelligence tool. \
You answer one step of a larger plan about a multi-repository WORKSPACE as a whole, \
with read-only tools: workspace_status, workspace_reachability, workspace_check, \
xservice_build_deps and workspace_roster read the workspace's own read-models; \
xservice_route_providers, xservice_callers, xservice_impact and xservice_search answer \
across services. workspace_roster reads the manifest alone and starts no member, so \
it is the first move for a question about which members exist. Pass `repo` to \
xservice_search when you already know the member, so only that member is opened. Do \
not scope xservice_callers or xservice_impact to the provider's own member: their \
residue would then cover only that member's outbound calls, not the consumers that \
might reach it. Pass a hit's canonical `symbol` to them, never a bare name. \
Cross-service results are repo-qualified: name the member with every one, and never \
merge the same symbol from two members into one. A reading that says UNRESOLVED or NOT CHECKED is \
not an absence — report it as it says, with its count, never as \"none\". Never \
compute a mean, sum or score across members' signals; report each member's own. Call \
the tools you need, then reply with a concise plain-text summary grounded in the tool \
results.";

/// System preamble for the workspace roster's repo-addressed **Graph-Navigator**
/// ([S-481], [S-480]) — [`GRAPH_NAVIGATOR_PREAMBLE`]'s tools and `context`
/// steering, over the one member the step's `repo` names.
///
/// [S-481]: ../../../docs/planning/journal.md#s-481-a-workspace-roster-centred-on-the-workspace-and-the-member-roster-single-backing-only
/// [S-480]: ../../../docs/planning/journal.md#s-480-agent-tools-address-a-named-member-and-the-workspace-read-models-become-tools
pub const WORKSPACE_GRAPH_NAVIGATOR_PREAMBLE: &str = "\
You are the Graph-Navigator subagent of Logos, a structural code-intelligence tool, \
in a multi-repository WORKSPACE. You answer one step of a larger plan by navigating \
ONE member's code graph with your tools: search, context, node, callers, callees, \
impact, explore, affected. Every tool takes a required `repo`: the member the step \
names, exactly as named. Only that member is opened. An unknown `repo` is an error \
listing the members — choose from it, never guess. Call the tools you need to gather \
grounded facts, then reply with a concise plain-text summary of what you found that \
names the member. Ground every claim in a tool result — never invent a symbol, edge, \
or count. When a step is broad — it spans multiple symbols, or asks for a \
neighborhood/overview rather than one known symbol — prefer a single `context` call \
(one ranked multi-symbol bundle) over several separate `search`/`node` calls; reach \
for `search`/`node` once you already know the specific symbol you need.";

/// System preamble for the workspace roster's repo-addressed
/// **Governance-Analyst** ([S-481], [S-480]).
///
/// [S-481]: ../../../docs/planning/journal.md#s-481-a-workspace-roster-centred-on-the-workspace-and-the-member-roster-single-backing-only
/// [S-480]: ../../../docs/planning/journal.md#s-480-agent-tools-address-a-named-member-and-the-workspace-read-models-become-tools
pub const WORKSPACE_GOVERNANCE_ANALYST_PREAMBLE: &str = "\
You are the Governance-Analyst subagent of Logos, a structural code-intelligence \
tool, in a multi-repository WORKSPACE. You answer one step of a larger plan by \
running ONE member's governance/quality read-models with your tools: scan, \
check_rules, hotspots, dsm, gate, evolution, doc_gaps, health. Every tool takes a \
required `repo`: the member the step names, exactly as named. An unknown `repo` is an \
error listing the members — choose from it, never guess. Each member's signal is \
measured against that member's own baseline: report it as that member's, and never \
combine two members' signals into one figure. Call the tools you need, then reply \
with a concise plain-text summary of the grounded signal that names the member. Never \
fabricate a metric or a verdict.";

/// System preamble for the workspace roster's repo-addressed **Source-Reader**
/// ([S-481], [S-480], [NFR-SE-04]).
///
/// [S-481]: ../../../docs/planning/journal.md#s-481-a-workspace-roster-centred-on-the-workspace-and-the-member-roster-single-backing-only
/// [S-480]: ../../../docs/planning/journal.md#s-480-agent-tools-address-a-named-member-and-the-workspace-read-models-become-tools
/// [NFR-SE-04]: ../../../docs/specs/requirements/NFR-SE-04.md
pub const WORKSPACE_SOURCE_READER_PREAMBLE: &str = "\
You are the Source-Reader subagent of Logos, a structural code-intelligence tool, in \
a multi-repository WORKSPACE. You answer one step of a larger plan by reading source \
files WITHIN one member with your sandboxed tools: read, grep, glob. Every tool takes \
a required `repo`: the member the step names, exactly as named. Paths are relative to \
that member's root, and the tools are confined to it. An unknown `repo` is an error \
listing the members — choose from it, never guess. Call the tools you need, then reply \
with a concise plain-text summary grounded in the file contents you read that names \
the member. Never invent file contents.";

/// The [BR-59] clause of the workspace Synthesizer's preamble: a cross-member
/// answer ranks the members by their own signals and never composes one.
///
/// [BR-59]: ../../../docs/specs/software-spec.md#327-workspace-federation
pub const WORKSPACE_RANKING_CLAUSE: &str = "\
When you compare members, rank them by their own named signals or findings, citing \
each with the member it belongs to and its baseline. Never state a mean, sum, \
weighted figure or workspace score of per-member signals: each is measured against \
its own member's baseline, so a figure composed from several has no referent. A \
workspace-level figure enters the answer only when a workspace tool computed it from \
workspace-level evidence.";

/// The workspace roster's **Synthesizer** preamble for `federation` ([S-481],
/// [FR-WS-34]): told it answers for the workspace, given every member with its
/// declared kind ([`member_roster`]), told that a reading beats a subagent's prose
/// ([BR-53], carried from the retired cross-service addendum), and bound by the
/// [BR-59] ranking clause ([`WORKSPACE_RANKING_CLAUSE`]). It keeps
/// [`SYNTHESIZER_PREAMBLE`]'s grounding discipline and its Mermaid rule (HF-2).
///
/// [S-481]: ../../../docs/planning/journal.md#s-481-a-workspace-roster-centred-on-the-workspace-and-the-member-roster-single-backing-only
/// [FR-WS-34]: ../../../docs/specs/requirements/FR-WS-34.md
/// [BR-53]: ../../../docs/specs/software-spec.md#327-workspace-federation
/// [BR-59]: ../../../docs/specs/software-spec.md#327-workspace-federation
pub fn workspace_synthesizer_preamble(federation: &Federation) -> String {
    format!(
        "\
You are the Synthesizer subagent of Logos, a structural code-intelligence tool, \
answering for the multi-repository WORKSPACE \"{name}\". You have NO tools. Using only \
the observations the other subagents have already gathered (provided in your \
instruction), compose the final, grounded answer to the user's question in clear \
prose. Ground every claim in those observations; if they are insufficient, say so \
honestly rather than inventing facts.\n\n\
{roster}\n\n\
Name the member with every result, and never merge the same symbol from two members \
into one. Observations may end with \"{READINGS_HEADING}\" — verbatim tool results. \
Where a reading and a subagent's prose disagree, the reading wins. A reading marked \
UNRESOLVED or NOT CHECKED means the cross-service answer is incomplete: say so, with \
its count and reasons, and never answer \"none\" or \"no other service\" from it.\n\n\
{WORKSPACE_RANKING_CLAUSE}\n\n\
In a Mermaid sequence diagram, never put a bare ';' in message or note text — use \
'#59;' or rephrase, and use '<br/>' for line breaks.",
        name = federation.name,
        roster = member_roster(federation),
    )
}

/// Supplies the grounding context the tool-less **Synthesizer** composes its
/// final answer from — the seam that wires S-175's persisted scratchpad into the
/// Synthesizer's prompt in production ([FR-UI-20], [S-175] AC1: "the Synthesizer
/// uses the scratchpad in the final answer").
///
/// The orchestrator's in-memory loop already threads observations to the planner,
/// which encodes them into the Synthesizer step's instruction. This seam makes the
/// grounding **authoritative and complete**: rather than trusting whatever prose
/// the planner copied into the instruction, the roster injects the rendered
/// per-turn scratchpad (the plan + every subagent observation) directly, so the
/// final answer reflects the recorded findings even as the persisted record is the
/// single source of truth. Production supplies
/// [`MemoryGrounding`](crate::memory::MemoryGrounding) over
/// [`MemoryStore::render_scratchpad`](crate::memory::MemoryStore::render_scratchpad);
/// it is read at synthesis time, after the prior steps' observations have streamed
/// into the store.
///
/// [FR-UI-20]: ../../../docs/specs/requirements/FR-UI-20.md
/// [S-175]: ../../../docs/planning/journal.md#s-175-multi-step-agent-memory-store-scratchpad-and-working-memory
pub trait SynthesizerGrounding: Send + Sync {
    /// The grounding block to prepend to the Synthesizer's instruction — typically
    /// the rendered per-turn scratchpad.
    fn grounding(&self) -> String;
}

/// Compose the tool-less Synthesizer's prompt from the grounding scratchpad and
/// the planner's synthesis instruction, so the model answers **from the recorded
/// observations** rather than the bare instruction alone ([S-175] AC1).
fn compose_synthesis_prompt(grounding: &str, instruction: &str) -> String {
    format!(
        "Scratchpad — the plan and every subagent observation gathered this turn:\n\
         {grounding}\n\n\
         Using only those observations, {instruction}"
    )
}

/// The member roster's system preamble for a subagent [`StepRole`]; `None` for
/// the [`WorkspaceAnalyst`](StepRole::WorkspaceAnalyst), a role only the
/// workspace roster carries.
fn preamble_for(role: StepRole) -> Option<&'static str> {
    match role {
        StepRole::GraphNavigator => Some(GRAPH_NAVIGATOR_PREAMBLE),
        StepRole::GovernanceAnalyst => Some(GOVERNANCE_ANALYST_PREAMBLE),
        StepRole::SourceReader => Some(SOURCE_READER_PREAMBLE),
        StepRole::Synthesizer => Some(SYNTHESIZER_PREAMBLE),
        StepRole::WorkspaceAnalyst => None,
    }
}

/// Append the running per-step budget status to a tool-bearing subagent's
/// preamble — the [S-182] budget-awareness that complements the [S-181] soft
/// cap ([CR-048]) by reducing how often a step is bounded at all.
///
/// Names the step's tool-call cap and the calls **currently** remaining on
/// `budget`, and steers the subagent to summarize before it runs out. Rebuilt
/// fresh before every model round (never baked into the `&'static str` base
/// preamble) so "calls remaining" is a genuinely **running** count that falls
/// as the step's [`ToolBudget`] is charged — not a stale, request-time-zero
/// snapshot.
fn budget_aware_preamble(base: &str, budget: &ToolBudget) -> String {
    format!(
        "{base}\n\nBudget: this step's tool-call cap is {limit}; you have {remaining} \
         call(s) remaining. Economize — gather only what this step needs — and summarize \
         what you have found in plain text BEFORE you exhaust your remaining calls, rather \
         than after.",
        limit = budget.limit(),
        remaining = budget.remaining(),
    )
}

/// The completion model backing each subagent role.
///
/// One model per role so the `[chat.models]` per-role overrides ([FR-CF-06],
/// [`StepRole::as_chat_role`]) can resolve a distinct model per subagent; build
/// with [`RoleModels::uniform`] to share one model across the roster (the common
/// case — and how the offline mock backs all four roles in tests). The workspace
/// roster takes its Workspace-Analyst's model beside these
/// ([`WorkspaceRoster::with_models`]).
#[derive(Debug, Clone)]
pub struct RoleModels<M> {
    /// Model for the Graph-Navigator.
    pub graph_navigator: M,
    /// Model for the Governance-Analyst.
    pub governance_analyst: M,
    /// Model for the Source-Reader.
    pub source_reader: M,
    /// Model for the Synthesizer.
    pub synthesizer: M,
}

impl<M: Clone> RoleModels<M> {
    /// Use one model for every role.
    pub fn uniform(model: M) -> Self {
        Self {
            graph_navigator: model.clone(),
            governance_analyst: model.clone(),
            source_reader: model.clone(),
            synthesizer: model,
        }
    }

    /// The model for `role`; `None` for the Workspace-Analyst, whose model the
    /// workspace roster holds beside these.
    fn for_role(&self, role: StepRole) -> Option<&M> {
        match role {
            StepRole::GraphNavigator => Some(&self.graph_navigator),
            StepRole::GovernanceAnalyst => Some(&self.governance_analyst),
            StepRole::SourceReader => Some(&self.source_reader),
            StepRole::Synthesizer => Some(&self.synthesizer),
            StepRole::WorkspaceAnalyst => None,
        }
    }
}

/// What both rosters run a step with beyond the role's model, preamble and tools:
/// the optional `[chat]` sampling params and the Synthesizer's grounding — and the
/// one step runner both rosters share, so the two can never drift apart in how a
/// step is executed.
#[derive(Default)]
struct StepRunner {
    temperature: Option<f64>,
    max_tokens: Option<u64>,
    /// The per-turn scratchpad grounding injected into the Synthesizer's prompt
    /// ([`SynthesizerGrounding`], [S-175] AC1). `None` (the default) leaves the
    /// Synthesizer grounded only by the planner-built instruction — the offline
    /// roster tests' baseline; production sets a
    /// [`MemoryGrounding`](crate::memory::MemoryGrounding) per turn.
    ///
    /// [S-175]: ../../../docs/planning/journal.md#s-175-multi-step-agent-memory-store-scratchpad-and-working-memory
    synthesizer_grounding: Option<Arc<dyn SynthesizerGrounding>>,
}

impl StepRunner {
    /// Run `step` as `role` over `model`, `preamble` and `toolset` — a
    /// tool-bearing subagent's bounded loop when there is a toolset, the
    /// tool-less Synthesizer's single streamed completion when there is none.
    async fn run<M>(
        &self,
        model: &M,
        preamble: &str,
        toolset: Option<agent_core::rig::tool::ToolSet>,
        step: &PlanStep,
        ctx: &StepContext<'_>,
    ) -> Result<StepObservation, StepError>
    where
        M: CompletionModel + Clone + Send + Sync + 'static,
    {
        match toolset {
            // Tool-bearing subagent: run its bounded tool loop.
            Some(toolset) => {
                run_tool_subagent(
                    model,
                    step.role,
                    preamble,
                    toolset,
                    &step.instruction,
                    ctx,
                    self.temperature,
                    self.max_tokens,
                )
                .await
            }
            // Tool-less Synthesizer: one tool-free completion, no budget charged.
            // When a grounding source is wired (production, S-170), the rendered
            // per-turn scratchpad is injected into the prompt so the answer is
            // composed from the recorded observations ([S-175] AC1); otherwise the
            // planner-built instruction stands alone.
            None => {
                let instruction = match &self.synthesizer_grounding {
                    Some(grounding) => {
                        compose_synthesis_prompt(&grounding.grounding(), &step.instruction)
                    }
                    None => step.instruction.clone(),
                };
                run_synthesizer(
                    model,
                    preamble,
                    &instruction,
                    self.temperature,
                    self.max_tokens,
                    ctx,
                )
                .await
            }
        }
    }
}

/// The **member** roster of specialized subagents — the real [`StepExecutor`]
/// the orchestrator dispatches a member chat's plan steps to ([S-174],
/// [ADR-41]).
///
/// Holds the shared [`Engine`] (behind the graph + governance tools), the
/// [`Sandbox`] (behind the source tools), the per-role [`RoleModels`], and the
/// optional `[chat]` sampling params applied to every subagent request.
///
/// It is single-backing only ([S-481], [ADR-71]): it takes no query backing, so
/// whichever backing its engine came from — a single root, or one member of a
/// workspace — it registers exactly the single-backing tools under the
/// single-backing preambles. Cross-service reach is the [`WorkspaceRoster`]'s.
///
/// [S-481]: ../../../docs/planning/journal.md#s-481-a-workspace-roster-centred-on-the-workspace-and-the-member-roster-single-backing-only
/// [ADR-71]: ../../../docs/specs/architecture/decisions/ADR-71.md
pub struct SubagentRoster<M> {
    engine: Arc<Engine>,
    sandbox: Arc<Sandbox>,
    models: RoleModels<M>,
    runner: StepRunner,
}

impl<M> SubagentRoster<M>
where
    M: CompletionModel + Clone + Send + Sync + 'static,
{
    /// Build a roster that backs every role with a single shared `model`.
    pub fn new(engine: Arc<Engine>, sandbox: Arc<Sandbox>, model: M) -> Self {
        Self::with_models(engine, sandbox, RoleModels::uniform(model))
    }

    /// Build a roster with a distinct model per role (the `[chat.models]`
    /// per-role overrides, [FR-CF-06]).
    pub fn with_models(engine: Arc<Engine>, sandbox: Arc<Sandbox>, models: RoleModels<M>) -> Self {
        Self {
            engine,
            sandbox,
            models,
            runner: StepRunner::default(),
        }
    }

    /// Ground the tool-less Synthesizer's final answer on `grounding` — the
    /// per-turn scratchpad the SSE seam (S-170) wires from the persisted
    /// [`MemoryStore`](crate::memory::MemoryStore), enforcing [S-175] AC1 in
    /// production. Built per turn because the grounding is turn-scoped.
    ///
    /// [S-175]: ../../../docs/planning/journal.md#s-175-multi-step-agent-memory-store-scratchpad-and-working-memory
    pub fn with_synthesizer_grounding(mut self, grounding: Arc<dyn SynthesizerGrounding>) -> Self {
        self.runner.synthesizer_grounding = Some(grounding);
        self
    }

    /// `role`'s registered tool definitions (name, description, schema), in
    /// registration order — exactly what its model is offered. Introspection for
    /// the [ADR-52] byte-identity assertion, which is made on what is registered
    /// rather than on a behaviour; the tool-less Synthesizer has none, and neither
    /// has the Workspace-Analyst, which this roster does not carry.
    ///
    /// [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
    pub async fn registered_tools(
        &self,
        role: StepRole,
    ) -> Vec<agent_core::rig::completion::ToolDefinition> {
        definitions(self.toolset_for(role)).await
    }

    /// The system preamble `role` runs under — empty for the Workspace-Analyst,
    /// which this roster does not carry.
    pub fn preamble(&self, role: StepRole) -> String {
        preamble_for(role).unwrap_or_default().to_string()
    }

    /// The planner preamble that matches this roster: the default one, which
    /// names exactly the four roles it carries. Owned here, as the workspace
    /// roster owns its own ([`WorkspaceRoster::planner_preamble`]), so a planner
    /// is never told about a role or tool its roster does not carry.
    pub fn planner_preamble(&self) -> String {
        super::planner::DEFAULT_PLANNER_PREAMBLE.to_string()
    }

    /// Set the sampling temperature applied to every subagent request
    /// (`[chat].temperature`, [FR-CF-06]).
    pub fn with_temperature(mut self, temperature: Option<f64>) -> Self {
        self.runner.temperature = temperature;
        self
    }

    /// Set the max-tokens applied to every subagent request
    /// (`[chat].max_tokens`, [FR-CF-06]).
    pub fn with_max_tokens(mut self, max_tokens: Option<u64>) -> Self {
        self.runner.max_tokens = max_tokens;
        self
    }

    /// Build the `rig` `ToolSet` for a tool-bearing role; the Synthesizer is
    /// tool-less ([`None`]), and the Workspace-Analyst is not on this roster.
    fn toolset_for(&self, role: StepRole) -> Option<agent_core::rig::tool::ToolSet> {
        match role {
            StepRole::GraphNavigator => Some(graph_toolset(self.engine.clone())),
            StepRole::GovernanceAnalyst => Some(governance_toolset(self.engine.clone())),
            StepRole::SourceReader => Some(source_toolset(self.sandbox.clone())),
            StepRole::Synthesizer | StepRole::WorkspaceAnalyst => None,
        }
    }
}

impl<M> StepExecutor for SubagentRoster<M>
where
    M: CompletionModel + Clone + Send + Sync + 'static,
{
    async fn execute(
        &self,
        step: &PlanStep,
        ctx: &StepContext<'_>,
    ) -> Result<StepObservation, StepError> {
        // A step routed to a role this roster does not carry is a misroute, not a
        // turn failure: it degrades to an `[unavailable — …]` observation naming
        // the roles there are, so the planner can reroute ([CR-060] Layer 3). It is
        // reached only when a planner names `workspace_analyst` unprompted — the
        // member planner preamble never does.
        //
        // [CR-060]: ../../../docs/requests/CR-060-chat-resilience-recoverable-faults.md
        let Some(model) = self.models.for_role(step.role) else {
            return Err(StepError::Unavailable(format!(
                "this chat answers for one codebase and has no {:?} subagent; route the step to \
                 graph_navigator, governance_analyst or source_reader",
                step.role
            )));
        };
        let preamble = self.preamble(step.role);
        self.runner
            .run(model, &preamble, self.toolset_for(step.role), step, ctx)
            .await
    }
}

/// The **workspace** roster of specialized subagents ([S-481], [FR-WS-34],
/// [ADR-71]) — the [`StepExecutor`] a workspace chat dispatches its plan steps
/// to, centred on the workspace and narrowing to a named member.
///
/// Built over one [`XserviceBacking`] — which exists only over a federated
/// backing — that serves every role: the Workspace-Analyst's workspace and
/// `xservice_*` tools, and the repo-addressed Graph-Navigator, Governance-Analyst
/// and Source-Reader ([S-480]). Building it, listing its tools or rendering its
/// preambles starts no member engine ([NFR-PE-10]): a repo-addressed call opens
/// its member when it runs.
///
/// The Workspace-Analyst runs under its own per-subagent cap: it is a role of its
/// own, so each of its steps draws its own [`ToolBudget`] from the turn's budget
/// tree rather than sharing a Graph-Navigator step's.
///
/// [S-481]: ../../../docs/planning/journal.md#s-481-a-workspace-roster-centred-on-the-workspace-and-the-member-roster-single-backing-only
/// [S-480]: ../../../docs/planning/journal.md#s-480-agent-tools-address-a-named-member-and-the-workspace-read-models-become-tools
/// [FR-WS-34]: ../../../docs/specs/requirements/FR-WS-34.md
/// [ADR-71]: ../../../docs/specs/architecture/decisions/ADR-71.md
/// [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md
pub struct WorkspaceRoster<M> {
    xservice: XserviceBacking,
    models: RoleModels<M>,
    workspace_analyst: M,
    runner: StepRunner,
}

impl<M> WorkspaceRoster<M>
where
    M: CompletionModel + Clone + Send + Sync + 'static,
{
    /// Build a workspace roster over `xservice` that backs every role with a
    /// single shared `model`.
    pub fn new(xservice: XserviceBacking, model: M) -> Self {
        Self::with_models(xservice, RoleModels::uniform(model.clone()), model)
    }

    /// Build a workspace roster over `xservice` with a distinct model per role:
    /// `models` for the four roles both rosters carry, `workspace_analyst` for
    /// the Workspace-Analyst.
    pub fn with_models(xservice: XserviceBacking, models: RoleModels<M>, workspace_analyst: M) -> Self {
        Self {
            xservice,
            models,
            workspace_analyst,
            runner: StepRunner::default(),
        }
    }

    /// Ground the tool-less Synthesizer's final answer on `grounding`, as
    /// [`SubagentRoster::with_synthesizer_grounding`] does.
    pub fn with_synthesizer_grounding(mut self, grounding: Arc<dyn SynthesizerGrounding>) -> Self {
        self.runner.synthesizer_grounding = Some(grounding);
        self
    }

    /// Set the sampling temperature applied to every subagent request
    /// (`[chat].temperature`, [FR-CF-06]).
    pub fn with_temperature(mut self, temperature: Option<f64>) -> Self {
        self.runner.temperature = temperature;
        self
    }

    /// Set the max-tokens applied to every subagent request
    /// (`[chat].max_tokens`, [FR-CF-06]).
    pub fn with_max_tokens(mut self, max_tokens: Option<u64>) -> Self {
        self.runner.max_tokens = max_tokens;
        self
    }

    /// The federated backing every role reads through — for assertions on what a
    /// turn constructed ([`XserviceBacking::registry`], [NFR-PE-10]).
    ///
    /// [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md
    pub fn xservice(&self) -> &XserviceBacking {
        &self.xservice
    }

    /// `role`'s registered tool definitions (name, description, schema), in
    /// registration order — exactly what its model is offered. The tool-less
    /// Synthesizer has none.
    pub async fn registered_tools(
        &self,
        role: StepRole,
    ) -> Vec<agent_core::rig::completion::ToolDefinition> {
        definitions(self.toolset_for(role)).await
    }

    /// The system preamble `role` runs under. The Synthesizer's names every
    /// member with its declared kind and carries the [BR-59] ranking clause
    /// ([`workspace_synthesizer_preamble`]).
    ///
    /// [BR-59]: ../../../docs/specs/software-spec.md#327-workspace-federation
    pub fn preamble(&self, role: StepRole) -> String {
        match role {
            StepRole::WorkspaceAnalyst => WORKSPACE_ANALYST_PREAMBLE.to_string(),
            StepRole::GraphNavigator => WORKSPACE_GRAPH_NAVIGATOR_PREAMBLE.to_string(),
            StepRole::GovernanceAnalyst => WORKSPACE_GOVERNANCE_ANALYST_PREAMBLE.to_string(),
            StepRole::SourceReader => WORKSPACE_SOURCE_READER_PREAMBLE.to_string(),
            StepRole::Synthesizer => workspace_synthesizer_preamble(self.federation()),
        }
    }

    /// The planner preamble that matches this roster: the workspace one, naming
    /// every member with its declared kind and each role's registered tools
    /// ([`workspace_planner_preamble`](super::planner::workspace_planner_preamble)).
    pub fn planner_preamble(&self) -> String {
        super::planner::workspace_planner_preamble(self.federation())
    }

    /// The resolved workspace — the manifest's member set and kinds, held by the
    /// registry; reading it starts no engine.
    fn federation(&self) -> &Federation {
        self.xservice.registry().federation()
    }

    /// Build the `rig` `ToolSet` for a tool-bearing role; the Synthesizer is
    /// tool-less ([`None`]). The Workspace-Analyst's set is the workspace tools
    /// followed by the `xservice_*` tools; every other role's is repo-addressed.
    /// Building any of them touches no member engine ([NFR-PE-10]).
    ///
    /// [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md
    fn toolset_for(&self, role: StepRole) -> Option<agent_core::rig::tool::ToolSet> {
        let xs = self.xservice.clone();
        match role {
            StepRole::WorkspaceAnalyst => {
                let mut toolset = workspace_toolset(xs.clone());
                toolset.add_tools(xservice_toolset(xs));
                Some(toolset)
            }
            StepRole::GraphNavigator => Some(addressed_toolset(ToolDomain::Graph, xs)),
            StepRole::GovernanceAnalyst => Some(addressed_toolset(ToolDomain::Governance, xs)),
            StepRole::SourceReader => Some(addressed_toolset(ToolDomain::Source, xs)),
            StepRole::Synthesizer => None,
        }
    }
}

impl<M> StepExecutor for WorkspaceRoster<M>
where
    M: CompletionModel + Clone + Send + Sync + 'static,
{
    async fn execute(
        &self,
        step: &PlanStep,
        ctx: &StepContext<'_>,
    ) -> Result<StepObservation, StepError> {
        let model = self
            .models
            .for_role(step.role)
            .unwrap_or(&self.workspace_analyst);
        let preamble = self.preamble(step.role);
        self.runner
            .run(model, &preamble, self.toolset_for(step.role), step, ctx)
            .await
    }
}

/// A toolset's definitions in registration order; none for a tool-less role.
async fn definitions(
    toolset: Option<agent_core::rig::tool::ToolSet>,
) -> Vec<agent_core::rig::completion::ToolDefinition> {
    match toolset {
        Some(toolset) => toolset.get_tool_definitions().await.unwrap_or_default(),
        None => Vec::new(),
    }
}

/// Run a tool-bearing subagent's bounded tool loop to a grounded observation.
///
/// Prompts the model with its preamble + the running conversation; on a tool call
/// it charges the budget tree and dispatches through the [`BoundedDispatcher`]
/// (which refuses out-of-subset tools and enforces the per-subagent cap), feeds
/// the tool result back, and re-prompts; on a plain-text reply it returns that as
/// the step's observation.
///
/// # Soft per-subagent cap ([CR-048], [ADR-41])
///
/// The per-subagent cap is a **soft** bound: when it is reached (or the shared
/// global ceiling is spent mid-step), the subagent does **not** halt the turn.
/// Instead it closes its conversation **well-formed** — a synthetic `tool_result`
/// answers every dangling `tool_use`, which both providers require — runs **one**
/// tool-free completion to summarize what it gathered, and returns a **marked,
/// bounded** [`StepObservation`] the orchestrator continues from
/// ([`close_and_summarize`]). Only the global ceiling and max-replans remain hard
/// turn-halts, enforced by the orchestrator loop ([`super::Orchestrator::run`]).
///
/// # Tool errors are self-correcting observations ([CR-060], [FR-UI-28])
///
/// A tool that ran and failed ([`DispatchError::Tool`], e.g. `read` of a missing
/// path) and an out-of-domain tool request ([`DispatchError::ToolNotFound`], a
/// misroute outside the subagent's least-privilege subset) are **not** turn-fatal.
/// Each is fed back to the model as a `tool_result` **observation** it can
/// self-correct from — the out-of-domain refusal names the subagent's available
/// tools so it can reroute — and the loop re-prompts. This is bounded by a
/// **consecutive-tool-error streak cap** ([`MAX_CONSECUTIVE_TOOL_ERRORS`]) that
/// resets on **any** successful dispatch; when the streak reaches the cap the step
/// soft-closes through the same [`close_and_summarize`] machinery with a
/// [`CloseReason::ToolErrors`] marker. Because a `ToolNotFound` charges no budget,
/// this streak cap is the sole termination guarantee for a pure `ToolNotFound`
/// loop.
///
/// The **one exception** is a security-sandbox / containment refusal — a source
/// tool refusing a path that escapes the project root ([`DispatchError::Containment`],
/// [NFR-SE-04]). It is **turn-fatal**: it returns [`StepError::Failed`] naming the
/// refusal with no fabricated answer, never the recoverable observation path
/// ([NFR-CC-04], CR-063). The distinction is drawn structurally at the dispatch
/// seam (the typed variant), so it is robust to the refusal's message wording.
///
/// # Recoverable faults degrade, they don't abort ([CR-060] Layer 3, [FR-UI-28])
///
/// A provider-call fault surviving the agent-core retry seam ([S-240]), a model
/// that returns neither a tool call nor text, and a bounded-summarization provider
/// fault are **recoverable**: each is surfaced as [`StepError::Unavailable`], which
/// the orchestrator loop degrades to a `[unavailable — …]` scratchpad observation
/// so the turn routes around this step ([`super::Orchestrator::run`]). Only
/// **structural** faults — the toolset failing to load, or an inconsistent
/// (unexpectedly empty) conversation — stay honest turn-fatal [`StepError::Failed`].
/// A bounded summary is never fabricated: an empty closing reply yields an honest
/// "no summary produced" observation, not an invented one ([NFR-CC-04]).
#[allow(clippy::too_many_arguments)]
async fn run_tool_subagent<M>(
    model: &M,
    role: StepRole,
    base_preamble: &str,
    toolset: agent_core::rig::tool::ToolSet,
    instruction: &str,
    ctx: &StepContext<'_>,
    temperature: Option<f64>,
    max_tokens: Option<u64>,
) -> Result<StepObservation, StepError>
where
    M: CompletionModel + Clone + 'static,
{
    let tool_defs = toolset.get_tool_definitions().await.map_err(|e| {
        StepError::Failed(format!("could not load the {role:?} subagent's tools: {e}"))
    })?;
    // The names of this subagent's tools — named back to the model when it
    // requests one outside its domain, so an out-of-domain refusal is an
    // actionable, self-correcting observation ([CR-060] Layer 2).
    let tool_names: Vec<String> = tool_defs.iter().map(|d| d.name.clone()).collect();
    let dispatcher = BoundedDispatcher::new(toolset, ctx.subagent_budget());

    // The running conversation; its last message is the prompt, the rest the
    // history (the build step prepends the system preamble).
    let mut conversation: Vec<Message> = vec![Message::user(instruction)];

    // Consecutive tool errors with no successful dispatch in between — bounds the
    // self-correction loop ([CR-060], [`MAX_CONSECUTIVE_TOOL_ERRORS`]). Reset to 0
    // on any `Ok` dispatch; when it reaches the cap the step soft-closes.
    let mut tool_error_streak: usize = 0;

    // The `reading` line of every `xservice_*` and workspace read-model result
    // this step dispatched, in dispatch order — appended verbatim to whatever
    // observation the step ends with ([`with_readings`]). Always empty on the
    // member roster, which registers neither family.
    let mut readings: Vec<String> = Vec::new();

    loop {
        // Rebuilt every round from the step's CURRENT `ToolBudget` state, so the
        // "calls remaining" the subagent is told is a genuinely running count,
        // not a value fixed when the step started ([S-182], [CR-048]).
        let preamble = budget_aware_preamble(base_preamble, ctx.subagent_budget());

        // The conversation is seeded non-empty and only ever grown, so this holds
        // structurally; surface it as an honest fault rather than a panic, since
        // this loop now runs inside the SSE turn's spawned task and a panic there
        // would close the stream with no error event ([NFR-CC-04]).
        let (prompt, history) = conversation.split_last().ok_or_else(|| {
            StepError::Failed("the subagent conversation was unexpectedly empty".to_string())
        })?;
        let request = model
            .completion_request(prompt.clone())
            .preamble(preamble.clone())
            .messages(history.iter().cloned())
            .tools(tool_defs.clone())
            .temperature_opt(temperature)
            .max_tokens_opt(max_tokens)
            .build();
        let response = model.completion(request).await.map_err(|e| {
            // A provider fault that survived the agent-core retry seam ([S-240]) is
            // RECOVERABLE, not turn-fatal: degrade this step to a `[unavailable — …]`
            // observation and let the turn route around it ([CR-060] Layer 3,
            // [FR-UI-28]). Carry the classified, source-chained cause (S-199) rather
            // than the flattened `{e}`, so a subagent transport/HTTP/auth failure is
            // legible.
            StepError::Unavailable(format!(
                "the {role:?} subagent provider failed: {}",
                agent_core::classify_provider_error(&e)
            ))
        })?;

        // Partition the assistant content into the tool calls it requested and any
        // plain text it produced.
        let mut tool_calls = Vec::new();
        let mut text = String::new();
        for content in response.choice.iter() {
            match content {
                AssistantContent::ToolCall(tc) => tool_calls.push(tc.clone()),
                AssistantContent::Text(t) => push_line(&mut text, &t.text),
                // Reasoning/image content carries no grounded result for the
                // scratchpad; ignore it.
                _ => {}
            }
        }

        // No tool call this round → the subagent's grounded summary is the
        // observation the planner and Synthesizer read.
        if tool_calls.is_empty() {
            if text.trim().is_empty() {
                // A model that produced neither a tool call nor text is a RECOVERABLE
                // runtime fault, not a turn-fatal one: degrade this step so the turn
                // routes around it ([CR-060] Layer 3, [FR-UI-28], [NFR-CC-04]).
                return Err(StepError::Unavailable(format!(
                    "the {role:?} subagent returned neither a tool call nor an answer"
                )));
            }
            return Ok(StepObservation::new(with_readings(text, &readings)));
        }

        // Record the assistant's tool-call turn, then run each call (charged).
        conversation.push(Message::Assistant {
            id: response.message_id.clone(),
            content: response.choice.clone(),
        });

        for (idx, tool_call) in tool_calls.iter().enumerate() {
            // A spent global ceiling ends this step's exploration — but as a SOFT
            // bound, not a turn halt ([CR-048], [ADR-41]): the subagent closes out
            // well-formed and summarizes what it already gathered rather than
            // discarding it. The tool calls not yet dispatched (this one and any
            // after it) are the dangling `tool_use`s the close-out must answer.
            // The turn-level hard halt on the global ceiling is the orchestrator
            // loop's job ([`super::Orchestrator::run`]).
            if ctx.budget_tree().global_remaining() == 0 {
                // Rebuilt here rather than reusing the loop-top `preamble`: an
                // earlier call THIS round may already have charged
                // `ctx.subagent_budget()` (e.g. `tool_calls[idx - 1]` dispatched
                // successfully before this one found the global ceiling spent), so
                // the loop-top snapshot could under-state calls already spent —
                // the close-out preamble must reflect the budget as it stands
                // right now, never a stale count ([S-182]).
                let closing_preamble = budget_aware_preamble(base_preamble, ctx.subagent_budget());
                return close_and_summarize(
                    model,
                    role,
                    &closing_preamble,
                    conversation,
                    &tool_calls[idx..],
                    CloseReason::GlobalCeiling {
                        limit: ctx.budget_tree().global_limit(),
                    },
                    &readings,
                    temperature,
                    max_tokens,
                )
                .await;
            }

            let name = tool_call.function.name.as_str();
            let args = tool_call.function.arguments.to_string();
            match dispatcher.dispatch(name, args).await {
                Ok(output) => {
                    // The dispatcher charged this step's per-subagent budget; now
                    // charge the shared global ceiling for the call that ran.
                    ctx.budget_tree().charge_global()?;
                    if let Some(reading) = xservice_reading(name, &output)
                        .or_else(|| workspace_reading(name, &output))
                    {
                        readings.push(reading);
                    }
                    conversation.push(Message::tool_result(tool_call.id.clone(), output));
                    // Progress — a successful dispatch resets the consecutive-error
                    // streak so an earlier stumble the subagent recovered from never
                    // counts toward the soft-close cap ([CR-060] Layer 2).
                    tool_error_streak = 0;
                }
                // Per-subagent cap reached — the SOFT bound: close the conversation
                // out well-formed and summarize, so the turn continues from a marked
                // bounded observation rather than halting ([CR-048], [NFR-CC-04]).
                Err(DispatchError::BudgetExhausted(exhausted)) => {
                    // Same rebuild-before-close-out reasoning as the global-ceiling
                    // branch above: reflect the budget as it stands right now, not
                    // the loop-top snapshot ([S-182]).
                    let closing_preamble =
                        budget_aware_preamble(base_preamble, ctx.subagent_budget());
                    return close_and_summarize(
                        model,
                        role,
                        &closing_preamble,
                        conversation,
                        &tool_calls[idx..],
                        CloseReason::SubagentCap {
                            limit: exhausted.limit,
                        },
                        &readings,
                        temperature,
                        max_tokens,
                    )
                    .await;
                }
                // A tool outside this subagent's least-privilege domain — refused
                // with NO charge (S-174 acceptance preserved). Rather than aborting
                // the turn, feed the refusal back as a model-visible observation
                // that NAMES the subagent's available tools, so it can reroute
                // ([CR-060] Layer 2, [FR-UI-28]). Since a misroute charges no
                // budget, the consecutive-error cap is its sole termination bound.
                Err(DispatchError::ToolNotFound(requested)) => {
                    conversation.push(Message::tool_result(
                        tool_call.id.clone(),
                        format!(
                            "error: `{requested}` is not one of your available tools. Your tools \
                             are: {tools}. Call one of those instead, or — if you already have \
                             enough — reply with your grounded summary in plain text.",
                            tools = tool_names.join(", "),
                        ),
                    ));
                    if let Some(observation) = note_tool_error_and_maybe_close(
                        model,
                        role,
                        base_preamble,
                        &mut conversation,
                        &tool_calls[idx + 1..],
                        &mut tool_error_streak,
                        &readings,
                        ctx,
                        temperature,
                        max_tokens,
                    )
                    .await?
                    {
                        return Ok(observation);
                    }
                }
                // A security-sandbox / containment refusal — a source tool refusing
                // a path that escapes the project root ([NFR-SE-04]). This is the
                // ONE tool failure that is NOT a recoverable route-around fault:
                // unlike a benign tool error it is TURN-FATAL, surfaced as an honest
                // `StepError::Failed` that NAMES the refusal with no fabricated
                // answer ([NFR-CC-04], CR-063). The dispatch seam classified it
                // structurally (the typed `DispatchError::Containment` variant, not
                // an error-text match), so every source tool that consults the
                // sandbox inherits this behavior. The turn aborts here — no
                // re-prompt, no soft-close, no best-effort synthesis over the escape.
                Err(DispatchError::Containment(refusal)) => {
                    return Err(StepError::Failed(format!(
                        "the {role:?} subagent's `{name}` tool refused a sandbox escape: {refusal}"
                    )));
                }
                // The tool ran and failed inside itself (e.g. `read` of a missing
                // path). Feed the failure back as a self-correcting observation the
                // subagent can adapt to — try a different tool/arguments or answer
                // from what it has — rather than aborting the turn ([CR-060] Layer 2,
                // [FR-UI-28]). Bounded by the same consecutive-error streak cap.
                Err(DispatchError::Tool(e)) => {
                    conversation.push(Message::tool_result(
                        tool_call.id.clone(),
                        format!(
                            "error: tool `{name}` failed: {e}. This is an observation, not a turn \
                             failure — adapt (try a different tool or arguments), or — if you \
                             already have enough — reply with your grounded summary in plain text.",
                        ),
                    ));
                    if let Some(observation) = note_tool_error_and_maybe_close(
                        model,
                        role,
                        base_preamble,
                        &mut conversation,
                        &tool_calls[idx + 1..],
                        &mut tool_error_streak,
                        &readings,
                        ctx,
                        temperature,
                        max_tokens,
                    )
                    .await?
                    {
                        return Ok(observation);
                    }
                }
            }
        }
    }
}

/// Bump the consecutive-tool-error `streak` after its error observation has been
/// pushed onto `conversation`, and soft-close the step when the streak reaches
/// [`MAX_CONSECUTIVE_TOOL_ERRORS`] ([CR-060] Layer 2, [FR-UI-28]).
///
/// Returns `Ok(Some(observation))` when the cap tripped and the step soft-closed
/// (the caller returns it), or `Ok(None)` when the subagent should keep going and
/// re-prompt. The caller must push the error `tool_result` for the **current**
/// failing call *before* invoking this, so `dangling` is only the calls **after**
/// it in the same assistant turn — every `tool_use` stays answered and the
/// close-out conversation is well-formed.
#[allow(clippy::too_many_arguments)]
async fn note_tool_error_and_maybe_close<M>(
    model: &M,
    role: StepRole,
    base_preamble: &str,
    conversation: &mut Vec<Message>,
    dangling: &[ToolCall],
    streak: &mut usize,
    readings: &[String],
    ctx: &StepContext<'_>,
    temperature: Option<f64>,
    max_tokens: Option<u64>,
) -> Result<Option<StepObservation>, StepError>
where
    M: CompletionModel + Clone + 'static,
{
    *streak += 1;
    if *streak < MAX_CONSECUTIVE_TOOL_ERRORS {
        return Ok(None);
    }
    // Rebuilt from the CURRENT budget rather than a loop-top snapshot, mirroring
    // the budget-bound close-outs ([S-182]): an earlier successful call this round
    // may already have charged the per-subagent budget.
    let closing_preamble = budget_aware_preamble(base_preamble, ctx.subagent_budget());
    let observation = close_and_summarize(
        model,
        role,
        &closing_preamble,
        std::mem::take(conversation),
        dangling,
        CloseReason::ToolErrors { count: *streak },
        readings,
        temperature,
        max_tokens,
    )
    .await?;
    Ok(Some(observation))
}

/// Why a tool-bearing subagent is closing out its step early — the soft bound it
/// reached, used to phrase the honest close-out directive and the observation's
/// bounded marker ([CR-048]).
#[derive(Debug, Clone, Copy)]
enum CloseReason {
    /// The per-subagent tool-call cap (`max_subagent_tool_calls`) was reached.
    SubagentCap {
        /// The per-subagent cap that bound this step.
        limit: usize,
    },
    /// The shared global per-turn ceiling (`max_tool_calls`) was spent mid-step.
    GlobalCeiling {
        /// The global ceiling that bound this step.
        limit: usize,
    },
    /// The consecutive-tool-error streak cap ([`MAX_CONSECUTIVE_TOOL_ERRORS`])
    /// was reached — the subagent kept hitting tool errors / out-of-domain
    /// requests without a successful dispatch resetting the streak ([CR-060]
    /// Layer 2, [FR-UI-28]). Unlike the two budget bounds this is not a budget
    /// halt: it is the **sole** termination guarantee for a pure `ToolNotFound`
    /// loop, which charges no budget.
    ///
    /// [CR-060]: ../../../docs/requests/CR-060-chat-resilience-recoverable-faults.md
    /// [FR-UI-28]: ../../../docs/specs/requirements/FR-UI-28.md
    ToolErrors {
        /// The number of consecutive tool errors that tripped the cap.
        count: usize,
    },
}

impl CloseReason {
    /// The bounded-marker fragment naming which bound was reached — the prefix of
    /// the marked observation the planner and Synthesizer read.
    fn marker_reason(self) -> String {
        match self {
            CloseReason::SubagentCap { limit } => {
                format!("reached the {limit}-tool-call subagent cap")
            }
            CloseReason::GlobalCeiling { limit } => {
                format!("reached the turn's {limit}-tool-call ceiling")
            }
            CloseReason::ToolErrors { count } => {
                format!("hit {count} consecutive tool errors")
            }
        }
    }

    /// How the close-out directive names the spent budget to the model.
    fn budget_phrase(self) -> String {
        match self {
            CloseReason::SubagentCap { limit } => {
                format!("all {limit} of your tool calls for this step")
            }
            CloseReason::GlobalCeiling { limit } => {
                format!("the turn's shared {limit}-tool-call budget")
            }
            CloseReason::ToolErrors { count } => {
                format!("up your allowance after {count} consecutive tool errors")
            }
        }
    }
}

/// Close a bounded subagent's conversation **well-formed** and run **one**
/// tool-free completion to summarize what it gathered, returning a **marked,
/// bounded** observation ([CR-048] Strategy A, [ADR-41], [NFR-CC-04]).
///
/// The soft-cap contract has three parts, each load-bearing:
///
/// 1. **Well-formedness.** Every `dangling` `tool_use` (the refused call and any
///    the assistant requested after it in the same turn) is answered with a
///    synthetic `tool_result`. Both the Anthropic and OpenAI-compatible providers
///    reject a request whose history leaves a `tool_use` unanswered, so this
///    close-out is what lets the summarization request succeed at all.
/// 2. **Tool-free summarization.** The closing completion offers **no** tool
///    definitions and a directive not to request tools; any stray tool call the
///    model emits anyway is ignored (only its text is taken), so an unbudgeted
///    tool is never run ([NFR-CC-04]).
/// 3. **Honest marking.** The returned observation is prefixed with a bounded
///    marker so the planner and Synthesizer know it may be partial; an empty
///    closing reply yields an explicit "no summary produced" note rather than a
///    fabricated finding.
#[allow(clippy::too_many_arguments)]
async fn close_and_summarize<M>(
    model: &M,
    role: StepRole,
    preamble: &str,
    mut conversation: Vec<Message>,
    dangling: &[ToolCall],
    reason: CloseReason,
    readings: &[String],
    temperature: Option<f64>,
    max_tokens: Option<u64>,
) -> Result<StepObservation, StepError>
where
    M: CompletionModel + Clone + 'static,
{
    // 1. Answer every dangling `tool_use` so the conversation is well-formed.
    for tool_call in dangling {
        conversation.push(Message::tool_result(
            tool_call.id.clone(),
            "the step's tool-call budget was reached before this tool ran; it was not dispatched",
        ));
    }

    // 2. Re-prompt tool-free with a closing directive to summarize.
    conversation.push(Message::user(format!(
        "You have used {}. Do not request any more tools — using only the tool results \
         above, summarize what you found in concise plain text. If you gathered nothing \
         usable, say so plainly rather than inventing anything.",
        reason.budget_phrase(),
    )));

    let (prompt, history) = conversation.split_last().ok_or_else(|| {
        StepError::Failed("the bounded subagent conversation was unexpectedly empty".to_string())
    })?;
    // No `.tools(...)`: the closing request is tool-free, so no tool can be
    // dispatched and no budget is charged.
    let request = model
        .completion_request(prompt.clone())
        .preamble(preamble.to_string())
        .messages(history.iter().cloned())
        .temperature_opt(temperature)
        .max_tokens_opt(max_tokens)
        .build();
    let response = model.completion(request).await.map_err(|e| {
        // The tool-free closing round is still a provider call: if it faults after
        // the retry seam ([S-240]), the step is RECOVERABLE — degrade it so the turn
        // routes around it rather than aborting ([CR-060] Layer 3, [NFR-CC-04]).
        StepError::Unavailable(format!(
            "the {role:?} subagent's bounded summarization failed: {}",
            agent_core::classify_provider_error(&e)
        ))
    })?;

    // 3. Take only the plain text; ignore any stray tool call (none were offered).
    let mut text = String::new();
    for content in response.choice.iter() {
        if let AssistantContent::Text(t) = content {
            push_line(&mut text, &t.text);
        }
    }

    let reason = reason.marker_reason();
    let summary = if text.trim().is_empty() {
        // Honest: a bounded step that produced no prose says so, never fabricates.
        format!("[bounded — {reason}; no summary produced]")
    } else {
        format!("[bounded — {reason}; this summary may be partial]\n{text}")
    };
    Ok(StepObservation::new(with_readings(summary, readings)))
}

/// The heading the readings ride under in an observation ([`with_readings`]) —
/// named here once, because the workspace Synthesizer's preamble tells the model
/// to look for it ([`workspace_synthesizer_preamble`]).
const READINGS_HEADING: &str = "Workspace readings";

/// Append the step's `xservice_*` and workspace read-model readings, verbatim, to
/// its observation text ([S-431], [S-481], [BR-53], [NFR-CC-04]).
///
/// The readings are the tools' own deterministic lines — repo-qualified, and
/// `UNRESOLVED` where an empty cross-service answer sits over a non-zero residue
/// — so the planner and the Synthesizer read that qualification even when the
/// model's summary drops it. No readings (every member-roster step) leaves the
/// text untouched, byte for byte ([ADR-52]).
///
/// [S-431]: ../../../docs/planning/journal.md#s-431-the-chat-agents-tool-surface-is-workspace-aware
/// [S-481]: ../../../docs/planning/journal.md#s-481-a-workspace-roster-centred-on-the-workspace-and-the-member-roster-single-backing-only
/// [BR-53]: ../../../docs/specs/software-spec.md#327-workspace-federation
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
/// [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
fn with_readings(text: String, readings: &[String]) -> String {
    if readings.is_empty() {
        return text;
    }
    let mut out = text;
    // Deliberately neutral: the verdict word lives in each reading, so a
    // zero-residue answer carries no qualification it did not earn ([BR-53]).
    out.push_str(&format!("\n\n{READINGS_HEADING} (verbatim tool results):"));
    for reading in readings {
        out.push_str("\n- ");
        out.push_str(reading);
    }
    out
}

/// Run the tool-less Synthesizer: one **streaming** completion with **no** tools
/// attached, so it can make no tool call and charges no budget. Its prose is the
/// final grounded answer composed from the turn's observations, and — because that
/// prose IS the user-facing answer — each token is streamed through the step's
/// [`AnswerSink`](super::step::AnswerSink) as an
/// [`OrchestratorEvent::AnswerDelta`](super::OrchestratorEvent::AnswerDelta) so the
/// Chat tab types it out live ([FR-UI-19]). The accumulated text is returned as the
/// step's observation (and the planner's terminal `FinalAnswer` carries the
/// authoritative full text).
async fn run_synthesizer<M>(
    model: &M,
    preamble: &str,
    instruction: &str,
    temperature: Option<f64>,
    max_tokens: Option<u64>,
    ctx: &StepContext<'_>,
) -> Result<StepObservation, StepError>
where
    M: CompletionModel + Clone + 'static,
{
    use agent_core::rig::streaming::StreamedAssistantContent;
    use futures::StreamExt as _;

    let request = model
        .completion_request(Message::user(instruction))
        .preamble(preamble.to_string())
        .temperature_opt(temperature)
        .max_tokens_opt(max_tokens)
        .build();
    let mut stream = model.stream(request).await.map_err(|e| {
        // Classified, source-chained cause (S-199), not the flattened `{e}`.
        StepError::Failed(format!(
            "the synthesizer provider failed: {}",
            agent_core::classify_provider_error(&e)
        ))
    })?;

    // Concatenate the streamed text deltas (token-by-token, not line-joined) into
    // the full answer, emitting each chunk live as it arrives.
    let mut text = String::new();
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(StreamedAssistantContent::Text(t)) => {
                ctx.emit_answer_delta(&t.text);
                text.push_str(&t.text);
            }
            // The Synthesizer is given no tools; a tool call is a confused model,
            // surfaced honestly rather than dispatched.
            Ok(StreamedAssistantContent::ToolCall { tool_call, .. }) => {
                return Err(StepError::Failed(format!(
                    "the synthesizer is tool-less but requested `{}`",
                    tool_call.function.name
                )));
            }
            // Reasoning, partial-tool, and final-response markers carry no answer
            // prose; ignore them.
            Ok(_) => {}
            Err(e) => {
                return Err(StepError::Failed(format!(
                    "the synthesizer stream failed: {}",
                    agent_core::classify_provider_error(&e)
                )));
            }
        }
    }
    if text.trim().is_empty() {
        return Err(StepError::Failed(
            "the synthesizer produced no answer".to_string(),
        ));
    }
    Ok(StepObservation::new(text))
}

/// Append `line` to `text`, separating from any prior content with a newline.
fn push_line(text: &mut String, line: &str) {
    if !text.is_empty() {
        text.push('\n');
    }
    text.push_str(line);
}

#[cfg(test)]
mod budget_awareness_tests {
    //! [S-182]/[CR-048]: each tool-bearing subagent's preamble must name its
    //! per-step tool-call cap and a running "calls remaining", instruct
    //! summarizing before that budget is exhausted, and — for the
    //! Graph-Navigator specifically, the only role with a `context` tool —
    //! steer toward `context` over several `search`/`node` calls for a broad
    //! step. No change to the tool set or any subagent's tools ([S-182] AC3).

    use super::*;

    #[test]
    fn graph_navigator_preamble_prefers_context_over_search_and_node_for_breadth() {
        assert!(
            GRAPH_NAVIGATOR_PREAMBLE.contains("context"),
            "names the breadth-efficient `context` tool: {GRAPH_NAVIGATOR_PREAMBLE}"
        );
        assert!(
            GRAPH_NAVIGATOR_PREAMBLE.contains("search")
                && GRAPH_NAVIGATOR_PREAMBLE.contains("node"),
            "search/node stay named — the steering complements them, doesn't remove them: \
             {GRAPH_NAVIGATOR_PREAMBLE}"
        );
        assert!(
            GRAPH_NAVIGATOR_PREAMBLE.to_lowercase().contains("broad"),
            "the preference is scoped to broad steps: {GRAPH_NAVIGATOR_PREAMBLE}"
        );
    }

    /// The Graph-Navigator preamble text exactly as it stood immediately before
    /// [S-182] — the literal "un-hinted baseline" the sprint's acceptance
    /// criteria compare against. Kept here, not in production code, purely so
    /// this test can assert the current preamble is a genuine addition over it
    /// (never a replacement), grounding the "versus the un-hinted baseline"
    /// language at the level this offline suite CAN verify: the preamble text
    /// itself. The mock `CompletionModel` never reads a request's preamble (see
    /// `agent_core::MockCompletionModel::completion`/`stream`, which are scripted
    /// independently of it), so no offline test can drive an actual model
    /// through both preambles and observe a differing tool-call count from the
    /// text alone; the integration comparison test in `tests/roster.rs`
    /// (`a_single_context_call_answers_breadth_for_fewer_charges_than_several_search_node_calls`)
    /// instead proves the system-level efficiency property this new wording is
    /// designed to produce — a single `context` call is charged strictly fewer
    /// tool calls than several `search`/`node` calls for the same breadth, with
    /// every tool still fully dispatchable.
    const PRE_S182_GRAPH_NAVIGATOR_PREAMBLE: &str = "\
You are the Graph-Navigator subagent of Logos, a structural code-intelligence \
tool. You answer one step of a larger plan by navigating THIS codebase's code \
graph with your tools: search, context, node, callers, callees, impact, explore, \
affected. Call the tools you need to gather grounded facts, then reply with a \
concise plain-text summary of what you found. Ground every claim in a tool \
result — never invent a symbol, edge, or count.";

    #[test]
    fn graph_navigator_preamble_gained_the_context_preference_over_the_pre_s182_baseline() {
        assert!(
            !PRE_S182_GRAPH_NAVIGATOR_PREAMBLE
                .to_lowercase()
                .contains("prefer"),
            "the pre-S-182 baseline is genuinely un-hinted: {PRE_S182_GRAPH_NAVIGATOR_PREAMBLE}"
        );
        assert!(
            GRAPH_NAVIGATOR_PREAMBLE.to_lowercase().contains("prefer"),
            "the current preamble adds the context-preference steering: {GRAPH_NAVIGATOR_PREAMBLE}"
        );
        // The steering is an ADDITION over the un-hinted baseline, not a rewrite —
        // every word of the pre-S-182 text still leads the current preamble.
        assert!(
            GRAPH_NAVIGATOR_PREAMBLE.starts_with(PRE_S182_GRAPH_NAVIGATOR_PREAMBLE),
            "the un-hinted baseline text is preserved verbatim as a prefix: {GRAPH_NAVIGATOR_PREAMBLE}"
        );
    }

    #[test]
    fn budget_aware_preamble_names_the_cap_and_a_running_calls_remaining() {
        let budget = ToolBudget::new(5);
        budget.charge().expect("first charge");
        budget.charge().expect("second charge");

        let preamble = budget_aware_preamble(GRAPH_NAVIGATOR_PREAMBLE, &budget);

        assert!(
            preamble.contains("5"),
            "names the step's tool-call cap: {preamble}"
        );
        assert!(
            preamble.contains("3") && preamble.to_lowercase().contains("remaining"),
            "names the calls currently remaining (2 charged of 5): {preamble}"
        );
        assert!(
            preamble.to_lowercase().contains("summarize"),
            "instructs summarizing before the budget is exhausted: {preamble}"
        );
        // The base role text still leads the preamble — the budget line is an
        // addition, not a replacement.
        assert!(preamble.starts_with(GRAPH_NAVIGATOR_PREAMBLE));
    }

    #[test]
    fn budget_aware_preamble_calls_remaining_falls_as_the_budget_is_charged() {
        let budget = ToolBudget::new(3);
        let fresh = budget_aware_preamble(SOURCE_READER_PREAMBLE, &budget);
        assert!(fresh.contains("3 call(s) remaining"), "{fresh}");

        budget.charge().expect("charge");
        let after_one = budget_aware_preamble(SOURCE_READER_PREAMBLE, &budget);
        assert!(
            after_one.contains("2 call(s) remaining"),
            "the remaining count is a RUNNING count, not fixed at step start: {after_one}"
        );
    }

    #[test]
    fn every_tool_bearing_preamble_gets_budget_awareness() {
        let budget = ToolBudget::new(8);
        for base in [
            GRAPH_NAVIGATOR_PREAMBLE,
            GOVERNANCE_ANALYST_PREAMBLE,
            SOURCE_READER_PREAMBLE,
        ] {
            let preamble = budget_aware_preamble(base, &budget);
            assert!(preamble.contains("8"), "names the cap: {preamble}");
            assert!(
                preamble.to_lowercase().contains("remaining"),
                "names calls remaining: {preamble}"
            );
            assert!(
                preamble.to_lowercase().contains("summarize"),
                "instructs summarizing before exhaustion: {preamble}"
            );
        }
    }

    #[test]
    fn synthesizer_preamble_has_no_budget_annotation() {
        // The tool-less Synthesizer has no `ToolBudget` to report — `execute()`
        // routes it to `run_synthesizer` with the raw `preamble_for(role)` text,
        // never through `budget_aware_preamble`. This guards that invariant: a
        // future refactor that accidentally merged the tool-bearing and
        // Synthesizer branches (annotating a role with no budget) would fail
        // this assertion instead of shipping silently.
        assert!(
            !SYNTHESIZER_PREAMBLE.to_lowercase().contains("remaining"),
            "the Synthesizer has no tool budget to report: {SYNTHESIZER_PREAMBLE}"
        );
    }
}

#[cfg(test)]
mod workspace_preamble_tests {
    //! [S-481]: the workspace preambles are rewrites of the member ones, and the
    //! rules both chats must share are re-typed in them. These guards derive each
    //! shared sentence from the member preamble itself, so amending it there and
    //! not here fails a test instead of silently diverging the workspace chat.
    //!
    //! [S-481]: ../../../docs/planning/journal.md#s-481-a-workspace-roster-centred-on-the-workspace-and-the-member-roster-single-backing-only

    use super::*;
    use logos_core::federation::Member;

    fn federation() -> Federation {
        Federation {
            name: "shop".to_string(),
            root: std::path::PathBuf::from("/ws"),
            members: vec![Member { name: "api".to_string(), root: "/ws/api".into() }],
            default: None,
            links: Vec::new(),
            governance: Default::default(),
            warm_concurrency: None,
            member_kinds: Default::default(),
        }
    }

    /// `text` from the first occurrence of `from` to its end.
    fn tail_from<'a>(text: &'a str, from: &str) -> &'a str {
        &text[text.find(from).unwrap_or_else(|| panic!("{from:?} in {text}"))..]
    }

    #[test]
    fn the_workspace_synthesizer_keeps_the_member_grounding_and_mermaid_rules() {
        let workspace = workspace_synthesizer_preamble(&federation());
        // HF-2: both chats render through one Mermaid renderer, so the rule is the
        // member preamble's, verbatim, and it closes the workspace preamble too.
        let mermaid = tail_from(SYNTHESIZER_PREAMBLE, "In a Mermaid sequence diagram");
        assert!(workspace.ends_with(mermaid), "{workspace}");
        let grounding = tail_from(SYNTHESIZER_PREAMBLE, "Using only the observations");
        let grounding = &grounding[..grounding.find(" In a Mermaid").expect("rule follows")];
        assert!(workspace.contains(grounding), "{grounding}\n---\n{workspace}");
    }

    #[test]
    fn the_workspace_graph_navigator_keeps_the_member_context_steering() {
        let steering = tail_from(GRAPH_NAVIGATOR_PREAMBLE, "When a step is broad");
        assert!(WORKSPACE_GRAPH_NAVIGATOR_PREAMBLE.ends_with(steering));
        let grounding = "Ground every claim in a tool result — never invent a symbol, edge, or count.";
        assert!(GRAPH_NAVIGATOR_PREAMBLE.contains(grounding));
        assert!(WORKSPACE_GRAPH_NAVIGATOR_PREAMBLE.contains(grounding));
    }
}
