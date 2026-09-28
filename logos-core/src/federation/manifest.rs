//! The `logos.workspace.toml` manifest — schema and fail-loud parse
//! ([federation component], [FR-WS-01], [ADR-52]).
//!
//! A manifest at a **parent folder** declares an *application workspace*: a set
//! of member git repositories that together form one system. It is the only
//! on-disk artefact federation reads — the overlay itself is never persisted
//! ([ADR-52]). This module owns the schema ([`Manifest`]) and the parse
//! ([`parse`]); the up-tree location + member resolution live in the parent
//! [`super`] module. It also owns the manifest's two writers, which differ in
//! what they may change: [`upsert`] rewrites **only** `members` for
//! `logos init --workspace` ([FR-WS-02]), and [`save_document`] replaces the
//! **whole** document for the workspace Config editor ([FR-UI-38]) — validated
//! first, written verbatim, and refused when the file moved since it was read.
//!
//! # Failure posture
//! Parsing mirrors the checked-in policy files ([config component], [FR-CF-01]):
//! `#[serde(deny_unknown_fields)]` so a typo'd key fails **loud** rather than
//! being silently ignored, and a malformed manifest is a [`ConfigError`] the
//! surfaces map to exit code 2. A *missing* manifest is **not** a fault — it is
//! the single-root case, handled one level up as `Ok(None)`.
//!
//! `deny_unknown_fields` makes registration **load-bearing**: every key an
//! operator may write has to be declared here or the manifest is rejected
//! whole. `[workspace.warm] concurrency` ([`Warm`], [FR-WS-14]) is the current
//! example — unregistered, a manifest carrying it did not merely lose its warm
//! bound, it took the whole workspace back to single-root behind an exit-2
//! config error.
//!
//! Ranges `serde` cannot express are checked by `Manifest::validate` at the end
//! of [`parse`], so an out-of-range value is a load-time
//! [`ConfigError::InvalidValue`] with an actionable message rather than
//! something a downstream default silently clamps.
//!
//! [federation component]: ../../../docs/specs/architecture/components/federation.md
//! [config component]: ../../../docs/specs/architecture/components/config.md
//! [FR-WS-01]: ../../../docs/specs/requirements/FR-WS-01.md
//! [FR-WS-14]: ../../../docs/specs/requirements/FR-WS-14.md
//! [FR-WS-02]: ../../../docs/specs/requirements/FR-WS-02.md
//! [FR-UI-38]: ../../../docs/specs/requirements/FR-UI-38.md
//! [FR-CF-01]: ../../../docs/specs/requirements/FR-CF-01.md
//! [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::config::ConfigError;
use crate::models::pipeline::{InitAction, InitStep};
use crate::observability::{traced, Tool};

/// The manifest filename discovered by the up-tree walk ([`super::discover`]).
///
/// Named to avoid collision with the git-worktree resolution module
/// (`workspace.rs`) and `TargetClass::Workspace`; the user-facing term is
/// "workspace" ([ADR-52] Notes).
///
/// [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
pub const MANIFEST_FILENAME: &str = "logos.workspace.toml";

/// The parsed `logos.workspace.toml` — the declared workspace, before member
/// resolution ([FR-WS-01]).
///
/// This is the *raw* manifest shape; [`super::discover`] turns it into a
/// [`super::Federation`] by resolving and validating each member against the
/// filesystem. Unknown top-level keys are rejected ([`serde(deny_unknown_fields)`]).
///
/// [FR-WS-01]: ../../../docs/specs/requirements/FR-WS-01.md
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// The `[workspace]` section — name, members, default, and the optional
    /// autodiscover toggle.
    pub workspace: WorkspaceSection,
    /// User-asserted cross-service edges (`[[links]]`), carried through verbatim
    /// for the contract bridge to consume ([FR-WS-04]); labelled `asserted`,
    /// never fabricated. Empty when the manifest declares none.
    ///
    /// [FR-WS-04]: ../../../docs/specs/requirements/FR-WS-04.md
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<Link>,

    /// The `[governance]` workspace rule family ([FR-WS-13], [ADR-56]) — the
    /// cross-service policies [`super::governance`] evaluates over bridge
    /// matches. A **separate family** from the per-repo `.logos/rules.toml`
    /// ([FR-GV-01]): it lives in a different file, compiles to a different
    /// violation type, and is reported at the workspace level without ever
    /// touching a member's gated signal.
    ///
    /// Defaults to empty — a manifest declaring no `[governance]` produces **no**
    /// workspace governance output at all ([NFR-CC-04] honest empty).
    ///
    /// [FR-WS-13]: ../../../docs/specs/requirements/FR-WS-13.md
    /// [FR-GV-01]: ../../../docs/specs/requirements/FR-GV-01.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    /// [ADR-56]: ../../../docs/specs/architecture/decisions/ADR-56.md
    /// Skipped on write only when the table is **entirely unset** — NOT when
    /// [`Governance::is_empty`] holds. The two predicates differ deliberately and
    /// must not be conflated: `is_empty` is the *policy* predicate (layers alone
    /// are vocabulary, so they declare no contract to check), while a layers-only
    /// table is still **user-authored content** that [`upsert`] must round-trip.
    /// Wiring `is_empty` here would make `logos init --workspace` silently delete
    /// a manifest that declares layers but no rule yet — a natural intermediate
    /// authoring state.
    #[serde(default, skip_serializing_if = "Governance::is_unset")]
    pub governance: Governance,
}

impl Manifest {
    /// The declared warm-concurrency override, or `None` for the core-derived
    /// default ([FR-WS-01], [FR-WS-14]).
    ///
    /// The `Some` source of
    /// [`warm::effective_concurrency`](super::warm::effective_concurrency).
    /// Reading it through one accessor is what lets a bare `[workspace.warm]`
    /// (table present, key absent) mean exactly what an absent table means,
    /// without every caller having to flatten two `Option`s the same way.
    ///
    /// In `1..=`[`warm::MANIFEST_CONCURRENCY_MAX`](super::warm::MANIFEST_CONCURRENCY_MAX)
    /// **when the `Manifest` came from [`parse`]** — that is the only
    /// constructor that validates. The bound is not enforced by the type:
    /// [`Warm::concurrency`] is a plain `pub Option<usize>`, so a
    /// programmatically built `Manifest` can carry anything, and
    /// [`warm::effective_concurrency`](super::warm::effective_concurrency)
    /// applies no ceiling. Stated precisely because an unconditional
    /// "guaranteed" here is what would stop the next author from re-checking.
    ///
    /// [FR-WS-01]: ../../../docs/specs/requirements/FR-WS-01.md
    /// [FR-WS-14]: ../../../docs/specs/requirements/FR-WS-14.md
    #[must_use]
    pub fn warm_concurrency(&self) -> Option<usize> {
        self.workspace.warm.and_then(|w| w.concurrency)
    }

    /// Validate the ranges `serde` cannot express, at load ([FR-WS-01],
    /// [FR-CF-01], [NFR-UX-02]).
    ///
    /// Serde already rejects a non-integer `concurrency` by type, with the
    /// offending key and line — actionable as-is. What it cannot say is that
    /// `0` would stall the queue forever and that a value one-per-member
    /// recreates the very fan-out [BR-44] bounds, so those are checked here and
    /// reported as [`ConfigError::InvalidValue`] (exit 2). Rejecting **at parse
    /// time** is the point: [`warm::effective_concurrency`](super::warm::effective_concurrency)
    /// floors a zero, so a value left unvalidated here would be silently
    /// clamped rather than corrected by whoever wrote it.
    ///
    /// [FR-WS-01]: ../../../docs/specs/requirements/FR-WS-01.md
    /// [FR-CF-01]: ../../../docs/specs/requirements/FR-CF-01.md
    /// [NFR-UX-02]: ../../../docs/specs/requirements/NFR-UX-02.md
    /// [BR-44]: ../../../docs/specs/software-spec.md#327-workspace-federation
    fn validate(&self) -> Result<(), ConfigError> {
        let Some(k) = self.warm_concurrency() else {
            return Ok(());
        };
        /// The smallest declarable bound. 1, not 0: a zero bound could never
        /// drain the queue, so it can never be honoured literally.
        const LEGAL_MIN: usize = 1;

        if (LEGAL_MIN..=super::warm::MANIFEST_CONCURRENCY_MAX).contains(&k) {
            return Ok(());
        }
        Err(ConfigError::InvalidValue {
            key: "workspace.warm.concurrency".to_string(),
            // Both branches name the whole legal range, not just the bound
            // that was breached. Naming only the floor once let the zero
            // message mention `CONCURRENCY_CAP` (the *default's* cap) and
            // nothing else, which reads as "4 is the highest I may declare"
            // when the ceiling is four times that. The core-derived default is
            // reported as the resolved number for this host rather than as the
            // `cores / 4` formula, which lives in `warm::derive_concurrency`
            // and would make this message lie the moment it retunes.
            message: format!(
                "{k} is outside the valid range {}..={} — {}. Omit the key \
                 entirely for the core-derived default ({} on this host).",
                LEGAL_MIN,
                super::warm::MANIFEST_CONCURRENCY_MAX,
                if k < LEGAL_MIN {
                    "a bound of 0 would stall the background warm forever".to_string()
                } else {
                    format!(
                        "each concurrent member index is itself parallel over the \
                         host's cores, so K costs K × cores worker threads and up \
                         to K × one index's peak memory; a value near the member \
                         count is the unbounded fan-out the bound exists to \
                         prevent (the default is capped at {})",
                        super::warm::CONCURRENCY_CAP
                    )
                },
                super::warm::default_concurrency()
            ),
        })
    }
}

/// The `[governance]` table — the workspace-level rule family ([FR-WS-13]).
///
/// Every field is an optional array-of-tables, so a manifest may declare any
/// subset. [`is_empty`](Self::is_empty) is the **honest-empty predicate**: when
/// it holds, [`super::governance::workspace_governance`] returns `None` and no
/// report is produced — an undeclared policy is never reported as a passing one
/// ([NFR-CC-04]).
///
/// [FR-WS-13]: ../../../docs/specs/requirements/FR-WS-13.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Governance {
    /// Named service layers over the **member set** (`[[governance.service_layers]]`)
    /// — the workspace analogue of `rules.toml`'s path-glob `[[layers]]`, except a
    /// band here is a set of *services*, not a set of files.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub service_layers: Vec<ServiceLayer>,

    /// Forbidden cross-service calls between named layers
    /// (`[[governance.boundaries]]`), e.g. `edge` → `core`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub boundaries: Vec<ServiceBoundary>,

    /// Providers that must have **no** cross-service callers
    /// (`[[governance.no_cross_service_callers]]`) — the "deprecated endpoint"
    /// contract.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub no_cross_service_callers: Vec<NoCrossServiceCallers>,
}

impl Governance {
    /// Whether the workspace declares **no governance rules** — the honest-empty
    /// *policy* predicate ([NFR-CC-04]).
    ///
    /// `service_layers` alone does not count as a rule: layers are *vocabulary*,
    /// not policy. A manifest that names layers but forbids nothing has declared
    /// no contract to check, so it still produces no report.
    ///
    /// **Not** the serialization predicate — see [`is_unset`](Self::is_unset).
    ///
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    pub fn is_empty(&self) -> bool {
        self.boundaries.is_empty() && self.no_cross_service_callers.is_empty()
    }

