//! The compiled-in grammar table ([FR-PL-01], [ADR-09]).
//!
//! Each [`GrammarEntry`] pairs a grammar's embedded `plugin.toml` + `.scm`
//! assets (via `include_str!`) with its `tree_sitter_language::LanguageFn`. The
//! table is assembled by [`compiled`] from cargo-feature-gated rows: the
//! default build links the Rust grammar (`lang-rust`); further languages append
//! one gated row each as their grammar crates land, touching no other core
//! source ([NFR-MA-01]). A row is declared with `entry!` and `query!`, which
//! build the label, relative path and `include_str!` source of each asset from
//! one language-directory literal and one query name ([CR-211]).
//!
//! Storing the grammar as a [`LanguageFn`] (not a `tree_sitter::Language`) is
//! the mechanism that resolves the duplicate-symbol hazard ([NFR-PC-05],
//! [AR-04], tree-sitter #4209): the C runtime is linked exactly once by the
//! `tree-sitter` crate, and each grammar contributes only its own
//! `tree_sitter_<lang>` symbol, so linking N grammars never duplicates runtime
//! symbols.
//!
//! [CR-211]: ../../../docs/requests/CR-211-grammar-entries-are-declared-once.md
//! [FR-PL-01]: ../../../docs/specs/requirements/FR-PL-01.md
//! [NFR-MA-01]: ../../../docs/specs/requirements/NFR-MA-01.md
//! [NFR-PC-05]: ../../../docs/specs/requirements/NFR-PC-05.md
//! [ADR-09]: ../../../docs/specs/architecture/decisions/ADR-09.md
//! [AR-04]: ../../../docs/specs/architecture.md

use tree_sitter_language::LanguageFn;

/// Declares one [`EmbeddedQuery`] from its language directory and its name.
///
/// `query!("rust", "symbols")` is the `queries/symbols.scm` asset of
/// `plugins/rust/`: the relative path, the `rust/queries/symbols.scm` label and
/// the `include_str!` source are all built from those two literals with
/// `concat!`, so the three cannot disagree ([CR-211]).
///
/// [CR-211]: ../../../docs/requests/CR-211-grammar-entries-are-declared-once.md
// Every use is behind a `lang-*` feature, and `query!` is used only by a grammar
// that ships `.scm` queries: a build with no such grammar (`--no-default-features`,
// or only structural ones like `lang-markdown`) has no call site. The macro is
// still the one declaration of the shape.
#[allow(unused_macros)]
macro_rules! query {
    ($lang:literal, $name:literal) => {
        EmbeddedQuery {
            relative_path: concat!("queries/", $name, ".scm"),
            label: concat!($lang, "/queries/", $name, ".scm"),
            source: include_str!(concat!("../../plugins/", $lang, "/queries/", $name, ".scm")),
        }
    };
}

/// Declares one [`GrammarEntry`]: the `plugins/<lang>/plugin.toml` descriptor
/// (label and `include_str!` from the one directory literal), the grammar's
/// `LanguageFn`, and the `query!` rows named in the optional list — none when
/// the grammar is structural (documentation / artifact) and ships no `.scm`
/// ([CR-211]).
///
/// [CR-211]: ../../../docs/requests/CR-211-grammar-entries-are-declared-once.md
// Every use is behind a `lang-*` feature, and `query!` is used only by a grammar
// that ships `.scm` queries: a build with no such grammar (`--no-default-features`,
// or only structural ones like `lang-markdown`) has no call site. The macro is
// still the one declaration of the shape.
#[allow(unused_macros)]
macro_rules! entry {
    ($lang:literal, $language:expr) => {
        entry!($lang, $language, [])
    };
    ($lang:literal, $language:expr, [$($name:literal),* $(,)?]) => {
        GrammarEntry {
            manifest_label: concat!($lang, "/plugin.toml"),
            manifest_toml: include_str!(concat!("../../plugins/", $lang, "/plugin.toml")),
            language: $language,
            embedded_queries: &[$(query!($lang, $name)),*],
        }
    };
}

/// One embedded `.scm` query asset shipped with a grammar.
#[derive(Debug, Clone, Copy)]
pub struct EmbeddedQuery {
    /// Path relative to the descriptor directory (matches a `[queries]` value),
    /// e.g. `"queries/symbols.scm"`. Also the suffix joined onto an override
    /// directory when resolving an on-disk shadow.
    pub relative_path: &'static str,
    /// Human-facing label for compile errors, e.g. `"rust/queries/symbols.scm"`.
    pub label: &'static str,
    /// The embedded query source.
    pub source: &'static str,
}

/// One compiled-in grammar: its descriptor, its `LanguageFn`, and its queries.
///
/// `Debug` is hand-written because `tree_sitter_language::LanguageFn` (an opaque
/// C function pointer) does not implement it.
#[derive(Clone, Copy)]
pub struct GrammarEntry {
    /// Embedded label of the descriptor, e.g. `"rust/plugin.toml"`.
    pub manifest_label: &'static str,
    /// The embedded `plugin.toml` text.
    pub manifest_toml: &'static str,
    /// The grammar's `LanguageFn` — version-decoupled from the workspace
    /// tree-sitter ([ADR-09]).
    pub language: LanguageFn,
    /// The embedded queries shipped with this grammar.
    pub embedded_queries: &'static [EmbeddedQuery],
}

impl std::fmt::Debug for GrammarEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GrammarEntry")
            .field("manifest_label", &self.manifest_label)
            .field("embedded_queries", &self.embedded_queries)
            .finish_non_exhaustive()
    }
}

