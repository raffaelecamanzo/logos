//! No-network fitness function (NFR-SE-01, ADR-17, ADR-27, ADR-60).
//!
//! Logos is local-only and never phones home. This test enforces that invariant
//! *structurally*: it resolves the dependency tree of the shipped `logos` binary
//! and fails the build if a socket / HTTP crate has entered it. A regression
//! (someone adding `reqwest`, an AWS SDK, a gRPC stack, etc.) is caught at `cargo
//! test` time rather than discovered by a network sandbox at runtime (NFR-SE-01,
//! [UAT-OB-01], [UAT-UI-02] step 1).
//!
//! # Two trees, two denylists (CR-078, ADR-60)
//!
//! Until CR-078 there was one tree and one denylist: the **default** tree denied
//! the whole socket stack — HTTP clients *and* the `hyper` server stack + raw
//! `socket2`. That worked while the web dashboard lived behind a *non-default*
//! `ui` feature ([ADR-27]): the default binary linked no server at all, so
//! denying `hyper`/`h2`/`socket2` in the default tree was exactly right.
//!
//! CR-078/[ADR-60] made the dashboard the shipped **default** (S-287) while
//! keeping the LLM egress *client* behind a separate, opt-in `agents` feature.
//! The default binary now *legitimately links the loopback HTTP server*
//! (`hyper`/`hyper-util`/`socket2`) so it can **listen** — but it still links no
//! HTTP *client*, so it can never **dial**. A single all-or-nothing denylist can
//! no longer express that: `hyper` is a false positive against the shipped
//! default (the dashboard needs it to serve) yet a true positive against the
//! slim absolute-offline build (which links nothing network at all). So the
//! check splits along the listen/dial seam ADR-60 draws:
//!
//! - the **default-tree** check ([`default_tree_denies_only_egress_client_crates`])
//!   denies only the egress *client* crates — the ones that open an **outbound**
//!   connection (`reqwest`, `ureq`, `attohttpc`, `isahc`, `surf`, `tonic`, the
//!   `aws-sdk-*` family). `hyper`/`h2`/`socket2` are deliberately **not** here:
//!   the loopback dashboard server links them to listen, never to dial. This is
//!   the guarantee that matters for the shipped binary — *it cannot phone home*.
//! - the **slim-tree** check ([`slim_tree_denies_the_full_network_stack`]) keeps
//!   the original full denylist **verbatim** (server stack included) and runs it
//!   over the `--no-default-features` build — the genuine headless binary with no
//!   listener at all. This is the absolute-offline anchor: it links **nothing**
//!   that can open or accept a socket, and this check keeps that honest.
//!
//! The `ui`-vs-default `rig`/`reqwest` boundary and the behavioral zero-egress
//! proofs get their own carve-out tests (agent-core/tests/carve_out.rs,
//! web/tests/carve_out.rs, and the sandboxed session, [UAT-UI-02] steps 2–4);
//! this file owns the two structural denylist anchors.
//!
//! # Why the resolved *tree*, not the raw `Cargo.lock` (CR-012, ADR-27)
//!
//! `Cargo.lock` is the union of every dependency and is *not* feature-partitioned,
//! so a raw-lock scan reports optional/feature-gated crates as false positives
//! (e.g. `socket2` unifies into `tokio`'s lock entry). Both checks below inspect
//! the *feature-resolved* tree of the `logos` package — exactly the graph that
//! ships in each build, and the same scope cargo-deny uses (`deny.toml [graph]
//! all-features = false`).

use std::process::Command;

/// Egress **client** crates — the ones that open an *outbound* connection. These
/// are denied in **both** trees: they must never enter the shipped binary, in
/// any build short of the opt-in `agents` feature.
///
/// HTTP clients (`reqwest`, `ureq`, `attohttpc`, `isahc`, `surf`) and the `tonic`
/// gRPC client stack. Matched exactly as atomic crate names. The `aws-sdk-*`
/// family is matched by prefix (see [`DENIED_CLIENT_PREFIXES`]).
const DENIED_CLIENT_EXACT: &[&str] = &[
    "reqwest",
    "ureq",
    "attohttpc",
    "isahc",
    "surf",
    "tonic",
];

