//! The [`LanguageRegistry`] — the plugin micro-kernel ([plugin-registry], [ADR-09]).
//!
//! At startup the registry walks the compiled-in grammar table
//! ([`super::grammars::compiled`]) and, for each grammar:
//!
//! 1. parses its embedded `plugin.toml` ([FR-PL-02]);
//! 2. builds the `Language` from its `LanguageFn` and asserts ABI — a mismatch
//!    is skipped-and-warned, never fatal ([FR-PL-03], [NFR-PC-03]);
//! 3. resolves each capability's query (on-disk override shadows embedded) and
//!    compiles it, failing fast and naming the file on error ([FR-PL-02],
//!    [FR-PL-04]);
//! 4. indexes the loaded grammar by extension for `for_extension` lookups.
//!
//! Built once per process ([ADR-04]); thereafter every lookup is a hash probe.
//!
//! [plugin-registry]: ../../../docs/specs/architecture/components/plugin-registry.md
//! [ADR-09]: ../../../docs/specs/architecture/decisions/ADR-09.md
//! [ADR-04]: ../../../docs/specs/architecture/decisions/ADR-04.md
//! [FR-PL-02]: ../../../docs/specs/requirements/FR-PL-02.md
//! [FR-PL-03]: ../../../docs/specs/requirements/FR-PL-03.md
//! [FR-PL-04]: ../../../docs/specs/requirements/FR-PL-04.md
//! [NFR-PC-03]: ../../../docs/specs/requirements/NFR-PC-03.md

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use tree_sitter::Language;

use super::abi::{assert_abi, AbiRange};
use super::error::{PluginError, SkippedGrammar};
use super::grammars::{self, GrammarEntry};
use super::manifest::PluginManifest;
use super::plugin::{CompiledPlugin, LanguagePlugin};
use super::queries;

/// The in-memory registry of loaded language grammars.
#[derive(Debug)]
pub struct LanguageRegistry {
    /// Loaded plugins in declaration order (the order `languages` lists them).
    plugins: Vec<CompiledPlugin>,
    /// Normalised extension → index into `plugins`.
    by_extension: HashMap<String, usize>,
    /// Basename claims in declaration order: `(claim, plugin index)` (S-062,
    /// [CR-010], [FR-CG-01]). Kept as an ordered list rather than a map so both
    /// the exact and the `Name.*` prefix lookup in [`for_path`](LanguageRegistry::for_path)
    /// are deterministic (first declaration wins), and so the prefix scan is a
    /// simple ordered walk — the claim set is tiny (≤ a few per artifact plugin).
    ///
    /// [CR-010]: ../../../docs/requests/CR-010-config-artifact-graph-layer.md
    /// [FR-CG-01]: ../../../docs/specs/requirements/FR-CG-01.md
    filename_claims: Vec<(String, usize)>,
    /// Grammars skipped at load (ABI mismatch), recorded for `languages` and
    /// diagnostics ([FR-PL-03]).
    skipped: Vec<SkippedGrammar>,
}

impl LanguageRegistry {
    /// Load every compiled-in grammar, resolving on-disk overrides under
    /// `project_root` and asserting ABI against the linked tree-sitter runtime.
    ///
    /// ABI mismatches are skipped with a warning to stderr; a malformed
    /// descriptor or a query that fails to compile is a hard error naming the
    /// file ([FR-PL-02]).
    ///
    /// # Errors
    /// Returns [`PluginError`] on a descriptor parse error, a query compile
    /// error, or an unreadable override file.
    pub fn load(project_root: impl AsRef<Path>) -> Result<Self, PluginError> {
        Self::load_from(
            &grammars::compiled(),
            AbiRange::runtime(),
            Some(project_root.as_ref()),
            // Warnings route through the single tracing seam — never a direct
            // print (FR-OB-01, NFR-OO-01); stderr rendering is the fmt layer's.
            &mut |w| tracing::warn!("{w}"),
        )
    }

