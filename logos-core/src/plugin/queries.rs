//! Query resolution + compilation ([FR-PL-02], [FR-PL-04], [FR-PL-05]).
//!
//! A capability's `.scm` query has two possible sources, in priority order:
//!
//! 1. **On-disk override** — `<project>/.logos/plugins/<lang>/<relative path>`,
//!    if present, *shadows* the embedded query. This is what lets a maintainer
//!    tune extraction for an already-compiled grammar without a rebuild
//!    ([FR-PL-04], [FR-PL-05], [UAT-PL-03], [NFR-MA-05]).
//! 2. **Embedded** — the `include_str!`-embedded source shipped in the binary.
//!
//! Whichever source wins, its label (the on-disk path or the embedded asset
//! name) is threaded into a compile error so the message always points at the
//! source the operator can actually edit ([FR-PL-02]).
//!
//! Compiling is the expensive half, so it happens **when a language is first
//! used**, not when the [`LanguageRegistry`] loads (CR-197). Each language's
//! queries sit in a [`LanguageQueries`] cell that compiles them all, as one
//! unit, on the first extraction that asks for one. A cold start therefore pays
//! only for override queries, which still compile at load so a broken one fails
//! naming its file ([FR-PL-04]); a broken *embedded* query is caught at test
//! time instead, by the test that compiles every one of them.
//!
//! Underneath, [`compile_shared`] compiles each distinct query **once per
//! process** and hands every later load the same [`Arc<Query>`]. A workspace
//! starts one engine (and so one registry load) per member, and before this
//! every member start recompiled every built-in query from scratch (HF-3). The
//! cache key is the grammar, the capability and a content hash of the
//! *resolved* source, so a root's own override compiles to its own entry and is
//! never served a neighbour's, while byte-identical sources — the embedded
//! default above all — share one. Only successful compiles are kept: a query
//! that fails still fails every load that resolves it, naming its file.
//!
//! [`LanguageRegistry`]: super::LanguageRegistry
//! [FR-PL-02]: ../../../docs/specs/requirements/FR-PL-02.md
//! [FR-PL-04]: ../../../docs/specs/requirements/FR-PL-04.md
//! [FR-PL-05]: ../../../docs/specs/requirements/FR-PL-05.md
//! [UAT-PL-03]: ../../../docs/specs/requirements/UAT-PL-03.md
//! [NFR-MA-05]: ../../../docs/specs/requirements/NFR-MA-05.md

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use tree_sitter::{Language, Query};

use super::error::PluginError;
use super::manifest::NAMESPACE_CAPTURE;

/// A query whose source has been resolved (override-or-embedded), ready to
/// compile.
#[derive(Debug, Clone)]
pub struct ResolvedQuery {
    /// The capability this query backs (e.g. `"symbols"`).
    pub capability: String,
    /// Human-facing label of the *winning* source: the on-disk override path
    /// when overridden, else the embedded asset name. Used in compile errors.
    pub file_label: String,
    /// The query text to compile.
    pub source: String,
    /// `true` when the source came from an on-disk override (for observability
    /// and tests), `false` when it is the embedded default.
    pub overridden: bool,
}

/// Resolve a single capability's query source, preferring an on-disk override.
///
/// - `capability` / `relative_path` come from the descriptor's `[queries]`.
/// - `embedded_label` is the embedded asset name used in error messages.
/// - `embedded_source` is the `include_str!`-embedded query text.
/// - `override_dir`, when `Some`, is `<project>/.logos/plugins/<lang>/`; the
///   override file is `override_dir.join(relative_path)`.
///
/// # Errors
/// Returns [`PluginError::Io`] if an override file exists but cannot be read.
pub fn resolve_query(
    capability: &str,
    relative_path: &str,
    embedded_label: &str,
    embedded_source: &str,
    override_dir: Option<&Path>,
) -> Result<ResolvedQuery, PluginError> {
    if let Some(dir) = override_dir {
        let candidate = dir.join(relative_path);
        if candidate.is_file() {
            let source = std::fs::read_to_string(&candidate).map_err(|e| PluginError::Io {
                file: candidate.display().to_string(),
                detail: e.to_string(),
            })?;
            return Ok(ResolvedQuery {
                capability: capability.to_string(),
                file_label: candidate.display().to_string(),
                source,
                overridden: true,
            });
        }
    }
    Ok(ResolvedQuery {
        capability: capability.to_string(),
        file_label: embedded_label.to_string(),
        source: embedded_source.to_string(),
        overridden: false,
    })
}

