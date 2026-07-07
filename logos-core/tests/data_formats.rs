//! Behavioural integration test for the YAML/JSON/TOML data-format artifact
//! plugins (S-063, [CR-010]), driving the plugin substrate, the generic config
//! extractor, and the discover → extract → persist pipeline through their public
//! APIs.
//!
//! Covers the S-063 acceptance criteria that need a real parse over the three
//! data grammars:
//! - the verification preflight ([FR-CG-06]): each grammar's crate resolves, the
//!   grammar loads at the workspace tree-sitter ABI, the descriptor parses, the
//!   plugin is the artifact class, and a fixture corpus parses;
//! - generic depth-bounded `ConfigSection` extraction with deterministic
//!   `path#anchor` ids and byte-identical re-index ([FR-CG-02], [NFR-RA-06]);
//! - lock files excluded by default and re-admitted on a glob override, and
//!   section keys FTS-searchable + kind-filterable ([UAT-CG-02], [FR-CG-04]);
//! - measured node/FTS growth on a config-heavy fixture, bounded by the depth
//!   cap ([FR-CG-02], BR-30).
//!
//! Gated on all three data features so a `--no-default-features` build that
//! excludes them does not run these data-format assertions.
#![cfg(all(feature = "lang-yaml", feature = "lang-json", feature = "lang-toml"))]

use std::fs;
use std::path::Path;

use logos_core::extract::{extract, FileInput, SymbolContext};
use logos_core::model::NodeKind;
use logos_core::plugin::LanguageRegistry;
use logos_core::{Engine, Runtime};

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Write `contents` to `<root>/<rel>`, creating parent directories.
fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().expect("has parent")).expect("mkdir");
    fs::write(path, contents).expect("write fixture");
}

/// The human-facing names of every node of `kind` currently in the graph.
fn node_names(rt: &Runtime, kind: NodeKind) -> Vec<String> {
    rt.submit_read(move |store| {
        Ok(store
            .all_nodes()?
            .into_iter()
            .filter(|n| n.kind == kind)
            .map(|n| n.name)
            .collect())
    })
    .expect("read runs")
}

/// The count of every node of `kind` currently in the graph.
fn node_count(rt: &Runtime, kind: NodeKind) -> usize {
    node_names(rt, kind).len()
}

/// The project-relative paths of every indexed file.
fn indexed_paths(rt: &Runtime) -> Vec<String> {
    rt.submit_read(|store| Ok(store.indexed_files()?.into_iter().map(|f| f.path).collect()))
        .expect("read runs")
}

/// The config-section symbols emitted for `src` under `rel`, in canonical order.
fn section_symbols(reg: &LanguageRegistry, rel: &str, src: &str) -> Vec<String> {
    let plugin = reg.for_path(rel).expect("a data-format plugin claims rel");
    let facts = extract(&FileInput::new(rel, src), plugin, &SymbolContext::default());
    facts
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::ConfigSection)
        .map(|n| n.symbol.as_str().to_string())
        .collect()
}

// ── FR-CG-06: the three-grammar verification preflight ────────────────────────

