//! Unit tests for the watch module's pure pieces: the ignored-path filter
//! (feedback-loop containment + indexer-ignored dirs) and the
//! drop-and-coalesce slot semantics ([AQ-01]). End-to-end OS-event behaviour
//! lives in `tests/watcher.rs` (real FSEvents/inotify against a temp project).
//!
//! [AQ-01]: ../../../docs/specs/architecture.md#14-open-questions

use super::*;

/// The ignored-dir union the watcher builds in [`spawn`]: the always-on
/// feedback-loop internal dirs (`.logos`, `.git`) plus the indexer's default
/// `ignored_dirs`. Mirrors `INTERNAL_DIRS ∪ default_ignored_dirs()` — kept in
/// lockstep with the CR-029/FR-CF-05 broadening and the CR-054 scratch-dir
/// additions so the watched set still matches what indexing admits (FR-IX-02).
fn ignored_set() -> HashSet<String> {
    [
        ".logos",
        ".git",
        "target",
        "node_modules",
        "dist",
        "build",
        "vendor",
        // CR-029/FR-CF-05 broadening (agent/tooling + per-language build dirs).
        ".agents",
        ".claude",
        "__pycache__",
        ".venv",
        "venv",
        ".tox",
        ".mypy_cache",
        ".pytest_cache",
        "bin",
        "obj",
        ".gradle",
        "out",
        "Pods",
        ".next",
        ".svelte-kit",
        "coverage",
        "cmake-build-debug",
        "cmake-build-release",
        // CR-054/S-213 scratch-dir additions.
        ".worktrees",
        ".playwright-mcp",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect()
}

// ── is_ignored: feedback-loop containment ──────────────────────────────────

#[test]
fn logos_store_paths_are_ignored() {
    let root = Path::new("/project");
    let ig = ignored_set();
    assert!(is_ignored(root, Path::new("/project/.logos/logos.db"), &ig));
    assert!(is_ignored(
        root,
        Path::new("/project/.logos/logos.db-wal"),
        &ig
    ));
    assert!(is_ignored(
        root,
        Path::new("/project/.logos/telemetry.db"),
        &ig
    ));
    // Nested too — e.g. plugin overrides under .logos/plugins/.
    assert!(is_ignored(
        root,
        Path::new("/project/.logos/plugins/rust/plugin.toml"),
        &ig
    ));
}

#[test]
fn git_dir_paths_are_ignored() {
    let root = Path::new("/project");
    let ig = ignored_set();
    assert!(is_ignored(root, Path::new("/project/.git/index"), &ig));
    assert!(is_ignored(
        root,
        Path::new("/project/.git/refs/heads/main"),
        &ig
    ));
    // A nested repo's .git (vendored checkout) is filtered as well.
    assert!(is_ignored(
        root,
        Path::new("/project/vendor/dep/.git/HEAD"),
        &ig
    ));
}

/// The regression that melted the dev machine: build-output churn under
/// indexer-ignored directories (`target/`, `node_modules/`, …) must be
/// filtered, or a single `cargo build` storms the sync worker across every
/// core. The watched set must match what indexing admits (FR-IX-02).
#[test]
fn build_output_dirs_are_ignored() {
    let root = Path::new("/project");
    let ig = ignored_set();
    assert!(is_ignored(
        root,
        Path::new("/project/target/debug/deps/foo.rlib"),
        &ig
    ));
    assert!(is_ignored(
        root,
        Path::new("/project/target/release/build/x/out/bindings.rs"),
        &ig
    ));
    assert!(is_ignored(
        root,
        Path::new("/project/node_modules/react/index.js"),
        &ig
    ));
    assert!(is_ignored(root, Path::new("/project/dist/bundle.js"), &ig));
    assert!(is_ignored(
        root,
        Path::new("/project/build/CMakeCache.txt"),
        &ig
    ));
}

/// CR-054/S-213: the watcher's runtime `ignored_dirs` union is built from
/// `config.semantics.ignored_dirs` ([`spawn`]), so a project that doesn't
/// gitignore `.worktrees`/`.playwright-mcp` still has watcher events under
/// them filtered before they ever reach `Engine::sync` — the same admission
/// divergence CR-054 targets, defended belt-and-suspenders at this layer too.
#[test]
fn scratch_dirs_are_ignored() {
    let root = Path::new("/project");
    let ig = ignored_set();
    assert!(is_ignored(
        root,
        Path::new("/project/.worktrees/sprint-1/src/copy.rs"),
        &ig
    ));
    assert!(is_ignored(
        root,
        Path::new("/project/.playwright-mcp/trace.json"),
        &ig
    ));
}

#[test]
fn source_paths_are_not_ignored() {
    let root = Path::new("/project");
    let ig = ignored_set();
    assert!(!is_ignored(root, Path::new("/project/src/main.rs"), &ig));
    assert!(!is_ignored(
        root,
        Path::new("/project/deep/nested/mod.rs"),
        &ig
    ));
    // Names that merely *contain* the ignored markers are kept: only exact
    // path components are filtered.
    assert!(!is_ignored(
        root,
        Path::new("/project/src/.github/workflows/ci.yml"),
        &ig
    ));
    assert!(!is_ignored(
        root,
        Path::new("/project/logos-core/src/lib.rs"),
        &ig
    ));
    // A file literally named like an ignored dir is still source.
    assert!(!is_ignored(
        root,
        Path::new("/project/src/target_resolver.rs"),
        &ig
    ));
}

#[test]
fn paths_outside_the_root_are_ignored() {
    let root = Path::new("/project");
    let ig = ignored_set();
    assert!(is_ignored(
        root,
        Path::new("/elsewhere/src/main.rs"),
        &ig
    ));
}

// ── classify: the coverage-artifact allow-list exception (FR-CV-10, ADR-38) ──

/// The conventional/configured coverage artifacts are admitted as `Coverage`
/// even under a `target/`-class ignored dir — the allow-list EXCEPTION — while
/// the `.logos/`/`.git/` feedback-loop dirs stay filtered and ordinary build
/// output stays ignored. This is the central S-140 admission contract.
#[test]
fn classify_admits_coverage_artifacts_over_target_with_feedback_dirs_filtered() {
    let root = Path::new("/project");
    let ig = ignored_set();
    let m = builtin_matcher();

    // Conventions, including ones UNDER the ignored `target/`/`coverage/` dirs.
    for artifact in [
        "/project/lcov.info",
        "/project/coverage.xml",
        "/project/coverage/cobertura-coverage.xml",
        "/project/target/llvm-cov/cobertura.xml",
    ] {
        assert_eq!(
            classify(root, Path::new(artifact), &ig, &m, None),
            Admission::Coverage,
            "{artifact} is a coverage artifact, re-admitted over the ignore filter"
        );
    }

    // Feedback-loop dirs are NEVER admitted — even an artifact-named file there.
    assert_eq!(
        classify(root, Path::new("/project/.logos/coverage.xml"), &ig, &m, None),
        Admission::Ignored,
        ".logos/ stays filtered (no allow-list exception over the feedback loop)"
    );
    assert_eq!(
        classify(root, Path::new("/project/.git/coverage.xml"), &ig, &m, None),
        Admission::Ignored,
        ".git/ stays filtered"
    );

    // Ordinary build output under target/ is still ignored (not an artifact).
    assert_eq!(
        classify(
            root,
            Path::new("/project/target/debug/deps/foo.rlib"),
            &ig,
            &m,
            None,
        ),
        Admission::Ignored
    );
    // Real source is a Source path.
    assert_eq!(
        classify(root, Path::new("/project/src/main.rs"), &ig, &m, None),
        Admission::Source
    );
    // Outside the root is ignored.
    assert_eq!(
        classify(root, Path::new("/elsewhere/lcov.info"), &ig, &m, None),
        Admission::Ignored
    );
}

/// A configured `[coverage_ingest].artifact_glob` extends the conventions and is
/// likewise re-admitted under an ignored dir.
#[test]
fn classify_admits_a_configured_artifact_glob() {
    let root = Path::new("/project");
    let ig = ignored_set();
    let m = matcher_with(&["build/reports/**/*.xml"]);
    assert_eq!(
        classify(
            root,
            Path::new("/project/build/reports/unit/cov.xml"),
            &ig,
            &m,
            None,
        ),
        Admission::Coverage,
        "the configured glob admits a custom artifact path under build/"
    );
}

/// Even a (pathological) artifact glob that would match inside `.logos/`/`.git/`
/// cannot re-open the self-trigger feedback loop — the feedback filter is checked
/// before the allow-list ([ADR-11], [ADR-38]).
#[test]
fn an_artifact_glob_can_never_re_admit_the_feedback_loop_dirs() {
    let root = Path::new("/project");
    let ig = ignored_set();
    let m = matcher_with(&["**/*.xml"]);
    assert_eq!(
        classify(root, Path::new("/project/.logos/history.xml"), &ig, &m, None),
        Admission::Ignored,
        "a `**/*.xml` glob must NOT re-admit a path under .logos/"
    );
    assert_eq!(
        classify(root, Path::new("/project/.git/x.xml"), &ig, &m, None),
        Admission::Ignored
    );
}

/// CR-054 / [FR-SY-11]: the walk-level admission pre-filter (`classify` step 4).
/// The other `classify` tests pass `authority: None` (fictional `/project`
/// paths); this one builds a real [`AdmissionAuthority`] over a temp tree so the
/// gitignore/boundary verdicts — and the load-bearing **existence guard** — are
/// exercised. Without this, dropping the `path.exists() &&` guard (which would
/// swallow deletion events and leak removed files' nodes until the next
/// reconcile) would pass every other test.
#[test]
fn classify_pre_filters_gitignored_and_boundary_paths_but_lets_deletions_through() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();

    // A root `.gitignore` (a present file + a name we never create, to model a
    // deletion), an ordinary source file, and a nested-`.git` boundary under a
    // name that is NOT an `ignored_dirs` entry (so only the boundary rule can
    // exclude it — isolating the authority from the name-based `is_ignored`).
    std::fs::write(root.join(".gitignore"), "secret.rs\ndeleted.rs\n").unwrap();
    std::fs::write(root.join("secret.rs"), "fn s() {}\n").unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    std::fs::create_dir_all(root.join("nested")).unwrap();
    std::fs::write(root.join("nested/.git"), "gitdir: /elsewhere\n").unwrap();
    std::fs::write(root.join("nested/copy.rs"), "fn c() {}\n").unwrap();

    // Clear the default exclude globs so only the gitignore/boundary gates decide;
    // keep the default `ignored_dirs` (none of which name `src`/`nested`).
    let config = crate::config::Config {
        exclude: vec![],
        ..crate::config::Config::default()
    };
    let authority = AdmissionAuthority::from_config(&root, &config).unwrap();
    let ig = ignored_set();
    let m = builtin_matcher();

    // An existing gitignored file → Ignored (the CR-054 leak surface).
    assert_eq!(
        classify(&root, &root.join("secret.rs"), &ig, &m, Some(&authority)),
        Admission::Ignored,
        "an existing gitignored path is dropped by the pre-filter"
    );
    // An existing nested-`.git`-boundary path → Ignored.
    assert_eq!(
        classify(&root, &root.join("nested/copy.rs"), &ig, &m, Some(&authority)),
        Admission::Ignored,
        "an existing nested-`.git`-boundary path is dropped by the pre-filter"
    );
    // An ordinary admitted source → Source (the pre-filter is not over-broad).
    assert_eq!(
        classify(&root, &root.join("src/main.rs"), &ig, &m, Some(&authority)),
        Admission::Source,
        "an admitted source path passes the pre-filter"
    );
    // A NON-EXISTENT gitignored path (a deletion event) → Source: the existence
    // guard lets it fall through to `sync`'s removal arm. This is the guard the
    // impl comment calls load-bearing — asserted so a regression cannot pass.
    assert!(!root.join("deleted.rs").exists());
    assert_eq!(
        classify(&root, &root.join("deleted.rs"), &ig, &m, Some(&authority)),
        Admission::Source,
        "a deletion of a gitignored path falls through to sync (existence guard)"
    );
}