/// Compile a resolved query against the built `Language`, failing fast and
/// naming the source on error ([FR-PL-02]).
///
/// # Errors
/// Returns [`PluginError::QueryCompile`] with `file` set to the resolved
/// query's `file_label` when the query has a syntax or node-type error.
pub fn compile(language: &Language, resolved: &ResolvedQuery) -> Result<Query, PluginError> {
    Query::new(language, &resolved.source).map_err(|e| PluginError::QueryCompile {
        file: resolved.file_label.clone(),
        detail: e.to_string(),
    })
}

/// Capability → compiled query, each shared with every other plugin in the
/// process whose resolved source for it is byte-identical (HF-3; see
/// [`compile_shared`]).
pub(crate) type CompiledQueries = BTreeMap<String, Arc<Query>>;

/// A compiled query's identity: the `Language` it is bound to (its node-kind
/// ids are that grammar's), the grammar and capability it backs, and the
/// blake3 hash of the source it was compiled from.
type CompiledKey = (Language, String, String, [u8; 32]);

/// One compiled query's cell: empty until its first successful compile. Its
/// own lock is what makes concurrent first uses of one query compile it once.
type CompiledSlot = Arc<Mutex<Option<Arc<Query>>>>;

/// Every query this process has compiled, one cell per distinct (grammar,
/// capability, source). Bounded by construction: the built-in queries plus each
/// distinct override text seen, however many loads resolve them.
static COMPILED: LazyLock<Mutex<HashMap<CompiledKey, CompiledSlot>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// A language's compile unit's identity: its `Language`, its grammar, and one
/// blake3 hash over every capability and source it resolved.
type UnitKey = (Language, String, [u8; 32]);

/// One lock per distinct compile unit, held across a first-use compile so that
/// registries racing on one language's first use run it one at a time: the
/// first compiles every query and reports, and the rest find them all cached,
/// compile nothing and report nothing. Without it, two registries could split a
/// language's queries between them and each report a share (CR-197).
static UNITS: LazyLock<Mutex<HashMap<UnitKey, Arc<Mutex<()>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Each language whose queries compiled on first use, in the order they did
/// (CR-197). Read by [`first_use_compiles`].
static FIRST_USES: LazyLock<Mutex<Vec<FirstUseCompile>>> = LazyLock::new(|| Mutex::new(Vec::new()));

/// [`compile`], once per process for each distinct source: every call that
/// resolves the same `grammar`/capability to byte-identical text shares one
/// compiled [`Query`] (HF-3). Returns the query and whether this call is the
/// one that compiled it (`true`) or was served an earlier compile (`false`).
///
/// Calls racing on one query compile it once: the first holds that query's
/// cell while it compiles and the rest wait for the result. Calls on different
/// queries never wait on each other — the process-wide map is locked only to
/// find the cell.
///
/// # Errors
/// As [`compile`]. A failed compile leaves its cell empty, so every later call
/// that resolves the same text compiles again and fails again, naming its own
/// file.
pub fn compile_shared(
    language: &Language,
    grammar: &str,
    resolved: &ResolvedQuery,
) -> Result<(Arc<Query>, bool), PluginError> {
    let key: CompiledKey = (
        language.clone(),
        grammar.to_string(),
        resolved.capability.clone(),
        *blake3::hash(resolved.source.as_bytes()).as_bytes(),
    );
    let slot = Arc::clone(compiled_cache().entry(key).or_default());
    let mut held = slot.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(hit) = held.as_ref() {
        return Ok((Arc::clone(hit), false));
    }
    let query = Arc::new(compile(language, resolved)?);
    *held = Some(Arc::clone(&query));
    Ok((query, true))
}

/// Forget every query [`compile_shared`] has compiled, and every
/// [`first_use_compiles`] record, so the next load or first use compiles cold.
/// For measurement harnesses that time several cold starts in one process;
/// production never calls it. Queries already handed out stay valid — their
/// holders keep them alive.
#[doc(hidden)]
pub fn clear_compiled_cache() {
    compiled_cache().clear();
    first_uses().clear();
    UNITS.lock().unwrap_or_else(PoisonError::into_inner).clear();
}

/// One language's first-use compile (CR-197): which language, how many of its
/// queries this compile built (the rest were already in the process cache),
/// and how long the whole unit took.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FirstUseCompile {
    /// The grammar's name (the descriptor `name`).
    pub language: String,
    /// Queries compiled by this first use rather than served from the cache.
    pub compiled: usize,
    /// Wall time of the language's whole compile unit.
    pub elapsed: Duration,
}

