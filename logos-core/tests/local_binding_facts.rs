//! A callable's local bindings are persisted beside the graph and move nothing
//! in it (S-593, CR-191, FR-EX-33, NFR-RA-06) — exercised end to end through
//! the public [`Engine`] façade against temp-directory fixtures.
//!
//! Migration 41 records every name a callable body binds (`local_bindings`):
//! its kind, position, scope span and hoisting. No node, symbol, edge or
//! reference reads it, so the graph a fresh index builds is the graph it built
//! with the `locals` queries emptied. A one-file edit re-derives the file's
//! bindings through sync to exactly what a fresh index of the edited tree
//! records, a removed file takes its bindings with it, and the sync ≡ reindex
//! fingerprint sees each binding.
//!
//! Fixtures are written inline into temp directories, like every sibling
//! suite here: a fixture tree checked into this repository would be indexed
//! into its own graph.

#![cfg(all(feature = "lang-go", feature = "lang-python", feature = "lang-typescript"))]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use logos_core::model::LocalBindingKind;
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

/// The fixture tree, its repository coordinate pinned (S-703) so two trees'
/// stores hold the same symbols whatever their temp directories are called.
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

const GO: &str = "svc/svc.go";
const GO_SRC: &str = "package svc

type Svc struct{}

func (s *Svc) CloseAll(xs []Svc) {
	for _, s := range xs {
		s.Close()
	}
}

func (s *Svc) Close() {}
";

const PY: &str = "app/run.py";
const PY_SRC: &str = "def run(a):
    from m import f
    f(a)
";

const TS: &str = "web/main.ts";
const TS_SRC: &str = "export function main() {
  helper();
  var helper = () => 1;
}
";

/// One recorded binding as `file name kind line:col in start-end [hoisted]`.
fn bindings(rt: &Runtime) -> Vec<String> {
    rt.submit_read(|store| store.local_bindings())
        .expect("read runs")
        .into_iter()
        .map(|b| {
            format!(
                "{} {} {} {}:{} in {}:{}-{}:{}{}",
                b.file_path,
                b.name,
                b.kind.as_str(),
                b.line,
                b.column,
                b.scope_start_line,
                b.scope_start_column,
                b.scope_end_line,
                b.scope_end_column,
                if b.hoisted { " hoisted" } else { "" }
            )
        })
        .collect()
}