/// The `hyper` server/client stack, the HTTP/2 + HTTP/3 protocol crates, and
/// `socket2` (raw socket configuration — the lowest-level network primitive).
///
/// These are denied **only** in the slim tree (the absolute-offline anchor). The
/// shipped default deliberately links `hyper`/`h2`/`socket2` for the loopback
/// dashboard *server* — it listens, it does not dial — so they are *not* on the
/// default-tree denylist (CR-078, ADR-60). Concatenated with
/// [`DENIED_CLIENT_EXACT`] to form the full offline denylist.
const DENIED_SERVER_STACK_EXACT: &[&str] = &["hyper", "h2", "h3", "socket2"];

/// Denied crate-name *prefixes* — families published as many sub-crates. Applied
/// in both trees (an AWS SDK is an egress client, not a listener).
///
/// The AWS SDK ships as `aws-sdk-s3`, `aws-sdk-dynamodb`, … — enumerating every
/// sub-crate would be brittle, so the whole family is matched by prefix.
const DENIED_CLIENT_PREFIXES: &[&str] = &["aws-sdk"];

/// Returns `true` if `name` is an egress **client** crate (denied in every build).
///
/// Exact match against [`DENIED_CLIENT_EXACT`] OR prefix match against
/// [`DENIED_CLIENT_PREFIXES`]. Exact matching for atomic names avoids false
/// positives; prefix matching catches multi-crate families like the AWS SDK.
fn is_egress_client(name: &str) -> bool {
    DENIED_CLIENT_EXACT.contains(&name)
        || DENIED_CLIENT_PREFIXES.iter().any(|p| name.starts_with(p))
}

/// Returns `true` if `name` is denied in the **slim** absolute-offline tree — the
/// full stack: every egress client PLUS the `hyper` server stack / `h2` / `h3` /
/// `socket2`. This is the original pre-CR-078 denylist, verbatim.
fn is_denied_offline(name: &str) -> bool {
    is_egress_client(name) || DENIED_SERVER_STACK_EXACT.contains(&name)
}

