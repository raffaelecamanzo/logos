//! Unit tests for the declared-type reader (S-472).
//!
//! In their own file, the `build_manifest_tests.rs` shape. One fixture per
//! declaration kind in each tree and each language, per refusal, and per Avro
//! naming path. The Java fixtures are the shape that dominates the reference
//! estate (1,891 main-tree declaring files, every one agreeing with its
//! directory, S-471 2026-09-28): one top-level type per file under a Maven
//! `src/main/java` root, whose `package` statement is its directory. Every
//! resolving case has a near miss beside it — the refusals are what the story's
//! second clause is about.

use super::*;

use crate::extract::{extract_files, FileInput, SymbolContext};
use crate::plugin::LanguageRegistry;

/// The loaded registry, built once per test binary.
fn registry() -> &'static LanguageRegistry {
    static ONCE: std::sync::OnceLock<LanguageRegistry> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| {
        let tmp = tempfile::tempdir().expect("tempdir");
        LanguageRegistry::load(tmp.path()).expect("registry loads")
    })
}

/// Extract one file through the product's multi-file entry point and return
/// its declared types.
fn declared(path: &str, source: &str) -> Vec<SourceType> {
    let facts = extract_files(
        &[FileInput::new(path, source)],
        registry(),
        &SymbolContext::default(),
    );
    assert_eq!(facts.len(), 1, "{path} is extracted");
    facts.into_iter().next().unwrap().declared_types
}

/// `(fqn or refusal, kind, tree)` per declared type — the projection most
/// assertions compare.
fn summary(types: &[SourceType]) -> Vec<(Result<&str, &str>, &'static str, &'static str)> {
    types
        .iter()
        .map(|t| (t.fqn.as_deref().map_err(String::as_str), t.kind.as_str(), t.tree.as_str()))
        .collect()
}

fn avro(text: &str) -> SchemaFacts {
    schema_facts("src/main/avro/events.avsc", text)
}

fn avro_names(text: &str) -> Vec<(String, &'static str)> {
    let facts = avro(text);
    assert_eq!(facts.status, SchemaStatus::Read, "{:?}", facts.detail);
    facts.types.iter().map(|t| (t.fqn.clone(), t.kind.as_str())).collect()
}

fn malformed(text: &str) -> String {
    let facts = avro(text);
    assert_eq!(facts.status, SchemaStatus::Malformed, "{text}");
    assert!(facts.types.is_empty(), "a malformed schema yields no type: {:?}", facts.types);
    facts.detail.expect("a refusal carries its reason")
}

// ── schema identity ──────────────────────────────────────────────────────────

#[test]
fn a_schema_is_named_by_its_exact_extension_and_nothing_one_character_off() {
    for schema in ["a.avsc", "src/main/avro/Event.avsc", "x/y/.z.avsc"] {
        assert!(is_avro_schema(schema), "{schema} is a schema");
    }
    for near_miss in [
        "a.avsc.bak",
        "a.AVSC",
        "a.avs",
        "a.avscx",
        "a.avpr",
        "a.avdl",
        "avsc",
        ".avsc",
        "dir/.avsc",
        "a.avsc/b.json",
        "aavsc",
    ] {
        assert!(!is_avro_schema(near_miss), "{near_miss} is not a schema");
    }
}

// ── Java ─────────────────────────────────────────────────────────────────────

#[test]
fn a_java_class_interface_enum_and_record_are_named_by_their_package_in_the_main_tree() {
    for (file, source, kind) in [
        ("Svc", "package com.x.core;\npublic class Svc {}\n", "class"),
        ("Port", "package com.x.core;\npublic interface Port {}\n", "interface"),
        ("Mode", "package com.x.core;\npublic enum Mode { A, B }\n", "enum"),
        // A record is captured as a class (the grammar's `record_declaration`).
        ("Point", "package com.x.core;\npublic record Point(int x, int y) {}\n", "class"),
    ] {
        let path = format!("svc/src/main/java/com/x/core/{file}.java");
        let types = declared(&path, source);
        let fqn = format!("com.x.core.{file}");
        assert_eq!(summary(&types), vec![(Ok(fqn.as_str()), kind, "main")], "{path}");
        assert_eq!(types[0].name, file);
    }
}

