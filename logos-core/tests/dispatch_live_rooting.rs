//! Integration tests for the framework-dispatch live-rooting pass (S-161,
//! [CR-043], [ADR-39], FR-RS-03 / FR-AN-01), exercised end-to-end through
//! [`Engine::index`] / [`Engine::sync`] against real temp-directory fixtures.
//!
//! The pass live-roots the methods an external framework dispatches but the
//! binder cannot bind a call edge to — `impl Trait for Type` methods
//! (trait-impl dispatch) and `#[tool]`-attributed methods (tool dispatch) — so
//! the dead-code pass no longer mis-reports them dead, while a genuinely
//! unreachable inherent method still flags dead (no over-rooting), and the
//! marker set is reconciled across syncs (a method that stops being a dispatch
//! entry flips back to dead, and a synced state matches a fresh reindex).

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

/// The fixture: every callable is private and has no source-visible caller, so
/// without dispatch live-rooting all four would be dead. `on_event` is a
/// trait-impl method, `session_end` carries `#[tool]`, `into_record` is reached
/// only through `on_event`, and `truly_dead` is a plain unreachable inherent
/// method (the control).
const WITH_TOOL: &str = "\
pub trait Sink {
    fn on_event(&self);
}
struct Layer;
impl Sink for Layer {
    fn on_event(&self) {
        self.into_record();
    }
}
impl Layer {
    fn into_record(&self) {}
    fn truly_dead(&self) {}
    #[tool(description = \"end the session\")]
    fn session_end(&self) {}
}
";

/// The same fixture with the `#[tool]` attribute removed from `session_end`, so
/// it is no longer a dispatch entry.
const WITHOUT_TOOL: &str = "\
pub trait Sink {
    fn on_event(&self);
}
struct Layer;
impl Sink for Layer {
    fn on_event(&self) {
        self.into_record();
    }
}
impl Layer {
    fn into_record(&self) {}
    fn truly_dead(&self) {}
    fn session_end(&self) {}
}
";

#[test]
fn trait_impl_and_tool_methods_are_live_inherent_unreachable_stays_dead() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/lib.rs", WITH_TOOL);

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let result = engine.index();
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    let rt = engine.runtime().unwrap();
    let snap = snapshot(rt);

    // Trait-impl dispatch: invoked through the trait object / vtable, no
    // source-visible caller — live-rooted, not dead.
    assert_eq!(
        dead_of(&snap, "on_event"),
        Some(false),
        "a trait-impl method is live-rooted (framework dispatch)"
    );
    // Tool dispatch: invoked by the `#[tool]` router macro — live-rooted.
    assert_eq!(
        dead_of(&snap, "session_end"),
        Some(false),
        "a #[tool] method is live-rooted (tool dispatch)"
    );
    // Reached only through the live-rooted trait-impl method — live transitively.
    assert_eq!(
        dead_of(&snap, "into_record"),
        Some(false),
        "a helper reached only through a live-rooted method is live"
    );
    // The control: a plain inherent method with no caller and no dispatch
    // attribute is still flagged dead — the live-rooting does not over-reach.
    assert_eq!(
        dead_of(&snap, "truly_dead"),
        Some(true),
        "an unreachable inherent method is still dead (no over-rooting)"
    );

    // The marker is a non-derived `RoutesTo` self-edge (CR-043) — never a self-edge
    // for a genuine framework route. Assert the mechanism, not just the effect.
    let self_routes = rt
        .submit_read(|s| s.all_edges())
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == EdgeKind::RoutesTo && e.source == e.target)
        .count();
    assert!(
        self_routes >= 2,
        "at least the trait-impl and #[tool] methods carry a self-RoutesTo marker (got {self_routes})"
    );
}

#[test]
fn removing_the_tool_attribute_retires_the_marker_and_flips_dead() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/lib.rs", WITH_TOOL);
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.index();
    let rt = engine.runtime().unwrap();
    assert_eq!(
        dead_of(&snapshot(rt), "session_end"),
        Some(false),
        "live while #[tool] is present"
    );

    // Drop the attribute and sync only that file: the dispatch entry is gone, so
    // its marker must be reconciled away and the method flips to dead.
    write(tmp.path(), "src/lib.rs", WITHOUT_TOOL);
    let synced = engine.sync(&["src/lib.rs".into()]);
    assert!(synced.warnings.is_empty(), "{:?}", synced.warnings);
    assert_eq!(
        dead_of(&snapshot(rt), "session_end"),
        Some(true),
        "after #[tool] is removed the method is no longer live-rooted"
    );
    // The trait-impl method is untouched by the edit — still live.
    assert_eq!(
        dead_of(&snapshot(rt), "on_event"),
        Some(false),
        "the trait-impl method stays live across the sync"
    );
}

#[test]
fn synced_state_matches_a_fresh_reindex_of_the_same_state() {
    // Incremental sync ≡ reindex for the dispatch verdicts: a tree synced into
    // a state yields the same dead-code verdicts as a from-scratch index of it.
    let synced_tmp = TempDir::new().unwrap();
    write(synced_tmp.path(), "src/lib.rs", WITH_TOOL);
    let synced_engine = Engine::start(synced_tmp.path()).expect("engine starts");
    synced_engine.index();
    write(synced_tmp.path(), "src/lib.rs", WITHOUT_TOOL);
    synced_engine.sync(&["src/lib.rs".into()]);
    let synced_snap = snapshot(synced_engine.runtime().unwrap());

    let fresh_tmp = TempDir::new().unwrap();
    write(fresh_tmp.path(), "src/lib.rs", WITHOUT_TOOL);
    let fresh_engine = Engine::start(fresh_tmp.path()).expect("engine starts");
    fresh_engine.index();
    let fresh_snap = snapshot(fresh_engine.runtime().unwrap());

    for name in ["on_event", "into_record", "truly_dead", "session_end"] {
        assert_eq!(
            dead_of(&synced_snap, name),
            dead_of(&fresh_snap, name),
            "sync and reindex agree on is_dead for {name}"
        );
    }
}

