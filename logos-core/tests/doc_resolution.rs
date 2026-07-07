//! Integration tests for documentation link resolution (S-035 / FR-DG-03,
//! FR-DG-04, FR-RS-03, NFR-RA-05, ADR-19), exercised end-to-end through
//! [`Engine::index`]/[`Engine::sync`] against real temp-directory fixtures and
//! the Logos dogfood `docs/` tree.
//!
//! Coverage by acceptance criterion:
//! - a doc→doc link to an existing anchor binds to exactly one `DocSection`
//!   (FR-DG-03); a link to a missing target stays unresolved and binds once the
//!   target appears (FR-DG-03 / FR-RS-03 / UAT-RS-01);
//! - a doc→code inline-code token for a workspace-unique symbol binds; a
//!   two-candidate mention stays unresolved; a prose word never binds
//!   (FR-DG-04 / NFR-RA-05);
//! - re-runs are byte-identical and a doc edit re-resolves through the sync
//!   (NFR-RA-06 / FR-RS-03).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use logos_core::model::{EdgeKind, NodeId, NodeKind};
use logos_core::{Engine, Runtime};
use tempfile::TempDir;

/// Write `contents` at `root/rel`, creating parents.
fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// The id of the unique node with `name` and `kind`, read straight from
/// `all_nodes` (doc nodes are not in FTS until S-037, so search is not used).
fn node_id(rt: &Runtime, name: &str, kind: NodeKind) -> NodeId {
    rt.submit_read(move |store| {
        let mut hits = store
            .all_nodes()?
            .into_iter()
            .filter(|n| n.name == name && n.kind == kind)
            .map(|n| n.id)
            .collect::<Vec<_>>();
        Ok(hits.pop().filter(|_| hits.is_empty()))
    })
    .expect("read runs")
    .unwrap_or_else(|| panic!("expected exactly one {kind:?} node named {name}"))
}

/// All `(source, target)` pairs of edges with `kind`.
fn edges_of(rt: &Runtime, kind: EdgeKind) -> Vec<(NodeId, NodeId)> {
    rt.submit_read(move |store| {
        Ok(store
            .all_edges()?
            .into_iter()
            .filter(|e| e.kind == kind)
            .map(|e| (e.source, e.target))
            .collect())
    })
    .expect("read runs")
}

/// The `DocReference` edges keyed by their endpoints' **symbol** strings, sorted
/// — stable across a re-index (which reassigns rowids), so it is the right lens
/// for the byte-identical determinism check (NFR-RA-06).
fn doc_ref_symbols(rt: &Runtime) -> Vec<(String, String)> {
    rt.submit_read(|store| {
        let by_id: HashMap<NodeId, String> = store
            .all_nodes()?
            .into_iter()
            .map(|n| (n.id, n.symbol.as_str().to_string()))
            .collect();
        let mut out: Vec<(String, String)> = store
            .all_edges()?
            .into_iter()
            .filter(|e| e.kind == EdgeKind::DocReference)
            .filter_map(|e| Some((by_id.get(&e.source)?.clone(), by_id.get(&e.target)?.clone())))
            .collect();
        out.sort();
        Ok(out)
    })
    .expect("read runs")
}

// ── FR-DG-03: a doc→doc link binds to exactly one DocSection ─────────────────

#[test]
fn doc_to_doc_link_with_anchor_binds_to_the_one_docsection() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "docs/a.md",
        "# A\n\n## Target Heading\n\nbody\n",
    );
    write(
        tmp.path(),
        "docs/b.md",
        "# B\n\nSee [the target](a.md#target-heading) for details.\n",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    engine.index();

    let b_section = node_id(rt, "B", NodeKind::DocSection);
    let target = node_id(rt, "Target Heading", NodeKind::DocSection);
    assert!(
        edges_of(rt, EdgeKind::DocReference).contains(&(b_section, target)),
        "the anchored link binds to the matching DocSection (FR-DG-03)"
    );
}

