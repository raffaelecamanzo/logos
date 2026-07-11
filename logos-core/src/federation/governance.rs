//! The **workspace-level governance rule family** ([FR-WS-13], [ADR-56]).
//!
//! A second, *separate* rule family alongside the per-repo contract
//! ([`crate::governance`], [FR-GV-01]). Where that one quantifies over one
//! member's stored `nodes`/`edges` rows, this one quantifies over the
//! [bridge](super::bridge)'s **cross-service matches** — the in-memory
//! [`BridgeEdge`] set. Two rule kinds, both declared in the workspace manifest's
//! `[governance]` table:
//!
//! - `[[governance.boundaries]]` — a forbidden call between two named **service
//!   layers** (`[[governance.service_layers]]`), e.g. *"no `edge`-layer service
//!   may call a `core`-layer service"*.
//! - `[[governance.no_cross_service_callers]]` — a provider that must have **no**
//!   cross-service callers, e.g. *"this deprecated endpoint is called by nobody"*.
//!
//! # Separate from the per-repo gate, structurally ([ADR-56])
//! The separation is enforced by the type system, not by convention. This module
//! emits [`WorkspaceViolation`] — a **distinct type** from the per-repo
//! [`Violation`](crate::models::quality::Violation) the `violations` table stores
//! and the gate scores. There is no `From` impl between them and no writer here,
//! so a workspace rule *cannot* be persisted into a member's store or folded into
//! its signal even by mistake. In the other direction, [`crate::governance`] has
//! no dependency on `federation` at all, and this module is reachable only
//! through a [`Federation`] — which exists only when a manifest is present. The
//! single-root path never constructs one, so a repo with no workspace is
//! byte-for-byte unchanged ([FR-WS-01]).
//!
//! # Honest empty ([NFR-CC-04])
//! A workspace that declares **no** rules produces **no report** —
//! [`workspace_governance`] returns `None`, not a zero-violation report. An
//! undeclared policy must never read as a *passing* one: "we checked and found
//! nothing" and "there was nothing to check" are different claims, and only the
//! second is true.
//!
//! # Never fabricated ([NFR-RA-05])
//! Rules read the bridge's **actually-matched** edges. A consumer whose key never
//! bound (ambiguous, unresolvable, no provider in the workspace) contributes no
//! edge, so it can neither trigger nor satisfy a rule — it is simply not
//! evidence. The honest consequence: workspace governance is only as *complete*
//! as invocation coverage ([ADR-54]), the same monotone-toward-live posture
//! [ADR-56] takes for app-wide reachability. A malformed rule fails **loud**
//! rather than silently matching nothing, because a rule that quietly never fires
//! reports a false all-clear ([ADR-14]).
//!
//! [FR-WS-01]: ../../../docs/specs/requirements/FR-WS-01.md
//! [FR-WS-13]: ../../../docs/specs/requirements/FR-WS-13.md
//! [FR-GV-01]: ../../../docs/specs/requirements/FR-GV-01.md
//! [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
//! [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
//! [ADR-14]: ../../../docs/specs/architecture/decisions/ADR-14.md
//! [ADR-54]: ../../../docs/specs/architecture/decisions/ADR-54.md
//! [ADR-56]: ../../../docs/specs/architecture/decisions/ADR-56.md

use std::collections::{BTreeSet, HashMap};

