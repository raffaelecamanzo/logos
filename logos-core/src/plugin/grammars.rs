//! The compiled-in grammar table ([FR-PL-01], [ADR-09]).
//!
//! Each [`GrammarEntry`] pairs a grammar's embedded `plugin.toml` + `.scm`
//! assets (via `include_str!`) with its `tree_sitter_language::LanguageFn`. The
//! table is assembled by [`compiled`] from cargo-feature-gated rows: the
//! default build links the Rust grammar (`lang-rust`); further languages append
//! one gated row each as their grammar crates land, touching no other core
//! source ([NFR-MA-01]).
//!
//! Storing the grammar as a [`LanguageFn`] (not a `tree_sitter::Language`) is
//! the mechanism that resolves the duplicate-symbol hazard ([NFR-PC-05],
//! [AR-04], tree-sitter #4209): the C runtime is linked exactly once by the
//! `tree-sitter` crate, and each grammar contributes only its own
//! `tree_sitter_<lang>` symbol, so linking N grammars never duplicates runtime
//! symbols.
//!
//! [FR-PL-01]: ../../../docs/specs/requirements/FR-PL-01.md
//! [NFR-MA-01]: ../../../docs/specs/requirements/NFR-MA-01.md
//! [NFR-PC-05]: ../../../docs/specs/requirements/NFR-PC-05.md
//! [ADR-09]: ../../../docs/specs/architecture/decisions/ADR-09.md
//! [AR-04]: ../../../docs/specs/architecture.md

use tree_sitter_language::LanguageFn;

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
    GrammarEntry {
        manifest_label: "yaml/plugin.toml",
        manifest_toml: include_str!("../../plugins/yaml/plugin.toml"),
        language: tree_sitter_yaml::LANGUAGE,
        embedded_queries: &[],
    }
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
    GrammarEntry {
        manifest_label: "dockerfile/plugin.toml",
        manifest_toml: include_str!("../../plugins/dockerfile/plugin.toml"),
        language: arborium_dockerfile::language(),
        embedded_queries: &[],
    }
}

/// The JSON data-format artifact grammar entry (S-063, [CR-010]).
///
/// Uses `tree_sitter_json::LANGUAGE`; `[config] section_kinds = ["pair"]`. The
/// producer of the parsed JSON trees the OpenAPI capstone (S-067) content-sniffs.
///
/// [CR-010]: ../../../docs/requests/CR-010-config-artifact-graph-layer.md
#[cfg(feature = "lang-json")]
fn json_entry() -> GrammarEntry {
    GrammarEntry {
        manifest_label: "json/plugin.toml",
        manifest_toml: include_str!("../../plugins/json/plugin.toml"),
        language: tree_sitter_json::LANGUAGE,
        embedded_queries: &[],
    }
}

/// The Makefile *artifact* grammar entry (S-064, [CR-010], [ADR-25]).
///
/// Structural extraction (no `.scm` queries); `[config] node_kind = "make_target"`
/// drives one anchor per rule. Grammar: `tree-sitter-make` — FR-CG-06's
/// highest-risk grammar; its preflight passed at ABI 14.
#[cfg(feature = "lang-make")]
fn makefile_entry() -> GrammarEntry {
    GrammarEntry {
        manifest_label: "makefile/plugin.toml",
        manifest_toml: include_str!("../../plugins/makefile/plugin.toml"),
        language: tree_sitter_make::LANGUAGE,
        embedded_queries: &[],
    }
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
    GrammarEntry {
        manifest_label: "toml/plugin.toml",
        manifest_toml: include_str!("../../plugins/toml/plugin.toml"),
        language: tree_sitter_toml_ng::LANGUAGE,
        embedded_queries: &[],
    }
}

/// The Shell *artifact* grammar entry (S-064, [CR-010], [ADR-25]).
///
/// Structural extraction (no `.scm` queries); `[config] node_kind =
/// "shell_function"` drives one anchor per function. Shell is the layer's
/// deliberate metric-neutral guard ([FR-CG-05]): `ShellFunction` is `is_non_code`,
/// so a shell-heavy repo moves no metric. Grammar: `tree-sitter-bash`.
#[cfg(feature = "lang-shell")]
fn shell_entry() -> GrammarEntry {
    GrammarEntry {
        manifest_label: "shell/plugin.toml",
        manifest_toml: include_str!("../../plugins/shell/plugin.toml"),
        language: tree_sitter_bash::LANGUAGE,
        embedded_queries: &[],
    }
}

