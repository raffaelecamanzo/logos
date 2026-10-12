//! The wiki's markdown reference rewriter ([FR-WK-25]) is on exactly where the
//! core's markdown grammar is (S-644).
//!
//! Inside the core the rewriter was gated on the core's own `lang-markdown`.
//! Out here it has a feature of its own, and nothing ties the two together but
//! the manifests. Getting one of them wrong does not show up anywhere else.
//! With the wiki's feature off, the rewriter's tests compile out instead of
//! failing, and the CLI and MCP twins still agree with each other, because both
//! lose the rewriter together. With it forced on from below, the documented
//! Rust-only build starts indexing markdown. So both halves are pinned here, on
//! the `logos` binary's resolved features.
//!
//! [FR-WK-25]: ../../docs/specs/requirements/FR-WK-25.md

use std::process::Command;

/// The features `cargo tree` resolves for `package` in `root`'s normal
/// dependency tree, built with `features`. Each line of the tree is
/// `name vX.Y.Z (path)|feat,feat`. `--offline` keeps it network-free: the
/// registry cache is warm after the build that precedes `cargo test`.
fn resolved_in(root: &str, features: &[&str], package: &str) -> Vec<String> {
    resolved_if_linked(root, features, package)
        .unwrap_or_else(|tree| panic!("{package} is in the {root} tree for {features:?}:\n{tree}"))
}

/// [`resolved_in`], or — when `package` is not in the tree at all — `Err` with the
/// tree that was read.
fn resolved_if_linked(root: &str, features: &[&str], package: &str) -> Result<Vec<String>, String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let output = Command::new(cargo)
        .args(["tree", "--package", root, "--edges", "normal", "--prefix", "none"])
        .args(["--format", "{p}|{f}", "--color", "never", "--offline"])
        .args(features)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("cargo runs");
    assert!(
        output.status.success(),
        "cargo tree failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let Some(line) = stdout.lines().find(|line| line.split_whitespace().next() == Some(package)) else {
        return Err(stdout.into_owned());
    };
    Ok(line
        .rsplit_once('|')
        .map(|(_, feats)| feats.split(',').filter(|f| !f.is_empty()).map(str::to_string).collect())
        .unwrap_or_default())
}

/// The features `package` resolves to in the `logos` binary built with `features`.
fn resolved_features(features: &[&str], package: &str) -> Vec<String> {
    resolved_in("logos", features, package)
}

/// This crate's default build — the one `cargo test -p logos-wiki` runs — carries
/// `lang-markdown`, so the rewriter's tests run. They sit behind
/// `cfg(feature = "lang-markdown")`: without the feature they would not fail,
/// they would vanish. Read from the manifest's resolution rather than `cfg!`, so
/// it states the default even in a build that turned the feature off.
#[test]
fn the_default_test_build_compiles_the_rewriter_and_its_tests_in() {
    let own = resolved_in("logos-wiki", &[], "logos-wiki");
    assert!(own.iter().any(|f| f == "lang-markdown"), "logos-wiki's default carries lang-markdown: {own:?}");
}

/// The shipped build (`lang-all`) turns the rewriter on with the core grammar.
#[test]
fn the_default_binary_runs_the_rewriter_with_the_core_markdown_grammar() {
    let core = resolved_features(&[], "logos-core");
    let wiki = resolved_features(&[], "logos-wiki");
    assert!(core.iter().any(|f| f == "lang-markdown"), "the core indexes markdown: {core:?}");
    assert!(wiki.iter().any(|f| f == "lang-markdown"), "and the wiki rewrites with it: {wiki:?}");
}

/// A Rust-only build that links the wiki (`--features lang-rust,wiki`) indexes no
/// markdown and rewrites none: linking the wiki does not switch the core's
/// markdown indexing on. The documented Rust-only build
/// (`docs/howto/installation.md`, `--no-default-features --features lang-rust`)
/// does not link the wiki at all since it became a default-on feature (S-644).
#[test]
fn the_rust_only_binary_neither_indexes_nor_rewrites_markdown() {
    let slim_wiki = ["--no-default-features", "--features", "lang-rust,wiki"];
    let core = resolved_features(&slim_wiki, "logos-core");
    let wiki = resolved_features(&slim_wiki, "logos-wiki");
    // The denominator: the features were read, not an empty list.
    assert!(core.iter().any(|f| f == "lang-rust"), "the slim build was resolved: {core:?}");
    assert!(!core.iter().any(|f| f == "lang-markdown"), "the core indexes no markdown: {core:?}");
    assert!(!wiki.iter().any(|f| f == "lang-markdown"), "and the wiki rewrites none: {wiki:?}");

    let slim = ["--no-default-features", "--features", "lang-rust"];
    let core = resolved_features(&slim, "logos-core");
    assert!(core.iter().any(|f| f == "lang-rust"), "the slim build was resolved: {core:?}");
    assert!(
        resolved_if_linked("logos", &slim, "logos-wiki").is_err(),
        "a build without the `wiki` feature links no wiki"
    );
}