/// The grammars linked into this build, in declaration order.
///
/// Returns a `Vec` (not a `const` slice) because membership is a compile-time
/// feature decision and each row is `cfg`-gated; the cost is one small
/// allocation at startup ([ADR-04] — built once).
// The pushes are `cfg`-gated: zero rows with `--no-default-features`, one per
// enabled `lang-*` feature otherwise. `vec![]` cannot express that, so the
// init-then-push shape is intentional here.
#[allow(clippy::vec_init_then_push)]
pub fn compiled() -> Vec<GrammarEntry> {
    #[allow(unused_mut)]
    let mut entries: Vec<GrammarEntry> = Vec::new();

    #[cfg(feature = "lang-rust")]
    entries.push(rust_entry());

    #[cfg(feature = "lang-python")]
    entries.push(python_entry());

    // One crate, two grammars (ADR-09): `.ts`/`.js` parse with the TypeScript
    // grammar, `.tsx`/`.jsx` with the TSX grammar (JSX changes the syntax, so
    // tree-sitter ships it as a distinct `Language`). Both rows ride the single
    // `lang-typescript` feature.
    #[cfg(feature = "lang-typescript")]
    entries.push(typescript_entry());
    #[cfg(feature = "lang-typescript")]
    entries.push(tsx_entry());

    #[cfg(feature = "lang-go")]
    entries.push(go_entry());

    #[cfg(feature = "lang-java")]
    entries.push(java_entry());

    #[cfg(feature = "lang-c")]
    entries.push(c_entry());
    #[cfg(feature = "lang-kotlin")]
    entries.push(kotlin_entry());
    #[cfg(feature = "lang-c-sharp")]
    entries.push(c_sharp_entry());
    #[cfg(feature = "lang-cpp")]
    entries.push(cpp_entry());
    #[cfg(feature = "lang-ruby")]
    entries.push(ruby_entry());
    #[cfg(feature = "lang-php")]
    entries.push(php_entry());
    #[cfg(feature = "lang-scala")]
    entries.push(scala_entry());

    // The markdown *documentation* grammar (S-033, CR-003, ADR-19): registered
    // through the same substrate as the code grammars, but its descriptor sets
    // `documentation = true` and declares no tagging queries — extraction walks
    // its `section` tree structurally (extract::doc) rather than running a
    // `symbols` query.
    #[cfg(feature = "lang-markdown")]
    entries.push(markdown_entry());

    // The config/artifact data-format grammars (S-063, [CR-010], [ADR-25]): each
    // sets `artifact = true` and a `[config]` section descriptor, so extraction
    // walks its mapping tree structurally (extract::config) into a `ConfigFile` +
    // depth-bounded `ConfigSection` tree rather than running a `symbols` query.
    // They carry no embedded queries, exactly like the markdown documentation
    // grammar — pure plugin data over the S-062 substrate ([NFR-MA-01]).
    //
    // [CR-010]: ../../../docs/requests/CR-010-config-artifact-graph-layer.md
    // [ADR-25]: ../../../docs/specs/architecture/decisions/ADR-25.md
    #[cfg(feature = "lang-yaml")]
    entries.push(yaml_entry());
    #[cfg(feature = "lang-json")]
    entries.push(json_entry());
    #[cfg(feature = "lang-toml")]
    entries.push(toml_entry());

    // The build-format *artifact* grammars (S-064, CR-010, ADR-25): like the
    // markdown documentation grammar, each routes structurally through
    // `extract::config` (descriptor `artifact = true`) and ships no tagging
    // queries — its typed anchors are pure descriptor data (`[config] node_kind`).
    #[cfg(feature = "lang-dockerfile")]
    entries.push(dockerfile_entry());

    #[cfg(feature = "lang-make")]
    entries.push(makefile_entry());

    #[cfg(feature = "lang-shell")]
    entries.push(shell_entry());

    // The schema-format *artifact* grammars (S-065, CR-010, ADR-25): registered
    // through the same substrate as the code/doc grammars, but their descriptors
    // set `artifact = true` and declare typed `[[config.anchors]]` rather than
    // tagging queries — extraction walks for the declared anchor node kinds
    // (`message`/`service`, the six GraphQL type definitions) structurally
    // (extract::config) and emits `ProtoMessage`/`ProtoService`/`GqlType` nodes.
    #[cfg(feature = "lang-protobuf")]
    entries.push(protobuf_entry());
    #[cfg(feature = "lang-graphql")]
    entries.push(graphql_entry());

    // The infra/artifact grammars (S-066, CR-010, ADR-25): registered through the
    // same substrate as the code grammars, but their descriptors set
    // `artifact = true` and declare no tagging queries — extraction is structural
    // (a ConfigFile root + per-format typed anchors) via `extract::config`, not a
    // `symbols` query. Like every other grammar, ABI is asserted at load.
    #[cfg(feature = "lang-terraform")]
    entries.push(terraform_entry());
    #[cfg(feature = "lang-sql")]
    entries.push(sql_entry());

    entries
}

/// The names of the compiled-in **code** grammars — every linked grammar whose
/// descriptor declares neither `documentation = true` nor `artifact = true`
/// ([ADR-19], [ADR-25]: the three plugin classes).
///
/// Read off the embedded descriptors rather than listed by hand, so a grammar
/// that lands or changes class is classified by its own `plugin.toml` and no
/// second roster can drift from it. A file in one of these languages is
/// **runnable source**; a file in any other (YAML, JSON, Markdown, SQL, …) is a
/// document or configuration. Used by `workspace status`'s member-kind
/// candidate hint ([FR-WS-32]), which lists members that hold API documents and
/// no runnable source.
///
/// Parsed once per process. A descriptor that fails to parse contributes no
/// name here; the same descriptor already fails
/// [`LanguageRegistry::load`](super::LanguageRegistry::load) loudly, which is
/// where that fault is reported.
///
/// [ADR-19]: ../../../docs/specs/architecture/decisions/ADR-19.md
/// [ADR-25]: ../../../docs/specs/architecture/decisions/ADR-25.md
/// [FR-WS-32]: ../../../docs/specs/requirements/FR-WS-32.md
pub fn code_language_names() -> &'static std::collections::BTreeSet<String> {
    static NAMES: std::sync::OnceLock<std::collections::BTreeSet<String>> =
        std::sync::OnceLock::new();
    NAMES.get_or_init(|| {
        compiled()
            .iter()
            .filter_map(|entry| {
                super::PluginManifest::parse(entry.manifest_label, entry.manifest_toml).ok()
            })
            .filter(|manifest| !manifest.documentation && !manifest.artifact)
            .map(|manifest| manifest.name)
            .collect()
    })
}