use anyhow::{Context, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::Serialize;

use super::bridge::{BridgeEdge, BridgeEndpoint};
use super::manifest::Governance;
use super::Federation;

/// Every workspace-rule violation is an error — like the per-repo contract,
/// checked-in workspace policy is binding ([FR-WS-13]).
const SEVERITY_ERROR: &str = "error";

/// The `rule_type` discriminator for a `[[governance.boundaries]]` breach.
const RULE_TYPE_BOUNDARY: &str = "workspace-boundary";

/// The `rule_type` discriminator for a `[[governance.no_cross_service_callers]]`
/// breach.
const RULE_TYPE_NO_CALLERS: &str = "workspace-no-cross-service-callers";

/// One workspace-rule violation, anchored on the **bridge binding** that breached
/// it ([FR-WS-13]).
///
/// Deliberately **not** [`crate::models::quality::Violation`]: that type is what
/// the per-repo `violations` table stores and the gate scores. Keeping the
/// workspace family in its own type is what makes "a workspace rule can never
/// move a member's gated signal" a compile-time fact rather than a promise
/// ([ADR-56]). It carries no `node_id` and no `file` for the same reason a
/// [`BridgeEndpoint`] carries no `NodeId` — neither is portable across member
/// databases ([ADR-52]).
///
/// [FR-WS-13]: ../../../docs/specs/requirements/FR-WS-13.md
/// [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
/// [ADR-56]: ../../../docs/specs/architecture/decisions/ADR-56.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkspaceViolation {
    /// The rule key: `workspace-boundary:<from>-><to>` or
    /// `no-cross-service-callers:<symbol-glob>`.
    pub rule: String,
    /// [`RULE_TYPE_BOUNDARY`] or [`RULE_TYPE_NO_CALLERS`] — the family
    /// discriminator, mirroring the per-repo `rule_type`.
    pub rule_type: String,
    /// `"error"` — workspace policy is binding.
    pub severity: String,
    /// The relation class of the breaching binding (e.g. `"route"`), carried from
    /// the [`BridgeEdge`] so a reader can see *how* the two services are coupled.
    pub relation: String,
    /// The consumer endpoint of the breaching binding — the calling side.
    pub from: BridgeEndpoint,
    /// The provider endpoint of the breaching binding — the called side.
    pub to: BridgeEndpoint,
    /// The human-facing explanation, carrying the rule's `reason` when declared.
    pub message: String,
}

/// The workspace governance report ([FR-WS-13]) — produced **only** when the
/// workspace declares at least one rule.
///
/// [`bindings_checked`](Self::bindings_checked) is the honesty rider: it states
/// how many bridge matches the rules were quantified over, so a clean report over
/// *zero* bindings is legible as "nothing was bound to check" rather than
/// "everything is fine" ([NFR-CC-04], [ADR-53]).
///
/// [FR-WS-13]: ../../../docs/specs/requirements/FR-WS-13.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
/// [ADR-53]: ../../../docs/specs/architecture/decisions/ADR-53.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkspaceGovernance {
    /// The workspace name (`[workspace] name`).
    pub workspace: String,
    /// How many rules were evaluated (boundaries + no-cross-service-callers).
    pub rules_checked: usize,
    /// How many bridge bindings the rules quantified over — the coverage rider.
    pub bindings_checked: usize,
    /// The violations found, in deterministic order ([NFR-RA-06]).
    ///
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    pub violations: Vec<WorkspaceViolation>,
}

impl WorkspaceGovernance {
    /// Whether the workspace rules all hold. Reported, **never gated** — the
    /// caller decides what to do with it, and no member's signal moves either way
    /// ([ADR-56]).
    ///
    /// [ADR-56]: ../../../docs/specs/architecture/decisions/ADR-56.md
    pub fn passed(&self) -> bool {
        self.violations.is_empty()
    }
}

/// One `[[governance.no_cross_service_callers]]` rule with its symbol glob
/// compiled once — the compiled-matcher-per-declaration pattern the per-repo
/// engine uses for every glob family ([FR-WS-13], [FR-GV-01]).
struct CompiledNoCallers {
    /// The original glob string, retained for the violation's `rule` key.
    symbol_glob: String,
    /// The compiled matcher over the provider endpoint's canonical symbol.
    symbol: GlobSet,
    /// Optional member scope — only providers owned by this member match.
    member: Option<String>,
    reason: Option<String>,
}

/// The compiled workspace rule family: the member→layer index plus one compiled
/// matcher per glob declaration.
struct CompiledWorkspaceRules<'a> {
    /// member name → layer name. Built from `[[governance.service_layers]]` in
    /// declaration order, **first declaration wins** (the tiebreak the per-repo
    /// layer matcher uses for overlapping globs). A member in no layer is absent
    /// here — unlayered, and thus unclassifiable by any boundary rule.
    layer_of: HashMap<&'a str, &'a str>,
    /// The `[[governance.boundaries]]` declarations, in declaration order.
    boundaries: &'a [super::manifest::ServiceBoundary],
    /// The `[[governance.no_cross_service_callers]]` declarations, compiled.
    no_callers: Vec<CompiledNoCallers>,
}

