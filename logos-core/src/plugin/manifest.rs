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

/// The grammar a language's **import specifier** is written in — the text an
/// `@ref.import` capture holds (S-439, [CR-142] D1, [FR-RS-01]).
///
/// A specifier and a member expression are two grammars, and a descriptor that
/// declared only [`module_separator`](PluginManifest::module_separator) could not
/// say so. In a member path `a.b.c` the dot separates names; in a module
/// specifier `"./nav.ts"` it introduces a file extension, and in
/// `"github.com/org/repo/internal/admin"` it sits *inside* a host name. With no
/// way to declare the difference, every import was split by the member-path
/// grammar — on every separator any language's member paths use — which
/// recorded `./nav.ts` as `nav::ts` and cut the Go host in half, so neither
/// could ever match a file: the reason TypeScript, TSX, JavaScript and Go
/// produced zero `Imports` edges.
///
/// The default is [`ImportSpecifier::Name`], the behaviour every descriptor had
/// before this field existed, so a language that omits it is untouched
/// ([NFR-MA-01]).
///
/// [CR-142]: ../../../docs/requests/CR-142-cross-file-call-resolution-is-rust-only.md
/// [FR-RS-01]: ../../../docs/specs/requirements/FR-RS-01.md
/// [NFR-MA-01]: ../../../docs/specs/requirements/NFR-MA-01.md
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ImportSpecifier {
    /// A dotted/scoped **name** (`django.urls`, `org.springframework.web`,
    /// `Illuminate\Support\Route`): canonicalised like a member path, every
    /// separator the language's paths use splitting a segment.
    #[default]
    Name,
    /// A **path** (`"./nav.ts"`, `"github.com/lib/pq"`): only `/` separates, a
    /// dot is part of the segment it sits in, a relative specifier keeps its
    /// leading `.`/`..` so the binder can resolve it against the importing file,
    /// and a relative specifier's trailing
    /// [`specifier_extensions`](PluginManifest::specifier_extensions) extension is
    /// stripped.
    Path,
}

/// What an **unqualified** call inside a class body means in this language —
/// the implicit-receiver policy of the receiver-shape seam (S-514, [FR-EX-13]).
///
/// It decides only for a call the `references` query marks
/// `@ref.receiver.implicit` (a call written with no receiver at all, `m()`):
///
/// | value | an implicit call inside a named class | anywhere else |
/// |---|---|---|
/// | `"none"` (default) | a free call | a free call |
/// | `"self"` | a method-form call on the current instance (`self` shape) | a free call |
///
/// "Anywhere else" is a call at file or module scope, in a free function, or
/// inside an anonymous class body, whose instance the graph has no node for. A
/// free call is recorded as the Path-form bare name a `@ref.call` records and
/// is bound by the scope hierarchy, so a language that omits the key, or a call
/// it does not mark, is recorded exactly as before ([NFR-MA-01]).
///
/// `"self"` is for the languages where `m()` inside a class means `this.m()` —
/// C#, Kotlin, Scala, C++, Ruby. Java does not need it: its bare calls are
/// `@ref.call` rows that S-467's receiver typing already qualifies by the
/// enclosing class.
///
/// An **explicit** `"none"` says more than the absent default, and the binder
/// reads the difference (S-590, [FR-RS-07]): it declares a language whose bare
/// call can never reach an instance member — Go, Rust, Python, PHP, TypeScript
/// and TSX — so a bare `f()` binds free callables only
/// ([`PluginManifest::bare_calls_free_only`]). Extraction reads the two alike.
/// Java declares nothing: its bare in-class call does mean `this.m()`, and keeps
/// reaching a member through the scope walk.
///
/// [FR-EX-13]: ../../../docs/specs/requirements/FR-EX-13.md
/// [FR-RS-07]: ../../../docs/specs/requirements/FR-RS-07.md
/// [NFR-MA-01]: ../../../docs/specs/requirements/NFR-MA-01.md
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
pub enum ImplicitReceiver {
    /// An unqualified call is a free call wherever it is written.
    #[default]
    #[serde(rename = "none")]
    None,
    /// An unqualified call inside a named class body is a call on the current
    /// instance.
    #[serde(rename = "self")]
    SelfInstance,
}

/// Which declarations besides a `Function` or `Method` a call in this language
/// may bind to (S-521, [FR-RS-16]) — the resolved form of the descriptor's
/// [`class_call_instantiates`](PluginManifest::class_call_instantiates) and
/// [`macros_callable`](PluginManifest::macros_callable) keys.
///
/// Both default to `false`, which is every language's call admission before
/// the keys existed, so a plugin that declares neither resolves exactly as
/// before ([NFR-MA-01]). The acceptance rule does not change: the call binds
/// only when its candidate set has exactly one element ([NFR-RA-05]).
///
/// [FR-RS-16]: ../../../docs/specs/requirements/FR-RS-16.md
/// [NFR-MA-01]: ../../../docs/specs/requirements/NFR-MA-01.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct CallTargets {
    /// A call may name a `Class`, and binds to it as `Instantiates`: `Foo()`
    /// constructs (Python, Kotlin, Scala).
    pub classes: bool,
    /// A call may name a `Macro`, and binds to it as `Calls` (C).
    pub macros: bool,
}

impl CallTargets {
    /// `true` when the language admits any call target beyond a callable.
    pub fn any(self) -> bool {
        self.classes || self.macros
    }
}

/// How far a code language's references bind **across a file boundary** — the
/// one-word summary of a `[reach]` declaration ([FR-PL-09], [CR-180]).
///
/// `references` as a capability says a plugin *captures* references; it says
/// nothing about whether they bind to a declaration in another file. The level
/// is the honest answer, declared where the plugin is, so `logos languages`, the
/// manual and the README never present a same-file language with the capability
/// of Java ([NFR-CC-04]).
///
/// [FR-PL-09]: ../../../docs/specs/requirements/FR-PL-09.md
/// [CR-180]: ../../../docs/requests/CR-180-scala-is-declared-as-limited-support-and-every-language-declares-its-reach.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReachLevel {
    /// Calls, imports and type relations bind across files.
    Resolved,
    /// Some relations bind across files, not all.
    Partial,
    /// References bind only inside the file that wrote them.
    SameFile,
    /// Declarations only: nothing binds across files (a call to a function of the
    /// same file may still bind).
    Symbols,
}

impl ReachLevel {
    /// The wire / manual spelling — the one the descriptor, `logos languages`
    /// and the generated manual table all print.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ReachLevel::Resolved => "resolved",
            ReachLevel::Partial => "partial",
            ReachLevel::SameFile => "same-file",
            ReachLevel::Symbols => "symbols",
        }
    }
}

/// A relation a plugin can bind across a file boundary — the vocabulary of
/// [`Reach::cross_file`] ([FR-PL-09]). Declared order is the display order.
///
/// [FR-PL-09]: ../../../docs/specs/requirements/FR-PL-09.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CrossFileRelation {
    /// A call edge to a callable declared in another file.
    Calls,
    /// An import edge to a module or declaration in another file.
    Imports,
    /// An `extends`/`implements`/type-use edge to a type declared in another file.
    TypeRelations,
    /// A member-access edge (a field read) to a declaration in another file.
    MemberAccess,
    /// A route edge from a framework route to a handler in another file.
    Routes,
}

impl CrossFileRelation {
    /// The wire / manual spelling (`type_relations`, …).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            CrossFileRelation::Calls => "calls",
            CrossFileRelation::Imports => "imports",
            CrossFileRelation::TypeRelations => "type_relations",
            CrossFileRelation::MemberAccess => "member_access",
            CrossFileRelation::Routes => "routes",
        }
    }
}

/// The `[reach]` descriptor table: which relations this code language binds
/// across files, and the level that summarises them ([FR-PL-09], [CR-180]).
///
/// Declared, then **verified**: `tests/reach_declared.rs` indexes a fixture per
/// language and fails when a declared relation binds nothing across files, or an
/// undeclared one does — so a story that changes a language's reach updates this
/// table in the same change.
///
/// [FR-PL-09]: ../../../docs/specs/requirements/FR-PL-09.md
/// [CR-180]: ../../../docs/requests/CR-180-scala-is-declared-as-limited-support-and-every-language-declares-its-reach.md
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reach {
    /// The summary level.
    pub level: ReachLevel,
    /// The relations bound across a file boundary. Empty for `same-file` and
    /// `symbols`, non-empty for `resolved` and `partial`.
    #[serde(default)]
    pub cross_file: Vec<CrossFileRelation>,
}

/// The `[package_modules]` descriptor sub-table: this language's module path is
/// **package-shaped** under the named source roots ([CR-149], [FR-RS-01]).
///
/// The binder's default module model is Rust's — the directory before the last
/// `src/` names the crate and every later directory is a module — which keys
/// `src/main/java/com/x/Svc.java` as `main::java::com::x::Svc`, so the import
/// `com.x.Svc` could never descend to it. A language declaring this table has
/// each file under one of `source_roots` keyed by the path *after* the root
/// instead (`com::x::Svc`), which is the name its imports spell. A file of the
/// language outside every root keeps the default key, which is already
/// package-shaped for a flat or `src/`-rooted layout.
///
/// Absent (the default) leaves a language's module keys exactly as they were
/// ([NFR-MA-01]). The one derivation of a package from a path is
/// [`crate::resolve::package_key`]; this table is only its data.
///
/// [CR-149]: ../../../docs/requests/CR-149-java-imports-and-type-relations-never-bind.md
/// [FR-RS-01]: ../../../docs/specs/requirements/FR-RS-01.md
/// [NFR-MA-01]: ../../../docs/specs/requirements/NFR-MA-01.md
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageModules {
    /// Project-relative directory sequences a package path starts beneath, each
    /// `/`-separated with no leading or trailing `/` (`"src/main/java"`). Matched
    /// as whole path segments anywhere in a file's path, so a Maven module's
    /// `mailbox-core/src/main/java/…` is keyed like a root-level one, under its
    /// own crate.
    pub source_roots: Vec<String>,
}

