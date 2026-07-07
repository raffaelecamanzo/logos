//! Black-box integration tests for the explicit config-apply seam (S-097,
//! [CR-025], [FR-UI-13], [FR-SY-07], [ADR-31]).
//!
//! These drive the story's acceptance criteria through the public `Engine`
//! façade exactly as the mutating web surface (S-098/S-100) will:
//! - a `config.toml` apply reconciles the graph to the new admission policy,
//!   reusing the admission-fingerprint reconciliation
//!   ([FR-SY-07](../../docs/specs/requirements/FR-SY-07.md));
//! - a `rules.toml` apply re-evaluates governance/the gate against the
//!   *unchanged* graph — no reindex
//!   ([FR-UI-13](../../docs/specs/requirements/FR-UI-13.md));
//! - Save alone (`config_write`) runs no pipeline — derived state is untouched
//!   until Apply;
//! - the call is a single instrumented façade span (one `config_apply`
//!   telemetry record, no nested public `index`/`scan` span) on the engine pool;
//! - Apply is safe to invoke beside a concurrently-writing watcher — the writer
//!   actor serializes the mutations ([ADR-02], [ADR-31]).
//!
//! Gated on `lang-rust`: integration tests share the crate's feature set and a
//! `--no-default-features` build excludes the Rust grammar these fixtures need.
#![cfg(feature = "lang-rust")]

use std::path::Path;
use std::sync::{Arc, Mutex};

use tempfile::TempDir;

use logos_core::config::{ConfigApplyOutcome, PolicyFile};
use logos_core::model::{NodeId, NodeKind};
use logos_core::{Engine, Runtime};

/// Write `contents` to `<root>/<rel>`, creating parent directories.
fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create parent dirs");
    }
    std::fs::write(path, contents).expect("write fixture file");
}

/// The `lib.rs` → `util.rs` cross-file-call fixture: `alpha` calls
/// `crate::util::run`, which binds to `run` in `util.rs`.
fn write_cross_file_fixture(root: &Path) {
    write(
        root,
        "src/lib.rs",
        "pub fn alpha() {\n    crate::util::run();\n}\n",
    );
    write(root, "src/util.rs", "pub fn run() {}\n");
}

/// Does the graph hold any node whose defining file is `rel`?
fn has_nodes_for_file(rt: &Runtime, rel: &str) -> bool {
    let rel = rel.to_string();
    rt.submit_read(move |store| {
        Ok(store
            .all_nodes()?
            .iter()
            .any(|n| n.file_path.as_deref() == Some(rel.as_str())))
    })
    .expect("read runs")
}

/// Is there an unresolved (`resolved = false`) ledger row whose target text
/// contains `needle`?
fn has_unresolved_target(rt: &Runtime, needle: &str) -> bool {
    let needle = needle.to_string();
    rt.submit_read(move |store| {
        Ok(store
            .unresolved_refs()?
            .iter()
            .any(|r| !r.resolved && r.target.contains(needle.as_str())))
    })
    .expect("read runs")
}

/// The rowid of the named function, or `None` — a stable identity that changes
/// only if the node is deleted and re-extracted (a reindex).
fn function_rowid(rt: &Runtime, name: &str) -> Option<NodeId> {
    let name = name.to_string();
    rt.submit_read(move |s| {
        Ok(s.search(&name, Some(NodeKind::Function), 8)?
            .into_iter()
            .find(|r| r.name == name)
            .map(|r| r.id))
    })
    .expect("read runs")
}

// ── FR-SY-07 / FR-UI-13: a config.toml apply reconciles to the new policy ──────

