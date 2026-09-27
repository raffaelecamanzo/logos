//! **S-456's blocking measurement gate** — do the reference estate's *vendored*
//! spec documents carry enough evidence to justify building any of [CR-147]'s
//! three gated halves?
//!
//! Three independent floors, declared in [`vendored_spec_contracts_floor.txt`]
//! **before this module existed** and parsed out of it by
//! [`the_floors_are_the_ones_declared_before_the_run`](tests::the_floors_are_the_ones_declared_before_the_run).
//! The constants are [`DECLARED_CONTRACTS_FLOOR`], [`INVOCATION_EXTERNAL_FLOOR`]
//! and [`OWN_SPEC_TIES_FLOOR`]; no figure is restated in prose here, because a
//! number in a module header is guarded by nothing.
//!
//!   - **Declared contracts** — ordered `(holder, contract)` pairs from a spec
//!     document the holder does not implement, to the member whose own spec it is
//!     by document identity or to a named external grouping its copies. Gates
//!     [S-458].
//!   - **Invocation → external** — `no-provider-in-workspace` REST rows whose
//!     composed path equals an operation of an external the same member vendors,
//!     exactly, under a committed base path. Gates [S-459].
//!   - **Own-spec ties** — contract-surface ambiguous rows whose holder is tied
//!     and holds the spec under its own `src/main/resources`. Gates [S-460].
//!
//! Each half prints its own verdict; one clearing does not carry another. Below a
//! floor the half is recorded FALSIFIED in [`vendored_spec_contracts_finding.txt`]
//! and [ADR-68]'s matching decision point is superseded in place.
//!
//! # Every row is the product's
//!
//! This gate classifies nothing the shipped federation already classifies. Its
//! rows are [`cross_service_coverage`]'s, unchanged — bucket, reason, intake, the
//! tied candidates and the configuration evidence. Its documents and operations
//! are [`MemberContracts::contract_surface`], the read the bridge itself matches
//! on; its call-site templates are [`MemberContracts::invocation_refs`], the one
//! ledger seam the bridge and the coverage tier share. The committed
//! configuration is the shipped [`ConfigCorpus`], and the deploy overlays are
//! [S-411]'s walk ([`walk_overlays`]) and its `runnable` rule
//! ([`runnable_members`]), both called rather than copied.
//!
//! What the product does **not** publish on a row, the gate derives from those
//! reads and names where it does so:
//!
//!   - whether a holder provides an operation — read off the coverage tier's own
//!     verdict: an operation with **no** row is the tier's `None`, a sole
//!     same-member provider ([`OpProvision::Sole`]);
//!   - a spec document's path — the file descriptor of its operations' SCIP
//!     symbol ([`document_path`]);
//!   - an operation's key — [`operation_key`], a three-line mirror of the crate-
//!     private `route_key` over the public [`normalize_template`]. It is the one
//!     hand-mirrored rule here, and it is fixture-pinned to the same key shape;
//!   - an invocation row's composed path — the ledger target with each
//!     placeholder replaced by the row's own evidence value ([`compose`]). Every
//!     placeholder KEY is read by the shipped [`placeholder_keys`], the function
//!     the bridge derives the row's evidence keys through; only the substitution
//!     is local, because the bridge's own composition (`identify`) is private. A
//!     row whose evidence diverges is left uncomposed and enumerated, never
//!     guessed.
//!
//! # The one framework rule, and the canonicalisation it inherits
//!
//! [`overlay_overrides`] is Spring's environment-variable relaxed binding — the
//! rule that makes `PECSERVER_BASEURL` in a Helm values file override
//! `pec-server.base-url` in `application.yml`. It is language judgement of exactly
//! the kind the parent module's carve-out names, and it must not be lifted into
//! `logos-core/src`.
//!
//! The overlay keys reach it **already canonicalised** by the shipped
//! `parse_yaml` (`_` and `-` dropped per segment), so the environment variable's
//! underscore positions are gone before this module sees them. The comparison is
//! therefore made on the dot-free canonical form: `pecserverbaseurl` against
//! `pecserver.baseurl` with its dots removed. That admits an underscore placed
//! anywhere (`PECSERVERBASE_URL` too) — a known, bounded over-admission, confined
//! to one member's own base-url namespace, and every overriding value is printed
//! with its file so an adjudicator can see it.
//!
//! [ADR-68]: ../../../docs/specs/architecture/decisions/ADR-68.md
//! [CR-147]: ../../../docs/requests/CR-147-vendored-specs-declare-contracts-and-name-externals.md
//! [S-411]: ../../../docs/planning/journal.md#s-411-measure-config-declared-coupling-over-the-reference-estate
//! [S-458]: ../../../docs/planning/journal.md#s-458-a-vendored-spec-is-a-declared-contract-to-a-member-or-a-named-external
//! [S-459]: ../../../docs/planning/journal.md#s-459-a-no-provider-call-binds-to-the-external-its-member-vendors-under-a-committed-base-path
//! [S-460]: ../../../docs/planning/journal.md#s-460-a-spec-co-located-with-its-implementer-resolves-its-own-tie
//! [`vendored_spec_contracts_floor.txt`]: ./vendored_spec_contracts_floor.txt
//! [`vendored_spec_contracts_finding.txt`]: ./vendored_spec_contracts_finding.txt
//! [`walk_overlays`]: super::config_declared_coupling::walk_overlays
//! [`runnable_members`]: super::config_declared_coupling::runnable_members

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use logos_core::extract::config::corpus::canonical_key;
use logos_core::federation::{
    cross_service_coverage, discover, BridgeIntake, CoverageState, EngineRegistry,
    MemberContracts, ProviderDisposition, ReferenceCoverage, RegistryMode, UnboundReason,
};
use logos_core::model::{ArtifactRelation, BridgeNamespace, BridgeRole, NodeKind};
use logos_core::resolve::binding::{placeholder_keys, Provenance};
use logos_core::resolve::route_template::normalize_template;
use logos_core::Engine;

use super::config_declared_coupling::{
    members_with_source, runnable_members, walk_overlays, Scalar, SourceSet,
};
use super::configuration_agreement::ConfigCorpus;
use super::identity;

/// **The floors, as declared before the run.** Embedded, following
/// [`super::config_declared_coupling::DECLARED_FLOOR`]: a file the build embeds
/// cannot be deleted or renamed without breaking compilation.
pub const DECLARED_FLOOR: &str = include_str!("vendored_spec_contracts_floor.txt");

/// The documentation / mock declaration the subtraction reads — never a heuristic.
pub const DECLARED_KINDS: &str = include_str!("vendored_spec_contracts_declared_kinds.txt");

/// The recorded verdict, reproduced by the estate run and asserted against it.
pub const RECORDED_FINDING: &str = include_str!("vendored_spec_contracts_finding.txt");

/// Floor for the first half, parsed out of [`DECLARED_FLOOR`] by the always-run
/// test so the two cannot drift.
pub const DECLARED_CONTRACTS_FLOOR: usize = 3;

/// Floor for the second half.
pub const INVOCATION_EXTERNAL_FLOOR: usize = 15;

/// Floor for the third half.
pub const OWN_SPEC_TIES_FLOOR: usize = 45;

/// The three half names, verbatim from CR-147 §3.2 B1's first column — the
/// strings the floor lines, the report and the finding all carry.
pub const HALF_DECLARED: &str = "Declared contracts";
pub const HALF_INVOCATION: &str = "Invocation → external";
pub const HALF_OWN_SPEC: &str = "Own-spec ties";

/// The springdoc default `info.title`, which names no external. Every internal
/// springdoc spec on the estate carries it (CR-147 §2.1).
pub const SPRINGDOC_DEFAULT_TITLE: &str = "OpenAPI definition";

/// The Maven/Gradle main-resources directory — a directory convention the third
/// half's metric names in terms, not a framework table.
pub const MAIN_RESOURCES: &str = "src/main/resources/";

// ── Recorded figures ───────────────────────────────────────────────────────
//
// Pinned so a drift in the product's rows, the identity rule, the overlay walk or
// the composition is a failure rather than a quietly different number. S-411
// learned why a floor alone is not enough: a `>=` cannot fail upward, and a split
// an adjudicator leans on must not be freely corruptible under a green gate.

/// Declared-contract pairs measured.
pub const RECORDED_DECLARED_PAIRS: usize = 6;
/// `(document identity, named external)` split of those pairs.
pub const RECORDED_DECLARED_SPLIT: (usize, usize) = (1, 5);
/// Invocation rows exact under a committed base path.
pub const RECORDED_INVOCATION_EXACT: usize = 20;
/// The same count under application configuration alone — the counterfactual.
pub const RECORDED_INVOCATION_EXACT_APP_ONLY: usize = 1;
/// Invocation rows in the population (`no-provider-in-workspace`, `route`).
pub const RECORDED_INVOCATION_POPULATION: usize = 42;
/// Invocation `no-provider-in-workspace` rows of other relations (two
/// `broker-topic` rows): 42 + 2 = the intake's 44 in CR-147 §2.1.
pub const RECORDED_NON_REST_NO_PROVIDER: usize = 2;
/// Own-spec ties measured.
pub const RECORDED_OWN_SPEC_TIES: usize = 43;
/// Contract-surface ambiguous rows — the third half's population.
pub const RECORDED_TIE_POPULATION: usize = 140;
/// Candidates under the narrower `invocations` reading — the figure the finding
/// hands S-457 (`pecserver-mock`, `notification-gateway-mock`).
pub const RECORDED_CANDIDATES_STRICT: usize = 2;
/// Counted half-1 pairs whose holder the narrower reading would call
/// non-runnable — the sensitivity the finding cites as zero.
pub const RECORDED_DECLARED_SENSITIVITY: usize = 0;

// ── Operation keys and documents ───────────────────────────────────────────

/// `(upper-cased METHOD, positional template)`.
pub type OpKey = (String, String);

/// The operation key of a contract node named `"METHOD /template"`, or `None`
/// when the name is malformed or its template does not normalize.
///
/// A mirror of the crate-private `resolve::route_template::route_key`, over the
/// public [`normalize_template`] — so the positional judgement itself is the
/// product's, and only the three-line split is repeated.
pub fn operation_key(name: &str) -> Option<OpKey> {
    let (method, template) = name.split_once(' ')?;
    if method.is_empty() || !template.starts_with('/') {
        return None;
    }
    Some((method.to_ascii_uppercase(), normalize_template(template)?))
}

/// The member-relative path of the file a SCIP symbol is declared in — its
/// descriptors up to the last `/` outside a backtick-quoted name, unquoted.
///
/// `logos . . . api/src/main/resources/openapi/`v1.yaml`/v1-users#get#` →
/// `api/src/main/resources/openapi/v1.yaml`. `None` for a symbol with no
/// descriptor path at all.
pub fn document_path(symbol: &str) -> Option<String> {
    let descriptors = symbol.splitn(5, ' ').nth(4)?;
    let mut quoted = false;
    let mut last_slash = None;
    for (i, c) in descriptors.char_indices() {
        match c {
            '`' => quoted = !quoted,
            '/' if !quoted => last_slash = Some(i),
            _ => {}
        }
    }
    let path: String = descriptors[..last_slash?].chars().filter(|c| *c != '`').collect();
    (!path.is_empty()).then_some(path)
}

/// Whether the holder provides one operation, as the coverage tier decided it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpProvision {
    /// No coverage row: the tier's `None`, a sole same-member provider.
    Sole,
    /// An ambiguous row, with the tied members as the row lists them.
    Tied { members: Vec<String>, truncated: bool },
    /// Any other row — bound elsewhere, no provider, not composed.
    Other,
}

/// Whether the tier's verdict names `holder` as a provider. `None` when the tied
/// list was truncated and does not show the holder — undecidable, never guessed.
pub fn holder_provides(holder: &str, provision: &OpProvision) -> Option<bool> {
    match provision {
        OpProvision::Sole => Some(true),
        OpProvision::Tied { members, truncated } => {
            if members.iter().any(|m| m == holder) {
                Some(true)
            } else if *truncated {
                None
            } else {
                Some(false)
            }
        }
        OpProvision::Other => Some(false),
    }
}

/// One spec document: a file of one member holding at least one operation.
#[derive(Debug, Clone, Default)]
pub struct Document {
    pub member: String,
    pub path: String,
    /// The keys of its keyed operations.
    pub keys: BTreeSet<OpKey>,
    /// Keyed operations (a key held twice counts twice here).
    pub keyed_ops: usize,
    /// Keyed operations the holder provides.
    pub provided: usize,
    /// Keyed operations whose provision is undecidable (truncated tie list).
    pub undecided: usize,
    /// Operations with no key.
    pub unkeyed: usize,
    /// `info.title`, when one was read.
    pub title: Option<String>,
}

impl Document {
    pub fn implementation(&self) -> Implementation {
        implementation(self.provided, self.keyed_ops, self.undecided)
    }
}

/// How far a holder implements a document it holds — the floor's "implements".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Implementation {
    /// The holder provides >= 90 % of its keyed operations: an own spec.
    Own,
    /// The holder provides none: a vendored document.
    Vendored,
    /// In between.
    Partial,
    /// No keyed operation, or an undecidable provision: counts toward nothing.
    Unjudged,
}

