//! Behaviour tests for the scope-hierarchy binder (S-011), organised by
//! acceptance criterion. Pure in-memory: a synthetic snapshot (no SQLite, no
//! tree-sitter) exercises every level of the hierarchy and the
//! never-fabricate rule.
//!
//! The fixture models a two-crate workspace:
//!
//! ```text
//! crate `crate`                     crate `other`
//! ├── src/lib.rs    (module 1)      └── other/src/lib.rs (module 20)
//! │   ├── fn alpha  (2)                 └── fn beta      (21)
//! │   ├── fn helper (3)
//! │   ├── fn dup    (8) ┐  same-name siblings (ordinal-
//! │   ├── fn dup    (9) ┘  disambiguated symbols)
//! │   └── mod inner (6)
//! │       └── fn deep (7)
//! └── src/util.rs   (module 4)
//!     └── fn run    (5)
//! ```

use super::binder::{bind, Index, Outcome};
use crate::config::BindingPolicy;
use crate::graph_store::{EdgeRow, NodeRow, UnresolvedRefRow};
use crate::model::{ArtifactRelation, EdgeKind, LogosSymbol, NodeId, NodeKind, RefForm};

/// File ids for the ledger rows.
const LIB_RS: i64 = 10;
const UTIL_RS: i64 = 11;
const OTHER_LIB_RS: i64 = 12;

fn node(id: i64, name: &str, kind: NodeKind, file: &str) -> NodeRow {
    NodeRow {
        id: NodeId(id),
        symbol: LogosSymbol::parse(&format!("local sym{id}")).unwrap(),
        kind,
        name: name.to_string(),
        file_path: Some(file.to_string()),
        start_line: None,
        end_line: None,
    }
}

fn contains(source: i64, target: i64) -> EdgeRow {
    EdgeRow {
        source: NodeId(source),
        target: NodeId(target),
        kind: EdgeKind::Contains,
    }
}

/// The standard fixture: nodes + Contains edges of the two-crate workspace.
fn fixture() -> (Vec<NodeRow>, Vec<EdgeRow>) {
    let nodes = vec![
        node(1, "crate", NodeKind::Module, "src/lib.rs"),
        node(2, "alpha", NodeKind::Function, "src/lib.rs"),
        node(3, "helper", NodeKind::Function, "src/lib.rs"),
        node(4, "util", NodeKind::Module, "src/util.rs"),
        node(5, "run", NodeKind::Function, "src/util.rs"),
        node(6, "inner", NodeKind::Module, "src/lib.rs"),
        node(7, "deep", NodeKind::Function, "src/lib.rs"),
        node(8, "dup", NodeKind::Function, "src/lib.rs"),
        node(9, "dup", NodeKind::Function, "src/lib.rs"),
        node(20, "other", NodeKind::Module, "other/src/lib.rs"),
        node(21, "beta", NodeKind::Function, "other/src/lib.rs"),
    ];
    let edges = vec![
        contains(1, 2),
        contains(1, 3),
        contains(1, 6),
        contains(1, 8),
        contains(1, 9),
        contains(6, 7),
        contains(4, 5),
        contains(20, 21),
    ];
    (nodes, edges)
}

/// A ledger row with the given shape, sourced from `source_sym`'s node.
fn make_ref(
    id: i64,
    file_id: i64,
    source_node: i64,
    target: &str,
    alias: Option<&str>,
    form: RefForm,
    kind: EdgeKind,
) -> UnresolvedRefRow {
    UnresolvedRefRow {
        id,
        file_id: Some(file_id),
        source_symbol: format!("local sym{source_node}"),
        target: target.to_string(),
        alias: alias.map(str::to_string),
        form,
        kind,
        line: Some(1),
        resolved: false,
        payload: None,
    }
}

fn call(id: i64, file_id: i64, source_node: i64, target: &str) -> UnresolvedRefRow {
    make_ref(
        id,
        file_id,
        source_node,
        target,
        None,
        RefForm::Path,
        EdgeKind::Calls,
    )
}

/// Bind `r` against the fixture (plus extra `refs` providing scope facts).
fn bind_with(refs: &[UnresolvedRefRow], r: &UnresolvedRefRow, policy: BindingPolicy) -> Outcome {
    let (nodes, edges) = fixture();
    let mut all = refs.to_vec();
    all.push(r.clone());
    let ix = Index::build(&nodes, &edges, &all);
    bind(r, &ix, policy)
}

fn bound_to(outcome: Outcome, source: i64, target: i64, kind: EdgeKind) {
    assert_eq!(
        outcome,
        Outcome::Bound {
            source: NodeId(source),
            target: NodeId(target),
            kind,
            payload: None,
        }
    );
}

// ── Level 1-2: lexical / module scope ([FR-RS-03]) ──────────────────────────

#[test]
fn bare_name_binds_at_module_scope() {
    // alpha() calls helper(): both top-level in lib.rs — the Contains walk
    // reaches the file module and finds exactly one `helper`.
    let r = call(100, LIB_RS, 2, "helper");
    bound_to(
        bind_with(&[], &r, BindingPolicy::Strict),
        2,
        3,
        EdgeKind::Calls,
    );
}

#[test]
fn bare_name_from_an_inline_module_walks_outward_to_the_file_scope() {
    // deep() (inside `mod inner`) calls helper(): inner has no helper, the
    // walk continues to the file module.
    let r = call(100, LIB_RS, 7, "helper");
    bound_to(
        bind_with(&[], &r, BindingPolicy::Strict),
        7,
        3,
        EdgeKind::Calls,
    );
}

#[test]
fn sibling_file_module_resolves_through_the_path_derived_tree() {
    // alpha() calls util::run(): `util` is a sibling *file*, reachable only
    // through the module tree (no cross-file Contains edges exist).
    let r = call(100, LIB_RS, 2, "util::run");
    bound_to(
        bind_with(&[], &r, BindingPolicy::Strict),
        2,
        5,
        EdgeKind::Calls,
    );
}

// ── Level 3: imports — aliases and globs ([FR-RS-01], [FR-RS-02]) ───────────

#[test]
fn use_alias_resolves_a_bare_call() {
    // `use crate::util::run;` then `run()`: binds through the alias map.
    let import = make_ref(
        50,
        LIB_RS,
        1,
        "crate::util::run",
        Some("run"),
        RefForm::Path,
        EdgeKind::Imports,
    );
    let r = call(100, LIB_RS, 2, "run");
    bound_to(
        bind_with(&[import], &r, BindingPolicy::Strict),
        2,
        5,
        EdgeKind::Calls,
    );
}

