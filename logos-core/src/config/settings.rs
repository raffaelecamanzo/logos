//! [`Config`] — the parsed `.logos/config.toml` read-model ([FR-CF-01]).
//!
//! `Config` is the read-model the [pipeline-orchestrator] consumes (languages,
//! include/exclude globs, `max_file_size`, framework hints, watcher debounce).
//! Every field has a sensible default, so an empty (or absent) `config.toml`
//! deserialises to [`Config::default`] — the "sensible defaults when fields are
//! omitted" half of [FR-CF-01]. `#[serde(deny_unknown_fields)]` on every struct
//! supplies the other half: an unknown key fails loud (exit 2).
//!
//! This is checked-in *policy* that travels into every worktree ([NFR-DM-04],
//! [ADR-15]); the derived DBs do not.
//!
//! [FR-CF-01]: ../../../../docs/specs/requirements/FR-CF-01.md
//! [NFR-DM-04]: ../../../../docs/specs/requirements/NFR-DM-04.md
//! [ADR-15]: ../../../../docs/specs/architecture/decisions/ADR-15.md
//! [pipeline-orchestrator]: ../../../../docs/specs/architecture/components/pipeline-orchestrator.md

use serde::{Deserialize, Serialize};

use super::chat::ChatConfig;
use super::secrets::Secrets;
use super::wiki::{EffectiveWikiModel, WikiConfig};

/// `max_file_size` default: 2 MiB ([FR-CF-04]). Files above this are skipped
/// during discovery with a notice.
///
/// [FR-CF-04]: ../../../../docs/specs/requirements/FR-CF-04.md
pub const DEFAULT_MAX_FILE_SIZE: u64 = 2 * 1024 * 1024;

/// The default code-language admission allowlist: **empty**, meaning every
/// compiled-in code grammar is admitted — the twelve-out-of-the-box default
/// ([CR-009]/[FR-PL-01]). A non-empty `languages` restricts code admission to the
/// named grammars (CR-017/S-081, [`Config::language_allowlist`]); it never
/// *enables* a grammar the build did not compile in.
///
/// [FR-PL-01]: ../../../../docs/specs/requirements/FR-PL-01.md
/// [CR-009]: ../../../../docs/requests/CR-009-seven-language-plugins.md
fn default_languages() -> Vec<String> {
    Vec::new()
}

/// Default include set: `**` — every path under the root. Discovery then
/// subtracts the gitignore ∪ excludes ∪ `ignored_dirs` union.
fn default_include() -> Vec<String> {
    vec!["**".to_string()]
}

fn default_max_file_size() -> u64 {
    DEFAULT_MAX_FILE_SIZE
}

