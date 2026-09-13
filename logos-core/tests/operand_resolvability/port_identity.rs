//! **S-400 — does a runtime port identify the callee?** ([CR-124] §3.2,
//! [FR-WS-20], [FR-WS-04], [FR-CG-09], [NFR-RA-05], [NFR-CC-04]).
//!
//! This is a **blocking measurement gate**, and it builds nothing. [FR-WS-22]
//! was withdrawn on 2026-09-13 after [S-384] falsified identity-first binding at
//! 12 net-new edges against a floor of 16 — 11 of them self-ties, leaving **one**
//! genuine third-party edge. Its withdrawal banner retains its criteria *"so the
//! decision stays visible and is not re-proposed without new evidence."* This
//! module measures whether a **different signal** is that evidence.
//!
//! # The two signals, and why they are not the same experiment
//!
//! [FR-WS-20]'s ladder resolves a target **by name** — a Kubernetes `Service`,
//! a Helm chart, a Compose key, a directory, a framework application name — and
//! [S-384] joined a deploy manifest back to a call site on the first **DNS
//! label** of a host. This signal never reads a deploy manifest. It reads the
//! **port** out of a `base-url` the configuration corpus already proves, and
//! matches it against the member that declares that port as its own
//! `server.port`:
//!
//! ```yaml
//! # funnel-aggregator-api/src/main/resources/application.yml — the consumer
//! filters:
//!   api:
//!     base-url: http://localhost:9013
//!     uri-get-filters: /v1/users/{userId}/filters
//!
//! # filters-api/src/main/resources/application.yml — the provider
//! server:
//!   port: 9013
//! ```
//!
//! The host label here is `localhost` for every `base-url` on the estate, so
//! [FR-WS-20]'s ladder matches no member and the port — the only part of the
//! authority that distinguishes one callee from another — is **discarded**.
//! That is why [S-384]'s review could record joining application-config evidence
//! as *"verified neutral on this estate"* and this gate can still be a genuinely
//! new question: the two readings differ in which byte of the authority they
//! join on, not in how confident they are about the same byte.
//!
//! # What decides the gate
//!
//! Exactly one figure: **net-new third-party** port-resolved consumer→provider
//! call-site edges, at the `(consumer, normalized template, provider)` grain
//! [S-384]'s [`identity::Findings::net_new`] counts. The floor of **16** was
//! declared before any of this code existed and lives in the tracked
//! [`port_identity_floor.txt`], which [`DECLARED_FLOOR`] embeds and
//! [`the_floor_is_the_one_declared_before_the_run`] **parses** — so the constant
//! cannot be edited to clear a run without the declaration being edited too.
//!
//! Three words in that sentence do work, and each one is a lesson paid for:
//!
//! - **net-new** — an edge path-only matching would already have bound on its
//!   own is reported and is *not* evidence for the port signal ([S-384]'s AC3
//!   split).
//! - **third-party** — a self-tie is counted and printed **apart from** the
//!   third-party figure *at the point the headline appears*. That is precisely
//!   the defect [S-384] exposed, where a headline of 12 was 11 self-ties and did
//!   not say so. The word turns out to have **two** defensible definitions on
//!   this estate, and both are measured — see [`Reading`], and the paragraph
//!   below.
//! - **call-site edge** — the triple, not the member→member coupling. [CR-124]'s
//!   Decision Log fixes the unit and gives the reason: a pair-based floor "would
//!   reject on a unit [S-384] never measured".
//!
//! # The declaration and S-384 do not define "third-party" the same way
//!
//! Found during the run, and reported rather than resolved by fiat. The floor
//! declaration says *"THIRD-PARTY means, exactly: consumer member != provider
//! member"* — while citing, as its whole motivation, [S-384]'s split of 11
//! self-ties and one third-party edge. Those two things are inconsistent:
//! [S-384]'s self-ties are **not** consumers bound to themselves. Its finding
//! says so directly — *"an aggregator re-registers the callee's template on its
//! own controller, so two members serve it, path-only refuses, and identity
//! binds … identity's genuine THIRD-PARTY contribution, where the tie is between
//! two other members, is ONE edge"* — so the discriminator is the **serving
//! set**, not the two member names.
//!
//! The declaration is committed evidence and its own COMMITMENT section forbids
//! revising it after a run, so it is not revised. Instead both readings are
//! measured ([`Reading`]), the verdict is read off [`Reading::AsDeclared`]
//! because the declaration is the contract, and the run **asserts** that
//! [`Reading::AsMeasuredByS384`] is below the floor too — so nothing about the
//! verdict turns on the disagreement.
//!
//! # The population, and the two judgement calls in it
//!
//! Both sides are read from **main-tree application** configuration. Two choices
//! are packed into that, and neither is left as an assertion:
//!
//! - **No deploy evidence.** [CR-124] §2 fixes this: *"The signal here never
//!   touches a deploy manifest."* It also matters mechanically — a deploy values
//!   file writes `http://official-log-export-api.…:9020`, whose host label is
//!   [S-384]'s signal, so admitting it would make this gate re-measure the
//!   experiment it claims to be independent of.
//! - **No test tree.** The estate's test profiles point their `base-url`s at
//!   mocks (`localhost:3000`, `localhost:50000`, `localhost:60004`), which are
//!   not members, and a `server.port` declared in `src/test` is a test harness's
//!   port rather than the member's identity. `identity::scan_providers` and
//!   `identity::judge` already exclude test source for the same reason.
//!
//! [`Population::WithDeployEvidence`] and [`Population::WithTestTree`] re-run the
//! whole join with each choice reversed, and both counterfactuals are reported
//! beside the headline and asserted below the floor. The headline decides.
//!
//! # One walk, and no hand-mirrored twin
//!
//! The deploy corpus, the provider route index and the client-call measurement
//! all come from [`identity::findings`] and [`crate::measurement`], each behind
//! its own `OnceLock` — so adding this gate adds no traversal of the estate. The
//! decision itself is [`identity::classify`], **called** rather than copied: the
//! two signals differ in how they name a provider, never in what identity then
//! does to a call site.
//!
//! What this module does own is its own traversal of the resolved call sites,
//! because the target population it reads (`base-url` values the *application*
//! corpus proves) is the one [`identity::judge`] filters out with
//! `TargetRef::is_deploy`. A traversal copied from a sibling is exactly the
//! divergence that makes one of two measurements quietly wrong, so it is not
//! left to a comment: [`assert_traversal_agrees_with_the_identity_gate`] asserts
//! that this module and [S-384] considered the **same** call sites and refused
//! the **same** templates, on every estate run.
//!
//! # Read-only, and against the estate SOURCE
//!
//! Nothing here opens an `Engine`, indexes anything, or writes a byte under the
//! reference workspace.
//!
//! [CR-124]: ../../../docs/requests/CR-124-runtime-port-as-a-target-identity-tier.md
//! [FR-WS-04]: ../../../docs/specs/requirements/FR-WS-04.md
//! [FR-WS-20]: ../../../docs/specs/requirements/FR-WS-20.md
//! [FR-WS-22]: ../../../docs/specs/requirements/FR-WS-22.md
//! [FR-CG-09]: ../../../docs/specs/requirements/FR-CG-09.md
//! [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
//! [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
//! [S-384]: ../../../docs/planning/journal.md#s-384-measure-service-identity-resolvability-across-the-deploy-corpus
//! [S-400]: ../../../docs/planning/journal.md#s-400-measure-whether-a-runtime-port-identifies-the-callee
//! [`port_identity_floor.txt`]: ./port_identity_floor.txt

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use logos_core::resolve::route_template::normalize_template;

use super::configuration_agreement::Tree;
use super::identity::{self, flat_key, Corpus, PairClass, Providers, TargetRef};

/// The recorded verdict, reproduced by the run and printed by it.
///
/// `include_str!` rather than a doc link, following
/// [`identity::RECORDED_FINDING`]: a file the build embeds cannot be deleted or
/// renamed without breaking compilation. It protects the file's existence, not
/// its agreement with the run — the figures it states are pinned as the
/// `RECORDED_*` constants below, and the rest is a hand-maintained record.
pub const RECORDED_FINDING: &str = include_str!("port_identity_finding.txt");

/// **The floor, as declared before the run** — the tracked
/// `port_identity_floor.txt`, committed at 2026-09-13T13:13:50Z in this
/// session's first commit, before any of this module existed.
///
/// Tracked rather than written to `docs/planning/sprints/.pending/`, which is
/// gitignored: [S-384]'s declaration had to be copied back into the repository
/// after the fact because the original would have vanished at sprint teardown
/// (its review finding 18). [`the_floor_is_the_one_declared_before_the_run`]
/// parses the figure out of this text and compares it to
/// [`NET_NEW_THIRD_PARTY_FLOOR`].
///
/// [S-384]: ../../../docs/planning/journal.md#s-384-measure-service-identity-resolvability-across-the-deploy-corpus
pub const DECLARED_FLOOR: &str = include_str!("port_identity_floor.txt");