/// The `[module_model]` descriptor sub-table: **which** module model keys this
/// language's files ([FR-RS-01], [FR-RS-13]) — the one declaration the binder's
/// module key ([`crate::resolve::package_key`]) dispatches on.
///
/// ```toml
/// [module_model]
/// kind = "namespace"
/// ```
///
/// One table, one `kind`, so a further model is a further [`ModuleModelKind`]
/// variant validated in [`validate_module_model`], never a second table with
/// its own precedence rules. A kind that needs data carries it beside `kind` in
/// this table — except `package`, whose source roots predate this table and
/// stay in `[package_modules]` ([`PackageModules`]).
///
/// Omitted, the model is `package` when `[package_modules]` is declared and
/// `path` otherwise ([`PluginManifest::module_model_kind`]), so every
/// descriptor written before the table keeps its keys ([NFR-MA-01]).
///
/// The `path` model's data (S-519, [FR-RS-14]) sits beside `kind`:
///
/// ```toml
/// [module_model]
/// kind = "path"
/// package_stems = ["__init__"]   # a package file names its directory
/// import_roots = ["src"]         # candidate roots; the repository root is the fallback
/// family = "python"              # which languages' modules and types it may bind
/// ```
///
/// The `namespace` model's data (S-595, [FR-RS-45]) sits beside `kind` too:
///
/// ```toml
/// [module_model]
/// kind = "namespace"
/// enclosing_namespaces = true    # a namespace sees its enclosing namespaces' types
/// ```
///
/// [FR-RS-45]: ../../../docs/specs/requirements/FR-RS-45.md
///
/// [FR-RS-01]: ../../../docs/specs/requirements/FR-RS-01.md
/// [FR-RS-13]: ../../../docs/specs/requirements/FR-RS-13.md
/// [FR-RS-14]: ../../../docs/specs/requirements/FR-RS-14.md
/// [NFR-MA-01]: ../../../docs/specs/requirements/NFR-MA-01.md
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleModel {
    /// The model's name.
    pub kind: ModuleModelKind,
    /// The `path` model's **package-file stems** (S-519, [FR-RS-14]): a file
    /// with one of these stems names its directory rather than adding a module
    /// of its own — Python's `__init__`, Rust's `mod`/`lib`/`main`. Empty (the
    /// default) folds nothing, so a JavaScript `main.js` is the module `main`.
    ///
    /// [FR-RS-14]: ../../../docs/specs/requirements/FR-RS-14.md
    #[serde(default)]
    pub package_stems: Vec<String>,
    /// The `path` model's **candidate import roots** (S-519, [FR-RS-14]), each
    /// a `/`-separated directory (`"src"`). Declaring the key switches the
    /// language's files to import-root keying: a candidate is chosen when it
    /// holds a package (a package-stem file in a directory beneath it), the
    /// repository root is always the fallback, and every directory under a root
    /// is a module. `.logos/config.toml`'s `[resolution.import_roots]` replaces
    /// the detection. `None` (omitted) keeps the default model's `src/` crate
    /// rule — Rust's.
    ///
    /// [FR-RS-14]: ../../../docs/specs/requirements/FR-RS-14.md
    #[serde(default)]
    pub import_roots: Option<Vec<String>>,
    /// The **interop family** this language binds within (S-519, [NFR-RA-05]):
    /// languages that can name each other's types declare one family (Java,
    /// Kotlin and Scala declare `jvm`), so the fully-qualified type index, the
    /// namespace-files index and an import-root language's module tree reach
    /// only targets of the source's own family. Omitted, a language is its own
    /// family (its plugin name).
    ///
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    #[serde(default)]
    pub family: Option<String>,
    /// The `namespace` model's **enclosing-namespace visibility** (S-595,
    /// [FR-RS-45]): when `true`, a type name — or the head of a qualified
    /// name — that the source's own namespace does not supply is looked up in
    /// each enclosing namespace of the source's declared one, nearest first
    /// (`A.B.C` → `A.B` → `A`, never the global namespace), before any
    /// namespace wildcard is read. Exactly one type decides a level; two stop
    /// the walk unbound, and none passes outward. Defaults to `false`, so a
    /// language that does not declare it binds exactly as before.
    ///
    /// [FR-RS-45]: ../../../docs/specs/requirements/FR-RS-45.md
    #[serde(default)]
    pub enclosing_namespaces: bool,
}

/// The module models a language may declare ([`ModuleModel`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModuleModelKind {
    /// The model keyed by the file's path, the default: the directory before
    /// the last `src/` names the crate and every later directory is a module —
    /// or, when the plugin declares import roots, the path after its import
    /// root, under its family's own crate. A declared package-file stem names
    /// its directory ([`ModuleModel::package_stems`], S-519).
    #[default]
    Path,
    /// A package under fixed source roots, named by the path after the root
    /// (Java, [CR-149]). Requires `[package_modules]`.
    ///
    /// [CR-149]: ../../../docs/requests/CR-149-java-imports-and-type-relations-never-bind.md
    Package,
    /// The namespace or package each file **declares** (PHP, C#, Kotlin, Scala;
    /// [FR-RS-13]): its `symbols` query captures the declaration's name with
    /// `@module.namespace`, and that name — not the path — is the file's
    /// package. A file declaring none is in the global namespace.
    ///
    /// [FR-RS-13]: ../../../docs/specs/requirements/FR-RS-13.md
    Namespace,
}

/// The capture a declared-namespace language's `symbols` query names a
/// namespace or package declaration's **name** with (S-518, [FR-RS-13]):
/// PHP's `namespace`, C#'s file-scoped or block `namespace`, Kotlin's and
/// Scala's `package`. Its group is `module`, not the declaration group
/// `symbol`, so extraction's declaration walk never reads it as a node; a
/// namespace-model plugin whose `symbols` query lacks it fails to load
/// (`registry::check_namespace_capture`).
///
/// [FR-RS-13]: ../../../docs/specs/requirements/FR-RS-13.md
pub(crate) const NAMESPACE_CAPTURE: &str = "module.namespace";

/// The marker a `symbols` query puts beside [`NAMESPACE_CAPTURE`] when the
/// language's bodiless namespace declarations **compose** rather than replace
/// one another — Scala's chained `package a` / `package b` is `a.b`, where
/// PHP's `namespace A;` … `namespace B;` puts what follows in `B` alone (S-518).
pub(crate) const NAMESPACE_CHAINED_CAPTURE: &str = "module.namespace.chained";

