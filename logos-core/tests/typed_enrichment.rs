//! Integration tests for the swe-skills typed-node enrichment (S-039 /
//! FR-DG-07, ADR-19), exercised end-to-end through [`Engine::index`] against
//! real temp-directory fixtures and the Logos dogfood `docs/` tree.
//!
//! Coverage by acceptance criterion (FR-DG-07):
//! - on a swe-skills fixture, the `FR-*`/`ADR-*`/`S-NNN` artifacts become typed
//!   `Requirement`/`Adr`/`Story` nodes with `traces-to` edges between them;
//! - on a plain markdown repo, only generic `DocFile`/`DocSection` nodes appear
//!   and nothing errors — the enrichment is never a prerequisite;
//! - the enrichment is auto-detected from the convention files and disabling it
//!   in config produces only generic nodes (BR-24);
//! - promotion is a pure relabel: a re-index is byte-identical (NFR-RA-06), and
//!   no trace edge exists without a resolved target (NFR-RA-05).

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

/// All node `(name, kind)` pairs in the graph.
fn nodes(rt: &Runtime) -> Vec<(String, NodeKind)> {
    rt.submit_read(|store| {
        Ok(store
            .all_nodes()?
            .into_iter()
            .map(|n| (n.name, n.kind))
            .collect())
    })
    .expect("read runs")
}

/// `true` if exactly one node has `name` and `kind`.
fn has_node(rt: &Runtime, name: &str, kind: NodeKind) -> bool {
    nodes(rt)
        .iter()
        .filter(|(n, k)| n == name && *k == kind)
        .count()
        == 1
}

/// The id of the unique node with `name` and `kind`.
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

/// All `(symbol, kind)` node pairs, sorted — the stable lens for the
/// byte-identical re-index check (NFR-RA-06), invariant across rowid reassignment.
fn node_symbols(rt: &Runtime) -> Vec<(String, NodeKind)> {
    rt.submit_read(|store| {
        let mut out: Vec<(String, NodeKind)> = store
            .all_nodes()?
            .into_iter()
            .map(|n| (n.symbol.as_str().to_string(), n.kind))
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.as_i32().cmp(&b.1.as_i32())));
        Ok(out)
    })
    .expect("read runs")
}

/// A swe-skills convention fixture: two requirement files (one tracing to the
/// other), an ADR, and a journal carrying an `S-NNN` story that traces to a
/// requirement. The links are the existing doc→doc references (S-035); the
/// enrichment promotes the endpoints and re-types the references as `traces-to`.
fn write_swe_skills_fixture(root: &Path) {
    // CR-029/FR-CF-05 prunes `docs/planning/**` by default; a project that wants
    // its journal (and the Story nodes it carries) indexed opts back in by
    // overriding `exclude`. Empty here re-admits the planning journal below.
    write(root, ".logos/config.toml", "exclude = []\n");
    write(
        root,
        "docs/specs/requirements/FR-DG-07.md",
        "# FR-DG-07: typed enrichment\n\n## Dependencies\n\nDepends on [FR-DG-02](FR-DG-02.md).\n",
    );
    write(
        root,
        "docs/specs/requirements/FR-DG-02.md",
        "# FR-DG-02: doc sections\n\nThe heading model.\n",
    );
    write(
        root,
        "docs/specs/architecture/decisions/ADR-19.md",
        "# ADR-19: documentation graph\n\nDocumentation is a first-class layer.\n",
    );
    write(
        root,
        "docs/planning/journal.md",
        "# Journal\n\n### S-039: swe-skills typed-node enrichment\n\n\
         Implements [FR-DG-07](../specs/requirements/FR-DG-07.md) per \
         [ADR-19](../specs/architecture/decisions/ADR-19.md).\n",
    );
}

// ── FR-DG-07 AC1: convention artifacts become typed nodes with trace edges ───