/// The materiality floor, declared before the run in `port_identity_floor.txt`
/// and reproduced here so the assertion and the declaration cannot drift.
///
/// [S-384]'s floor and [S-384]'s metric, unchanged, because comparability is the
/// whole point of offering this measurement as new evidence against [FR-WS-22]'s
/// withdrawal. Deliberately **not** derived from [CR-124]'s own hand census of
/// 39 sites — see the declaration's "WHY 16" section, and
/// [S-397](../../../docs/planning/journal.md#s-397-the-accessor-capture-hop-reaches-the-invocation-arm),
/// where a harness census became an unreachable story criterion.
///
/// [CR-124]: ../../../docs/requests/CR-124-runtime-port-as-a-target-identity-tier.md
/// [FR-WS-22]: ../../../docs/specs/requirements/FR-WS-22.md
/// [S-384]: ../../../docs/planning/journal.md#s-384-measure-service-identity-resolvability-across-the-deploy-corpus
pub const NET_NEW_THIRD_PARTY_FLOOR: usize = 16;

/// The configuration key a member declares its own runtime port under, in the
/// canonical form [`logos_core::extract::config::canonical_key`] produces.
///
/// One key and no synonyms, deliberately. `server.port` is what [CR-124] names
/// and what all 30 of the estate's declaring members write; admitting
/// `management.server.port` (the actuator's *separate* port) or a
/// `services.<x>.port` Helm value would join a caller's URL to a port its
/// callee does not serve traffic on, which is a fabricated edge under
/// [NFR-RA-05] rather than a wider signal.
///
/// [CR-124]: ../../../docs/requests/CR-124-runtime-port-as-a-target-identity-tier.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
const SERVER_PORT_KEY: &str = "server.port";

// ── Which evidence the join is allowed to read ──────────────────────────────

/// Which evidence one pass of the join reads, on both sides.
///
/// Three variants and not a pair of booleans: each one answers a question that
/// was actually asked of this measurement, and the two counterfactuals exist to
/// turn the headline's two judgement calls into measured numbers rather than
/// assertions in a comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Population {
    /// **The headline.** Main-tree *application* configuration on both sides,
    /// and no deploy evidence at all. [CR-124] §2 fixes this in one sentence:
    /// *"The signal here never touches a deploy manifest. It reads the port out
    /// of a `base-url` already indexed by [FR-WS-19]."*
    ///
    /// [CR-124]: ../../../docs/requests/CR-124-runtime-port-as-a-target-identity-tier.md
    /// [FR-WS-19]: ../../../docs/specs/requirements/FR-WS-19.md
    Headline,
    /// Counterfactual 1 — test-tree configuration admitted on both sides, so
    /// the cost of excluding it is a number.
    WithTestTree,
    /// Counterfactual 2 — deploy evidence admitted as a consumer-side target
    /// too.
    ///
    /// This is reported because it must be *subtracted*, not added: a deploy
    /// values file writes `http://official-log-export-api.…:9020`, whose host
    /// label [S-384] already resolves. An edge this variant adds is [S-384]'s
    /// own signal wearing a port, and counting it in the headline would make
    /// this gate re-measure the experiment it claims to be independent of.
    ///
    /// [S-384]: ../../../docs/planning/journal.md#s-384-measure-service-identity-resolvability-across-the-deploy-corpus
    WithDeployEvidence,
}

impl Population {
    /// Whether a consumer-side base-URL reference is admitted.
    fn admits_target(self, target: &TargetRef) -> bool {
        match self {
            Self::Headline => !target.is_deploy() && Tree::of(&target.file) == Tree::Main,
            Self::WithTestTree => !target.is_deploy(),
            Self::WithDeployEvidence => Tree::of(&target.file) == Tree::Main,
        }
    }

    /// Whether a provider-side configuration source is admitted for its
    /// `server.port`.
    fn admits_provider_source(self, path: &str) -> bool {
        match self {
            Self::Headline | Self::WithDeployEvidence => Tree::of(path) == Tree::Main,
            Self::WithTestTree => true,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Headline => "main-tree application configuration only",
            Self::WithTestTree => "test-tree application configuration admitted too",
            Self::WithDeployEvidence => "deploy evidence admitted as a consumer-side target",
        }
    }
}

// ── The port index ──────────────────────────────────────────────────────────

/// Which member, if any, each committed `server.port` identifies.
#[derive(Debug, Default)]
pub struct PortIndex {
    /// port → the members declaring it as their own `server.port`.
    pub claimants: BTreeMap<String, BTreeSet<String>>,
    /// Declarations read, for the census denominator — larger than
    /// `claimants.len()` wherever one member declares the same port in several
    /// profiles.
    pub declarations: usize,
    /// Members declaring at least one port.
    pub declaring_members: BTreeSet<String>,
}

impl PortIndex {
    /// The member a port identifies, or `None` — either because no member
    /// declares it, or because **two or more** do and a same-tier collision
    /// resolves to nothing rather than to a guess.
    ///
    /// This mirrors [`identity::Corpus::member_for`]'s rule exactly, and
    /// [FR-WS-20] AC2 is where that rule is written. [CR-124]'s own risk table
    /// names the estate's two collisions (`9009`, `9007`) and requires this
    /// outcome for them: *"Two members claiming one port resolve to nothing."*
    ///
    /// [CR-124]: ../../../docs/requests/CR-124-runtime-port-as-a-target-identity-tier.md
    /// [FR-WS-20]: ../../../docs/specs/requirements/FR-WS-20.md
    pub fn member_for(&self, port: &str) -> Option<&str> {
        let claimants = self.claimants.get(port)?;
        match claimants.len() {
            1 => claimants.iter().next().map(String::as_str),
            _ => None, // zero is unreachable through `claimants`; two or more collide
        }
    }

    /// How the estate's ports partition: identifying exactly one member,
    /// claimed by two or more, and (over the ports actually *referenced* by a
    /// caller) claimed by none.
    pub fn resolution_census(&self, referenced: &BTreeMap<String, BTreeSet<String>>) -> PortCensus {
        let mut census = PortCensus::default();
        for port in referenced.keys() {
            match self.claimants.get(port).map_or(0, BTreeSet::len) {
                0 => {
                    census.unclaimed.insert(port.clone());
                }
                1 => {
                    census.exactly_one.insert(port.clone());
                }
                _ => {
                    census.collided.insert(port.clone());
                }
            }
        }
        census
    }
}

/// How the ports a caller actually referenced resolve.
#[derive(Debug, Default)]
pub struct PortCensus {
    pub exactly_one: BTreeSet<String>,
    pub collided: BTreeSet<String>,
    pub unclaimed: BTreeSet<String>,
}

/// Whether a committed configuration value is a port at all.
///
/// A named free function rather than an inline guard inside [`port_index`], for
/// the reason [`identity::classify`] and [`Verdict::decide`] are: [`port_index`]
/// runs only where `LOGOS_REF_WORKSPACE` is set, so a rule expressed inside it
/// has **no coverage** in a default `cargo test` — and this one is a rule, not a
/// formality. A `server.port` of `${PORT}` or `${SERVER_PORT:9000}` is a
/// placeholder the deploy tool substitutes; admitting it would let a member
/// claim a label no caller can ever write, and let two members holding the same
/// placeholder collide and cancel each other's real ports out.
///
/// Found by the falsifiability sweep: replacing the guard with `if false` left
/// every fixture green.
fn is_a_port(value: &str) -> bool {
    !value.is_empty() && value.chars().all(|c| c.is_ascii_digit())
}

/// Read every member's own `server.port` out of the configuration corpus the
/// client-call measurement already discovered.
///
/// `crate::measurement(root).config` rather than a second
/// `ConfigCorpus::discover(root)`: the corpus is behind that measurement's
/// `OnceLock`, so this gate costs no additional traversal of the estate.
fn port_index(root: &Path, members: &BTreeSet<String>, population: Population) -> PortIndex {
    let mut index = PortIndex::default();
    for source in &crate::measurement(root).config.sources {
        if !population.admits_provider_source(&source.path) {
            continue;
        }
        let Some(member) = source.path.split('/').next() else { continue };
        if !members.contains(member) {
            continue;
        }
        let declared = source.values.get(SERVER_PORT_KEY).into_iter().flatten();
        for port in declared.filter(|p| is_a_port(p)) {
            index.declarations += 1;
            index.declaring_members.insert(member.to_string());
            index.claimants.entry(port.clone()).or_default().insert(member.to_string());
        }
    }
    index
}

// ── Pairs ───────────────────────────────────────────────────────────────────

/// The two readings of **third-party**, both measured, because the floor
/// declaration and the experiment it was written to be comparable with do not
/// define the word the same way.
///
/// This is a real disagreement found during the run, and it is reported rather
/// than resolved by fiat — the declaration is committed evidence and
/// [`port_identity_floor.txt`]'s own COMMITMENT section forbids revising it
/// after the fact.
///
/// [`port_identity_floor.txt`]: ./port_identity_floor.txt
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Reading {
    /// **The declaration's literal words**, and therefore the reading the gate
    /// is decided on: *"THIRD-PARTY means, exactly: consumer member != provider
    /// member."*
    AsDeclared,
    /// **[S-384]'s own rule**, which the declaration cites as its motivation
    /// while defining something narrower than it: the tie must be between two
    /// **other** members, so a consumer that itself serves the template has
    /// manufactured the ambiguity it is being credited with resolving.
    ///
    /// [S-384]: ../../../docs/planning/journal.md#s-384-measure-service-identity-resolvability-across-the-deploy-corpus
    AsMeasuredByS384,
}

impl Reading {
    pub fn label(self) -> &'static str {
        match self {
            Self::AsDeclared => "as the floor declaration defines it (consumer != provider)",
            Self::AsMeasuredByS384 => "as S-384 measured it (the tie is between two other members)",
        }
    }
}