#[test]
fn config_apply_on_config_reconciles_to_the_new_admission_policy() {
    let tmp = TempDir::new().expect("temp root");
    write_cross_file_fixture(tmp.path());

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().expect("runtime present");

    // Index under the default (wide) config: util.rs is admitted and the
    // cross-file call binds.
    engine.index();
    assert!(has_nodes_for_file(rt, "src/util.rs"), "util.rs is indexed");

    // Save a narrowing edit through the real write seam, then Apply it.
    engine
        .config_write(PolicyFile::Config, "exclude = [\"src/util.rs\"]\n")
        .expect("a valid config write succeeds");
    let outcome = engine
        .config_apply(PolicyFile::Config)
        .expect("config apply runs");

    match outcome {
        ConfigApplyOutcome::Reconciled {
            reconciled_files,
            full_index,
            unresolved_refs,
            files_failed,
            warnings,
        } => {
            assert!(
                reconciled_files >= 1,
                "the narrowing purged at least the excluded file: {reconciled_files}"
            );
            assert!(!full_index, "an existing graph reconciles, not full-indexes");
            assert!(
                unresolved_refs >= 1,
                "the inbound reference into the purged file re-enters the ledger"
            );
            assert!(files_failed.is_empty(), "clean reconcile: {files_failed:?}");
            assert!(warnings.is_empty(), "clean reconcile: {warnings:?}");
        }
        other => panic!("a config.toml apply must reconcile, got {other:?}"),
    }

    // The graph now reflects the new admission policy (FR-SY-07).
    assert!(
        !has_nodes_for_file(rt, "src/util.rs"),
        "the now-excluded file's nodes are purged after Apply (FR-SY-07)"
    );
    assert!(
        has_unresolved_target(rt, "run"),
        "the inbound reference returned to unresolved_refs, never fabricated (NFR-RA-05)"
    );
    assert!(
        has_nodes_for_file(rt, "src/lib.rs"),
        "the still-admitted caller is untouched"
    );
}

// ── FR-UI-13: a rules.toml apply re-evaluates governance, no reindex ───────────