pub fn implementation(provided: usize, keyed: usize, undecided: usize) -> Implementation {
    if keyed == 0 || undecided > 0 {
        return Implementation::Unjudged;
    }
    if provided * 10 >= keyed * 9 {
        Implementation::Own
    } else if provided == 0 {
        Implementation::Vendored
    } else {
        Implementation::Partial
    }
}

/// Group a member's operations into documents, reading each operation's
/// provision off the coverage tier.
///
/// `operations` is `(member, symbol, name)` for every `ApiOperation` node;
/// `provisions` is keyed on `(member, symbol)` and holds a row's verdict — an
/// operation absent from it had no coverage row.
pub fn build_documents(
    operations: &[(String, String, String)],
    provisions: &BTreeMap<(String, String), OpProvision>,
) -> Vec<Document> {
    let mut by_doc: BTreeMap<(String, String), Document> = BTreeMap::new();
    for (member, symbol, name) in operations {
        let Some(path) = document_path(symbol) else { continue };
        let doc = by_doc.entry((member.clone(), path.clone())).or_insert_with(|| Document {
            member: member.clone(),
            path,
            ..Document::default()
        });
        let Some(key) = operation_key(name) else {
            doc.unkeyed += 1;
            continue;
        };
        doc.keyed_ops += 1;
        doc.keys.insert(key);
        let provision = provisions
            .get(&(member.clone(), symbol.clone()))
            .cloned()
            .unwrap_or(OpProvision::Sole);
        match holder_provides(member, &provision) {
            Some(true) => doc.provided += 1,
            Some(false) => {}
            None => doc.undecided += 1,
        }
    }
    by_doc.into_values().collect()
}

/// `(shared, |d|)` — how many of `d`'s keys `other` holds.
pub fn score(d: &BTreeSet<OpKey>, other: &BTreeSet<OpKey>) -> (usize, usize) {
    (d.intersection(other).count(), d.len())
}

/// The 90 % identity threshold, in integers.
pub fn meets_identity((shared, total): (usize, usize)) -> bool {
    total > 0 && shared * 10 >= total * 9
}

/// The outcome of matching a vendored document against every member's own specs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Identity {
    /// One member's own spec scores highest, at >= 90 %.
    Member { member: String, path: String, shared: usize, total: usize },
    /// Two or more members at the same best score: resolves to neither.
    Collision { members: Vec<String>, shared: usize, total: usize },
    /// No own spec reaches the threshold.
    NoMember,
}

/// Resolve document identity for `d` against the own specs of every other member
/// not declared documentation or mock.
pub fn resolve_identity(
    d: &Document,
    own_specs: &[&Document],
    excluded: &BTreeSet<String>,
) -> Identity {
    // Best own spec per member.
    let mut best: BTreeMap<&str, (usize, &str)> = BTreeMap::new();
    for spec in own_specs {
        if spec.member == d.member || excluded.contains(&spec.member) {
            continue;
        }
        let s = score(&d.keys, &spec.keys);
        if !meets_identity(s) {
            continue;
        }
        let entry = best.entry(spec.member.as_str()).or_insert((s.0, spec.path.as_str()));
        if s.0 > entry.0 {
            *entry = (s.0, spec.path.as_str());
        }
    }
    let Some(top) = best.values().map(|(shared, _)| *shared).max() else {
        return Identity::NoMember;
    };
    let winners: Vec<(&str, &str)> = best
        .iter()
        .filter(|(_, (shared, _))| *shared == top)
        .map(|(m, (_, p))| (*m, *p))
        .collect();
    match winners.as_slice() {
        [(member, path)] => Identity::Member {
            member: (*member).to_string(),
            path: (*path).to_string(),
            shared: top,
            total: d.keys.len(),
        },
        _ => Identity::Collision {
            members: winners.iter().map(|(m, _)| (*m).to_string()).collect(),
            shared: top,
            total: d.keys.len(),
        },
    }
}

/// Whether `a` is a copy contained in `b`: >= 90 % of `a`'s keys are in `b`.
pub fn contained(a: &BTreeSet<OpKey>, b: &BTreeSet<OpKey>) -> bool {
    meets_identity(score(a, b))
}

/// Connected components of the copy graph over `docs`: an edge joins two
/// documents when either is contained in the other. Returns each document's
/// component, as the smallest index in it.
pub fn external_components(docs: &[&BTreeSet<OpKey>]) -> Vec<usize> {
    fn find(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    let mut parent: Vec<usize> = (0..docs.len()).collect();
    for i in 0..docs.len() {
        for j in (i + 1)..docs.len() {
            if contained(docs[i], docs[j]) || contained(docs[j], docs[i]) {
                let (a, b) = (find(&mut parent, i), find(&mut parent, j));
                parent[a.max(b)] = a.min(b);
            }
        }
    }
    (0..docs.len()).map(|i| find(&mut parent, i)).collect()
}

/// The display name of an external: the most common non-default `info.title`
/// across its copies, else the stem of the first copy in path order.
pub fn external_name(copies: &[(&str, Option<&str>)]) -> String {
    let mut titles: BTreeMap<&str, usize> = BTreeMap::new();
    for (_, title) in copies {
        if let Some(t) = title.filter(|t| !t.is_empty() && *t != SPRINGDOC_DEFAULT_TITLE) {
            *titles.entry(t).or_default() += 1;
        }
    }
    if let Some(top) = titles.values().max().copied() {
        if let Some((t, _)) = titles.iter().find(|(_, n)| **n == top) {
            return (*t).to_string();
        }
    }
    let mut paths: Vec<&str> = copies.iter().map(|(p, _)| *p).collect();
    paths.sort_unstable();
    let first = paths.first().copied().unwrap_or("");
    let file = first.rsplit('/').next().unwrap_or(first);
    file.rsplit_once('.').map_or(file, |(stem, _)| stem).to_string()
}

/// The `info.title` of a YAML or JSON spec, read textually: the first `title`
/// key after the first `info` key. A label, never an identity.
pub fn spec_title(text: &str) -> Option<String> {
    let after_info = text
        .find("\ninfo:")
        .map(|i| i + 6)
        .or_else(|| text.starts_with("info:").then_some(5))
        .or_else(|| text.find("\"info\"").map(|i| i + 6))?;
    let rest = &text[after_info..];
    let at = rest.find("title")?;
    let tail = &rest[at + "title".len()..];
    let tail = tail.trim_start_matches('"').trim_start();
    let value = tail.strip_prefix(':')?.trim_start();
    let title = match value.chars().next() {
        Some(q @ ('"' | '\'')) => {
            let inner = &value[1..];
            &inner[..inner.find(q)?]
        }
        _ => value.lines().next().unwrap_or("").trim(),
    };
    let title = title.trim();
    (!title.is_empty()).then(|| title.to_string())
}

// ── Declarations ───────────────────────────────────────────────────────────

/// A declared member kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Documentation,
    Mock,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Documentation => "documentation",
            Self::Mock => "mock",
        }
    }
}

/// Parse the committed declaration list. A malformed line or an unknown kind is
/// an error, never a skip.
pub fn parse_kinds(text: &str) -> Result<BTreeMap<String, Kind>, String> {
    let mut out = BTreeMap::new();
    for (n, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split_whitespace().collect();
        let [member, kind] = fields.as_slice() else {
            return Err(format!("line {}: expected `<member> <kind>`, got `{raw}`", n + 1));
        };
        let kind = match *kind {
            "documentation" => Kind::Documentation,
            "mock" => Kind::Mock,
            other => return Err(format!("line {}: unknown kind `{other}`", n + 1)),
        };
        if out.insert((*member).to_string(), kind).is_some() {
            return Err(format!("line {}: `{member}` declared twice", n + 1));
        }
    }
    Ok(out)
}

/// The floor the declaration states for `half`, on exactly ONE `>= NN <half>`
/// line — S-411's rule: a declaration that states its own floor twice has none.
pub fn declared_floor(text: &str, half: &str) -> Result<usize, String> {
    let hits: Vec<usize> = text
        .lines()
        .filter(|l| l.contains(half))
        .filter_map(|l| {
            let rest = l.trim().strip_prefix(">= ")?;
            let (n, tail) = rest.split_once(' ')?;
            tail.starts_with(half).then(|| n.parse().ok())?
        })
        .collect();
    match hits.as_slice() {
        [one] => Ok(*one),
        _ => Err(format!(
            "the declaration must state `>= NN {half}` on exactly one line; found {hits:?}"
        )),
    }
}

// ── Half 1: declared contracts ─────────────────────────────────────────────

/// What a vendored document declares a contract to.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Contract {
    /// Document identity to a member's own spec.
    Member(String),
    /// A named external — the component index of the copy graph.
    External(usize),
}

/// Why a vendored document's pair does not count, when it does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Holder {
    Eligible,
    Declared(Kind),
    NotRunnable,
}

/// One vendored document and what it declares.
#[derive(Debug, Clone)]
pub struct DeclaredRow {
    pub holder: String,
    pub path: String,
    pub contract: Contract,
    pub identity: Identity,
    pub standing: Holder,
}

/// Half 1's judgement.
#[derive(Debug, Default)]
pub struct DeclaredHalf {
    /// Every vendored document, classified.
    pub rows: Vec<DeclaredRow>,
    /// Every external component's display name and copies `(member, path)`.
    pub externals: BTreeMap<usize, (String, Vec<(String, String)>)>,
    /// Partially implemented documents `(member, path, provided, keyed)`.
    pub partial: Vec<(String, String, usize, usize)>,
    /// Documents with no keyed operation or an undecidable provision.
    pub unjudged: Vec<(String, String)>,
}

impl DeclaredHalf {
    /// Distinct eligible `(holder, contract)` pairs — the metric.
    pub fn pairs(&self) -> BTreeSet<(String, Contract)> {
        self.pairs_of(Holder::Eligible)
    }

    pub fn pairs_of(&self, standing: Holder) -> BTreeSet<(String, Contract)> {
        self.rows
            .iter()
            .filter(|r| r.standing == standing)
            .map(|r| (r.holder.clone(), r.contract.clone()))
            .collect()
    }

    /// Every vendored document whose identity tied between members — counted or
    /// not — so the floor's "identity collisions, enumerated" split is printed
    /// rather than inferred from absent lines.
    pub fn collisions(&self) -> Vec<&DeclaredRow> {
        self.rows.iter().filter(|r| matches!(r.identity, Identity::Collision { .. })).collect()
    }

    /// `(document identity, named external)` among the counted pairs.
    pub fn split(&self) -> (usize, usize) {
        let pairs = self.pairs();
        let identity = pairs.iter().filter(|(_, c)| matches!(c, Contract::Member(_))).count();
        (identity, pairs.len() - identity)
    }

    pub fn contract_label(&self, c: &Contract) -> String {
        match c {
            Contract::Member(m) => format!("member {m}"),
            Contract::External(i) => {
                format!("external {}", self.externals.get(i).map_or("?", |(n, _)| n.as_str()))
            }
        }
    }
}

/// Judge half 1 over the workspace's documents.
pub fn judge_declared(
    documents: &[Document],
    runnable: &BTreeSet<String>,
    kinds: &BTreeMap<String, Kind>,
) -> DeclaredHalf {
    let mut out = DeclaredHalf::default();
    let excluded: BTreeSet<String> = kinds.keys().cloned().collect();
    let own: Vec<&Document> =
        documents.iter().filter(|d| d.implementation() == Implementation::Own).collect();
    // The copy graph: every document that is not an own spec — plus EVERY copy a
    // declared documentation or mock member holds, served or not. A mock whose
    // routes serve its spec is still a stand-in for the external it mocks, and
    // the floor puts its copy in that external's group (ADR-68 point 5).
    let pool: Vec<&Document> = documents
        .iter()
        .filter(|d| d.implementation() != Implementation::Own || kinds.contains_key(&d.member))
        .collect();
    let keysets: Vec<&BTreeSet<OpKey>> = pool.iter().map(|d| &d.keys).collect();
    let component = external_components(&keysets);
    let mut copies: BTreeMap<usize, Vec<&Document>> = BTreeMap::new();
    for (i, d) in pool.iter().enumerate() {
        copies.entry(component[i]).or_default().push(d);
    }
    for (c, docs) in &copies {
        let named: Vec<(&str, Option<&str>)> =
            docs.iter().map(|d| (d.path.as_str(), d.title.as_deref())).collect();
        out.externals.insert(
            *c,
            (
                external_name(&named),
                docs.iter().map(|d| (d.member.clone(), d.path.clone())).collect(),
            ),
        );
    }
    for (i, d) in pool.iter().enumerate() {
        match d.implementation() {
            Implementation::Partial => {
                out.partial.push((d.member.clone(), d.path.clone(), d.provided, d.keyed_ops));
                continue;
            }
            Implementation::Unjudged => {
                out.unjudged.push((d.member.clone(), d.path.clone()));
                continue;
            }
            // Only a declared holder's served copy is in the pool as an own spec:
            // it joins the copy graph and declares nothing.
            Implementation::Own => continue,
            Implementation::Vendored => {}
        }
        let identity = resolve_identity(d, &own, &excluded);
        let contract = match &identity {
            Identity::Member { member, .. } => Contract::Member(member.clone()),
            _ => Contract::External(component[i]),
        };
        let standing = match kinds.get(&d.member) {
            Some(k) => Holder::Declared(*k),
            None if !runnable.contains(&d.member) => Holder::NotRunnable,
            None => Holder::Eligible,
        };
        out.rows.push(DeclaredRow {
            holder: d.member.clone(),
            path: d.path.clone(),
            contract,
            identity,
            standing,
        });
    }
    out
}