/// Whether an edge is one the consumer could not have reached without crossing
/// a service boundary.
///
/// A separate type rather than a `bool` because the whole finding turns on the
/// split reaching the point the headline is printed: [S-384]'s headline of 12
/// was 11 `SelfTie`s and one `ThirdParty`, and a `bool` is exactly the shape
/// that gets dropped on the way to a `println!`.
///
/// [S-384]: ../../../docs/planning/journal.md#s-384-measure-service-identity-resolvability-across-the-deploy-corpus
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Party {
    /// Counted toward the floor.
    ThirdParty,
    /// Reported, never counted toward the floor.
    SelfTie,
}

impl Party {
    pub fn label(self) -> &'static str {
        match self {
            Self::ThirdParty => "third-party",
            Self::SelfTie => "SELF-TIE",
        }
    }
}

/// One judged consumer→provider candidate, bound by port.
#[derive(Debug, Clone)]
pub struct PortPair {
    pub consumer: String,
    pub provider: String,
    pub port: String,
    pub overlay: String,
    pub normalized: String,
    pub via_key: String,
    pub base_key: String,
    pub serving: usize,
    pub class: PairClass,
    /// Whether the **consumer itself** is among the members registering this
    /// template — the discriminator [`Reading::AsMeasuredByS384`] turns on, kept
    /// on the pair so both readings are derived from one measurement rather than
    /// from two passes that could disagree.
    pub consumer_serves: bool,
    pub site: String,
}

impl PortPair {
    /// Which party this edge belongs to, under one reading of "third-party".
    ///
    /// `consumer == provider` is a self-tie under **both** readings, and the
    /// disjunct is not redundant in the second: for
    /// [`PairClass::TargetServesNothing`] the provider registers nothing at this
    /// template, so `consumer_serves` can be false while the two members are the
    /// same one.
    pub fn party(&self, reading: Reading) -> Party {
        let self_tie = match reading {
            Reading::AsDeclared => self.consumer == self.provider,
            Reading::AsMeasuredByS384 => self.consumer == self.provider || self.consumer_serves,
        };
        if self_tie {
            Party::SelfTie
        } else {
            Party::ThirdParty
        }
    }
}

/// Everything one pass of the join measured.
#[derive(Debug, Default)]
pub struct Judgement {
    pub pairs: Vec<PortPair>,
    /// Resolved `base-url` values a consumer call site actually read, counted
    /// once per `(member, key, value)` — the denominator [CR-124] §6's first
    /// report is taken over.
    pub base_urls_read: BTreeSet<(String, String, String)>,
    /// The subset of those that parse to an authority carrying a port.
    pub base_urls_with_port: BTreeSet<(String, String, String)>,
    /// Every port a consumer call site referenced, and which members referenced
    /// it — so a port that resolves to nothing can be adjudicated by a human
    /// against the caller that wrote it, rather than as a bare number.
    pub ports_referenced: BTreeMap<String, BTreeSet<String>>,
    /// Call sites that resolved a template and whose base-URL sibling key no
    /// admitted configuration source proves — the residue, enumerated.
    pub sites_without_base_url: BTreeSet<String>,
    /// Call sites whose base URL resolved but carries no port at all.
    pub sites_without_port: BTreeSet<String>,
    pub sites_considered: usize,
    /// Templates that resolved but do not positionally normalize, so they can
    /// reach no provider. A limit of the reader, not a fact about the estate.
    pub templates_not_normalizable: BTreeSet<String>,
}

/// One cross-service REST edge at the grain [S-384] counts and the declaration
/// fixes: `(consumer member, normalized template, provider member)`.
///
/// [S-384]: ../../../docs/planning/journal.md#s-384-measure-service-identity-resolvability-across-the-deploy-corpus
type Edge<'a> = (&'a str, &'a str, &'a str);

impl Judgement {
    /// Distinct edges in one class and one party, under one reading. Every
    /// headline figure is this function, so no two splits can come apart.
    pub fn edges(&self, class: PairClass, party: Party, reading: Reading) -> BTreeSet<Edge<'_>> {
        self.pairs
            .iter()
            .filter(|p| p.class == class && p.party(reading) == party)
            .map(|p| (p.consumer.as_str(), p.normalized.as_str(), p.provider.as_str()))
            .collect()
    }

    /// **The figure the gate is read off**, under the reading asked for. The
    /// verdict uses [`Reading::AsDeclared`], because the declaration is the
    /// contract; [`Reading::AsMeasuredByS384`] is reported beside it.
    pub fn net_new_third_party(&self, reading: Reading) -> usize {
        self.edges(PairClass::NetNewAmbiguous, Party::ThirdParty, reading).len()
    }

    /// The half [S-384]'s headline hid. Printed beside the figure above, never
    /// added to it.
    ///
    /// [S-384]: ../../../docs/planning/journal.md#s-384-measure-service-identity-resolvability-across-the-deploy-corpus
    pub fn net_new_self_ties(&self, reading: Reading) -> usize {
        self.edges(PairClass::NetNewAmbiguous, Party::SelfTie, reading).len()
    }

    pub fn already_bound(&self, party: Party, reading: Reading) -> usize {
        self.edges(PairClass::AlreadyBoundByPath, party, reading).len()
    }

    pub fn target_serves_nothing(&self, party: Party, reading: Reading) -> usize {
        self.edges(PairClass::TargetServesNothing, party, reading).len()
    }

    /// Distinct member→member couplings among the edges identity actually
    /// produced — the "9 service pairs" unit [CR-124]'s census reports, kept
    /// beside the headline precisely because the Decision Log refused it as the
    /// floor's unit.
    ///
    /// [CR-124]: ../../../docs/requests/CR-124-runtime-port-as-a-target-identity-tier.md
    pub fn couplings(&self, party: Party, reading: Reading) -> BTreeSet<(&str, &str)> {
        self.pairs
            .iter()
            .filter(|p| p.class != PairClass::TargetServesNothing && p.party(reading) == party)
            .map(|p| (p.consumer.as_str(), p.provider.as_str()))
            .collect()
    }

    /// Literal call-site occurrences behind the net-new third-party edges — a
    /// larger figure than the edge count, reported so [CR-124]'s "39 call sites"
    /// census can be read against something rather than against silence.
    ///
    /// [CR-124]: ../../../docs/requests/CR-124-runtime-port-as-a-target-identity-tier.md
    pub fn net_new_third_party_sites(&self, reading: Reading) -> BTreeSet<&str> {
        self.pairs
            .iter()
            .filter(|p| {
                p.class == PairClass::NetNewAmbiguous && p.party(reading) == Party::ThirdParty
            })
            .map(|p| p.site.as_str())
            .collect()
    }
}

/// Judge every consumer call site the client-call arm resolved a template for,
/// binding its target by **port**.
///
/// Deliberately this module's own traversal rather than a parameterisation of
/// [`identity::judge`]: that one reads only `TargetRef::is_deploy` targets, and
/// the population this signal is about is the one it excludes. The risk that
/// buys — a traversal that drifts from its sibling's — is discharged by
/// [`assert_traversal_agrees_with_the_identity_gate`], which asserts on every
/// estate run that both considered the same sites and refused the same
/// templates.
fn judge_ports(
    root: &Path,
    corpus: &Corpus,
    providers: &Providers,
    ports: &PortIndex,
    population: Population,
) -> Judgement {
    let m = crate::measurement(root);
    let mut out = Judgement::default();

    for stats in m.per_language.values() {
        for site in stats.sites.iter().filter(|s| s.gate_admitted) {
            if Tree::of(&site.file) == Tree::Test {
                continue;
            }
            let Some(template) = site.cr115.resolved() else { continue };
            let Some(consumer) = site.file.split('/').next().map(str::to_string) else { continue };
            out.sites_considered += 1;
            let where_ = format!("{}:{}", site.file, site.line);
            let Some(normalized) = normalize_template(template) else {
                out.templates_not_normalizable.insert(format!("{where_}  {template}"));
                continue;
            };

            let mut read_a_base_url = false;
            let mut saw_a_port = false;
            // The base URL of a path key is its sibling under the same prefix:
            // "the same configuration that supplies the path also supplies the
            // base URL" (CR-121 §3.2), which is the rule `identity::judge`
            // applies and the one CR-124's hypothesis is written over.
            for key in site.key_outcomes.iter().flatten().filter_map(|o| o.key()) {
                let Some((prefix, _)) = key.rsplit_once('.') else { continue };
                let base_flat = flat_key(&format!("{prefix}.base-url"));
                for target in corpus.targets.iter().filter(|t| {
                    t.member == consumer
                        && t.via_flat == base_flat
                        && population.admits_target(t)
                }) {
                    read_a_base_url = true;
                    judge_one_target(&mut out, target, &consumer);
                    let Some(port) = target.port.as_deref() else { continue };
                    saw_a_port = true;
                    out.ports_referenced
                        .entry(port.to_string())
                        .or_default()
                        .insert(consumer.clone());
                    let Some(provider) = ports.member_for(port) else { continue };
                    let serving = providers.serving(&normalized);
                    out.pairs.push(PortPair {
                        consumer: consumer.clone(),
                        provider: provider.to_string(),
                        port: port.to_string(),
                        overlay: target.overlay.clone(),
                        normalized: normalized.clone(),
                        via_key: target.via_key.clone(),
                        base_key: base_flat.clone(),
                        serving: serving.len(),
                        class: identity::classify(&serving, provider),
                        consumer_serves: serving.contains(consumer.as_str()),
                        site: where_.clone(),
                    });
                }
            }
            if !read_a_base_url {
                out.sites_without_base_url.insert(where_);
            } else if !saw_a_port {
                out.sites_without_port.insert(where_);
            }
        }
    }
    out
}