/// Directory *names* pruned anywhere in the tree by default ([FR-IX-02],
/// [FR-CF-05]). A name here prunes a directory wherever it appears in the tree,
/// so only names that are build-output / tooling conventions across the board
/// belong here — anything that must be path-anchored (e.g. `docs/planning`) is
/// an [`default_exclude`] glob instead ([FR-CF-05] Notes).
///
/// `.logos/` is included so Logos never indexes its own derived store
/// ([NFR-SE-04] "no writes outside `.logos/`" — and nothing reads back in).
///
/// Broadened by [CR-029]/[FR-CF-05] beyond the original seven
/// (`target`, `node_modules`, `dist`, `build`, `vendor`, `.git`, `.logos`):
/// the agent/tooling dirs `.agents` and `.claude`, and the standard
/// per-language build/output directory names listed below. Every entry stays
/// fully user-overridable in `config.toml` ([FR-CF-02], CRA-03).
///
/// [CR-054] adds `.worktrees` and `.playwright-mcp` — scratch dirs a `sync`/
/// watcher path could admit if a project doesn't gitignore them, defended
/// belt-and-suspenders alongside the gitignore/nested-`.git`-boundary fix.
/// Neither name collides with a real source directory in any supported
/// ecosystem (both are dot-prefixed, tool-specific names — not a Go/Java/
/// Rust/Python/JS package or module convention).
///
/// [FR-IX-02]: ../../../../docs/specs/requirements/FR-IX-02.md
/// [FR-CF-02]: ../../../../docs/specs/requirements/FR-CF-02.md
/// [FR-CF-05]: ../../../../docs/specs/requirements/FR-CF-05.md
/// [NFR-SE-04]: ../../../../docs/specs/requirements/NFR-SE-04.md
/// [CR-029]: ../../../../docs/requests/CR-029-graph-layer-visibility-and-canvas-fixes.md
/// [CR-054]: ../../../../docs/requests/CR-054-graph-update-admission-unification.md
fn default_ignored_dirs() -> Vec<String> {
    [
        // Originals (pre-CR-029): build outputs + VCS + the derived store.
        "target",
        "node_modules",
        "dist",
        "build",
        "vendor",
        ".git",
        ".logos",
        // Agent/tooling dirs ([FR-CF-05]).
        ".agents",
        ".claude",
        // Per-language build / output / cache dirs ([FR-CF-05]).
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
        // Scratch dirs ([CR-054] / [FR-CF-05]).
        ".worktrees",
        ".playwright-mcp",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect()
}

/// Default code `exclude` globs ([FR-CF-05]): the planning/security/notes prose
/// paths that are noise in the code/doc graph by default. These are
/// **root-anchored** path globs (not [`default_ignored_dirs`] names) so they
/// prune exactly `docs/planning/`, `docs/security/`, and a root `notes/` — both
/// the code *and* the documentation under them — without touching a same-named
/// directory nested elsewhere in real source.
///
/// Unioned with gitignore and `ignored_dirs` during discovery ([FR-CF-02]);
/// fully user-overridable — a `config.toml` `exclude` replaces this set
/// wholesale (CRA-03, [CR-029]).
///
/// [FR-CF-05]: ../../../../docs/specs/requirements/FR-CF-05.md
/// [FR-CF-02]: ../../../../docs/specs/requirements/FR-CF-02.md
/// [CR-029]: ../../../../docs/requests/CR-029-graph-layer-visibility-and-canvas-fixes.md
fn default_exclude() -> Vec<String> {
    ["docs/planning/**", "docs/security/**", "notes/**"]
        .iter()
        .map(|s| (*s).to_string())
        .collect()
}

/// Default dead-code entry points ([FR-AN-01]): `main` — the conventional
/// binary root. Exported symbols and framework `route` nodes are *always*
/// reachability roots regardless of this list; `entry_points` adds the
/// project's named roots on top (additional binaries, scheduled jobs, FFI
/// entry symbols).
///
/// [FR-AN-01]: ../../../../docs/specs/requirements/FR-AN-01.md
fn default_entry_points() -> Vec<String> {
    vec!["main".to_string()]
}

/// Default test markers (S-020, [FR-GV-08]): the conventional test
/// vocabulary across the v1 language set.
///
/// [FR-GV-08]: ../../../../docs/specs/requirements/FR-GV-08.md
fn default_test_markers() -> Vec<String> {
    ["test", "tests", "spec"]
        .iter()
        .map(|s| (*s).to_string())
        .collect()
}

/// Documentation indexing default: **on** ([FR-DG-01], [CR-003]). Documentation
/// is a first-class graph layer available on every repo out of the box; the
/// toggle exists so a project can opt out ([ADR-19]).
///
/// [FR-DG-01]: ../../../../docs/specs/requirements/FR-DG-01.md
/// [CR-003]: ../../../../docs/requests/CR-003-documentation-graph-layer.md
/// [ADR-19]: ../../../../docs/specs/architecture/decisions/ADR-19.md
fn default_doc_enabled() -> bool {
    true
}

/// Default documentation include globs ([FR-DG-01]): markdown under `docs/`,
/// any top-level `*.md`, and a root `README*`. These compile with **anchored**
/// (`literal_separator`) semantics (see [`super::globs::compile_anchored`]), so
/// `*.md` is top-level only and `README*` is the root README — the precise
/// scoping [FR-DG-01] names.
///
/// [FR-DG-01]: ../../../../docs/specs/requirements/FR-DG-01.md
fn default_doc_include() -> Vec<String> {
    ["docs/**/*.md", "*.md", "README*"]
        .iter()
        .map(|s| (*s).to_string())
        .collect()
}

/// Config-artifact indexing default: **on** ([FR-CG-01], [CR-010]). The config &
/// artifact graph layer is available on every repo out of the box; the toggle
/// exists so a project can opt out ([ADR-25]).
///
/// [FR-CG-01]: ../../../../docs/specs/requirements/FR-CG-01.md
/// [CR-010]: ../../../../docs/requests/CR-010-config-artifact-graph-layer.md
/// [ADR-25]: ../../../../docs/specs/architecture/decisions/ADR-25.md
fn default_config_enabled() -> bool {
    true
}

/// Default config-artifact include globs ([FR-CG-02]): `**` — every path is a
/// candidate, with the actual filter being the per-descriptor extension/basename
/// claim plus the default lock-file excludes below. Non-anchored (code-glob)
/// semantics (see [`super::globs::ConfigGlobs`]).
fn default_config_include() -> Vec<String> {
    vec!["**".to_string()]
}

/// Default config-artifact exclude globs ([FR-CG-02], [BR-30]): the generated /
/// lock files that are noise by default — `package-lock.json`, `Cargo.lock`,
/// `yarn.lock`, `pnpm-lock.yaml`, and `*.min.json`. Each is written `**/name`
/// (non-anchored) so it matches the file at the root and at any nested depth.
/// Overridable in `config.toml` to re-admit a lock file.
fn default_config_exclude() -> Vec<String> {
    [
        "**/package-lock.json",
        "**/Cargo.lock",
        "**/yarn.lock",
        "**/pnpm-lock.yaml",
        "**/*.min.json",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect()
}

/// Watcher debounce default (ms) — **300**, the resolved SRS OQ-04 value
/// (stakeholder decision 2026-06-02; see the filesystem-watcher integration
/// spec). The debounced `notify` watcher ([FR-SY-04]) coalesces bursts of
/// edits within this window into one batched sync.
///
/// [FR-SY-04]: ../../../../docs/specs/requirements/FR-SY-04.md
fn default_debounce_ms() -> u64 {
    300
}

/// Default settle window (ms): after a debounced batch, the worker waits this
/// long with no further change before syncing, so a machine-speed edit burst
/// coalesces into one sync instead of one-per-debounce-window (CR-015). Each new
/// change resets the timer.
fn default_settle_ms() -> u64 {
    1_500
}

/// Default rate-limit floor (ms): the worker never starts two syncs closer than
/// this, so rapid settles can't drive back-to-back whole-graph work (CR-015).
fn default_min_sync_interval_ms() -> u64 {
    3_000
}

/// Default staleness cap (ms): the worker syncs anyway once the oldest pending
/// change is this old, so a continuous (never-quiet) edit stream still refreshes
/// periodically rather than deferring forever (CR-015).
fn default_max_staleness_ms() -> u64 {
    10_000
}

/// The parsed `config.toml` read-model.
///
/// `Eq` is intentionally **not** derived: the `[chat]` section's
/// [`temperature`](ChatConfig::temperature) is an `f64` (S-169), so the contract
/// is only `PartialEq` — the same posture [`Rules`](super::Rules) takes for its
/// own float budgets.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Language plugins to enable (defaults to the curated v1 set).
    #[serde(default = "default_languages")]
    pub languages: Vec<String>,

    /// Include globs — a file is discovered only if it matches one
    /// (default `["**"]`, i.e. everything). Matched against the root-relative path.
    #[serde(default = "default_include")]
    pub include: Vec<String>,

    /// Exclude globs — unioned with gitignore and `ignored_dirs` during
    /// discovery ([FR-CF-02]). A file matching any exclude is not indexed.
    /// Defaults to the [`default_exclude`] planning/security/notes set
    /// ([FR-CF-05]); a `config.toml` `exclude` replaces it wholesale.
    ///
    /// [FR-CF-02]: ../../../../docs/specs/requirements/FR-CF-02.md
    /// [FR-CF-05]: ../../../../docs/specs/requirements/FR-CF-05.md
    #[serde(default = "default_exclude")]
    pub exclude: Vec<String>,

    /// Files larger than this (bytes) are skipped with a notice ([FR-CF-04]).
    #[serde(default = "default_max_file_size")]
    pub max_file_size: u64,

    /// Framework hints biasing route/component extraction ([FR-FW-03]).
    /// Best-effort; consumed by the framework extractors in a later story.
    ///
    /// [FR-FW-03]: ../../../../docs/specs/requirements/FR-FW-03.md
    #[serde(default)]
    pub framework_hints: Vec<String>,

    /// Discovery/semantics knobs (`[semantics]`).
    #[serde(default)]
    pub semantics: Semantics,

    /// Resolution-engine knobs (`[resolution]`, S-011).
    #[serde(default)]
    pub resolution: Resolution,

    /// Filesystem-watcher knobs (`[watcher]`).
    #[serde(default)]
    pub watcher: Watcher,

    /// Documentation indexing knobs (`[documentation]`, S-034, [CR-003]).
    ///
    /// [CR-003]: ../../../../docs/requests/CR-003-documentation-graph-layer.md
    #[serde(default)]
    pub documentation: Documentation,

    /// Config-artifact indexing knobs (`[config_artifacts]`, S-062, [CR-010]).
    ///
    /// [CR-010]: ../../../../docs/requests/CR-010-config-artifact-graph-layer.md
    #[serde(default)]
    pub config_artifacts: ConfigArtifacts,

    /// Automatic coverage-ingest knobs (`[coverage_ingest]`, [FR-CV-10],
    /// [CR-036], [ADR-38]). Optional; an absent table behaves as all-defaults
    /// (the built-in convention artifact paths, `auto` format, no refresh
    /// command). Tunes the **advisory** coverage tier only — never the gate
    /// ([BR-28]).
    ///
    /// [FR-CV-10]: ../../../../docs/specs/requirements/FR-CV-10.md
    /// [CR-036]: ../../../../docs/requests/CR-036-automatic-coverage-ingest-and-coverage-cross-quadrant.md
    /// [ADR-38]: ../../../../docs/specs/architecture/decisions/ADR-38.md
    #[serde(default)]
    pub coverage_ingest: CoverageIngest,

    /// Agentic-chat policy + the orchestrator budget tree (`[chat]`, S-169,
    /// [FR-CF-06], [ADR-40], [ADR-41]). Optional; an absent table behaves as
    /// all-defaults (OpenRouter `base_url`, the documented budget-tree defaults).
    /// The **API key is not here** — it lives in the gitignored
    /// [`secrets.toml`](super::secrets) ([NFR-SE-07]). Chat is `ui`-only at
    /// runtime, but its *policy* is parsed in the core like every other table so
    /// the substrate needs no `ui`-only config dependency.
    ///
    /// [FR-CF-06]: ../../../../docs/specs/requirements/FR-CF-06.md
    /// [NFR-SE-07]: ../../../../docs/specs/requirements/NFR-SE-07.md
    /// [ADR-40]: ../../../../docs/specs/architecture/decisions/ADR-40.md
    /// [ADR-41]: ../../../../docs/specs/architecture/decisions/ADR-41.md
    #[serde(default)]
    pub chat: ChatConfig,

    /// Dedicated wiki generation model, distinct from [`chat`](Self::chat)
    /// (`[wiki]`, [CR-047], [FR-CF-07], [ADR-42]). Optional; an absent table
    /// resolves to the chat model ([`Config::effective_wiki_model`]). There is
    /// no separate wiki `provider`/`base_url`/secret — those are always
    /// inherited from `[chat]`/`secrets.toml`.
    ///
    /// [CR-047]: ../../../../docs/requests/CR-047-internal-wiki-generation-on-agent-substrate.md
    /// [FR-CF-07]: ../../../../docs/specs/requirements/FR-CF-07.md
    /// [ADR-42]: ../../../../docs/specs/architecture/decisions/ADR-42.md
    #[serde(default)]
    pub wiki: WikiConfig,
}

/// The `[semantics]` table: discovery pruning ([FR-IX-02]), dead-code roots
/// (S-014), and test markers (S-020).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Semantics {
    /// Directory names pruned anywhere in the tree during discovery
    /// ([FR-IX-02], [FR-CF-02]).
    #[serde(default = "default_ignored_dirs")]
    pub ignored_dirs: Vec<String>,

    /// Declared dead-code reachability roots, matched against node *names*
    /// (S-014, [FR-AN-01]). Defaults to `["main"]`.
    ///
    /// [FR-AN-01]: ../../../../docs/specs/requirements/FR-AN-01.md
    #[serde(default = "default_entry_points")]
    pub entry_points: Vec<String>,

    /// Test markers for test-gap analysis (S-020, [FR-GV-08]): a node is
    /// test-marked when its file matches the built-in path conventions
    /// (`tests/` segments, `*_test.*`, `*.spec.*`, …) **or** its name starts
    /// with `<marker>_` / ends with `_<marker>` for any marker here
    /// (ratified 2026-06-06: path conventions + marker name-affix).
    /// Defaults to `["test", "tests", "spec"]`.
    ///
    /// [FR-GV-08]: ../../../../docs/specs/requirements/FR-GV-08.md
    #[serde(default = "default_test_markers")]
    pub test_markers: Vec<String>,
}