#[test]
fn doc_to_doc_link_without_anchor_binds_to_the_docfile() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "docs/a.md", "# A\n\nbody\n");
    write(tmp.path(), "docs/b.md", "# B\n\nSee [doc a](a.md).\n");

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    engine.index();

    let b_section = node_id(rt, "B", NodeKind::DocSection);
    let a_file = node_id(rt, "a.md", NodeKind::DocFile);
    assert!(
        edges_of(rt, EdgeKind::DocReference).contains(&(b_section, a_file)),
        "an anchorless link binds to the target DocFile (FR-DG-03)"
    );
}

// ── FR-DG-03 / FR-RS-03: a link to a missing target binds when it appears ────

#[test]
fn doc_link_to_missing_target_binds_once_the_target_is_added() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "docs/b.md",
        "# B\n\nSee [the target](a.md#target-heading).\n",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    let first = engine.index();
    assert!(
        edges_of(rt, EdgeKind::DocReference).is_empty(),
        "a.md does not exist yet — the link must stay unresolved (NFR-RA-05)"
    );
    assert!(
        first.resolution.refs_unresolved >= 1,
        "the unresolved link is recorded for retry (FR-RS-03)"
    );

    // The target doc appears later; the sync's retry sweep binds the link.
    write(
        tmp.path(),
        "docs/a.md",
        "# A\n\n## Target Heading\n\nbody\n",
    );
    engine.sync(&[PathBuf::from("docs/a.md")]);

    let b_section = node_id(rt, "B", NodeKind::DocSection);
    let target = node_id(rt, "Target Heading", NodeKind::DocSection);
    assert!(
        edges_of(rt, EdgeKind::DocReference).contains(&(b_section, target)),
        "the deferred link binds on the sync that indexes its target (FR-RS-03)"
    );
}

#[test]
fn cross_directory_parent_relative_link_binds() {
    // A `..` link from a nested doc resolves against the source doc's directory
    // (markdown-link semantics) — exercises normalise/fold of `..`.
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "docs/a.md", "# A\n\n## Target\n\nbody\n");
    write(
        tmp.path(),
        "docs/sub/b.md",
        "# B\n\nSee [target](../a.md#target).\n",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    engine.index();

    let b_section = node_id(rt, "B", NodeKind::DocSection);
    let target = node_id(rt, "Target", NodeKind::DocSection);
    assert!(
        edges_of(rt, EdgeKind::DocReference).contains(&(b_section, target)),
        "a parent-relative (../) link resolves against the source dir (FR-DG-03)"
    );
}

#[test]
fn bare_anchor_self_link_binds_within_the_same_file() {
    // `[x](#anchor)` targets a sibling heading in the same doc.
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "docs/a.md",
        "# A\n\njump to [the section](#second-heading).\n\n## Second Heading\n\nbody\n",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    engine.index();

    let a_section = node_id(rt, "A", NodeKind::DocSection);
    let second = node_id(rt, "Second Heading", NodeKind::DocSection);
    assert!(
        edges_of(rt, EdgeKind::DocReference).contains(&(a_section, second)),
        "a bare #anchor binds to a heading in the same file (FR-DG-03)"
    );
}

#[test]
fn explicit_repo_path_binds_to_the_code_file_module() {
    // An explicit repo file-path reference resolves to that file's module node,
    // root-relative even from a nested doc (FR-DG-04 "explicit repo file-path").
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/util.rs", "pub fn helper() {}\n");
    write(
        tmp.path(),
        "docs/guide.md",
        "# Guide\n\nThe helper lives in `src/util.rs`.\n",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    engine.index();

    let guide = node_id(rt, "Guide", NodeKind::DocSection);
    // `src/util.rs` → module key (crate, [util]) → the file-root module "util".
    let util_mod = node_id(rt, "util", NodeKind::Module);
    assert!(
        edges_of(rt, EdgeKind::DocReference).contains(&(guide, util_mod)),
        "an explicit repo path binds to the code file's module (FR-DG-04)"
    );
}

#[test]
fn duplicate_heading_anchor_stays_unresolved() {
    // Two headings slugify to the same anchor → the link is ambiguous and must
    // never bind (NFR-RA-05, the doc→doc analogue of the doc→code ambiguity).
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "docs/a.md",
        "# A\n\n## Notes\n\nfirst\n\n## Notes\n\nsecond\n",
    );
    write(tmp.path(), "docs/b.md", "# B\n\nsee [notes](a.md#notes).\n");

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    engine.index();

    // No DocReference edge targets either "Notes" section.
    let notes_ids = rt
        .submit_read(|store| {
            Ok(store
                .all_nodes()?
                .into_iter()
                .filter(|n| n.name == "Notes" && n.kind == NodeKind::DocSection)
                .map(|n| n.id)
                .collect::<Vec<_>>())
        })
        .expect("read runs");
    assert_eq!(notes_ids.len(), 2, "the ambiguity premise holds");
    let doc_edges = edges_of(rt, EdgeKind::DocReference);
    for id in &notes_ids {
        assert!(
            !doc_edges.iter().any(|(_, t)| t == id),
            "an ambiguous duplicate-heading anchor must never bind (NFR-RA-05)"
        );
    }
}

