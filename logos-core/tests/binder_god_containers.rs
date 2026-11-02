//! The binder's types are not god containers (S-827, S-828, [CR-258]): a copy
//! of `logos-core/src/resolve/` indexed on its own, its god containers listed
//! by the `no_god_containers` rule ([FR-GV-11], [FR-QM-12]) under the default
//! thresholds, names no binder type.
//!
//! The per-row context `Ctx` keeps its row facts, the acceptance rule and the
//! one associated-item lookup; every other concern is a rung type of its own
//! module under `resolve/binder/` borrowing the row's `Ctx`. Before that split
//! the list named `Ctx` (182 methods). The lookup index `Index` keeps the
//! graph's node facts and containment: its `with_*` steps are an
//! `IndexBuilder`'s, and each query concern is a sub-index it owns, with its
//! fields. Before that split the list named `Index` (54 methods).
//!
//! [CR-258]: ../../docs/requests/CR-258-the-binders-ctx-and-index-are-split-by-concern.md
//! [FR-GV-11]: ../../docs/specs/requirements/FR-GV-11.md
//! [FR-QM-12]: ../../docs/specs/requirements/FR-QM-12.md

use std::fs;
use std::path::Path;

use logos_core::Engine;
use tempfile::TempDir;

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

/// Index a copy of this crate's `src/resolve/` under the rules `rules` and list
/// its god containers ([`God`]), every one of them.
fn resolve_gods(rules: &str) -> Vec<God> {
    let tmp = TempDir::new().expect("a temporary directory");
    let resolve = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/resolve");
    copy_tree(&resolve, &tmp.path().join("src/resolve"));
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

/// `text` cut at its first `#[cfg(test)]` module (`mod tests`, `mod
/// test_api`): its production items.
fn production(text: &str) -> &str {
    text.find("#[cfg(test)]\nmod ").map_or(text, |at| &text[..at])
}

/// The production text of every binder source: `binder.rs` and each `.rs`
/// file under `binder/`, nested directories included, each cut at its first
/// test module ([`production`]).
fn binder_production_sources() -> Vec<String> {
    let resolve = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/resolve");
    let mut files = vec![resolve.join("binder.rs")];
    let mut dirs = vec![resolve.join("binder")];
    while let Some(dir) = dirs.pop() {
        for entry in fs::read_dir(&dir).expect("a binder directory is readable") {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                files.push(path);
            }
        }
    }
    files
        .iter()
        .map(|file| production(&fs::read_to_string(file).expect("a binder source is readable")).to_string())
        .collect()
}

/// `item` past the attributes written on its own line (`#[derive(Default)]`)
/// and its visibility, if any: `pub`, `pub(crate)`, `pub(super)`,
/// `pub(in crate::x)`.
fn past_visibility(item: &str) -> &str {
    let mut item = item.trim_start();
    while let Some(attr) = item.strip_prefix("#[") {
        let Some((_, rest)) = attr.split_once(']') else { break };
        item = rest.trim_start();
    }
    match item.strip_prefix("pub") {
        Some(rest) if rest.starts_with('(') => rest.split_once(')').map_or(rest, |(_, item)| item).trim_start(),
        Some(rest) if rest.starts_with(' ') => rest.trim_start(),
        _ => item,
    }
}

/// The name a type path is written with in an `impl` header or a `struct`
/// declaration: its last path segment, its generics dropped —
/// `super::x::Ctx<'a>` → `Ctx`.
fn type_name(written: &str) -> &str {
    let path = written.split(['<', ' ', '{', '(', ';']).next().unwrap_or(written);
    path.rsplit("::").next().unwrap_or(path)
}

