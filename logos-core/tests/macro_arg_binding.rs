//! Integration tests for binder-coverage Phase 2 — calls nested in macro /
//! format-string arguments (S-162, [CR-043], FR-RS-03 / FR-AN-01), exercised
//! end-to-end through [`Engine::index`] / [`Engine::sync`] against real
//! temp-directory fixtures.
//!
//! tree-sitter parses a macro's argument list as a `token_tree`, not as
//! expressions, so the `references` query never saw the calls inside
//! `format!(…)` and a callee whose *only* call site was a macro argument fell
//! out of the reachable set and was mis-reported dead. Extraction now walks the
//! token tree and emits the same `Calls` path/method RefFacts, which the
//! existing binder resolves to a concrete target (or leaves honestly
//! unresolved). These tests mirror the four verified Logos false positives:
//! `activity_card` / `noscript_twin` (path calls in a `format!` arg),
//! `chip_class` / `chip_label` (receiver-method calls in a `format!` arg), and a
//! callee reached only transitively through a macro-bound one (`cap_notice`).

use std::fs;
use std::path::Path;

use logos_core::graph_store::AnnotationNodeRow;
use logos_core::model::{EdgeKind, NodeKind};
use logos_core::{Engine, Runtime};
use tempfile::TempDir;

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn snapshot(rt: &Runtime) -> Vec<AnnotationNodeRow> {
    rt.submit_read(|store| store.annotation_nodes())
        .expect("read runs")
}

fn dead_of(snap: &[AnnotationNodeRow], name: &str) -> Option<bool> {
    snap.iter()
        .find(|n| !n.derived && n.name == name && n.kind == NodeKind::Function)
        .unwrap_or_else(|| panic!("no Function node named {name}"))
        .is_dead
}

/// The fixture. Every helper below is private, so each is live only if a call
/// edge reaches it from a live (here: `pub`, exported) root:
///
/// - `header_chip` / `chip_label` — a path call (`header_chip`) and a
///   receiver-method call (`self.chip_label()`), both **only** inside a
///   `format!` argument of the exported `Header::render`.
/// - `noscript_twin` — a path call only inside a `format!` argument of the
///   exported `graph_view`; it in turn calls `cap_notice` with an ordinary
///   (non-macro) call, so `cap_notice` is live **transitively** through a
///   macro-bound callee.
/// - `truly_dead` — the control: never called anywhere, so it stays dead (the
///   coverage must not over-bind / fabricate, NFR-RA-05 / AR-05).
/// - `mystery_missing()` is called inside the macro but matches no declaration,
///   so it must stay unresolved and create no edge (never fabricate).
const SRC: &str = "\
pub struct Header;
impl Header {
    pub fn render(&self) -> String {
        format!(\"{a}{b}\", a = header_chip(), b = self.chip_label())
    }
    fn chip_label(&self) -> &'static str {
        \"label\"
    }
}

fn header_chip() -> String {
    String::new()
}