/// The contract that flags the upward domain→presentation dependency in the
/// fixture below (a layer-ordering + boundary violation).
const LAYERED_CONTRACT: &str = "\
[[layers]]
name  = \"domain\"
paths = [\"src/domain_*.rs\"]
order = 1

[[layers]]
name  = \"presentation\"
paths = [\"src/ui_*.rs\"]
order = 2

[[boundaries]]
from   = \"domain\"
to     = \"presentation\"
reason = \"the domain must not reach upward into presentation\"
";

#[test]
fn config_apply_on_rules_re_evaluates_governance_without_a_reindex() {
    let tmp = TempDir::new().expect("temp root");
    // The upward dependency exists in code, but no rules.toml constrains it yet.
    write(
        tmp.path(),
        "src/domain_core.rs",
        "use crate::ui_view::render;\n\npub fn compute() {\n    render();\n}\n",
    );
    write(tmp.path(), "src/ui_view.rs", "pub fn render() {}\n");

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().expect("runtime present");
    engine.index();

    // Baseline: a clean scan with no contract finds no violations.
    let baseline = engine.scan(false).expect("baseline scan");
    assert!(
        baseline.violations.is_empty(),
        "no contract yet, so no violations"
    );
    let rowid_before = function_rowid(rt, "compute").expect("compute exists");

    // Save a stricter rules.toml, then Apply it.
    engine
        .config_write(PolicyFile::Rules, LAYERED_CONTRACT)
        .expect("a valid rules write succeeds");
    let outcome = engine
        .config_apply(PolicyFile::Rules)
        .expect("rules apply runs");

    match outcome {
        ConfigApplyOutcome::Reevaluated {
            signal,
            violations,
            freshness,
            warnings,
        } => {
            assert!(signal.is_some(), "a non-empty graph carries a signal");
            assert!(
                violations >= 1,
                "the new contract's upward dependency is now flagged: {violations}"
            );
            assert!(
                freshness.contains("assumed-fresh"),
                "a rules apply reconciles no graph (no reindex): {freshness}"
            );
            assert!(warnings.is_empty(), "clean re-eval: {warnings:?}");
        }
        other => panic!("a rules.toml apply must re-evaluate governance, got {other:?}"),
    }

    // No reindex: the unchanged graph's node identities are stable.
    let rowid_after = function_rowid(rt, "compute").expect("compute still exists");
    assert_eq!(
        rowid_before, rowid_after,
        "a rules apply must not re-extract the graph — no rowid churn (no reindex)"
    );
    assert!(
        has_nodes_for_file(rt, "src/domain_core.rs") && has_nodes_for_file(rt, "src/ui_view.rs"),
        "both source files remain — admission is unchanged by a rules edit"
    );
}

// ── FR-UI-13: Save alone runs no pipeline ─────────────────────────────────────

#[test]
fn config_write_without_apply_leaves_the_graph_unchanged() {
    let tmp = TempDir::new().expect("temp root");
    write_cross_file_fixture(tmp.path());

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().expect("runtime present");
    engine.index();
    assert!(has_nodes_for_file(rt, "src/util.rs"), "util.rs is indexed");

    // Save a narrowing edit but DO NOT apply it.
    engine
        .config_write(PolicyFile::Config, "exclude = [\"src/util.rs\"]\n")
        .expect("a valid config write succeeds");

    // The graph is untouched: Save runs no pipeline (FR-UI-13).
    assert!(
        has_nodes_for_file(rt, "src/util.rs"),
        "Save alone runs no pipeline — the excluded file's nodes survive until Apply"
    );
}

// ── ADR-14: fail loud on a transient (no-runtime) engine ──────────────────────

#[test]
fn config_apply_on_a_transient_engine_is_an_error() {
    // A transient `Engine::open` engine has no read-only/writer runtime, so an
    // apply (which drives the pipeline/governance over the graph) must fail loud
    // on both paths rather than silently no-op (ADR-14; the method's documented
    // error contract). Pure filesystem reads/writes (`config_read`/`config_write`)
    // still work on such an engine — apply does not.
    let tmp = TempDir::new().expect("temp root");
    write_cross_file_fixture(tmp.path());
    let engine = Engine::open(tmp.path());

    assert!(
        engine.config_apply(PolicyFile::Config).is_err(),
        "a config apply on a transient engine fails loud (no runtime)"
    );
    assert!(
        engine.config_apply(PolicyFile::Rules).is_err(),
        "a rules apply on a transient engine fails loud (no runtime)"
    );
}

// ── FR-UI-13 / NFR-CC-01: a single instrumented façade span ───────────────────

/// A minimal tracing layer that records the `tool` field of every telemetry
/// completion event (`target = "logos::telemetry"`) — the single emission point
/// each `traced` façade call funnels through.
#[derive(Clone, Default)]
struct ToolRecorder {
    tools: Arc<Mutex<Vec<String>>>,
}

struct ToolVisitor<'a>(&'a mut Option<String>);

impl tracing::field::Visit for ToolVisitor<'_> {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "tool" {
            *self.0 = Some(value.to_string());
        }
    }

    // `record_debug` is the `Visit` trait's only required method; the telemetry
    // `tool` field is a `&str` captured via `record_str` above, so this is a
    // no-op rather than a (dead, quote-brittle) second capture path.
    fn record_debug(&mut self, _field: &tracing::field::Field, _value: &dyn std::fmt::Debug) {}
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for ToolRecorder {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        if event.metadata().target() != "logos::telemetry" {
            return;
        }
        let mut tool = None;
        event.record(&mut ToolVisitor(&mut tool));
        if let Some(tool) = tool {
            self.tools.lock().expect("tools lock").push(tool);
        }
    }
}

/// Raise the process-wide max level once: a scoped `with_default` subscriber
/// does **not** bump the global level gate the `info!` macros check first, so
/// without a permissive global default the telemetry events are dropped before
/// reaching a scoped layer. A no-op `registry()` global default sets the runtime
/// max to its permissive hint; per-capture routing still uses the scoped layer.
/// This must run before any `observability::init` (which installs its own global
/// default) — nothing in this test binary calls it, so the ordering holds.
fn ensure_global_level() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = tracing::subscriber::set_global_default(tracing_subscriber::registry());
    });
}

/// Capture the `tool` fields of telemetry events emitted on the current thread
/// while running `f` (the façade chokepoint emits its completion event on the
/// calling thread; inner pipeline passes run on the pool and are not captured).
fn captured_tools(f: impl FnOnce()) -> Vec<String> {
    use tracing_subscriber::layer::SubscriberExt;

    ensure_global_level();
    let recorder = ToolRecorder::default();
    let tools = Arc::clone(&recorder.tools);
    let subscriber = tracing_subscriber::registry().with(recorder);
    tracing::subscriber::with_default(subscriber, f);
    let captured = tools.lock().expect("tools lock").clone();
    captured
}