/// The `[resolution]` table — resolution-engine knobs (S-011).
///
/// Checked-in policy like everything else here ([ADR-15]): the binder's
/// aggressiveness is a per-project decision (blast radius of a mis-bind vs the
/// cost of unresolved refs), not a compile-time constant. Because resolution
/// re-evaluates the whole `unresolved_refs` ledger on every index/sync,
/// changing the policy needs no migration — run `logos index` and the graph
/// re-binds under the new rules.
///
/// [ADR-15]: ../../../../docs/specs/architecture/decisions/ADR-15.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Resolution {
    /// How aggressively the binder falls back beyond scope-proven matches.
    #[serde(default)]
    pub policy: BindingPolicy,
}

/// The binder's fallback aggressiveness ([NFR-RA-05], [AR-05]).
///
/// **Every** policy preserves the never-fabricate invariant structurally: an
/// edge is created only when the candidate search yields **exactly one**
/// existing node. The policy widens the *search*, never the acceptance rule —
/// so the trade is recall (coverage) against the residual mis-bind risk of
/// inference, [AR-05]'s highest product risk.
///
/// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
/// [AR-05]: ../../../../docs/specs/architecture.md#13-risk-register
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum BindingPolicy {
    /// Scope-proven bindings only: function-local → enclosing scopes → module
    /// → `use`-aliases/globs → explicit `crate`/`self`/`super` paths. A
    /// receiver-method call (`x.f()`) binds only when its name resolves within
    /// that scope hierarchy (a sibling or module-level callable) — never through
    /// the workspace name-match, whose receiver type is unknown ([CR-066]).
    /// Maximum precision, lowest recall.
    Strict,
    /// `strict`, plus one exactly-one-candidate workspace fallback for **path**
    /// calls: a multi-segment path binds on a unique module-path suffix match
    /// (crate-first, then workspace). The workspace name fallback is **disabled
    /// for receiver-unqualified method calls** (`x.f()`) — extraction discards
    /// the receiver, so a bare workspace name-match is not evidence and would
    /// fabricate a `Calls` edge ([FR-RS-06], [NFR-RA-05], [CR-066]); such a call
    /// binds only on genuine scope evidence (as under `strict`), else stays
    /// unresolved and retries on sync. The default.
    ///
    /// [FR-RS-06]: ../../../../docs/specs/requirements/FR-RS-06.md
    /// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
    /// [CR-066]: ../../../../docs/requests/CR-066-receiver-method-overbinding.md
    #[default]
    Balanced,
    /// `balanced`, plus a bare single identifier binds on a workspace-unique
    /// name. Highest recall; weakest (still deterministic, still
    /// exactly-one-candidate) inference.
    Aggressive,
}

/// The `[documentation]` table — documentation indexing policy (S-034,
/// [CR-003], [ADR-19]).
///
/// Documentation rides the same discover → extract → persist pipeline, blake3
/// dirty-detection, git hooks, and watcher as code, with no new change-detection
/// ([FR-DG-01]). This table only decides **whether** docs are indexed and
/// **which** markdown files count as documentation; everything downstream is the
/// existing machinery.
///
/// The globs use **anchored** semantics (`literal_separator`), distinct from the
/// code [`Config::include`]/[`Config::exclude`] globs, so the default
/// top-level `*.md` and `README*` mean exactly that (see
/// [`super::globs::compile_anchored`]).
///
/// [FR-DG-01]: ../../../../docs/specs/requirements/FR-DG-01.md
/// [CR-003]: ../../../../docs/requests/CR-003-documentation-graph-layer.md
/// [ADR-19]: ../../../../docs/specs/architecture/decisions/ADR-19.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Documentation {
    /// Whether markdown documentation is discovered and indexed (default `true`,
    /// [FR-DG-01]). When `false`, no `DocFile`/`DocSection` node is produced.
    #[serde(default = "default_doc_enabled")]
    pub enabled: bool,

    /// Doc include globs — a markdown file is admitted as documentation only if
    /// it matches one (default `["docs/**/*.md", "*.md", "README*"]`). Matched
    /// against the root-relative path with anchored semantics.
    #[serde(default = "default_doc_include")]
    pub include: Vec<String>,

    /// Doc exclude globs — a markdown file matching any of these is not indexed
    /// as documentation even if it matched an include (default empty).
    #[serde(default)]
    pub exclude: Vec<String>,

    /// swe-skills typed-node enrichment policy (S-039, [FR-DG-07]). Default
    /// [`TypedEnrichment::Auto`] — promote `FR-*`/`ADR-*`/`S-NNN` artifacts to
    /// typed `Requirement`/`Adr`/`Story` nodes only when the convention files
    /// are detected. Additive and never required: a plain repo (or `disabled`)
    /// produces only generic `DocFile`/`DocSection` nodes.
    ///
    /// [FR-DG-07]: ../../../../docs/specs/requirements/FR-DG-07.md
    #[serde(default)]
    pub typed_enrichment: TypedEnrichment,
}

