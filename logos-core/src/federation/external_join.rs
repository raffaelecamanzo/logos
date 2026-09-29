//! The **external join** — a `no-provider-in-workspace` REST call binds
//! `bound-external` to the named external its own member declares, on an exact
//! path under a committed base path ([ADR-68] point 3, [FR-WS-05], [FR-WS-19]).
//!
//! A member that vendors a spec document declares a contract to a named
//! external ([`declared_contracts`](super::declared_contracts), [ADR-68]
//! point 1). Its outbound calls to that API have no provider in the workspace —
//! an external is not a member — so the coverage tier files them
//! `no-provider-in-workspace`. This module reads the document the member itself
//! holds and names, for each such call, the operation it invokes: when the
//! call's composed path, joined under a base path the member's committed
//! sources prove, **equals** one of the member's own copy's operations.
//!
//! # Beside, never inside ([BR-57], [ADR-26], [ADR-52])
//!
//! The join produces no [`BridgeEdge`](super::BridgeEdge), moves no coverage
//! row and no bucket, and is reported as [`BoundExternal`] beside
//! `egress_resolution` with its own denominator — every
//! invocation-intake `no-provider-in-workspace` REST row. An external has no
//! engine and no route, so the bridge has nothing to draw an edge to; and a row
//! the coverage tier reported `bound` without a bridge edge would be the
//! two-classifier drift [ADR-52] forbids. `resolved_cross_service_edges` and
//! `egress_resolution` are therefore unchanged by construction, and the
//! coverage tier's tests pin that byte for byte.
//!
//! # The consumed API — never re-derived
//!
//! Which externals a member declares, and which operations its copy carries,
//! is [`DeclaredContractRelation::externals_declared_by`] and
//! [`DeclaredContract::operations`] — [S-458]'s relation, read, not rebuilt.
//! The composed paths are the coverage tier's own: a configuration-bound call's
//! committed compositions (the bridge's `identify`), a literal call's
//! stored target.
//!
//! # The committed base path ([ADR-64], [ADR-68] point 3)
//!
//! A call's base URL is the application-configuration key committing a URL in
//! the **same namespace** as the key its path was read from
//! (`pec-server.base-url` beside `pec-server.uri-get-mailbox-path`). A deploy
//! overlay that overrides that key — `PECSERVER_BASEURL` in a Helm values file —
//! replaces it, as it does at deploy time; **for this join only** the overlay's
//! base-url path is committed evidence. Every overriding overlay must commit the
//! same path, else the row is refused naming each path and its file. A call
//! whose key names no namespace — a literal target — proves no base path.
//!
//! **Only files the discovery walk admitted are opened** ([FR-WS-19]): the
//! walk is [`ConfigCorpus::discover`]'s — hidden directories and ignored files
//! excluded — and among the files it admitted only Helm values files and Compose
//! files are read as overlays ([`is_deploy_overlay`]). Nothing read here enters
//! the configuration corpus: no key is added, no store is written, and the read
//! happens on the cross-service query, never at index or sync time — which is
//! why incremental sync is untouched ([NFR-PE-02]). The rest of [S-412]'s
//! overlay admission stays out of scope.
//!
//! # Refusals ([NFR-RA-05])
//!
//! Suffix matching is never admitted: a call whose joined path is only the tail
//! of an operation is refused [`JoinRefusal::SuffixOnly`]. So are a call to an
//! external its member does not itself declare, overlays that disagree on the
//! base path, and a base path committed only as an environment indirection.
//! Every refusal is a row with its reason, counted in the denominator.
//!
//! # An accepted [NFR-MA-01] carve-out, recorded rather than implied
//!
//! [`overlay_overrides`] is Spring's environment-variable relaxed binding — the
//! rule by which `PECSERVER_BASEURL` overrides `pec-server.base-url`. It is the
//! environment-variable form of [`canonical_key`], whose Spring relaxed binding
//! [`corpus`](crate::extract::config::corpus) already carries as a recorded
//! [NFR-MA-01] exception; this rule joins that exception rather than opening a
//! new one, and it is named here so a second language's binding convention is
//! known to need a plugin row. [`is_deploy_overlay`]'s file names are a
//! directory convention (Helm, Compose), not framework judgement.
//!
//! # Promoted from the S-456 harness
//!
//! [`url_path`], [`parent`], [`overlay_overrides`] and [`join_base`] were moved
//! **verbatim** out of `logos-core/tests/operand_resolvability/vendored_spec_contracts.rs`,
//! which measured this join before it was built, and the harness now imports
//! them — one rule, not a hand-mirrored twin. [`base_reading`] is the product's
//! own reading and differs from the harness's in two deliberate ways: the
//! overlays are the discovery walk's (the harness walked hidden directories,
//! which is how it read `notification-adapter`'s `.helm/values.yaml`), and a
//! base-url key committed only as a `${…}` indirection is a base key whose
//! refusal is named ([`JoinRefusal::BasePathUncommitted`]) where the harness saw
//! no key at all.
//!
//! [ADR-26]: ../../../docs/specs/architecture/decisions/ADR-26.md
//! [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
//! [ADR-64]: ../../../docs/specs/architecture/decisions/ADR-64.md
//! [ADR-68]: ../../../docs/specs/architecture/decisions/ADR-68.md
//! [BR-57]: ../../../docs/specs/software-spec.md#327-workspace-federation
//! [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
//! [FR-WS-19]: ../../../docs/specs/requirements/FR-WS-19.md
//! [NFR-MA-01]: ../../../docs/specs/requirements/NFR-MA-01.md
//! [NFR-PE-02]: ../../../docs/specs/requirements/NFR-PE-02.md
//! [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
//! [S-412]: ../../../docs/planning/journal.md#s-412-the-configuration-corpus-admits-committed-deploy-overlays
//! [S-458]: ../../../docs/planning/journal.md#s-458-a-vendored-spec-is-a-declared-contract-to-a-member-or-a-named-external
//! [`canonical_key`]: crate::extract::config::corpus::canonical_key

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::Serialize;

