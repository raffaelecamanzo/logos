//! The rmcp features Logos compiles are pinned, and the suppressions that rely
//! on them are backed by this test (S-781, NFR-MA-10, FR-SC-11, CR-241).
//!
//! `.logos/security-ignore.toml` suppresses four `Cargo.lock` advisories against
//! rmcp. Each one sits in rmcp's Streamable-HTTP transport or its OAuth client
//! (features `transport-streamable-http-*` and `auth`), which Logos does not
//! compile: it enables `server`, `transport-io` and `macros` (stdio), plus
//! `client` in this crate's dev tests. A suppression is only true while that
//! holds, and a `Cargo.toml` edit that enables one of those features would reopen
//! the advisories without a single scanner run noticing. This file turns the
//! claim in the suppressions' `reason` into a failing test.
//!
//! Two layers, both offline:
//!
//! - **Declared** — `cargo metadata --no-deps --offline` lists every workspace
//!   package's `rmcp` dependency with its declared features, and each package's
//!   own `[features]` entries that forward to an rmcp feature (`rmcp/<feature>`). It needs no
//!   registry and no resolution, so it still names the feature when the edit
//!   under test pulls crates the offline cache does not hold.
//! - **Resolved** — `cargo tree -e features --offline` lists the features cargo
//!   actually enables across the workspace, which also catches a feature switched
//!   on indirectly (another crate's feature, a new dependent). The expected set
//!   is the declared roots closed over rmcp's own `[features]` table from
//!   `cargo metadata --offline --filter-platform <host>`, so an implied feature
//!   such as `transport-async-rw` is not a false positive. The filter matters:
//!   unfiltered, `cargo metadata` needs every target's crates (Android's
//!   included), which a build on one host never downloads, so it failed offline
//!   on CI.
//!
//! The assertions are non-vacuous: no `rmcp` dependency, no `server` feature, an
//! empty `rmcp` tree or an ignore file with no `rmcp` entry each fail. The pure
//! checks take their input as text so the fixtures below can plant each case.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

/// The features Logos compiles in a shipped build — stdio server only.
const BUILD_FEATURES: &[&str] = &["server", "transport-io", "macros"];
/// What the mcp crate's dev tests add on top: the in-process client.
const DEV_FEATURES: &[&str] = &["client"];

/// The ignore file's own name, as the failure messages cite it.
const IGNORE_FILE: &str = ".logos/security-ignore.toml";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the mcp crate sits one level under the workspace root")
        .to_path_buf()
}

fn set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|n| (*n).to_string()).collect()
}

/// Which family a feature belongs to when it is one the suppressions cover.
/// Matched on the feature's own name by prefix or exact name, so a near miss
/// (`transport-ssh`, `authz`) is not claimed.
fn forbidden_family(feature: &str) -> Option<&'static str> {
    if feature.starts_with("transport-streamable-http") {
        Some("the Streamable-HTTP transport")
    } else if feature.starts_with("transport-sse") {
        Some("an SSE transport")
    } else if feature == "auth" || feature.starts_with("auth-") {
        Some("the OAuth client")
    } else {
        None
    }
}

// ---- the ignore file ------------------------------------------------------

/// One `[[ignore]]` block's fields this test reads.
#[derive(Debug, Default)]
struct IgnoreEntry {
    path: String,
    rule: String,
    reason: String,
}

/// Read the `[[ignore]]` blocks from the file's text. Only flat `key = "value"`
/// lines are understood, which is the whole of what the file holds.
fn parse_ignore_entries(text: &str) -> Vec<IgnoreEntry> {
    let mut entries: Vec<IgnoreEntry> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line == "[[ignore]]" {
            entries.push(IgnoreEntry::default());
            continue;
        }
        let (Some(entry), Some((key, value))) = (entries.last_mut(), line.split_once('=')) else {
            continue;
        };
        let value = value.trim().trim_matches('"').to_string();
        match key.trim() {
            "path" => entry.path = value,
            "rule" => entry.rule = value,
            "reason" => entry.reason = value,
            _ => {}
        }
    }
    entries
}

