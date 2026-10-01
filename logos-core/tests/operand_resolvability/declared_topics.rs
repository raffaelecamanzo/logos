//! **S-488's blocking measurement gate** — how many member–topic pairs does a
//! member's committed `src/main` configuration declare that the product does
//! **not** already observe as a `Producer` or `Consumer`?
//!
//! [S-411]'s topic half held at 13 shared topics, but it was measured before
//! [CR-131] cluster A shipped, against a captured population of 16
//! `@KafkaListener` subscribes and zero publishes. [CR-157] carries that half
//! forward gated on what it adds **today**: a declaration counts only when the
//! product's own promoted topic graph — the merged-main binary, cluster A
//! included, over a private estate copy — shows no producer or consumer of that
//! topic in that member. Below the floor the declared-topic intake ([S-489]–
//! [S-491]) is never built.
//!
//! The floor ([`NET_NEW_PAIR_FLOOR`]), the decisive population and every term's
//! instrument are declared in [`declared_topics_floor.txt`], committed before this
//! module existed; [`the_floor_and_the_population_are_the_declared_ones`] parses
//! both out of it, so neither can be edited to clear a run without the
//! declaration being edited too.
//!
//! # Every instrument is called, never copied
//!
//! - **Declarations** are [S-411]'s application reader: the shipped
//!   `ConfigCorpus` this binary's [`crate::measurement`] already discovered,
//!   folded through [S-411]'s [`collect_values`] under [S-411]'s own
//!   [`Provenance`] and overlay tag. `config_declared_coupling.rs` is not edited.
//! - **Value resolution** is [S-410]'s function,
//!   [`topic_identity`](logos_core::resolve::broker_identity::topic_identity),
//!   over a lookup scoped to the declaring source's module ([`ModuleScoped`]) —
//!   one placeholder hop, a refusal in the shipped `ValueRefusal` words.
//! - **Captured and observed** are the shipped
//!   [`workspace_topics`](logos_core::federation::topics::workspace_topics)
//!   read-model, through member engines over the re-indexed copy.
//! - **The B3 split** reads the shipped
//!   [`cross_service_coverage`](logos_core::federation::cross_service_coverage)
//!   broker rows for the member, refusals with their own reason tokens.
//! - **The reconciliation** reads [S-411]'s own [`judgement`] over the same copy.
//!
//! The only rules this module owns are the ones [CR-157] §3.2 A states and the
//! shipped code has no word for: [`key_shape`] (what a topic-named key is),
//! [`Reading::admits`] (what `src/main` means), [`classify`] (declared-only /
//! observed / net-new), [`site_state`] (the B3 split) and [`mechanism`] (why a
//! declaration differs from [S-411]'s). Each is one function and each is pinned
//! by always-run fixtures.
//!
//! # A private copy only
//!
//! The gate reads through member **engines**, so it refuses a store this binary
//! would migrate ([`refuse_a_store_that_would_migrate`], S-456's guard), and it
//! is pointed at `~/source/.estate-copies/pec-services-S-488`, never at the live
//! estate. Like every `LOGOS_REF_WORKSPACE` harness it is **invisible to
//! `gate.sh` and to CI**: both run without the variable, so only this module's
//! fixtures run there and the estate arm prints VOID.
//!
//! [CR-131]: ../../../docs/requests/CR-131-cross-service-coupling-from-committed-configuration.md
//! [CR-157]: ../../../docs/requests/CR-157-config-declared-topics-the-half-that-held.md
//! [S-410]: ../../../docs/planning/journal.md#s-410-topic-identity-is-the-committed-configured-value-so-a-streams-publish-meets-a-subscribe
//! [S-411]: ../../../docs/planning/journal.md#s-411-measure-config-declared-coupling-over-the-reference-estate
//! [S-489]: ../../../docs/planning/journal.md#s-489-the-declared-topic-intake-reported-apart-from-observed-broker-edges
//! [S-491]: ../../../docs/planning/journal.md#s-491-declared-topics-on-the-web-service-map
//! [`declared_topics_floor.txt`]: ./declared_topics_floor.txt
//! [`refuse_a_store_that_would_migrate`]: super::vendored_spec_contracts::refuse_a_store_that_would_migrate

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use logos_core::federation::topics::workspace_topics;
use logos_core::federation::{
    cross_service_coverage, discover, CoverageState, EngineRegistry, Member, RegistryMode,
    UnboundReason,
};
use logos_core::graph_store::ConfigDefinition;
use logos_core::model::BridgeNamespace;
use logos_core::resolve::binding::{Provenance as ValueProvenance, ValueRefusal};
use logos_core::resolve::broker_identity::{topic_identity, TopicIdentity};
use logos_core::Engine;

use super::config_declared_coupling::{
    collect_values, judgement, Judgement, Provenance, Scalar, SourceSet, TopicOutcome,
};
use super::configuration_agreement::{ConfigCorpus, ConfigLookup, CorpusLookup};
use super::identity;
use super::vendored_spec_contracts::refuse_a_store_that_would_migrate;

/// **The floor and the population, as declared before the run.**
///
/// `include_str!` rather than a path reference, following
/// [`super::config_declared_coupling::DECLARED_FLOOR`]: a file the build embeds
/// cannot be deleted or renamed without breaking compilation.
pub const DECLARED_FLOOR: &str = include_str!("declared_topics_floor.txt");

/// The recorded verdict, reproduced by the run and printed by it.
pub const RECORDED_FINDING: &str = include_str!("declared_topics_finding.txt");

/// The net-new pair floor, parsed out of [`DECLARED_FLOOR`] by
/// [`the_floor_and_the_population_are_the_declared_ones`]. The stakeholder's
/// figure, [CR-157] B1.
///
/// [CR-157]: ../../../docs/requests/CR-157-config-declared-topics-the-half-that-held.md
pub const NET_NEW_PAIR_FLOOR: usize = 3;

/// [S-411]'s recorded topic half — the population [CR-157] B3 reconciles
/// against. Read from S-411's own pins, never restated.
///
/// [CR-157]: ../../../docs/requests/CR-157-config-declared-topics-the-half-that-held.md
/// [S-411]: ../../../docs/planning/journal.md#s-411-measure-config-declared-coupling-over-the-reference-estate
pub const S411_TOPICS: usize = super::config_declared_coupling::RECORDED_SHARED_TOPICS;
pub const S411_DECLARATIONS: usize =
    super::config_declared_coupling::RECORDED_DECLARATION_KEY_SPLIT.0;

/// The member whose `cdk.json` [CR-157] §3.3 names as a blind spot.
///
/// [CR-157]: ../../../docs/requests/CR-157-config-declared-topics-the-half-that-held.md
pub const CDK_BLIND_SPOT: &str = "filters-ingestion-cdk/cdk.json";

// ── What a topic-named key is (A1) ──────────────────────────────────────────

/// The three key shapes [CR-157] A1 admits, in the order a key is tested.
///
/// [CR-157]: ../../../docs/requests/CR-157-config-declared-topics-the-half-that-held.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum KeyShape {
    /// `spring.kafka.topics.<k>`.
    SpringKafkaTopics,
    /// The last segment IS `topic` / `topics` (`filteraggregate.topic`).
    LastSegment,
    /// The last segment ENDS in `topic` / `topics`
    /// (`spring.kafka.template.default-topic` → `defaulttopic`).
    LastSegmentSuffix,
}

impl KeyShape {
    pub fn label(self) -> &'static str {
        match self {
            Self::SpringKafkaTopics => "spring.kafka.topics.*",
            Self::LastSegment => "last segment is topic/topics",
            Self::LastSegmentSuffix => "last segment ends in topic/topics",
        }
    }

    pub const ALL: [Self; 3] = [
        Self::SpringKafkaTopics,
        Self::LastSegment,
        Self::LastSegmentSuffix,
    ];
}

/// The shape of a **canonical** key, or `None` when it does not name a topic.
///
/// Applied to the relaxed-binding canonical form the corpus stores, so
/// `default-topic` arrives as `defaulttopic` and still ends in `topic`.
pub fn key_shape(canonical: &str) -> Option<KeyShape> {
    if canonical.starts_with("spring.kafka.topics.") {
        return Some(KeyShape::SpringKafkaTopics);
    }
    let last = canonical.rsplit('.').next().unwrap_or(canonical);
    if last == "topic" || last == "topics" {
        Some(KeyShape::LastSegment)
    } else if last.ends_with("topic") || last.ends_with("topics") {
        Some(KeyShape::LastSegmentSuffix)
    } else {
        None
    }
}

// ── The readings ────────────────────────────────────────────────────────────

/// The three readings the declaration fixes. Only [`Reading::Decisive`] moves
/// the verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Reading {
    /// The 83 manifest members, `src/main` sources — **the gate**.
    Decisive,
    /// The 84 `.git` directories, `src/main` sources. Never decisive.
    Directory,
    /// The 83 manifest members, `src/main` and `src/test` sources. Never
    /// decisive.
    TestTree,
}

