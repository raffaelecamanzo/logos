//! C# preprocessor regions are reduced to their first branch before parsing
//! (S-555, CR-177, FR-EX-23, NFR-RA-05, NFR-RA-06) — exercised end to end
//! through the public [`Engine`] façade against temp-directory fixtures, with
//! the SHIPPED `plugin.toml` and queries of the C# plugin (no query override).
//!
//! The fixtures are the two shapes behind Newtonsoft.Json's damage:
//!
//! - `JsonWriter.Async.cs`: a base type written only `#if HAVE_ASYNC_DISPOSABLE`
//!   between a class's name and its body, and a member under the same symbol —
//!   tree-sitter read a class named `HAVE_ASYNC_DISPOSABLE` and hung the
//!   members off it;
//! - `JValue.cs`: an interface added to the base list under `#if`, and an
//!   `#if … #else … #endif` inside a method body whose two branches each open
//!   a block — the members after it landed under the file's Module.
//!
//! What it pins: no declaration is named after a preprocessor symbol, every
//! member sits under its class, every node's start and end line is the raw
//! file's, and a synced file equals a fresh index of it.
//!
//! Fixtures are written inline into temp directories, like every sibling
//! C# suite (`csharp_partial_types.rs`): a fixture tree checked into this
//! repository would be indexed into its own graph.

#![cfg(feature = "lang-c-sharp")]

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use logos_core::model::{EdgeKind, NodeId, NodeKind};
use logos_core::{Engine, Runtime};
use tempfile::TempDir;

#[path = "support/graph_fingerprint.rs"]
mod graph_fingerprint;
use graph_fingerprint::graph_fingerprint;

#[path = "support/coordinate.rs"]
mod coordinate;

const WRITER: &str = "\
#if HAVE_ASYNC
using System;
#if HAVE_BIG_INTEGER
using System.Numerics;
#endif
using System.Threading.Tasks;

namespace App
{
    public abstract partial class JsonWriter
#if HAVE_ASYNC_DISPOSABLE
        : IAsyncDisposable
#endif
    {
#if HAVE_ASYNC_DISPOSABLE
        async ValueTask IAsyncDisposable.DisposeAsync()
        {
            await CloseAsync();
        }
#endif

        public virtual Task CloseAsync()
        {
            return Task.CompletedTask;
        }

        public virtual Task FlushAsync()
        {
            return Task.CompletedTask;
        }
    }
}
#endif
";