#[test]
fn a_java_type_carries_its_nodes_symbol() {
    let path = "src/main/java/com/x/Svc.java";
    let facts = extract_files(
        &[FileInput::new(path, "package com.x;\npublic class Svc { void run() {} }\n")],
        registry(),
        &SymbolContext::default(),
    );
    let node = facts[0]
        .nodes
        .iter()
        .find(|n| n.name == "Svc" && n.kind == NodeKind::Class)
        .expect("the class node — the file module shares its name");
    assert_eq!(facts[0].declared_types.len(), 1);
    assert_eq!(facts[0].declared_types[0].symbol, node.symbol, "the fact names the node it declares");
}

#[test]
fn a_java_type_in_the_test_tree_is_tagged_test() {
    let types = declared(
        "svc/src/test/java/com/x/SvcTest.java",
        "package com.x;\nclass SvcTest {}\n",
    );
    assert_eq!(summary(&types), vec![(Ok("com.x.SvcTest"), "class", "test")]);
    // A `test` PACKAGE segment under the main root is still the main tree: the
    // tree is the root's, never a substring of the path.
    let types = declared(
        "src/main/java/com/test/Fixtures.java",
        "package com.test;\npublic class Fixtures {}\n",
    );
    assert_eq!(summary(&types), vec![(Ok("com.test.Fixtures"), "class", "main")]);
}

#[test]
fn only_top_level_types_are_declarations_of_the_package() {
    let source = "package com.x;\n\
                  public class Outer {\n\
                      public static class Inner {}\n\
                      interface Port {}\n\
                      enum Mode { A }\n\
                      int field;\n\
                      void method() {}\n\
                  }\n\
                  class Sibling {}\n";
    let types = declared("src/main/java/com/x/Outer.java", source);
    assert_eq!(
        summary(&types),
        vec![(Ok("com.x.Outer"), "class", "main"), (Ok("com.x.Sibling"), "class", "main")],
        "nested types, fields and methods are not package declarations; a second top-level type is"
    );
}

#[test]
fn a_package_that_disagrees_with_the_directory_is_refused_with_both_named() {
    let types = declared(
        "src/main/java/com/x/Svc.java",
        "package com.y;\npublic class Svc {}\n",
    );
    assert_eq!(types.len(), 1, "the type is recorded — refused, not dropped");
    let reason = types[0].fqn.as_ref().expect_err("never resolved to the path");
    assert!(
        reason.contains("package `com.y`") && reason.contains("package `com.x`"),
        "{reason}"
    );
    assert_eq!(types[0].name, "Svc");
}

#[test]
fn a_missing_package_statement_agrees_only_with_the_default_package() {
    let types = declared("src/main/java/com/x/Svc.java", "public class Svc {}\n");
    let reason = types[0].fqn.as_ref().expect_err("a packaged directory needs a statement");
    assert!(reason.contains("declares no package"), "{reason}");

    let types = declared("src/main/java/Main.java", "public class Main {}\n");
    assert_eq!(summary(&types), vec![(Ok("Main"), "class", "main")]);

    let types = declared("src/main/java/Main.java", "package com.x;\npublic class Main {}\n");
    let reason = types[0].fqn.as_ref().expect_err("a package directly under the root disagrees");
    assert!(reason.contains("the default package"), "{reason}");
}

#[test]
fn a_package_statement_is_read_through_comments_whitespace_and_annotations() {
    for source in [
        "/* licence */\npackage com . x ;\npublic class Svc {}\n",
        "package com./* inline */x;\npublic class Svc {}\n",
        "// header\n@Deprecated\npackage com.x;\npublic class Svc {}\n",
        "package com.x;\nimport java.util.List;\npublic class Svc {}\n",
    ] {
        let types = declared("src/main/java/com/x/Svc.java", source);
        assert_eq!(summary(&types), vec![(Ok("com.x.Svc"), "class", "main")], "{source}");
    }
    // One segment is still a package, and a one-character-off one disagrees.
    let types = declared("src/main/java/x/Svc.java", "package x;\npublic class Svc {}\n");
    assert_eq!(summary(&types), vec![(Ok("x.Svc"), "class", "main")]);
    let types = declared("src/main/java/com/x/Svc.java", "package com.xx;\npublic class Svc {}\n");
    assert!(types[0].fqn.is_err(), "com.xx is not com.x");
}