// ── Half 2: invocation → external ──────────────────────────────────────────

/// A template's canonical placeholder keys, as the product reads them — the
/// shipped [`placeholder_keys`], the same function the bridge derives a row's
/// evidence keys through, so the ledger join cannot compare two key rules.
fn keys_of(template: &str) -> BTreeSet<String> {
    placeholder_keys(template).unwrap_or_default().into_iter().collect()
}

/// Why an invocation row was not composed.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum NotComposed {
    /// No ledger target on the row's call site matched its evidence keys.
    NoLedgerTarget,
    /// Two or more distinct ledger targets matched.
    LedgerJoinNotUnique,
    /// A placeholder's evidence carries several values (profiles disagree).
    DivergentEvidence(String),
    /// A placeholder the row's evidence does not name.
    MissingEvidence(String),
    /// The target is not `METHOD /path`.
    Malformed,
}

/// Compose a ledger target with the row's evidence: `(METHOD, path)`.
///
/// `evidence` is keyed on the canonical configuration key.
pub fn compose(
    target: &str,
    evidence: &BTreeMap<String, BTreeSet<String>>,
) -> Result<(String, String), NotComposed> {
    let (method, template) = target.split_once(' ').ok_or(NotComposed::Malformed)?;
    // Only the span boundaries are found here; each span's KEY is the product's
    // reading of that span alone, so a nested or empty span — which the product
    // refuses — refuses here too rather than inventing a key.
    let mut path = String::new();
    let mut rest = template;
    while let Some(open) = rest.find("${") {
        let close = open + rest[open..].find('}').ok_or(NotComposed::Malformed)?;
        let span = &rest[open..=close];
        let [key] = &placeholder_keys(span).unwrap_or_default()[..] else {
            return Err(NotComposed::Malformed);
        };
        let values = evidence.get(key).ok_or_else(|| NotComposed::MissingEvidence(key.clone()))?;
        let [value] = values.iter().collect::<Vec<_>>()[..] else {
            return Err(NotComposed::DivergentEvidence(key.clone()));
        };
        path.push_str(&rest[..open]);
        path.push_str(value);
        rest = &rest[close + 1..];
    }
    path.push_str(rest);
    if method.is_empty() || !path.starts_with('/') {
        return Err(NotComposed::Malformed);
    }
    Ok((method.to_ascii_uppercase(), path))
}

/// Pick the ledger target an invocation row belongs to: the call site's targets
/// whose placeholder keys are exactly the row's evidence keys.
pub fn join_ledger<'a>(
    targets: &'a [String],
    evidence_keys: &BTreeSet<String>,
) -> Result<&'a str, NotComposed> {
    let matching: BTreeSet<&str> = targets
        .iter()
        .filter(|t| keys_of(t) == *evidence_keys)
        .map(String::as_str)
        .collect();
    match matching.into_iter().collect::<Vec<_>>()[..] {
        [one] => Ok(one),
        [] => Err(NotComposed::NoLedgerTarget),
        _ => Err(NotComposed::LedgerJoinNotUnique),
    }
}

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
/// Spring's relaxed binding — see the module docs for the dot-free comparison
/// the shipped parser's canonicalisation forces.
pub fn overlay_overrides(overlay_key: &str, base_key: &str) -> bool {
    if overlay_key == base_key || overlay_key.ends_with(&format!(".{base_key}")) {
        return true;
    }
    let last = overlay_key.rsplit('.').next().unwrap_or(overlay_key);
    !last.is_empty() && last == base_key.replace('.', "")
}

/// A call's committed base path, under one reading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BaseReading {
    /// No application key in the call's namespace commits a URL.
    NoKey,
    /// Exactly one path; the files committing it; whether an overlay supplied it.
    One { path: String, files: Vec<String>, overlay: bool },
    /// Two or more distinct paths, each with the file committing it.
    Disagree(Vec<(String, String)>),
    /// Overlays override the base-url key, but none commits a URL path (a
    /// placeholder, a templated authority): no path is proven, and the
    /// application value they override is not read in their place.
    OverlayWithoutPath(Vec<String>),
}

/// Read a call's committed base path.
///
/// `call_keys` are the canonical placeholder keys of the row's evidence; `app`
/// and `overlays` are the member's committed values. With `admit_overlays` off,
/// the overlays are ignored — the application-config-only counterfactual.
pub fn base_reading(
    call_keys: &BTreeSet<String>,
    app: &[&Scalar],
    overlays: &[&Scalar],
    admit_overlays: bool,
) -> BaseReading {
    let parents: BTreeSet<&str> = call_keys.iter().map(|k| parent(k)).collect();
    let base_values: Vec<&Scalar> = app
        .iter()
        .copied()
        .filter(|s| parents.contains(parent(&s.key)) && url_path(&s.value).is_some())
        .collect();
    let base_keys: BTreeSet<&str> = base_values.iter().map(|s| s.key.as_str()).collect();
    if base_keys.is_empty() {
        return BaseReading::NoKey;
    }
    // An override is decided by the KEY, as the floor states it: an overlay
    // value that is no URL still replaces the application value at deploy time,
    // so it must not let that value back in. It contributes no path below.
    let overriding: Vec<&Scalar> = if admit_overlays {
        overlays
            .iter()
            .copied()
            .filter(|s| base_keys.iter().any(|k| overlay_overrides(&s.key, k)))
            .collect()
    } else {
        Vec::new()
    };
    let (chosen, overlay) =
        if overriding.is_empty() { (base_values, false) } else { (overriding, true) };
    let mut by_path: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for s in &chosen {
        if let Some(p) = url_path(&s.value) {
            by_path.entry(p).or_default().insert(s.file.clone());
        }
    }
    if by_path.is_empty() {
        let files: BTreeSet<String> = chosen.iter().map(|s| s.file.clone()).collect();
        return BaseReading::OverlayWithoutPath(files.into_iter().collect());
    }
    if by_path.len() == 1 {
        let (path, files) = by_path.into_iter().next().expect("one path");
        BaseReading::One { path, files: files.into_iter().collect(), overlay }
    } else {
        BaseReading::Disagree(
            by_path
                .into_iter()
                .flat_map(|(p, files)| files.into_iter().map(move |f| (p.clone(), f)))
                .collect(),
        )
    }
}

/// Where one invocation row lands — exactly one, in this precedence order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallClass {
    /// The member holds no named external.
    NoExternal,
    /// The row's path could not be composed.
    NotComposed(NotComposed),
    /// No committed base-url key; whether `P` alone equals an operation.
    NoBaseKey { p_exact: bool },
    /// The committed base paths disagree.
    Disagree(Vec<(String, String)>),
    /// Overlays override the base-url key without committing a URL path.
    OverlayWithoutPath(Vec<String>),
    /// Exact under a committed base path — the counted rows.
    Exact { base: String, files: Vec<String>, overlay: bool, document: String },
    /// `P` alone equals an operation but the base path is not empty.
    Overshoot { base: String, files: Vec<String>, document: String },
    /// An operation's template ends with `P` and is longer, the prefix unproven.
    SuffixOnly { base: String, files: Vec<String>, operation: String, document: String },
    /// Nothing in the member's externals matches.
    NoMatch,
}

impl CallClass {
    pub fn label(&self) -> &'static str {
        match self {
            Self::NoExternal => "THE MEMBER HOLDS NO NAMED EXTERNAL",
            Self::NotComposed(_) => "NOT COMPOSED",
            Self::NoBaseKey { .. } => "NO COMMITTED BASE-URL KEY",
            Self::Disagree(_) => "REFUSED, BASE PATHS DISAGREE",
            Self::OverlayWithoutPath(_) => "REFUSED, OVERRIDING OVERLAY COMMITS NO URL PATH",
            Self::Exact { .. } => "EXACT UNDER A COMMITTED BASE PATH",
            Self::Overshoot { .. } => "BASE PATH OVERSHOOTS",
            Self::SuffixOnly { .. } => "SUFFIX-ONLY",
            Self::NoMatch => "NO MATCH",
        }
    }
}

/// Join a base path and a call path: `/prov` + `/domain/{d}` → `/prov/domain/{d}`.
pub fn join_base(base: &str, path: &str) -> String {
    format!("{}{path}", base.trim_end_matches('/'))
}

/// Judge one composed invocation row against the member's external operations,
/// given as `(document path, keys)`.
pub fn judge_call(
    method: &str,
    path: &str,
    reading: &BaseReading,
    externals: &[(&str, &BTreeSet<OpKey>)],
) -> CallClass {
    if externals.is_empty() {
        return CallClass::NoExternal;
    }
    let holding = |key: &OpKey| externals.iter().find(|(_, keys)| keys.contains(key)).map(|(d, _)| *d);
    let bare = normalize_template(path).map(|t| (method.to_string(), t));
    let (base, files) = match reading {
        BaseReading::NoKey => {
            return CallClass::NoBaseKey { p_exact: bare.as_ref().and_then(holding).is_some() }
        }
        BaseReading::Disagree(rows) => return CallClass::Disagree(rows.clone()),
        BaseReading::OverlayWithoutPath(files) => {
            return CallClass::OverlayWithoutPath(files.clone())
        }
        BaseReading::One { path: base, files, overlay } => {
            let composed = normalize_template(&join_base(base, path)).map(|t| (method.to_string(), t));
            if let Some(document) = composed.as_ref().and_then(holding) {
                return CallClass::Exact {
                    base: base.clone(),
                    files: files.clone(),
                    overlay: *overlay,
                    document: document.to_string(),
                };
            }
            (base.clone(), files.clone())
        }
    };
    let Some(bare) = bare else { return CallClass::NoMatch };
    // No empty-base guard: with an empty base B + P is P, so a `P` that equals an
    // operation already returned `Exact` above, and reaching here means B is not
    // empty.
    if let Some(document) = holding(&bare) {
        return CallClass::Overshoot { base, files, document: document.to_string() };
    }
    for (document, keys) in externals {
        if let Some((_, template)) = keys
            .iter()
            .find(|(m, t)| *m == bare.0 && t.len() > bare.1.len() && t.ends_with(&bare.1))
        {
            return CallClass::SuffixOnly {
                base,
                files,
                operation: format!("{} {template}", bare.0),
                document: (*document).to_string(),
            };
        }
    }
    CallClass::NoMatch
}

/// One invocation row of the second half's population, reduced to what the
/// judgement reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvocationRow {
    pub member: String,
    pub symbol: String,
    /// Canonical key → the values the row's evidence names for it.
    pub evidence: BTreeMap<String, BTreeSet<String>>,
}

/// One judged invocation row.
#[derive(Debug, Clone)]
pub struct JudgedCall {
    pub row: InvocationRow,
    pub composed: Option<(String, String)>,
    pub class: CallClass,
    /// The same row under application configuration alone.
    pub app_only: CallClass,
}

/// Judge half 2.
pub fn judge_invocations(
    rows: &[InvocationRow],
    ledger: &BTreeMap<(String, String), Vec<String>>,
    declared: &DeclaredHalf,
    documents: &[Document],
    scalars: &[Scalar],
) -> Vec<JudgedCall> {
    let keys_of: BTreeMap<(&str, &str), &BTreeSet<OpKey>> =
        documents.iter().map(|d| ((d.member.as_str(), d.path.as_str()), &d.keys)).collect();
    rows.iter()
        .map(|row| {
            let externals: Vec<(&str, &BTreeSet<OpKey>)> = declared
                .rows
                .iter()
                .filter(|r| r.holder == row.member && r.standing == Holder::Eligible)
                .filter(|r| matches!(r.contract, Contract::External(_)))
                .filter_map(|r| {
                    keys_of.get(&(r.holder.as_str(), r.path.as_str())).map(|k| (r.path.as_str(), *k))
                })
                .collect();
            let evidence_keys: BTreeSet<String> = row.evidence.keys().cloned().collect();
            let targets = ledger
                .get(&(row.member.clone(), row.symbol.clone()))
                .map_or(&[][..], Vec::as_slice);
            let composed = join_ledger(targets, &evidence_keys)
                .and_then(|t| compose(t, &row.evidence));
            let (app, overlays): (Vec<&Scalar>, Vec<&Scalar>) = scalars
                .iter()
                .filter(|s| s.member == row.member)
                .partition(|s| s.source == SourceSet::Application);
            let judge = |admit: bool| -> CallClass {
                if externals.is_empty() {
                    return CallClass::NoExternal;
                }
                match &composed {
                    Err(why) => CallClass::NotComposed(why.clone()),
                    Ok((method, path)) => judge_call(
                        method,
                        path,
                        &base_reading(&evidence_keys, &app, &overlays, admit),
                        &externals,
                    ),
                }
            };
            JudgedCall {
                row: row.clone(),
                composed: composed.clone().ok(),
                class: judge(true),
                app_only: judge(false),
            }
        })
        .collect()
}