    /// Whether the `[governance]` table holds **nothing at all** — the
    /// serialization predicate.
    ///
    /// Distinct from [`is_empty`](Self::is_empty) by design: a layers-only table
    /// declares no *policy* (so it produces no report) but is still user-authored
    /// *content*, and [`upsert`] rebuilds the whole manifest from this struct. If
    /// the serializer skipped on `is_empty`, re-running `logos init --workspace`
    /// over a manifest that declared layers but no rule yet would silently erase
    /// those layers from disk.
    pub fn is_unset(&self) -> bool {
        self.service_layers.is_empty() && self.is_empty()
    }
}

/// A `[[governance.service_layers]]` band: a named layer over workspace members
/// ([FR-WS-13]).
///
/// Where a `rules.toml` `[[layers]]` assigns *files* to a band by path glob, a
/// service layer assigns *members* to a band by name. A member named by two
/// bands takes the **first declaration** (the same first-wins tiebreak the
/// per-repo layer matcher uses); a member named by none is **unlayered** and no
/// boundary rule can classify it — so it is never a violation, never fabricated
/// ([NFR-RA-05]).
///
/// [FR-WS-13]: ../../../docs/specs/requirements/FR-WS-13.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceLayer {
    /// The layer name (e.g. `edge`, `core`) — the vocabulary
    /// [`ServiceBoundary`] refers to.
    pub name: String,
    /// The member names ([`Member::name`](super::Member::name)) in this layer.
    pub members: Vec<String>,
}

/// A `[[governance.boundaries]]` entry: a forbidden cross-service call from one
/// service layer to another ([FR-WS-13]).
///
/// Read over **bridge matches**, not stored edges: a [`BridgeEdge`](super::BridgeEdge)
/// whose consumer (`from`) member sits in layer `from` and whose provider (`to`)
/// member sits in layer `to` violates the boundary. An edge the bridge never
/// matched cannot violate anything — the rules quantify over what was actually
/// bound ([NFR-RA-05]).
///
/// [FR-WS-13]: ../../../docs/specs/requirements/FR-WS-13.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceBoundary {
    /// The calling layer that may not reach `to`.
    pub from: String,
    /// The forbidden callee layer.
    pub to: String,
    /// Human-readable rationale surfaced in the violation message (optional).
    #[serde(default)]
    pub reason: Option<String>,
}

/// A `[[governance.no_cross_service_callers]]` entry: a provider that must have
/// **no** cross-service callers ([FR-WS-13]) — the "deprecated endpoint" contract.
///
/// [`symbol`](Self::symbol) is a **glob** matched against the provider endpoint's
/// canonical [`LogosSymbol`](crate::model::LogosSymbol) string (compiled once via
/// the same `globset` matcher every `rules.toml` family uses), so a whole
/// deprecated namespace fences in one rule. [`member`](Self::member), when given,
/// additionally scopes the rule to providers owned by that member.
///
/// Any [`BridgeEdge`](super::BridgeEdge) whose **provider** endpoint matches is a
/// violation — the edge *is* the cross-service caller. The rule reads the bridge;
/// it never synthesises a caller set ([NFR-RA-05]).
///
/// [FR-WS-13]: ../../../docs/specs/requirements/FR-WS-13.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NoCrossServiceCallers {
    /// Glob matched against the provider endpoint's canonical symbol string.
    pub symbol: String,
    /// Scope the rule to providers owned by this member (optional; omit to match
    /// the symbol glob in **any** member).
    #[serde(default)]
    pub member: Option<String>,
    /// Human-readable rationale surfaced in the violation message (optional).
    #[serde(default)]
    pub reason: Option<String>,
}

/// The `[workspace]` table.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceSection {
    /// The workspace name (`[workspace] name`) — required.
    pub name: String,
    /// Member repository paths, **relative to the manifest directory**
    /// (`members = [...]`). Resolved and validated by [`super::discover`];
    /// absent ⇒ rely solely on [`autodiscover`](Self::autodiscover).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<String>,
    /// The optional default member (`[workspace] default`) — a member path as
    /// written in [`members`](Self::members) (or an autodiscovered directory
    /// name). Carried through; validated against the resolved set by
    /// [`super::discover`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    /// The optional `[workspace.autodiscover]` toggle. Present ⇒ immediate child
    /// directories that are git roots (or already carry `.logos/logos.db`) are
    /// unioned with the explicit [`members`](Self::members) ([FR-WS-01]).
    ///
    /// [FR-WS-01]: ../../../docs/specs/requirements/FR-WS-01.md
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autodiscover: Option<Autodiscover>,
    /// The optional `[workspace.warm]` table — the per-workspace override of
    /// the background index warm's concurrency bound ([FR-WS-01], [FR-WS-14]).
    ///
    /// Absent on every manifest written before this key existed, and on every
    /// manifest `logos init --workspace` writes: the table is operator-authored
    /// only, and an absent one means "use the core-derived default", not
    /// "concurrency = 0".
    ///
    /// [FR-WS-01]: ../../../docs/specs/requirements/FR-WS-01.md
    /// [FR-WS-14]: ../../../docs/specs/requirements/FR-WS-14.md
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warm: Option<Warm>,
    /// Per-member declarations (`[workspace.member.<name>]`), keyed by the
    /// member path as written in [`members`](Self::members) (or an
    /// autodiscovered directory name) — today only the member's
    /// [`kind`](MemberDecl::kind) ([FR-WS-01], [FR-WS-32]).
    ///
    /// A table of tables rather than a richer `members` entry, so `members`
    /// stays the plain path list [`upsert`] owns and an autodiscovered member —
    /// listed nowhere — can still be declared. Keys are resolved against the
    /// member set by [`super::discover`], under the same normalisation
    /// `default` gets; a key naming no resolved member is dropped there, never
    /// here, because parsing cannot know the member set.
    ///
    /// Absent on every manifest written before the key existed and on every
    /// manifest `logos init --workspace` creates: a kind is declared by a
    /// human, never inferred ([ADR-68] point 5).
    ///
    /// [FR-WS-01]: ../../../docs/specs/requirements/FR-WS-01.md
    /// [FR-WS-32]: ../../../docs/specs/requirements/FR-WS-32.md
    /// [ADR-68]: ../../../docs/specs/architecture/decisions/ADR-68.md
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub member: BTreeMap<String, MemberDecl>,
}

/// One `[workspace.member.<name>]` table — what the manifest declares about a
/// single member ([FR-WS-01], [FR-WS-32]).
///
/// Every field is optional, so a bare table is valid and inert, like a bare
/// `[workspace.warm]`. `deny_unknown_fields` holds here as everywhere in this
/// module: a misspelt `kinds = …` is rejected at parse time, naming the key.
///
/// [FR-WS-01]: ../../../docs/specs/requirements/FR-WS-01.md
/// [FR-WS-32]: ../../../docs/specs/requirements/FR-WS-32.md
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MemberDecl {
    /// `kind = "documentation" | "mock" | "platform"` — what the member **is**,
    /// as declared by whoever knows ([FR-WS-32], [FR-WS-33]). `None` (the key
    /// absent) is an ordinary member; logos never fills it in.
    ///
    /// [FR-WS-32]: ../../../docs/specs/requirements/FR-WS-32.md
    /// [FR-WS-33]: ../../../docs/specs/requirements/FR-WS-33.md
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<MemberKind>,
}

/// A declared member kind ([FR-WS-32], [FR-WS-33], [ADR-68] point 5, [ADR-69]
/// point 3).
///
/// **Designed to be extended.** A `platform` member does *not* leave the
/// contract-surface headline — only its inbound build edges are counted apart
/// ([`sets_inbound_build_edges_apart`](Self::sets_inbound_build_edges_apart)).
/// So what a kind does is asked of the kind
/// ([`leaves_contract_surface_headline`](Self::leaves_contract_surface_headline))
/// through exhaustive `match`es, never by testing for "any kind declared": a
/// new variant fails to compile until every such question has an answer for
/// it.
///
/// A malformed value fails the parse naming the key and the legal values
/// (`unknown variant `docs`, expected one of `documentation`, `mock`,
/// `platform``), with the offending line quoted — the manifest-wide fail-loud
/// posture.
///
/// [FR-WS-32]: ../../../docs/specs/requirements/FR-WS-32.md
/// [FR-WS-33]: ../../../docs/specs/requirements/FR-WS-33.md
/// [ADR-68]: ../../../docs/specs/architecture/decisions/ADR-68.md
/// [ADR-69]: ../../../docs/specs/architecture/decisions/ADR-69.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum MemberKind {
    /// A documentation repository: it holds API documents that describe
    /// services, and neither consumes nor provides them.
    Documentation,
    /// A stand-in **provider** of an API it mocks — never a consumer of it
    /// ([ADR-68] point 5).
    ///
    /// [ADR-68]: ../../../docs/specs/architecture/decisions/ADR-68.md
    Mock,
    /// A near-universal build hub — a parent POM or a shared library most
    /// members build against ([FR-WS-33], [ADR-69] point 3). Its inbound build
    /// edges are counted apart from `build_dependency_pairs`; everything else
    /// about it, its contract surface included, is an ordinary member's.
    ///
    /// [FR-WS-33]: ../../../docs/specs/requirements/FR-WS-33.md
    /// [ADR-69]: ../../../docs/specs/architecture/decisions/ADR-69.md
    Platform,
}

impl MemberKind {
    /// The manifest spelling (`"documentation"`, `"mock"`, `"platform"`) — the
    /// same token the payloads serialize.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Documentation => "documentation",
            Self::Mock => "mock",
            Self::Platform => "platform",
        }
    }

    /// Whether a member of this kind has its contract-surface rows reported
    /// **apart** from the headline and from `spec_conformance_ratio`
    /// ([FR-WS-32], [BR-51]).
    ///
    /// A documentation repo's spec copies describe services rather than
    /// consume them, and a mock's are the API it stands in for. A platform
    /// member's contract surface is an ordinary member's: it stays
    /// ([ADR-69] point 3).
    ///
    /// [FR-WS-32]: ../../../docs/specs/requirements/FR-WS-32.md
    /// [BR-51]: ../../../docs/specs/software-spec.md#327-workspace-federation
    /// [ADR-69]: ../../../docs/specs/architecture/decisions/ADR-69.md
    #[must_use]
    pub fn leaves_contract_surface_headline(self) -> bool {
        match self {
            Self::Documentation | Self::Mock => true,
            Self::Platform => false,
        }
    }

    /// Whether the build edges **into** a member of this kind are counted
    /// apart from the `build_dependency_pairs` headline ([FR-WS-33], [ADR-69]
    /// point 3).
    ///
    /// Only `platform`: a hub most members build against would otherwise make
    /// the headline a count of "who has the shared parent". A documentation
    /// repo or a mock that someone builds against is an ordinary edge.
    ///
    /// [FR-WS-33]: ../../../docs/specs/requirements/FR-WS-33.md
    /// [ADR-69]: ../../../docs/specs/architecture/decisions/ADR-69.md
    #[must_use]
    pub fn sets_inbound_build_edges_apart(self) -> bool {
        match self {
            Self::Platform => true,
            Self::Documentation | Self::Mock => false,
        }
    }
}

