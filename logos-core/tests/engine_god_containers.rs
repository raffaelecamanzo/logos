//! `Engine` and its feature-area façades are not god containers (S-829,
//! [CR-259]): a copy of `logos-core/src/engine.rs` and `logos-core/src/engine/`
//! indexed on its own, its god containers listed by the `no_god_containers`
//! rule ([FR-GV-11], [FR-QM-12]) under the default thresholds, names none of
//! them. Before the split the list named `Engine` (150 methods), the one façade
//! every surface called.
//!
//! `Engine` keeps its lifecycle and one accessor per area; each area is a type
//! of its own module under `engine/` borrowing the engine
//! (`engine.security().scan(…)`), and `Engine` forwards to none of them. The
//! copy holds no federation trait impl, so the count is `Engine`'s own methods
//! under either metric semantics.
//!
//! [CR-259]: ../../docs/requests/CR-259-engine-is-split-by-feature-area.md
//! [FR-GV-11]: ../../docs/specs/requirements/FR-GV-11.md
//! [FR-QM-12]: ../../docs/specs/requirements/FR-QM-12.md
//!
//! Gated on `lang-rust`, as `relational_denominator_audit.rs` is: the copy is
//! Rust, and the forwarding check parses it with the Rust grammar.
#![cfg(feature = "lang-rust")]

use std::fs;
use std::path::{Path, PathBuf};

use logos_core::Engine;
use tempfile::TempDir;
use tree_sitter::{Node, Parser, Tree};

/// Copy every file under `from` to `to`, keeping the relative layout.
fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).expect("the copy's directory is created");
    for entry in fs::read_dir(from).expect("the source directory is readable") {
        let path = entry.expect("a directory entry").path();
        let dest = to.join(path.file_name().expect("an entry has a name"));
        if path.is_dir() {
            copy_tree(&path, &dest);
        } else {
            fs::copy(&path, &dest).expect("a source file is copied");
        }
    }
}

/// This crate's `src/engine.rs` and every file under `src/engine/`, whichever
/// of the two exist.
fn engine_sources() -> Vec<PathBuf> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files: Vec<PathBuf> = vec![src.join("engine.rs")];
    if let Ok(entries) = fs::read_dir(src.join("engine")) {
        files.extend(entries.map(|e| e.expect("a directory entry").path()));
    }
    files.retain(|f| f.is_file() && f.extension().is_some_and(|e| e == "rs"));
    files.sort();
    files
}

/// A god container the rule lists: its name, the file declaring it, and the
/// rule's own message.
struct God {
    name: String,
    file: String,
    message: String,
}

/// The contract the plain list is read under: the budget without a ratchet, so
/// no blessed baseline hides a god container, at the default thresholds.
const NO_RATCHET: &str = "[constraints]\nno_god_containers = true\n";

/// Index a copy of this crate's engine sources under the rules `rules` and list
/// their god containers ([`God`]), every one of them.
fn engine_gods(rules: &str) -> Vec<God> {
    let tmp = TempDir::new().expect("a temporary directory");
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let copy = tmp.path().join("src");
    fs::create_dir_all(&copy).expect("the copy's src directory is created");
    if src.join("engine.rs").is_file() {
        fs::copy(src.join("engine.rs"), copy.join("engine.rs")).expect("engine.rs is copied");
    }
    if src.join("engine").is_dir() {
        copy_tree(&src.join("engine"), &copy.join("engine"));
    }
    fs::create_dir_all(tmp.path().join(".logos")).expect("the .logos directory is created");
    fs::write(tmp.path().join(".logos/rules.toml"), rules).expect("the rules are written");
    let engine = Engine::start(tmp.path()).expect("the engine starts");
    engine.pipeline().index();
    let scanned = engine.quality().scan(false).expect("the scan runs");
    scanned
        .violations
        .iter()
        .filter(|v| v.rule == "no_god_containers")
        .map(|v| God {
            name: v.message.split('`').nth(1).expect("the message names its container").to_string(),
            file: v.file.clone(),
            message: v.message.clone(),
        })
        .collect()
}

