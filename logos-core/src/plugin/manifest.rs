//! The declarative `plugin.toml` descriptor ([FR-PL-02], [ADR-09]).
//!
//! A [`PluginManifest`] is the parsed form of one grammar's `plugin.toml`. It is
//! the *declarative* half of the plugin substrate: everything here tunes the
//! grammar's behaviour without touching `logos-core` source ([NFR-MA-01]).
//! Today the descriptor is parsed from the embedded asset; droppable on-disk
//! *query* overrides under `.logos/plugins/<name>/queries/` already take effect
//! without a rebuild ([FR-PL-04]), and shadowing this descriptor itself on disk
//! (tuning semantics like `complexity_keywords`, [NFR-MA-05]) is the next
//! increment.
//!
//! [FR-PL-02]: ../../../docs/specs/requirements/FR-PL-02.md
//! [FR-PL-04]: ../../../docs/specs/requirements/FR-PL-04.md
//! [NFR-MA-01]: ../../../docs/specs/requirements/NFR-MA-01.md
//! [NFR-MA-05]: ../../../docs/specs/requirements/NFR-MA-05.md
//! [ADR-09]: ../../../docs/specs/architecture/decisions/ADR-09.md

use serde::Deserialize;
use std::collections::BTreeMap;

use super::error::PluginError;
use crate::model::NodeKind;

/// How a language marks a declaration as exported/public — the declarative
/// rule behind the `exported` dead-code root flag (S-015, [FR-AN-01]).
///
/// Carried as descriptor data so a new language tunes export semantics without
/// touching `logos-core` ([NFR-MA-01]); the extraction engine interprets the
/// variant structurally (see `extract::shape::is_exported`).
///
/// [FR-AN-01]: ../../../docs/specs/requirements/FR-AN-01.md
/// [NFR-MA-01]: ../../../docs/specs/requirements/NFR-MA-01.md
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExportConvention {
    /// Every declaration is exported — the conservative default for a language
    /// with no declared convention: exported-is-live can only *under*-report
    /// dead code, never flag a live symbol dead (the safe direction, AR-05).
    #[default]
    All,
    /// A `visibility_modifier` child marks the declaration (Rust `pub`).
    VisibilityModifier,
    /// An `export_statement` ancestor marks the declaration (TS/JS `export`).
    ExportStatement,
    /// A leading-uppercase name marks the declaration (Go).
    Capitalized,
    /// A `public` token inside a `modifiers` child marks the declaration (Java).
    PublicModifier,
    /// Every name not starting with `_` is exported (Python convention).
    UnderscorePrivate,
    /// A file-scope declaration with **no** `static` storage-class specifier is
    /// externally visible (C, S-056). The inverse of a positive marker: absence
    /// of `static` is the export. The bounded new variant C's non-`static`
    /// export idiom needs ([CR-009] §4.1) — the same one-variant cost
    /// [`PublicModifier`] paid for Java.
    ///
    /// [CR-009]: ../../../docs/requests/CR-009-seven-language-plugins.md
    NonStatic,
    /// C# (S-057, [CR-009]): an explicit `public` access modifier among the
    /// declaration's flat `modifier` children. C# defaults types to `internal`
    /// and members to `private`, so an explicit `public` is the export root;
    /// stricter modifiers are non-roots (the conservative [AR-05] direction, as
    /// [`PublicModifier`](Self::PublicModifier)). Distinct from the Java variant
    /// only because tree-sitter-c-sharp emits flat `modifier` children rather
    /// than a wrapping `modifiers` node.
    ///
    /// [CR-009]: ../../../docs/requests/CR-009-seven-language-plugins.md
    ExplicitModifier,
    /// C++ (S-058): a declaration has external linkage — exported — unless it is
    /// `static`-qualified or lexically nested in an *anonymous* namespace
    /// (`namespace { … }`). Both are the language's own "internal linkage"
    /// markers, so this reads the conservative exported-is-live direction
    /// (under-reports dead code, never flags a live symbol dead — [AR-05]). The
    /// bounded core variant C++'s idiom requires (the same shape Java's
    /// `PublicModifier` landed in S-015), not derivable from any existing one:
    /// C++ default visibility is external, the inverse of Java's default-private.
    CppExternalLinkage,
    /// PHP: **public-by-default** — a declaration is exported unless a direct
    /// `visibility_modifier` child carries the `private`/`protected` keyword
    /// (S-060, [CR-009]). Top-level `function`s and modifier-less class members
    /// are public and therefore exported; only an explicit `private`/`protected`
    /// demotes one. This is the inverse of [`ExportConvention::PublicModifier`]
    /// (Java requires a `public` token) and stricter than
    /// [`ExportConvention::VisibilityModifier`] (Rust treats any visibility node
    /// as exported, which would wrongly promote a `private` PHP method).
    ///
    /// [CR-009]: ../../../docs/requests/CR-009-seven-language-plugins.md
    PhpVisibility,
    /// Public-by-default: a declaration is exported **unless** a `modifiers`
    /// child carries an `access_modifier` (Scala `private`/`protected`) — the
    /// inverse of [`PublicModifier`](ExportConvention::PublicModifier). Scala has
    /// no `public` keyword; visibility is public until narrowed, so absence of an
    /// access modifier is the export signal (S-061, [FR-AN-01]). A package-scoped
    /// `private[pkg]` still parses as an `access_modifier`, so it is correctly a
    /// non-root — conservative in the dead-code-safe direction (AR-05).
    PublicDefault,
}