/// Every language whose queries this process compiled on first use, in order.
/// A language appears once per process — a later registry's first use of it,
/// or one racing it, is served from the cache, compiles nothing and is not
/// reported — unless
/// [`clear_compiled_cache`] made it compile again. Each entry is also emitted
/// as an `info` event through the tracing seam when it happens.
#[doc(hidden)]
pub fn first_use_compiles() -> Vec<FirstUseCompile> {
    first_uses().clone()
}

/// The cache, locked. A poisoned lock is recovered: the map is only ever
/// inserted into whole, so no panic can leave it half-written.
fn compiled_cache() -> MutexGuard<'static, HashMap<CompiledKey, CompiledSlot>> {
    COMPILED.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The first-use record, locked; recovered on poison like [`compiled_cache`].
fn first_uses() -> MutexGuard<'static, Vec<FirstUseCompile>> {
    FIRST_USES.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One language's capability queries, compiled together as one unit — at load
/// when any of them is an on-disk override, else on the language's first use
/// (CR-197, [FR-PL-02], [FR-PL-04]).
///
/// The cell is a [`OnceLock`], so concurrent first uses of one language compile
/// it once and every caller sees the same outcome, and no caller ever observes
/// a partly compiled language.
///
/// An override forces its whole language to compile at load: the override must
/// fail naming its file where the operator can see it, and a namespace
/// language's `symbols` override must prove its namespace capture before the
/// registry is handed out.
#[derive(Debug)]
pub(crate) struct LanguageQueries {
    /// The grammar's name: the process cache's key and the first-use report's.
    grammar: String,
    /// The descriptor's label, named when the namespace check refuses.
    manifest_label: &'static str,
    /// Whether the language declares the namespace module model (S-518), whose
    /// `symbols` query must capture [`NAMESPACE_CAPTURE`].
    namespace_model: bool,
    /// Every capability's resolved source, in declaration order.
    resolved: Vec<ResolvedQuery>,
    /// The compiled unit — or the error that refused it — once compiled.
    compiled: OnceLock<Result<CompiledQueries, PluginError>>,
}

impl LanguageQueries {
    /// Hold `resolved` for `grammar`, compiling it now when any source is an
    /// override.
    ///
    /// # Errors
    /// A [`PluginError`] from compiling an overridden language: an override
    /// that does not compile names its file; a namespace language whose
    /// `symbols` does not capture its namespace names the descriptor.
    pub(crate) fn new(
        grammar: &str,
        manifest_label: &'static str,
        namespace_model: bool,
        resolved: Vec<ResolvedQuery>,
        language: &Language,
    ) -> Result<Self, PluginError> {
        let queries = Self {
            grammar: grammar.to_string(),
            manifest_label,
            namespace_model,
            resolved,
            compiled: OnceLock::new(),
        };
        if queries.resolved.iter().any(|q| q.overridden) {
            let (compiled, _) = queries.compile_unit(language)?;
            // The cell was created three lines up and nothing else holds it.
            let _ = queries.compiled.set(Ok(compiled));
        }
        Ok(queries)
    }

    /// A language whose queries are already compiled — for tests that build a
    /// [`CompiledPlugin`](super::CompiledPlugin) by hand.
    #[cfg(test)]
    pub(crate) fn precompiled(queries: CompiledQueries) -> Self {
        Self {
            grammar: String::new(),
            manifest_label: "",
            namespace_model: false,
            resolved: Vec::new(),
            compiled: OnceLock::from(Ok(queries)),
        }
    }

    /// Whether the language's queries are compiled yet (or refused).
    #[cfg(test)]
    pub(crate) fn is_compiled(&self) -> bool {
        self.compiled.get().is_some()
    }

    /// The capability → query map, compiling the language on its first call.
    ///
    /// # Errors
    /// The [`PluginError`] that refused the compile, the same one on every
    /// call. Unreachable for a shipped build: every embedded query is compiled
    /// by a test.
    pub(crate) fn get(&self, language: &Language) -> Result<&CompiledQueries, &PluginError> {
        self.compiled
            .get_or_init(|| self.first_use(language))
            .as_ref()
    }