/// The engine's types, read from this crate's source: every type an engine file
/// declares an inherent `impl` for — `Engine`, each façade, `Setup`,
/// `StartOptions` and `ColdStartPhases` — however its struct is written.
fn engine_types() -> Vec<String> {
    let mut types = Vec::new();
    for file in engine_sources() {
        let source = fs::read_to_string(&file).expect("an engine source is readable");
        let tree = parse(&source);
        types.extend(inherent_impls(tree.root_node(), &source).into_iter().map(|(name, _)| name));
    }
    types.sort();
    types.dedup();
    types
}

/// `true` for a file of the engine: `engine.rs` or a module under `engine/`.
fn is_engine_file(file: &str) -> bool {
    file == "src/engine.rs" || file.starts_with("src/engine/")
}

#[test]
fn no_engine_type_is_a_god_container() {
    let gods = engine_gods(NO_RATCHET);
    let listed: Vec<&str> = gods
        .iter()
        .filter(|g| is_engine_file(&g.file))
        .map(|g| g.message.as_str())
        .collect();
    assert!(listed.is_empty(), "god containers among the engine's types: {listed:?}");
}

/// The list above names only types whose methods counted toward them: a type
/// another struct of the copy shares a name with counts none (the attribution
/// needs exactly one struct of the name), and would pass however many methods
/// it had. Under `god_methods = 1` every engine type with an inherent `impl`
/// is listed, so each must be.
#[test]
fn every_engine_type_counts_its_own_methods() {
    let types = engine_types();
    assert!(types.len() > 8, "`Engine` and its façades are read from the source: {types:?}");
    let gods = engine_gods(&format!("{NO_RATCHET}\n[metric_thresholds]\ngod_methods = 1\n"));
    let listed: Vec<&str> = gods.iter().filter(|g| is_engine_file(&g.file)).map(|g| g.name.as_str()).collect();
    let missing: Vec<&String> = types.iter().filter(|t| !listed.contains(&t.as_str())).collect();
    assert!(missing.is_empty(), "engine types whose methods reach no struct: {missing:?}");
}

/// The accessor names `Engine` reaches its façades by: `engine.<area>()`.
const ACCESSORS: [&str; 8] = ["graph", "navigation", "pipeline", "quality", "history", "coverage", "security", "config"];

fn parse(source: &str) -> Tree {
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_rust::LANGUAGE.into()).expect("the Rust grammar loads");
    parser.parse(source, None).expect("tree-sitter returns a tree")
}

fn text<'s>(node: Node<'_>, source: &'s str) -> &'s str {
    &source[node.byte_range()]
}

/// Every inherent `impl` of `source` at file level (not inside an inline
/// module, so a test module's helper types are out), with the type it
/// implements without path or generics: `impl<'e> Graph<'e>` is `Graph`,
/// `impl crate::Engine` is `Engine`.
fn inherent_impls<'t>(root: Node<'t>, source: &str) -> Vec<(String, Node<'t>)> {
    let mut cursor = root.walk();
    root.named_children(&mut cursor)
        .filter(|item| item.kind() == "impl_item" && item.child_by_field_name("trait").is_none())
        .filter_map(|item| {
            let ty = text(item.child_by_field_name("type")?, source);
            let name = ty.split('<').next()?.rsplit("::").next()?.trim();
            Some((name.to_string(), item))
        })
        .collect()
}

/// The methods `source`'s `impl Engine` blocks declare (how many were read) and
/// the names of those that only forward to a façade.
fn forwarders(source: &str) -> (usize, Vec<String>) {
    let tree = parse(source);
    let (mut read, mut found) = (0, Vec::new());
    for (ty, item) in inherent_impls(tree.root_node(), source) {
        let Some(body) = item.child_by_field_name("body").filter(|_| ty == "Engine") else { continue };
        let mut cursor = body.walk();
        for function in body.named_children(&mut cursor).filter(|n| n.kind() == "function_item") {
            read += 1;
            if function.child_by_field_name("body").is_some_and(|block| forwards(block, source)) {
                let name = function.child_by_field_name("name").map_or("?", |n| text(n, source));
                found.push(name.to_string());
            }
        }
    }
    (read, found)
}

