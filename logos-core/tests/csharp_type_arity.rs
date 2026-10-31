//! A C# type name binds only a type of its own arity (S-602, CR-199,
//! FR-RS-45, FR-RS-13, FR-RS-46, NFR-RA-05, NFR-RA-06) — exercised end to end
//! through the public [`Engine`] façade against temp-directory fixtures, with
//! the SHIPPED queries and `plugin.toml` of the C# plugin.
//!
//! C# tells `Result` and `Result<T>` apart by their arity. The C# plugin
//! declares `[module_model] type_arity = true`, so each type declaration
//! records its type-parameter count and each base type or `new` its written
//! type-argument count, and the own-namespace, enclosing-namespace and
//! `using`-wildcard rungs admit only a type of the written arity. What the
//! fixtures pin:
//!
//! - the CR's shape: `Result` written in `A.Sub` binds `X.Result` through
//!   `using X;` past the generic `A.Result<T>` of the enclosing namespace, and
//!   `Result<int>` binds `A.Result<T>`; and its mirror, where the generic type
//!   is the `using`'s and the plain one the enclosing namespace's;
//! - two types of one name and different arities, in one namespace — the
//!   source's own, an enclosing one, or two `using` namespaces — are no
//!   ambiguity: each written arity binds its own (FR-RS-46 judges ambiguity
//!   among arity-matching candidates only), while two of one arity still bind
//!   nothing;
//! - a type declared in the reference's own file is matched by arity too;
//! - a count above one is matched exactly, and interfaces and records record
//!   their arity as classes do;
//! - the supertype walk climbs the base of the written arity, so an inherited
//!   call binds through the base the `Extends` edge names;
//! - every state a sync reaches, adding and removing a generic type and a
//!   same-named plain one, equals a cold index of the same tree.
//!
//! Fixtures are written inline into temp directories, like every sibling
//! binding suite: a fixture tree checked into this repository would be indexed
//! into its own graph.

#![cfg(feature = "lang-c-sharp")]

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use logos_core::model::{EdgeKind, NodeId};
use logos_core::{Engine, Runtime};
use tempfile::TempDir;

#[path = "support/graph_fingerprint.rs"]
mod graph_fingerprint;
use graph_fingerprint::graph_fingerprint;

#[path = "support/coordinate.rs"]
mod coordinate;

fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
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