/// Record one `base-url` value a call site read, in the two census populations
/// [CR-124] §6's first report is taken over.
///
/// A named free function rather than two `insert`s inside [`judge_ports`]'s
/// innermost loop, because the run reports **18 read and 18 carrying a port** —
/// a figure that reads exactly like a counter incremented in the
/// port-resolution branch, i.e. equal by construction. It is not: the reader can
/// see here that `base_urls_read` is written unconditionally, and
/// [`a_base_url_with_no_port_is_still_a_base_url_that_was_read`] pins it without
/// needing the estate.
///
/// [CR-124]: ../../../docs/requests/CR-124-runtime-port-as-a-target-identity-tier.md
fn judge_one_target(out: &mut Judgement, target: &TargetRef, consumer: &str) {
    let value = (
        consumer.to_string(),
        target.via_key.clone(),
        format!("{}://{}", target.scheme, authority_of(target)),
    );
    if target.port.is_some() {
        out.base_urls_with_port.insert(value.clone());
    }
    out.base_urls_read.insert(value);
}

/// The authority a [`TargetRef`] was parsed from, reassembled for the census.
///
/// The reference keeps the host **label** and the port separately, so this is
/// the closest honest reconstruction — enough to distinguish two values in the
/// census, and deliberately not presented as the original text.
fn authority_of(target: &TargetRef) -> String {
    match &target.port {
        Some(port) => format!("{}:{port}", target.label),
        None => target.label.clone(),
    }
}

// ── The verdict ─────────────────────────────────────────────────────────────

/// What the gate returns. `Void` exists because of [NFR-CC-04]: a run that
/// could not see the estate has produced **no** finding, and reporting it as
/// zero net-new edges reads as a falsification that nobody measured.
///
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// No estate was configured, so nothing was measured.
    Void,
    /// Measured, and below the floor.
    Falsified { measured: usize, floor: usize },
    /// Measured, and at or above the floor.
    Holds { measured: usize, floor: usize },
}

impl Verdict {
    /// The gate's whole decision, as a pure function of what was measured.
    ///
    /// A free function of an `Option` rather than a branch inside the test, for
    /// the reason [`identity::classify`] is one: the test body runs only where
    /// `LOGOS_REF_WORKSPACE` is set, so a decision expressed there has no
    /// coverage at all in a default `cargo test` — [S-384]'s review finding 16.
    /// Here `None` is "the estate was not visible", which is the case the
    /// requirement is about and the one a branch inside the estate arm can never
    /// reach.
    ///
    /// [S-384]: ../../../docs/planning/journal.md#s-384-measure-service-identity-resolvability-across-the-deploy-corpus
    pub fn decide(measured: Option<usize>, floor: usize) -> Self {
        match measured {
            None => Self::Void,
            Some(measured) if measured >= floor => Self::Holds { measured, floor },
            Some(measured) => Self::Falsified { measured, floor },
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Void => "VOID",
            Self::Falsified { .. } => "FALSIFIED",
            Self::Holds { .. } => "HOLDS",
        }
    }
}

// ── The report ──────────────────────────────────────────────────────────────

/// Print the census. One section per acceptance criterion, in the order
/// [CR-124] §6 states them, so the output reads against the story without a
/// decoder — the shape `identity::report` already uses.
///
/// [CR-124]: ../../../docs/requests/CR-124-runtime-port-as-a-target-identity-tier.md
fn report(ports: &PortIndex, j: &Judgement, counterfactuals: &[(Population, Judgement)], members: usize) {
    println!("\n=== S-400 · does a runtime port identify the callee? ===");
    println!(
        "members {members} · members declaring their own `{SERVER_PORT_KEY}` {} \
         ({} declarations over {} distinct ports) · consumer call sites considered {}",
        ports.declaring_members.len(),
        ports.declarations,
        ports.claimants.len(),
        j.sites_considered,
    );
    report_base_urls(j);
    report_ports(ports, j);
    report_pairs(j);
    report_counterfactuals(j, counterfactuals);
}

/// [CR-124] §6, report 1: how many resolved `base-url` values carry a port.
///
/// [CR-124]: ../../../docs/requests/CR-124-runtime-port-as-a-target-identity-tier.md
fn report_base_urls(j: &Judgement) {
    let read = j.base_urls_read.len();
    let with = j.base_urls_with_port.len();
    println!("\n-- resolved base-url values at consumer call sites --");
    println!(
        "  read {read} · carrying a port {with} · carrying none {}",
        read.saturating_sub(with),
    );
    println!(
        "  call sites whose base-URL key no admitted source proves: {}",
        j.sites_without_base_url.len(),
    );
    println!("  call sites whose base URL carries no port: {}", j.sites_without_port.len());
    if !j.templates_not_normalizable.is_empty() {
        println!(
            "  templates that resolved but do not positionally normalize (a limit of the \
             reader, not of the estate): {}",
            j.templates_not_normalizable.len(),
        );
        for t in &j.templates_not_normalizable {
            println!("      {t}");
        }
    }
}

/// [CR-124] §6, report 2: how the referenced ports resolve, with the ones that
/// resolve to nothing **enumerated for human adjudication**.
///
/// [CR-124]: ../../../docs/requests/CR-124-runtime-port-as-a-target-identity-tier.md
fn report_ports(ports: &PortIndex, j: &Judgement) {
    let census = ports.resolution_census(&j.ports_referenced);
    let referrers = |port: &str| {
        j.ports_referenced
            .get(port)
            .into_iter()
            .flatten()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(", ")
    };
    println!("\n-- ports referenced by a consumer, and what they identify --");
    println!(
        "  referenced {} · identifying exactly one member {} · claimed by two or more {} \
         · claimed by none {}",
        j.ports_referenced.len(),
        census.exactly_one.len(),
        census.collided.len(),
        census.unclaimed.len(),
    );
    for port in &census.exactly_one {
        println!(
            "    {port:>6}  ->  {}    referenced by {}",
            ports.member_for(port).unwrap_or("<none>"),
            referrers(port),
        );
    }
    if !census.collided.is_empty() {
        println!(
            "  claimed by two or more members, so they resolve to NOTHING (FR-WS-20 AC2's \
             same-tier collision rule, NFR-RA-05):"
        );
        for port in &census.collided {
            let claimants: Vec<&str> = ports
                .claimants
                .get(port)
                .into_iter()
                .flatten()
                .map(String::as_str)
                .collect();
            println!(
                "    {port:>6}  ->  NOTHING; claimed by {}    referenced by {}",
                claimants.join(", "),
                referrers(port),
            );
        }
    }
    if !census.unclaimed.is_empty() {
        println!("  claimed by no member — enumerated for human adjudication:");
        for port in &census.unclaimed {
            println!(
                "    {port:>6}  ->  no member declares this as its own {SERVER_PORT_KEY}    \
                 referenced by {}",
                referrers(port),
            );
        }
    }
}

/// [CR-124] §6, reports 3 and 4: the pairs, split by whether path-only matching
/// would already have bound them **and** by whether they cross a service
/// boundary — both splits at the point the headline appears.
///
/// [CR-124]: ../../../docs/requests/CR-124-runtime-port-as-a-target-identity-tier.md
fn report_pairs(j: &Judgement) {
    println!("\n-- consumer->provider edges (consumer, normalized template, provider) --");
    println!(
        "  Two readings of THIRD-PARTY are printed, because the floor declaration and the\n  \
         experiment it was written to be comparable with do not define the word the same way.\n  \
         The VERDICT is read off the declared one; the declaration is the contract and is not\n  \
         revised after a run."
    );
    for reading in [Reading::AsDeclared, Reading::AsMeasuredByS384] {
        println!("\n  reading: {}", reading.label());
        println!("  {:<38} {:>12} {:>12}", "", "third-party", "SELF-TIE");
        for (label, class) in [
            ("NET-NEW (path-only ties, >=2 serving)", PairClass::NetNewAmbiguous),
            ("already bound by path alone", PairClass::AlreadyBoundByPath),
            ("port target serves no such route", PairClass::TargetServesNothing),
        ] {
            println!(
                "  {label:<38} {:>12} {:>12}",
                j.edges(class, Party::ThirdParty, reading).len(),
                j.edges(class, Party::SelfTie, reading).len(),
            );
        }
        println!(
            "  {:<38} {:>12} {:>12}",
            "distinct member->member couplings",
            j.couplings(Party::ThirdParty, reading).len(),
            j.couplings(Party::SelfTie, reading).len(),
        );
        println!(
            "  {:<38} {:>12} {:>12}",
            "literal call sites behind NET-NEW",
            j.net_new_third_party_sites(reading).len(),
            "-",
        );
    }
    report_edge_detail(j);
}

