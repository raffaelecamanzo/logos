//! A file a code plugin claims by its exact name is read as a file of that
//! language's extensions (S-840, [FR-EX-26], [FR-RS-33], [FR-RS-34]).
//!
//! Every lookup the resolver keys on a file's language — the layout's
//! per-language predicates (the arg-free ones [`layout_answers`] lists), the
//! load path, the relative-require map, member access and framework candidacy
//! — asks the registry's claim on the path
//! ([`LanguageRegistry::for_path`]) through one claim key, so a language that
//! claims `Name` answers for `Name` as it does for `dir/x.<ext>`. The test walks
//! every loaded code plugin that claims a name, so no language id is spelt here
//! and a future plugin that claims one is held to the same parity.
//!
//! [FR-EX-26]: ../../../docs/specs/requirements/FR-EX-26.md
//! [FR-RS-33]: ../../../docs/specs/requirements/FR-RS-33.md
//! [FR-RS-34]: ../../../docs/specs/requirements/FR-RS-34.md

use std::collections::HashSet;

use super::binder::IndexBuilder;
use super::framework::{framework_semantics, read_member_file};
use super::load_path;
use super::package_key::PackageLayout;
use crate::plugin::registry::{claimed_file_names, declares_by_extension};
use crate::plugin::{LanguagePlugin, LanguageRegistry};

fn registry() -> LanguageRegistry {
    // A fresh root, so no `.logos/plugins` override of the machine's shared
    // temp directory changes what the test sees.
    let root = tempfile::tempdir().expect("tempdir");
    LanguageRegistry::load(root.path()).expect("registry loads")
}

/// Every `(claimed name, the plugin's extension)` pair of the loaded code
/// plugins, with the plugin that claims it.
fn claimed_pairs(registry: &LanguageRegistry) -> Vec<(String, String, &dyn LanguagePlugin)> {
    registry
        .iter()
        .flat_map(|plugin| {
            claimed_file_names(plugin).flat_map(move |name| {
                plugin.extensions().iter().map(move |ext| (name.to_string(), ext.clone(), plugin))
            })
        })
        .collect()
}

/// The layout's answers for `path`, one string per per-language lookup.
fn layout_answers(layout: &PackageLayout, path: &str) -> Vec<String> {
    vec![
        format!("{:?}", layout.family(path)),
        format!("{:?}", layout.language(path)),
        format!("{:?}", layout.companion_name(path)),
        format!("{:?}", layout.file_facade(path)),
        layout.moves_import_roots(path).to_string(),
        layout.has_import_roots(path).to_string(),
        layout.sees_enclosing_namespaces(path).to_string(),
        layout.matches_type_arity(path).to_string(),
        layout.resolves_lexical_constants(path).to_string(),
        layout.declares_directory_packages(path).to_string(),
        layout.links_units(path).to_string(),
        layout.pairs_companion_objects(path).to_string(),
        layout.declares_top_level_callables(path).to_string(),
        layout.reads_wildcards_first(path).to_string(),
        layout.binds_extension_calls(path).to_string(),
        layout.declares_file_facades(path).to_string(),
        layout.falls_back_to_global_functions(path).to_string(),
        layout.resolves_includes(path).to_string(),
        layout.bare_calls_free_only(path).to_string(),
        layout.overloads_calls(path).to_string(),
        layout.looks_up_impl_blocks(path).to_string(),
        layout.checks_arity(path).to_string(),
        layout.peels_receivers(path).to_string(),
        layout.supertype_kind_follows_target(path).to_string(),
        layout.inherits_interface_bodies(path).to_string(),
        layout.has_structural_types(path).to_string(),
        layout.declares_namespaces(path).to_string(),
        layout.is_package_file(path).to_string(),
        format!("{:?}", layout.package_dir(path)),
        format!("{:?}", layout.call_targets(path)),
    ]
}

#[test]
fn a_claimed_name_answers_every_layout_lookup_as_the_languages_extensions_do() {
    let registry = registry();
    let layout = PackageLayout::from_registry(&registry);
    let pairs = claimed_pairs(&registry);
    assert!(!pairs.is_empty(), "a shipped code plugin claims a name");
    for (name, ext, _) in &pairs {
        let by_extension = layout_answers(&layout, &format!("tasks/x.{ext}"));
        for path in [name.clone(), format!("sub/dir/{name}")] {
            assert_eq!(layout_answers(&layout, &path), by_extension, "{path} reads as .{ext}");
        }
    }
    // Not vacuous: a language that claims a name declares something beyond the
    // defaults, so the answers being equal is not two all-false rows.
    assert!(
        pairs.iter().any(|(name, _, _)| layout.resolves_lexical_constants(name)),
        "a claimed name resolves lexical constants for some loaded language"
    );
}