#[test]
fn use_as_rename_resolves_the_renamed_head() {
    // `use crate::util as u;` then `u::run()`.
    let import = make_ref(
        50,
        LIB_RS,
        1,
        "crate::util",
        Some("u"),
        RefForm::Path,
        EdgeKind::Imports,
    );
    let r = call(100, LIB_RS, 2, "u::run");
    bound_to(
        bind_with(&[import], &r, BindingPolicy::Strict),
        2,
        5,
        EdgeKind::Calls,
    );
}

#[test]
fn glob_import_resolves_a_bare_call() {
    // `use crate::util::*;` then `run()`.
    let glob = make_ref(
        50,
        LIB_RS,
        1,
        "crate::util",
        None,
        RefForm::Glob,
        EdgeKind::Imports,
    );
    let r = call(100, LIB_RS, 2, "run");
    bound_to(
        bind_with(&[glob], &r, BindingPolicy::Strict),
        2,
        5,
        EdgeKind::Calls,
    );
}

#[test]
fn import_of_a_module_binds_to_the_module_node() {
    // FR-RS-01 acceptance: `use crate::util;` → Imports edge to module node.
    let r = make_ref(
        100,
        LIB_RS,
        1,
        "crate::util",
        Some("util"),
        RefForm::Path,
        EdgeKind::Imports,
    );
    bound_to(
        bind_with(&[], &r, BindingPolicy::Strict),
        1,
        4,
        EdgeKind::Imports,
    );
}

#[test]
fn glob_ref_itself_binds_to_the_globbed_module() {
    let r = make_ref(
        100,
        LIB_RS,
        1,
        "crate::util",
        None,
        RefForm::Glob,
        EdgeKind::Imports,
    );
    bound_to(
        bind_with(&[], &r, BindingPolicy::Strict),
        1,
        4,
        EdgeKind::Imports,
    );
}

// ── Level 4: crate paths — crate:: / self:: / super:: ───────────────────────

#[test]
fn crate_self_and_super_heads_resolve() {
    let crate_path = call(100, LIB_RS, 2, "crate::util::run");
    bound_to(
        bind_with(&[], &crate_path, BindingPolicy::Strict),
        2,
        5,
        EdgeKind::Calls,
    );

    // deep() is in module ["inner"]: super::helper → file scope helper.
    let super_path = call(101, LIB_RS, 7, "super::helper");
    bound_to(
        bind_with(&[], &super_path, BindingPolicy::Strict),
        7,
        3,
        EdgeKind::Calls,
    );

    // self::inner::deep from alpha (module []).
    let self_path = call(102, LIB_RS, 2, "self::inner::deep");
    bound_to(
        bind_with(&[], &self_path, BindingPolicy::Strict),
        2,
        7,
        EdgeKind::Calls,
    );
}

#[test]
fn extern_crate_name_head_resolves_across_crates() {
    // beta() in crate `other` is reachable from crate `crate` via its name.
    let r = call(100, LIB_RS, 2, "other::beta");
    bound_to(
        bind_with(&[], &r, BindingPolicy::Strict),
        2,
        21,
        EdgeKind::Calls,
    );
}

// ── Never fabricate ([NFR-RA-05]) ────────────────────────────────────────────

#[test]
fn ambiguous_candidates_stay_unbound_under_every_policy() {
    // Two `dup` functions in the same module: a call to `dup` has two
    // candidates — no policy may pick one.
    for policy in [
        BindingPolicy::Strict,
        BindingPolicy::Balanced,
        BindingPolicy::Aggressive,
    ] {
        let r = call(100, LIB_RS, 2, "dup");
        assert_eq!(
            bind_with(&[], &r, policy),
            Outcome::Unbound,
            "two candidates must never bind ({policy:?})"
        );
    }
}

#[test]
fn unknown_targets_stay_unbound() {
    // An external (un-indexed) path — `anyhow::Context` — has no candidate:
    // it persists as unresolved, never invented.
    let r = call(100, LIB_RS, 2, "anyhow::bail");
    assert_eq!(
        bind_with(&[], &r, BindingPolicy::Aggressive),
        Outcome::Unbound
    );
}

#[test]
fn missing_source_node_stays_unbound() {
    // A captured ref whose source file was removed: no source node, no edge.
    let mut r = call(100, LIB_RS, 2, "helper");
    r.source_symbol = "local gone".to_string();
    assert_eq!(
        bind_with(&[], &r, BindingPolicy::Balanced),
        Outcome::Unbound
    );
}

#[test]
fn alias_cycle_terminates_as_unbound() {
    // `use b as a; use a as b;` — a self-referential import cycle must
    // terminate (the S-011 sprint verification), not hang.
    let a = make_ref(
        50,
        LIB_RS,
        1,
        "b",
        Some("a"),
        RefForm::Path,
        EdgeKind::Imports,
    );
    let b = make_ref(
        51,
        LIB_RS,
        1,
        "a",
        Some("b"),
        RefForm::Path,
        EdgeKind::Imports,
    );
    let r = call(100, LIB_RS, 2, "a::missing");
    assert_eq!(
        bind_with(&[a, b], &r, BindingPolicy::Balanced),
        Outcome::Unbound
    );
}

// ── Exact-symbol (capture-before-delete) refs ([ADR-10]) ────────────────────

#[test]
fn symbol_form_is_pure_lookup() {
    let hit = make_ref(
        100,
        UTIL_RS,
        2,
        "local sym5",
        None,
        RefForm::Symbol,
        EdgeKind::Calls,
    );
    bound_to(
        bind_with(&[], &hit, BindingPolicy::Strict),
        2,
        5,
        EdgeKind::Calls,
    );

    let miss = make_ref(
        101,
        UTIL_RS,
        2,
        "local symgone",
        None,
        RefForm::Symbol,
        EdgeKind::Calls,
    );
    assert_eq!(
        bind_with(&[], &miss, BindingPolicy::Strict),
        Outcome::Unbound
    );
}

// ── Policy gating: strict / balanced / aggressive ────────────────────────────

#[test]
fn receiver_method_calls_do_not_use_the_workspace_name_fallback() {
    // `x.run()` from `alpha` (crate `crate`, module lib.rs) — `run` (id 5) lives
    // in the *other* module `util`, so it is not in the caller's scope. The old
    // balanced/aggressive workspace name fallback bound it anyway (unique name),
    // fabricating a cross-module `Calls` edge with no receiver-type evidence.
    // Under CR-066/FR-RS-06 the workspace name fallback is gated for the method
    // form, so a cross-scope receiver call stays unresolved at *every* policy
    // tier — never fabricate (NFR-RA-05).
    let r = make_ref(
        100,
        LIB_RS,
        2,
        "run",
        None,
        RefForm::Method,
        EdgeKind::Calls,
    );
    for policy in [
        BindingPolicy::Strict,
        BindingPolicy::Balanced,
        BindingPolicy::Aggressive,
    ] {
        assert_eq!(
            bind_with(&[], &r, policy),
            Outcome::Unbound,
            "a receiver-method call to a cross-scope name must not bind via the \
             workspace fallback at {policy:?} (CR-066, FR-RS-06)"
        );
    }
}