use crate::extract::config::corpus::{parse_yaml, ConfigCorpus};
use crate::resolve::route_template::{normalize_template, parse_method_and_template};

use super::bridge::BridgeEndpoint;
use super::declared_contracts::{
    ContractTarget, DeclaredContract, DeclaredContractRelation, ExternalId, NamedExternal, OperationKey,
};

// ── Promoted verbatim from the S-456 harness ───────────────────────────────

/// The URL path of a committed value: `https://h:8443/prov` → `/prov`,
/// `http://localhost:8082` → `""`. `None` when the value is not a URL with a
/// literal authority.
pub fn url_path(value: &str) -> Option<String> {
    let (scheme, rest) = value.split_once("://")?;
    if scheme.is_empty() || !scheme.chars().all(|c| c.is_ascii_alphanumeric() || c == '+') {
        return None;
    }
    let cut = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..cut];
    if authority.is_empty() || authority.contains(['{', '}', '$']) {
        return None;
    }
    let tail = &rest[cut..];
    let path = tail.split(['?', '#']).next().unwrap_or("");
    Some(path.trim_end_matches('/').to_string())
}

/// The parent namespace of a canonical key — everything before its last `.`.
pub fn parent(key: &str) -> &str {
    key.rsplit_once('.').map_or("", |(p, _)| p)
}

/// Whether a canonical overlay key overrides a canonical application key under
/// Spring's relaxed binding.
///
/// The overlay keys reach it **already canonicalised** by the shipped
/// [`parse_yaml`] (`_` and `-` dropped per segment), so an environment
/// variable's underscore positions are gone before this sees them. The
/// comparison is therefore made on the dot-free canonical form:
/// `pecserverbaseurl` against `pecserver.baseurl` with its dots removed. That
/// admits an underscore placed anywhere (`PECSERVERBASE_URL` too) — a known,
/// bounded over-admission, confined to one member's own base-url namespace.
pub fn overlay_overrides(overlay_key: &str, base_key: &str) -> bool {
    if overlay_key == base_key || overlay_key.ends_with(&format!(".{base_key}")) {
        return true;
    }
    let last = overlay_key.rsplit('.').next().unwrap_or(overlay_key);
    !last.is_empty() && last == base_key.replace('.', "")
}

