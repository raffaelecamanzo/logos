//! Offline carve-out boundary fitness test (S-166, S-286, [NFR-SE-01],
//! [NFR-SE-07], ADR-40, ADR-41, [ADR-60]).
//!
//! The structural no-networking-crate scans over the **default** and **slim**
//! `logos` trees live in `logos-core/tests/no_network_deps.rs` (split along the
//! listen/dial seam by CR-078/[ADR-60], S-288). This test asserts the *other
//! side* of the carve-out — the `rig`/`reqwest` substrate boundary itself.
//!
//! CR-078/ADR-60 split *listening* from *dialing*: the egress client is no longer
//! gated by `ui` but by a separate `agents` feature (requiring `ui`). So the
//! invariant this test locks is now three-valued:
//!
//! - the **slim** (`--no-default-features --features lang-all`) `logos` tree links
//!   **no** HTTP client (`reqwest`), **no** server stack (`hyper`), and not
//!   `rig-core` — the absolute-offline build carries no network surface at all.
//!   (Since S-287 made `ui` default, this is the *slim* tree, not the default one:
//!   the true default (`lang-all` + `ui`) legitimately links the loopback server.)
//! - the **`ui`**-feature `logos` tree (listen-only dashboard) links **no**
//!   `rig-core` and **no** `reqwest` — the egress client is carved out of the
//!   dashboard, not merely the default tree ([ADR-60]);
//! - the **`agents`**-feature `logos` tree **does** link `rig-core` and `reqwest`
//!   — the egress seam genuinely exists, but only there.
//!
//! Together with the existing default-tree scan, this locks the invariant that
//! `rig` reaches the binary *only* under `--features agents` (via the `web`
//! adapter): a regression that pulled `rig`/`reqwest` into the default or the
//! listen-only `ui` tree, or that severed the substrate from the `agents` build,
//! fails here at `cargo test` time rather than being discovered later.
//!
//! [ADR-60]: ../../docs/specs/architecture/decisions/ADR-60.md

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
        // The slim tree (`--no-default-features --features lang-all`): the
        // headless build with no dashboard/listener. Since S-287 made `ui`
        // default this is NOT the default-feature tree — the true default
        // (`lang-all` + `ui`) links the loopback server; that boundary is owned
        // by the `ui` case below and by no_network_deps.rs's default-tree anchor.
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

/// The slim build (`--no-default-features --features lang-all`): no HTTP client,
/// no server stack, no `rig` — the absolute-offline binary carries no network
/// surface (the same scope no_network_deps.rs's slim-tree anchor guards). Since
/// S-287 made `ui` default this is the SLIM tree, **not** the default one: the
/// true default links the loopback `hyper` server (asserted in the `ui` case).
#[test]
fn slim_logos_tree_excludes_rig_http_client_and_server() {
    let crates = logos_tree_crates(None);
    for denied in ["rig-core", "reqwest", "hyper"] {
        assert!(
            !contains(&crates, denied),
            "the SLIM (`--no-default-features --features lang-all`) `logos` tree \
             must not link `{denied}` — it is the absolute-offline build with no \
             dashboard, no listener, and no egress client (NFR-SE-01, ADR-40, ADR-60)",
        );
    }
}

/// The listen-only `ui` build (CR-078, ADR-60): the dashboard is present but the
/// egress client is **not** — neither `rig-core` nor `reqwest` is linked. This is
/// the S-286 carve-out that lets `ui` become the shipped default (S-287) without
/// dragging the outbound HTTP client into every artifact. If this fails, the
/// egress client has leaked back into the listen-only surface.
#[test]
fn ui_logos_tree_excludes_rig_and_its_http_client() {
    let crates = logos_tree_crates(Some("ui"));
    // Positive anchor first, so this exclusion test can never pass **vacuously**:
    // if a regression severed `ui = ["dep:web"]` (or otherwise stopped pulling the
    // web surface), the `ui` tree would trivially lack `rig-core`/`reqwest` and the
    // denials below would pass on a broken, dashboard-less build. `hyper` is the
    // sharpest anchor — the default-tree test above explicitly *denies* it, so
    // asserting its presence here documents the listen(`hyper`)/dial(`reqwest`)
    // split ADR-60 draws.
    assert!(
        contains(&crates, "hyper"),
        "the `ui`-feature `logos` tree must link `hyper` (the loopback dashboard \
         server) — otherwise this exclusion test would pass vacuously on a build \
         that severed the listen-only `ui` web surface (ADR-27, ADR-60)",
    );
    for denied in ["rig-core", "reqwest"] {
        assert!(
            !contains(&crates, denied),
            "the `ui`-feature (listen-only dashboard) `logos` tree must not link \
             `{denied}` — the rig substrate and its HTTP client are `agents`-only \
             since CR-078 (NFR-SE-01, ADR-60)",
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
        "the `agents`-feature `logos` tree links `reqwest` (rig's HTTP client) — the \
         sole, `agents`-gated outbound seam (NFR-SE-07, ADR-40, ADR-60)",
    );
}