#[test]
fn receiver_method_call_still_binds_on_genuine_scope_evidence() {
    // Recall preservation (CR-066 §3.2 "equivalent scope evidence"): gating only
    // the *workspace name* fallback leaves genuine scope resolution intact. A
    // `self.helper()` call from `alpha` binds to the sibling module-level
    // `helper` (id 3, same module lib.rs) through the lexical/module scope — the
    // common self-/sibling-method case keeps its edge, at every policy tier
    // (it is scope-proven, so even `strict` binds it).
    let r = make_ref(
        100,
        LIB_RS,
        2,
        "helper",
        None,
        RefForm::Method,
        EdgeKind::Calls,
    );
    for policy in [
        BindingPolicy::Strict,
        BindingPolicy::Balanced,
        BindingPolicy::Aggressive,
    ] {
        bound_to(bind_with(&[], &r, policy), 2, 3, EdgeKind::Calls);
    }
}

#[test]
fn typed_and_path_qualified_calls_to_a_method_name_still_bind() {
    // Recall guard (CR-066 §7 / FR-RS-03): gating the *method* form's workspace
    // fallback must not touch path-form calls. The same target name `run`
    // reached as a path-qualified call (`util::run`, RefForm::Path) still binds
    // through the scope hierarchy / suffix fallback — no typed-call regression.
    let path_call = call(101, OTHER_LIB_RS, 21, "util::run");
    bound_to(
        bind_with(&[], &path_call, BindingPolicy::Balanced),
        21,
        5,
        EdgeKind::Calls,
    );
    // A bare-name *path* call (RefForm::Path, not Method) to a unique name also
    // still binds at aggressive — the workspace gate is method-form-only.
    let bare_path_call = call(102, OTHER_LIB_RS, 21, "run");
    bound_to(
        bind_with(&[], &bare_path_call, BindingPolicy::Aggressive),
        21,
        5,
        EdgeKind::Calls,
    );
}

#[test]
fn method_form_workspace_gate_is_deterministic_across_repeated_binds() {
    // Determinism (NFR-RA-06): re-binding the same receiver-method ref against
    // the same snapshot yields the identical outcome — on both the gated
    // (cross-scope → Unbound) and the scope-evidence (→ Bound) paths, so a
    // non-determinism on the *bound* branch would be caught too.
    let gated = make_ref(100, LIB_RS, 2, "run", None, RefForm::Method, EdgeKind::Calls);
    let first = bind_with(&[], &gated, BindingPolicy::Balanced);
    let second = bind_with(&[], &gated, BindingPolicy::Balanced);
    assert_eq!(first, Outcome::Unbound);
    assert_eq!(first, second, "the gate is a pure function of the snapshot");

    let bound = make_ref(101, LIB_RS, 2, "helper", None, RefForm::Method, EdgeKind::Calls);
    let b1 = bind_with(&[], &bound, BindingPolicy::Balanced);
    let b2 = bind_with(&[], &bound, BindingPolicy::Balanced);
    bound_to(b1.clone(), 2, 3, EdgeKind::Calls);
    assert_eq!(b1, b2, "a scope-evidence method bind is also deterministic");
}

#[test]
fn method_call_via_alias_does_not_reach_the_workspace_suffix_fallback() {
    // The other half of the gate: an alias-expanded method name must not reach
    // the `resolve_path` step-8 `suffix_match` workspace tier either (CR-066).
    // `beta` (crate `other`) has `use util::run as run;`, then calls `x.run()`.
    // The method name expands via that alias to the multi-segment `["util",
    // "run"]`; scoped resolution can't place it in crate `other`, so *without*
    // the suffix suppression it would suffix-match the lone `run` (id 5, module
    // path ending in `[util]`) at balanced/aggressive. The method-form gate must
    // keep it Unbound — pinning the step-8 guard, not just the step-5 one.
    let import = make_ref(
        50,
        OTHER_LIB_RS,
        20,
        "util::run",
        Some("run"),
        RefForm::Path,
        EdgeKind::Imports,
    );
    let r = make_ref(
        100,
        OTHER_LIB_RS,
        21,
        "run",
        None,
        RefForm::Method,
        EdgeKind::Calls,
    );
    // A control: as a *path* call the same alias legitimately suffix-binds at
    // balanced (the fallback is method-form-only), proving the fixture actually
    // reaches step 8 — so the Unbound above is the gate, not an unrelated miss.
    let path_probe = call(101, OTHER_LIB_RS, 21, "run");
    bound_to(
        bind_with(std::slice::from_ref(&import), &path_probe, BindingPolicy::Balanced),
        21,
        5,
        EdgeKind::Calls,
    );
    for policy in [BindingPolicy::Balanced, BindingPolicy::Aggressive] {
        assert_eq!(
            bind_with(std::slice::from_ref(&import), &r, policy),
            Outcome::Unbound,
            "a receiver-method call must not reach the suffix fallback via an \
             alias at {policy:?} (CR-066 step-8 gate)"
        );
    }
}

#[test]
fn ambiguous_method_name_in_scope_stays_unbound() {
    // `dup` is defined twice in the caller's own module (ids 8, 9): the scope
    // lookup finds a *known* ambiguity, which never resolves to a pick — the
    // single-candidate acceptance rule holds for the method form too
    // (NFR-RA-05), independent of the workspace gate.
    let r = make_ref(
        100,
        LIB_RS,
        2,
        "dup",
        None,
        RefForm::Method,
        EdgeKind::Calls,
    );
    for policy in [
        BindingPolicy::Strict,
        BindingPolicy::Balanced,
        BindingPolicy::Aggressive,
    ] {
        assert_eq!(bind_with(&[], &r, policy), Outcome::Unbound);
    }
}

#[test]
fn suffix_fallback_binds_cross_crate_paths_at_balanced_only() {
    // beta() (crate `other`) calls util::run() with no import: scoped
    // attempts fail (util is not other's module), the workspace suffix
    // match finds exactly one `run` under a module path ending in [util].
    let r = call(100, OTHER_LIB_RS, 21, "util::run");
    assert_eq!(
        bind_with(&[], &r, BindingPolicy::Strict),
        Outcome::Unbound,
        "strict has no workspace fallback"
    );
    bound_to(
        bind_with(&[], &r, BindingPolicy::Balanced),
        21,
        5,
        EdgeKind::Calls,
    );
}

#[test]
fn bare_name_workspace_fallback_is_aggressive_only() {
    // beta() calls helper() with no import: nothing scoped matches in crate
    // `other`; only aggressive may use the workspace-unique name.
    let r = call(100, OTHER_LIB_RS, 21, "helper");
    assert_eq!(bind_with(&[], &r, BindingPolicy::Strict), Outcome::Unbound);
    assert_eq!(
        bind_with(&[], &r, BindingPolicy::Balanced),
        Outcome::Unbound
    );
    bound_to(
        bind_with(&[], &r, BindingPolicy::Aggressive),
        21,
        3,
        EdgeKind::Calls,
    );
}