// ── Half 3: own-spec ties ──────────────────────────────────────────────────

/// Whether a member-relative path lies under a `src/main/resources/` directory,
/// at the member root or in a build module — and nowhere inside a documentation
/// tree ([`identity::is_documentation`], S-384's guard) or a test tree, which
/// the floor excludes in terms.
pub fn under_main_resources(path: &str) -> bool {
    let under = path.starts_with(MAIN_RESOURCES) || path.contains(&format!("/{MAIN_RESOURCES}"));
    let test_tree = path.starts_with("src/test/") || path.contains("/src/test/");
    under && !test_tree && !identity::is_documentation(path)
}

/// Where one contract-surface ambiguous row lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TieClass {
    /// The counted rows.
    OwnSpec,
    HolderCandidateNotMainResources,
    HolderNotCandidate,
    /// The tied list was truncated and does not show the holder.
    Undecided,
}

impl TieClass {
    pub fn label(self) -> &'static str {
        match self {
            Self::OwnSpec => "OWN-SPEC TIES",
            Self::HolderCandidateNotMainResources => {
                "HOLDER IS A CANDIDATE, SPEC NOT UNDER ITS MAIN RESOURCES"
            }
            Self::HolderNotCandidate => "HOLDER IS NOT A CANDIDATE",
            Self::Undecided => "UNDECIDABLE (TRUNCATED TIED LIST)",
        }
    }
}

/// One contract-surface ambiguous row, reduced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TieRow {
    pub holder: String,
    pub document: String,
    pub tied: Vec<String>,
    pub truncated: bool,
}

pub fn judge_tie(row: &TieRow) -> TieClass {
    let provision = OpProvision::Tied { members: row.tied.clone(), truncated: row.truncated };
    match holder_provides(&row.holder, &provision) {
        None => TieClass::Undecided,
        Some(false) => TieClass::HolderNotCandidate,
        Some(true) if under_main_resources(&row.document) => TieClass::OwnSpec,
        Some(true) => TieClass::HolderCandidateNotMainResources,
    }
}

// ── The whole judgement ────────────────────────────────────────────────────

/// Everything the three halves read, in plain data — built from the estate by
/// [`census`], or by hand in a fixture.
#[derive(Debug, Default)]
pub struct Census {
    pub members: BTreeSet<String>,
    /// S-411's `runnable` — the declared rule, which decides.
    pub runnable: BTreeSet<String>,
    /// Members committing source in a language that ships an `invocations`
    /// query — the narrower reading, printed beside the declared one and never
    /// deciding a verdict. See the finding for why it exists.
    pub invocation_capable: BTreeSet<String>,
    pub kinds: BTreeMap<String, Kind>,
    pub documents: Vec<Document>,
    pub invocation_rows: Vec<InvocationRow>,
    /// Invocation `no-provider-in-workspace` rows outside the REST metric, by
    /// relation — with [`Census::invocation_rows`], the intake's whole
    /// no-provider figure.
    pub non_rest_no_provider: BTreeMap<String, usize>,
    /// `(member, call-site symbol)` → its HTTP consumer ledger targets.
    pub ledger: BTreeMap<(String, String), Vec<String>>,
    pub tie_rows: Vec<TieRow>,
    /// Application config and deploy-overlay values, both source sets.
    pub scalars: Vec<Scalar>,
    pub members_read: usize,
    pub members_total: usize,
}

/// The three halves' verdicts plus the report inputs.
#[derive(Debug)]
pub struct Judgement {
    pub declared: DeclaredHalf,
    pub calls: Vec<JudgedCall>,
    pub ties: Vec<(TieRow, TieClass)>,
    /// Members holding a spec document and no runnable source, under the
    /// declared (S-411) rule.
    pub candidates: BTreeSet<String>,
    /// The same under the narrower `invocations`-capable reading — a report.
    pub candidates_strict: BTreeSet<String>,
}

impl Judgement {
    pub fn declared_pairs(&self) -> usize {
        self.declared.pairs().len()
    }
    pub fn invocation_exact(&self) -> usize {
        self.calls.iter().filter(|c| matches!(c.class, CallClass::Exact { .. })).count()
    }
    pub fn invocation_exact_app_only(&self) -> usize {
        self.calls.iter().filter(|c| matches!(c.app_only, CallClass::Exact { .. })).count()
    }
    pub fn own_spec_ties(&self) -> usize {
        self.ties.iter().filter(|(_, c)| *c == TieClass::OwnSpec).count()
    }
}

pub fn judge(census: &Census) -> Judgement {
    let declared = judge_declared(&census.documents, &census.runnable, &census.kinds);
    let calls = judge_invocations(
        &census.invocation_rows,
        &census.ledger,
        &declared,
        &census.documents,
        &census.scalars,
    );
    let ties = census.tie_rows.iter().map(|r| (r.clone(), judge_tie(r))).collect();
    let holders: BTreeSet<String> = census.documents.iter().map(|d| d.member.clone()).collect();
    let candidates = holders.iter().filter(|m| !census.runnable.contains(*m)).cloned().collect();
    let candidates_strict =
        holders.iter().filter(|m| !census.invocation_capable.contains(*m)).cloned().collect();
    Judgement { declared, calls, ties, candidates, candidates_strict }
}

/// A half's verdict against its floor.
pub fn verdict(measured: usize, floor: usize) -> &'static str {
    if measured >= floor {
        "HOLDS"
    } else {
        "FALSIFIED"
    }
}

/// The one verdict line — rendered identically in the report and the finding,
/// so the finding can be checked against the run by string equality.
pub fn verdict_line(half: &str, measured: usize, floor: usize) -> String {
    format!("{half}: measured {measured}, floor {floor} — {}", verdict(measured, floor))
}

// ── The estate read ────────────────────────────────────────────────────────

/// Read the estate into a [`Census`]: the product's coverage rows, every member's
/// contract surface and ledger, the committed configuration and the overlays.
pub fn census(root: &Path) -> Census {
    let federation = discover(root)
        .expect("the workspace manifest parses")
        .unwrap_or_else(|| {
            panic!(
                "{} is not a Logos workspace (no logos.workspace.toml up-tree) — this gate \
                 never enrols one",
                root.display()
            )
        });
    let members: BTreeSet<String> = federation.members.iter().map(|m| m.name.clone()).collect();
    let member_roots: BTreeMap<String, std::path::PathBuf> =
        federation.members.iter().map(|m| (m.name.clone(), m.root.clone())).collect();
    let kinds = parse_kinds(DECLARED_KINDS).expect("the declaration list parses");
    for name in kinds.keys() {
        assert!(
            members.contains(name),
            "the declaration names `{name}`, which is not a workspace member — a declaration \
             that names nothing is a failure, not a silent no-op"
        );
    }

    let registry = EngineRegistry::<Engine>::new(federation, RegistryMode::Lazy);
    let answer = registry.answer();
    let coverage = cross_service_coverage(&answer);
    let surfaces = answer.fan_out(|_, engine| engine.contract_surface());
    let ledgers = answer.fan_out(|_, engine| engine.invocation_refs());

    let mut provisions: BTreeMap<(String, String), OpProvision> = BTreeMap::new();
    let mut tie_rows = Vec::new();
    let mut invocation_rows = Vec::new();
    let mut non_rest_no_provider: BTreeMap<String, usize> = BTreeMap::new();
    for row in &coverage.references {
        match row_use(row) {
            RowUse::ContractSurface { provision, tie } => {
                let key = (row.from.member.clone(), row.from.symbol.as_str().to_string());
                provisions.insert(key, provision);
                tie_rows.extend(tie);
            }
            RowUse::Invocation(r) => invocation_rows.push(r),
            RowUse::NonRestNoProvider(relation) => {
                *non_rest_no_provider.entry(relation).or_default() += 1;
            }
            RowUse::Outside => {}
        }
    }

    let mut operations = Vec::new();
    for scoped in &surfaces {
        let Ok(Ok(nodes)) = &scoped.value else { continue };
        for node in nodes.iter().filter(|n| n.kind == NodeKind::ApiOperation) {
            operations.push((scoped.member.clone(), node.symbol.as_str().to_string(), node.name.clone()));
        }
    }
    let mut documents = build_documents(&operations, &provisions);
    for doc in &mut documents {
        if let Some(member_root) = member_roots.get(&doc.member) {
            if let Ok(text) = std::fs::read_to_string(member_root.join(&doc.path)) {
                doc.title = spec_title(&text);
            }
        }
    }

    let mut ledger: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    for scoped in &ledgers {
        let Ok(Ok(refs)) = &scoped.value else { continue };
        for r in refs {
            if is_http_consumer(r.relation) {
                ledger
                    .entry((scoped.member.clone(), r.symbol.as_str().to_string()))
                    .or_default()
                    .push(r.target.clone());
            }
        }
    }

    let config = ConfigCorpus::discover(root);
    let runnable = runnable_members(root, &members, &config);
    let invocation_capable =
        members_with_source(root, &members, &config, |p| p.query("invocations").is_some());
    let mut scalars = walk_overlays(root, &members).scalars;
    for source in &config.sources {
        let member = source.path.split('/').next().unwrap_or("").to_string();
        if !members.contains(&member) {
            continue;
        }
        for (key, values) in &source.values {
            for value in values {
                scalars.push(Scalar {
                    member: member.clone(),
                    key: key.clone(),
                    value: value.clone(),
                    source: SourceSet::Application,
                    file: source.path.clone(),
                });
            }
        }
    }

    Census {
        members,
        runnable,
        invocation_capable,
        kinds,
        documents,
        invocation_rows,
        non_rest_no_provider,
        ledger,
        tie_rows,
        scalars,
        members_read: coverage.members_read as usize,
        members_total: coverage.members_total as usize,
    }
}

/// What one product coverage row contributes to the census — the ONE place a row
/// is sorted into the halves' populations, so the sorting is fixture-pinned
/// rather than exercised only when the estate is configured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowUse {
    /// A contract-surface row: its operation's provision, and — when it is a
    /// tie — the third half's reduced row.
    ContractSurface { provision: OpProvision, tie: Option<TieRow> },
    /// A second-half row: invocation intake, relation `route`, reason
    /// `no-provider-in-workspace`.
    Invocation(InvocationRow),
    /// An invocation `no-provider-in-workspace` row of another relation — not
    /// REST, so outside the metric; counted so the population reconciles to the
    /// intake's own no-provider figure.
    NonRestNoProvider(String),
    /// In neither population.
    Outside,
}

pub fn row_use(row: &ReferenceCoverage) -> RowUse {
    match row.intake {
        BridgeIntake::ContractSurface => {
            let Some((members, truncated)) = tied(row) else {
                return RowUse::ContractSurface { provision: OpProvision::Other, tie: None };
            };
            let tie = TieRow {
                holder: row.from.member.clone(),
                document: document_path(row.from.symbol.as_str()).unwrap_or_default(),
                tied: members.clone(),
                truncated,
            };
            RowUse::ContractSurface {
                provision: OpProvision::Tied { members, truncated },
                tie: Some(tie),
            }
        }
        BridgeIntake::Invocation => {
            let no_provider = matches!(
                row.state,
                CoverageState::Unbound { reason: UnboundReason::NoProviderInWorkspace }
            );
            match (no_provider, row.relation == "route") {
                (true, true) => RowUse::Invocation(InvocationRow {
                    member: row.from.member.clone(),
                    symbol: row.from.symbol.as_str().to_string(),
                    evidence: evidence(&row.provenance),
                }),
                (true, false) => RowUse::NonRestNoProvider(row.relation.clone()),
                (false, _) => RowUse::Outside,
            }
        }
    }
}

/// Whether a ledger reference is an HTTP call site — the consumer side of the
/// HTTP namespace, read off the arm's own descriptors.
pub fn is_http_consumer(relation: ArtifactRelation) -> bool {
    relation.bridge_namespace() == Some(BridgeNamespace::Http)
        && relation.bridge_role() == Some(BridgeRole::Consumer)
}

/// An ambiguous row's tied members, as the row lists them, and whether the list
/// was truncated. `None` for any other row.
fn tied(row: &ReferenceCoverage) -> Option<(Vec<String>, bool)> {
    let candidates = row.candidates.as_ref()?;
    (candidates.disposition == ProviderDisposition::TiedBetween).then(|| {
        (
            candidates.providers.iter().map(|p| p.member.clone()).collect(),
            candidates.omitted > 0,
        )
    })
}

/// A row's configuration evidence, canonical key → values.
fn evidence(provenance: &Provenance) -> BTreeMap<String, BTreeSet<String>> {
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    if let Provenance::ConfigBound { bound } = provenance {
        for b in bound {
            let entry = out.entry(canonical_key(&b.key)).or_default();
            entry.extend(b.values.iter().map(|v| v.value.clone()));
        }
    }
    out
}

/// The measurement, computed once per test binary.
pub(crate) fn judgement(root: &Path) -> &'static (Census, Judgement) {
    static ONCE: std::sync::OnceLock<(Census, Judgement)> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| {
        let census = census(root);
        let judgement = judge(&census);
        (census, judgement)
    })
}

// ── The report ─────────────────────────────────────────────────────────────