#[test]
fn marker_persists_when_an_unrelated_file_is_synced() {
    // The incremental-safety invariant: syncing file B must not disturb the
    // live-root marker on a dispatch method in the untouched file A. (A derived
    // marker would be wiped by the annotation pass's clear_derived and not
    // re-added by B's scan; the non-derived marker persists — CR-043.)
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/a.rs",
        "\
pub trait Sink { fn on_event(&self); }
struct A;
impl Sink for A {
    fn on_event(&self) {}
}
",
    );
    write(tmp.path(), "src/b.rs", "pub fn b_one() {}\n");

    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.index();
    let rt = engine.runtime().unwrap();
    assert_eq!(
        dead_of(&snapshot(rt), "on_event"),
        Some(false),
        "the trait-impl method in a.rs is live after the initial index"
    );

    // Change only b.rs and sync only b.rs — a.rs is untouched.
    write(tmp.path(), "src/b.rs", "pub fn b_one() {}\npub fn b_two() {}\n");
    let synced = engine.sync(&["src/b.rs".into()]);
    assert!(synced.warnings.is_empty(), "{:?}", synced.warnings);

    assert_eq!(
        dead_of(&snapshot(rt), "on_event"),
        Some(false),
        "the untouched file's dispatch marker survives a sync of an unrelated file"
    );
}

/// `true` if the `Function` named `name` carries a dispatch live-root marker — a
/// `RoutesTo` self-edge (the CR-043 mechanism).
fn has_self_marker(rt: &Runtime, name: &str) -> bool {
    let Some(id) = snapshot(rt)
        .iter()
        .find(|n| !n.derived && n.name == name && n.kind == NodeKind::Function)
        .map(|n| n.id)
    else {
        return false;
    };
    rt.submit_read(|s| s.all_edges())
        .unwrap()
        .into_iter()
        .any(|e| e.kind == EdgeKind::RoutesTo && e.source == id && e.target == id)
}

#[test]
fn markers_are_planted_per_node_and_absent_on_non_dispatch_nodes() {
    // The marker mechanism is per-node: the dispatch methods carry it, the
    // ordinary (transitively-live and dead) methods do not — so a future
    // over-rooting regression is caught.
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/lib.rs", WITH_TOOL);
    let engine = Engine::start(tmp.path()).expect("engine starts");
    let result = engine.index();
    let rt = engine.runtime().unwrap();

    assert!(has_self_marker(rt, "on_event"), "trait-impl method is marked");
    assert!(has_self_marker(rt, "session_end"), "#[tool] method is marked");
    assert!(
        !has_self_marker(rt, "into_record"),
        "a transitively-live helper is NOT itself marked"
    );
    assert!(
        !has_self_marker(rt, "truly_dead"),
        "an unreachable inherent method is NOT marked"
    );
    // The full index reports the markers it planted (DispatchStats, NFR-PE-02).
    assert!(
        result.dispatch.markers_added >= 2 && result.dispatch.markers_removed == 0,
        "index planted ≥2 markers, retired none: {:?}",
        result.dispatch
    );
}

#[test]
fn adding_a_tool_attribute_plants_the_marker_and_flips_live() {
    // The symmetric (add) direction of marker reconcile, and the DispatchStats
    // add counter.
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/lib.rs", WITHOUT_TOOL);
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.index();
    let rt = engine.runtime().unwrap();
    assert_eq!(
        dead_of(&snapshot(rt), "session_end"),
        Some(true),
        "session_end is dead before #[tool] is added"
    );
    assert!(!has_self_marker(rt, "session_end"));

    write(tmp.path(), "src/lib.rs", WITH_TOOL);
    let synced = engine.sync(&["src/lib.rs".into()]);
    assert!(synced.warnings.is_empty(), "{:?}", synced.warnings);
    assert_eq!(
        dead_of(&snapshot(rt), "session_end"),
        Some(false),
        "session_end is live once #[tool] is added"
    );
    assert!(has_self_marker(rt, "session_end"), "the marker was planted");
    assert!(
        synced.dispatch.markers_added >= 1,
        "the sync reported the added marker: {:?}",
        synced.dispatch
    );
}

#[test]
fn transitive_helper_stays_live_and_marker_retires_on_tool_removal() {
    // After #[tool] is removed: session_end's marker is retired (gone), but
    // into_record stays live (reached through the still-live trait-impl on_event).
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/lib.rs", WITH_TOOL);
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.index();
    let rt = engine.runtime().unwrap();

    write(tmp.path(), "src/lib.rs", WITHOUT_TOOL);
    let synced = engine.sync(&["src/lib.rs".into()]);
    assert!(synced.warnings.is_empty(), "{:?}", synced.warnings);

    assert!(
        !has_self_marker(rt, "session_end"),
        "the #[tool] marker is retired after the attribute is removed"
    );
    assert_eq!(
        dead_of(&snapshot(rt), "into_record"),
        Some(false),
        "the helper reached via the trait-impl method stays live across the edit"
    );
    // Note: a same-file edit re-extracts the file, so the old marker is dropped by
    // capture-before-delete's cascade (the node is deleted+recreated) rather than by
    // the dispatch reconcile's delete_edge path — `markers_removed` is therefore not
    // asserted here. The observable outcome (marker gone, helper still live) holds.
    let _ = &synced.dispatch;
}