    /// The first-use compile, reported: its wall time once per process when it
    /// compiled anything, and a refusal as an error event naming the file.
    fn first_use(&self, language: &Language) -> Result<CompiledQueries, PluginError> {
        let unit = self.unit_lock(language);
        let _one_at_a_time = unit.lock().unwrap_or_else(PoisonError::into_inner);
        let started = Instant::now();
        match self.compile_unit(language) {
            Ok((queries, compiled)) => {
                let elapsed = started.elapsed();
                if compiled > 0 {
                    tracing::info!(
                        language = %self.grammar,
                        queries = compiled,
                        duration_ms = elapsed.as_secs_f64() * 1000.0,
                        "compiled the language's queries on first use"
                    );
                    first_uses().push(FirstUseCompile {
                        language: self.grammar.clone(),
                        compiled,
                        elapsed,
                    });
                }
                Ok(queries)
            }
            Err(err) => {
                tracing::error!(
                    language = %self.grammar,
                    "the language's queries failed to compile on first use; it extracts \
                     nothing this process: {err}"
                );
                Err(err)
            }
        }
    }

    /// This unit's entry in [`UNITS`]: the same lock for every registry whose
    /// language resolved to the same sources.
    fn unit_lock(&self, language: &Language) -> Arc<Mutex<()>> {
        let mut hasher = blake3::Hasher::new();
        for resolved in &self.resolved {
            for part in [&resolved.capability, &resolved.source] {
                hasher.update(&(part.len() as u64).to_le_bytes());
                hasher.update(part.as_bytes());
            }
        }
        let key: UnitKey = (
            language.clone(),
            self.grammar.clone(),
            *hasher.finalize().as_bytes(),
        );
        let mut units = UNITS.lock().unwrap_or_else(PoisonError::into_inner);
        Arc::clone(units.entry(key).or_default())
    }

    /// Compile every capability through the process cache, then check the
    /// namespace capture. Returns the map and how many queries were compiled
    /// rather than served from the cache.
    fn compile_unit(&self, language: &Language) -> Result<(CompiledQueries, usize), PluginError> {
        let mut queries = BTreeMap::new();
        let mut compiled = 0;
        for resolved in &self.resolved {
            let (query, fresh) = compile_shared(language, &self.grammar, resolved)?;
            compiled += usize::from(fresh);
            queries.insert(resolved.capability.clone(), query);
        }
        if self.namespace_model {
            check_namespace_capture(self.manifest_label, &queries)?;
        }
        Ok((queries, compiled))
    }
}