/// Join a base path and a call path: `/prov` + `/domain/{d}` → `/prov/domain/{d}`.
pub fn join_base(base: &str, path: &str) -> String {
    format!("{}{path}", base.trim_end_matches('/'))
}

// ── The committed facts one member's base path is read from ────────────────

/// Directories whose charts describe **something else** — a documentation,
/// example or tutorial tree's complete, valid-looking chart is not this
/// member's deployment ([S-411]'s finding, where one such chart was read as a
/// decisive identity).
///
/// [S-411]: ../../../docs/planning/journal.md#s-411-measure-config-declared-coupling-over-the-reference-estate
const NOT_DEPLOYMENT_TREES: [&str; 5] = ["documentation/", "examples/", "tutorial/", "tutorials/", "docs/"];

/// Whether a member-relative path the discovery walk admitted is a **deploy
/// overlay** this join reads: a Helm values file (`values` anywhere in a YAML
/// file's stem — `values.yaml`, `values_TEMPLATE.yaml`, `prod-values.yml`) or a
/// Compose file (`docker-compose*.yml`), outside a documentation, example or
/// tutorial tree.
///
/// Narrower than S-456's harness, which read any YAML the configuration corpus
/// does not own: a raw Kubernetes manifest carries its environment as a
/// sequence the shipped [`parse_yaml`] deliberately does not bind, so reading
/// one here would cost a parse and prove nothing.
pub fn is_deploy_overlay(rel: &str) -> bool {
    let lower = rel.to_ascii_lowercase();
    if NOT_DEPLOYMENT_TREES.iter().any(|d| lower.starts_with(d) || lower.contains(&format!("/{d}"))) {
        return false;
    }
    let name = lower.rsplit('/').next().unwrap_or(&lower);
    let Some(stem) = name.strip_suffix(".yaml").or_else(|| name.strip_suffix(".yml")) else {
        return false;
    };
    stem.contains("values") || stem.starts_with("docker-compose")
}

/// One committed scalar: its canonical key, its value and the member-relative
/// file committing it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CommittedValue {
    /// The canonical key ([`canonical_key`](crate::extract::config::corpus::canonical_key)).
    pub key: String,
    /// The committed literal.
    pub value: String,
    /// The member-relative file that commits it.
    pub file: String,
}

impl CommittedValue {
    /// Where this value is committed: its file and key.
    fn source(&self) -> BaseSource {
        BaseSource { file: self.file.clone(), key: self.key.clone() }
    }
}

/// Everything one member's base path is read from: its application
/// configuration and its admitted deploy overlays.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BaseFacts {
    /// Every value the member's configuration sources commit, every profile.
    pub application: Vec<CommittedValue>,
    /// Every value its admitted deploy overlays commit.
    pub overlays: Vec<CommittedValue>,
}

impl BaseFacts {
    /// Read `root`'s base facts: one [`ConfigCorpus::discover`] walk, whose
    /// sources are the application configuration, and — among the files that
    /// same walk admitted — each [deploy overlay](is_deploy_overlay), read and
    /// flattened by the shipped [`parse_yaml`].
    ///
    /// Opens no file the walk did not admit, and writes nothing: the overlay
    /// values live in the returned value only, never in the corpus.
    pub fn read(root: &Path) -> Self {
        let corpus = ConfigCorpus::discover(root);
        let application = corpus
            .sources
            .iter()
            .flat_map(|source| {
                source.values.iter().flat_map(move |(key, values)| {
                    values.iter().map(move |value| CommittedValue {
                        key: key.clone(),
                        value: value.clone(),
                        file: source.path.clone(),
                    })
                })
            })
            .collect();
        let mut overlays = Vec::new();
        for rel in corpus.files().iter().filter(|rel| is_deploy_overlay(rel)) {
            let Ok(text) = std::fs::read_to_string(root.join(rel)) else {
                continue;
            };
            for (key, values) in parse_yaml(&text) {
                for value in values {
                    overlays.push(CommittedValue { key: key.clone(), value, file: rel.clone() });
                }
            }
        }
        Self { application, overlays }
    }
}