impl Documentation {
    /// Compile the doc globs into a matcher, or `None` when documentation is
    /// disabled (`enabled = false`) — the pipeline reads `None` as "admit no
    /// markdown".
    ///
    /// # Errors
    /// [`ConfigError`](super::error::ConfigError) if a doc glob is malformed or
    /// would escape the project root ([NFR-SE-04]).
    pub(crate) fn compile(
        &self,
    ) -> Result<Option<super::globs::DocGlobs>, super::error::ConfigError> {
        if !self.enabled {
            return Ok(None);
        }
        super::globs::DocGlobs::compile(&self.include, &self.exclude).map(Some)
    }

    /// Resolve whether typed-node enrichment is active for this run (S-039,
    /// [FR-DG-07]).
    ///
    /// `conventions_detected` is the auto-detection signal — whether the indexed
    /// file set carries the swe-skills convention layout (requirement/ADR files),
    /// computed by [`crate::extract::doc::enrich::conventions_present`]. The
    /// config override wins over it: [`TypedEnrichment::Enabled`]/[`Disabled`]
    /// force the answer, [`TypedEnrichment::Auto`] defers to detection. When
    /// documentation is disabled there are no doc nodes to promote, so the answer
    /// is moot but reported `false` for honesty.
    ///
    /// [FR-DG-07]: ../../../../docs/specs/requirements/FR-DG-07.md
    pub(crate) fn enrichment_active(&self, conventions_detected: bool) -> bool {
        if !self.enabled {
            return false;
        }
        match self.typed_enrichment {
            TypedEnrichment::Enabled => true,
            TypedEnrichment::Disabled => false,
            TypedEnrichment::Auto => conventions_detected,
        }
    }
}

/// The `[config_artifacts]` table — config & artifact indexing policy (S-062,
/// [CR-010], [ADR-25]).
///
/// Mirrors [`Documentation`] in shape — a separate layer with the same
/// discover → extract → persist machinery, blake3 dirty-detection, hooks, and
/// watcher, with no new change-detection. This table only decides **whether**
/// config artifacts are indexed and **which** files count (on top of the
/// per-descriptor extension/basename claim); everything downstream is the
/// existing pipeline.
///
/// The globs use **non-anchored** (code-glob) semantics — distinct from the
/// anchored `[documentation]` globs — so the default lock-file excludes match at
/// any depth (see [`super::globs::ConfigGlobs`]).
///
/// [CR-010]: ../../../../docs/requests/CR-010-config-artifact-graph-layer.md
/// [ADR-25]: ../../../../docs/specs/architecture/decisions/ADR-25.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigArtifacts {
    /// Whether config artifacts are discovered and indexed (default `true`,
    /// [FR-CG-01]). When `false`, no `ConfigFile`/`ConfigSection` (or typed
    /// anchor) node is produced.
    #[serde(default = "default_config_enabled")]
    pub enabled: bool,

    /// Config include globs (default `["**"]`): a candidate file must match one.
    /// The real gate is the per-descriptor extension/basename claim; this exists
    /// for symmetry and project-level narrowing.
    #[serde(default = "default_config_include")]
    pub include: Vec<String>,

    /// Config exclude globs (default: the [FR-CG-02]/[BR-30] lock-file set). A
    /// claimed file matching any of these is not indexed even if it matched an
    /// include — overridable to re-admit a lock file.
    #[serde(default = "default_config_exclude")]
    pub exclude: Vec<String>,
}

impl ConfigArtifacts {
    /// Compile the config globs into a matcher, or `None` when the layer is
    /// disabled (`enabled = false`) — the pipeline reads `None` as "admit no
    /// config artifact", exactly as [`Documentation::compile`] does for docs.
    ///
    /// # Errors
    /// [`ConfigError`](super::error::ConfigError) if a config glob is malformed or
    /// would escape the project root ([NFR-SE-04]).
    pub(crate) fn compile(
        &self,
    ) -> Result<Option<super::globs::ConfigGlobs>, super::error::ConfigError> {
        if !self.enabled {
            return Ok(None);
        }
        super::globs::ConfigGlobs::compile(&self.include, &self.exclude).map(Some)
    }
}

impl Default for ConfigArtifacts {
    fn default() -> Self {
        ConfigArtifacts {
            enabled: default_config_enabled(),
            include: default_config_include(),
            exclude: default_config_exclude(),
        }
    }
}

/// swe-skills typed-node enrichment policy (`[documentation] typed_enrichment`,
/// S-039, [FR-DG-07], [ADR-19]).
///
/// The enrichment promotes the swe-skills convention artifacts
/// (`docs/specs/requirements/FR-*.md`/`NFR-*.md`, `ADR-NN.md`, journal `S-NNN`
/// stories) from generic `DocFile`/`DocSection` nodes to typed
/// `Requirement`/`Adr`/`Story` nodes with `traces-to` edges. It is **additive**
/// and **never a prerequisite**: the generic markdown layer is primary on any
/// repository, and this only lights up the higher-value traceability where the
/// conventions hold ([ADR-19] "on by default, generic-first").
///
/// [FR-DG-07]: ../../../../docs/specs/requirements/FR-DG-07.md
/// [ADR-19]: ../../../../docs/specs/architecture/decisions/ADR-19.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TypedEnrichment {
    /// Auto-detect from convention files: promote when the swe-skills layout is
    /// present, stay generic-only otherwise. The default — graceful degradation
    /// on any repo with no configuration.
    #[default]
    Auto,
    /// Force enrichment on regardless of detection (a swe-skills repo that the
    /// detector would miss, e.g. a non-standard layout).
    Enabled,
    /// Force enrichment off: only generic `DocFile`/`DocSection` nodes, even
    /// where the conventions are present.
    Disabled,
}

/// The `[watcher]` table — the debounced `notify` watcher under `serve --mcp`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Watcher {
    /// Debounce window in milliseconds (default 300; resolved SRS OQ-04).
    #[serde(default = "default_debounce_ms")]
    pub debounce_ms: u64,
    /// Settle window in milliseconds (CR-015, default 1500): after a debounced
    /// batch, wait this long with no further change before syncing, so an edit
    /// storm coalesces into one sync. Decouples sync cadence from edit cadence.
    #[serde(default = "default_settle_ms")]
    pub settle_ms: u64,
    /// Minimum interval between syncs in milliseconds (CR-015, default 3000): a
    /// rate-limit floor so rapid settles can't drive back-to-back syncs.
    #[serde(default = "default_min_sync_interval_ms")]
    pub min_sync_interval_ms: u64,
    /// Maximum staleness in milliseconds (CR-015, default 10000): sync anyway once
    /// the oldest pending change is this old, so a continuous edit stream still
    /// refreshes periodically.
    #[serde(default = "default_max_staleness_ms")]
    pub max_staleness_ms: u64,
}

