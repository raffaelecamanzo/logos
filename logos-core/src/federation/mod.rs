//! Workspace **federation** — the in-memory overlay that turns a parent folder
//! of sibling repositories into one queryable workspace ([federation component],
//! [FR-WS-01], [ADR-52]).
//!
//! Federation is an **overlay, never a union** ([ADR-52]): each member keeps its
//! own `.logos/logos.db` and its single-root behaviour byte-for-byte unchanged.
//! This module is the foundation the rest of the overlay is built on. Here we
//! own:
//!
//! - the manifest schema + parse/write ([`manifest`]);
//! - [`discover`] — the up-tree walk that locates the manifest, resolves and
//!   validates each member, and returns the [`Federation`] member set;
//! - the [`registry`] — the `root → Engine` registry over the member set, with
//!   the repo-qualified fan-out helper and the `Backing::Single | Federated`
//!   serve-layer choice ([FR-WS-03], [NFR-PE-10]);
//! - [`discover_candidates`] — the child-directory scan `logos init
//!   --workspace` proposes for approval before any manifest exists
//!   ([`enable`], [FR-WS-02]);
//! - [`enable`] — the `logos init --workspace` orchestration: per-member
//!   non-clobber `init`, the incremental manifest write, and the workspace
//!   MCP injection ([FR-WS-02]).
//! - the [`bridge`] — the in-memory cross-service contract bridge: reads each
//!   member's contract surface through its read pool, matches portable keys
//!   across members exactly-one, and emits ephemeral `BridgeEdge` values cached
//!   on member sync-stamps; never persisted, never `ATTACH`-ed ([FR-WS-04]).
//! - the [`build_deps`] — the in-memory **build-dependency relation**:
//!   member-local Maven/Gradle facts joined into `builds-against(A → B, kind,
//!   scope, artifact)` on first query, with its own `build_dependency_pairs`
//!   headline and a declared platform's inbound edges counted apart, plus the
//!   `xservice build-deps` read-model and its cross-context model hint. Never a
//!   runtime coupling: nothing here enters the bridge or a runtime figure
//!   ([FR-WS-33], [BR-58], [ADR-69]).
//! - the [`coverage`] — the 3-state (bound/ambiguous/unbound) cross-service
//!   coverage read-model with a per-reference reason, a non-gated advisory
//!   tier over the same contract surfaces the bridge reads ([FR-WS-05],
//!   [ADR-53]).
//! - the [`declared_contracts`] — the in-memory **declared-contract relation**:
//!   a spec document a member holds but does not implement declares
//!   `declares-contract(A → C)`, provenance `vendored-spec`, to the member
//!   whose own spec it is by document identity or to a named external grouped
//!   across copies. Derived by the coverage tier from the walk it already
//!   makes and published beside its figures with its own
//!   `declared_contract_pairs` headline; never a binding candidate
//!   ([FR-WS-31], [BR-57], [ADR-68]).
//! - the [`external_join`] — the **external join**: a `no-provider-in-workspace`
//!   REST call binds `bound-external` to the named external its own member
//!   declares, on an exact path under a committed base path — a deploy
//!   overlay's base-url path admitted for this join only. Reported beside
//!   `egress_resolution` with its own `bound_external` headline; no
//!   `BridgeEdge`, no moved row ([FR-WS-05], [FR-WS-19], [ADR-68] point 3).
//! - the [`reach`] — the app-wide cross-service reachability **union view**: a
//!   separate, explicitly-labeled union of every member's `Calls`/`RoutesTo`
//!   adjacency plus the bridge's edges as extra live roots, additive and
//!   monotone toward live, advisory, with a coverage rider on every claim
//!   ([FR-WS-12], [ADR-56]).
//! - the [`governance`] — the workspace-level rule family quantifying over
//!   [`bridge`] matches (service-layer boundaries, no-cross-service-callers),
//!   reported at the workspace level and **structurally incapable** of moving a
//!   member's per-repo gate ([FR-WS-13], [ADR-56]).
//! - the [`telemetry`] — the workspace **usage aggregate**: every member's
//!   `telemetry.db` summed over one window through the engine-free read, stating
//!   the member denominator it summed over and naming every member it could not
//!   read with its reason. Constructs no member engine, in the same spirit as
//!   the manifest-only roster ([FR-UI-37], [NFR-PE-10], [NFR-CC-04]).
//! - the [`type_refs`] — the in-memory **cross-member type-reference overlay**:
//!   an unresolved Java/Kotlin import of a type exactly one other member
//!   declares (in main-tree source or an Avro schema) binds to it, admitted
//!   only between members the build relation relates or a build collision
//!   backs; everything else is counted `type-only`, `ambiguous-owner`,
//!   `self-owned` or `no owner`, with its own `type_reference_pairs` headline.
//!   An advisory `TypeReference`, never a `BridgeEdge`, never a coupling and
//!   never a gate input ([FR-WS-35], [BR-60], [ADR-70]).
//! - the [`warm`] — the bounded background index warm: the effective-bound
//!   resolution seam (K = `max(1, cores / 4)` capped at 4, override-ready) and
//!   the bounded queue one detached supervisor runs over the newly approved
//!   member delta, replacing the unbounded one-child-per-member fan-out
//!   ([FR-WS-14], [BR-44]).
//! - the [`warm_state`] — the user-facing `warm`/`warming`/`deferred`/`degraded`
//!   vocabulary `workspace status` labels each member with, plus the roll-up.
//!   Pure derivation over what a status walk already gathered, with `warming`
//!   and warm-failure taken as explicit inputs so neither is ever inferred
//!   ([FR-WS-15], [NFR-CC-04], [BR-44]).
//! - the [`open_state`] axis — whether each member's store was
//!   **opened**, which is a different question from whether it holds an index.
//!   A member attempted-and-failed is named, given a host-resource cause where
//!   the evidence identifies one, and forces a non-zero exit; a member merely
//!   skipped by laziness or reclaimed by the [`budget`]'s eviction is never
//!   reported degraded ([FR-WS-16], [NFR-CC-04], [BR-45]).
//!
//! [FR-WS-33]: ../../../docs/specs/requirements/FR-WS-33.md
//! [FR-WS-35]: ../../../docs/specs/requirements/FR-WS-35.md
//! [BR-58]: ../../../docs/specs/software-spec.md#327-workspace-federation
//! [BR-60]: ../../../docs/specs/software-spec.md#327-workspace-federation
//! [ADR-70]: ../../../docs/specs/architecture/decisions/ADR-70.md
//! [ADR-69]: ../../../docs/specs/architecture/decisions/ADR-69.md
//! [FR-WS-31]: ../../../docs/specs/requirements/FR-WS-31.md
//! [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
//! [FR-WS-19]: ../../../docs/specs/requirements/FR-WS-19.md
//! [BR-57]: ../../../docs/specs/software-spec.md#327-workspace-federation
//! [ADR-68]: ../../../docs/specs/architecture/decisions/ADR-68.md
//! [FR-UI-37]: ../../../docs/specs/requirements/FR-UI-37.md
//! [FR-WS-15]: ../../../docs/specs/requirements/FR-WS-15.md
//! [FR-WS-16]: ../../../docs/specs/requirements/FR-WS-16.md
//! [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
//! [BR-45]: ../../../docs/specs/software-spec.md#327-workspace-federation
//!
//! [FR-WS-02]: ../../../docs/specs/requirements/FR-WS-02.md
//! [FR-WS-12]: ../../../docs/specs/requirements/FR-WS-12.md
//! [FR-WS-13]: ../../../docs/specs/requirements/FR-WS-13.md
//! [FR-WS-14]: ../../../docs/specs/requirements/FR-WS-14.md
//! [BR-44]: ../../../docs/specs/software-spec.md#327-workspace-federation
//! [ADR-53]: ../../../docs/specs/architecture/decisions/ADR-53.md
//! [ADR-56]: ../../../docs/specs/architecture/decisions/ADR-56.md
//!
//! # Single-root invariant
//! When **no** manifest is found anywhere up-tree, [`discover`] returns
//! `Ok(None)` and touches nothing — behaviour is identical to today and no file
//! is created ([FR-WS-01]). Federation is purely additive and read-only
//! ([ADR-52] Reversibility).
//!
//! # Naming
//! The internal module is `federation` (not `workspace`) to avoid collision with
//! git-worktree root resolution ([`crate::workspace`]) and `TargetClass::Workspace`;
//! the user-facing term is "workspace" ([ADR-52] Notes).
//!
//! [federation component]: ../../../docs/specs/architecture/components/federation.md
//! [FR-WS-01]: ../../../docs/specs/requirements/FR-WS-01.md
//! [FR-WS-03]: ../../../docs/specs/requirements/FR-WS-03.md
//! [FR-WS-04]: ../../../docs/specs/requirements/FR-WS-04.md
//! [NFR-PE-10]: ../../../docs/specs/requirements/NFR-PE-10.md
//! [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md