#[test]
fn verification_preflight_lists_all_three_grammars_as_artifact_plugins() {
    // The preflight, surfaced through the same `languages` read-model
    // `logos languages` prints (FR-PL-06 as modified): each grammar's crate
    // resolved, it loaded at the workspace ABI, and its descriptor parsed.
    let tmp = tempfile::tempdir().unwrap();
    let info = Engine::open(tmp.path()).languages();

    for (name, exts) in [
        ("yaml", &["yml", "yaml"][..]),
        ("json", &["json"][..]),
        ("toml", &["toml"][..]),
    ] {
        let d = info
            .languages
            .iter()
            .find(|d| d.name == name)
            .unwrap_or_else(|| panic!("`logos languages` must list the {name} grammar (FR-CG-06)"));
        assert!(
            exts.iter().all(|e| d.extensions.iter().any(|x| x == e)),
            "{name} claims {exts:?}, got {:?}",
            d.extensions
        );
        assert_eq!(d.abi_version, 14, "{name} loaded at ABI 14");
        assert!(d.artifact, "{name} is an artifact-class plugin (CR-010)");
        assert!(
            d.capabilities.is_empty(),
            "a data-format artifact declares no tagging capabilities: {:?}",
            d.capabilities
        );
    }

    // The registry exposes each as an artifact plugin under every claimed
    // extension (the load-path half of the preflight).
    let reg = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");
    for ext in ["yml", "yaml", "json", "toml"] {
        let p = reg
            .for_extension(ext)
            .unwrap_or_else(|| panic!(".{ext} resolves to a loaded grammar"));
        assert!(p.is_artifact(), ".{ext} is an artifact plugin");
        assert!(
            !p.is_documentation(),
            ".{ext} is the artifact class, not documentation"
        );
    }
}

#[test]
fn fixture_corpus_parses_into_a_configfile_for_each_grammar() {
    // The parse half of the preflight (FR-CG-06): a representative fixture for
    // each format parses without a binding/parse failure and yields exactly one
    // `ConfigFile` root.
    let tmp = tempfile::tempdir().unwrap();
    let reg = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");
    let ctx = SymbolContext::default();

    let corpus = [
        ("config.yaml", "name: svc\nport: 8080\n"),
        ("config.json", r#"{"name":"svc","port":8080}"#),
        ("config.toml", "name = \"svc\"\n[server]\nport = 8080\n"),
    ];
    for (rel, src) in corpus {
        let plugin = reg.for_path(rel).expect("a data-format plugin claims it");
        let facts = extract(&FileInput::new(rel, src), plugin, &ctx);
        assert!(
            facts.warnings.is_empty(),
            "{rel} parsed cleanly, got warnings: {:?}",
            facts.warnings
        );
        assert!(!facts.partial, "{rel} is not a partial parse");
        assert_eq!(
            facts
                .nodes
                .iter()
                .filter(|n| n.kind == NodeKind::ConfigFile)
                .count(),
            1,
            "{rel} yields exactly one ConfigFile root"
        );
    }
}

// ── FR-CG-02 / BR-30: depth-bounded ConfigSection extraction ──────────────────

#[test]
fn yaml_yields_configfile_and_depth_bounded_sections_with_path_anchor_ids() {
    let tmp = tempfile::tempdir().unwrap();
    let reg = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");
    let ctx = SymbolContext::default();

    // Four mapping levels: top → nested → deep → deeper. Only the two shallowest
    // become sections (the fixed depth bound of 2, BR-30).
    let src = "\
top:
  nested:
    deep:
      deeper: 1
sibling: 2
";
    let plugin = reg.for_extension("yaml").expect("yaml grammar");
    let facts = extract(&FileInput::new("conf/app.yaml", src), plugin, &ctx);

    assert_eq!(
        facts
            .nodes
            .iter()
            .filter(|n| n.kind == NodeKind::ConfigFile)
            .count(),
        1,
        "one ConfigFile root"
    );
    let sections: Vec<&str> = facts
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::ConfigSection)
        .map(|n| n.name.as_str())
        .collect();
    // depth-1: top, sibling; depth-2: nested. deep/deeper are past the bound.
    assert!(sections.contains(&"top"), "got {sections:?}");
    assert!(sections.contains(&"sibling"), "got {sections:?}");
    assert!(sections.contains(&"nested"), "got {sections:?}");
    assert!(
        !sections.contains(&"deep") && !sections.contains(&"deeper"),
        "depth-3/4 keys must not be emitted: {sections:?}"
    );

    // Identity is the `…/path/key#` anchor form reusing ADR-07.
    let top = facts
        .nodes
        .iter()
        .find(|n| n.kind == NodeKind::ConfigSection && n.name == "top")
        .expect("the top section");
    let sym = top.symbol.as_str();
    assert!(sym.contains("app.yaml"), "id carries the file path: {sym}");
    assert!(sym.contains('#'), "a section uses the `#` anchor: {sym}");
}

