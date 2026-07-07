//! Offline carve-out boundary fitness test (S-166, [NFR-SE-01], [NFR-SE-07],
//! ADR-40, ADR-41).
//!
//! The byte-identical no-networking-crate scan over the **default** `logos`
//! tree lives in `logos-core/tests/no_network_deps.rs` and is unchanged. This
//! test asserts the *other side* of the carve-out — the boundary itself:
//!
//! - the **default**-feature `logos` tree links **no** HTTP client (`reqwest`,
//!   `hyper`) and not `rig-core` — the substrate is absent;
//! - the **`ui`**-feature `logos` tree **does** link `rig-core` and `reqwest`
//!   — the egress seam genuinely exists, but only there.
//!
//! Together with the existing default-tree scan, this locks the invariant that
//! `rig` reaches the binary *only* under `--features ui` (via the `web`
//! adapter): a regression that pulled `rig`/`reqwest` into the default tree, or
//! that severed the substrate from the ui build, fails here at `cargo test`
//! time rather than being discovered later.

use std::process::Command;

/// Resolve the crate names in the `logos` binary's dependency tree for the
/// given feature selection (normal + build edges, dev excluded). `--offline`
/// keeps it network-free (the registry cache is warm after the build that
/// precedes `cargo test`).
fn logos_tree_crates(features: Option<&str>) -> Vec<String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let mut args = vec![
        "tree",
        "--package",
        "logos",
        "--edges",
        "normal,build",
        "--prefix",
        "none",
        "--format",
        "{p}",
        "--color",
        "never",
        "--offline",
    ];
    if let Some(features) = features {
        args.push("--features");
        args.push(features);
    } else {
        // The default-feature tree: no extra features, but make the intent
        // explicit so a future default-feature change is a deliberate edit.
        args.push("--no-default-features");
        args.push("--features");
        args.push("lang-all");
    }

    let output = Command::new(cargo)
        .args(&args)
        .output()
        .expect("`cargo tree` runs (the carve-out fitness gate must resolve the tree)");
    assert!(
        output.status.success(),
        "`cargo tree` failed resolving the {features:?} tree:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );

    let stdout = String::from_utf8(output.stdout).expect("`cargo tree` output is UTF-8");
    let mut names: Vec<String> = stdout
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_string)
        .collect();
    names.sort_unstable();
    names.dedup();
    names
}

fn contains(crates: &[String], name: &str) -> bool {
    crates.iter().any(|c| c == name)
}

/// The default build: no HTTP client, no `rig` — the substrate is fully carved
/// out (the same scope the byte-identical no-networking-crate scan guards).
#[test]
fn default_logos_tree_excludes_rig_and_http_client() {
    let crates = logos_tree_crates(None);
    for denied in ["rig-core", "reqwest", "hyper"] {
        assert!(
            !contains(&crates, denied),
            "the DEFAULT-feature `logos` tree must not link `{denied}` — the rig \
             substrate and its HTTP client are ui-only (NFR-SE-01, ADR-40)",
        );
    }
}

/// The `ui` build: `rig-core` and `reqwest` are present — the egress seam
/// exists, confined to this feature. If this fails, the substrate has been
/// severed from the ui binary (or `rig`'s HTTP backend changed).
#[test]
fn ui_logos_tree_links_rig_and_its_http_client() {
    let crates = logos_tree_crates(Some("ui"));
    assert!(
        contains(&crates, "rig-core"),
        "the `ui`-feature `logos` tree must link `rig-core` (the agent substrate)",
    );
    assert!(
        contains(&crates, "reqwest"),
        "the `ui`-feature `logos` tree links `reqwest` (rig's HTTP client) — the \
         sole, ui-gated outbound seam (NFR-SE-07, ADR-40)",
    );
}