/// The YAML data-format artifact grammar entry (S-063, [CR-010]).
///
/// Uses `tree_sitter_yaml::LANGUAGE`; its descriptor sets `artifact = true` and
/// `[config] section_kinds = ["block_mapping_pair"]`, so a matched `.yml`/`.yaml`
/// file is extracted structurally by `extract::config`. Ships **no** embedded
/// queries (ABI is still asserted at load like every grammar).
///
/// [CR-010]: ../../../docs/requests/CR-010-config-artifact-graph-layer.md
#[cfg(feature = "lang-yaml")]
fn yaml_entry() -> GrammarEntry {
    entry!("yaml", tree_sitter_yaml::LANGUAGE)
}

/// The Dockerfile *artifact* grammar entry (S-064, [CR-010], [ADR-25]).
///
/// Structural extraction (no `.scm` queries), exactly like [`markdown_entry`];
/// the descriptor's `[config] node_kind = "dockerfile_stage"` drives one typed
/// anchor per build stage. Grammar: `arborium-dockerfile` — the modern
/// `LanguageFn` re-binding of the `tree_sitter_dockerfile` C grammar (the legacy
/// `tree-sitter-dockerfile` crate would pull a second tree-sitter runtime, the
/// [NFR-PC-05] duplicate-symbol hazard).
#[cfg(feature = "lang-dockerfile")]
fn dockerfile_entry() -> GrammarEntry {
    entry!("dockerfile", arborium_dockerfile::language())
}

/// The JSON data-format artifact grammar entry (S-063, [CR-010]).
///
/// Uses `tree_sitter_json::LANGUAGE`; `[config] section_kinds = ["pair"]`. The
/// producer of the parsed JSON trees the OpenAPI capstone (S-067) content-sniffs.
///
/// [CR-010]: ../../../docs/requests/CR-010-config-artifact-graph-layer.md
#[cfg(feature = "lang-json")]
fn json_entry() -> GrammarEntry {
    entry!("json", tree_sitter_json::LANGUAGE)
}

/// The Makefile *artifact* grammar entry (S-064, [CR-010], [ADR-25]).
///
/// Structural extraction (no `.scm` queries); `[config] node_kind = "make_target"`
/// drives one anchor per rule. Grammar: `tree-sitter-make` — FR-CG-06's
/// highest-risk grammar; its preflight passed at ABI 14.
#[cfg(feature = "lang-make")]
fn makefile_entry() -> GrammarEntry {
    entry!("makefile", tree_sitter_make::LANGUAGE)
}

/// The TOML data-format artifact grammar entry (S-063, [CR-010]).
///
/// Uses the maintained `tree_sitter_toml_ng::LANGUAGE`; `[config] section_kinds`
/// names `table`/`table_array_element`/`pair`. No `key_field` (the TOML grammar
/// exposes no field name on those nodes), so section names fall back to the
/// node's first source line.
///
/// [CR-010]: ../../../docs/requests/CR-010-config-artifact-graph-layer.md
#[cfg(feature = "lang-toml")]
fn toml_entry() -> GrammarEntry {
    entry!("toml", tree_sitter_toml_ng::LANGUAGE)
}

/// The Shell *artifact* grammar entry (S-064, [CR-010], [ADR-25]).
///
/// Structural extraction (no `.scm` queries); `[config] node_kind =
/// "shell_function"` drives one anchor per function. Shell is the layer's
/// deliberate metric-neutral guard ([FR-CG-05]): `ShellFunction` is `is_non_code`,
/// so a shell-heavy repo moves no metric. Grammar: `tree-sitter-bash`.
#[cfg(feature = "lang-shell")]
fn shell_entry() -> GrammarEntry {
    entry!("shell", tree_sitter_bash::LANGUAGE)
}

/// The Python grammar entry ([FR-PL-01], S-015).
#[cfg(feature = "lang-python")]
fn python_entry() -> GrammarEntry {
    entry!(
        "python",
        tree_sitter_python::LANGUAGE,
        [
            "symbols",
            "references",
            "frameworks",
            // The outbound HTTP client-call arm (S-344, [FR-WS-08], [CR-108]):
            // `requests`/`httpx` free functions and the session/client
            // receiver form.
            "invocations",
        ]
    )
}

/// The TypeScript grammar entry (`.ts`/`.js`, [FR-PL-01], S-015).
#[cfg(feature = "lang-typescript")]
fn typescript_entry() -> GrammarEntry {
    entry!(
        "typescript",
        tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
        [
            "symbols",
            "references",
            "frameworks",
            // The outbound HTTP client-call arm (S-343, [FR-WS-08]): the
            // consumer side that makes this language's routes bindable.
            "invocations",
        ]
    )
}

/// The TSX grammar entry (`.tsx`/`.jsx`, [FR-PL-01], S-015). Shares the
/// `lang-typescript` feature with [`typescript_entry`] but parses with the
/// distinct TSX `Language` and ships its own queries (JSX node kinds).
#[cfg(feature = "lang-typescript")]
fn tsx_entry() -> GrammarEntry {
    entry!(
        "tsx",
        tree_sitter_typescript::LANGUAGE_TSX,
        [
            "symbols",
            "references",
            "frameworks",
            // The outbound HTTP client-call arm (S-343, [FR-WS-08]): the
            // consumer side that makes this language's routes bindable.
            "invocations",
        ]
    )
}