/// The `[coverage_ingest]` table — automatic coverage-ingest policy
/// ([FR-CV-10], [CR-036], [ADR-38]).
///
/// The raw parsed shape (every key optional, so an omitted key means "use the
/// documented default"), mirroring the `[coverage]` table in `rules.toml`
/// ([`Coverage`](super::rules::Coverage)). [`CoverageIngest::effective`] resolves
/// the defaults into an [`EffectiveCoverageIngest`] whose
/// [`hash`](EffectiveCoverageIngest::hash) is folded into every coverage snapshot
/// the [BR-27] provenance pattern reapplied ([FR-CV-09]).
///
/// It configures **automatic** ingest only — the watcher watches the
/// discovered/configured artifact path(s) and re-ingests on change, and the
/// opt-in `coverage refresh` runs `refresh_cmd`. Like `[coverage]` it tunes the
/// **non-gated advisory tier** exclusively ([BR-28]); changing it never moves the
/// gate, and it is deliberately excluded from the admission fingerprint (it
/// admits the same source files, so it must not trigger a re-baseline).
///
/// [FR-CV-10]: ../../../../docs/specs/requirements/FR-CV-10.md
/// [FR-CV-09]: ../../../../docs/specs/requirements/FR-CV-09.md
/// [BR-27]: ../../../../docs/specs/software-spec.md#322-git-history-analytics
/// [BR-28]: ../../../../docs/specs/software-spec.md#323-coverage-test-evidence
/// [CR-036]: ../../../../docs/requests/CR-036-automatic-coverage-ingest-and-coverage-cross-quadrant.md
/// [ADR-38]: ../../../../docs/specs/architecture/decisions/ADR-38.md
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoverageIngest {
    /// Glob(s) that **extend** the built-in convention artifact paths the
    /// watcher admits as coverage artifacts (matched against the root-relative
    /// path, non-anchored code-glob semantics). Default empty — the conventions
    /// alone. A configured glob is re-admitted through the `target/`-class ignore
    /// filter as an explicit allow-list exception ([FR-CV-10]).
    #[serde(default)]
    pub artifact_glob: Vec<String>,

    /// Report format for auto-ingest: `"auto"` (default, content-detected),
    /// `"lcov"`, or `"cobertura"`. Only the automatic/refresh path consults this;
    /// the manual `coverage ingest --format` flag is unaffected ([FR-CV-01]).
    pub format: Option<String>,

    /// Optional command the opt-in `coverage refresh` runs (via `sh -c`, cwd =
    /// project root) to regenerate the artifact before ingesting it. This is the
    /// **only** place Logos ever spawns a coverage subprocess, and only on
    /// explicit invocation — never on the serve/watcher path ([ADR-38],
    /// [NFR-SE-01]).
    ///
    /// [NFR-SE-01]: ../../../../docs/specs/requirements/NFR-SE-01.md
    pub refresh_cmd: Option<String>,
}

/// The **effective** coverage-ingest configuration: [`CoverageIngest`] with every
/// default resolved. Its [`hash`](Self::hash) is folded into every coverage
/// snapshot's config hash ([FR-CV-09]), mirroring the
/// [`EffectiveCoverage`](super::rules::EffectiveCoverage) split exactly.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EffectiveCoverageIngest {
    /// Extra artifact globs, in declared order.
    pub artifact_glob: Vec<String>,
    /// The resolved format token: `"auto"` (default), `"lcov"`, or `"cobertura"`.
    pub format: String,
    /// The configured refresh command, if any.
    pub refresh_cmd: Option<String>,
}

/// The default auto-ingest format token ([FR-CV-10]): content auto-detection.
const DEFAULT_COVERAGE_INGEST_FORMAT: &str = "auto";

impl CoverageIngest {
    /// Resolve the raw table into the [`EffectiveCoverageIngest`] set,
    /// substituting the documented default (`auto` format, empty extra globs, no
    /// refresh command) for an omitted key.
    pub fn effective(&self) -> EffectiveCoverageIngest {
        EffectiveCoverageIngest {
            artifact_glob: self.artifact_glob.clone(),
            format: self
                .format
                .clone()
                .unwrap_or_else(|| DEFAULT_COVERAGE_INGEST_FORMAT.to_string()),
            refresh_cmd: self.refresh_cmd.clone(),
        }
    }
}

impl EffectiveCoverageIngest {
    /// A blake3 digest over a fixed-order canonical `key=value` string —
    /// byte-identical across the four CI targets ([NFR-RA-06]). Changing any
    /// `[coverage_ingest]` key changes the hash ([FR-CV-09] acceptance), exactly
    /// as [`EffectiveCoverage::hash`](super::rules::EffectiveCoverage::hash) does
    /// for the `[coverage]` table.
    ///
    /// The globs are joined with a Unit-Separator (`\x1f`) that cannot occur in a
    /// TOML string value, so two glob lists can never collide by concatenation.
    ///
    /// [NFR-RA-06]: ../../../../docs/specs/requirements/NFR-RA-06.md
    /// [FR-CV-09]: ../../../../docs/specs/requirements/FR-CV-09.md
    pub fn hash(&self) -> String {
        let canonical = format!(
            "artifact_glob={};format={};refresh_cmd={}",
            self.artifact_glob.join("\u{1f}"),
            self.format,
            self.refresh_cmd.as_deref().unwrap_or(""),
        );
        blake3::hash(canonical.as_bytes()).to_hex().to_string()
    }

    /// The `--format`-style override the ingest path consumes: `None` for the
    /// default `auto` (content auto-detection), otherwise the explicit token.
    pub fn format_override(&self) -> Option<&str> {
        (self.format != DEFAULT_COVERAGE_INGEST_FORMAT).then_some(self.format.as_str())
    }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            languages: default_languages(),
            include: default_include(),
            exclude: default_exclude(),
            max_file_size: default_max_file_size(),
            framework_hints: Vec::new(),
            semantics: Semantics::default(),
            resolution: Resolution::default(),
            watcher: Watcher::default(),
            documentation: Documentation::default(),
            config_artifacts: ConfigArtifacts::default(),
            coverage_ingest: CoverageIngest::default(),
            chat: ChatConfig::default(),
            wiki: WikiConfig::default(),
        }
    }
}

impl Default for Documentation {
    fn default() -> Self {
        Documentation {
            enabled: default_doc_enabled(),
            include: default_doc_include(),
            exclude: Vec::new(),
            typed_enrichment: TypedEnrichment::default(),
        }
    }
}

impl Default for Semantics {
    fn default() -> Self {
        Semantics {
            ignored_dirs: default_ignored_dirs(),
            entry_points: default_entry_points(),
            test_markers: default_test_markers(),
        }
    }
}

impl Default for Watcher {
    fn default() -> Self {
        Watcher {
            debounce_ms: default_debounce_ms(),
            settle_ms: default_settle_ms(),
            min_sync_interval_ms: default_min_sync_interval_ms(),
            max_staleness_ms: default_max_staleness_ms(),
        }
    }
}

/// Feed one length-prefixed byte field into the admission-fingerprint hasher.
///
/// The 8-byte little-endian length prefix is what makes the stream unambiguous:
/// without it, `["ab", "c"]` and `["a", "bc"]` would hash identically.
fn feed_bytes(h: &mut blake3::Hasher, bytes: &[u8]) {
    h.update(&(bytes.len() as u64).to_le_bytes());
    h.update(bytes);
}

/// Feed a string list as a **sorted**, length-prefixed sequence: a leading item
/// count, then each item length-prefixed. Sorting makes the fingerprint depend
/// on the *set* the list denotes, not its written order ([FR-IX-02] admission
/// unions these, so order carries no meaning).
///
/// [FR-IX-02]: ../../../../docs/specs/requirements/FR-IX-02.md
fn feed_list(h: &mut blake3::Hasher, items: &[String]) {
    let mut sorted: Vec<&str> = items.iter().map(String::as_str).collect();
    sorted.sort_unstable();
    h.update(&(sorted.len() as u64).to_le_bytes());
    for item in sorted {
        feed_bytes(h, item.as_bytes());
    }
}

