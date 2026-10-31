//! JavaScript is its own language, parsed with the JSX-capable grammar
//! (S-538, [FR-PL-08], [CR-174]) — exercised end-to-end through the public
//! [`Engine`] façade against real temp-directory fixtures.
//!
//! Before S-538 the `typescript` plugin claimed `.js`/`.mjs`/`.cjs` and parsed
//! them with the non-JSX TypeScript grammar, so JSX in a `.js` file was a
//! syntax error and a React component rendered there bound nothing; the `tsx`
//! plugin claimed `.jsx`. Every JavaScript figure was folded into those two
//! rows. A `javascript` plugin now claims all four extensions and parses them
//! with the TSX grammar, sharing the `tsx` plugin's queries.
//!
//! [FR-PL-08]: ../../docs/specs/requirements/FR-PL-08.md
//! [CR-174]: ../../docs/requests/CR-174-javascript-is-its-own-language-and-its-node-idioms-resolve.md

#![cfg(feature = "lang-typescript")]

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use logos_core::model::{EdgeKind, NodeId};
use logos_core::models::navigation::LanguageResolution;
use logos_core::{Engine, Runtime};
use rusqlite::{Connection, OpenFlags};
use tempfile::TempDir;

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// A JSX component library written as plain `.js`, the shape preact and most
/// React code bases ship: the component is declared in one file and rendered
/// in another through an ESM import.
fn jsx_in_js(root: &Path) {
    write(
        root,
        "src/Button.js",
        "export function Button({ label }) {\n  return <button className=\"btn\">{label}</button>;\n}\n",
    );
    write(
        root,
        "src/App.js",
        "import { Button } from './Button';\n\nexport function App() {\n  return (\n    <div>\n      <Button label=\"go\" />\n    </div>\n  );\n}\n",
    );
}

/// The `language` column of every indexed file, by path.
fn file_languages(root: &Path) -> BTreeMap<String, String> {
    let conn = Connection::open_with_flags(root.join(".logos/logos.db"), OpenFlags::SQLITE_OPEN_READ_ONLY)
        .unwrap();
    let mut stmt = conn.prepare("SELECT path, language FROM files WHERE language IS NOT NULL").unwrap();
    stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

/// Every `Calls` edge as `(source file:name, target file:name)`, sorted.
fn calls(rt: &Runtime) -> Vec<(String, String)> {
    rt.submit_read(move |store| {
        let label: BTreeMap<NodeId, String> = store
            .all_nodes()?
            .into_iter()
            .map(|n| (n.id, format!("{}:{}", n.file_path.unwrap_or_default(), n.name)))
            .collect();
        let mut out: Vec<(String, String)> = store
            .all_edges()?
            .into_iter()
            .filter(|e| e.kind == EdgeKind::Calls)
            .map(|e| (label[&e.source].clone(), label[&e.target].clone()))
            .collect();
        out.sort();
        Ok(out)
    })
    .expect("read runs")
}

fn status_row(engine: &Engine, language: &str) -> Option<LanguageResolution> {
    engine
        .status()
        .resolution_by_language
        .into_iter()
        .find(|row| row.language == language)
}

#[test]
fn every_javascript_extension_is_labelled_javascript_and_typescript_keeps_its_own() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    for (rel, src) in [
        ("lib/a.js", "export function a() {}\n"),
        ("lib/b.mjs", "export function b() {}\n"),
        ("lib/c.cjs", "function c() {}\nmodule.exports = { c };\n"),
        ("lib/d.jsx", "export function D() {\n  return <p />;\n}\n"),
        ("lib/e.ts", "export function e(): number {\n  return 1;\n}\n"),
        ("lib/f.tsx", "export function F() {\n  return <p />;\n}\n"),
    ] {
        write(root, rel, src);
    }
    Engine::start(root).expect("engine starts").index();

    let want: BTreeMap<String, String> = [
        ("lib/a.js", "javascript"),
        ("lib/b.mjs", "javascript"),
        ("lib/c.cjs", "javascript"),
        ("lib/d.jsx", "javascript"),
        ("lib/e.ts", "typescript"),
        ("lib/f.tsx", "tsx"),
    ]
    .into_iter()
    .map(|(p, l)| (p.to_string(), l.to_string()))
    .collect();
    assert_eq!(file_languages(root), want);
}

#[test]
fn a_js_file_containing_jsx_parses_cleanly_and_its_component_call_binds() {
    let tmp = TempDir::new().unwrap();
    jsx_in_js(tmp.path());
    let engine = Engine::start(tmp.path()).expect("engine starts");
    let result = engine.index();

    let syntax: Vec<&String> = result.warnings.iter().filter(|w| w.contains("syntax error")).collect();
    assert!(syntax.is_empty(), "JSX in a `.js` file is no syntax error: {syntax:?}");
    assert!(
        calls(engine.runtime().unwrap()).contains(&("src/App.js:App".to_string(), "src/Button.js:Button".to_string())),
        "`<Button />` in App.js binds the imported component: {:?}",
        calls(engine.runtime().unwrap())
    );
}