#[test]
fn json_yields_depth_bounded_sections_under_contains() {
    let tmp = tempfile::tempdir().unwrap();
    let reg = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");
    let ctx = SymbolContext::default();

    let src = r#"{"top":{"nested":{"deep":{"deeper":1}}},"sibling":2}"#;
    let plugin = reg.for_extension("json").expect("json grammar");
    let facts = extract(&FileInput::new("conf/app.json", src), plugin, &ctx);

    let sections: Vec<&str> = facts
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::ConfigSection)
        .map(|n| n.name.as_str())
        .collect();
    // JSON keys carry the surrounding quotes verbatim (the raw key text).
    assert!(
        sections.iter().any(|s| s.contains("top")),
        "got {sections:?}"
    );
    assert!(
        sections.iter().any(|s| s.contains("nested")),
        "got {sections:?}"
    );
    assert!(
        !sections.iter().any(|s| s.contains("deep")),
        "depth-3+ keys past the bound: {sections:?}"
    );

    // The nested section is Contains-nested under its parent (FR-CG-02).
    let sym = |needle: &str| {
        facts
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::ConfigSection && n.name.contains(needle))
            .map(|n| n.symbol.clone())
            .unwrap()
    };
    let top = sym("top");
    let nested = sym("nested");
    assert!(
        facts
            .edges
            .iter()
            .any(|e| e.source == top && e.target == nested),
        "nested is Contains-nested under top"
    );
}

#[test]
fn toml_tables_and_pairs_are_sections_with_inner_pairs_at_depth_two() {
    let tmp = tempfile::tempdir().unwrap();
    let reg = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");
    let ctx = SymbolContext::default();

    let src = "\
title = \"x\"
[server]
host = \"a\"
[[items]]
name = \"one\"
";
    let plugin = reg.for_extension("toml").expect("toml grammar");
    let facts = extract(&FileInput::new("conf/app.toml", src), plugin, &ctx);

    let sections: Vec<&str> = facts
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::ConfigSection)
        .map(|n| n.name.as_str())
        .collect();
    // depth-1: the top-level pair (named by its source line), the `[server]`
    // table, and the `[[items]]` array element; depth-2: each table's inner pair.
    assert!(
        sections.iter().any(|s| s.contains("title")),
        "top-level pair: {sections:?}"
    );
    assert!(
        sections.iter().any(|s| s.contains("[server]")),
        "table header: {sections:?}"
    );
    assert!(
        sections.iter().any(|s| s.contains("[[items]]")),
        "table-array header: {sections:?}"
    );
    assert!(
        sections.iter().any(|s| s.contains("host")),
        "inner pair (depth 2): {sections:?}"
    );

    // The inner `host` pair is Contains-nested under the `[server]` table.
    let sym = |needle: &str| {
        facts
            .nodes
            .iter()
            .find(|n| n.kind == NodeKind::ConfigSection && n.name.contains(needle))
            .map(|n| n.symbol.clone())
            .unwrap()
    };
    assert!(
        facts
            .edges
            .iter()
            .any(|e| e.source == sym("[server]") && e.target == sym("host")),
        "host is nested under [server]"
    );
}

