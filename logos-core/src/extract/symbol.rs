//! Canonical-ordinal SCIP symbol construction ([ADR-07], [ADR-08]).
//!
//! The extraction engine turns a declaration — identified by its file path, its
//! enclosing scope chain, and a per-parent-scope ordinal — into a canonical SCIP
//! symbol string, which is then validated and normalised by
//! [`LogosSymbol::parse`]. Building a *string* and routing it through the codec
//! is deliberate: it keeps the `scip` type out of this module entirely (the
//! `scip_containment` fitness test, [NFR-MA-07], [ADR-06]) while still producing
//! a guaranteed-canonical identity.
//!
//! # Why this is where ID stability lives
//!
//! A symbol is a pure function of `(package coordinate, file path, scope chain,
//! per-scope ordinal)`. The ordinal is assigned only *after* the canonical
//! sibling sort in [`super`] ([ADR-07]); none of the inputs depend on rayon
//! completion order or on byte offsets of unrelated code, so the resulting ID
//! survives both parallel extraction and unrelated edits ([NFR-RA-03],
//! [NFR-RA-06]).
//!
//! [ADR-07]: ../../../docs/specs/architecture/decisions/ADR-07.md
//! [ADR-08]: ../../../docs/specs/architecture/decisions/ADR-08.md
//! [ADR-06]: ../../../docs/specs/architecture/decisions/ADR-06.md
//! [NFR-MA-07]: ../../../docs/specs/requirements/NFR-MA-07.md
//! [NFR-RA-03]: ../../../docs/specs/requirements/NFR-RA-03.md
//! [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md

use crate::model::{LogosSymbol, NodeKind};

/// The package-coordinate prefix shared by every symbol from one extraction run.
///
/// These four fields are the SCIP global-symbol coordinate
/// (`<scheme> <manager> <package-name> <version>`); the per-symbol descriptor
/// path is appended after them. An empty `manager`/`package`/`version` is
/// rendered as the SCIP empty-field marker `.` — `scheme` is never empty (SCIP
/// requires it), defaulting to `logos`.
///
/// Resolving the *precise* per-file cargo crate + version is a
/// [resolution-engine](../../../docs/specs/architecture/components/resolution-engine.md)
/// concern (S-011); Pass 1 takes a single coordinate for the run. Symbol
/// *stability* and *uniqueness* do not depend on it — the file path and scope
/// chain in the descriptor carry the identity — so a coarse coordinate here is
/// sound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolContext {
    /// The SCIP scheme; never empty. Logos uses `logos`.
    pub scheme: String,
    /// The package manager (`cargo`, …) or empty for an unknown manager.
    pub manager: String,
    /// The package name (crate name) or empty.
    pub package: String,
    /// The package version or empty.
    pub version: String,
}

impl Default for SymbolContext {
    fn default() -> Self {
        Self {
            scheme: "logos".to_string(),
            manager: String::new(),
            package: String::new(),
            version: String::new(),
        }
    }
}

impl SymbolContext {
    /// A `logos cargo <package> <version>` coordinate — the common Rust case.
    pub fn cargo(package: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            scheme: "logos".to_string(),
            manager: "cargo".to_string(),
            package: package.into(),
            version: version.into(),
        }
    }
}

/// Split a project-relative path into the namespace segments a symbol is
/// built from, dropping empty and `.` components (`./a//b.rs` → `["a",
/// "b.rs"]`).
///
/// The single source of truth for the symbol namespace: extraction (Pass 1)
/// and the framework-promotion pass (S-012) both build symbols from these
/// segments, so any drift between them would silently fork the identity
/// space ([ADR-07]).
///
/// [ADR-07]: ../../../docs/specs/architecture/decisions/ADR-07.md
pub(crate) fn path_segments(rel: &str) -> Vec<&str> {
    rel.split('/')
        .filter(|s| !s.is_empty() && *s != ".")
        .collect()
}