/// A declared-namespace language (S-518, [FR-RS-13]) must name its namespace
/// declarations: its compiled `symbols` query — embedded or an on-disk override
/// — carries the `@module.namespace` capture. Without it every file of the
/// language would read as the global namespace, and every type of the
/// repository would be visible to every other without an import — honest
/// absence at the query becoming a fabricated binding ([NFR-RA-05]). Refused
/// with the language's compile instead, naming the descriptor.
///
/// [FR-RS-13]: ../../../docs/specs/requirements/FR-RS-13.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
fn check_namespace_capture(
    manifest_label: &str,
    compiled: &CompiledQueries,
) -> Result<(), PluginError> {
    let captures = compiled
        .get("symbols")
        .is_some_and(|q| q.capture_names().contains(&NAMESPACE_CAPTURE));
    if captures {
        return Ok(());
    }
    Err(PluginError::Manifest {
        file: manifest_label.to_string(),
        detail: format!(
            "`[module_model]` kind 'namespace' requires the `symbols` query to capture \
             `@{NAMESPACE_CAPTURE}`, or every file would read as the global namespace"
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CR-197: registries racing on the first use of one language with several
    /// queries report it once, with every query counted and a real duration —
    /// never two shares of it. The race is narrow, so it is run many times
    /// over a fresh language each round.
    #[cfg(feature = "lang-rust")]
    #[test]
    fn racing_registries_report_a_multi_query_language_once() {
        const ROUNDS: usize = 200;
        const REGISTRIES: usize = 8;
        const QUERIES: usize = 12;
        let language: Language = tree_sitter_rust::LANGUAGE.into();
        let mut split = Vec::new();
        for round in 0..ROUNDS {
            let grammar = format!("toyunit{round}");
            let registries: Vec<LanguageQueries> = (0..REGISTRIES)
                .map(|_| {
                    let resolved = (0..QUERIES)
                        .map(|q| ResolvedQuery {
                            capability: format!("cap{q}"),
                            file_label: format!("{grammar}/queries/cap{q}.scm"),
                            source: format!("; query {q}\n(identifier) @x"),
                            overridden: false,
                        })
                        .collect();
                    LanguageQueries::new(
                        &grammar,
                        "toyunit/plugin.toml",
                        false,
                        resolved,
                        &language,
                    )
                    .expect("no override, nothing compiles at construction")
                })
                .collect();
            let barrier = std::sync::Barrier::new(REGISTRIES);
            std::thread::scope(|scope| {
                for queries in &registries {
                    let (barrier, language) = (&barrier, &language);
                    scope.spawn(move || {
                        barrier.wait();
                        assert_eq!(queries.get(language).expect("compiles").len(), QUERIES);
                    });
                }
            });
            let reports: Vec<FirstUseCompile> = first_use_compiles()
                .into_iter()
                .filter(|r| r.language == grammar)
                .collect();
            let whole = reports.len() == 1
                && reports[0].compiled == QUERIES
                && reports[0].elapsed > Duration::ZERO;
            if !whole {
                split.push((round, reports));
            }
        }
        assert!(
            split.is_empty(),
            "rounds not reported once, whole: {split:?}"
        );
    }

    /// CR-197: a language's first use reports its compile time as one `info`
    /// event naming the language, its query count and the duration; a second
    /// use, and a second registry's first use, report nothing more.
    #[cfg(feature = "lang-rust")]
    #[test]
    fn a_first_use_compile_emits_one_info_event_with_its_duration() {
        #[derive(Clone, Default)]
        struct Captured(Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for Captured {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let language: Language = tree_sitter_rust::LANGUAGE.into();
        let registry = || {
            let resolved = ["symbols", "references"]
                .map(|cap| ResolvedQuery {
                    capability: cap.to_string(),
                    file_label: format!("toyevent/queries/{cap}.scm"),
                    source: format!("; CR-197 event {cap}\n(identifier) @x"),
                    overridden: false,
                })
                .to_vec();
            LanguageQueries::new("toyevent", "toyevent/plugin.toml", false, resolved, &language)
                .expect("no override, nothing compiles at construction")
        };
        let captured = Captured::default();
        let writer = captured.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || writer.clone())
            .with_max_level(tracing::Level::INFO)
            .with_ansi(false)
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            let (first, second) = (registry(), registry());
            first.get(&language).expect("compiles");
            first.get(&language).expect("compiled already");
            second.get(&language).expect("served from the cache");
        });

        let log = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
        let events: Vec<&str> = log
            .lines()
            .filter(|l| l.contains("compiled the language's queries on first use"))
            .collect();
        assert_eq!(events.len(), 1, "one first-use event: {log}");
        let event = events[0];
        assert!(event.contains("INFO"), "{event}");
        assert!(event.contains("language=toyevent") && event.contains("queries=2"), "{event}");
        assert!(event.contains("duration_ms="), "the event carries the compile time: {event}");
    }

    #[test]
    fn resolves_embedded_when_no_override_dir() {
        let r = resolve_query(
            "symbols",
            "queries/symbols.scm",
            "rust/queries/symbols.scm",
            "(identifier) @x",
            None,
        )
        .unwrap();
        assert!(!r.overridden);
        assert_eq!(r.file_label, "rust/queries/symbols.scm");
        assert_eq!(r.source, "(identifier) @x");
    }

    #[test]
    fn resolves_embedded_when_override_file_absent() {
        let dir = tempfile::tempdir().unwrap();
        let r = resolve_query(
            "symbols",
            "queries/symbols.scm",
            "rust/queries/symbols.scm",
            "(identifier) @x",
            Some(dir.path()),
        )
        .unwrap();
        assert!(!r.overridden, "absent override must fall back to embedded");
    }

    #[test]
    fn override_file_shadows_embedded() {
        let dir = tempfile::tempdir().unwrap();
        let qdir = dir.path().join("queries");
        std::fs::create_dir_all(&qdir).unwrap();
        std::fs::write(qdir.join("symbols.scm"), "(type_identifier) @overridden").unwrap();

        let r = resolve_query(
            "symbols",
            "queries/symbols.scm",
            "rust/queries/symbols.scm",
            "(identifier) @embedded",
            Some(dir.path()),
        )
        .unwrap();

        assert!(r.overridden, "present override must win over embedded");
        assert_eq!(r.source, "(type_identifier) @overridden");
        assert_eq!(r.file_label, qdir.join("symbols.scm").display().to_string());
    }
}