#[test]
fn a_java_file_outside_every_source_root_is_named_by_the_default_key_and_refused_when_it_disagrees() {
    // A flat layout: the default key is already the package.
    let types = declared("com/x/Svc.java", "package com.x;\npublic class Svc {}\n");
    assert_eq!(summary(&types), vec![(Ok("com.x.Svc"), "class", "main")]);
    // An undeclared root (`src/it/java`) keys the file under `it.java.…`, which
    // its statement disagrees with — refused, never re-derived a second way.
    let types = declared("src/it/java/com/x/SvcIT.java", "package com.x;\nclass SvcIT {}\n");
    let reason = types[0].fqn.as_ref().expect_err("the layout does not know this root");
    assert!(reason.contains("package `it.java.com.x`"), "{reason}");
}

#[test]
fn a_test_shaped_file_outside_every_source_root_is_tagged_test() {
    let types = declared("com/x/SvcTest.java", "package com.x;\nclass SvcTest {}\n");
    assert_eq!(summary(&types), vec![(Ok("com.x.SvcTest"), "class", "test")]);
    let types = declared("com/x/Svc.java", "package com.x;\nclass Svc {}\n");
    assert_eq!(summary(&types), vec![(Ok("com.x.Svc"), "class", "main")]);
}

#[test]
fn a_file_of_a_language_that_is_not_package_shaped_declares_nothing() {
    assert!(declared("src/lib.rs", "pub struct S;\npub enum E { A }\npub trait T {}\n").is_empty());
    assert!(declared("src/main/java/app.py", "class Svc:\n    pass\n").is_empty());
}

// ── Kotlin ───────────────────────────────────────────────────────────────────

#[test]
fn a_kotlin_class_interface_enum_and_object_are_named_by_their_package_in_both_trees() {
    let source = "package com.x.core\n\n\
                  class Svc\n\
                  interface Port\n\
                  enum class Mode { A, B }\n\
                  object Registry\n\
                  data class Point(val x: Int)\n\
                  fun helper() {}\n";
    let types = declared("svc/src/main/kotlin/com/x/core/Types.kt", source);
    assert_eq!(
        summary(&types),
        vec![
            (Ok("com.x.core.Svc"), "class", "main"),
            (Ok("com.x.core.Port"), "interface", "main"),
            (Ok("com.x.core.Mode"), "class", "main"),
            (Ok("com.x.core.Registry"), "class", "main"),
            (Ok("com.x.core.Point"), "class", "main"),
        ],
        "a Kotlin file declares every top-level type it holds, whatever its file name; \
         a top-level function is not a type"
    );
    let types = declared(
        "svc/src/test/kotlin/com/x/core/SvcTest.kt",
        "package com.x.core\nclass SvcTest\n",
    );
    assert_eq!(summary(&types), vec![(Ok("com.x.core.SvcTest"), "class", "test")]);
}

/// Kotlin is keyed by the package it declares (S-518, FR-RS-13), not by its
/// source root: a package that differs from the directory names the type, and
/// a Multiplatform source set outside `src/main/kotlin` is named the same way.
#[test]
fn a_kotlin_type_is_named_by_its_declared_package_whatever_its_directory() {
    let types = declared("src/main/kotlin/com/x/Svc.kt", "package com.y\nclass Svc\n");
    assert_eq!(summary(&types), vec![(Ok("com.y.Svc"), "class", "main")]);
    let types = declared(
        "core/src/commonMain/kotlin/org/koin/core/Koin.kt",
        "package org.koin.core\nclass Koin\n",
    );
    assert_eq!(summary(&types), vec![(Ok("org.koin.core.Koin"), "class", "main")]);
    // Backtick escapes are the compiler's, not the name's.
    let types = declared("src/main/kotlin/com/x/Svc.kt", "package com.`x`\nclass Svc\n");
    assert_eq!(summary(&types), vec![(Ok("com.x.Svc"), "class", "main")]);
    // No package header: the default package.
    let types = declared("src/main/kotlin/Top.kt", "class Top\n");
    assert_eq!(summary(&types), vec![(Ok("Top"), "class", "main")]);
}

// ── Declared namespaces (S-518) ──────────────────────────────────────────────

/// The namespace one file declares, as extraction records it.
fn namespace(path: &str, source: &str) -> Option<String> {
    let facts = extract_files(
        &[FileInput::new(path, source)],
        registry(),
        &SymbolContext::default(),
    );
    assert_eq!(facts.len(), 1, "{path} is extracted");
    facts.into_iter().next().unwrap().namespace
}