/// Resolve the crate names in the dependency tree of the shipped `logos` binary
/// (normal + build edges, dev excluded) for the given feature selection, via
/// `cargo tree`. `--offline` keeps it network-free (the registry cache is warm
/// after the build that precedes `cargo test`); feature resolution is what scopes
/// this to a specific build rather than the union lock.
///
/// `no_default` drops the default feature set (the slim build); `extra_features`
/// adds an explicit feature list on top (e.g. `lang-all` for the grammars).
fn logos_tree_crates(no_default: bool, extra_features: Option<&str>) -> Vec<String> {
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
    if no_default {
        args.push("--no-default-features");
    }
    if let Some(features) = extra_features {
        args.push("--features");
        args.push(features);
    }

    let output = Command::new(cargo)
        .args(&args)
        .output()
        .expect("`cargo tree` runs (the no-network fitness gate must be able to resolve the tree)");

    assert!(
        output.status.success(),
        "`cargo tree` failed; the no-network fitness gate could not resolve the tree:\n{}",
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

/// **Default-tree anchor** (CR-078, ADR-60): the shipped `logos` binary — default
/// features (`lang-all` + `ui`) — must link **no egress client crate**. It links
/// the loopback dashboard server (`hyper`/`socket2`) to *listen*, but nothing
/// that can *dial*, so it can never phone home even though `ui` is now default.
///
/// Passes on the current default tree (`ui` on, `agents` off). It fails the
/// moment any egress client enters — e.g. flipping `agents` into the default set
/// would pull `reqwest` in; the non-vacuity guard below proves this check *does*
/// fire on such a tree.
#[test]
fn default_tree_denies_only_egress_client_crates() {
    let offenders: Vec<String> = logos_tree_crates(false, None)
        .into_iter()
        .filter(|name| is_egress_client(name))
        .collect();

    assert!(
        offenders.is_empty(),
        "no-network invariant violated (NFR-SE-01): the DEFAULT-feature dependency \
         tree of the shipped `logos` binary contains egress-client crate(s) \
         {offenders:?}. The default build ships the loopback dashboard (`ui`) and \
         may link an HTTP *server* to listen, but it must link no HTTP/gRPC *client* \
         — it can never dial out. The LLM egress client is opt-in behind the \
         `agents` feature (ADR-60); if this dependency is genuinely required in the \
         default build, the invariant — and docs/security/trusted-input-boundary.md \
         — must be revisited deliberately, not silently."
    );
}

/// **Slim-tree anchor** (NFR-SE-01): the genuine headless binary
/// (`--no-default-features --features lang-all`) — no dashboard, no listener —
/// must link **nothing** that can open or accept a socket. This runs the original
/// full denylist verbatim (egress clients PLUS the `hyper` server stack / `h2` /
/// `h3` / `socket2`) and is the absolute-offline guarantee CR-078 preserves.
///
/// `lang-all` only turns on the tree-sitter grammars (pure parsers, no network),
/// so this is exactly the shipped slim binary — the same scope
/// `agent-core/tests/carve_out.rs` resolves for its `--no-default-features` case.
#[test]
fn slim_tree_denies_the_full_network_stack() {
    let offenders: Vec<String> = logos_tree_crates(true, Some("lang-all"))
        .into_iter()
        .filter(|name| is_denied_offline(name))
        .collect();

    assert!(
        offenders.is_empty(),
        "no-network invariant violated (NFR-SE-01): the SLIM (`--no-default-features \
         --features lang-all`) dependency tree of the `logos` binary contains \
         socket/HTTP crate(s) {offenders:?}. The slim build is the absolute-offline \
         anchor — no dashboard, no listener — and must link nothing that can open \
         or accept a socket. If a listener is genuinely wanted here, it belongs \
         behind the `ui` feature (ADR-27, ADR-60), not in the slim tree."
    );
}

/// Non-vacuity guard for the default-tree check: resolving the **`agents`** tree
/// — the one build that legitimately links the egress client — proves the
/// egress-client filter actually *fires* when `reqwest` is present. Without this,
/// a regression that silently emptied [`DENIED_CLIENT_EXACT`] would let
/// [`default_tree_denies_only_egress_client_crates`] pass on a tree that had
/// quietly pulled in an HTTP client. This is the "negative fixture" the default
/// tree can't provide (it is egress-free by construction): the `agents` tree is a
/// real tree that *does* contain `reqwest`.
#[test]
fn egress_client_filter_fires_on_the_agents_tree() {
    let agents_offenders: Vec<String> = logos_tree_crates(false, Some("agents"))
        .into_iter()
        .filter(|name| is_egress_client(name))
        .collect();

    assert!(
        agents_offenders.iter().any(|c| c == "reqwest"),
        "the `agents` tree must contain `reqwest` (the opt-in egress client) and the \
         egress-client filter must catch it — otherwise the default-tree check could \
         pass vacuously on a tree that had leaked an HTTP client. Found: \
         {agents_offenders:?}",
    );
}

/// Pins the matching semantics of [`is_egress_client`] and [`is_denied_offline`]
/// directly.
///
/// The fitness functions above only observe a *passing* result when no offender
/// is present, so a regression that silently broke a matcher (wrong negation, a
/// typo'd `starts_with`, an emptied list) would not be caught by them. This test
/// asserts every branch — exact match, prefix match, the deliberate `tokio`
/// exclusion, and crucially the CR-078 listen/dial split: `hyper`/`h2`/`socket2`
/// are *offline*-denied but *not* egress clients, so they may ride the default
/// (listening) tree while never being allowed into the slim (offline) tree.
#[test]
fn denylist_matchers_encode_the_listen_dial_split() {
    // Egress clients — denied in EVERY build (exact matches).
    for client in ["reqwest", "ureq", "attohttpc", "isahc", "surf", "tonic"] {
        assert!(is_egress_client(client), "{client} is an egress client");
        assert!(is_denied_offline(client), "{client} is denied offline too");
    }
    // The AWS SDK family — prefix match, in both denylists.
    for aws in ["aws-sdk-s3", "aws-sdk-dynamodb"] {
        assert!(is_egress_client(aws), "{aws} matches the aws-sdk prefix");
        assert!(is_denied_offline(aws), "{aws} is denied offline too");
    }

    // The listen/dial split (CR-078, ADR-60): the server stack is NOT an egress
    // client (so the loopback dashboard may link it in the default tree) but IS
    // denied in the slim offline tree (which links no listener at all).
    for server in ["hyper", "h2", "h3", "socket2"] {
        assert!(
            !is_egress_client(server),
            "{server} is a listen-side crate, not an egress client — the default \
             dashboard legitimately links it to serve (CR-078, ADR-60)",
        );
        assert!(
            is_denied_offline(server),
            "{server} must stay denied in the slim absolute-offline tree (NFR-SE-01)",
        );
    }

    // Must NOT match in either list: general-purpose crates and near-misses.
    for benign in ["tokio", "serde", "h2o"] {
        assert!(!is_egress_client(benign), "{benign} is not an egress client");
        assert!(!is_denied_offline(benign), "{benign} is not a network crate");
    }
    // `tokio` is the async runtime (many non-network uses); `h2o` must not
    // over-fire on the `h2` exact match.
}