fn report(root: &Path, census: &Census, j: &Judgement) {
    println!(
        "S-456 / CR-147 §3.2 B1 over {} — {} manifest members, {} read of {}; {} spec \
         documents; {} runnable members",
        root.display(),
        census.members.len(),
        census.members_read,
        census.members_total,
        census.documents.len(),
        census.runnable.len(),
    );

    // Half 1.
    let d = &j.declared;
    let (identity, external) = d.split();
    println!("\n== HALF 1 — {HALF_DECLARED}");
    println!("  {}", verdict_line(HALF_DECLARED, j.declared_pairs(), DECLARED_CONTRACTS_FLOOR));
    println!("  split: {identity} document-identity · {external} named-external");
    for (holder, contract) in d.pairs() {
        println!("    COUNTED  {holder} -> {}", d.contract_label(&contract));
    }
    for row in d.rows.iter().filter(|r| r.standing == Holder::Eligible) {
        let how = match &row.identity {
            Identity::Member { member, path, shared, total } => {
                format!("identity {shared}/{total} with {member}:{path}")
            }
            Identity::Collision { members, shared, total } => {
                format!("identity COLLISION {shared}/{total} between {members:?}")
            }
            Identity::NoMember => "no member's own spec reaches 90 %".to_string(),
        };
        println!("      via {}:{} — {how}", row.holder, row.path);
    }
    for standing in [
        Holder::Declared(Kind::Documentation),
        Holder::Declared(Kind::Mock),
        Holder::NotRunnable,
    ] {
        let pairs = d.pairs_of(standing);
        let label = match standing {
            Holder::Declared(k) => format!("{} holder (declared)", k.label()),
            Holder::NotRunnable => "non-runnable holder".to_string(),
            Holder::Eligible => unreachable!(),
        };
        println!("  subtracted, {label}: {} pair(s)", pairs.len());
        for (holder, contract) in pairs {
            println!("    {holder} -> {}", d.contract_label(&contract));
        }
    }
    let collisions = d.collisions();
    println!("  identity collisions (resolve to neither member): {}", collisions.len());
    for row in collisions {
        if let Identity::Collision { members, shared, total } = &row.identity {
            println!("    {}:{} — {shared}/{total} with each of {members:?}", row.holder, row.path);
        }
    }
    println!("  partially implemented documents: {}", d.partial.len());
    for (m, p, provided, keyed) in &d.partial {
        println!("    {m}:{p} — holder provides {provided} of {keyed}");
    }
    println!("  unjudged documents (no keyed operation, or undecidable): {}", d.unjudged.len());
    for (m, p) in &d.unjudged {
        println!("    {m}:{p}");
    }
    println!("  named externals (copy-graph components holding a vendored document):");
    let used: BTreeSet<usize> = d
        .rows
        .iter()
        .filter_map(|r| match r.contract {
            Contract::External(c) => Some(c),
            Contract::Member(_) => None,
        })
        .collect();
    for c in used {
        let (name, copies) = &d.externals[&c];
        println!("    {name} — {} cop(ies)", copies.len());
        for (m, p) in copies {
            println!("      {m}:{p}");
        }
    }

    // Half 2.
    println!("\n== HALF 2 — {HALF_INVOCATION}");
    println!("  {}", verdict_line(HALF_INVOCATION, j.invocation_exact(), INVOCATION_EXTERNAL_FLOOR));
    println!(
        "  population: {} invocation-intake `no-provider-in-workspace` `route` rows \
         (+ {} of other relations, outside the REST metric: {:?})",
        j.calls.len(),
        census.non_rest_no_provider.values().sum::<usize>(),
        census.non_rest_no_provider,
    );
    println!(
        "  counterfactual, application config alone: {} exact",
        j.invocation_exact_app_only()
    );
    let mut app_only: BTreeMap<&str, usize> = BTreeMap::new();
    for call in &j.calls {
        *app_only.entry(call.app_only.label()).or_default() += 1;
    }
    for (label, n) in &app_only {
        println!("    under application config alone, {label}: {n}");
    }
    let mut by_class: BTreeMap<&str, Vec<&JudgedCall>> = BTreeMap::new();
    for call in &j.calls {
        by_class.entry(call.class.label()).or_default().push(call);
    }
    for (label, calls) in &by_class {
        println!("  {label}: {}", calls.len());
        for call in calls {
            let path = call
                .composed
                .as_ref()
                .map_or_else(|| "(not composed)".to_string(), |(m, p)| format!("{m} {p}"));
            let site = call.row.symbol.rsplit('/').next().unwrap_or(&call.row.symbol);
            let detail = match &call.class {
                CallClass::Exact { base, files, overlay, document } => format!(
                    "base `{base}` from {} ({}) -> {document}",
                    files.join(", "),
                    if *overlay { "deploy overlay" } else { "application config" }
                ),
                CallClass::Overshoot { base, files, document } => {
                    format!("base `{base}` from {} overshoots {document}", files.join(", "))
                }
                CallClass::SuffixOnly { base, files, operation, document } => format!(
                    "base `{base}` from {}; `{operation}` in {document} only suffix-matches",
                    files.join(", ")
                ),
                CallClass::Disagree(rows) => rows
                    .iter()
                    .map(|(p, f)| format!("`{p}` from {f}"))
                    .collect::<Vec<_>>()
                    .join("; "),
                CallClass::NotComposed(why) => format!("{why:?}"),
                CallClass::OverlayWithoutPath(files) => format!("overridden by {}", files.join(", ")),
                CallClass::NoBaseKey { p_exact } => format!("P alone exact: {p_exact}"),
                CallClass::NoMatch | CallClass::NoExternal => String::new(),
            };
            println!("    {} {site} — {path} — {detail}", call.row.member);
        }
    }

    // Half 3.
    println!("\n== HALF 3 — {HALF_OWN_SPEC}");
    println!("  {}", verdict_line(HALF_OWN_SPEC, j.own_spec_ties(), OWN_SPEC_TIES_FLOOR));
    println!("  population: {} contract-surface ambiguous rows", j.ties.len());
    let mut ties: BTreeMap<TieClass, BTreeMap<String, usize>> = BTreeMap::new();
    for (row, class) in &j.ties {
        let holder = match census.kinds.get(&row.holder) {
            Some(k) => format!("{} ({}, declared)", row.holder, k.label()),
            None => row.holder.clone(),
        };
        *ties.entry(*class).or_default().entry(holder).or_default() += 1;
    }
    for (class, holders) in &ties {
        println!("  {}: {}", class.label(), holders.values().sum::<usize>());
        for (holder, n) in holders {
            println!("    {holder}: {n}");
        }
    }

    // Candidates.
    println!(
        "\n== CANDIDATES — members holding a spec document and no runnable source (a report; \
         the declaration list, not this, is what subtracts)"
    );
    println!("  under the declared S-411 `runnable` rule: {}", j.candidates.len());
    for m in &j.candidates {
        let declared = census.kinds.get(m).map_or("NOT declared".to_string(), |k| {
            format!("declared {}", k.label())
        });
        println!("  {m} — {declared}");
    }
    for (m, k) in &census.kinds {
        if !j.candidates.contains(m) {
            println!("  {m} — declared {} but NOT a candidate (it holds runnable source)", k.label());
        }
    }
    println!(
        "  under the narrower reading (no file in a language shipping `invocations`): {}",
        j.candidates_strict.len()
    );
    for m in &j.candidates_strict {
        let declared = census.kinds.get(m).map_or("NOT declared".to_string(), |k| {
            format!("declared {}", k.label())
        });
        println!("  {m} — {declared}");
    }
    println!(
        "  half-1 sensitivity: counted pairs whose holder the narrower reading would call \
         non-runnable: {}",
        j.declared.pairs().iter().filter(|(h, _)| !census.invocation_capable.contains(h)).count()
    );

    // Reconciliation.
    println!("\n== RECONCILIATION — CR-147 §2.1, by name");
    for row in j.declared.rows.iter().filter(|r| r.holder == "webmail") {
        let ties_in_doc = census
            .tie_rows
            .iter()
            .filter(|t| t.holder == "webmail" && t.document == row.path)
            .count();
        println!(
            "  webmail:{} -> {} ({:?}); {ties_in_doc} of its operations tied per-operation",
            row.path,
            j.declared.contract_label(&row.contract),
            row.identity,
        );
    }
    let facade: Vec<&JudgedCall> =
        j.calls.iter().filter(|c| c.row.member == "pecserver-facade").collect();
    println!(
        "  pecserver-facade: {} no-provider route rows; {} exact under a committed base path; \
         {} exact under application config alone",
        facade.len(),
        facade.iter().filter(|c| matches!(c.class, CallClass::Exact { .. })).count(),
        facade.iter().filter(|c| matches!(c.app_only, CallClass::Exact { .. })).count(),
    );
}

// ── The estate gate ────────────────────────────────────────────────────────

/// Measure the three halves over the reference estate and hold the result to
/// the recorded finding.
///
/// Skips without `LOGOS_REF_WORKSPACE`, like every estate gate in this binary —
/// which is why the classifiers deciding each half are pinned by the always-run
/// fixtures in [`tests`], and why this run must be shown explicitly: it is
/// invisible to `gate.sh` and to CI.
#[test]
fn measure_vendored_spec_contracts_over_the_reference_workspace() {
    let Some(root) = crate::corpus_root() else {
        eprintln!(
            "SKIPPED: set LOGOS_REF_WORKSPACE=<path to the reference workspace> to run the \
             S-456 vendored-spec gate (see vendored_spec_contracts_finding.txt for the \
             recorded verdicts)."
        );
        return;
    };
    let (census, j) = judgement(&root);
    report(&root, census, j);

    assert!(
        census.members_read == census.members_total && census.members_total > 0,
        "every member must be read: {} of {} — a partial workspace is a different estate",
        census.members_read,
        census.members_total,
    );
    assert!(
        j.ties.iter().all(|(_, c)| *c != TieClass::Undecided),
        "a truncated tied list hid a holder — the third half is undecidable as measured"
    );

    let measured = [
        (HALF_DECLARED, j.declared_pairs(), DECLARED_CONTRACTS_FLOOR),
        (HALF_INVOCATION, j.invocation_exact(), INVOCATION_EXTERNAL_FLOOR),
        (HALF_OWN_SPEC, j.own_spec_ties(), OWN_SPEC_TIES_FLOOR),
    ];
    for (half, n, floor) in measured {
        let line = verdict_line(half, n, floor);
        assert!(
            RECORDED_FINDING.contains(&line),
            "the recorded finding does not carry this run's verdict line `{line}` — record \
             the new figures in vendored_spec_contracts_finding.txt (and supersede ADR-68 on \
             a falsified half), never bend a classifier to reproduce the old ones",
        );
    }

    assert_eq!(j.declared_pairs(), RECORDED_DECLARED_PAIRS, "declared-contract pairs drifted");
    assert_eq!(j.declared.split(), RECORDED_DECLARED_SPLIT, "identity/external split drifted");
    assert_eq!(j.calls.len(), RECORDED_INVOCATION_POPULATION, "invocation population drifted");
    assert_eq!(
        census.non_rest_no_provider.values().sum::<usize>(),
        RECORDED_NON_REST_NO_PROVIDER,
        "the non-REST no-provider rows drifted — the reconciliation to the intake's figure moved"
    );
    assert_eq!(j.invocation_exact(), RECORDED_INVOCATION_EXACT, "exact invocation rows drifted");
    assert_eq!(
        j.invocation_exact_app_only(),
        RECORDED_INVOCATION_EXACT_APP_ONLY,
        "the application-config-only counterfactual drifted"
    );
    assert_eq!(j.ties.len(), RECORDED_TIE_POPULATION, "tie population drifted");
    assert_eq!(j.own_spec_ties(), RECORDED_OWN_SPEC_TIES, "own-spec ties drifted");
    assert_eq!(j.candidates_strict.len(), RECORDED_CANDIDATES_STRICT, "strict candidates drifted");
    assert_eq!(
        j.declared.pairs().iter().filter(|(h, _)| !census.invocation_capable.contains(h)).count(),
        RECORDED_DECLARED_SENSITIVITY,
        "the half-1 sensitivity to the narrower runnable reading drifted"
    );
}