/// `header`, the text after `impl`, past a leading generic list read by
/// bracket depth — so a bound's `Fn() -> bool` does not end it.
fn past_generics(header: &str) -> &str {
    let Some(list) = header.strip_prefix('<') else { return header };
    let (mut depth, mut prev) = (1usize, '<');
    for (at, c) in list.char_indices() {
        match c {
            '<' => depth += 1,
            '>' if prev != '-' => {
                depth -= 1;
                if depth == 0 {
                    return list[at + 1..].trim_start();
                }
            }
            _ => {}
        }
        prev = c;
    }
    list
}

/// The struct one line of a source declares, if any.
fn declared_struct(line: &str) -> Option<&str> {
    past_visibility(line.trim_start()).strip_prefix("struct ").map(type_name)
}

/// The type one line's inherent `impl` header gives methods, if any — never a
/// trait impl's (`impl Deref for …`), which counts toward no type (metric
/// semantics v10).
fn inherent_impl(line: &str) -> Option<&str> {
    let header = line.trim_start().strip_prefix("impl").filter(|h| h.starts_with([' ', '<']))?;
    let header = past_generics(header.trim_start());
    let head = header.split(" where ").next().unwrap_or(header);
    let head = head.split('{').next().unwrap_or(head);
    if head.contains(" for ") {
        return None;
    }
    Some(type_name(header))
}

/// The binder's types, read from this crate's source: every struct a binder
/// file declares outside its tests that an inherent `impl` of a binder file
/// gives methods — the kernel `Ctx` and its rungs, the lookup index `Index`,
/// its builder and its sub-indexes, and the binder's smaller types.
fn binder_types() -> Vec<String> {
    let sources = binder_production_sources();
    let lines = || sources.iter().flat_map(|text| text.lines());
    let implemented: Vec<&str> = lines().filter_map(inherent_impl).collect();
    let mut types: Vec<String> = lines()
        .filter_map(declared_struct)
        .filter(|t| implemented.contains(t))
        .map(str::to_string)
        .collect();
    types.sort();
    types.dedup();
    types
}

/// `true` for a file of the binder: `binder.rs` or a module under `binder/`.
fn is_binder_file(file: &str) -> bool {
    file == "src/resolve/binder.rs" || file.starts_with("src/resolve/binder/")
}

#[test]
fn no_binder_type_is_a_god_container() {
    let gods = resolve_gods(NO_RATCHET);
    // The rule ran over the copy and counts a struct's `impl` methods toward
    // it: the package layout, out of CR-258's scope, is still listed.
    let names: Vec<&str> = gods.iter().map(|g| g.name.as_str()).collect();
    assert!(names.contains(&"PackageLayout"), "the rule lists the copy's god containers: {names:?}");
    let binder: Vec<&str> = gods
        .iter()
        .filter(|g| is_binder_file(&g.file))
        .map(|g| g.message.as_str())
        .collect();
    assert!(binder.is_empty(), "no binder type is god: {binder:?}");
}

/// The list above names only types whose methods counted toward them: a type
/// another struct of the copy shares a name with counts none (the attribution
/// needs exactly one struct of the name), and would pass however many methods
/// it had. Under `god_methods = 1` every binder type that counts its own
/// methods is listed, so each must be.
/// Every `.rs` file of this crate's `src/`, path relative to it and text —
/// test code included: a test-scope namesake breaks the attribution as a
/// production one does (S-827).
fn crate_sources() -> Vec<(String, String)> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut dirs = vec![src.clone()];
    let mut out = Vec::new();
    while let Some(dir) = dirs.pop() {
        for entry in fs::read_dir(&dir).expect("a source directory is readable") {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let rel = path.strip_prefix(&src).expect("under src").display().to_string();
                out.push((rel, fs::read_to_string(&path).expect("a source file is readable")));
            }
        }
    }
    out
}

