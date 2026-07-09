//! Offline carve-out boundary fitness test for the wiki-agent (S-177, S-286,
//! [NFR-SE-01], [ADR-40], [ADR-42], [ADR-60]).
//!
//! The byte-identical no-networking-crate scan over the **default** `logos` tree
//! lives in `logos-core/tests/no_network_deps.rs` and is unchanged by this story.
//! This test asserts the wiki-agent side of the same carve-out — the boundary
//! itself, mirroring `agent-core/tests/carve_out.rs`.
//!
//! CR-078/ADR-60 split *listening* from *dialing*: the egress client is no longer
//! gated by `ui` but by a separate `agents` feature (requiring `ui`). So the
//! invariant is now three-valued:
//!
//! - the **default**-feature `logos` tree links **no** HTTP client (`reqwest`,
//!   `hyper`) and not `rig-core` — the substrate the wiki-agent builds on is
//!   absent;
//! - the **`ui`**-feature `logos` tree (listen-only dashboard, wiki-**view**
//!   included) links **no** `rig-core` and **no** `reqwest` — the wiki generator's
//!   egress client is carved out of the dashboard ([ADR-60]);
//! - the **`agents`**-feature `logos` tree **does** link `rig-core` and `reqwest`
//!   — the egress seam exists, but only there.
//!
//! A regression that pulled the wiki-agent's `rig`/`reqwest` into the default or
//! the listen-only `ui` tree (e.g. a non-`agents` edge from the cli/core to
//! `wiki-agent`) fails here at `cargo test` time rather than being discovered
//! later.
//!
//! [ADR-60]: ../../docs/specs/architecture/decisions/ADR-60.md

use std::process::Command;

/// Resolve the crate names in the `logos` binary's dependency tree for the given
/// feature selection (normal + build edges, dev excluded). `--offline` keeps it
/// network-free (the registry cache is warm after the build that precedes `cargo
/// test`).
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

/// The default build: no HTTP client, no `rig` — the wiki-agent (and the substrate
/// it builds on) is fully carved out (the same scope the byte-identical
/// no-networking-crate scan guards, unchanged by S-177).
#[test]
fn default_logos_tree_excludes_rig_and_http_client() {
    let crates = logos_tree_crates(None);
    for denied in ["rig-core", "reqwest", "hyper"] {
        assert!(
            !contains(&crates, denied),
            "the DEFAULT-feature `logos` tree must not link `{denied}` — the wiki-agent \
             and the rig substrate/HTTP client are ui-only (NFR-SE-01, ADR-40, ADR-42)",
        );
    }
}

/// The listen-only `ui` build (CR-078, ADR-60): the dashboard (wiki-**view**
/// included) is present but the wiki generator's egress client is **not** —
/// neither `rig-core` nor `reqwest` is linked. This is the S-286 carve-out that
/// lets `ui` become the shipped default (S-287) without the outbound HTTP client.
/// If this fails, the egress client has leaked back into the listen-only surface.
#[test]
fn ui_logos_tree_excludes_rig_and_its_http_client() {
    let crates = logos_tree_crates(Some("ui"));
    for denied in ["rig-core", "reqwest"] {
        assert!(
            !contains(&crates, denied),
            "the `ui`-feature (listen-only dashboard) `logos` tree must not link \
             `{denied}` — the wiki-agent's rig substrate and its HTTP client are \
             `agents`-only since CR-078 (NFR-SE-01, ADR-60, ADR-42)",
        );
    }
}

/// The `agents` build: `rig-core` and `reqwest` are present — the egress seam
/// exists, confined to this feature (a superset of `ui`). If this fails, the
/// substrate has been severed from the `agents` binary (or `rig`'s HTTP backend
/// changed).
#[test]
fn agents_logos_tree_links_rig_and_its_http_client() {
    let crates = logos_tree_crates(Some("agents"));
    assert!(
        contains(&crates, "rig-core"),
        "the `agents`-feature `logos` tree must link `rig-core` (the agent substrate)",
    );
    assert!(
        contains(&crates, "reqwest"),
        "the `agents`-feature `logos` tree links `reqwest` (rig's HTTP client) — the sole, \
         `agents`-gated outbound seam (NFR-SE-07, ADR-40, ADR-60)",
    );
}