/// The rules of the `Cargo.lock` suppressions that rest on rmcp's feature set.
fn rmcp_suppression_rules(entries: &[IgnoreEntry]) -> Vec<String> {
    entries
        .iter()
        .filter(|e| e.path == "Cargo.lock" && e.reason.contains("rmcp"))
        .map(|e| e.rule.clone())
        .collect()
}

/// Non-vacuity of the suppression side: at least one rmcp entry must exist,
/// otherwise there is nothing for the feature pin to back.
fn check_suppressions(rules: &[String]) -> Result<(), String> {
    if rules.is_empty() {
        return Err(format!(
            "{IGNORE_FILE} holds no `Cargo.lock` suppression that names rmcp, so this test backs nothing: \
             either the suppressions were removed (then retire this test with them) or their `reason` \
             no longer names rmcp (restore it)"
        ));
    }
    Ok(())
}

fn forbidden_message(feature: &str, family: &str, location: &str, rules: &[String]) -> String {
    format!(
        "rmcp feature `{feature}` ({family}) is enabled {location}. The {n} `Cargo.lock` suppressions in \
         {IGNORE_FILE} ({list}) are valid only while Logos does not compile rmcp's Streamable-HTTP \
         transport, SSE transports or OAuth client; enabling it reopens those advisories. Remove the \
         feature, or re-assess and rewrite the suppressions first.",
        n = rules.len(),
        list = rules.join(", "),
    )
}

// ---- layer 1: declared features -------------------------------------------

/// One `rmcp` dependency of a workspace package, as `cargo metadata` declares it.
#[derive(Debug)]
struct RmcpDep {
    package: String,
    /// `None` for a normal dependency, else `"dev"` or `"build"`.
    kind: Option<String>,
    features: BTreeSet<String>,
    uses_default: bool,
}

impl RmcpDep {
    fn location(&self) -> String {
        format!(
            "by {}'s {} dependency on rmcp",
            self.package,
            self.kind.as_deref().unwrap_or("normal")
        )
    }
}

fn rmcp_deps(no_deps_metadata: &Value) -> Vec<RmcpDep> {
    let mut found = Vec::new();
    for package in no_deps_metadata["packages"]
        .as_array()
        .into_iter()
        .flatten()
    {
        for dep in package["dependencies"].as_array().into_iter().flatten() {
            if dep["name"] != "rmcp" {
                continue;
            }
            found.push(RmcpDep {
                package: package["name"].as_str().unwrap_or("?").to_string(),
                kind: dep["kind"].as_str().map(str::to_string),
                features: dep["features"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|f| f.as_str().map(str::to_string))
                    .collect(),
                uses_default: dep["uses_default_features"].as_bool().unwrap_or(true),
            });
        }
    }
    found
}

/// An `rmcp/<feature>` (or `rmcp?/<feature>`) entry in a workspace package's own
/// `[features]` table. It is not part of the package's `rmcp` dependency entry,
/// so [`rmcp_deps`] cannot see it, yet building with that package feature
/// enables the rmcp feature.
#[derive(Debug)]
struct Forward {
    package: String,
    package_feature: String,
    rmcp_feature: String,
}

fn rmcp_forwards(no_deps_metadata: &Value) -> Vec<Forward> {
    let mut found = Vec::new();
    for package in no_deps_metadata["packages"]
        .as_array()
        .into_iter()
        .flatten()
    {
        for (name, entries) in package["features"].as_object().into_iter().flatten() {
            for entry in entries.as_array().into_iter().flatten() {
                let forwarded = entry
                    .as_str()
                    .and_then(|e| e.strip_prefix("rmcp/").or_else(|| e.strip_prefix("rmcp?/")));
                if let Some(rmcp_feature) = forwarded {
                    found.push(Forward {
                        package: package["name"].as_str().unwrap_or("?").to_string(),
                        package_feature: name.clone(),
                        rmcp_feature: rmcp_feature.to_string(),
                    });
                }
            }
        }
    }
    found
}