/// FR-RS-13 AC: a C# file-scoped `namespace X;` and a block `namespace X { }`
/// give the same identity, and nested blocks compose.
#[test]
fn a_csharp_file_scoped_and_a_block_namespace_give_the_same_identity() {
    let file_scoped = "using System;\nnamespace Shop.Domain;\npublic class Order { }\n";
    let block = "using System;\nnamespace Shop.Domain\n{\n    public class Order { }\n}\n";
    let nested = "namespace Shop\n{\n    namespace Domain\n    {\n        public class Order { }\n    }\n}\n";
    for source in [file_scoped, block, nested] {
        assert_eq!(namespace("src/Order.cs", source).as_deref(), Some("Shop.Domain"), "{source}");
        assert_eq!(
            summary(&declared("src/Order.cs", source)),
            vec![(Ok("Shop.Domain.Order"), "class", "main")],
            "{source}"
        );
    }
}

#[test]
fn a_php_kotlin_and_scala_file_declare_their_namespace_or_package() {
    let php = "<?php\nnamespace Monolog\\Handler;\n\nuse Monolog\\Logger;\n\nclass StreamHandler {}\n";
    assert_eq!(namespace("src/Monolog/Handler/StreamHandler.php", php).as_deref(), Some("Monolog.Handler"));
    let braced = "<?php\nnamespace App\\Models {\n    class Song {}\n}\n";
    assert_eq!(namespace("app/Song.php", braced).as_deref(), Some("App.Models"));
    let kotlin = "package org.koin.core\n\nimport org.koin.core.module.Module\n\nclass Koin\n";
    assert_eq!(namespace("core/src/commonMain/kotlin/Koin.kt", kotlin).as_deref(), Some("org.koin.core"));
    // Scala's chained clauses compose; a package block scopes its body.
    let chained = "package com.x\npackage core\n\nclass Svc\n";
    assert_eq!(namespace("src/main/scala/Svc.scala", chained).as_deref(), Some("com.x.core"));
    let block = "package com.x {\n  class Svc\n}\n";
    assert_eq!(namespace("src/main/scala/Svc.scala", block).as_deref(), Some("com.x"));
    // A file of no declaration still takes the namespace in force at its end.
    let empty = "<?php\nnamespace App\\Support;\n\nfunction helper() {}\n";
    assert_eq!(namespace("app/helpers.php", empty).as_deref(), Some("App.Support"));
}

#[test]
fn a_file_that_declares_no_namespace_is_in_the_global_one() {
    assert_eq!(namespace("lib/util.php", "<?php\nclass Util {}\n").as_deref(), Some(""));
    assert_eq!(namespace("Program.cs", "public class Program { }\n").as_deref(), Some(""));
    assert_eq!(namespace("src/Top.kt", "class Top\n").as_deref(), Some(""));
}

/// A file whose top-level declarations sit in two namespaces has no one
/// namespace, and records none rather than naming one set of its types wrongly
/// (NFR-RA-05) — the near miss beside every agreeing case above.
#[test]
fn a_file_whose_declarations_sit_in_two_namespaces_records_none() {
    let siblings = "namespace A { class X { } }\nnamespace B { class Y { } }\n";
    assert_eq!(namespace("src/Two.cs", siblings), None);
    // PHP's statement form: a second statement starts a second namespace.
    let php = "<?php\nnamespace Acme;\nclass Tester {}\nnamespace Monolog\\Processor;\nclass ProcessorTest {}\n";
    assert_eq!(namespace("tests/ProcessorTest.php", php), None);
    // A declaration outside the only namespace block is in the global one.
    let mixed = "namespace A { class X { } }\nclass Y { }\n";
    assert_eq!(namespace("src/Mixed.cs", mixed), None);
    // …and so the file names no type at all.
    assert!(declared("src/Two.cs", siblings).is_empty());
}

/// A namespace declared but enclosing none of the file's declarations — what a
/// parse damaged around preprocessor branches leaves — is not read as the
/// global namespace.
#[test]
fn a_declared_namespace_that_encloses_no_declaration_records_none() {
    let stranded = "namespace A { }\nclass X { }\n";
    assert_eq!(namespace("src/X.cs", stranded), None);
}