const VALUE: &str = "\
namespace App
{
    public partial class JValue : JToken, IComparable
#if HAVE_ICONVERTIBLE
        , IConvertible
#endif
    {
        private object _value;

        private static int Compare(object a, object b)
        {
#if HAVE_BIG_INTEGER
            if (a is BigInteger big)
            {
#else
            if (a is long big)
            {
#endif
                return 1;
            }
            return 0;
        }

        private static int CompareFloat(object a, object b)
        {
            return 0;
        }

        public object Value
        {
            get { return _value; }
        }
    }
}
";

fn tree(files: &[(&str, &str)]) -> TempDir {
    let tmp = TempDir::new().unwrap();
    for (rel, text) in files {
        let path = tmp.path().join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    coordinate::pin(tmp.path());
    tmp
}

/// Index `root`, returning the engine and the index's warnings.
fn index_warned(root: &Path) -> (Engine, Vec<String>) {
    let engine = Engine::start(root).expect("engine starts");
    let warnings = engine.index().warnings;
    (engine, warnings)
}

fn index(root: &Path) -> Engine {
    index_warned(root).0
}

/// One extracted node of a fixture file: its name, kind, lines and the name of
/// the node that contains it.
#[derive(Debug)]
struct Row {
    name: String,
    kind: NodeKind,
    lines: (i64, i64),
    parent: Option<(String, NodeKind)>,
}

/// Every node of `file`, with the node a `Contains` edge hangs it under.
fn rows(rt: &Runtime, file: &str) -> Vec<Row> {
    let nodes = rt.submit_read(|store| store.all_nodes()).expect("read runs");
    let edges = rt.submit_read(|store| store.all_edges()).expect("read runs");
    let by_id: HashMap<NodeId, _> = nodes.iter().map(|n| (n.id, n)).collect();
    let parent: HashMap<NodeId, NodeId> = edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Contains)
        .map(|e| (e.target, e.source))
        .collect();
    nodes
        .iter()
        .filter(|n| n.file_path.as_deref() == Some(file))
        .map(|n| Row {
            name: n.name.clone(),
            kind: n.kind,
            lines: (n.start_line.unwrap_or(0), n.end_line.unwrap_or(0)),
            parent: parent.get(&n.id).map(|p| (by_id[p].name.clone(), by_id[p].kind)),
        })
        .collect()
}

/// The 1-based line of `raw` on which `needle` is first written.
fn line_of(raw: &str, needle: &str) -> i64 {
    let at = raw.find(needle).unwrap_or_else(|| panic!("{needle:?} is in the fixture"));
    raw[..at].matches('\n').count() as i64 + 1
}

/// The 1-based line of the `n`th line of `raw` that is exactly `text` after trimming.
fn nth_line_equal(raw: &str, text: &str, n: usize) -> i64 {
    raw.lines()
        .enumerate()
        .filter(|(_, l)| l.trim() == text)
        .nth(n)
        .map(|(i, _)| i as i64 + 1)
        .unwrap_or_else(|| panic!("{text:?} #{n} is in the fixture"))
}

fn fixture() -> (TempDir, Engine) {
    let tmp = tree(&[("src/JsonWriter.Async.cs", WRITER), ("src/JValue.cs", VALUE)]);
    let engine = index(tmp.path());
    (tmp, engine)
}

const SYMBOLS: [&str; 4] = ["HAVE_ASYNC", "HAVE_BIG_INTEGER", "HAVE_ASYNC_DISPOSABLE", "HAVE_ICONVERTIBLE"];

#[test]
fn a_file_whose_regions_are_reduced_parses_without_damage() {
    let tmp = tree(&[("src/JsonWriter.Async.cs", WRITER), ("src/JValue.cs", VALUE)]);
    let (_engine, warnings) = index_warned(tmp.path());
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn no_declaration_is_named_after_a_preprocessor_symbol() {
    let (_tmp, engine) = fixture();
    let rt = engine.runtime().unwrap();
    for file in ["src/JsonWriter.Async.cs", "src/JValue.cs"] {
        let phantoms: Vec<_> = rows(rt, file).into_iter().filter(|r| SYMBOLS.contains(&r.name.as_str())).collect();
        assert!(phantoms.is_empty(), "{file}: {phantoms:?}");
    }
}

#[test]
fn a_file_opening_with_a_byte_order_mark_is_reduced_too() {
    // Visual Studio saves C# files with a BOM; the first `#if` follows it.
    let text = format!("\u{feff}{WRITER}");
    let tmp = tree(&[("src/JsonWriter.Async.cs", text.as_str())]);
    let (engine, warnings) = index_warned(tmp.path());
    assert!(warnings.is_empty(), "{warnings:?}");
    let rows = rows(engine.runtime().unwrap(), "src/JsonWriter.Async.cs");
    let dispose = rows.iter().find(|r| r.name == "DisposeAsync").expect("DisposeAsync is extracted");
    assert_eq!(dispose.parent, Some(("JsonWriter".to_string(), NodeKind::Class)));
    assert!(rows.iter().all(|r| !SYMBOLS.contains(&r.name.as_str())), "{rows:?}");
}

#[test]
fn every_member_sits_under_its_class() {
    let (_tmp, engine) = fixture();
    let rt = engine.runtime().unwrap();
    let cases = [
        ("src/JsonWriter.Async.cs", "JsonWriter", &["DisposeAsync", "CloseAsync", "FlushAsync"][..]),
        ("src/JValue.cs", "JValue", &["_value", "Compare", "CompareFloat", "Value"][..]),
    ];
    for (file, class, members) in cases {
        let rows = rows(rt, file);
        for member in members {
            let row = rows
                .iter()
                .find(|r| r.name == *member)
                .unwrap_or_else(|| panic!("{file}: {member} is extracted: {rows:?}"));
            assert_eq!(
                row.parent,
                Some((class.to_string(), NodeKind::Class)),
                "{file}: {member} sits under {class}"
            );
        }
    }
}

#[test]
fn every_nodes_lines_are_the_raw_files() {
    let (_tmp, engine) = fixture();
    let rt = engine.runtime().unwrap();
    let writer = rows(rt, "src/JsonWriter.Async.cs");
    let value = rows(rt, "src/JValue.cs");
    // A declaration's lines; the file's Module (named after the file, as
    // `JValue` is) is read apart.
    let lines = |rows: &[Row], name: &str| {
        rows.iter().find(|r| r.name == name && r.kind != NodeKind::Module).map(|r| r.lines)
    };
    let module_end = |rows: &[Row]| rows.iter().find(|r| r.kind == NodeKind::Module).map(|r| r.lines.1);
    let expected = [
        (&writer, WRITER, "JsonWriter", "class JsonWriter", ("}", 3)),
        (&writer, WRITER, "DisposeAsync", "async ValueTask", ("}", 0)),
        (&writer, WRITER, "CloseAsync", "Task CloseAsync", ("}", 1)),
        (&value, VALUE, "JValue", "class JValue", ("}", 4)),
        (&value, VALUE, "Compare", "int Compare(", ("}", 1)),
        (&value, VALUE, "CompareFloat", "int CompareFloat", ("}", 2)),
    ];
    for (rows, raw, name, start, (end, nth)) in expected {
        assert_eq!(
            lines(rows, name),
            Some((line_of(raw, start), nth_line_equal(raw, end, nth))),
            "{name}'s lines are the raw file's"
        );
    }
    // The Module spans every line the raw file has.
    assert_eq!(module_end(&writer), Some(WRITER.lines().count() as i64));
    assert_eq!(module_end(&value), Some(VALUE.lines().count() as i64));
}

#[test]
fn a_synced_file_with_a_region_matches_a_fresh_reindex() {
    // The file starts without its regions; the edit adds them.
    let plain = "namespace App\n{\n    public abstract partial class JsonWriter\n    {\n        public virtual Task CloseAsync() { return null; }\n    }\n}\n";
    let tmp_a = tree(&[("src/JsonWriter.Async.cs", plain), ("src/JValue.cs", VALUE)]);
    let engine_a = index(tmp_a.path());
    let path = tmp_a.path().join("src/JsonWriter.Async.cs");
    fs::write(&path, WRITER).unwrap();
    engine_a.sync(&[path.canonicalize().unwrap()]);
    // The synced file is reduced too: its member sits under its class.
    let synced = rows(engine_a.runtime().unwrap(), "src/JsonWriter.Async.cs");
    let close = synced.iter().find(|r| r.name == "CloseAsync").expect("CloseAsync is extracted");
    assert_eq!(close.parent, Some(("JsonWriter".to_string(), NodeKind::Class)));

    let tmp_b = tree(&[("src/JsonWriter.Async.cs", WRITER), ("src/JValue.cs", VALUE)]);
    let engine_b = index(tmp_b.path());
    assert_eq!(
        graph_fingerprint(engine_a.runtime().unwrap()),
        graph_fingerprint(engine_b.runtime().unwrap()),
        "sync-to-state must equal index-of-state"
    );
}
