//! No-network fitness function (NFR-SE-01, ADR-17, ADR-27).
//!
//! Logos is local-only and never phones home. This test enforces that invariant
//! *structurally*: it resolves the **default-feature dependency tree of the
//! shipped `logos` binary** and fails the build if any known socket / HTTP
//! client crate has entered it. A regression (someone adding `reqwest`, an HTTP
//! server, an AWS SDK, etc. to the default build) is caught at `cargo test` time
//! rather than discovered by a network sandbox at runtime (NFR-SE-01,
//! [UAT-OB-01], [UAT-UI-02] step 1).
//!
//! # Why the default-feature *tree*, not the raw `Cargo.lock` (CR-012, ADR-27)
//!
//! Until the web UI, this check parsed `Cargo.lock` directly — the lock is the
//! union of all dependencies and is *not* feature-partitioned, which made it a
//! conservative over-approximation that was the safe direction for a security
//! gate. CR-012 added the feature-gated web surface ([ADR-27]): `axum` lives
//! behind the **non-default `ui` cargo feature**, so the default `logos` binary
//! never links it. But an optional dependency still appears in `Cargo.lock`, and
//! enabling `tokio`'s `net` feature for the ui build unifies `socket2` into the
//! lock's `tokio` entry — so a raw-lock scan would now report `socket2` as a
//! false positive even though the default build does not compile it.
//!
//! The carve-out's whole point is that the **default *feature set*** guards the
//! invariant ([ADR-27]: "the default feature set guards the fitness function").
//! So this test now inspects the default-feature tree of the `logos` package —
//! exactly the graph that ships in the default binary, and the same scope
//! cargo-deny uses (`deny.toml [graph] all-features = false`). The ui build
//! gets its own carve-out proofs (web/tests/carve_out.rs + the sandboxed
//! zero-egress session, [UAT-UI-02] steps 2–4). The denylist and matching
//! semantics below are unchanged — only the data source moved from the raw lock
//! to the feature-resolved tree.

use std::process::Command;

/// Known socket / HTTP crates that must never appear in the dependency graph.
///
/// These are *atomic* crate names matched exactly: HTTP clients (`reqwest`,
/// `ureq`, `attohttpc`, `isahc`, `surf`), the `hyper` client/server stack, the
/// HTTP/2 + HTTP/3 protocol crates (`h2`, `h3`), the `tonic` gRPC stack, and
/// `socket2` (raw socket configuration — the lowest-level network primitive).
/// `tokio` is intentionally absent — it is a general async runtime with many
/// non-network uses, so its mere presence is not evidence of a network surface;
/// the crates above are the ones that actually open sockets.
///
/// This list is representative of the network crates a contributor would
/// plausibly introduce, not an exhaustive enumeration of every socket-capable
/// crate. New network families should be added here as they become relevant.
const DENIED_EXACT: &[&str] = &[
    "reqwest",
    "hyper",
    "ureq",
    "attohttpc",
    "isahc",
    "surf",
    "h2",
    "h3",
    "tonic",
    "socket2",
];

/// Denied crate-name *prefixes* — families published as many sub-crates.
///
/// The AWS SDK ships as `aws-sdk-s3`, `aws-sdk-dynamodb`, … — enumerating every
/// sub-crate would be brittle, so the whole family is matched by prefix.
const DENIED_PREFIXES: &[&str] = &["aws-sdk"];

/// Returns `true` if `name` is a denylisted socket/HTTP crate.
///
/// Matching strategy: exact match against [`DENIED_EXACT`] OR prefix match
/// against [`DENIED_PREFIXES`]. Exact matching for atomic names avoids false
/// positives (e.g. an unrelated crate whose name merely contains `h2`), while
/// prefix matching catches multi-crate families like the AWS SDK.
fn is_denied(name: &str) -> bool {
    DENIED_EXACT.contains(&name) || DENIED_PREFIXES.iter().any(|p| name.starts_with(p))
}

/// Resolve the crate names in the **default-feature** dependency tree of the
/// shipped `logos` binary (normal + build edges, dev excluded), via `cargo
/// tree`. `--offline` keeps it network-free (the registry cache is warm after
/// the build that precedes `cargo test`); feature resolution is what makes this
/// scoped to the default build rather than the union lock.
fn default_feature_tree_crates() -> Vec<String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let output = Command::new(cargo)
        .args([
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
        ])
        .output()
        .expect("`cargo tree` runs (the no-network fitness gate must be able to resolve the tree)");

    assert!(
        output.status.success(),
        "`cargo tree` failed; the no-network fitness gate could not resolve the default tree:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );

    let stdout = String::from_utf8(output.stdout).expect("`cargo tree` output is UTF-8");
    let mut names: Vec<String> = stdout
        .lines()
        // Each line is "name vX.Y.Z [(...)]"; the crate name is the first token.
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_string)
        .collect();
    names.sort_unstable();
    names.dedup();
    names
}

/// Asserts the default-feature dependency tree contains no socket/HTTP crate.
#[test]
fn default_dependency_graph_has_no_network_crate() {
    let offenders: Vec<String> = default_feature_tree_crates()
        .into_iter()
        .filter(|name| is_denied(name))
        .collect();

    assert!(
        offenders.is_empty(),
        "no-network invariant violated (NFR-SE-01): the DEFAULT-feature dependency \
         tree of the `logos` binary contains socket/HTTP crate(s) {offenders:?}. \
         Logos is local-only and must never open a network connection in the \
         default build. The web surface's axum stack must stay behind the \
         non-default `ui` feature (ADR-27); if this dependency is genuinely \
         required in the default build, the invariant — and \
         docs/security/trusted-input-boundary.md — must be revisited \
         deliberately, not silently."
    );
}

/// Pins the matching semantics of [`is_denied`] directly.
///
/// The fitness function above only observes a *passing* result when no offender
/// is present, so a regression that silently broke `is_denied` (wrong negation,
/// a typo'd `starts_with`, an emptied list) would not be caught by it. This test
/// asserts both branches — exact match and prefix match — plus the deliberate
/// `tokio` exclusion, so the guard cannot quietly become a no-op.
#[test]
fn is_denied_matches_expected_names() {
    // Exact matches (each entry of DENIED_EXACT).
    assert!(is_denied("reqwest"));
    assert!(is_denied("hyper"));
    assert!(is_denied("h2"));
    assert!(is_denied("tonic"));
    assert!(is_denied("socket2"));

    // Prefix matches (DENIED_PREFIXES).
    assert!(is_denied("aws-sdk-s3"));
    assert!(is_denied("aws-sdk-dynamodb"));

    // Must NOT match: general-purpose crates and near-misses.
    assert!(!is_denied("tokio")); // async runtime — non-network uses, see docs
    assert!(!is_denied("serde"));
    assert!(!is_denied("h2o")); // unrelated; exact match must not over-fire on "h2"
}