/// How a language marks a function as a test — the declarative rule behind the
/// extraction-time test-marker evidence flag ([FR-EX-06], [ADR-18], [CR-001]).
///
/// Carried as descriptor data so a language reusing an existing idiom tunes
/// test detection without touching `logos-core` ([NFR-MA-01]); the extraction
/// engine interprets the variant structurally (see `extract::testmarker`). A
/// genuinely new idiom costs one new variant — the same bounded core change as
/// [`ExportConvention`].
///
/// The default is [`TestConvention::None`]: a descriptor that declares nothing
/// emits no evidence and still indexes normally (absence ≠ error — [FR-EX-06],
/// [NFR-MA-01]). Conservative is the *safe* direction here: a false-positive
/// test marker would silently exempt production code from the quality gate
/// ([ADR-18]), so classification is positive-evidence-only and never inferred.
///
/// [FR-EX-06]: ../../../docs/specs/requirements/FR-EX-06.md
/// [NFR-MA-01]: ../../../docs/specs/requirements/NFR-MA-01.md
/// [ADR-18]: ../../../docs/specs/architecture/decisions/ADR-18.md
/// [CR-001]: ../../../docs/requests/CR-001-test-aware-quality-metrics.md
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TestConvention {
    /// No extraction-time test-marker detection — the conservative default.
    #[default]
    None,
    /// Rust: a `#[test]`-family attribute on the function (`#[test]`,
    /// `#[tokio::test]`, …), or containment in a `#[cfg(test)]` module.
    RustAttributes,
    /// Python: a `test_*`-named function, or a `test`-prefixed method of a
    /// `unittest.TestCase` subclass.
    PythonTest,
    /// TS/JS: a function lexically enclosed by an `it`/`test`/`describe` call.
    JsCallback,
    /// Go: a `Test`/`Benchmark`/`Fuzz` function defined in a `*_test.go` file.
    GoTestFunc,
    /// Java: a `@Test`-family annotation on the method (`@Test`,
    /// `@ParameterizedTest`, `@RepeatedTest`, …).
    JavaAnnotations,
    /// Kotlin: a `@Test`-family annotation on the function (`@Test`,
    /// `@ParameterizedTest`, `@RepeatedTest`, …) covering both JUnit and
    /// `kotlin.test` (S-055, [CR-009]). A distinct variant from
    /// [`JavaAnnotations`](Self::JavaAnnotations) because Kotlin's grammar
    /// models an annotation as `annotation → user_type`/`constructor_invocation`
    /// with **no `name` field**, so the Java reader cannot detect it.
    ///
    /// [CR-009]: ../../../docs/requests/CR-009-seven-language-plugins.md
    KotlinAnnotations,
    /// C# (S-057, [CR-009]): a test-attribute in an `attribute_list` on the
    /// method — `[Fact]`/`[Theory]` (xUnit), `[Test]` (NUnit), `[TestMethod]`
    /// (MSTest). The bounded core change [FR-EX-06] anticipates for a genuinely
    /// new idiom (the same shape as [`JavaAnnotations`]); the C# `.cs` plugin is
    /// otherwise pure descriptor data ([NFR-MA-01]).
    ///
    /// [CR-009]: ../../../docs/requests/CR-009-seven-language-plugins.md
    CSharpAttributes,
    /// C++ (S-058): a GoogleTest function-like test macro — a function whose
    /// name is `TEST`/`TEST_F`/`TEST_P`/`TYPED_TEST`/`TYPED_TEST_P` (the
    /// `TEST(Suite, Name) { … }` form parses as a return-type-less
    /// `function_definition` named for the macro). The Catch2 macros
    /// (`TEST_CASE`/`SCENARIO`) are recognised too, but their string-argument
    /// form (`TEST_CASE("name", "[tag]")`) does not parse as a function under
    /// `tree-sitter-cpp`, so no function node exists to mark — the measured
    /// precision floor, surfaced honestly rather than fabricated ([NFR-RA-05]).
    ///
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    CppTestMacros,
    /// Ruby: a minitest `test_*`-named method, or a method/example lexically
    /// enclosed by (or being) an RSpec `it`/`describe`/`context` block call
    /// (S-059, [CR-009]). The dual idiom is why Ruby needs its own variant:
    /// no single existing convention covers both minitest naming and RSpec's
    /// block-callback enclosure.
    RubyTest,
    /// PHP/PHPUnit: a method marked a test by any of the three coexisting
    /// idioms (S-060, [CR-009]) — a `test`-prefixed method name (`testFoo`), a
    /// PHP 8 `#[Test]` attribute, or a `@test` tag in the method's preceding
    /// docblock comment. Positive-evidence-only like every other convention: a
    /// helper without any of the three markers carries no evidence ([ADR-18]).
    ///
    /// [CR-009]: ../../../docs/requests/CR-009-seven-language-plugins.md
    PhpUnit,
    /// Scala: a callable that is, or is lexically enclosed by, a `test(…)` /
    /// `it(…)` marker call — the munit / ScalaTest (`FunSuite`/`FunSpec`) idiom,
    /// where a test case is a `test("name") { … }` *call* rather than a
    /// declaration (S-061, [FR-EX-06], [ADR-18]). Best-effort and
    /// positive-evidence-only: only the `test`/`it` marker names are recognised,
    /// so a production call enclosing a helper is never misread as a test.
    ScalaTest,
}