/// A forwarded feature is optional, so it is outside every pinned build: any
/// forbidden one is the named failure, and any other one outside the pinned
/// roots is an unexpected addition.
fn check_forwards(forwards: &[Forward], rules: &[String]) -> Result<(), String> {
    let pinned: BTreeSet<String> = set(BUILD_FEATURES)
        .union(&set(DEV_FEATURES))
        .cloned()
        .collect();
    for forward in forwards {
        let location = format!(
            "through {}'s own feature `{}` (`rmcp/{}`)",
            forward.package, forward.package_feature, forward.rmcp_feature
        );
        if let Some(family) = forbidden_family(&forward.rmcp_feature) {
            return Err(forbidden_message(
                &forward.rmcp_feature,
                family,
                &location,
                rules,
            ));
        }
        if !pinned.contains(&forward.rmcp_feature) {
            return Err(format!(
                "rmcp feature `{}` is enabled {location} and is not in the pinned set {pinned:?} (unexpected). \
                 The `Cargo.lock` suppressions in {IGNORE_FILE} ({}) rest on that set; change this test and \
                 the suppressions together.",
                forward.rmcp_feature,
                rules.join(", "),
            ));
        }
    }
    Ok(())
}

fn check_declared(deps: &[RmcpDep], rules: &[String]) -> Result<(), String> {
    if deps.is_empty() {
        return Err(
            "no workspace package declares a dependency on rmcp, so there is no feature set to pin \
             (the MCP surface cannot be built without it)"
                .to_string(),
        );
    }
    // The named failure comes first: a forbidden feature is the reason this
    // test exists, and must be reported as such rather than as a set mismatch.
    for dep in deps {
        for feature in &dep.features {
            if let Some(family) = forbidden_family(feature) {
                return Err(forbidden_message(feature, family, &dep.location(), rules));
            }
        }
    }
    for dep in deps {
        if dep.uses_default {
            return Err(format!(
                "{} leaves rmcp's default features on; they are not part of the pinned set \
                 (`server`, `transport-io`, `macros`) — declare `default-features = false`",
                dep.location().replacen("by ", "", 1)
            ));
        }
        let mut expected = set(BUILD_FEATURES);
        if dep.kind.as_deref() == Some("dev") {
            expected.extend(set(DEV_FEATURES));
        }
        if dep.features != expected {
            return Err(format!(
                "rmcp features declared {} are {:?}; the pinned set is {:?} (missing {:?}, unexpected {:?}). \
                 The `Cargo.lock` suppressions in {IGNORE_FILE} ({}) rest on that set; change this test and \
                 the suppressions together.",
                dep.location(),
                dep.features,
                expected,
                expected.difference(&dep.features).collect::<Vec<_>>(),
                dep.features.difference(&expected).collect::<Vec<_>>(),
                rules.join(", "),
            ));
        }
    }
    // Non-vacuity: a normal dependency that carries `server` must exist.
    if !deps
        .iter()
        .any(|d| d.kind.is_none() && d.features.contains("server"))
    {
        return Err(
            "no normal dependency on rmcp enables `server`; the stdio server is not built"
                .to_string(),
        );
    }
    Ok(())
}

// ---- layer 2: resolved features -------------------------------------------

/// The features rmcp has in a `cargo tree -e features --prefix none` listing.
/// A line counts only when its package is exactly `rmcp`, so `rmcp-macros` and a
/// crate merely named like it are not read as rmcp.
fn tree_features(tree_output: &str) -> BTreeSet<String> {
    tree_output
        .lines()
        .filter_map(|line| line.strip_prefix("rmcp feature \""))
        .filter_map(|rest| rest.split('"').next())
        .map(str::to_string)
        .collect()
}