/// S-593 / FR-EX-33: every binding a callable body makes is persisted with its
/// kind, position, scope span and hoisting, ordered by file and position — the
/// Go receiver's name rebound by a `range` (never the receiver itself), a
/// Python local import, a JavaScript `var` hoisted over a call before it.
#[test]
fn every_local_binding_is_persisted_with_its_kind_scope_and_position() {
    let tmp = tree(&[(GO, GO_SRC), (PY, PY_SRC), (TS, TS_SRC)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        bindings(rt),
        [
            "app/run.py a parameter 1:8 in 1:0-3:8",
            "app/run.py f import 2:18 in 1:0-3:8 hoisted",
            "svc/svc.go xs parameter 5:23 in 5:0-9:1",
            "svc/svc.go s variable 6:8 in 6:1-8:2",
            "web/main.ts helper variable 3:6 in 1:7-4:1 hoisted",
        ]
    );
    let kinds: Vec<LocalBindingKind> = rt
        .submit_read(|store| store.local_bindings())
        .unwrap()
        .into_iter()
        .map(|b| b.kind)
        .collect();
    assert_eq!(
        kinds,
        [
            LocalBindingKind::Parameter,
            LocalBindingKind::Import,
            LocalBindingKind::Parameter,
            LocalBindingKind::Variable,
            LocalBindingKind::Variable,
        ]
    );
}

/// S-593 × S-538: a `.js` file, labelled `javascript` since S-538, records its
/// bindings as it did under the TypeScript plugin — the `javascript` descriptor
/// declares the `locals` capability and runs the shared TSX `locals` query.
#[test]
fn a_javascript_file_records_its_local_bindings() {
    let tmp = tree(&[("web/main.js", TS_SRC)]);
    let engine = index(tmp.path());
    assert_eq!(bindings(engine.runtime().unwrap()), ["web/main.js helper variable 3:6 in 1:7-4:1 hoisted"]);
}

/// The fingerprint with its local-binding section (`L` lines) left out.
fn without_bindings(fingerprint: &str) -> String {
    fingerprint.lines().filter(|l| !l.starts_with("L ")).collect::<Vec<_>>().join("\n")
}

/// S-593 AC2, as S-594 amends it: recording the bindings moves no node,
/// symbol, impl block or annotation, and an edge or a reference only as the
/// shadowing rules decide. The same tree indexed with every `locals` query
/// emptied by an on-disk override records no binding, and differs exactly by
/// the rows those rules move — pinned here line by line.
#[test]
fn the_bindings_move_only_the_shadowed_calls() {
    let files = [(GO, GO_SRC), (PY, PY_SRC), (TS, TS_SRC)];
    let with = tree(&files);
    let engine = index(with.path());
    let recorded = graph_fingerprint(engine.runtime().unwrap());
    assert!(recorded.lines().any(|l| l.starts_with("L ")), "the fixture records bindings");

    let emptied = tree(&files);
    for lang in ["go", "python", "typescript"] {
        write(emptied.path(), &format!(".logos/plugins/{lang}/queries/locals.scm"), "");
    }
    let engine = index(emptied.path());
    let bare = graph_fingerprint(engine.runtime().unwrap());
    assert!(!bare.lines().any(|l| l.starts_with("L ")), "an emptied query records none");
    let (recorded, bare) = (without_bindings(&recorded), without_bindings(&bare));
    let only = |a: &str, b: &str| -> Vec<String> {
        let other: std::collections::BTreeSet<&str> = b.lines().collect();
        a.lines().filter(|l| !other.contains(l)).map(str::to_string).collect()
    };
    let fixture = "logos . example.com/fixture .";
    // Go: the `range` variable rebinds `s`, so `s.Close()` is `other` and no
    // longer the receiver's `Self::Close`. Python: the local `from m import f`
    // marks `f(a)` a call through it. The TypeScript `var helper = () => 1` is a
    // declaration the graph records, so `helper()` still binds it.
    let run_f = |alias: &str| {
        format!("R {PY}|{fixture} app/`run.py`/run().|f|{alias}|Path|Calls|false|None|None|None|Some(1)|None|Some(3)")
    };
    let close_all = format!("{fixture} svc/`svc.go`/CloseAll().");
    assert_eq!(
        only(&recorded, &bare),
        vec![
            run_f("Some(\"local-import\")"),
            format!("R {GO}|{close_all}|Close|None|Method|Calls|false|None|Some(Other)|None|Some(0)|None|Some(7)"),
        ]
    );
    assert_eq!(
        only(&bare, &recorded),
        vec![
            format!("E {close_all} -> {fixture} svc/`svc.go`/Close(). [Calls]"),
            run_f("None"),
            format!("R {GO}|{close_all}|Self::Close|None|Path|Calls|true|None|None|None|Some(0)|None|Some(7)"),
        ]
    );
}

// ── Sync ≡ reindex ────────────────────────────────────────────────────────

/// Index `initial`, overwrite `edits` (an empty text removes the file) and sync
/// them; then index the post-edit tree from scratch. The two graphs must be
/// identical; the synced store's bindings are returned.
fn synced_equals_reindexed(initial: &[(&str, &str)], edits: &[(&str, &str)]) -> Vec<String> {
    let tmp_a = tree(initial);
    let engine_a = index(tmp_a.path());
    let root_a = tmp_a.path().canonicalize().expect("canonicalize root");
    let mut changed: Vec<PathBuf> = Vec::new();
    for (rel, text) in edits {
        if text.is_empty() {
            fs::remove_file(tmp_a.path().join(rel)).unwrap();
        } else {
            fs::write(tmp_a.path().join(rel), text).unwrap();
        }
        changed.push(root_a.join(rel));
    }
    engine_a.sync(&changed);
    let rt_a = engine_a.runtime().unwrap();

    let mut final_state: BTreeMap<&str, &str> = initial.iter().copied().collect();
    final_state.extend(edits.iter().copied());
    let files: Vec<(&str, &str)> = final_state.into_iter().filter(|(_, text)| !text.is_empty()).collect();
    let tmp_b = tree(&files);
    let engine_b = index(tmp_b.path());
    assert_eq!(
        graph_fingerprint(rt_a),
        graph_fingerprint(engine_b.runtime().unwrap()),
        "sync-to-state must equal index-of-state"
    );
    bindings(rt_a)
}

/// S-593 / NFR-RA-06: a binding added, renamed or moved in one file, and a
/// file removed, are recorded by sync exactly as a fresh index of the edited
/// tree records them; the other files' bindings are untouched.
#[test]
fn an_edited_or_removed_file_syncs_its_bindings_as_a_fresh_index() {
    let base = [(GO, GO_SRC), (PY, PY_SRC)];

    let renamed = GO_SRC.replace("for _, s := range xs {\n\t\ts.Close()", "for _, t := range xs {\n\t\tt.Close()");
    let rows = synced_equals_reindexed(&base, &[(GO, &renamed)]);
    assert!(rows.contains(&"svc/svc.go t variable 6:8 in 6:1-8:2".to_string()), "renamed: {rows:?}");
    assert!(!rows.iter().any(|r| r.starts_with("svc/svc.go s ")), "the old name is gone: {rows:?}");
    assert!(rows.iter().any(|r| r.starts_with("app/run.py f import")), "the other file is untouched");

    let added = PY_SRC.replace("    f(a)\n", "    g = a\n    f(g)\n");
    let rows = synced_equals_reindexed(&base, &[(PY, &added)]);
    assert!(rows.contains(&"app/run.py g variable 3:4 in 1:0-4:8 hoisted".to_string()), "added: {rows:?}");

    let rows = synced_equals_reindexed(&base, &[(PY, "")]);
    assert!(rows.iter().all(|r| !r.starts_with("app/run.py")), "removed: {rows:?}");
    assert!(rows.iter().any(|r| r.starts_with("svc/svc.go")), "the other file is untouched");
}

/// S-593: the sync ≡ reindex fingerprint sees every column of a binding — a
/// store that differs from a fresh index in any one fingerprints differently,
/// so a sync that lost one could never pass as equal. Each is changed in place
/// in an indexed store, and the fingerprint read again.
#[test]
fn the_graph_fingerprint_sees_every_local_binding_column() {
    let mutations = [
        ("name", "UPDATE local_bindings SET name = 'z' WHERE name = 's'"),
        ("kind", "UPDATE local_bindings SET kind = 1 WHERE name = 's'"),
        ("line", "UPDATE local_bindings SET line = 7 WHERE name = 's'"),
        ("column", "UPDATE local_bindings SET col = 9 WHERE name = 's'"),
        ("scope start line", "UPDATE local_bindings SET scope_start_line = 5 WHERE name = 's'"),
        ("scope start column", "UPDATE local_bindings SET scope_start_col = 0 WHERE name = 's'"),
        ("scope end line", "UPDATE local_bindings SET scope_end_line = 9 WHERE name = 's'"),
        ("scope end column", "UPDATE local_bindings SET scope_end_col = 0 WHERE name = 's'"),
        ("hoisting", "UPDATE local_bindings SET hoisted = 1 WHERE name = 's'"),
        ("binding", "DELETE FROM local_bindings WHERE name = 's'"),
    ];
    for (column, sql) in mutations {
        let tmp = tree(&[(GO, GO_SRC)]);
        let engine = index(tmp.path());
        let before = graph_fingerprint(engine.runtime().unwrap());
        drop(engine);
        let conn = rusqlite::Connection::open(tmp.path().join(".logos").join("logos.db")).unwrap();
        assert_eq!(conn.execute(sql, []).unwrap(), 1, "{column}: the fixture holds exactly one such binding");
        drop(conn);
        let engine = Engine::start(tmp.path()).expect("engine restarts");
        let after = graph_fingerprint(engine.runtime().unwrap());
        assert_ne!(before, after, "the fingerprint must see a changed {column}");
    }
}