pub mod bridge;
pub mod broker;
pub mod build_deps;
pub mod budget;
pub mod coverage;
pub mod declared_contracts;
pub mod enable;
pub mod external_join;
pub mod governance;
pub mod links;
pub mod manifest;
pub mod open_state;
pub mod query;
pub mod reach;
pub mod registry;
pub mod residue;
pub mod telemetry;
pub mod topics;
pub mod type_refs;
pub mod warm;
pub mod warm_state;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::config::ConfigError;
use crate::workspace::{is_git_root, resolve_root};

pub use bridge::{
    BridgeEdge, BridgeEndpoint, BridgeIntake, ContractBridge, ContractNode, InvocationRef,
    MemberContracts, ReachabilityRead,
};
pub use coverage::{
    cross_service_coverage, ClassificationCounts, CoverageState, CrossServiceCoverage,
    DeclaredApart, DeclaredMemberRows, IntakeSplit, ProviderCandidates, ProviderDisposition,
    ReferenceCoverage, SpecConformanceReading, UnboundReason,
};
pub use declared_contracts::{
    ContractTarget, Counterparty, DeclaredContract, DeclaredContractHeadline,
    DeclaredContractRelation, DocumentAccounting, ExternalCopy, ExternalId, IdentityCollision,
    NamedExternal, ResolvedTie, VENDORED_SPEC,
};
pub use external_join::{
    BaseOrigin, BasePath, BasePathEvidence, BaseSource, BoundExternal, BoundExternalHeadline,
    ExternalBinding, ExternalJoinRow, JoinAccounting, JoinOutcome, JoinRefusal, OperationMatch,
};
pub use governance::{workspace_governance, WorkspaceGovernance, WorkspaceViolation};
pub use links::{
    asserted_edges, resolve_links, InvalidLink, InvalidReason, LinkCandidates, LinkEnd, LinkStatus,
    LinkVerdict,
};
pub use manifest::{
    ExcludeSet, Governance, Link, LinkRelation, MemberDecl, MemberKind, NoCrossServiceCallers, ServiceBoundary,
    ServiceLayer, EXCLUDE_KEY, MANIFEST_FILENAME,
};
pub use reach::{
    app_wide_reachability, AppWideReachability, AppWideVerdict, BoundedReachability, CoverageRider,
    MemberReachability, ReachNode, ReachabilityClaim, ReachabilityScope, ReachabilitySurface,
    RowsApart, UNION_VIEW,
};
pub use query::{
    workspace_status, xservice_callers, xservice_impact, xservice_route_providers, xservice_search,
    CrossServiceImpact, KindCandidates, MemberResult, MemberStatus, WorkspaceStatus,
    XserviceCallers, XserviceImpact, XserviceRouteProviders, XserviceSearch,
};
pub use build_deps::{
    model_library_context, xservice_build_deps, ArtifactCollision, BuildDependencies,
    BuildDependencyHeadline, BuildDependencyRelation, BuildEdgeKind, BuildsAgainst,
    CrossContextHint, MemberBuildDependencies, MemberBuildFacts, MembersRead, ModelLibrary,
    PairCount, PlatformApart, PlatformCandidate, ReferenceAccounting, XserviceBuildDeps,
    MODEL_LIBRARY_ARTIFACT,
};
pub use budget::WorkspaceBudget;
pub use open_state::{DegradedCause, DegradedRollup, MemberOpen, MemberOpenState, StoreFile};
pub use registry::{
    AnswerScope, Backing, EngineRegistry, MemberAnswer, MemberEngine, MemberReads, MemberScoped,
    MemberSelectionError, RegistryMode, SelectedEngine,
};
// The surfaces serialize `EgressResidue`/`ResidueReason` and pass
// `WorkspaceEgressResidue` through; `AnswerReach` and `egress_residue` are the
// assembly primitives and stay reachable only as `federation::residue::…`, so
// "assembled once" is a property of the export surface and not only of today's
// call sites ([CR-125] §4.4).
pub use residue::{
    EgressResidue, MemberEgressResidue, MemberResolved, ResidueReason, WorkspaceEgressResidue,
};
pub use telemetry::{
    workspace_statistics, UnreadMember, UnreadReason, WorkspaceStatistics,
};
pub use topics::{workspace_topics, MemberTopics, TopicSummary};
pub use type_refs::{
    build_index, read_type_facts, AmbiguousReference, AmbiguousType, CollisionBackedPair, MemberTypeFacts,
    MemberTypeReferences, PairEvidence, TypeImporter, TypeMembersRead, TypeNaming, TypeOnlyPair,
    TypeOrigin, TypeOwner, TypeRefForm, TypeReference, TypeReferenceHeadline, TypeReferenceIndex,
    TypeReferences, TypeRowAccounting, AMBIGUOUS_OWNER,
};
pub use warm_state::{MemberWarmState, WarmEvidence, WarmRollup};

/// One resolved, validated member repository of a [`Federation`] ([FR-WS-01]).
///
/// A member is always a **distinct git root contained within the workspace**;
/// [`discover`] drops anything that is not. The [`name`](Self::name) is the
/// member's path relative to the workspace root — the stable label the registry
/// and cross-service read-models use to *repo-qualify* results ([FR-WS-03]).
///
/// [FR-WS-01]: ../../../docs/specs/requirements/FR-WS-01.md
/// [FR-WS-03]: ../../../docs/specs/requirements/FR-WS-03.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Member {
    /// The member's path relative to the workspace root (e.g. `"api"`,
    /// `"services/web"`) — the stable, human-facing label for the member.
    pub name: String,
    /// The member repository's resolved (symlink-canonical) working-tree root.
    pub root: PathBuf,
}