/// The Go grammar entry ([FR-PL-01], S-015).
#[cfg(feature = "lang-go")]
fn go_entry() -> GrammarEntry {
    entry!(
        "go",
        tree_sitter_go::LANGUAGE,
        [
            "symbols",
            "references",
            "frameworks",
            // The outbound `net/http` client-call arm (S-345, [FR-WS-08],
            // [CR-108]): the consumer side of the route key `frameworks`
            // promotes on the provider side.
            "invocations",
            // The receiver-gated Kafka Streams topology form (S-408,
            // [CR-131] §3.2 A1). Go's whole broker arm — deliberately not the
            // Rust file's bare-verb patterns. Its CAPTURE half is fixture-pinned;
            // its OVER-capture half is measured, at 0 broker rows over the
            // reference estate's 262 real `.go` files. The query header says why
            // the two halves must not be reported as one number.
            "brokers",
        ]
    )
}

/// The Java grammar entry ([FR-PL-01], S-015).
#[cfg(feature = "lang-java")]
fn java_entry() -> GrammarEntry {
    entry!(
        "java",
        tree_sitter_java::LANGUAGE,
        [
            "symbols",
            "references",
            "frameworks",
            // The outbound HTTP client-call arm (S-341, [CR-108], [FR-WS-08]):
            // RestClient/WebClient fluent chains, the `java.net.http.HttpClient`
            // builder, and the plain receiver-method idiom.
            "invocations",
            // The message-broker publish/subscribe invocation arm (S-254,
            // [FR-WS-10]): a per-language `.scm` is the entire capture surface.
            "brokers",
            // The configuration-binding arm (S-381, [CR-121], [FR-WS-19]): the
            // `@ConfigurationProperties` class, its key prefix and the
            // properties it declares.
            "properties",
        ]
    )
}

/// The C grammar entry (S-056, [CR-009], the honesty fixture).
///
/// `tree-sitter-c` exposes its grammar as a `tree_sitter_language::LanguageFn`
/// (`LANGUAGE`) at the workspace ABI, so it rides the substrate and the
/// load-time ABI assertion with no second tree-sitter runtime ([ADR-09],
/// [NFR-PC-05]). Ships only `symbols` + `references` queries — no `frameworks`
/// (the honesty posture, [NFR-CC-04]).
///
/// [CR-009]: ../../../docs/requests/CR-009-seven-language-plugins.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
#[cfg(feature = "lang-c")]
fn c_entry() -> GrammarEntry {
    entry!("c", tree_sitter_c::LANGUAGE, ["symbols", "references",])
}

/// The Kotlin grammar entry (S-055, [CR-009]).
///
/// Uses `tree_sitter_kotlin_ng::LANGUAGE` — the maintained
/// `tree-sitter-grammars/tree-sitter-kotlin` crate exposed as a `LanguageFn` at
/// ABI 14, the same decoupling the five v1 code grammars use ([ADR-09]). Ships
/// the full code-grammar query set (symbols/references/frameworks) plus the
/// outbound HTTP client-call arm (`invocations`, S-342/[CR-108]) — exactly like
/// [`java_entry`], whose JVM client APIs Kotlin binds.
///
/// [CR-009]: ../../../docs/requests/CR-009-seven-language-plugins.md
/// [CR-108]: ../../../docs/requests/CR-108-per-language-http-client-call-capture.md
#[cfg(feature = "lang-kotlin")]
fn kotlin_entry() -> GrammarEntry {
    entry!(
        "kotlin",
        tree_sitter_kotlin_ng::LANGUAGE,
        [
            "symbols",
            "references",
            "frameworks",
            "invocations",
            // The configuration-binding arm (S-381, [CR-121], [FR-WS-19]) — the
            // second language on the binding substrate. THIS ROW is the whole of
            // what it cost `logos-core`: an asset registration, no interpreter,
            // no dispatch, no per-language branch ([NFR-MA-01]). The row is
            // mandatory, not incidental — a capability whose `[queries]` path has
            // no embedded source is a hard startup error (see `registry`) — so
            // "adding a language costs zero `logos-core` edits" would be false
            // here, and [NFR-MA-01]'s own Notes retired that exact over-claim
            // once already (S-364, over CR-108's AC7): the Measurable Target
            // concedes a grammar-registry binding and never claimed zero-touch
            // for registering a query against an existing grammar entry.
            "properties",
        ]
    )
}

/// The C# grammar entry (S-057, [CR-009]).
///
/// `tree_sitter_c_sharp::LANGUAGE` is a `LanguageFn` at ABI 15, so it rides the
/// substrate and load-time ABI assertion with no second tree-sitter runtime
/// ([ADR-09], [NFR-PC-05]). Ships the full three-query set
/// (symbols/references/frameworks) like the other code grammars.
///
/// [CR-009]: ../../../docs/requests/CR-009-seven-language-plugins.md
#[cfg(feature = "lang-c-sharp")]
fn c_sharp_entry() -> GrammarEntry {
    entry!(
        "c-sharp",
        tree_sitter_c_sharp::LANGUAGE,
        [
            "symbols",
            "references",
            "frameworks",
            // The outbound `HttpClient` client-call arm (S-346, [FR-WS-08],
            // [CR-108]): the consumer side of the route key `frameworks`
            // promotes on the provider side.
            "invocations",
        ]
    )
}

/// The C++ grammar entry (S-058, [CR-009]).
///
/// The largest grammar of the language-breadth set and its expected precision
/// floor: the preprocessor and templates produce constructs the resolver cannot
/// bind, which surface as missing edges, never fabricated ones ([NFR-RA-05]).
/// Owns `.h` headers (the fixed `.h` → C++ ownership rule) jointly with the C
/// plugin's `.c`-only claim. Uses `tree_sitter_cpp::LANGUAGE` (a `LanguageFn` at
/// ABI 14), so it rides the substrate and load-time ABI assertion unchanged
/// ([ADR-09]). Ships `symbols`/`references` queries; no `frameworks`
/// (C++ has no single dominant web framework to detect, [FR-FW-03]).
///
/// [CR-009]: ../../../docs/requests/CR-009-seven-language-plugins.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
#[cfg(feature = "lang-cpp")]
fn cpp_entry() -> GrammarEntry {
    entry!("cpp", tree_sitter_cpp::LANGUAGE, ["symbols", "references",])
}

