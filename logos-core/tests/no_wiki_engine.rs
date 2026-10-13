//! `logos-core` holds no wiki engine ([S-644], CR-213, CR-214, [ADR-74]).
//!
//! The wiki is not part of this repository: it is an extension a build composes
//! over `&Engine`, above the core. What keeps the engine out of the core is pinned
//! here, because it can come back without a compile error: no crate-root `wiki`
//! module and no `wiki_*` method on the core's façade. `WikiConfig`
//! (`config/wiki.rs`) is the one named exception: the config read-model
//! (`effective_wiki`) carries it, so the type stays in the core while a build that
//! composes no extension ignores the `[wiki]` table. It is a config type, not the
//! engine.
//!
//! [S-644]: ../../docs/planning/journal.md#s-644-the-wiki-is-its-own-crate-and-a-registered-extension
//! [ADR-74]: ../../docs/specs/architecture/decisions/ADR-74.md

use std::path::{Path, PathBuf};

/// Whether a line of Rust declares a **visible** `wiki_*` function — a façade
/// method (`pub fn wiki_read(`, `pub(crate) fn wiki_x(`), not a private test
/// function that merely starts with the word (`fn wiki_generation_calls_…`).
fn declares_a_wiki_method(line: &str) -> bool {
    let code = line.trim_start();
    code.starts_with("pub") && code.contains(" fn wiki_")
}

/// Whether a line of Rust declares a module named `wiki`, whatever its
/// visibility or form — `mod wiki;`, `pub(crate) mod wiki {`, `#[path = "…"] mod
/// wiki;` — and not a module that merely starts with the word.
fn declares_a_wiki_module(line: &str) -> bool {
    let code = line.trim_start();
    if code.starts_with("//") {
        return false;
    }
    let tokens: Vec<&str> = code.split_whitespace().collect();
    tokens.windows(2).any(|pair| {
        pair[0] == "mod" && pair[1].trim_end_matches([';', '{']) == "wiki"
    })
}

/// The module matcher, probed like the method matcher below.
#[test]
fn the_wiki_module_matcher_catches_every_visibility_and_admits_near_misses() {
    for hit in [
        "pub mod wiki;",
        "mod wiki;",
        "pub(crate) mod wiki { pub fn read_page() {} }",
        "#[path = \"store/w.rs\"] pub mod wiki;",
        "pub(crate) mod wiki{",
    ] {
        assert!(declares_a_wiki_module(hit), "must catch: {hit}");
    }
    for miss in [
        "pub mod wikis;",
        "mod wiki_store;",
        "/// pub mod wiki was the engine until S-644",
        "pub mod watch;",
    ] {
        assert!(!declares_a_wiki_module(miss), "must admit: {miss}");
    }
}

/// The matcher is probed with the shapes it must catch and the near misses it
/// must admit, so a vacuous matcher cannot make the scan below pass.
#[test]
fn the_wiki_method_matcher_catches_facade_methods_and_admits_near_misses() {
    for hit in [
        "    pub fn wiki_read(&self, slug: &str) -> Result<Option<WikiPage>> {",
        "    pub(crate) fn wiki_status(&self) -> Result<WikiStatus> {",
        "pub fn wiki_skill_emit(",
    ] {
        assert!(declares_a_wiki_method(hit), "must catch: {hit}");
    }
    for miss in [
        "fn wiki_generation_calls_are_separable_from_the_surfaces_they_would_be_summed_with() {",
        "    /// pub fn wiki_read was a façade method until S-644",
        "    pub fn effective_wiki(&self) -> EffectiveWikiModel {",
        "    let wiki_dir = root.join(\"wiki\");",
    ] {
        assert!(!declares_a_wiki_method(miss), "must admit: {miss}");
    }
}

/// Every `.rs` file under `dir`, recursively.
fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("readable source dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn logos_core_declares_no_wiki_module_and_no_wiki_method() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    assert!(!src.join("wiki").exists(), "no crate-root wiki module directory");
    assert!(!src.join("wiki.rs").exists(), "no crate-root wiki module file");
    let lib = std::fs::read_to_string(src.join("lib.rs")).expect("lib.rs");
    let declared: Vec<&str> = lib.lines().filter(|l| declares_a_wiki_module(l)).collect();
    assert!(declared.is_empty(), "lib.rs declares no wiki module, at any visibility: {declared:?}");

    let mut files = Vec::new();
    rust_files(&src, &mut files);
    // The denominator: the walk must have reached the façade itself.
    assert!(files.iter().any(|f| f.ends_with("engine.rs")), "the scan saw engine.rs: {files:?}");

    let declared: Vec<String> = files
        .iter()
        .flat_map(|file| {
            let text = std::fs::read_to_string(file).expect("readable source file");
            text.lines()
                .filter(|line| declares_a_wiki_method(line))
                .map(|line| format!("{}: {}", file.display(), line.trim()))
                .collect::<Vec<_>>()
        })
        .collect();
    assert!(declared.is_empty(), "no wiki_* method in logos-core: {declared:#?}");
}