// ── The base-path reading ──────────────────────────────────────────────────

/// Where a base path was committed: the file and the key.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct BaseSource {
    /// The member-relative file.
    pub file: String,
    /// The canonical key it commits.
    pub key: String,
}

/// One committed base path and where it came from — a disagreement's line.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct BasePath {
    /// The URL path (`""` for a URL with none).
    pub path: String,
    /// The member-relative file committing it.
    pub file: String,
    /// The key it commits it under.
    pub key: String,
}

/// Which committed source proved a base path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BaseOrigin {
    /// The member's application configuration.
    ApplicationConfig,
    /// A deploy overlay overriding the application's base-url key.
    DeployOverlay,
}

/// Whether a committed value is an indirection — a `${…}` placeholder.
fn is_indirection(value: &str) -> bool {
    value.contains("${")
}

/// Read a call's committed base path from its member's `facts`: the path and
/// its evidence, or the refusal naming why none is proven
/// ([`NoBaseKey`](JoinRefusal::NoBaseKey),
/// [`BasePathUncommitted`](JoinRefusal::BasePathUncommitted),
/// [`BasePathsDisagree`](JoinRefusal::BasePathsDisagree)).
///
/// `call_keys` are the canonical keys the call's target named. The base-url
/// keys are the other keys of their namespaces whose application value is a URL
/// or an indirection. An overlay value overriding one of them
/// ([`overlay_overrides`]) replaces the application value — decided by the
/// **key**, so an overriding overlay committing no URL path proves nothing and
/// does not let the application value back in; beside overlays that do commit a
/// path, it adds nothing.
pub fn base_reading(call_keys: &BTreeSet<String>, facts: &BaseFacts) -> Result<BasePathEvidence, JoinRefusal> {
    let parents: BTreeSet<&str> = call_keys.iter().map(|k| parent(k)).filter(|p| !p.is_empty()).collect();
    let base_values: Vec<&CommittedValue> = facts
        .application
        .iter()
        .filter(|v| parents.contains(parent(&v.key)) && !call_keys.contains(&v.key))
        .filter(|v| url_path(&v.value).is_some() || is_indirection(&v.value))
        .collect();
    let base_keys: BTreeSet<&str> = base_values.iter().map(|v| v.key.as_str()).collect();
    if base_keys.is_empty() {
        return Err(JoinRefusal::NoBaseKey);
    }
    let overriding: Vec<&CommittedValue> = facts
        .overlays
        .iter()
        .filter(|v| base_keys.iter().any(|k| overlay_overrides(&v.key, k)))
        .collect();
    let (chosen, origin) = if overriding.is_empty() {
        (base_values, BaseOrigin::ApplicationConfig)
    } else {
        (overriding, BaseOrigin::DeployOverlay)
    };
    let mut by_path: BTreeMap<String, BTreeSet<BaseSource>> = BTreeMap::new();
    for v in &chosen {
        if let Some(path) = url_path(&v.value) {
            by_path.entry(path).or_default().insert(v.source());
        }
    }
    match by_path.len() {
        0 => Err(JoinRefusal::BasePathUncommitted {
            sources: chosen.iter().map(|v| v.source()).collect::<BTreeSet<_>>().into_iter().collect(),
        }),
        1 => {
            let (path, sources) = by_path.into_iter().next().expect("one path");
            Ok(BasePathEvidence { path, origin, sources: sources.into_iter().collect() })
        }
        _ => Err(JoinRefusal::BasePathsDisagree {
            paths: by_path
                .into_iter()
                .flat_map(|(path, sources)| {
                    sources.into_iter().map(move |s| BasePath { path: path.clone(), file: s.file, key: s.key })
                })
                .collect(),
        }),
    }
}

// ── One row's outcome ──────────────────────────────────────────────────────

/// The base path a binding was made under, and the committed evidence for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BasePathEvidence {
    /// The base path (`""` for a base URL with none).
    pub path: String,
    /// Which source set proved it.
    pub origin: BaseOrigin,
    /// Every file and key committing it — the overlay or the application
    /// configuration file, and the key.
    pub sources: Vec<BaseSource>,
}