impl ModuleModelKind {
    /// The descriptor token.
    pub fn as_str(self) -> &'static str {
        match self {
            ModuleModelKind::Path => "path",
            ModuleModelKind::Package => "package",
            ModuleModelKind::Namespace => "namespace",
        }
    }
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
    /// The **member-path** separator joining symbol segments (`::`, `.`, `/`) —
    /// the `a.b.c` grammar only. An import specifier's grammar is declared
    /// separately by [`import_specifier`](Self::import_specifier), because the
    /// two are different grammars (S-439).
    pub module_separator: String,
    /// The grammar this language's import specifiers are written in
    /// ([`ImportSpecifier`]). Defaults to [`ImportSpecifier::Name`] when omitted.
    #[serde(default)]
    pub import_specifier: ImportSpecifier,
    /// File extensions (without the leading dot) a **relative** path specifier
    /// may spell and that name the imported file itself, so `"./nav.ts"` and
    /// `"./nav"` canonicalise to the one ledger target (S-439). Only meaningful
    /// under [`ImportSpecifier::Path`]; declaring any under
    /// [`ImportSpecifier::Name`] is a descriptor error. An extension not listed
    /// here (`"./styles.css"`) is kept, so it can never be read as a code file of
    /// the same stem ([NFR-RA-05]).
    ///
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    #[serde(default)]
    pub specifier_extensions: Vec<String>,
    /// What an unqualified call inside a class body means
    /// ([`ImplicitReceiver`], S-514), as declared: `None` when the key is
    /// omitted. Read through [`implicit_receiver`](Self::implicit_receiver),
    /// which defaults it, and [`bare_calls_free_only`](Self::bare_calls_free_only),
    /// which tells an explicit `"none"` from the omission (S-590).
    #[serde(default)]
    pub implicit_receiver: Option<ImplicitReceiver>,
    /// Whether calling a class constructs it (S-521, [FR-RS-16]): a call whose
    /// one candidate is a `Class` records `Instantiates` to it. For the
    /// languages where `Foo()` builds a `Foo` — Python, Kotlin, Scala. Defaults
    /// to `false`: a call binds a `Function` or `Method` only ([`CallTargets`]).
    ///
    /// [FR-RS-16]: ../../../docs/specs/requirements/FR-RS-16.md
    #[serde(default)]
    pub class_call_instantiates: bool,
    /// Whether a call may bind a `Macro` (S-521, [FR-RS-16]): a call whose one
    /// candidate is a function-like macro records `Calls` to it — C, where
    /// `f(x)` may expand a `#define f(x)`. Defaults to `false` ([`CallTargets`]).
    ///
    /// [FR-RS-16]: ../../../docs/specs/requirements/FR-RS-16.md
    #[serde(default)]
    pub macros_callable: bool,
    /// Whether this language's supertype clause leaves unsaid whether a
    /// supertype is a base class or an interface (S-522, [FR-RS-15]): C#'s
    /// `class A : B, IC` and Kotlin's `class A : Base(), Iface` write both in
    /// one list. Each supertype is then captured as an `Extends` row, and a
    /// class's row binds the one in-repository class, interface or trait it
    /// names, its edge kind following the target — `Extends` to a class,
    /// `Implements` to an interface or a trait. An interface's supertypes are
    /// interfaces whatever the key says. Defaults to `false`: an `Extends`
    /// binds a class (an interface, from an interface) and an `Implements` an
    /// interface or a trait, as each clause spells it.
    ///
    /// [FR-RS-15]: ../../../docs/specs/requirements/FR-RS-15.md
    #[serde(default)]
    pub supertype_kind_follows_target: bool,
    /// The methods a peeled receiver wrapper provides itself (S-588,
    /// [FR-RS-42]): wrapper name → method names. A call on a receiver proven
    /// through such a wrapper (`x: Arc<T>`, recorded with `peeled = "Arc"`)
    /// never binds one of these among `T`'s methods: `x.clone()` calls
    /// `Arc::clone`, whatever `T` defines. A wrapper with no entry — a
    /// reference — provides none. Defaults to empty: no language but Rust
    /// peels a receiver ([NFR-MA-01]).
    ///
    /// ```toml
    /// [wrapper_methods]
    /// Arc = ["clone", "as_ref", "borrow"]
    /// ```
    ///
    /// [FR-RS-42]: ../../../docs/specs/requirements/FR-RS-42.md
    /// [NFR-MA-01]: ../../../docs/specs/requirements/NFR-MA-01.md
    #[serde(default)]
    pub wrapper_methods: BTreeMap<String, Vec<String>>,
    /// Whether, and under which source roots, this language's module path is
    /// package-shaped ([`PackageModules`], [CR-149]). `None` when the
    /// `[package_modules]` table is omitted — the default module model.
    ///
    /// [CR-149]: ../../../docs/requests/CR-149-java-imports-and-type-relations-never-bind.md
    #[serde(default)]
    pub package_modules: Option<PackageModules>,
    /// Which module model keys this language's files ([`ModuleModel`],
    /// [FR-RS-13]). `None` when the `[module_model]` table is omitted; read it
    /// through [`module_model_kind`](Self::module_model_kind), which resolves
    /// the omission.
    ///
    /// [FR-RS-13]: ../../../docs/specs/requirements/FR-RS-13.md
    #[serde(default)]
    pub module_model: Option<ModuleModel>,
    /// The cross-file reach this code language declares ([`Reach`], [FR-PL-09]).
    /// `None` for the documentation and artifact classes, which bind no code
    /// reference at all.
    ///
    /// [FR-PL-09]: ../../../docs/specs/requirements/FR-PL-09.md
    #[serde(default)]
    pub reach: Option<Reach>,
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
    /// Tree-sitter node kinds that mark a callable as **implemented** — the
    /// declarative input to the per-callable has-body fact ([FR-EX-11],
    /// CR-163), the same descriptor pattern as
    /// [`nesting_block_kinds`](Self::nesting_block_kinds). A `Function`/`Method`
    /// declaration has a body when the declaration node itself, or one of its
    /// direct children, is of a listed kind: usually the body child (`block`,
    /// `function_body`, `compound_statement`), or — for a grammar that tells a
    /// definition from a declaration by node kind, as Scala does — the bodied
    /// declaration kind itself. An abstract method, an interface method with no
    /// default or a C++ pure-virtual carries none of them.
    ///
    /// Defaults to empty when omitted, and a language declaring none treats
    /// every callable as bodied, so its extraction is byte-identical to before
    /// ([NFR-MA-01]).
    ///
    /// [FR-EX-11]: ../../../docs/specs/requirements/FR-EX-11.md
    /// [NFR-MA-01]: ../../../docs/specs/requirements/NFR-MA-01.md
    #[serde(default)]
    pub body_node_kinds: Vec<String>,
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
    /// Canonical (`::`-joined) reference-path prefixes whose presence in a
    /// file's ledger makes the file an **HTTP client-call** candidate (S-341,
    /// [CR-108], [FR-WS-08]) — the consumer-side twin of `framework_detectors`,
    /// e.g. `["reqwest", "hyper"]` or
    /// `["org::springframework::web::client", "java::net::http"]`. Empty = this
    /// language captures no outbound calls, so its `invocations` query (if any)
    /// never runs.
    ///
    /// Declarative for the same reason the framework detectors are: the shared
    /// interpreter must be driven by descriptor data, never by a branch on which
    /// language it is looking at (`resolve::framework::tests::jvm_parity`).
    ///
    /// A **bare-identifier** row (TypeScript's `fetch`, S-343) matches a plain
    /// call as well as an import, which makes the gate non-independent for that
    /// entry and obliges the language's `invocations.scm` to carry the scope
    /// instead — the rule is stated once on `extract::capture_http_client_call_arm`
    /// (private to that module, so named rather than linked); read it before
    /// adding one.
    ///
    /// [CR-108]: ../../../docs/requests/CR-108-per-language-http-client-call-capture.md
    /// [FR-WS-08]: ../../../docs/specs/requirements/FR-WS-08.md
    #[serde(default)]
    pub http_client_detectors: Vec<String>,
    /// Captured `@fw.route.method` text → upper-cased HTTP method (`"GET"`, …,
    /// `"ANY"`) for the declarative framework-query contract (S-015,
    /// [FR-FW-01]). A captured method text with no entry here is dropped — the
    /// table doubles as the recognised-registration filter.
    ///
    /// [FR-FW-01]: ../../../docs/specs/requirements/FR-FW-01.md
    #[serde(default)]
    pub framework_methods: BTreeMap<String, String>,
    /// Captured `@invoke.http.method` text → upper-cased HTTP method for the
    /// **consumer** side (S-346, [CR-108], [FR-WS-08]) — the client-call twin of
    /// [`framework_methods`](Self::framework_methods), and it plays the same
    /// two roles: it *normalizes* a verb whose source spelling is not a bare
    /// HTTP verb, and it *filters*, because a captured text with no entry here
    /// is dropped.
    ///
    /// **Empty (the default) means "no normalization and no filter"**: the
    /// captured text goes straight to `extract::is_http_method`, which is what
    /// every language whose verbs are already bare spells (Rust, Go, Java,
    /// TypeScript) relies on. Only a language that declares rows opts into the
    /// filter.
    ///
    /// C# is the first such language and the reason this field exists: its verbs
    /// carry an `Async` suffix (`GetAsync`) or live in a named constant
    /// (`HttpMethod.Get`), and `is_http_method` speaks only bare verbs. Keys are
    /// matched **exactly**, like `framework_methods`, so a row spells the token
    /// as the language's own API does.
    ///
    /// The mapped value is **still** passed through `is_http_method`, so a
    /// mistyped value (`"GTE"`) captures nothing rather than inventing a method
    /// ([NFR-RA-05]).
    ///
    /// [CR-108]: ../../../docs/requests/CR-108-per-language-http-client-call-capture.md
    /// [FR-WS-08]: ../../../docs/specs/requirements/FR-WS-08.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    #[serde(default)]
    pub invocation_methods: BTreeMap<String, String>,
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
    /// The configuration-**binding** descriptor (S-381, [CR-121], [FR-WS-19]):
    /// the annotation vocabulary that marks a properties class and the accessor
    /// convention a use site reads it by, driving the generic interpreter in
    /// [`crate::extract::config::binding`]. Meaningful only alongside the
    /// `properties` capability and its query; absent for every language that
    /// binds no configuration. See [`PropertiesDescriptor`].
    ///
    /// [CR-121]: ../../../docs/requests/CR-121-caller-to-callee-and-producer-to-consumer-across-services.md
    /// [FR-WS-19]: ../../../docs/specs/requirements/FR-WS-19.md
    #[serde(default)]
    pub properties: Option<PropertiesDescriptor>,
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