/// The `[workspace.warm]` sub-table ([FR-WS-01], [FR-WS-14], [BR-44]).
///
/// Every field is optional, so a bare `[workspace.warm]` is valid and inert —
/// the same "documented but currently off" posture
/// `[workspace.autodiscover] enabled = false` has, and a natural intermediate
/// authoring state — while a manifest without the table parses byte-for-byte as
/// it did before. Why registering it at all is load-bearing under
/// `deny_unknown_fields` is stated in the module docs.
///
/// [FR-WS-01]: ../../../docs/specs/requirements/FR-WS-01.md
/// [FR-WS-14]: ../../../docs/specs/requirements/FR-WS-14.md
/// [BR-44]: ../../../docs/specs/software-spec.md#327-workspace-federation
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Warm {
    /// `concurrency = N` — how many member indexes the warm supervisor may run
    /// at once, overriding the core-derived `max(1, cores / 4)` default
    /// ([`warm::default_concurrency`](super::warm::default_concurrency)).
    ///
    /// K is not free — it buys `K × cores` worker threads and up to `K ×` one
    /// member index's peak RSS, which is why it is range-checked rather than
    /// accepted verbatim. That cost model, and the reasoning behind the
    /// ceiling, is stated once at
    /// [`warm::MANIFEST_CONCURRENCY_MAX`](super::warm::MANIFEST_CONCURRENCY_MAX);
    /// `docs/howto/commands.md` states it for the operator.
    ///
    /// Whatever resolves from this is a **hard** ceiling: no member count and no
    /// `--yes` can put more indexes in flight ([BR-44]).
    ///
    /// Omit the key (or the whole table) for the default. `None` here — a bare
    /// table — is "no override", never zero.
    ///
    /// [BR-44]: ../../../docs/specs/software-spec.md#327-workspace-federation
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concurrency: Option<usize>,
}

/// The `[workspace.autodiscover]` sub-table ([FR-WS-01]).
///
/// Its mere presence opts a workspace into auto-discovery of child repositories;
/// [`enabled`](Self::enabled) defaults to `true` so a bare `[workspace.autodiscover]`
/// section turns it on, while `enabled = false` keeps the section documented but
/// inert.
///
/// [FR-WS-01]: ../../../docs/specs/requirements/FR-WS-01.md
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Autodiscover {
    /// Whether auto-discovery is active (default `true`).
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// serde default for [`Autodiscover::enabled`].
fn default_true() -> bool {
    true
}

/// A user-asserted cross-service edge (`[[links]]`) — the escape hatch for
/// couplings the static bridge cannot see (dynamic URLs, computed topics),
/// declared explicitly and labelled `asserted`, **never** fabricated
/// ([FR-WS-04], [ADR-52]).
///
/// Endpoints are free-form portable identifiers (a member-qualified symbol or
/// portable key); this foundation module parses and carries them, and the
/// contract bridge ([FR-WS-04]) interprets them. Kept minimal and stable so the
/// bridge story can grow the interpretation without a manifest-schema churn.
///
/// [FR-WS-04]: ../../../docs/specs/requirements/FR-WS-04.md
/// [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Link {
    /// The asserted relation kind (e.g. `"http_call"`, `"grpc_call"`,
    /// `"publishes"`).
    pub relation: String,
    /// The producing/consuming endpoint the edge starts at.
    pub from: String,
    /// The endpoint the edge points to.
    pub to: String,
}

/// Parse a `logos.workspace.toml` at `path` ([FR-WS-01]).
///
/// # Errors
/// - [`ConfigError::Io`] if the file cannot be read (it was located by the
///   up-tree walk, so a read failure is a real fault, not "no manifest").
/// - [`ConfigError::Parse`] if the TOML is syntactically invalid, contains an
///   unknown key (`deny_unknown_fields`), or gives a key a value of the wrong
///   type — surfaced as exit code 2 ([FR-CF-01]).
/// - [`ConfigError::InvalidValue`] if a well-typed value is out of range —
///   today `[workspace.warm] concurrency` outside
///   `1..=`[`warm::MANIFEST_CONCURRENCY_MAX`](super::warm::MANIFEST_CONCURRENCY_MAX)
///   ([`Manifest::validate`], also exit code 2).
///
/// [FR-WS-01]: ../../../docs/specs/requirements/FR-WS-01.md
/// [FR-CF-01]: ../../../docs/specs/requirements/FR-CF-01.md
pub fn parse(path: &Path) -> Result<Manifest, ConfigError> {
    let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    parse_text(&text, path)
}

/// The body of [`parse`] over text already in hand: the `deny_unknown_fields`
/// deserialise, then [`Manifest::validate`]. `path` only labels the error.
///
/// Split out so [`save_document`] validates a candidate through **this** code
/// rather than a second spelling of it: a candidate the editor may write is
/// exactly one [`discover`](super::discover) will accept on the next command.
fn parse_text(text: &str, path: &Path) -> Result<Manifest, ConfigError> {
    let manifest: Manifest = toml::from_str(text).map_err(|source| ConfigError::Parse {
        path: path.to_path_buf(),
        source,
    })?;
    manifest.validate()?;
    Ok(manifest)
}

/// Create or incrementally update the manifest at `root` from the approved
/// member-name set (`logos init --workspace`, [FR-WS-02]): a fresh manifest is
/// created with `name` and `members`; an existing one keeps its `name`,
/// `default`, `autodiscover`, `warm`, per-member `[workspace.member.<name>]`
/// declarations, `links`, and `governance` untouched and
/// only has `members` upserted (sorted, de-duplicated). A result byte-identical to what's already
/// on disk reports [`InitAction::Unchanged`] without writing — the
/// write-if-different extension of [FR-IN-01]'s write-if-absent posture,
/// applied to the one field this command owns.
///
/// # Errors
/// [`ConfigError::Io`]/[`ConfigError::Parse`]/[`ConfigError::InvalidValue`]
/// reading a malformed existing manifest — it is read through [`parse`], so a
/// re-run over an out-of-range manifest fails loud rather than rewriting it;
/// [`ConfigError::Write`] if the write itself fails.
///
/// Because the existing manifest is validated on the way in, everything this
/// function carries forward is already in range: it can never write a manifest
/// its own [`parse`] would reject.
///
/// [FR-WS-02]: ../../../docs/specs/requirements/FR-WS-02.md
/// [FR-IN-01]: ../../../docs/specs/requirements/FR-IN-01.md
pub fn upsert(root: &Path, name: &str, members: &[String]) -> Result<InitStep, ConfigError> {
    let path = root.join(MANIFEST_FILENAME);
    let mut members = members.to_vec();
    members.sort();
    members.dedup();

    let existing = if path.is_file() {
        Some(parse(&path)?)
    } else {
        None
    };

    let manifest = Manifest {
        workspace: WorkspaceSection {
            name: existing
                .as_ref()
                .map_or_else(|| name.to_string(), |m| m.workspace.name.clone()),
            members,
            default: existing.as_ref().and_then(|m| m.workspace.default.clone()),
            autodiscover: existing.as_ref().and_then(|m| m.workspace.autodiscover.clone()),
            // Operator-authored tuning this command does not own, carried
            // across for the same reason `name` and `autodiscover` are: `upsert`
            // rebuilds the whole struct, so a field not named here is silently
            // erased on the next `logos init --workspace` re-run ([FR-WS-01]).
            // The bare table is preserved too, not only a set key — see
            // `Governance::is_unset` for the same distinction stated at length.
            warm: existing.as_ref().and_then(|m| m.workspace.warm),
            // A member's declared `kind` is human-authored classification this
            // command must never erase ([FR-WS-32]); carried whole, including a
            // declaration naming a member the approved set no longer holds —
            // `upsert` owns `members`, not what a human wrote about them.
            member: existing
                .as_ref()
                .map_or_else(BTreeMap::new, |m| m.workspace.member.clone()),
        },
        links: existing.as_ref().map_or_else(Vec::new, |m| m.links.clone()),
        // Like `links`, the workspace rule family is user-authored policy this
        // command does not own: `upsert` rebuilds the whole struct, so a section
        // not carried across here would be silently dropped on the next
        // `logos init --workspace` ([FR-WS-13]).
        governance: existing
            .as_ref()
            .map_or_else(Governance::default, |m| m.governance.clone()),
    };

    // The struct holds no floats — every field is a String, Vec<String>,
    // Option<String>, Option<Autodiscover>, Option<Warm> (an Option<usize>), a
    // string-keyed BTreeMap of MemberDecl tables (an Option of a unit enum), or
    // a Vec of Link/governance tables of the same — so TOML serialisation cannot
    // fail in practice.
    let text = toml::to_string_pretty(&manifest)
        .expect("Manifest holds only TOML-representable scalar/table fields");

    if existing.is_some() {
        let current = std::fs::read_to_string(&path).map_err(|source| ConfigError::Io {
            path: path.clone(),
            source,
        })?;
        if current == text {
            return Ok(InitStep {
                target: MANIFEST_FILENAME.to_string(),
                action: InitAction::Unchanged,
                detail: String::new(),
            });
        }
    }

    std::fs::write(&path, &text).map_err(|source| ConfigError::Write {
        path: path.clone(),
        source,
    })?;

    Ok(InitStep {
        target: MANIFEST_FILENAME.to_string(),
        action: if existing.is_none() {
            InitAction::Created
        } else {
            InitAction::Updated
        },
        detail: String::new(),
    })
}

// ── The whole-manifest write path (S-430, FR-UI-38) ────────────────────────

/// The load-time fingerprint of the manifest's on-disk bytes — the token that
/// makes a save **refuse to clobber** an edit it never saw ([FR-UI-38]).
///
/// A content hash, not an mtime: two writes inside one filesystem timestamp tick
/// (a `logos init --workspace` re-run racing a save) leave the mtime where it was,
/// and a touch without a change moves it. Only the bytes decide whether the file
/// the editor loaded is still the file on disk.
///
/// [FR-UI-38]: ../../../docs/specs/requirements/FR-UI-38.md
#[must_use]
pub fn fingerprint(bytes: &[u8]) -> String {
    crate::config::atomic::fingerprint(bytes)
}

/// The manifest as the editor loads it ([FR-UI-38]): the literal document, the
/// [`fingerprint`] of those exact bytes, and the parse verdict over them.
///
/// `content` is what the raw pane shows and what a save posts back — never a
/// re-serialisation of `parsed`, which would drop the operator's comments and
/// key order on the first save.
///
/// A manifest that no longer parses (a hand edit went wrong while the serve was
/// running) is **not** an error here: it is reported as `parsed = None` with the
/// parser's own message in `error`, because the editor is the repair path and it
/// cannot repair a document it was refused.
///
/// [FR-UI-38]: ../../../docs/specs/requirements/FR-UI-38.md
#[derive(Debug, Clone, Serialize)]
pub struct ManifestDocument {
    /// The manifest's filename relative to the workspace root ([`MANIFEST_FILENAME`]).
    pub path: String,
    /// The literal on-disk document.
    pub content: String,
    /// [`fingerprint`] of `content`'s bytes — posted back with a save.
    pub fingerprint: String,
    /// The parsed manifest, or `None` when `content` does not parse.
    pub parsed: Option<Manifest>,
    /// Why `content` does not parse — `None` exactly when `parsed` is `Some`.
    pub error: Option<String>,
}