impl Member {
    /// The repository coordinate this member resolves to ([FR-WS-39], [ADR-85]):
    /// its configured `[project] id`, else the identity derived from its
    /// remote — what the member's symbols carry once its store is indexed, and
    /// what makes them distinct from every other member's.
    ///
    /// Resolved through [`crate::config::resolve_project_id`], the one
    /// definition, so a member and a single-root project of the same checkout
    /// can never disagree. A config the member cannot load (a syntax error, an
    /// unknown key) must not move its identity: the engine opens without reading
    /// the config, so the member still serves a store whose symbols carry the
    /// coordinate the file declared. The `[project] id` is therefore read from
    /// the raw file when the full config does not load, and only when that
    /// holds no valid id either does the member take the derived identity.
    ///
    /// Runs one config read and up to two short `git` subprocesses; callers
    /// resolve it once per member (the registry caches it) and never per symbol.
    ///
    /// [FR-WS-39]: ../../../docs/specs/requirements/FR-WS-39.md
    /// [ADR-85]: ../../../docs/specs/architecture/decisions/ADR-85.md
    #[must_use]
    pub fn coordinate(&self) -> String {
        match crate::config::load_config_from_root(&self.root) {
            Ok(config) => crate::config::resolve_project_id(&self.root, &config),
            Err(_) => declared_project_id(&self.root)
                .unwrap_or_else(|| crate::config::derive_project_id(&self.root)),
        }
    }
}

/// The `[project] id` a member's `.logos/config.toml` declares, read from the
/// raw file when the config as a whole does not load ([`Member::coordinate`]).
///
/// A file that is TOML but fails config validation (an unknown key, a bad glob)
/// is read as a table. A file that is not TOML at all has no table to read, so
/// its `[project]` section is scanned line by line for an `id = "…"` entry: the
/// engine does not read the config either, and the coordinate the file declared
/// is still the one its store's symbols carry.
///
/// `None` when the file is unreadable or declares no valid id
/// ([`crate::config::is_valid_coordinate`]).
fn declared_project_id(root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(root.join(".logos").join("config.toml")).ok()?;
    let declared = match text.parse::<toml::Table>() {
        Ok(table) => table
            .get("project")?
            .as_table()?
            .get("id")?
            .as_str()
            .map(str::to_string),
        Err(_) => scan_project_id(&text),
    };
    declared.filter(|id| crate::config::is_valid_coordinate(id))
}

/// The `id = "…"` line of the `[project]` section of `text`, for a file too
/// broken to parse as TOML ([`declared_project_id`]).
fn scan_project_id(text: &str) -> Option<String> {
    let mut in_project = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_project = line == "[project]";
        } else if in_project {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            if key.trim() == "id" {
                return value.trim().strip_prefix('"')?.split_once('"').map(|(id, _)| id.to_string());
            }
        }
    }
    None
}

/// A discovered application workspace — the resolved member set the overlay
/// federates ([FR-WS-01], [ADR-52]).
///
/// Produced by [`discover`] from a [`manifest::Manifest`]. Every [`Member`] here
/// has already been resolved to a distinct git root and validated for
/// containment; [`default`](Self::default), when present, is guaranteed to name
/// one of [`members`](Self::members).
///
/// [FR-WS-01]: ../../../docs/specs/requirements/FR-WS-01.md
/// [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Federation {
    /// The workspace name (`[workspace] name`).
    pub name: String,
    /// The workspace root — the directory containing the manifest, and the
    /// containment boundary every member must sit within.
    pub root: PathBuf,
    /// The resolved, de-duplicated member set: explicit `members` (in manifest
    /// order) unioned with autodiscovered child repositories (sorted).
    pub members: Vec<Member>,
    /// The default member's [`name`](Member::name), if `[workspace] default`
    /// named a member that survived resolution; otherwise `None`.
    pub default: Option<String>,
    /// User-asserted cross-service links (`[[links]]`), carried through verbatim
    /// for [`links::resolve_links`] to validate into `asserted` bridge edges
    /// ([FR-WS-04], [FR-WS-41]).
    ///
    /// [FR-WS-04]: ../../../docs/specs/requirements/FR-WS-04.md
    /// [FR-WS-41]: ../../../docs/specs/requirements/FR-WS-41.md
    pub links: Vec<Link>,
    /// The `[governance]` workspace rule family ([FR-WS-13]), carried through
    /// verbatim for [`governance::workspace_governance`] to compile and evaluate
    /// over bridge matches. Empty ⇒ no workspace governance output at all.
    ///
    /// [FR-WS-13]: ../../../docs/specs/requirements/FR-WS-13.md
    pub governance: Governance,
    /// The declared `[workspace.warm] concurrency` override, or `None` for the
    /// core-derived default ([FR-WS-01], [FR-WS-14]).
    ///
    /// Carried on the federation rather than re-read from disk so the surface
    /// that spawns the warm supervisor gets K from the same manifest parse that
    /// resolved the member set — one read, one source of truth.
    ///
    /// In `1..=`[`warm::MANIFEST_CONCURRENCY_MAX`] when it came from
    /// [`discover`], which validates via [`manifest::parse`]. The field is
    /// `pub` on a struct built by literal in tests across several crates, so
    /// that is a property of the producer, not of the type — see
    /// [`manifest::Manifest::warm_concurrency`].
    ///
    /// This is the value as **declared**, not the resolved K: `None` means "the
    /// core-derived default applies", which only
    /// [`warm::effective_concurrency`] can turn into a number, and only on the
    /// host that will run the warm. Skipped on serialisation when absent, so an
    /// un-tuned workspace serialises byte-identically to before the key
    /// existed.
    ///
    /// [FR-WS-01]: ../../../docs/specs/requirements/FR-WS-01.md
    /// [FR-WS-14]: ../../../docs/specs/requirements/FR-WS-14.md
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warm_concurrency: Option<usize>,
    /// Each declared member kind (`[workspace.member.<name>] kind`), keyed by
    /// the **resolved** [`Member::name`] ([FR-WS-01], [FR-WS-32]).
    ///
    /// Resolved by [`discover`] under the same normalisation `default` gets, so
    /// every key names a member of [`members`](Self::members) when it came from
    /// there; a declaration naming no resolved member is dropped with a
    /// warning. Empty on a workspace declaring no kind, and skipped on
    /// serialisation then, so such a workspace serialises byte-identically to
    /// before the key existed. Like [`warm_concurrency`](Self::warm_concurrency),
    /// "names a member" is a property of the producer, not of the type.
    ///
    /// [FR-WS-01]: ../../../docs/specs/requirements/FR-WS-01.md
    /// [FR-WS-32]: ../../../docs/specs/requirements/FR-WS-32.md
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub member_kinds: BTreeMap<String, MemberKind>,
    /// What `[workspace] exclude` removed from this workspace, and which
    /// explicit members it did **not** remove ([FR-WS-40]) — read by
    /// `logos init --workspace` (the persisted globs feed its candidate list) and
    /// by `workspace status` (so an exclusion is visible rather than silent).
    ///
    /// Skipped on serialisation when the manifest declares no `exclude`, so a
    /// workspace without the key serialises byte-identically to before it existed.
    ///
    /// [FR-WS-40]: ../../../docs/specs/requirements/FR-WS-40.md
    #[serde(skip_serializing_if = "Exclusion::is_unset")]
    pub exclusion: Exclusion,
}