#[test]
fn crate_local_candidate_wins_over_a_cross_crate_one() {
    // Both crates declare `local_fn`; an aggressive bare-name *path* bind from
    // crate `crate` must pick the crate-local one (crate before workspace,
    // FR-RS-03 hierarchy), not report ambiguity. The crate-local `local_fn`
    // lives in a *different* module (`util`) than the caller, so the lexical
    // walk cannot see it — the bind is decided by the workspace fallback's
    // `prefer_crate` tie-break, which is exactly what this pins.
    //
    // A `RefForm::Path` bare name is used deliberately: the receiver-method
    // form no longer reaches this fallback at all (CR-066).
    let (mut nodes, mut edges) = fixture();
    nodes.push(node(30, "local_fn", NodeKind::Function, "src/util.rs"));
    nodes.push(node(31, "local_fn", NodeKind::Function, "other/src/lib.rs"));
    edges.push(contains(4, 30)); // crate `crate`, module `util` (not the caller's)
    edges.push(contains(20, 31)); // crate `other`
    let r = call(100, LIB_RS, 2, "local_fn"); // from `alpha` (crate `crate`)
    let all = vec![r.clone()];
    let ix = Index::build(&nodes, &edges, &all);
    assert_eq!(
        bind(&r, &ix, BindingPolicy::Aggressive),
        Outcome::Bound {
            source: NodeId(2),
            target: NodeId(30),
            kind: EdgeKind::Calls,
            payload: None,
        },
        "the source crate's candidate wins (crate → workspace order)"
    );
}

// ── The Type::func impl-collapse rule ────────────────────────────────────────

#[test]
fn associated_function_paths_bind_through_the_type_collapse_rule() {
    // struct Widget + fn new in the same module (impl blocks are not
    // captured scopes): `Widget::new()` binds to the module's `new`.
    let (mut nodes, mut edges) = fixture();
    nodes.push(node(40, "Widget", NodeKind::Struct, "src/util.rs"));
    nodes.push(node(41, "new", NodeKind::Function, "src/util.rs"));
    edges.push(contains(4, 40));
    edges.push(contains(4, 41));
    let r = call(100, LIB_RS, 2, "util::Widget::new");
    let all = vec![r.clone()];
    let ix = Index::build(&nodes, &edges, &all);
    assert_eq!(
        bind(&r, &ix, BindingPolicy::Strict),
        Outcome::Bound {
            source: NodeId(2),
            target: NodeId(41),
            kind: EdgeKind::Calls,
            payload: None,
        }
    );
}

// ── Review round 2: ambiguity across globs, super overflow, policy matrix ────

#[test]
fn two_globs_with_the_same_exported_name_stay_unbound() {
    // `use crate::util::*; use other::*;` where both modules export `shared`:
    // the cross-glob candidate set has two members — Ambiguous, never a pick.
    let (mut nodes, mut edges) = fixture();
    nodes.push(node(50, "shared", NodeKind::Function, "src/util.rs"));
    nodes.push(node(51, "shared", NodeKind::Function, "other/src/lib.rs"));
    edges.push(contains(4, 50));
    edges.push(contains(20, 51));

    let glob_util = make_ref(
        60,
        LIB_RS,
        1,
        "crate::util",
        None,
        RefForm::Glob,
        EdgeKind::Imports,
    );
    let glob_other = make_ref(
        61,
        LIB_RS,
        1,
        "other",
        None,
        RefForm::Glob,
        EdgeKind::Imports,
    );
    let r = call(100, LIB_RS, 2, "shared");

    let all = vec![glob_util, glob_other, r.clone()];
    let ix = Index::build(&nodes, &edges, &all);
    assert_eq!(
        bind(&r, &ix, BindingPolicy::Strict),
        Outcome::Unbound,
        "two globs exporting the same name must stay unbound (NFR-RA-05)"
    );
}

#[test]
fn super_chain_overflow_stays_unbound() {
    // deep() lives in mod inner (module path ["inner"], depth 1): a
    // `super::super::…` path asks for more ancestors than exist — the guard
    // must return Unbound, never slice-panic.
    let r = call(100, LIB_RS, 7, "super::super::alpha");
    assert_eq!(
        bind_with(&[], &r, BindingPolicy::Strict),
        Outcome::Unbound,
        "more supers than module depth must be Unbound, not a panic"
    );
}

#[test]
fn one_local_fn_map_gets_no_fabricated_fan_in_from_dot_map_calls() {
    // UAT-RS-04 / CR-066: a project with a single local `fn map` and many
    // `.map()` receiver-call sites in *other* modules must not collect a
    // fabricated `Calls` edge into that `fn map` — the exact dogfood pathology
    // (`map` absorbed 640 spurious edges, all via the cross-module workspace
    // name fallback). Each `.map()` is a `RefForm::Method` whose name is not in
    // its caller's scope, so with the workspace name fallback gated it stays
    // unresolved at every policy; meanwhile a path-qualified call still resolves.
    let (mut nodes, mut edges) = fixture();
    nodes.push(node(30, "map", NodeKind::Function, "src/util.rs"));
    edges.push(contains(4, 30)); // the one local `fn map`, in module `util`

    // Three `.map()` receiver calls from sources in *different* modules than
    // `fn map` (alpha/helper in the crate root, deep in `inner`) — the
    // cross-module shape that fabricated fan-in via the workspace fallback.
    let sites = [(200, 2), (201, 3), (202, 7)];
    for (id, src) in sites {
        let m = make_ref(id, LIB_RS, src, "map", None, RefForm::Method, EdgeKind::Calls);
        let all = vec![m.clone()];
        let ix = Index::build(&nodes, &edges, &all);
        for policy in [
            BindingPolicy::Strict,
            BindingPolicy::Balanced,
            BindingPolicy::Aggressive,
        ] {
            assert_eq!(
                bind(&m, &ix, policy),
                Outcome::Unbound,
                "a `.map()` receiver call must not fabricate an edge to the \
                 lone `fn map` at {policy:?} (UAT-RS-04)"
            );
        }
    }

    // Recall preserved: a path-qualified `util::map()` still binds to it.
    let typed = call(203, LIB_RS, 2, "util::map");
    let all = vec![typed.clone()];
    let ix = Index::build(&nodes, &edges, &all);
    assert_eq!(
        bind(&typed, &ix, BindingPolicy::Balanced),
        Outcome::Bound {
            source: NodeId(2),
            target: NodeId(30),
            kind: EdgeKind::Calls,
            payload: None,
        },
        "a path-qualified call to the same name still resolves (recall guard)"
    );
}