// ── FR-DG-04: a doc→code token for a unique symbol binds ─────────────────────

#[test]
fn doc_to_code_unique_symbol_token_binds() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/lib.rs",
        "pub fn extract_files() {}\npub fn other() {}\n",
    );
    write(
        tmp.path(),
        "docs/guide.md",
        "# Guide\n\nCall `extract_files` to walk the tree.\n",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    engine.index();

    let guide = node_id(rt, "Guide", NodeKind::DocSection);
    let func = node_id(rt, "extract_files", NodeKind::Function);
    assert!(
        edges_of(rt, EdgeKind::DocReference).contains(&(guide, func)),
        "a workspace-unique inline-code token binds to its symbol (FR-DG-04)"
    );
}

// ── FR-DG-04 / NFR-RA-05: ambiguity and prose never fabricate an edge ────────

#[test]
fn doc_to_code_two_candidate_mention_stays_unresolved() {
    let tmp = TempDir::new().unwrap();
    // Two functions named `run` — a mention of `run` is ambiguous.
    write(tmp.path(), "src/a.rs", "pub fn run() {}\n");
    write(tmp.path(), "src/b.rs", "pub fn run() {}\n");
    write(
        tmp.path(),
        "docs/guide.md",
        "# Guide\n\nThe entry point is `run`.\n",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    engine.index();

    // No DocReference edge targets either `run` (NFR-RA-05).
    let run_ids = rt
        .submit_read(|store| {
            Ok(store
                .all_nodes()?
                .into_iter()
                .filter(|n| n.name == "run" && n.kind == NodeKind::Function)
                .map(|n| n.id)
                .collect::<Vec<_>>())
        })
        .expect("read runs");
    assert!(run_ids.len() >= 2, "the ambiguity premise holds");
    let doc_edges = edges_of(rt, EdgeKind::DocReference);
    for id in &run_ids {
        assert!(
            !doc_edges.iter().any(|(_, t)| t == id),
            "an ambiguous mention must never bind (NFR-RA-05)"
        );
    }
}

#[test]
fn prose_word_with_no_symbol_is_never_linked() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/lib.rs", "pub fn real_symbol() {}\n");
    write(
        tmp.path(),
        "docs/guide.md",
        "# Guide\n\nThe `nonexistent_symbol_xyz` token names nothing here.\n",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    engine.index();

    assert!(
        edges_of(rt, EdgeKind::DocReference).is_empty(),
        "a token matching no symbol produces no edge (NFR-RA-05)"
    );
}

// ── NFR-RA-06: byte-identical re-runs ────────────────────────────────────────

#[test]
fn doc_reference_edges_are_byte_identical_across_runs() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/lib.rs", "pub fn extract_files() {}\n");
    write(tmp.path(), "docs/a.md", "# A\n\n## Target\n\nbody\n");
    write(
        tmp.path(),
        "docs/b.md",
        "# B\n\n[t](a.md#target) and `extract_files`.\n",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();

    let first_run = engine.index();
    let first = doc_ref_symbols(rt);
    let second_run = engine.index();
    let second = doc_ref_symbols(rt);
    assert_eq!(first, second, "the bound doc-reference set is reproducible");
    assert!(
        !first.is_empty(),
        "the fixture binds at least one reference"
    );
    // The surfaced stats are stable too (NFR-RA-06).
    assert_eq!(
        first_run.resolution.refs_total,
        second_run.resolution.refs_total
    );
    assert_eq!(
        first_run.resolution.refs_resolved,
        second_run.resolution.refs_resolved
    );
}

// ── FR-RS-03: a doc edit re-resolves through the sync ────────────────────────

#[test]
fn editing_a_doc_rebinds_its_references_through_sync() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/lib.rs", "pub fn extract_files() {}\n");
    write(tmp.path(), "docs/a.md", "# A\n\n## Target\n\nbody\n");
    write(tmp.path(), "docs/b.md", "# B\n\nplaceholder.\n");

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    engine.index();
    assert!(
        edges_of(rt, EdgeKind::DocReference).is_empty(),
        "b.md has no references yet"
    );

    // Edit b.md to add a doc→doc link and a doc→code token; the sync re-extracts
    // and resolution binds both.
    write(
        tmp.path(),
        "docs/b.md",
        "# B\n\nSee [target](a.md#target) and call `extract_files`.\n",
    );
    let result = engine.sync(&[PathBuf::from("docs/b.md")]);
    assert_eq!(result.files_modified, 1);

    let b_section = node_id(rt, "B", NodeKind::DocSection);
    let target = node_id(rt, "Target", NodeKind::DocSection);
    let func = node_id(rt, "extract_files", NodeKind::Function);
    let doc_edges = edges_of(rt, EdgeKind::DocReference);
    assert!(
        doc_edges.contains(&(b_section, target)),
        "the new doc→doc link binds after the edit (FR-RS-03)"
    );
    assert!(
        doc_edges.contains(&(b_section, func)),
        "the new doc→code token binds after the edit (FR-RS-03)"
    );
}