impl<'a> CompiledWorkspaceRules<'a> {
    /// Compile the declared family once ([FR-GV-01]'s "compiled once" discipline).
    ///
    /// # Errors
    /// A symbol glob that fails to compile is a **hard** error: a governance rule
    /// that silently matched nothing would report a false all-clear, which is the
    /// one failure mode a policy tool must never have ([ADR-14], [NFR-CC-04]).
    fn compile(governance: &'a Governance) -> Result<Self> {
        let mut layer_of: HashMap<&str, &str> = HashMap::new();
        for layer in &governance.service_layers {
            for member in &layer.members {
                // First declaration wins — a member named twice keeps its first
                // band, so the assignment is deterministic ([NFR-RA-06]).
                layer_of.entry(member.as_str()).or_insert(&layer.name);
            }
        }

        let no_callers = governance
            .no_cross_service_callers
            .iter()
            .map(|rule| {
                Ok(CompiledNoCallers {
                    symbol_glob: rule.symbol.clone(),
                    symbol: compile_symbol_glob(&rule.symbol).with_context(|| {
                        format!("compiling no_cross_service_callers symbol '{}'", rule.symbol)
                    })?,
                    member: rule.member.clone(),
                    reason: rule.reason.clone(),
                })
            })
            .collect::<Result<Vec<_>>>()?;

        Ok(Self {
            layer_of,
            boundaries: &governance.boundaries,
            no_callers,
        })
    }

    /// How many rules this family declares — layers are vocabulary, not policy,
    /// so they are not counted.
    fn rules_checked(&self) -> usize {
        self.boundaries.len() + self.no_callers.len()
    }
}

/// Compile one symbol glob into a matcher.
///
/// Deliberately **not** [`crate::config::compile_globs`]: that helper additionally
/// runs the path-containment check ([NFR-SE-04]) that keeps a *discovery walk*
/// inside the project root. A symbol glob drives no walk and matches no path — it
/// matches a [`LogosSymbol`](crate::model::LogosSymbol) string — so applying a
/// filesystem-escape check to it would reject legitimate patterns for a threat
/// that does not exist here.
///
/// `globset`'s default semantics apply, so `*` matches across `/` — which is what
/// a symbol glob wants, since a canonical symbol embeds `/`-separated path
/// segments (`… src/routes.ts/legacyOrders().`).
///
/// # Errors
/// A malformed glob — surfaced by the caller as a loud compile failure.
///
/// [NFR-SE-04]: ../../../docs/specs/requirements/NFR-SE-04.md
fn compile_symbol_glob(pattern: &str) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    builder.add(Glob::new(pattern)?);
    Ok(builder.build()?)
}

/// Evaluate the workspace rule family over the bridge's matched bindings
/// ([FR-WS-13], [ADR-56]).
///
/// Returns `Ok(None)` when the workspace declares no rules — the **honest empty**:
/// no report at all, rather than a zero-violation report that would read as a
/// passing check ([NFR-CC-04]).
///
/// `edges` is the [bridge](super::bridge)'s matched edge set. Rules quantify over
/// exactly these — a reference that never bound is not evidence and cannot
/// trigger a rule ([NFR-RA-05]).
///
/// # Errors
/// A malformed rule (an uncompilable symbol glob) fails loud rather than silently
/// never matching ([ADR-14]).
///
/// [FR-WS-13]: ../../../docs/specs/requirements/FR-WS-13.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
/// [ADR-14]: ../../../docs/specs/architecture/decisions/ADR-14.md
/// [ADR-56]: ../../../docs/specs/architecture/decisions/ADR-56.md
pub fn workspace_governance(
    federation: &Federation,
    edges: &[BridgeEdge],
) -> Result<Option<WorkspaceGovernance>> {
    // Honest empty: no declared policy ⇒ no report ([NFR-CC-04]).
    if federation.governance.is_empty() {
        return Ok(None);
    }

    let compiled = CompiledWorkspaceRules::compile(&federation.governance)?;

    let mut violations = check_boundaries(&compiled, edges);
    violations.extend(check_no_cross_service_callers(&compiled, edges));

    Ok(Some(WorkspaceGovernance {
        workspace: federation.name.clone(),
        rules_checked: compiled.rules_checked(),
        bindings_checked: edges.len(),
        violations,
    }))
}