// ── Drop-and-coalesce slot semantics (AQ-01) + artifact routing (FR-CV-10) ───

/// A debounced batch whose paths all filter away (internal/ignored churn)
/// must not wake the worker at all: a `.logos`/`target` write storm costs
/// zero syncs.
#[test]
fn ignored_only_batch_never_wakes_the_worker() {
    let h = Harness::new();
    on_debounced(
        &h.sink(),
        Ok(debounced_events(&[
            "/project/.logos/logos.db-wal",
            "/project/.git/index",
            "/project/target/debug/deps/foo.rlib",
        ])),
    );

    assert!(
        h.wake_rx.try_recv().is_err(),
        "no wake for internal/ignored churn"
    );
    assert_eq!(h.counters.snapshot().paths_accepted, 0);
    assert_eq!(h.counters.snapshot().artifacts_accepted, 0);
    assert_eq!(h.counters.snapshot().batches_delivered, 1);
}

/// Storm semantics: many batches while a wake is already pending coalesce —
/// the slot holds exactly one wake, the set holds the union of paths, and
/// every extra batch is counted as coalesced (the "drop" half of AQ-01).
#[test]
fn storm_batches_coalesce_into_one_pending_wake() {
    let h = Harness::new();

    // Nobody is draining the channel — this models a worker busy in a long
    // sync while the storm continues.
    for i in 0..10 {
        let path = format!("/project/src/file{i}.rs");
        on_debounced(&h.sink(), Ok(debounced_events(&[&path])));
    }

    let stats = h.counters.snapshot();
    assert_eq!(stats.batches_delivered, 10);
    assert_eq!(stats.paths_accepted, 10);
    // First batch took the slot; the other nine coalesced into it.
    assert_eq!(stats.wakes_coalesced, 9);
    // Exactly one wake is deliverable…
    assert!(h.wake_rx.try_recv().is_ok());
    assert!(h.wake_rx.try_recv().is_err());
    // …and the pending set carries the union for that single wake to drain.
    assert_eq!(h.sources.lock().unwrap().len(), 10);
}