/// Whether a method body only forwards to a façade: every statement but the
/// last binds a façade to a local, and the value — under any `Ok(…)`, `?`,
/// `return` or `.await` — is a method call whose receiver chain starts at one.
fn forwards(block: Node<'_>, source: &str) -> bool {
    let mut locals: Vec<&str> = Vec::new();
    let mut value: Option<Node<'_>> = None;
    let mut cursor = block.walk();
    for child in block.named_children(&mut cursor) {
        match child.kind() {
            "line_comment" | "block_comment" => {}
            _ if value.is_some() => return false,
            "let_declaration" => {
                let (Some(pattern), Some(init)) = (child.child_by_field_name("pattern"), child.child_by_field_name("value"))
                else {
                    return false;
                };
                if !is_facade(init, source, &locals) {
                    return false;
                }
                locals.push(text(pattern, source));
            }
            "expression_statement" => value = child.named_child(0),
            _ => value = Some(child),
        }
    }
    value.is_some_and(|v| calls_a_facade(v, source, &locals))
}

/// `node` without the wrappers that pass a forwarded value through unchanged.
fn unwrapped<'t>(node: Node<'t>, source: &str) -> Node<'t> {
    match node.kind() {
        "return_expression" | "parenthesized_expression" | "try_expression" | "await_expression" => {
            node.named_child(0).map_or(node, |inner| unwrapped(inner, source))
        }
        "call_expression" => {
            let wraps = node.child_by_field_name("function").is_some_and(|f| matches!(text(f, source), "Ok" | "Some"));
            let arguments = node.child_by_field_name("arguments");
            match arguments.filter(|a| wraps && a.named_child_count() == 1).and_then(|a| a.named_child(0)) {
                Some(inner) => unwrapped(inner, source),
                None => node,
            }
        }
        _ => node,
    }
}

/// Whether `node` is a method call whose receiver chain starts at a façade.
fn calls_a_facade(node: Node<'_>, source: &str, locals: &[&str]) -> bool {
    let node = unwrapped(node, source);
    let receiver = (node.kind() == "call_expression")
        .then(|| node.child_by_field_name("function"))
        .flatten()
        .filter(|f| f.kind() == "field_expression")
        .and_then(|f| f.child_by_field_name("value"));
    receiver.is_some_and(|r| is_facade(r, source, locals) || calls_a_facade(r, source, locals))
}

/// Whether `node` is a façade of this engine: `self.<accessor>()`,
/// `Self::<accessor>(self)`, a literal borrowing the engine
/// (`Graph { engine: self }`), or a local bound to one.
fn is_facade(node: Node<'_>, source: &str, locals: &[&str]) -> bool {
    let node = unwrapped(node, source);
    match node.kind() {
        "identifier" => locals.contains(&text(node, source)),
        "struct_expression" => text(node, source).contains("engine"),
        "call_expression" => {
            let Some(function) = node.child_by_field_name("function") else { return false };
            let (owner, name) = match function.kind() {
                "field_expression" => (function.child_by_field_name("value"), function.child_by_field_name("field")),
                "scoped_identifier" => (function.child_by_field_name("path"), function.child_by_field_name("name")),
                _ => return false,
            };
            let owner = owner.map(|o| text(o, source));
            matches!(owner, Some("self" | "Self" | "Engine")) && name.is_some_and(|n| ACCESSORS.contains(&text(n, source)))
        }
        _ => false,
    }
}