/// `[[governance.boundaries]]`: a bridge binding whose consumer sits in layer
/// `from` and whose provider sits in layer `to` breaches the boundary
/// ([FR-WS-13]).
///
/// Only bindings between **two assigned layers** are checked — an unlayered
/// member is exempt, exactly as the per-repo layer check exempts a file no
/// `[[layers]]` glob claims. Silence about an unclassifiable service is honest;
/// guessing its layer would not be ([NFR-RA-05]).
fn check_boundaries(compiled: &CompiledWorkspaceRules<'_>, edges: &[BridgeEdge]) -> Vec<WorkspaceViolation> {
    let mut violations = Vec::new();
    // One violation per (rule, binding) — two arms coupling the same endpoints
    // (say a route match and a gRPC match) would otherwise double-report the same
    // policy breach against the same pair.
    let mut seen: BTreeSet<(usize, &BridgeEndpoint, &BridgeEndpoint)> = BTreeSet::new();

    for edge in edges {
        let (Some(&from_layer), Some(&to_layer)) = (
            compiled.layer_of.get(edge.from.member.as_str()),
            compiled.layer_of.get(edge.to.member.as_str()),
        ) else {
            continue; // an unlayered member is unclassifiable — never a violation
        };

        for (i, boundary) in compiled.boundaries.iter().enumerate() {
            if boundary.from == from_layer
                && boundary.to == to_layer
                && seen.insert((i, &edge.from, &edge.to))
            {
                violations.push(boundary_violation(edge, boundary, from_layer, to_layer));
            }
        }
    }

    violations
}

/// Render one boundary breach.
fn boundary_violation(
    edge: &BridgeEdge,
    boundary: &super::manifest::ServiceBoundary,
    from_layer: &str,
    to_layer: &str,
) -> WorkspaceViolation {
    let reason = boundary
        .reason
        .as_deref()
        .map(|r| format!(" — {r}"))
        .unwrap_or_default();
    WorkspaceViolation {
        rule: format!("workspace-boundary:{}->{}", boundary.from, boundary.to),
        rule_type: RULE_TYPE_BOUNDARY.to_string(),
        severity: SEVERITY_ERROR.to_string(),
        relation: edge.relation.clone(),
        from: edge.from.clone(),
        to: edge.to.clone(),
        message: format!(
            "`{}` (layer `{from_layer}`) calls `{}` (layer `{to_layer}`) over a \
             `{}` binding, a forbidden service boundary{reason}",
            edge.from.member, edge.to.member, edge.relation
        ),
    }
}

/// `[[governance.no_cross_service_callers]]`: any bridge binding whose **provider**
/// endpoint matches the rule is itself the forbidden caller ([FR-WS-13]).
///
/// This reads the bridge rather than a synthesised caller set — the AC's
/// "reads the bridge, not a fabricated edge set" ([NFR-RA-05]). Each matching
/// edge is reported, so a deprecated endpoint with three cross-service callers
/// yields three violations naming all three.
fn check_no_cross_service_callers(
    compiled: &CompiledWorkspaceRules<'_>,
    edges: &[BridgeEdge],
) -> Vec<WorkspaceViolation> {
    let mut violations = Vec::new();

    for edge in edges {
        for rule in &compiled.no_callers {
            let member_scoped = rule
                .member
                .as_deref()
                .is_none_or(|member| member == edge.to.member);
            if member_scoped && rule.symbol.is_match(edge.to.symbol.as_str()) {
                violations.push(no_callers_violation(edge, rule));
            }
        }
    }

    violations
}