#[test]
fn swe_skills_artifacts_are_promoted_to_typed_nodes() {
    let tmp = TempDir::new().unwrap();
    write_swe_skills_fixture(tmp.path());

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    engine.index();

    // The DocFile roots are promoted by path; the story section by its heading.
    assert!(
        has_node(rt, "FR-DG-07.md", NodeKind::Requirement),
        "an FR-*.md requirement file is a Requirement node (FR-DG-07)"
    );
    assert!(has_node(rt, "FR-DG-02.md", NodeKind::Requirement));
    assert!(
        has_node(rt, "ADR-19.md", NodeKind::Adr),
        "an ADR-NN.md file is an Adr node (FR-DG-07)"
    );
    assert!(
        has_node(
            rt,
            "S-039: swe-skills typed-node enrichment",
            NodeKind::Story
        ),
        "an S-NNN journal section is a Story node (FR-DG-07)"
    );

    // The journal file itself is not a convention artifact — it stays generic.
    assert!(has_node(rt, "journal.md", NodeKind::DocFile));
}

#[test]
fn typed_nodes_carry_traces_to_edges_between_them() {
    let tmp = TempDir::new().unwrap();
    write_swe_skills_fixture(tmp.path());

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    engine.index();

    let traces = edges_of(rt, EdgeKind::TracesTo);

    // Story → Requirement: the story implements the requirement it links to.
    let story = node_id(
        rt,
        "S-039: swe-skills typed-node enrichment",
        NodeKind::Story,
    );
    let fr07 = node_id(rt, "FR-DG-07.md", NodeKind::Requirement);
    let fr02 = node_id(rt, "FR-DG-02.md", NodeKind::Requirement);
    let adr19 = node_id(rt, "ADR-19.md", NodeKind::Adr);

    assert!(
        traces.contains(&(story, fr07)),
        "the story traces to the requirement it implements (FR-DG-07)"
    );
    assert!(
        traces.contains(&(story, adr19)),
        "the story traces to the ADR it references (FR-DG-07)"
    );
    assert!(
        traces.contains(&(fr07, fr02)),
        "a requirement traces to its dependency requirement (FR-DG-07)"
    );

    // None of these typed traces remain generic DocReference edges — they were
    // re-typed, not duplicated.
    let doc_refs = edges_of(rt, EdgeKind::DocReference);
    assert!(
        !doc_refs.contains(&(story, fr07)),
        "a typed trace is re-typed, not left as a DocReference"
    );
}

// ── FR-DG-07 AC2: a plain markdown repo yields only generic nodes, no error ──