/// A call bound to an operation of a named external its own member declares.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExternalBinding {
    /// The external's identity.
    pub external: ExternalId,
    /// Its display name (not unique).
    pub name: String,
    /// The member's own copy of the external's spec, member-relative.
    pub document: String,
    /// The matched operation, `METHOD /positional/{}/template`.
    pub operation: String,
    /// The committed base path it matched under.
    pub base: BasePathEvidence,
}

/// Why a `no-provider-in-workspace` REST row did not bind a named external.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "reason", rename_all = "kebab-case")]
pub enum JoinRefusal {
    /// The calling member declares no external the call's path could reach.
    NoDeclaredExternal,
    /// The path equals an operation of an external the calling member does not
    /// itself declare — another member vendors it.
    ExternalNotDeclaredByMember {
        /// That external.
        external: ExternalId,
        /// Its display name.
        name: String,
        /// The operation the path equals.
        operation: String,
    },
    /// No key in the call's namespace commits a base URL — a literal target
    /// names no namespace at all.
    NoBaseKey,
    /// The base-url key is committed only as an environment indirection.
    BasePathUncommitted {
        /// Where the indirection is committed.
        sources: Vec<BaseSource>,
    },
    /// The committed base paths disagree; every path is named with its file.
    BasePathsDisagree {
        /// Each distinct path, with the file and key committing it.
        paths: Vec<BasePath>,
    },
    /// The joined path is only the tail of an operation — never admitted.
    SuffixOnly {
        /// The operation it is a suffix of.
        operation: String,
        /// The base path it was joined under.
        base: String,
    },
    /// Nothing in the member's externals equals or ends with the path.
    NoMatch,
    /// The path equals two or more distinct `(external, operation)` pairs — two
    /// compositions naming two operations, or two externals the member declares
    /// carrying one operation. Exactly one is required; each is named.
    SeveralMatches {
        /// Every match, in `(external, operation)` order.
        matches: Vec<OperationMatch>,
    },
}

/// One `(external, operation)` a call's path equals — a [`JoinRefusal::SeveralMatches`] line.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct OperationMatch {
    /// The external.
    pub external: ExternalId,
    /// The operation, `METHOD /template`.
    pub operation: String,
}

/// One row's outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum JoinOutcome {
    /// Bound to a named external — a declared, never observed, binding.
    BoundExternal(ExternalBinding),
    /// Refused, with the reason.
    Refused(JoinRefusal),
}

/// One judged row: the coverage row's consumer end, the call's own target, and
/// its outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExternalJoinRow {
    /// The coverage row's `from` — the call site's enclosing declaration.
    pub from: BridgeEndpoint,
    /// The call's stored target (`METHOD /path`, placeholders as written): two
    /// calls in one method share a `from`, and this is what tells their rows
    /// apart.
    pub target: String,
    /// What the join decided.
    #[serde(flatten)]
    pub outcome: JoinOutcome,
}

/// Every judged row filed into exactly one bucket — the denominator
/// [`BoundExternalHeadline::bound_external`] is stated over ([BR-51]).
///
/// [BR-51]: ../../../docs/specs/software-spec.md#327-workspace-federation
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct JoinAccounting {
    /// Bound to a named external.
    pub bound_external: u64,
    /// [`JoinRefusal::NoDeclaredExternal`].
    pub no_declared_external: u64,
    /// [`JoinRefusal::ExternalNotDeclaredByMember`].
    pub external_not_declared_by_member: u64,
    /// [`JoinRefusal::NoBaseKey`].
    pub no_base_key: u64,
    /// [`JoinRefusal::BasePathUncommitted`].
    pub base_path_uncommitted: u64,
    /// [`JoinRefusal::BasePathsDisagree`].
    pub base_paths_disagree: u64,
    /// [`JoinRefusal::SuffixOnly`].
    pub suffix_only: u64,
    /// [`JoinRefusal::NoMatch`].
    pub no_match: u64,
    /// [`JoinRefusal::SeveralMatches`].
    pub several_matches: u64,
}