    /// The seam under [`load`](Self::load): explicit grammar table, ABI range,
    /// optional override root, and a warning sink.
    ///
    /// Tests drive this directly to force an ABI mismatch (narrow range or a
    /// disagreeing descriptor) and to capture warnings, without a second build
    /// artifact ([UAT-PL-02]).
    ///
    /// # Errors
    /// As [`load`](Self::load).
    pub(crate) fn load_from(
        entries: &[GrammarEntry],
        abi_range: AbiRange,
        project_root: Option<&Path>,
        warn: &mut dyn FnMut(&str),
    ) -> Result<Self, PluginError> {
        let mut plugins: Vec<CompiledPlugin> = Vec::new();
        let mut by_extension: HashMap<String, usize> = HashMap::new();
        let mut filename_claims: Vec<(String, usize)> = Vec::new();
        let mut skipped: Vec<SkippedGrammar> = Vec::new();

        for entry in entries {
            let manifest = PluginManifest::parse(entry.manifest_label, entry.manifest_toml)?;

            // Build the Language from its LanguageFn and assert ABI before use.
            let language: Language = entry.language.into();
            let compiled_abi = language.abi_version();
            if let Err(reason) = assert_abi(manifest.abi_version, compiled_abi, &abi_range) {
                let skip = SkippedGrammar {
                    name: manifest.name.clone(),
                    reason,
                };
                warn(&skip.to_string());
                skipped.push(skip);
                continue; // skip only this grammar — the run is not aborted
            }

            let override_dir = project_root.map(|root| override_dir_for(root, &manifest.name));

            let (queries, overridden) =
                compile_capabilities(entry, &manifest, &language, override_dir.as_deref())?;

            let plugin = CompiledPlugin::new(manifest, language, queries, overridden);

            let idx = plugins.len();
            for ext in plugin.extensions() {
                // Last declaration wins on an extension collision; warn so the
                // shadowing is visible rather than silent.
                if let Some(prev) = by_extension.insert(normalize_ext(ext), idx) {
                    warn(&format!(
                        "extension '{ext}' claimed by '{}' shadows '{}'",
                        plugin.name(),
                        plugins[prev].name()
                    ));
                }
            }
            // Basename claims (S-062, CR-010, FR-CG-01): recorded in declaration
            // order so `for_path`'s exact + `Name.*` prefix lookup is deterministic
            // (first declaration wins). A collision is surfaced as a warning, like
            // the extension case.
            for fname in plugin.filenames() {
                if let Some((_, prev)) = filename_claims.iter().find(|(c, _)| c == fname) {
                    warn(&format!(
                        "filename '{fname}' claimed by '{}' also claimed by '{}'",
                        plugin.name(),
                        plugins[*prev].name()
                    ));
                }
                filename_claims.push((fname.clone(), idx));
            }
            plugins.push(plugin);
        }

        Ok(Self {
            plugins,
            by_extension,
            filename_claims,
            skipped,
        })
    }

    /// The plugin that claims `ext` (with or without a leading dot, any case).
    pub fn for_extension(&self, ext: &str) -> Option<&dyn LanguagePlugin> {
        self.by_extension
            .get(&normalize_ext(ext))
            .map(|&i| &self.plugins[i] as &dyn LanguagePlugin)
    }