/// The feature names `roots` enable, directly or through rmcp's own `[features]`
/// table. Entries that are not features (`dep:x`, `tokio/io-util`) are skipped.
fn feature_closure(
    table: &BTreeMap<String, Vec<String>>,
    roots: &BTreeSet<String>,
) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut todo: Vec<String> = roots.iter().cloned().collect();
    while let Some(feature) = todo.pop() {
        if !out.insert(feature.clone()) {
            continue;
        }
        for implied in table.get(&feature).into_iter().flatten() {
            if table.contains_key(implied) {
                todo.push(implied.clone());
            }
        }
    }
    out
}

fn check_resolved(
    label: &str,
    tree_output: &str,
    table: &BTreeMap<String, Vec<String>>,
    roots: &BTreeSet<String>,
    rules: &[String],
) -> Result<(), String> {
    let enabled = tree_features(tree_output);
    if enabled.is_empty() {
        return Err(format!(
            "rmcp has no enabled feature in the {label} dependency graph: rmcp is absent from it, so the \
             stdio server is not built and the suppressions in {IGNORE_FILE} back nothing"
        ));
    }
    for feature in &enabled {
        if let Some(family) = forbidden_family(feature) {
            return Err(forbidden_message(
                feature,
                family,
                &format!("in the {label} graph"),
                rules,
            ));
        }
    }
    if !enabled.contains("server") {
        return Err(format!(
            "rmcp's `server` feature is not enabled in the {label} graph"
        ));
    }
    let expected = feature_closure(table, roots);
    if enabled != expected {
        return Err(format!(
            "rmcp's resolved features in the {label} graph are {enabled:?}; the pinned set closed over rmcp's \
             feature table is {expected:?} (missing {:?}, unexpected {:?}). The suppressions in {IGNORE_FILE} \
             ({}) rest on that set.",
            expected.difference(&enabled).collect::<Vec<_>>(),
            enabled.difference(&expected).collect::<Vec<_>>(),
            rules.join(", "),
        ));
    }
    Ok(())
}

// ---- running cargo --------------------------------------------------------

/// The host target triple, as `rustc -vV` reports it.
fn host_triple() -> String {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let output = Command::new(rustc)
        .arg("-vV")
        .output()
        .expect("rustc -vV runs");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|l| l.strip_prefix("host: "))
        .expect("rustc -vV names the host")
        .trim()
        .to_string()
}