impl Reading {
    pub fn label(self) -> &'static str {
        match self {
            Self::Decisive => "MANIFEST·MAIN (decisive)",
            Self::Directory => "DIRECTORY (84 dirs, src/main)",
            Self::TestTree => "TEST-TREE (manifest, src/main + src/test)",
        }
    }

    pub const ALL: [Self; 3] = [Self::Decisive, Self::Directory, Self::TestTree];

    /// Whether this reading reads a configuration source at this
    /// corpus-relative path. A path segment pair, so `src/maintenance/` is not
    /// `src/main/`.
    pub fn admits(self, path: &str) -> bool {
        let main = path.contains("/src/main/");
        match self {
            Self::Decisive | Self::Directory => main,
            Self::TestTree => main || path.contains("/src/test/"),
        }
    }

    /// The members this reading counts, out of the manifest and directory sets.
    fn members<'a>(
        self,
        manifest: &'a BTreeSet<String>,
        directories: &'a BTreeSet<String>,
    ) -> &'a BTreeSet<String> {
        match self {
            Self::Directory => directories,
            Self::Decisive | Self::TestTree => manifest,
        }
    }
}

/// A configuration source no reading admits — neither `src/main` nor
/// `src/test`. Listed, never counted.
pub fn outside_every_reading(path: &str) -> bool {
    Reading::ALL.iter().all(|r| !r.admits(path))
}

// ── Value resolution (A1) ───────────────────────────────────────────────────

/// A [`CorpusLookup`] pinned to the **declaring source's** module.
///
/// The shipped `topic_identity` builds its resolver with module `""`, which is
/// right for a member-local store and wrong for a whole-estate corpus; this
/// adapter supplies the module instead, so the function itself is called
/// unchanged ([ADR-64]'s within-reach rule, the harness's `CorpusLookup` scope).
///
/// [ADR-64]: ../../../docs/specs/architecture/decisions/ADR-64.md
pub struct ModuleScoped<'a> {
    pub corpus: &'a ConfigCorpus,
    pub module: &'a str,
}

impl ConfigLookup for ModuleScoped<'_> {
    fn definitions(&self, key: &str, _module: &str) -> Vec<ConfigDefinition> {
        CorpusLookup(self.corpus).definitions(key, self.module)
    }
}

/// What one committed value under a topic-named key declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    /// Candidate topics, each with the profile set that proves it.
    Topics(Vec<(String, String)>),
    /// The shipped refusal.
    Refused(ValueRefusal),
}

/// Resolve one declared value as [S-410] resolves a topic operand —
/// [`topic_identity`], called. `overlay` is the declaring source's tag; a hop
/// adds the profiles its `ConfigBound` proves.
///
/// [S-410]: ../../../docs/planning/journal.md#s-410-topic-identity-is-the-committed-configured-value-so-a-streams-publish-meets-a-subscribe
pub fn resolve(value: &str, lookup: &ModuleScoped<'_>, overlay: &str) -> Resolved {
    match topic_identity(value, lookup) {
        TopicIdentity::Literal => {
            Resolved::Topics(vec![(value.trim().to_string(), overlay.to_string())])
        }
        TopicIdentity::Committed { topics, bound } => {
            let mut profiles: BTreeSet<String> = BTreeSet::new();
            for b in &bound {
                for v in &b.values {
                    profiles.extend(v.profiles.iter().cloned());
                    if v.unprofiled {
                        profiles.insert("<none>".to_string());
                    }
                }
            }
            let hop = format!(
                "{overlay} via ${{{}}} [{}]",
                bound
                    .iter()
                    .map(|b| b.key.as_str())
                    .collect::<Vec<_>>()
                    .join(","),
                profiles.into_iter().collect::<Vec<_>>().join(",")
            );
            Resolved::Topics(topics.into_iter().map(|t| (t, hop.clone())).collect())
        }
        TopicIdentity::Unresolved { refusal, .. } => Resolved::Refused(refusal),
    }
}

// ── The classifier (A2, A3) ─────────────────────────────────────────────────

/// One member's promoted topics, as the shipped read-model lists them.
#[derive(Debug, Default, Clone)]
pub struct Observed {
    /// member → the topics it has at least one producer or consumer on,
    /// `#`-guard stripped.
    pub by_member: BTreeMap<String, BTreeSet<String>>,
}

impl Observed {
    /// The topic identities captured anywhere among `members` — A2's
    /// "somewhere in the workspace", under one reading's population.
    pub fn identities(&self, members: &BTreeSet<String>) -> BTreeSet<&str> {
        self.by_member
            .iter()
            .filter(|(m, _)| members.contains(*m))
            .flat_map(|(_, t)| t.iter().map(String::as_str))
            .collect()
    }

    pub fn on(&self, member: &str, topic: &str) -> bool {
        self.by_member
            .get(member)
            .is_some_and(|t| t.contains(topic))
    }

    pub fn has_topics(&self, member: &str) -> bool {
        self.by_member.get(member).is_some_and(|t| !t.is_empty())
    }
}

/// The topic half of a promoted key: a message-schema guard
/// (`orders#com.acme.OrderCreated`) is not part of the topic.
pub fn topic_of(key: &str) -> &str {
    key.split('#').next().unwrap_or(key)
}

/// What a resolved declaration is, against the product.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    /// T is captured and M already has a Producer/Consumer on it.
    Observed,
    /// T is captured and M has none on it — **the headline**.
    NetNew,
    /// No broker site in the reading's population resolves to T (A2).
    DeclaredOnly,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Self::Observed => "already observed (Producer/Consumer on T)",
            Self::NetNew => "NET-NEW",
            Self::DeclaredOnly => "declared-only (no broker site names T)",
        }
    }
}

/// **The gate's classifier, in one function** — A2 then A3.
pub fn classify(
    member: &str,
    topic: &str,
    captured: &BTreeSet<&str>,
    observed: &Observed,
) -> Status {
    if !captured.contains(topic) {
        Status::DeclaredOnly
    } else if observed.on(member, topic) {
        Status::Observed
    } else {
        Status::NetNew
    }
}

// ── The B3 split ────────────────────────────────────────────────────────────

/// Where a net-new pair's member stands with respect to broker sites.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum SiteState {
    /// No broker row and no promoted topic at all.
    ConfigOnly,
    /// At least one broker row the product refused, with its reasons.
    SiteRefused(Vec<String>),
    /// Broker sites, none refused, none on T.
    SiteUncaptured,
}

impl SiteState {
    pub fn label(&self) -> &'static str {
        match self {
            Self::ConfigOnly => "config-only",
            Self::SiteRefused(_) => "site-refused",
            Self::SiteUncaptured => "site-uncaptured",
        }
    }
}

/// The B3 split for one member, from its broker rows and promoted topics.
/// Member grain: a refused row carries no operand to tie it to T.
pub fn site_state(broker_rows: usize, has_promoted_topics: bool, refusals: &[String]) -> SiteState {
    if broker_rows == 0 && !has_promoted_topics {
        SiteState::ConfigOnly
    } else if !refusals.is_empty() {
        SiteState::SiteRefused(refusals.to_vec())
    } else {
        SiteState::SiteUncaptured
    }
}

/// The refusal one broker coverage row carries, or `None` when the product
/// did not refuse it. A keyed publish with no subscriber is unbound
/// `no-provider-in-workspace`, and that is an observed site, not a refusal.
pub fn refusal_of(state: &CoverageState, provenance: &ValueProvenance) -> Option<String> {
    if let ValueProvenance::ConfigUnresolved { refusal, .. } = provenance {
        return Some(format!("config-unresolved: {}", refusal.label()));
    }
    match state {
        CoverageState::Unbound { reason } if *reason != UnboundReason::NoProviderInWorkspace => {
            Some(reason.as_str().to_string())
        }
        _ => None,
    }
}

// ── The measurement ─────────────────────────────────────────────────────────

/// One resolved declaration (M declares T), with its A4 provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declaration {
    pub member: String,
    pub topic: String,
    pub key: String,
    pub file: String,
    pub profile: String,
    pub shape: KeyShape,
    /// `true` when T came through a placeholder hop rather than literally.
    pub hopped: bool,
}

/// One refused declaration.
#[derive(Debug, Clone)]
pub struct RefusedDeclaration {
    pub member: String,
    pub key: String,
    pub file: String,
    pub value: String,
    pub reason: ValueRefusal,
}

/// Everything one reading measures.
#[derive(Debug, Default)]
pub struct ReadingResult {
    pub declarations: Vec<(Declaration, Status)>,
    pub refused: Vec<RefusedDeclaration>,
    pub sources_read: usize,
    pub members_declaring: BTreeSet<String>,
    pub captured: BTreeSet<String>,
}

impl ReadingResult {
    /// Distinct (M, T) pairs with one status — **the grain the floor is read
    /// at**.
    pub fn pairs(&self, status: Status) -> BTreeMap<(String, String), Vec<&Declaration>> {
        let mut out: BTreeMap<(String, String), Vec<&Declaration>> = BTreeMap::new();
        for (d, s) in &self.declarations {
            if *s == status {
                out.entry((d.member.clone(), d.topic.clone()))
                    .or_default()
                    .push(d);
            }
        }
        out
    }

    pub fn net_new(&self) -> usize {
        self.pairs(Status::NetNew).len()
    }