/// The `[properties]` descriptor sub-table for a language that binds committed
/// configuration into a class (S-381, [CR-121], [FR-WS-19], [FR-PL-02]).
///
/// Drives the **generic**, grammar-agnostic properties-class interpreter in
/// [`crate::extract::config::binding`] declaratively — the same descriptor-data
/// pattern as [`ConfigDescriptor`] and [`PluginManifest::framework_methods`]:
/// the core walk reads only capture names, and each language supplies the
/// vocabulary its own `properties.scm` captures against.
///
/// Two halves, and the split is what makes the substrate language-agnostic — the
/// declaration vocabulary, and the use-site vocabulary that now carries two rows:
///
/// - [`annotations`](Self::annotations) names the **binding vocabulary** — the
///   annotation the language spells to mark a class as configuration-bound. The
///   query captures *every* annotation on a declaration and this table decides
///   which ones count, so the vocabulary lives in exactly one place. A query
///   that filtered with its own `#eq?` would be a second copy, free to drift
///   from this one.
/// - [`accessor_prefixes`](Self::accessor_prefixes) names the **accessor
///   convention** — how a use site spells a read of one property.
/// - [`self_references`](Self::self_references) names the **qualifier
///   convention** — how a use site spells the enclosing instance, when it
///   qualifies a field read with one.
///
/// [CR-121]: ../../../docs/requests/CR-121-caller-to-callee-and-producer-to-consumer-across-services.md
/// [FR-WS-19]: ../../../docs/specs/requirements/FR-WS-19.md
/// [FR-PL-02]: ../../../docs/specs/requirements/FR-PL-02.md
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PropertiesDescriptor {
    /// The annotation names that mark a class as configuration-bound, compared
    /// **exactly** against the text of the query's `@props.annotation` capture,
    /// e.g. `["ConfigurationProperties"]`. Never a prefix or substring test: an
    /// annotation whose text does not equal a row here contributes nothing, so
    /// a language widens its vocabulary by adding a row rather than by
    /// loosening a match ([NFR-RA-05]).
    ///
    /// A fully-qualified spelling
    /// (`@org.springframework.boot.context.properties.ConfigurationProperties`)
    /// is a **stated ceiling**, not a silent miss: it is one more row away, and
    /// the query's capture would carry the qualified text for it to match.
    ///
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    pub annotations: Vec<String>,
    /// How a use site names one property of a bound class — each entry a prefix
    /// stripped from the accessor's name to yield the property, e.g.
    /// `["get", "is"]` for a Java bean getter.
    ///
    /// The **empty string is a real, deliberate entry**: it means *direct
    /// property access*, where the accessor's name already **is** the property
    /// (Kotlin's `config.uriGetArchive`, a Python attribute read). A language
    /// whose use sites read a property both ways declares both, which is what
    /// Kotlin does — its own property syntax plus the JVM-interop getters a
    /// Java call site sees.
    ///
    /// Every entry is tried and the candidates are **intersected with what the
    /// class declares**; two surviving candidates are an ambiguity and bind
    /// nothing ([`crate::extract::config::binding::BindingRefusal::AmbiguousProperty`]),
    /// rather than the first one winning by table order.
    #[serde(default)]
    pub accessor_prefixes: Vec<String>,
    /// How a use site spells a reference to the **enclosing instance**, when it
    /// qualifies a field read with one — `["this"]` for Java (S-398,
    /// [FR-WS-19]).
    ///
    /// The third row of the use-site vocabulary, beside
    /// [`accessor_prefixes`](Self::accessor_prefixes), and here for the same
    /// reason: it is one language's spelling, and
    /// [`crate::extract::config::accessor`] may not name one. The [NFR-MA-01]
    /// structural guard beside the interpreter enforces that literally — it
    /// scans every double-quoted identifier in that file against both the JVM
    /// node-kind set and the all-grammar field-name set, and `this` is a named
    /// Java node kind. So a hardcoded qualifier fails the build; this row is
    /// where it goes instead.
    ///
    /// **Empty by default, and that default is the refusal.** A language that
    /// declares no row admits no qualified receiver at all, which is exactly
    /// what every language did before this row existed ([NFR-RA-05]). A
    /// language joins by adding a row, never by loosening a match.
    ///
    /// What an entry buys is narrow and bounded: a receiver **the grammar parsed
    /// as more than one node**, spelled `<entry><separator><name>`, is read as
    /// naming the field `<name>` of the enclosing class — and is then resolved
    /// against field-position declarations only. Both of those narrowings are
    /// review corrections, not decoration: without the first, `this$api` (one
    /// legal Java identifier) split into two; without the second, a same-named
    /// local answered for an inherited field. See
    /// [`BindingView::declared_receiver_type`](crate::extract::config::accessor)
    /// for both.
    ///
    /// It is **not** a general relaxation of the qualified-receiver refusal — a
    /// `holder.api` receiver stays refused, because reducing it would resolve
    /// against a same-named local, which is a different object. A self reference
    /// cannot name a different object: it names a field of the enclosing class.
    ///
    /// [FR-WS-19]: ../../../docs/specs/requirements/FR-WS-19.md
    /// [NFR-MA-01]: ../../../docs/specs/requirements/NFR-MA-01.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    #[serde(default)]
    pub self_references: Vec<String>,
}

impl PluginManifest {
    /// The module model this descriptor declares ([`ModuleModel`]), with an
    /// omitted `[module_model]` table resolved: `package` when
    /// `[package_modules]` is declared, `path` otherwise.
    pub fn module_model_kind(&self) -> ModuleModelKind {
        resolved_module_model(self.module_model.as_ref(), self.package_modules.as_ref())
    }

    /// The interop family this descriptor binds within ([`ModuleModel::family`]):
    /// the declared one, else the plugin's own name.
    pub fn module_family(&self) -> String {
        self.module_model
            .as_ref()
            .and_then(|m| m.family.clone())
            .unwrap_or_else(|| self.name.clone())
    }

    /// Whether a namespace of this language sees the types of its enclosing
    /// namespaces ([`ModuleModel::enclosing_namespaces`], S-595): `false`
    /// unless the descriptor declares it.
    pub fn enclosing_namespaces(&self) -> bool {
        self.module_model.as_ref().is_some_and(|m| m.enclosing_namespaces)
    }

    /// What an unqualified call inside a class body means ([`ImplicitReceiver`],
    /// S-514): the declared policy, else [`ImplicitReceiver::None`].
    pub fn implicit_receiver(&self) -> ImplicitReceiver {
        self.implicit_receiver.unwrap_or_default()
    }

    /// Whether a bare call of this language binds free callables only (S-590,
    /// [FR-RS-07]): `true` only when the descriptor **explicitly** declares
    /// `implicit_receiver = "none"`. An omitted key — Java's — is `false`, so a
    /// language that says nothing keeps reaching a member through the scope
    /// walk exactly as before ([NFR-MA-01]).
    ///
    /// [FR-RS-07]: ../../../docs/specs/requirements/FR-RS-07.md
    /// [NFR-MA-01]: ../../../docs/specs/requirements/NFR-MA-01.md
    pub fn bare_calls_free_only(&self) -> bool {
        self.implicit_receiver == Some(ImplicitReceiver::None)
    }

