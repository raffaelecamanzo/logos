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

use super::binder::{bind, residue, Index, Outcome, Residue};
use super::package_key::PackageLayout;
use crate::config::BindingPolicy;
use crate::graph_store::{EdgeRow, ImplBlockRow, NodeArity, NodeRow, UnresolvedRefRow};
use crate::model::{ArtifactRelation, EdgeKind, LogosSymbol, NodeId, NodeKind, ParamRange, ReceiverShape, RefForm};

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

/// `ix` given the impl blocks a Rust extraction records for `self_types`
/// (S-606), as the one associated-item lookup reads them (S-607): one block per
/// callable, on that callable's own line (its id), headed by its recorded self
/// type and — when an `Implements` row of `refs` sources at the callable — by
/// the trait that row names.
fn with_impl_blocks(
    ix: Index,
    nodes: &[NodeRow],
    self_types: &[(NodeId, String)],
    refs: &[UnresolvedRefRow],
) -> Index {
    let lined: Vec<NodeRow> = nodes
        .iter()
        .map(|n| NodeRow {
            start_line: Some(n.id.0),
            end_line: Some(n.id.0),
            ..n.clone()
        })
        .collect();
    let blocks: Vec<ImplBlockRow> = self_types
        .iter()
        .filter_map(|(id, ty)| {
            let file = nodes.iter().find(|n| n.id == *id)?.file_path.clone()?;
            let line = u32::try_from(id.0).ok()?;
            let trait_path = refs
                .iter()
                .find(|r| r.kind == EdgeKind::Implements && r.source_symbol == format!("local sym{}", id.0))
                .map(|r| r.target.clone());
            Some(ImplBlockRow {
                file_path: file,
                start_line: line,
                end_line: line,
                self_type: ty.clone(),
                self_ref: false,
                trait_path,
                deref_target: None,
            })
        })
        .collect();
    ix.with_associated_items(&lined, &blocks, &[])
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
        receiver: None,
        peeled: None,
        arg_count: None,
        exported: None,
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
fn a_receiver_call_never_binds_through_the_callers_scope_whatever_its_shape() {
    use super::binder::{residue, Residue};
    // CR-169 corrects CR-066's "same-scope evidence": the module `alpha` sits
    // in says where the CALLER is, not what the receiver is. `x.helper()` from
    // `alpha` once bound the sibling module-level `helper` (id 3); under S-514
    // no shape binds it — not `other`, not none, and not `self` / `super`
    // either, `alpha` being in no class — at any policy tier.
    for receiver in [None, Some(ReceiverShape::Other), Some(ReceiverShape::SelfInstance), Some(ReceiverShape::Super)] {
        let r = shaped(100, LIB_RS, 2, "helper", receiver);
        for policy in [BindingPolicy::Strict, BindingPolicy::Balanced, BindingPolicy::Aggressive] {
            assert_eq!(bind_with(&[], &r, policy), Outcome::Unbound, "{receiver:?} at {policy:?}");
        }
        let (nodes, edges) = fixture();
        let ix = Index::build(&nodes, &edges, std::slice::from_ref(&r));
        assert_eq!(
            residue(&r, &ix, BindingPolicy::Aggressive),
            Some(Residue::NoReceiverEvidence),
            "{receiver:?}: the readout reason, in a language that is not package-shaped"
        );
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

    // The bound branch: a `self` call binding its own class's member (S-514).
    let bound = shaped(101, PY_FILE, 204, "m", Some(ReceiverShape::SelfInstance));
    let b1 = bind_shapes(&bound, BindingPolicy::Balanced);
    let b2 = bind_shapes(&bound, BindingPolicy::Balanced);
    bound_to(b1.clone(), 204, 203, EdgeKind::Calls);
    assert_eq!(b1, b2, "a shape-dispatched method bind is also deterministic");
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

// ── The Type::func rule: the collapse, and Rust's one lookup ────────────────

#[test]
fn associated_function_paths_bind_through_the_type_collapse_rule() {
    // struct Widget + fn new in the same module: in a language without impl
    // blocks the `Type::func` collapse binds `Widget::new()` to the module's
    // `new`, as it always did.
    let (mut nodes, mut edges) = fixture();
    nodes.push(node(40, "Widget", NodeKind::Struct, "src/util.rs"));
    nodes.push(node(41, "new", NodeKind::Function, "src/util.rs"));
    edges.push(contains(4, 40));
    edges.push(contains(4, 41));
    let r = call(100, LIB_RS, 2, "util::Widget::new");
    let all = vec![r.clone()];
    let ix = Index::build_with_layout(&nodes, &edges, &all, PackageLayout::stems_only_for_tests());
    bound_to(bind(&r, &ix, BindingPolicy::Strict), 2, 41, EdgeKind::Calls);
    // Rust's call binds through the one lookup (S-607): `new` is no function
    // of an impl block of `Widget`, so the call binds nothing — 1.13.0 bound
    // any module-level `new`, `A::default()` → `Bb::default` among them.
    let ix = Index::build(&nodes, &edges, &all);
    assert_eq!(bind(&r, &ix, BindingPolicy::Strict), Outcome::Unbound);
    assert_eq!(residue(&r, &ix, BindingPolicy::Strict), Some(Residue::SupertypeUnreached));
    // With `new` recorded in `impl Widget`, it binds.
    let ix = with_impl_blocks(ix, &nodes, &[(NodeId(41), "Widget".to_string())], &all);
    bound_to(bind(&r, &ix, BindingPolicy::Strict), 2, 41, EdgeKind::Calls);
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
        receiver: None,
        peeled: None,
        arg_count: None,
        exported: None,
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

/// **[CR-107] never-fabricate guard.** A **keyless** artifact reference names
/// nothing and must bind to nothing, even when an artifact-layer node in the graph
/// happens to carry an empty name.
///
/// This is the gate the broker arm's recorded `topic-not-literal` refusals rely on.
/// A refusal is emitted as `(EdgeKind::ArtifactRef, RefForm::Method)` with an empty
/// target and no declared `target_kind`, which `bind`'s dispatch routes to
/// `resolve_artifact_name` with `want_kind = None` — where "any artifact-layer node
/// is a candidate". Without the empty-name guard the sole empty-named node below is
/// exactly one candidate, so the `exactly_one` rule would bind it: a refusal turned
/// into an edge, which is the opposite of what recording it is for ([NFR-RA-05]).
///
/// [CR-107]: ../../../docs/requests/CR-107-broker-topic-capture-drops-placeholder-and-array-literals.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
#[test]
fn a_keyless_artifact_ref_binds_to_nothing_even_beside_an_empty_named_node() {
    // One artifact-layer node whose name is empty — the sole candidate an
    // unguarded empty-name lookup would find and bind.
    let nodes = vec![
        cfg_node(SVC_PROTO, "svc.proto", NodeKind::ConfigFile, "svc.proto"),
        cfg_node(31, "", NodeKind::ProtoMessage, "svc.proto"),
    ];
    for target in ["", "   "] {
        let r = artifact_ref(target, RefForm::Method, ArtifactRelation::BrokerSubscribe);
        assert_eq!(
            bind_artifact(&nodes, &r),
            Outcome::Unbound,
            "a keyless broker refusal row must never bind (target {target:?})"
        );
    }
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

// ── CR-109 / S-349: wildcard-method matching with exact-method precedence ────
//
// The intra-repo half of the shared fixture matrix. The cross-member bridge and
// the federation coverage read-model drive the *same* rows from
// `resolve::route_method::matrix`, so an input that binds at one site and not at
// another fails at one of the three ([FR-CG-09] AC3, [ADR-52]).

/// Every matrix case, driven through the `ApiOperation` → `Route` binder.
#[test]
fn the_wildcard_method_matrix_holds_at_the_intra_repo_binder() {
    for case in super::route_method::matrix::MATRIX {
        let routes: Vec<NodeRow> = case
            .providers
            .iter()
            .enumerate()
            .map(|(i, name)| route_node(50 + i as i64, name))
            .collect();
        let outcome = bind_route(&routes, &route_ref(case.consumer));
        match case.expect.bound() {
            Some(i) => {
                let want = NodeId(50 + i as i64);
                assert!(
                    matches!(outcome, Outcome::Bound { target, .. } if target == want),
                    "{}: `{}` must bind provider {i} (`{}`), got {outcome:?}",
                    case.name,
                    case.consumer,
                    case.providers[i]
                );
            }
            None => assert_eq!(
                outcome,
                Outcome::Unbound,
                "{}: `{}` must not bind against {:?}",
                case.name,
                case.consumer,
                case.providers
            ),
        }
    }
}

/// A wildcard route is a candidate for *every* verb, not merely the one the
/// matrix happens to name — the property the `ANY` token asserts, checked over
/// the whole method vocabulary an OpenAPI document can carry.
#[test]
fn a_wildcard_route_binds_every_verb_an_operation_can_declare() {
    let routes = [route_node(50, "ANY /v1/things/{id}")];
    for verb in ["GET", "PUT", "POST", "DELETE", "PATCH", "HEAD", "OPTIONS"] {
        let r = route_ref(&format!("{verb} /v1/things/{{thingId}}"));
        assert!(
            matches!(bind_route(&routes, &r), Outcome::Bound { target, .. } if target == NodeId(50)),
            "a bare @RequestMapping route serves {verb} (FR-CG-09, CR-109)"
        );
    }
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
        receiver: None,
        peeled: None,
        arg_count: None,
        exported: None,
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
    // It holds for a language that does not declare `implicit_receiver = "none"`
    // — this layout declares nothing; S-590's filter is pinned below.
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
    // `Store::make()` is a multi-segment path: it binds the associated `make`
    // among `Store`'s recorded impl functions, by the one associated-item
    // lookup (S-607). The bare-call exclusion is strictly single-segment.
    let r = call(100, LIB_RS, 6, "Store::make");
    let (nodes, edges) = method_cluster();
    let ix = with_impl_blocks(
        Index::build(&nodes, &edges, std::slice::from_ref(&r)),
        &nodes,
        &cluster_self_types(),
        std::slice::from_ref(&r),
    );
    bound_to(bind(&r, &ix, BindingPolicy::Strict), 6, 5, EdgeKind::Calls);
}

#[test]
fn a_receiver_call_no_longer_binds_an_in_scope_method_by_name() {
    // CR-169 (S-514): `x.only_m()` once bound the uniquely-named in-scope method
    // through the scope walk. Its receiver is not proven to be anything, so it
    // stays unbound — the `bare_path_call` tie-break below never even runs for it.
    let r = method_call(100, LIB_RS, 6, "only_m");
    assert_eq!(bind_cluster(&r, BindingPolicy::Strict), Outcome::Unbound);
}

#[test]
fn receiver_method_call_does_not_apply_the_free_function_tiebreak() {
    // The load-bearing guard for the `bare_path_call` flag (vs keying the
    // tie-break on `want`): a receiver-unqualified method call (`x.ins()`,
    // RefForm::Method) to a name that has BOTH a free `Function` and a same-named
    // `Method` must stay ambiguous/unresolved — the free-function tie-break must
    // NOT fire here (the receiver type is unknown; CR-066 discipline). Contrast
    // `bare_call_binds_the_free_function_over_a_same_named_method`, which binds the
    // *same* name to the free fn (node 2) through the bare-path branch. If the
    // exclusion were keyed on `want == Want::Callable` instead of the flag, this
    // would wrongly bind and the test would fail.
    let r = method_call(100, LIB_RS, 6, "ins");
    assert_eq!(
        bind_cluster(&r, BindingPolicy::Strict),
        Outcome::Unbound,
        "a receiver call with a free-fn + method of the same name must stay ambiguous"
    );
}

// ── S-590: a bare call reaches no instance member (FR-RS-07 as amended) ──────
//
// The method cluster again, under a layout whose `.rs` language declares
// `implicit_receiver = "none"` explicitly, and with the self type the Rust
// plugin records for each `impl` member: `ins` (3), `make` (5), `dup` (9) and
// `only_m` (10) are methods of `Store`. A bare call drops them from its
// candidates on every rung; a path-qualified call still reaches them.

/// The self types of the cluster's associated methods.
fn cluster_self_types() -> Vec<(NodeId, String)> {
    [3, 5, 9, 10]
        .into_iter()
        .map(|id| (NodeId(id), "Store".to_string()))
        .collect()
}

/// [`bind_cluster`] with the self types recorded, under a layout that declares
/// the free-only bare call for `.rs` when `free_only` holds, and nothing else.
fn bind_cluster_declared(r: &UnresolvedRefRow, policy: BindingPolicy, free_only: bool) -> Outcome {
    let (nodes, edges) = method_cluster();
    let layout = super::package_key::PackageLayout::default()
        .with_free_only_bare_calls(free_only.then(|| "rs".to_string()));
    let ix = Index::build_with_layout(&nodes, &edges, std::slice::from_ref(r), layout)
        .with_self_types(cluster_self_types());
    bind(r, &ix, policy)
}

#[test]
fn a_bare_call_to_a_lone_method_binds_nothing_where_the_language_declares_none() {
    // No free `only_m`: the scope walk passes over the method, and so does the
    // aggressive workspace fallback that would otherwise find it by name.
    for policy in POLICIES {
        let r = call(100, LIB_RS, 6, "only_m");
        assert_eq!(
            bind_cluster_declared(&r, policy, true),
            Outcome::Unbound,
            "a bare call never reaches a self-typed method ({policy:?})"
        );
    }
}

#[test]
fn an_omitted_declaration_keeps_binding_the_lone_method() {
    // The same graph, self types and all, under a layout that declares nothing
    // — Java's case: the bare call binds the method exactly as before.
    for policy in POLICIES {
        let r = call(100, LIB_RS, 6, "only_m");
        bound_to(bind_cluster_declared(&r, policy, false), 6, 10, EdgeKind::Calls);
    }
}

#[test]
fn a_declared_none_bare_call_still_binds_the_free_function() {
    // `ins`: the free function binds; `dup`: two free functions stay ambiguous
    // once the method is gone, never a guess between them.
    for policy in POLICIES {
        bound_to(
            bind_cluster_declared(&call(100, LIB_RS, 6, "ins"), policy, true),
            6,
            2,
            EdgeKind::Calls,
        );
        assert_eq!(
            bind_cluster_declared(&call(101, LIB_RS, 6, "dup"), policy, true),
            Outcome::Unbound,
            "{policy:?}"
        );
    }
}

#[test]
fn a_declared_none_path_qualified_call_still_binds_the_associated_method() {
    let r = call(100, LIB_RS, 6, "Store::make");
    bound_to(bind_cluster_declared(&r, BindingPolicy::Strict, true), 6, 5, EdgeKind::Calls);
}

// A Python-shaped graph for (a), a member of a class-like container:
//
// ```text
// module 1 (app.py)
// ├── fn f        (2)  Function   (free; present only in `with_free`)
// ├── class A     (3)  Class
// │   ├── fn f    (4)  Function   (the method — a Python method is a Function)
// │   │   └── fn g (5) Function   (nested in the method)
// │   └── class Inner (6) Class   (nested in the class; a call constructs it)
// ```

fn class_member_index(r: &UnresolvedRefRow, with_free: bool, free_only: bool) -> Index {
    let mut nodes = vec![
        node(1, "app", NodeKind::Module, "app.py"),
        node(3, "A", NodeKind::Class, "app.py"),
        node(4, "f", NodeKind::Function, "app.py"),
        node(5, "g", NodeKind::Function, "app.py"),
        node(6, "Inner", NodeKind::Class, "app.py"),
    ];
    let mut edges = vec![contains(1, 3), contains(3, 4), contains(4, 5), contains(3, 6)];
    if with_free {
        nodes.push(node(2, "f", NodeKind::Function, "app.py"));
        edges.push(contains(1, 2));
    }
    let layout = super::package_key::PackageLayout::default()
        .with_call_targets(std::collections::HashMap::from([(
            "py".to_string(),
            crate::plugin::CallTargets {
                classes: true,
                macros: false,
            },
        )]))
        .with_free_only_bare_calls(free_only.then(|| "py".to_string()));
    Index::build_with_layout(&nodes, &edges, std::slice::from_ref(r), layout)
}

#[test]
fn a_bare_call_inside_a_same_named_class_member_makes_no_self_loop() {
    let r = call(100, PY_FILE, 4, "f");
    let ix = class_member_index(&r, false, true);
    assert_eq!(bind(&r, &ix, BindingPolicy::Strict), Outcome::Unbound);
    // Undeclared, the class's own member is the one candidate: the self-loop
    // the Sprint 87 review found.
    let ix = class_member_index(&r, false, false);
    bound_to(bind(&r, &ix, BindingPolicy::Strict), 4, 4, EdgeKind::Calls);
}

#[test]
fn a_bare_call_passes_over_the_class_member_to_the_free_function_further_out() {
    let r = call(100, PY_FILE, 4, "f");
    let ix = class_member_index(&r, true, true);
    bound_to(bind(&r, &ix, BindingPolicy::Strict), 4, 2, EdgeKind::Calls);
}

#[test]
fn a_bare_call_never_constructs_a_class_its_class_nests() {
    // A Python method's bare name never sees class scope: `Inner()` inside
    // `A.f` is a NameError, not `A.Inner`. Undeclared, it constructs it.
    let r = call(100, PY_FILE, 4, "Inner");
    let ix = class_member_index(&r, false, true);
    assert_eq!(bind(&r, &ix, BindingPolicy::Strict), Outcome::Unbound);
    let ix = class_member_index(&r, false, false);
    bound_to(bind(&r, &ix, BindingPolicy::Strict), 4, 6, EdgeKind::Instantiates);
}

#[test]
fn a_bare_call_to_a_function_nested_in_a_method_still_binds() {
    let r = call(100, PY_FILE, 4, "g");
    let ix = class_member_index(&r, false, true);
    bound_to(bind(&r, &ix, BindingPolicy::Strict), 4, 5, EdgeKind::Calls);
}

// ── Trait-object dynamic-dispatch fan-out (S-281, CR-073 Part C, FR-RS-08) ───
//
// A receiver-method call on a *provable* workspace trait object (`p.f()` where
// `p: &dyn T`) resolves to the SET of concrete workspace impls of that trait
// method, plus the trait's own default body — fan-out to exactly-the-impls,
// never fabricated. Extraction encodes the provable-trait-object gate as a
// trait-qualified `T::f` target on a `RefForm::Method`/`EdgeKind::Calls` ref
// (a bare `.f()` stays a single-segment target and never fans out, so the
// CR-066 receiver-method guard is untouched). The impl set is enumerated from
// `EdgeKind::Implements` refs (impl method → its trait), one per impl method.

/// Fixture for the dyn-dispatch fan-out tests: one trait `Plug` with a default
/// method `run`, two workspace impls of `run` (`Alpha`, `Beta`) linked to `Plug`
/// via Implements refs, and one same-named `run` on an UNRELATED type that
/// implements nothing — the fan-out must reach the first three and never the
/// unrelated one.
fn dyn_fixture() -> (Vec<NodeRow>, Vec<EdgeRow>) {
    let nodes = vec![
        node(1, "crate", NodeKind::Module, "src/lib.rs"),
        node(2, "caller", NodeKind::Function, "src/lib.rs"),
        node(41, "Plug", NodeKind::Trait, "src/lib.rs"),
        node(42, "run", NodeKind::Function, "src/lib.rs"), // Plug's default body
        node(51, "run", NodeKind::Method, "src/lib.rs"),   // impl Plug for Alpha
        node(61, "run", NodeKind::Method, "src/lib.rs"),   // impl Plug for Beta
        node(71, "run", NodeKind::Method, "src/lib.rs"),   // unrelated type's run
    ];
    let edges = vec![
        contains(1, 2),
        contains(1, 41),
        contains(41, 42), // the trait Contains its default method
        contains(1, 51),  // impl methods collapse to the file module (impl is not a scope)
        contains(1, 61),
        contains(1, 71),
    ];
    (nodes, edges)
}

/// An Implements ref linking impl method `source_node` to the trait named
/// `trait_name` (the extraction-side signal that enumerates a trait's impls).
fn implements(id: i64, source_node: i64, trait_name: &str) -> UnresolvedRefRow {
    make_ref(
        id,
        LIB_RS,
        source_node,
        trait_name,
        None,
        RefForm::Path,
        EdgeKind::Implements,
    )
}

/// A trait-qualified dyn-dispatch call `T::f` (the provable-trait-object shape
/// extraction emits for `p.f()` when `p: &dyn T`).
fn dyn_call(id: i64, source_node: i64, target: &str) -> UnresolvedRefRow {
    make_ref(
        id,
        LIB_RS,
        source_node,
        target,
        None,
        RefForm::Method,
        EdgeKind::Calls,
    )
}

/// Bind `r` against the dyn fixture plus the given scope refs (Implements rows).
fn bind_dyn(refs: &[UnresolvedRefRow], r: &UnresolvedRefRow, policy: BindingPolicy) -> Outcome {
    let (nodes, edges) = dyn_fixture();
    let mut all = refs.to_vec();
    all.push(r.clone());
    let ix = Index::build(&nodes, &edges, &all);
    bind(r, &ix, policy)
}

/// Assert a `BoundMany` fan-out to exactly `targets` (order-independent).
fn bound_many_to(outcome: Outcome, source: i64, mut targets: Vec<i64>, kind: EdgeKind) {
    targets.sort_unstable();
    match outcome {
        Outcome::BoundMany {
            source: got_source,
            targets: got_targets,
            kind: got_kind,
            payload,
        } => {
            let mut got: Vec<i64> = got_targets.iter().map(|t| t.0).collect();
            got.sort_unstable();
            assert_eq!(got_source, NodeId(source), "fan-out source");
            assert_eq!(got, targets, "fan-out target set");
            assert_eq!(got_kind, kind, "fan-out edge kind");
            assert_eq!(payload, None, "code fan-out carries no payload");
        }
        other => panic!("expected BoundMany, got {other:?}"),
    }
}

#[test]
fn dyn_trait_method_call_fans_out_to_trait_default_and_impls() {
    // `p.run()` with `p: &dyn Plug` (target `Plug::run`) binds to the trait's own
    // default body (42) AND every workspace impl of `run` (51, 61) — one edge
    // per target, the union-reachability model for dynamic dispatch (FR-RS-08).
    let scope = [implements(200, 51, "Plug"), implements(201, 61, "Plug")];
    let r = dyn_call(300, 2, "Plug::run");
    bound_many_to(
        bind_dyn(&scope, &r, BindingPolicy::Strict),
        2,
        vec![42, 51, 61],
        EdgeKind::Calls,
    );
}

#[test]
fn dyn_fanout_binds_exactly_the_workspace_impls_never_an_unrelated_type() {
    // Node 71 is a same-named `run` on a type that implements nothing (no
    // Implements ref). It must NEVER be a fan-out target — "exactly the impls,
    // never a same-named method on an unrelated type" (NFR-RA-05, the AC guard).
    let scope = [implements(200, 51, "Plug"), implements(201, 61, "Plug")];
    let r = dyn_call(300, 2, "Plug::run");
    let out = bind_dyn(&scope, &r, BindingPolicy::Strict);
    if let Outcome::BoundMany { targets, .. } = &out {
        assert!(
            !targets.contains(&NodeId(71)),
            "the unrelated same-named `run` (71) must not be bound: {targets:?}"
        );
    } else {
        panic!("expected BoundMany, got {out:?}");
    }
}

#[test]
fn dyn_call_with_a_default_but_no_impls_binds_the_default_only() {
    // A trait method with a default body and ZERO overriding workspace impls
    // binds to the default alone (FR-RS-08: "a default with zero overriding impls
    // binds to the default"). No Implements refs → only the trait default (42).
    let r = dyn_call(300, 2, "Plug::run");
    bound_many_to(
        bind_dyn(&[], &r, BindingPolicy::Strict),
        2,
        vec![42],
        EdgeKind::Calls,
    );
}

#[test]
fn dyn_call_to_an_external_trait_stays_unresolved() {
    // `q.f()` where `q: &dyn External` and `External` is not a workspace trait:
    // no workspace Trait node named `External`, no impls → honest miss, the ref
    // stays in the ledger (never fabricated to a same-named local, NFR-RA-05).
    let r = dyn_call(300, 2, "External::run");
    assert_eq!(
        bind_dyn(&[], &r, BindingPolicy::Aggressive),
        Outcome::Unbound,
        "a dyn call to a non-workspace trait must stay unresolved"
    );
}

#[test]
fn bare_receiver_method_call_never_fans_out() {
    // A bare `.run()` (single-segment `run`, no `::`) is NOT a provable trait
    // object: it must take the ordinary receiver-method path and never fan out —
    // the CR-066 guard (FR-RS-06) is not loosened. Here `run` is ambiguous at
    // module scope (three candidates), so it stays unresolved, and crucially the
    // outcome is never a BoundMany.
    let r = make_ref(300, LIB_RS, 2, "run", None, RefForm::Method, EdgeKind::Calls);
    let out = bind_dyn(&[implements(200, 51, "Plug")], &r, BindingPolicy::Aggressive);
    assert!(
        !matches!(out, Outcome::BoundMany { .. }),
        "a bare receiver-method call must never fan out: {out:?}"
    );
}

#[test]
fn implements_ref_binds_impl_method_to_its_trait() {
    // The Implements ref (impl method 51 → trait `Plug`) binds to an Implements
    // edge 51 --Implements--> 41 (the trait node). This is the structural link
    // the fan-out enumerates impls from.
    let r = implements(200, 51, "Plug");
    bound_to(
        bind_dyn(&[], &r, BindingPolicy::Strict),
        51,
        41,
        EdgeKind::Implements,
    );
}

#[test]
fn dyn_call_to_an_ambiguously_named_trait_stays_unresolved() {
    // Two workspace traits share the name `Plug`, so `trait_by_name` returns None
    // (never guesses one of several) and the dyn call is an honest miss — the
    // "several" branch of the never-fabricate rule (NFR-RA-05), distinct from the
    // "zero" (external) branch.
    let (mut nodes, edges) = dyn_fixture();
    nodes.push(node(80, "Plug", NodeKind::Trait, "other/src/lib.rs")); // a second `Plug`
    let scope = [implements(200, 51, "Plug"), implements(201, 61, "Plug")];
    let r = dyn_call(300, 2, "Plug::run");
    let mut all = scope.to_vec();
    all.push(r.clone());
    let ix = Index::build(&nodes, &edges, &all);
    assert_eq!(
        bind(&r, &ix, BindingPolicy::Aggressive),
        Outcome::Unbound,
        "a dyn call to an ambiguously-named trait must stay unresolved"
    );
}

// ── The imported rung for path-grammar files (S-440, CR-142 D2, [FR-RS-03]) ──
//
// A TypeScript/Go file's call through an import is recorded
// `<import target>::<name>` by extraction; the binder resolves it within what
// that file's `Imports` row for `<import target>` bound to. These fixtures pin
// the rung on its own: each fixture with an import first asserts what that
// import binds (or, for a package, that it binds nothing), so a failure here is
// the imported rung and never the specifier canonicaliser (S-439).
//
// ```text
// src/menu.ts (module 300)  ── menu()        (301)
// src/nav.ts  (module 310)  ── navItemsFor() (311), render (method, 312)
// src/b.ts    (module 320)  ── navItemsFor() (321)   ← same name, not imported
// pkg/one.go  (module 330)  ── F() (331)      pkg/two.go (module 340) ── F() (341), G() (342)
// ```

/// File id of `src/menu.ts`'s ledger rows.
const MENU_TS: i64 = 30;

fn imported_fixture() -> (Vec<NodeRow>, Vec<EdgeRow>) {
    let nodes = vec![
        node(300, "menu", NodeKind::Module, "src/menu.ts"),
        node(301, "menu", NodeKind::Function, "src/menu.ts"),
        node(310, "nav", NodeKind::Module, "src/nav.ts"),
        node(311, "navItemsFor", NodeKind::Function, "src/nav.ts"),
        node(312, "render", NodeKind::Method, "src/nav.ts"),
        node(320, "b", NodeKind::Module, "src/b.ts"),
        node(321, "navItemsFor", NodeKind::Function, "src/b.ts"),
        node(330, "one", NodeKind::Module, "pkg/one.go"),
        node(331, "F", NodeKind::Function, "pkg/one.go"),
        node(340, "two", NodeKind::Module, "pkg/two.go"),
        node(341, "F", NodeKind::Function, "pkg/two.go"),
        node(342, "G", NodeKind::Function, "pkg/two.go"),
    ];
    let edges = vec![
        contains(300, 301),
        contains(310, 311),
        contains(310, 312),
        contains(320, 321),
        contains(330, 331),
        contains(340, 341),
        contains(340, 342),
    ];
    (nodes, edges)
}

/// `.ts` files write path specifiers resolving to `.ts` files (S-439).
fn ts_specifiers() -> std::collections::HashMap<String, std::collections::HashSet<String>> {
    std::iter::once(("ts".to_string(), std::iter::once("ts".to_string()).collect())).collect()
}

/// An `Imports` row of `src/menu.ts`, attributed to its file module.
fn menu_import(id: i64, target: &str, form: RefForm) -> UnresolvedRefRow {
    make_ref(id, MENU_TS, 300, target, None, form, EdgeKind::Imports)
}

/// Build the index over `refs` — with the imported scope when `imported`.
fn imported_index(refs: &[UnresolvedRefRow], imported: bool) -> Index {
    let (nodes, edges) = imported_fixture();
    let ix = Index::build(&nodes, &edges, refs).with_path_specifiers(ts_specifiers(), Vec::new());
    if imported {
        ix.with_imported_bindings(refs, BindingPolicy::Balanced)
    } else {
        ix
    }
}

#[test]
fn a_call_through_a_bound_import_binds_to_the_imported_function() {
    let import = menu_import(1, ".::nav", RefForm::Path);
    let r = call(2, MENU_TS, 301, ".::nav::navItemsFor");
    let refs = [import.clone(), r.clone()];
    let ix = imported_index(&refs, true);
    // The import already binds — the precondition this rung builds on.
    bound_to(bind(&import, &ix, BindingPolicy::Balanced), 300, 310, EdgeKind::Imports);
    // …and the call binds through it, to nav.ts's navItemsFor — never b.ts's.
    bound_to(bind(&r, &ix, BindingPolicy::Balanced), 301, 311, EdgeKind::Calls);
}

#[test]
fn the_same_call_binds_nothing_when_the_imported_scope_is_not_built() {
    // The same ledger, the same bound import, but `with_imported_bindings`
    // never ran: the rung decides the call over an empty scope, and it binds
    // nowhere (in particular not to b.ts's same-named function). This is the
    // differential proving the bind in the previous test comes from the
    // import's binding, and nothing else.
    let import = menu_import(1, ".::nav", RefForm::Path);
    let r = call(2, MENU_TS, 301, ".::nav::navItemsFor");
    let refs = [import.clone(), r.clone()];
    let ix = imported_index(&refs, false);
    bound_to(bind(&import, &ix, BindingPolicy::Balanced), 300, 310, EdgeKind::Imports);
    assert_eq!(bind(&r, &ix, BindingPolicy::Aggressive), Outcome::Unbound);
}

#[test]
fn the_imported_rung_reads_the_import_binding_whatever_bound_it() {
    // The import row here is a capture-before-delete `Symbol` row: it binds by
    // exact symbol lookup, with none of S-439's specifier rules involved. The
    // rung still resolves the call within it — the two defects are provable
    // apart.
    let import = menu_import(1, "local sym310", RefForm::Symbol);
    let r = call(2, MENU_TS, 301, "local sym310::navItemsFor");
    let refs = [import.clone(), r.clone()];
    let ix = imported_index(&refs, true);
    bound_to(bind(&import, &ix, BindingPolicy::Balanced), 300, 310, EdgeKind::Imports);
    bound_to(bind(&r, &ix, BindingPolicy::Balanced), 301, 311, EdgeKind::Calls);
}

#[test]
fn a_call_is_resolved_only_within_the_import_it_names() {
    // The file imports both nav.ts and b.ts, each defining `navItemsFor`. The
    // call names its import, so each binds to its own module's function and
    // neither ever binds to both (NFR-RA-05).
    let via_nav = call(3, MENU_TS, 301, ".::nav::navItemsFor");
    let via_b = call(4, MENU_TS, 301, ".::b::navItemsFor");
    let (nav, b) = (menu_import(1, ".::nav", RefForm::Path), menu_import(2, ".::b", RefForm::Path));
    let refs = [nav.clone(), b.clone(), via_nav.clone(), via_b.clone()];
    let ix = imported_index(&refs, true);
    bound_to(bind(&nav, &ix, BindingPolicy::Balanced), 300, 310, EdgeKind::Imports);
    bound_to(bind(&b, &ix, BindingPolicy::Balanced), 300, 320, EdgeKind::Imports);
    bound_to(bind(&via_nav, &ix, BindingPolicy::Aggressive), 301, 311, EdgeKind::Calls);
    bound_to(bind(&via_b, &ix, BindingPolicy::Aggressive), 301, 321, EdgeKind::Calls);
}

#[test]
fn an_import_that_binds_nothing_decides_its_calls_unbound() {
    // `import { navItemsFor } from 'react-nav'` — a package, bound to nothing.
    // Two workspace `navItemsFor`s exist and a lexical `menu` too; none is the
    // call's target, and the call must not fall through to any wider scope.
    let import = menu_import(1, "react-nav", RefForm::Path);
    let r = call(2, MENU_TS, 301, "react-nav::navItemsFor");
    let lexical = call(3, MENU_TS, 301, "react-nav::menu");
    let refs = [import.clone(), r.clone(), lexical.clone()];
    let ix = imported_index(&refs, true);
    assert_eq!(bind(&import, &ix, BindingPolicy::Aggressive), Outcome::Unbound);
    assert_eq!(bind(&r, &ix, BindingPolicy::Aggressive), Outcome::Unbound);
    assert_eq!(bind(&lexical, &ix, BindingPolicy::Aggressive), Outcome::Unbound);
}

#[test]
fn only_a_top_level_function_is_an_imported_candidate() {
    // `render` is a method of nav.ts: reached through a value, not through the
    // module, so the rung never binds it (the FR-RS-06 receiver discipline).
    let import = menu_import(1, ".::nav", RefForm::Path);
    let r = call(2, MENU_TS, 301, ".::nav::render");
    let control = call(3, MENU_TS, 301, ".::nav::navItemsFor");
    let refs = [import.clone(), r.clone(), control.clone()];
    let ix = imported_index(&refs, true);
    bound_to(bind(&import, &ix, BindingPolicy::Balanced), 300, 310, EdgeKind::Imports);
    // The same import, the same scope: a function binds through it…
    bound_to(bind(&control, &ix, BindingPolicy::Balanced), 301, 311, EdgeKind::Calls);
    // …and the method beside it does not.
    assert_eq!(bind(&r, &ix, BindingPolicy::Aggressive), Outcome::Unbound);
}

#[test]
fn a_package_import_binding_several_files_needs_exactly_one_definition() {
    // A Go import binds its package — every file of the directory, one
    // `BoundMany` row. `G` is defined once across the package and binds; `F`
    // is defined in both files and stays unresolved rather than binding to
    // either, or to both (NFR-RA-05).
    let (mut nodes, mut edges) = imported_fixture();
    nodes.extend([
        node(350, "main", NodeKind::Module, "cmd/main.go"),
        node(351, "main", NodeKind::Function, "cmd/main.go"),
    ]);
    edges.push(contains(350, 351));
    let import = make_ref(1, 31, 350, "example.com::shop::pkg", None, RefForm::Path, EdgeKind::Imports);
    let g = make_ref(2, 31, 351, "example.com::shop::pkg::G", None, RefForm::Path, EdgeKind::Calls);
    let f = make_ref(3, 31, 351, "example.com::shop::pkg::F", None, RefForm::Path, EdgeKind::Calls);
    let refs = [import.clone(), g.clone(), f.clone()];
    let go: std::collections::HashMap<String, std::collections::HashSet<String>> =
        std::iter::once(("go".to_string(), std::iter::once("go".to_string()).collect())).collect();
    let module = super::go_module::GoModule {
        root: String::new(),
        path: "example.com/shop".to_string(),
    };
    let ix = Index::build(&nodes, &edges, &refs)
        .with_path_specifiers(go, vec![module])
        .with_imported_bindings(&refs, BindingPolicy::Balanced);
    assert_eq!(
        bind(&import, &ix, BindingPolicy::Balanced),
        Outcome::BoundMany {
            source: NodeId(350),
            targets: vec![NodeId(330), NodeId(340)],
            kind: EdgeKind::Imports,
            payload: None,
        },
        "precondition: the import binds the whole package"
    );
    bound_to(bind(&g, &ix, BindingPolicy::Balanced), 351, 342, EdgeKind::Calls);
    assert_eq!(bind(&f, &ix, BindingPolicy::Aggressive), Outcome::Unbound);
}

#[test]
fn a_rust_qualified_call_never_reaches_the_imported_rung() {
    // The rung is keyed on the source file's grammar: a Rust `util::run` call
    // binds through the module tree exactly as it did before S-440, with the
    // imported scope built and path specifiers declared for `.ts`.
    let r = call(100, LIB_RS, 2, "util::run");
    let (nodes, edges) = fixture();
    let refs = [r.clone()];
    let ix = Index::build(&nodes, &edges, &refs)
        .with_path_specifiers(ts_specifiers(), Vec::new())
        .with_imported_bindings(&refs, BindingPolicy::Strict);
    bound_to(bind(&r, &ix, BindingPolicy::Strict), 2, 5, EdgeKind::Calls);
}

// ── S-493 / FR-RS-11: `self.m()` / `Self::m()` through the caller's self type ──
//
// Extraction records a `self.m()` in an impl method as the Path-form `Self::m`
// (the row a written `Self::m()` records) and the method's impl block; since
// S-607 (FR-RS-47) the binder reads `T` from the caller's own block header,
// resolved to one type node in the block's scope, and binds the row through the
// one associated-item lookup — exactly one or nothing. The crate rung and the
// module rung S-493 introduced are gone: a header names its own scope's type.

/// Two impl blocks in `src/lib.rs` — `A` (`run` 70, `helper` 71) and `B` (`run`
/// 72, `helper` 73) — plus a free `lone` (74, no self type); a second type
/// named `A` in module `util` (`helper` 75, `run` 79) and a third in the inline
/// `mod inner` (`helper` 78); and an `A` with a `helper` in crate `other` (76).
/// `B` is a type the crate declares once (struct 60): a second `B::helper` in
/// `util` (84) and a `B::run` in `inner` (86) exercise it.
fn self_type_fixture() -> (Vec<NodeRow>, Vec<EdgeRow>, Vec<(NodeId, String)>) {
    let (mut nodes, mut edges) = fixture();
    for (id, name, file, module) in [
        (70, "run", "src/lib.rs", 1),
        (71, "helper", "src/lib.rs", 1),
        (72, "run", "src/lib.rs", 1),
        (73, "helper", "src/lib.rs", 1),
        (74, "lone", "src/lib.rs", 1),
        (75, "helper", "src/util.rs", 4),
        (76, "helper", "other/src/lib.rs", 20),
        (78, "helper", "src/lib.rs", 6),
        (79, "run", "src/util.rs", 4),
        (84, "helper", "src/util.rs", 4),
        (86, "run", "src/lib.rs", 6),
        (60, "B", "src/lib.rs", 1),
        (61, "A", "src/lib.rs", 1),
        (62, "A", "src/util.rs", 4),
        (63, "A", "src/lib.rs", 6),
        (64, "A", "other/src/lib.rs", 20),
    ] {
        let kind = match id {
            74 => NodeKind::Function,
            60..=64 => NodeKind::Struct,
            _ => NodeKind::Method,
        };
        nodes.push(node(id, name, kind, file));
        edges.push(contains(module, id));
    }
    let self_types = [
        (70, "A"),
        (71, "A"),
        (72, "B"),
        (73, "B"),
        (75, "A"),
        (76, "A"),
        (78, "A"),
        (79, "A"),
        (84, "B"),
        (86, "B"),
    ]
        .into_iter()
        .map(|(id, ty)| (NodeId(id), ty.to_string()))
        .collect();
    (nodes, edges, self_types)
}

/// The fixture's index with the self types, minus the nodes in `drop`.
fn self_type_index(r: &UnresolvedRefRow, drop: &[i64]) -> Index {
    let (mut nodes, edges, mut self_types) = self_type_fixture();
    nodes.retain(|n| !drop.contains(&n.id.0));
    self_types.retain(|(id, _)| !drop.contains(&id.0));
    with_impl_blocks(Index::build(&nodes, &edges, std::slice::from_ref(r)).with_self_types(self_types.clone()), &nodes, &self_types, std::slice::from_ref(r))
}

#[test]
fn a_self_type_call_binds_to_the_callers_own_types_method() {
    for policy in [BindingPolicy::Strict, BindingPolicy::Balanced, BindingPolicy::Aggressive] {
        let from_a = call(100, LIB_RS, 70, "Self::helper");
        let from_b = call(101, LIB_RS, 72, "Self::helper");
        // The other modules' `A`s and `B::helper`s are dropped: one candidate
        // per type name.
        let drop = [75, 78, 84];
        bound_to(bind(&from_a, &self_type_index(&from_a, &drop), policy), 70, 71, EdgeKind::Calls);
        bound_to(bind(&from_b, &self_type_index(&from_b, &drop), policy), 72, 73, EdgeKind::Calls);
    }
    // The bare Method-form row the same call recorded before S-493 cannot tell
    // the two `helper`s of one module apart — why the receiver is recorded.
    let bare = make_ref(102, LIB_RS, 70, "helper", None, RefForm::Method, EdgeKind::Calls);
    assert_eq!(
        bind(&bare, &self_type_index(&bare, &[75, 78, 84]), BindingPolicy::Aggressive),
        Outcome::Unbound
    );
}

#[test]
fn same_named_types_of_two_modules_are_told_apart_by_the_callers_module() {
    // Three types named `A` in crate `crate` — in the root module, `util` and
    // `inner` — each defining `helper`: each header resolves to its own module's
    // `A`, so the call binds the `helper` of its own type.
    let from_root = call(100, LIB_RS, 70, "Self::helper");
    bound_to(bind(&from_root, &self_type_index(&from_root, &[]), BindingPolicy::Strict), 70, 71, EdgeKind::Calls);
    let from_util = call(101, UTIL_RS, 79, "Self::helper");
    bound_to(bind(&from_util, &self_type_index(&from_util, &[]), BindingPolicy::Strict), 79, 75, EdgeKind::Calls);
}

#[test]
fn a_self_type_call_with_two_or_no_candidates_stays_unbound_with_its_reason() {
    use super::binder::{residue, Residue};
    // Two: `util` imports the crate's `B` (`use crate::B`), so its impl's
    // `helper` (84) is `B`'s as well as `lib.rs`'s (73): two inherent
    // functions of one type, whichever module calls.
    let two = call(100, LIB_RS, 86, "Self::helper");
    let use_b = make_ref(90, UTIL_RS, 4, "crate::B", Some("B"), RefForm::Path, EdgeKind::Imports);
    let (nodes, edges, self_types) = self_type_fixture();
    let refs = [use_b, two.clone()];
    let ix = with_impl_blocks(Index::build(&nodes, &edges, &refs).with_self_types(self_types.clone()), &nodes, &self_types, &refs);
    assert_eq!(bind(&two, &ix, BindingPolicy::Aggressive), Outcome::Unbound);
    assert_eq!(residue(&two, &ix, BindingPolicy::Aggressive), Some(Residue::OverloadAmbiguous));
    // Without the import, `util`'s header names no type of its scope: the one
    // `B::helper` binds, from any module.
    let ix = self_type_index(&two, &[]);
    bound_to(bind(&two, &ix, BindingPolicy::Strict), 86, 73, EdgeKind::Calls);
    // None: `B` records no `lone` — the free `lone` beside it in the module is
    // no method of `B`, and the row never falls through to a scope walk.
    let none = call(101, LIB_RS, 72, "Self::lone");
    let ix = self_type_index(&none, &[]);
    assert_eq!(bind(&none, &ix, BindingPolicy::Aggressive), Outcome::Unbound);
    assert_eq!(residue(&none, &ix, BindingPolicy::Aggressive), Some(Residue::SupertypeUnreached));
}

#[test]
fn a_same_named_type_in_another_crate_is_never_a_candidate() {
    use super::binder::{residue, Residue};
    // Crate `crate`'s `A` without its own `helper`s: only crate `other`'s `A`
    // records one, and it is not the caller's type.
    let r = call(100, LIB_RS, 70, "Self::helper");
    let ix = self_type_index(&r, &[71, 75, 78]);
    assert_eq!(bind(&r, &ix, BindingPolicy::Aggressive), Outcome::Unbound);
    assert_eq!(residue(&r, &ix, BindingPolicy::Aggressive), Some(Residue::SupertypeUnreached));
}

#[test]
fn a_self_call_from_a_caller_in_no_impl_block_proves_no_receiver() {
    use super::binder::{residue, Residue};
    // `lone` is in no impl block and no trait (a free function): its
    // `Self::helper` proves no type, and never reaches the scope hierarchy
    // (S-607; a trait default body's call fans out instead, S-608).
    let r = call(100, LIB_RS, 74, "Self::helper");
    let ix = self_type_index(&r, &[75, 78, 84]);
    assert_eq!(bind(&r, &ix, BindingPolicy::Aggressive), Outcome::Unbound);
    assert_eq!(residue(&r, &ix, BindingPolicy::Aggressive), Some(Residue::NoReceiverEvidence));
    // And an index given no impl blocks binds no `Self::` call through one.
    let (nodes, edges, _) = self_type_fixture();
    let from_a = call(101, LIB_RS, 70, "Self::helper");
    let ix = Index::build(&nodes, &edges, std::slice::from_ref(&from_a));
    assert_eq!(bind(&from_a, &ix, BindingPolicy::Aggressive), Outcome::Unbound);
    assert_eq!(residue(&from_a, &ix, BindingPolicy::Aggressive), Some(Residue::NoReceiverEvidence));
}

#[test]
fn an_inherent_function_beats_a_trait_impls_in_any_module() {
    // `E` has an inherent `start` in `lib.rs` (82, called from 83) and a trait
    // impl's `start` in `util.rs` (81, its `Implements` row naming `Member`,
    // called from 80; `util` imports `E`). Rust resolves `Self::start` to the
    // inherent one even inside the trait impl (S-607: inherent beats trait in
    // any module — 1.13.0's module rung left 80's call `overload-ambiguous`).
    let (mut nodes, mut edges) = fixture();
    for (id, name, file, module) in [
        (80, "run", "src/util.rs", 4),
        (81, "start", "src/util.rs", 4),
        (82, "start", "src/lib.rs", 1),
        (83, "boot", "src/lib.rs", 1),
    ] {
        nodes.push(node(id, name, NodeKind::Method, file));
        edges.push(contains(module, id));
    }
    nodes.push(node(65, "E", NodeKind::Struct, "src/lib.rs"));
    edges.push(contains(1, 65));
    let self_types: Vec<(NodeId, String)> =
        [80, 81, 82, 83].into_iter().map(|id| (NodeId(id), "E".to_string())).collect();
    let implements = make_ref(90, UTIL_RS, 81, "Member", None, RefForm::Path, EdgeKind::Implements);
    let use_e = make_ref(91, UTIL_RS, 4, "crate::E", Some("E"), RefForm::Path, EdgeKind::Imports);
    let from_trait_impl = call(100, UTIL_RS, 80, "Self::start");
    let from_inherent = call(101, LIB_RS, 83, "Self::start");
    let refs = [implements, use_e, from_trait_impl.clone(), from_inherent.clone()];
    let ix = with_impl_blocks(Index::build(&nodes, &edges, &refs).with_self_types(self_types.clone()), &nodes, &self_types, &refs);

    bound_to(bind(&from_trait_impl, &ix, BindingPolicy::Strict), 80, 82, EdgeKind::Calls);
    bound_to(bind(&from_inherent, &ix, BindingPolicy::Strict), 83, 82, EdgeKind::Calls);
}

#[test]
fn a_header_names_its_own_scopes_type_never_a_same_named_one_elsewhere() {
    use super::binder::{residue, Residue};
    // `A` is declared in three modules of crate `crate`. A call on the root's
    // `A` — whose impl defines no `helper` here — never takes `util`'s, the
    // crate's only `A::helper`: `util`'s header names `util`'s `A` (1.13.0 read
    // the name crate-wide and called the pair `type-ambiguous`).
    let r = call(100, LIB_RS, 70, "Self::helper");
    let ix = self_type_index(&r, &[71, 78]);
    assert_eq!(bind(&r, &ix, BindingPolicy::Aggressive), Outcome::Unbound);
    assert_eq!(residue(&r, &ix, BindingPolicy::Aggressive), Some(Residue::SupertypeUnreached));
    // A header naming no type of its scope (an external `impl Trait for
    // Vec<T>`) contributes no candidate to any type.
    let (mut nodes, edges, self_types) = self_type_fixture();
    nodes.retain(|n| ![61, 62, 63, 71, 78].contains(&n.id.0));
    let ix = with_impl_blocks(Index::build(&nodes, &edges, std::slice::from_ref(&r)).with_self_types(self_types.clone()), &nodes, &self_types, std::slice::from_ref(&r));
    assert_eq!(bind(&r, &ix, BindingPolicy::Aggressive), Outcome::Unbound);
}

#[test]
fn a_self_type_the_crate_does_not_declare_or_imports_from_outside_binds_nothing() {
    use super::binder::{residue, Residue};
    let external = || Some(Residue::ExternalType { candidates: Vec::new() });
    // No type `A` declared in crate `crate` (a library type, a generic
    // parameter): the lone module-local `helper` may be shadowed by an inherent
    // method the graph cannot see.
    let r = call(100, LIB_RS, 70, "Self::helper");
    let (mut nodes, edges, self_types) = self_type_fixture();
    nodes.retain(|n| ![61, 62, 63, 75, 78].contains(&n.id.0));
    let ix = with_impl_blocks(Index::build(&nodes, &edges, std::slice::from_ref(&r)).with_self_types(self_types.clone()), &nodes, &self_types, std::slice::from_ref(&r));
    assert_eq!(bind(&r, &ix, BindingPolicy::Aggressive), Outcome::Unbound);
    assert_eq!(residue(&r, &ix, BindingPolicy::Aggressive), external());

    // `util.rs` imports a foreign `B` (`use std::x::B`): its `B::helper` (84) is
    // that type's, never a candidate for the crate's `B`, so the call from
    // `inner` binds the crate's one `B::helper` (73).
    let import = make_ref(90, UTIL_RS, 4, "std::x::B", Some("B"), RefForm::Path, EdgeKind::Imports);
    let from_inner = call(101, LIB_RS, 86, "Self::helper");
    let (nodes, edges, self_types) = self_type_fixture();
    let refs = [import.clone(), from_inner.clone()];
    let ix = with_impl_blocks(Index::build(&nodes, &edges, &refs).with_self_types(self_types.clone()), &nodes, &self_types, &refs);
    bound_to(bind(&from_inner, &ix, BindingPolicy::Strict), 86, 73, EdgeKind::Calls);
    // And a caller in a file importing its self type's name from outside the
    // crate — and declaring none of its own — binds nothing.
    let from_util = call(102, UTIL_RS, 79, "Self::helper");
    let lib_import = make_ref(91, UTIL_RS, 4, "std::x::A", Some("A"), RefForm::Path, EdgeKind::Imports);
    let refs = [lib_import, from_util.clone()];
    let nodes: Vec<NodeRow> = nodes.into_iter().filter(|n| n.id.0 != 62).collect();
    let ix = with_impl_blocks(Index::build(&nodes, &edges, &refs).with_self_types(self_types.clone()), &nodes, &self_types, &refs);
    assert_eq!(bind(&from_util, &ix, BindingPolicy::Strict), Outcome::Unbound);
    assert_eq!(residue(&from_util, &ix, BindingPolicy::Strict), external());
}

// ── S-588 / FR-RS-42: a call on a proven Rust receiver binds among its type's methods ──
//
// S-587 retypes `x.m()` on a proven receiver to the Path-form `T::m` of shape
// `other`, with the wrappers peeled to reach `T`. The binder reads `T` through
// the caller file's `use` declarations to one in-repository type, then binds
// exactly one of that type's methods (S-493's universe), inherent first.

/// The receiver fixture's nodes, edges, self types and `Implements` rows.
type ReceiverFixture = (Vec<NodeRow>, Vec<EdgeRow>, Vec<(NodeId, String)>, Vec<UnresolvedRefRow>);

/// `Store` (400) and `Other` (401) in `src/util.rs`, both defining `get` (402,
/// 403). `Store` has an inherent `put` (405) beside a trait impl's (404), a
/// `len` (406), a trait impl's `clone` (407) and two trait impls' `close` (408,
/// 409). Crate `other` declares its own `Store` (410) with a `get` (411). The
/// caller is `alpha` (2) in `src/lib.rs`, which imports nothing yet.
fn receiver_fixture() -> ReceiverFixture {
    let (mut nodes, mut edges) = fixture();
    let mut self_types = Vec::new();
    for (id, name, kind, file, module, self_type) in [
        (400, "Store", NodeKind::Struct, "src/util.rs", 4, None),
        (401, "Other", NodeKind::Struct, "src/util.rs", 4, None),
        (402, "get", NodeKind::Method, "src/util.rs", 4, Some("Store")),
        (403, "get", NodeKind::Method, "src/util.rs", 4, Some("Other")),
        (404, "put", NodeKind::Method, "src/util.rs", 4, Some("Store")),
        (405, "put", NodeKind::Method, "src/util.rs", 4, Some("Store")),
        (406, "len", NodeKind::Method, "src/util.rs", 4, Some("Store")),
        (407, "clone", NodeKind::Method, "src/util.rs", 4, Some("Store")),
        (408, "close", NodeKind::Method, "src/util.rs", 4, Some("Store")),
        (409, "close", NodeKind::Method, "src/util.rs", 4, Some("Store")),
        (410, "Store", NodeKind::Struct, "other/src/lib.rs", 20, None),
        (411, "get", NodeKind::Method, "other/src/lib.rs", 20, Some("Store")),
    ] {
        nodes.push(node(id, name, kind, file));
        edges.push(contains(module, id));
        if let Some(ty) = self_type {
            self_types.push((NodeId(id), ty.to_string()));
        }
    }
    let implements = [(404, "Tr"), (407, "Clone"), (408, "Open"), (409, "Shut")]
        .into_iter()
        .enumerate()
        .map(|(i, (id, tr))| make_ref(300 + i as i64, UTIL_RS, id, tr, None, RefForm::Path, EdgeKind::Implements))
        .collect();
    (nodes, edges, self_types, implements)
}

/// A call `x.<target's method>()` S-587 retyped from a proven receiver, made
/// from `alpha` in `src/lib.rs`.
fn proven(id: i64, target: &str, peeled: Option<&str>) -> UnresolvedRefRow {
    UnresolvedRefRow {
        receiver: Some(ReceiverShape::Other),
        peeled: peeled.map(str::to_string),
        ..call(id, LIB_RS, 2, target)
    }
}

/// An `Imports` row of `src/lib.rs`: `use <path> as <alias>`.
fn lib_use(id: i64, path: &str, alias: &str) -> UnresolvedRefRow {
    make_ref(id, LIB_RS, 1, path, Some(alias), RefForm::Path, EdgeKind::Imports)
}

/// A `use` glob of `src/lib.rs`: `use <path>::*`.
fn lib_glob(id: i64, path: &str) -> UnresolvedRefRow {
    make_ref(id, LIB_RS, 1, path, None, RefForm::Glob, EdgeKind::Imports)
}

/// The receiver fixture's index over `r` and `extra`, minus the nodes in
/// `drop`, keyed by `layout`.
fn receiver_index_with(
    r: &UnresolvedRefRow,
    extra: &[UnresolvedRefRow],
    drop: &[i64],
    layout: PackageLayout,
) -> Index {
    let (mut nodes, edges, mut self_types, mut refs) = receiver_fixture();
    nodes.retain(|n| !drop.contains(&n.id.0));
    self_types.retain(|(id, _)| !drop.contains(&id.0));
    refs.extend(extra.iter().cloned());
    refs.push(r.clone());
    with_impl_blocks(Index::build_with_layout(&nodes, &edges, &refs, layout).with_self_types(self_types.clone()), &nodes, &self_types, &refs)
}

/// [`receiver_index_with`] under the layout the Rust plugin declares: its
/// stems, and `Arc`/`Box` providing `clone` themselves.
fn receiver_index(r: &UnresolvedRefRow, extra: &[UnresolvedRefRow], drop: &[i64]) -> Index {
    let wrappers = std::collections::BTreeMap::from([
        ("Arc".to_string(), vec!["clone".to_string(), "downgrade".to_string()]),
        ("Box".to_string(), vec!["clone".to_string()]),
    ]);
    let layout = PackageLayout::rust_stems_for_tests()
        .with_wrapper_methods(std::collections::HashMap::from([("rs".to_string(), wrappers)]));
    receiver_index_with(r, extra, drop, layout)
}

#[test]
fn a_proven_receiver_binds_its_own_types_method_and_never_a_same_named_one() {
    // `Store` and `Other` both define `get`; each proven receiver binds its own
    // type's, at every tier — and the same rows as written-out Method rows bind
    // nothing (S-514), which is why the proof is recorded.
    let imports = [lib_use(90, "crate::util::Store", "Store"), lib_use(91, "crate::util::Other", "Other")];
    for policy in POLICIES {
        let store = proven(100, "Store::get", None);
        bound_to(bind(&store, &receiver_index(&store, &imports, &[]), policy), 2, 402, EdgeKind::Calls);
        let other = proven(101, "Other::get", Some("&"));
        bound_to(bind(&other, &receiver_index(&other, &imports, &[]), policy), 2, 403, EdgeKind::Calls);
    }
    // A path the file writes out names the type without a `use`, in its own
    // crate and in another in-repository crate.
    let qualified = proven(102, "crate::util::Store::get", None);
    bound_to(bind(&qualified, &receiver_index(&qualified, &[], &[]), BindingPolicy::Strict), 2, 402, EdgeKind::Calls);
    let cross_crate = proven(103, "other::Store::get", None);
    bound_to(bind(&cross_crate, &receiver_index(&cross_crate, &[], &[]), BindingPolicy::Strict), 2, 411, EdgeKind::Calls);
    // …and so does a `use` of the other crate's type.
    let imported = proven(104, "Store::get", None);
    let ix = receiver_index(&imported, &[lib_use(92, "other::Store", "Store")], &[]);
    bound_to(bind(&imported, &ix, BindingPolicy::Strict), 2, 411, EdgeKind::Calls);
}

#[test]
fn a_proven_receivers_type_is_read_through_the_files_use_alone() {
    let external = Some(Residue::ExternalType { candidates: Vec::new() });
    // No `use` names `Store`: the crate's `util::Store` and crate `other`'s
    // are both same-named types the file never imports — neither is a
    // candidate, not even through the aggressive workspace fallback, which
    // would pick the caller's crate's one.
    let r = proven(100, "Store::get", None);
    let ix = receiver_index(&r, &[], &[]);
    for policy in POLICIES {
        assert_eq!(bind(&r, &ix, policy), Outcome::Unbound, "{policy:?}");
    }
    assert_eq!(residue(&r, &ix, BindingPolicy::Aggressive), external);
    // A `use` of a type outside the repository decides alone: the glob that
    // brings the crate's `Store` into view does not override it, as Rust's
    // explicit import shadows a glob.
    let ix = receiver_index(&r, &[lib_use(90, "std::x::Store", "Store"), lib_glob(91, "crate::util")], &[]);
    assert_eq!(bind(&r, &ix, BindingPolicy::Aggressive), Outcome::Unbound);
    assert_eq!(residue(&r, &ix, BindingPolicy::Aggressive), external);
    // The glob alone brings it into view: a glob is an import.
    let ix = receiver_index(&r, &[lib_glob(91, "crate::util")], &[]);
    bound_to(bind(&r, &ix, BindingPolicy::Strict), 2, 402, EdgeKind::Calls);
    // Two globs each bringing a `Store`: the file names two types.
    let ix = receiver_index(&r, &[lib_glob(91, "crate::util"), lib_glob(92, "other")], &[]);
    assert_eq!(bind(&r, &ix, BindingPolicy::Aggressive), Outcome::Unbound);
    assert_eq!(residue(&r, &ix, BindingPolicy::Aggressive), Some(Residue::TypeAmbiguous));
}

#[test]
fn a_type_another_crate_re_exports_is_followed_to_its_declaration() {
    // Crate `other` declares `Engine` (414, `start` 415) in `other/src/engine.rs`
    // (module 25) and re-exports it from its root: `pub use engine::Engine;`.
    // `use other::Engine` names that declaration, which no member of the root
    // module is.
    let (mut nodes, mut edges, mut self_types, implements) = receiver_fixture();
    nodes.extend([
        node(25, "engine", NodeKind::Module, "other/src/engine.rs"),
        node(414, "Engine", NodeKind::Struct, "other/src/engine.rs"),
        node(415, "start", NodeKind::Method, "other/src/engine.rs"),
    ]);
    edges.extend([contains(25, 414), contains(25, 415)]);
    self_types.push((NodeId(415), "Engine".to_string()));
    let index = |r: &UnresolvedRefRow, extra: &[UnresolvedRefRow]| {
        let mut refs = implements.clone();
        refs.extend(extra.iter().cloned());
        refs.push(r.clone());
        with_impl_blocks(Index::build(&nodes, &edges, &refs).with_self_types(self_types.clone()), &nodes, &self_types, &refs)
    };
    let reexport = |path: &str| make_ref(95, OTHER_LIB_RS, 20, path, Some("Engine"), RefForm::Path, EdgeKind::Imports);
    let r = proven(100, "Engine::start", None);
    let import = lib_use(90, "other::Engine", "Engine");
    for written in ["engine::Engine", "self::engine::Engine", "crate::engine::Engine"] {
        let ix = index(&r, &[import.clone(), reexport(written)]);
        bound_to(bind(&r, &ix, BindingPolicy::Strict), 2, 415, EdgeKind::Calls);
    }
    // Written out in full, the path takes the same re-export.
    let qualified = proven(101, "other::Engine::start", None);
    bound_to(bind(&qualified, &index(&qualified, &[reexport("engine::Engine")]), BindingPolicy::Strict), 2, 415, EdgeKind::Calls);
    // No re-export: the root declares no `Engine`, so the import names none.
    let ix = index(&r, std::slice::from_ref(&import));
    assert_eq!(bind(&r, &ix, BindingPolicy::Aggressive), Outcome::Unbound);
    assert_eq!(residue(&r, &ix, BindingPolicy::Aggressive), Some(Residue::ExternalType { candidates: Vec::new() }));
    // A glob of the re-exporting crate's root reads none of its imports (only
    // an ancestor's glob does: the graph cannot tell a `pub use` from a
    // private one), while a `crate::` path written inside that crate does.
    let ix = index(&r, &[lib_glob(91, "other"), reexport("engine::Engine")]);
    assert_eq!(bind(&r, &ix, BindingPolicy::Aggressive), Outcome::Unbound);
    let own_crate = UnresolvedRefRow {
        receiver: Some(ReceiverShape::Other),
        ..call(102, OTHER_LIB_RS, 21, "crate::Engine::start")
    };
    let ix = index(&own_crate, &[reexport("engine::Engine")]);
    bound_to(bind(&own_crate, &ix, BindingPolicy::Strict), 21, 415, EdgeKind::Calls);
    // A re-export of something the repository does not declare is external too,
    // and a re-export naming itself ends.
    for written in ["std::x::Engine", "Engine"] {
        let ix = index(&r, &[import.clone(), reexport(written)]);
        assert_eq!(bind(&r, &ix, BindingPolicy::Aggressive), Outcome::Unbound, "{written}");
    }
}

/// Crate `other` declares `Deep` (431, `run` 432) in `other/src/deep.rs`
/// (module 30). `mid.rs` (module 31) and `mid/sub.rs` (module 33) sit in that
/// crate, and a crate `facade` (module 35) beside it; `reexports` gives each
/// file's `use` of `Deep` as `(file id, source module, path)`, and `imports`
/// the caller's own.
fn reexport_chain_index(r: &UnresolvedRefRow, reexports: &[(i64, i64, &str)], imports: &[UnresolvedRefRow]) -> Index {
    let (mut nodes, mut edges, mut self_types, mut refs) = receiver_fixture();
    nodes.extend([
        node(30, "deep", NodeKind::Module, "other/src/deep.rs"),
        node(31, "mid", NodeKind::Module, "other/src/mid.rs"),
        node(33, "sub", NodeKind::Module, "other/src/mid/sub.rs"),
        node(35, "facade", NodeKind::Module, "facade/src/lib.rs"),
        node(431, "Deep", NodeKind::Struct, "other/src/deep.rs"),
        node(432, "run", NodeKind::Method, "other/src/deep.rs"),
    ]);
    edges.extend([contains(30, 431), contains(30, 432)]);
    self_types.push((NodeId(432), "Deep".to_string()));
    for (i, (file, module, path)) in reexports.iter().enumerate() {
        refs.push(make_ref(60 + i as i64, *file, *module, path, Some("Deep"), RefForm::Path, EdgeKind::Imports));
    }
    refs.extend(imports.iter().cloned());
    refs.push(r.clone());
    with_impl_blocks(Index::build(&nodes, &edges, &refs).with_self_types(self_types.clone()), &nodes, &self_types, &refs)
}

#[test]
fn a_re_export_chain_of_two_hops_is_followed_and_a_cycle_ends() {
    // `other`'s root re-exports `mid::Deep`, which `mid` re-exports from
    // `deep`: two hops to the declaration.
    let r = proven(100, "Deep::run", None);
    let import = [lib_use(90, "other::Deep", "Deep")];
    let ix = reexport_chain_index(&r, &[(OTHER_LIB_RS, 20, "mid::Deep"), (40, 31, "crate::deep::Deep")], &import);
    bound_to(bind(&r, &ix, BindingPolicy::Strict), 2, 432, EdgeKind::Calls);
    // Two modules re-exporting each other end as not found: external.
    let ix = reexport_chain_index(&r, &[(OTHER_LIB_RS, 20, "mid::Deep"), (40, 31, "crate::Deep")], &import);
    assert_eq!(bind(&r, &ix, BindingPolicy::Aggressive), Outcome::Unbound);
    assert_eq!(residue(&r, &ix, BindingPolicy::Aggressive), Some(Residue::ExternalType { candidates: Vec::new() }));
}

#[test]
fn a_facade_crate_re_exporting_another_crates_type_is_followed() {
    // `facade`'s root: `pub use other::deep::Deep;` — an anchor on another
    // crate's name.
    let r = proven(100, "Deep::run", None);
    let ix = reexport_chain_index(&r, &[(41, 35, "other::deep::Deep")], &[lib_use(90, "facade::Deep", "Deep")]);
    bound_to(bind(&r, &ix, BindingPolicy::Strict), 2, 432, EdgeKind::Calls);
}

#[test]
fn a_super_re_export_is_followed() {
    // `other::mid::sub` re-exports `super::super::deep::Deep`; the caller
    // writes the path through `sub`.
    let r = proven(100, "other::mid::sub::Deep::run", None);
    let ix = reexport_chain_index(&r, &[(42, 33, "super::super::deep::Deep")], &[]);
    bound_to(bind(&r, &ix, BindingPolicy::Strict), 2, 432, EdgeKind::Calls);
}

#[test]
fn an_inherent_method_outranks_a_trait_impls_and_two_of_one_rank_bind_nothing() {
    let store = [lib_use(90, "crate::util::Store", "Store")];
    let put = proven(100, "Store::put", None);
    bound_to(bind(&put, &receiver_index(&put, &store, &[]), BindingPolicy::Strict), 2, 405, EdgeKind::Calls);
    // With no inherent `put`, the trait impl's is the type's one `put`.
    bound_to(bind(&put, &receiver_index(&put, &store, &[405]), BindingPolicy::Strict), 2, 404, EdgeKind::Calls);
    // Two trait impls' `close`, no inherent one: Rust would make the caller
    // say which.
    let close = proven(101, "Store::close", None);
    let ix = receiver_index(&close, &store, &[]);
    assert_eq!(bind(&close, &ix, BindingPolicy::Aggressive), Outcome::Unbound);
    assert_eq!(residue(&close, &ix, BindingPolicy::Aggressive), Some(Residue::OverloadAmbiguous));
    // None: the type records no `missing` — a derive supplies it — and the
    // free `helper` beside the caller is no method of it.
    for target in ["Store::missing", "Store::helper"] {
        let none = proven(102, target, None);
        let ix = receiver_index(&none, &store, &[]);
        assert_eq!(bind(&none, &ix, BindingPolicy::Aggressive), Outcome::Unbound, "{target}");
        assert_eq!(residue(&none, &ix, BindingPolicy::Aggressive), Some(Residue::SupertypeUnreached), "{target}");
    }
}

#[test]
fn a_method_the_peeled_wrapper_provides_binds_nothing() {
    let external = Some(Residue::ExternalType { candidates: Vec::new() });
    let store = [lib_use(90, "crate::util::Store", "Store")];
    // `Store` implements `clone` (407). Through `Arc`, `Box` or a reference to
    // either, `x.clone()` is the wrapper's own method.
    for peeled in ["Arc", "Box", "& Arc", "Arc &"] {
        let r = proven(100, "Store::clone", Some(peeled));
        let ix = receiver_index(&r, &store, &[]);
        assert_eq!(bind(&r, &ix, BindingPolicy::Aggressive), Outcome::Unbound, "{peeled}");
        assert_eq!(residue(&r, &ix, BindingPolicy::Aggressive), external, "{peeled}");
    }
    // A reference provides nothing: `x.clone()` on `&Store` is `Store`'s.
    for peeled in [None, Some("&"), Some("&mut")] {
        let r = proven(101, "Store::clone", peeled);
        bound_to(bind(&r, &receiver_index(&r, &store, &[]), BindingPolicy::Strict), 2, 407, EdgeKind::Calls);
    }
    // A method the wrapper does not provide reaches `T`'s through it.
    let get = proven(102, "Store::get", Some("Arc"));
    bound_to(bind(&get, &receiver_index(&get, &store, &[]), BindingPolicy::Strict), 2, 402, EdgeKind::Calls);
    // The list is the plugin's declaration, not the binder's: a layout that
    // declares none lets `Arc`'s `clone` through.
    let r = proven(103, "Store::clone", Some("Arc"));
    let ix = receiver_index_with(&r, &store, &[], PackageLayout::rust_stems_for_tests());
    bound_to(bind(&r, &ix, BindingPolicy::Strict), 2, 407, EdgeKind::Calls);
}

#[test]
fn a_receiver_type_the_repository_does_not_declare_never_binds() {
    // `Store` defines `len` and `get`; a `String`, `Vec<_>` or `std::io::Error`
    // receiver — imported or not — never reaches them, at any tier.
    let imports = [lib_use(90, "crate::util::Store", "Store"), lib_use(91, "std::io", "io")];
    for target in ["String::len", "Vec::len", "std::io::Error::get", "io::Error::get", "Option::get"] {
        let r = proven(100, target, None);
        let ix = receiver_index(&r, &imports, &[]);
        for policy in POLICIES {
            assert_eq!(bind(&r, &ix, policy), Outcome::Unbound, "{target} at {policy:?}");
        }
        assert_eq!(
            residue(&r, &ix, BindingPolicy::Aggressive),
            Some(Residue::ExternalType { candidates: Vec::new() }),
            "{target}"
        );
    }
}

#[test]
fn a_type_declared_once_binds_a_method_whose_impl_sits_in_another_module() {
    // `Store` is declared once in crate `crate` (in `util`); an impl block in
    // `lib.rs` adds `scan` (416). Its name denotes the one `Store`, so the
    // method binds wherever its impl sits.
    let (mut nodes, mut edges, mut self_types, implements) = receiver_fixture();
    nodes.push(node(416, "scan", NodeKind::Method, "src/lib.rs"));
    edges.push(contains(1, 416));
    self_types.push((NodeId(416), "Store".to_string()));
    let r = proven(100, "Store::scan", None);
    let mut refs = implements;
    refs.extend([lib_use(90, "crate::util::Store", "Store"), r.clone()]);
    let ix = with_impl_blocks(Index::build(&nodes, &edges, &refs).with_self_types(self_types.clone()), &nodes, &self_types, &refs);
    bound_to(bind(&r, &ix, BindingPolicy::Strict), 2, 416, EdgeKind::Calls);
}

#[test]
fn a_same_named_type_of_another_module_is_told_apart_by_its_own_module() {
    // A second `Store` in `mod inner` (412) with its own `get` (413): each
    // impl header names its own module's `Store`, so a `get` binds only among
    // the proven type's own impls.
    let (mut nodes, mut edges, mut self_types, implements) = receiver_fixture();
    nodes.extend([node(412, "Store", NodeKind::Struct, "src/lib.rs"), node(413, "get", NodeKind::Method, "src/lib.rs")]);
    edges.extend([contains(6, 412), contains(6, 413)]);
    self_types.push((NodeId(413), "Store".to_string()));
    let index = |r: &UnresolvedRefRow, extra: &[UnresolvedRefRow], drop: &[i64]| {
        let mut refs = implements.clone();
        refs.extend(extra.iter().cloned());
        refs.push(r.clone());
        let nodes: Vec<NodeRow> = nodes.iter().filter(|n| !drop.contains(&n.id.0)).cloned().collect();
        with_impl_blocks(Index::build(&nodes, &edges, &refs).with_self_types(self_types.clone()), &nodes, &self_types, &refs)
    };
    let r = proven(100, "Store::get", None);
    bound_to(bind(&r, &index(&r, &[lib_use(90, "crate::util::Store", "Store")], &[]), BindingPolicy::Strict), 2, 402, EdgeKind::Calls);
    bound_to(bind(&r, &index(&r, &[lib_use(90, "crate::inner::Store", "Store")], &[]), BindingPolicy::Strict), 2, 413, EdgeKind::Calls);
    // `inner`'s `Store` with no `get` of its own: `util`'s lone `get` is the
    // other type's, so nothing binds — the type has no such method (1.13.0
    // could not tell whose it was: `type-ambiguous`).
    let ix = index(&r, &[lib_use(90, "crate::inner::Store", "Store")], &[413]);
    assert_eq!(bind(&r, &ix, BindingPolicy::Aggressive), Outcome::Unbound);
    assert_eq!(residue(&r, &ix, BindingPolicy::Aggressive), Some(Residue::SupertypeUnreached));
}

// ── S-604 / CR-200: a Rust method call binds only a callable that takes `self` ──
//
// S-591 records whether each Rust `impl` function takes `self`
// (`nodes.takes_self`). A method-syntax call — a proven receiver's, a
// `self.m()` — drops every candidate recorded as not taking it before the
// inherent-over-trait rank; an unknown fact keeps the candidate. A path call
// (`Self::m()`, `T::m()`) keeps it (S-607: syntax decides the filter).

/// The receiver fixture plus `Store` methods in `src/util.rs`, each with its
/// takes-`self` fact: an inherent associated `name()` (420) beside a trait
/// impl's `name(&self)` (421); two genuine `pair`s, inherent (422) and a trait
/// impl's (423); an associated `make()` (424) alone; an inherent `vague` whose
/// fact is unknown (425) beside a trait impl's `vague(&self)` (426); an
/// inherent method `caller(&self)` (427) for `Self::m` calls; and two trait
/// impls' `build`, `Build`'s associated `fn build() -> Self` (428) beside
/// `Use`'s `build(&self)` (429). `facts` is whether the index is given the
/// facts at all.
fn self_fact_index(r: &UnresolvedRefRow, extra: &[UnresolvedRefRow], facts: bool) -> Index {
    let (mut nodes, mut edges, mut self_types, mut refs) = receiver_fixture();
    let mut arities: Vec<NodeArity> = Vec::new();
    for (id, name, takes_self) in [
        (420, "name", Some(false)),
        (421, "name", Some(true)),
        (422, "pair", Some(true)),
        (423, "pair", Some(true)),
        (424, "make", Some(false)),
        (425, "vague", None),
        (426, "vague", Some(true)),
        (427, "caller", Some(true)),
        (428, "build", Some(false)),
        (429, "build", Some(true)),
    ] {
        nodes.push(node(id, name, NodeKind::Method, "src/util.rs"));
        edges.push(contains(4, id));
        self_types.push((NodeId(id), "Store".to_string()));
        // The store's shape: every callable carries its range, so a node whose
        // takes-`self` fact is unknown is still a row.
        arities.push((NodeId(id), Some(ParamRange { min: 0, max: Some(0) }), takes_self));
    }
    refs.extend([(421, "Named"), (423, "Pair"), (426, "Vague"), (428, "Build"), (429, "Use")].into_iter().enumerate().map(|(i, (id, tr))| {
        make_ref(310 + i as i64, UTIL_RS, id, tr, None, RefForm::Path, EdgeKind::Implements)
    }));
    refs.extend(extra.iter().cloned());
    refs.push(r.clone());
    let ix = with_impl_blocks(Index::build(&nodes, &edges, &refs).with_self_types(self_types.clone()), &nodes, &self_types, &refs);
    if facts {
        ix.with_arities(&arities)
    } else {
        ix
    }
}

#[test]
fn a_method_call_binds_the_trait_method_over_an_inherent_associated_function() {
    // `impl Store { fn name() }` + `impl Named for Store { fn name(&self) }`:
    // rustc calls the trait method for `x.name()`. Without the facts the
    // inherent associated function outranks it — the pre-S-604 edge.
    let store = [lib_use(90, "crate::util::Store", "Store")];
    for policy in POLICIES {
        for (target, peeled) in [("Store::name", None), ("Store::name", Some("&")), ("Store::name", Some("Arc")), ("crate::util::Store::name", None)] {
            let r = proven(100, target, peeled);
            bound_to(bind(&r, &self_fact_index(&r, &store, true), policy), 2, 421, EdgeKind::Calls);
        }
    }
    let r = proven(100, "Store::name", None);
    bound_to(bind(&r, &self_fact_index(&r, &store, false), BindingPolicy::Strict), 2, 420, EdgeKind::Calls);
}

#[test]
fn a_trait_impls_associated_function_is_no_candidate_either() {
    // Two trait impls' `build`, one an associated function: without the facts
    // they are two of one rank (`overload-ambiguous`); with them, the one that
    // takes `self` is the call's. The filter is not inherent-only.
    let store = [lib_use(90, "crate::util::Store", "Store")];
    let r = proven(100, "Store::build", None);
    for policy in POLICIES {
        bound_to(bind(&r, &self_fact_index(&r, &store, true), policy), 2, 429, EdgeKind::Calls);
    }
    let ix = self_fact_index(&r, &store, false);
    assert_eq!(bind(&r, &ix, BindingPolicy::Aggressive), Outcome::Unbound);
    assert_eq!(residue(&r, &ix, BindingPolicy::Aggressive), Some(Residue::OverloadAmbiguous));
}

#[test]
fn two_genuine_methods_still_bind_the_inherent_one() {
    let store = [lib_use(90, "crate::util::Store", "Store")];
    for policy in POLICIES {
        for (target, peeled) in [("Store::pair", None), ("Store::pair", Some("&mut")), ("Store::pair", Some("Box")), ("crate::util::Store::pair", None)] {
            let r = proven(100, target, peeled);
            bound_to(bind(&r, &self_fact_index(&r, &store, true), policy), 2, 422, EdgeKind::Calls);
        }
    }
    // Through a glob as well as a `use`.
    let r = proven(101, "Store::pair", None);
    bound_to(bind(&r, &self_fact_index(&r, &[lib_glob(91, "crate::util")], true), BindingPolicy::Strict), 2, 422, EdgeKind::Calls);
}

#[test]
fn a_type_whose_only_m_takes_no_self_leaves_the_method_call_unbound() {
    let store = [lib_use(90, "crate::util::Store", "Store")];
    let r = proven(100, "Store::make", None);
    let ix = self_fact_index(&r, &store, true);
    for policy in POLICIES {
        assert_eq!(bind(&r, &ix, policy), Outcome::Unbound, "{policy:?}");
    }
    assert_eq!(residue(&r, &ix, BindingPolicy::Aggressive), Some(Residue::SupertypeUnreached));
    // Without the fact it is the type's one `make`.
    bound_to(bind(&r, &self_fact_index(&r, &store, false), BindingPolicy::Strict), 2, 424, EdgeKind::Calls);
}

#[test]
fn a_candidate_without_self_elsewhere_leaves_nothing_to_tell_apart() {
    // A second `Store` in `mod inner` (412) with no `get`: `util`'s lone `get`
    // (402) is `util`'s `Store`'s (S-607), so `inner`'s type has no such
    // method — and recorded as taking no `self`, it would be no candidate for
    // `x.get()` either way.
    let (mut nodes, mut edges, self_types, implements) = receiver_fixture();
    nodes.push(node(412, "Store", NodeKind::Struct, "src/lib.rs"));
    edges.push(contains(6, 412));
    let r = proven(100, "Store::get", None);
    let mut refs = implements;
    refs.extend([lib_use(90, "crate::inner::Store", "Store"), r.clone()]);
    let ix = with_impl_blocks(Index::build(&nodes, &edges, &refs).with_self_types(self_types.clone()), &nodes, &self_types, &refs);
    assert_eq!(residue(&r, &ix, BindingPolicy::Aggressive), Some(Residue::SupertypeUnreached));
    let ix = ix.with_arities(&[(NodeId(402), Some(ParamRange { min: 0, max: Some(0) }), Some(false))]);
    for policy in POLICIES {
        assert_eq!(bind(&r, &ix, policy), Outcome::Unbound, "{policy:?}");
    }
    assert_eq!(residue(&r, &ix, BindingPolicy::Aggressive), Some(Residue::SupertypeUnreached));
}

#[test]
fn an_unknown_takes_self_fact_never_filters_a_candidate() {
    // `vague` (425) records no fact: it stays a candidate, and as the inherent
    // one it outranks the trait impl's `vague(&self)`, exactly as before.
    let store = [lib_use(90, "crate::util::Store", "Store")];
    for policy in POLICIES {
        let r = proven(100, "Store::vague", None);
        bound_to(bind(&r, &self_fact_index(&r, &store, true), policy), 2, 425, EdgeKind::Calls);
    }
}

#[test]
fn an_unknown_fact_never_filters_a_path_call() {
    // FR-RS-47 rule 2: `vague` (425) records no takes-`self` fact, so a path
    // call passing one argument keeps it — neither counted as a receiver nor
    // dropped — and as the inherent one it binds. With no facts at all, no
    // range filters: `make` (424) takes any count.
    let store = [lib_use(90, "crate::util::Store", "Store")];
    let counted = |target: &str, args: u32| UnresolvedRefRow {
        arg_count: Some(args),
        ..call(100, LIB_RS, 2, target)
    };
    for policy in POLICIES {
        let r = counted("Store::vague", 1);
        bound_to(bind(&r, &self_fact_index(&r, &store, true), policy), 2, 425, EdgeKind::Calls);
        let r = counted("Store::make", 5);
        bound_to(bind(&r, &self_fact_index(&r, &store, false), policy), 2, 424, EdgeKind::Calls);
    }
}

#[test]
fn a_self_call_reads_the_takes_self_fact_by_its_syntax() {
    // From `caller` (427), S-607: a written `Self::m()` is path syntax and keeps
    // an associated function; a `self.m()` — the `Self::m` row of shape `self`
    // extraction records, or a Method-form row extracted without that rewrite —
    // is method syntax and drops it. 1.13.0's `Self::m` arm read no fact: the
    // two `name`s were `overload-ambiguous` whatever the syntax.
    let path = |name: &str| call(100, UTIL_RS, 427, &format!("Self::{name}"));
    let rewritten = |name: &str| UnresolvedRefRow {
        receiver: Some(ReceiverShape::SelfInstance),
        ..path(name)
    };
    let unrewritten = |name: &str| UnresolvedRefRow {
        receiver: Some(ReceiverShape::SelfInstance),
        ..make_ref(100, UTIL_RS, 427, name, None, RefForm::Method, EdgeKind::Calls)
    };
    for policy in POLICIES {
        let r = path("name");
        bound_to(bind(&r, &self_fact_index(&r, &[], true), policy), 427, 420, EdgeKind::Calls);
        let r = path("make");
        bound_to(bind(&r, &self_fact_index(&r, &[], true), policy), 427, 424, EdgeKind::Calls);
        for r in [rewritten("name"), unrewritten("name")] {
            bound_to(bind(&r, &self_fact_index(&r, &[], true), policy), 427, 421, EdgeKind::Calls);
            // Without the facts nothing is dropped: the inherent one outranks.
            bound_to(bind(&r, &self_fact_index(&r, &[], false), policy), 427, 420, EdgeKind::Calls);
        }
        for r in [rewritten("make"), unrewritten("make")] {
            let ix = self_fact_index(&r, &[], true);
            assert_eq!(bind(&r, &ix, policy), Outcome::Unbound, "{policy:?}");
            assert_eq!(residue(&r, &ix, policy), Some(Residue::SupertypeUnreached));
        }
    }
}

// ── S-514 / FR-RS-12: a receiver call binds by its receiver's shape ─────────
//
// Extraction records each Method-form call's receiver shape (`self`, `super`,
// `other`, or none); the binder dispatches on it. `self` binds among the
// caller's own class's members, then up its proven `Extends`; `super` only up
// that chain; `other` and none never bind through the caller's scope.

/// The ledger file of the class-nested fixture.
const PY_FILE: i64 = 20;

/// A Method-form `Calls` row carrying `receiver`.
fn shaped(
    id: i64,
    file_id: i64,
    source_node: i64,
    target: &str,
    receiver: Option<ReceiverShape>,
) -> UnresolvedRefRow {
    UnresolvedRefRow {
        receiver,
        peeled: None,
        ..make_ref(id, file_id, source_node, target, None, RefForm::Method, EdgeKind::Calls)
    }
}

/// A class-nested language's module, as a Python or TypeScript file indexes:
///
/// ```text
/// src/a.py (module 200)
/// ├── fn m          (201)   a module-level function named like the methods
/// ├── class A (202) ─ m (203), n (204)
/// ├── class B (206) ─ m (207)
/// ├── class C (208) ─ n (209)            no `m` of its own
/// ├── class D (210) ─ o (211), o (212), n (213)
/// └── fn free_caller (214)                in no class
/// ```
fn shape_fixture() -> (Vec<NodeRow>, Vec<EdgeRow>) {
    let file = "src/a.py";
    let mut nodes = vec![node(200, "a", NodeKind::Module, file)];
    let mut edges = Vec::new();
    for (id, name, kind, parent) in [
        (201, "m", NodeKind::Function, 200),
        (202, "A", NodeKind::Class, 200),
        (203, "m", NodeKind::Method, 202),
        (204, "n", NodeKind::Method, 202),
        (206, "B", NodeKind::Class, 200),
        (207, "m", NodeKind::Method, 206),
        (208, "C", NodeKind::Class, 200),
        (209, "n", NodeKind::Method, 208),
        (210, "D", NodeKind::Class, 200),
        (211, "o", NodeKind::Method, 210),
        (212, "o", NodeKind::Method, 210),
        (213, "n", NodeKind::Method, 210),
        (214, "free_caller", NodeKind::Function, 200),
    ] {
        nodes.push(node(id, name, kind, file));
        edges.push(contains(parent, id));
    }
    (nodes, edges)
}

fn bind_shapes(r: &UnresolvedRefRow, policy: BindingPolicy) -> Outcome {
    let (nodes, edges) = shape_fixture();
    bind(r, &Index::build(&nodes, &edges, std::slice::from_ref(r)), policy)
}

fn shape_residue(r: &UnresolvedRefRow) -> Option<super::binder::Residue> {
    let (nodes, edges) = shape_fixture();
    let ix = Index::build(&nodes, &edges, std::slice::from_ref(r));
    super::binder::residue(r, &ix, BindingPolicy::Aggressive)
}

const POLICIES: [BindingPolicy; 3] = [BindingPolicy::Strict, BindingPolicy::Balanced, BindingPolicy::Aggressive];

#[test]
fn a_self_call_binds_only_to_a_member_of_the_callers_own_class() {
    use super::binder::Residue;
    let own = Some(ReceiverShape::SelfInstance);
    for policy in POLICIES {
        // `self.m()` in `A.n` binds `A.m` — never the module-level `m` (201)
        // nor `B.m` (207).
        bound_to(bind_shapes(&shaped(100, PY_FILE, 204, "m", own), policy), 204, 203, EdgeKind::Calls);
        // `self.m()` in `A.m` is genuine recursion.
        bound_to(bind_shapes(&shaped(101, PY_FILE, 203, "m", own), policy), 203, 203, EdgeKind::Calls);
        // `C` has no `m`: the module-level `m` and the other classes' are no
        // members of it, and no proven base supplies one.
        let from_c = shaped(102, PY_FILE, 209, "m", own);
        assert_eq!(bind_shapes(&from_c, policy), Outcome::Unbound, "{policy:?}");
        assert_eq!(shape_residue(&from_c), Some(Residue::SupertypeUnreached));
        // Two `o` in `D`: an ambiguity, never a pick.
        let two = shaped(103, PY_FILE, 213, "o", own);
        assert_eq!(bind_shapes(&two, policy), Outcome::Unbound, "{policy:?}");
        assert_eq!(shape_residue(&two), Some(Residue::OverloadAmbiguous));
        // A call made in the class body itself (Scala, a Kotlin `init`): the
        // caller is the class, and its own `m` is the member.
        bound_to(bind_shapes(&shaped(105, PY_FILE, 202, "m", own), policy), 202, 203, EdgeKind::Calls);
        // A caller in no class has no class to bind through.
        let free = shaped(104, PY_FILE, 214, "m", own);
        assert_eq!(bind_shapes(&free, policy), Outcome::Unbound, "{policy:?}");
        assert_eq!(shape_residue(&free), Some(Residue::NoReceiverEvidence));
    }
}

#[test]
fn an_other_or_unshaped_call_never_binds_the_callers_own_method() {
    use super::binder::Residue;
    // `other.m()` inside `A.m`: the scope walk this replaces bound it to `A.m`
    // itself — a fabricated self-loop. A row with no shape is read as `other`.
    for receiver in [Some(ReceiverShape::Other), None] {
        let r = shaped(100, PY_FILE, 203, "m", receiver);
        for policy in POLICIES {
            assert_eq!(bind_shapes(&r, policy), Outcome::Unbound, "{receiver:?} at {policy:?}");
        }
        assert_eq!(shape_residue(&r), Some(Residue::NoReceiverEvidence), "{receiver:?}");
    }
}

#[test]
fn a_super_call_without_a_proven_extends_stays_unbound() {
    use super::binder::Residue;
    // `super().m()` inside `A.n`: `A` records no `Extends`, so there is no level
    // to bind at — and `A.m` itself is never one.
    let r = shaped(100, PY_FILE, 204, "m", Some(ReceiverShape::Super));
    for policy in POLICIES {
        assert_eq!(bind_shapes(&r, policy), Outcome::Unbound, "{policy:?}");
    }
    assert_eq!(shape_residue(&r), Some(Residue::SupertypeUnreached));
}

/// The file ids of the package-shaped hierarchy fixture.
const BASE_JAVA: i64 = 30;
const A_JAVA: i64 = 31;

/// A package-shaped hierarchy, whose `Extends` rows bind (S-468):
///
/// ```text
/// com.x  Base.java (module 300) ─ class Base (301) ─ m (302), only_base (303)
///        A.java    (module 310) ─ class A (311) extends Base ─ m (312), n (313)
///        Lone.java (module 320) ─ class Lone (321) ─ n (322)        no extends
/// ```
fn hierarchy_index(r: &UnresolvedRefRow) -> Index {
    hierarchy_index_with(r, &[])
}

/// [`hierarchy_index`] with the `(row id, file, source node, target)` `Extends`
/// rows of `extra` added.
fn hierarchy_index_with(r: &UnresolvedRefRow, extra: &[(i64, i64, i64, &str)]) -> Index {
    let dir = "src/main/java/com/x";
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    for (id, name, kind, file, parent) in [
        (300, "Base", NodeKind::Module, "Base", None),
        (301, "Base", NodeKind::Class, "Base", Some(300)),
        (302, "m", NodeKind::Method, "Base", Some(301)),
        (303, "only_base", NodeKind::Method, "Base", Some(301)),
        (310, "A", NodeKind::Module, "A", None),
        (311, "A", NodeKind::Class, "A", Some(310)),
        (312, "m", NodeKind::Method, "A", Some(311)),
        (313, "n", NodeKind::Method, "A", Some(311)),
        (320, "Lone", NodeKind::Module, "Lone", None),
        (321, "Lone", NodeKind::Class, "Lone", Some(320)),
        (322, "n", NodeKind::Method, "Lone", Some(321)),
    ] {
        nodes.push(node(id, name, kind, &format!("{dir}/{file}.java")));
        edges.extend(parent.map(|p| contains(p, id)));
    }
    let mut refs = vec![make_ref(1, A_JAVA, 311, "Base", None, RefForm::Path, EdgeKind::Extends)];
    for &(id, file, source, target) in extra {
        refs.push(make_ref(id, file, source, target, None, RefForm::Path, EdgeKind::Extends));
    }
    refs.push(r.clone());
    let tmp = tempfile::tempdir().expect("tempdir");
    let registry = crate::plugin::LanguageRegistry::load(tmp.path()).expect("registry loads");
    let layout = super::package_key::PackageLayout::from_registry(&registry);
    Index::build_with_layout(&nodes, &edges, &refs, layout)
}

#[test]
fn a_self_call_climbs_the_nearest_proven_extends_level_after_its_own_class() {
    let own = Some(ReceiverShape::SelfInstance);
    for policy in POLICIES {
        // `A` declares `m`: its own, never `Base.m`.
        let r = shaped(100, A_JAVA, 313, "m", own);
        bound_to(bind(&r, &hierarchy_index(&r), policy), 313, 312, EdgeKind::Calls);
        // `A` declares no `only_base`: the proven base's.
        let r = shaped(101, A_JAVA, 313, "only_base", own);
        bound_to(bind(&r, &hierarchy_index(&r), policy), 313, 303, EdgeKind::Calls);
    }
}

#[test]
fn a_super_call_binds_only_through_a_proven_extends() {
    use super::binder::{residue, Residue};
    let base = Some(ReceiverShape::Super);
    for policy in POLICIES {
        // `super.m()` in `A.n` binds `Base.m` — never `A.m`.
        let r = shaped(100, A_JAVA, 313, "m", base);
        bound_to(bind(&r, &hierarchy_index(&r), policy), 313, 302, EdgeKind::Calls);
        // No level of the chain declares it.
        let none = shaped(101, A_JAVA, 313, "nowhere", base);
        let ix = hierarchy_index(&none);
        assert_eq!(bind(&none, &ix, policy), Outcome::Unbound);
        assert_eq!(residue(&none, &ix, policy), Some(Residue::SupertypeUnreached));
        // `Lone` extends nothing: its own `n` is never a `super` target.
        let lone = shaped(102, BASE_JAVA, 322, "n", base);
        let ix = hierarchy_index(&lone);
        assert_eq!(bind(&lone, &ix, policy), Outcome::Unbound);
        assert_eq!(residue(&lone, &ix, policy), Some(Residue::SupertypeUnreached));
    }
}

/// An interface hierarchy in one package (S-609, FR-RS-48):
///
/// ```text
/// com.x  Port.java (module 330) ─ interface Port (331) ─ ping (332), gone (333)
///        Impl.java (module 340) ─ class Impl (341) implements Port ─ n (342)
/// ```
///
/// Built with `layout`, and the `Implements` row of `Impl`.
fn interface_index(r: &UnresolvedRefRow, layout: PackageLayout) -> Index {
    let dir = "src/main/java/com/x";
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    for (id, name, kind, file, parent) in [
        (330, "Port", NodeKind::Module, "Port", None),
        (331, "Port", NodeKind::Interface, "Port", Some(330)),
        (332, "ping", NodeKind::Method, "Port", Some(331)),
        (333, "gone", NodeKind::Method, "Port", Some(331)),
        (340, "Impl", NodeKind::Module, "Impl", None),
        (341, "Impl", NodeKind::Class, "Impl", Some(340)),
        (342, "n", NodeKind::Method, "Impl", Some(341)),
    ] {
        nodes.push(node(id, name, kind, &format!("{dir}/{file}.java")));
        edges.extend(parent.map(|p| contains(p, id)));
    }
    let refs = [
        make_ref(1, IMPL_JAVA, 341, "Port", None, RefForm::Path, EdgeKind::Implements),
        r.clone(),
    ];
    Index::build_with_layout(&nodes, &edges, &refs, layout)
}

const IMPL_JAVA: i64 = 34;

/// The walk visits an implemented interface's member bodies only where the
/// language declares `inherits_interface_bodies`, and never a member the store
/// records as uninherited (S-609, FR-RS-48). The key is read off the layout,
/// so the same graph keyed by a layout without it binds nothing — which is how
/// every language that does not declare it keeps its graph.
#[test]
fn an_interface_body_is_reached_only_where_the_layout_declares_the_key() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let registry = crate::plugin::LanguageRegistry::load(tmp.path()).expect("registry loads");
    let declaring = || PackageLayout::from_registry(&registry);
    let silent = || PackageLayout::new(registry.package_source_roots()).with_families(registry.families());
    let own = Some(ReceiverShape::SelfInstance);
    for policy in POLICIES {
        let r = shaped(100, IMPL_JAVA, 342, "ping", own);
        bound_to(bind(&r, &interface_index(&r, declaring()), policy), 342, 332, EdgeKind::Calls);
        let ix = interface_index(&r, silent());
        assert_eq!(bind(&r, &ix, policy), Outcome::Unbound, "no key, no interface level");
        assert_eq!(residue(&r, &ix, policy), Some(Residue::SupertypeUnreached));
        // An uninherited member (no body, or marked) is never a candidate.
        let r = shaped(101, IMPL_JAVA, 342, "gone", own);
        bound_to(bind(&r, &interface_index(&r, declaring()), policy), 342, 333, EdgeKind::Calls);
        let ix = interface_index(&r, declaring()).with_uninherited([NodeId(333)]);
        assert_eq!(bind(&r, &ix, policy), Outcome::Unbound, "an uninherited member is never bound");
        assert_eq!(residue(&r, &ix, policy), Some(Residue::SupertypeUnreached));
    }
}

#[test]
fn a_super_call_never_reaches_the_callers_own_class_through_a_cyclic_hierarchy() {
    use super::binder::{residue, Residue};
    // `A.n`'s `super.n()`: no base level declares `n`, and `A`'s own `n` is never
    // a candidate — not when `A` extends itself (it parses), and not when the
    // chain cycles back to `A` through `Base`.
    let r = shaped(100, A_JAVA, 313, "n", Some(ReceiverShape::Super));
    for extra in [vec![(2, A_JAVA, 311, "A")], vec![(2, BASE_JAVA, 301, "A")]] {
        let ix = hierarchy_index_with(&r, &extra);
        for policy in POLICIES {
            assert_eq!(bind(&r, &ix, policy), Outcome::Unbound, "{extra:?} at {policy:?}");
        }
        assert_eq!(residue(&r, &ix, BindingPolicy::Strict), Some(Residue::SupertypeUnreached), "{extra:?}");
    }
}

#[test]
fn a_self_call_from_a_module_level_method_binds_through_its_recorded_self_type() {
    use super::binder::{residue, Residue};
    // Rust and Go methods sit at module level: their class is the self type the
    // plugin recorded (S-493's column), bound through the one self-type arm.
    // Extraction records such a call as `Self::m`; a Method-form `self` row
    // reaches the same arm.
    let own = Some(ReceiverShape::SelfInstance);
    let drop = [75, 78, 84];
    for policy in POLICIES {
        let r = shaped(100, LIB_RS, 70, "helper", own);
        bound_to(bind(&r, &self_type_index(&r, &drop), policy), 70, 71, EdgeKind::Calls);
        // `B` records no `lone`: the free `lone` (74) in the same module is no
        // method of it.
        let r = shaped(101, LIB_RS, 72, "lone", own);
        let ix = self_type_index(&r, &drop);
        assert_eq!(bind(&r, &ix, policy), Outcome::Unbound);
        assert_eq!(residue(&r, &ix, policy), Some(Residue::SupertypeUnreached));
        // `other.helper()` beside the caller's own `helper` binds nothing.
        let r = shaped(102, LIB_RS, 70, "helper", Some(ReceiverShape::Other));
        let ix = self_type_index(&r, &drop);
        assert_eq!(bind(&r, &ix, policy), Outcome::Unbound);
        assert_eq!(residue(&r, &ix, policy), Some(Residue::NoReceiverEvidence));
    }
}

// ── The declared-namespace module model (S-518, [FR-RS-13]) ──────────────────
//
// A synthetic C#-shaped graph keyed by the namespaces its files declare —
// deliberately not by their directories:
//
// ```text
// src/Domain/Order.cs      Shop.Domain   class Order (401)
// src/Domain/Item.cs       Shop.Domain   class Item  (411)
// legacy/Odd.cs            Shop.Domain   class Odd   (431)  ← not its directory
// src/Other/Order.cs       Shop.Other    class Order (441)
// src/Api/Svc.cs           Shop.Api      class Svc   (421)
// src/Api/GlobalUsings.cs  (global)      — its rows only
// tools/Tool.cs            Tools         class Tool  (461)
// ```
//
// [FR-RS-13]: ../../../docs/specs/requirements/FR-RS-13.md

const ORDER_CS: i64 = 40;
const SVC_CS: i64 = 42;
const GLOBALS_CS: i64 = 45;
const TOOL_CS: i64 = 46;

/// The namespace fixture's nodes, `Contains` edges and declared namespaces.
fn namespace_graph() -> (Vec<NodeRow>, Vec<EdgeRow>, Vec<(String, String)>) {
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut declared = Vec::new();
    for (module, file, namespace, ty, name) in [
        (400, "src/Domain/Order.cs", "Shop.Domain", 401, "Order"),
        (410, "src/Domain/Item.cs", "Shop.Domain", 411, "Item"),
        (430, "legacy/Odd.cs", "Shop.Domain", 431, "Odd"),
        (440, "src/Other/Order.cs", "Shop.Other", 441, "Order"),
        (420, "src/Api/Svc.cs", "Shop.Api", 421, "Svc"),
        (460, "tools/Tool.cs", "Tools", 461, "Tool"),
    ] {
        let stem = file.rsplit('/').next().unwrap().trim_end_matches(".cs");
        nodes.push(node(module, stem, NodeKind::Module, file));
        nodes.push(node(ty, name, NodeKind::Class, file));
        edges.push(contains(module, ty));
        declared.push((file.to_string(), namespace.to_string()));
    }
    nodes.push(node(450, "GlobalUsings", NodeKind::Module, "src/Api/GlobalUsings.cs"));
    declared.push(("src/Api/GlobalUsings.cs".to_string(), String::new()));
    (nodes, edges, declared)
}

/// The namespace fixture's index, with `refs` as its ledger.
fn namespace_index(refs: &[UnresolvedRefRow]) -> Index {
    let (nodes, edges, declared) = namespace_graph();
    let layout = super::package_key::PackageLayout::default()
        .with_namespace_extensions(["cs".to_string()])
        .with_declared_namespaces(declared);
    Index::build_with_layout(&nodes, &edges, refs, layout)
}

fn import(id: i64, file: i64, source: i64, target: &str, form: RefForm) -> UnresolvedRefRow {
    let alias = (form == RefForm::Path).then(|| target.rsplit("::").next().unwrap());
    make_ref(id, file, source, target, alias, form, EdgeKind::Imports)
}

fn type_use(id: i64, file: i64, source: i64, target: &str) -> UnresolvedRefRow {
    make_ref(id, file, source, target, None, RefForm::Path, EdgeKind::TypeUses)
}

/// Bind the last of `refs` against [`namespace_index`] over all of them.
fn bind_namespaced(refs: &[UnresolvedRefRow], policy: BindingPolicy) -> Outcome {
    let r = refs.last().expect("a row to bind");
    bind(r, &namespace_index(refs), policy)
}

#[test]
fn a_single_type_import_binds_through_the_declared_namespace_not_the_directory() {
    for policy in POLICIES {
        let r = import(1, SVC_CS, 420, "Shop::Domain::Item", RefForm::Path);
        bound_to(bind_namespaced(&[r], policy), 420, 411, EdgeKind::Imports);
        // `Odd` lives under `legacy/`, but declares `Shop.Domain`.
        let r = import(2, SVC_CS, 420, "Shop::Domain::Odd", RefForm::Path);
        bound_to(bind_namespaced(&[r], policy), 420, 431, EdgeKind::Imports);
        // A namespace no file declares — a framework, PSR — stays unbound, and
        // so does the directory spelling of an in-repository type.
        for external in ["Psr::Log::LoggerInterface", "src::Domain::Item", "Legacy::Odd"] {
            let r = import(3, SVC_CS, 420, external, RefForm::Path);
            assert_eq!(bind_namespaced(&[r], policy), Outcome::Unbound, "{external}");
        }
    }
}

#[test]
fn a_namespace_wildcard_binds_to_every_other_file_declaring_the_namespace() {
    for policy in POLICIES {
        let r = import(1, SVC_CS, 420, "Shop::Domain", RefForm::Glob);
        assert_eq!(
            bind_namespaced(&[r], policy),
            Outcome::BoundMany {
                source: NodeId(420),
                targets: vec![NodeId(400), NodeId(410), NodeId(430)],
                kind: EdgeKind::Imports,
                payload: None,
            },
            "every file declaring `Shop.Domain`, whatever its directory"
        );
        // A file's wildcard of its own namespace never names itself.
        let own = import(2, ORDER_CS, 400, "Shop::Domain", RefForm::Glob);
        assert_eq!(
            bind_namespaced(&[own], policy),
            Outcome::BoundMany {
                source: NodeId(400),
                targets: vec![NodeId(410), NodeId(430)],
                kind: EdgeKind::Imports,
                payload: None,
            }
        );
        // `using System.Text.Json;` names no in-repository namespace.
        let r = import(3, SVC_CS, 420, "System::Text::Json", RefForm::Glob);
        assert_eq!(bind_namespaced(&[r], policy), Outcome::Unbound);
    }
}

#[test]
fn a_namespace_wildcard_brings_its_types_into_view() {
    for policy in POLICIES {
        let using = import(1, SVC_CS, 420, "Shop::Domain", RefForm::Glob);
        let r = type_use(2, SVC_CS, 421, "Item");
        bound_to(bind_namespaced(&[using.clone(), r], policy), 421, 411, EdgeKind::TypeUses);
        // Without it, `Item` is not in view from `Shop.Api`.
        let r = type_use(2, SVC_CS, 421, "Item");
        assert_eq!(bind_namespaced(&[r], policy), Outcome::Unbound);
        // Two wildcards each supplying an `Order` are an ambiguity, never a pick.
        let other = import(3, SVC_CS, 420, "Shop::Other", RefForm::Glob);
        let r = type_use(4, SVC_CS, 421, "Order");
        assert_eq!(bind_namespaced(&[using, other, r], policy), Outcome::Unbound);
    }
}

/// FR-RS-13 rule 1: a single-type import is final for the name it imports —
/// it shadows what a wildcard brings into view. Read as a wildcard itself, it
/// would make `Order` ambiguous between the two namespaces.
#[test]
fn a_single_type_import_is_final_over_a_namespace_wildcard() {
    for policy in POLICIES {
        let using = import(1, SVC_CS, 420, "Shop::Domain", RefForm::Glob);
        let single = import(2, SVC_CS, 420, "Shop::Other::Order", RefForm::Path);
        let r = type_use(3, SVC_CS, 421, "Order");
        bound_to(bind_namespaced(&[using, single, r], policy), 421, 441, EdgeKind::TypeUses);
    }
}

/// FR-RS-13 rule 2: a type of the source's own namespace is visible without an
/// import — including one declared in a file whose directory names another.
#[test]
fn a_type_of_the_same_namespace_is_visible_without_an_import() {
    for policy in POLICIES {
        let r = type_use(1, ORDER_CS, 401, "Item");
        bound_to(bind_namespaced(&[r], policy), 401, 411, EdgeKind::TypeUses);
        let r = type_use(2, ORDER_CS, 401, "Odd");
        bound_to(bind_namespaced(&[r], policy), 401, 431, EdgeKind::TypeUses);
        // `Shop.Other.Order` is another namespace's: not in view from `Svc`.
        let r = type_use(3, SVC_CS, 421, "Order");
        assert_eq!(bind_namespaced(&[r], policy), Outcome::Unbound);
    }
}

/// FR-RS-13 rule 4: a `global using` brings its namespace into view in every
/// file of the language under the declaring file's directory — the project
/// root, by convention — and nowhere else.
#[test]
fn a_global_wildcard_applies_to_every_file_under_its_directory() {
    use super::binder::GLOBAL_WILDCARD_ALIAS;
    let global = make_ref(
        1,
        GLOBALS_CS,
        450,
        "Shop::Domain",
        Some(GLOBAL_WILDCARD_ALIAS),
        RefForm::Glob,
        EdgeKind::Imports,
    );
    for policy in POLICIES {
        // `Svc.cs` sits beside `GlobalUsings.cs`: `Item` is in view there.
        let r = type_use(2, SVC_CS, 421, "Item");
        bound_to(bind_namespaced(&[global.clone(), r], policy), 421, 411, EdgeKind::TypeUses);
        // `tools/Tool.cs` is outside `src/Api/`: it is not.
        let r = type_use(3, TOOL_CS, 461, "Item");
        assert_eq!(bind_namespaced(&[global.clone(), r], policy), Outcome::Unbound);
        // The global row itself binds like the wildcard it is.
        assert!(matches!(
            bind_namespaced(std::slice::from_ref(&global), policy),
            Outcome::BoundMany { .. }
        ));
    }
}

/// A file whose namespace is not recorded keeps the default model, and a
/// layout without namespaces binds the namespace fixture's imports not at all —
/// the model is what makes them bind.
#[test]
fn without_a_recorded_namespace_a_file_keeps_the_default_model() {
    let (nodes, edges, _) = namespace_graph();
    let r = import(1, SVC_CS, 420, "Shop::Domain::Item", RefForm::Path);
    let layout = super::package_key::PackageLayout::default()
        .with_namespace_extensions(["cs".to_string()]);
    let ix = Index::build_with_layout(&nodes, &edges, std::slice::from_ref(&r), layout);
    assert!(!ix.is_package_shaped("src/Api/Svc.cs"));
    assert_eq!(bind(&r, &ix, BindingPolicy::Strict), Outcome::Unbound);
}

/// Nothing moved but the change-set itself: no supertype walk, no import root.
fn unmoved() -> super::Moved {
    super::Moved {
        hierarchy: false,
        import_roots: false,
    }
}

/// A sync that adds or removes a `global using` re-binds every row of a
/// declared-namespace file (S-518): the wildcard moves a bare name in a file
/// the sync never touched, under a name no dirty token spells. Without the
/// flag the same change-set selects nothing outside the changed file.
#[test]
fn a_moved_global_wildcard_reselects_every_namespaced_row() {
    let r = type_use(2, SVC_CS, 421, "Item");
    // The global row is already gone from the ledger: the sync removed it.
    let ix = namespace_index(std::slice::from_ref(&r));
    let file_paths: std::collections::HashMap<i64, String> = [
        (SVC_CS, "src/Api/Svc.cs".to_string()),
        (GLOBALS_CS, "src/Api/GlobalUsings.cs".to_string()),
    ]
    .into_iter()
    .collect();
    let delta = |moved: bool| super::Delta {
        changed_paths: ["src/Api/GlobalUsings.cs".to_string()].into_iter().collect(),
        dirty_tokens: ["globalusings", "src", "api", "cs"].iter().map(|t| t.to_string()).collect(),
        global_imports_moved: moved,
    };
    assert!(super::is_affected(&r, &delta(true), &file_paths, &ix, unmoved()));
    assert!(!super::is_affected(&r, &delta(false), &file_paths, &ix, unmoved()));
}

/// A `global using` applies to its own language's files only (S-518): a PHP
/// file beside `GlobalUsings.cs`, itself keyed by a declared namespace, never
/// sees the C# wildcard — the `.cs` file beside it does.
#[test]
fn a_global_wildcard_never_crosses_into_another_language() {
    use super::binder::GLOBAL_WILDCARD_ALIAS;
    let (mut nodes, mut edges, mut declared) = namespace_graph();
    nodes.push(node(470, "Helper", NodeKind::Module, "src/Api/Helper.php"));
    nodes.push(node(471, "Helper", NodeKind::Class, "src/Api/Helper.php"));
    edges.push(contains(470, 471));
    declared.push(("src/Api/Helper.php".to_string(), "Shop.Api".to_string()));
    let layout = super::package_key::PackageLayout::default()
        .with_namespace_extensions(["cs".to_string(), "php".to_string()])
        .with_declared_namespaces(declared);
    const HELPER_PHP: i64 = 47;
    let global = make_ref(
        1,
        GLOBALS_CS,
        450,
        "Shop::Domain",
        Some(GLOBAL_WILDCARD_ALIAS),
        RefForm::Glob,
        EdgeKind::Imports,
    );
    let php = type_use(2, HELPER_PHP, 471, "Item");
    let cs = type_use(3, SVC_CS, 421, "Item");
    let refs = [global, php.clone(), cs.clone()];
    let ix = Index::build_with_layout(&nodes, &edges, &refs, layout);
    for policy in POLICIES {
        assert_eq!(bind(&php, &ix, policy), Outcome::Unbound, "{policy:?}");
        bound_to(bind(&cs, &ix, policy), 421, 411, EdgeKind::TypeUses);
    }
}

// ── A namespace sees the types of its enclosing namespaces (S-595, [FR-RS-45]) ─
//
// A synthetic C#-shaped graph built per test from `(extension, namespace, type)`
// triples — the type at index `i` is the class `1001 + 10 * i` of file
// `src/t{i}.{ext}`, declared in `namespace`, its module `1000 + 10 * i`. The
// rung is declared by the plugin, so each test binds under the layout with and
// without it: without it every name below stays unbound, and with it the
// enclosing namespaces decide.
//
// [FR-RS-45]: ../../../docs/specs/requirements/FR-RS-45.md

/// The class node of the type at index `i` of an [`enclosing_index`].
fn ty(i: i64) -> i64 {
    1001 + 10 * i
}

/// The file of the type at index `i`: one file id per type.
fn ty_file(i: i64) -> i64 {
    700 + i
}

/// An index over `types` (`(extension, namespace, name)`), `refs` as the
/// ledger, the layout declaring `enclosing` for the `cs` extension. `nested`
/// are `(inner id, name, outer type index)` classes nested in a type.
fn enclosing_index(
    types: &[(&str, &str, &str)],
    nested: &[(i64, &str, i64)],
    refs: &[UnresolvedRefRow],
    enclosing: bool,
) -> Index {
    enclosing_index_of_kinds(types, &[], nested, refs, enclosing)
}

/// [`enclosing_index`] where the types at the indexes of `kinds` are of that
/// kind rather than a class.
fn enclosing_index_of_kinds(
    types: &[(&str, &str, &str)],
    kinds: &[(i64, NodeKind)],
    nested: &[(i64, &str, i64)],
    refs: &[UnresolvedRefRow],
    enclosing: bool,
) -> Index {
    let (mut nodes, mut edges, mut declared) = (Vec::new(), Vec::new(), Vec::new());
    for (i, (ext, namespace, name)) in types.iter().enumerate() {
        let i = i as i64;
        let file = format!("src/t{i}.{ext}");
        let kind = kinds.iter().find(|(at, _)| *at == i).map_or(NodeKind::Class, |(_, k)| *k);
        nodes.push(node(1000 + 10 * i, &format!("t{i}"), NodeKind::Module, &file));
        nodes.push(node(ty(i), name, kind, &file));
        edges.push(contains(1000 + 10 * i, ty(i)));
        declared.push((file, namespace.to_string()));
    }
    for (id, name, outer) in nested {
        let file = format!("src/t{outer}.{}", types[*outer as usize].0);
        nodes.push(node(*id, name, NodeKind::Class, &file));
        edges.push(contains(ty(*outer), *id));
    }
    let mut layout = super::package_key::PackageLayout::default()
        .with_namespace_extensions(["cs".to_string(), "fx".to_string()])
        .with_declared_namespaces(declared);
    if enclosing {
        layout = layout.with_enclosing_namespaces(["cs".to_string()]);
    }
    Index::build_with_layout(&nodes, &edges, refs, layout)
}

/// `target` as a supertype of the type at index `source`.
fn extends(id: i64, source: i64, target: &str) -> UnresolvedRefRow {
    make_ref(id, ty_file(source), ty(source), target, None, RefForm::Path, EdgeKind::Extends)
}

/// Bind `r` over `types` under every policy, with the rung declared; the
/// outcome must be one for all of them.
fn bind_enclosing(
    types: &[(&str, &str, &str)],
    nested: &[(i64, &str, i64)],
    extra: &[UnresolvedRefRow],
    r: &UnresolvedRefRow,
) -> Outcome {
    let mut refs = extra.to_vec();
    refs.push(r.clone());
    let first = bind(r, &enclosing_index(types, nested, &refs, true), POLICIES[0]);
    for policy in POLICIES {
        assert_eq!(bind(r, &enclosing_index(types, nested, &refs, true), policy), first, "{policy:?}");
    }
    first
}

/// What the same row does when the plugin declares nothing: the before-state.
fn bind_without_rung(types: &[(&str, &str, &str)], r: &UnresolvedRefRow) -> Outcome {
    bind(r, &enclosing_index(types, &[], std::slice::from_ref(r), false), BindingPolicy::Balanced)
}

/// `A.B.C.D : Base` binds `A.Base`; without the key, nothing does.
#[test]
fn a_type_of_an_outer_namespace_is_in_view_from_a_nested_one() {
    let types = [("cs", "A", "Base"), ("cs", "A.B.C", "D")];
    let r = extends(1, 1, "Base");
    bound_to(bind_enclosing(&types, &[], &[], &r), ty(1), ty(0), EdgeKind::Extends);
    assert_eq!(bind_without_rung(&types, &r), Outcome::Unbound);
}

/// Nearest first: a `Base` in `A.B` is read before the one in `A`, and the
/// source's own namespace before either.
#[test]
fn the_nearest_enclosing_namespace_decides() {
    let types = [("cs", "A", "Base"), ("cs", "A.B", "Base"), ("cs", "A.B.C", "D")];
    bound_to(bind_enclosing(&types, &[], &[], &extends(1, 2, "Base")), ty(2), ty(1), EdgeKind::Extends);
    // The source's own namespace shadows every enclosing one.
    let types = [("cs", "A", "Base"), ("cs", "A.B", "Base"), ("cs", "A.B.C", "D"), ("cs", "A.B.C", "Base")];
    bound_to(bind_enclosing(&types, &[], &[], &extends(1, 2, "Base")), ty(2), ty(3), EdgeKind::Extends);
}

/// Exactly one per level, and an ambiguity stops the walk: two `Base` in `A.B`
/// bind nothing — the one in `A` is never reached past them.
#[test]
fn two_types_in_one_enclosing_namespace_bind_nothing_and_never_fall_through() {
    let types = [("cs", "A", "Base"), ("cs", "A.B", "Base"), ("cs", "A.B", "Base"), ("cs", "A.B.C", "D")];
    assert_eq!(bind_enclosing(&types, &[], &[], &extends(1, 3, "Base")), Outcome::Unbound);
    // The same through a qualified call head, which also says why.
    let call = make_ref(2, ty_file(3), ty(3), "Base::m", None, RefForm::Path, EdgeKind::Calls);
    let ix = enclosing_index(&types, &[], std::slice::from_ref(&call), true);
    assert_eq!(bind(&call, &ix, BindingPolicy::Balanced), Outcome::Unbound);
    assert_eq!(super::binder::residue(&call, &ix, BindingPolicy::Balanced), Some(Residue::TypeAmbiguous));
    // A level that holds one type does not mind the ambiguity beyond it.
    let types = [("cs", "A", "Base"), ("cs", "A", "Base"), ("cs", "A.B", "Base"), ("cs", "A.B.C", "D")];
    bound_to(bind_enclosing(&types, &[], &[], &extends(3, 3, "Base")), ty(3), ty(2), EdgeKind::Extends);
}

/// A qualified head is read under each enclosing namespace: `B.Thing` written
/// in `A.X` is `A.B.Thing`, and a head that is a type walks its nested types.
#[test]
fn a_qualified_head_is_read_under_each_enclosing_namespace() {
    let types = [("cs", "A.B", "Thing"), ("cs", "A.X", "Y")];
    let r = extends(1, 1, "B::Thing");
    bound_to(bind_enclosing(&types, &[], &[], &r), ty(1), ty(0), EdgeKind::Extends);
    assert_eq!(bind_without_rung(&types, &r), Outcome::Unbound);
    // `Outer.Inner` from `A.B.C`: `Outer` is a type of `A`.
    let types = [("cs", "A", "Outer"), ("cs", "A.B.C", "D")];
    let r = extends(2, 1, "Outer::Inner");
    bound_to(bind_enclosing(&types, &[(900, "Inner", 0)], &[], &r), ty(1), 900, EdgeKind::Extends);
    // Two `Base` in `A.B` are an ambiguous head: the `Base` of `A`, whose
    // `Inner` would bind, is never reached past them.
    let types = [("cs", "A", "Base"), ("cs", "A.B", "Base"), ("cs", "A.B", "Base"), ("cs", "A.B.C", "D")];
    let nested = [(900, "Inner", 0)];
    assert_eq!(bind_enclosing(&types, &nested, &[], &extends(3, 3, "Base::Inner")), Outcome::Unbound);
    // With one `Base` in `A.B` — which holds no `Inner` — that level still decides.
    let types = [("cs", "A", "Base"), ("cs", "A.B", "Base"), ("cs", "A.B.C", "D")];
    assert_eq!(bind_enclosing(&types, &nested, &[], &extends(4, 2, "Base::Inner")), Outcome::Unbound);
}

/// A nearer level whose type has no such member supplies nothing: the
/// wildcard that does is still read, as it was before the rung existed, and an
/// outer level is still never reached past that type (the rung adds bindings
/// where its walk succeeds and takes none away).
#[test]
fn an_enclosing_type_without_the_member_leaves_the_wildcards_to_decide() {
    // `Result.Inner` from `A.Sub`: `A.Result` has no `Inner`; `using X;` supplies
    // an `X.Result` that has one.
    let types = [("cs", "A", "Result"), ("cs", "X", "Result"), ("cs", "A.Sub", "Impl")];
    let nested = [(900, "Inner", 1)];
    let glob = make_ref(10, ty_file(2), ty(2), "X", None, RefForm::Glob, EdgeKind::Imports);
    let r = extends(1, 2, "Result::Inner");
    bound_to(bind_enclosing(&types, &nested, std::slice::from_ref(&glob), &r), ty(2), 900, EdgeKind::Extends);
    // Without the wildcard nothing supplies the member, and without the key the
    // wildcard binds it just the same — the rung took nothing away.
    assert_eq!(bind_enclosing(&types, &nested, &[], &r), Outcome::Unbound);
    let ix = enclosing_index(&types, &nested, &[glob, r.clone()], false);
    bound_to(bind(&r, &ix, BindingPolicy::Balanced), ty(2), 900, EdgeKind::Extends);
    // A nearer level that reached a type still hides the outer level's: the
    // `Inner` of `A`'s `Result` is not bound past `A.B`'s `Result`.
    let types = [("cs", "A", "Result"), ("cs", "A.B", "Result"), ("cs", "A.B.C", "Impl")];
    let nested = [(900, "Inner", 0)];
    assert_eq!(bind_enclosing(&types, &nested, &[], &extends(2, 2, "Result::Inner")), Outcome::Unbound);
}

/// A level holds exactly one *admitted* type (FR-RS-45): an enum a supertype
/// cannot name is skipped for the class of an outer level, and a bare call never
/// binds a class of an enclosing namespace.
#[test]
fn only_a_type_the_lookup_admits_decides_an_enclosing_level() {
    let types = [("cs", "A", "Base"), ("cs", "A.B", "Base"), ("cs", "A.B.C", "D")];
    let kinds = [(1, NodeKind::Enum)];
    let r = extends(1, 2, "Base");
    let ix = enclosing_index_of_kinds(&types, &kinds, &[], std::slice::from_ref(&r), true);
    for policy in POLICIES {
        bound_to(bind(&r, &ix, policy), ty(2), ty(0), EdgeKind::Extends);
    }
    let call = make_ref(2, ty_file(2), ty(2), "Base", None, RefForm::Path, EdgeKind::Calls);
    let ix = enclosing_index(&types[..1].iter().chain(&types[2..]).copied().collect::<Vec<_>>(), &[], std::slice::from_ref(&call), true);
    for policy in POLICIES {
        assert_eq!(bind(&call, &ix, policy), Outcome::Unbound, "{policy:?}");
    }
}

/// The qualified-head rung keeps the name rung's order: the source's own
/// namespace first, then each enclosing one, then the wildcards.
#[test]
fn a_qualified_head_reads_its_own_namespace_then_the_enclosing_ones_then_the_wildcards() {
    // `Outer.Inner` from `A.B.C`: `A.B.C.Outer` shadows `A.B.Outer`, which
    // shadows `A.Outer` — each with an `Inner` of its own.
    let types = [("cs", "A", "Outer"), ("cs", "A.B", "Outer"), ("cs", "A.B.C", "Outer"), ("cs", "A.B.C", "D")];
    let nested = [(900, "Inner", 0), (901, "Inner", 1), (902, "Inner", 2)];
    let r = extends(1, 3, "Outer::Inner");
    bound_to(bind_enclosing(&types, &nested, &[], &r), ty(3), 902, EdgeKind::Extends);
    let types = [("cs", "A", "Outer"), ("cs", "A.B", "Outer"), ("cs", "A.B.C", "D")];
    bound_to(bind_enclosing(&types, &nested[..2], &[], &extends(2, 2, "Outer::Inner")), ty(2), 901, EdgeKind::Extends);
    // An enclosing `A.Outer` is read before `using X;`'s `X.Outer`.
    let types = [("cs", "A", "Outer"), ("cs", "X", "Outer"), ("cs", "A.B", "D")];
    let nested = [(900, "Inner", 0), (901, "Inner", 1)];
    let glob = make_ref(10, ty_file(2), ty(2), "X", None, RefForm::Glob, EdgeKind::Imports);
    let r = extends(2, 2, "Outer::Inner");
    bound_to(bind_enclosing(&types, &nested, &[glob], &r), ty(2), 900, EdgeKind::Extends);
}

/// The source's own namespace is the first prefix a qualified head is read
/// under: `B.Thing` written in `A.X` is `A.X.B.Thing` when that type exists, and
/// `A.B.Thing` of the enclosing `A` is never bound past it (NFR-RA-05).
#[test]
fn a_qualified_head_is_read_under_the_sources_own_namespace_first() {
    let types = [("cs", "A.X.B", "Thing"), ("cs", "A.B", "Thing"), ("cs", "A.X", "User")];
    let r = extends(1, 2, "B::Thing");
    bound_to(bind_enclosing(&types, &[], &[], &r), ty(2), ty(0), EdgeKind::Extends);
    // Alone, the nested namespace's type binds too.
    let types = [("cs", "A.X.B", "Thing"), ("cs", "A.X", "User")];
    bound_to(bind_enclosing(&types, &[], &[], &extends(2, 1, "B::Thing")), ty(1), ty(0), EdgeKind::Extends);
}

/// A type spelled like an enclosing namespace is not the owner of a path read
/// under it: `A.B` the class does not make `Thing.Inner` of `A.B.C` anything but
/// `A.Thing`'s.
#[test]
fn a_type_named_like_an_enclosing_namespace_does_not_own_the_path() {
    let types = [("cs", "A", "B"), ("cs", "A", "Thing"), ("cs", "A.B.C", "D")];
    let nested = [(900, "Inner", 1)];
    bound_to(bind_enclosing(&types, &nested, &[], &extends(1, 2, "Thing::Inner")), ty(2), 900, EdgeKind::Extends);
}

/// A call through a head no rung reaches names, as the candidates for another
/// workspace member to declare, the types the source's scope would have read —
/// its enclosing namespaces among them, in the order the rung reads them.
#[test]
fn an_unreached_head_lists_the_enclosing_namespaces_among_its_candidates() {
    let types = [("cs", "A.B.C", "D")];
    let call = make_ref(1, ty_file(0), ty(0), "Gone::m", None, RefForm::Path, EdgeKind::Calls);
    let candidates = |enclosing| {
        let ix = enclosing_index(&types, &[], std::slice::from_ref(&call), enclosing);
        match super::binder::residue(&call, &ix, BindingPolicy::Balanced) {
            Some(Residue::ExternalType { candidates }) => candidates,
            other => panic!("expected an external type, got {other:?}"),
        }
    };
    let name = |ns: &[&str]| -> Vec<String> {
        ns.iter().chain(&["Gone"]).map(|s| s.to_string()).collect()
    };
    assert_eq!(candidates(true), [name(&["A", "B", "C"]), name(&["A", "B"]), name(&["A"])]);
    assert_eq!(candidates(false), [name(&["A", "B", "C"])]);
}

/// The global namespace is not an enclosing level, and another interop
/// family's namespace of the same spelling is never a candidate.
#[test]
fn a_global_namespace_type_and_another_familys_namespace_are_never_bound() {
    for types in [
        [("cs", "", "Base"), ("cs", "A.B", "D")],
        [("fx", "A", "Base"), ("cs", "A.B", "D")],
    ] {
        assert_eq!(bind_enclosing(&types, &[], &[], &extends(1, 1, "Base")), Outcome::Unbound, "{types:?}");
    }
    // A source of `A` has no enclosing namespace at all; `A.B`'s `Base` is a
    // nested namespace's, not an enclosing one.
    let types = [("cs", "A.B", "Base"), ("cs", "A", "D")];
    assert_eq!(bind_enclosing(&types, &[], &[], &extends(1, 1, "Base")), Outcome::Unbound);
}

/// The rung is read before a namespace wildcard, and after a single-type
/// import, which stays final for the name it imports.
#[test]
fn the_enclosing_rung_sits_before_the_namespace_wildcard_and_after_a_single_type_import() {
    let types = [("cs", "A", "Base"), ("cs", "X", "Base"), ("cs", "A.B", "D")];
    let glob = make_ref(10, ty_file(2), ty(2), "X", None, RefForm::Glob, EdgeKind::Imports);
    let r = extends(1, 2, "Base");
    bound_to(bind_enclosing(&types, &[], std::slice::from_ref(&glob), &r), ty(2), ty(0), EdgeKind::Extends);
    // Without the key the wildcard supplies it — the rung is what moved.
    let ix = enclosing_index(&types, &[], &[glob.clone(), r.clone()], false);
    bound_to(bind(&r, &ix, BindingPolicy::Balanced), ty(2), ty(1), EdgeKind::Extends);
    // A single-type import of the name is final.
    let single = import(11, ty_file(2), ty(2), "X::Base", RefForm::Path);
    bound_to(bind_enclosing(&types, &[], &[single], &r), ty(2), ty(1), EdgeKind::Extends);
}

/// Another language's files, which do not declare the key, are untouched even
/// under a layout that does for `cs`.
#[test]
fn a_language_that_does_not_declare_the_key_binds_as_before() {
    let types = [("fx", "A", "Base"), ("fx", "A.B.C", "D")];
    assert_eq!(bind_enclosing(&types, &[], &[], &extends(1, 1, "Base")), Outcome::Unbound);
}

// ── S-596 / FR-SY-12: the incremental sweep and its retraction ─────────────

fn edge(source: i64, target: i64, kind: EdgeKind) -> EdgeRow {
    EdgeRow {
        source: NodeId(source),
        target: NodeId(target),
        kind,
    }
}

fn bound(source: i64, target: i64, kind: EdgeKind) -> Outcome {
    Outcome::Bound {
        source: NodeId(source),
        target: NodeId(target),
        kind,
        payload: None,
    }
}

fn some_delta() -> super::Delta {
    super::Delta {
        changed_paths: ["src/util.rs".to_string()].into_iter().collect(),
        dirty_tokens: ["run".to_string()].into_iter().collect(),
        global_imports_moved: false,
    }
}

/// [`super::swept_sources`] over a tree with no file paths and no moved roots.
fn sweep<'s>(
    selected: &mut Vec<&'s UnresolvedRefRow>,
    refs: &'s [UnresolvedRefRow],
    ix: &Index,
    delta: Option<&super::Delta>,
) -> std::collections::HashSet<&'s str> {
    let (nodes, _) = fixture();
    super::swept_sources(selected, refs, &nodes, &std::collections::HashMap::new(), ix, delta, false)
}

/// A selected row that was bound sweeps its source: every other row of that
/// source joins the re-bind — a capture row living in another file too — and
/// no row of another source does. A full index sweeps nothing.
#[test]
fn a_bound_selected_row_sweeps_every_row_of_its_source() {
    let (nodes, edges) = fixture();
    let moved = UnresolvedRefRow {
        resolved: true,
        ..call(1, LIB_RS, 2, "run")
    };
    let sibling = call(2, LIB_RS, 2, "helper");
    let captured = make_ref(3, UTIL_RS, 2, "local sym5", None, RefForm::Symbol, EdgeKind::Calls);
    let other_source = call(4, LIB_RS, 3, "run");
    let refs = vec![moved.clone(), sibling, captured, other_source];
    let ix = Index::build(&nodes, &edges, &refs);
    let delta = some_delta();

    let mut selected = vec![&refs[0]];
    let swept = sweep(&mut selected, &refs, &ix, Some(&delta));
    assert_eq!(swept, ["local sym2"].into_iter().collect());
    let mut ids: Vec<i64> = selected.iter().map(|r| r.id).collect();
    ids.sort_unstable();
    assert_eq!(ids, [1, 2, 3]);

    // An unbound selected row has no edge to retract: nothing is swept.
    let unbound = call(5, LIB_RS, 2, "run");
    let mut selected = vec![&unbound];
    assert!(sweep(&mut selected, &refs, &ix, Some(&delta)).is_empty());
    assert_eq!(selected.len(), 1);

    let mut selected = vec![&refs[0]];
    assert!(sweep(&mut selected, &refs, &ix, None).is_empty());
    assert_eq!(selected.len(), 1);
}

/// A selected capture row whose source lies in a file this sync re-extracted
/// sweeps that source, although none of the source's fresh rows was bound
/// before: they, not the capture, decide its edges. A capture row whose source
/// file the sync left alone sweeps nothing — it still stands in for its
/// source's unchanged row.
#[test]
fn a_capture_row_from_a_re_extracted_source_sweeps_its_source() {
    let (nodes, edges) = fixture();
    // Captured under `src/util.rs`, out of `alpha` in `src/lib.rs`.
    let captured = make_ref(1, UTIL_RS, 2, "local sym5", None, RefForm::Symbol, EdgeKind::Calls);
    let fresh = call(2, LIB_RS, 2, "helper");
    let refs = vec![captured, fresh];
    let ix = Index::build(&nodes, &edges, &refs);
    let both = super::Delta {
        changed_paths: ["src/util.rs".to_string(), "src/lib.rs".to_string()].into_iter().collect(),
        ..some_delta()
    };
    let mut selected = vec![&refs[0]];
    assert_eq!(sweep(&mut selected, &refs, &ix, Some(&both)), ["local sym2"].into_iter().collect());
    assert_eq!(selected.len(), 2, "the source's fresh row joins the re-bind");

    let mut selected = vec![&refs[0]];
    assert!(sweep(&mut selected, &refs, &ix, Some(&some_delta())).is_empty());
}

/// Out of a swept source, the reference-bound edges no re-bound row produces
/// are retracted — every one of FR-SY-12's seven kinds; one a row still
/// produces, a containment edge, and an edge out of a source that was not
/// swept are kept.
#[test]
fn a_swept_source_retracts_only_the_reference_edges_no_row_produces() {
    let (nodes, mut edges) = fixture();
    edges.extend([
        // alpha -> helper: still produced.
        edge(2, 3, EdgeKind::Calls),
        // alpha -> run: the moved row's old edge.
        edge(2, 5, EdgeKind::Calls),
        // alpha -> beta: no row produces it.
        edge(2, 21, EdgeKind::Imports),
        // Containment is another pass's.
        edge(2, 7, EdgeKind::Contains),
        // helper -> run: helper was not swept.
        edge(3, 5, EdgeKind::Calls),
    ]);
    // alpha -> other: one unproduced edge of each remaining reference kind.
    let other_kinds = [
        EdgeKind::Accesses,
        EdgeKind::Extends,
        EdgeKind::Implements,
        EdgeKind::Instantiates,
        EdgeKind::TypeUses,
    ];
    edges.extend(other_kinds.iter().map(|&kind| edge(2, 20, kind)));
    let swept = ["local sym2"].into_iter().collect();
    let mut outcomes = vec![
        (1, true, Outcome::Unbound),
        (2, true, bound(2, 3, EdgeKind::Calls)),
    ];
    let no_captures = Default::default();
    let mut stale = super::retract_unproduced(&nodes, &edges, &swept, &no_captures, &mut outcomes);
    let mut expected = vec![
        (NodeId(2), NodeId(5), EdgeKind::Calls),
        (NodeId(2), NodeId(21), EdgeKind::Imports),
    ];
    expected.extend(other_kinds.iter().map(|&kind| (NodeId(2), NodeId(20), kind)));
    for list in [&mut stale, &mut expected] {
        list.sort_by_key(|(s, t, k)| (s.0, t.0, k.as_i32()));
    }
    assert_eq!(stale, expected);
    // Nothing swept, nothing retracted.
    let unswept = Default::default();
    let none = super::retract_unproduced(&nodes, &edges, &unswept, &no_captures, &mut outcomes);
    assert!(none.is_empty());
}

/// A capture row out of a swept source restores its edge only when one of the
/// source's own re-bound rows produces it too; otherwise it comes back unbound
/// and its edge is retracted. A capture row out of an unswept source keeps its
/// binding: it still stands in for its source's row.
#[test]
fn a_capture_row_never_restores_an_edge_its_source_no_longer_produces() {
    let (nodes, mut edges) = fixture();
    edges.extend([edge(2, 3, EdgeKind::Calls), edge(2, 5, EdgeKind::Calls)]);
    let swept = ["local sym2"].into_iter().collect();
    let captures = [10, 11, 12].into_iter().collect();
    let mut outcomes = vec![
        (1, true, bound(2, 3, EdgeKind::Calls)),
        (10, false, bound(2, 5, EdgeKind::Calls)),
        (11, false, bound(2, 3, EdgeKind::Calls)),
        (12, false, bound(3, 5, EdgeKind::Calls)),
    ];
    let stale = super::retract_unproduced(&nodes, &edges, &swept, &captures, &mut outcomes);
    assert_eq!(stale, [(NodeId(2), NodeId(5), EdgeKind::Calls)]);
    assert_eq!(outcomes[1].2, Outcome::Unbound);
    assert_eq!(outcomes[2].2, bound(2, 3, EdgeKind::Calls));
    assert_eq!(outcomes[3].2, bound(3, 5, EdgeKind::Calls));
}

/// A capture row is spent once it binds — this run, or as a resolved row an
/// earlier version kept — once its source's own rows were re-bound, or once its
/// source is gone; one that cannot bind for a live, unswept source stays. Only
/// capture rows are ever spent (CR-187).
#[test]
fn a_capture_row_is_spent_once_it_binds_or_its_source_is_re_bound() {
    let capture = |id: i64, source_node: i64, resolved: bool| UnresolvedRefRow {
        resolved,
        ..make_ref(id, 1, source_node, "local sym5", None, RefForm::Symbol, EdgeKind::Calls)
    };
    let refs = [
        capture(1, 2, false), // binds this run
        capture(2, 3, true),  // a stale resolved capture, not re-bound
        capture(3, 4, false), // unbound, its source swept
        capture(4, 6, false), // unbound, its source live and not swept: stays
        capture(6, 7, false), // unbound, its source gone
        UnresolvedRefRow {
            resolved: true,
            ..make_ref(5, 1, 4, "run", None, RefForm::Path, EdgeKind::Calls)
        },
    ];
    let swept = ["local sym4"].into_iter().collect();
    let bound_now: std::collections::HashMap<i64, bool> =
        [(1, true), (3, false), (4, false), (5, true)].into();
    let live = ["local sym2", "local sym3", "local sym4", "local sym6"].into_iter().collect();
    let spent = super::spent_captures(&refs, &swept, &live, |r| {
        bound_now.get(&r.id).copied().unwrap_or(r.resolved)
    });
    let mut spent: Vec<i64> = spent.into_iter().collect();
    spent.sort_unstable();
    assert_eq!(spent, [1, 2, 3, 6]);
}

// ── S-592 / FR-RS-43: a call binds only a callable whose arity admits it ────
//
// S-591 records each callable's parameter range and each call's argument
// count. The `self`, `super`, typed and overloading bare-call arms drop every
// candidate whose range excludes the count before exactly-one; a level left
// with none is passed over; nothing applicable anywhere is
// `no-applicable-overload`. An unknown range or count never filters.

/// `r` with its argument count set.
fn counted(r: UnresolvedRefRow, count: Option<u32>) -> UnresolvedRefRow {
    UnresolvedRefRow { arg_count: count, ..r }
}

fn range(min: u32, max: Option<u32>) -> Option<ParamRange> {
    Some(ParamRange { min, max })
}

/// [`hierarchy_index`] given the ranges of `Base.m` (302) and `A.m` (312).
fn ranged_hierarchy(r: &UnresolvedRefRow, base_m: Option<ParamRange>, own_m: Option<ParamRange>) -> Index {
    hierarchy_index(r).with_arities(&[(NodeId(302), base_m, None), (NodeId(312), own_m, None)])
}

#[test]
fn a_self_call_passes_over_an_own_member_its_count_cannot_fit() {
    let own = Some(ReceiverShape::SelfInstance);
    for policy in POLICIES {
        // `A.m()` takes none, `Base.m(x)` one: `self.m(1)` binds the inherited
        // overload, and `self.m()` the own one.
        let one = counted(shaped(100, A_JAVA, 313, "m", own), Some(1));
        let ix = ranged_hierarchy(&one, range(1, Some(1)), range(0, Some(0)));
        bound_to(bind(&one, &ix, policy), 313, 302, EdgeKind::Calls);
        let none = counted(shaped(101, A_JAVA, 313, "m", own), Some(0));
        let ix = ranged_hierarchy(&none, range(1, Some(1)), range(0, Some(0)));
        bound_to(bind(&none, &ix, policy), 313, 312, EdgeKind::Calls);
        // `super.m(1)` skips nothing of its own class, and binds `Base.m`.
        let up = counted(shaped(102, A_JAVA, 313, "m", Some(ReceiverShape::Super)), Some(1));
        let ix = ranged_hierarchy(&up, range(1, Some(1)), range(1, Some(1)));
        bound_to(bind(&up, &ix, policy), 313, 302, EdgeKind::Calls);
    }
}

#[test]
fn a_call_nothing_admits_is_no_applicable_overload() {
    for receiver in [ReceiverShape::SelfInstance, ReceiverShape::Super] {
        let r = counted(shaped(100, A_JAVA, 313, "m", Some(receiver)), Some(2));
        let ix = ranged_hierarchy(&r, range(1, Some(1)), range(0, Some(0)));
        for policy in POLICIES {
            assert_eq!(bind(&r, &ix, policy), Outcome::Unbound, "{receiver:?} at {policy:?}");
        }
        assert_eq!(residue(&r, &ix, BindingPolicy::Strict), Some(Residue::NoApplicableOverload), "{receiver:?}");
    }
    // A name no level holds at all is still `supertype-unreached`.
    let r = counted(shaped(101, A_JAVA, 313, "nowhere", Some(ReceiverShape::SelfInstance)), Some(2));
    let ix = ranged_hierarchy(&r, range(1, Some(1)), range(0, Some(0)));
    assert_eq!(residue(&r, &ix, BindingPolicy::Strict), Some(Residue::SupertypeUnreached));
}

#[test]
fn an_unknown_range_or_count_never_filters_a_candidate() {
    let own = Some(ReceiverShape::SelfInstance);
    for policy in POLICIES {
        // `A.m`'s range is unknown: it stays the own class's candidate, though
        // `Base.m` admits the call too.
        let r = counted(shaped(100, A_JAVA, 313, "m", own), Some(1));
        let ix = ranged_hierarchy(&r, range(1, Some(1)), None);
        bound_to(bind(&r, &ix, policy), 313, 312, EdgeKind::Calls);
        // The call's count is unknown (a spread): nothing is filtered, and the
        // own `A.m()` binds as it did by name.
        let r = counted(shaped(101, A_JAVA, 313, "m", own), None);
        let ix = ranged_hierarchy(&r, range(1, Some(1)), range(0, Some(0)));
        bound_to(bind(&r, &ix, policy), 313, 312, EdgeKind::Calls);
    }
}

#[test]
fn a_variadic_or_defaulted_range_admits_the_call() {
    let own = Some(ReceiverShape::SelfInstance);
    for (count, own_m) in [(5, range(0, None)), (1, range(1, None)), (3, range(1, Some(3))), (1, range(1, Some(3)))] {
        let r = counted(shaped(100, A_JAVA, 313, "m", own), Some(count));
        let ix = ranged_hierarchy(&r, range(count, Some(count)), own_m);
        bound_to(bind(&r, &ix, BindingPolicy::Strict), 313, 312, EdgeKind::Calls);
    }
}

#[test]
fn same_arity_overloads_stay_overload_ambiguous_and_others_are_told_apart() {
    // `D` declares two `o` (211, 212).
    let r = counted(shaped(100, PY_FILE, 213, "o", Some(ReceiverShape::SelfInstance)), Some(1));
    let (nodes, edges) = shape_fixture();
    let index = |o1: Option<ParamRange>, o2: Option<ParamRange>| {
        Index::build(&nodes, &edges, std::slice::from_ref(&r))
            .with_arities(&[(NodeId(211), o1, None), (NodeId(212), o2, None)])
    };
    let ix = index(range(1, Some(1)), range(1, Some(1)));
    assert_eq!(bind(&r, &ix, BindingPolicy::Strict), Outcome::Unbound);
    assert_eq!(residue(&r, &ix, BindingPolicy::Strict), Some(Residue::OverloadAmbiguous));
    bound_to(bind(&r, &index(range(1, Some(1)), range(2, Some(2))), BindingPolicy::Strict), 213, 211, EdgeKind::Calls);
    bound_to(bind(&r, &index(range(0, Some(0)), range(1, None)), BindingPolicy::Strict), 213, 212, EdgeKind::Calls);
}

#[test]
fn a_bare_call_is_filtered_only_in_a_language_that_overloads() {
    // `A.n`'s bare `m(1)` reaches `A.m` through the lexical chain. Java
    // overloads: `A.m()` cannot take it, and nothing else of the name is in
    // scope. Without the overloading key (a synthetic layout), it binds by
    // name, as before.
    let r = counted(call(100, A_JAVA, 313, "m"), Some(1));
    let ix = ranged_hierarchy(&r, range(1, Some(1)), range(0, Some(0)));
    assert_eq!(bind(&r, &ix, BindingPolicy::Strict), Outcome::Unbound);
    assert_eq!(residue(&r, &ix, BindingPolicy::Strict), Some(Residue::NoApplicableOverload));
    // The aggressive workspace name match filters too: `A.m()` cannot take
    // it, so the one `m` left is the inherited `Base.m(x)` — never an
    // ambiguity with the own member the count rules out.
    bound_to(bind(&r, &ix, BindingPolicy::Aggressive), 313, 302, EdgeKind::Calls);
    let (nodes, edges) = shape_fixture();
    let bare = counted(call(101, PY_FILE, 204, "m"), Some(1));
    let ix = Index::build(&nodes, &edges, std::slice::from_ref(&bare))
        .with_arities(&[(NodeId(203), range(0, Some(0)), None)]);
    bound_to(bind(&bare, &ix, BindingPolicy::Strict), 204, 203, EdgeKind::Calls);
}

#[test]
fn the_takes_self_filter_decides_before_the_arity_filter() {
    let store = [lib_use(90, "crate::util::Store", "Store")];
    // `Store`'s only `make` (424) takes no `self` and no argument: `x.make(1)`
    // is unbound because no method of that name exists for it, not because
    // one exists that takes no argument.
    let r = counted(proven(100, "Store::make", None), Some(1));
    let ix = self_fact_index(&r, &store, true);
    assert_eq!(bind(&r, &ix, BindingPolicy::Strict), Outcome::Unbound);
    assert_eq!(residue(&r, &ix, BindingPolicy::Strict), Some(Residue::SupertypeUnreached));
    // Both `pair`s take `self` and no argument: `x.pair(1)` fits neither.
    let r = counted(proven(101, "Store::pair", None), Some(1));
    let ix = self_fact_index(&r, &store, true);
    assert_eq!(bind(&r, &ix, BindingPolicy::Strict), Outcome::Unbound);
    assert_eq!(residue(&r, &ix, BindingPolicy::Strict), Some(Residue::NoApplicableOverload));
    // The inherent `pair` (422) cannot take one argument and the trait impl's
    // (423) can: the arity filter runs before the inherent-over-trait rank.
    let ix = self_fact_index(&r, &store, false).with_arities(&[
        (NodeId(422), range(0, Some(0)), Some(true)),
        (NodeId(423), range(1, Some(1)), Some(true)),
    ]);
    bound_to(bind(&r, &ix, BindingPolicy::Strict), 2, 423, EdgeKind::Calls);
}
