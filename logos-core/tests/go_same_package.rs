//! A Go call binds within its own package across files (S-528, CR-172,
//! FR-RS-17, NFR-RA-05, NFR-RA-06) — exercised end to end through the public
//! [`Engine`] façade against the SHIPPED Go `symbols.scm`, `references.scm`
//! and `plugin.toml`.
//!
//! In Go every file of one directory that declares the same `package` clause
//! shares one scope. A bare `F()` its own file does not declare binds to the
//! exactly-one top-level `F` among the package's other files. The fixtures pin
//! the shapes the 2026-10-03 inspection named — zap's `Build → New`
//! (`config.go` → `logger.go`) and ollama's `GenerateHandler → GetModel`,
//! `PullHandler → PullModel` and `PullHandler → parseNormalizePullModelRef`,
//! each a method calling a free function of a sibling file — and the cases that
//! must stay unbound: a name two build-tag files declare, an unexported name
//! reached from a `package foo_test` file, a same-clause file of another
//! directory, and a method that only shares the name.
//!
//! Fixtures are written inline into temp directories, like every sibling
//! binding suite: a fixture tree checked into this repository would be indexed
//! into its own graph.
//!
//! [FR-RS-17]: ../../docs/specs/requirements/FR-RS-17.md

#![cfg(feature = "lang-go")]

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use logos_core::model::{EdgeKind, NodeId};
use logos_core::{Engine, Runtime};
use tempfile::TempDir;

#[path = "support/graph_fingerprint.rs"]
mod graph_fingerprint;
use graph_fingerprint::graph_fingerprint;

#[path = "support/coordinate.rs"]
mod coordinate;

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn tree(files: &[(&str, &str)]) -> TempDir {
    let tmp = TempDir::new().unwrap();
    for (rel, text) in files {
        write(tmp.path(), rel, text);
    }
    coordinate::pin(tmp.path());
    tmp
}

fn index(root: &Path) -> Engine {
    let engine = Engine::start(root).expect("engine starts");
    let result = engine.index();
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    engine
}

/// Every bound edge of `kind` as `(source file:name, target file:name)`, sorted.
fn edges_of(rt: &Runtime, kind: EdgeKind) -> Vec<(String, String)> {
    rt.submit_read(move |store| {
        let label: HashMap<NodeId, String> = store
            .all_nodes()?
            .into_iter()
            .map(|n| (n.id, format!("{}:{}", n.file_path.unwrap_or_default(), n.name)))
            .collect();
        let mut out: Vec<(String, String)> = store
            .all_edges()?
            .into_iter()
            .filter(|e| e.kind == kind)
            .map(|e| (label[&e.source].clone(), label[&e.target].clone()))
            .collect();
        out.sort();
        Ok(out)
    })
    .expect("read runs")
}

/// The `Calls` edges whose target is the node `file:name`, as source labels.
fn callers_of(rt: &Runtime, target: &str) -> Vec<String> {
    edges_of(rt, EdgeKind::Calls)
        .into_iter()
        .filter(|(_, t)| t == target)
        .map(|(s, _)| s)
        .collect()
}

/// The `Calls` edges whose target is named `name`, in any file.
fn calls_to_name(rt: &Runtime, name: &str) -> Vec<(String, String)> {
    let suffix = format!(":{name}");
    edges_of(rt, EdgeKind::Calls)
        .into_iter()
        .filter(|(_, t)| t.ends_with(&suffix))
        .collect()
}

/// Whether an unresolved `Calls` row named `name` is sourced in `file`.
fn unbound_call_in(rt: &Runtime, file: &str, name: &str) -> bool {
    let (file, name) = (file.to_string(), name.to_string());
    rt.submit_read(move |store| {
        let in_file: std::collections::HashSet<String> = store
            .all_nodes()?
            .into_iter()
            .filter(|n| n.file_path.as_deref() == Some(file.as_str()))
            .map(|n| n.symbol.as_str().to_string())
            .collect();
        Ok(store.unresolved_refs()?.into_iter().any(|r| {
            r.kind == EdgeKind::Calls && !r.resolved && r.target == name && in_file.contains(r.source_symbol.as_str())
        }))
    })
    .expect("read runs")
}

const GO_MOD: (&str, &str) = ("go.mod", "module example.com/shop\n\ngo 1.22\n");

// ── The inspection's spot checks ─────────────────────────────────────────────

