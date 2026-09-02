//! The single-root path is untouched by the workspace connection budget (S-324,
//! [FR-WS-03], [NFR-PE-11], [ADR-52], [ADR-63]).
//!
//! [ADR-63] bounds a **workspace's** resource cost and explicitly leaves the
//! single-root path alone: "no manifest, no budget, core-sized pools exactly as
//! today". This binary pins both halves of that against a real indexed repo:
//!
//! 1. **No budget, core-sized pool** — an [`Engine::start`] engine's read pool
//!    is still [`RuntimeConfig::default`]'s one-per-core, never a budgeted share.
//! 2. **Identical payloads** — the read-model bytes a repo produces do not
//!    depend on how many read connections served them. The comparison is run
//!    against the *same* store, so any difference is attributable to pool sizing
//!    and to nothing else.
//!
//! Together those say the budget is invisible below the workspace seam: a plain
//! repo answers byte-for-byte what it answered before this story.
//!
//! Kept out of `workspace_connection_budget.rs` because that binary lowers the
//! process-wide descriptor limit, which would make these engines compete for
//! descriptors with its 72 members.
//!
//! [FR-WS-03]: ../../docs/specs/requirements/FR-WS-03.md
//! [NFR-PE-11]: ../../docs/specs/requirements/NFR-PE-11.md
//! [ADR-52]: ../../docs/specs/architecture/decisions/ADR-52.md
//! [ADR-63]: ../../docs/specs/architecture/decisions/ADR-63.md
#![cfg(feature = "lang-rust")]

use std::path::{Path, PathBuf};

use logos_core::{Engine, RuntimeConfig};

const SOURCE: &str = r#"
pub fn alpha() -> u32 { 1 }
pub fn beta() -> u32 { alpha() + 1 }
pub struct Gamma { pub n: u32 }
"#;

/// A one-file repo, indexed once, then closed so later engines re-open the same
/// store rather than building a different one.
fn indexed_repo(root: &Path) {
    std::fs::create_dir_all(root.join("src")).expect("src dir");
    std::fs::write(root.join("src").join("lib.rs"), SOURCE).expect("fixture source");
    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    let _ = engine.sync(&[] as &[PathBuf]);
}

/// The read-model bytes a single engine produces — the payload the invariant is
/// about. Search and the graph counters are content-derived and therefore
/// deterministic across engines over one store; wall-clock and on-disk-size
/// fields are not, and are deliberately excluded rather than papered over.
fn payload(engine: &Engine) -> String {
    let search = serde_json::to_string(&engine.search("alpha", None, None)).expect("search json");
    let status = engine.status();
    format!(
        "{search}|files={}|nodes={}|edges={}|refs={}/{}",
        status.file_count,
        status.node_count,
        status.edge_count,
        status.refs_resolved,
        status.refs_total,
    )
}

/// A single-root engine keeps the core-sized read pool it sizes for itself — the
/// budget never reaches below the workspace seam ([ADR-63]).
#[test]
fn a_single_root_engine_keeps_its_core_sized_read_pool() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    indexed_repo(root);

    let engine = Engine::start(root).expect("engine starts");
    let runtime = engine.runtime().expect("a started engine owns a runtime");
    assert_eq!(
        runtime.reader_pool_size(),
        RuntimeConfig::default().reader_pool_size,
        "the single-root path must keep RuntimeConfig::default()'s one-connection-\
         per-core pool, not a workspace budget's share"
    );
}

/// A repo's payload is byte-for-byte identical whichever pool size served it, so
/// the budget cannot change what a single-root answer says ([FR-WS-03]).
#[test]
fn payloads_are_byte_identical_whatever_the_read_pool_size() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    indexed_repo(root);

    // The single-root path: the engine sizes its own pool.
    let single_root = Engine::start(root).expect("engine starts");
    let baseline = payload(&single_root);
    drop(single_root);

    // A control re-read through the same path, proving the payload is stable
    // across engine instances at all — without it, an equality below would not
    // distinguish "pool sizing is invisible" from "nothing here varies anyway".
    let control = Engine::start(root).expect("engine starts");
    let control_payload = payload(&control);
    drop(control);
    assert_eq!(
        control_payload, baseline,
        "the payload is not stable across engine instances; the comparison below \
         would prove nothing"
    );

    // The budgeted path: the tightest share a workspace could ever hand a member.
    for read_connections in [1, 2] {
        let budgeted =
            Engine::start_with_read_pool(root, read_connections).expect("engine starts");
        assert_eq!(
            budgeted
                .runtime()
                .expect("a started engine owns a runtime")
                .reader_pool_size(),
            read_connections,
            "the budgeted seam must actually shrink the pool, or this proves nothing"
        );
        assert_eq!(
            payload(&budgeted),
            baseline,
            "a {read_connections}-connection pool answered differently from the \
             core-sized single-root pool"
        );
    }
}