/// The Python grammar entry ([FR-PL-01], S-015).
#[cfg(feature = "lang-python")]
fn python_entry() -> GrammarEntry {
    GrammarEntry {
        manifest_label: "python/plugin.toml",
        manifest_toml: include_str!("../../plugins/python/plugin.toml"),
        language: tree_sitter_python::LANGUAGE,
        embedded_queries: &[
            EmbeddedQuery {
                relative_path: "queries/symbols.scm",
                label: "python/queries/symbols.scm",
                source: include_str!("../../plugins/python/queries/symbols.scm"),
            },
            EmbeddedQuery {
                relative_path: "queries/references.scm",
                label: "python/queries/references.scm",
                source: include_str!("../../plugins/python/queries/references.scm"),
            },
            EmbeddedQuery {
                relative_path: "queries/frameworks.scm",
                label: "python/queries/frameworks.scm",
                source: include_str!("../../plugins/python/queries/frameworks.scm"),
            },
        ],
    }
}

/// The TypeScript grammar entry (`.ts`/`.js`, [FR-PL-01], S-015).
#[cfg(feature = "lang-typescript")]
fn typescript_entry() -> GrammarEntry {
    GrammarEntry {
        manifest_label: "typescript/plugin.toml",
        manifest_toml: include_str!("../../plugins/typescript/plugin.toml"),
        language: tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
        embedded_queries: &[
            EmbeddedQuery {
                relative_path: "queries/symbols.scm",
                label: "typescript/queries/symbols.scm",
                source: include_str!("../../plugins/typescript/queries/symbols.scm"),
            },
            EmbeddedQuery {
                relative_path: "queries/references.scm",
                label: "typescript/queries/references.scm",
                source: include_str!("../../plugins/typescript/queries/references.scm"),
            },
            EmbeddedQuery {
                relative_path: "queries/frameworks.scm",
                label: "typescript/queries/frameworks.scm",
                source: include_str!("../../plugins/typescript/queries/frameworks.scm"),
            },
        ],
    }
}

/// The TSX grammar entry (`.tsx`/`.jsx`, [FR-PL-01], S-015). Shares the
/// `lang-typescript` feature with [`typescript_entry`] but parses with the
/// distinct TSX `Language` and ships its own queries (JSX node kinds).
#[cfg(feature = "lang-typescript")]
fn tsx_entry() -> GrammarEntry {
    GrammarEntry {
        manifest_label: "tsx/plugin.toml",
        manifest_toml: include_str!("../../plugins/tsx/plugin.toml"),
        language: tree_sitter_typescript::LANGUAGE_TSX,
        embedded_queries: &[
            EmbeddedQuery {
                relative_path: "queries/symbols.scm",
                label: "tsx/queries/symbols.scm",
                source: include_str!("../../plugins/tsx/queries/symbols.scm"),
            },
            EmbeddedQuery {
                relative_path: "queries/references.scm",
                label: "tsx/queries/references.scm",
                source: include_str!("../../plugins/tsx/queries/references.scm"),
            },
            EmbeddedQuery {
                relative_path: "queries/frameworks.scm",
                label: "tsx/queries/frameworks.scm",
                source: include_str!("../../plugins/tsx/queries/frameworks.scm"),
            },
        ],
    }
}

/// The Go grammar entry ([FR-PL-01], S-015).
#[cfg(feature = "lang-go")]
fn go_entry() -> GrammarEntry {
    GrammarEntry {
        manifest_label: "go/plugin.toml",
        manifest_toml: include_str!("../../plugins/go/plugin.toml"),
        language: tree_sitter_go::LANGUAGE,
        embedded_queries: &[
            EmbeddedQuery {
                relative_path: "queries/symbols.scm",
                label: "go/queries/symbols.scm",
                source: include_str!("../../plugins/go/queries/symbols.scm"),
            },
            EmbeddedQuery {
                relative_path: "queries/references.scm",
                label: "go/queries/references.scm",
                source: include_str!("../../plugins/go/queries/references.scm"),
            },
            EmbeddedQuery {
                relative_path: "queries/frameworks.scm",
                label: "go/queries/frameworks.scm",
                source: include_str!("../../plugins/go/queries/frameworks.scm"),
            },
        ],
    }
}