/// Every judged edge, with the evidence that bound it. Grouped by the **S-384
/// reading**, which is the stricter of the two: an edge it calls a self-tie is
/// one the consumer serves itself, and saying so beside the edge is what lets a
/// human check the split rather than take it.
fn report_edge_detail(j: &Judgement) {
    for (party, class) in [
        (Party::ThirdParty, PairClass::NetNewAmbiguous),
        (Party::SelfTie, PairClass::NetNewAmbiguous),
        (Party::ThirdParty, PairClass::AlreadyBoundByPath),
        (Party::SelfTie, PairClass::AlreadyBoundByPath),
    ] {
        let edges = j.edges(class, party, Reading::AsMeasuredByS384);
        if edges.is_empty() {
            continue;
        }
        println!("\n  {} · {} (S-384 reading):", class.label(), party.label());
        for (consumer, template, provider) in edges {
            println!("    {consumer}  --->  {provider}    {template}");
            // Every call site behind the edge, with the evidence that bound it:
            // an edge is adjudicable only if the reader can see WHICH key in
            // WHICH profile carried the port, how many members serve the
            // template it landed on, and whether the consumer is one of them.
            for e in j.pairs.iter().filter(|p| {
                p.consumer == consumer && p.normalized == template && p.provider == provider
            }) {
                println!(
                    "        :{}  via {} [{}] (flat {}) · {} member(s) serve this template{} · {}",
                    e.port,
                    e.via_key,
                    e.overlay,
                    e.base_key,
                    e.serving,
                    if e.consumer_serves { ", INCLUDING THE CONSUMER" } else { "" },
                    e.site,
                );
            }
        }
    }
}

/// The headline's two judgement calls, quantified rather than asserted: what
/// each wider population would have done to the figure the gate is read off.
///
/// The delta is printed beside each, because the number that matters is how far
/// the verdict is from the boundary under a different reading — not the reading
/// itself.
fn report_counterfactuals(headline: &Judgement, counterfactuals: &[(Population, Judgement)]) {
    println!("\n-- counterfactuals (reported, never the headline) --");
    println!("  headline population: {}", Population::Headline.label());
    for (population, c) in counterfactuals {
        for reading in [Reading::AsDeclared, Reading::AsMeasuredByS384] {
            println!(
                "  {:<52} net-new third-party {:>3} ({:+}) · self-ties {:>3}   [{}]",
                population.label(),
                c.net_new_third_party(reading),
                c.net_new_third_party(reading) as i64
                    - headline.net_new_third_party(reading) as i64,
                c.net_new_self_ties(reading),
                reading.label(),
            );
        }
    }
}

/// The reconciliation [CR-124] §6 requires: this reading against [S-384]'s, on
/// the same estate, naming what differs rather than leaving two numbers side by
/// side.
///
/// [CR-124]: ../../../docs/requests/CR-124-runtime-port-as-a-target-identity-tier.md
/// [S-384]: ../../../docs/planning/journal.md#s-384-measure-service-identity-resolvability-across-the-deploy-corpus
fn report_reconciliation(j: &Judgement, s384: &identity::Findings) -> usize {
    let s384_third_party = s384_net_new_third_party(s384);
    println!("\n-- reconciliation against S-384, same estate, same process --");
    println!(
        "  S-384 (host label, deploy evidence):      net-new {:>3}  third-party {s384_third_party:>3}  \
         self-tie {:>3}",
        s384.net_new(),
        s384.net_new() - s384_third_party,
    );
    let reading = Reading::AsMeasuredByS384;
    println!(
        "  S-400 (runtime port, application config): net-new {:>3}  third-party {:>3}  self-tie {:>3}",
        j.net_new_third_party(reading) + j.net_new_self_ties(reading),
        j.net_new_third_party(reading),
        j.net_new_self_ties(reading),
    );
    println!(
        "\n  Both rows are computed in THIS process, from the same walk, and BOTH third-party\n  \
         columns use S-384's own rule applied to each experiment's own pairs — a reproduction,\n  \
         not a quotation of its finding text. The S-384 row reproducing its recorded 1 is\n  \
         ASSERTED, not eyeballed; if it ever stops doing so, the two readings are not\n  \
         comparable and this run fails rather than printing two numbers side by side."
    );
    println!(
        "\n  WHAT DIFFERS BETWEEN THE TWO SIGNALS\n  \
         S-384 joins the first DNS label of a DEPLOY-overridden base URL\n  \
         (`http://official-log-export-api.pec-services.svc.cluster.local:9020`) to a member's\n  \
         Service / chart / Compose / directory / application name. S-400 joins the PORT of the\n  \
         APPLICATION-committed base URL (`http://localhost:9020`) to that member's own\n  \
         `{SERVER_PORT_KEY}`, and reads no deploy evidence at all — `TargetRef::is_deploy` is\n  \
         the partition, and `Population::Headline` enforces it.\n\n  \
         The two readings are therefore NOT contradictory. They are taken over DISJOINT target\n  \
         populations, and each is silent on the other's: every application-committed base URL\n  \
         on this estate has the host label `localhost`, which no member claims, so S-384's\n  \
         ladder produces nothing there by construction rather than refusing anything; and no\n  \
         deploy values file is read here at all. A third signal reading BOTH is reported as the\n  \
         `deploy evidence admitted` counterfactual above, and it adds nothing on this estate —\n  \
         the same edges, because the deploy override and the application default name the same\n  \
         port.\n\n  \
         What the two DO agree on is the shape of the shortfall: in both experiments the\n  \
         ambiguity identity discharges is overwhelmingly one the consumer created against\n  \
         itself by re-registering its callee's template on its own controller."
    );
    s384_third_party
}

/// [S-384]'s net-new figure, re-split by [`Reading::AsMeasuredByS384`] — its own
/// definition of a self-tie, applied to its own pairs, in this process.
///
/// Computed rather than quoted. The alternative is to restate "1" from
/// `identity_finding.txt`, which would make the reconciliation a comparison of
/// this run against a piece of prose that no longer has to be true.
///
/// [S-384]: ../../../docs/planning/journal.md#s-384-measure-service-identity-resolvability-across-the-deploy-corpus
fn s384_net_new_third_party(s384: &identity::Findings) -> usize {
    s384.pairs
        .iter()
        .filter(|p| p.class == PairClass::NetNewAmbiguous)
        .filter(|p| {
            p.consumer != p.provider
                && !s384.providers.serving(&p.normalized).contains(p.consumer.as_str())
        })
        .map(|p| (p.consumer.as_str(), p.normalized.as_str(), p.provider.as_str()))
        .collect::<BTreeSet<_>>()
        .len()
}

// ── The measurement ─────────────────────────────────────────────────────────