#[test]
fn a_language_of_another_module_model_records_no_namespace() {
    assert_eq!(namespace("src/lib.rs", "pub mod a { pub struct S; }\n"), None);
    assert_eq!(namespace("src/main/java/com/x/Svc.java", "package com.x;\nclass Svc {}\n"), None);
}

/// The single-file interface names types exactly as the multi-file driver
/// does — the layout comes from the plugin it is handed.
#[test]
fn the_single_file_entry_point_names_types_as_the_driver_does() {
    for (path, source) in [
        ("src/main/java/com/x/Svc.java", "package com.x;\npublic class Svc {}\n"),
        ("src/main/kotlin/com/x/Svc.kt", "package com.x\nclass Svc\n"),
        ("src/Shop/Order.cs", "namespace Shop.Domain;\npublic class Order { }\n"),
    ] {
        let ext = path.rsplit('.').next().unwrap();
        let plugin = registry().for_extension(ext).expect("plugin");
        let single = crate::extract::extract(
            &FileInput::new(path, source),
            plugin,
            &SymbolContext::default(),
        );
        assert_eq!(single.declared_types, declared(path, source), "{path}");
        assert_eq!(single.declared_types.len(), 1, "{path}");
    }
}

// ── Avro ─────────────────────────────────────────────────────────────────────

#[test]
fn an_avro_record_and_its_nested_named_types_take_the_enclosing_namespace() {
    let schema = r#"{
        "type": "record", "name": "MailSent", "namespace": "com.x.events",
        "fields": [
            {"name": "id", "type": "string"},
            {"name": "status", "type": {"type": "enum", "name": "Status", "symbols": ["OK", "KO"]}},
            {"name": "to", "type": {"type": "array", "items":
                {"type": "record", "name": "Recipient", "fields": [{"name": "a", "type": "string"}]}}},
            {"name": "meta", "type": ["null", {"type": "map", "values":
                {"type": "record", "name": "Meta", "namespace": "com.x.meta", "fields": [
                    {"name": "k", "type": {"type": "enum", "name": "Key", "symbols": ["A"]}}
                ]}}]},
            {"name": "digest", "type": {"type": "fixed", "name": "Digest", "size": 16}}
        ]
    }"#;
    assert_eq!(
        avro_names(schema),
        vec![
            ("com.x.events.MailSent".into(), "record"),
            ("com.x.events.Status".into(), "enum"),
            ("com.x.events.Recipient".into(), "record"),
            ("com.x.meta.Meta".into(), "record"),
            ("com.x.meta.Key".into(), "enum"),
        ],
        "document order; a nested type inherits the nearest enclosing namespace; a fixed is \
         named but not recorded"
    );
    let facts = avro(schema);
    assert_eq!(facts.types[0].name, "MailSent");
    assert_eq!(facts.path, "src/main/avro/events.avsc", "the schema path is the provenance");
}

#[test]
fn an_avro_dotted_name_is_already_full_and_an_empty_namespace_is_the_null_one() {
    let schema = r#"{"type": "record", "name": "a.b.Outer", "namespace": "ignored",
        "fields": [
            {"name": "x", "type": {"type": "enum", "name": "Inner", "symbols": ["A"]}},
            {"name": "y", "type": {"type": "enum", "name": "Bare", "namespace": "", "symbols": ["A"]}}
        ]}"#;
    assert_eq!(
        avro_names(schema),
        vec![
            ("a.b.Outer".into(), "record"),
            ("a.b.Inner".into(), "enum"),
            ("Bare".into(), "enum"),
        ]
    );
}

#[test]
fn an_avro_union_file_and_a_namespace_on_a_non_named_type_are_read_by_the_avro_rule() {
    let schema = r#"[
        {"type": "enum", "name": "A", "namespace": "n1", "symbols": ["X"]},
        {"type": "record", "name": "B", "namespace": "n2", "fields": [
            {"name": "f", "namespace": "not.a.named.type", "type":
                {"type": "array", "namespace": "ignored.too", "items":
                    {"type": "enum", "name": "C", "symbols": ["Y"]}}}
        ]},
        "string"
    ]"#;
    assert_eq!(
        avro_names(schema),
        vec![("n1.A".into(), "enum"), ("n2.B".into(), "record"), ("n2.C".into(), "enum")],
        "a namespace on a field or an array is not a named type's and is ignored"
    );
}