/// What a manifest's `[workspace] exclude` did to the workspace it declares
/// ([FR-WS-40]) — the read-model `workspace status` publishes and `init
/// --workspace` reads its persisted globs from.
///
/// All three lists are empty exactly when the manifest declares no `exclude`;
/// [`is_unset`](Self::is_unset) is that test and the serde skip predicate.
///
/// [FR-WS-40]: ../../../docs/specs/requirements/FR-WS-40.md
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Exclusion {
    /// The declared globs, as written (`[workspace] exclude`).
    pub globs: Vec<String>,
    /// The directories the globs dropped — child repositories autodiscovery
    /// would otherwise have added, by member name, sorted. Empty when
    /// autodiscovery is off: nothing is autodiscovered, so nothing is dropped
    /// (`init --workspace` filters its own candidates by the same globs). A
    /// directory an explicit `members` entry keeps is not listed here; it is in
    /// [`conflicts`](Self::conflicts).
    pub excluded: Vec<String>,
    /// Each explicit `members` entry that matches a glob and was **kept**
    /// anyway ([FR-WS-40]: an explicit member wins).
    ///
    /// [FR-WS-40]: ../../../docs/specs/requirements/FR-WS-40.md
    pub conflicts: Vec<ExcludeConflict>,
}

impl Exclusion {
    /// Whether the manifest declared no `exclude` — nothing was filtered, nothing
    /// is reported.
    #[must_use]
    pub fn is_unset(&self) -> bool {
        self.globs.is_empty()
    }
}

/// An explicit `members` entry that matches a `[workspace] exclude` glob: kept,
/// and said so ([FR-WS-40], [NFR-CC-04]).
///
/// [FR-WS-40]: ../../../docs/specs/requirements/FR-WS-40.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExcludeConflict {
    /// The `members` entry as written in the manifest.
    pub entry: String,
    /// The glob it matches.
    pub glob: String,
    /// The one-line warning naming both and the fix, as logged on every
    /// workspace load.
    pub warning: String,
}

impl ExcludeConflict {
    fn new(entry: &str, glob: &str) -> Self {
        Self {
            entry: entry.to_string(),
            glob: glob.to_string(),
            warning: format!(
                "[workspace] members entry {entry:?} matches [workspace] exclude glob {glob:?} — \
                 kept, because an explicit member wins; remove the entry from `members` to \
                 exclude it"
            ),
        }
    }
}

/// Discover the workspace by walking **up** from `hint`'s resolved root
/// ([FR-WS-01], [ADR-52]).
///
/// `hint` is resolved to its working-tree root ([`resolve_root`]); the walk then
/// climbs its ancestors for a [`MANIFEST_FILENAME`]. On finding one, each
/// declared member is resolved through [`resolve_root`] and **kept only if** it
/// is a distinct git root contained within the workspace directory; duplicates
/// and path-escapes are dropped. `[workspace.autodiscover]`, when enabled,
/// unions immediate child directories that are git roots (or already carry
/// `.logos/logos.db`) with the explicit members.
///
/// # Returns
/// - `Ok(None)` — **no manifest anywhere up-tree**: single-root mode, byte-for-byte
///   unchanged, nothing created ([FR-WS-01]).
/// - `Ok(Some(federation))` — a manifest was found and parsed; the member set is
///   resolved (possibly empty if every declared member was dropped).
///
/// # Errors
/// Returns a [`ConfigError`] (exit code 2) when a manifest **is** found but is
/// unreadable ([`ConfigError::Io`]), malformed / has an unknown key or a
/// wrong-typed value ([`ConfigError::Parse`]), or gives a key an out-of-range
/// value ([`ConfigError::InvalidValue`]) — a discovered-but-broken manifest
/// fails loud rather than silently degrading to single-root ([ADR-14]).
///
/// Note the reach: this walk is on the path of **every** command, so a manifest
/// rejected here fails all of them, not just the one that would have used the
/// offending key. That is the intended posture, and it is why the messages name
/// the key and what a legal value is.
///
/// [FR-WS-01]: ../../../docs/specs/requirements/FR-WS-01.md
/// [ADR-14]: ../../../docs/specs/architecture/decisions/ADR-14.md
/// [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
pub fn discover(hint: &Path) -> Result<Option<Federation>, ConfigError> {
    let start = resolve_root(hint);
    let Some(manifest_path) = find_manifest_uptree(&start) else {
        // No manifest up-tree → single-root, unchanged, nothing created.
        return Ok(None);
    };

    let manifest = manifest::parse(&manifest_path)?;

    // Surface the adopted manifest so an *unexpected* federation is visible
    // rather than silent: the walk deliberately climbs above the resolved git
    // root (the manifest lives at the parent folder, outside any member repo),
    // so a stray `logos.workspace.toml` in an ancestor would otherwise flip
    // single-root to federated with no trace ([FR-WS-01]).
    tracing::info!(
        manifest = %manifest_path.display(),
        "workspace manifest adopted — operating in federated mode"
    );

    // The workspace root is the manifest's directory — the containment boundary
    // for every member. Canonicalise so escape checks compare like-for-like.
    let root_raw = manifest_path
        .parent()
        .expect("a located manifest file has a parent directory")
        .to_path_buf();
    let root = root_raw.canonicalize().unwrap_or(root_raw);

    let (members, exclusion) = resolve_members(&root, &manifest.workspace)?;
    for conflict in &exclusion.conflicts {
        tracing::warn!(
            entry = %conflict.entry,
            glob = %conflict.glob,
            "{}",
            conflict.warning
        );
    }

    // Keep `default` only when it names a member that survived resolution, so
    // downstream can trust it points at a real member. Match by the SAME
    // normalisation used to derive member names (resolve → canonicalise →
    // relativise), so a `default` written non-canonically (`"api/"`, `"./api"`)
    // still binds its member, and the stored value is the canonical member name.
    let default = manifest
        .workspace
        .default
        .as_deref()
        .and_then(|spec| member_name(&root, &root.join(spec)))
        .filter(|name| members.iter().any(|m| &m.name == name));

    // Read before the literal moves `manifest` apart field by field, the same
    // reason `members` and `default` above are locals rather than inline.
    let warm_concurrency = manifest.warm_concurrency();
    let member_kinds = resolve_member_kinds(&root, &manifest.workspace, &members);

    Ok(Some(Federation {
        name: manifest.workspace.name,
        root,
        members,
        default,
        links: manifest.links,
        governance: manifest.governance,
        warm_concurrency,
        member_kinds,
        exclusion,
    }))
}

/// Resolve each `[workspace.member.<name>] kind` declaration onto the resolved
/// member set ([FR-WS-32]).
///
/// The key is matched by the SAME normalisation `default` is (resolve →
/// canonicalise → relativise, [`member_name`]), so `"./docs"` and `"docs/"`
/// declare the member `docs`. A declaration whose key names no surviving
/// member — a typo, a member since removed — cannot apply to anything and is
/// dropped, but loudly: a silent drop would leave a documentation repo in the
/// headline with nobody told why.
///
/// [FR-WS-32]: ../../../docs/specs/requirements/FR-WS-32.md
fn resolve_member_kinds(
    root: &Path,
    workspace: &manifest::WorkspaceSection,
    members: &[Member],
) -> BTreeMap<String, MemberKind> {
    let mut kinds = BTreeMap::new();
    for (spec, decl) in &workspace.member {
        let Some(kind) = decl.kind else {
            continue;
        };
        match member_name(root, &root.join(spec)).filter(|name| members.iter().any(|m| &m.name == name)) {
            Some(name) => {
                kinds.insert(name, kind);
            }
            None => tracing::warn!(
                member = %spec,
                kind = kind.as_str(),
                "[workspace.member] declares a kind for a name that is not a resolved member — ignored"
            ),
        }
    }
    kinds
}