fn cargo(args: &[&str]) -> Result<String, String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let output = Command::new(cargo)
        .args(args)
        .current_dir(repo_root())
        .output()
        .map_err(|e| format!("`cargo {}` did not start: {e}", args.join(" ")))?;
    if !output.status.success() {
        return Err(format!(
            "`cargo {}` failed:\n{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    String::from_utf8(output.stdout)
        .map_err(|e| format!("`cargo {}` output is not UTF-8: {e}", args.join(" ")))
}

fn live_rules() -> Vec<String> {
    let text = std::fs::read_to_string(repo_root().join(IGNORE_FILE))
        .unwrap_or_else(|e| panic!("{IGNORE_FILE} must be readable: {e}"));
    rmcp_suppression_rules(&parse_ignore_entries(&text))
}

fn rmcp_feature_table(metadata: &Value) -> BTreeMap<String, Vec<String>> {
    let rmcp = metadata["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|p| p["name"] == "rmcp")
        .expect("rmcp is in the resolved package list");
    rmcp["features"]
        .as_object()
        .expect("a package lists its features as an object")
        .iter()
        .map(|(name, implied)| {
            let implied = implied
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect();
            (name.clone(), implied)
        })
        .collect()
}

// ---- the live gates -------------------------------------------------------

#[test]
fn the_declared_rmcp_features_are_the_stdio_server_set() {
    let metadata = cargo(&[
        "metadata",
        "--no-deps",
        "--offline",
        "--format-version",
        "1",
    ])
    .expect("cargo metadata --no-deps resolves the workspace manifests");
    let metadata: Value = serde_json::from_str(&metadata).expect("cargo metadata prints JSON");
    let rules = live_rules();
    if let Err(message) = check_forwards(&rmcp_forwards(&metadata), &rules) {
        panic!("{message}");
    }
    if let Err(message) = check_declared(&rmcp_deps(&metadata), &rules) {
        panic!("{message}");
    }
}

#[test]
fn the_resolved_rmcp_features_are_the_stdio_server_set() {
    let host = host_triple();
    let metadata = cargo(&[
        "metadata",
        "--offline",
        "--format-version",
        "1",
        "--filter-platform",
        &host,
    ])
    .expect("cargo metadata resolves offline for the host (the build warmed the host's crates)");
    let table =
        rmcp_feature_table(&serde_json::from_str(&metadata).expect("cargo metadata prints JSON"));
    let rules = live_rules();

    let tree = |edges: &str| {
        cargo(&[
            "tree",
            "--workspace",
            // Every package feature on: a feature that forwards to an rmcp
            // feature (`foo = ["rmcp/<feature>"]`) is off in a default build
            // but is one `--features foo` away from the shipped one.
            "--all-features",
            "-e",
            edges,
            "--invert",
            "rmcp",
            "--prefix",
            "none",
            "--color",
            "never",
            "--offline",
        ])
    };
    // Builds: normal and build edges only — what ships.
    let build = tree("features,normal,build").expect("cargo tree resolves rmcp's build features");
    if let Err(message) = check_resolved("build", &build, &table, &set(BUILD_FEATURES), &rules) {
        panic!("{message}");
    }
    // Dev tests: the in-process client joins.
    let dev = tree("features,normal,build,dev").expect("cargo tree resolves rmcp's dev features");
    let mut dev_roots = set(BUILD_FEATURES);
    dev_roots.extend(set(DEV_FEATURES));
    if let Err(message) = check_resolved("dev-test", &dev, &table, &dev_roots, &rules) {
        panic!("{message}");
    }
}

#[test]
fn the_ignore_file_holds_the_rmcp_suppressions_this_test_backs() {
    let rules = live_rules();
    if let Err(message) = check_suppressions(&rules) {
        panic!("{message}");
    }
}

// ---- the checks against planted input -------------------------------------

#[cfg(test)]
mod planted {
    use super::*;
    use serde_json::json;

    fn rules() -> Vec<String> {
        [
            "CVE-2026-63127",
            "CVE-2026-63128",
            "CVE-2026-64684",
            "GHSA-c9xm-49cp-xcr9",
        ]
        .iter()
        .map(|r| (*r).to_string())
        .collect()
    }

    fn dep(kind: Option<&str>, features: &[&str], uses_default: bool) -> RmcpDep {
        RmcpDep {
            package: "mcp".to_string(),
            kind: kind.map(str::to_string),
            features: set(features),
            uses_default,
        }
    }

    fn good_deps() -> Vec<RmcpDep> {
        vec![
            dep(None, BUILD_FEATURES, false),
            dep(
                Some("dev"),
                &["server", "transport-io", "macros", "client"],
                false,
            ),
        ]
    }

    #[test]
    fn the_pinned_declaration_passes() {
        assert_eq!(check_declared(&good_deps(), &rules()), Ok(()));
    }

    #[test]
    fn each_forbidden_feature_fails_naming_it_and_the_four_suppressions() {
        for feature in [
            "transport-streamable-http-server",
            "transport-streamable-http-client",
            "transport-streamable-http-client-reqwest",
            "transport-sse-client",
            "transport-sse-server",
            "transport-sse",
            "auth",
            "auth-client-credentials-jwt",
        ] {
            let mut features: Vec<&str> = BUILD_FEATURES.to_vec();
            features.push(feature);
            let message = check_declared(&[dep(None, &features, false)], &rules())
                .expect_err(&format!("`{feature}` must fail the declared check"));
            assert!(
                message.contains(&format!("`{feature}`")),
                "{feature}: {message}"
            );
            assert!(message.contains(IGNORE_FILE), "{feature}: {message}");
            for rule in rules() {
                assert!(
                    message.contains(&rule),
                    "{feature} message omits {rule}: {message}"
                );
            }
        }
    }

    #[test]
    fn a_feature_one_character_from_a_forbidden_name_is_not_claimed_as_forbidden() {
        for near_miss in [
            "transport-ssh",
            "transport-ss",
            "authz",
            "authentic",
            "transport-streamable",
            "oauth",
        ] {
            assert_eq!(forbidden_family(near_miss), None, "{near_miss}");
        }
        // It still fails — as an unexpected feature, not as a suppression breach.
        let mut features: Vec<&str> = BUILD_FEATURES.to_vec();
        features.push("transport-ssh");
        let message = check_declared(&[dep(None, &features, false)], &rules()).unwrap_err();
        assert!(message.contains("unexpected"), "{message}");
        assert!(!message.contains("is enabled"), "{message}");
    }

    #[test]
    fn a_dev_only_extra_beyond_client_fails() {
        let mut deps = good_deps();
        deps[1]
            .features
            .insert("transport-child-process".to_string());
        let message = check_declared(&deps, &rules()).unwrap_err();
        assert!(
            message.contains("unexpected [\"transport-child-process\"]"),
            "{message}"
        );
    }

    #[test]
    fn the_client_feature_is_not_allowed_in_a_build_dependency() {
        let deps = vec![dep(
            None,
            &["server", "transport-io", "macros", "client"],
            false,
        )];
        let message = check_declared(&deps, &rules()).unwrap_err();
        assert!(message.contains("unexpected [\"client\"]"), "{message}");
    }

    #[test]
    fn default_features_left_on_fail() {
        let deps = vec![dep(None, BUILD_FEATURES, true)];
        assert!(check_declared(&deps, &rules())
            .unwrap_err()
            .contains("default"));
    }

    // Non-vacuity probe 1: rmcp absent from the dependency graph.
    #[test]
    fn probe_rmcp_absent_fails() {
        assert!(check_declared(&[], &rules())
            .unwrap_err()
            .contains("no workspace package declares"));
        let message = check_resolved(
            "build",
            "",
            &BTreeMap::new(),
            &set(BUILD_FEATURES),
            &rules(),
        )
        .unwrap_err();
        assert!(message.contains("rmcp is absent"), "{message}");
    }

    // Non-vacuity probe 2: the `server` feature missing.
    #[test]
    fn probe_server_missing_fails() {
        let deps = vec![dep(None, &["transport-io", "macros"], false)];
        let message = check_declared(&deps, &rules()).unwrap_err();
        assert!(message.contains("missing [\"server\"]"), "{message}");

        let table = BTreeMap::new();
        let tree = "rmcp v1.7.0\nrmcp feature \"macros\"\nrmcp feature \"transport-io\"\n";
        let message =
            check_resolved("build", tree, &table, &set(BUILD_FEATURES), &rules()).unwrap_err();
        assert!(
            message.contains("`server` feature is not enabled"),
            "{message}"
        );
    }

    // The last non-vacuity check in `check_declared`: the sets are right but
    // only a dev dependency carries them, so no shipped build has `server`.
    #[test]
    fn probe_server_only_in_a_dev_dependency_fails() {
        let deps = vec![dep(
            Some("dev"),
            &["server", "transport-io", "macros", "client"],
            false,
        )];
        let message = check_declared(&deps, &rules()).unwrap_err();
        assert!(
            message.contains("no normal dependency on rmcp enables `server`"),
            "{message}"
        );
    }

    // The set is exact in both directions: a feature that is missing fails too.
    #[test]
    fn a_declared_set_missing_a_pinned_feature_other_than_server_fails() {
        let deps = vec![dep(None, &["server", "transport-io"], false)];
        let message = check_declared(&deps, &rules()).unwrap_err();
        assert!(message.contains("missing [\"macros\"]"), "{message}");
    }

    #[test]
    fn a_resolved_graph_missing_a_pinned_feature_other_than_server_fails() {
        let tree = "rmcp feature \"macros\"\nrmcp feature \"server\"\nrmcp feature \"transport-async-rw\"\n";
        let message =
            check_resolved("build", tree, &table(), &set(BUILD_FEATURES), &rules()).unwrap_err();
        assert!(message.contains("missing [\"transport-io\"]"), "{message}");
    }

    #[test]
    fn the_closure_follows_a_chain_past_a_non_feature_entry() {
        let mut chain: BTreeMap<String, Vec<String>> = BTreeMap::new();
        chain.insert(
            "a".into(),
            vec!["dep:x".into(), "tokio/io-util".into(), "b".into()],
        );
        chain.insert("b".into(), vec!["c".into()]);
        chain.insert("c".into(), vec![]);
        assert_eq!(feature_closure(&chain, &set(&["a"])), set(&["a", "b", "c"]));
        assert_eq!(feature_closure(&chain, &set(&["c"])), set(&["c"]));
    }

    // Non-vacuity probe 3: the ignore file holds no rmcp entry.
    #[test]
    fn probe_ignore_file_without_an_rmcp_entry_fails() {
        let text = "[[ignore]]\npath = \"Cargo.lock\"\nrule = \"CVE-1\"\nreason = \"a different crate\"\n\n\
                    [[ignore]]\npath = \"web/ui/package-lock.json\"\nrule = \"CVE-2\"\nreason = \"rmcp is mentioned but this is not the lock\"\n";
        let found = rmcp_suppression_rules(&parse_ignore_entries(text));
        assert!(found.is_empty(), "{found:?}");
        assert!(check_suppressions(&found)
            .unwrap_err()
            .contains("holds no `Cargo.lock` suppression"));
        assert!(check_suppressions(&[]).is_err());
    }

    #[test]
    fn the_ignore_parser_reads_rule_path_and_reason_from_each_block() {
        let text = "[[ignore]]\npath = \"Cargo.lock\"\nrule = \"CVE-A\"\nreason = \"rmcp 1.7.0 advisory (a = b)\"\nexpires = 2027-04-19\n\n\
                    [[ignore]]\npath = \"Cargo.lock\"\nrule = \"CVE-B\"\nreason = \"rmcp too\"\n";
        assert_eq!(
            rmcp_suppression_rules(&parse_ignore_entries(text)),
            vec!["CVE-A".to_string(), "CVE-B".to_string()]
        );
    }

    fn table() -> BTreeMap<String, Vec<String>> {
        let t = |pairs: &[(&str, &[&str])]| {
            pairs
                .iter()
                .map(|(k, v)| {
                    (
                        (*k).to_string(),
                        v.iter().map(|s| (*s).to_string()).collect(),
                    )
                })
                .collect()
        };
        t(&[
            ("server", &["transport-async-rw", "dep:schemars"]),
            ("transport-io", &["transport-async-rw", "tokio/io-std"]),
            ("transport-async-rw", &["tokio/io-util"]),
            ("macros", &["dep:rmcp-macros"]),
            ("client", &["dep:tokio-stream"]),
            ("auth", &["dep:oauth2"]),
            ("transport-streamable-http-server", &["server"]),
        ])
    }

    const BUILD_TREE: &str = "rmcp v1.7.0\n\
        rmcp feature \"macros\"\nmcp v1.20.5 (/x)\n\
        rmcp feature \"server\"\nrmcp feature \"transport-async-rw\"\n\
        rmcp feature \"server\" (*)\nrmcp feature \"transport-io\"\n";

    #[test]
    fn the_resolved_build_graph_passes_with_its_implied_feature() {
        assert_eq!(
            check_resolved(
                "build",
                BUILD_TREE,
                &table(),
                &set(BUILD_FEATURES),
                &rules()
            ),
            Ok(())
        );
    }

    #[test]
    fn a_resolved_forbidden_feature_fails_naming_it() {
        for feature in [
            "transport-streamable-http-server",
            "transport-sse-client",
            "auth",
        ] {
            let tree = format!("{BUILD_TREE}rmcp feature \"{feature}\"\n");
            let message = check_resolved("build", &tree, &table(), &set(BUILD_FEATURES), &rules())
                .unwrap_err();
            assert!(message.contains(&format!("`{feature}`")), "{message}");
            assert!(message.contains("CVE-2026-64684"), "{message}");
        }
    }

    #[test]
    fn client_in_the_build_graph_fails_but_passes_in_the_dev_graph() {
        let tree = format!("{BUILD_TREE}rmcp feature \"client\"\n");
        let message =
            check_resolved("build", &tree, &table(), &set(BUILD_FEATURES), &rules()).unwrap_err();
        assert!(message.contains("unexpected [\"client\"]"), "{message}");
        let mut roots = set(BUILD_FEATURES);
        roots.extend(set(DEV_FEATURES));
        assert_eq!(
            check_resolved("dev-test", &tree, &table(), &roots, &rules()),
            Ok(())
        );
    }

    #[test]
    fn the_tree_reader_takes_rmcp_exactly() {
        let tree = "rmcp-macros feature \"server\"\n rmcp feature \"x\"\nrmcpx feature \"y\"\nrmcp feature \"z\"\n";
        assert_eq!(tree_features(tree), set(&["z"]));
    }

    #[test]
    fn the_declared_reader_takes_only_rmcp_dependencies() {
        let metadata = json!({"packages": [
            {"name": "mcp", "dependencies": [
                {"name": "rmcp", "kind": null, "features": ["server"], "uses_default_features": false},
                {"name": "rmcp-macros", "kind": null, "features": ["auth"], "uses_default_features": false},
                {"name": "rmcp", "kind": "dev", "features": ["client"], "uses_default_features": false}]}]});
        let deps = rmcp_deps(&metadata);
        assert_eq!(deps.len(), 2);
        assert_eq!(deps[1].kind.as_deref(), Some("dev"));
        assert_eq!(deps[0].features, set(&["server"]));
    }

    fn metadata_with_forward(entry: &str) -> Value {
        json!({"packages": [{"name": "mcp", "dependencies": [],
            "features": {"default": [], "http": [entry, "dep:other"]}}]})
    }

    #[test]
    fn a_package_feature_forwarding_a_forbidden_rmcp_feature_fails_naming_it() {
        for entry in [
            "rmcp/transport-streamable-http-server-session",
            "rmcp?/transport-sse-client",
            "rmcp/auth",
        ] {
            let forwards = rmcp_forwards(&metadata_with_forward(entry));
            assert_eq!(forwards.len(), 1, "{entry}");
            let message = check_forwards(&forwards, &rules()).unwrap_err();
            let feature = entry.split('/').nth(1).unwrap();
            assert!(message.contains(&format!("`{feature}`")), "{message}");
            assert!(message.contains("mcp's own feature `http`"), "{message}");
            assert!(message.contains("GHSA-c9xm-49cp-xcr9"), "{message}");
        }
    }

    #[test]
    fn a_forwarded_feature_outside_the_pinned_set_fails_and_a_pinned_one_passes() {
        let message = check_forwards(
            &rmcp_forwards(&metadata_with_forward("rmcp/elicitation")),
            &rules(),
        )
        .unwrap_err();
        assert!(message.contains("unexpected"), "{message}");
        assert_eq!(
            check_forwards(
                &rmcp_forwards(&metadata_with_forward("rmcp/client")),
                &rules()
            ),
            Ok(())
        );
    }

    #[test]
    fn the_forward_reader_takes_only_rmcp_entries() {
        for entry in [
            "rmcp-macros/auth",
            "rmcpx/auth",
            "dep:rmcp",
            "tokio/auth",
            "other/rmcp/auth",
        ] {
            assert!(
                rmcp_forwards(&metadata_with_forward(entry)).is_empty(),
                "{entry}"
            );
        }
    }
}