// ── CR-011 / S-068: cross-artifact binding (ArtifactRef / ArtifactBinding) ────
//
// The substrate's resolution clients, dispatched by (kind, form) under the same
// exactly-one-candidate rule as code and docs ([FR-CG-07], [ADR-26],
// [NFR-RA-05]). Sources are config-layer nodes; targets are a sibling
// `ConfigFile` (artifact→artifact path), an artifact node by name
// (artifact→artifact name), or a type-like code symbol (artifact→code name).

const SVC_PROTO: i64 = 30;

fn cfg_node(id: i64, name: &str, kind: NodeKind, file: &str) -> NodeRow {
    node(id, name, kind, file)
}

/// An artifact reference sourced from the `svc.proto` ConfigFile (id 30),
/// carrying its relation class as the payload — exactly as the config
/// extraction walk would emit it.
fn artifact_ref(target: &str, form: RefForm, relation: ArtifactRelation) -> UnresolvedRefRow {
    UnresolvedRefRow {
        id: 200,
        file_id: Some(SVC_PROTO),
        source_symbol: format!("local sym{SVC_PROTO}"),
        target: target.to_string(),
        alias: None,
        form,
        kind: relation.edge_kind(),
        line: Some(1),
        resolved: false,
        payload: Some(relation.as_str().to_string()),
    }
}

/// Bind `r` against an ad-hoc node set (no Contains topology needed — artifact
/// resolution keys off file paths and names, not the scope hierarchy).
fn bind_artifact(nodes: &[NodeRow], r: &UnresolvedRefRow) -> Outcome {
    let ix = Index::build(nodes, &[], std::slice::from_ref(r));
    bind(r, &ix, BindingPolicy::Strict)
}

#[test]
fn artifact_path_ref_binds_to_the_one_config_file_at_the_path() {
    let nodes = vec![
        cfg_node(SVC_PROTO, "svc.proto", NodeKind::ConfigFile, "svc.proto"),
        cfg_node(31, "common.proto", NodeKind::ConfigFile, "common.proto"),
    ];
    let r = artifact_ref("common.proto", RefForm::Path, ArtifactRelation::ProtoImport);
    assert_eq!(
        bind_artifact(&nodes, &r),
        Outcome::Bound {
            source: NodeId(SVC_PROTO),
            target: NodeId(31),
            kind: EdgeKind::ArtifactRef,
            // The relation class is stamped onto the edge (FR-CG-11).
            payload: Some("proto-import".to_string()),
        },
        "a workspace-relative import binds to the sibling ConfigFile"
    );
}

#[test]
fn artifact_path_ref_is_unbound_until_its_target_is_indexed() {
    // The late-bind contract (FR-CG-07, FR-RS-03): with the sibling absent the
    // import stays unbound (and would persist in the ledger for retry); once the
    // target is indexed the same row binds on the next pass — never fabricated.
    let r = artifact_ref("common.proto", RefForm::Path, ArtifactRelation::ProtoImport);

    let before = vec![cfg_node(
        SVC_PROTO,
        "svc.proto",
        NodeKind::ConfigFile,
        "svc.proto",
    )];
    assert_eq!(
        bind_artifact(&before, &r),
        Outcome::Unbound,
        "the import is unresolved while its target is unindexed"
    );

    let after = vec![
        cfg_node(SVC_PROTO, "svc.proto", NodeKind::ConfigFile, "svc.proto"),
        cfg_node(31, "common.proto", NodeKind::ConfigFile, "common.proto"),
    ];
    assert!(
        matches!(bind_artifact(&after, &r), Outcome::Bound { target, .. } if target == NodeId(31)),
        "the same row binds once the target appears (retry-on-sync)"
    );
}

#[test]
fn artifact_path_ref_with_two_config_files_at_the_path_stays_unbound() {
    // Exactly-one-candidate: two ConfigFiles at the same path is ambiguous, so
    // the reference is never bound (NFR-RA-05). (A model-prohibited shape, but the
    // binder must not fabricate a pick.)
    let nodes = vec![
        cfg_node(SVC_PROTO, "svc.proto", NodeKind::ConfigFile, "svc.proto"),
        cfg_node(31, "common.proto", NodeKind::ConfigFile, "common.proto"),
        cfg_node(32, "common.proto", NodeKind::ConfigFile, "common.proto"),
    ];
    let r = artifact_ref("common.proto", RefForm::Path, ArtifactRelation::ProtoImport);
    assert_eq!(bind_artifact(&nodes, &r), Outcome::Unbound);
}

#[test]
fn schema_type_name_binds_to_exactly_one_type_like_code_symbol() {
    // ArtifactBinding + literal name → the one type-like CODE symbol of that name
    // (FR-CG-10): no synthesized candidates, code only.
    let nodes = vec![
        cfg_node(SVC_PROTO, "svc.proto", NodeKind::ConfigFile, "svc.proto"),
        node(40, "UserProfile", NodeKind::Struct, "src/user.rs"),
    ];
    let r = artifact_ref("UserProfile", RefForm::Method, ArtifactRelation::SchemaType);
    assert_eq!(
        bind_artifact(&nodes, &r),
        Outcome::Bound {
            source: NodeId(SVC_PROTO),
            target: NodeId(40),
            kind: EdgeKind::ArtifactBinding,
            payload: Some("type-name".to_string()),
        }
    );
}

#[test]
fn duplicate_type_name_stays_unbound() {
    // A name shared by two type-like symbols is ambiguous — never bound
    // (NFR-RA-05): the coverage count makes the low recall visible, honestly.
    let nodes = vec![
        cfg_node(SVC_PROTO, "svc.proto", NodeKind::ConfigFile, "svc.proto"),
        node(40, "User", NodeKind::Struct, "src/a.rs"),
        node(41, "User", NodeKind::Class, "src/b.rs"),
    ];
    let r = artifact_ref("User", RefForm::Method, ArtifactRelation::SchemaType);
    assert_eq!(bind_artifact(&nodes, &r), Outcome::Unbound);
}

#[test]
fn schema_type_name_never_binds_to_a_non_type_or_config_node() {
    // Only type-like code kinds are candidates: a function of the same name and a
    // config node of the same name are both excluded, so the reference stays
    // unbound rather than mis-binding (FR-CG-10, no synthesized candidates).
    let nodes = vec![
        cfg_node(SVC_PROTO, "svc.proto", NodeKind::ConfigFile, "svc.proto"),
        node(40, "Order", NodeKind::Function, "src/a.rs"),
        cfg_node(41, "Order", NodeKind::ProtoMessage, "svc.proto"),
    ];
    let r = artifact_ref("Order", RefForm::Method, ArtifactRelation::SchemaType);
    assert_eq!(bind_artifact(&nodes, &r), Outcome::Unbound);
}