/// **S-400's blocking gate.** Skips — loudly, and as [`Verdict::Void`] rather
/// than as a zero — when no corpus is configured, so `cargo test --workspace`
/// stays green on a machine without one ([NFR-CC-04]).
///
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
#[test]
fn measure_port_target_identity_over_the_reference_workspace() {
    let Some(root) = crate::corpus_root() else {
        eprintln!(
            "SKIPPED — VERDICT: {}. No LOGOS_REF_WORKSPACE, so S-400's port-identity gate \
             measured NOTHING; this is not a net-new figure of zero and must never be read \
             as one (NFR-CC-04). Set LOGOS_REF_WORKSPACE=<path to the reference workspace> \
             to run it; see port_identity_finding.txt for the recorded finding.",
            Verdict::decide(None, NET_NEW_THIRD_PARTY_FLOOR).label(),
        );
        return;
    };
    let s384 = identity::findings(&root);
    let judge = |population: Population| {
        let ports = port_index(&root, &s384.corpus.members, population);
        let judgement = judge_ports(&root, &s384.corpus, &s384.providers, &ports, population);
        (ports, judgement)
    };
    let (ports, j) = judge(Population::Headline);
    let counterfactuals: Vec<(Population, Judgement)> =
        [Population::WithTestTree, Population::WithDeployEvidence]
            .into_iter()
            .map(|population| (population, judge(population).1))
            .collect();

    report(&ports, &j, &counterfactuals, s384.corpus.members.len());
    let s384_third_party = report_reconciliation(&j, s384);
    println!("\n{RECORDED_FINDING}");

    assert_traversal_agrees_with_the_identity_gate(&j, s384);
    assert_eq!(
        s384_third_party, S384_RECORDED_NET_NEW_THIRD_PARTY,
        "re-splitting S-384's own net-new pairs by S-384's own self-tie rule yields \
         {s384_third_party} third-party edges, not the \
         {S384_RECORDED_NET_NEW_THIRD_PARTY} its finding records. \
         Either this gate's self-tie rule is not S-384's, in which case the two readings are \
         not comparable and the reconciliation is meaningless, or S-384's finding text has \
         drifted from the measurement it describes. Resolve which before reading the verdict \
         below.",
    );
    assert_estate_engaged(&j, &ports, s384);

    // The verdict is read off the DECLARED reading: the declaration is the
    // contract, and `port_identity_floor.txt`'s COMMITMENT section forbids
    // revising it after a run. S-384's stricter reading is printed beside it,
    // and asserted to be below the floor too — so nothing about the verdict
    // turns on which definition of "third-party" is taken.
    let declared = j.net_new_third_party(Reading::AsDeclared);
    let as_s384 = j.net_new_third_party(Reading::AsMeasuredByS384);
    let verdict = Verdict::decide(Some(declared), NET_NEW_THIRD_PARTY_FLOOR);
    println!(
        "\nVERDICT: net-new THIRD-PARTY port-resolved edges {declared} (self-ties {}, reported \
         apart and not counted) against a floor of {NET_NEW_THIRD_PARTY_FLOOR} declared before \
         the run  =>  {}\n  \
         read {}.\n  \
         Under {} the figure is {as_s384} (self-ties {}) — also below the floor, so the verdict \
         is invariant under the disagreement. See the finding for why the two differ.",
        j.net_new_self_ties(Reading::AsDeclared),
        verdict.label(),
        Reading::AsDeclared.label(),
        Reading::AsMeasuredByS384.label(),
        j.net_new_self_ties(Reading::AsMeasuredByS384),
    );

    // The recorded finding. S-400 measured this as FALSIFIED; the assertion pins
    // the verdict so a change that flips it has to be decided rather than
    // absorbed. Re-open CR-124 and re-decide it before relaxing this — do not
    // edit it to make the suite green.
    assert_eq!(
        verdict,
        Verdict::Falsified {
            measured: RECORDED_NET_NEW_THIRD_PARTY,
            floor: NET_NEW_THIRD_PARTY_FLOOR,
        },
        "S-400's recorded finding is that the port signal yields \
         {RECORDED_NET_NEW_THIRD_PARTY} net-new third-party edges under the declared reading, \
         below the floor of {NET_NEW_THIRD_PARTY_FLOOR} declared before the run; this run \
         decided {verdict:?}. If that is real, CR-124's gate has re-opened: re-decide the CR \
         and plan a delivery story — do not relax this assertion.",
    );
    assert_eq!(
        as_s384, RECORDED_NET_NEW_THIRD_PARTY_AS_S384,
        "the S-384 reading of the headline moved from {RECORDED_NET_NEW_THIRD_PARTY_AS_S384} \
         to {as_s384}",
    );
    assert!(
        as_s384 < NET_NEW_THIRD_PARTY_FLOOR,
        "the S-384 reading reached {as_s384}, at or above the floor, while the declared \
         reading falsified. The verdict would then rest on which definition of `third-party` \
         is taken — which is a finding to adjudicate, not a number to absorb.",
    );
    assert_eq!(
        j.net_new_self_ties(Reading::AsMeasuredByS384),
        RECORDED_NET_NEW_SELF_TIES_AS_S384,
        "the self-tie half of the headline moved from {RECORDED_NET_NEW_SELF_TIES_AS_S384} to \
         {}; it is pinned because S-384's finding turned on exactly this figure being \
         reported apart from the third-party one",
        j.net_new_self_ties(Reading::AsMeasuredByS384),
    );
    assert_eq!(
        j.already_bound(Party::ThirdParty, Reading::AsDeclared),
        RECORDED_ALREADY_BOUND_THIRD_PARTY,
        "the already-bound-by-path third-party column moved from \
         {RECORDED_ALREADY_BOUND_THIRD_PARTY} to {}; a change that moved edges between the \
         two columns while leaving the total alone is exactly the change that would matter",
        j.already_bound(Party::ThirdParty, Reading::AsDeclared),
    );
    // Both counterfactuals are pinned, and both must stay below the floor under
    // BOTH readings: a falsification that survives only under one of the
    // headline's own choices rests on that choice rather than on the
    // measurement.
    for (population, recorded) in [
        (Population::WithTestTree, RECORDED_NET_NEW_THIRD_PARTY_WITH_TEST),
        (Population::WithDeployEvidence, RECORDED_NET_NEW_THIRD_PARTY_WITH_DEPLOY),
    ] {
        let c = counterfactuals
            .iter()
            .find(|(p, _)| *p == population)
            .map(|(_, c)| c)
            .expect("every counterfactual named here is also computed");
        let measured = c.net_new_third_party(Reading::AsDeclared);
        assert_eq!(
            measured,
            recorded,
            "the `{}` counterfactual moved from {recorded} to {measured}",
            population.label(),
        );
        for reading in [Reading::AsDeclared, Reading::AsMeasuredByS384] {
            assert!(
                c.net_new_third_party(reading) < NET_NEW_THIRD_PARTY_FLOOR,
                "the `{}` counterfactual reached {} under `{}`, at or above the floor of \
                 {NET_NEW_THIRD_PARTY_FLOOR}, while the headline falsified. The falsification \
                 would then rest on the population chosen rather than on the measurement, \
                 which is a finding to adjudicate — not a number to absorb.",
                population.label(),
                c.net_new_third_party(reading),
                reading.label(),
            );
        }
    }
}

/// **The twin check.** This module owns its own traversal of the resolved call
/// sites — see [`judge_ports`] for why it must — and a traversal copied from a
/// sibling is exactly the divergence that makes one of two measurements quietly
/// wrong while both stay green.
///
/// So the agreement is asserted on every estate run rather than promised in a
/// comment: both gates must have considered the **same** call sites and refused
/// the **same** templates. Only what each does with a site afterwards may
/// differ, which is the whole experiment.
fn assert_traversal_agrees_with_the_identity_gate(j: &Judgement, s384: &identity::Findings) {
    assert_eq!(
        j.sites_considered, s384.sites_considered,
        "S-400 considered {} consumer call sites and S-384 considered {} over the same \
         estate in the same process. The two traversals have diverged, so the reconciliation \
         between the readings compares different populations and means nothing.",
        j.sites_considered, s384.sites_considered,
    );
    assert_eq!(
        j.templates_not_normalizable, s384.templates_not_normalizable,
        "the two gates refused different templates as non-normalizable, so one of the two \
         traversals has drifted from the other",
    );
}

/// The census floors that separate "measured and found nothing" from "measured
/// nothing at all". Every one of these counters could be neutered with the
/// verdict still printing FALSIFIED and the embedded finding still showing its
/// populated figures beside them.
///
/// Floors rather than equalities: they can only grow as the estate grows, and a
/// re-clone must not redden the run.
fn assert_estate_engaged(j: &Judgement, ports: &PortIndex, s384: &identity::Findings) {
    assert!(
        s384.corpus.members.len() >= 80,
        "the estate yielded {} members, fewer than the 84 recorded — point \
         LOGOS_REF_WORKSPACE at the reference estate rather than at a single repository",
        s384.corpus.members.len(),
    );
    assert!(
        s384.providers.files_gated > 0,
        "no file in the estate passed the FR-FW-04 ledger gate, so the provider side of \
         every pair is empty and every figure here is vacuous",
    );
    let census: [(&str, usize, usize); 5] = [
        ("consumer call sites considered", j.sites_considered, 20),
        ("members declaring their own server.port", ports.declaring_members.len(), 15),
        ("distinct server.port values", ports.claimants.len(), 15),
        ("resolved base-url values read at a call site", j.base_urls_read.len(), 5),
        ("ports referenced by a consumer", j.ports_referenced.len(), 3),
    ];
    for (what, measured, floor) in census {
        assert!(
            measured >= floor,
            "the {what} census read {measured}, under its floor of {floor}. The verdict may \
             still print, but a census at zero cannot attribute the falsification to the join \
             that broke — and a zero that reads as a falsification is the one outcome \
             NFR-CC-04 forbids.",
        );
    }
}

/// [S-384]'s genuine third-party contribution, as its own finding records it:
/// *"identity's genuine THIRD-PARTY contribution, where the tie is between two
/// other members, is ONE edge."*
///
/// Pinned here so the reconciliation is a **reproduction** of that figure rather
/// than a quotation of it: [`s384_net_new_third_party`] recomputes it from
/// S-384's pairs with this module's [`Party`] rule, and the estate run asserts
/// the two agree. If they ever do not, either the self-tie rule here is not
/// S-384's or its finding text has drifted — and the reconciliation the story
/// requires would be comparing two different things.
///
/// [S-384]: ../../../docs/planning/journal.md#s-384-measure-service-identity-resolvability-across-the-deploy-corpus
pub const S384_RECORDED_NET_NEW_THIRD_PARTY: usize = 1;

/// **The figure the gate was read off** — [`Reading::AsDeclared`], the floor
/// declaration's own definition — pinned so a drift in the port join or in the
/// provider scan is a failure rather than a quietly different number.
pub const RECORDED_NET_NEW_THIRD_PARTY: usize = 3;

/// The same headline under [`Reading::AsMeasuredByS384`] — the stricter of the
/// two definitions, and the one S-384's own finding uses.
pub const RECORDED_NET_NEW_THIRD_PARTY_AS_S384: usize = 0;

/// The self-tie half of the headline under S-384's reading, pinned for the
/// reason its assertion gives.
pub const RECORDED_NET_NEW_SELF_TIES_AS_S384: usize = 3;

/// The already-bound-by-path third-party column.
pub const RECORDED_ALREADY_BOUND_THIRD_PARTY: usize = 8;

/// Net-new third-party with test-tree configuration admitted — the sensitivity
/// of the headline to the first of its two judgement calls.
pub const RECORDED_NET_NEW_THIRD_PARTY_WITH_TEST: usize = 3;

/// Net-new third-party with deploy evidence admitted as a consumer-side target.
///
/// Reported to be **subtracted**: the edges it adds are bound by a host label
/// [S-384] already resolves, so they are that experiment's product and not this
/// one's.
///
/// [S-384]: ../../../docs/planning/journal.md#s-384-measure-service-identity-resolvability-across-the-deploy-corpus
pub const RECORDED_NET_NEW_THIRD_PARTY_WITH_DEPLOY: usize = 3;