impl JoinAccounting {
    /// File one outcome. Exhaustive, so a new refusal does not compile until
    /// it is counted.
    fn record(&mut self, outcome: &JoinOutcome) {
        let bucket = match outcome {
            JoinOutcome::BoundExternal(_) => &mut self.bound_external,
            JoinOutcome::Refused(refusal) => match refusal {
                JoinRefusal::NoDeclaredExternal => &mut self.no_declared_external,
                JoinRefusal::ExternalNotDeclaredByMember { .. } => &mut self.external_not_declared_by_member,
                JoinRefusal::NoBaseKey => &mut self.no_base_key,
                JoinRefusal::BasePathUncommitted { .. } => &mut self.base_path_uncommitted,
                JoinRefusal::BasePathsDisagree { .. } => &mut self.base_paths_disagree,
                JoinRefusal::SuffixOnly { .. } => &mut self.suffix_only,
                JoinRefusal::NoMatch => &mut self.no_match,
                JoinRefusal::SeveralMatches { .. } => &mut self.several_matches,
            },
        };
        *bucket += 1;
    }

    /// The refusals, labelled, in declaration order — destructured, so a new
    /// bucket cannot be left out of the summary line.
    fn refusals(self) -> [(&'static str, u64); 8] {
        let Self {
            bound_external: _,
            no_declared_external,
            external_not_declared_by_member,
            no_base_key,
            base_path_uncommitted,
            base_paths_disagree,
            suffix_only,
            no_match,
            several_matches,
        } = self;
        [
            ("no declared external", no_declared_external),
            ("external not declared by the member", external_not_declared_by_member),
            ("no base-url key", no_base_key),
            ("base path uncommitted", base_path_uncommitted),
            ("base paths disagree", base_paths_disagree),
            ("suffix-only", suffix_only),
            ("no match", no_match),
            ("several matches", several_matches),
        ]
    }
}

/// The join's own headline, beside its denominator ([BR-51], [BR-57]).
///
/// [BR-51]: ../../../docs/specs/software-spec.md#327-workspace-federation
/// [BR-57]: ../../../docs/specs/software-spec.md#327-workspace-federation
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoundExternalHeadline {
    /// Rows bound to a named external their own member declares.
    pub bound_external: u64,
    /// The denominator: every invocation-intake `no-provider-in-workspace` REST
    /// row. Equals the sum of [`accounting`](Self::accounting)'s buckets.
    pub no_provider_rows: u64,
    /// Every row, by outcome.
    pub accounting: JoinAccounting,
    /// The headline, its denominator and what it is not, as one line.
    pub summary: String,
}

/// The external join over a workspace: its headline and every judged row
/// ([ADR-68] point 3). Rendered beside `egress_resolution`, never inside it.
///
/// [ADR-68]: ../../../docs/specs/architecture/decisions/ADR-68.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoundExternal {
    /// The headline.
    pub headline: BoundExternalHeadline,
    /// Every judged row, in the coverage rows' endpoint order.
    pub rows: Vec<ExternalJoinRow>,
}

impl BoundExternal {
    /// Every judged row of the call site `from` — one per call it makes, in
    /// target order.
    pub fn rows_of<'a>(&'a self, from: &'a BridgeEndpoint) -> impl Iterator<Item = &'a ExternalJoinRow> {
        self.rows.iter().filter(move |r| &r.from == from)
    }
}

/// What the coverage tier knows about one HTTP reference, handed to the join:
/// its stored target, its committed compositions (`METHOD /path`) and the
/// canonical keys its target named.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JoinCall {
    /// The stored target, as the ledger holds it — the reference's identity
    /// within its enclosing declaration.
    pub target: String,
    /// A configuration-bound call's committed compositions, or a literal
    /// call's stored target.
    pub compositions: Vec<String>,
    /// The canonical configuration keys the target named — empty for a literal.
    pub keys: Vec<String>,
}

/// One vendored copy the join compares a call against: the declaring contract,
/// whose [`operations`](DeclaredContract::operations) are that holder's copy's,
/// and the registered external it names.
struct Copy<'a> {
    contract: &'a DeclaredContract,
    external: &'a NamedExternal,
}