// ── Always-run classifier fixtures ─────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn key(method: &str, template: &str) -> OpKey {
        operation_key(&format!("{method} {template}")).expect("fixture key normalizes")
    }

    fn keys(ops: &[(&str, &str)]) -> BTreeSet<OpKey> {
        ops.iter().map(|(m, t)| key(m, t)).collect()
    }

    fn doc(member: &str, path: &str, ops: &[(&str, &str)], provided: usize) -> Document {
        let keys = keys(ops);
        Document {
            member: member.into(),
            path: path.into(),
            keyed_ops: keys.len(),
            keys,
            provided,
            ..Document::default()
        }
    }

    fn scalar(member: &str, key: &str, value: &str, source: SourceSet, file: &str) -> Scalar {
        Scalar {
            member: member.into(),
            key: canonical_key(key),
            value: value.into(),
            source,
            file: file.into(),
        }
    }

    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    // ── Declarations ────────────────────────────────────────────────────────

    #[test]
    fn the_floors_are_the_ones_declared_before_the_run() {
        // Parses the DECLARATION rather than comparing a constant with itself.
        for (half, constant) in [
            (HALF_DECLARED, DECLARED_CONTRACTS_FLOOR),
            (HALF_INVOCATION, INVOCATION_EXTERNAL_FLOOR),
            (HALF_OWN_SPEC, OWN_SPEC_TIES_FLOOR),
        ] {
            let declared = declared_floor(DECLARED_FLOOR, half).unwrap();
            assert_eq!(
                declared, constant,
                "`{half}` floor constant is {constant} but the declaration says {declared} — \
                 change the constant only by re-deciding CR-147, never to clear a run"
            );
        }
        assert!(
            DECLARED_FLOOR.contains("2026-09-27T22:13:34Z"),
            "the declaration must carry the UTC timestamp that makes it a floor"
        );
    }

    #[test]
    fn a_floor_stated_twice_is_no_floor() {
        let text = ">= 3 Declared contracts here.\n>= 9 Declared contracts, superseded.\n";
        assert!(declared_floor(text, HALF_DECLARED).is_err());
        assert!(declared_floor("nothing", HALF_DECLARED).is_err());
        // A near miss: a line naming the half without the `>= ` prefix is prose.
        assert_eq!(
            declared_floor("Declared contracts >= 7\n>= 3 Declared contracts x\n", HALF_DECLARED),
            Ok(3)
        );
    }

    #[test]
    fn the_committed_declaration_list_parses_to_the_three_cr_147_members() {
        let kinds = parse_kinds(DECLARED_KINDS).unwrap();
        assert_eq!(kinds.get("software-architecture-documents"), Some(&Kind::Documentation));
        assert_eq!(kinds.get("pecserver-mock"), Some(&Kind::Mock));
        assert_eq!(kinds.get("notification-gateway-mock"), Some(&Kind::Mock));
        assert_eq!(kinds.len(), 3);
    }

    #[test]
    fn a_malformed_declaration_is_an_error_never_a_skip() {
        assert!(parse_kinds("a documentation extra").is_err());
        assert!(parse_kinds("a docs").is_err(), "an unknown kind must fail");
        assert!(parse_kinds("a mock\na mock").is_err(), "a duplicate must fail");
        assert_eq!(parse_kinds("# c\n\n a mock # x\n").unwrap().len(), 1);
    }

    // ── Keys and documents ──────────────────────────────────────────────────

    #[test]
    fn an_operation_key_is_the_products_positional_key() {
        assert_eq!(
            operation_key("get /users/{id}/mail"),
            Some(("GET".into(), "/users/{}/mail".into()))
        );
        assert_eq!(operation_key("GET /users/{userId}"), operation_key("GET /users/{id}"));
        assert_eq!(operation_key("get"), None, "a bare method has no key");
        assert_eq!(operation_key("GET users"), None);
        assert_eq!(operation_key("GET /files/{*rest}"), None, "a catch-all never normalizes");
    }

    #[test]
    fn a_document_path_is_the_symbols_file_descriptor() {
        assert_eq!(
            document_path("logos . . . api/src/main/resources/openapi/`v1.yaml`/v1-users#get#")
                .as_deref(),
            Some("api/src/main/resources/openapi/v1.yaml")
        );
        assert_eq!(
            document_path("logos . . . `PSS-API_index_v1.0.3_20231025.yaml`/prov-domain#get#")
                .as_deref(),
            Some("PSS-API_index_v1.0.3_20231025.yaml")
        );
        // A `/` inside a quoted name is not a descriptor boundary.
        assert_eq!(
            document_path("logos . . . src/`v1.yaml`/`a/b`#get#").as_deref(),
            Some("src/v1.yaml")
        );
        assert_eq!(document_path("logos . . . op#get#"), None);
    }

    #[test]
    fn the_holder_provides_an_operation_exactly_when_the_tier_says_so() {
        assert_eq!(holder_provides("a", &OpProvision::Sole), Some(true));
        let tied = |m: &[&str], truncated| OpProvision::Tied {
            members: m.iter().map(|s| (*s).to_string()).collect(),
            truncated,
        };
        assert_eq!(holder_provides("a", &tied(&["a", "b"], false)), Some(true));
        assert_eq!(holder_provides("a", &tied(&["b", "c"], false)), Some(false));
        assert_eq!(holder_provides("a", &tied(&["b"], true)), None, "truncated: undecidable");
        assert_eq!(holder_provides("a", &OpProvision::Other), Some(false));
    }

    #[test]
    fn implementation_bands_are_own_vendored_partial() {
        assert_eq!(implementation(9, 10, 0), Implementation::Own);
        assert_eq!(implementation(8, 10, 0), Implementation::Partial, "89 % is not own");
        assert_eq!(implementation(0, 10, 0), Implementation::Vendored);
        assert_eq!(implementation(1, 10, 0), Implementation::Partial, "one provided is not vendored");
        assert_eq!(implementation(0, 0, 0), Implementation::Unjudged);
        assert_eq!(implementation(0, 10, 1), Implementation::Unjudged);
    }

    #[test]
    fn documents_group_by_file_and_read_provision_off_the_tier() {
        let ops = vec![
            ("m".to_string(), "logos . . . `a.yaml`/x#get#".to_string(), "GET /x".to_string()),
            ("m".to_string(), "logos . . . `a.yaml`/y#get#".to_string(), "GET /y".to_string()),
            ("m".to_string(), "logos . . . `a.yaml`/z#get#".to_string(), "get".to_string()),
            ("m".to_string(), "logos . . . `b.yaml`/x#get#".to_string(), "GET /x".to_string()),
        ];
        let mut provisions = BTreeMap::new();
        provisions.insert(("m".to_string(), ops[1].1.clone()), OpProvision::Other);
        let docs = build_documents(&ops, &provisions);
        assert_eq!(docs.len(), 2);
        let a = docs.iter().find(|d| d.path == "a.yaml").unwrap();
        assert_eq!((a.keyed_ops, a.provided, a.unkeyed), (2, 1, 1));
        let b = docs.iter().find(|d| d.path == "b.yaml").unwrap();
        assert_eq!((b.keyed_ops, b.provided), (1, 1), "no row is the tier's sole-provider None");
    }

    // ── Half 1 ──────────────────────────────────────────────────────────────

    #[test]
    fn identity_needs_ninety_percent_of_the_held_document() {
        let held = doc("w", "agg.yaml", &[("GET", "/a"), ("GET", "/b"), ("GET", "/c"), ("GET", "/d"),
            ("GET", "/e"), ("GET", "/f"), ("GET", "/g"), ("GET", "/h"), ("GET", "/i"), ("GET", "/j")], 0);
        let nine = doc("agg", "v1.yaml", &[("GET", "/a"), ("GET", "/b"), ("GET", "/c"), ("GET", "/d"),
            ("GET", "/e"), ("GET", "/f"), ("GET", "/g"), ("GET", "/h"), ("GET", "/i")], 9);
        let eight = doc("agg", "v1.yaml", &[("GET", "/a"), ("GET", "/b"), ("GET", "/c"), ("GET", "/d"),
            ("GET", "/e"), ("GET", "/f"), ("GET", "/g"), ("GET", "/h")], 8);
        let none = BTreeSet::new();
        assert!(matches!(
            resolve_identity(&held, &[&nine], &none),
            Identity::Member { ref member, shared: 9, total: 10, .. } if member == "agg"
        ));
        assert_eq!(resolve_identity(&held, &[&eight], &none), Identity::NoMember, "80 % is below");
    }

    #[test]
    fn identity_takes_the_best_member_and_each_members_best_spec() {
        let ten: Vec<(&str, &str)> = ["/a", "/b", "/c", "/d", "/e", "/f", "/g", "/h", "/i", "/j"]
            .iter()
            .map(|t| ("GET", *t))
            .collect();
        let held = doc("w", "x.yaml", &ten, 0);
        let p_full = doc("p", "v1.yaml", &ten, 10);
        let p_old = doc("p", "old.yaml", &ten[..9], 9);
        let q_nine = doc("q", "v1.yaml", &ten[..9], 9);
        let none = BTreeSet::new();
        // Across members: the higher score wins, both clearing 90 %.
        assert!(matches!(
            resolve_identity(&held, &[&q_nine, &p_full], &none),
            Identity::Member { ref member, shared: 10, .. } if member == "p"
        ));
        // Within a member: its best spec stands for it, whatever order they come
        // in — else p's weaker copy would tie q and resolve to neither.
        assert!(matches!(
            resolve_identity(&held, &[&p_old, &p_full, &q_nine], &none),
            Identity::Member { ref member, ref path, shared: 10, .. } if member == "p" && path == "v1.yaml"
        ));
    }

    #[test]
    fn an_equal_best_score_resolves_to_neither_member() {
        let held = doc("w", "x.yaml", &[("GET", "/a")], 0);
        let one = doc("p", "v1.yaml", &[("GET", "/a")], 1);
        let two = doc("q", "v1.yaml", &[("GET", "/a")], 1);
        assert!(matches!(
            resolve_identity(&held, &[&one, &two], &BTreeSet::new()),
            Identity::Collision { ref members, .. } if members.len() == 2
        ));
    }

    #[test]
    fn a_collision_is_enumerated_and_falls_through_to_an_external() {
        let c = Census {
            runnable: set(&["w", "p", "q"]),
            documents: vec![
                doc("w", "x.yaml", &[("GET", "/a")], 0),
                doc("p", "v1.yaml", &[("GET", "/a")], 1),
                doc("q", "v1.yaml", &[("GET", "/a")], 1),
            ],
            ..Census::default()
        };
        let j = judge(&c);
        let hits = j.declared.collisions();
        assert_eq!(hits.len(), 1);
        assert_eq!((hits[0].holder.as_str(), hits[0].path.as_str()), ("w", "x.yaml"));
        assert!(matches!(hits[0].contract, Contract::External(_)));
    }

    #[test]
    fn a_declared_mock_is_never_the_member_a_document_identifies() {
        let held = doc("facade", "pss.yaml", &[("GET", "/prov/a")], 0);
        let mock = doc("pss-mock", "swagger.yaml", &[("GET", "/prov/a")], 1);
        assert_eq!(resolve_identity(&held, &[&mock], &set(&["pss-mock"])), Identity::NoMember);
        assert!(matches!(resolve_identity(&held, &[&mock], &BTreeSet::new()), Identity::Member { .. }));
    }

    #[test]
    fn copies_group_by_containment_so_a_newer_version_is_the_same_external() {
        let old: BTreeSet<OpKey> = (0..10).map(|i| key("GET", &format!("/p/{i}"))).collect();
        let new: BTreeSet<OpKey> = (0..13).map(|i| key("GET", &format!("/p/{i}"))).collect();
        let other: BTreeSet<OpKey> = keys(&[("GET", "/q")]);
        // 10 of 13 is 77 % one way, 100 % the other: containment joins them.
        assert!(!contained(&new, &old));
        assert!(contained(&old, &new));
        assert_eq!(external_components(&[&old, &new, &other]), vec![0, 0, 2]);
    }

    #[test]
    fn an_external_is_named_by_its_title_unless_it_is_the_springdoc_default() {
        assert_eq!(external_name(&[("b/pss.yaml", Some("PSS")), ("a/x.yaml", Some("PSS"))]), "PSS");
        assert_eq!(
            external_name(&[("z/v1.yaml", Some(SPRINGDOC_DEFAULT_TITLE)), ("a/agg.yml", None)]),
            "agg"
        );
        assert_eq!(spec_title("openapi: 3.0.1\ninfo:\n  title: Notification Gateway\n  version: 1\n").as_deref(),
            Some("Notification Gateway"));
        assert_eq!(spec_title("{\"openapi\":\"3\",\"info\":{\"title\":\"PSS\",\"v\":1}}").as_deref(), Some("PSS"));
        assert_eq!(spec_title("info:\n  title: 'OpenAPI definition'\n").as_deref(), Some(SPRINGDOC_DEFAULT_TITLE));
        assert_eq!(spec_title("paths: {}\n"), None);
    }

    /// The estate's dominant shapes: a consumer holding an aggregator's own spec
    /// byte-for-byte, two consumers and a docs repo holding one external in two
    /// versions, a mock holding a copy, and a holder's own spec.
    fn half_one_fixture() -> Census {
        let agg: &[(&str, &str)] = &[("GET", "/v1/a"), ("POST", "/v1/a"), ("GET", "/v1/b")];
        // Two versions of one external: the newer a superset of the older.
        let pss_old: Vec<(String, String)> =
            (0..10).map(|i| ("GET".to_string(), format!("/prov/{i}"))).collect();
        let pss_new: Vec<(String, String)> =
            (0..12).map(|i| ("GET".to_string(), format!("/prov/{i}"))).collect();
        let mk = |member: &str, path: &str, ops: &[(String, String)], provided: usize| {
            let refs: Vec<(&str, &str)> = ops.iter().map(|(m, t)| (m.as_str(), t.as_str())).collect();
            doc(member, path, &refs, provided)
        };
        let mut pss_copy = mk("facade", "src/main/resources/pss.yaml", &pss_old, 0);
        pss_copy.title = Some("PSS".into());
        Census {
            runnable: set(&["webmail", "facade", "agg", "notif"]),
            kinds: [("docs".to_string(), Kind::Documentation), ("pss-mock".to_string(), Kind::Mock)]
                .into_iter()
                .collect(),
            documents: vec![
                doc("agg", "src/main/resources/openapi/v1.yaml", agg, 3),
                doc("webmail", "agg.yaml", agg, 0),
                mk("webmail", "pss-1.yaml", &pss_old, 0),
                mk("webmail", "pss-2.yaml", &pss_new, 0),
                pss_copy,
                mk("pss-mock", "swagger.yaml", &pss_old, 10),
                mk("docs", "PSS-api.yaml", &pss_old, 0),
                doc("docs", "agg.yml", agg, 0),
                doc("notif", "gw.yaml", &[("POST", "/v1/notification/send")], 0),
            ],
            ..Census::default()
        }
    }

    #[test]
    fn half_one_counts_identity_and_external_pairs_and_subtracts_declared_holders() {
        let c = half_one_fixture();
        let j = judge(&c);
        let pairs: Vec<String> = j
            .declared
            .pairs()
            .iter()
            .map(|(h, c)| format!("{h} -> {}", j.declared.contract_label(c)))
            .collect();
        assert_eq!(
            pairs,
            vec![
                "facade -> external PSS".to_string(),
                "notif -> external gw".to_string(),
                "webmail -> member agg".to_string(),
                "webmail -> external PSS".to_string(),
            ]
        );
        assert_eq!(j.declared.split(), (1, 3));
        // The docs repo's copies are subtracted and enumerated, never counted.
        let docs = j.declared.pairs_of(Holder::Declared(Kind::Documentation));
        assert_eq!(docs.len(), 2, "docs holds an identity copy and an external copy: {docs:?}");
        // The mock serves its copy, so it declares no pair — but the copy joins
        // PSS's copy graph, and, being declared, the mock is never the member PSS
        // resolves to.
        assert!(j.declared.pairs_of(Holder::Declared(Kind::Mock)).is_empty());
        assert!(j.declared.externals.values().any(|(name, copies)| name == "PSS"
            && copies.iter().any(|(m, _)| m == "pss-mock")));
        assert_eq!(j.candidates, set(&["docs", "pss-mock"]));
    }

    #[test]
    fn a_served_mock_copy_joins_two_partial_copies_into_one_external() {
        // Neither of facade's copies contains the other; the mock's full copy
        // contains both, so they are one external and one pair.
        let range = |a: usize, b: usize| -> Vec<(String, String)> {
            (a..b).map(|i| ("GET".to_string(), format!("/prov/{i}"))).collect()
        };
        let mk = |member: &str, path: &str, ops: &[(String, String)], provided: usize| {
            let refs: Vec<(&str, &str)> = ops.iter().map(|(m, t)| (m.as_str(), t.as_str())).collect();
            doc(member, path, &refs, provided)
        };
        let c = Census {
            runnable: set(&["facade"]),
            kinds: [("pss-mock".to_string(), Kind::Mock)].into_iter().collect(),
            documents: vec![
                mk("facade", "a.yaml", &range(0, 10), 0),
                mk("facade", "b.yaml", &range(5, 20), 0),
                mk("pss-mock", "swagger.yaml", &range(0, 20), 20),
            ],
            ..Census::default()
        };
        assert_eq!(judge(&c).declared_pairs(), 1);
    }

    #[test]
    fn a_partially_implemented_document_counts_toward_nothing() {
        let mut c = half_one_fixture();
        c.documents.push(doc("facade", "src/main/resources/openapi/v1.yaml",
            &[("GET", "/v1/m/a"), ("GET", "/v1/m/b")], 1));
        let j = judge(&c);
        assert_eq!(j.declared.partial.len(), 1);
        assert_eq!(j.declared_pairs(), 4, "the partial document adds no pair");
    }

    #[test]
    fn the_candidate_reports_read_each_rule_and_move_no_verdict() {
        let mut c = half_one_fixture();
        // The estate's shape: under S-411's rule every spec holder is runnable.
        c.runnable.extend(set(&["docs", "pss-mock"]));
        c.invocation_capable = set(&["webmail", "facade", "agg", "notif", "docs"]);
        let j = judge(&c);
        assert!(j.candidates.is_empty());
        assert_eq!(j.candidates_strict, set(&["pss-mock"]));
        assert_eq!(j.declared_pairs(), 4, "a candidate report subtracts nothing");
    }

    #[test]
    fn a_yaml_only_member_has_source_under_s411s_rule_and_none_under_the_narrower_one() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let write = |rel: &str, text: &str| {
            let path = tmp.path().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        write("svc/app.py", "import requests\n");
        write("mock/openapi.yaml", "openapi: 3.0.1\n");
        write("docsonly/docs/tool.py", "print(1)\n");
        let members = set(&["svc", "mock", "docsonly"]);
        let config = ConfigCorpus::discover(tmp.path());
        assert_eq!(runnable_members(tmp.path(), &members, &config), set(&["mock", "svc"]));
        assert_eq!(
            members_with_source(tmp.path(), &members, &config, |p| p.query("invocations").is_some()),
            set(&["svc"])
        );
    }

    #[test]
    fn a_non_runnable_holder_is_subtracted() {
        let mut c = half_one_fixture();
        c.runnable.remove("notif");
        let j = judge(&c);
        assert_eq!(j.declared_pairs(), 3);
        assert_eq!(j.declared.pairs_of(Holder::NotRunnable).len(), 1);
    }

    // ── Half 2 ──────────────────────────────────────────────────────────────

    #[test]
    fn placeholders_and_composition_use_the_rows_own_evidence() {
        assert_eq!(keys_of("PUT ${pecserver.uri-x:/d}/y/${a.b}"), set(&["pecserver.urix", "a.b"]));
        let mut ev = BTreeMap::new();
        ev.insert("pecserver.urix".to_string(), set(&["/domain/{d}/user/{u}"]));
        assert_eq!(
            compose("put ${pecserver.uri-x}", &ev),
            Ok(("PUT".into(), "/domain/{d}/user/{u}".into()))
        );
        ev.insert("a.b".to_string(), set(&["/one", "/two"]));
        assert_eq!(
            compose("GET ${a.b}", &ev),
            Err(NotComposed::DivergentEvidence("a.b".into()))
        );
        assert_eq!(
            compose("GET ${c.d}", &ev),
            Err(NotComposed::MissingEvidence("c.d".into()))
        );
        assert_eq!(compose("GET /literal/{id}", &BTreeMap::new()), Ok(("GET".into(), "/literal/{id}".into())));
        // The product's key reading, not a copy of it: whitespace is trimmed, a
        // repeated key substitutes everywhere, a nested span invents no key.
        assert_eq!(
            compose("GET ${ pecserver.uri-x }/z${pecserver.urix}", &ev),
            Ok(("GET".into(), "/domain/{d}/user/{u}/z/domain/{d}/user/{u}".into()))
        );
        assert_eq!(compose("GET ${a${b}}", &ev), Err(NotComposed::Malformed));
    }

    #[test]
    fn the_ledger_join_needs_one_target_with_exactly_the_rows_keys() {
        let targets = vec!["PUT ${p.a}".to_string(), "GET ${p.b}".to_string(), "GET /lit".to_string()];
        assert_eq!(join_ledger(&targets, &set(&["p.a"])), Ok("PUT ${p.a}"));
        assert_eq!(join_ledger(&targets, &BTreeSet::new()), Ok("GET /lit"));
        assert_eq!(join_ledger(&targets, &set(&["p.z"])), Err(NotComposed::NoLedgerTarget));
        let spaced = vec!["GET ${ p-a }".to_string()];
        assert_eq!(join_ledger(&spaced, &set(&["pa"])), Ok("GET ${ p-a }"), "keys read as the product reads them");
        let twice = vec!["PUT ${p.a}".to_string(), "DELETE ${p.a}".to_string()];
        assert_eq!(join_ledger(&twice, &set(&["p.a"])), Err(NotComposed::LedgerJoinNotUnique));
    }

    #[test]
    fn a_url_path_is_taken_from_a_literal_authority_only() {
        assert_eq!(url_path("https://tinvpecprovis01w:8443/prov").as_deref(), Some("/prov"));
        assert_eq!(url_path("http://localhost:8082").as_deref(), Some(""));
        assert_eq!(url_path("http://h/api/?q=1").as_deref(), Some("/api"));
        assert_eq!(url_path("/v1/notification/send"), None, "a path is not a URL");
        assert_eq!(url_path("http://${HOST}/api"), None, "a templated authority proves nothing");
        assert_eq!(url_path("_#PLACEHOLDER"), None);
    }

    #[test]
    fn an_environment_variable_overrides_its_relaxed_property_and_nothing_else() {
        let base = canonical_key("pec-server.base-url");
        assert!(overlay_overrides(&canonical_key("envFrom.PECSERVER_BASEURL"), &base));
        assert!(overlay_overrides(&canonical_key("config.pec-server.base-url"), &base));
        assert!(overlay_overrides(&canonical_key("pec-server.base-url"), &base), "the key itself");
        // Near misses, one token away.
        assert!(!overlay_overrides(&canonical_key("envFrom.PECSERVER_BASEURLS"), &base));
        assert!(!overlay_overrides(&canonical_key("envFrom.NOTIFICATIONGATEWAY_API_BASEURL"), &base));
        assert!(!overlay_overrides(&canonical_key("envFrom.SERVER_BASEURL"), &base));
    }

    fn facade_scalars() -> Vec<Scalar> {
        vec![
            scalar("facade", "pec-server.base-url", "http://localhost:8082", SourceSet::Application,
                "facade/src/main/resources/application.yml"),
            scalar("facade", "pec-server.uri-get", "/domain/{d}", SourceSet::Application,
                "facade/src/main/resources/application.yml"),
            scalar("facade", "envFrom.PECSERVER_BASEURL", "https://t:8443/prov", SourceSet::Deploy,
                "facade/deploy-coll/values.yaml"),
            scalar("facade", "envFrom.PECSERVER_BASEURL", "https://s:8443/prov", SourceSet::Deploy,
                "facade/deploy-svil/values.yaml"),
            scalar("facade", "envFrom.OTHER_BASEURL", "https://x/elsewhere", SourceSet::Deploy,
                "facade/deploy-svil/values.yaml"),
        ]
    }

    fn split_sources(s: &[Scalar]) -> (Vec<&Scalar>, Vec<&Scalar>) {
        s.iter().partition(|s| s.source == SourceSet::Application)
    }

    #[test]
    fn an_overlay_overrides_the_application_base_path_when_every_overlay_agrees() {
        let s = facade_scalars();
        let (app, overlays) = split_sources(&s);
        let keys = set(&["pecserver.uriget"]);
        assert_eq!(
            base_reading(&keys, &app, &overlays, true),
            BaseReading::One {
                path: "/prov".into(),
                files: vec!["facade/deploy-coll/values.yaml".into(), "facade/deploy-svil/values.yaml".into()],
                overlay: true,
            }
        );
        assert_eq!(
            base_reading(&keys, &app, &overlays, false),
            BaseReading::One {
                path: String::new(),
                files: vec!["facade/src/main/resources/application.yml".into()],
                overlay: false,
            }
        );
        assert_eq!(base_reading(&set(&["other.uri"]), &app, &overlays, true), BaseReading::NoKey);
    }

    #[test]
    fn an_override_is_decided_by_key_and_a_pathless_one_proves_no_base() {
        let s = facade_scalars();
        let (app, _) = split_sources(&s);
        let keys = set(&["pecserver.uriget"]);
        // A lone overriding placeholder: the application path must not come back.
        let placeholder = scalar("facade", "envFrom.PECSERVER_BASEURL", "${PECSERVER_URL}",
            SourceSet::Deploy, "facade/deploy-x/values.yaml");
        assert_eq!(
            base_reading(&keys, &app, &[&placeholder], true),
            BaseReading::OverlayWithoutPath(vec!["facade/deploy-x/values.yaml".into()])
        );
        let templated = scalar("facade", "envFrom.PECSERVER_BASEURL", "https://${PSS_HOST}/api",
            SourceSet::Deploy, "facade/deploy-y/values.yaml");
        assert!(matches!(
            base_reading(&keys, &app, &[&templated], true),
            BaseReading::OverlayWithoutPath(_)
        ));
        // Beside overlays that do commit a path, a pathless one adds nothing —
        // the estate's `values_TEMPLATE.yaml` shape.
        let literal = scalar("facade", "envFrom.PECSERVER_BASEURL", "https://t:8443/prov",
            SourceSet::Deploy, "facade/deploy-coll/values.yaml");
        assert!(matches!(
            base_reading(&keys, &app, &[&placeholder, &literal], true),
            BaseReading::One { ref path, overlay: true, .. } if path == "/prov"
        ));
        // Only a URL-valued application key is a base-url key.
        let path_only = scalar("m", "ns.uri", "/p", SourceSet::Application, "a.yml");
        assert_eq!(base_reading(&set(&["ns.uri"]), &[&path_only], &[], true), BaseReading::NoKey);
        let ext = keys_set_prov();
        assert!(matches!(
            judge_call("GET", "/domain/{x}", &BaseReading::OverlayWithoutPath(vec!["o".into()]), &[("pss.yaml", &ext)]),
            CallClass::OverlayWithoutPath(_)
        ));
    }

    fn keys_set_prov() -> BTreeSet<OpKey> {
        keys(&[("GET", "/prov/domain/{d}")])
    }

    #[test]
    fn overlays_that_disagree_on_the_base_path_are_refused() {
        let mut s = facade_scalars();
        s.push(scalar("facade", "envFrom.PECSERVER_BASEURL", "https://p:8443/pss", SourceSet::Deploy,
            "facade/deploy-prod/values.yaml"));
        let (app, overlays) = split_sources(&s);
        assert!(matches!(
            base_reading(&set(&["pecserver.uriget"]), &app, &overlays, true),
            BaseReading::Disagree(rows) if rows.len() == 3
        ));
    }

    #[test]
    fn a_call_binds_only_on_an_exact_path_under_the_committed_base_never_a_suffix() {
        let pss = keys(&[("GET", "/prov/domain/{d}"), ("POST", "/v1/notification/send")]);
        let ext = [("pss.yaml", &pss)];
        let prov = BaseReading::One { path: "/prov".into(), files: vec!["o".into()], overlay: true };
        let empty = BaseReading::One { path: String::new(), files: vec!["a".into()], overlay: false };
        let api = BaseReading::One { path: "/api".into(), files: vec!["h".into()], overlay: true };
        assert!(matches!(judge_call("GET", "/domain/{x}", &prov, &ext), CallClass::Exact { .. }));
        // Under application config alone the same call is only a suffix match.
        assert!(matches!(judge_call("GET", "/domain/{x}", &empty, &ext), CallClass::SuffixOnly { .. }));
        // The method is part of the key.
        assert!(matches!(judge_call("DELETE", "/domain/{x}", &prov, &ext), CallClass::NoMatch));
        // A base path the spec does not carry overshoots rather than binds.
        assert!(matches!(
            judge_call("POST", "/v1/notification/send", &api, &ext),
            CallClass::Overshoot { .. }
        ));
        assert!(matches!(judge_call("POST", "/v1/notification/send", &empty, &ext), CallClass::Exact { .. }));
        // A near miss one segment off the suffix boundary is no suffix at all.
        assert!(matches!(judge_call("GET", "/main/{x}", &empty, &ext), CallClass::NoMatch));
        assert!(matches!(judge_call("GET", "/domain/{x}", &prov, &[]), CallClass::NoExternal));
        assert!(matches!(
            judge_call("GET", "/domain/{x}", &BaseReading::NoKey, &ext),
            CallClass::NoBaseKey { p_exact: false }
        ));
    }

    #[test]
    fn half_two_counts_exact_rows_of_the_members_own_external_only() {
        let mut c = half_one_fixture();
        c.scalars = facade_scalars();
        c.invocation_rows = vec![
            InvocationRow {
                member: "facade".into(),
                symbol: "s1".into(),
                evidence: [("pecserver.uriget".to_string(), set(&["/1"]))].into_iter().collect(),
            },
            // webmail holds PSS too, but has no base key: never exact.
            InvocationRow {
                member: "webmail".into(),
                symbol: "s2".into(),
                evidence: [("x.uri".to_string(), set(&["/1"]))].into_iter().collect(),
            },
            // agg vendors nothing.
            InvocationRow { member: "agg".into(), symbol: "s3".into(), evidence: BTreeMap::new() },
            // webmail's call lands on its IDENTITY document (agg's own spec), which
            // is a declared contract to a member, never an external.
            InvocationRow {
                member: "webmail".into(),
                symbol: "s4".into(),
                evidence: [("agg.uri".to_string(), set(&["/v1/b"]))].into_iter().collect(),
            },
            // docs holds a PSS copy, but a declared documentation holder vendors
            // no external a call can bind to.
            InvocationRow {
                member: "docs".into(),
                symbol: "s5".into(),
                evidence: [("pss.uri".to_string(), set(&["/1"]))].into_iter().collect(),
            },
        ];
        c.scalars.push(scalar("webmail", "agg.base-url", "http://agg:8080", SourceSet::Application, "w/a.yml"));
        c.scalars.push(scalar("docs", "pss.base-url", "http://pss/prov", SourceSet::Application, "d/a.yml"));
        c.ledger.insert(("webmail".into(), "s4".into()), vec!["GET ${agg.uri}".into()]);
        c.ledger.insert(("docs".into(), "s5".into()), vec!["GET ${pss.uri}".into()]);
        // Another member's overlay on the SAME key: it must never reach
        // facade's reading, or facade's overlays would disagree and its exact
        // row would be refused.
        c.scalars.push(scalar("webmail", "envFrom.PECSERVER_BASEURL", "https://w:8443/other",
            SourceSet::Deploy, "webmail/values.yaml"));
        c.ledger.insert(("facade".into(), "s1".into()), vec!["GET ${pecserver.uri-get}".into()]);
        c.ledger.insert(("webmail".into(), "s2".into()), vec!["GET ${x.uri}".into()]);
        c.ledger.insert(("agg".into(), "s3".into()), vec!["GET /prov/1".into()]);
        let j = judge(&c);
        let classes: Vec<&str> = j.calls.iter().map(|c| c.class.label()).collect();
        assert_eq!(
            classes,
            vec!["EXACT UNDER A COMMITTED BASE PATH", "NO COMMITTED BASE-URL KEY",
                "THE MEMBER HOLDS NO NAMED EXTERNAL", "NO MATCH",
                "THE MEMBER HOLDS NO NAMED EXTERNAL"]
        );
        assert_eq!(j.invocation_exact(), 1);
        assert_eq!(j.invocation_exact_app_only(), 0, "without the overlay it is suffix inference");
    }

    // ── Sorting product rows into the populations ──────────────────────────

    fn row(
        member: &str,
        symbol: &str,
        intake: BridgeIntake,
        relation: &str,
        state: CoverageState,
        candidates: Option<(ProviderDisposition, &[&str], u64)>,
        provenance: Provenance,
    ) -> ReferenceCoverage {
        let endpoint = |m: &str| logos_core::federation::BridgeEndpoint {
            member: m.into(),
            symbol: logos_core::model::LogosSymbol::parse(symbol).expect("fixture symbol parses"),
        };
        ReferenceCoverage {
            relation: relation.into(),
            from: endpoint(member),
            bucket: state.bucket(),
            state,
            to: None,
            intake,
            candidates: candidates.map(|(disposition, members, omitted)| {
                logos_core::federation::ProviderCandidates {
                    disposition,
                    providers: members.iter().map(|m| endpoint(m)).collect(),
                    total: members.len() as u64 + omitted,
                    omitted,
                    summary: String::new(),
                }
            }),
            provenance,
        }
    }

    const OP: &str = "logos . . . src/main/resources/openapi/`v1.yaml`/v1-a#get#";
    const SITE: &str = "logos . . . src/`Client.java`/Client#get().";
    fn unbound(reason: UnboundReason) -> CoverageState {
        CoverageState::Unbound { reason }
    }
    fn bound(key: &str, values: &[&str]) -> Provenance {
        Provenance::ConfigBound {
            bound: vec![logos_core::resolve::binding::ConfigBound {
                key: key.into(),
                source: logos_core::resolve::binding::KeySource::Placeholder,
                values: values
                    .iter()
                    .map(|v| logos_core::resolve::binding::ProfiledValue {
                        value: (*v).into(),
                        profiles: Vec::new(),
                        unprofiled: true,
                        sources: vec!["application.yml".into()],
                    })
                    .collect(),
            }],
        }
    }

    #[test]
    fn a_contract_surface_tie_is_a_provision_and_a_third_half_row() {
        let tie = row("agg", OP, BridgeIntake::ContractSurface, "route",
            unbound(UnboundReason::Ambiguous),
            Some((ProviderDisposition::TiedBetween, &["agg", "api"], 0)), Provenance::Literal);
        let RowUse::ContractSurface { provision, tie: Some(t) } = row_use(&tie) else {
            panic!("a tie is a contract-surface provision with a tie row");
        };
        assert_eq!(provision, OpProvision::Tied { members: vec!["agg".into(), "api".into()], truncated: false });
        assert_eq!(t.document, "src/main/resources/openapi/v1.yaml");
        assert!(!t.truncated);
        // A truncated list says so, or a hidden holder reads as not tied.
        let cut = row("agg", OP, BridgeIntake::ContractSurface, "route",
            unbound(UnboundReason::Ambiguous),
            Some((ProviderDisposition::TiedBetween, &["x", "y"], 1)), Provenance::Literal);
        assert!(matches!(row_use(&cut), RowUse::ContractSurface { tie: Some(TieRow { truncated: true, .. }), .. }));
        // A bound fan-out names a set but is no tie.
        let fan = row("agg", OP, BridgeIntake::ContractSurface, "route", CoverageState::Bound,
            Some((ProviderDisposition::BoundTo, &["a", "b"], 0)), Provenance::Literal);
        assert_eq!(row_use(&fan), RowUse::ContractSurface { provision: OpProvision::Other, tie: None });
    }

    #[test]
    fn only_a_no_provider_route_invocation_row_enters_the_second_half() {
        let hit = row("facade", SITE, BridgeIntake::Invocation, "route",
            unbound(UnboundReason::NoProviderInWorkspace), None,
            bound("pecserver.uriget", &["/domain/{d}", "/other"]));
        let RowUse::Invocation(r) = row_use(&hit) else { panic!("a no-provider route row counts") };
        assert_eq!(r.evidence.get("pecserver.uriget").map(BTreeSet::len), Some(2), "every value is kept");
        // Near misses: another relation, another reason, another intake.
        let broker = row("facade", SITE, BridgeIntake::Invocation, "broker-topic",
            unbound(UnboundReason::NoProviderInWorkspace), None, Provenance::Literal);
        assert_eq!(row_use(&broker), RowUse::NonRestNoProvider("broker-topic".into()));
        let tied = row("facade", SITE, BridgeIntake::Invocation, "route",
            unbound(UnboundReason::Ambiguous), None, Provenance::Literal);
        let declared = row("facade", OP, BridgeIntake::ContractSurface, "route",
            unbound(UnboundReason::NoProviderInWorkspace), None, Provenance::Literal);
        assert_eq!(row_use(&tied), RowUse::Outside);
        assert!(matches!(row_use(&declared), RowUse::ContractSurface { tie: None, .. }));
    }

    #[test]
    fn the_ledger_keeps_http_call_sites_only() {
        assert!(is_http_consumer(ArtifactRelation::HttpClientCall));
        // `Route` is a declaration, in no bridge namespace. The role conjunct is
        // redundant today — `HttpClientCall` is the HTTP namespace's only member
        // — and is kept so a future HTTP provider arm cannot enter the ledger.
        assert!(!is_http_consumer(ArtifactRelation::Route), "a route declaration is no call site");
        assert!(!is_http_consumer(ArtifactRelation::BrokerPublish));
        assert!(!is_http_consumer(ArtifactRelation::GrpcCall));
    }

    // ── Half 3 ──────────────────────────────────────────────────────────────

    #[test]
    fn main_resources_means_the_members_or_a_modules_own() {
        assert!(under_main_resources("src/main/resources/openapi/v1.yaml"));
        assert!(under_main_resources("api/src/main/resources/openapi/v1.yaml"));
        assert!(!under_main_resources("src/test/resources/openapi/v1.yaml"));
        assert!(!under_main_resources("mailbox-aggregator.yaml"));
        // Near misses one character off a segment boundary.
        assert!(!under_main_resources("mysrc/main/resources/v1.yaml"));
        assert!(!under_main_resources("src/main/resourcesx/v1.yaml"));
        // A main-resources directory nested inside a documentation or test tree.
        assert!(!under_main_resources("docs/api/src/main/resources/openapi/v1.yaml"));
        assert!(!under_main_resources("src/test/resources/fixtures/src/main/resources/openapi.yaml"));
    }

    #[test]
    fn an_own_spec_tie_needs_the_holder_tied_and_the_spec_under_main_resources() {
        let tie = |holder: &str, document: &str, tied: &[&str], truncated| TieRow {
            holder: holder.into(),
            document: document.into(),
            tied: tied.iter().map(|s| (*s).to_string()).collect(),
            truncated,
        };
        let own = "src/main/resources/openapi/v1.yaml";
        assert_eq!(judge_tie(&tie("agg", own, &["agg", "api"], false)), TieClass::OwnSpec);
        assert_eq!(
            judge_tie(&tie("agg", "copy.yaml", &["agg", "api"], false)),
            TieClass::HolderCandidateNotMainResources
        );
        assert_eq!(judge_tie(&tie("webmail", "agg.yaml", &["agg", "api"], false)), TieClass::HolderNotCandidate);
        assert_eq!(judge_tie(&tie("x", own, &["a", "b"], false)), TieClass::HolderNotCandidate);
        assert_eq!(judge_tie(&tie("x", own, &["a"], true)), TieClass::Undecided);
    }

    #[test]
    fn verdicts_hold_at_the_floor_and_fall_one_below() {
        assert_eq!(verdict(45, 45), "HOLDS");
        assert_eq!(verdict(44, 45), "FALSIFIED");
        assert_eq!(
            verdict_line(HALF_OWN_SPEC, 43, 45),
            "Own-spec ties: measured 43, floor 45 — FALSIFIED"
        );
    }
}