/// The parsed `plugin.toml` descriptor for one grammar.
///
/// `#[serde(deny_unknown_fields)]` makes a typo in a descriptor a loud,
/// file-naming parse error rather than a silently ignored key — consistent with
/// the fail-fast posture of [FR-PL-02].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginManifest {
    /// Display + lookup name; also the `.logos/plugins/<name>/` override dir.
    pub name: String,
    /// File extensions claimed (without the leading dot), e.g. `["rs"]`.
    pub extensions: Vec<String>,
    /// Module path separator joining symbol segments (`::`, `.`, `/`).
    pub module_separator: String,
    /// The tree-sitter ABI version this grammar was generated against. Asserted
    /// against the compiled grammar at load ([FR-PL-02], `abi::assert_abi`).
    pub abi_version: usize,
    /// Extraction capabilities this descriptor provides queries for.
    pub capabilities: Vec<String>,
    /// Keywords that increment cyclomatic complexity for this language.
    /// Defaults to empty when omitted so descriptors that do not tune
    /// complexity stay terse.
    #[serde(default)]
    pub complexity_keywords: Vec<String>,
    /// Tree-sitter node kinds that introduce one level of block-structure
    /// nesting for this language — the declarative input to per-function
    /// maximum nesting depth (CR-005, [FR-EX-07]), the same descriptor pattern
    /// as [`complexity_keywords`](Self::complexity_keywords) ([FR-PL-02],
    /// [ADR-09]). Each is a compound/control node kind (`if_expression`,
    /// `for_statement`, …); depth 0 is a flat body and every nested such node
    /// increments by one. Defaults to empty when omitted, so a descriptor that
    /// does not declare nesting simply reports depth 0 ([NFR-MA-01]).
    ///
    /// [FR-EX-07]: ../../../docs/specs/requirements/FR-EX-07.md
    /// [FR-PL-02]: ../../../docs/specs/requirements/FR-PL-02.md
    /// [ADR-09]: ../../../docs/specs/architecture/decisions/ADR-09.md
    /// [NFR-MA-01]: ../../../docs/specs/requirements/NFR-MA-01.md
    #[serde(default)]
    pub nesting_block_kinds: Vec<String>,
    /// Capability → relative `.scm` query path (resolved against the descriptor
    /// directory). Defaults to empty when the `[queries]` table is omitted.
    #[serde(default)]
    pub queries: BTreeMap<String, String>,
    /// Canonical (`::`-joined) reference-path prefixes whose presence in a
    /// file's ledger makes the file a framework candidate (S-015, [FR-FW-04]
    /// ledger-gated candidacy), e.g. `["axum", "actix_web"]` or
    /// `["org::springframework"]`. Empty = this language promotes nothing.
    ///
    /// [FR-FW-04]: ../../../docs/specs/requirements/FR-FW-04.md
    #[serde(default)]
    pub framework_detectors: Vec<String>,
    /// Captured `@fw.route.method` text → upper-cased HTTP method (`"GET"`, …,
    /// `"ANY"`) for the declarative framework-query contract (S-015,
    /// [FR-FW-01]). A captured method text with no entry here is dropped — the
    /// table doubles as the recognised-registration filter.
    ///
    /// [FR-FW-01]: ../../../docs/specs/requirements/FR-FW-01.md
    #[serde(default)]
    pub framework_methods: BTreeMap<String, String>,
    /// How this language marks a declaration exported ([`ExportConvention`]).
    /// Defaults to [`ExportConvention::All`] when omitted.
    #[serde(default)]
    pub export_convention: ExportConvention,
    /// How this language marks a function as a test ([`TestConvention`]).
    /// Defaults to [`TestConvention::None`] when omitted — a plugin without
    /// test detection still indexes normally ([FR-EX-06], [NFR-MA-01]).
    ///
    /// [FR-EX-06]: ../../../docs/specs/requirements/FR-EX-06.md
    /// [NFR-MA-01]: ../../../docs/specs/requirements/NFR-MA-01.md
    #[serde(default)]
    pub test_convention: TestConvention,
    /// Whether this language declares the optional **reachability capability**
    /// (S-159, [CR-043], [ADR-39]) — the signal the [annotation-engine] Pass-3
    /// dead-code detector gates on ([FR-AN-01]). `true` asserts the language's
    /// binder coverage ([FR-RS-03]) is proven well enough that reachability over
    /// its bound `Calls`/`RoutesTo` edges is a trustworthy dead-code signal;
    /// `false` (the default) makes every callable in the language render
    /// `is_dead = NULL` ("not computed", [NFR-CC-04]) instead of a fabricated
    /// verdict. The same optional-capability pattern as
    /// [`test_convention`](Self::test_convention): a descriptor that omits it
    /// still indexes normally ([NFR-MA-01]). Initially only Rust declares it;
    /// other languages opt in as their binder coverage is proven on the dogfood.
    ///
    /// [CR-043]: ../../../docs/requests/CR-043-dead-code-detector-precision.md
    /// [ADR-39]: ../../../docs/specs/architecture/decisions/ADR-39.md
    /// [FR-AN-01]: ../../../docs/specs/requirements/FR-AN-01.md
    /// [FR-RS-03]: ../../../docs/specs/requirements/FR-RS-03.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    /// [NFR-MA-01]: ../../../docs/specs/requirements/NFR-MA-01.md
    #[serde(default)]
    pub reachability: bool,
    /// Whether this grammar is a **documentation** plugin (S-033, [CR-003],
    /// [ADR-19]). `false` (the default) marks a code grammar extracted via the
    /// `symbols`/`references` tagging queries; `true` marks a markdown-style
    /// documentation grammar extracted structurally into `DocFile`/`DocSection`
    /// nodes by `extract::doc`, with a `path#heading-slug` identity rather than
    /// SCIP ([FR-DG-02]). Defaulting to `false` keeps every existing code
    /// descriptor valid and unchanged ([NFR-MA-01]).
    ///
    /// [CR-003]: ../../../docs/requests/CR-003-documentation-graph-layer.md
    /// [ADR-19]: ../../../docs/specs/architecture/decisions/ADR-19.md
    /// [FR-DG-02]: ../../../docs/specs/requirements/FR-DG-02.md
    #[serde(default)]
    pub documentation: bool,
    /// Whether this grammar is a **config/artifact** plugin (S-062, [CR-010],
    /// [ADR-25]) — the third plugin class beside code and documentation, exactly
    /// parallel to [`documentation`](Self::documentation). `true` routes a matched
    /// file structurally through `extract::config` into a `ConfigFile` + nested
    /// `ConfigSection` tree (plus any per-format typed anchors), not the code
    /// `symbols`/`references` path. Mutually exclusive with `documentation`.
    /// Defaulting to `false` keeps every existing descriptor valid ([NFR-MA-01]).
    ///
    /// [CR-010]: ../../../docs/requests/CR-010-config-artifact-graph-layer.md
    /// [ADR-25]: ../../../docs/specs/architecture/decisions/ADR-25.md
    #[serde(default)]
    pub artifact: bool,
    /// Optional **basename** claims (S-062, [CR-010], [FR-CG-01]): file names with
    /// no useful extension that this plugin claims, e.g. `["Dockerfile"]` or
    /// `["Makefile", "makefile", "GNUmakefile"]`. The registry indexes these
    /// beside extensions; discovery admits a file when its extension **or**
    /// basename is claimed. The match rule is exact **plus** a documented `Name.*`
    /// prefix (so `Dockerfile.dev` binds to the `Dockerfile` plugin). Defaults to
    /// empty — any code/doc descriptor that omits it is unaffected.
    ///
    /// [FR-CG-01]: ../../../docs/specs/requirements/FR-CG-01.md
    #[serde(default)]
    pub filenames: Vec<String>,
    /// The structural config-extraction descriptor (S-062, [CR-010], [FR-CG-02]):
    /// the tree-sitter node kinds the generic `ConfigSection` walk treats as
    /// sections, and how to read each section's key. Meaningful only when
    /// [`artifact`](Self::artifact) is `true`; absent for a plain `ConfigFile`-only
    /// artifact or a typed-anchor-only format. See [`ConfigDescriptor`].
    ///
    /// [FR-CG-02]: ../../../docs/specs/requirements/FR-CG-02.md
    #[serde(default)]
    pub config: Option<ConfigDescriptor>,
}