    /// The plugin that claims `rel` by its **extension or basename** (S-062,
    /// [CR-010], [FR-CG-01], [FR-IX-02] as modified).
    ///
    /// Resolution order, extension first so a code/doc file is unaffected:
    /// 1. the file's extension (`for_extension`);
    /// 2. else the **exact** basename among the `filenames` claims (so
    ///    `Dockerfile` binds the Dockerfile plugin);
    /// 3. else the documented **`Name.*` prefix** rule — a basename `Name.<rest>`
    ///    matches a claim `Name` (so `Dockerfile.dev` binds the same plugin).
    ///
    /// Returns the first match in declaration order, so the lookup is
    /// deterministic ([NFR-RA-06]).
    ///
    /// [CR-010]: ../../../docs/requests/CR-010-config-artifact-graph-layer.md
    /// [FR-CG-01]: ../../../docs/specs/requirements/FR-CG-01.md
    /// [FR-IX-02]: ../../../docs/specs/requirements/FR-IX-02.md
    pub fn for_path(&self, rel: &str) -> Option<&dyn LanguagePlugin> {
        let path = Path::new(rel);
        if let Some(plugin) = path
            .extension()
            .and_then(|e| e.to_str())
            .and_then(|ext| self.for_extension(ext))
        {
            return Some(plugin);
        }
        let base = path.file_name().and_then(|b| b.to_str())?;
        // Exact basename claim first, then the `Name.*` prefix rule.
        let idx = self
            .filename_claims
            .iter()
            .find(|(claim, _)| claim == base)
            .or_else(|| {
                self.filename_claims.iter().find(|(claim, _)| {
                    base.len() > claim.len()
                        && base.starts_with(claim.as_str())
                        && base.as_bytes()[claim.len()] == b'.'
                })
            })
            .map(|&(_, i)| i)?;
        Some(&self.plugins[idx] as &dyn LanguagePlugin)
    }

    /// All loaded plugins in declaration order (the `languages` listing order).
    pub fn iter(&self) -> impl Iterator<Item = &dyn LanguagePlugin> {
        self.plugins.iter().map(|p| p as &dyn LanguagePlugin)
    }

    /// The set of file extensions (normalised: lower-case, no leading dot) whose
    /// loaded plugin declares the **reachability capability** (S-159, [CR-043],
    /// [ADR-39]). The [annotation-engine] gates Pass-3 dead-code reachability on
    /// it: a callable whose extension is absent renders `is_dead = NULL` ("not
    /// computed", [NFR-CC-04]) rather than a fabricated verdict. Empty when no
    /// loaded grammar declares it — every callable then renders NULL, the honest
    /// degraded state.
    ///
    /// [CR-043]: ../../../docs/requests/CR-043-dead-code-detector-precision.md
    /// [ADR-39]: ../../../docs/specs/architecture/decisions/ADR-39.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    pub fn reachability_extensions(&self) -> std::collections::HashSet<String> {
        self.plugins
            .iter()
            .filter(|p| p.supports_reachability())
            .flat_map(|p| p.extensions().iter().map(|e| normalize_ext(e)))
            .collect()
    }

    /// Grammars skipped at load due to an ABI mismatch ([FR-PL-03]).
    pub fn skipped(&self) -> &[SkippedGrammar] {
        &self.skipped
    }

    /// Number of successfully loaded grammars.
    pub fn len(&self) -> usize {
        self.plugins.len()
    }

    /// `true` when no grammar loaded successfully.
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }
}

/// Resolve and compile every capability's query for one grammar.
///
/// Returns the capability → compiled query map and the list of query keys whose
/// source was an on-disk override.
fn compile_capabilities(
    entry: &GrammarEntry,
    manifest: &PluginManifest,
    language: &Language,
    override_dir: Option<&Path>,
) -> Result<(BTreeMap<String, tree_sitter::Query>, Vec<String>), PluginError> {
    let mut compiled = BTreeMap::new();
    let mut overridden = Vec::new();

    // Each declared capability has a required, fail-fast query (`validate`
    // guarantees the `[queries]` entry exists).
    let query_keys = manifest.capabilities.iter().map(String::as_str);

    for key in query_keys {
        // `validate` guarantees the capability's `[queries]` entry exists, so
        // the index is safe.
        let relative_path = &manifest.queries[key];
        let embedded = entry
            .embedded_queries
            .iter()
            .find(|q| q.relative_path == relative_path.as_str())
            .ok_or_else(|| PluginError::Manifest {
                file: entry.manifest_label.to_string(),
                detail: format!("query '{key}' maps to '{relative_path}' with no embedded source"),
            })?;

        let resolved = queries::resolve_query(
            key,
            relative_path,
            embedded.label,
            embedded.source,
            override_dir,
        )?;
        if resolved.overridden {
            overridden.push(key.to_string());
        }
        let query = queries::compile(language, &resolved)?;
        compiled.insert(key.to_string(), query);
    }

    Ok((compiled, overridden))
}