/// Render one declaration as its SCIP descriptor segment, folding the ordinal
/// in to disambiguate same-`(kind, name)` siblings.
///
/// The mapping from [`NodeKind`] to SCIP descriptor suffix:
/// - `Module` → namespace (`name/`)
/// - `Class`/`Interface`/`Trait`/`Struct`/`Enum`/`TypeAlias`/`Route`/`Component`/
///   `Topic`/`Producer`/`Consumer` → type (`name#`)
/// - `Function`/`Method` → method (`name().`, or `name(N).` when `ordinal > 0`)
/// - `Field`/`Constant`/`Variable` → term (`name.`)
/// - `Macro` → macro (`name!`)
///
/// Disambiguation when `ordinal > 0`:
/// - `Function`/`Method` ride SCIP's native **method disambiguator** slot
///   (`name(N).`) — the idiomatic encoding for an inherent method and a
///   trait-impl method that share a name in one module scope.
/// - Every other kind appends a trailing **meta descriptor** (`…N:`) carrying
///   the ordinal. This is needed because non-method same-name collisions *do*
///   occur in valid Rust: e.g. an associated `const ALL` in two different `impl`
///   blocks both land in the enclosing module scope (the `impl` is not a
///   captured scope), so a `Constant`/`Type`/… can legitimately collide. Those
///   kinds are always leaves under `symbols.scm` (no captured children), so the
///   appended meta never perturbs a descendant's chain.
pub(crate) fn descriptor_for(kind: NodeKind, name: &str, ordinal: u32) -> String {
    let n = escape_name(name);
    match kind {
        NodeKind::Function | NodeKind::Method => {
            if ordinal == 0 {
                format!("{n}().")
            } else {
                format!("{n}({ordinal}).")
            }
        }
        // `Layer`/`Boundary` are derived policy nodes the annotation engine
        // materialises (S-014); extraction never emits them, but the mapping is
        // total so a future caller gets a sensible namespace/type descriptor.
        // `DocFile`/`ConfigFile` are namespace-like containers: their structural
        // symbol builders (S-033 [`super::doc`], CR-010 [`super::config`]) build
        // them from the file path with an empty scope chain, so these arms are
        // reached only for completeness.
        NodeKind::Module | NodeKind::Layer | NodeKind::DocFile | NodeKind::ConfigFile => {
            with_ordinal(format!("{n}/"), ordinal)
        }
        // `DocSection` and the typed `Requirement`/`Adr`/`Story` doc nodes
        // (S-033/S-039, CR-003), and the config layer's `ConfigSection` plus the
        // per-format typed anchors (CR-010, FR-CG-02/03), take the type-descriptor
        // `#` suffix: a section/anchor identity therefore renders as the
        // `…/path/anchor#` form ([FR-DG-02], [ADR-07] reuse), disambiguated by the
        // same trailing-ordinal meta as the other non-method kinds.
        NodeKind::Class
        | NodeKind::Interface
        | NodeKind::Trait
        | NodeKind::Struct
        | NodeKind::Enum
        | NodeKind::TypeAlias
        | NodeKind::Route
        | NodeKind::Component
        | NodeKind::Boundary
        | NodeKind::DocSection
        | NodeKind::Requirement
        | NodeKind::Adr
        | NodeKind::Story
        | NodeKind::ConfigSection
        | NodeKind::ShellFunction
        | NodeKind::DockerfileStage
        | NodeKind::MakeTarget
        | NodeKind::ProtoMessage
        | NodeKind::ProtoService
        | NodeKind::GqlType
        | NodeKind::SqlObject
        | NodeKind::TfBlock
        | NodeKind::ApiPath
        | NodeKind::ApiOperation
        // `Topic`/`Producer`/`Consumer` (CR-061, S-255) are shared-identity
        // declarations like `Route`/`Component`; extraction never emits them
        // here (S-256's concern), but the mapping stays total.
        | NodeKind::Topic
        | NodeKind::Producer
        | NodeKind::Consumer => with_ordinal(format!("{n}#"), ordinal),
        NodeKind::Field | NodeKind::Constant | NodeKind::Variable => {
            with_ordinal(format!("{n}."), ordinal)
        }
        NodeKind::Macro => with_ordinal(format!("{n}!"), ordinal),
    }
}

/// Append a meta-descriptor carrying the ordinal to a non-method descriptor when
/// it must be disambiguated (`ordinal > 0`); a no-op for the common ordinal-0
/// case so unique symbols stay clean.
fn with_ordinal(base: String, ordinal: u32) -> String {
    if ordinal == 0 {
        base
    } else {
        format!("{base}{ordinal}:")
    }
}

/// Assemble and canonicalise a global SCIP symbol.
///
/// `path_segments` are the components of the file path relative to the project
/// root (each becomes a namespace descriptor); `scope_chain` is the pre-rendered
/// descriptor of each enclosing declaration from the outermost down to and
/// including the leaf, as produced by [`descriptor_for`].
///
/// # Errors
/// Returns an error if the assembled string is not a valid SCIP symbol — which,
/// given the escaping here, should only happen for a genuinely malformed input
/// (e.g. an empty scheme).
pub(crate) fn build_symbol(
    ctx: &SymbolContext,
    path_segments: &[&str],
    scope_chain: &[String],
) -> anyhow::Result<LogosSymbol> {
    let mut raw = String::new();
    raw.push_str(field(&ctx.scheme, "logos"));
    raw.push(' ');
    raw.push_str(dot(&ctx.manager));
    raw.push(' ');
    raw.push_str(dot(&ctx.package));
    raw.push(' ');
    raw.push_str(dot(&ctx.version));
    raw.push(' ');
    for seg in path_segments {
        raw.push_str(&escape_name(seg));
        raw.push('/');
    }
    for descriptor in scope_chain {
        raw.push_str(descriptor);
    }
    LogosSymbol::parse(&raw)
}