/// The Ruby grammar entry (S-059, [CR-009], [FR-PL-07]).
#[cfg(feature = "lang-ruby")]
fn ruby_entry() -> GrammarEntry {
    entry!(
        "ruby",
        tree_sitter_ruby::LANGUAGE,
        [
            "symbols",
            "references",
            "frameworks",
            // The outbound Net::HTTP / Faraday client-call arm (S-347,
            // [FR-WS-08], [CR-108]): the consumer side of the route key
            // `frameworks` promotes on the provider side.
            "invocations",
        ]
    )
}

/// The PHP grammar entry (S-060, [CR-009], [FR-PL-01]).
///
/// Bound via `LANGUAGE_PHP` — the crate's **full `php` grammar**, not
/// `LANGUAGE_PHP_ONLY`: a `.php` file is HTML-with-embedded-`<?php … ?>`, so the
/// full grammar parses the markup as `text`/`text_interpolation` islands and the
/// PHP code around them, and the `symbols`/`references` queries still extract the
/// PHP subtree. `LANGUAGE_PHP_ONLY` would fail to parse any template file. ABI
/// 15, exposed as a `LanguageFn`, so it rides the substrate and the load-time ABI
/// assertion unchanged ([ADR-09]).
///
/// [CR-009]: ../../../docs/requests/CR-009-seven-language-plugins.md
#[cfg(feature = "lang-php")]
fn php_entry() -> GrammarEntry {
    entry!(
        "php",
        tree_sitter_php::LANGUAGE_PHP,
        ["symbols", "references", "frameworks", "invocations",]
    )
}

/// The Scala grammar entry (S-061, [CR-009], [FR-PL-07]).
///
/// The highest-risk grammar of the language-breadth set, gated on a
/// verification preflight (CRA-01). `tree-sitter-scala` 0.26 exposes its grammar
/// as a `tree_sitter_language::LanguageFn` (`LANGUAGE`) generated against ABI 15
/// — within the workspace tree-sitter 0.25 runtime's range, so it rides the
/// ADR-09 substrate and load-time ABI assertion unchanged, and the LanguageFn
/// decoupling keeps the C runtime linked once ([NFR-PC-05]). Ships the
/// `symbols`/`references` queries (no `frameworks`: Scala has no
/// dominant-framework detector in this increment — an honest absence).
#[cfg(feature = "lang-scala")]
fn scala_entry() -> GrammarEntry {
    entry!(
        "scala",
        tree_sitter_scala::LANGUAGE,
        ["symbols", "references",]
    )
}

/// The markdown documentation grammar entry (S-033, [CR-003], [ADR-19]).
///
/// Uses `tree_sitter_md::LANGUAGE` — the **block** grammar, whose nested
/// `section` nodes mirror the heading hierarchy (the `DocFile` → `DocSection`
/// `Contains` tree). It ships **no** embedded queries: documentation is
/// extracted structurally by `extract::doc`, not by a tagging query, so the
/// descriptor's `capabilities` list is empty and the registry compiles nothing
/// for it (ABI is still asserted at load like every other grammar).
///
/// [CR-003]: ../../../docs/requests/CR-003-documentation-graph-layer.md
/// [ADR-19]: ../../../docs/specs/architecture/decisions/ADR-19.md
#[cfg(feature = "lang-markdown")]
fn markdown_entry() -> GrammarEntry {
    entry!("markdown", tree_sitter_md::LANGUAGE)
}

/// The Protobuf schema grammar entry (S-065, [CR-010], [ADR-25]).
///
/// An **artifact** grammar: its descriptor sets `artifact = true` and declares
/// `[[config.anchors]]` (`message` → `ProtoMessage`, `service` → `ProtoService`)
/// rather than tagging queries, so extraction routes structurally through
/// `extract::config` and emits typed anchors hung off the `ConfigFile` root by
/// `Contains` only — no import/reference edges (those are [CR-011]'s scope). The
/// grammar exposes a `LanguageFn` (`LANGUAGE`) at ABI 15, so it rides the plugin
/// substrate and load-time ABI assertion unchanged ([ADR-09]). Ships **no**
/// embedded queries: extraction is structural, not query-driven.
///
/// [CR-010]: ../../../docs/requests/CR-010-config-artifact-graph-layer.md
/// [CR-011]: ../../../docs/requests/CR-011-cross-artifact-resolution.md
/// [ADR-25]: ../../../docs/specs/architecture/decisions/ADR-25.md
#[cfg(feature = "lang-protobuf")]
fn protobuf_entry() -> GrammarEntry {
    entry!("protobuf", tree_sitter_proto::LANGUAGE)
}

/// The Terraform/HCL artifact grammar entry (S-066, [CR-010], [ADR-25]).
///
/// Uses `tree_sitter_hcl::LANGUAGE`. Its descriptor sets `artifact = true` and
/// ships **no** embedded queries: extraction walks the HCL `block` tree
/// structurally into `ConfigFile` + `TfBlock` typed anchors (`extract::config`),
/// not via a tagging query. One grammar covers both `.tf` and `.tfvars` (CRA-02).
///
/// [CR-010]: ../../../docs/requests/CR-010-config-artifact-graph-layer.md
/// [ADR-25]: ../../../docs/specs/architecture/decisions/ADR-25.md
#[cfg(feature = "lang-terraform")]
fn terraform_entry() -> GrammarEntry {
    entry!("terraform", tree_sitter_hcl::LANGUAGE)
}

