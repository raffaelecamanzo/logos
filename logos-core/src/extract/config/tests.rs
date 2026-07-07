//! Generic config-extractor tests (S-062, CR-010).
//!
//! These exercise the substrate's *mechanism* — the artifact routing arm, the
//! `ConfigFile` root, the descriptor-driven depth-bounded `ConfigSection` walk,
//! and `path#anchor` determinism — using a **synthetic** artifact plugin built
//! from the markdown block grammar (whose `section` nodes nest cleanly by heading
//! level). The real YAML/JSON/TOML proofs land in S-063, which ships those
//! grammars; the substrate ships no format grammar of its own, so the depth
//! bound is proven here against the one nesting grammar already compiled in.

use std::collections::BTreeMap;

use crate::extract::{extract, FileInput, SymbolContext};
use crate::model::NodeKind;
use crate::plugin::{CompiledPlugin, LanguagePlugin, PluginManifest};

/// Build a synthetic artifact-class plugin over the markdown block grammar,
/// declaring `section_kinds = ["section"]` so the generic walk treats each
/// markdown heading section as a `ConfigSection`. `with_config` toggles the
/// `[config]` table to also exercise the typed-anchor-only (ConfigFile-only) path.
fn fake_artifact_plugin(with_config: bool) -> CompiledPlugin {
    let toml = if with_config {
        r#"
            name = "fakeyaml"
            extensions = ["fakeyaml"]
            module_separator = "/"
            abi_version = 15
            capabilities = []
            artifact = true
            [config]
            section_kinds = ["section"]
        "#
    } else {
        r#"
            name = "fakeyaml"
            extensions = ["fakeyaml"]
            module_separator = "/"
            abi_version = 15
            capabilities = []
            artifact = true
        "#
    };
    let manifest = PluginManifest::parse("fakeyaml/plugin.toml", toml).unwrap();
    let language: tree_sitter::Language = tree_sitter_md::LANGUAGE.into();
    CompiledPlugin::new(manifest, language, BTreeMap::new(), Vec::new())
}

fn nodes_of_kind(facts: &crate::extract::Facts, kind: NodeKind) -> Vec<&str> {
    facts
        .nodes
        .iter()
        .filter(|n| n.kind == kind)
        .map(|n| n.name.as_str())
        .collect()
}

/// A 4-level-nested fixture yields a single `ConfigFile` and sections only at
/// depth ≤ 2 — the fixed depth bound ([FR-CG-02], BR-30). The two deepest
/// headings (depth 3 and 4) are deliberately invisible.
#[test]
fn depth_bound_caps_sections_at_two_levels() {
    let plugin = fake_artifact_plugin(true);
    // `# A` nests `## B` nests `### C` nests `#### D` — four mapping levels.
    let src = "# A\n\n## B\n\n### C\n\n#### D\n\ntail\n";
    let facts = extract(
        &FileInput::new("config.fakeyaml", src),
        &plugin,
        &SymbolContext::default(),
    );

    let files = nodes_of_kind(&facts, NodeKind::ConfigFile);
    assert_eq!(files, ["config.fakeyaml"], "exactly one ConfigFile root");

    let sections = nodes_of_kind(&facts, NodeKind::ConfigSection);
    assert_eq!(
        sections.len(),
        2,
        "only depth-1 and depth-2 sections are emitted (bound 2), got {sections:?}"
    );
    // The two shallowest headings; the deeper C/D are past the bound.
    assert!(sections.iter().any(|n| n.contains("A")));
    assert!(sections.iter().any(|n| n.contains("B")));
    assert!(
        !sections.iter().any(|n| n.contains("C") || n.contains("D")),
        "depth-3/4 sections must not be emitted: {sections:?}"
    );
}

/// Every config node is a config kind, excluded from the code subgraph at
/// hydration — the metric-neutrality contract at the node level ([FR-CG-05]).
#[test]
fn every_emitted_node_is_a_config_kind() {
    let plugin = fake_artifact_plugin(true);
    let src = "# top\n\n## nested\n";
    let facts = extract(
        &FileInput::new("a.fakeyaml", src),
        &plugin,
        &SymbolContext::default(),
    );
    assert!(!facts.nodes.is_empty());
    for node in &facts.nodes {
        assert!(
            node.kind.is_config() && node.kind.is_non_code(),
            "{} is a config / non-code kind",
            node.kind.as_str()
        );
    }
    // The only edges are Contains (the layer is Contains-only) and they connect
    // config nodes — so they vanish from the code subgraph with their endpoints.
    for edge in &facts.edges {
        assert_eq!(edge.kind, crate::model::EdgeKind::Contains);
    }
}

/// Sections nest under their parent by `Contains`, and identity is the
/// `…/path/key#` anchor form reusing ADR-07 — a `ConfigSection` symbol carries
/// the file path and the `#` type-descriptor anchor.
#[test]
fn sections_use_path_anchor_identity_under_contains() {
    let plugin = fake_artifact_plugin(true);
    let src = "# alpha\n\n## beta\n";
    let facts = extract(
        &FileInput::new("dir/app.fakeyaml", src),
        &plugin,
        &SymbolContext::default(),
    );

    let section = facts
        .nodes
        .iter()
        .find(|n| n.kind == NodeKind::ConfigSection && n.name.contains("alpha"))
        .expect("a top-level section");
    let sym = section.symbol.as_str();
    assert!(
        sym.contains("app.fakeyaml"),
        "identity carries the file path: {sym}"
    );
    assert!(
        sym.contains('#'),
        "a section uses the type-descriptor anchor: {sym}"
    );

    // A nested section is Contains-nested under its parent section (FR-CG-02).
    let nested = facts
        .nodes
        .iter()
        .find(|n| n.kind == NodeKind::ConfigSection && n.name.contains("beta"))
        .expect("a nested section");
    assert!(
        facts
            .edges
            .iter()
            .any(|e| e.source == section.symbol && e.target == nested.symbol),
        "the nested section is Contains-nested under its parent"
    );
}