/// The attribution counts a Rust method toward its struct only when exactly
/// one struct of the name exists in the crate, and the copy of `src/resolve/`
/// the tests above index cannot see a namesake declared anywhere else. So no
/// struct a binder file declares may share its name with one declared outside
/// the binder where either has an inherent `impl`: the binder type would count
/// none of its methods, or the other type none of its own, in this
/// repository's graph.
#[test]
fn no_binder_struct_shares_its_name_with_another_struct_of_the_crate() {
    let sources = crate_sources();
    let in_binder = |file: &str| file == "resolve/binder.rs" || file.starts_with("resolve/binder/");
    let binder: Vec<&str> = binder_production_sources_names(&sources);
    let implemented: Vec<&str> =
        sources.iter().flat_map(|(_, text)| text.lines().filter_map(inherent_impl)).collect();
    let shared: Vec<String> = sources
        .iter()
        .filter(|(file, _)| !in_binder(file))
        .flat_map(|(file, text)| text.lines().filter_map(declared_struct).map(move |name| (file, name)))
        .filter(|(_, name)| binder.contains(name) && implemented.contains(name))
        .map(|(file, name)| format!("{name} ({file})"))
        .collect();
    assert!(shared.is_empty(), "structs sharing a binder struct's name: {shared:?}");
}

/// The structs the binder's production text declares, from [`crate_sources`].
fn binder_production_sources_names(sources: &[(String, String)]) -> Vec<&str> {
    sources
        .iter()
        .filter(|(file, _)| file == "resolve/binder.rs" || file.starts_with("resolve/binder/"))
        .flat_map(|(_, text)| production(text).lines().filter_map(declared_struct))
        .collect()
}

/// The source reading [`binder_types`] rests on, pinned on the shapes a
/// binder type may be written in.
#[test]
fn the_binder_type_reader_reads_every_struct_and_inherent_impl_shape() {
    assert_eq!(declared_struct("pub(crate) struct Index {"), Some("Index"));
    assert_eq!(declared_struct("pub struct A;"), Some("A"));
    assert_eq!(declared_struct("pub(in crate::resolve) struct B<'a> {"), Some("B"));
    assert_eq!(declared_struct("    struct Climb {"), Some("Climb"));
    assert_eq!(declared_struct("#[derive(Debug, Default)] pub(super) struct Facts;"), Some("Facts"));
    assert_eq!(declared_struct("structural: bool,"), None);
    assert_eq!(inherent_impl("impl Index {"), Some("Index"));
    assert_eq!(inherent_impl("impl<'a> Ctx<'a> {"), Some("Ctx"));
    assert_eq!(inherent_impl("impl IncludeLookup<'_, '_> {"), Some("IncludeLookup"));
    assert_eq!(inherent_impl("impl<F: Fn() -> bool> Picker<F> {"), Some("Picker"));
    assert_eq!(inherent_impl("impl super::package_index::PackageIndex {"), Some("PackageIndex"));
    assert_eq!(inherent_impl("impl<T> Wrap<T> where T: Copy {"), Some("Wrap"));
    assert_eq!(inherent_impl("impl<'a> Deref for SelfTypes<'_, 'a> {"), None);
    assert_eq!(inherent_impl("impl Default for Thresholds {"), None);
    assert_eq!(inherent_impl("implemented: Vec<&str>,"), None);
}

#[test]
fn every_binder_type_counts_its_own_methods() {
    let types = binder_types();
    let named = |t: &str| types.iter().any(|ty| ty == t);
    assert!(
        types.len() > 30 && ["Ctx", "Index", "IndexBuilder", "ScopeHierarchy"].iter().all(|t| named(t)),
        "the kernel, its rungs, the index, its builder and its sub-indexes are read from the source: {types:?}"
    );
    let gods = resolve_gods(&format!("{NO_RATCHET}\n[metric_thresholds]\ngod_methods = 1\n"));
    let listed: Vec<&str> = gods.iter().filter(|g| is_binder_file(&g.file)).map(|g| g.name.as_str()).collect();
    let missing: Vec<&String> = types.iter().filter(|t| !listed.contains(&t.as_str())).collect();
    assert!(missing.is_empty(), "binder types whose methods reach no struct: {missing:?}");
}