/// Walk `start` and its ancestors for a [`MANIFEST_FILENAME`]; the first hit
/// (nearest ancestor) wins, or `None` at the filesystem root.
fn find_manifest_uptree(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .map(|dir| dir.join(MANIFEST_FILENAME))
        .find(|candidate| candidate.is_file())
}

/// Resolve and validate the member set: explicit members (manifest order) then
/// autodiscovered children (sorted), de-duplicated by canonical root — and
/// apply `[workspace] exclude` ([FR-WS-40]).
///
/// The exclusion filters the **autodiscovered** children only. An explicit
/// `members` entry that matches a glob is kept and recorded as an
/// [`ExcludeConflict`]; an exclusion that overrode an explicit entry would be a
/// silent contradiction. The returned [`Exclusion`] also lists the directories
/// the globs dropped, so `workspace status` can show them.
///
/// The child scan runs once, and only when autodiscovery is on — exactly as
/// before the key existed. A manifest that declares `exclude` without
/// autodiscovery pays nothing for it: no child is autodiscovered, so none is
/// excluded, and `init --workspace` applies the globs to its own candidates.
///
/// # Errors
/// A malformed `exclude` glob ([`ConfigError::InvalidValue`]). [`discover`]
/// has already parsed the manifest, which validates it, so this is the second
/// line of defence for a hand-built [`manifest::WorkspaceSection`].
///
/// [FR-WS-40]: ../../../docs/specs/requirements/FR-WS-40.md
fn resolve_members(
    root: &Path,
    workspace: &manifest::WorkspaceSection,
) -> Result<(Vec<Member>, Exclusion), ConfigError> {
    let exclude = ExcludeSet::compile(&workspace.exclude)?;
    let mut members: Vec<Member> = Vec::new();
    let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    let mut conflicts: Vec<ExcludeConflict> = Vec::new();

    // A fn, not a closure: the autodiscovery pass below reads `seen` between
    // pushes, which a closure holding it mutably would forbid.
    fn push(
        seen: &mut std::collections::HashSet<PathBuf>,
        members: &mut Vec<Member>,
        member: Member,
    ) {
        if seen.insert(member.root.clone()) {
            members.push(member);
        }
    }

    // Explicit members, in manifest order — resolved to a distinct git root.
    for spec in &workspace.members {
        if let Some(member) = resolve_explicit_member(root, spec) {
            if let Some(glob) = exclude.first_match(&member.name) {
                conflicts.push(ExcludeConflict::new(spec, glob));
            }
            push(&mut seen, &mut members, member);
        }
    }

    // Autodiscovered children (git root OR carries a .logos DB), when enabled.
    let mut excluded: Vec<String> = Vec::new();
    if workspace.autodiscover.as_ref().is_some_and(|a| a.enabled) {
        for member in discover_candidates(root) {
            if exclude.first_match(&member.name).is_some() {
                // A directory an explicit entry already admitted is the
                // conflict above, not an exclusion.
                if !seen.contains(&member.root) {
                    excluded.push(member.name);
                }
                continue;
            }
            push(&mut seen, &mut members, member);
        }
    }
    excluded.sort();

    let exclusion = Exclusion {
        globs: workspace.exclude.clone(),
        excluded,
        conflicts,
    };
    Ok((members, exclusion))
}

/// Resolve one explicit `members` entry (a path relative to the workspace root)
/// to a validated [`Member`], or `None` if it is not a distinct git root or
/// escapes the workspace.
fn resolve_explicit_member(root: &Path, spec: &str) -> Option<Member> {
    let candidate = root.join(spec);
    let resolved = resolve_root(&candidate);
    // Must be a *distinct git root* — a non-repo directory (or a subdir of one)
    // is dropped ([FR-WS-01]).
    if !is_git_root(&resolved) {
        return None;
    }
    validate_member(root, &resolved)
}

/// Enumerate immediate child directories of `root` that qualify as members: a
/// git root, or a directory already carrying `.logos/logos.db` ([FR-WS-01]).
/// Sorted by name for deterministic ordering ([NFR-RA-06]).
///
/// Two callers: [`resolve_members`]'s `[workspace.autodiscover]` union (an
/// *existing* manifest, already canonicalised by [`discover`]), and `logos
/// init --workspace`'s candidate-approval scan
/// ([`enable::candidates_for_approval`], [FR-WS-02]) — run **before** any
/// manifest exists, so it cannot go through [`discover`], and may be handed a
/// non-canonical `root` (a raw CLI hint). `root` is canonicalised here (not
/// just relied on from the caller) so [`validate_member`]'s `strip_prefix`
/// against it always lines up with each candidate's own canonicalised path.
///
/// [FR-WS-02]: ../../../docs/specs/requirements/FR-WS-02.md
pub fn discover_candidates(root: &Path) -> Vec<Member> {
    let root = &root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };

    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();

    dirs.into_iter()
        .filter_map(|dir| {
            // Compute `is_git_root` once — it spawns a `git` subprocess, so the
            // admission test and the resolution branch must share the result.
            let is_root = is_git_root(&dir);
            if !is_root && !dir.join(".logos").join("logos.db").is_file() {
                return None;
            }
            // A git-root child resolves through the same primitive; a DB-only
            // (non-git) child is its own root.
            let resolved = if is_root { resolve_root(&dir) } else { dir };
            validate_member(root, &resolved)
        })
        .collect()
}

/// Turn a resolved member root into a [`Member`], enforcing containment: the
/// (canonical) root must sit *within* the workspace and not *be* the workspace
/// itself. A member that escapes the parent — via `..` or a symlink — is dropped
/// ([FR-WS-01], [NFR-SE-04]).
fn validate_member(workspace_root: &Path, resolved: &Path) -> Option<Member> {
    let name = member_name(workspace_root, resolved)?;
    // `member_name` already proved `resolved` canonicalises, so this succeeds.
    let root = resolved.canonicalize().ok()?;
    Some(Member { name, root })
}

/// The canonical member name for a path resolved within the workspace: its
/// symlink-canonical form relativised to `workspace_root`, with `/` separators.
/// Returns `None` when the path cannot be canonicalised, escapes the workspace,
/// *is* the workspace root (the parent folder is not one of its own members), or
/// lies inside the workspace root's own `.logos/` — the second config tier its
/// chat policy and credential live in ([FR-WS-30]), which is state *of* the
/// workspace and never a member *in* it, whatever it happens to contain.
///
/// This is the single normalisation both member resolution ([`validate_member`])
/// and `default` matching ([`discover`]) share, so a `default` written
/// non-canonically still binds the member it names ([FR-WS-01], [NFR-SE-04]) —
/// and so the `.logos/` exclusion holds on every admission path at once.
///
/// [FR-WS-30]: ../../../docs/specs/requirements/FR-WS-30.md
fn member_name(workspace_root: &Path, path: &Path) -> Option<String> {
    let canon = path.canonicalize().ok()?;
    let rel = canon.strip_prefix(workspace_root).ok()?;
    if rel.as_os_str().is_empty() || rel.starts_with(WORKSPACE_CONFIG_DIR) {
        return None;
    }
    Some(rel.to_string_lossy().replace('\\', "/"))
}