/// The override directory for a language: `<root>/.logos/plugins/<name>/`.
fn override_dir_for(root: &Path, name: &str) -> std::path::PathBuf {
    root.join(".logos").join("plugins").join(name)
}

/// Normalise an extension for indexing: lower-cased, leading dot stripped.
fn normalize_ext(ext: &str) -> String {
    ext.trim_start_matches('.').to_ascii_lowercase()
}

#[cfg(all(test, feature = "lang-rust"))]
mod tests {
    use super::*;

    /// A synthetic second grammar that reuses the real Rust `LanguageFn` but
    /// declares a bogus `abi_version` (99). Its descriptor therefore disagrees
    /// with the compiled grammar (ABI 15) and the registry must skip *it* while
    /// the genuine Rust grammar still loads — proving selectivity ([UAT-PL-02]).
    const SKIP_MANIFEST: &str = r#"
        name = "rustskip"
        extensions = ["rsx"]
        module_separator = "::"
        abi_version = 99
        capabilities = ["symbols"]
        [queries]
        symbols = "queries/symbols.scm"
    "#;

    fn skip_entry() -> GrammarEntry {
        GrammarEntry {
            manifest_label: "rustskip/plugin.toml",
            manifest_toml: SKIP_MANIFEST,
            language: tree_sitter_rust::LANGUAGE,
            embedded_queries: &[grammars::EmbeddedQuery {
                relative_path: "queries/symbols.scm",
                label: "rustskip/queries/symbols.scm",
                source: "(function_item name: (identifier) @f)",
            }],
        }
    }

    /// A second grammar that loads cleanly (ABI 15, valid query) but claims the
    /// same `rs` extension as the real Rust grammar — to exercise the
    /// extension-collision warn-and-last-writer-wins path.
    const COLLIDE_MANIFEST: &str = r#"
        name = "rustdup"
        extensions = ["rs"]
        module_separator = "::"
        abi_version = 15
        capabilities = ["symbols"]
        [queries]
        symbols = "queries/symbols.scm"
    "#;

    fn collide_entry() -> GrammarEntry {
        GrammarEntry {
            manifest_label: "rustdup/plugin.toml",
            manifest_toml: COLLIDE_MANIFEST,
            language: tree_sitter_rust::LANGUAGE,
            embedded_queries: &[grammars::EmbeddedQuery {
                relative_path: "queries/symbols.scm",
                label: "rustdup/queries/symbols.scm",
                source: "(function_item name: (identifier) @f)",
            }],
        }
    }