/// The `[config]` descriptor sub-table for an artifact-class plugin (S-062,
/// [CR-010], [FR-CG-02]).
///
/// Drives the **generic**, depth-bounded `ConfigSection` walk in `extract::config`
/// declaratively — the same descriptor-data pattern as
/// [`nesting_block_kinds`](PluginManifest::nesting_block_kinds): the core walk is
/// grammar-agnostic, and each data format (YAML/JSON/TOML, S-063) supplies only
/// the node kinds its grammar uses. A format with no nested-section concept
/// (Dockerfile, Protobuf, …) omits the `[config]` table entirely and emits its
/// typed anchors through its own per-format walk.
///
/// [CR-010]: ../../../docs/requests/CR-010-config-artifact-graph-layer.md
/// [FR-CG-02]: ../../../docs/specs/requirements/FR-CG-02.md
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigDescriptor {
    /// Tree-sitter node kinds that introduce one `ConfigSection` level (a
    /// key→value mapping pair / block), e.g. `["block_mapping_pair"]` for YAML.
    /// Each such node becomes a `ConfigSection`; nesting is bounded at the fixed
    /// depth of 2 ([BR-30]) regardless of file size. Defaults to empty when
    /// omitted, so a **typed-anchor-only** format (Protobuf, GraphQL, S-065)
    /// declares a `[config]` table with only `[[config.anchors]]` and no nested-
    /// section concept.
    #[serde(default)]
    pub section_kinds: Vec<String>,
    /// Optional tree-sitter **field name** whose child text is the section's key
    /// (the human-facing `name`, FTS-indexed and the `#anchor` slug source), e.g.
    /// `"key"` for a YAML `block_mapping_pair`. When absent or the field is
    /// missing on a node, the walk falls back to the section node's first source
    /// line — so a key is always derivable, never fabricated.
    #[serde(default)]
    pub key_field: Option<String>,
    /// The [`NodeKind`] the generic walk emits for each matched section. Absent
    /// means [`NodeKind::ConfigSection`] — the generic data-format anchor
    /// (YAML/JSON/TOML, S-063). A build format (S-064) names a single **typed
    /// anchor** kind here — `"dockerfile_stage"`, `"make_target"`,
    /// `"shell_function"` — so the *same* descriptor-driven section walk emits its
    /// typed nodes as pure plugin data, with no per-format core code. Validation
    /// requires a config kind ([`NodeKind::is_config`]) so the emitted node is
    /// always metric-neutral ([FR-CG-05]). Formats needing several distinct anchor
    /// kinds per file (Protobuf/GraphQL, S-065) use [`anchors`](Self::anchors)
    /// instead. (Unifying the two mechanisms is a Sprint-Review item.)
    ///
    /// [FR-CG-03]: ../../../docs/specs/requirements/FR-CG-03.md
    /// [FR-CG-05]: ../../../docs/specs/requirements/FR-CG-05.md
    #[serde(default)]
    pub node_kind: Option<NodeKind>,
    /// Declarative **typed-anchor** table (S-065, [CR-010], [FR-CG-03]): the
    /// per-format structural anchors the generic anchor walk in `extract::config`
    /// emits, the same descriptor-data pattern as [`section_kinds`](Self::section_kinds).
    /// Empty for a generic-section-only format (YAML/JSON/TOML). Each entry maps a
    /// tree-sitter node kind to a config [`NodeKind`](crate::model::NodeKind) with an
    /// optional name-bearing child and payload subtype. See [`AnchorDescriptor`].
    ///
    /// [FR-CG-03]: ../../../docs/specs/requirements/FR-CG-03.md
    #[serde(default)]
    pub anchors: Vec<AnchorDescriptor>,
}

/// One declarative **typed-anchor** mapping in a `[[config.anchors]]` table
/// (S-065, [CR-010], [FR-CG-03]).
///
/// Drives the generic, descriptor-driven anchor walk in `extract::config`: a node
/// whose tree-sitter kind equals [`node_kind`](Self::node_kind) is emitted as a
/// config node of [`kind`](Self::kind), named from the first child of kind
/// [`name_child`](Self::name_child) (falling back to the node's first source line
/// so a name is never fabricated), carrying any [`payload`](Self::payload) subtype.
/// The emitted node hangs off the file's `ConfigFile` root by a `Contains` edge —
/// the layer is `Contains`-only ([CR-010] scope rule; reference edges are [CR-011]).
/// This is the substrate mechanism that lets a typed-anchor format (Protobuf,
/// GraphQL, and the sibling build/infra formats) ship as pure plugin data
/// ([NFR-MA-01]): the walk is grammar-agnostic, the mappings are descriptor data.
///
/// [CR-010]: ../../../docs/requests/CR-010-config-artifact-graph-layer.md
/// [CR-011]: ../../../docs/requests/CR-011-cross-artifact-resolution.md
/// [FR-CG-03]: ../../../docs/specs/requirements/FR-CG-03.md
/// [NFR-MA-01]: ../../../docs/specs/requirements/NFR-MA-01.md
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnchorDescriptor {
    /// The tree-sitter node kind that introduces this anchor, e.g. `"message"`
    /// (Protobuf) or `"object_type_definition"` (GraphQL).
    pub node_kind: String,
    /// The config [`NodeKind`](crate::model::NodeKind) wire name to emit, e.g.
    /// `"proto_message"` or `"gql_type"`. Validated at descriptor parse to be a
    /// known config kind (never `config_file`), so a typo fails loudly.
    pub kind: String,
    /// Optional tree-sitter **child node kind** whose text is the anchor's
    /// declared name (the FTS-indexed `name` and the `#anchor` slug source), e.g.
    /// `"message_name"` (Protobuf) or `"name"` (GraphQL). When absent or missing on
    /// a node, the walk falls back to the node's first source line.
    #[serde(default)]
    pub name_child: Option<String>,
    /// Optional payload subtype recorded on the emitted node (FR-CG-03's payload
    /// column capping discriminant growth), e.g. `"object"`/`"interface"` for a
    /// GraphQL `gql_type`. `None` for formats with no subtypes (Protobuf).
    #[serde(default)]
    pub payload: Option<String>,
}

impl PluginManifest {
    /// Parse a descriptor from TOML text, attributing any error to `file`.
    ///
    /// `file` is the embedded asset name or the on-disk override path; it is
    /// threaded into the error so a malformed descriptor names itself
    /// ([FR-PL-02]).
    ///
    /// # Errors
    /// Returns [`PluginError::Manifest`] on a TOML syntax error, an unknown
    /// field, or a semantic violation (empty name / no extensions / a
    /// capability with no backing query path).
    pub fn parse(file: &str, text: &str) -> Result<Self, PluginError> {
        let manifest: PluginManifest = toml::from_str(text).map_err(|e| PluginError::Manifest {
            file: file.to_string(),
            detail: e.message().to_string(),
        })?;
        manifest.validate(file)?;
        Ok(manifest)
    }