/// What a [`save_document`] did. Each arm names the fingerprint the editor must
/// hold from now on, so the next save is compared against the right bytes.
///
/// Internally tagged on `outcome` for the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ManifestSaveOutcome {
    /// The candidate replaced the manifest atomically.
    Written {
        /// The manifest's filename relative to the workspace root.
        path: String,
        /// The size of the document now on disk.
        bytes_written: u64,
        /// [`fingerprint`] of the document now on disk.
        fingerprint: String,
    },
    /// The candidate is byte-identical to the manifest on disk, so **nothing was
    /// written** — `upsert`'s write-if-different posture, carried in.
    Unchanged {
        /// The manifest's filename relative to the workspace root.
        path: String,
        /// [`fingerprint`] of the (untouched) document on disk.
        fingerprint: String,
    },
    /// The manifest changed on disk since the editor loaded it, so the save was
    /// **refused** and nothing was written. Carries what is on disk now, so the
    /// editor can show it and let the user choose which copy wins.
    Conflict {
        /// The manifest's filename relative to the workspace root.
        path: String,
        /// The fingerprint the save was made against (the editor's load).
        loaded_fingerprint: String,
        /// [`fingerprint`] of the document on disk now.
        disk_fingerprint: String,
        /// The document on disk now.
        disk_content: String,
    },
}

/// Read the manifest at `root` for the workspace Config editor ([FR-UI-38]).
///
/// # Errors
/// [`ConfigError::Io`] if the manifest cannot be read. A manifest that reads but
/// does not parse is **not** an error — see [`ManifestDocument`].
///
/// [FR-UI-38]: ../../../docs/specs/requirements/FR-UI-38.md
pub fn read_document(root: &Path) -> Result<ManifestDocument, ConfigError> {
    let path = root.join(MANIFEST_FILENAME);
    let content = std::fs::read_to_string(&path).map_err(|source| ConfigError::Io {
        path: path.clone(),
        source,
    })?;
    let (parsed, error) = match parse_text(&content, &path) {
        Ok(manifest) => (Some(manifest), None),
        Err(err) => (None, Some(err.to_string())),
    };
    Ok(ManifestDocument {
        path: MANIFEST_FILENAME.to_string(),
        fingerprint: fingerprint(content.as_bytes()),
        content,
        parsed,
        error,
    })
}

/// Replace the whole manifest at `root` with `candidate` — the write path the
/// workspace Config editor saves through ([FR-UI-38]), **beside** [`upsert`],
/// which owns only `members`.
///
/// `loaded_fingerprint` is the [`ManifestDocument::fingerprint`] the editor
/// loaded. In order, and each step decides before the next one runs:
///
/// 1. **Validate the candidate** through [`parse`]'s own body — the posture
///    `upsert` holds by reading through [`parse`], carried in rather than
///    re-invented. A candidate it rejects is an error and the file is not
///    touched, so the editor can never write a manifest that would fail every
///    command in the workspace on its next [`discover`](super::discover).
/// 2. **Byte-identical to disk ⇒ [`Unchanged`](ManifestSaveOutcome::Unchanged)**,
///    nothing written — `upsert`'s write-if-different. This is decided before the
///    fingerprint: a candidate equal to what is on disk overwrites nobody's edit,
///    whoever made it.
/// 3. **The disk moved since the load ⇒ [`Conflict`](ManifestSaveOutcome::Conflict)**,
///    nothing written. The manifest is routinely edited by hand and by
///    `logos init --workspace` while a tab is open; a save made against a
///    document the user never saw is refused, not merged.
/// 4. Otherwise the candidate replaces the manifest **verbatim** through the
///    shared atomic publish ([NFR-RA-07]). Verbatim is what keeps
///    [NFR-RA-05]: nothing is re-serialised, so no undeclared table, default or
///    reordering is invented, and the operator's comments survive.
///
/// Steps 3 and 4 are a compare-then-swap without a lock: a write landing between
/// the read and the rename is not detected. The window is the few microseconds
/// between them, and no other manifest writer (`upsert`, a text editor) takes a
/// lock either.
///
/// # Errors
/// The [`ConfigError`] [`parse`] raises for a candidate it rejects;
/// [`ConfigError::Io`] reading the current manifest; [`ConfigError::Write`] if
/// the atomic replace fails (the manifest is then unchanged).
///
/// [FR-UI-38]: ../../../docs/specs/requirements/FR-UI-38.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
/// [NFR-RA-07]: ../../../docs/specs/requirements/NFR-RA-07.md
pub fn save_document(
    root: &Path,
    candidate: &str,
    loaded_fingerprint: &str,
) -> Result<ManifestSaveOutcome, ConfigError> {
    let path = root.join(MANIFEST_FILENAME);
    parse_text(candidate, &path)?;

    let current = std::fs::read(&path).map_err(|source| ConfigError::Io {
        path: path.clone(),
        source,
    })?;
    let disk_fingerprint = fingerprint(&current);
    if current == candidate.as_bytes() {
        return Ok(ManifestSaveOutcome::Unchanged {
            path: MANIFEST_FILENAME.to_string(),
            fingerprint: disk_fingerprint,
        });
    }
    if disk_fingerprint != loaded_fingerprint {
        return Ok(ManifestSaveOutcome::Conflict {
            path: MANIFEST_FILENAME.to_string(),
            loaded_fingerprint: loaded_fingerprint.to_string(),
            disk_fingerprint,
            disk_content: String::from_utf8_lossy(&current).into_owned(),
        });
    }

    crate::config::atomic::publish(&path, candidate.as_bytes(), None).map_err(|source| {
        ConfigError::Write {
            path: path.clone(),
            source,
        }
    })?;
    Ok(ManifestSaveOutcome::Written {
        path: MANIFEST_FILENAME.to_string(),
        bytes_written: candidate.len() as u64,
        fingerprint: fingerprint(candidate.as_bytes()),
    })
}

/// [`read_document`] as the workspace Config editor's read — booked as the one
/// `config_read` telemetry event the S-450 workspace-tier read
/// ([`read_workspace_documents`](crate::config::read_workspace_documents)) emits,
/// through the one emission point (`observability::traced`).
///
/// No engine stands behind this route to emit the façade's event (the manifest
/// is a file at the workspace root, where none may be constructed, [ADR-67]), so
/// the event is emitted here, under whatever surface the caller entered — a
/// manifest read is as visible to `logos stats` as a member's config read.
///
/// # Errors
/// As [`read_document`].
///
/// [ADR-67]: ../../../docs/specs/architecture/decisions/ADR-67.md
pub fn read_workspace_manifest(root: &Path) -> anyhow::Result<ManifestDocument> {
    traced(Tool::ConfigRead, || Ok(read_document(root)?))
}