#[test]
fn re_extraction_is_byte_identical_for_all_three_formats() {
    // The determinism the `path#anchor` identity + fixed depth bound rest on
    // (NFR-RA-06): re-extracting the same source yields identical section ids,
    // including ordinal disambiguation of repeated keys.
    let tmp = tempfile::tempdir().unwrap();
    let reg = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");

    let cases = [
        // The same `env` key under two distinct parents (`a`/`b`): the section
        // ids stay unique by their distinct parent chains, not by ordinal.
        ("svc.yaml", "a:\n  env: x\nb:\n  env: y\n"),
        ("svc.json", r#"{"a":{"env":"x"},"b":{"env":"y"}}"#),
        ("svc.toml", "[a]\nenv = \"x\"\n[b]\nenv = \"y\"\n"),
    ];
    for (rel, src) in cases {
        let first = section_symbols(&reg, rel, src);
        let second = section_symbols(&reg, rel, src);
        assert_eq!(first, second, "{rel} re-extraction is byte-identical");
        // No two emitted section ids collide.
        let mut sorted = first.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), first.len(), "{rel} section ids are unique");
    }

    // Sibling-ordinal disambiguation proper: two object members with the *same*
    // key under the *same* parent (JSON permits duplicate object keys). They
    // slugify identically, so identity must fall back to document-order ordinals
    // — both are emitted, with distinct, reproducible ids ([ADR-07], NFR-RA-06).
    let dup = r#"{"dup":1,"dup":2}"#;
    let first = section_symbols(&reg, "dup.json", dup);
    assert_eq!(
        first,
        section_symbols(&reg, "dup.json", dup),
        "re-extract stable"
    );
    assert_eq!(
        first.len(),
        2,
        "both sibling `dup` sections are emitted: {first:?}"
    );
    assert_ne!(
        first[0], first[1],
        "same-key siblings are ordinal-disambiguated, not collided: {first:?}"
    );
}

// ── UAT-CG-02 / FR-CG-04: lock-file exclude, glob override, FTS search ────────