#[test]
fn the_one_layout_a_single_plugin_declares_answers_for_its_claimed_name_too() {
    let registry = registry();
    for (name, ext, plugin) in claimed_pairs(&registry) {
        let layout = PackageLayout::from_plugin(plugin);
        assert_eq!(
            layout_answers(&layout, &name),
            layout_answers(&layout, &format!("tasks/x.{ext}")),
            "{name} reads as .{ext} in the single-file layout"
        );
    }
}

#[test]
fn a_name_that_extends_a_claimed_one_is_unclaimed() {
    let registry = registry();
    let layout = PackageLayout::from_registry(&registry);
    for (name, _, _) in claimed_pairs(&registry) {
        // Each near miss answers as the same shape over a name no language
        // claims: `Gemfile.lock` as `Other.lock` (an extension keys it, as it
        // always did), `Gemfileish` as `Otherish`.
        for (path, control) in [
            (format!("{name}.lock"), "Other.lock".to_string()),
            (format!("{name}ish"), "Otherish".to_string()),
            (format!("{name}/inner.txt"), "Other/inner.txt".to_string()),
            (name.to_lowercase(), "other".to_string()),
        ] {
            if path == name {
                continue; // a name with no uppercase letter lower-cases to itself
            }
            assert_eq!(layout_answers(&layout, &path), layout_answers(&layout, &control), "{path} is no claimed file");
            assert!(registry.for_path(&path).is_none_or(|p| claimed_file_names(p).all(|n| n != name)), "{path}");
            assert!(framework_semantics(&registry, &path).is_none(), "{path} names no framework");
        }
    }
}

#[test]
fn a_claimed_name_binds_requires_and_member_access_as_the_languages_extensions_do() {
    let registry = registry();
    let tmp = tempfile::tempdir().expect("tempdir");
    let specifier_targets = registry.specifier_target_extensions();
    let member_access = registry.member_access_extensions();
    let reachable = registry.reachability_extensions();
    let mut compared = 0;
    for (name, ext, _) in claimed_pairs(&registry) {
        let by_extension = format!("tasks/x.{ext}");
        let files = [name.as_str(), by_extension.as_str()];
        let load_path = load_path::discover(tmp.path(), registry.load_path_extensions(), files);
        let index = IndexBuilder::new(&[], &[], &[], PackageLayout::from_registry(&registry))
            .with_path_specifiers(specifier_targets.clone(), Vec::new())
            .with_load_path(load_path.clone())
            .build();

        // Load path: the roots a bare require binds against.
        assert_eq!(load_path.roots_for(&name), load_path.roots_for(&by_extension), "{name}: load roots");
        // Relative require: a path-specifier file, which an incremental run
        // re-binds the imports of.
        assert_eq!(
            index.is_path_specifier_file(&name),
            index.is_path_specifier_file(&by_extension),
            "{name}: path specifiers"
        );
        // Lexical constants, through the binder's own delegate.
        assert_eq!(
            index.resolves_lexical_constants(&name),
            index.resolves_lexical_constants(&by_extension),
            "{name}: lexical constants"
        );
        // Member access and reachability: the metrics and annotation gates.
        for (label, set) in [("member access", &member_access), ("reachability", &reachable)] {
            assert_eq!(
                declares_by_extension(Some(&name), set),
                declares_by_extension(Some(&by_extension), set),
                "{name}: {label}"
            );
        }
        // Framework candidacy: the language's detectors.
        assert_eq!(
            framework_semantics(&registry, &name).map(|s| &s.framework_detectors),
            framework_semantics(&registry, &by_extension).map(|s| &s.framework_detectors),
            "{name}: framework detectors"
        );
        // The member-fold read of a file: the same grammar answers for the name.
        std::fs::write(tmp.path().join(&name), "").expect("write the claimed file");
        std::fs::create_dir_all(tmp.path().join("tasks")).expect("tasks dir");
        std::fs::write(tmp.path().join(&by_extension), "").expect("write the extension file");
        assert_eq!(
            read_member_file(&registry, tmp.path(), &name).map(|(plugin, _)| plugin.name().to_string()),
            read_member_file(&registry, tmp.path(), &by_extension).map(|(plugin, _)| plugin.name().to_string()),
            "{name}: the grammar a framework member scan reads it with"
        );
        compared += 1;
    }
    assert!(compared > 0, "a shipped code plugin claims a name");
}

/// A `Name` key is never an extension: a file called like a plugin's extension
/// (`rb` with no dot) is no file of that language.
#[test]
fn a_file_named_like_an_extension_is_not_that_languages() {
    let registry = registry();
    let layout = PackageLayout::from_registry(&registry);
    let unclaimed = layout_answers(&layout, "LICENSE");
    let extensions: HashSet<String> = registry.iter().flat_map(|p| p.extensions().to_vec()).collect();
    assert!(!extensions.is_empty());
    for ext in extensions {
        assert_eq!(layout_answers(&layout, &ext), unclaimed, "a file named `{ext}` has no language");
    }
}