/// `status --json`'s per-language readout ([FR-RS-09]) reports JavaScript as
/// its own row, and the TypeScript rows no longer count the `.js` files.
///
/// [FR-RS-09]: ../../docs/specs/requirements/FR-RS-09.md
#[test]
fn status_reports_a_javascript_row_and_the_typescript_rows_count_no_js_file() {
    let tmp = TempDir::new().unwrap();
    jsx_in_js(tmp.path());
    write(tmp.path(), "src/util.ts", "export function util(): number {\n  return 1;\n}\n");
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.index();

    let js = status_row(&engine, "javascript").expect("status reports a javascript row");
    assert_eq!(js.files, 2, "Button.js and App.js: {js:?}");
    assert!(js.calls.cross_file_edges >= Some(1), "the component call is a JavaScript cross-file edge: {js:?}");
    let ts = status_row(&engine, "typescript").expect("status reports a typescript row");
    assert_eq!(ts.files, 1, "util.ts alone: {ts:?}");
    assert!(status_row(&engine, "tsx").is_none(), "no `.tsx` file, so no tsx row");

    let json = serde_json::to_value(engine.status()).expect("status serialises");
    assert!(
        json["resolution_by_language"]
            .as_array()
            .expect("an array of rows")
            .iter()
            .any(|row| row["language"] == "javascript"),
        "the serialised readout carries the javascript row: {}",
        json["resolution_by_language"]
    );
}

/// Sync ≡ reindex ([NFR-RA-06]): a `.js` file edited to render another
/// component, and a `.jsx` file added beside it, are synced into exactly the
/// labels and calls a cold index of the edited tree produces.
///
/// [NFR-RA-06]: ../../docs/specs/requirements/NFR-RA-06.md
#[test]
fn a_synced_javascript_edit_equals_a_fresh_index() {
    let badge = "export function Badge() {\n  return <span />;\n}\n";
    let app = "import { Badge } from './Badge';\n\nexport function App() {\n  return <Badge />;\n}\n";

    let live = TempDir::new().unwrap();
    jsx_in_js(live.path());
    let engine = Engine::start(live.path()).expect("engine starts");
    engine.index();
    write(live.path(), "src/Badge.jsx", badge);
    write(live.path(), "src/App.js", app);
    engine.sync(&["src/Badge.jsx".into(), "src/App.js".into()]);

    let fresh = TempDir::new().unwrap();
    jsx_in_js(fresh.path());
    write(fresh.path(), "src/Badge.jsx", badge);
    write(fresh.path(), "src/App.js", app);
    let cold = Engine::start(fresh.path()).expect("engine starts");
    cold.index();

    assert_eq!(file_languages(live.path()), file_languages(fresh.path()));
    assert_eq!(file_languages(live.path())["src/Badge.jsx"], "javascript");
    let synced = calls(engine.runtime().unwrap());
    assert_eq!(synced, calls(cold.runtime().unwrap()));
    assert!(
        synced.contains(&("src/App.js:App".to_string(), "src/Badge.jsx:Badge".to_string())),
        "{synced:?}"
    );
}

/// A store indexed before S-538 holds its `.js` rows as `typescript`, and no
/// migration of this story's own rewrites them: re-extraction does. A synced
/// edit relabels the edited file, and a full index relabels every file.
#[test]
fn re_extraction_relabels_a_store_whose_javascript_rows_read_typescript() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    write(root, "lib/a.js", "export function a() {}\n");
    write(root, "lib/b.js", "export function b() {}\n");
    Engine::start(root).expect("engine starts").index();
    {
        let conn = Connection::open(root.join(".logos/logos.db")).unwrap();
        conn.execute("UPDATE files SET language = 'typescript' WHERE path LIKE '%.js'", [])
            .unwrap();
    }
    let lang = |rel: &str| file_languages(root)[rel].clone();
    assert_eq!(lang("lib/a.js"), "typescript", "the pre-S-538 store this test starts from");

    let engine = Engine::start(root).expect("engine restarts");
    write(root, "lib/a.js", "export function a() {\n  return 1;\n}\n");
    engine.sync(&["lib/a.js".into()]);
    assert_eq!(lang("lib/a.js"), "javascript", "a synced edit relabels its file");

    engine.index();
    assert_eq!(lang("lib/b.js"), "javascript", "a full index relabels every file");
}