#[test]
fn lock_files_are_excluded_by_default_and_re_admitted_on_override() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // A regular config file plus the full default-excluded lock-file set —
    // package-lock.json (root + nested), Cargo.lock, yarn.lock, pnpm-lock.yaml,
    // and a *.min.json — so every shipped default exclude is exercised
    // end-to-end (UAT-CG-02 names package-lock.json and Cargo.lock explicitly).
    write(root, "config.yaml", "name: svc\n");
    write(
        root,
        "package-lock.json",
        r#"{"name":"locked","lockfileVersion":3}"#,
    );
    write(root, "frontend/package-lock.json", r#"{"name":"nested"}"#);
    write(root, "Cargo.lock", "[[package]]\nname = \"x\"\n");
    write(root, "yarn.lock", "# yarn lockfile v1\n");
    write(root, "pnpm-lock.yaml", "lockfileVersion: '6.0'\n");
    write(root, "vendor/app.min.json", r#"{"a":1}"#);

    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    let rt = engine.runtime().expect("runtime present");

    let files = indexed_paths(rt);
    assert!(
        files.contains(&"config.yaml".to_string()),
        "the regular config is indexed: {files:?}"
    );
    // Every default-excluded lock file is absent at any depth (the full
    // default_config_exclude() set, not just a subset).
    for excluded in [
        "package-lock.json",
        "frontend/package-lock.json",
        "Cargo.lock",
        "yarn.lock",
        "pnpm-lock.yaml",
        "vendor/app.min.json",
    ] {
        assert!(
            !files.iter().any(|p| p == excluded),
            "{excluded} must be excluded by default: {files:?}"
        );
    }

    // Override the exclude set to re-admit package-lock.json (drop it from the
    // default lock-file excludes); the YAML lock stays excluded.
    let tmp2 = tempfile::tempdir().unwrap();
    let root2 = tmp2.path();
    write(
        root2,
        ".logos/config.toml",
        "[config_artifacts]\nexclude = [\"**/pnpm-lock.yaml\"]\n",
    );
    write(root2, "package-lock.json", r#"{"name":"locked"}"#);
    write(root2, "pnpm-lock.yaml", "lockfileVersion: '6.0'\n");

    let engine2 = Engine::start(root2).expect("engine starts");
    engine2.index();
    let rt2 = engine2.runtime().expect("runtime present");
    let files2 = indexed_paths(rt2);
    assert!(
        files2.contains(&"package-lock.json".to_string()),
        "package-lock.json re-admitted on override: {files2:?}"
    );
    assert!(
        !files2.contains(&"pnpm-lock.yaml".to_string()),
        "the still-excluded lock stays out: {files2:?}"
    );
}

#[test]
fn section_keys_are_fts_searchable_and_kind_filterable() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(
        root,
        "service.yaml",
        "database:\n  connectionString: postgres\nlogging:\n  level: debug\n",
    );

    let engine = Engine::start(root).expect("engine starts");
    engine.index();

    // A known section key is found, filtered to the ConfigSection kind.
    let hits = engine.search("connectionString", Some(NodeKind::ConfigSection), None);
    assert!(hits.warnings.is_empty(), "{:?}", hits.warnings);
    assert!(
        hits.hits.iter().any(|h| h.name == "connectionString"),
        "the section key is FTS-searchable: {:?}",
        hits.hits
    );
    assert!(
        hits.hits.iter().all(|h| h.kind == NodeKind::ConfigSection),
        "the kind filter restricts to ConfigSection: {:?}",
        hits.hits
    );

    // The kind filter genuinely excludes: a Function-kind search for the same key
    // returns nothing (config nodes never carry the code kinds).
    let none = engine.search("connectionString", Some(NodeKind::Function), None);
    assert!(
        none.hits.is_empty(),
        "a config key is not a Function: {:?}",
        none.hits
    );
}

// ── FR-CG-02 / BR-30: measured node/FTS growth on a config-heavy fixture ──────

#[test]
fn node_and_fts_growth_on_a_config_heavy_fixture_is_depth_bounded() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    // A config-heavy fixture: several files per format, each with deep nesting
    // that the depth bound must cap, plus wide top-level breadth.
    write(
        root,
        "deploy/values.yaml",
        "\
replicaCount: 2
image:
  repository: app
  tag: v1
  pullPolicy: IfNotPresent
resources:
  limits:
    cpu: 500m
    memory: 512Mi
  requests:
    cpu: 250m
service:
  type: ClusterIP
  port: 80
",
    );
    write(
        root,
        "pkg/app.json",
        r#"{"name":"app","version":"1.0.0","scripts":{"build":"tsc","test":"jest"},"dependencies":{"left":"1","right":"2"}}"#,
    );
    write(
        root,
        "Cargo-ish.toml",
        "\
[package]
name = \"demo\"
version = \"0.1.0\"
[dependencies]
serde = \"1\"
[server.tls]
enabled = true
cert = \"a.pem\"
",
    );

    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    let rt = engine.runtime().expect("runtime present");

    let files = node_count(rt, NodeKind::ConfigFile);
    let sections = node_count(rt, NodeKind::ConfigSection);
    // Three config files indexed.
    assert_eq!(files, 3, "one ConfigFile per indexed data file");
    // The growth is exact and deterministic — and, crucially, *bounded*: this is
    // the regression guard for the depth cap. The fixture has many depth-3 keys
    // (the YAML `resources.limits.cpu`/`memory`, `requests.cpu`) that the bound
    // drops, so the emitted count is far below the raw key count. The expected
    // total is the value recorded in the impl notes: YAML 11 + JSON 8 + TOML 8.
    // Asserting it exactly means removing or widening the depth bound (which would
    // emit the dropped depth-3 keys) fails this test loudly.
    assert_eq!(
        sections, 27,
        "depth-bounded section growth is exact (YAML 11 + JSON 8 + TOML 8); got {sections}"
    );

    // No depth-3 key leaked in (the bound holds end-to-end through the pipeline).
    let names = node_names(rt, NodeKind::ConfigSection);
    assert!(
        !names.iter().any(|n| n == "cpu" || n == "memory"),
        "depth-3 keys (limits.cpu/memory, requests.cpu) must not appear: {names:?}"
    );

    // nodes_fts is a 1:1 external-content index over nodes.name, so FTS rows grow
    // exactly with node count; every section key is therefore individually
    // searchable. Spot-check representative keys across formats and depths — a
    // YAML depth-1 (`replicaCount`) and depth-2 (`repository`), a JSON object
    // member (`scripts`), and a TOML table token (`package`, from `[package]`,
    // which the FTS tokenizer splits out of the bracketed header).
    for key in ["replicaCount", "repository", "scripts", "package"] {
        let hits = engine.search(key, Some(NodeKind::ConfigSection), None);
        assert!(
            hits.hits.iter().any(|h| h.name.contains(key)),
            "{key} is FTS-searchable: {:?}",
            hits.hits
        );
    }

    // Record the measured growth for the impl notes (visible with --nocapture).
    println!(
        "GROWTH config-heavy fixture: {files} ConfigFile + {sections} ConfigSection = {} config nodes (= {} FTS rows added)",
        files + sections,
        files + sections,
    );
}