/// Re-extracting the same fixture is byte-identical — the determinism the
/// `path#anchor` identity and fixed depth bound rest on ([NFR-RA-06]).
#[test]
fn re_extraction_is_byte_identical() {
    let plugin = fake_artifact_plugin(true);
    let src = "# one\n\n## a\n\n## a\n\n# two\n";
    let input = FileInput::new("dup.fakeyaml", src);
    let first = extract(&input, &plugin, &SymbolContext::default());
    let second = extract(&input, &plugin, &SymbolContext::default());

    let syms = |f: &crate::extract::Facts| {
        f.nodes
            .iter()
            .map(|n| n.symbol.as_str().to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        syms(&first),
        syms(&second),
        "node identities are reproducible"
    );
    // Two sibling `## a` sections slugify identically and are ordinal-disambiguated,
    // so their symbols differ — no collision, still deterministic.
    let a_sections: Vec<_> = first
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::ConfigSection && n.name.contains('a'))
        .map(|n| n.symbol.as_str())
        .collect();
    assert_eq!(a_sections.len(), 2, "both sibling `a` sections are emitted");
    assert_ne!(
        a_sections[0], a_sections[1],
        "sibling collisions are ordinal-disambiguated"
    );
}

/// A typed-anchor-only artifact (no `[config]` table) yields just the
/// `ConfigFile` root — no generic sections — so a Dockerfile/Protobuf-style
/// format gets its file node and attaches its own anchors in its format walk.
#[test]
fn config_file_only_when_no_section_descriptor() {
    let plugin = fake_artifact_plugin(false);
    let src = "# heading\n\n## nested\n";
    let facts = extract(
        &FileInput::new("Dockerfile.fakeyaml", src),
        &plugin,
        &SymbolContext::default(),
    );
    assert_eq!(nodes_of_kind(&facts, NodeKind::ConfigFile).len(), 1);
    assert!(
        nodes_of_kind(&facts, NodeKind::ConfigSection).is_empty(),
        "no [config] table ⇒ no generic sections, ConfigFile only"
    );
}

/// The artifact routing arm: `is_artifact()` sends a file through the config
/// extractor (ConfigFile), never the code `symbols` path.
#[test]
fn artifact_plugin_routes_through_the_config_extractor() {
    let plugin = fake_artifact_plugin(true);
    assert!(plugin.is_artifact());
    let facts = extract(
        &FileInput::new("x.fakeyaml", "# h\n"),
        &plugin,
        &SymbolContext::default(),
    );
    assert_eq!(facts.language, "fakeyaml");
    assert_eq!(nodes_of_kind(&facts, NodeKind::ConfigFile).len(), 1);
}

/// The `node_kind` override (S-064): a build/schema/infra format names a typed
/// anchor kind, and the *same* generic walk emits it instead of `ConfigSection`.
/// Proven here with the markdown carrier grammar so the substrate mechanism is
/// covered independently of any real build grammar; the real Dockerfile/Makefile/
/// Shell proofs ride their own grammars in `build_formats::tests`.
#[test]
fn node_kind_override_emits_a_typed_anchor() {
    let toml = r#"
        name = "faketyped"
        extensions = ["faketyped"]
        module_separator = "/"
        abi_version = 15
        capabilities = []
        artifact = true
        [config]
        section_kinds = ["section"]
        node_kind = "make_target"
    "#;
    let manifest = PluginManifest::parse("faketyped/plugin.toml", toml).unwrap();
    let language: tree_sitter::Language = tree_sitter_md::LANGUAGE.into();
    let plugin = CompiledPlugin::new(manifest, language, BTreeMap::new(), Vec::new());

    let src = "# build\n\n# test\n";
    let facts = extract(
        &FileInput::new("Makefile.faketyped", src),
        &plugin,
        &SymbolContext::default(),
    );

    // The walk emits the overridden kind, never the default ConfigSection.
    // (Names carry the markdown carrier's `# ` prefix — the real grammars in
    // `build_formats::tests` prove clean names; here only the *kind* matters.)
    assert_eq!(
        nodes_of_kind(&facts, NodeKind::MakeTarget),
        vec!["# build", "# test"]
    );
    assert!(
        nodes_of_kind(&facts, NodeKind::ConfigSection).is_empty(),
        "node_kind override replaces ConfigSection entirely"
    );
    // The ConfigFile root is still emitted, and every typed anchor is non-code
    // (metric-neutral, FR-CG-05).
    assert_eq!(nodes_of_kind(&facts, NodeKind::ConfigFile).len(), 1);
    assert!(
        facts.nodes.iter().all(|n| n.kind.is_non_code()),
        "ConfigFile + typed anchors are all non-code"
    );
}