/// An operation key as the wire names it: `METHOD /positional/{}/template`.
fn operation_label((method, template): &OperationKey) -> String {
    format!("{method} {template}")
}

/// The copies `member` itself declares — [`DeclaredContractRelation::externals_declared_by`],
/// S-458's join API — and the copies of every external only **other** members
/// declare, which a call can equal only to be refused.
fn copies_for<'a>(relation: &'a DeclaredContractRelation, member: &'a str) -> (Vec<Copy<'a>>, Vec<Copy<'a>>) {
    let own: Vec<Copy<'a>> =
        relation.externals_declared_by(member).map(|(contract, external)| Copy { contract, external }).collect();
    let own_ids: BTreeSet<&ExternalId> = own.iter().map(|c| &c.external.id).collect();
    let others = relation
        .contracts
        .iter()
        .filter(|c| c.holder != member)
        .filter_map(|contract| match &contract.target {
            ContractTarget::External { external, .. } => relation.external(external).map(|e| Copy { contract, external: e }),
            ContractTarget::Member { .. } => None,
        })
        .filter(|c| !own_ids.contains(&c.external.id))
        .collect();
    (own, others)
}

/// Whether any operation in `copies` has `method` and a template ending with
/// `template` — the only operations any base path could make it equal.
fn reachable(copies: &[Copy<'_>], method: &str, template: &str) -> bool {
    copies.iter().any(|c| c.contract.operations.iter().any(|(m, t)| m == method && t.ends_with(template)))
}

/// The base facts already read, per member, and the reader for the rest — so
/// a member is read once however many of its rows are judged.
struct FactsCache<'r> {
    read: &'r mut dyn FnMut(&str) -> BaseFacts,
    by_member: BTreeMap<String, BaseFacts>,
}

impl FactsCache<'_> {
    fn of(&mut self, member: &str) -> &BaseFacts {
        let read = &mut self.read;
        self.by_member.entry(member.to_string()).or_insert_with(|| read(member))
    }
}