// ── FR-CG-05 / UAT-CG-01: end-to-end metric-neutrality over a real grammar ────

/// The byte-identical fitness at the **engine** level over real, pipeline-indexed
/// config nodes — the proof S-062 deferred to "a story with a real config
/// grammar" (S-062 impl notes, Known limitations). Adding YAML/JSON/TOML config
/// artifacts to a project leaves the aggregate quality signal byte-identical: the
/// config nodes are `is_non_code()` and excluded from the hydrated code subgraph,
/// so they never enter metrics, cycles, DSM, or dead-code ([FR-CG-05],
/// [UAT-CG-01]).
#[cfg(feature = "lang-rust")]
#[test]
fn config_artifacts_are_metric_neutral_end_to_end() {
    const CODE: &str = "\
pub fn alpha(n: u32) -> u32 {
    if n > 0 {
        n
    } else {
        0
    }
}

pub fn beta() -> u32 {
    alpha(1)
}
";

    // (a) code only.
    let bare = tempfile::tempdir().unwrap();
    write(bare.path(), "src/lib.rs", CODE);
    let e1 = Engine::start(bare.path()).expect("engine starts");
    e1.index();
    let m1 = e1.scan(true).expect("scan runs").metrics;
    assert!(
        !m1.empty && m1.aggregate_signal.is_some(),
        "the code-only project has a signal"
    );

    // (b) the same code plus a pile of config artifacts in all three formats.
    let mixed = tempfile::tempdir().unwrap();
    write(mixed.path(), "src/lib.rs", CODE);
    write(
        mixed.path(),
        "deploy/values.yaml",
        "image:\n  repository: app\n  tag: v1\nservice:\n  port: 80\n",
    );
    write(
        mixed.path(),
        "pkg/app.json",
        r#"{"name":"app","scripts":{"build":"tsc"}}"#,
    );
    write(
        mixed.path(),
        "settings.toml",
        "[server]\nhost = \"a\"\n[server.tls]\nenabled = true\n",
    );
    let e2 = Engine::start(mixed.path()).expect("engine starts");
    e2.index();
    let m2 = e2.scan(true).expect("scan runs").metrics;

    // The config nodes genuinely exist in (b)...
    let rt2 = e2.runtime().expect("runtime present");
    assert!(
        node_count(rt2, NodeKind::ConfigSection) > 0,
        "config artifacts were indexed in the mixed project"
    );
    // ...yet the signal and the code-graph counts are byte-identical to the
    // config-free project — config artifacts move no metric (FR-CG-05).
    assert_eq!(
        m1.aggregate_signal, m2.aggregate_signal,
        "aggregate_signal is byte-identical with config artifacts added vs absent"
    );
    assert_eq!(
        m1.function_count, m2.function_count,
        "the code function count is unchanged by config artifacts"
    );
}