/// Feed a `u64` field as fixed-width little-endian bytes (no length prefix
/// needed — the width is constant).
fn feed_u64(h: &mut blake3::Hasher, value: u64) {
    h.update(&value.to_le_bytes());
}

/// Feed a boolean field as a single discriminant byte.
fn feed_bool(h: &mut blake3::Hasher, value: bool) {
    h.update(&[u8::from(value)]);
}

impl Config {
    /// A deterministic fingerprint of the **admission-relevant** configuration
    /// (CR-004, [ADR-20], [FR-SY-07]) — the hash the pipeline persists and
    /// compares to detect that the set of files the config admits has changed.
    ///
    /// It folds in exactly the inputs Pass-0 discovery ([FR-IX-02]) consults to
    /// decide admission, and nothing else, so two configs that admit the same
    /// file set hash identically (a reconciled graph stays byte-identical to a
    /// fresh index, [NFR-RA-06]) while *any* admission-narrowing edit perturbs
    /// it: the active language set (its plugin extension claims), the code
    /// include/exclude globs, `[semantics].ignored_dirs`, `max_file_size`, and
    /// the documentation and config-artifact layer toggles and globs. List
    /// fields are sorted before hashing because admission unions them — order is
    /// not semantically significant, so a pure reorder must not spuriously
    /// re-baseline. Non-admission knobs (resolution policy, watcher debounce,
    /// framework hints, entry points, test markers, typed-enrichment) are
    /// deliberately excluded — changing them admits the same files, so they must
    /// not trigger a purge.
    ///
    /// blake3 over a length-prefixed field stream: every field (and every list
    /// item) is written with an explicit byte length, so no concatenation of
    /// distinct configs can collide ([NFR-RA-06] determinism).
    ///
    /// [ADR-20]: ../../../../docs/specs/architecture/decisions/ADR-20.md
    /// [FR-SY-07]: ../../../../docs/specs/requirements/FR-SY-07.md
    /// [FR-IX-02]: ../../../../docs/specs/requirements/FR-IX-02.md
    /// [NFR-RA-06]: ../../../../docs/specs/requirements/NFR-RA-06.md
    pub fn admission_fingerprint(&self) -> String {
        let mut h = blake3::Hasher::new();
        // A scheme tag so the fingerprint can evolve without colliding with a
        // value computed under an earlier field layout.
        feed_bytes(&mut h, b"logos-admission-fingerprint-v1");
        feed_list(&mut h, &self.languages);
        feed_list(&mut h, &self.include);
        feed_list(&mut h, &self.exclude);
        feed_list(&mut h, &self.semantics.ignored_dirs);
        feed_u64(&mut h, self.max_file_size);
        feed_bool(&mut h, self.documentation.enabled);
        feed_list(&mut h, &self.documentation.include);
        feed_list(&mut h, &self.documentation.exclude);
        feed_bool(&mut h, self.config_artifacts.enabled);
        feed_list(&mut h, &self.config_artifacts.include);
        feed_list(&mut h, &self.config_artifacts.exclude);
        h.finalize().to_hex().to_string()
    }

    /// The effective **code-language** admission allowlist ([FR-CF-01],
    /// CR-017/S-081): `None` when `languages` is empty/omitted — every compiled-in
    /// code grammar is admitted (the twelve-out-of-the-box default,
    /// [CR-009]/[FR-PL-01]) — or `Some(set)` of grammar names (as listed by
    /// `logos languages`) restricting code admission to exactly those.
    ///
    /// Only the code-discovery gate consults this; documentation and
    /// config-artifact admission keep their own `enabled` toggles and are never
    /// gated by this list. Narrowing it re-baselines the admission fingerprint
    /// ([`admission_fingerprint`](Self::admission_fingerprint)), so dropping a
    /// grammar purges its now-unadmitted nodes on the next reconcile ([FR-SY-07]).
    ///
    /// [FR-CF-01]: ../../../../docs/specs/requirements/FR-CF-01.md
    /// [CR-009]: ../../../../docs/requests/CR-009-seven-language-plugins.md
    /// [FR-SY-07]: ../../../../docs/specs/requirements/FR-SY-07.md
    pub fn language_allowlist(&self) -> Option<std::collections::HashSet<String>> {
        (!self.languages.is_empty()).then(|| self.languages.iter().cloned().collect())
    }

    /// Validate the configuration: every include/exclude glob must compile and
    /// stay within the project root ([NFR-SE-04]).
    ///
    /// Called by the loader so a bad pattern fails at load time (exit 2), not on
    /// first discovery.
    pub(crate) fn validate(&self) -> Result<(), super::error::ConfigError> {
        super::globs::validate(&self.include)?;
        super::globs::validate(&self.exclude)?;
        // Doc globs are anchored but obey the same containment rule (S-034); a
        // bad `[documentation]` glob fails at load, not on first discovery.
        super::globs::validate_anchored(&self.documentation.include)?;
        super::globs::validate_anchored(&self.documentation.exclude)?;
        // Config-artifact globs use the non-anchored (code-glob) semantics but the
        // same containment rule (S-062); a bad `[config_artifacts]` glob fails at
        // load, not on first discovery.
        super::globs::validate(&self.config_artifacts.include)?;
        super::globs::validate(&self.config_artifacts.exclude)?;
        // [FR-CV-10] / [CR-036]: the `[coverage_ingest]` artifact globs obey the
        // same containment rule (they re-admit an artifact under a `target/`-class
        // ignored dir, so they must still stay within the root, [NFR-SE-04]); a
        // bad glob fails at load (exit 2), not on first watch.
        super::globs::validate(&self.coverage_ingest.artifact_glob)?;
        // The format token, if set, must be a recognized parser name — a typo
        // fails loud at load rather than silently disabling auto-ingest.
        if let Some(format) = &self.coverage_ingest.format {
            if !matches!(format.as_str(), "auto" | "lcov" | "cobertura") {
                return Err(super::error::ConfigError::InvalidValue {
                    key: "coverage_ingest.format".to_string(),
                    message: format!(
                        "{format:?} is not a recognized coverage format (expected auto, lcov, or cobertura)"
                    ),
                });
            }
        }
        // A present-but-blank refresh command is a misconfiguration (it would run
        // `sh -c ''` to no effect) — reject it loud rather than silently no-op.
        if let Some(cmd) = &self.coverage_ingest.refresh_cmd {
            if cmd.trim().is_empty() {
                return Err(super::error::ConfigError::InvalidValue {
                    key: "coverage_ingest.refresh_cmd".to_string(),
                    message: "must not be empty".to_string(),
                });
            }
        }
        // [FR-CF-06] / S-169: the `[chat]` budget-tree + range knobs fail loud at
        // load (exit 2) — an out-of-range value never reaches the orchestrator.
        self.chat.validate()?;
        // [FR-CF-07] / CR-047: a blank `[wiki].model` fails loud at load (exit 2)
        // rather than resolving to a meaningless override. [FR-WK-17]: a `0`
        // `revision_stale_threshold` is likewise rejected at load.
        self.wiki.validate()?;
        Ok(())
    }