/// The Java grammar entry ([FR-PL-01], S-015).
#[cfg(feature = "lang-java")]
fn java_entry() -> GrammarEntry {
    GrammarEntry {
        manifest_label: "java/plugin.toml",
        manifest_toml: include_str!("../../plugins/java/plugin.toml"),
        language: tree_sitter_java::LANGUAGE,
        embedded_queries: &[
            EmbeddedQuery {
                relative_path: "queries/symbols.scm",
                label: "java/queries/symbols.scm",
                source: include_str!("../../plugins/java/queries/symbols.scm"),
            },
            EmbeddedQuery {
                relative_path: "queries/references.scm",
                label: "java/queries/references.scm",
                source: include_str!("../../plugins/java/queries/references.scm"),
            },
            EmbeddedQuery {
                relative_path: "queries/frameworks.scm",
                label: "java/queries/frameworks.scm",
                source: include_str!("../../plugins/java/queries/frameworks.scm"),
            },
        ],
    }
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
    GrammarEntry {
        manifest_label: "c/plugin.toml",
        manifest_toml: include_str!("../../plugins/c/plugin.toml"),
        language: tree_sitter_c::LANGUAGE,
        embedded_queries: &[
            EmbeddedQuery {
                relative_path: "queries/symbols.scm",
                label: "c/queries/symbols.scm",
                source: include_str!("../../plugins/c/queries/symbols.scm"),
            },
            EmbeddedQuery {
                relative_path: "queries/references.scm",
                label: "c/queries/references.scm",
                source: include_str!("../../plugins/c/queries/references.scm"),
            },
        ],
    }
}

/// The Kotlin grammar entry (S-055, [CR-009]).
///
/// Uses `tree_sitter_kotlin_ng::LANGUAGE` — the maintained
/// `tree-sitter-grammars/tree-sitter-kotlin` crate exposed as a `LanguageFn` at
/// ABI 14, the same decoupling the five v1 code grammars use ([ADR-09]). Ships
/// the full three-query code-grammar set (symbols/references/frameworks),
/// exactly like [`java_entry`].
///
/// [CR-009]: ../../../docs/requests/CR-009-seven-language-plugins.md
#[cfg(feature = "lang-kotlin")]
fn kotlin_entry() -> GrammarEntry {
    GrammarEntry {
        manifest_label: "kotlin/plugin.toml",
        manifest_toml: include_str!("../../plugins/kotlin/plugin.toml"),
        language: tree_sitter_kotlin_ng::LANGUAGE,
        embedded_queries: &[
            EmbeddedQuery {
                relative_path: "queries/symbols.scm",
                label: "kotlin/queries/symbols.scm",
                source: include_str!("../../plugins/kotlin/queries/symbols.scm"),
            },
            EmbeddedQuery {
                relative_path: "queries/references.scm",
                label: "kotlin/queries/references.scm",
                source: include_str!("../../plugins/kotlin/queries/references.scm"),
            },
            EmbeddedQuery {
                relative_path: "queries/frameworks.scm",
                label: "kotlin/queries/frameworks.scm",
                source: include_str!("../../plugins/kotlin/queries/frameworks.scm"),
            },
        ],
    }
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
    GrammarEntry {
        manifest_label: "c-sharp/plugin.toml",
        manifest_toml: include_str!("../../plugins/c-sharp/plugin.toml"),
        language: tree_sitter_c_sharp::LANGUAGE,
        embedded_queries: &[
            EmbeddedQuery {
                relative_path: "queries/symbols.scm",
                label: "c-sharp/queries/symbols.scm",
                source: include_str!("../../plugins/c-sharp/queries/symbols.scm"),
            },
            EmbeddedQuery {
                relative_path: "queries/references.scm",
                label: "c-sharp/queries/references.scm",
                source: include_str!("../../plugins/c-sharp/queries/references.scm"),
            },
            EmbeddedQuery {
                relative_path: "queries/frameworks.scm",
                label: "c-sharp/queries/frameworks.scm",
                source: include_str!("../../plugins/c-sharp/queries/frameworks.scm"),
            },
        ],
    }
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
    GrammarEntry {
        manifest_label: "cpp/plugin.toml",
        manifest_toml: include_str!("../../plugins/cpp/plugin.toml"),
        language: tree_sitter_cpp::LANGUAGE,
        embedded_queries: &[
            EmbeddedQuery {
                relative_path: "queries/symbols.scm",
                label: "cpp/queries/symbols.scm",
                source: include_str!("../../plugins/cpp/queries/symbols.scm"),
            },
            EmbeddedQuery {
                relative_path: "queries/references.scm",
                label: "cpp/queries/references.scm",
                source: include_str!("../../plugins/cpp/queries/references.scm"),
            },
        ],
    }
}