/// A non-empty field value, or the fallback when empty (used for `scheme`).
fn field<'a>(value: &'a str, fallback: &'a str) -> &'a str {
    if value.is_empty() {
        fallback
    } else {
        value
    }
}

/// A package field value, or the SCIP empty-field marker `.` when empty.
fn dot(value: &str) -> &str {
    if value.is_empty() {
        "."
    } else {
        value
    }
}

/// Escape a descriptor name per the SCIP rule: a name made only of
/// `[A-Za-z0-9_+\-$]` is emitted verbatim; anything else is backtick-wrapped with
/// internal backticks doubled.
///
/// This mirrors the `scip` codec's own escaping so the assembled string parses
/// unambiguously (a path segment like `mod.rs` or `lib.rs` contains a `.`, which
/// is otherwise the term-descriptor suffix). Conservative over-escaping is safe:
/// [`LogosSymbol::parse`] round-trips the string through the codec's
/// parse→format, which strips any unnecessary escaping back to canonical form.
pub(crate) fn escape_name(name: &str) -> String {
    let simple = !name.is_empty()
        && name
            .chars()
            .all(|c| c == '_' || c == '+' || c == '-' || c == '$' || c.is_ascii_alphanumeric());
    if simple {
        name.to_string()
    } else {
        format!("`{}`", name.replace('`', "``"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_suffix_per_kind() {
        assert_eq!(descriptor_for(NodeKind::Module, "foo", 0), "foo/");
        assert_eq!(descriptor_for(NodeKind::Struct, "Foo", 0), "Foo#");
        assert_eq!(descriptor_for(NodeKind::Enum, "E", 0), "E#");
        assert_eq!(descriptor_for(NodeKind::Trait, "T", 0), "T#");
        assert_eq!(descriptor_for(NodeKind::TypeAlias, "A", 0), "A#");
        assert_eq!(descriptor_for(NodeKind::Function, "bar", 0), "bar().");
        assert_eq!(descriptor_for(NodeKind::Method, "m", 0), "m().");
        assert_eq!(descriptor_for(NodeKind::Constant, "C", 0), "C.");
        assert_eq!(descriptor_for(NodeKind::Variable, "S", 0), "S.");
        assert_eq!(descriptor_for(NodeKind::Macro, "mac", 0), "mac!");
        // The CR-003 documentation kinds (S-033): DocFile is namespace-like;
        // DocSection and the typed Requirement/Adr/Story take the `#`
        // type-descriptor so a section renders as the `…/path.md/slug#` anchor.
        // The DocSection arm is the identity primitive every doc node id rests on.
        assert_eq!(
            descriptor_for(NodeKind::DocFile, "guide.md", 0),
            "`guide.md`/"
        );
        assert_eq!(descriptor_for(NodeKind::DocSection, "setup", 0), "setup#");
        assert_eq!(descriptor_for(NodeKind::DocSection, "setup", 1), "setup#1:");
        assert_eq!(
            descriptor_for(NodeKind::Requirement, "fr-dg-01", 0),
            "fr-dg-01#"
        );
        assert_eq!(descriptor_for(NodeKind::Adr, "adr-19", 0), "adr-19#");
        assert_eq!(descriptor_for(NodeKind::Story, "s-033", 0), "s-033#");
    }

    /// The CR-010 config & artifact kinds (S-062): `ConfigFile` is namespace-like
    /// (the `name/` arm, built from the file path with an empty scope chain);
    /// `ConfigSection` and every typed anchor take the `#` type-descriptor so a
    /// section/anchor identity renders as the `…/path/anchor#` form (the [ADR-07]
    /// reuse, [FR-CG-02]/[FR-CG-03]). Pinning each arm protects the frozen
    /// identity contract the format stories (S-063+) bind against — a wrong suffix
    /// on any anchor would silently fork that format's anchor identities.
    #[test]
    fn config_kind_descriptor_suffixes() {
        assert_eq!(
            descriptor_for(NodeKind::ConfigFile, "values.yaml", 0),
            "`values.yaml`/",
            "ConfigFile is a namespace-like container"
        );
        // ConfigSection and the ten typed anchors all take the `#` type descriptor.
        for kind in [
            NodeKind::ConfigSection,
            NodeKind::ShellFunction,
            NodeKind::DockerfileStage,
            NodeKind::MakeTarget,
            NodeKind::ProtoMessage,
            NodeKind::ProtoService,
            NodeKind::GqlType,
            NodeKind::SqlObject,
            NodeKind::TfBlock,
            NodeKind::ApiPath,
            NodeKind::ApiOperation,
        ] {
            assert_eq!(
                descriptor_for(kind, "anchor", 0),
                "anchor#",
                "{} must take the `#` type-descriptor anchor",
                kind.as_str()
            );
            // Sibling-ordinal disambiguation rides the trailing meta, like every
            // other non-method kind.
            assert_eq!(
                descriptor_for(kind, "anchor", 2),
                "anchor#2:",
                "{} ordinal disambiguation",
                kind.as_str()
            );
        }
    }

    #[test]
    fn methods_disambiguate_via_the_native_slot() {
        // Methods/functions: the ordinal rides the native SCIP disambiguator slot.
        assert_eq!(descriptor_for(NodeKind::Function, "bar", 0), "bar().");
        assert_eq!(descriptor_for(NodeKind::Function, "bar", 1), "bar(1).");
        assert_eq!(descriptor_for(NodeKind::Method, "bar", 2), "bar(2).");
    }

    #[test]
    fn non_methods_disambiguate_via_a_trailing_meta() {
        // Ordinal 0 stays clean; a real collision (e.g. associated `const ALL`
        // in two impl blocks) appends a meta descriptor carrying the ordinal.
        assert_eq!(descriptor_for(NodeKind::Constant, "ALL", 0), "ALL.");
        assert_eq!(descriptor_for(NodeKind::Constant, "ALL", 1), "ALL.1:");
        assert_eq!(descriptor_for(NodeKind::Struct, "Foo", 0), "Foo#");
        assert_eq!(descriptor_for(NodeKind::Struct, "Foo", 2), "Foo#2:");
        assert_eq!(descriptor_for(NodeKind::Macro, "m", 1), "m!1:");
        // The framework type-like kinds share the type suffix + meta path.
        assert_eq!(descriptor_for(NodeKind::Route, "r", 1), "r#1:");
        assert_eq!(descriptor_for(NodeKind::Component, "C", 1), "C#1:");
    }

    #[test]
    fn escapes_path_segment_with_a_dot() {
        // `mod.rs` contains a `.` (the term suffix) so it must be backtick-wrapped.
        assert_eq!(escape_name("mod.rs"), "`mod.rs`");
        assert_eq!(escape_name("lib.rs"), "`lib.rs`");
        // Simple identifiers and the `-`/`_` cases stay verbatim.
        assert_eq!(escape_name("logos-core"), "logos-core");
        assert_eq!(escape_name("my_fn"), "my_fn");
    }

    #[test]
    fn builds_a_canonical_global_symbol() {
        let ctx = SymbolContext::cargo("logos-core", "0.1.0");
        let chain = vec![
            descriptor_for(NodeKind::Struct, "LogosSymbol", 0),
            descriptor_for(NodeKind::Function, "parse", 0),
        ];
        let sym = build_symbol(&ctx, &["src", "model", "mod.rs"], &chain).unwrap();
        // The path segment `mod.rs` is escaped; everything round-trips canonically.
        assert_eq!(
            sym.as_str(),
            "logos cargo logos-core 0.1.0 src/model/`mod.rs`/LogosSymbol#parse()."
        );
    }

    #[test]
    fn empty_package_fields_render_as_dot_markers() {
        let ctx = SymbolContext::default();
        let chain = vec![descriptor_for(NodeKind::Function, "main", 0)];
        let sym = build_symbol(&ctx, &["src", "main.rs"], &chain).unwrap();
        assert_eq!(sym.as_str(), "logos . . . src/`main.rs`/main().");
    }

    #[test]
    fn ordinal_disambiguated_methods_get_distinct_symbols() {
        let ctx = SymbolContext::cargo("c", "1");
        let zero = build_symbol(
            &ctx,
            &["a.rs"],
            &[descriptor_for(NodeKind::Function, "f", 0)],
        )
        .unwrap();
        let one = build_symbol(
            &ctx,
            &["a.rs"],
            &[descriptor_for(NodeKind::Function, "f", 1)],
        )
        .unwrap();
        assert_ne!(zero, one);
        assert!(one.as_str().contains("f(1)."));
    }

    #[test]
    fn rebuilt_symbol_is_stable() {
        // The same inputs always yield byte-identical symbols (determinism).
        let ctx = SymbolContext::cargo("logos-core", "0.1.0");
        let chain = || {
            vec![
                descriptor_for(NodeKind::Module, "extract", 0),
                descriptor_for(NodeKind::Function, "run", 0),
            ]
        };
        let a = build_symbol(&ctx, &["src", "lib.rs"], &chain()).unwrap();
        let b = build_symbol(&ctx, &["src", "lib.rs"], &chain()).unwrap();
        assert_eq!(a, b);
    }
}