#[test]
fn artifact_name_ref_binds_to_exactly_one_artifact_node() {
    // ArtifactRef + literal name → the one artifact-layer node of that name
    // (FR-CG-08): a proto/GraphQL type reference resolving within the artifact
    // layer, code symbols excluded.
    let nodes = vec![
        cfg_node(SVC_PROTO, "svc.proto", NodeKind::ConfigFile, "svc.proto"),
        cfg_node(42, "Common", NodeKind::ProtoMessage, "common.proto"),
        // A code symbol of the same name must NOT be a candidate here.
        node(43, "Common", NodeKind::Struct, "src/common.rs"),
    ];
    let r = artifact_ref("Common", RefForm::Method, ArtifactRelation::ProtoType);
    assert_eq!(
        bind_artifact(&nodes, &r),
        Outcome::Bound {
            source: NodeId(SVC_PROTO),
            target: NodeId(42),
            kind: EdgeKind::ArtifactRef,
            payload: Some("proto-type".to_string()),
        }
    );
}

#[test]
fn artifact_name_ref_never_binds_across_formats() {
    // A proto type reference is fenced to `ProtoMessage` by the relation's
    // target kind: a same-named TfBlock is NOT a candidate, so the reference
    // stays unbound rather than cross-binding proto→terraform ([NFR-RA-05],
    // [ADR-26]). The cross-format never-fabricate guard at the relation grain.
    let nodes = vec![
        cfg_node(SVC_PROTO, "svc.proto", NodeKind::ConfigFile, "svc.proto"),
        cfg_node(44, "User", NodeKind::TfBlock, "main.tf"),
    ];
    let r = artifact_ref("User", RefForm::Method, ArtifactRelation::ProtoType);
    assert_eq!(
        bind_artifact(&nodes, &r),
        Outcome::Unbound,
        "a proto type ref must not bind to a same-named TfBlock"
    );
}

// ── CR-011 / S-069: OpenAPI ApiOperation → route binding ─────────────────────
//
// The positional-template + exact-method match (FR-CG-09): an operation rendered
// `"METHOD /template"` binds to the one `route` node whose method and
// positionally-normalized template match. Parameter names and syntax are erased;
// ambiguity, a method mismatch, and a non-normalizing route all stay unresolved —
// never approximately matched ([NFR-RA-05]).

const OPENAPI_YAML_FILE: &str = "openapi.yaml";

/// A `route` node named `"METHOD /path"`, as the framework-promotion pass emits
/// it (S-012). Defined in a code file so it is a code-layer node.
fn route_node(id: i64, name: &str) -> NodeRow {
    node(id, name, NodeKind::Route, "src/main.rs")
}

/// An OpenAPI operation→route reference, sourced from an `ApiOperation` node
/// (id 30), exactly as the config extraction walk encodes it: target
/// `"METHOD /template"`, `ArtifactBinding` + `Path`, relation payload `route`.
fn route_ref(target: &str) -> UnresolvedRefRow {
    artifact_ref(target, RefForm::Path, ArtifactRelation::Route)
}

/// Bind a route reference against the `ApiOperation` source node plus `routes`.
fn bind_route(routes: &[NodeRow], r: &UnresolvedRefRow) -> Outcome {
    let mut nodes = vec![cfg_node(
        SVC_PROTO,
        "get",
        NodeKind::ApiOperation,
        OPENAPI_YAML_FILE,
    )];
    nodes.extend_from_slice(routes);
    bind_artifact(&nodes, r)
}

#[test]
fn operation_binds_to_its_route_across_parameter_name_drift() {
    // The acceptance fixture: the spec writes `{id}`, the route writes
    // `{user_id}` — parameter names are erased, so the operation binds.
    let routes = [route_node(50, "GET /users/{user_id}")];
    let r = route_ref("GET /users/{id}");
    assert_eq!(
        bind_route(&routes, &r),
        Outcome::Bound {
            source: NodeId(SVC_PROTO),
            target: NodeId(50),
            kind: EdgeKind::ArtifactBinding,
            // The relation class is stamped onto the edge for navigation (FR-CG-11).
            payload: Some("route".to_string()),
        },
        "an operation binds to its route despite a parameter-name drift"
    );
}

#[test]
fn operation_binds_across_express_parameter_syntax() {
    // The route uses Express `:id` syntax; the OpenAPI operation uses `{id}` —
    // the shared normalizer aligns the two dialects (the framework matrix).
    let routes = [route_node(50, "GET /users/:id")];
    let r = route_ref("GET /users/{id}");
    assert!(
        matches!(bind_route(&routes, &r), Outcome::Bound { target, .. } if target == NodeId(50)),
        "an axum/OpenAPI `{{id}}` operation binds to an Express `:id` route"
    );
}

#[test]
fn two_routes_sharing_a_normalized_template_and_method_stay_unresolved() {
    // Two routes collapse to the same `(GET, /users/{})` key: ambiguous, so the
    // operation is never bound — surfaced in the ledger, never guessed.
    let routes = [
        route_node(50, "GET /users/{id}"),
        route_node(51, "GET /users/{userId}"),
    ];
    let r = route_ref("GET /users/{id}");
    assert_eq!(
        bind_route(&routes, &r),
        Outcome::Unbound,
        "two routes sharing a normalized template + method leave the operation unresolved"
    );
}

#[test]
fn a_method_mismatch_never_binds() {
    // The only same-template route is a POST; a GET operation must not bind to it.
    let routes = [route_node(50, "POST /users/{id}")];
    let r = route_ref("GET /users/{id}");
    assert_eq!(
        bind_route(&routes, &r),
        Outcome::Unbound,
        "a method mismatch never binds (FR-CG-09)"
    );
}

#[test]
fn a_catch_all_or_regex_route_is_never_a_candidate() {
    // A route whose template does not normalize cleanly (catch-all, regex) is
    // absent from the route index, so the operation stays honestly unresolved
    // rather than approximately matching it.
    for non_normalizing in ["GET /files/{*rest}", "GET /files/{path:path}"] {
        let routes = [route_node(50, non_normalizing)];
        let r = route_ref("GET /files/{path}");
        assert_eq!(
            bind_route(&routes, &r),
            Outcome::Unbound,
            "{non_normalizing} must never be approximately matched"
        );
    }
}

#[test]
fn an_operation_is_unbound_until_its_route_is_indexed() {
    // The retry-on-sync contract (FR-CG-09, FR-RS-03): the framework pass promotes
    // route nodes *after* resolution, so an operation is unbound on the index that
    // captures it and binds on the next sync once its route exists — never fabricated.
    let r = route_ref("GET /widgets/{id}");
    assert_eq!(
        bind_route(&[], &r),
        Outcome::Unbound,
        "no route yet → the operation is honestly unresolved"
    );
    let routes = [route_node(50, "GET /widgets/{widgetId}")];
    assert!(
        matches!(bind_route(&routes, &r), Outcome::Bound { target, .. } if target == NodeId(50)),
        "the same operation binds once its route is promoted (retry-on-sync)"
    );
}