    /// NFR-MA-01 / S-015 acceptance: adding a language is *pure data* — a
    /// descriptor (with framework/export semantics), query text, and a
    /// `LanguageFn` row. This test assembles such a grammar entirely from
    /// literals against the unchanged registry and gets a fully capable
    /// plugin back: nothing in `logos-core` knows the language exists.
    #[test]
    fn a_new_language_loads_from_pure_data_with_no_core_edit() {
        const TOY_MANIFEST: &str = r#"
            name = "toylang"
            extensions = ["toy"]
            module_separator = "."
            abi_version = 15
            capabilities = ["symbols"]
            export_convention = "underscore-private"
            framework_detectors = ["toyweb"]
            [queries]
            symbols = "queries/symbols.scm"
            [framework_methods]
            get = "GET"
        "#;
        let entry = GrammarEntry {
            manifest_label: "toylang/plugin.toml",
            manifest_toml: TOY_MANIFEST,
            // Any compiled grammar works — the point is the *registry* needs
            // no new code, only a LanguageFn it has never seen named.
            language: tree_sitter_rust::LANGUAGE,
            embedded_queries: &[grammars::EmbeddedQuery {
                relative_path: "queries/symbols.scm",
                label: "toylang/queries/symbols.scm",
                source: "(function_item name: (identifier) @symbol.function)",
            }],
        };

        let mut entries = grammars::compiled();
        entries.push(entry);
        let reg = LanguageRegistry::load_from(&entries, AbiRange::runtime(), None, &mut |_| {})
            .expect("a data-only grammar loads");

        let toy = reg.for_extension("toy").expect("toylang claims .toy");
        assert_eq!(toy.name(), "toylang");
        assert!(toy.query("symbols").is_some(), "its query compiled");
        let semantics = toy.semantics();
        assert_eq!(semantics.framework_detectors, ["toyweb"]);
        assert_eq!(semantics.framework_methods.get("get").unwrap(), "GET");
        assert_eq!(
            semantics.export_convention,
            crate::plugin::ExportConvention::UnderscorePrivate
        );
    }