/// [`save_document`] as the workspace Config editor's save — booked as the one
/// `config_write` telemetry event the S-450 workspace-tier save
/// ([`write_workspace_config`](crate::config::write_workspace_config)) emits,
/// `ok` tracking the result. A conflict or an unchanged candidate is an `Ok`
/// outcome, as it is there.
///
/// # Errors
/// As [`save_document`]; the [`ConfigError`] stays the root of the `anyhow`
/// chain, so a caller can still map validation vs I/O faults.
pub fn save_workspace_manifest(
    root: &Path,
    candidate: &str,
    loaded_fingerprint: &str,
) -> anyhow::Result<ManifestSaveOutcome> {
    traced(Tool::ConfigWrite, || {
        Ok(save_document(root, candidate, loaded_fingerprint)?)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;
    use tempfile::TempDir;

    use crate::federation::warm;

    /// Write `body` to `<tmp>/logos.workspace.toml` and return its path.
    fn write_manifest(tmp: &TempDir, body: &str) -> std::path::PathBuf {
        let path = tmp.path().join(MANIFEST_FILENAME);
        fs::write(&path, body).unwrap();
        path
    }

    /// The config component's [`REMEDY_ARTEFACT`] is **this** filename.
    ///
    /// `ZeroAdmissionDiagnostic::derive` suppresses itself when the root already
    /// carries the artefact `logos init --workspace` leaves, and it has to spell
    /// that name itself: the dependency runs **federation → config** (this module
    /// imports `config::ConfigError`; `enable` imports the diagnostic), so
    /// `config` cannot name a federation constant.
    ///
    /// That leaves exactly one place the two spellings can drift, and this is it.
    /// The assertion belongs on **this** side because federation is the side that
    /// may see both. It is the only check on the literal: every other reference in
    /// either component goes through one of the two constants.
    ///
    /// [`REMEDY_ARTEFACT`]: crate::config::ZeroAdmissionDiagnostic::REMEDY_ARTEFACT
    #[test]
    fn the_remedy_artefact_is_this_manifest_filename() {
        assert_eq!(
            crate::config::ZeroAdmissionDiagnostic::REMEDY_ARTEFACT,
            MANIFEST_FILENAME,
            "the zero-admission diagnostic probes for the file `init --workspace` writes"
        );
    }

    /// A full manifest parses: name, members, default, autodiscover, and links.
    #[test]
    fn parses_a_full_manifest() {
        let tmp = TempDir::new().unwrap();
        let path = write_manifest(
            &tmp,
            r#"
            [workspace]
            name = "shop"
            members = ["web", "api"]
            default = "api"

            [workspace.autodiscover]
            enabled = true

            [[links]]
            relation = "http_call"
            from = "web::fetchCart"
            to = "api::get_cart"
            "#,
        );
        let m = parse(&path).expect("valid manifest parses");
        assert_eq!(m.workspace.name, "shop");
        assert_eq!(m.workspace.members, ["web", "api"]);
        assert_eq!(m.workspace.default.as_deref(), Some("api"));
        assert!(m.workspace.autodiscover.expect("present").enabled);
        assert_eq!(m.links.len(), 1);
        assert_eq!(m.links[0].relation, "http_call");
        assert_eq!(m.links[0].from, "web::fetchCart");
        assert_eq!(m.links[0].to, "api::get_cart");
    }

    /// The minimal manifest is just a name: members/default/autodiscover/links
    /// all default to empty/absent.
    #[test]
    fn parses_a_minimal_manifest() {
        let tmp = TempDir::new().unwrap();
        let path = write_manifest(&tmp, "[workspace]\nname = \"solo\"\n");
        let m = parse(&path).expect("a name-only manifest is valid");
        assert_eq!(m.workspace.name, "solo");
        assert!(m.workspace.members.is_empty());
        assert!(m.workspace.default.is_none());
        assert!(m.workspace.autodiscover.is_none());
        assert!(m.links.is_empty());
    }

    /// A bare `[workspace.autodiscover]` section turns discovery on (enabled
    /// defaults to true).
    #[test]
    fn bare_autodiscover_section_defaults_enabled() {
        let tmp = TempDir::new().unwrap();
        let path = write_manifest(
            &tmp,
            "[workspace]\nname = \"a\"\n\n[workspace.autodiscover]\n",
        );
        let m = parse(&path).unwrap();
        assert!(m.workspace.autodiscover.expect("present").enabled);
    }

    /// `enabled = false` keeps the section but disables discovery.
    #[test]
    fn autodiscover_can_be_disabled() {
        let tmp = TempDir::new().unwrap();
        let path = write_manifest(
            &tmp,
            "[workspace]\nname = \"a\"\n\n[workspace.autodiscover]\nenabled = false\n",
        );
        let m = parse(&path).unwrap();
        assert!(!m.workspace.autodiscover.expect("present").enabled);
    }

    /// An unknown key fails loud (`deny_unknown_fields`) — the FR-CF-01 posture,
    /// surfaced as a parse error (exit 2), never silently ignored.
    #[test]
    fn unknown_key_is_rejected() {
        let tmp = TempDir::new().unwrap();
        let path = write_manifest(
            &tmp,
            "[workspace]\nname = \"a\"\nmembrs = [\"typo\"]\n",
        );
        let err = parse(&path).expect_err("an unknown key must fail loud");
        assert!(matches!(err, ConfigError::Parse { .. }));
        assert_eq!(err.exit_code(), 2);
    }

    /// A missing `name` is a parse error — `[workspace] name` is required.
    #[test]
    fn missing_name_is_rejected() {
        let tmp = TempDir::new().unwrap();
        let path = write_manifest(&tmp, "[workspace]\nmembers = [\"x\"]\n");
        assert!(matches!(parse(&path), Err(ConfigError::Parse { .. })));
    }

    /// Syntactically invalid TOML is a parse error, not a panic.
    #[test]
    fn invalid_toml_is_a_parse_error() {
        let tmp = TempDir::new().unwrap();
        let path = write_manifest(&tmp, "this is not toml = = =");
        assert!(matches!(parse(&path), Err(ConfigError::Parse { .. })));
    }

    /// A path that does not exist is an I/O error carrying the offending path.
    #[test]
    fn a_missing_file_is_an_io_error() {
        let tmp = TempDir::new().unwrap();
        let ghost = tmp.path().join(MANIFEST_FILENAME);
        match parse(&ghost) {
            Err(ConfigError::Io { path, .. }) => assert_eq!(path, ghost),
            other => panic!("expected an Io error, got {other:?}"),
        }
    }

    // ── upsert (FR-WS-02) ─────────────────────────────────────────────────

    /// No manifest yet: `upsert` creates one with the given name and members,
    /// sorted and de-duplicated.
    #[test]
    fn upsert_creates_a_fresh_manifest() {
        let tmp = TempDir::new().unwrap();
        let step = upsert(tmp.path(), "shop", &["web".into(), "api".into(), "api".into()])
            .expect("creates");
        assert_eq!(step.action, InitAction::Created);

        let m = parse(&tmp.path().join(MANIFEST_FILENAME)).unwrap();
        assert_eq!(m.workspace.name, "shop");
        assert_eq!(m.workspace.members, ["api", "web"], "sorted + de-duplicated");
        assert!(m.workspace.default.is_none());
        assert!(m.workspace.autodiscover.is_none());
        assert!(m.links.is_empty());
    }

    /// An existing manifest keeps its name/default/autodiscover/links
    /// untouched; only `members` is upserted.
    #[test]
    fn upsert_preserves_hand_written_sections_on_an_existing_manifest() {
        let tmp = TempDir::new().unwrap();
        write_manifest(
            &tmp,
            "[workspace]\nname = \"shop\"\nmembers = [\"api\"]\ndefault = \"api\"\n\n\
             [workspace.autodiscover]\nenabled = false\n\n\
             [[links]]\nrelation = \"http_call\"\nfrom = \"web::c\"\nto = \"api::h\"\n",
        );

        let step = upsert(tmp.path(), "ignored-name", &["api".into(), "web".into()])
            .expect("updates");
        assert_eq!(step.action, InitAction::Updated);

        let m = parse(&tmp.path().join(MANIFEST_FILENAME)).unwrap();
        assert_eq!(m.workspace.name, "shop", "name is never overwritten by a re-run");
        assert_eq!(m.workspace.members, ["api", "web"]);
        assert_eq!(m.workspace.default.as_deref(), Some("api"));
        assert!(!m.workspace.autodiscover.unwrap().enabled, "preserved verbatim");
        assert_eq!(m.links.len(), 1, "links carried through untouched");
    }

    /// Re-running `upsert` with the same member set is a no-op — `Unchanged`,
    /// no write.
    #[test]
    fn upsert_is_unchanged_on_an_identical_rerun() {
        let tmp = TempDir::new().unwrap();
        upsert(tmp.path(), "shop", &["api".into()]).unwrap();
        let path = tmp.path().join(MANIFEST_FILENAME);
        let before = fs::read_to_string(&path).unwrap();

        let step = upsert(tmp.path(), "shop", &["api".into()]).expect("no-op");
        assert_eq!(step.action, InitAction::Unchanged);
        assert_eq!(fs::read_to_string(&path).unwrap(), before, "byte-identical, no rewrite");
    }

    /// A member dropped from the approved set on a re-run is pruned from
    /// `members` — the incremental-prune half of FR-WS-02.
    #[test]
    fn upsert_prunes_a_member_no_longer_in_the_approved_set() {
        let tmp = TempDir::new().unwrap();
        upsert(tmp.path(), "shop", &["api".into(), "web".into()]).unwrap();
        upsert(tmp.path(), "shop", &["api".into()]).unwrap();

        let m = parse(&tmp.path().join(MANIFEST_FILENAME)).unwrap();
        assert_eq!(m.workspace.members, ["api"], "web was pruned");
    }

    // ── the `[governance]` workspace rule family (S-258, FR-WS-13) ────────

    /// The `[governance]` table parses into the rule family: named service
    /// layers, forbidden boundaries, and no-cross-service-callers contracts.
    #[test]
    fn parses_the_governance_rule_family() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join(MANIFEST_FILENAME);
        fs::write(
            &path,
            "[workspace]\nname = \"shop\"\nmembers = [\"web\", \"api\"]\n\n\
             [[governance.service_layers]]\nname = \"edge\"\nmembers = [\"web\"]\n\n\
             [[governance.service_layers]]\nname = \"core\"\nmembers = [\"api\"]\n\n\
             [[governance.boundaries]]\nfrom = \"edge\"\nto = \"core\"\nreason = \"gateway only\"\n\n\
             [[governance.no_cross_service_callers]]\nsymbol = \"*legacy*\"\nmember = \"api\"\n",
        )
        .unwrap();

        let m = parse(&path).unwrap();
        assert_eq!(m.governance.service_layers.len(), 2);
        assert_eq!(m.governance.service_layers[0].name, "edge");
        assert_eq!(m.governance.service_layers[0].members, ["web"]);
        assert_eq!(m.governance.boundaries.len(), 1);
        assert_eq!(m.governance.boundaries[0].from, "edge");
        assert_eq!(m.governance.boundaries[0].reason.as_deref(), Some("gateway only"));
        assert_eq!(m.governance.no_cross_service_callers.len(), 1);
        assert_eq!(m.governance.no_cross_service_callers[0].symbol, "*legacy*");
        assert_eq!(
            m.governance.no_cross_service_callers[0].member.as_deref(),
            Some("api")
        );
        assert!(!m.governance.is_empty(), "a declared boundary is a rule");
    }

    /// A manifest with no `[governance]` table parses to the empty family — the
    /// honest-empty predicate holds, so no report is ever produced ([NFR-CC-04]).
    #[test]
    fn a_manifest_without_governance_is_the_empty_family() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join(MANIFEST_FILENAME);
        fs::write(&path, "[workspace]\nname = \"shop\"\nmembers = [\"api\"]\n").unwrap();

        let m = parse(&path).unwrap();
        assert!(m.governance.is_empty(), "no [governance] ⇒ nothing to check");
    }

    /// Service layers ALONE are vocabulary, not policy: the family still reads as
    /// empty, so naming bands without forbidding anything produces no report.
    #[test]
    fn service_layers_alone_are_still_the_empty_family() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join(MANIFEST_FILENAME);
        fs::write(
            &path,
            "[workspace]\nname = \"shop\"\nmembers = [\"api\"]\n\n\
             [[governance.service_layers]]\nname = \"core\"\nmembers = [\"api\"]\n",
        )
        .unwrap();

        let m = parse(&path).unwrap();
        assert!(
            m.governance.is_empty(),
            "layers declare vocabulary; only a boundary/no-callers rule is a contract",
        );
    }

    /// `upsert` rebuilds the whole manifest, so it must carry the user-authored
    /// rule family across — a re-run of `logos init --workspace` must never
    /// silently drop `[governance]` (the same guarantee `[[links]]` has).
    #[test]
    fn upsert_preserves_the_governance_family() {
        let tmp = TempDir::new().unwrap();
        upsert(tmp.path(), "shop", &["api".into()]).unwrap();

        let path = tmp.path().join(MANIFEST_FILENAME);
        let existing = fs::read_to_string(&path).unwrap();
        fs::write(
            &path,
            format!(
                "{existing}\n[[governance.service_layers]]\nname = \"core\"\nmembers = [\"api\"]\n\n\
                 [[governance.boundaries]]\nfrom = \"edge\"\nto = \"core\"\n"
            ),
        )
        .unwrap();

        // A re-run that adds a member must not clobber the rules.
        upsert(tmp.path(), "shop", &["api".into(), "web".into()]).unwrap();

        let m = parse(&path).unwrap();
        assert_eq!(m.workspace.members, ["api", "web"], "the new member landed");
        assert_eq!(
            m.governance.boundaries.len(),
            1,
            "the user's workspace rules survived the re-run",
        );
        assert_eq!(m.governance.service_layers[0].name, "core");
    }

    /// A **layers-only** `[governance]` table survives `upsert` too.
    ///
    /// Regression test: the serialization predicate must be [`Governance::is_unset`],
    /// not [`Governance::is_empty`]. `is_empty` is the *policy* predicate and
    /// deliberately ignores `service_layers`, so wiring it to
    /// `skip_serializing_if` made `upsert` silently erase a manifest that declared
    /// layers but no rule yet — a natural intermediate authoring state. The
    /// sibling test above missed this because its fixture declares a boundary too.
    #[test]
    fn upsert_preserves_a_layers_only_governance_table() {
        let tmp = TempDir::new().unwrap();
        upsert(tmp.path(), "shop", &["api".into()]).unwrap();

        let path = tmp.path().join(MANIFEST_FILENAME);
        let existing = fs::read_to_string(&path).unwrap();
        fs::write(
            &path,
            format!("{existing}\n[[governance.service_layers]]\nname = \"core\"\nmembers = [\"api\"]\n"),
        )
        .unwrap();

        // The layers declare no policy...
        let m = parse(&path).unwrap();
        assert!(m.governance.is_empty(), "layers alone are not a contract");
        assert!(!m.governance.is_unset(), "...but they ARE authored content");

        upsert(tmp.path(), "shop", &["api".into(), "web".into()]).unwrap();

        let m = parse(&path).unwrap();
        assert_eq!(
            m.governance.service_layers.len(),
            1,
            "a layers-only table must survive the re-run, not be silently erased",
        );
        assert_eq!(m.governance.service_layers[0].name, "core");
    }

    /// An unknown key inside `[governance]` fails loud, like every other section
    /// (`deny_unknown_fields`) — a typo'd rule must never be silently ignored.
    #[test]
    fn an_unknown_governance_key_fails_loud() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join(MANIFEST_FILENAME);
        fs::write(
            &path,
            "[workspace]\nname = \"shop\"\nmembers = [\"api\"]\n\n\
             [[governance.boundaries]]\nfrom = \"edge\"\nto = \"core\"\nseverity = \"warn\"\n",
        )
        .unwrap();

        let err = parse(&path).expect_err("an unknown rule key must fail loud");
        assert_eq!(err.exit_code(), 2);
    }

    // ── the `[workspace.warm]` table (S-322, CR-099, FR-WS-01) ────────────

    /// The declared table parses and `warm_concurrency` reads the override —
    /// the value [`warm::effective_concurrency`] takes as its `Some` source.
    #[test]
    fn parses_the_workspace_warm_concurrency_key() {
        let tmp = TempDir::new().unwrap();
        let path = write_manifest(
            &tmp,
            "[workspace]\nname = \"pec\"\nmembers = [\"api\"]\n\n[workspace.warm]\nconcurrency = 2\n",
        );
        let m = parse(&path).expect("a declared warm bound parses");
        assert_eq!(m.workspace.warm.expect("present").concurrency, Some(2));
        assert_eq!(m.warm_concurrency(), Some(2));
        assert_eq!(
            warm::effective_concurrency(m.warm_concurrency()),
            2,
            "the declared value is the K the supervisor honours (BR-44)"
        );
    }

    /// A manifest **without** the table parses exactly as before: no override,
    /// so the core-derived default applies — and nothing is invented on write.
    #[test]
    fn a_manifest_without_the_warm_table_declares_no_override() {
        let tmp = TempDir::new().unwrap();
        let path = write_manifest(&tmp, "[workspace]\nname = \"solo\"\nmembers = [\"api\"]\n");
        let m = parse(&path).expect("a pre-key manifest keeps working");
        assert!(m.workspace.warm.is_none());
        assert_eq!(m.warm_concurrency(), None);
        assert_eq!(
            warm::effective_concurrency(m.warm_concurrency()),
            warm::default_concurrency(),
            "absent ⇒ the core-derived default"
        );

        // The write side of "unchanged": an absent table is never materialised,
        // so `upsert` over a pre-key manifest cannot introduce one.
        let text = toml::to_string_pretty(&m).unwrap();
        assert!(!text.contains("warm"), "no phantom table on write: {text}");
    }

    /// A bare `[workspace.warm]` is valid and declares no override — the same
    /// "documented but inert" posture `[workspace.autodiscover] enabled = false`
    /// has. It must not be mistaken for `concurrency = 0`.
    #[test]
    fn a_bare_warm_table_is_valid_and_declares_no_override() {
        let tmp = TempDir::new().unwrap();
        let path = write_manifest(&tmp, "[workspace]\nname = \"a\"\n\n[workspace.warm]\n");
        let m = parse(&path).expect("a bare table is valid");
        assert!(m.workspace.warm.is_some(), "the table itself is carried");
        assert_eq!(m.warm_concurrency(), None, "but it declares no override");
    }

    /// `concurrency = 0` is rejected at parse time with an actionable message —
    /// never silently floored to 1 by [`warm::effective_concurrency`], which is
    /// where a zero would otherwise disappear.
    #[test]
    fn a_zero_warm_concurrency_is_rejected_not_silently_floored() {
        let tmp = TempDir::new().unwrap();
        let path = write_manifest(
            &tmp,
            "[workspace]\nname = \"a\"\n\n[workspace.warm]\nconcurrency = 0\n",
        );
        let err = parse(&path).expect_err("zero must fail loud");
        let ConfigError::InvalidValue { ref key, ref message } = err else {
            panic!("expected an InvalidValue, got {err:?}");
        };
        assert_eq!(key, "workspace.warm.concurrency");
        assert!(
            message.contains(&format!("1..={}", warm::MANIFEST_CONCURRENCY_MAX)),
            "names the whole legal range, not just the bound breached: {message}"
        );
        assert!(
            message.contains("stall"),
            "and why zero specifically cannot be honoured: {message}"
        );
        assert!(
            message.contains("Omit the key"),
            "and how to get the default instead: {message}"
        );
        // A wrapped literal that lost its `\` continuations reads as a run of
        // spaces on the operator's terminal. Cheap to assert, and it is how
        // this very message was caught garbled before review.
        assert!(!message.contains("  "), "message is not garbled: {message:?}");
        assert_eq!(err.exit_code(), 2);
    }

    /// A value above [`warm::MANIFEST_CONCURRENCY_MAX`] is rejected at parse
    /// time — `concurrency = 84` (one per member) is exactly the unbounded
    /// fan-out BR-44 exists to prevent, so it must not be accepted verbatim.
    #[test]
    fn a_warm_concurrency_above_the_maximum_is_rejected() {
        let tmp = TempDir::new().unwrap();
        let path = write_manifest(
            &tmp,
            &format!(
                "[workspace]\nname = \"a\"\n\n[workspace.warm]\nconcurrency = {}\n",
                warm::MANIFEST_CONCURRENCY_MAX + 1
            ),
        );
        let err = parse(&path).expect_err("an out-of-range bound must fail loud");
        let ConfigError::InvalidValue { ref key, ref message } = err else {
            panic!("expected an InvalidValue, got {err:?}");
        };
        assert_eq!(key, "workspace.warm.concurrency");
        assert!(
            message.contains(&warm::MANIFEST_CONCURRENCY_MAX.to_string()),
            "names the ceiling: {message}"
        );
        assert!(
            message.contains(&(warm::MANIFEST_CONCURRENCY_MAX + 1).to_string()),
            "names the rejected value: {message}"
        );
        assert!(
            message.contains(&format!("1..={}", warm::MANIFEST_CONCURRENCY_MAX)),
            "names the whole legal range: {message}"
        );
        assert!(!message.contains("  "), "message is not garbled: {message:?}");
    }

    /// Both ends of the accepted range parse — the rejection is a range check,
    /// not an off-by-one that also refuses the legal boundary.
    #[test]
    fn the_warm_concurrency_range_boundaries_are_accepted() {
        for k in [1, warm::MANIFEST_CONCURRENCY_MAX] {
            let tmp = TempDir::new().unwrap();
            let path = write_manifest(
                &tmp,
                &format!("[workspace]\nname = \"a\"\n\n[workspace.warm]\nconcurrency = {k}\n"),
            );
            assert_eq!(
                parse(&path)
                    .expect("a boundary value is legal")
                    .warm_concurrency(),
                Some(k)
            );
        }
    }

    /// A non-integer value is a parse error (serde's own type rejection),
    /// carrying the offending key and line — never coerced, never clamped.
    #[test]
    fn a_non_integer_warm_concurrency_is_a_parse_error() {
        for bad in ["\"two\"", "2.5", "true", "-1"] {
            let tmp = TempDir::new().unwrap();
            let path = write_manifest(
                &tmp,
                &format!("[workspace]\nname = \"a\"\n\n[workspace.warm]\nconcurrency = {bad}\n"),
            );
            let err = parse(&path).expect_err("a non-integer must be rejected");
            assert!(
                matches!(err, ConfigError::Parse { .. }),
                "{bad} must be a parse error, got {err:?}"
            );
            // The doc promises the offending key reaches the operator. It does
            // so only through toml's rendered source snippet, so pin that
            // rather than the variant alone — losing the span would leave the
            // message un-actionable with every other assertion still green.
            let text = err.to_string();
            assert!(
                text.contains("concurrency"),
                "{bad} must name the offending key: {text}"
            );
            assert_eq!(err.exit_code(), 2);
        }
    }

    /// A value beyond the integer type is still a clean exit-2 config fault,
    /// not a panic or a wrap-around.
    ///
    /// Only the exit code is asserted: which variant it lands in is
    /// target-width dependent — on a 64-bit host `i64::MAX` deserialises into
    /// `usize` and is caught by the range check as `InvalidValue`, while on a
    /// 32-bit target serde rejects it by type as `Parse` first. Both are exit
    /// 2 with the key named, which is the property that matters.
    #[test]
    fn an_integer_beyond_the_type_is_still_a_clean_exit_2() {
        for bad in ["9223372036854775807", "99999999999999999999"] {
            let tmp = TempDir::new().unwrap();
            let path = write_manifest(
                &tmp,
                &format!("[workspace]\nname = \"a\"\n\n[workspace.warm]\nconcurrency = {bad}\n"),
            );
            let err = parse(&path).expect_err("an oversized integer must be rejected");
            assert_eq!(err.exit_code(), 2, "{bad}: {err}");
        }
    }

    /// An unknown key inside `[workspace.warm]` fails loud — the table is
    /// registered under `deny_unknown_fields` like every other.
    #[test]
    fn an_unknown_warm_key_fails_loud() {
        let tmp = TempDir::new().unwrap();
        let path = write_manifest(
            &tmp,
            "[workspace]\nname = \"a\"\n\n[workspace.warm]\nconcurrancy = 2\n",
        );
        assert!(matches!(parse(&path), Err(ConfigError::Parse { .. })));
    }

    /// `upsert` preserves the key across an incremental re-run — the same
    /// preservation `name`, `default`, `autodiscover` and `links` already get.
    /// The command owns `members` and nothing else.
    #[test]
    fn upsert_preserves_the_warm_concurrency_key() {
        let tmp = TempDir::new().unwrap();
        write_manifest(
            &tmp,
            "[workspace]\nname = \"pec\"\nmembers = [\"api\"]\n\n[workspace.warm]\nconcurrency = 3\n",
        );

        let step = upsert(tmp.path(), "ignored", &["api".into(), "web".into()]).expect("updates");
        assert_eq!(step.action, InitAction::Updated);

        let m = parse(&tmp.path().join(MANIFEST_FILENAME)).unwrap();
        assert_eq!(m.workspace.members, ["api", "web"], "members are upserted");
        assert_eq!(
            m.warm_concurrency(),
            Some(3),
            "a re-run must not erase the operator's tuned bound"
        );
    }

    /// Every optional table at once survives an `upsert` re-write together.
    ///
    /// Not redundant with the per-section preservation tests: `upsert` rebuilds
    /// the struct and re-serialises it, and TOML requires a table's scalar keys
    /// to precede its sub-tables. A field order that round-trips with **one**
    /// sub-table can still emit unparseable TOML with two, and the manifest
    /// this key is for is exactly the one that already declares the others.
    #[test]
    fn upsert_round_trips_a_manifest_declaring_every_optional_table() {
        let tmp = TempDir::new().unwrap();
        write_manifest(
            &tmp,
            "[workspace]\nname = \"pec\"\nmembers = [\"api\"]\ndefault = \"api\"\n\n\
             [workspace.autodiscover]\nenabled = false\n\n\
             [workspace.warm]\nconcurrency = 2\n\n\
             [workspace.member.api]\nkind = \"mock\"\n\n\
             [[links]]\nrelation = \"http_call\"\nfrom = \"web::c\"\nto = \"api::h\"\n\n\
             [[governance.service_layers]]\nname = \"core\"\nmembers = [\"api\"]\n",
        );

        upsert(tmp.path(), "ignored", &["api".into(), "web".into()]).expect("updates");

        // Re-parsing is the assertion that matters: it proves the re-written
        // TOML is still valid, not merely that a struct field was copied.
        let m = parse(&tmp.path().join(MANIFEST_FILENAME)).expect("the re-write is valid TOML");
        assert_eq!(m.workspace.members, ["api", "web"]);
        assert_eq!(m.workspace.default.as_deref(), Some("api"));
        assert!(!m.workspace.autodiscover.as_ref().expect("kept").enabled);
        assert_eq!(m.warm_concurrency(), Some(2));
        assert_eq!(m.workspace.member["api"].kind, Some(MemberKind::Mock));
        assert_eq!(m.links.len(), 1);
        assert_eq!(m.governance.service_layers.len(), 1);

        // And the rewrite is a fixed point. Re-parsing proves the output is
        // valid; this proves it is *settled* — without it, a serialisation that
        // never converges would rewrite the operator's tuned manifest on every
        // `logos init --workspace` and never once report `Unchanged`.
        let step = upsert(tmp.path(), "ignored", &["api".into(), "web".into()])
            .expect("a settled manifest re-runs clean");
        assert_eq!(step.action, InitAction::Unchanged);
    }

    /// A bare table survives `upsert` too: it is user-authored content, the
    /// same reason a layers-only `[governance]` is round-tripped rather than
    /// silently deleted on the next `logos init --workspace`.
    #[test]
    fn upsert_preserves_a_bare_warm_table() {
        let tmp = TempDir::new().unwrap();
        write_manifest(
            &tmp,
            "[workspace]\nname = \"pec\"\nmembers = [\"api\"]\n\n[workspace.warm]\n",
        );
        upsert(tmp.path(), "ignored", &["api".into()]).expect("upserts");

        let m = parse(&tmp.path().join(MANIFEST_FILENAME)).unwrap();
        assert!(
            m.workspace.warm.is_some(),
            "the declared table is carried through"
        );
    }

    // ── the per-member `kind` declaration (S-457, FR-WS-32) ──────────────

    /// A manifest declaring one member of each kind, beside one it leaves
    /// undeclared — the shape of the reference estate once its documentation
    /// repo and its mocks are declared.
    const KINDS: &str = "[workspace]\nname = \"pec\"\nmembers = [\"api\", \"docs\", \"pecserver-mock\"]\n\n\
        [workspace.member.docs]\nkind = \"documentation\"\n\n\
        [workspace.member.pecserver-mock]\nkind = \"mock\"\n";

    /// Both kinds parse under `deny_unknown_fields`, keyed by member path, and a
    /// member with no table declares nothing.
    #[test]
    fn parses_a_member_kind_for_each_variant() {
        let tmp = TempDir::new().unwrap();
        let m = parse(&write_manifest(&tmp, KINDS)).expect("both kinds parse");

        assert_eq!(m.workspace.member.len(), 2, "one table per declared member");
        assert_eq!(m.workspace.member["docs"].kind, Some(MemberKind::Documentation));
        assert_eq!(m.workspace.member["pecserver-mock"].kind, Some(MemberKind::Mock));
        assert!(
            !m.workspace.member.contains_key("api"),
            "an undeclared member has no table and no kind"
        );
    }

    /// `platform` parses beside `documentation` and `mock` under
    /// `deny_unknown_fields`, and survives `upsert` the same way the other two
    /// do (S-463, FR-WS-33).
    #[test]
    fn platform_parses_beside_documentation_and_mock_and_survives_upsert() {
        let tmp = TempDir::new().unwrap();
        let text = format!(
            "{KINDS}\n[workspace.member.poste-pec-starter]\nkind = \"platform\"\n"
        );
        let m = parse(&write_manifest(&tmp, &text)).expect("all three kinds parse");
        assert_eq!(m.workspace.member["docs"].kind, Some(MemberKind::Documentation));
        assert_eq!(m.workspace.member["pecserver-mock"].kind, Some(MemberKind::Mock));
        assert_eq!(
            m.workspace.member["poste-pec-starter"].kind,
            Some(MemberKind::Platform)
        );

        upsert(tmp.path(), "ignored", &["api".into(), "poste-pec-starter".into()])
            .expect("upserts");
        let m = parse(&tmp.path().join(MANIFEST_FILENAME)).expect("the re-write parses");
        assert_eq!(
            m.workspace.member["poste-pec-starter"].kind,
            Some(MemberKind::Platform),
            "a platform declaration is carried across a re-run"
        );
    }

    /// What each kind does is asked of the kind: a platform member keeps its
    /// contract surface in the headline and sets its inbound build edges apart;
    /// documentation and mock do the reverse. The spelling round-trips through
    /// serde so the payload token and the manifest token cannot drift.
    #[test]
    fn each_kind_answers_both_headline_questions() {
        for (kind, token, leaves_contract, sets_build_apart) in [
            (MemberKind::Documentation, "documentation", true, false),
            (MemberKind::Mock, "mock", true, false),
            (MemberKind::Platform, "platform", false, true),
        ] {
            assert_eq!(kind.as_str(), token);
            assert_eq!(
                serde_json::to_value(kind).unwrap(),
                serde_json::Value::String(token.to_string())
            );
            assert_eq!(kind.leaves_contract_surface_headline(), leaves_contract, "{token}");
            assert_eq!(kind.sets_inbound_build_edges_apart(), sets_build_apart, "{token}");
        }
    }

    /// A manifest without any `[workspace.member]` table parses exactly as
    /// before the key existed, and `upsert`'s fresh manifest writes none —
    /// logos never declares a kind on a human's behalf.
    #[test]
    fn a_manifest_without_member_declarations_parses_unchanged() {
        let tmp = TempDir::new().unwrap();
        let m = parse(&write_manifest(&tmp, "[workspace]\nname = \"a\"\nmembers = [\"x\"]\n"))
            .expect("parses");
        assert!(m.workspace.member.is_empty());

        let fresh = TempDir::new().unwrap();
        upsert(fresh.path(), "pec", &["api".into()]).expect("creates");
        let text = fs::read_to_string(fresh.path().join(MANIFEST_FILENAME)).unwrap();
        assert_eq!(
            text, "[workspace]\nname = \"pec\"\nmembers = [\"api\"]\n",
            "a fresh manifest is byte-identical to what it was before the key existed"
        );
    }

    /// A value outside the enum is rejected **at parse time**, and the message
    /// names the key, the offending value and every legal value — the operator
    /// can fix the line from the message alone.
    #[test]
    fn a_malformed_member_kind_is_rejected_at_parse_time_naming_the_field() {
        let tmp = TempDir::new().unwrap();
        let path = write_manifest(
            &tmp,
            "[workspace]\nname = \"a\"\nmembers = [\"docs\"]\n\n[workspace.member.docs]\nkind = \"docs\"\n",
        );
        let err = parse(&path).expect_err("an unknown kind must not parse");
        assert!(matches!(err, ConfigError::Parse { .. }), "exit-2 parse error, got {err:?}");
        let message = err.to_string();
        for needle in ["kind", "`docs`", "documentation", "mock", "platform"] {
            assert!(message.contains(needle), "{needle:?} missing from {message:?}");
        }
    }

    /// A wrong-typed value is the same fail-loud parse error, naming the key.
    #[test]
    fn a_non_string_member_kind_is_rejected_naming_the_field() {
        let tmp = TempDir::new().unwrap();
        let path = write_manifest(
            &tmp,
            "[workspace]\nname = \"a\"\n\n[workspace.member.docs]\nkind = 3\n",
        );
        let err = parse(&path).expect_err("a non-string kind must not parse");
        assert!(matches!(err, ConfigError::Parse { .. }));
        assert!(err.to_string().contains("kind"), "{err}");
    }

    /// A misspelt key inside a member table fails loud under
    /// `deny_unknown_fields`, naming the key that was written.
    #[test]
    fn an_unknown_key_in_a_member_table_fails_loud() {
        let tmp = TempDir::new().unwrap();
        let path = write_manifest(
            &tmp,
            "[workspace]\nname = \"a\"\n\n[workspace.member.docs]\nkinds = \"documentation\"\n",
        );
        let err = parse(&path).expect_err("an unknown member key must not parse");
        assert!(matches!(err, ConfigError::Parse { .. }));
        assert!(err.to_string().contains("kinds"), "{err}");
    }

    /// A bare member table is valid and inert, like a bare `[workspace.warm]`.
    #[test]
    fn a_bare_member_table_is_valid_and_declares_no_kind() {
        let tmp = TempDir::new().unwrap();
        let m = parse(&write_manifest(
            &tmp,
            "[workspace]\nname = \"a\"\n\n[workspace.member.docs]\n",
        ))
        .expect("parses");
        assert_eq!(m.workspace.member["docs"].kind, None);
    }

    /// `upsert` carries every declaration across an `init --workspace` re-run —
    /// including one for a member the approved set no longer holds, because the
    /// command owns `members`, not what a human wrote about them — and the
    /// rewrite is a fixed point.
    #[test]
    fn upsert_preserves_member_kind_declarations() {
        let tmp = TempDir::new().unwrap();
        write_manifest(&tmp, KINDS);

        let step = upsert(tmp.path(), "ignored", &["api".into(), "docs".into(), "web".into()])
            .expect("updates");
        assert_eq!(step.action, InitAction::Updated);

        let m = parse(&tmp.path().join(MANIFEST_FILENAME)).expect("the re-write is valid TOML");
        assert_eq!(m.workspace.members, ["api", "docs", "web"]);
        assert_eq!(m.workspace.member["docs"].kind, Some(MemberKind::Documentation));
        assert_eq!(
            m.workspace.member["pecserver-mock"].kind,
            Some(MemberKind::Mock),
            "a declaration outlives its member's removal from the approved set"
        );

        let step = upsert(tmp.path(), "ignored", &["api".into(), "docs".into(), "web".into()])
            .expect("a settled manifest re-runs clean");
        assert_eq!(step.action, InitAction::Unchanged);
    }

    // ── the whole-manifest write path (S-430, FR-UI-38) ──────────────────

    /// A manifest declaring every optional table, with operator comments — the
    /// shape a hand-tuned workspace actually has, and the one a re-serialising
    /// writer would damage first.
    const EVERY_TABLE: &str = "# the estate's manifest — hand-tuned\n\
        [workspace]\nname = \"pec\"\nmembers = [\"api\", \"web\"] # sorted by hand\ndefault = \"api\"\n\n\
        [workspace.autodiscover]\nenabled = false\n\n\
        [workspace.warm]\nconcurrency = 2\n\n\
        [[links]]\nrelation = \"http_call\"\nfrom = \"web::c\"\nto = \"api::h\"\n\n\
        # layers first, then the rules over them\n\
        [[governance.service_layers]]\nname = \"core\"\nmembers = [\"api\"]\n\n\
        [[governance.service_layers]]\nname = \"edge\"\nmembers = [\"web\"]\n\n\
        [[governance.boundaries]]\nfrom = \"edge\"\nto = \"core\"\nreason = \"edge talks to core through the gateway\"\n\n\
        [[governance.no_cross_service_callers]]\nsymbol = \"*legacy*\"\n";

    /// Pin the manifest's mtime to a fixed instant in the past, so "no write
    /// happened" is an observable fact: any write moves it to now.
    fn backdate(path: &std::path::Path) -> std::time::SystemTime {
        let past = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(past)
            .unwrap();
        assert_eq!(fs::metadata(path).unwrap().modified().unwrap(), past, "backdated");
        past
    }

    /// The editor loads the literal document with the fingerprint of its bytes
    /// and the parse verdict over them.
    #[test]
    fn read_document_returns_the_literal_bytes_their_fingerprint_and_the_parse() {
        let tmp = TempDir::new().unwrap();
        write_manifest(&tmp, EVERY_TABLE);
        let doc = read_document(tmp.path()).expect("reads");
        assert_eq!(doc.path, MANIFEST_FILENAME);
        assert_eq!(doc.content, EVERY_TABLE, "the literal document, comments included");
        assert_eq!(doc.fingerprint, fingerprint(EVERY_TABLE.as_bytes()));
        let parsed = doc.parsed.expect("a valid manifest parses");
        assert_eq!(parsed.governance.boundaries.len(), 1);
        assert!(doc.error.is_none());
    }

    /// A manifest broken on disk is reported, not refused: the editor is the
    /// repair path, so it must still receive the document and a fingerprint.
    #[test]
    fn read_document_reports_an_unparsable_manifest_rather_than_refusing_it() {
        let tmp = TempDir::new().unwrap();
        let broken = "[workspace]\nname = \"a\"\nmembrs = [\"typo\"]\n";
        write_manifest(&tmp, broken);
        let doc = read_document(tmp.path()).expect("an unparsable manifest still reads");
        assert_eq!(doc.content, broken);
        assert!(doc.parsed.is_none());
        let error = doc.error.expect("the parse fault is named");
        assert!(error.contains("membrs"), "the parser's own message: {error}");

        // …and a save against its fingerprint repairs it.
        let fixed = "[workspace]\nname = \"a\"\nmembers = [\"typo\"]\n";
        let out = save_document(tmp.path(), fixed, &doc.fingerprint).expect("repairs");
        assert!(matches!(out, ManifestSaveOutcome::Written { .. }), "{out:?}");
        assert!(parse(&tmp.path().join(MANIFEST_FILENAME)).is_ok());
    }

    /// A workspace whose manifest vanished is an I/O fault, not an empty editor.
    #[test]
    fn read_document_of_a_missing_manifest_is_an_io_error() {
        let tmp = TempDir::new().unwrap();
        assert!(matches!(read_document(tmp.path()), Err(ConfigError::Io { .. })));
    }

    /// **Validate before write.** Every candidate `parse` rejects — bad TOML, an
    /// unknown key, a missing required key, an out-of-range value — is refused
    /// with the parser's error and exit code 2, and the manifest on disk is
    /// byte-identical afterwards (and was not even rewritten: mtime unmoved).
    #[test]
    fn save_document_refuses_a_candidate_parse_rejects_and_leaves_the_file_byte_identical() {
        let tmp = TempDir::new().unwrap();
        let path = write_manifest(&tmp, EVERY_TABLE);
        let loaded = read_document(tmp.path()).unwrap().fingerprint;
        let past = backdate(&path);

        for candidate in [
            "this is not toml = = =",
            "[workspace]\nname = \"pec\"\nmembrs = [\"api\"]\n",
            "[workspace]\nmembers = [\"api\"]\n",
            "[workspace]\nname = \"pec\"\n\n[workspace.warm]\nconcurrency = 0\n",
            "[workspace]\nname = \"pec\"\n\n[[governance.boundaries]]\nfrom = \"a\"\n",
        ] {
            let err = save_document(tmp.path(), candidate, &loaded)
                .expect_err("a candidate parse rejects is refused");
            assert!(
                matches!(err, ConfigError::Parse { .. } | ConfigError::InvalidValue { .. }),
                "refused as a config fault, not an I/O one: {err:?}"
            );
            assert_eq!(err.exit_code(), 2);
            assert_eq!(fs::read(&path).unwrap(), EVERY_TABLE.as_bytes(), "byte-identical");
            assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), past, "not rewritten");
        }
    }

    /// **Byte-identical ⇒ unchanged, without writing** — `upsert`'s posture.
    #[test]
    fn save_document_reports_unchanged_without_writing_on_an_identical_candidate() {
        let tmp = TempDir::new().unwrap();
        let path = write_manifest(&tmp, EVERY_TABLE);
        let loaded = read_document(tmp.path()).unwrap().fingerprint;
        let past = backdate(&path);

        let out = save_document(tmp.path(), EVERY_TABLE, &loaded).expect("a no-op save");
        assert_eq!(
            out,
            ManifestSaveOutcome::Unchanged {
                path: MANIFEST_FILENAME.to_string(),
                fingerprint: loaded,
            }
        );
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), past, "no write (mtime unmoved)");
    }

    /// **Nothing is invented on save** ([NFR-RA-05]). The candidate is written
    /// verbatim — never re-serialised — so a manifest declaring no optional table
    /// gains none, and one declaring all of them keeps every comment, key order
    /// and table exactly as the operator wrote them. `upsert`, which rebuilds the
    /// struct, cannot say either.
    #[test]
    fn save_document_writes_the_candidate_verbatim_and_invents_no_undeclared_table() {
        let tmp = TempDir::new().unwrap();
        let path = write_manifest(&tmp, EVERY_TABLE);

        // Every table declared; one value changed by the editor.
        let edited = EVERY_TABLE.replace("concurrency = 2", "concurrency = 3");
        let loaded = read_document(tmp.path()).unwrap().fingerprint;
        let out = save_document(tmp.path(), &edited, &loaded).expect("writes");
        assert_eq!(
            out,
            ManifestSaveOutcome::Written {
                path: MANIFEST_FILENAME.to_string(),
                bytes_written: edited.len() as u64,
                fingerprint: fingerprint(edited.as_bytes()),
            }
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), edited, "verbatim, comments included");

        // No optional table declared: none appears on disk.
        let bare = "[workspace]\nname = \"pec\"\n";
        let loaded = read_document(tmp.path()).unwrap().fingerprint;
        save_document(tmp.path(), bare, &loaded).expect("writes");
        assert_eq!(fs::read_to_string(&path).unwrap(), bare, "no table invented");
        let m = parse(&path).unwrap();
        assert!(m.governance.is_unset() && m.workspace.warm.is_none() && m.links.is_empty());
    }

    /// **No silent clobber.** The manifest is changed on disk after the editor
    /// loaded it — by `upsert`, the `logos init --workspace` writer that really
    /// does run while a tab is open — so a save against the stale fingerprint is
    /// refused with a conflict carrying what is on disk now, and writes nothing.
    /// Re-saving against the conflict's own fingerprint is the explicit overwrite.
    #[test]
    fn save_document_refuses_a_stale_fingerprint_with_a_conflict_and_writes_nothing() {
        let tmp = TempDir::new().unwrap();
        let path = write_manifest(&tmp, EVERY_TABLE);
        let loaded = read_document(tmp.path()).unwrap().fingerprint;

        upsert(tmp.path(), "ignored", &["api".into(), "web".into(), "worker".into()])
            .expect("a concurrent init --workspace re-run");
        let on_disk = fs::read_to_string(&path).unwrap();
        assert_ne!(on_disk, EVERY_TABLE, "the fixture really did change under the editor");
        let past = backdate(&path);

        let mine = EVERY_TABLE.replace("concurrency = 2", "concurrency = 3");
        let out = save_document(tmp.path(), &mine, &loaded).expect("a conflict is an outcome");
        let disk_fingerprint = fingerprint(on_disk.as_bytes());
        assert_eq!(
            out,
            ManifestSaveOutcome::Conflict {
                path: MANIFEST_FILENAME.to_string(),
                loaded_fingerprint: loaded.clone(),
                disk_fingerprint: disk_fingerprint.clone(),
                disk_content: on_disk.clone(),
            }
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), on_disk, "the other writer's edit survives");
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), past, "nothing was written");

        let out = save_document(tmp.path(), &mine, &disk_fingerprint).expect("explicit overwrite");
        assert!(matches!(out, ManifestSaveOutcome::Written { .. }), "{out:?}");
        assert_eq!(fs::read_to_string(&path).unwrap(), mine);
    }

    /// The fingerprint is a content hash of the exact bytes — pinned to BLAKE3's
    /// published digest of the empty input, so a cheaper stand-in (a length, an
    /// mtime) cannot pass for it.
    #[test]
    fn fingerprint_is_the_blake3_hex_of_the_exact_bytes() {
        assert_eq!(
            fingerprint(b""),
            "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
        );
        assert_ne!(fingerprint(b"concurrency = 2"), fingerprint(b"concurrency = 3"));
    }

    /// **No silent clobber, same-length edit.** The concurrent change most likely
    /// to slip past a weak check is one that keeps the file's size — a hand-tuned
    /// digit — so that is what changes on disk here.
    #[test]
    fn a_same_length_edit_on_disk_since_the_load_is_still_a_conflict() {
        let tmp = TempDir::new().unwrap();
        let path = write_manifest(&tmp, EVERY_TABLE);
        let loaded = read_document(tmp.path()).unwrap().fingerprint;
        let theirs = EVERY_TABLE.replace("concurrency = 2", "concurrency = 3");
        assert_eq!(theirs.len(), EVERY_TABLE.len(), "the edit keeps the size");
        fs::write(&path, &theirs).unwrap();

        let mine = EVERY_TABLE.replace("concurrency = 2", "concurrency = 4");
        let out = save_document(tmp.path(), &mine, &loaded).expect("a conflict is an outcome");
        assert!(matches!(out, ManifestSaveOutcome::Conflict { .. }), "{out:?}");
        assert_eq!(fs::read_to_string(&path).unwrap(), theirs, "their edit survives");
    }

    /// A manifest removed after the editor loaded it is an I/O fault — never an
    /// empty "disk" that a conflict would then offer to overwrite, recreating a
    /// workspace someone deliberately took down.
    #[test]
    fn a_manifest_removed_since_the_load_is_an_io_error_and_nothing_is_written() {
        let tmp = TempDir::new().unwrap();
        let path = write_manifest(&tmp, EVERY_TABLE);
        let loaded = read_document(tmp.path()).unwrap().fingerprint;
        fs::remove_file(&path).unwrap();

        let err = save_document(tmp.path(), EVERY_TABLE, &loaded).expect_err("no manifest to save over");
        assert!(matches!(err, ConfigError::Io { .. }), "{err:?}");
        assert!(!path.exists(), "the manifest was not recreated");
    }
}