/// The Ruby grammar entry (S-059, [CR-009], [FR-PL-07]).
#[cfg(feature = "lang-ruby")]
fn ruby_entry() -> GrammarEntry {
    GrammarEntry {
        manifest_label: "ruby/plugin.toml",
        manifest_toml: include_str!("../../plugins/ruby/plugin.toml"),
        language: tree_sitter_ruby::LANGUAGE,
        embedded_queries: &[
            EmbeddedQuery {
                relative_path: "queries/symbols.scm",
                label: "ruby/queries/symbols.scm",
                source: include_str!("../../plugins/ruby/queries/symbols.scm"),
            },
            EmbeddedQuery {
                relative_path: "queries/references.scm",
                label: "ruby/queries/references.scm",
                source: include_str!("../../plugins/ruby/queries/references.scm"),
            },
            EmbeddedQuery {
                relative_path: "queries/frameworks.scm",
                label: "ruby/queries/frameworks.scm",
                source: include_str!("../../plugins/ruby/queries/frameworks.scm"),
            },
        ],
    }
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
    GrammarEntry {
        manifest_label: "php/plugin.toml",
        manifest_toml: include_str!("../../plugins/php/plugin.toml"),
        language: tree_sitter_php::LANGUAGE_PHP,
        embedded_queries: &[
            EmbeddedQuery {
                relative_path: "queries/symbols.scm",
                label: "php/queries/symbols.scm",
                source: include_str!("../../plugins/php/queries/symbols.scm"),
            },
            EmbeddedQuery {
                relative_path: "queries/references.scm",
                label: "php/queries/references.scm",
                source: include_str!("../../plugins/php/queries/references.scm"),
            },
            EmbeddedQuery {
                relative_path: "queries/frameworks.scm",
                label: "php/queries/frameworks.scm",
                source: include_str!("../../plugins/php/queries/frameworks.scm"),
            },
        ],
    }
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
    GrammarEntry {
        manifest_label: "scala/plugin.toml",
        manifest_toml: include_str!("../../plugins/scala/plugin.toml"),
        language: tree_sitter_scala::LANGUAGE,
        embedded_queries: &[
            EmbeddedQuery {
                relative_path: "queries/symbols.scm",
                label: "scala/queries/symbols.scm",
                source: include_str!("../../plugins/scala/queries/symbols.scm"),
            },
            EmbeddedQuery {
                relative_path: "queries/references.scm",
                label: "scala/queries/references.scm",
                source: include_str!("../../plugins/scala/queries/references.scm"),
            },
        ],
    }
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
    GrammarEntry {
        manifest_label: "markdown/plugin.toml",
        manifest_toml: include_str!("../../plugins/markdown/plugin.toml"),
        language: tree_sitter_md::LANGUAGE,
        embedded_queries: &[],
    }
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
    GrammarEntry {
        manifest_label: "protobuf/plugin.toml",
        manifest_toml: include_str!("../../plugins/protobuf/plugin.toml"),
        language: tree_sitter_proto::LANGUAGE,
        embedded_queries: &[],
    }
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
    GrammarEntry {
        manifest_label: "terraform/plugin.toml",
        manifest_toml: include_str!("../../plugins/terraform/plugin.toml"),
        language: tree_sitter_hcl::LANGUAGE,
        embedded_queries: &[],
    }
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
    GrammarEntry {
        manifest_label: "graphql/plugin.toml",
        manifest_toml: include_str!("../../plugins/graphql/plugin.toml"),
        language: tree_sitter_graphql::LANGUAGE,
        embedded_queries: &[],
    }
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
    GrammarEntry {
        manifest_label: "sql/plugin.toml",
        manifest_toml: include_str!("../../plugins/sql/plugin.toml"),
        language: tree_sitter_sequel::LANGUAGE,
        embedded_queries: &[],
    }
}

/// The Rust grammar entry ([FR-PL-01]).
#[cfg(feature = "lang-rust")]
fn rust_entry() -> GrammarEntry {
    GrammarEntry {
        manifest_label: "rust/plugin.toml",
        manifest_toml: include_str!("../../plugins/rust/plugin.toml"),
        language: tree_sitter_rust::LANGUAGE,
        embedded_queries: &[
            EmbeddedQuery {
                relative_path: "queries/symbols.scm",
                label: "rust/queries/symbols.scm",
                source: include_str!("../../plugins/rust/queries/symbols.scm"),
            },
            EmbeddedQuery {
                relative_path: "queries/references.scm",
                label: "rust/queries/references.scm",
                source: include_str!("../../plugins/rust/queries/references.scm"),
            },
            EmbeddedQuery {
                relative_path: "queries/frameworks.scm",
                label: "rust/queries/frameworks.scm",
                source: include_str!("../../plugins/rust/queries/frameworks.scm"),
            },
        ],
    }
}