/// The matcher catches a forwarder however it is written, and admits the
/// lifecycle and accessor methods `Engine` keeps — probed on hits and near
/// misses, so a vacuous matcher cannot make the scan below pass.
#[test]
fn the_forwarder_matcher_catches_every_shape_and_admits_the_lifecycle() {
    for hit in [
        "impl Engine { pub fn index(&self) -> IndexResult { self.pipeline().index() } }",
        "impl Engine {\n    pub fn status(&self) -> StatusInfo {\n        self.graph()\n            .status()\n    }\n}",
        "impl Engine {\n    pub fn sync(\n        &self,\n        paths: &[PathBuf],\n    ) -> SyncResult {\n        self.pipeline().sync(paths)\n    }\n}",
        "impl Engine { pub fn index(&self) -> IndexResult { Self::pipeline(self).index() } }",
        "impl Engine { pub fn index(&self) -> IndexResult { let p = self.pipeline(); p.index() } }",
        "impl Engine { pub fn index(&self) -> IndexResult { let p: Pipeline<'_> = self.pipeline(); p.index() } }",
        "impl Engine { pub fn scan(&self) -> Result<ScanResult> { Ok(self.quality().scan(true)?) } }",
        "impl Engine { pub fn readout(&self) -> Result<QualityReadout> { self.quality().readouts().readout() } }",
        "impl Engine { pub fn status(&self) -> StatusInfo { Graph { engine: self }.status() } }",
        "impl Engine { pub fn index(&self) { self.pipeline().index(); } }",
        "impl crate::Engine {\n    #[inline]\n    pub fn languages(&self) -> LanguagesInfo { self.graph().languages() }\n}",
    ] {
        assert_eq!(forwarders(hit), (1, vec![hit.split("fn ").nth(1).unwrap().split('(').next().unwrap().to_string()]), "must catch: {hit}");
    }
    for miss in [
        "impl Engine { pub fn graph(&self) -> Graph<'_> { Graph { engine: self } } }",
        "impl Engine { pub fn start(root: &Path) -> Result<Self> { Self::start_with(root, StartOptions::default()) } }",
        "impl Engine { pub fn root(&self) -> &Path { &self.root } }",
        "impl Engine { pub fn runtime(&self) -> Option<&Runtime> { self.runtime.as_ref() } }",
        "impl Engine { pub fn warm(&self) { let p = self.pipeline(); p.index(); log(); } }",
        "impl Other { pub fn index(&self) -> IndexResult { self.pipeline().index() } }",
    ] {
        assert!(forwarders(miss).1.is_empty(), "must admit: {miss}");
    }
}

/// `Engine` declares no method that only forwards to a façade ([CR-259] AC 4):
/// a forwarder keeps on `Engine` the count the split removed.
#[test]
fn engine_declares_no_forwarding_method() {
    let mut read = 0;
    let mut found = Vec::new();
    for file in engine_sources() {
        let source = fs::read_to_string(&file).expect("an engine source is readable");
        let (methods, forwarders) = forwarders(&source);
        read += methods;
        found.extend(forwarders.into_iter().map(|name| format!("{}: {name}", file.display())));
    }
    assert!(read >= 10, "the engine's own methods are read from the source ({read} read)");
    assert!(found.is_empty(), "`Engine` methods that forward to a façade: {found:?}");
}

/// Every `.rs` file under `dir`, recursively.
fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("a readable source directory") {
        let path = entry.expect("a directory entry").path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// The 1-based lines of every inherent `impl Engine` under `node`, at any
/// depth — an inline module's included.
fn engine_impl_lines(node: Node<'_>, source: &str, out: &mut Vec<usize>) {
    let inherent = node.kind() == "impl_item" && node.child_by_field_name("trait").is_none();
    let names_engine = node
        .child_by_field_name("type")
        .is_some_and(|t| text(t, source).split('<').next().and_then(|s| s.rsplit("::").next()) == Some("Engine"));
    if inherent && names_engine {
        out.push(node.start_position().row + 1);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        engine_impl_lines(child, source, out);
    }
}

/// `Engine` is implemented in the engine files alone: an inherent `impl Engine`
/// anywhere else in the crate would add methods the god-container count above
/// never indexes and the forwarding check never reads. The federation trait
/// impls (`MemberContracts`, `MemberEngine`) are not inherent and stay where
/// they are.
#[test]
fn engine_has_no_inherent_impl_outside_the_engine_files() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    let engine_files = engine_sources();
    assert!(files.len() > engine_files.len() * 10, "the walk read the crate ({} files)", files.len());
    let mut outside = Vec::new();
    for file in files.iter().filter(|f| !engine_files.contains(f)) {
        let source = fs::read_to_string(file).expect("a source file is readable");
        if !source.contains("Engine") {
            continue;
        }
        let tree = parse(&source);
        let mut lines = Vec::new();
        engine_impl_lines(tree.root_node(), &source, &mut lines);
        outside.extend(lines.into_iter().map(|line| format!("{}:{line}", file.display())));
    }
    assert!(outside.is_empty(), "inherent `impl Engine` outside engine.rs and engine/: {outside:?}");
}