#[test]
fn a_schema_wrapped_in_a_schema_declares_what_it_wraps() {
    assert_eq!(
        avro_names(r#"{"type": {"type": "record", "name": "W", "namespace": "n", "fields": []}}"#),
        vec![("n.W".into(), "record")]
    );
    assert_eq!(
        avro_names(r#"{"type": ["null", {"type": "enum", "name": "E", "namespace": "n", "symbols": ["A"]}]}"#),
        vec![("n.E".into(), "enum")]
    );
    // A `type` that is neither a name nor a schema is malformed, not read.
    assert!(malformed(r#"{"type": 7}"#).contains("not a type"));
}

#[test]
fn a_field_is_never_read_as_a_named_type() {
    // A field named `Oops` whose type is a reference to a named type spelled
    // `record` is not a record named `Oops` — only a schema position declares.
    let schema = r#"{"type": "record", "name": "R", "namespace": "n",
        "fields": [{"name": "Oops", "type": "enum"}]}"#;
    assert_eq!(avro_names(schema), vec![("n.R".into(), "record")]);
}

#[test]
fn a_schema_that_declares_nothing_is_read_with_no_types() {
    assert_eq!(avro_names(r#""string""#), vec![]);
    assert_eq!(avro_names(r#"{"type": "array", "items": "long"}"#), vec![]);
}

#[test]
fn a_malformed_schema_is_refused_with_a_reason_and_no_name_is_guessed() {
    assert!(malformed("{ not json").contains("not valid JSON"));
    assert!(malformed(r#"{"type": "record", "fields": []}"#).contains("no `name`"));
    assert!(malformed(r#"{"type": "enum", "name": 7, "symbols": []}"#).contains("non-string `name`"));
    assert!(malformed(r#"{"type": "record", "name": "R", "namespace": 3, "fields": []}"#)
        .contains("non-string `namespace`"));
    assert!(malformed(r#"{"type": "record", "name": "my-event", "fields": []}"#)
        .contains("not a valid Avro name"));
    assert!(malformed(r#"{"type": "record", "name": "R", "namespace": "a..b", "fields": []}"#)
        .contains("not a valid Avro name"));
    assert!(malformed(r#"{"type": "record", "name": "R"}"#).contains("no `fields` array"));
    assert!(malformed(r#"{"type": "record", "name": "R", "fields": [{"name": "f"}]}"#)
        .contains("has no `type`"));
    assert!(malformed(r#"{"name": "R"}"#).contains("no `type`"));
    assert!(malformed(r#"{"type": "array"}"#).contains("no `items`"));
    assert!(malformed(r#"{"type": "map"}"#).contains("no `values`"));
    // A NESTED bad name refuses the whole schema — the names that did parse
    // are not recorded either.
    let detail = malformed(
        r#"{"type": "record", "name": "Good", "namespace": "n", "fields": [
            {"name": "f", "type": {"type": "enum", "name": "1Bad", "symbols": ["A"]}}]}"#,
    );
    assert!(detail.contains("`n.1Bad`"), "{detail}");
    let detail = malformed(
        r#"[{"type": "enum", "name": "E", "namespace": "n", "symbols": ["A"]},
            {"type": "enum", "name": "E", "namespace": "n", "symbols": ["B"]}]"#,
    );
    assert!(detail.contains("defines `n.E` twice"), "{detail}");
}

#[test]
fn an_unreadable_schema_keeps_its_path_and_reason() {
    let facts = SchemaFacts::unreadable("a.avsc", "not UTF-8".into());
    assert_eq!(
        (facts.path.as_str(), facts.status.as_str(), facts.detail.as_deref(), facts.types.len()),
        ("a.avsc", "unreadable", Some("not UTF-8"), 0)
    );
}

#[test]
fn the_kind_status_and_tree_tokens_are_the_persisted_vocabulary() {
    let kinds: Vec<_> = [TypeKind::Class, TypeKind::Interface, TypeKind::Enum, TypeKind::Record]
        .iter()
        .map(|k| k.as_str())
        .collect();
    assert_eq!(kinds, ["class", "interface", "enum", "record"]);
    let statuses: Vec<_> = [SchemaStatus::Read, SchemaStatus::Malformed, SchemaStatus::Unreadable]
        .iter()
        .map(|s| s.as_str())
        .collect();
    assert_eq!(statuses, ["read", "malformed", "unreadable"]);
    assert_eq!([SourceTree::Main.as_str(), SourceTree::Test.as_str()], ["main", "test"]);
}