// ── ADR-19 dogfood: resolve over Logos's own docs/ ───────────────────────────

/// Recursively copy every `.rs` under `src_dir` and every `.md` under `docs_dir`
/// into `dst_root`, preserving the path relative to `base`.
fn copy_tree(base: &Path, dir: &Path, dst_root: &Path, exts: &[&str]) {
    for entry in fs::read_dir(dir).expect("readable dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            copy_tree(base, &path, dst_root, exts);
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| exts.contains(&e))
        {
            let rel = path.strip_prefix(base).unwrap();
            let dst = dst_root.join(rel);
            fs::create_dir_all(dst.parent().unwrap()).unwrap();
            fs::copy(&path, &dst).unwrap();
        }
    }
}

#[test]
fn dogfood_resolves_doc_references_on_logos_own_docs() {
    // Run the new resolution on Logos's own source + docs and assert it binds
    // real doc references while keeping the never-fabricate invariant: every run
    // is byte-identical and no edge is created without a resolved target.
    let tmp = TempDir::new().unwrap();
    let crate_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_root = crate_root.parent().expect("workspace root");
    copy_tree(&crate_root, &crate_root.join("src"), tmp.path(), &["rs"]);
    let docs = repo_root.join("docs");
    if docs.is_dir() {
        copy_tree(repo_root, &docs, tmp.path(), &["md", "markdown"]);
    }

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    engine.index();

    let doc_edges = edges_of(rt, EdgeKind::DocReference);
    println!("dogfood doc references bound: {}", doc_edges.len());
    assert!(
        !doc_edges.is_empty(),
        "Logos's own docs/ cross-link enough to bind some doc references"
    );

    // Never-fabricate: every DocReference endpoint is a real node in the graph.
    let node_ids = rt
        .submit_read(|store| {
            Ok(store
                .all_nodes()?
                .into_iter()
                .map(|n| n.id)
                .collect::<Vec<_>>())
        })
        .expect("read runs");
    for (s, t) in &doc_edges {
        assert!(
            node_ids.contains(s) && node_ids.contains(t),
            "a bound edge references only real nodes (NFR-RA-05)"
        );
    }

    // Determinism: a second index binds a symbol-identical set (NFR-RA-06).
    let once = doc_ref_symbols(rt);
    engine.index();
    let again = doc_ref_symbols(rt);
    assert_eq!(once, again, "doc-reference resolution is reproducible");
}