/// A coverage artifact lands in the SEPARATE artifact set (never the source
/// set), is counted as an artifact, and wakes the worker ([FR-CV-10]). A source
/// edit in the same batch routes to the source set — the two are partitioned.
#[test]
fn coverage_artifact_and_source_route_to_separate_sets() {
    let h = Harness::new();
    on_debounced(
        &h.sink(),
        Ok(debounced_events(&[
            "/project/src/main.rs",
            "/project/target/llvm-cov/cobertura.xml",
        ])),
    );

    let stats = h.counters.snapshot();
    assert_eq!(stats.paths_accepted, 1, "one source path");
    assert_eq!(stats.artifacts_accepted, 1, "one coverage artifact");
    assert_eq!(
        h.sources.lock().unwrap().iter().collect::<Vec<_>>(),
        vec![&PathBuf::from("/project/src/main.rs")],
        "the source set holds only the source path"
    );
    assert_eq!(
        h.artifacts_pending.lock().unwrap().iter().collect::<Vec<_>>(),
        vec![&PathBuf::from("/project/target/llvm-cov/cobertura.xml")],
        "the artifact set holds only the artifact path"
    );
    // The artifact alone is enough to wake the worker.
    assert!(h.wake_rx.try_recv().is_ok());
}

/// Duplicate events for the same path occupy one pending entry — the set is
/// bounded by distinct dirty files, not event count.
#[test]
fn duplicate_paths_coalesce_in_the_pending_set() {
    let h = Harness::new();
    for _ in 0..5 {
        on_debounced(&h.sink(), Ok(debounced_events(&["/project/src/main.rs"])));
    }

    assert_eq!(h.sources.lock().unwrap().len(), 1);
    // Only the first insertion counts as accepted; re-dirtying an
    // already-pending path is a no-op.
    assert_eq!(h.counters.snapshot().paths_accepted, 1);
}