    /// Declarations (rows, not pairs) by key shape — over every resolved
    /// declaration, whatever its status.
    pub fn key_shapes(&self) -> BTreeMap<KeyShape, usize> {
        let mut out = BTreeMap::new();
        for (d, _) in &self.declarations {
            *out.entry(d.shape).or_default() += 1;
        }
        out
    }
}

/// Measure one reading from S-411's scalars.
pub fn measure_reading(
    reading: Reading,
    config: &ConfigCorpus,
    scalars: &[Scalar],
    members: &BTreeSet<String>,
    observed: &Observed,
) -> ReadingResult {
    // The reading's own corpus: a value resolves only against sources this
    // reading reads, so a `src/main` declaration never hops into `src/test`.
    let mut scoped = ConfigCorpus::default();
    scoped.sources = config
        .sources
        .iter()
        .filter(|s| reading.admits(&s.path) && members.contains(member_of(&s.path)))
        .cloned()
        .collect();
    let by_path: BTreeMap<&str, &super::configuration_agreement::ConfigSource> = scoped
        .sources
        .iter()
        .map(|s| (s.path.as_str(), s))
        .collect();

    let captured = observed.identities(members);
    let mut out = ReadingResult {
        sources_read: scoped.sources.len(),
        captured: captured.iter().map(|t| (*t).to_string()).collect(),
        ..ReadingResult::default()
    };
    for s in scalars {
        let Some(source) = by_path.get(s.file.as_str()) else {
            continue;
        };
        let Some(shape) = key_shape(&s.key) else {
            continue;
        };
        if s.value.trim().is_empty() {
            continue;
        }
        let overlay = identity::application_overlay(source.profile.as_deref());
        let lookup = ModuleScoped {
            corpus: &scoped,
            module: &source.module,
        };
        match resolve(&s.value, &lookup, &overlay) {
            Resolved::Refused(reason) => out.refused.push(RefusedDeclaration {
                member: s.member.clone(),
                key: s.key.clone(),
                file: s.file.clone(),
                value: s.value.clone(),
                reason,
            }),
            Resolved::Topics(topics) => {
                for (topic, profile) in topics {
                    let status = classify(&s.member, &topic, &captured, observed);
                    out.members_declaring.insert(s.member.clone());
                    out.declarations.push((
                        Declaration {
                            member: s.member.clone(),
                            hopped: topic != s.value.trim(),
                            topic,
                            key: s.key.clone(),
                            file: s.file.clone(),
                            profile,
                            shape,
                        },
                        status,
                    ));
                }
            }
        }
    }
    out
}

fn member_of(path: &str) -> &str {
    path.split('/').next().unwrap_or("")
}

/// S-411's application reader, called: every application-config scalar of
/// every member in `members`, as [`collect_values`] admits it.
pub fn application_scalars(config: &ConfigCorpus, members: &BTreeSet<String>) -> Vec<Scalar> {
    let mut scalars = Vec::new();
    let mut targets = Vec::new();
    for source in &config.sources {
        let member = member_of(&source.path);
        if !members.contains(member) {
            continue;
        }
        let overlay = identity::application_overlay(source.profile.as_deref());
        collect_values(
            &Provenance {
                member,
                file: &source.path,
                overlay: &overlay,
                source: SourceSet::Application,
            },
            &source.values,
            &mut scalars,
            &mut targets,
        );
    }
    scalars
}

// ── The reconciliation against S-411 ────────────────────────────────────────

/// Why a declaration appears on one side of the S-411 comparison and not the
/// other. [`Mechanism::Unexplained`] fails the run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Mechanism {
    /// S-411 counted the 84 directories; the decisive reading counts the 83
    /// manifest members.
    ManifestPopulation,
    /// S-411 read every application source; CR-157 reads `src/main` only.
    SrcMainOnly,
    /// S-411's key guard was looser (`contains("topic")`) than A1's rule.
    TopicNamedKeyRule,
    /// T is captured by the product today (cluster A) but was not by S-411's
    /// harness capture of 2026-09-15.
    CapturedByProductNotHarness,
    /// T was captured by S-411's harness but no product broker site resolves
    /// to it — the declaration is `declared-only` here.
    CapturedByHarnessNotProduct,
    /// The declaration reaches T through a placeholder hop; S-411 joined raw
    /// committed values only.
    PlaceholderHop,
    /// S-411 counted a topic only when two or more members declared it.
    DeclarerThreshold,
    Unexplained,
}

impl Mechanism {
    pub fn label(self) -> &'static str {
        match self {
            Self::ManifestPopulation => "manifest population (83, not 84 dirs)",
            Self::SrcMainOnly => "src/main restriction",
            Self::TopicNamedKeyRule => "topic-named key rule (A1) stricter than S-411's guard",
            Self::CapturedByProductNotHarness => {
                "captured by the product now, not by S-411's harness"
            }
            Self::CapturedByHarnessNotProduct => {
                "captured by S-411's harness, not by the product (declared-only)"
            }
            Self::PlaceholderHop => "one placeholder hop (S-411 joined raw values)",
            Self::DeclarerThreshold => "S-411's >= 2 declarer threshold",
            Self::Unexplained => "UNEXPLAINED",
        }
    }
}

/// One S-411 declaration, as its evidence records it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct S411Declaration {
    pub member: String,
    pub topic: String,
    pub key: String,
    pub file: String,
}

/// What S-411's run knows about a topic identity: its outcome bucket.
pub type S411Topics = BTreeMap<String, TopicOutcome>;

/// Why an S-411 declaration is **missing** from this reading's captured
/// declarations (`mine` = (M, T) with status Observed or NetNew), or why one
/// of ours is **missing** from S-411 — one function, so the two directions
/// cannot use two different rules.
///
/// `ours` is `Some(&Declaration)` for a pair this run has and S-411 does not;
/// `theirs` is `Some(&S411Declaration)` for the reverse.
pub fn mechanism(
    ours: Option<&Declaration>,
    theirs: Option<&S411Declaration>,
    manifest: &BTreeSet<String>,
    s411_topics: &S411Topics,
    declared_only_here: bool,
) -> Mechanism {
    match (ours, theirs) {
        (None, Some(t)) => {
            if !manifest.contains(&t.member) {
                Mechanism::ManifestPopulation
            } else if !Reading::Decisive.admits(&t.file) {
                Mechanism::SrcMainOnly
            } else if key_shape(&t.key).is_none() {
                Mechanism::TopicNamedKeyRule
            } else if declared_only_here {
                Mechanism::CapturedByHarnessNotProduct
            } else {
                Mechanism::Unexplained
            }
        }
        (Some(d), None) => match s411_topics.get(&d.topic) {
            None => Mechanism::CapturedByProductNotHarness,
            Some(TopicOutcome::OneSideOnly) => Mechanism::DeclarerThreshold,
            Some(TopicOutcome::BothSidesDeclared) if d.hopped => Mechanism::PlaceholderHop,
            Some(_) => Mechanism::Unexplained,
        },
        _ => Mechanism::Unexplained,
    }
}

/// S-411's declarations: one per (declarer, both-sides-declared identity), as
/// its `evidence` records them — the population its finding counts as 36.
pub fn s411_declarations(j: &Judgement) -> Vec<S411Declaration> {
    let mut out = Vec::new();
    for t in j.topics(TopicOutcome::BothSidesDeclared) {
        let Some(topic) = t.identity.as_deref() else {
            continue;
        };
        for (member, key, file) in &t.evidence {
            out.push(S411Declaration {
                member: member.clone(),
                topic: topic.to_string(),
                key: key.clone(),
                file: file.clone(),
            });
        }
    }
    out.sort();
    out
}

/// The S-411 identities and their outcome, under its decisive RESOLVED reading.
pub fn s411_topics(j: &Judgement) -> S411Topics {
    j.topics_resolved
        .iter()
        .filter_map(|t| Some((t.identity.clone()?, t.outcome)))
        .collect()
}

/// One row of the reconciliation, each side's difference named.
#[derive(Debug, Clone)]
pub struct Difference {
    pub member: String,
    pub topic: String,
    pub side: &'static str,
    pub mechanism: Mechanism,
}

/// Reconcile the decisive reading against S-411's declarations at (M, T) grain.
/// Returns (matched pairs, differences).
pub fn reconcile(
    r: &ReadingResult,
    s411: &[S411Declaration],
    manifest: &BTreeSet<String>,
    topics: &S411Topics,
) -> (usize, Vec<Difference>) {
    let ours: BTreeMap<(String, String), &Declaration> = r
        .declarations
        .iter()
        .filter(|(_, s)| *s != Status::DeclaredOnly)
        .map(|(d, _)| ((d.member.clone(), d.topic.clone()), d))
        .collect();
    let declared_only: BTreeSet<(String, String)> = r
        .declarations
        .iter()
        .filter(|(_, s)| *s == Status::DeclaredOnly)
        .map(|(d, _)| (d.member.clone(), d.topic.clone()))
        .collect();
    let theirs: BTreeMap<(String, String), &S411Declaration> = s411
        .iter()
        .map(|d| ((d.member.clone(), d.topic.clone()), d))
        .collect();

    let mut matched = 0;
    let mut diffs = Vec::new();
    for (pair, t) in &theirs {
        if ours.contains_key(pair) {
            matched += 1;
            continue;
        }
        diffs.push(Difference {
            member: pair.0.clone(),
            topic: pair.1.clone(),
            side: "in S-411, not here",
            mechanism: mechanism(
                None,
                Some(t),
                manifest,
                topics,
                declared_only.contains(pair),
            ),
        });
    }
    for (pair, d) in &ours {
        if theirs.contains_key(pair) {
            continue;
        }
        diffs.push(Difference {
            member: pair.0.clone(),
            topic: pair.1.clone(),
            side: "here, not in S-411",
            mechanism: mechanism(Some(d), None, manifest, topics, false),
        });
    }
    (matched, diffs)
}