/// The workspace root's own config directory ([FR-WS-30]) — the one child
/// [`member_name`] refuses to name.
///
/// [FR-WS-30]: ../../../docs/specs/requirements/FR-WS-30.md
const WORKSPACE_CONFIG_DIR: &str = ".logos";

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;
    use std::process::Command;

    use tempfile::TempDir;

    /// Run a git command in `cwd`, panicking on failure — fixtures only.
    ///
    /// Auto-maintenance is off: a `commit` otherwise spawns a detached
    /// `git maintenance run --auto` whose `objects/maintenance.lock` can appear
    /// and vanish while a test is snapshotting the tree.
    fn sh_git(cwd: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(["-c", "user.email=test@logos", "-c", "user.name=logos-test"])
            .args(["-c", "maintenance.auto=false", "-c", "gc.auto=0"])
            .args(args)
            .output()
            .expect("git is on PATH");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Initialise `dir` as a committed git repository.
    fn init_repo(dir: &Path) {
        fs::create_dir_all(dir).unwrap();
        sh_git(dir, &["init", "-q", "-b", "main"]);
        fs::write(dir.join("f.txt"), "x\n").unwrap();
        sh_git(dir, &["add", "."]);
        sh_git(dir, &["commit", "-q", "-m", "init"]);
    }

    /// Write a manifest into `dir`.
    fn write_manifest(dir: &Path, body: &str) {
        fs::write(dir.join(MANIFEST_FILENAME), body).unwrap();
    }

    /// Member names, in the order `discover` returned them.
    fn names(fed: &Federation) -> Vec<String> {
        fed.members.iter().map(|m| m.name.clone()).collect()
    }

    /// With a manifest, explicit members resolve to distinct git roots and
    /// duplicates collapse to one.
    #[test]
    fn discovers_explicit_members_deduplicated() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init_repo(&root.join("web"));
        init_repo(&root.join("api"));
        // "api" listed twice (once trailing-slashed) must de-duplicate.
        write_manifest(
            root,
            "[workspace]\nname = \"shop\"\nmembers = [\"web\", \"api\", \"api/\"]\n",
        );

        let fed = discover(root).unwrap().expect("a manifest yields a workspace");
        assert_eq!(fed.name, "shop");
        assert_eq!(names(&fed), ["web", "api"]);
        assert_eq!(
            fed.root.canonicalize().unwrap(),
            root.canonicalize().unwrap()
        );
    }

    /// A member that escapes the workspace via `..` is dropped even when it is a
    /// real git root elsewhere on disk ([NFR-SE-04]).
    #[test]
    fn drops_a_member_that_escapes_the_parent() {
        let tmp = TempDir::new().unwrap();
        let parent = tmp.path().join("parent");
        fs::create_dir_all(&parent).unwrap();
        init_repo(&parent.join("inside"));
        // A real repo *outside* the workspace, reachable only via `..`.
        init_repo(&tmp.path().join("outside"));
        write_manifest(
            &parent,
            "[workspace]\nname = \"w\"\nmembers = [\"inside\", \"../outside\"]\n",
        );

        let fed = discover(&parent).unwrap().unwrap();
        assert_eq!(names(&fed), ["inside"], "the ../ escape is dropped");
    }

    /// A member directory that is not a git root is dropped.
    #[test]
    fn drops_a_non_git_root_member() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init_repo(&root.join("real"));
        fs::create_dir_all(root.join("plain")).unwrap(); // exists, but not a repo
        write_manifest(
            root,
            "[workspace]\nname = \"w\"\nmembers = [\"real\", \"plain\", \"ghost\"]\n",
        );

        let fed = discover(root).unwrap().unwrap();
        assert_eq!(
            names(&fed),
            ["real"],
            "a non-repo dir and a nonexistent path are both dropped"
        );
    }

    /// `[workspace.autodiscover]` unions child git roots (and `.logos`-carrying
    /// dirs) with the explicit members, without duplicating an overlap.
    #[test]
    fn autodiscover_unions_child_roots_with_explicit_members() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init_repo(&root.join("api")); // explicit AND a child root
        init_repo(&root.join("web")); // autodiscovered only
        // A non-git child that already carries a Logos DB is a member too.
        let dbonly = root.join("legacy");
        fs::create_dir_all(dbonly.join(".logos")).unwrap();
        fs::write(dbonly.join(".logos").join("logos.db"), b"db").unwrap();
        // A plain child dir that is neither is ignored.
        fs::create_dir_all(root.join("docs")).unwrap();

        write_manifest(
            root,
            "[workspace]\nname = \"w\"\nmembers = [\"api\"]\n\n[workspace.autodiscover]\n",
        );

        let fed = discover(root).unwrap().unwrap();
        // Explicit "api" first; then autodiscovered, sorted: legacy, web.
        assert_eq!(names(&fed), ["api", "legacy", "web"]);
    }

    /// The workspace root's own `.logos/` — where its chat policy and credential
    /// live ([FR-WS-30]) — is never admitted as a member, however it is reached:
    /// autodiscovered, named explicitly, named as `.`, or chosen as `default`
    /// ([FR-WS-01]). Asserted at a root that is a git working tree and at one that
    /// is not, and with the worst-case contents: the tier's two files, a stray
    /// `logos.db`, and a nested `.logos/logos.db` that would otherwise satisfy the
    /// DB-only admission rule for a child directory. The near miss `.logos-api`
    /// is an ordinary member: the exclusion is the directory, not a prefix.
    #[test]
    fn a_logos_dir_at_the_workspace_root_is_never_a_member() {
        for tracked in [false, true] {
            let tmp = TempDir::new().unwrap();
            let root = tmp.path();
            if tracked {
                init_repo(root);
            }
            init_repo(&root.join("api"));
            // The near miss: a member whose name merely STARTS with `.logos`.
            init_repo(&root.join(".logos-api"));
            let tier = root.join(".logos");
            fs::create_dir_all(tier.join(".logos")).unwrap();
            fs::write(tier.join("config.toml"), "[chat]\nmodel = \"ws/m\"\n").unwrap();
            fs::write(tier.join("secrets.toml"), "[chat]\napi_key = \"sk-x\"\n").unwrap();
            fs::write(tier.join("logos.db"), b"db").unwrap();
            fs::write(tier.join(".logos").join("logos.db"), b"db").unwrap();
            // A repository INSIDE the tier directory: the exclusion is anything
            // under `.logos/`, not only the directory itself.
            init_repo(&tier.join("inner"));
            write_manifest(
                root,
                "[workspace]\nname = \"w\"\nmembers = [\"api\", \".logos-api\", \".\", \".logos\", \".logos/\", \".logos/inner\"]\n\
                 default = \".logos\"\n\n[workspace.autodiscover]\n",
            );

            let fed = discover(root).unwrap().unwrap();
            assert_eq!(names(&fed), ["api", ".logos-api"], "tracked root: {tracked}");
            assert_eq!(fed.default, None, "`.logos` is no member, so it cannot be the default");
            let candidates: Vec<String> =
                discover_candidates(root).into_iter().map(|m| m.name).collect();
            assert_eq!(
                candidates,
                [".logos-api", "api"],
                "nor an enablement candidate (tracked root: {tracked})"
            );
        }
    }

    /// `[workspace] exclude` drops a matching autodiscovered child from the
    /// member set (so from fan-out, bridge and coverage, which all read it) and
    /// lists it as excluded; near-miss names stay members ([FR-WS-40]).
    #[test]
    fn exclude_drops_an_autodiscovered_fork_and_lists_it() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init_repo(&root.join("svc"));
        init_repo(&root.join("svc-fork"));
        init_repo(&root.join("svc-forked")); // near miss: `*-fork` is a suffix glob
        init_repo(&root.join("fork"));
        write_manifest(
            root,
            "[workspace]\nname = \"w\"\nexclude = [\"*-fork\"]\n\n[workspace.autodiscover]\n",
        );

        let fed = discover(root).unwrap().unwrap();
        assert_eq!(names(&fed), ["fork", "svc", "svc-forked"]);
        assert_eq!(fed.exclusion.globs, ["*-fork"]);
        assert_eq!(fed.exclusion.excluded, ["svc-fork"]);
        assert!(fed.exclusion.conflicts.is_empty());
        assert!(!fed.exclusion.is_unset());
    }

    /// Without the key the exclusion is unset and the member set is exactly what
    /// autodiscovery found — the byte-identity contract for existing manifests.
    #[test]
    fn a_manifest_without_exclude_filters_nothing_and_reports_nothing() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init_repo(&root.join("svc"));
        init_repo(&root.join("svc-fork"));
        write_manifest(root, "[workspace]\nname = \"w\"\n\n[workspace.autodiscover]\n");

        let fed = discover(root).unwrap().unwrap();
        assert_eq!(names(&fed), ["svc", "svc-fork"]);
        assert!(fed.exclusion.is_unset());
        assert_eq!(fed.exclusion, Exclusion::default());
        let json = serde_json::to_value(&fed).unwrap();
        assert!(json.get("exclusion").is_none(), "skipped on the wire: {json}");
    }

    /// An explicit `members` entry that matches a glob is kept and recorded as a
    /// conflict naming the entry and the glob; the same directory is not also
    /// listed as excluded ([FR-WS-40]).
    #[test]
    fn an_explicit_member_matching_a_glob_is_kept_and_warned() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init_repo(&root.join("svc"));
        init_repo(&root.join("svc-fork"));
        init_repo(&root.join("old-fork"));
        write_manifest(
            root,
            "[workspace]\nname = \"w\"\nmembers = [\"./svc-fork\"]\nexclude = [\"*-fork\"]\n\n\
             [workspace.autodiscover]\n",
        );

        let fed = discover(root).unwrap().unwrap();
        assert_eq!(names(&fed), ["svc-fork", "svc"], "explicit kept, autodiscovered fork dropped");
        assert_eq!(fed.exclusion.excluded, ["old-fork"], "the kept one is not 'excluded'");
        assert_eq!(fed.exclusion.conflicts.len(), 1);
        let conflict = &fed.exclusion.conflicts[0];
        assert_eq!(conflict.entry, "./svc-fork", "the entry as written");
        assert_eq!(conflict.glob, "*-fork");
        assert!(
            conflict.warning.contains("\"./svc-fork\"") && conflict.warning.contains("\"*-fork\""),
            "the warning names both: {}",
            conflict.warning
        );
    }

    /// The conflict is matched on the resolved member NAME, not on the entry's
    /// text: `./svc-fork` and `svc-fork/` name the member `svc-fork`, so the
    /// literal glob `svc-fork` catches both, and the entry is reported as
    /// written. (A glob like `*-fork` matches the raw text too, so it cannot tell
    /// the two apart.)
    #[test]
    fn an_explicit_conflict_matches_the_member_name_not_the_entry_text() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init_repo(&root.join("svc-fork"));
        init_repo(&root.join("web"));
        write_manifest(
            root,
            "[workspace]\nname = \"w\"\nmembers = [\"./svc-fork\", \"web/\"]\nexclude = [\"svc-fork\"]\n",
        );

        let fed = discover(root).unwrap().unwrap();
        assert_eq!(names(&fed), ["svc-fork", "web"], "both explicit entries are kept");
        let conflicts: Vec<(&str, &str)> = fed
            .exclusion
            .conflicts
            .iter()
            .map(|c| (c.entry.as_str(), c.glob.as_str()))
            .collect();
        assert_eq!(conflicts, [("./svc-fork", "svc-fork")], "only the matching one, as written");
    }

    /// Autodiscovery off: no child is autodiscovered, so a glob drops nothing and
    /// lists nothing — an unlisted repository beside the members does not appear
    /// out of nowhere, and the scan is not paid for.
    #[test]
    fn exclude_with_autodiscover_off_admits_and_lists_nothing() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init_repo(&root.join("svc"));
        init_repo(&root.join("svc-fork"));
        init_repo(&root.join("other"));
        write_manifest(
            root,
            "[workspace]\nname = \"w\"\nmembers = [\"svc\"]\nexclude = [\"*-fork\"]\n",
        );

        let fed = discover(root).unwrap().unwrap();
        assert_eq!(names(&fed), ["svc"], "only the explicit member");
        assert_eq!(fed.exclusion.globs, ["*-fork"]);
        assert!(fed.exclusion.excluded.is_empty());
    }

    /// A malformed glob fails the workspace load (exit 2, naming the key) rather
    /// than silently discovering the fork.
    #[test]
    fn a_malformed_exclude_glob_fails_the_workspace_load() {
        let tmp = TempDir::new().unwrap();
        init_repo(&tmp.path().join("svc"));
        write_manifest(
            tmp.path(),
            "[workspace]\nname = \"w\"\nexclude = [\"{svc\"]\n\n[workspace.autodiscover]\n",
        );
        let err = discover(tmp.path()).expect_err("fails loud");
        assert_eq!(err.exit_code(), 2);
        assert!(err.to_string().contains("workspace.exclude"), "{err}");
    }

    /// A disabled autodiscover section falls back to explicit members only.
    #[test]
    fn autodiscover_disabled_keeps_only_explicit_members() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init_repo(&root.join("api"));
        init_repo(&root.join("web"));
        write_manifest(
            root,
            "[workspace]\nname = \"w\"\nmembers = [\"api\"]\n\n\
             [workspace.autodiscover]\nenabled = false\n",
        );

        let fed = discover(root).unwrap().unwrap();
        assert_eq!(names(&fed), ["api"]);
    }

    /// `default` is retained only when it names a surviving member; a `default`
    /// pointing at a dropped member is nulled out.
    #[test]
    fn default_is_validated_against_the_resolved_set() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init_repo(&root.join("api"));
        write_manifest(
            root,
            "[workspace]\nname = \"w\"\nmembers = [\"api\"]\ndefault = \"api\"\n",
        );
        let fed = discover(root).unwrap().unwrap();
        assert_eq!(fed.default.as_deref(), Some("api"));

        // A default that names a non-member is dropped.
        write_manifest(
            root,
            "[workspace]\nname = \"w\"\nmembers = [\"api\"]\ndefault = \"nope\"\n",
        );
        let fed = discover(root).unwrap().unwrap();
        assert_eq!(fed.default, None);
    }

    /// The discovery walks *up*: a hint deep inside a member still finds the
    /// manifest at the workspace parent.
    #[test]
    fn discover_walks_up_from_a_nested_hint() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let api = root.join("api");
        init_repo(&api);
        fs::create_dir_all(api.join("src")).unwrap();
        write_manifest(root, "[workspace]\nname = \"w\"\nmembers = [\"api\"]\n");

        let fed = discover(&api.join("src")).unwrap().unwrap();
        assert_eq!(fed.name, "w");
        assert_eq!(names(&fed), ["api"]);
    }

    /// `[[links]]` are carried through verbatim onto the [`Federation`].
    #[test]
    fn links_are_carried_through() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init_repo(&root.join("api"));
        write_manifest(
            root,
            "[workspace]\nname = \"w\"\nmembers = [\"api\"]\n\n\
             [[links]]\nrelation = \"http_call\"\nfrom = \"web::c\"\nto = \"api::h\"\n",
        );
        let fed = discover(root).unwrap().unwrap();
        assert_eq!(fed.links.len(), 1);
        assert_eq!(fed.links[0].relation, LinkRelation::HttpCall);
    }

    /// A manifest whose every declared member is dropped still yields a
    /// workspace (`Some` with an empty member set) — NOT single-root `None`.
    /// This distinction is load-bearing: an empty-but-present workspace must not
    /// be silently downgraded to the no-manifest path.
    #[test]
    fn an_all_dropped_member_set_stays_some_not_single_root() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join("plain")).unwrap(); // not a git root
        write_manifest(
            root,
            "[workspace]\nname = \"empty\"\nmembers = [\"plain\"]\ndefault = \"plain\"\n",
        );

        let fed = discover(root)
            .unwrap()
            .expect("a present manifest yields Some, even with no valid members");
        assert_eq!(fed.name, "empty");
        assert!(fed.members.is_empty(), "the non-git member was dropped");
        assert_eq!(fed.default, None, "a default naming a dropped member is nulled");
    }

    /// A multi-segment member (a git root at `services/web`) resolves to a
    /// nested, `/`-separated name — exercising the relative-path derivation.
    #[test]
    fn discovers_a_nested_member_name() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init_repo(&root.join("services").join("web"));
        write_manifest(
            root,
            "[workspace]\nname = \"w\"\nmembers = [\"services/web\"]\n",
        );
        let fed = discover(root).unwrap().unwrap();
        assert_eq!(names(&fed), ["services/web"]);
    }

    /// `default` binds its member even when written non-canonically (a trailing
    /// slash, a leading `./`), and the stored value is the canonical member name.
    #[test]
    fn default_matches_a_non_canonical_spec() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init_repo(&root.join("api"));
        for spec in ["api/", "./api"] {
            write_manifest(
                root,
                &format!("[workspace]\nname = \"w\"\nmembers = [\"api\"]\ndefault = \"{spec}\"\n"),
            );
            let fed = discover(root).unwrap().unwrap();
            assert_eq!(
                fed.default.as_deref(),
                Some("api"),
                "default {spec:?} binds member \"api\" as its canonical name"
            );
        }
    }

    /// A declared kind is resolved onto the member set under the SAME
    /// normalisation `default` gets — `"./docs"` declares the member `docs`, an
    /// autodiscovered member is declarable though `members` never lists it — and
    /// a declaration naming no resolved member is dropped rather than applied
    /// to nothing ([FR-WS-32]).
    ///
    /// [FR-WS-32]: ../../../docs/specs/requirements/FR-WS-32.md
    #[test]
    fn member_kinds_resolve_onto_the_member_set_and_drop_a_non_member() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init_repo(&root.join("api"));
        init_repo(&root.join("docs"));
        init_repo(&root.join("found"));
        write_manifest(
            root,
            "[workspace]\nname = \"w\"\nmembers = [\"api\", \"docs\"]\n\n\
             [workspace.autodiscover]\n\n\
             [workspace.member.\"./docs\"]\nkind = \"documentation\"\n\n\
             [workspace.member.found]\nkind = \"mock\"\n\n\
             [workspace.member.gone]\nkind = \"mock\"\n\n\
             [workspace.member.api]\n",
        );
        let fed = discover(root).unwrap().unwrap();

        assert_eq!(
            fed.member_kinds,
            BTreeMap::from([
                ("docs".to_string(), MemberKind::Documentation),
                ("found".to_string(), MemberKind::Mock),
            ]),
            "canonical names only; the bare `api` table declares no kind and `gone` is no member"
        );
    }

    /// A workspace declaring no kind serializes its federation exactly as before
    /// the key existed: the empty map is skipped, not emitted as `{}`.
    #[test]
    fn a_federation_without_member_kinds_serializes_without_the_key() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init_repo(&root.join("api"));
        write_manifest(root, "[workspace]\nname = \"w\"\nmembers = [\"api\"]\n");
        let fed = discover(root).unwrap().unwrap();

        assert!(fed.member_kinds.is_empty());
        let json = serde_json::to_value(&fed).unwrap();
        assert!(json.get("member_kinds").is_none(), "no key on the wire: {json}");
    }

    // ── the single-root invariant (FR-WS-01) ──────────────────────────────

    /// With NO manifest anywhere up-tree, discovery yields `None` and creates
    /// nothing — the byte-for-byte-unchanged single-root case.
    #[test]
    fn no_manifest_is_single_root_and_creates_nothing() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        init_repo(&root.join("solo"));

        let before = dir_snapshot(root);
        let result = discover(&root.join("solo")).unwrap();
        assert!(result.is_none(), "no manifest → single-root (None)");
        let after = dir_snapshot(root);
        assert_eq!(before, after, "discovery must not create or touch any file");
    }

    /// A discovered-but-malformed manifest fails loud (exit 2), never silently
    /// degrading to single-root.
    #[test]
    fn a_malformed_manifest_fails_loud() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write_manifest(root, "[workspace]\nname = \"w\"\nbogus_key = 1\n");
        let err = discover(root).expect_err("an unknown key must fail loud");
        assert_eq!(err.exit_code(), 2);
    }

    /// A recursive sorted listing of every path under `root` — used to assert
    /// discovery is side-effect-free. A `.git` directory is listed but not
    /// descended: its internals belong to git, not to discovery.
    fn dir_snapshot(root: &Path) -> Vec<PathBuf> {
        fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
            let mut entries: Vec<PathBuf> = match fs::read_dir(dir) {
                Ok(rd) => rd.flatten().map(|e| e.path()).collect(),
                Err(_) => return,
            };
            entries.sort();
            for p in entries {
                out.push(p.clone());
                if p.is_dir() && p.file_name().is_none_or(|n| n != ".git") {
                    walk(&p, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(root, &mut out);
        out
    }

    /// The raw-file read of `[project] id` takes only the `[project]` section's
    /// own `id`, and only a valid one — the near misses a looser scan would
    /// accept (another section's `id`, a sub-table, a different key, an invalid
    /// value) all read as none.
    #[test]
    fn a_broken_configs_project_id_is_read_from_its_own_section_only() {
        let read = |text: &str| scan_project_id(text);
        assert_eq!(read("[project]\nid = \"a/b\"\nnot toml\n").as_deref(), Some("a/b"));
        assert_eq!(read("not toml\n[project]\n  id  =  \"a/b\"  \n").as_deref(), Some("a/b"));
        assert_eq!(read("[other]\nid = \"a/b\"\n"), None, "another section's id");
        assert_eq!(read("[project.sub]\nid = \"a/b\"\n"), None, "a sub-table");
        assert_eq!(read("[project]\nidx = \"a/b\"\n"), None, "a different key");
        assert_eq!(read("[project]\nid = a/b\n"), None, "an unquoted value");
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join(".logos")).unwrap();
        fs::write(dir.path().join(".logos/config.toml"), "[project]\nid = \"bad id\"\nnot toml\n").unwrap();
        assert_eq!(declared_project_id(dir.path()), None, "an invalid id is not a declared one");
    }
}