#[test]
fn plain_markdown_repo_produces_only_generic_doc_nodes() {
    let tmp = TempDir::new().unwrap();
    // A repo with markdown that *looks* structured but carries no swe-skills
    // convention files: no requirements/ dir, no ADR-NN files.
    write(
        tmp.path(),
        "README.md",
        "# Project\n\nSee [the guide](docs/guide.md).\n",
    );
    write(
        tmp.path(),
        "docs/guide.md",
        "# Guide\n\n## Setup\n\nFollow [the readme](../README.md).\n",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    // Nothing errors — graceful degradation, the enrichment is never required.
    let result = engine.index();
    assert!(
        result.warnings.is_empty(),
        "a plain markdown repo indexes cleanly: {:?}",
        result.warnings
    );

    // Only generic doc kinds appear — no typed node, no trace edge.
    for (name, kind) in nodes(rt) {
        assert!(
            matches!(kind, NodeKind::DocFile | NodeKind::DocSection),
            "plain repo node {name:?} is generic, got {kind:?} (FR-DG-07)"
        );
    }
    assert!(
        edges_of(rt, EdgeKind::TracesTo).is_empty(),
        "a plain repo has no typed trace edges (FR-DG-07)"
    );
    // The generic doc layer is still primary: links still resolve.
    assert!(
        !edges_of(rt, EdgeKind::DocReference).is_empty(),
        "generic doc references still bind on a plain repo (S-035 unaffected)"
    );
}

// ── FR-DG-07 / BR-24: the enrichment is overridable via config ───────────────

#[test]
fn disabling_enrichment_in_config_produces_only_generic_nodes() {
    let tmp = TempDir::new().unwrap();
    write_swe_skills_fixture(tmp.path());
    // The convention files are present, but the override forces generic-only.
    // Keep `exclude = []` from the fixture so the planning journal stays indexed —
    // otherwise the default CR-029 exclude would prune the S-039 story and the
    // "stays generic" assertions would pass vacuously (the story would be absent
    // because it was never discovered, not because enrichment was disabled).
    write(
        tmp.path(),
        ".logos/config.toml",
        "exclude = []\n[documentation]\ntyped_enrichment = \"disabled\"\n",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    engine.index();

    for (name, kind) in nodes(rt) {
        assert!(
            matches!(kind, NodeKind::DocFile | NodeKind::DocSection),
            "with enrichment disabled, node {name:?} stays generic, got {kind:?} (BR-24)"
        );
    }
    assert!(
        edges_of(rt, EdgeKind::TracesTo).is_empty(),
        "disabled enrichment emits no trace edges (BR-24)"
    );
    // But the generic doc→doc references still resolve — only the typing is off.
    assert!(
        !edges_of(rt, EdgeKind::DocReference).is_empty(),
        "the generic layer is unaffected by the enrichment toggle"
    );
}

#[test]
fn forcing_enrichment_enabled_promotes_even_without_strong_layout() {
    let tmp = TempDir::new().unwrap();
    // No requirements/ or ADR files — auto-detection would stay off — but the
    // story heading is present and the override forces promotion on.
    write(
        tmp.path(),
        "docs/notes.md",
        "# Notes\n\n## S-001: kickoff\n\nFirst story.\n",
    );
    write(
        tmp.path(),
        ".logos/config.toml",
        "[documentation]\ntyped_enrichment = \"enabled\"\n",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    engine.index();

    assert!(
        has_node(rt, "S-001: kickoff", NodeKind::Story),
        "forced enrichment promotes the story heading even with no convention files"
    );
}

// ── NFR-RA-06: promotion is a pure relabel — re-index is byte-identical ───────

#[test]
fn promotion_is_a_stable_relabel_across_reindex() {
    let tmp = TempDir::new().unwrap();
    write_swe_skills_fixture(tmp.path());

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    engine.index();
    let once = node_symbols(rt);
    engine.index();
    let again = node_symbols(rt);
    assert_eq!(
        once, again,
        "the (symbol, kind) set is reproducible across a re-index (NFR-RA-06)"
    );
}

// ── NFR-RA-05: never-fabricate — a trace binds only on a resolved target ─────

#[test]
fn a_broken_typed_link_fabricates_no_trace_edge() {
    let tmp = TempDir::new().unwrap();
    // A requirement whose only outgoing link points at a requirement that does
    // not exist. The convention files are present (so enrichment is active and
    // the file is a Requirement), but the link must not bind.
    write(
        tmp.path(),
        "docs/specs/requirements/FR-DG-08.md",
        "# FR-DG-08: dangling\n\nDepends on [FR-DG-99](FR-DG-99.md).\n",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    let result = engine.index();

    assert!(
        has_node(rt, "FR-DG-08.md", NodeKind::Requirement),
        "the requirement file is still promoted"
    );
    assert!(
        edges_of(rt, EdgeKind::TracesTo).is_empty(),
        "a link to a non-existent target fabricates no trace edge (NFR-RA-05)"
    );
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
}

// ── FR-DG-04: a doc→code reference from a typed node stays a DocReference ─────

#[test]
fn doc_to_code_reference_from_a_requirement_stays_a_doc_reference() {
    let tmp = TempDir::new().unwrap();
    // A workspace-unique code symbol the requirement references by inline-code.
    write(tmp.path(), "src/lib.rs", "pub fn frobnicate_widget() {}\n");
    write(
        tmp.path(),
        "docs/specs/requirements/FR-DG-10.md",
        "# FR-DG-10: api\n\nCalls `frobnicate_widget` at startup.\n",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    engine.index();

    let func = node_id(rt, "frobnicate_widget", NodeKind::Function);
    // The doc→code reference binds (S-035), but as a generic DocReference — the
    // code symbol has no typed owner, so it is not elevated to a trace.
    assert!(
        edges_of(rt, EdgeKind::DocReference)
            .iter()
            .any(|(_, t)| *t == func),
        "the doc→code reference binds as a DocReference (FR-DG-04)"
    );
    assert!(
        edges_of(rt, EdgeKind::TracesTo)
            .iter()
            .all(|(_, t)| *t != func),
        "a doc→code reference is never elevated to a TracesTo edge (FR-DG-07)"
    );
}

// ── FR-DG-07: an intra-artifact anchor link is not a self-trace ──────────────

#[test]
fn an_intra_artifact_link_produces_no_self_trace() {
    let tmp = TempDir::new().unwrap();
    // Both endpoints of this link live in the same requirement file, so they
    // elevate to the same typed owner — no self-loop trace.
    write(
        tmp.path(),
        "docs/specs/requirements/FR-DG-11.md",
        "# FR-DG-11: self\n\n## Statement\n\nSee [the rationale](#rationale).\n\n\
         ## Rationale\n\nBecause.\n",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    engine.index();

    assert!(
        edges_of(rt, EdgeKind::TracesTo).iter().all(|(s, t)| s != t),
        "an intra-artifact link never produces a self-trace (FR-DG-07)"
    );
}

// ── ADR-19 dogfood: typed nodes light up on Logos's own docs/ ────────────────

/// Recursively copy every file with one of `exts` under `dir` into `dst_root`,
/// preserving the path relative to `base`.
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
fn dogfood_promotes_typed_nodes_on_logos_own_docs() {
    // Logos's own docs/ ARE a swe-skills corpus, so the enrichment must light up:
    // the requirement/ADR files become typed nodes and the story sections trace
    // to them — while remaining byte-identical across re-index (NFR-RA-06).
    let tmp = TempDir::new().unwrap();
    let crate_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_root = crate_root.parent().expect("workspace root");
    let docs = repo_root.join("docs");
    if !docs.is_dir() {
        eprintln!("no docs/ tree to dogfood; skipping");
        return;
    }
    copy_tree(repo_root, &docs, tmp.path(), &["md", "markdown"]);
    // Logos's journal lives under `docs/planning/`, which CR-029/FR-CF-05 prunes
    // by default; opt back in so the dogfood corpus still carries its Story
    // sections (a real swe-skills project that wants its journal indexed does the
    // same in its own `config.toml`).
    write(tmp.path(), ".logos/config.toml", "exclude = []\n");

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    engine.index();

    let all = nodes(rt);
    let count = |k: NodeKind| all.iter().filter(|(_, nk)| *nk == k).count();

    assert!(
        count(NodeKind::Requirement) > 0,
        "Logos's own requirements/FR-*.md files become Requirement nodes (FR-DG-07)"
    );
    assert!(
        count(NodeKind::Adr) > 0,
        "Logos's own decisions/ADR-NN.md files become Adr nodes (FR-DG-07)"
    );
    assert!(
        count(NodeKind::Story) > 0,
        "Logos's own journal S-NNN sections become Story nodes (FR-DG-07)"
    );
    assert!(
        !edges_of(rt, EdgeKind::TracesTo).is_empty(),
        "the typed trace web binds on Logos's own docs (FR-DG-07)"
    );

    // Determinism: a second index yields a byte-identical (symbol, kind) set.
    let first = node_symbols(rt);
    engine.index();
    assert_eq!(
        first,
        node_symbols(rt),
        "typed enrichment is reproducible on the dogfood corpus (NFR-RA-06)"
    );
}