// ── Fixtures ────────────────────────────────────────────────────────────────
//
// The estate measurement runs only where a corpus is configured, so every rule
// the gate depends on is pinned here too — including, and especially, the
// classifier that decides the gate (CR-124 §6: "always-run coverage that does
// not depend on LOGOS_REF_WORKSPACE being set"). Each matcher is probed with the
// near miss it must reject, not only with the case it must accept.

#[cfg(test)]
mod fixtures {
    use super::*;

    fn index_of(claims: &[(&str, &str)]) -> PortIndex {
        let mut index = PortIndex::default();
        for (member, port) in claims {
            index.declarations += 1;
            index.declaring_members.insert((*member).to_string());
            index
                .claimants
                .entry((*port).to_string())
                .or_default()
                .insert((*member).to_string());
        }
        index
    }

    fn referenced_ports(ports: &[&str]) -> BTreeMap<String, BTreeSet<String>> {
        ports
            .iter()
            .map(|p| ((*p).to_string(), ["a-consumer".to_string()].into_iter().collect()))
            .collect()
    }

    fn judgement(pairs: Vec<PortPair>) -> Judgement {
        Judgement { pairs, ..Judgement::default() }
    }

    fn pair_serving(
        consumer: &str,
        provider: &str,
        template: &str,
        class: PairClass,
        consumer_serves: bool,
    ) -> PortPair {
        PortPair {
            consumer: consumer.to_string(),
            provider: provider.to_string(),
            port: "9000".to_string(),
            overlay: "application:<none>".to_string(),
            normalized: template.to_string(),
            via_key: "x.api.base-url".to_string(),
            base_key: flat_key("x.api.base-url"),
            serving: 2,
            class,
            consumer_serves,
            site: format!("{consumer}/src/main/java/X.java:1"),
        }
    }

    /// A pair whose consumer does NOT serve the template itself — third-party
    /// under both readings, so a fixture about something else is not silently
    /// also about the self-tie rule.
    fn pair(consumer: &str, provider: &str, template: &str, class: PairClass) -> PortPair {
        pair_serving(consumer, provider, template, class, false)
    }

    // ── The floor ───────────────────────────────────────────────────────────

    #[test]
    fn the_floor_is_the_one_declared_before_the_run() {
        // Reads the DECLARATION, not the constant. Asserting
        // `NET_NEW_THIRD_PARTY_FLOOR == 16` would be a comparison of a value
        // with the literal written a few hundred lines above it, which no
        // mutation can falsify and which says nothing about what was declared
        // before the run.
        let declared: usize = DECLARED_FLOOR
            .lines()
            .find_map(|l| l.trim().strip_prefix(">= ")?.split_whitespace().next()?.parse().ok())
            .expect("the declaration states its floor as a `>= NN ...` line");
        assert_eq!(
            declared, NET_NEW_THIRD_PARTY_FLOOR,
            "NET_NEW_THIRD_PARTY_FLOOR is {NET_NEW_THIRD_PARTY_FLOOR} but the floor declared \
             before the run was {declared}. The declaration is the record; change the \
             constant only by re-deciding CR-124, never to make a run clear it.",
        );
        assert!(
            DECLARED_FLOOR.contains("2026-09-13T13:13:50Z"),
            "the declaration must carry the UTC timestamp that makes it a floor rather than \
             a result",
        );
        // The three words the floor turns on must be in the declaration, or the
        // unit it fixes is only in this file's comments.
        for word in ["NET-NEW", "THIRD-PARTY", "call-site edges"] {
            assert!(
                DECLARED_FLOOR.contains(word),
                "the declaration must fix the term `{word}`, or the metric is defined only \
                 in the measurement code it is supposed to constrain",
            );
        }
        const {
            assert!(
                RECORDED_NET_NEW_THIRD_PARTY < NET_NEW_THIRD_PARTY_FLOOR,
                "the recorded finding is FALSIFIED; if that changes, re-decide CR-124 rather \
                 than relaxing this",
            );
        }
    }

    // ── The classifier that decides the gate ────────────────────────────────

    #[test]
    fn an_estate_blind_run_is_void_and_never_a_zero() {
        // NFR-CC-04, and the whole reason `Verdict` has three variants rather
        // than being a `bool`: a run that saw no estate produced no finding, and
        // 0 net-new edges is a finding.
        assert_eq!(Verdict::decide(None, 16), Verdict::Void);
        assert_ne!(Verdict::decide(None, 16), Verdict::decide(Some(0), 16));
        assert_eq!(Verdict::decide(None, 16).label(), "VOID");
    }

    #[test]
    fn the_gate_falsifies_strictly_below_the_floor_and_holds_at_it() {
        assert_eq!(Verdict::decide(Some(0), 16), Verdict::Falsified { measured: 0, floor: 16 });
        // The near miss on each side of the boundary: one below falsifies, the
        // floor itself holds. `>= 16` is what the declaration says.
        assert_eq!(Verdict::decide(Some(15), 16), Verdict::Falsified { measured: 15, floor: 16 });
        assert_eq!(Verdict::decide(Some(16), 16), Verdict::Holds { measured: 16, floor: 16 });
        assert_eq!(Verdict::decide(Some(17), 16), Verdict::Holds { measured: 17, floor: 16 });
    }

    #[test]
    fn a_self_tie_is_never_counted_toward_the_floor_under_either_reading() {
        // A headline of four net-new edges where three are a consumer tying to
        // itself is a headline of ONE, under both definitions of "third-party".
        let j = judgement(vec![
            pair("funnel-aggregator-api", "mailbox-api", "/v1/u/{}", PairClass::NetNewAmbiguous),
            pair("mailbox-api", "mailbox-api", "/v1/u/{}", PairClass::NetNewAmbiguous),
            pair("archive-api", "archive-api", "/v1/archives/{}", PairClass::NetNewAmbiguous),
            pair("webmail", "webmail", "/v1/mail/{}", PairClass::NetNewAmbiguous),
        ]);
        for reading in [Reading::AsDeclared, Reading::AsMeasuredByS384] {
            assert_eq!(j.net_new_third_party(reading), 1, "{}", reading.label());
            assert_eq!(j.net_new_self_ties(reading), 3, "{}", reading.label());
        }
        assert_eq!(
            Verdict::decide(Some(j.net_new_third_party(Reading::AsDeclared)), 16),
            Verdict::Falsified { measured: 1, floor: 16 },
        );
    }

    #[test]
    fn the_two_readings_of_third_party_disagree_exactly_where_the_consumer_serves() {
        // THE defect S-384 exposed, and the one place the two definitions come
        // apart. The provider is a different member, so the declaration's
        // literal `consumer != provider` calls this third-party; S-384's rule
        // sees that the consumer re-registered its callee's template on its own
        // controller, so the ambiguity the port "resolved" was self-inflicted.
        let j = judgement(vec![pair_serving(
            "mailbox-aggregator-api",
            "official-log-export-api",
            "/v1/users/{}/mailboxes/{}/official-log",
            PairClass::NetNewAmbiguous,
            true,
        )]);
        assert_eq!(j.net_new_third_party(Reading::AsDeclared), 1);
        assert_eq!(j.net_new_self_ties(Reading::AsDeclared), 0);
        assert_eq!(j.net_new_third_party(Reading::AsMeasuredByS384), 0);
        assert_eq!(j.net_new_self_ties(Reading::AsMeasuredByS384), 1);

        // …and they agree everywhere else. The near miss on member identity:
        // `archive-api` and `archive-api-logiclens-fork` are two members on this
        // estate, so a prefix or containment test would merge them.
        let agree = judgement(vec![
            pair("archive-api", "archive-api-logiclens-fork", "/v1/a/{}", PairClass::NetNewAmbiguous),
            pair_serving("a2", "a2", "/v1/b/{}", PairClass::NetNewAmbiguous, false),
        ]);
        for reading in [Reading::AsDeclared, Reading::AsMeasuredByS384] {
            assert_eq!(agree.net_new_third_party(reading), 1, "{}", reading.label());
            assert_eq!(agree.net_new_self_ties(reading), 1, "{}", reading.label());
        }
    }

    #[test]
    fn an_edge_already_bound_by_path_is_not_evidence_for_the_port() {
        let j = judgement(vec![
            pair("a", "b", "/v1/x/{}", PairClass::NetNewAmbiguous),
            pair("a", "c", "/v1/y/{}", PairClass::AlreadyBoundByPath),
            pair("a", "d", "/v1/z/{}", PairClass::TargetServesNothing),
        ]);
        let r = Reading::AsDeclared;
        assert_eq!(j.net_new_third_party(r), 1);
        assert_eq!(j.already_bound(Party::ThirdParty, r), 1);
        assert_eq!(j.target_serves_nothing(Party::ThirdParty, r), 1);
    }

    #[test]
    fn two_call_sites_at_one_template_are_one_edge() {
        // The floor's unit. Without the de-duplication a class with two `.uri(…)`
        // calls at the same template would count twice, and the gate would be
        // read off a figure S-384 never measured.
        let r = Reading::AsDeclared;
        let mut second = pair("a", "b", "/v1/x/{}", PairClass::NetNewAmbiguous);
        second.site = "a/src/main/java/Other.java:7".to_string();
        let mut j = judgement(vec![pair("a", "b", "/v1/x/{}", PairClass::NetNewAmbiguous), second]);
        assert_eq!(j.net_new_third_party(r), 1);
        // …while the call-site count, reported beside it, sees both.
        assert_eq!(j.net_new_third_party_sites(r).len(), 2);
        // And two DIFFERENT templates between the same two members are two
        // edges but one coupling — the unit CR-124's Decision Log refused.
        j.pairs.push(pair("a", "b", "/v1/y/{}", PairClass::NetNewAmbiguous));
        assert_eq!(j.net_new_third_party(r), 2);
        assert_eq!(j.couplings(Party::ThirdParty, r).len(), 1);
    }