/// Judge one call of `member` against the relation, reading its base facts only
/// when some operation could match.
fn judge(
    member: &str,
    call: &JoinCall,
    relation: &DeclaredContractRelation,
    facts: &mut FactsCache<'_>,
) -> JoinOutcome {
    let (own, others) = copies_for(relation, member);
    // The raw path of each composition: the base is joined BEFORE normalizing,
    // because a positional `{}` does not normalize a second time.
    let paths: BTreeSet<(String, String)> = call
        .compositions
        .iter()
        .filter_map(|c| parse_method_and_template(c).map(|(m, p)| (m, p.to_string())))
        .collect();
    let bare: Vec<(&str, String)> =
        paths.iter().filter_map(|(m, p)| normalize_template(p).map(|t| (m.as_str(), t))).collect();
    let could_match = bare.iter().any(|(m, t)| reachable(&own, m, t) || reachable(&others, m, t));
    if !could_match {
        return JoinOutcome::Refused(if own.is_empty() {
            JoinRefusal::NoDeclaredExternal
        } else {
            JoinRefusal::NoMatch
        });
    }

    let keys: BTreeSet<String> = call.keys.iter().cloned().collect();
    let evidence = match base_reading(&keys, facts.of(member)) {
        Ok(evidence) => evidence,
        Err(refusal) => return JoinOutcome::Refused(refusal),
    };
    let base = evidence.path.clone();

    // Keyed by external AND operation: two externals carrying one operation are
    // two matches, never a first-wins binding.
    let mut bound: BTreeMap<OperationMatch, &Copy<'_>> = BTreeMap::new();
    let mut not_declared: Option<(&Copy<'_>, String)> = None;
    let mut suffix: Option<String> = None;
    for (method, path) in &paths {
        let Some(joined) = normalize_template(&join_base(&base, path)) else {
            continue;
        };
        let key = (method.clone(), joined);
        for copy in own.iter().filter(|c| c.contract.operations.contains(&key)) {
            let at = OperationMatch { external: copy.external.id.clone(), operation: operation_label(&key) };
            bound.entry(at).or_insert(copy);
        }
        if not_declared.is_none() {
            not_declared =
                others.iter().find(|c| c.contract.operations.contains(&key)).map(|c| (c, operation_label(&key)));
        }
        if suffix.is_none() {
            suffix = own.iter().find_map(|c| {
                c.contract
                    .operations
                    .iter()
                    .find(|(m, t)| *m == key.0 && t.len() > key.1.len() && t.ends_with(&key.1))
                    .map(operation_label)
            });
        }
    }

    let refusal = match bound.len() {
        1 => {
            let (OperationMatch { operation, .. }, copy) = bound.into_iter().next().expect("one match");
            return JoinOutcome::BoundExternal(ExternalBinding {
                external: copy.external.id.clone(),
                name: copy.external.name.clone(),
                document: copy.contract.document.clone(),
                operation,
                base: evidence,
            });
        }
        0 => match (not_declared, suffix) {
            (Some((copy, operation)), _) => JoinRefusal::ExternalNotDeclaredByMember {
                external: copy.external.id.clone(),
                name: copy.external.name.clone(),
                operation,
            },
            _ if own.is_empty() => JoinRefusal::NoDeclaredExternal,
            (None, Some(operation)) => JoinRefusal::SuffixOnly { operation, base },
            (None, None) => JoinRefusal::NoMatch,
        },
        _ => JoinRefusal::SeveralMatches { matches: bound.into_keys().collect() },
    };
    JoinOutcome::Refused(refusal)
}

/// The one-line summary: the figure, its denominator, the non-zero refusals,
/// and what it is not.
fn summarize(accounting: JoinAccounting, rows: u64) -> String {
    let refused: Vec<String> = accounting
        .refusals()
        .into_iter()
        .filter(|(_, n)| *n > 0)
        .map(|(label, n)| format!("{n} {label}"))
        .collect();
    let refused = if refused.is_empty() { String::new() } else { format!(" (refused: {})", refused.join(", ")) };
    let noun = if rows == 1 { "row" } else { "rows" };
    format!(
        "{} of {rows} invocation no-provider-in-workspace REST {noun} bound to a named external their own \
         member declares{refused}; declared by vendored specs, never a cross-service edge, and outside \
         egress_resolution",
        accounting.bound_external
    )
}

/// Join every invocation `no-provider-in-workspace` REST reference against the
/// externals its own member declares ([ADR-68] point 3).
///
/// `population` is those references, one entry per coverage row — its `from`
/// and the [`JoinCall`] the coverage tier resolved it by; `facts` reads a
/// member's [`BaseFacts`] and is called at most
/// once per member, and only for a member some row of which could match an
/// operation. `None` when no member declares a named external — the join has
/// nothing to bind to, and the payload stays as it was.
///
/// [ADR-68]: ../../../docs/specs/architecture/decisions/ADR-68.md
pub fn derive(
    population: &[(BridgeEndpoint, JoinCall)],
    relation: &DeclaredContractRelation,
    mut facts: impl FnMut(&str) -> BaseFacts,
) -> Option<BoundExternal> {
    let declares_an_external =
        relation.contracts.iter().any(|c| matches!(c.target, ContractTarget::External { .. }));
    if !declares_an_external {
        return None;
    }
    let mut cache = FactsCache { read: &mut facts, by_member: BTreeMap::new() };
    let mut accounting = JoinAccounting::default();
    let mut judged: Vec<ExternalJoinRow> = population
        .iter()
        .map(|(from, call)| {
            let outcome = judge(&from.member, call, relation, &mut cache);
            accounting.record(&outcome);
            ExternalJoinRow { from: from.clone(), target: call.target.clone(), outcome }
        })
        .collect();
    // The coverage rows' own order (by endpoint), then the call's target.
    judged.sort_by(|a, b| (&a.from, &a.target).cmp(&(&b.from, &b.target)));
    let no_provider_rows = judged.len() as u64;
    Some(BoundExternal {
        headline: BoundExternalHeadline {
            bound_external: accounting.bound_external,
            no_provider_rows,
            accounting,
            summary: summarize(accounting, no_provider_rows),
        },
        rows: judged,
    })
}

#[cfg(test)]
mod tests;