/// The targets of every `kind` edge out of the declaration named `source`, as
/// `file:name`, sorted.
fn bound(rt: &Runtime, source: &str, kind: EdgeKind) -> Vec<String> {
    let source = source.to_string();
    rt.submit_read(move |store| {
        let nodes = store.all_nodes()?;
        let label: HashMap<NodeId, String> = nodes
            .iter()
            .map(|n| (n.id, format!("{}:{}", n.file_path.clone().unwrap_or_default(), n.name)))
            .collect();
        let sources: Vec<NodeId> = nodes.iter().filter(|n| n.name == source).map(|n| n.id).collect();
        let mut out: Vec<String> = store
            .all_edges()?
            .into_iter()
            .filter(|e| e.kind == kind && sources.contains(&e.source))
            .map(|e| label[&e.target].clone())
            .collect();
        out.sort();
        Ok(out)
    })
    .expect("read runs")
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

/// The CR-199 shape: a generic `A.Result<T>` in the enclosing namespace and a
/// plain `X.Result` under `using X;`.
const GENERIC_ENCLOSING: &[(&str, &str)] = &[
    ("src/A/Result.cs", "namespace A\n{\n    public class Result<T> { }\n}\n"),
    ("src/X/Result.cs", "namespace X\n{\n    public class Result { }\n}\n"),
    (
        "src/A/Sub/Impl.cs",
        "using X;\n\nnamespace A.Sub\n{\n    public class Plain : Result { }\n    public class Gen : Result<int> { }\n}\n",
    ),
];

/// `Result` written in `A.Sub` binds `X.Result` through the `using`, past the
/// generic type of the enclosing namespace; `Result<int>` binds that generic
/// type (CR-199 finding 1b).
#[test]
fn a_plain_name_passes_over_an_enclosing_generic_type_to_the_using_s() {
    let tmp = tree(GENERIC_ENCLOSING);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(bound(rt, "Plain", EdgeKind::Extends), strings(&["src/X/Result.cs:Result"]));
    assert_eq!(bound(rt, "Gen", EdgeKind::Extends), strings(&["src/A/Result.cs:Result"]));
}

/// The mirror: the generic type is the `using`'s and the plain one the
/// enclosing namespace's, so `Result<int>` passes over the enclosing level
/// and a `new Result<string>()` does too.
#[test]
fn a_generic_name_passes_over_an_enclosing_plain_type_to_the_using_s() {
    let tmp = tree(&[
        ("src/A/Result.cs", "namespace A\n{\n    public class Result { }\n}\n"),
        ("src/X/Result.cs", "namespace X\n{\n    public class Result<T> { }\n}\n"),
        (
            "src/A/Sub/Impl.cs",
            "using X;\n\nnamespace A.Sub\n{\n    public class Plain : Result { }\n    public class Gen : Result<int>\n    {\n        public void Run() { var r = new Result<string>(); }\n    }\n}\n",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(bound(rt, "Plain", EdgeKind::Extends), strings(&["src/A/Result.cs:Result"]));
    assert_eq!(bound(rt, "Gen", EdgeKind::Extends), strings(&["src/X/Result.cs:Result"]));
    assert_eq!(bound(rt, "Run", EdgeKind::Instantiates), strings(&["src/X/Result.cs:Result"]));
}

/// Two types of one name and different arities in the source's own namespace
/// are no ambiguity: each written arity binds its own. Two of one arity still
/// bind nothing (FR-RS-46).
#[test]
fn the_own_namespace_judges_ambiguity_among_types_of_the_written_arity() {
    let tmp = tree(&[
        ("src/Box.cs", "namespace App\n{\n    public class Box { }\n}\n"),
        ("src/BoxOfT.cs", "namespace App\n{\n    public class Box<T> { }\n    public class Pair<K, V> { }\n}\n"),
        ("src/Pair.cs", "namespace App\n{\n    public class Pair<A, B> { }\n}\n"),
        (
            "src/Use.cs",
            "namespace App\n{\n    public class Plain : Box { }\n    public class Gen : Box<int> { }\n    public class Two : Pair<int, string> { }\n}\n",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(bound(rt, "Plain", EdgeKind::Extends), strings(&["src/Box.cs:Box"]));
    assert_eq!(bound(rt, "Gen", EdgeKind::Extends), strings(&["src/BoxOfT.cs:Box"]));
    assert!(bound(rt, "Two", EdgeKind::Extends).is_empty(), "two `Pair<,>` bind nothing");
}

/// Types declared in the reference's own file are read first (the lexical
/// chain), and by arity too: a `Result<T>` beside `Plain : Result` does not
/// hide the `using`'s plain `X.Result`, and `Box`, `Box<T>`, `Plain : Box` and
/// `Gen : Box<int>` in one file each bind their own.
#[test]
fn a_type_of_the_same_file_binds_only_at_the_written_arity() {
    let tmp = tree(&[
        ("src/X/Result.cs", "namespace X\n{\n    public class Result { }\n}\n"),
        (
            "src/A/Impl.cs",
            "using X;\n\nnamespace A\n{\n    public class Result<T> { }\n    public class Plain : Result { }\n    public class Gen : Result<int> { }\n}\n",
        ),
        (
            "src/Box.cs",
            "namespace App\n{\n    public class Box { }\n    public class Box<T> { }\n    public class BoxPlain : Box { }\n    public class BoxGen : Box<int> { }\n}\n",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(bound(rt, "Plain", EdgeKind::Extends), strings(&["src/X/Result.cs:Result"]));
    assert_eq!(bound(rt, "Gen", EdgeKind::Extends), strings(&["src/A/Impl.cs:Result"]));
    let line = |source: &str| {
        let source = source.to_string();
        rt.submit_read(move |store| {
            let nodes = store.all_nodes()?;
            let line_of: HashMap<NodeId, Option<i64>> = nodes.iter().map(|n| (n.id, n.start_line)).collect();
            let from = nodes.iter().find(|n| n.name == source).map(|n| n.id);
            Ok(store
                .all_edges()?
                .into_iter()
                .filter(|e| e.kind == EdgeKind::Extends && Some(e.source) == from)
                .map(|e| line_of[&e.target])
                .collect::<Vec<_>>())
        })
        .expect("read runs")
    };
    assert_eq!(line("BoxPlain"), vec![Some(3)], "the plain `Box` on line 3");
    assert_eq!(line("BoxGen"), vec![Some(4)], "the generic `Box<T>` on line 4");
}

/// An enclosing namespace holding `Result` and `Result<T>` is no ambiguity
/// either: each written arity binds its own, at that level.
#[test]
fn an_enclosing_namespace_judges_ambiguity_among_types_of_the_written_arity() {
    let tmp = tree(&[
        ("src/Result.cs", "namespace A\n{\n    public class Result { }\n    public class Result<T> { }\n}\n"),
        (
            "src/Sub/Impl.cs",
            "namespace A.Sub\n{\n    public class Plain : Result { }\n    public class Gen : Result<int> { }\n}\n",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(bound(rt, "Plain", EdgeKind::Extends), strings(&["src/Result.cs:Result"]));
    assert_eq!(bound(rt, "Gen", EdgeKind::Extends), strings(&["src/Result.cs:Result"]));
    let lines = |name: &str| {
        let name = name.to_string();
        rt.submit_read(move |store| {
            let nodes = store.all_nodes()?;
            let by_id: HashMap<NodeId, u32> =
                nodes.iter().map(|n| (n.id, n.start_line.unwrap_or(0) as u32)).collect();
            let source = nodes.iter().find(|n| n.name == name).map(|n| n.id);
            Ok(store
                .all_edges()?
                .into_iter()
                .filter(|e| e.kind == EdgeKind::Extends && Some(e.source) == source)
                .map(|e| by_id[&e.target])
                .collect::<Vec<u32>>())
        })
        .expect("read runs")
    };
    assert_eq!(lines("Plain"), vec![3], "the plain `Result` on line 3");
    assert_eq!(lines("Gen"), vec![4], "the generic `Result<T>` on line 4");
}

/// Two `using` namespaces each supplying a `Result` of a different arity are
/// no ambiguity: each written arity binds its own. Two of one arity still bind
/// nothing.
#[test]
fn the_using_wildcards_judge_ambiguity_among_types_of_the_written_arity() {
    let tmp = tree(&[
        ("src/X/Result.cs", "namespace X\n{\n    public class Result<T> { }\n    public class Same { }\n}\n"),
        ("src/Y/Result.cs", "namespace Y\n{\n    public class Result { }\n    public class Same { }\n}\n"),
        (
            "src/Use.cs",
            "using X;\nusing Y;\n\nnamespace App\n{\n    public class Plain : Result { }\n    public class Gen : Result<int> { }\n    public class Both : Same { }\n}\n",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(bound(rt, "Plain", EdgeKind::Extends), strings(&["src/Y/Result.cs:Result"]));
    assert_eq!(bound(rt, "Gen", EdgeKind::Extends), strings(&["src/X/Result.cs:Result"]));
    assert!(bound(rt, "Both", EdgeKind::Extends).is_empty(), "two plain `Same` bind nothing");
}

/// The supertype walk climbs the base the `Extends` edge names: a bare call to
/// an inherited method in `Plain : Result` binds `X.Result`'s, never the
/// enclosing `A.Result<T>`'s, and one in `Gen : Result<int>` binds the generic
/// type's.
#[test]
fn an_inherited_call_climbs_the_base_of_the_written_arity() {
    let tmp = tree(&[
        ("src/A/Result.cs", "namespace A\n{\n    public class Result<T>\n    {\n        public void Done() { }\n    }\n}\n"),
        ("src/X/Result.cs", "namespace X\n{\n    public class Result\n    {\n        public void Done() { }\n    }\n}\n"),
        (
            "src/A/Sub/Impl.cs",
            "using X;\n\nnamespace A.Sub\n{\n    public class Plain : Result\n    {\n        public void RunPlain() { Done(); }\n    }\n    public class Gen : Result<int>\n    {\n        public void RunGen() { Done(); }\n    }\n}\n",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(bound(rt, "RunPlain", EdgeKind::Calls), strings(&["src/X/Result.cs:Done"]));
    assert_eq!(bound(rt, "RunGen", EdgeKind::Calls), strings(&["src/A/Result.cs:Done"]));
}

/// Every state a sync reaches — a generic type added beside the written plain
/// name, a same-named plain one added nearer, each removed again — equals a
/// cold index of the same tree (NFR-RA-06), and binds as that tree says.
#[test]
fn sync_equals_reindex_as_generic_and_plain_types_come_and_go() {
    let tmp = tree(&[
        ("src/X/Result.cs", "namespace X\n{\n    public class Result { }\n}\n"),
        (
            "src/A/Sub/Impl.cs",
            "using X;\n\nnamespace A.Sub\n{\n    public class Plain : Result { }\n    public class Gen : Result<int> { }\n}\n",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let cold = |tmp: &TempDir| {
        let copy = TempDir::new().unwrap();
        for rel in ["src/X/Result.cs", "src/A/Sub/Impl.cs", "src/A/Result.cs", "src/A/Plain.cs"] {
            if let Ok(text) = fs::read_to_string(tmp.path().join(rel)) {
                write(copy.path(), rel, &text);
            }
        }
        coordinate::pin(copy.path());
        graph_fingerprint(index(copy.path()).runtime().unwrap())
    };
    assert_eq!(bound(rt, "Plain", EdgeKind::Extends), strings(&["src/X/Result.cs:Result"]));
    assert!(bound(rt, "Gen", EdgeKind::Extends).is_empty(), "no `Result<T>` is declared");

    // A generic `A.Result<T>` binds `Result<int>` and leaves `Result` alone.
    let generic = "src/A/Result.cs";
    write(tmp.path(), generic, "namespace A\n{\n    public class Result<T> { }\n}\n");
    engine.sync(&[generic.into()]);
    assert_eq!(bound(rt, "Plain", EdgeKind::Extends), strings(&["src/X/Result.cs:Result"]));
    assert_eq!(bound(rt, "Gen", EdgeKind::Extends), strings(&["src/A/Result.cs:Result"]));
    assert_eq!(graph_fingerprint(rt), cold(&tmp));

    // A plain `A.Result` is nearer than the `using`'s.
    let plain = "src/A/Plain.cs";
    write(tmp.path(), plain, "namespace A\n{\n    public class Result { }\n}\n");
    engine.sync(&[plain.into()]);
    assert_eq!(bound(rt, "Plain", EdgeKind::Extends), strings(&["src/A/Plain.cs:Result"]));
    assert_eq!(bound(rt, "Gen", EdgeKind::Extends), strings(&["src/A/Result.cs:Result"]));
    assert_eq!(graph_fingerprint(rt), cold(&tmp));

    // The generic type turning plain makes two plain `A.Result`s.
    write(tmp.path(), generic, "namespace A\n{\n    public class Result { }\n}\n");
    engine.sync(&[generic.into()]);
    assert!(bound(rt, "Plain", EdgeKind::Extends).is_empty(), "two plain `A.Result`s bind nothing");
    assert!(bound(rt, "Gen", EdgeKind::Extends).is_empty(), "no `Result<T>` is declared");
    assert_eq!(graph_fingerprint(rt), cold(&tmp));

    // Removing both returns `Result` to the `using`'s.
    for path in [generic, plain] {
        fs::remove_file(tmp.path().join(path)).unwrap();
        engine.sync(&[path.into()]);
    }
    assert_eq!(bound(rt, "Plain", EdgeKind::Extends), strings(&["src/X/Result.cs:Result"]));
    assert!(bound(rt, "Gen", EdgeKind::Extends).is_empty());
    assert_eq!(graph_fingerprint(rt), cold(&tmp));
}

/// A count above one is matched exactly: `Pair<int>` binds the one-parameter
/// `Pair<A>` and `Pair<int, string>` the two-parameter `Pair<A, B>`.
#[test]
fn a_written_arity_of_two_binds_the_two_parameter_type() {
    let tmp = tree(&[
        ("src/Pair1.cs", "namespace App\n{\n    public class Pair<A> { }\n}\n"),
        ("src/Pair2.cs", "namespace App\n{\n    public class Pair<A, B> { }\n}\n"),
        (
            "src/Use.cs",
            "namespace App\n{\n    public class One : Pair<int> { }\n    public class Two : Pair<int, string> { }\n}\n",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(bound(rt, "One", EdgeKind::Extends), strings(&["src/Pair1.cs:Pair"]));
    assert_eq!(bound(rt, "Two", EdgeKind::Extends), strings(&["src/Pair2.cs:Pair"]));
}

/// Interfaces and records record their arity as classes do: `IRepo` and
/// `IRepo<T>`, `record Rec` and `record Rec<T>`, are each two types.
#[test]
fn interfaces_and_records_are_told_apart_by_arity() {
    let tmp = tree(&[
        ("src/I0.cs", "namespace App\n{\n    public interface IRepo { }\n}\n"),
        ("src/I1.cs", "namespace App\n{\n    public interface IRepo<T> { }\n}\n"),
        ("src/R0.cs", "namespace App\n{\n    public record Rec;\n}\n"),
        ("src/R1.cs", "namespace App\n{\n    public record Rec<T>;\n}\n"),
        (
            "src/Use.cs",
            "namespace App\n{\n    public class Plain : IRepo { }\n    public class Gen : IRepo<int> { }\n    public record RPlain : Rec;\n    public record RGen : Rec<int>;\n}\n",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(bound(rt, "Plain", EdgeKind::Implements), strings(&["src/I0.cs:IRepo"]));
    assert_eq!(bound(rt, "Gen", EdgeKind::Implements), strings(&["src/I1.cs:IRepo"]));
    assert_eq!(bound(rt, "RPlain", EdgeKind::Extends), strings(&["src/R0.cs:Rec"]));
    assert_eq!(bound(rt, "RGen", EdgeKind::Extends), strings(&["src/R1.cs:Rec"]));
}

/// A comment written inside a type-argument or type-parameter list is no type:
/// `Result<int /* the value */>` binds the one-parameter `Result<T>`, and
/// `class Box<T /* the item */>` is still `Box<int>`'s.
#[test]
fn a_comment_inside_a_type_list_is_not_counted() {
    let tmp = tree(&[
        (
            "src/R.cs",
            "namespace App\n{\n    public class Result<T> { }\n    public class Result<K, V> { }\n    public class Box<T /* the item */> { }\n}\n",
        ),
        (
            "src/Use.cs",
            "namespace App\n{\n    public class Com : Result<int /* the value */> { }\n    public class B : Box<int> { }\n}\n",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let line = |source: &str| {
        let source = source.to_string();
        rt.submit_read(move |store| {
            let nodes = store.all_nodes()?;
            let line_of: HashMap<NodeId, Option<i64>> = nodes.iter().map(|n| (n.id, n.start_line)).collect();
            let from = nodes.iter().find(|n| n.name == source).map(|n| n.id);
            Ok(store
                .all_edges()?
                .into_iter()
                .filter(|e| e.kind == EdgeKind::Extends && Some(e.source) == from)
                .map(|e| line_of[&e.target])
                .collect::<Vec<_>>())
        })
        .expect("read runs")
    };
    assert_eq!(line("Com"), vec![Some(3)], "`Result<T>` on line 3, not `Result<K, V>`");
    assert_eq!(line("B"), vec![Some(5)], "`Box<T>` on line 5");
}

/// Each type declaration records its type-parameter count, and each base type
/// or `new` its written type-argument count — what the rungs filter by.
#[test]
fn declarations_and_written_types_record_their_arity() {
    let tmp = tree(GENERIC_ENCLOSING);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let (arities, rows) = rt
        .submit_read(|store| {
            let name: HashMap<NodeId, String> = store
                .all_nodes()?
                .into_iter()
                .map(|n| (n.id, format!("{}:{}", n.file_path.unwrap_or_default(), n.name)))
                .collect();
            let mut arities: Vec<String> = store
                .type_arities()?
                .into_iter()
                .map(|(id, arity)| format!("{} {arity}", name[&id]))
                .collect();
            arities.sort();
            let mut rows: Vec<String> = store
                .unresolved_refs()?
                .into_iter()
                .filter(|r| r.kind == EdgeKind::Extends)
                .map(|r| format!("{} {:?}", r.target, r.arg_count))
                .collect();
            rows.sort();
            Ok((arities, rows))
        })
        .expect("read runs");
    assert_eq!(
        arities,
        strings(&[
            "src/A/Result.cs:Result 1",
            "src/A/Sub/Impl.cs:Gen 0",
            "src/A/Sub/Impl.cs:Plain 0",
            "src/X/Result.cs:Result 0",
        ])
    );
    assert_eq!(rows, strings(&["Result Some(0)", "Result Some(1)"]));
}