/// A watch *error* batch is logged and dropped without waking the worker or
/// poisoning anything — reconcile is the backstop (FR-SY-06).
#[test]
fn error_batches_are_dropped_silently() {
    let h = Harness::new();
    on_debounced(&h.sink(), Err(Vec::new()));

    assert!(h.wake_rx.try_recv().is_err());
    assert_eq!(h.counters.snapshot().batches_delivered, 0);
}

/// A disconnected worker (shutdown race) makes the wake a silent no-op.
#[test]
fn wake_after_worker_shutdown_is_a_noop() {
    let mut h = Harness::new();
    h.disconnect_worker();
    on_debounced(&h.sink(), Ok(debounced_events(&["/project/src/main.rs"])));

    // Accepted into the set (harmless), but no panic and no coalesce count.
    assert_eq!(h.counters.snapshot().wakes_coalesced, 0);
}

// ── No-suite-in-serve posture (ADR-38, NFR-SE-01) ────────────────────────────

/// The load-bearing boundary: the watcher/serve path NEVER spawns a subprocess.
/// Auto-ingest is a local read+parse+store; only the explicit `coverage refresh`
/// (which lives in the engine, not here) ever runs a command. A structural guard
/// over the module source so a future edit can't silently re-introduce a spawn.
#[test]
fn the_watch_module_spawns_no_subprocess() {
    let src = include_str!("mod.rs");
    assert!(
        !src.contains("process::Command") && !src.contains("Command::new"),
        "the watch module must spawn no subprocess (ADR-38 no-suite-in-serve boundary)"
    );
    assert!(
        !src.contains("refresh_cmd"),
        "refresh_cmd belongs to the explicit `coverage refresh` engine path, never the watcher"
    );
}