pub fn graph_view() -> String {
    format!(\"<noscript>{t}</noscript>\", t = noscript_twin())
}

fn noscript_twin() -> String {
    cap_notice()
}

fn cap_notice() -> String {
    String::from(\"cap\")
}

pub fn with_missing() -> String {
    format!(\"{x}\", x = mystery_missing())
}

fn truly_dead() {}
";

#[test]
fn macro_arg_path_and_method_calls_bind_and_flip_live() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/lib.rs", SRC);

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let result = engine.index();
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    let snap = snapshot(engine.runtime().unwrap());

    // Path call inside a `format!` arg (the `activity_card` / `noscript_twin`
    // shape) — bound to the module-level function, so live.
    assert_eq!(
        dead_of(&snap, "header_chip"),
        Some(false),
        "a path call in a macro arg binds its callee live"
    );
    // Receiver-method call inside a `format!` arg (the `chip_class` /
    // `chip_label` shape) — bound by the unique-name method fallback, so live.
    assert_eq!(
        dead_of(&snap, "chip_label"),
        Some(false),
        "a receiver-method call in a macro arg binds its callee live"
    );
    // The transitive case (the `cap_notice` shape): `noscript_twin` is bound
    // live from the macro arg, and reaches `cap_notice` by an ordinary call.
    assert_eq!(
        dead_of(&snap, "noscript_twin"),
        Some(false),
        "a path call in a macro arg makes the callee live"
    );
    assert_eq!(
        dead_of(&snap, "cap_notice"),
        Some(false),
        "a callee reached only through a macro-bound function is live transitively"
    );
    // The control: still dead — the coverage adds liveness only along real
    // calls, never over-roots.
    assert_eq!(
        dead_of(&snap, "truly_dead"),
        Some(true),
        "an uncalled function stays dead (no over-binding)"
    );
}

#[test]
fn an_unresolvable_macro_arg_call_creates_no_edge() {
    // Never fabricate (NFR-RA-05): `mystery_missing()` is called in a macro arg
    // but matches no declaration, so it stays in `unresolved_refs` and the
    // caller gains no Calls edge for it. Asserted via the resolution coverage:
    // there is at least one unresolved ref, and no node by that name exists.
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/lib.rs", SRC);
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.index();
    let rt = engine.runtime().unwrap();

    // No node was fabricated for the missing target.
    let snap = snapshot(rt);
    assert!(
        !snap.iter().any(|n| n.name == "mystery_missing"),
        "the missing target is not fabricated as a node"
    );
    // The reference survives unresolved in the ledger (honest, retried).
    let unresolved = rt
        .submit_read(|s| s.unresolved_refs())
        .unwrap()
        .into_iter()
        .filter(|r| r.target == "mystery_missing" && !r.resolved)
        .count();
    assert_eq!(
        unresolved, 1,
        "the unbindable macro-arg call persists in unresolved_refs"
    );
    // And `with_missing` (its caller) has no outbound Calls edge to anything
    // named mystery_missing — there is nothing to fabricate it against.
    let edges = rt.submit_read(|s| s.all_edges()).unwrap();
    let nodes = snap;
    let missing_targets = edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Calls)
        .filter(|e| {
            nodes
                .iter()
                .any(|n| n.id == e.target && n.name == "mystery_missing")
        })
        .count();
    assert_eq!(missing_targets, 0, "no Calls edge to a fabricated target");
}

#[test]
fn synced_state_matches_a_fresh_reindex() {
    // sync ≡ reindex for the macro-arg verdicts (the CR-015 equivalence
    // invariant): a tree edited and synced yields the same dead-code verdicts as
    // a from-scratch index of the same state.
    const EDITED: &str = "\
pub fn graph_view() -> String {
    format!(\"<noscript>{t}</noscript>\", t = noscript_twin())
}
fn noscript_twin() -> String {
    cap_notice()
}
fn cap_notice() -> String {
    String::from(\"cap\")
}
fn truly_dead() {}
";

    let synced_tmp = TempDir::new().unwrap();
    write(synced_tmp.path(), "src/lib.rs", SRC);
    let synced_engine = Engine::start(synced_tmp.path()).expect("engine starts");
    synced_engine.index();
    write(synced_tmp.path(), "src/lib.rs", EDITED);
    synced_engine.sync(&["src/lib.rs".into()]);
    let synced_snap = snapshot(synced_engine.runtime().unwrap());

    let fresh_tmp = TempDir::new().unwrap();
    write(fresh_tmp.path(), "src/lib.rs", EDITED);
    let fresh_engine = Engine::start(fresh_tmp.path()).expect("engine starts");
    fresh_engine.index();
    let fresh_snap = snapshot(fresh_engine.runtime().unwrap());

    for name in ["noscript_twin", "cap_notice", "truly_dead"] {
        assert_eq!(
            dead_of(&synced_snap, name),
            dead_of(&fresh_snap, name),
            "sync and reindex agree on is_dead for {name}"
        );
    }
}