/// The GraphQL schema grammar entry (S-065, [CR-010], [ADR-25]).
///
/// An **artifact** grammar like [`protobuf_entry`]: its descriptor declares the
/// six GraphQL type-definition node kinds as `[[config.anchors]]`, each mapping
/// to a single `GqlType` node with a `payload` subtype (object / interface /
/// enum / input / union / scalar, [FR-CG-03]). `tree-sitter-graphql` is the
/// CR-010 **highest-risk** grammar; it nonetheless exposes a `LanguageFn`
/// (`LANGUAGE`) at ABI 15 and passes the verification preflight ([FR-CG-06],
/// recorded in `sprint-impl-10.md`). Ships **no** embedded queries.
///
/// [CR-010]: ../../../docs/requests/CR-010-config-artifact-graph-layer.md
/// [ADR-25]: ../../../docs/specs/architecture/decisions/ADR-25.md
/// [FR-CG-03]: ../../../docs/specs/requirements/FR-CG-03.md
/// [FR-CG-06]: ../../../docs/specs/requirements/FR-CG-06.md
#[cfg(feature = "lang-graphql")]
fn graphql_entry() -> GrammarEntry {
    entry!("graphql", tree_sitter_graphql::LANGUAGE)
}

/// The SQL artifact grammar entry (S-066, [CR-010], [ADR-25]), flagged
/// highest-risk.
///
/// Uses `tree_sitter_sequel::LANGUAGE`. Its descriptor sets `artifact = true`
/// and ships **no** embedded queries: extraction is conservative,
/// DDL-anchors-only, walking recognised `create_*` statements into a
/// `ConfigFile` root with `SqlObject` typed anchors and **skipping** anything it
/// cannot parse, so no node is fabricated for an unparsed dialect construct
/// ([NFR-RA-05]).
///
/// [CR-010]: ../../../docs/requests/CR-010-config-artifact-graph-layer.md
/// [ADR-25]: ../../../docs/specs/architecture/decisions/ADR-25.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
#[cfg(feature = "lang-sql")]
fn sql_entry() -> GrammarEntry {
    entry!("sql", tree_sitter_sequel::LANGUAGE)
}

/// The Rust grammar entry ([FR-PL-01]).
#[cfg(feature = "lang-rust")]
fn rust_entry() -> GrammarEntry {
    entry!(
        "rust",
        tree_sitter_rust::LANGUAGE,
        [
            "symbols",
            "references",
            "frameworks",
            "invocations",
            // The message-broker publish/subscribe invocation arm (S-291,
            // [FR-WS-10]): a per-language `.scm` is the entire capture surface.
            // Rust ships it alongside `reachability = true`, closing the
            // capability-matrix gap ([CR-081], [FR-WS-12] AC1).
            "brokers",
        ]
    )
}

#[cfg(test)]
mod tests {
    //! Pins [`compiled`] to its byte content, so the macros that declare the
    //! table ([CR-211]) cannot change a label, a path, an asset or the order.
    //!
    //! [CR-211]: ../../../docs/requests/CR-211-grammar-entries-are-declared-once.md

    use super::*;

    /// One expected row: the manifest and every query as `(path, label, byte
    /// length, FNV-1a 64 of the source)`. The hash is FNV-1a (specified, unlike
    /// `DefaultHasher`), so the pin does not drift with the toolchain.
    struct Row {
        /// Whether this row's `lang-*` feature is compiled into this build.
        feature: bool,
        manifest: (&'static str, usize, u64),
        queries: &'static [(&'static str, &'static str, usize, u64)],
    }