#[test]
fn relation_coverage_groups_ledger_rows_by_relation_class() {
    // The per-relation-class coverage surface (FR-CG-11, FR-RS-04): bound vs
    // unresolved counts keyed by relation payload; rows without a payload (code/
    // doc/access refs) contribute nothing.
    let rows = [
        (Some("proto-import"), true),
        (Some("proto-import"), false),
        (Some("proto-import"), true),
        (Some("route"), false),
        (None, true), // a code ref — excluded from the artifact breakdown
    ];
    let cov = super::relation_coverage(rows.iter().copied());
    assert_eq!(cov.len(), 2, "only the two artifact relations appear");
    let proto = &cov["proto-import"];
    assert_eq!((proto.bound, proto.unresolved), (2, 1));
    let route = &cov["route"];
    assert_eq!((route.bound, route.unresolved), (0, 1));
}

// ── CR-011 / S-071: infra & shell binding (SQL, Terraform, shell) ─────────────
//
// SQL view/FK clauses bind by name to their `SqlObject` tables; shell `source`
// binds by path to a `ConfigFile`; Terraform `var`/`local`/`module` references
// bind by name to their declaring `TfBlock`; and a local module call is the one
// multi-target relation — it fans out to every admitted `.tf` `ConfigFile` in its
// source directory ([FR-CG-08], [UAT-CG-04]).

/// An S-071 reference sourced from an arbitrary artifact node, carrying its
/// relation class as the payload — as the SQL/Terraform/shell capture walks emit.
fn infra_ref(
    source_node: i64,
    source_file_id: i64,
    target: &str,
    form: RefForm,
    relation: ArtifactRelation,
) -> UnresolvedRefRow {
    UnresolvedRefRow {
        id: 300,
        file_id: Some(source_file_id),
        source_symbol: format!("local sym{source_node}"),
        target: target.to_string(),
        alias: None,
        form,
        kind: relation.edge_kind(),
        line: Some(1),
        resolved: false,
        payload: Some(relation.as_str().to_string()),
    }
}

#[test]
fn tf_module_call_binds_to_every_admitted_tf_in_its_source_dir() {
    // The multi-target fan-out (FR-CG-08, UAT-CG-03 step 1): a `module` block's
    // local source dir binds to each `.tf` ConfigFile *directly* in it — never a
    // non-`.tf` sibling, never a file in a nested directory.
    let nodes = vec![
        // The calling `module "net"` block lives in the root main.tf.
        node(50, "module net", NodeKind::TfBlock, "main.tf"),
        node(52, "main.tf", NodeKind::ConfigFile, "modules/net/main.tf"),
        node(
            53,
            "variables.tf",
            NodeKind::ConfigFile,
            "modules/net/variables.tf",
        ),
        // A nested-dir `.tf` is not a direct child — excluded.
        node(
            54,
            "deep.tf",
            NodeKind::ConfigFile,
            "modules/net/sub/deep.tf",
        ),
        // A non-`.tf` file in the dir is not a candidate.
        node(55, "README.md", NodeKind::DocFile, "modules/net/README.md"),
    ];
    let r = infra_ref(
        50,
        99,
        "./modules/net",
        RefForm::Path,
        ArtifactRelation::TfModuleCall,
    );
    assert_eq!(
        bind_artifact(&nodes, &r),
        Outcome::BoundMany {
            source: NodeId(50),
            targets: vec![NodeId(52), NodeId(53)],
            kind: EdgeKind::ArtifactRef,
            payload: Some("tf-module-call".to_string()),
        },
        "the module call binds to every admitted .tf directly in its source dir"
    );
}

#[test]
fn tf_module_call_to_a_dir_with_no_indexed_tf_stays_unbound() {
    // Late-bind: a module whose source dir has no indexed `.tf` yet stays unbound
    // and retries on the sync that indexes them (FR-CG-07, FR-RS-03).
    let nodes = vec![node(50, "module net", NodeKind::TfBlock, "main.tf")];
    let r = infra_ref(
        50,
        99,
        "./modules/net",
        RefForm::Path,
        ArtifactRelation::TfModuleCall,
    );
    assert_eq!(bind_artifact(&nodes, &r), Outcome::Unbound);
}

#[test]
fn tf_var_ref_binds_to_its_declaring_block() {
    // `var.region` → the `variable "region"` block, fenced to `TfBlock` by the
    // relation's target kind (FR-CG-08): a same-named non-TfBlock never binds.
    let nodes = vec![
        node(
            60,
            "resource aws_instance web",
            NodeKind::TfBlock,
            "main.tf",
        ),
        node(61, "variable region", NodeKind::TfBlock, "main.tf"),
    ];
    let r = infra_ref(
        60,
        99,
        "variable region",
        RefForm::Method,
        ArtifactRelation::TfVarRef,
    );
    assert_eq!(
        bind_artifact(&nodes, &r),
        Outcome::Bound {
            source: NodeId(60),
            target: NodeId(61),
            kind: EdgeKind::ArtifactRef,
            payload: Some("tf-var-ref".to_string()),
        }
    );
}

#[test]
fn sql_object_ref_binds_a_view_to_its_table() {
    // A view/FK clause → the `SqlObject` table it reads, by the table anchor's
    // `table <name>` form (FR-CG-08); a same-named non-SqlObject never binds.
    let nodes = vec![
        node(70, "view active_users", NodeKind::SqlObject, "schema.sql"),
        node(71, "table app.users", NodeKind::SqlObject, "schema.sql"),
    ];
    let r = infra_ref(
        70,
        99,
        "table app.users",
        RefForm::Method,
        ArtifactRelation::SqlObjectRef,
    );
    assert_eq!(
        bind_artifact(&nodes, &r),
        Outcome::Bound {
            source: NodeId(70),
            target: NodeId(71),
            kind: EdgeKind::ArtifactRef,
            payload: Some("sql-object-ref".to_string()),
        }
    );
}

#[test]
fn shell_source_binds_to_the_target_config_file_by_path() {
    // `source ./lib/common.sh` → the ConfigFile at that workspace-relative path,
    // folded against the script's own directory (FR-CG-08).
    let nodes = vec![
        node(80, "deploy.sh", NodeKind::ConfigFile, "deploy.sh"),
        node(81, "common.sh", NodeKind::ConfigFile, "lib/common.sh"),
    ];
    let r = infra_ref(
        80,
        99,
        "./lib/common.sh",
        RefForm::Path,
        ArtifactRelation::ShellSource,
    );
    assert_eq!(
        bind_artifact(&nodes, &r),
        Outcome::Bound {
            source: NodeId(80),
            target: NodeId(81),
            kind: EdgeKind::ArtifactRef,
            payload: Some("shell-source".to_string()),
        }
    );
}