    /// Semantic validation beyond what the type system enforces.
    fn validate(&self, file: &str) -> Result<(), PluginError> {
        let bail = |detail: String| {
            Err(PluginError::Manifest {
                file: file.to_string(),
                detail,
            })
        };

        if self.name.trim().is_empty() {
            return bail("`name` must not be empty".to_string());
        }
        // `name` is used as a path component (`.logos/plugins/<name>/`) when
        // resolving on-disk overrides, so it must never contain a path
        // separator or a parent-dir component — defense-in-depth against a
        // future descriptor whose name is read from disk (NFR-SE-04, FR-PL-02).
        if self.name.contains('/') || self.name.contains('\\') || self.name.contains("..") {
            return bail("`name` must not contain a path separator or '..'".to_string());
        }
        // A plugin must claim *something*: at least one extension, or — for an
        // extensionless artifact like `Dockerfile`/`Makefile` ([CR-010],
        // [FR-CG-01]) — at least one basename. Code/doc descriptors that predate
        // CR-010 carry no `filenames`, so this stays an extension requirement for
        // them (their `filenames` defaults empty).
        if self.extensions.is_empty() && self.filenames.is_empty() {
            return bail(
                "must claim at least one `extensions` entry or one `filenames` basename"
                    .to_string(),
            );
        }
        if self.extensions.iter().any(|e| e.starts_with('.')) {
            return bail(
                "extensions must omit the leading dot (use \"rs\", not \".rs\")".to_string(),
            );
        }
        // A descriptor is exactly one class: code, documentation, **or** artifact.
        // Both flags set is a descriptor bug ([CR-010] third class beside, not
        // overlapping, the documentation class).
        if self.documentation && self.artifact {
            return bail(
                "`documentation` and `artifact` are mutually exclusive plugin classes".to_string(),
            );
        }
        // A basename claim is a file name, never a path: it must not contain a
        // separator (it is matched against the file's basename, [FR-CG-01]).
        for fname in &self.filenames {
            if fname.trim().is_empty() {
                return bail("`filenames` entries must not be empty".to_string());
            }
            if fname.contains('/') || fname.contains('\\') {
                return bail(format!(
                    "filename claim '{fname}' must be a basename, not a path"
                ));
            }
        }
        // The `[config]` extraction table only makes sense for an artifact plugin
        // — it drives `extract::config`, which a non-artifact file never reaches.
        if self.config.is_some() && !self.artifact {
            return bail("`[config]` is only valid when `artifact = true`".to_string());
        }
        if let Some(cfg) = &self.config {
            // A `node_kind` override (S-064) must name a config/artifact kind: the
            // generic section walk emits it for every matched section, and only
            // config kinds are `is_non_code` and therefore metric-neutral
            // ([FR-CG-05]) — naming a code kind would smuggle a node into the metric
            // graph. It is also meaningless without `section_kinds` to match.
            // Checked before the umbrella below so the pairing gets its specific
            // diagnostic rather than the generic "must declare something" message.
            if let Some(nk) = cfg.node_kind {
                if !nk.is_config() {
                    return bail(format!(
                        "[config] node_kind '{}' must be a config/artifact kind",
                        nk.as_str()
                    ));
                }
                if cfg.section_kinds.is_empty() {
                    return bail(
                        "[config] node_kind requires at least one `section_kinds` entry to match"
                            .to_string(),
                    );
                }
            }
            // A `[config]` table must drive *something*: either the generic section
            // walk (`section_kinds`) or at least one typed anchor (`[[config.anchors]]`).
            // An empty table is a descriptor bug — fail loudly per the fail-fast posture.
            if cfg.section_kinds.is_empty() && cfg.anchors.is_empty() {
                return bail(
                    "`[config]` must declare `section_kinds` or at least one `[[config.anchors]]`"
                        .to_string(),
                );
            }
            // Each anchor maps a tree-sitter node kind to a *typed* config kind.
            // The `kind` must resolve to a config/artifact kind that is neither the
            // generic `ConfigFile` root nor `ConfigSection` (both are emitted only
            // by the generic file/section walk, never as anchors) — so a typo or a
            // wrong kind is a loud parse error, not a silently dropped or
            // structurally inconsistent anchor (S-065, [CR-010], [FR-CG-03]). A
            // duplicate `node_kind` is likewise rejected: the anchor walk keeps the
            // first match per node kind, so a duplicate would silently drop a
            // mapping — fail loud instead.
            let mut seen_node_kinds: Vec<&str> = Vec::new();
            for anchor in &cfg.anchors {
                let node_kind = anchor.node_kind.trim();
                if node_kind.is_empty() {
                    return bail("`[[config.anchors]]` `node_kind` must not be empty".to_string());
                }
                if seen_node_kinds.contains(&node_kind) {
                    return bail(format!(
                        "duplicate `[[config.anchors]]` `node_kind` '{node_kind}'"
                    ));
                }
                seen_node_kinds.push(node_kind);
                match NodeKind::from_wire(&anchor.kind) {
                    Some(k)
                        if k.is_config()
                            && k != NodeKind::ConfigFile
                            && k != NodeKind::ConfigSection => {}
                    Some(_) => {
                        return bail(format!(
                            "anchor kind '{}' is not a typed config/artifact node kind",
                            anchor.kind
                        ))
                    }
                    None => {
                        return bail(format!(
                            "anchor kind '{}' is not a known node kind",
                            anchor.kind
                        ))
                    }
                }
            }
        }
        if self.module_separator.is_empty() {
            return bail("`module_separator` must not be empty".to_string());
        }
        // Every declared capability must have a query backing it, so a `logos
        // languages` capability claim can never be a query the engine cannot
        // run.
        for cap in &self.capabilities {
            if !self.queries.contains_key(cap) {
                return bail(format!("capability '{cap}' has no entry in [queries]"));
            }
        }
        // An empty detector would prefix-match every reference and turn every
        // file into a framework candidate — a descriptor bug worth failing on.
        if self.framework_detectors.iter().any(|d| d.trim().is_empty()) {
            return bail("`framework_detectors` entries must not be empty".to_string());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"
        name = "rust"
        extensions = ["rs"]
        module_separator = "::"
        abi_version = 15
        capabilities = ["symbols"]
        complexity_keywords = ["if", "match"]
        [queries]
        symbols = "queries/symbols.scm"
    "#;

    #[test]
    fn parses_a_well_formed_descriptor() {
        let m = PluginManifest::parse("rust/plugin.toml", GOOD).unwrap();
        assert_eq!(m.name, "rust");
        assert_eq!(m.extensions, ["rs"]);
        assert_eq!(m.module_separator, "::");
        assert_eq!(m.abi_version, 15);
        assert_eq!(m.capabilities, ["symbols"]);
        assert_eq!(m.complexity_keywords, ["if", "match"]);
        // A descriptor that does not declare nesting block kinds defaults to
        // empty — every pre-CR-005 descriptor stays valid (NFR-MA-01).
        assert!(m.nesting_block_kinds.is_empty());
        assert_eq!(m.queries.get("symbols").unwrap(), "queries/symbols.scm");
        // The S-015 framework/export fields default to empty/All when omitted,
        // so pre-existing descriptors keep parsing unchanged (NFR-MA-01).
        assert!(m.framework_detectors.is_empty());
        assert!(m.framework_methods.is_empty());
        assert_eq!(m.export_convention, ExportConvention::All);
        // A descriptor that declares no test idiom defaults to None — the
        // optional-evidence contract (FR-EX-06, NFR-MA-01).
        assert_eq!(m.test_convention, TestConvention::None);
        // The CR-043 reachability capability defaults to off — a descriptor that
        // omits it renders `is_dead = NULL` and still indexes (NFR-MA-01, S-159).
        assert!(!m.reachability);
        // A descriptor that does not declare `documentation` is a code grammar
        // (the default) — every pre-CR-003 descriptor stays valid (NFR-MA-01).
        assert!(!m.documentation);
        // Likewise the CR-010 artifact-class fields default to off/empty, so a
        // pre-CR-010 descriptor is unchanged (NFR-MA-01).
        assert!(!m.artifact);
        assert!(m.filenames.is_empty());
        assert!(m.config.is_none());
    }

    #[test]
    fn parses_an_artifact_descriptor_with_filenames_and_config() {
        // A YAML-style data-format artifact descriptor (S-062/S-063, CR-010):
        // artifact = true, basename claims, and a `[config]` extraction table.
        let toml = r#"
            name = "yaml"
            extensions = ["yml", "yaml"]
            module_separator = "/"
            abi_version = 14
            capabilities = []
            artifact = true
            filenames = ["Dockerfile"]
            [config]
            section_kinds = ["block_mapping_pair"]
            key_field = "key"
        "#;
        let m = PluginManifest::parse("yaml/plugin.toml", toml).unwrap();
        assert!(m.artifact, "the descriptor is an artifact plugin");
        assert!(!m.documentation, "artifact is not the documentation class");
        assert_eq!(m.filenames, ["Dockerfile"]);
        let config = m.config.expect("the [config] table parsed");
        assert_eq!(config.section_kinds, ["block_mapping_pair"]);
        assert_eq!(config.key_field.as_deref(), Some("key"));
        assert_eq!(
            config.node_kind, None,
            "a data-format descriptor leaves node_kind defaulting to ConfigSection"
        );
    }

    #[test]
    fn parses_a_typed_anchor_node_kind_override() {
        // A build-format descriptor (S-064): `node_kind` names a typed anchor so
        // the generic walk emits `DockerfileStage` instead of `ConfigSection`.
        let toml = r#"
            name = "dockerfile"
            extensions = ["dockerfile"]
            module_separator = "/"
            abi_version = 15
            capabilities = []
            artifact = true
            filenames = ["Dockerfile"]
            [config]
            section_kinds = ["from_instruction"]
            key_field = "as"
            node_kind = "dockerfile_stage"
        "#;
        let m = PluginManifest::parse("dockerfile/plugin.toml", toml).unwrap();
        let config = m.config.expect("the [config] table parsed");
        assert_eq!(config.node_kind, Some(NodeKind::DockerfileStage));
    }

    #[test]
    fn a_non_config_node_kind_is_rejected() {
        // A `node_kind` naming a *code* kind would smuggle a node into the metric
        // graph (it is not `is_non_code`) — the descriptor must be rejected
        // ([FR-CG-05] metric-neutrality).
        let toml = r#"
            name = "x"
            extensions = ["x"]
            module_separator = "/"
            abi_version = 15
            capabilities = []
            artifact = true
            [config]
            section_kinds = ["thing"]
            node_kind = "function"
        "#;
        let err = PluginManifest::parse("x/plugin.toml", toml).unwrap_err();
        assert!(
            err.to_string().contains("must be a config/artifact kind"),
            "got: {err}"
        );
    }

    #[test]
    fn a_node_kind_without_section_kinds_is_rejected() {
        // `node_kind` only takes effect through the generic section walk, which
        // never fires when `section_kinds` is empty — so the pairing is a silent
        // no-op the validator must reject rather than accept quietly.
        let toml = r#"
            name = "x"
            extensions = ["x"]
            module_separator = "/"
            abi_version = 15
            capabilities = []
            artifact = true
            [config]
            section_kinds = []
            node_kind = "make_target"
        "#;
        let err = PluginManifest::parse("x/plugin.toml", toml).unwrap_err();
        assert!(
            err.to_string()
                .contains("requires at least one `section_kinds`"),
            "got: {err}"
        );
    }

    #[test]
    fn parses_an_extensionless_artifact_descriptor() {
        // Dockerfile claims only basenames + a `.dockerfile` extension; Makefile
        // proves a descriptor can be admitted on basenames with no nested-section
        // `[config]` table (typed-anchor-only formats, S-064).
        let toml = r#"
            name = "makefile"
            extensions = ["mk"]
            module_separator = "/"
            abi_version = 14
            capabilities = []
            artifact = true
            filenames = ["Makefile", "makefile", "GNUmakefile"]
        "#;
        let m = PluginManifest::parse("makefile/plugin.toml", toml).unwrap();
        assert!(m.artifact);
        assert_eq!(m.filenames, ["Makefile", "makefile", "GNUmakefile"]);
        assert!(
            m.config.is_none(),
            "no [config] table is valid for an artifact"
        );
    }

    #[test]
    fn documentation_and_artifact_together_is_rejected() {
        let toml = r#"
            name = "x"
            extensions = ["x"]
            module_separator = "/"
            abi_version = 14
            capabilities = []
            documentation = true
            artifact = true
        "#;
        let err = PluginManifest::parse("x/plugin.toml", toml).unwrap_err();
        assert!(err.to_string().contains("mutually exclusive"));
    }

    #[test]
    fn a_filename_only_artifact_with_no_extensions_is_accepted() {
        // The extensionless-format case ([CR-010], FR-CG-01): empty `extensions`
        // is admitted when `filenames` claims at least one basename.
        let toml = r#"
            name = "dockerfile"
            extensions = []
            module_separator = "/"
            abi_version = 14
            capabilities = []
            artifact = true
            filenames = ["Dockerfile"]
        "#;
        let m = PluginManifest::parse("dockerfile/plugin.toml", toml).unwrap();
        assert!(m.extensions.is_empty());
        assert_eq!(m.filenames, ["Dockerfile"]);
    }

    #[test]
    fn a_filename_with_a_path_separator_is_rejected() {
        let toml = r#"
            name = "x"
            extensions = ["x"]
            module_separator = "/"
            abi_version = 14
            capabilities = []
            artifact = true
            filenames = ["sub/Dockerfile"]
        "#;
        let err = PluginManifest::parse("x/plugin.toml", toml).unwrap_err();
        assert!(err.to_string().contains("basename"));
    }

    #[test]
    fn a_config_table_without_artifact_is_rejected() {
        let toml = r#"
            name = "x"
            extensions = ["x"]
            module_separator = "/"
            abi_version = 14
            capabilities = []
            [config]
            section_kinds = ["pair"]
        "#;
        let err = PluginManifest::parse("x/plugin.toml", toml).unwrap_err();
        assert!(err.to_string().contains("[config]"));
    }

    #[test]
    fn parses_a_typed_anchor_descriptor() {
        // A schema-format artifact (S-065): `[[config.anchors]]` with no
        // `section_kinds`, mapping a node kind to a typed config NodeKind with an
        // optional name-child and payload subtype.
        let toml = r#"
            name = "graphql"
            extensions = ["graphql", "gql"]
            module_separator = "/"
            abi_version = 15
            capabilities = []
            artifact = true
            [[config.anchors]]
            node_kind = "object_type_definition"
            kind = "gql_type"
            name_child = "name"
            payload = "object"
            [[config.anchors]]
            node_kind = "scalar_type_definition"
            kind = "gql_type"
            name_child = "name"
            payload = "scalar"
        "#;
        let m = PluginManifest::parse("graphql/plugin.toml", toml).unwrap();
        let cfg = m.config.expect("the [config] table parsed");
        assert!(
            cfg.section_kinds.is_empty(),
            "typed-anchor-only: no sections"
        );
        assert_eq!(cfg.anchors.len(), 2);
        assert_eq!(cfg.anchors[0].node_kind, "object_type_definition");
        assert_eq!(cfg.anchors[0].kind, "gql_type");
        assert_eq!(cfg.anchors[0].name_child.as_deref(), Some("name"));
        assert_eq!(cfg.anchors[0].payload.as_deref(), Some("object"));
        assert_eq!(cfg.anchors[1].payload.as_deref(), Some("scalar"));
    }

    #[test]
    fn an_anchor_with_an_unknown_kind_is_rejected() {
        let toml = r#"
            name = "x"
            extensions = ["x"]
            module_separator = "/"
            abi_version = 15
            capabilities = []
            artifact = true
            [[config.anchors]]
            node_kind = "message"
            kind = "not_a_kind"
        "#;
        let err = PluginManifest::parse("x/plugin.toml", toml).unwrap_err();
        assert!(err.to_string().contains("not a known node kind"), "{err}");
    }

    #[test]
    fn an_anchor_mapping_to_a_non_config_kind_is_rejected() {
        // A code kind (`function`) is not a valid typed-anchor target — only
        // config/artifact kinds (never the `config_file` root) are.
        let toml = r#"
            name = "x"
            extensions = ["x"]
            module_separator = "/"
            abi_version = 15
            capabilities = []
            artifact = true
            [[config.anchors]]
            node_kind = "message"
            kind = "function"
        "#;
        let err = PluginManifest::parse("x/plugin.toml", toml).unwrap_err();
        assert!(
            err.to_string()
                .contains("not a typed config/artifact node kind"),
            "{err}"
        );
    }

    #[test]
    fn the_config_file_root_kind_is_not_a_valid_anchor() {
        let toml = r#"
            name = "x"
            extensions = ["x"]
            module_separator = "/"
            abi_version = 15
            capabilities = []
            artifact = true
            [[config.anchors]]
            node_kind = "message"
            kind = "config_file"
        "#;
        let err = PluginManifest::parse("x/plugin.toml", toml).unwrap_err();
        assert!(err
            .to_string()
            .contains("not a typed config/artifact node kind"));
    }

    #[test]
    fn the_config_section_kind_is_not_a_valid_anchor() {
        // `config_section` is a config kind but is emitted only by the generic
        // section walk; mapping an anchor to it would bypass depth bounding and
        // section semantics, so it is rejected like the `config_file` root.
        let toml = r#"
            name = "x"
            extensions = ["x"]
            module_separator = "/"
            abi_version = 15
            capabilities = []
            artifact = true
            [[config.anchors]]
            node_kind = "message"
            kind = "config_section"
        "#;
        let err = PluginManifest::parse("x/plugin.toml", toml).unwrap_err();
        assert!(err
            .to_string()
            .contains("not a typed config/artifact node kind"));
    }

    #[test]
    fn a_duplicate_anchor_node_kind_is_rejected() {
        // Two anchors claiming the same tree-sitter `node_kind`: the walk would
        // keep only the first, so a duplicate is a loud parse error.
        let toml = r#"
            name = "x"
            extensions = ["x"]
            module_separator = "/"
            abi_version = 15
            capabilities = []
            artifact = true
            [[config.anchors]]
            node_kind = "message"
            kind = "proto_message"
            [[config.anchors]]
            node_kind = "message"
            kind = "proto_service"
        "#;
        let err = PluginManifest::parse("x/plugin.toml", toml).unwrap_err();
        assert!(err.to_string().contains("duplicate"), "{err}");
    }

    #[test]
    fn an_empty_config_table_is_rejected() {
        // A `[config]` table that declares neither sections nor anchors drives
        // nothing — a descriptor bug, failed loudly.
        let toml = r#"
            name = "x"
            extensions = ["x"]
            module_separator = "/"
            abi_version = 15
            capabilities = []
            artifact = true
            [config]
        "#;
        let err = PluginManifest::parse("x/plugin.toml", toml).unwrap_err();
        assert!(
            err.to_string().contains("must declare `section_kinds` or"),
            "{err}"
        );
    }

    #[test]
    fn parses_a_documentation_descriptor() {
        // The markdown documentation plugin (S-033, CR-003): a structural
        // grammar with no tagging queries — `capabilities` empty, `documentation`
        // true, and a `[queries]` table omitted entirely.
        let toml = r#"
            name = "markdown"
            extensions = ["md", "markdown"]
            module_separator = "/"
            abi_version = 15
            capabilities = []
            documentation = true
        "#;
        let m = PluginManifest::parse("markdown/plugin.toml", toml).unwrap();
        assert!(m.documentation, "the markdown descriptor is a doc plugin");
        assert_eq!(m.extensions, ["md", "markdown"]);
        assert!(m.capabilities.is_empty());
        assert!(m.queries.is_empty());
    }

    #[test]
    fn every_test_convention_wire_name_parses() {
        for (wire, expect) in [
            ("none", TestConvention::None),
            ("rust-attributes", TestConvention::RustAttributes),
            ("python-test", TestConvention::PythonTest),
            ("js-callback", TestConvention::JsCallback),
            ("go-test-func", TestConvention::GoTestFunc),
            ("java-annotations", TestConvention::JavaAnnotations),
            ("kotlin-annotations", TestConvention::KotlinAnnotations),
            ("c-sharp-attributes", TestConvention::CSharpAttributes),
            ("cpp-test-macros", TestConvention::CppTestMacros),
            ("ruby-test", TestConvention::RubyTest),
            ("php-unit", TestConvention::PhpUnit),
            ("scala-test", TestConvention::ScalaTest),
        ] {
            let toml = format!(
                r#"
                name = "x"
                extensions = ["x"]
                module_separator = "."
                abi_version = 15
                capabilities = []
                test_convention = "{wire}"
            "#
            );
            let m = PluginManifest::parse("x/plugin.toml", &toml).unwrap();
            assert_eq!(m.test_convention, expect, "wire name '{wire}'");
        }
    }

    #[test]
    fn unknown_test_convention_fails_naming_the_file() {
        let toml = r#"
            name = "x"
            extensions = ["x"]
            module_separator = "."
            abi_version = 15
            capabilities = []
            test_convention = "bogus"
        "#;
        let err = PluginManifest::parse("x/plugin.toml", toml).unwrap_err();
        assert!(matches!(err, PluginError::Manifest { .. }));
    }

    #[test]
    fn parses_the_reachability_capability_flag() {
        // S-159 / CR-043 / ADR-39: an opt-in `reachability = true` parses, and a
        // descriptor that omits it defaults to false — the optional-capability
        // pattern (NFR-MA-01). Only languages with proven binder coverage opt in.
        let toml = r#"
            name = "rust"
            extensions = ["rs"]
            module_separator = "::"
            abi_version = 15
            capabilities = []
            reachability = true
        "#;
        let m = PluginManifest::parse("rust/plugin.toml", toml).unwrap();
        assert!(m.reachability, "the descriptor opts into reachability");

        let without = r#"
            name = "javascript"
            extensions = ["js"]
            module_separator = "."
            abi_version = 15
            capabilities = []
        "#;
        let m = PluginManifest::parse("javascript/plugin.toml", without).unwrap();
        assert!(
            !m.reachability,
            "a descriptor that omits the flag is not reachability-capable"
        );
    }

    #[test]
    fn parses_the_s015_framework_and_export_fields() {
        let toml = r#"
            name = "java"
            extensions = ["java"]
            module_separator = "."
            abi_version = 14
            capabilities = ["symbols"]
            framework_detectors = ["org::springframework"]
            export_convention = "public-modifier"
            [queries]
            symbols = "queries/symbols.scm"
            [framework_methods]
            GetMapping = "GET"
            RequestMapping = "ANY"
        "#;
        let m = PluginManifest::parse("java/plugin.toml", toml).unwrap();
        assert_eq!(m.framework_detectors, ["org::springframework"]);
        assert_eq!(m.framework_methods.get("GetMapping").unwrap(), "GET");
        assert_eq!(m.framework_methods.get("RequestMapping").unwrap(), "ANY");
        assert_eq!(m.export_convention, ExportConvention::PublicModifier);
    }

    #[test]
    fn every_export_convention_wire_name_parses() {
        for (wire, expect) in [
            ("all", ExportConvention::All),
            ("visibility-modifier", ExportConvention::VisibilityModifier),
            ("export-statement", ExportConvention::ExportStatement),
            ("capitalized", ExportConvention::Capitalized),
            ("public-modifier", ExportConvention::PublicModifier),
            ("underscore-private", ExportConvention::UnderscorePrivate),
            ("non-static", ExportConvention::NonStatic),
            ("explicit-modifier", ExportConvention::ExplicitModifier),
            ("cpp-external-linkage", ExportConvention::CppExternalLinkage),
            ("php-visibility", ExportConvention::PhpVisibility),
            ("public-default", ExportConvention::PublicDefault),
        ] {
            let toml = format!(
                r#"
                name = "x"
                extensions = ["x"]
                module_separator = "."
                abi_version = 15
                capabilities = []
                export_convention = "{wire}"
            "#
            );
            let m = PluginManifest::parse("x/plugin.toml", &toml).unwrap();
            assert_eq!(m.export_convention, expect, "wire name '{wire}'");
        }
    }

    #[test]
    fn unknown_export_convention_fails_naming_the_file() {
        let toml = r#"
            name = "x"
            extensions = ["x"]
            module_separator = "."
            abi_version = 15
            capabilities = []
            export_convention = "bogus"
        "#;
        let err = PluginManifest::parse("x/plugin.toml", toml).unwrap_err();
        assert!(matches!(err, PluginError::Manifest { .. }));
    }

    #[test]
    fn empty_framework_detector_is_rejected() {
        let toml = r#"
            name = "x"
            extensions = ["x"]
            module_separator = "."
            abi_version = 15
            capabilities = []
            framework_detectors = [""]
        "#;
        let err = PluginManifest::parse("x/plugin.toml", toml).unwrap_err();
        assert!(err.to_string().contains("framework_detectors"));
    }

    #[test]
    fn unknown_field_fails_naming_the_file() {
        let toml = format!("{GOOD}\n        bogus_key = 1\n");
        let err = PluginManifest::parse("rust/plugin.toml", &toml).unwrap_err();
        match err {
            PluginError::Manifest { file, .. } => assert_eq!(file, "rust/plugin.toml"),
            other => panic!("expected Manifest error, got {other:?}"),
        }
    }

    #[test]
    fn capability_without_a_query_is_rejected() {
        let toml = r#"
            name = "x"
            extensions = ["x"]
            module_separator = "."
            abi_version = 15
            capabilities = ["symbols", "calls"]
            [queries]
            symbols = "queries/symbols.scm"
        "#;
        let err = PluginManifest::parse("x/plugin.toml", toml).unwrap_err();
        assert!(matches!(err, PluginError::Manifest { .. }));
        assert!(err.to_string().contains("calls"));
    }

    #[test]
    fn extension_with_leading_dot_is_rejected() {
        let toml = r#"
            name = "x"
            extensions = [".x"]
            module_separator = "."
            abi_version = 15
            capabilities = []
        "#;
        let err = PluginManifest::parse("x/plugin.toml", toml).unwrap_err();
        assert!(err.to_string().contains("leading dot"));
    }

    #[test]
    fn empty_extensions_is_rejected() {
        let toml = r#"
            name = "x"
            extensions = []
            module_separator = "."
            abi_version = 15
            capabilities = []
        "#;
        assert!(PluginManifest::parse("x/plugin.toml", toml).is_err());
    }

    #[test]
    fn empty_name_is_rejected() {
        let toml = r#"
            name = ""
            extensions = ["x"]
            module_separator = "."
            abi_version = 15
            capabilities = []
        "#;
        let err = PluginManifest::parse("x/plugin.toml", toml).unwrap_err();
        assert!(err.to_string().contains("name"));
    }

    #[test]
    fn empty_module_separator_is_rejected() {
        let toml = r#"
            name = "x"
            extensions = ["x"]
            module_separator = ""
            abi_version = 15
            capabilities = []
        "#;
        let err = PluginManifest::parse("x/plugin.toml", toml).unwrap_err();
        assert!(err.to_string().contains("module_separator"));
    }

    #[test]
    fn name_with_path_separator_is_rejected() {
        // TOML *literal* (single-quoted) strings so backslashes are not escape
        // sequences — `'a\b'` is a literal backslash, unlike the basic string
        // `"a\b"` which would be a backspace.
        for bad in ["../etc", "a/b", r"a\b"] {
            let toml = format!(
                r#"
                name = '{bad}'
                extensions = ["x"]
                module_separator = "."
                abi_version = 15
                capabilities = []
            "#
            );
            let err = PluginManifest::parse("x/plugin.toml", &toml).unwrap_err();
            assert!(
                err.to_string().contains("path separator"),
                "name '{bad}' must be rejected as a path component"
            );
        }
    }
}