    /// Resolve the effective wiki generation policy ([FR-CF-07], [ADR-42]):
    /// [`wiki`](Self::wiki)'s `model` if set, else [`chat`](Self::chat)'s —
    /// plus the `provider`/`base_url`/API key **inherited** from `[chat]`/
    /// `secrets` verbatim (no separate wiki provider, endpoint, or secret).
    pub fn effective_wiki_model(&self, secrets: &Secrets) -> EffectiveWikiModel {
        self.wiki.resolve(&self.chat, secrets)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a `Documentation` with a given enrichment policy and enabled flag.
    fn doc(enabled: bool, typed_enrichment: TypedEnrichment) -> Documentation {
        Documentation {
            enabled,
            typed_enrichment,
            ..Default::default()
        }
    }

    #[test]
    fn enrichment_active_resolves_every_branch() {
        // Auto defers to the detection signal (S-039, FR-DG-07).
        assert!(doc(true, TypedEnrichment::Auto).enrichment_active(true));
        assert!(!doc(true, TypedEnrichment::Auto).enrichment_active(false));
        // Enabled / Disabled override detection in either direction (BR-24).
        assert!(doc(true, TypedEnrichment::Enabled).enrichment_active(false));
        assert!(!doc(true, TypedEnrichment::Disabled).enrichment_active(true));
        // Documentation off ⇒ no doc nodes to promote, so never active — even
        // when the override forces enrichment on.
        assert!(!doc(false, TypedEnrichment::Enabled).enrichment_active(true));
        assert!(!doc(false, TypedEnrichment::Auto).enrichment_active(true));
    }

    #[test]
    fn default_exclusions_are_broadened_per_fr_cf_05() {
        // CR-029 / FR-CF-05: the built-in defaults prune agent/tooling dirs and
        // per-language build outputs via `ignored_dirs`, and the planning/
        // security/notes prose paths via root-anchored `exclude` globs.
        let cfg = Config::default();

        // The originals survive the broadening.
        for original in [
            "target",
            "node_modules",
            "dist",
            "build",
            "vendor",
            ".git",
            ".logos",
        ] {
            assert!(
                cfg.semantics.ignored_dirs.iter().any(|d| d == original),
                "ignored_dirs must retain the original {original}"
            );
        }
        // The CR-029 additions: agent/tooling dirs and per-language build dirs.
        for added in [
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
        ] {
            assert!(
                cfg.semantics.ignored_dirs.iter().any(|d| d == added),
                "ignored_dirs must contain the FR-CF-05 addition {added}"
            );
        }
        // The planning/security/notes paths are root-anchored exclude globs, NOT
        // ignored_dirs names (FR-CF-05 Notes: ignored_dirs matches a bare name
        // anywhere, so a path-anchored prune must be an exclude glob).
        assert_eq!(
            cfg.exclude,
            vec![
                "docs/planning/**".to_string(),
                "docs/security/**".to_string(),
                "notes/**".to_string(),
            ],
            "default exclude prunes the planning/security/notes paths"
        );
        for path_name in ["planning", "security", "notes"] {
            assert!(
                !cfg.semantics.ignored_dirs.iter().any(|d| d == path_name),
                "{path_name} must be a path-anchored exclude, not an ignored_dirs name"
            );
        }
    }

    #[test]
    fn cr054_worktrees_and_playwright_mcp_are_default_ignored_dirs() {
        // CR-054 / S-213: belt-and-suspenders defaults for a project that
        // doesn't gitignore these scratch dirs — the AdmissionAuthority's
        // gitignore/boundary fix is the primary defense.
        let cfg = Config::default();
        for added in [".worktrees", ".playwright-mcp"] {
            assert!(
                cfg.semantics.ignored_dirs.iter().any(|d| d == added),
                "ignored_dirs must contain the CR-054 addition {added}"
            );
        }
    }

    #[test]
    fn config_artifacts_default_on_with_lock_excludes() {
        // The layer is on by default (FR-CG-01), with the BR-30 lock-file
        // excludes present out of the box.
        let ca = ConfigArtifacts::default();
        assert!(ca.enabled, "config-artifact indexing is on by default");
        assert_eq!(ca.include, ["**"]);
        for lock in [
            "**/package-lock.json",
            "**/Cargo.lock",
            "**/yarn.lock",
            "**/pnpm-lock.yaml",
            "**/*.min.json",
        ] {
            assert!(
                ca.exclude.iter().any(|g| g == lock),
                "default excludes must contain {lock} (BR-30)"
            );
        }
        // compile() yields a matcher that excludes a lock file and admits config.
        let globs = ca.compile().unwrap().expect("enabled ⇒ Some");
        assert!(globs.admits("k8s/values.yaml"));
        assert!(!globs.admits("package-lock.json"));
    }

    #[test]
    fn config_artifacts_disabled_compiles_to_none() {
        // enabled = false ⇒ None, the pipeline's "admit no config artifact" signal
        // (the FR-CF-01 toggle, mirroring documentation).
        let ca = ConfigArtifacts {
            enabled: false,
            ..Default::default()
        };
        assert!(ca.compile().unwrap().is_none(), "disabled ⇒ None");
    }

    #[test]
    fn an_empty_config_deserialises_with_the_config_layer_on() {
        // An absent/empty config.toml gets the default `[config_artifacts]` —
        // the "sensible defaults when fields are omitted" half of FR-CF-01.
        let cfg: Config = toml::from_str("").unwrap();
        assert!(cfg.config_artifacts.enabled);
        assert_eq!(cfg.config_artifacts, ConfigArtifacts::default());
    }

    #[test]
    fn typed_enrichment_defaults_to_auto() {
        assert_eq!(TypedEnrichment::default(), TypedEnrichment::Auto);
        assert_eq!(
            Documentation::default().typed_enrichment,
            TypedEnrichment::Auto
        );
    }

    #[test]
    fn admission_fingerprint_is_deterministic_and_self_consistent() {
        // CR-004 / FR-SY-07: the fingerprint is a pure function of the config —
        // two equal configs hash identically, and a 64-char blake3 hex digest.
        let fp = Config::default().admission_fingerprint();
        assert_eq!(fp, Config::default().admission_fingerprint());
        assert_eq!(fp.len(), 64, "blake3 hex digest is 64 chars");
    }

    #[test]
    fn every_admission_input_perturbs_the_fingerprint() {
        // ADR-20 fitness function: each input Pass-0 admission consults must move
        // the fingerprint, or a change to it would silently leave a stale graph.
        let base = Config::default();
        let base_fp = base.admission_fingerprint();

        // A mutation per admission input; each must differ from the baseline.
        let mut langs = base.clone();
        langs.languages.push("ruby".to_string());

        let mut include = base.clone();
        include.include.push("src/**".to_string());

        let mut exclude = base.clone();
        exclude.exclude.push("generated/**".to_string());

        let mut ignored = base.clone();
        // Must be a name NOT already in `default_ignored_dirs()` — a duplicate
        // would still move the hash (via the item-count prefix) but would stop
        // exercising the "novel admission-narrowing entry perturbs" invariant if
        // list fields were ever deduplicated before hashing.
        ignored
            .semantics
            .ignored_dirs
            .push("custom_build_output".to_string());

        let mut size = base.clone();
        size.max_file_size += 1;

        let mut doc_off = base.clone();
        doc_off.documentation.enabled = false;

        let mut doc_inc = base.clone();
        doc_inc.documentation.include.push("guide/**/*.md".to_string());

        let mut doc_exc = base.clone();
        doc_exc.documentation.exclude.push("docs/private/**".to_string());

        let mut cfg_off = base.clone();
        cfg_off.config_artifacts.enabled = false;

        let mut cfg_inc = base.clone();
        cfg_inc.config_artifacts.include.push("deploy/**".to_string());

        let mut cfg_exc = base.clone();
        cfg_exc.config_artifacts.exclude.push("**/secret.yaml".to_string());

        for (label, cfg) in [
            ("languages", langs),
            ("include", include),
            ("exclude", exclude),
            ("ignored_dirs", ignored),
            ("max_file_size", size),
            ("documentation.enabled", doc_off),
            ("documentation.include", doc_inc),
            ("documentation.exclude", doc_exc),
            ("config_artifacts.enabled", cfg_off),
            ("config_artifacts.include", cfg_inc),
            ("config_artifacts.exclude", cfg_exc),
        ] {
            assert_ne!(
                base_fp,
                cfg.admission_fingerprint(),
                "changing {label} must perturb the admission fingerprint"
            );
        }
    }

    #[test]
    fn reordering_a_list_does_not_change_the_fingerprint() {
        // Admission unions the lists, so a pure reorder admits the same files —
        // the fingerprint must not move, or an untuned repo would re-baseline on
        // a cosmetic edit (no node churn under FR-SY-07's no-op rule).
        let a = Config {
            exclude: vec!["a/**".to_string(), "b/**".to_string()],
            languages: vec!["rust".to_string(), "python".to_string()],
            ..Config::default()
        };
        let mut b = a.clone();
        b.exclude.reverse();
        b.languages.reverse();
        assert_eq!(
            a.admission_fingerprint(),
            b.admission_fingerprint(),
            "a reordered (but set-equal) config must hash identically"
        );
    }

    #[test]
    fn non_admission_knobs_do_not_perturb_the_fingerprint() {
        // Resolution policy, watcher debounce, framework hints, entry points,
        // test markers, and typed-enrichment do not gate admission — changing
        // them admits the same files, so they must not trigger a purge.
        let base = Config::default();
        let base_fp = base.admission_fingerprint();

        let mut policy = base.clone();
        policy.resolution.policy = BindingPolicy::Aggressive;

        let mut watcher = base.clone();
        watcher.watcher.debounce_ms += 100;

        let mut hints = base.clone();
        hints.framework_hints.push("axum".to_string());

        let mut entry = base.clone();
        entry.semantics.entry_points.push("run".to_string());

        let mut markers = base.clone();
        markers.semantics.test_markers.push("it".to_string());

        let mut enrich = base.clone();
        enrich.documentation.typed_enrichment = TypedEnrichment::Disabled;

        // CR-036/FR-CV-10: `[coverage_ingest]` tunes *when* coverage is ingested,
        // never which source files are admitted, so it must not move the
        // admission fingerprint (else a coverage-config edit would purge the
        // graph). It rides the coverage-snapshot config hash instead (FR-CV-09).
        let mut cov_glob = base.clone();
        cov_glob
            .coverage_ingest
            .artifact_glob
            .push("ci/coverage.xml".to_string());
        let mut cov_format = base.clone();
        cov_format.coverage_ingest.format = Some("lcov".to_string());
        let mut cov_cmd = base.clone();
        cov_cmd.coverage_ingest.refresh_cmd = Some("make coverage".to_string());

        for (label, cfg) in [
            ("resolution.policy", policy),
            ("watcher.debounce_ms", watcher),
            ("framework_hints", hints),
            ("entry_points", entry),
            ("test_markers", markers),
            ("typed_enrichment", enrich),
            ("coverage_ingest.artifact_glob", cov_glob),
            ("coverage_ingest.format", cov_format),
            ("coverage_ingest.refresh_cmd", cov_cmd),
        ] {
            assert_eq!(
                base_fp,
                cfg.admission_fingerprint(),
                "changing the non-admission knob {label} must not perturb the fingerprint"
            );
        }
    }

    /// FR-CV-10 / CR-036: an absent/empty `[coverage_ingest]` table is
    /// all-defaults — the built-in conventions, `auto` format, no refresh command.
    #[test]
    fn coverage_ingest_defaults_to_auto_with_no_globs_or_command() {
        let cfg: Config = toml::from_str("").unwrap();
        assert_eq!(cfg.coverage_ingest, CoverageIngest::default());
        let eff = cfg.coverage_ingest.effective();
        assert!(eff.artifact_glob.is_empty());
        assert_eq!(eff.format, "auto");
        assert!(eff.refresh_cmd.is_none());
        // `auto` resolves to no `--format` override (content auto-detection).
        assert_eq!(eff.format_override(), None);
    }

    /// FR-CV-09 acceptance reapplied to `[coverage_ingest]`: changing ANY key
    /// changes the exposed hash, identical configs hash identically, and the
    /// format override resolves only for an explicit non-`auto` token.
    #[test]
    fn coverage_ingest_hash_changes_when_any_key_changes() {
        let base = CoverageIngest::default().effective();
        let glob = CoverageIngest {
            artifact_glob: vec!["ci/cov.xml".to_string()],
            ..Default::default()
        }
        .effective();
        let format = CoverageIngest {
            format: Some("cobertura".to_string()),
            ..Default::default()
        }
        .effective();
        let cmd = CoverageIngest {
            refresh_cmd: Some("make cov".to_string()),
            ..Default::default()
        }
        .effective();

        assert_eq!(base.hash(), base.hash(), "hash is deterministic");
        assert_ne!(base.hash(), glob.hash(), "artifact_glob change moves the hash");
        assert_ne!(base.hash(), format.hash(), "format change moves the hash");
        assert_ne!(base.hash(), cmd.hash(), "refresh_cmd change moves the hash");
        assert_eq!(format.format_override(), Some("cobertura"));
    }

    /// FR-CV-10: a misconfigured `[coverage_ingest]` fails loud at load (exit 2)
    /// — an unknown format token, a blank refresh command, or an escaping glob.
    #[test]
    fn validate_rejects_bad_coverage_ingest_config() {
        let bad_format = Config {
            coverage_ingest: CoverageIngest {
                format: Some("clover".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(
            matches!(bad_format.validate(), Err(crate::config::ConfigError::InvalidValue { ref key, .. }) if key == "coverage_ingest.format"),
            "an unknown format token is rejected"
        );

        let blank_cmd = Config {
            coverage_ingest: CoverageIngest {
                refresh_cmd: Some("   ".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(
            matches!(blank_cmd.validate(), Err(crate::config::ConfigError::InvalidValue { ref key, .. }) if key == "coverage_ingest.refresh_cmd"),
            "a blank refresh command is rejected"
        );

        let escaping_glob = Config {
            coverage_ingest: CoverageIngest {
                artifact_glob: vec!["../outside/cov.xml".to_string()],
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(
            matches!(
                escaping_glob.validate(),
                Err(crate::config::ConfigError::EscapingPattern { .. })
            ),
            "an artifact glob escaping the root is rejected"
        );

        // A valid table (auto format, a contained glob, a non-empty command).
        let ok = Config {
            coverage_ingest: CoverageIngest {
                artifact_glob: vec!["build/cov/cobertura.xml".to_string()],
                format: Some("cobertura".to_string()),
                refresh_cmd: Some("cargo llvm-cov --cobertura".to_string()),
            },
            ..Default::default()
        };
        assert!(ok.validate().is_ok(), "a valid [coverage_ingest] validates");
    }
}