#[test]
fn every_s071_relation_edge_is_metric_neutral() {
    // The load-bearing UAT-CG-04 gate at the relation grain: every edge this story
    // produces is an `ArtifactRef`, which the hydration edge predicate fences at
    // both audit points (`is_config_reference`), exactly as the S-068 substrate
    // fences the node predicate — so SQL/Terraform/shell wiring never moves
    // `aggregate_signal`, cycles, DSM, or dead-code ([UAT-CG-04], [FR-CG-05]).
    for relation in [
        ArtifactRelation::SqlObjectRef,
        ArtifactRelation::TfModuleCall,
        ArtifactRelation::TfVarRef,
        ArtifactRelation::ShellSource,
    ] {
        assert_eq!(
            relation.edge_kind(),
            EdgeKind::ArtifactRef,
            "{} is an artifact→artifact reference",
            relation.as_str()
        );
        assert!(
            relation.edge_kind().is_config_reference(),
            "{} edges must be fenced out of the code subgraph at hydration",
            relation.as_str()
        );
    }
}

// ── CR-068 Part B: bare-path Method exclusion (FR-RS-07) ─────────────────────
//
// A separate fixture that models the pathology the change fixes: a free function
// and same-named associated methods collapsed to one module scope (`impl` is not
// a captured scope, so associated items live at module scope alongside free fns).
//
// ```text
// module 1 (crate, src/lib.rs)
// ├── fn ins        (2)  Function   ┐ free fn + same-named associated method
// ├── fn ins        (3)  Method     ┘ (the graph_store insert cluster shape)
// ├── struct Store  (4)  Struct
// ├── fn make       (5)  Method     (Store::make, collapsed to module scope)
// ├── fn caller     (6)  Function   (the call site)
// ├── fn dup        (7)  Function   ┐
// ├── fn dup        (8)  Function   ┤ two free fns + a method, all named `dup`
// ├── fn dup        (9)  Method     ┘
// └── fn only_m    (10)  Method     (a lone associated method, no free twin)
// ```

fn method_cluster() -> (Vec<NodeRow>, Vec<EdgeRow>) {
    let nodes = vec![
        node(1, "crate", NodeKind::Module, "src/lib.rs"),
        node(2, "ins", NodeKind::Function, "src/lib.rs"),
        node(3, "ins", NodeKind::Method, "src/lib.rs"),
        node(4, "Store", NodeKind::Struct, "src/lib.rs"),
        node(5, "make", NodeKind::Method, "src/lib.rs"),
        node(6, "caller", NodeKind::Function, "src/lib.rs"),
        node(7, "dup", NodeKind::Function, "src/lib.rs"),
        node(8, "dup", NodeKind::Function, "src/lib.rs"),
        node(9, "dup", NodeKind::Method, "src/lib.rs"),
        node(10, "only_m", NodeKind::Method, "src/lib.rs"),
    ];
    let edges = vec![
        contains(1, 2),
        contains(1, 3),
        contains(1, 4),
        contains(1, 5),
        contains(1, 6),
        contains(1, 7),
        contains(1, 8),
        contains(1, 9),
        contains(1, 10),
    ];
    (nodes, edges)
}

fn bind_cluster(r: &UnresolvedRefRow, policy: BindingPolicy) -> Outcome {
    let (nodes, edges) = method_cluster();
    let ix = Index::build(&nodes, &edges, std::slice::from_ref(r));
    bind(r, &ix, policy)
}

fn method_call(id: i64, file_id: i64, source_node: i64, target: &str) -> UnresolvedRefRow {
    make_ref(
        id,
        file_id,
        source_node,
        target,
        None,
        RefForm::Method,
        EdgeKind::Calls,
    )
}

#[test]
fn bare_call_binds_the_free_function_over_a_same_named_method() {
    // FR-RS-07: `ins()` in the same module as a free `fn ins` AND an associated
    // `ins` method binds the *free function* — the exclusion breaks the tie that
    // previously left this `Ambiguous`/unbound. Holds under every policy (it is a
    // genuine scope-evidence bind, not a workspace fallback).
    for policy in [
        BindingPolicy::Strict,
        BindingPolicy::Balanced,
        BindingPolicy::Aggressive,
    ] {
        let r = call(100, LIB_RS, 6, "ins");
        bound_to(bind_cluster(&r, policy), 6, 2, EdgeKind::Calls);
    }
}

#[test]
fn bare_call_ambiguous_among_two_free_functions_stays_unresolved() {
    // Never-fabricate ([NFR-RA-05]): excluding the `dup` *method* still leaves two
    // free `dup` functions — a real ambiguity — so the bare call stays unbound.
    for policy in [
        BindingPolicy::Strict,
        BindingPolicy::Balanced,
        BindingPolicy::Aggressive,
    ] {
        let r = call(100, LIB_RS, 6, "dup");
        assert_eq!(
            bind_cluster(&r, policy),
            Outcome::Unbound,
            "two free-fn candidates must never bind ({policy:?})"
        );
    }
}

#[test]
fn bare_call_to_a_lone_method_still_binds_the_method_monotonic() {
    // The exclusion is a *tie-break*, not a filter: with no free function of the
    // name, the full callable set stands, so a bare call whose only same-scope
    // candidate is an associated method still binds it — no previously-resolved
    // edge is lost (monotonic, [NFR-RA-05]). This is also what keeps a language
    // whose free callables are `Method` (e.g. a Ruby top-level `def`) unaffected.
    for policy in [
        BindingPolicy::Strict,
        BindingPolicy::Balanced,
        BindingPolicy::Aggressive,
    ] {
        let r = call(100, LIB_RS, 6, "only_m");
        bound_to(bind_cluster(&r, policy), 6, 10, EdgeKind::Calls);
    }
}

#[test]
fn path_qualified_call_still_binds_an_associated_method() {
    // Unchanged: `Store::make()` is a multi-segment path, so the full callable set
    // (methods included) is used — the `Type::func` collapse binds the associated
    // `make` method. The exclusion is strictly single-segment.
    let r = call(100, LIB_RS, 6, "Store::make");
    bound_to(bind_cluster(&r, BindingPolicy::Strict), 6, 5, EdgeKind::Calls);
}

#[test]
fn receiver_method_call_still_binds_a_method() {
    // Unchanged: a receiver-unqualified method call (`x.only_m()`, RefForm::Method)
    // resolves through scope evidence and still admits `Method` candidates — the
    // exclusion applies only to single-segment *bare-path* calls, not receiver
    // calls. A uniquely-named in-scope method binds.
    let r = method_call(100, LIB_RS, 6, "only_m");
    bound_to(bind_cluster(&r, BindingPolicy::Strict), 6, 10, EdgeKind::Calls);
}