    // ── The port join ───────────────────────────────────────────────────────

    #[test]
    fn a_placeholder_is_not_a_port() {
        assert!(is_a_port("9000"));
        assert!(is_a_port("80"));
        // The near misses, each of which the estate or a sibling corpus can
        // actually write. An unsubstituted placeholder is the one that matters:
        // two members holding `${PORT}` would collide and cancel out the ports
        // they really declare.
        assert!(!is_a_port("${PORT}"));
        assert!(!is_a_port("${SERVER_PORT:9000}"));
        assert!(!is_a_port(""));
        assert!(!is_a_port("9000 "));
        assert!(!is_a_port("-1"));
        assert!(!is_a_port("9000,9001"));
        assert!(!is_a_port("http://localhost:9000"));
    }

    #[test]
    fn a_port_exactly_one_member_declares_identifies_that_member() {
        let index = index_of(&[("filters-api", "9013"), ("mailbox-api", "9000")]);
        assert_eq!(index.member_for("9013"), Some("filters-api"));
        assert_eq!(index.member_for("9000"), Some("mailbox-api"));
    }

    #[test]
    fn two_members_claiming_one_port_resolve_to_nothing() {
        // FR-WS-20 AC2's same-tier collision rule, and CR-124's own risk row.
        // Real estate collisions are the fixture rather than invented ones:
        // 9009 is claimed by `archive-api` and its clone
        // `archive-api-logiclens-fork`, 9007 by `mailbox-notification-adapter`
        // and `archive-manager` — the two CR-124's risk row names. The run found
        // a third, 9000 (`mailbox-api` and `deprecated-mailbox-core`), which the
        // CR did not predict; the rule is the same for all of them.
        let index = index_of(&[
            ("archive-api", "9009"),
            ("archive-api-logiclens-fork", "9009"),
            ("mailbox-notification-adapter", "9007"),
            ("archive-manager", "9007"),
        ]);
        assert_eq!(index.member_for("9009"), None);
        assert_eq!(index.member_for("9007"), None);
    }

    #[test]
    fn one_member_declaring_a_port_twice_is_not_a_collision() {
        // The near miss on the collision rule: a member declaring the same port
        // in `application.yml` and `application-local.yml` is ONE claimant, and
        // a rule counting declarations rather than claimants would refuse every
        // multi-profile member on the estate.
        let index = index_of(&[("mailbox-api", "9000"), ("mailbox-api", "9000")]);
        assert_eq!(index.member_for("9000"), Some("mailbox-api"));
        assert_eq!(index.declarations, 2);
        assert_eq!(index.claimants["9000"].len(), 1);
    }

    #[test]
    fn a_port_no_member_declares_identifies_nothing_and_is_enumerated() {
        let index = index_of(&[("mailbox-api", "9000")]);
        assert_eq!(index.member_for("8000"), None);
        let referenced = referenced_ports(&["9000", "8000", "3000"]);
        let census = index.resolution_census(&referenced);
        assert_eq!(census.exactly_one, ["9000".to_string()].into_iter().collect());
        assert_eq!(
            census.unclaimed,
            ["3000".to_string(), "8000".to_string()].into_iter().collect(),
        );
        assert!(census.collided.is_empty());
    }

    #[test]
    fn the_census_puts_every_referenced_port_in_exactly_one_bucket() {
        // The three buckets are a partition, or "resolves to none" and
        // "resolves to two or more" can overlap and the report double-counts.
        let index = index_of(&[("a", "9000"), ("b", "9009"), ("c", "9009")]);
        let referenced = referenced_ports(&["9000", "9009", "1234"]);
        let census = index.resolution_census(&referenced);
        assert_eq!(
            census.exactly_one.len() + census.collided.len() + census.unclaimed.len(),
            referenced.len(),
        );
        assert!(census.exactly_one.is_disjoint(&census.collided));
        assert!(census.exactly_one.is_disjoint(&census.unclaimed));
        assert!(census.collided.is_disjoint(&census.unclaimed));
    }

    // ── The population ──────────────────────────────────────────────────────

    fn target(overlay: &str, file: &str) -> TargetRef {
        TargetRef {
            member: "mailbox-aggregator-api".into(),
            overlay: overlay.into(),
            label: "localhost".into(),
            scheme: "http".into(),
            port: Some("9020".into()),
            via_flat: flat_key("official-log-aggregate.api.base-url"),
            via_key: "official-log-aggregate.api.base-url".into(),
            file: file.into(),
        }
    }

    #[test]
    fn the_headline_reads_application_configuration_and_never_a_deploy_manifest() {
        // CR-124 §2: "The signal here never touches a deploy manifest." A deploy
        // values file writes `http://official-log-export-api.…:9020`, whose HOST
        // LABEL S-384 already resolves — admitting it would make this gate
        // re-measure the experiment it claims to be independent of, and would
        // make the reconciliation section's "disjoint populations" claim false.
        let deploy = target(".helm", "mailbox-aggregator-api/.helm/values.yaml");
        let app = target(
            "application:<none>",
            "mailbox-aggregator-api/src/main/resources/application.yml",
        );
        let test_app = target(
            "application:it-local",
            "mailbox-aggregator-api/src/test/resources/application-it-local.yml",
        );
        assert!(deploy.is_deploy(), "the fixture must actually be deploy evidence");
        assert!(!app.is_deploy());

        assert!(Population::Headline.admits_target(&app));
        assert!(!Population::Headline.admits_target(&deploy));
        assert!(!Population::Headline.admits_target(&test_app));

        assert!(Population::WithTestTree.admits_target(&test_app));
        assert!(!Population::WithTestTree.admits_target(&deploy));

        assert!(Population::WithDeployEvidence.admits_target(&deploy));
        assert!(Population::WithDeployEvidence.admits_target(&app));
        assert!(!Population::WithDeployEvidence.admits_target(&test_app));
    }

    #[test]
    fn the_provider_side_reads_main_source_unless_the_test_tree_counterfactual_asks() {
        let main = "mailbox-api/src/main/resources/application.yml";
        let test = "mailbox-api/src/test/resources/application-it-local.yml";
        assert!(Population::Headline.admits_provider_source(main));
        assert!(!Population::Headline.admits_provider_source(test));
        assert!(Population::WithTestTree.admits_provider_source(test));
        // The deploy counterfactual widens the CONSUMER side only: a
        // `server.port` in `src/test` is a test harness's port under every
        // reading, and letting it vary here would confound the two questions.
        assert!(!Population::WithDeployEvidence.admits_provider_source(test));
    }

    #[test]
    fn a_base_url_with_no_port_is_still_a_base_url_that_was_read() {
        // The estate reports 18 read and 18 carrying a port, which reads exactly
        // like a denominator that is equal by construction. This is the proof it
        // is not — and it needs no estate to run.
        let mut out = Judgement::default();
        let with_port = target("application:<none>", "a/src/main/resources/application.yml");
        let without = TargetRef {
            port: None,
            via_key: "pec-server.base-url".into(),
            ..with_port.clone()
        };
        judge_one_target(&mut out, &with_port, "a");
        judge_one_target(&mut out, &without, "a");
        assert_eq!(out.base_urls_read.len(), 2);
        assert_eq!(out.base_urls_with_port.len(), 1);
        // …and two call sites reading the SAME value are one value, or the
        // denominator counts sites rather than values.
        judge_one_target(&mut out, &with_port, "a");
        assert_eq!(out.base_urls_read.len(), 2);
    }

    #[test]
    fn the_authority_a_reference_is_reassembled_from_keeps_its_port() {
        let with_port = TargetRef {
            member: "a".into(),
            overlay: "application:<none>".into(),
            label: "localhost".into(),
            scheme: "http".into(),
            port: Some("9013".into()),
            via_flat: flat_key("filters.api.base-url"),
            via_key: "filters.api.base-url".into(),
            file: "a/src/main/resources/application.yml".into(),
        };
        assert_eq!(authority_of(&with_port), "localhost:9013");
        let without = TargetRef { port: None, ..with_port };
        assert_eq!(authority_of(&without), "localhost");
    }

    #[test]
    fn a_localhost_base_url_carries_its_port_where_the_host_label_carries_nothing() {
        // The premise of the whole gate, pinned against the parser that supplies
        // it: the label is `localhost` for every member, so the label
        // establishes nothing and the port is the only discriminating byte.
        let (label, scheme, port) = identity::url_target("http://localhost:9013").expect("a URL");
        assert_eq!(label, "localhost");
        assert_eq!(scheme, "http");
        assert_eq!(port.as_deref(), Some("9013"));
        // …and a base URL with no port carries nothing this gate can join on.
        assert_eq!(identity::url_target("http://localhost").map(|t| t.2), Some(None));
        // The near miss: a placeholder port is not a port, so it can neither
        // identify a member nor pad the "resolves to nothing" census.
        assert_eq!(identity::url_target("http://localhost:xxxx"), None);
    }
}