// ── helpers ─────────────────────────────────────────────────────────────────

/// The built-in coverage-artifact matcher (conventions only, no configured glob).
fn builtin_matcher() -> ArtifactMatcher {
    ArtifactMatcher::compile(&crate::config::EffectiveCoverageIngest::default())
        .expect("the conventions compile")
}

/// A matcher whose configured globs extend the conventions.
fn matcher_with(globs: &[&str]) -> ArtifactMatcher {
    ArtifactMatcher::compile(&crate::config::EffectiveCoverageIngest {
        artifact_glob: globs.iter().map(|s| (*s).to_string()).collect(),
        format: "auto".to_string(),
        refresh_cmd: None,
    })
    .expect("the configured globs compile")
}

/// Owns the debouncer-callback's shared state for a test and hands out a
/// borrowing [`DebounceSink`] per `on_debounced` call.
struct Harness {
    root: PathBuf,
    ignored_dirs: HashSet<String>,
    artifacts: ArtifactMatcher,
    /// The best-effort admission pre-filter (CR-054). `None` here: these tests use
    /// fictional `/project` paths and exercise the coverage/ignored-dir routing,
    /// not the walk-level authority (whose parity is proven in `config::admission`
    /// and end-to-end in the `pipeline` integration tests). `None` keeps the
    /// existing routing behaviour under test unchanged.
    authority: Option<AdmissionAuthority>,
    sources: Mutex<HashSet<PathBuf>>,
    artifacts_pending: Mutex<HashSet<PathBuf>>,
    wake_tx: Sender<()>,
    wake_rx: Receiver<()>,
    counters: Counters,
}

impl Harness {
    fn new() -> Self {
        let (wake_tx, wake_rx) = bounded::<()>(1);
        Harness {
            root: PathBuf::from("/project"),
            ignored_dirs: ignored_set(),
            artifacts: builtin_matcher(),
            authority: None,
            sources: Mutex::new(HashSet::new()),
            artifacts_pending: Mutex::new(HashSet::new()),
            wake_tx,
            wake_rx,
            counters: Counters::default(),
        }
    }

    /// Drop the receiver to model a worker that shut down mid-storm.
    fn disconnect_worker(&mut self) {
        let (tx, _rx) = bounded::<()>(1);
        // Replace the live channel with one whose receiver is already dropped.
        self.wake_tx = tx;
    }

    fn sink(&self) -> DebounceSink<'_> {
        DebounceSink {
            root: &self.root,
            ignored_dirs: &self.ignored_dirs,
            artifacts: &self.artifacts,
            authority: &self.authority,
            pending_sources: &self.sources,
            pending_artifacts: &self.artifacts_pending,
            wake_tx: &self.wake_tx,
            counters: &self.counters,
        }
    }
}

/// Build a debounced-event batch carrying the given paths.
fn debounced_events(paths: &[&str]) -> Vec<notify_debouncer_full::DebouncedEvent> {
    paths
        .iter()
        .map(|p| {
            let mut event = notify::Event::new(notify::EventKind::Modify(
                notify::event::ModifyKind::Data(notify::event::DataChange::Content),
            ));
            event = event.add_path(PathBuf::from(p));
            notify_debouncer_full::DebouncedEvent::new(event, Instant::now())
        })
        .collect()
}