/// Render one no-cross-service-callers breach.
fn no_callers_violation(edge: &BridgeEdge, rule: &CompiledNoCallers) -> WorkspaceViolation {
    let reason = rule
        .reason
        .as_deref()
        .map(|r| format!(" — {r}"))
        .unwrap_or_default();
    WorkspaceViolation {
        rule: format!("no-cross-service-callers:{}", rule.symbol_glob),
        rule_type: RULE_TYPE_NO_CALLERS.to_string(),
        severity: SEVERITY_ERROR.to_string(),
        relation: edge.relation.clone(),
        from: edge.from.clone(),
        to: edge.to.clone(),
        message: format!(
            "`{}` in `{}` must have no cross-service callers, but `{}` in `{}` \
             calls it over a `{}` binding{reason}",
            edge.to.symbol.as_str(),
            edge.to.member,
            edge.from.symbol.as_str(),
            edge.from.member,
            edge.relation
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::federation::manifest::{NoCrossServiceCallers, ServiceBoundary, ServiceLayer};
    use crate::model::LogosSymbol;

    /// A bridge endpoint. `local <name>` is the smallest valid SCIP symbol the
    /// federation fixtures use — a bare name is not a parseable symbol.
    fn endpoint(member: &str, symbol: &str) -> BridgeEndpoint {
        BridgeEndpoint {
            member: member.to_string(),
            symbol: LogosSymbol::parse(&format!("local {symbol}")).unwrap(),
        }
    }

    /// A matched bridge binding: `from` (consumer) → `to` (provider).
    fn edge(from_member: &str, from_sym: &str, to_member: &str, to_sym: &str) -> BridgeEdge {
        BridgeEdge {
            relation: "route".to_string(),
            from: endpoint(from_member, from_sym),
            to: endpoint(to_member, to_sym),
        }
    }

    fn layer(name: &str, members: &[&str]) -> ServiceLayer {
        ServiceLayer {
            name: name.to_string(),
            members: members.iter().map(|m| (*m).to_string()).collect(),
        }
    }

    fn boundary(from: &str, to: &str, reason: Option<&str>) -> ServiceBoundary {
        ServiceBoundary {
            from: from.to_string(),
            to: to.to_string(),
            reason: reason.map(str::to_string),
        }
    }

    /// A federation carrying `governance`, with no members resolved (the rules
    /// quantify over the bridge edges passed in, not over the member set).
    fn federation(governance: Governance) -> Federation {
        Federation {
            name: "shop".to_string(),
            root: "/ws".into(),
            members: Vec::new(),
            default: None,
            links: Vec::new(),
            governance,
        }
    }

    /// The canonical workspace: `web` is `edge`, `api`/`orders` are `core`, and
    /// `edge` → `core` calls are forbidden.
    fn layered(reason: Option<&str>) -> Governance {
        Governance {
            service_layers: vec![layer("edge", &["web"]), layer("core", &["api", "orders"])],
            boundaries: vec![boundary("edge", "core", reason)],
            no_cross_service_callers: Vec::new(),
        }
    }

    fn run(governance: Governance, edges: &[BridgeEdge]) -> Option<WorkspaceGovernance> {
        workspace_governance(&federation(governance), edges).expect("rules compile")
    }

    // ── AC 1: a layer rule evaluates over bridge matches and reports ──────

    /// An `edge`→`core` bridge binding breaches the declared boundary, and the
    /// declared `reason` is surfaced in the message ([FR-WS-13]).
    #[test]
    fn an_edge_to_core_binding_violates_the_declared_boundary() {
        let edges = [edge("web", "fetchOrder", "api", "getOrder")];
        let report = run(layered(Some("edge must go through the gateway")), &edges)
            .expect("declared rules produce a report");

        assert_eq!(report.violations.len(), 1);
        let violation = &report.violations[0];
        assert_eq!(violation.rule, "workspace-boundary:edge->core");
        assert_eq!(violation.rule_type, "workspace-boundary");
        assert_eq!(violation.severity, "error");
        assert_eq!(violation.from.member, "web");
        assert_eq!(violation.to.member, "api");
        assert!(
            violation.message.contains("edge must go through the gateway"),
            "the declared reason is surfaced: {}",
            violation.message
        );
        // The honesty rider: the report states what it quantified over.
        assert_eq!(report.rules_checked, 1);
        assert_eq!(report.bindings_checked, 1);
    }

    /// The boundary is **directional**: `core`→`edge` is not the declared rule,
    /// so the same two members coupled the other way is clean.
    #[test]
    fn the_boundary_is_directional() {
        let edges = [edge("api", "notifyUi", "web", "handleEvent")];
        let report = run(layered(None), &edges).expect("a report");
        assert!(
            report.passed(),
            "core→edge is not the forbidden direction: {:?}",
            report.violations
        );
        assert_eq!(report.bindings_checked, 1, "the binding was still checked");
    }

    /// A binding whose member sits in **no** declared layer is unclassifiable, so
    /// it can never violate a boundary — never fabricated ([NFR-RA-05]).
    #[test]
    fn an_unlayered_member_is_never_a_violation() {
        // `legacy` is in no service_layer.
        let edges = [
            edge("legacy", "call", "api", "getOrder"),
            edge("web", "call", "legacy", "handler"),
        ];
        let report = run(layered(None), &edges).expect("a report");
        assert!(
            report.passed(),
            "an unlayered member is exempt, not guessed: {:?}",
            report.violations
        );
    }

    /// A member named by two layers takes the **first** declaration — the
    /// deterministic first-wins tiebreak ([NFR-RA-06]).
    #[test]
    fn a_member_in_two_layers_keeps_its_first_band() {
        let governance = Governance {
            // `web` is claimed by `edge` first, then by `core`.
            service_layers: vec![layer("edge", &["web"]), layer("core", &["web", "api"])],
            boundaries: vec![boundary("edge", "core", None)],
            no_cross_service_callers: Vec::new(),
        };
        let edges = [edge("web", "fetchOrder", "api", "getOrder")];
        let report = run(governance, &edges).expect("a report");
        assert_eq!(
            report.violations.len(),
            1,
            "web stays in `edge` (first declaration), so edge→core still fires",
        );
    }

    /// Rules quantify over the bridge's **matched** edges: with no bindings, a
    /// declared rule reports zero violations over zero bindings — legible as
    /// "nothing was bound to check", not "everything is fine" ([NFR-CC-04]).
    #[test]
    fn declared_rules_over_no_bindings_report_zero_of_zero() {
        let report = run(layered(None), &[]).expect("declared rules still report");
        assert!(report.passed());
        assert_eq!(report.rules_checked, 1);
        assert_eq!(
            report.bindings_checked, 0,
            "the rider states the rules quantified over nothing",
        );
    }

    // ── AC 3: no-cross-service-callers reads the bridge ───────────────────

    /// A deprecated provider with a cross-service caller violates its rule, and
    /// the violation names the *actual* caller read off the bridge ([FR-WS-13]).
    #[test]
    fn a_deprecated_provider_with_a_cross_service_caller_violates() {
        let governance = Governance {
            service_layers: Vec::new(),
            boundaries: Vec::new(),
            no_cross_service_callers: vec![NoCrossServiceCallers {
                symbol: "*LegacyOrders*".to_string(),
                member: None,
                reason: Some("deprecated in v3".to_string()),
            }],
        };
        let edges = [
            edge("web", "fetchOrder", "api", "getLegacyOrdersV1"),
            edge("web", "fetchUser", "api", "getUser"), // untouched by the glob
        ];
        let report = run(governance, &edges).expect("a report");

        assert_eq!(report.violations.len(), 1, "only the matching provider fires");
        let violation = &report.violations[0];
        assert_eq!(violation.rule, "no-cross-service-callers:*LegacyOrders*");
        assert_eq!(violation.rule_type, "workspace-no-cross-service-callers");
        // The caller is read off the bridge edge, not synthesised.
        assert_eq!(violation.from.member, "web");
        assert!(
            violation.message.contains("fetchOrder") && violation.message.contains("deprecated in v3"),
            "the real caller and the reason are surfaced: {}",
            violation.message
        );
    }

    /// Every cross-service caller of a deprecated provider is reported — three
    /// callers, three violations, so the migration list is complete.
    #[test]
    fn every_cross_service_caller_of_a_deprecated_provider_is_named() {
        let governance = Governance {
            service_layers: Vec::new(),
            boundaries: Vec::new(),
            no_cross_service_callers: vec![NoCrossServiceCallers {
                symbol: "*legacy*".to_string(),
                member: None,
                reason: None,
            }],
        };
        let edges = [
            edge("web", "a", "api", "legacyHandler"),
            edge("admin", "b", "api", "legacyHandler"),
            edge("jobs", "c", "api", "legacyHandler"),
        ];
        let report = run(governance, &edges).expect("a report");
        let callers: Vec<&str> = report
            .violations
            .iter()
            .map(|v| v.from.member.as_str())
            .collect();
        assert_eq!(callers, ["web", "admin", "jobs"]);
    }

    /// The optional `member` scope narrows the rule to providers owned by that
    /// member — the same symbol in another member is untouched.
    #[test]
    fn the_member_scope_narrows_the_rule_to_one_provider_member() {
        let governance = Governance {
            service_layers: Vec::new(),
            boundaries: Vec::new(),
            no_cross_service_callers: vec![NoCrossServiceCallers {
                symbol: "*handler*".to_string(),
                member: Some("api".to_string()),
                reason: None,
            }],
        };
        let edges = [
            edge("web", "a", "api", "handler"),      // in scope
            edge("web", "b", "orders", "handler"),   // same symbol, other member
        ];
        let report = run(governance, &edges).expect("a report");
        assert_eq!(report.violations.len(), 1);
        assert_eq!(report.violations[0].to.member, "api");
    }

    // ── AC 4: honest empty ────────────────────────────────────────────────

    /// With **no** workspace rules declared, there is NO report — not a
    /// zero-violation one. An undeclared policy must never read as a passing
    /// check ([NFR-CC-04]).
    #[test]
    fn no_declared_rules_produce_no_report_at_all() {
        let edges = [edge("web", "fetchOrder", "api", "getOrder")];
        let report = workspace_governance(&federation(Governance::default()), &edges)
            .expect("an empty family is not an error");
        assert!(
            report.is_none(),
            "no rules ⇒ no governance output whatsoever (honest empty)",
        );
    }

    /// Service layers **alone** are vocabulary, not policy: naming bands without
    /// forbidding anything still produces no report.
    #[test]
    fn service_layers_without_a_rule_are_still_an_honest_empty() {
        let governance = Governance {
            service_layers: vec![layer("edge", &["web"]), layer("core", &["api"])],
            boundaries: Vec::new(),
            no_cross_service_callers: Vec::new(),
        };
        let edges = [edge("web", "fetchOrder", "api", "getOrder")];
        assert!(
            workspace_governance(&federation(governance), &edges)
                .expect("not an error")
                .is_none(),
            "layers declare vocabulary, not a contract to check",
        );
    }

    // ── fail-loud on a malformed rule ─────────────────────────────────────

    /// A symbol glob that does not compile fails **loud**. A governance rule that
    /// silently matched nothing would report a false all-clear — the one failure
    /// mode a policy tool must never have ([ADR-14]).
    #[test]
    fn a_malformed_symbol_glob_fails_loud_rather_than_matching_nothing() {
        let governance = Governance {
            service_layers: Vec::new(),
            boundaries: Vec::new(),
            no_cross_service_callers: vec![NoCrossServiceCallers {
                symbol: "[unclosed".to_string(),
                member: None,
                reason: None,
            }],
        };
        let err = workspace_governance(&federation(governance), &[])
            .expect_err("an uncompilable glob is a hard error");
        assert!(
            format!("{err:#}").contains("[unclosed"),
            "the error names the offending pattern: {err:#}",
        );
    }

    // ── the report is deterministic ───────────────────────────────────────

    /// Two arms coupling the SAME endpoint pair report the boundary once — a
    /// single policy breach between one consumer and one provider shows once.
    #[test]
    fn one_violation_per_rule_and_binding_pair() {
        let mut grpc = edge("web", "fetchOrder", "api", "getOrder");
        grpc.relation = "grpc".to_string();
        let edges = [edge("web", "fetchOrder", "api", "getOrder"), grpc];

        let report = run(layered(None), &edges).expect("a report");
        assert_eq!(
            report.violations.len(),
            1,
            "the same consumer→provider pair breaches the boundary once",
        );
        assert_eq!(report.bindings_checked, 2, "both bindings were still checked");
    }
}