#[test]
fn config_apply_is_a_single_facade_span_on_each_path() {
    // config.toml path: one config_apply record, never a nested public `index`
    // façade span. The structural guarantee is that config_apply calls the inner
    // `run_reconcile` seam, not `self.index()`; a stray public `index()` would be
    // a *synchronous* call on this thread and so captured here. The inner
    // pipeline passes (extract/resolve/annotate) run on the pool and are
    // intentionally not captured.
    let tmp_cfg = TempDir::new().expect("temp root");
    write_cross_file_fixture(tmp_cfg.path());
    let engine_cfg = Engine::start(tmp_cfg.path()).expect("engine starts");
    engine_cfg.index();
    engine_cfg
        .config_write(PolicyFile::Config, "exclude = [\"src/util.rs\"]\n")
        .expect("config write");
    let cfg_tools = captured_tools(|| {
        engine_cfg
            .config_apply(PolicyFile::Config)
            .expect("config apply runs");
    });
    assert_eq!(
        cfg_tools.iter().filter(|t| *t == "config_apply").count(),
        1,
        "exactly one config_apply façade span on the config path: {cfg_tools:?}"
    );
    assert!(
        !cfg_tools.iter().any(|t| t == "index"),
        "no nested public `index` façade span — the apply calls the inner seam: {cfg_tools:?}"
    );

    // rules.toml path: one config_apply record, never a nested public `scan`.
    let tmp_rules = TempDir::new().expect("temp root");
    write(tmp_rules.path(), "src/lib.rs", "pub fn api() {}\n");
    let engine_rules = Engine::start(tmp_rules.path()).expect("engine starts");
    engine_rules.index();
    engine_rules
        .config_write(PolicyFile::Rules, LAYERED_CONTRACT)
        .expect("rules write");
    let rules_tools = captured_tools(|| {
        engine_rules
            .config_apply(PolicyFile::Rules)
            .expect("rules apply runs");
    });
    assert_eq!(
        rules_tools.iter().filter(|t| *t == "config_apply").count(),
        1,
        "exactly one config_apply façade span on the rules path: {rules_tools:?}"
    );
    assert!(
        !rules_tools.iter().any(|t| t == "scan"),
        "no nested public `scan` façade span — the apply calls the inner seam: {rules_tools:?}"
    );
}

// ── ADR-02 / ADR-31: Apply is safe beside a concurrently-writing watcher ──────

#[test]
fn config_apply_is_safe_beside_a_concurrent_writer() {
    let tmp = TempDir::new().expect("temp root");
    write_cross_file_fixture(tmp.path());

    let engine = Arc::new(Engine::start(tmp.path()).expect("engine starts"));
    let rt = engine.runtime().expect("runtime present");
    engine.index();

    // Apply must be safe beside a concurrent writer. The `serve` watcher issues
    // `sync` calls; here a background thread issues reconciling `scan`s — both
    // route their writes through the single writer actor, which serializes them
    // against the apply's own reconcile, so neither corrupts the graph nor
    // panics (ADR-02, ADR-31). The race-safe check is the final graph state, not
    // a reconciled-file count: the at-most-once config-change purge may fire on
    // either path depending on interleaving.
    let writer = Arc::clone(&engine);
    let handle = std::thread::spawn(move || {
        for _ in 0..20 {
            // Reconciling scan — exercises the same writer the apply uses.
            writer.scan(true).expect("concurrent scan runs");
        }
    });

    engine
        .config_write(PolicyFile::Config, "exclude = [\"src/util.rs\"]\n")
        .expect("config write");
    let outcome = engine
        .config_apply(PolicyFile::Config)
        .expect("config apply runs beside the concurrent writer");
    assert!(
        matches!(outcome, ConfigApplyOutcome::Reconciled { .. }),
        "the apply completed beside the concurrent writer"
    );

    handle.join().expect("the concurrent writer thread did not panic");

    // The graph converged on the new admission policy regardless of interleaving.
    assert!(
        !has_nodes_for_file(rt, "src/util.rs"),
        "the excluded file is purged after the apply, even beside a concurrent writer"
    );
}