    #[test]
    fn extension_collision_warns_and_last_writer_wins() {
        let mut warnings = Vec::new();
        let mut entries = grammars::compiled(); // rust claims "rs" first
        entries.push(collide_entry()); // rustdup also claims "rs"

        let reg = LanguageRegistry::load_from(&entries, AbiRange::runtime(), None, &mut |w| {
            warnings.push(w.to_string())
        })
        .expect("a valid colliding grammar still loads");

        // Every compiled-in grammar plus the collider loaded; the later
        // declaration wins the `rs` lookup.
        assert_eq!(reg.len(), grammars::compiled().len() + 1);
        assert_eq!(reg.for_extension("rs").unwrap().name(), "rustdup");
        // ...and the collision was surfaced as a warning, not silently.
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("rs") && w.contains("shadow")),
            "expected an extension-shadow warning, got {warnings:?}"
        );
    }

    /// S-159 / CR-043 / ADR-39: the reachability-capability set is built from the
    /// loaded descriptors — Rust declares the capability (so `rs` is present),
    /// while a synthetic grammar that omits the flag is absent. The
    /// [`crate::annotate`] dead-code pass gates on exactly this set.
    #[test]
    fn reachability_extensions_collects_only_capable_languages() {
        // A second grammar that loads cleanly but does NOT declare reachability.
        const PLAIN: &str = r#"
            name = "toyplain"
            extensions = ["TOY"]
            module_separator = "."
            abi_version = 15
            capabilities = ["symbols"]
            [queries]
            symbols = "queries/symbols.scm"
        "#;
        let entry = GrammarEntry {
            manifest_label: "toyplain/plugin.toml",
            manifest_toml: PLAIN,
            language: tree_sitter_rust::LANGUAGE,
            embedded_queries: &[grammars::EmbeddedQuery {
                relative_path: "queries/symbols.scm",
                label: "toyplain/queries/symbols.scm",
                source: "(function_item name: (identifier) @f)",
            }],
        };
        let mut entries = grammars::compiled();
        entries.push(entry);
        let reg = LanguageRegistry::load_from(&entries, AbiRange::runtime(), None, &mut |_| {})
            .expect("the grammars load");

        let exts = reg.reachability_extensions();
        assert!(
            exts.contains("rs"),
            "Rust declares the reachability capability — `rs` is capable"
        );
        assert!(
            !exts.contains("toy"),
            "a grammar that omits the flag is not capable (normalised lower-case)"
        );
    }

    /// A synthetic artifact-class grammar (S-062, CR-010): it reuses the Rust
    /// `LanguageFn` (any compiled grammar works — `for_path` is a pure lookup, no
    /// parse) but declares `artifact = true` and basename claims, proving the
    /// substrate admits a filename-claimed format from **pure descriptor data**
    /// with no core edit (NFR-MA-01).
    const ARTIFACT_MANIFEST: &str = r#"
        name = "dockerfile"
        extensions = ["dockerfile"]
        module_separator = "/"
        abi_version = 15
        capabilities = []
        artifact = true
        filenames = ["Dockerfile"]
    "#;

    fn artifact_entry() -> GrammarEntry {
        GrammarEntry {
            manifest_label: "dockerfile/plugin.toml",
            manifest_toml: ARTIFACT_MANIFEST,
            language: tree_sitter_rust::LANGUAGE,
            embedded_queries: &[],
        }
    }

    #[test]
    fn for_path_admits_by_extension_or_claimed_basename() {
        let mut entries = grammars::compiled();
        entries.push(artifact_entry());
        let reg = LanguageRegistry::load_from(&entries, AbiRange::runtime(), None, &mut |_| {})
            .expect("the artifact grammar loads from pure data");

        // The artifact plugin is the third class — flagged, with its basename claim
        // surfaced.
        let docker = reg
            .for_extension("dockerfile")
            .expect("claims the .dockerfile extension");
        assert!(docker.is_artifact(), "the descriptor is an artifact plugin");
        assert_eq!(docker.filenames(), ["Dockerfile"]);

        // Extension-or-basename admission (FR-IX-02 as modified):
        // 1. extension still resolves code files.
        assert_eq!(reg.for_path("src/lib.rs").map(|p| p.name()), Some("rust"));
        // 2. an extensionless `Dockerfile` resolves via its exact basename claim.
        assert_eq!(
            reg.for_path("Dockerfile").map(|p| p.name()),
            Some("dockerfile"),
            "extensionless Dockerfile admitted by its basename claim"
        );
        assert_eq!(
            reg.for_path("deploy/Dockerfile").map(|p| p.name()),
            Some("dockerfile"),
            "a nested Dockerfile resolves by basename regardless of directory"
        );
        // 3. the `Name.*` prefix rule binds `Dockerfile.dev` to the same plugin.
        assert_eq!(
            reg.for_path("Dockerfile.dev").map(|p| p.name()),
            Some("dockerfile"),
            "Dockerfile.dev binds via the Name.* prefix rule"
        );
        // A look-alike that is neither the exact name nor a `Name.` prefix is not
        // claimed — `Dockerfileish` shares a prefix but no dot boundary.
        assert!(
            reg.for_path("Dockerfileish").is_none(),
            "the prefix rule requires a `.` boundary, never a bare prefix"
        );
        // An unrelated extensionless file is unclaimed.
        assert!(reg.for_path("LICENSE").is_none());
    }

    #[test]
    fn loads_the_real_rust_grammar() {
        let reg = LanguageRegistry::load_from(
            &grammars::compiled(),
            AbiRange::runtime(),
            None,
            &mut |_| {},
        )
        .expect("embedded grammars load cleanly");

        assert!(!reg.is_empty());
        let rust = reg.for_extension("rs").expect("rust claims .rs");
        assert_eq!(rust.name(), "rust");
        assert!(rust.capabilities().iter().any(|c| c == "symbols"));
        assert!(rust.query("symbols").is_some());
    }

    #[test]
    fn extension_lookup_is_case_and_dot_insensitive() {
        let reg = LanguageRegistry::load_from(
            &grammars::compiled(),
            AbiRange::runtime(),
            None,
            &mut |_| {},
        )
        .unwrap();
        assert!(reg.for_extension("rs").is_some());
        assert!(reg.for_extension(".rs").is_some());
        assert!(reg.for_extension(".RS").is_some());
        // `.py` resolves exactly when the Python grammar is compiled in (S-015).
        assert_eq!(
            reg.for_extension("py").is_some(),
            cfg!(feature = "lang-python")
        );
        assert!(reg.for_extension("zig").is_none());
    }

    #[test]
    fn abi_mismatch_skips_only_the_affected_grammar_and_warns() {
        let mut warnings = Vec::new();
        let mut entries = grammars::compiled();
        entries.push(skip_entry());

        let reg = LanguageRegistry::load_from(&entries, AbiRange::runtime(), None, &mut |w| {
            warnings.push(w.to_string())
        })
        .expect("an ABI mismatch must not abort the run (FR-PL-03)");

        // The genuine grammar still works...
        assert!(reg.for_extension("rs").is_some(), "rust must still load");
        // ...the bogus one is skipped, not loaded...
        assert!(
            reg.for_extension("rsx").is_none(),
            "rustskip must be skipped"
        );
        // ...and recorded with the disagreeing-ABI reason.
        assert_eq!(reg.skipped().len(), 1);
        assert_eq!(reg.skipped()[0].name, "rustskip");
        // ...with a warning emitted to the sink (stderr in production).
        assert!(
            warnings.iter().any(|w| w.contains("rustskip")),
            "expected a skip warning naming rustskip, got {warnings:?}"
        );
    }

    /// FR-PL-03 / S-061 (CR-009 CRA-01): a *forced* Scala grammar failure
    /// skips Scala alone with a warning and never aborts the run — the runtime
    /// half of the gate that makes the highest-risk grammar safe to ship. The
    /// real Scala `LanguageFn` (compiled ABI 15) is paired with a descriptor that
    /// declares ABI 14, so `assert_abi` reports a disagreement and the registry
    /// skips it while every genuine grammar still loads.
    #[cfg(feature = "lang-scala")]
    #[test]
    fn a_forced_scala_grammar_failure_skips_only_scala_and_warns() {
        const TAMPERED_SCALA: &str = r#"
            name = "scala"
            extensions = ["scala", "sc"]
            module_separator = "."
            abi_version = 14
            capabilities = ["symbols"]
            [queries]
            symbols = "queries/symbols.scm"
        "#;
        let tampered = GrammarEntry {
            manifest_label: "scala/plugin.toml",
            manifest_toml: TAMPERED_SCALA,
            language: tree_sitter_scala::LANGUAGE,
            embedded_queries: &[grammars::EmbeddedQuery {
                relative_path: "queries/symbols.scm",
                label: "scala/queries/symbols.scm",
                source: "(class_definition name: (identifier) @c)",
            }],
        };
        // The full compiled set, then the real Scala row replaced by the
        // tampered one (push is enough: last declaration wins on the `scala`/`sc`
        // extensions, so the tampered row is the one the ABI check sees).
        let mut entries = grammars::compiled();
        entries.push(tampered);

        let mut warnings = Vec::new();
        let reg = LanguageRegistry::load_from(&entries, AbiRange::runtime(), None, &mut |w| {
            warnings.push(w.to_string())
        })
        .expect("a forced Scala failure must not abort the run (FR-PL-03)");

        // Every genuine grammar still loaded (Rust is the harness's control).
        assert!(reg.for_extension("rs").is_some(), "rust must still load");
        // Scala was skipped with the disagreeing-ABI reason, naming scala.
        assert!(
            reg.skipped().iter().any(|s| s.name == "scala"),
            "scala must be skipped, got {:?}",
            reg.skipped()
        );
        assert!(
            warnings.iter().any(|w| w.contains("scala")),
            "expected a skip warning naming scala, got {warnings:?}"
        );
    }

    #[test]
    fn narrow_abi_range_skips_an_in_spec_grammar() {
        // A runtime that understands no real ABI (1..=2) can load none of the
        // compiled grammars (ABI 14/15): every one is skipped, the registry is
        // empty, and the run is not aborted.
        let mut warnings = Vec::new();
        let reg = LanguageRegistry::load_from(
            &grammars::compiled(),
            AbiRange { min: 1, max: 2 },
            None,
            &mut |w| warnings.push(w.to_string()),
        )
        .expect("ABI skip is not an error");
        assert!(reg.is_empty());
        assert_eq!(reg.skipped().len(), grammars::compiled().len());
        assert!(!warnings.is_empty());
    }
}