    /// The call targets this descriptor declares beyond a callable
    /// ([`CallTargets`], S-521).
    pub fn call_targets(&self) -> CallTargets {
        CallTargets {
            classes: self.class_call_instantiates,
            macros: self.macros_callable,
        }
    }

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
        if let Err(detail) = validate_wrapper_methods(&self.wrapper_methods) {
            return bail(detail);
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
        // Stripping is a path-grammar rule: a name-grammar language has no file
        // extension in its specifiers to strip, so a list there is a descriptor
        // bug rather than a no-op to tolerate.
        if !self.specifier_extensions.is_empty() && self.import_specifier != ImportSpecifier::Path {
            return bail(
                "`specifier_extensions` requires `import_specifier = \"path\"`".to_string(),
            );
        }
        if let Some(bad) = self
            .specifier_extensions
            .iter()
            .find(|e| e.is_empty() || e.contains(['.', '/']))
        {
            return bail(format!(
                "`specifier_extensions` entry '{bad}' must be a bare extension (no `.` or `/`)"
            ));
        }
        if let Err(detail) =
            validate_module_model(self.module_model.as_ref(), self.package_modules.as_ref())
        {
            return bail(detail);
        }
        if let Err(detail) = validate_reach(self.reach.as_ref()) {
            return bail(detail);
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
        // Same trap on the consumer side: an empty client detector would make
        // every file an outbound-call candidate, which is exactly the
        // over-capture the ledger gate exists to prevent ([NFR-RA-05]).
        if self.http_client_detectors.iter().any(|d| d.trim().is_empty()) {
            return bail("`http_client_detectors` entries must not be empty".to_string());
        }
        // A detector is compared against canonical reference targets verbatim,
        // so a stray surrounding space matches nothing — the arm would degrade
        // to silent no-capture, the exact failure mode CR-108 was filed for.
        // Fail loudly instead of trimming silently.
        if self.http_client_detectors.iter().any(|d| d != d.trim()) {
            return bail(
                "`http_client_detectors` entries must not carry surrounding whitespace"
                    .to_string(),
            );
        }
        // The third leg of the FR-WS-08 capability invariant. A descriptor can
        // declare `invocations`, ship a query that compiles, satisfy the
        // `frameworks` => `invocations` guard — and still capture nothing for
        // ever, because `capture_http_client_call_arm` returns early on an empty
        // detector set. That is honest absence at the capability layer becoming
        // *invisible* absence at the product layer, which is precisely the
        // defect CR-108 exists to correct; refuse the descriptor instead.
        if self.capabilities.iter().any(|c| c == "invocations")
            && self.http_client_detectors.is_empty()
        {
            return bail(
                "capability 'invocations' requires at least one `http_client_detectors` \
                 entry, or the arm can never capture (FR-WS-08, CR-108)"
                    .to_string(),
            );
        }
        // The same invariant on the binding arm, and for the same reason: the
        // generic interpreter reads its vocabulary from `[properties]`, so a
        // descriptor declaring the capability without one ships a query that
        // matches and a filter that admits nothing — honest absence at the
        // capability layer becoming invisible absence at the product layer
        // (S-381, FR-WS-19).
        if self.capabilities.iter().any(|c| c == "properties") {
            let Some(properties) = &self.properties else {
                return bail(
                    "capability 'properties' requires a `[properties]` table naming the \
                     binding vocabulary, or the arm can never bind (FR-WS-19, S-381)"
                        .to_string(),
                );
            };
            if properties.annotations.is_empty() {
                return bail(
                    "`[properties] annotations` must name at least one binding annotation \
                     (FR-WS-19, S-381)"
                        .to_string(),
                );
            }
            // The same invariant on the convention half. A descriptor naming a
            // vocabulary but no accessor convention indexes its classes and then
            // recognises no accessor at all, so every use site refuses as
            // "not an accessor" — the capability reads present while nothing it
            // captures can ever bind, one step further along than the two guards
            // above. (`""` stays a legal ENTRY — it spells direct property
            // access; only the empty LIST is the bug.)
            if properties.accessor_prefixes.is_empty() {
                return bail(
                    "capability 'properties' requires at least one `[properties] \
                     accessor_prefixes` entry, or no accessor can ever bind \
                     (FR-WS-19, S-381)"
                        .to_string(),
                );
            }
        }
        if let Some(properties) = &self.properties {
            if let Err(detail) = validate_properties(properties) {
                return bail(detail);
            }
        }
        Ok(())
    }
}


/// The `[wrapper_methods]` table's rule (S-588, [FR-RS-42]): a wrapper or
/// method name is matched exactly against one token of a space-joined
/// `peeled` column and a call's last path segment, so an empty or space-bearing
/// entry could never match — a descriptor bug.
///
/// Its own function for the reason [`validate_reach`] is: inline, it pushed
/// [`PluginManifest::validate`] to 53, past the `max_cc = 50` rule.
///
/// [FR-RS-42]: ../../../docs/specs/requirements/FR-RS-42.md
fn validate_wrapper_methods(declared: &BTreeMap<String, Vec<String>>) -> Result<(), String> {
    let bad = |t: &str| t.is_empty() || t.chars().any(char::is_whitespace);
    match declared
        .iter()
        .find(|(wrapper, methods)| bad(wrapper) || methods.iter().any(|m| bad(m)))
    {
        Some((wrapper, _)) => Err(format!(
            "`[wrapper_methods]` entry '{wrapper}' must name non-empty, space-free tokens"
        )),
        None => Ok(()),
    }
}

/// The `[reach]` table's rules ([FR-PL-09]): the level and the relation set must
/// agree, so a descriptor cannot declare `same-file` and still list `calls`, or
/// `resolved` and list nothing — and a relation is named once.
///
/// Its own function for the reason [`validate_package_modules`] is, and it takes
/// the `Option` so the call site is one branch: two at the call site pushed
/// [`PluginManifest::validate`] to 51, past the `max_cc = 50` rule.
///
/// [FR-PL-09]: ../../../docs/specs/requirements/FR-PL-09.md
fn validate_reach(reach: Option<&Reach>) -> Result<(), String> {
    let Some(reach) = reach else {
        return Ok(());
    };
    let binds_across_files = matches!(reach.level, ReachLevel::Resolved | ReachLevel::Partial);
    if binds_across_files && reach.cross_file.is_empty() {
        return Err(format!(
            "`[reach]` level '{}' must list at least one `cross_file` relation",
            reach.level.as_str()
        ));
    }
    if !binds_across_files && !reach.cross_file.is_empty() {
        return Err(format!(
            "`[reach]` level '{}' binds nothing across files, so `cross_file` must be empty",
            reach.level.as_str()
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    if let Some(dup) = reach.cross_file.iter().find(|r| !seen.insert(**r)) {
        return Err(format!(
            "`[reach]` `cross_file` names '{}' twice",
            dup.as_str()
        ));
    }
    Ok(())
}

/// The module-model declaration's rules ([`ModuleModel`], S-518): the declared
/// `kind` and the `[package_modules]` table must agree — `package` needs its
/// source roots, and roots under any other kind would be data nothing reads —
/// and the package model's roots pass [`validate_package_modules`].
///
/// One arm per kind, so a kind that gains data validates it here: the `path`
/// model's package stems and import roots (S-519) are refused under any other
/// kind, and the family — meaningful under every kind — is a bare token.
fn validate_module_model(
    model: Option<&ModuleModel>,
    package_modules: Option<&PackageModules>,
) -> Result<(), String> {
    let kind = resolved_module_model(model, package_modules);
    if let Some(m) = model {
        validate_path_model_data(m)?;
    }
    match (kind, package_modules) {
        (ModuleModelKind::Package, Some(pm)) => validate_package_modules(pm),
        (ModuleModelKind::Package, None) => Err(
            "`[module_model]` kind 'package' requires a `[package_modules]` table naming its \
             source roots"
                .to_string(),
        ),
        (other, Some(_)) => Err(format!(
            "`[package_modules]` is the data of the 'package' module model, but \
             `[module_model]` declares kind '{}'",
            other.as_str()
        )),
        (ModuleModelKind::Path | ModuleModelKind::Namespace, None) => Ok(()),
    }
}

/// The `[module_model]` keys beside `kind` (S-519): `package_stems` and
/// `import_roots` are the `path` model's data, and nothing else reads them; a
/// stem is a bare file stem, a root a relative `/`-separated directory, and the
/// family a bare token — an entry that could never match is a descriptor bug,
/// not data that silently matches nothing. `enclosing_namespaces` (S-595) is
/// the `namespace` model's data, and is refused under any other kind for the
/// same reason.
fn validate_path_model_data(m: &ModuleModel) -> Result<(), String> {
    if m.enclosing_namespaces && m.kind != ModuleModelKind::Namespace {
        return Err(format!(
            "`enclosing_namespaces` is the data of the 'namespace' module model, but \
             `[module_model]` declares kind '{}'",
            m.kind.as_str()
        ));
    }
    let path_data = !m.package_stems.is_empty() || m.import_roots.is_some();
    if path_data && m.kind != ModuleModelKind::Path {
        return Err(format!(
            "`package_stems` and `import_roots` are the data of the 'path' module model, but \
             `[module_model]` declares kind '{}'",
            m.kind.as_str()
        ));
    }
    if let Some(bad) = m
        .package_stems
        .iter()
        .find(|s| s.is_empty() || s.contains(['.', '/', '\\']))
    {
        return Err(format!(
            "`[module_model]` package stem '{bad}' must be a bare file stem (no `.`, `/` or `\\`)"
        ));
    }
    if m.import_roots.is_some() && m.package_stems.is_empty() {
        return Err(
            "`[module_model]` `import_roots` are detected by the package files beneath them, so \
             they require `package_stems`"
                .to_string(),
        );
    }
    if let Some(bad) = m
        .import_roots
        .iter()
        .flatten()
        .find(|r| !is_relative_dir(r))
    {
        return Err(format!(
            "`[module_model]` import root '{bad}' must be a relative `/`-separated directory \
             path (no empty segment, no leading or trailing `/`, no `..`)"
        ));
    }
    if let Some(family) = &m.family {
        if family.is_empty() || !family.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
            return Err(format!(
                "`[module_model]` family '{family}' must be a bare token (letters, digits, `_`, `-`)"
            ));
        }
    }
    Ok(())
}

/// Whether `dir` is a relative `/`-separated directory path whose every segment
/// can equal a path segment: no backslash, no empty segment, no `..`.
pub(crate) fn is_relative_dir(dir: &str) -> bool {
    !dir.contains('\\') && dir.split('/').all(|seg| !seg.is_empty() && seg != "..")
}

/// The module model a descriptor declares: its `[module_model]` kind, else
/// `package` when `[package_modules]` is declared, else `path` — the one
/// resolution of an omitted table, shared by validation and
/// [`PluginManifest::module_model_kind`].
fn resolved_module_model(
    model: Option<&ModuleModel>,
    package_modules: Option<&PackageModules>,
) -> ModuleModelKind {
    match (model, package_modules) {
        (Some(m), _) => m.kind,
        (None, Some(_)) => ModuleModelKind::Package,
        (None, None) => ModuleModelKind::Path,
    }
}

/// The `[package_modules]` table's rules (S-465, CR-149): at least one root to
/// strip, and each root matchable as whole path segments — so an entry that
/// could never equal a segment sequence (empty, absolute, `..`, a backslash, an
/// empty segment) is a descriptor bug, not a root that silently matches nothing.
///
/// Its own function for the reason [`validate_properties`] is: inline, these
/// rows pushed [`PluginManifest::validate`] past the `max_cc = 50` rule.
fn validate_package_modules(pm: &PackageModules) -> Result<(), String> {
    if pm.source_roots.is_empty() {
        return Err(
            "`[package_modules]` must declare at least one `source_roots` entry".to_string(),
        );
    }
    if let Some(bad) = pm.source_roots.iter().find(|r| !is_relative_dir(r)) {
        return Err(format!(
            "`[package_modules]` source root '{bad}' must be a relative `/`-separated \
             directory path (no empty segment, no leading or trailing `/`, no `..`)"
        ));
    }
    Ok(())
}

/// The `[properties]` table's typo and shape rules (S-381, S-398).
///
/// Extracted from [`PluginManifest::validate`] rather than inlined beside its
/// siblings: the row S-398 added pushed that function past the `max_cc = 50`
/// architecture rule, and one table's rules are the natural seam. The caller
/// attributes the returned detail to the descriptor file, so nothing here
/// names one.
fn validate_properties(properties: &PropertiesDescriptor) -> Result<(), String> {
        // An annotation row is compared against captured text verbatim, so a
        // stray space matches nothing and the arm degrades to silent
        // no-capture — the failure mode the `http_client_detectors` guard
        // above exists for, here on the binding arm.
        if properties.annotations.iter().any(|a| a.trim().is_empty()) {
            return Err("`[properties] annotations` entries must not be empty".to_string());
        }
        if properties.annotations.iter().any(|a| a != a.trim()) {
            return Err(
                "`[properties] annotations` entries must not carry surrounding whitespace"
                    .to_string(),
            );
        }
        // `""` IS a legal accessor prefix — it spells direct property access
        // — so the emptiness test that guards the annotation rows would be
        // wrong here. Whitespace is still a typo rather than a convention.
        if properties.accessor_prefixes.iter().any(|p| p != p.trim()) {
            return Err(
                "`[properties] accessor_prefixes` entries must not carry surrounding \
                 whitespace"
                    .to_string(),
            );
        }
        // A duplicated prefix would derive the same candidate twice. The
        // interpreter dedups by canonical key so it cannot fabricate an
        // ambiguity out of one, but a duplicate is a descriptor bug either
        // way and reads as intent.
        let mut seen = std::collections::BTreeSet::new();
        if let Some(dup) = properties
            .accessor_prefixes
            .iter()
            .find(|p| !seen.insert((*p).clone()))
        {
            return Err(format!(
                "`[properties] accessor_prefixes` carries the duplicate entry '{dup}'"
            ));
        }
        // Unlike `accessor_prefixes`, the EMPTY LIST is legal here — it is
        // the default, and it means "this language admits no qualified
        // receiver". An empty ENTRY is not: it would strip nothing and
        // leave any separator-led receiver reading as a field of the
        // enclosing class, which is the fabrication [NFR-RA-05] forbids.
        if properties.self_references.iter().any(|s| s.trim().is_empty()) {
            return Err(
                "`[properties] self_references` entries must not be empty (S-398)".to_string(),
            );
        }
        // A self reference is compared against captured receiver text
        // verbatim, so a stray space matches nothing and the qualifier
        // degrades to a silent refusal — the same failure mode the
        // `annotations` and `accessor_prefixes` guards above exist for.
        if properties.self_references.iter().any(|s| s != s.trim()) {
            return Err(
                "`[properties] self_references` entries must not carry surrounding whitespace"
                    .to_string(),
            );
        }
        // A qualifier is matched as a PREFIX of the receiver's text, so a row
        // that is not a self-contained word silently widens into something
        // else entirely: `["t"]` reduces `t.api` — a general `holder.api`
        // reduction, the exact thing this row's own documentation says it is
        // not — and `["this.x"]` reduces `this.x.api`. Neither is a spelling
        // any language uses for the enclosing instance, and both read as a
        // typo rather than as intent ([NFR-RA-05]).
        //
        // The shape is "letters, digits and `_`, not starting with a digit",
        // deliberately the SAME class the interpreter's own receiver
        // predicate uses, so a row that parses is a row the matcher can
        // recognise as a whole token. A sigil-led spelling (`$this`) is a
        // real convention this rule would refuse; no shipped language needs
        // one, and widening the rule when one does is a row in this comment
        // away — which is the direction that fails safe.
        if let Some(bad) = properties.self_references.iter().find(|s| {
            let mut chars = s.chars();
            !chars.next().is_some_and(|c| c.is_alphabetic() || c == '_')
                || !s.chars().all(|c| c.is_alphanumeric() || c == '_')
        }) {
            return Err(format!(
                "`[properties] self_references` entry '{bad}' is not a single word; a                      qualifier is matched as a prefix, so a partial one silently reduces                      an unrelated receiver (S-398, NFR-RA-05)"
            ));
        }
        let mut seen = std::collections::BTreeSet::new();
        if let Some(dup) = properties
            .self_references
            .iter()
            .find(|s| !seen.insert((*s).clone()))
        {
            return Err(format!(
                "`[properties] self_references` carries the duplicate entry '{dup}'"
            ));
        }
    Ok(())
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
        // Nor body node kinds (S-500, FR-EX-11): every callable is bodied.
        assert!(m.body_node_kinds.is_empty());
        assert_eq!(m.queries.get("symbols").unwrap(), "queries/symbols.scm");
        // The S-015 framework/export fields default to empty/All when omitted,
        // so pre-existing descriptors keep parsing unchanged (NFR-MA-01).
        assert!(m.framework_detectors.is_empty());
        assert!(m.http_client_detectors.is_empty());
        assert!(m.framework_methods.is_empty());
        assert!(m.invocation_methods.is_empty());
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
        // A descriptor that does not declare its specifier grammar keeps the
        // name grammar it always had (S-439, NFR-MA-01).
        assert_eq!(m.import_specifier, ImportSpecifier::Name);
        assert!(m.specifier_extensions.is_empty());
        // …and the default module model: no package-shaped path (CR-149), no
        // declared model, which resolves to `path` (S-518).
        assert!(m.package_modules.is_none());
        assert!(m.module_model.is_none());
        assert_eq!(m.module_model_kind(), ModuleModelKind::Path);
        // …and no declared reach: only a code language declares one (FR-PL-09).
        assert!(m.reach.is_none());
    }

    /// A `[reach]` table parses into its level and relation set, and a level
    /// that disagrees with its set — or a typo — fails loudly by file
    /// ([FR-PL-09]).
    ///
    /// [FR-PL-09]: ../../../docs/specs/requirements/FR-PL-09.md
    #[test]
    fn a_reach_declaration_parses_and_its_level_must_agree_with_its_relations() {
        let none = PluginManifest::parse("x/plugin.toml", GOOD).unwrap();
        assert!(none.reach.is_none(), "a descriptor with no `[reach]` has none");

        let java = format!(
            "{GOOD}\n[reach]\nlevel = \"resolved\"\ncross_file = [\"calls\", \"type_relations\"]\n"
        );
        let reach = PluginManifest::parse("java/plugin.toml", &java)
            .unwrap()
            .reach
            .unwrap();
        assert_eq!(reach.level, ReachLevel::Resolved);
        assert_eq!(
            reach.cross_file,
            [CrossFileRelation::Calls, CrossFileRelation::TypeRelations]
        );

        // The whole relation vocabulary parses and prints under the spelling the
        // docs and `logos languages` use — including the two no language declares
        // yet, which no fixture would otherwise pin.
        let every = format!(
            "{GOOD}\n[reach]\nlevel = \"resolved\"\ncross_file = [\"calls\", \"imports\", \
             \"type_relations\", \"member_access\", \"routes\"]\n"
        );
        let reach = PluginManifest::parse("x/plugin.toml", &every)
            .unwrap()
            .reach
            .unwrap();
        let spelled: Vec<&str> = reach.cross_file.iter().map(|r| r.as_str()).collect();
        assert_eq!(
            spelled,
            ["calls", "imports", "type_relations", "member_access", "routes"]
        );
        for (level, spelling) in [
            (ReachLevel::Resolved, "resolved"),
            (ReachLevel::Partial, "partial"),
            (ReachLevel::SameFile, "same-file"),
            (ReachLevel::Symbols, "symbols"),
        ] {
            assert_eq!(level.as_str(), spelling);
        }

        let scala = format!("{GOOD}\n[reach]\nlevel = \"same-file\"\n");
        let reach = PluginManifest::parse("scala/plugin.toml", &scala)
            .unwrap()
            .reach
            .unwrap();
        assert_eq!(reach.level, ReachLevel::SameFile);
        assert!(reach.cross_file.is_empty());

        for bad in [
            "level = \"same-file\"\ncross_file = [\"calls\"]",
            "level = \"symbols\"\ncross_file = [\"imports\"]",
            "level = \"resolved\"",
            "level = \"partial\"\ncross_file = []",
            "level = \"partial\"\ncross_file = [\"calls\", \"calls\"]",
            "level = \"full\"",
            "level = \"partial\"\ncross_file = [\"call\"]",
            "level = \"same-file\"\nrelations = []",
        ] {
            let toml = format!("{GOOD}\n[reach]\n{bad}\n");
            let err = PluginManifest::parse("go/plugin.toml", &toml)
                .unwrap_err()
                .to_string();
            assert!(err.contains("go/plugin.toml"), "{bad}: {err}");
        }
    }

    /// A package-shaped module path is descriptor data (CR-149, NFR-MA-01):
    /// the table parses into its roots, and a root that could never equal a
    /// path-segment sequence — or no root at all — fails loudly by file.
    #[test]
    fn a_package_shaped_module_path_is_declared_under_named_source_roots() {
        let java = format!(
            "{GOOD}\n[package_modules]\nsource_roots = [\"src/main/java\", \"src/test/java\"]\n"
        );
        let m = PluginManifest::parse("java/plugin.toml", &java).unwrap();
        assert_eq!(
            m.package_modules.map(|p| p.source_roots),
            Some(vec!["src/main/java".to_string(), "src/test/java".to_string()])
        );

        for bad in [
            "[]",
            "[\"\"]",
            "[\"/src\"]",
            "[\"src/\"]",
            "[\"src//java\"]",
            "[\"../src\"]",
            "[\"src\\\\java\"]",
        ] {
            let toml = format!("{GOOD}\n[package_modules]\nsource_roots = {bad}\n");
            let err = PluginManifest::parse("java/plugin.toml", &toml)
                .unwrap_err()
                .to_string();
            assert!(
                err.contains("java/plugin.toml") && err.contains("package_modules"),
                "{bad}: {err}"
            );
        }
        let unknown =
            format!("{GOOD}\n[package_modules]\nsource_roots = [\"src\"]\nroots = [\"x\"]\n");
        assert!(PluginManifest::parse("java/plugin.toml", &unknown).is_err());
    }

    /// The module model is one declaration naming the model (S-518, FR-RS-13):
    /// each kind parses, an omitted table resolves to `package` beside
    /// `[package_modules]` and `path` otherwise, and the kind and the package
    /// roots must agree — so a further model is one more kind, never a second
    /// table with its own precedence.
    #[test]
    fn the_module_model_is_one_declaration_naming_the_model() {
        let with = |extra: &str| PluginManifest::parse("x/plugin.toml", &format!("{GOOD}\n{extra}"));
        let roots = "[package_modules]\nsource_roots = [\"src/main/java\"]\n";

        let namespace = with("[module_model]\nkind = \"namespace\"\n").unwrap();
        assert_eq!(namespace.module_model_kind(), ModuleModelKind::Namespace);
        let path = with("[module_model]\nkind = \"path\"\n").unwrap();
        assert_eq!(path.module_model_kind(), ModuleModelKind::Path);
        let package = with(&format!("[module_model]\nkind = \"package\"\n{roots}")).unwrap();
        assert_eq!(package.module_model_kind(), ModuleModelKind::Package);
        // Omitted beside the roots: the package model, as every descriptor
        // written before the table declared it.
        let implied = with(roots).unwrap();
        assert!(implied.module_model.is_none());
        assert_eq!(implied.module_model_kind(), ModuleModelKind::Package);

        // The kind and the roots must agree, in both directions.
        let err = with("[module_model]\nkind = \"package\"\n").unwrap_err().to_string();
        assert!(err.contains("x/plugin.toml") && err.contains("package_modules"), "{err}");
        for kind in ["namespace", "path"] {
            let err = with(&format!("[module_model]\nkind = \"{kind}\"\n{roots}"))
                .unwrap_err()
                .to_string();
            assert!(err.contains(&format!("kind '{kind}'")), "{err}");
        }
        // The package model's roots are still validated through the table.
        let err = with("[module_model]\nkind = \"package\"\n[package_modules]\nsource_roots = []\n")
            .unwrap_err()
            .to_string();
        assert!(err.contains("at least one `source_roots`"), "{err}");
        // An unknown kind or key is refused loudly, never read as the default.
        assert!(with("[module_model]\nkind = \"modules\"\n").is_err());
        assert!(with("[module_model]\nkind = \"namespace\"\nroots = []\n").is_err());
        assert!(with("[module_model]\n").is_err(), "a table without a kind");
    }

    /// The namespace model's enclosing-namespace key (S-595, FR-RS-45) sits
    /// beside `kind`: it parses under `namespace`, defaults to off, and is
    /// refused under any other kind and when it is not a boolean.
    #[test]
    fn the_enclosing_namespaces_key_is_the_namespace_models_data() {
        let with = |extra: &str| PluginManifest::parse("x/plugin.toml", &format!("{GOOD}\n{extra}"));
        let on = with("[module_model]\nkind = \"namespace\"\nenclosing_namespaces = true\n").unwrap();
        assert!(on.enclosing_namespaces());
        let off = with("[module_model]\nkind = \"namespace\"\nenclosing_namespaces = false\n").unwrap();
        assert!(!off.enclosing_namespaces());
        // Omitted — the key, the table, or both — a language binds as before.
        assert!(!with("[module_model]\nkind = \"namespace\"\n").unwrap().enclosing_namespaces());
        assert!(!with("").unwrap().enclosing_namespaces());
        // A package or path model has no namespaces to enclose.
        let err = with("[module_model]\nkind = \"path\"\nenclosing_namespaces = true\n")
            .unwrap_err()
            .to_string();
        assert!(err.contains("x/plugin.toml") && err.contains("kind 'path'"), "{err}");
        let roots = "[package_modules]\nsource_roots = [\"src/main/java\"]\n";
        assert!(
            with(&format!("[module_model]\nkind = \"package\"\nenclosing_namespaces = true\n{roots}")).is_err()
        );
        assert!(with("[module_model]\nkind = \"namespace\"\nenclosing_namespaces = \"yes\"\n").is_err());
    }

    /// The path model's data (S-519, FR-RS-14) sits beside `kind` in the one
    /// table: stems and import roots parse under `path` and are refused under
    /// any other kind, a malformed entry fails loudly by file, and the family
    /// — any kind — defaults to the plugin's own name.
    #[test]
    fn the_path_models_stems_roots_and_family_are_one_tables_data() {
        let with = |extra: &str| PluginManifest::parse("x/plugin.toml", &format!("{GOOD}\n{extra}"));
        let python = with(
            "[module_model]\nkind = \"path\"\npackage_stems = [\"__init__\"]\nimport_roots = [\"src\", \"lib/py\"]\nfamily = \"python\"\n",
        )
        .unwrap();
        let model = python.module_model.as_ref().unwrap();
        assert_eq!(model.package_stems, ["__init__"]);
        assert_eq!(model.import_roots.as_deref(), Some(&["src".to_string(), "lib/py".to_string()][..]));
        assert_eq!(python.module_family(), "python");
        // Stems alone (Rust's), and an explicit empty root list (the
        // repository root only), are both a path model.
        let rust = with("[module_model]\nkind = \"path\"\npackage_stems = [\"mod\", \"lib\", \"main\"]\n").unwrap();
        assert_eq!(rust.module_model.as_ref().unwrap().import_roots, None);
        assert!(with("[module_model]\nkind = \"path\"\npackage_stems = [\"__init__\"]\nimport_roots = []\n").is_ok());
        // No family declared: the plugin's own name.
        assert_eq!(rust.module_family(), rust.name);
        // A family under another kind is fine; stems or roots are not.
        assert!(with("[module_model]\nkind = \"namespace\"\nfamily = \"jvm\"\n").is_ok());
        for bad in [
            "[module_model]\nkind = \"namespace\"\npackage_stems = [\"__init__\"]\n",
            "[module_model]\nkind = \"namespace\"\nimport_roots = [\"src\"]\n",
        ] {
            let err = with(bad).unwrap_err().to_string();
            assert!(err.contains("x/plugin.toml") && err.contains("'path' module model"), "{bad}: {err}");
        }
        for bad in [
            "package_stems = [\"\"]",
            "package_stems = [\"__init__.py\"]",
            "package_stems = [\"a/b\"]",
            "package_stems = [\"x\"]\nimport_roots = [\"../src\"]",
            "package_stems = [\"x\"]\nimport_roots = [\"/src\"]",
            "package_stems = [\"x\"]\nimport_roots = [\"src/\"]",
            "import_roots = [\"src\"]",
            "family = \"\"",
            "family = \"j vm\"",
        ] {
            let text = format!("[module_model]\nkind = \"path\"\n{bad}\n");
            let err = with(&text).unwrap_err().to_string();
            assert!(err.contains("x/plugin.toml") && err.contains("module_model"), "{bad}: {err}");
        }
    }

    /// The implicit-receiver policy (S-514) defaults to `none`, parses `self`,
    /// and refuses any other word loudly rather than reading it as the default.
    #[test]
    fn the_implicit_receiver_policy_defaults_to_none_and_parses_self() {
        let m = PluginManifest::parse("rust/plugin.toml", GOOD).unwrap();
        assert_eq!(m.implicit_receiver(), ImplicitReceiver::None);
        let declared = GOOD.replace(
            "module_separator = \"::\"",
            "module_separator = \"::\"\nimplicit_receiver = \"self\"",
        );
        let m = PluginManifest::parse("kotlin/plugin.toml", &declared).unwrap();
        assert_eq!(m.implicit_receiver(), ImplicitReceiver::SelfInstance);
        let explicit_none = GOOD.replace(
            "module_separator = \"::\"",
            "module_separator = \"::\"\nimplicit_receiver = \"none\"",
        );
        let m = PluginManifest::parse("kotlin/plugin.toml", &explicit_none).unwrap();
        assert_eq!(m.implicit_receiver(), ImplicitReceiver::None);
        for bad in ["\"this\"", "\"Self\"", "\"\"", "true"] {
            let text = GOOD.replace(
                "module_separator = \"::\"",
                &format!("module_separator = \"::\"\nimplicit_receiver = {bad}"),
            );
            assert!(PluginManifest::parse("x/plugin.toml", &text).is_err(), "{bad}");
        }
    }

    /// An explicit `implicit_receiver = "none"` (S-590) is told apart from the
    /// omitted key, though extraction reads both as `none`: only the explicit
    /// declaration makes a bare call bind free callables only, and `"self"`
    /// never does. Java omits the key, so it must stay `false`.
    #[test]
    fn an_explicit_none_is_told_apart_from_the_omitted_default() {
        let with = |line: &str| {
            let text = GOOD.replace(
                "module_separator = \"::\"",
                &format!("module_separator = \"::\"\n{line}"),
            );
            PluginManifest::parse("x/plugin.toml", &text).unwrap()
        };
        let omitted = PluginManifest::parse("java/plugin.toml", GOOD).unwrap();
        let explicit = with("implicit_receiver = \"none\"");
        let instance = with("implicit_receiver = \"self\"");
        assert_eq!(omitted.implicit_receiver(), explicit.implicit_receiver());
        assert!(!omitted.bare_calls_free_only(), "an omitted key is the default");
        assert!(explicit.bare_calls_free_only(), "an explicit `none` is declared");
        assert!(!instance.bare_calls_free_only(), "`self` reaches the instance");
    }

    /// The wrapper-method table (S-588) defaults empty, parses wrapper →
    /// methods, and refuses an entry that could never match a `peeled` token or
    /// a call's method name.
    #[test]
    fn the_wrapper_methods_table_defaults_empty_and_refuses_unmatchable_entries() {
        assert!(PluginManifest::parse("x/plugin.toml", GOOD).unwrap().wrapper_methods.is_empty());
        let with = |table: &str| PluginManifest::parse("x/plugin.toml", &format!("{GOOD}\n[wrapper_methods]\n{table}\n"));
        let m = with("Arc = [\"clone\", \"downgrade\"]\nBox = [\"as_mut\"]").unwrap();
        assert_eq!(m.wrapper_methods["Arc"], ["clone", "downgrade"]);
        assert_eq!(m.wrapper_methods["Box"], ["as_mut"]);
        for bad in ["Arc = [\"\"]", "Arc = [\"as ref\"]", "\"\" = [\"clone\"]", "\"Arc Rc\" = [\"clone\"]"] {
            assert!(with(bad).is_err(), "{bad}");
        }
    }

    /// The two call-target keys (S-521) default to `false` — a callable only —
    /// resolve independently into [`CallTargets`], and refuse a non-boolean
    /// rather than reading it as the default.
    #[test]
    fn the_call_target_keys_default_off_and_resolve_independently() {
        let m = PluginManifest::parse("rust/plugin.toml", GOOD).unwrap();
        assert_eq!(m.call_targets(), CallTargets::default());
        assert!(!m.call_targets().any());
        let with = |keys: &str| {
            let text = GOOD.replace(
                "module_separator = \"::\"",
                &format!("module_separator = \"::\"\n{keys}"),
            );
            PluginManifest::parse("x/plugin.toml", &text)
        };
        let python = with("class_call_instantiates = true").unwrap().call_targets();
        assert_eq!(python, CallTargets { classes: true, macros: false });
        let c = with("macros_callable = true").unwrap().call_targets();
        assert_eq!(c, CallTargets { classes: false, macros: true });
        let off = with("class_call_instantiates = false\nmacros_callable = false").unwrap();
        assert!(!off.call_targets().any());
        for bad in ["class_call_instantiates = \"yes\"", "macros_callable = 1"] {
            assert!(with(bad).is_err(), "{bad}");
        }
    }

    /// The supertype key (S-522) defaults to `false` — each clause names its
    /// kind — and refuses a non-boolean rather than reading it as the default.
    #[test]
    fn the_supertype_kind_key_defaults_off_and_refuses_a_non_boolean() {
        let m = PluginManifest::parse("rust/plugin.toml", GOOD).unwrap();
        assert!(!m.supertype_kind_follows_target);
        let with = |keys: &str| {
            let text = GOOD.replace(
                "module_separator = \"::\"",
                &format!("module_separator = \"::\"\n{keys}"),
            );
            PluginManifest::parse("x/plugin.toml", &text)
        };
        assert!(with("supertype_kind_follows_target = true").unwrap().supertype_kind_follows_target);
        assert!(!with("supertype_kind_follows_target = false").unwrap().supertype_kind_follows_target);
        assert!(with("supertype_kind_follows_target = \"yes\"").is_err());
    }

    /// The specifier grammar is declared apart from the member-path separator
    /// (S-439): `module_separator = "."` and `import_specifier = "path"` coexist,
    /// and stripping is a path-grammar-only rule over bare extensions.
    #[test]
    fn the_specifier_grammar_is_declared_apart_from_the_member_path_separator() {
        let ts = GOOD.replace(
            "module_separator = \"::\"",
            "module_separator = \".\"\nimport_specifier = \"path\"\nspecifier_extensions = [\"ts\", \"tsx\"]",
        );
        let m = PluginManifest::parse("typescript/plugin.toml", &ts).unwrap();
        assert_eq!(m.module_separator, ".");
        assert_eq!(m.import_specifier, ImportSpecifier::Path);
        assert_eq!(m.specifier_extensions, ["ts", "tsx"]);

        let on_a_name_grammar = GOOD.replace(
            "module_separator = \"::\"",
            "module_separator = \"::\"\nspecifier_extensions = [\"rs\"]",
        );
        let err = PluginManifest::parse("x/plugin.toml", &on_a_name_grammar)
            .unwrap_err()
            .to_string();
        assert!(err.contains("requires `import_specifier"), "{err}");

        for bad in ["\".ts\"", "\"\"", "\"a/b\""] {
            let toml = GOOD.replace(
                "module_separator = \"::\"",
                &format!("module_separator = \".\"\nimport_specifier = \"path\"\nspecifier_extensions = [{bad}]"),
            );
            let err = PluginManifest::parse("x/plugin.toml", &toml)
                .unwrap_err()
                .to_string();
            assert!(err.contains("must be a bare extension"), "{bad}: {err}");
        }
        let unknown = GOOD.replace(
            "module_separator = \"::\"",
            "module_separator = \".\"\nimport_specifier = \"url\"",
        );
        assert!(PluginManifest::parse("x/plugin.toml", &unknown).is_err());
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

    /// The consumer-side twin of [`empty_framework_detector_is_rejected`]: the
    /// same trap, one layer down (S-341, [CR-108]). An empty detector would
    /// prefix-match every reference and make every file an outbound-call
    /// candidate.
    #[test]
    fn empty_http_client_detector_is_rejected() {
        let toml = r#"
            name = "x"
            extensions = ["x"]
            module_separator = "."
            abi_version = 15
            capabilities = []
            http_client_detectors = [""]
        "#;
        let err = PluginManifest::parse("x/plugin.toml", toml).unwrap_err();
        assert!(err.to_string().contains("http_client_detectors"));
    }

    /// A detector is matched against canonical targets verbatim, so a stray
    /// space would silently disable the arm rather than fail — refuse it.
    #[test]
    fn untrimmed_http_client_detector_is_rejected() {
        let toml = r#"
            name = "x"
            extensions = ["x"]
            module_separator = "."
            abi_version = 15
            capabilities = []
            http_client_detectors = ["reqwest "]
        "#;
        let err = PluginManifest::parse("x/plugin.toml", toml).unwrap_err();
        assert!(err.to_string().contains("whitespace"), "got: {err}");
    }

    /// A declared table parses, including a dotted key (C#'s `HttpMethod.Get`),
    /// which TOML requires to be quoted (S-346). The empty default is pinned by
    /// [`parses_a_well_formed_descriptor`], beside its sibling fields.
    #[test]
    fn a_declared_invocation_methods_table_parses() {
        let toml = r#"
            name = "x"
            extensions = ["x"]
            module_separator = "."
            abi_version = 15
            capabilities = []
            [invocation_methods]
            GetAsync = "GET"
            "HttpMethod.Get" = "GET"
        "#;
        let m = PluginManifest::parse("x/plugin.toml", toml).expect("parses");
        assert_eq!(m.invocation_methods.get("GetAsync").unwrap(), "GET");
        assert_eq!(m.invocation_methods.get("HttpMethod.Get").unwrap(), "GET");
    }

    /// Declaring `invocations` with no detector set is a permanently no-op arm:
    /// the capability reads present while the product captures nothing. Refused,
    /// so honest absence cannot masquerade as presence ([FR-WS-08], [CR-108]).
    #[test]
    fn declaring_invocations_without_a_client_detector_is_rejected() {
        let toml = r#"
            name = "x"
            extensions = ["x"]
            module_separator = "."
            abi_version = 15
            capabilities = ["invocations"]
            [queries]
            invocations = "queries/invocations.scm"
        "#;
        let err = PluginManifest::parse("x/plugin.toml", toml).unwrap_err();
        assert!(err.to_string().contains("http_client_detectors"), "got: {err}");
    }

    /// The binding arm's twin of the guard above (S-381, [FR-WS-19]): a
    /// descriptor declaring `properties` with no vocabulary ships a query that
    /// matches and a filter that admits nothing, which reads as an absent
    /// mechanism rather than as a broken descriptor.
    ///
    /// [FR-WS-19]: ../../../docs/specs/requirements/FR-WS-19.md
    #[test]
    fn declaring_properties_without_a_vocabulary_is_rejected() {
        let head = r#"
            name = "x"
            extensions = ["x"]
            module_separator = "."
            abi_version = 15
            capabilities = ["properties"]
            [queries]
            properties = "queries/properties.scm"
        "#;
        let err = PluginManifest::parse("x/plugin.toml", head).unwrap_err();
        // Needle specific to THIS rule: every bail in the block mentions
        // `[properties]`, so that substring alone cannot tell a missing table
        // from an empty vocabulary — review proved it by swapping one rule's
        // message for another's and watching the case stay green.
        assert!(err.to_string().contains("requires a `[properties]` table"), "got: {err}");

        let empty = format!("{head}
            [properties]
            annotations = []
");
        let err = PluginManifest::parse("x/plugin.toml", &empty).unwrap_err();
        assert!(err.to_string().contains("at least one binding annotation"), "got: {err}");

        let no_convention = format!("{head}
            [properties]
            annotations = [\"A\"]
");
        let err = PluginManifest::parse("x/plugin.toml", &no_convention).unwrap_err();
        assert!(err.to_string().contains("accessor_prefixes` entry"), "got: {err}");
    }

    /// The whitespace and duplicate traps, and the one entry that must NOT be
    /// caught by them: `""` is a legal accessor prefix — it spells direct
    /// property access — so the emptiness rule that guards the annotation rows
    /// would be wrong on this list (S-381).
    #[test]
    fn the_properties_table_rejects_typos_but_admits_the_empty_accessor_prefix() {
        let with = |table: &str| {
            format!(
                r#"
                name = "x"
                extensions = ["x"]
                module_separator = "."
                abi_version = 15
                capabilities = []
                [queries]
                {table}
                "#
            )
        };
        let cases = [
            (r#"[properties]
                annotations = [""]"#, "must not be empty"),
            (r#"[properties]
                annotations = [" ConfigurationProperties"]"#, "surrounding whitespace"),
            (r#"[properties]
                annotations = ["A"]
                accessor_prefixes = ["get "]"#, "surrounding whitespace"),
            (r#"[properties]
                annotations = ["A"]
                accessor_prefixes = ["get", "get"]"#, "duplicate entry"),
            // The S-398 row. The empty ENTRY is a typo here even though the
            // empty LIST is this row's default — the two are not the same
            // claim, and the `accessor_prefixes` rule above is the reason the
            // distinction has to be asserted rather than assumed.
            (r#"[properties]
                annotations = ["A"]
                accessor_prefixes = ["get"]
                self_references = [""]"#, "self_references` entries must not be empty"),
            (r#"[properties]
                annotations = ["A"]
                accessor_prefixes = ["get"]
                self_references = ["this "]"#, "surrounding whitespace"),
            (r#"[properties]
                annotations = ["A"]
                accessor_prefixes = ["get"]
                self_references = ["this", "this"]"#, "duplicate entry"),
            // A qualifier is matched as a PREFIX, so a row that is not a whole
            // word reduces a receiver it does not name. `this.x` and `*` are the
            // shapes review reproduced turning into a general `holder.api`
            // reduction; `1st` pins the leading-digit half of the rule.
            (r#"[properties]
                annotations = ["A"]
                accessor_prefixes = ["get"]
                self_references = ["this.x"]"#, "is not a single word"),
            (r#"[properties]
                annotations = ["A"]
                accessor_prefixes = ["get"]
                self_references = ["*"]"#, "is not a single word"),
            (r#"[properties]
                annotations = ["A"]
                accessor_prefixes = ["get"]
                self_references = ["1st"]"#, "is not a single word"),
        ];
        for (table, needle) in cases {
            let err = PluginManifest::parse("x/plugin.toml", &with(table)).unwrap_err();
            assert!(err.to_string().contains(needle), "{table}\n  got: {err}");
        }

        let ok = PluginManifest::parse(
            "x/plugin.toml",
            &with(
                r#"[properties]
                   annotations = ["A"]
                   accessor_prefixes = ["", "get"]"#,
            ),
        )
        .expect("the empty prefix is direct property access, not a typo");
        let ok = ok.properties.expect("[properties]");
        assert_eq!(ok.accessor_prefixes, ["", "get"]);
        // …and an ABSENT `self_references` is the legal default, not a rejected
        // table: a language that admits no qualified receiver declares no row
        // (S-398). Asserted on the same admitted descriptor, so the default and
        // the typo rules above are proved not to overlap.
        assert!(
            ok.self_references.is_empty(),
            "an absent `self_references` defaults to the empty vocabulary",
        );
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