/// zap: `config.go`'s `(Config) Build` calls `New`, which `logger.go` declares.
#[test]
fn a_method_binds_a_free_function_of_a_sibling_file_zap_build_new() {
    let tmp = tree(&[
        GO_MOD,
        (
            "config.go",
            "package zap\n\ntype Config struct{}\n\nfunc (cfg Config) Build() *Logger {\n\treturn New(nil)\n}\n",
        ),
        (
            "logger.go",
            "package zap\n\ntype Logger struct{}\n\nfunc New(core *int) *Logger {\n\treturn &Logger{}\n}\n",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(callers_of(rt, "logger.go:New"), ["config.go:Build"]);
}

/// ollama: the handlers of `server/routes.go` call free functions of
/// `server/images.go` — each binds, and `GetModel`'s callers are its
/// `server/` callers, whichever file they sit in.
#[test]
fn the_ollama_handlers_bind_their_sibling_files_functions() {
    let tmp = tree(&[
        GO_MOD,
        (
            "server/routes.go",
            "package server\n\ntype Server struct{}\n\n\
func (s *Server) GenerateHandler() {\n\tGetModel(\"m\")\n}\n\n\
func (s *Server) PullHandler() {\n\tname, _ := parseNormalizePullModelRef(\"m\")\n\tPullModel(name)\n}\n",
        ),
        (
            "server/images.go",
            "package server\n\nfunc GetModel(name string) error {\n\treturn nil\n}\n\n\
func PullModel(name string) error {\n\treturn nil\n}\n",
        ),
        (
            "server/model.go",
            "package server\n\nfunc parseNormalizePullModelRef(s string) (string, error) {\n\treturn s, nil\n}\n\n\
func ShowModel() {\n\tGetModel(\"m\")\n}\n",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        callers_of(rt, "server/images.go:GetModel"),
        ["server/model.go:ShowModel", "server/routes.go:GenerateHandler"]
    );
    assert_eq!(callers_of(rt, "server/images.go:PullModel"), ["server/routes.go:PullHandler"]);
    assert_eq!(
        callers_of(rt, "server/model.go:parseNormalizePullModelRef"),
        ["server/routes.go:PullHandler"]
    );
}

// ── What stays unbound ──────────────────────────────────────────────────────

/// A name two build-tag variants of one package declare is ambiguous: the call
/// binds neither, and the workspace fallback never picks one.
#[test]
fn a_name_two_build_tag_files_declare_stays_unbound() {
    let tmp = tree(&[
        GO_MOD,
        ("term/term.go", "package term\n\nfunc Width() int {\n\treturn isTerminal()\n}\n"),
        (
            "term/term_unix.go",
            "//go:build !windows\n\npackage term\n\nfunc isTerminal() int {\n\treturn 1\n}\n",
        ),
        (
            "term/term_windows.go",
            "//go:build windows\n\npackage term\n\nfunc isTerminal() int {\n\treturn 2\n}\n",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert!(calls_to_name(rt, "isTerminal").is_empty(), "{:?}", calls_to_name(rt, "isTerminal"));
    assert!(unbound_call_in(rt, "term/term.go", "isTerminal"));
}

/// A `package foo_test` file is a package of its own: it never reaches `foo`'s
/// unexported names, while an internal `package foo` test file does, and the
/// external test package's own files reach one another.
#[test]
fn an_external_test_package_is_its_own_package() {
    let tmp = tree(&[
        GO_MOD,
        ("cache/cache.go", "package cache\n\nfunc normalize(k string) string {\n\treturn k\n}\n"),
        (
            "cache/cache_internal_test.go",
            "package cache\n\nimport \"testing\"\n\nfunc TestNormalize(t *testing.T) {\n\tnormalize(\"k\")\n}\n",
        ),
        (
            "cache/cache_test.go",
            "package cache_test\n\nimport \"testing\"\n\nfunc TestExternal(t *testing.T) {\n\tnormalize(\"k\")\n\tfixture()\n}\n",
        ),
        ("cache/helpers_test.go", "package cache_test\n\nfunc fixture() {}\n"),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        callers_of(rt, "cache/cache.go:normalize"),
        ["cache/cache_internal_test.go:TestNormalize"]
    );
    assert!(unbound_call_in(rt, "cache/cache_test.go", "normalize"));
    assert_eq!(callers_of(rt, "cache/helpers_test.go:fixture"), ["cache/cache_test.go:TestExternal"]);
}

/// The package is the directory: a file of another directory declaring the
/// same clause is another package, and so is a file of the same directory
/// declaring another clause (a `//go:build ignore` generator's `package main`).
#[test]
fn the_package_is_the_directory_and_the_clause() {
    let tmp = tree(&[
        GO_MOD,
        ("a/util.go", "package util\n\nfunc Shared() {}\n"),
        ("b/main.go", "package util\n\nfunc Run() {\n\tShared()\n}\n"),
        ("c/c.go", "package c\n\nfunc Run() {\n\tgenerate()\n}\n"),
        ("c/gen.go", "//go:build ignore\n\npackage main\n\nfunc generate() {}\n"),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert!(calls_to_name(rt, "Shared").is_empty(), "{:?}", calls_to_name(rt, "Shared"));
    assert!(calls_to_name(rt, "generate").is_empty(), "{:?}", calls_to_name(rt, "generate"));
    assert!(unbound_call_in(rt, "b/main.go", "Shared"));
    assert!(unbound_call_in(rt, "c/c.go", "generate"));
}

/// A method of a sibling file is never a bare call's target (FR-RS-07): the
/// package rung reads the sibling as the lexical rung reads the caller's own
/// file. With a free function of the name beside it, the call binds that.
#[test]
fn a_sibling_method_that_only_shares_the_name_is_never_bound() {
    let tmp = tree(&[
        GO_MOD,
        ("svc/run.go", "package svc\n\nfunc Start() {\n\tflush()\n}\n"),
        ("svc/buf.go", "package svc\n\ntype Buf struct{}\n\nfunc (b *Buf) flush() {}\n"),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert!(calls_to_name(rt, "flush").is_empty(), "{:?}", calls_to_name(rt, "flush"));

    write(tmp.path(), "svc/free.go", "package svc\n\nfunc flush() {}\n");
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(callers_of(rt, "svc/free.go:flush"), ["svc/run.go:Start"]);
}

/// The caller's own file comes first: a declaration of the name there is the
/// one bound, and the sibling's same-named one is never a rival.
#[test]
fn the_callers_own_declaration_outranks_the_package() {
    let tmp = tree(&[
        GO_MOD,
        ("p/a.go", "package p\n\nfunc helper() {}\n\nfunc Run() {\n\thelper()\n}\n"),
        ("p/b.go", "package p\n\nfunc Other() {\n\thelper()\n}\n"),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(callers_of(rt, "p/a.go:helper"), ["p/a.go:Run", "p/b.go:Other"]);
}

/// The own-file rung runs before the package rung: two build-tag variants that
/// each declare `helper` and call it bind their own file's declaration, never
/// the other variant's — which the package alone would find, and leave
/// ambiguous with it.
#[test]
fn build_tag_variants_each_bind_their_own_files_declaration() {
    let variant = |tag: &str| format!("//go:build {tag}\n\npackage p\n\nfunc helper() {{}}\n\nfunc Run() {{\n\thelper()\n}}\n");
    let (linux, windows) = (variant("linux"), variant("windows"));
    let tmp = tree(&[GO_MOD, ("p/a_linux.go", &linux), ("p/a_windows.go", &windows)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(callers_of(rt, "p/a_linux.go:helper"), ["p/a_linux.go:Run"]);
    assert_eq!(callers_of(rt, "p/a_windows.go:helper"), ["p/a_windows.go:Run"]);
}

// ── Sync ≡ reindex ───────────────────────────────────────────────────────────

/// Index `initial`, overwrite `edits` (an empty text removes the file) and sync
/// them; then index the post-edit tree from scratch. The two graphs must be
/// identical, and the synced one's `Calls` edges are returned.
fn synced_equals_reindexed(initial: &[(&str, &str)], edits: &[(&str, &str)]) -> Vec<(String, String)> {
    let tmp_a = tree(initial);
    let engine_a = index(tmp_a.path());
    let root_a = tmp_a.path().canonicalize().expect("canonicalize root");
    let mut changed: Vec<PathBuf> = Vec::new();
    for (rel, text) in edits {
        if text.is_empty() {
            fs::remove_file(tmp_a.path().join(rel)).unwrap();
        } else {
            write(tmp_a.path(), rel, text);
        }
        changed.push(root_a.join(rel));
    }
    engine_a.sync(&changed);
    let rt_a = engine_a.runtime().unwrap();

    let mut final_state: BTreeMap<&str, &str> = initial.iter().copied().collect();
    for (rel, text) in edits {
        if text.is_empty() {
            final_state.remove(rel);
        } else {
            final_state.insert(rel, text);
        }
    }
    let files: Vec<(&str, &str)> = final_state.into_iter().collect();
    let tmp_b = tree(&files);
    let engine_b = index(tmp_b.path());

    assert_eq!(
        graph_fingerprint(rt_a),
        graph_fingerprint(engine_b.runtime().unwrap()),
        "sync-to-state must equal index-of-state"
    );
    edges_of(rt_a, EdgeKind::Calls)
}

const CALLER: (&str, &str) = ("p/run.go", "package p\n\nfunc Run() {\n\thelper()\n}\n");
const HELPER: (&str, &str) = ("p/helper.go", "package p\n\nfunc helper() {}\n");

#[test]
fn a_declaration_added_in_a_sibling_binds_on_sync_as_on_reindex() {
    let edges = synced_equals_reindexed(&[GO_MOD, CALLER], &[HELPER]);
    assert_eq!(edges, [("p/run.go:Run".to_string(), "p/helper.go:helper".to_string())]);
}

#[test]
fn a_duplicate_added_in_a_build_tag_file_unbinds_on_sync_as_on_reindex() {
    let edges = synced_equals_reindexed(
        &[GO_MOD, CALLER, HELPER],
        &[("p/helper_windows.go", "//go:build windows\n\npackage p\n\nfunc helper() {}\n")],
    );
    assert!(edges.is_empty(), "{edges:?}");
}

#[test]
fn a_sibling_changing_its_clause_unbinds_on_sync_as_on_reindex() {
    let edges = synced_equals_reindexed(
        &[GO_MOD, CALLER, HELPER],
        &[("p/helper.go", "package p_test\n\nfunc helper() {}\n")],
    );
    assert!(edges.is_empty(), "{edges:?}");
}

#[test]
fn the_caller_changing_its_clause_unbinds_on_sync_as_on_reindex() {
    let edges = synced_equals_reindexed(
        &[GO_MOD, CALLER, HELPER],
        &[("p/run.go", "package p_test\n\nfunc Run() {\n\thelper()\n}\n")],
    );
    assert!(edges.is_empty(), "{edges:?}");
}

#[test]
fn the_rival_removed_binds_on_sync_as_on_reindex() {
    let rival = ("p/helper_windows.go", "//go:build windows\n\npackage p\n\nfunc helper() {}\n");
    let edges = synced_equals_reindexed(&[GO_MOD, CALLER, HELPER, rival], &[(rival.0, "")]);
    assert_eq!(edges, [("p/run.go:Run".to_string(), "p/helper.go:helper".to_string())]);
}


/// A store whose Go files record their clause one sync at a time — indexed
/// before the clause was captured, then edited file by file — never binds
/// through a directory holding a Go file whose clause is not recorded yet: that
/// file could declare the name too. Here the caller and one build-tag variant
/// are re-extracted and the other variant is not; the call stays unbound, as a
/// cold index of the tree leaves it.
#[test]
fn a_directory_holding_a_file_with_no_recorded_clause_binds_nothing_through_the_package() {
    const SHIPPED: &str = include_str!("../plugins/go/queries/symbols.scm");
    let start = SHIPPED.find("; The file's `package` clause").expect("the clause pattern is shipped");
    let end = SHIPPED.find("; ── Parameter range").expect("the arity block follows it");
    let without_clause = format!("{}{}", &SHIPPED[..start], &SHIPPED[end..]);

    let tmp = tree(&[
        GO_MOD,
        ("term/term.go", "package term\n\nfunc Width() int {\n\treturn isTerminal()\n}\n"),
        ("term/term_unix.go", "//go:build !windows\n\npackage term\n\nfunc isTerminal() int {\n\treturn 1\n}\n"),
        ("term/term_windows.go", "//go:build windows\n\npackage term\n\nfunc isTerminal() int {\n\treturn 2\n}\n"),
    ]);
    let override_path = tmp.path().join(".logos/plugins/go/queries/symbols.scm");
    write(tmp.path(), ".logos/plugins/go/queries/symbols.scm", &without_clause);
    drop(index(tmp.path()));
    fs::remove_file(&override_path).unwrap();

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let root = tmp.path().canonicalize().expect("canonicalize root");
    let mut changed = Vec::new();
    for (rel, text) in [
        ("term/term.go", "package term\n\n// Width reports the width.\nfunc Width() int {\n\treturn isTerminal()\n}\n"),
        ("term/term_unix.go", "//go:build !windows\n\npackage term\n\n// isTerminal on unix.\nfunc isTerminal() int {\n\treturn 1\n}\n"),
    ] {
        write(tmp.path(), rel, text);
        changed.push(root.join(rel));
    }
    engine.sync(&changed);
    let rt = engine.runtime().unwrap();
    assert!(calls_to_name(rt, "isTerminal").is_empty(), "{:?}", calls_to_name(rt, "isTerminal"));
    assert!(unbound_call_in(rt, "term/term.go", "isTerminal"));
}