    fn fnv1a(text: &str) -> u64 {
        text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
        })
    }

    /// The first code grammars (query-carrying rows) of `compiled()`, in
    /// declaration order, for the full `lang-all` roster; rows whose feature is
    /// off are filtered out by the test. The table is split over this and the two
    /// functions below only to keep each under the `max_fn_lines` architecture
    /// rule.
    fn expected_code_first() -> Vec<Row> {
        vec![
            Row {
                feature: cfg!(feature = "lang-rust"),
                manifest: ("rust/plugin.toml", 10743, 0x75c6271b31b59fa9),
                queries: &[
                    (
                        "queries/symbols.scm",
                        "rust/queries/symbols.scm",
                        8134,
                        0x536601068697c442,
                    ),
                    (
                        "queries/references.scm",
                        "rust/queries/references.scm",
                        11761,
                        0x004f860f6b1f54c6,
                    ),
                    (
                        "queries/frameworks.scm",
                        "rust/queries/frameworks.scm",
                        2487,
                        0x64cc81a77baa5ad7,
                    ),
                    (
                        "queries/invocations.scm",
                        "rust/queries/invocations.scm",
                        19344,
                        0xaa78a87e61aece2c,
                    ),
                    (
                        "queries/brokers.scm",
                        "rust/queries/brokers.scm",
                        12999,
                        0x591536b77328dd73,
                    ),
                ],
            },
            Row {
                feature: cfg!(feature = "lang-python"),
                manifest: ("python/plugin.toml", 6154, 0xf8811a0062e2a0bf),
                queries: &[
                    (
                        "queries/symbols.scm",
                        "python/queries/symbols.scm",
                        5781,
                        0x43bbfe75770584d0,
                    ),
                    (
                        "queries/references.scm",
                        "python/queries/references.scm",
                        6514,
                        0x88711c3203ae5098,
                    ),
                    (
                        "queries/frameworks.scm",
                        "python/queries/frameworks.scm",
                        8705,
                        0x0861e98411a247b2,
                    ),
                    (
                        "queries/invocations.scm",
                        "python/queries/invocations.scm",
                        8235,
                        0x520b44ad31174e4a,
                    ),
                ],
            },
            Row {
                feature: cfg!(feature = "lang-typescript"),
                manifest: ("typescript/plugin.toml", 6033, 0x2b0a8f05d13aebce),
                queries: &[
                    (
                        "queries/symbols.scm",
                        "typescript/queries/symbols.scm",
                        4334,
                        0xe0100e6b86947a44,
                    ),
                    (
                        "queries/references.scm",
                        "typescript/queries/references.scm",
                        5135,
                        0x2dfd30409cdb5eec,
                    ),
                    (
                        "queries/frameworks.scm",
                        "typescript/queries/frameworks.scm",
                        3419,
                        0xd3840e469f367696,
                    ),
                    (
                        "queries/invocations.scm",
                        "typescript/queries/invocations.scm",
                        7405,
                        0x089e8311da95e6b0,
                    ),
                ],
            },
            Row {
                feature: cfg!(feature = "lang-typescript"),
                manifest: ("tsx/plugin.toml", 4970, 0x44b952aed393d0d5),
                queries: &[
                    (
                        "queries/symbols.scm",
                        "tsx/queries/symbols.scm",
                        4313,
                        0x59c4a5774eb77072,
                    ),
                    (
                        "queries/references.scm",
                        "tsx/queries/references.scm",
                        5385,
                        0xd84a3a8a6bfc5a2b,
                    ),
                    (
                        "queries/frameworks.scm",
                        "tsx/queries/frameworks.scm",
                        3405,
                        0xd136b44c359a0530,
                    ),
                    (
                        "queries/invocations.scm",
                        "tsx/queries/invocations.scm",
                        7399,
                        0x0b82a0e876891fea,
                    ),
                ],
            },
            Row {
                feature: cfg!(feature = "lang-go"),
                manifest: ("go/plugin.toml", 5634, 0x897e062307e504df),
                queries: &[
                    (
                        "queries/symbols.scm",
                        "go/queries/symbols.scm",
                        4012,
                        0x8959f36cfa3f5540,
                    ),
                    (
                        "queries/references.scm",
                        "go/queries/references.scm",
                        3906,
                        0xb0603177705c1d2c,
                    ),
                    (
                        "queries/frameworks.scm",
                        "go/queries/frameworks.scm",
                        1644,
                        0xd070e52fb3c6a054,
                    ),
                    (
                        "queries/invocations.scm",
                        "go/queries/invocations.scm",
                        15021,
                        0xe9a6a9e4760fdbd9,
                    ),
                    (
                        "queries/brokers.scm",
                        "go/queries/brokers.scm",
                        8256,
                        0x9e2060e44083c47d,
                    ),
                ],
            },
            Row {
                feature: cfg!(feature = "lang-java"),
                manifest: ("java/plugin.toml", 10265, 0x2a081daa4fe2642b),
                queries: &[
                    (
                        "queries/symbols.scm",
                        "java/queries/symbols.scm",
                        3061,
                        0xd2e3af6f3ca3f830,
                    ),
                    (
                        "queries/references.scm",
                        "java/queries/references.scm",
                        11390,
                        0x5cc2f49c507f30ea,
                    ),
                    (
                        "queries/frameworks.scm",
                        "java/queries/frameworks.scm",
                        21430,
                        0x3a9ed21dfa313290,
                    ),
                    (
                        "queries/invocations.scm",
                        "java/queries/invocations.scm",
                        32074,
                        0xa88f45b35591f683,
                    ),
                    (
                        "queries/brokers.scm",
                        "java/queries/brokers.scm",
                        28966,
                        0x1bcd6bbebe2d9d53,
                    ),
                    (
                        "queries/properties.scm",
                        "java/queries/properties.scm",
                        9419,
                        0x211576acec806b2b,
                    ),
                ],
            },
        ]
    }

    /// The remaining code grammars (query-carrying rows), after
    /// [`expected_code_first`].
    fn expected_code_rest() -> Vec<Row> {
        vec![
            Row {
                feature: cfg!(feature = "lang-c"),
                manifest: ("c/plugin.toml", 3486, 0xe08a7b0e8cf9b3ae),
                queries: &[
                    (
                        "queries/symbols.scm",
                        "c/queries/symbols.scm",
                        4322,
                        0xf2b7e98cb849a9d6,
                    ),
                    (
                        "queries/references.scm",
                        "c/queries/references.scm",
                        1147,
                        0x759a30fe84202ef7,
                    ),
                ],
            },
            Row {
                feature: cfg!(feature = "lang-kotlin"),
                manifest: ("kotlin/plugin.toml", 11780, 0x2344974eda8b1145),
                queries: &[
                    (
                        "queries/symbols.scm",
                        "kotlin/queries/symbols.scm",
                        5445,
                        0x3f656076b5781204,
                    ),
                    (
                        "queries/references.scm",
                        "kotlin/queries/references.scm",
                        8082,
                        0xa68ef19461008f1c,
                    ),
                    (
                        "queries/frameworks.scm",
                        "kotlin/queries/frameworks.scm",
                        18666,
                        0x186405a809ad43cc,
                    ),
                    (
                        "queries/invocations.scm",
                        "kotlin/queries/invocations.scm",
                        23346,
                        0x415d94584879ff9e,
                    ),
                    (
                        "queries/properties.scm",
                        "kotlin/queries/properties.scm",
                        6183,
                        0x6efef4607b409eca,
                    ),
                ],
            },
            Row {
                feature: cfg!(feature = "lang-c-sharp"),
                manifest: ("c-sharp/plugin.toml", 9500, 0x045c0a95b989be36),
                queries: &[
                    (
                        "queries/symbols.scm",
                        "c-sharp/queries/symbols.scm",
                        3269,
                        0x60af034d61675963,
                    ),
                    (
                        "queries/references.scm",
                        "c-sharp/queries/references.scm",
                        7569,
                        0x316d9e9cb22e1c3a,
                    ),
                    (
                        "queries/frameworks.scm",
                        "c-sharp/queries/frameworks.scm",
                        1378,
                        0x8f43797db8e09a4b,
                    ),
                    (
                        "queries/invocations.scm",
                        "c-sharp/queries/invocations.scm",
                        10265,
                        0xb463d6744cac16ec,
                    ),
                ],
            },
            Row {
                feature: cfg!(feature = "lang-cpp"),
                manifest: ("cpp/plugin.toml", 4274, 0xb979cfc3e48b2c87),
                queries: &[
                    (
                        "queries/symbols.scm",
                        "cpp/queries/symbols.scm",
                        7136,
                        0x4307e3e695c06048,
                    ),
                    (
                        "queries/references.scm",
                        "cpp/queries/references.scm",
                        4566,
                        0xc16992938c69add3,
                    ),
                ],
            },
            Row {
                feature: cfg!(feature = "lang-ruby"),
                manifest: ("ruby/plugin.toml", 5404, 0x0e2423b98884f23b),
                queries: &[
                    (
                        "queries/symbols.scm",
                        "ruby/queries/symbols.scm",
                        2885,
                        0x2adb5e37cd35671b,
                    ),
                    (
                        "queries/references.scm",
                        "ruby/queries/references.scm",
                        7465,
                        0x661ea66aedb840cf,
                    ),
                    (
                        "queries/frameworks.scm",
                        "ruby/queries/frameworks.scm",
                        1865,
                        0x714c857c769f6c11,
                    ),
                    (
                        "queries/invocations.scm",
                        "ruby/queries/invocations.scm",
                        9149,
                        0x82f0520da4ac95a7,
                    ),
                ],
            },
            Row {
                feature: cfg!(feature = "lang-php"),
                manifest: ("php/plugin.toml", 6446, 0x4cbd56222bb1566e),
                queries: &[
                    (
                        "queries/symbols.scm",
                        "php/queries/symbols.scm",
                        3417,
                        0x42d85e53eae99587,
                    ),
                    (
                        "queries/references.scm",
                        "php/queries/references.scm",
                        7103,
                        0x6a6b43107a598435,
                    ),
                    (
                        "queries/frameworks.scm",
                        "php/queries/frameworks.scm",
                        1790,
                        0xaaa6fec47a6f5a1b,
                    ),
                    (
                        "queries/invocations.scm",
                        "php/queries/invocations.scm",
                        9935,
                        0x4f4c4c87873fe722,
                    ),
                ],
            },
            Row {
                feature: cfg!(feature = "lang-scala"),
                manifest: ("scala/plugin.toml", 5658, 0xf443858e00b869e4),
                queries: &[
                    (
                        "queries/symbols.scm",
                        "scala/queries/symbols.scm",
                        4437,
                        0x61b8318965c105dd,
                    ),
                    (
                        "queries/references.scm",
                        "scala/queries/references.scm",
                        5441,
                        0x464eaf86327ec71b,
                    ),
                ],
            },
        ]
    }

    /// The documentation, data-format, build-format, schema and infra grammars
    /// (no queries) of `compiled()`, in declaration order, after the code rows.
    fn expected_structural() -> Vec<Row> {
        vec![
            Row {
                feature: cfg!(feature = "lang-markdown"),
                manifest: ("markdown/plugin.toml", 2636, 0x221a1f670cc6402d),
                queries: &[],
            },
            Row {
                feature: cfg!(feature = "lang-yaml"),
                manifest: ("yaml/plugin.toml", 2686, 0xf4e095550ebb3643),
                queries: &[],
            },
            Row {
                feature: cfg!(feature = "lang-json"),
                manifest: ("json/plugin.toml", 1826, 0x87b608286a0bfe75),
                queries: &[],
            },
            Row {
                feature: cfg!(feature = "lang-toml"),
                manifest: ("toml/plugin.toml", 2005, 0x618e5b616b9a375f),
                queries: &[],
            },
            Row {
                feature: cfg!(feature = "lang-dockerfile"),
                manifest: ("dockerfile/plugin.toml", 2386, 0xc13030747a92973d),
                queries: &[],
            },
            Row {
                feature: cfg!(feature = "lang-make"),
                manifest: ("makefile/plugin.toml", 1808, 0x90de25c0cc1b9c48),
                queries: &[],
            },
            Row {
                feature: cfg!(feature = "lang-shell"),
                manifest: ("shell/plugin.toml", 1699, 0xa113b9773bdb2cc9),
                queries: &[],
            },
            Row {
                feature: cfg!(feature = "lang-protobuf"),
                manifest: ("protobuf/plugin.toml", 2674, 0x19b9d9b4f458e557),
                queries: &[],
            },
            Row {
                feature: cfg!(feature = "lang-graphql"),
                manifest: ("graphql/plugin.toml", 2838, 0x669d1b39dd8c99f4),
                queries: &[],
            },
            Row {
                feature: cfg!(feature = "lang-terraform"),
                manifest: ("terraform/plugin.toml", 2689, 0x92267414d9f19962),
                queries: &[],
            },
            Row {
                feature: cfg!(feature = "lang-sql"),
                manifest: ("sql/plugin.toml", 2216, 0x1e818770e6d819cb),
                queries: &[],
            },
        ]
    }

    /// Every grammar row in `compiled()` declaration order.
    fn expected() -> Vec<Row> {
        let mut rows = expected_code_first();
        rows.extend(expected_code_rest());
        rows.extend(expected_structural());
        rows
    }

    #[test]
    fn compiled_entries_are_byte_identical_to_the_pinned_table() {
        let want: Vec<Row> = expected().into_iter().filter(|row| row.feature).collect();
        let got = compiled();
        assert_eq!(
            got.iter().map(|e| e.manifest_label).collect::<Vec<_>>(),
            want.iter().map(|r| r.manifest.0).collect::<Vec<_>>(),
            "entry labels or order drifted"
        );
        for (entry, row) in got.iter().zip(&want) {
            let (label, len, hash) = row.manifest;
            assert_eq!(entry.manifest_toml.len(), len, "{label}: manifest length");
            assert_eq!(fnv1a(entry.manifest_toml), hash, "{label}: manifest bytes");
            assert_eq!(
                entry
                    .embedded_queries
                    .iter()
                    .map(|q| (q.relative_path, q.label))
                    .collect::<Vec<_>>(),
                row.queries.iter().map(|q| (q.0, q.1)).collect::<Vec<_>>(),
                "{label}: query paths, labels or order drifted"
            );
            for (query, &(_, qlabel, qlen, qhash)) in entry.embedded_queries.iter().zip(row.queries)
            {
                assert_eq!(query.source.len(), qlen, "{qlabel}: source length");
                assert_eq!(fnv1a(query.source), qhash, "{qlabel}: source bytes");
            }
        }
    }
}