// ── The verdict ─────────────────────────────────────────────────────────────

/// **The verdict, in one place.** HOLDS iff the decisive reading reaches the
/// floor; the other two readings never move it.
pub fn verdict(decisive: usize) -> &'static str {
    if decisive >= NET_NEW_PAIR_FLOOR {
        "HOLDS"
    } else {
        "FALSIFIED"
    }
}

/// The headline line, or VOID where no estate was measured ([NFR-CC-04]).
///
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
pub fn headline(decisive: Option<usize>) -> String {
    match decisive {
        None => "VOID: set LOGOS_REF_WORKSPACE=<path to a private copy of the reference \
                 workspace> to run the S-488 net-new declared-topic gate. A run that sees no \
                 estate reports VOID, never zero (see declared_topics_floor.txt)."
            .to_string(),
        Some(n) => format!(
            "VERDICT: net-new member-topic pairs on the decisive MANIFEST·MAIN reading {n} \
             against a floor of {NET_NEW_PAIR_FLOOR} declared before the run  =>  {}",
            verdict(n)
        ),
    }
}

/// The manifest members listed in the declaration, between its two markers.
pub fn declared_members() -> BTreeSet<String> {
    let begin = DECLARED_FLOOR
        .lines()
        .position(|l| l.starts_with("MANIFEST MEMBERS — BEGIN"))
        .expect("the declaration carries a BEGIN marker");
    DECLARED_FLOOR
        .lines()
        .skip(begin + 1)
        .take_while(|l| !l.starts_with("MANIFEST MEMBERS — END"))
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

// ── The estate read ─────────────────────────────────────────────────────────

/// What the product shows over the copy: promoted topics per member, and its
/// broker coverage rows per member.
pub struct Product {
    pub observed: Observed,
    /// member → (broker rows, refusals with reason and declaration symbol)
    pub broker: BTreeMap<String, (usize, Vec<String>)>,
    /// Members the read-model answered for — the denominator.
    pub members_read: BTreeSet<String>,
}

fn read_product(root: &Path, directories: &BTreeSet<String>) -> (BTreeSet<String>, Product) {
    let mut federation = discover(root)
        .expect("the workspace manifest parses")
        .unwrap_or_else(|| {
            panic!(
                "{} is not a Logos workspace (no logos.workspace.toml) — this gate never enrols one",
                root.display()
            )
        });
    let manifest: BTreeSet<String> = federation.members.iter().map(|m| m.name.clone()).collect();
    // The DIRECTORY reading needs the non-manifest directories' engines too.
    // Promotion and the refusal reasons are member-local, so adding them moves
    // nothing a manifest member reports.
    for name in directories.difference(&manifest) {
        let dir = root.join(name);
        let dir = dir.canonicalize().unwrap_or(dir);
        federation.members.push(Member {
            name: name.clone(),
            root: dir,
        });
    }
    refuse_a_store_that_would_migrate(&federation.members);

    let registry = EngineRegistry::<Engine>::new(federation, RegistryMode::Lazy);
    let answer = registry.answer();
    let mut observed = Observed::default();
    let mut members_read = BTreeSet::new();
    for m in workspace_topics(&answer) {
        members_read.insert(m.member.clone());
        let on: BTreeSet<String> = m
            .topics
            .iter()
            .filter(|t| t.producers + t.consumers > 0)
            .map(|t| topic_of(&t.topic).to_string())
            .collect();
        observed.by_member.insert(m.member, on);
    }
    let coverage = cross_service_coverage(&answer);
    let mut broker: BTreeMap<String, (usize, Vec<String>)> = BTreeMap::new();
    for row in coverage
        .references
        .iter()
        .filter(|r| r.relation == BridgeNamespace::BrokerTopic.relation())
    {
        let entry = broker.entry(row.from.member.clone()).or_default();
        entry.0 += 1;
        if let Some(reason) = refusal_of(&row.state, &row.provenance) {
            entry
                .1
                .push(format!("{reason} at {}", row.from.symbol.as_str()));
        }
    }
    (
        manifest,
        Product {
            observed,
            broker,
            members_read,
        },
    )
}

/// Everything the run measures, so the report and the assertions read one
/// object.
pub struct Run {
    pub manifest: BTreeSet<String>,
    pub directories: BTreeSet<String>,
    pub product: Product,
    pub readings: BTreeMap<Reading, ReadingResult>,
    pub s411: Vec<S411Declaration>,
    pub s411_topics: S411Topics,
    pub matched: usize,
    pub differences: Vec<Difference>,
    /// Application sources no reading admits, with their topic-named scalars.
    pub outside: (usize, Vec<(String, String, String)>),
    pub cdk_exists: bool,
}

fn run(root: &Path) -> Run {
    // S-411's own judgement over the same copy: its directories and its topic
    // half, which the reconciliation reads.
    let j = judgement(root);
    let directories = j.members.clone();
    let (manifest, product) = read_product(root, &directories);
    let config = &crate::measurement(root).config;
    let scalars = application_scalars(config, &directories);

    let mut readings = BTreeMap::new();
    for reading in Reading::ALL {
        let members = reading.members(&manifest, &directories);
        readings.insert(
            reading,
            measure_reading(reading, config, &scalars, members, &product.observed),
        );
    }
    let s411 = s411_declarations(j);
    let s411_topics = s411_topics(j);
    let (matched, differences) = reconcile(
        &readings[&Reading::Decisive],
        &s411,
        &manifest,
        &s411_topics,
    );

    let outside_sources: BTreeSet<&str> = config
        .sources
        .iter()
        .filter(|s| manifest.contains(member_of(&s.path)) && outside_every_reading(&s.path))
        .map(|s| s.path.as_str())
        .collect();
    let outside_decl = scalars
        .iter()
        .filter(|s| outside_sources.contains(s.file.as_str()) && key_shape(&s.key).is_some())
        .map(|s| (s.member.clone(), s.key.clone(), s.file.clone()))
        .collect();

    Run {
        cdk_exists: root.join(CDK_BLIND_SPOT).is_file(),
        manifest,
        directories,
        product,
        readings,
        s411,
        s411_topics,
        matched,
        differences,
        outside: (outside_sources.len(), outside_decl),
    }
}

// ── The report ──────────────────────────────────────────────────────────────

fn report(r: &Run) {
    let d = &r.readings[&Reading::Decisive];
    println!("\n══ S-488 — NET-NEW CONFIG-DECLARED TOPIC PAIRS (CR-157 gate B) ══");
    println!(
        "  manifest members {} · directories {} · member stores the read-model answered {}",
        r.manifest.len(),
        r.directories.len(),
        r.product.members_read.len()
    );
    println!(
        "  captured topic identities (decisive population) {} · members with a promoted topic {}",
        d.captured.len(),
        r.product
            .observed
            .by_member
            .values()
            .filter(|t| !t.is_empty())
            .count()
    );
    println!(
        "  decisive: {} src/main sources read · {} declarations resolved ({} pairs) · {} refused",
        d.sources_read,
        d.declarations.len(),
        d.declarations
            .iter()
            .map(|(x, _)| (&x.member, &x.topic))
            .collect::<BTreeSet<_>>()
            .len(),
        d.refused.len()
    );

    println!(
        "\n  {} PAIRS (decisive) — {}",
        Status::NetNew.label(),
        d.net_new()
    );
    for ((m, t), ev) in d.pairs(Status::NetNew) {
        let state = net_new_state(r, &m);
        println!("    {m} -> {t}   [{}]", state.label());
        for e in ev {
            println!(
                "        key {}  file {}  profile {}",
                e.key, e.file, e.profile
            );
        }
        if let SiteState::SiteRefused(reasons) = &state {
            for reason in reasons {
                println!("        refused site: {reason}");
            }
        }
    }
    let (config_only, refused, uncaptured) = net_new_split(r);
    println!(
        "  split: config-only {config_only} · site-refused {refused} · site-uncaptured \
         {uncaptured}   (member grain: a refused row stores no operand, so it is tied to M, \
         not provably to T)"
    );

    let observed = d.pairs(Status::Observed);
    println!(
        "\n  {} — {} pairs ({} declarations)",
        Status::Observed.label(),
        observed.len(),
        observed.values().map(Vec::len).sum::<usize>()
    );
    for (m, t) in observed.keys() {
        println!("    {m} -> {t}");
    }

    let declared_only = d.pairs(Status::DeclaredOnly);
    println!(
        "\n  {} — {} pairs",
        Status::DeclaredOnly.label(),
        declared_only.len()
    );
    for ((m, t), ev) in &declared_only {
        println!("    {m}: {t:?} via {}", ev[0].key);
    }

    let mut by_reason: BTreeMap<&str, Vec<&RefusedDeclaration>> = BTreeMap::new();
    for x in &d.refused {
        by_reason.entry(x.reason.label()).or_default().push(x);
    }
    println!("\n  REFUSED declarations — {}", d.refused.len());
    for (reason, xs) in &by_reason {
        println!("    {reason}: {}", xs.len());
        for x in xs {
            println!(
                "        {} {} = {:?}  ({})",
                x.member, x.key, x.value, x.file
            );
        }
    }

    println!("\n  KEY-SHAPE SPLIT (decisive, resolved declarations)");
    let shapes = d.key_shapes();
    for shape in KeyShape::ALL {
        println!(
            "    {:<40} {:>4}",
            shape.label(),
            shapes.get(&shape).unwrap_or(&0)
        );
    }

    println!("\n  THE THREE READINGS (only the first decides)");
    for reading in Reading::ALL {
        let x = &r.readings[&reading];
        println!(
            "    {:<44} net-new {:>3}  observed {:>3}  declared-only {:>3}  refused {:>3}  ({})",
            reading.label(),
            x.net_new(),
            x.pairs(Status::Observed).len(),
            x.pairs(Status::DeclaredOnly).len(),
            x.refused.len(),
            verdict(x.net_new())
        );
    }
    for reading in [Reading::Directory, Reading::TestTree] {
        let gained: Vec<_> = r.readings[&reading]
            .pairs(Status::NetNew)
            .into_keys()
            .filter(|p| !d.pairs(Status::NetNew).contains_key(p))
            .collect();
        println!("    {} adds over decisive: {gained:?}", reading.label());
    }

    println!(
        "\n  APPLICATION SOURCES OUTSIDE src/main AND src/test (manifest): {} — counted in no \
         reading; topic-named scalars in them: {}",
        r.outside.0,
        r.outside.1.len()
    );
    for (m, k, f) in &r.outside.1 {
        println!("    {m} {k} ({f})");
    }

    println!(
        "\n  BLIND SPOT: {CDK_BLIND_SPOT} ({}) — not a configuration source, never read, NEVER \
         counted (CR-157 section 3.3)",
        if r.cdk_exists {
            "present on the copy"
        } else {
            "absent on the copy"
        }
    );

    report_reconciliation(r);
}

/// `(config-only, site-refused, site-uncaptured)` over the decisive net-new
/// pairs — the one count the report prints and the pin asserts.
fn net_new_split(r: &Run) -> (usize, usize, usize) {
    let mut split = (0, 0, 0);
    for (m, _) in r.readings[&Reading::Decisive].pairs(Status::NetNew).keys() {
        match net_new_state(r, m) {
            SiteState::ConfigOnly => split.0 += 1,
            SiteState::SiteRefused(_) => split.1 += 1,
            SiteState::SiteUncaptured => split.2 += 1,
        }
    }
    split
}

fn net_new_state(r: &Run, member: &str) -> SiteState {
    let (rows, refusals) = r.product.broker.get(member).cloned().unwrap_or_default();
    site_state(rows, r.product.observed.has_topics(member), &refusals)
}

fn report_reconciliation(r: &Run) {
    let s411_pairs: BTreeSet<(&str, &str)> = r
        .s411
        .iter()
        .map(|d| (d.member.as_str(), d.topic.as_str()))
        .collect();
    let s411_topics: BTreeSet<&str> = r.s411.iter().map(|d| d.topic.as_str()).collect();
    println!(
        "\n  RECONCILIATION AGAINST S-411 (recorded {S411_TOPICS} topics / {S411_DECLARATIONS} \
         declarations; this copy re-reads {} / {} at (M, T) grain)",
        s411_topics.len(),
        s411_pairs.len()
    );
    let mut outcomes: BTreeMap<TopicOutcome, Vec<&str>> = BTreeMap::new();
    for (identity, outcome) in &r.s411_topics {
        outcomes.entry(*outcome).or_default().push(identity);
    }
    for (outcome, ids) in &outcomes {
        println!(
            "    S-411 identities on this copy, {:<44} {:>3}",
            outcome.label(),
            ids.len()
        );
        if *outcome != TopicOutcome::BothSidesDeclared {
            println!("        {ids:?}");
        }
    }
    println!("    matched (M, T) on both sides: {}", r.matched);
    let mut by: BTreeMap<(&str, Mechanism), Vec<&Difference>> = BTreeMap::new();
    for x in &r.differences {
        by.entry((x.side, x.mechanism)).or_default().push(x);
    }
    for ((side, mech), xs) in &by {
        println!("    {side:<20} {:<62} {:>3}", mech.label(), xs.len());
        for x in xs {
            println!("        {} -> {}", x.member, x.topic);
        }
    }
}

// ── The gate ────────────────────────────────────────────────────────────────

/// **S-488's blocking gate.** Reports VOID — and asserts nothing — when no
/// estate is configured, so `cargo test --workspace`, `gate.sh` and CI stay
/// green without one. That is also why they never see this figure.
#[test]
fn measure_net_new_declared_topics_over_the_reference_workspace() {
    let Some(root) = crate::corpus_root() else {
        eprintln!("{}", headline(None));
        return;
    };
    let r = run(&root);
    report(&r);
    println!("\n{RECORDED_FINDING}");

    assert_eq!(
        r.manifest,
        declared_members(),
        "the measured manifest's members differ from the population declared before the run — \
         the decisive population has moved, so this run measures a different gate"
    );
    assert_estate_engaged(&r);
    let unexplained: Vec<_> = r
        .differences
        .iter()
        .filter(|x| x.mechanism == Mechanism::Unexplained)
        .collect();
    assert!(
        unexplained.is_empty(),
        "{} difference(s) against S-411 no named mechanism explains: {unexplained:?}",
        unexplained.len()
    );

    let decisive = r.readings[&Reading::Decisive].net_new();
    println!(
        "\n{}\n  beside it: DIRECTORY {} ({}) · TEST-TREE {} ({})",
        headline(Some(decisive)),
        r.readings[&Reading::Directory].net_new(),
        verdict(r.readings[&Reading::Directory].net_new()),
        r.readings[&Reading::TestTree].net_new(),
        verdict(r.readings[&Reading::TestTree].net_new()),
    );
    assert_the_recorded_verdict(&r);
}

/// A run that read nothing prints "0 net-new" exactly like a run that read
/// everything and found none.
fn assert_estate_engaged(r: &Run) {
    assert!(
        r.directories.len() >= 80,
        "only {} directories — not the estate",
        r.directories.len()
    );
    assert_eq!(
        r.product.members_read.len(),
        r.directories.len(),
        "the topic read-model answered for {} of {} member stores",
        r.product.members_read.len(),
        r.directories.len()
    );
    let d = &r.readings[&Reading::Decisive];
    assert!(
        d.sources_read >= 50,
        "only {} src/main sources read",
        d.sources_read
    );
    assert!(
        !d.captured.is_empty(),
        "the product captured no topic — the copy is not indexed"
    );
    assert!(
        !d.declarations.is_empty(),
        "no declaration resolved — the reader did not engage"
    );
    assert!(
        !r.s411.is_empty(),
        "S-411's judgement produced no declaration on this copy"
    );
}

/// The figures this run recorded, pinned so a drift in the reader, the corpus,
/// the product or the classifier is a failure rather than a quietly different
/// verdict. See `declared_topics_finding.txt`.
pub const RECORDED_NET_NEW: usize = 8;
pub const RECORDED_OBSERVED: usize = 46;
pub const RECORDED_DECLARED_ONLY: usize = 1;
pub const RECORDED_REFUSED: usize = 0;
pub const RECORDED_DIRECTORY_NET_NEW: usize = 8;
pub const RECORDED_TEST_TREE_NET_NEW: usize = 8;

/// `(config-only, site-refused, site-uncaptured)` over the decisive net-new
/// pairs — the B3 split the finding reads.
pub const RECORDED_SPLIT: (usize, usize, usize) = (3, 5, 0);

/// `(matched, here-not-in-S-411, in-S-411-not-here)` at (M, T) grain — the
/// reconciliation the finding attributes, mechanism by mechanism.
pub const RECORDED_RECONCILIATION: (usize, usize, usize) = (35, 19, 1);

fn assert_the_recorded_verdict(r: &Run) {
    let d = &r.readings[&Reading::Decisive];
    let measured = (
        d.net_new(),
        d.pairs(Status::Observed).len(),
        d.pairs(Status::DeclaredOnly).len(),
        d.refused.len(),
        r.readings[&Reading::Directory].net_new(),
        r.readings[&Reading::TestTree].net_new(),
    );
    assert_eq!(
        measured,
        (
            RECORDED_NET_NEW,
            RECORDED_OBSERVED,
            RECORDED_DECLARED_ONLY,
            RECORDED_REFUSED,
            RECORDED_DIRECTORY_NET_NEW,
            RECORDED_TEST_TREE_NET_NEW,
        ),
        "(net-new, observed, declared-only, refused, directory net-new, test-tree net-new) moved \
         from the recorded run — re-decide CR-157, do not relax the pin"
    );
    assert_eq!(
        net_new_split(r),
        RECORDED_SPLIT,
        "the config-only / site-refused / site-uncaptured split moved"
    );
    let side = |s: &str| r.differences.iter().filter(|x| x.side == s).count();
    assert_eq!(
        (
            r.matched,
            side("here, not in S-411"),
            side("in S-411, not here")
        ),
        RECORDED_RECONCILIATION,
        "the reconciliation against S-411 moved"
    );
    let line = headline(Some(d.net_new()));
    assert!(
        RECORDED_FINDING.contains(&line),
        "the finding does not carry this run's verdict line:\n{line}"
    );
}

// ── Fixtures (always run) ───────────────────────────────────────────────────

#[cfg(test)]
mod fixtures {
    use super::*;
    use crate::configuration_agreement::ConfigSource;

    fn set(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|n| (*n).to_string()).collect()
    }

    /// `(member, topic)` rows of the promoted read-model.
    fn observed(rows: &[(&str, &str)]) -> Observed {
        let mut o = Observed::default();
        for (m, t) in rows {
            o.by_member
                .entry((*m).to_string())
                .or_default()
                .insert((*t).to_string());
        }
        o
    }

    fn source(
        path: &str,
        module: &str,
        profile: Option<&str>,
        kv: &[(&str, &str)],
    ) -> ConfigSource {
        let mut values: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (k, v) in kv {
            values
                .entry((*k).to_string())
                .or_default()
                .insert((*v).to_string());
        }
        ConfigSource {
            path: path.to_string(),
            profile: profile.map(str::to_string),
            module: module.to_string(),
            values,
        }
    }

    fn corpus(sources: Vec<ConfigSource>) -> ConfigCorpus {
        let mut c = ConfigCorpus::default();
        c.sources = sources;
        c
    }

    // ── The declaration ─────────────────────────────────────────────────────

    #[test]
    fn the_floor_and_the_population_are_the_declared_ones() {
        // Reads the DECLARATION, never the constant against itself, and requires
        // the floor line to be UNIQUE.
        let hits: Vec<usize> = DECLARED_FLOOR
            .lines()
            .filter(|l| l.contains("NET-NEW MEMBER-TOPIC PAIRS"))
            .filter_map(|l| {
                l.trim()
                    .strip_prefix(">= ")?
                    .split_whitespace()
                    .next()?
                    .parse()
                    .ok()
            })
            .collect();
        assert_eq!(
            hits,
            vec![NET_NEW_PAIR_FLOOR],
            "the declaration must state the floor on exactly one `>= N NET-NEW MEMBER-TOPIC \
             PAIRS` line, equal to NET_NEW_PAIR_FLOOR"
        );
        // The floor is the stakeholder's, carried by the verbatim B1 quote.
        let quoted: Vec<usize> = DECLARED_FLOOR
            .split("**Floor: ≥ ")
            .skip(1)
            .filter_map(|rest| {
                rest.split(|c: char| !c.is_ascii_digit())
                    .next()?
                    .parse()
                    .ok()
            })
            .collect();
        assert_eq!(
            quoted,
            vec![NET_NEW_PAIR_FLOOR],
            "CR-157 B1's quoted floor disagrees"
        );
        assert!(DECLARED_FLOOR.contains(
            "- **B1 — Metric and floor.** *Net-new member–topic pairs (A3) over the decisive \
             population.* **Floor: ≥ 3**, stated by the stakeholder on 2026-10-01."
        ));
        for clause in [
            "**A1, declaration.**",
            "**A2, topic identity.**",
            "**A3, net-new pair.**",
            "**A4, grain.**",
        ] {
            assert!(
                DECLARED_FLOOR.contains(clause),
                "the verbatim part A lacks {clause}"
            );
        }
        assert!(
            DECLARED_FLOOR.contains("logos commit 13498362"),
            "the named commit"
        );
        assert!(DECLARED_FLOOR.contains(
            "SHA-256:        9e1873392a2e366d59db7b375b9c55c5f984648cc4718f2a02dab93db7444f49"
        ));
        assert!(DECLARED_FLOOR
            .contains("MANIFEST·MAIN  the 83 members above, `src/main` sources only. DECISIVE."));

        let members = declared_members();
        assert!(
            DECLARED_FLOOR.contains(&format!("MANIFEST MEMBERS — BEGIN ({})", members.len())),
            "the member list parses to {} names, not the count its BEGIN marker states",
            members.len()
        );
        assert_eq!(members.len(), 83);
        assert!(
            !members.contains("archive-api-logiclens-fork"),
            "the fork is excluded"
        );
        assert!(
            members.contains("funnel-aggregator-api")
                && members.contains("mailbox-unread-mail-batch")
        );
        // The same population S-475 declared, byte for byte.
        assert_eq!(members, crate::sequence_addressed_pairs::declared_members());
        assert!(DECLARED_FLOOR.contains(CDK_BLIND_SPOT));
    }

    // ── A1: the key rule ────────────────────────────────────────────────────

    #[test]
    fn a_topic_named_key_is_one_of_three_shapes_and_its_near_misses_are_not() {
        assert_eq!(
            key_shape("spring.kafka.topics.archiveevents"),
            Some(KeyShape::SpringKafkaTopics)
        );
        assert_eq!(
            key_shape("filteraggregate.topic"),
            Some(KeyShape::LastSegment)
        );
        assert_eq!(
            key_shape("spring.kafka.topics"),
            Some(KeyShape::LastSegment)
        );
        assert_eq!(
            key_shape("spring.kafka.template.defaulttopic"),
            Some(KeyShape::LastSegmentSuffix)
        );
        assert_eq!(
            key_shape("app.outboundtopics"),
            Some(KeyShape::LastSegmentSuffix)
        );
        // Near misses: `topic` inside the last segment but not at its end, a
        // `topics` segment that is not the last, a prefix one character off.
        assert_eq!(key_shape("kafka.topicname"), None);
        assert_eq!(key_shape("app.topics.retention"), None);
        assert_eq!(key_shape("spring.kafka.topicsx.archiveevents"), None);
        assert_eq!(key_shape("spring.kafka.bootstrapservers"), None);
    }

    // ── B2: what `src/main` means ───────────────────────────────────────────

    #[test]
    fn src_main_is_a_segment_pair_and_only_the_test_tree_reading_reads_src_test() {
        let main = "m/svc/src/main/resources/application.yml";
        let test = "m/svc/src/test/resources/application.yml";
        let root = "m/application.yml";
        let near = "m/src/maintenance/application.yml";
        assert!(
            Reading::Decisive.admits(main)
                && Reading::Directory.admits(main)
                && Reading::TestTree.admits(main)
        );
        assert!(!Reading::Decisive.admits(test) && !Reading::Directory.admits(test));
        assert!(Reading::TestTree.admits(test));
        assert!(!Reading::TestTree.admits(root) && outside_every_reading(root));
        assert!(!Reading::TestTree.admits(near) && outside_every_reading(near));
        assert!(!outside_every_reading(test));
    }

    #[test]
    fn each_reading_counts_its_own_population_and_only_directory_counts_the_fork() {
        // DIRECTORY and the decisive reading recorded the same net-new figure on
        // the estate, so no estate pin can tell them apart: only this can.
        let manifest = set(&["a"]);
        let directories = set(&["a", "fork"]);
        assert_eq!(
            Reading::Directory.members(&manifest, &directories),
            &directories
        );
        assert_eq!(
            Reading::Decisive.members(&manifest, &directories),
            &manifest
        );
        assert_eq!(
            Reading::TestTree.members(&manifest, &directories),
            &manifest
        );
    }

    #[test]
    fn a_reading_reads_only_its_population_when_the_scalars_cover_more() {
        // The estate run folds scalars over all 84 directories and reads the
        // decisive population (83) out of them: this filter alone keeps the
        // fork's declarations out of the headline.
        let c = corpus(vec![
            source(
                "a/src/main/resources/application.yml",
                "a",
                None,
                &[("spring.kafka.topics.orders", "orders")],
            ),
            source(
                "fork/src/main/resources/application.yml",
                "fork",
                None,
                &[("spring.kafka.topics.orders", "orders")],
            ),
        ]);
        let manifest = set(&["a"]);
        let directories = set(&["a", "fork"]);
        let scalars = application_scalars(&c, &directories);
        let o = observed(&[("a", "orders")]);
        let decisive = measure_reading(Reading::Decisive, &c, &scalars, &manifest, &o);
        assert_eq!(
            decisive.net_new(),
            0,
            "the fork is not in the decisive population"
        );
        assert_eq!(decisive.declarations.len(), 1);
        assert_eq!(decisive.sources_read, 1);
        let directory = measure_reading(Reading::Directory, &c, &scalars, &directories, &o);
        assert!(directory
            .pairs(Status::NetNew)
            .contains_key(&("fork".to_string(), "orders".to_string())));
    }

    // ── A1: value resolution through the shipped topic_identity ─────────────

    #[test]
    fn a_literal_resolves_to_itself_with_its_source_profile() {
        let c = corpus(vec![]);
        let lookup = ModuleScoped {
            corpus: &c,
            module: "m",
        };
        assert_eq!(
            resolve(" orders ", &lookup, "application:prod"),
            Resolved::Topics(vec![("orders".to_string(), "application:prod".to_string())])
        );
    }

    #[test]
    fn one_placeholder_hop_reaches_a_committed_value_in_the_same_module_only() {
        let c = corpus(vec![
            source(
                "m/src/main/resources/application.yml",
                "m",
                None,
                &[("spring.kafka.topics.orders", "orders")],
            ),
            source(
                "n/src/main/resources/application.yml",
                "n",
                None,
                &[("spring.kafka.topics.payments", "payments")],
            ),
        ]);
        let lookup = ModuleScoped {
            corpus: &c,
            module: "m",
        };
        let Resolved::Topics(t) = resolve(
            "${spring.kafka.topics.orders}",
            &lookup,
            "application:<none>",
        ) else {
            panic!("the hop resolves");
        };
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].0, "orders");
        assert!(
            t[0].1.contains("via ${spring.kafka.topics.orders}"),
            "{}",
            t[0].1
        );
        // The near miss: the same shape, but the key is defined only in ANOTHER
        // module — out of reach, so refused rather than borrowed.
        assert_eq!(
            resolve("${spring.kafka.topics.payments}", &lookup, "x"),
            Resolved::Refused(ValueRefusal::MissingKey)
        );
    }

    #[test]
    fn a_placeholder_whose_value_is_itself_a_placeholder_is_refused() {
        let c = corpus(vec![source(
            "m/src/main/resources/application.yml",
            "m",
            None,
            &[("spring.kafka.topics.orders", "${ORDERS_TOPIC}")],
        )]);
        let lookup = ModuleScoped {
            corpus: &c,
            module: "m",
        };
        assert_eq!(
            resolve("${spring.kafka.topics.orders}", &lookup, "x"),
            Resolved::Refused(ValueRefusal::PlaceholderValue)
        );
    }

    // ── A2/A3: the classifier ───────────────────────────────────────────────

    #[test]
    fn a_declaration_with_an_observed_producer_on_the_same_topic_is_observed() {
        let o = observed(&[("m", "orders")]);
        let captured = o.identities(&set(&["m"]));
        assert_eq!(classify("m", "orders", &captured, &o), Status::Observed);
    }

    #[test]
    fn a_declaration_with_no_site_on_t_is_net_new() {
        // T is captured — by ANOTHER member — and M has a site, but on a
        // different topic: the two near misses for "observed".
        let o = observed(&[("p", "orders"), ("m", "payments")]);
        let captured = o.identities(&set(&["m", "p"]));
        assert_eq!(classify("m", "orders", &captured, &o), Status::NetNew);
        // …and a member with no promoted topic at all.
        assert_eq!(classify("q", "orders", &captured, &o), Status::NetNew);
    }

    #[test]
    fn a_value_no_broker_site_names_is_declared_only() {
        let o = observed(&[("p", "orders")]);
        let captured = o.identities(&set(&["p"]));
        assert_eq!(classify("m", "false", &captured, &o), Status::DeclaredOnly);
        // A topic captured only OUTSIDE the reading's population is not captured.
        let only_manifest = o.identities(&set(&["m"]));
        assert_eq!(
            classify("m", "orders", &only_manifest, &o),
            Status::DeclaredOnly
        );
        // A2 is decided BEFORE A3: a member's own site does not make T a
        // captured identity of a population it is not counted in. Unreachable
        // through `measure_reading`, which reads only the population's own
        // members, so only this direct call pins the order.
        let own = observed(&[("x", "orders")]);
        let without_x = own.identities(&set(&["m"]));
        assert_eq!(
            classify("x", "orders", &without_x, &own),
            Status::DeclaredOnly
        );
    }

    #[test]
    fn a_promoted_key_is_matched_on_its_topic_not_its_schema_guard() {
        assert_eq!(topic_of("orders#com.acme.OrderCreated"), "orders");
        assert_eq!(topic_of("orders"), "orders");
    }

    /// The end-to-end shape over S-411's reader: one fixture member per
    /// outcome, with the declared/observed/net-new/declared-only/refused
    /// buckets each reached and counted at pair grain.
    #[test]
    fn a_reading_buckets_every_declaration_and_counts_pairs_not_rows() {
        let c = corpus(vec![
            source(
                "a/src/main/resources/application.yml",
                "a",
                None,
                &[
                    ("spring.kafka.topics.orders", "orders"),
                    ("app.consumer.topic", "orders"),
                ],
            ),
            source(
                "b/src/main/resources/application.yml",
                "b",
                None,
                &[("filteraggregate.topic", "orders")],
            ),
            source(
                "c/src/main/resources/application.yml",
                "c",
                None,
                // A blank value is no value: skipped, never a declared-only "".
                &[("app.audit.topic", "audit-log"), ("app.retry.topic", "  ")],
            ),
            source(
                "d/src/main/resources/application.yml",
                "d",
                None,
                &[(
                    "spring.kafka.template.defaulttopic",
                    "${spring.kafka.topics.missing}",
                )],
            ),
            source(
                "e/src/test/resources/application.yml",
                "e",
                None,
                &[("app.topic", "orders")],
            ),
            source(
                "f/src/main/resources/application.yml",
                "f",
                None,
                &[("app.name", "orders")],
            ),
        ]);
        let members = set(&["a", "b", "c", "d", "e", "f"]);
        let scalars = application_scalars(&c, &members);
        let o = observed(&[("a", "orders")]);
        let r = measure_reading(Reading::Decisive, &c, &scalars, &members, &o);

        // a: two keys, one pair, observed. b: net-new. c: declared-only.
        // d: refused. e: test tree, not read. f: not a topic-named key.
        assert_eq!(r.pairs(Status::Observed).len(), 1);
        assert_eq!(
            r.pairs(Status::Observed)[&("a".to_string(), "orders".to_string())].len(),
            2
        );
        assert_eq!(r.net_new(), 1);
        assert!(r
            .pairs(Status::NetNew)
            .contains_key(&("b".to_string(), "orders".to_string())));
        assert_eq!(r.pairs(Status::DeclaredOnly).len(), 1);
        assert_eq!(r.refused.len(), 1);
        assert_eq!(r.refused[0].reason, ValueRefusal::MissingKey);
        assert_eq!(r.sources_read, 5, "the src/test source is not read");
        let shapes = r.key_shapes();
        assert_eq!(shapes.get(&KeyShape::SpringKafkaTopics), Some(&1));
        assert_eq!(shapes.get(&KeyShape::LastSegment), Some(&3));

        // The test-tree reading reads e — a second net-new pair.
        let t = measure_reading(Reading::TestTree, &c, &scalars, &members, &o);
        assert_eq!(t.net_new(), 2);
    }

    #[test]
    fn a_declaration_reached_through_a_hop_is_marked_hopped_and_a_literal_one_is_not() {
        let c = corpus(vec![source(
            "h/src/main/resources/application.yml",
            "h",
            None,
            &[
                ("spring.kafka.topics.payments", "payments"),
                ("app.payments.topic", "${spring.kafka.topics.payments}"),
            ],
        )]);
        let members = set(&["h"]);
        let scalars = application_scalars(&c, &members);
        let r = measure_reading(
            Reading::Decisive,
            &c,
            &scalars,
            &members,
            &Observed::default(),
        );
        let hopped: BTreeMap<&str, bool> = r
            .declarations
            .iter()
            .map(|(d, _)| (d.key.as_str(), d.hopped))
            .collect();
        assert_eq!(hopped.get("app.payments.topic"), Some(&true));
        assert_eq!(hopped.get("spring.kafka.topics.payments"), Some(&false));
        assert_eq!(r.pairs(Status::DeclaredOnly).len(), 1, "two keys, one pair");
    }

    // ── B3: the split ───────────────────────────────────────────────────────

    #[test]
    fn the_split_is_config_only_site_refused_or_site_uncaptured() {
        assert_eq!(site_state(0, false, &[]), SiteState::ConfigOnly);
        // A promoted topic with no coverage row is still a broker site.
        assert_eq!(site_state(0, true, &[]), SiteState::SiteUncaptured);
        assert_eq!(site_state(3, true, &[]), SiteState::SiteUncaptured);
        let r = vec!["topic-not-literal at s".to_string()];
        assert_eq!(site_state(1, false, &r), SiteState::SiteRefused(r.clone()));
    }

    #[test]
    fn a_member_the_read_model_answered_with_no_topic_has_none() {
        // `read_product` inserts an entry for EVERY member the read-model
        // answers, an empty set included: "has an entry" is not "has a topic",
        // and the difference is config-only against site-uncaptured.
        let mut o = Observed::default();
        o.by_member.insert("m".to_string(), BTreeSet::new());
        o.by_member.insert("p".to_string(), set(&["orders"]));
        assert!(!o.has_topics("m"));
        assert!(!o.has_topics("absent"));
        assert!(o.has_topics("p"));
        assert_eq!(site_state(0, o.has_topics("m"), &[]), SiteState::ConfigOnly);
        assert_eq!(
            site_state(0, o.has_topics("p"), &[]),
            SiteState::SiteUncaptured
        );
    }

    #[test]
    fn only_a_refusal_is_a_refusal_and_no_provider_is_not_one() {
        let lit = ValueProvenance::Literal;
        let unbound = |reason| CoverageState::Unbound { reason };
        assert_eq!(
            refusal_of(&unbound(UnboundReason::TopicNotLiteral), &lit).as_deref(),
            Some("topic-not-literal")
        );
        assert_eq!(
            refusal_of(&unbound(UnboundReason::NoProviderInWorkspace), &lit),
            None
        );
        assert_eq!(refusal_of(&CoverageState::Bound, &lit), None);
        let unresolved = ValueProvenance::ConfigUnresolved {
            keys: vec!["k".to_string()],
            refusal: ValueRefusal::MissingKey,
        };
        assert!(
            refusal_of(&unbound(UnboundReason::NoProviderInWorkspace), &unresolved)
                .is_some_and(|r| r.starts_with("config-unresolved"))
        );
    }

    // ── B3: the reconciliation ──────────────────────────────────────────────

    fn decl(member: &str, topic: &str, hopped: bool) -> Declaration {
        Declaration {
            member: member.to_string(),
            topic: topic.to_string(),
            key: "spring.kafka.topics.x".to_string(),
            file: format!("{member}/src/main/resources/application.yml"),
            profile: "p".to_string(),
            shape: KeyShape::SpringKafkaTopics,
            hopped,
        }
    }

    fn theirs(member: &str, topic: &str, key: &str, file: &str) -> S411Declaration {
        S411Declaration {
            member: member.to_string(),
            topic: topic.to_string(),
            key: key.to_string(),
            file: file.to_string(),
        }
    }

    /// One judged S-411 topic, as its `judge_topics` would record it.
    fn judged_topic(
        identity: Option<&str>,
        outcome: TopicOutcome,
        evidence: &[(&str, &str, &str)],
    ) -> crate::config_declared_coupling::JudgedTopic {
        crate::config_declared_coupling::JudgedTopic {
            topic: crate::config_declared_coupling::CapturedTopic {
                as_written: identity.unwrap_or("${unresolved}").to_string(),
                resolved: identity.map(str::to_string),
                publishes: 0,
                subscribes: 1,
            },
            identity: identity.map(str::to_string),
            outcome,
            declarers: evidence.iter().map(|e| e.0.to_string()).collect(),
            evidence: evidence
                .iter()
                .map(|(m, k, f)| (m.to_string(), k.to_string(), f.to_string()))
                .collect(),
            by_source: Vec::new(),
        }
    }

    /// A [`Judgement`] carrying only topics, after S-411's own `probe_judgement`.
    fn judgement_of(
        resolved: Vec<crate::config_declared_coupling::JudgedTopic>,
        as_written: Vec<crate::config_declared_coupling::JudgedTopic>,
    ) -> Judgement {
        Judgement {
            members: BTreeSet::new(),
            runnable: BTreeSet::new(),
            targets: Vec::new(),
            topics_resolved: resolved,
            topics_as_written: as_written,
            cost: crate::config_declared_coupling::WalkCost::default(),
            sequence_host_ceiling: Vec::new(),
            path_only: BTreeSet::new(),
            application_scalars: 0,
            deploy_scalars: 0,
            deploy_member_labels: BTreeSet::new(),
            deploy_external_labels: BTreeSet::new(),
            port_owners: BTreeMap::new(),
            claimants: BTreeMap::new(),
            blind_spot: BTreeSet::new(),
        }
    }

    #[test]
    fn the_s411_side_is_its_both_sides_declarations_under_the_resolved_reading() {
        let f = "x/src/main/resources/application.yml";
        let k = "spring.kafka.topics.orders";
        let j = judgement_of(
            vec![
                judged_topic(
                    Some("orders"),
                    TopicOutcome::BothSidesDeclared,
                    &[("b", k, f), ("a", k, f)],
                ),
                judged_topic(Some("solo"), TopicOutcome::OneSideOnly, &[("c", k, f)]),
                judged_topic(None, TopicOutcome::DanglingUnresolved, &[]),
            ],
            // The AS-WRITTEN reading is never the S-411 side.
            vec![judged_topic(
                Some("${spring.kafka.topics.orders}"),
                TopicOutcome::BothSidesDeclared,
                &[("z", k, f)],
            )],
        );
        assert_eq!(
            s411_declarations(&j),
            vec![theirs("a", "orders", k, f), theirs("b", "orders", k, f)],
            "only both-sides-declared identities, one row per declarer, sorted"
        );
        let topics = s411_topics(&j);
        assert_eq!(
            topics.into_iter().collect::<Vec<_>>(),
            vec![
                ("orders".to_string(), TopicOutcome::BothSidesDeclared),
                ("solo".to_string(), TopicOutcome::OneSideOnly),
            ],
            "resolved identities only; an unresolved one has no identity to key"
        );
    }

    #[test]
    fn every_reconciliation_mechanism_is_reachable_and_the_rest_is_unexplained() {
        let manifest = set(&["m"]);
        let topics: S411Topics = [
            ("both".to_string(), TopicOutcome::BothSidesDeclared),
            ("one".to_string(), TopicOutcome::OneSideOnly),
        ]
        .into_iter()
        .collect();
        let main = "m/src/main/resources/application.yml";
        let k = "spring.kafka.topics.x";
        let m = |o: Option<&Declaration>, t: Option<&S411Declaration>, d: bool| {
            mechanism(o, t, &manifest, &topics, d)
        };

        assert_eq!(
            m(None, Some(&theirs("fork", "both", k, main)), false),
            Mechanism::ManifestPopulation
        );
        assert_eq!(
            m(
                None,
                Some(&theirs("m", "both", k, "m/application.yml")),
                false
            ),
            Mechanism::SrcMainOnly
        );
        assert_eq!(
            m(
                None,
                Some(&theirs("m", "both", "kafka.topicname", main)),
                false
            ),
            Mechanism::TopicNamedKeyRule
        );
        assert_eq!(
            m(None, Some(&theirs("m", "both", k, main)), true),
            Mechanism::CapturedByHarnessNotProduct
        );
        assert_eq!(
            m(None, Some(&theirs("m", "both", k, main)), false),
            Mechanism::Unexplained
        );

        assert_eq!(
            m(Some(&decl("m", "new", false)), None, false),
            Mechanism::CapturedByProductNotHarness
        );
        assert_eq!(
            m(Some(&decl("m", "one", false)), None, false),
            Mechanism::DeclarerThreshold
        );
        assert_eq!(
            m(Some(&decl("m", "both", true)), None, false),
            Mechanism::PlaceholderHop
        );
        // A literal declaration of a topic S-411 counted, which S-411 did not
        // list for this member — nothing named explains it.
        assert_eq!(
            m(Some(&decl("m", "both", false)), None, false),
            Mechanism::Unexplained
        );
    }

    #[test]
    fn the_reconciliation_matches_at_pair_grain_and_names_each_side() {
        let manifest = set(&["m", "n"]);
        let topics: S411Topics = [("orders".to_string(), TopicOutcome::BothSidesDeclared)]
            .into_iter()
            .collect();
        let r = ReadingResult {
            declarations: vec![
                (decl("m", "orders", false), Status::Observed),
                (decl("m", "orders", false), Status::Observed),
                (decl("n", "fresh", false), Status::NetNew),
                // Declared-only here: never one of "ours", so S-411's pair on it
                // is a difference, not a match.
                (decl("m", "stale", false), Status::DeclaredOnly),
            ],
            ..ReadingResult::default()
        };
        let main = "m/src/main/resources/application.yml";
        let s411 = vec![
            theirs("m", "orders", "spring.kafka.topics.orders", main),
            theirs("m", "stale", "spring.kafka.topics.stale", main),
            theirs(
                "fork",
                "orders",
                "spring.kafka.topics.orders",
                "fork/src/main/resources/application.yml",
            ),
        ];
        let (matched, diffs) = reconcile(&r, &s411, &manifest, &topics);
        assert_eq!(matched, 1, "two rows of one pair are one match");
        assert_eq!(diffs.len(), 3);
        assert!(diffs.iter().any(|d| d.topic == "stale"
            && d.side == "in S-411, not here"
            && d.mechanism == Mechanism::CapturedByHarnessNotProduct));
        assert!(diffs
            .iter()
            .any(|d| d.member == "fork" && d.mechanism == Mechanism::ManifestPopulation));
        assert!(diffs.iter().any(|d| d.member == "n"
            && d.side == "here, not in S-411"
            && d.mechanism == Mechanism::CapturedByProductNotHarness));
    }

    // ── The verdict ─────────────────────────────────────────────────────────

    #[test]
    fn only_the_decisive_reading_decides_and_a_blind_run_is_void() {
        assert_eq!(verdict(NET_NEW_PAIR_FLOOR - 1), "FALSIFIED");
        assert_eq!(verdict(NET_NEW_PAIR_FLOOR), "HOLDS");
        let void = headline(None);
        assert!(void.starts_with("VOID"), "{void}");
        assert!(!void.contains("HOLDS") && !void.contains("FALSIFIED") && !void.contains(" 0 "));
        assert!(headline(Some(NET_NEW_PAIR_FLOOR)).contains("MANIFEST·MAIN reading 3 "));
        assert!(headline(Some(NET_NEW_PAIR_FLOOR)).ends_with("HOLDS"));
        assert!(headline(Some(0)).ends_with("FALSIFIED"));
    }
}
