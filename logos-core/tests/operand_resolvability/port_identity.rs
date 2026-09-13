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
//! The host label here is `localhost` for every `base-url` a resolved call site
//! reads on this estate (the estate also commits an IP literal and a `changeit`
//! placeholder elsewhere, which no resolved site reaches), so
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
/// canonical form [`logos_core::extract::config::corpus::canonical_key`] produces.
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
    ///
    /// The deploy exclusion is stated here rather than delegated. On the
    /// consumer side it is `TargetRef::is_deploy`, an **overlay** property; a
    /// path-shaped test alone returns `Tree::Main` for `.helm/values.yaml` and
    /// `docker-compose.yml`, so "the signal here never touches a deploy
    /// manifest" was enforced on one side only. It is latent rather than live —
    /// `ConfigCorpus::discover` admits a file only if its BASENAME is
    /// `application[-profile].{yml,yaml,properties}`, so no values file reaches
    /// this predicate today — but a guard that holds by another module's
    /// basename rule is a guard the reader cannot see, and it would stop holding
    /// the moment an `application-*.yml` were committed under a deploy
    /// directory.
    /// Whether a consumer-side configuration source is admitted — the same
    /// population [`Population::admits_target`] expresses over a `TargetRef`,
    /// stated over a path so the base-URL census can be taken over values that
    /// never became a `TargetRef`.
    fn admits_config_source(self, path: &str) -> bool {
        if is_deploy_path(path) {
            return false;
        }
        match self {
            Self::Headline => Tree::of(path) == Tree::Main,
            Self::WithTestTree => true,
            Self::WithDeployEvidence => Tree::of(path) == Tree::Main,
        }
    }

    fn admits_provider_source(self, path: &str) -> bool {
        if is_deploy_path(path) {
            return false;
        }
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

/// Whether a path is deploy evidence, by directory.
///
/// The consumer side gets this for free from `TargetRef::is_deploy`, which knows
/// the overlay a reference came from. The provider side reads configuration
/// sources, which carry only a path — so the same exclusion has to be spelled
/// here. Deliberately narrow: Helm's hidden chart directory and a Compose file,
/// the two shapes `identity::deploy_role` recognises on this estate.
fn is_deploy_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.contains("/.helm/")
        || lower.starts_with(".helm/")
        || lower.contains("/deploy-")
        || lower.starts_with("deploy-")
        || lower.rsplit('/').next().is_some_and(|n| n.starts_with("docker-compose"))
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
    /// Record one member's declaration of its own port, applying the one rule
    /// that decides whether it is a port at all.
    ///
    /// A method rather than three lines inside [`port_index`], because
    /// `port_index` runs only where `LOGOS_REF_WORKSPACE` is set: the review's
    /// mutation sweep deleted the `is_a_port` filter from that loop and all
    /// eighteen fixtures stayed green. Extracting the predicate alone did not
    /// close the hole — its CALL SITE is the rule. The fixtures now build every
    /// index through here, so the guard, the claimant set and the declaration
    /// counter are all exercised by production code.
    fn claim(&mut self, member: &str, port: &str) {
        if !is_a_port(port) {
            return;
        }
        self.declarations += 1;
        self.declaring_members.insert(member.to_string());
        self.claimants.entry(port.to_string()).or_default().insert(member.to_string());
    }

    /// The member a port identifies, or `None` — either because no member
    /// declares it, or because **two or more** do and a same-tier collision
    /// resolves to nothing rather than to a guess.
    ///
    /// This mirrors [`identity::Corpus::member_for`]'s rule exactly, and
    /// [FR-WS-20] AC2 is where that rule is written. [CR-124]'s risk table names
    /// two of them (`9009`, `9007`) and requires this outcome: *"Two members
    /// claiming one port resolve to nothing."* The estate carries more than two —
    /// `9000` and `9001` collide as well — so the count belongs to the CR's row,
    /// not to the estate.
    ///
    /// [CR-124]: ../../../docs/requests/CR-124-runtime-port-as-a-target-identity-tier.md
    /// [FR-WS-20]: ../../../docs/specs/requirements/FR-WS-20.md
    pub fn member_for(&self, port: &str) -> Option<&str> {
        let claimants = self.claimants.get(port)?;
        match claimants.len() {
            1 => claimants.iter().next().map(String::as_str),
            // Two or more collide. Zero is reachable only through a hand-built
            // index (`claimants` is `pub`), and resolves to nothing for the same
            // reason a collision does: nothing is established.
            _ => None,
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

/// The base-URL key that is the sibling of one path key, in the separator-free
/// form the join compares on — or `None` for a key with no prefix to hang it on.
///
/// **This is the join rule**, stated once: *"the same configuration that
/// supplies the path also supplies the base URL"* ([CR-121] §3.2), which is what
/// [CR-124]'s hypothesis is written over. `identity::judge` derives it inline and
/// byte-identically; the twin check in
/// [`assert_traversal_agrees_with_the_identity_gate`] compares the two gates'
/// resulting key sets on every estate run, so the duplication cannot drift
/// silently.
///
/// A named free function rather than an expression inside [`judge_ports`], for
/// the reason [`identity::classify`] and [`Verdict::decide`] are: `judge_ports`
/// runs only where `LOGOS_REF_WORKSPACE` is set. Replacing `.base-url` with
/// `.baseurl` here left all 18 fixtures green — the review's mutation sweep
/// proved it — and that is the single rule the whole join hangs on.
///
/// [CR-121]: ../../../docs/requests/CR-121-caller-to-callee-and-producer-to-consumer-across-services.md
/// [CR-124]: ../../../docs/requests/CR-124-runtime-port-as-a-target-identity-tier.md
fn base_url_sibling_key(path_key: &str) -> Option<String> {
    let (prefix, _) = path_key.rsplit_once('.')?;
    Some(flat_key(&format!("{prefix}.base-url")))
}

/// Whether an edge is one the consumer could not have reached without crossing a
/// service boundary, under [S-384]'s rule.
///
/// One function, called from both places that need it: [`PortPair::party`]'s
/// [`Reading::AsMeasuredByS384`] arm, and [`s384_net_new_third_party`], which
/// applies it to S-384's own pairs so the reconciliation is a reproduction
/// rather than a quotation. Written twice, the two copies could diverge and the
/// asserted reconciliation would compare different rules while staying green —
/// the review's mutation sweep found the second copy uncovered.
///
/// [S-384]: ../../../docs/planning/journal.md#s-384-measure-service-identity-resolvability-across-the-deploy-corpus
fn is_s384_third_party(consumer: &str, provider: &str, consumer_serves: bool) -> bool {
    consumer != provider && !consumer_serves
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
    // Digits alone is not enough, and the join is STRING equality. `09013` would
    // be a claim no caller can write (`url_target` yields the authority's own
    // spelling), `0` is not a listening port, and `99999` is outside the range
    // — each would either sit inert in the index or, worse, bind an edge to a
    // port nothing serves, which NFR-RA-05 forbids.
    !value.starts_with('0') && value.parse::<u16>().is_ok_and(|port| port != 0)
}

/// Every `base-url` value an admitted configuration source commits, by
/// `(member, flat key)` — **whether or not it parses as a URL**.
///
/// This exists because the residue buckets were wrong without it. `judge_ports`
/// can only see a value that became a `TargetRef`, and a value becomes a
/// `TargetRef` only if `identity::url_target_or_reason` parsed it. So a call
/// site whose sibling base-URL key IS committed, by a value that establishes no
/// authority, was reported under "no admitted source proves this key" — false
/// for it — and could never reach the `base_urls_read` denominator, which made
/// the "carrying a port" share structurally unable to count its own
/// counterexample.
///
/// Built once per pass, from the same `OnceLock`-backed corpus as
/// [`port_index`], so it costs no traversal of the estate.
fn base_url_index(
    root: &Path,
    members: &BTreeSet<String>,
    population: Population,
) -> BTreeMap<(String, String), BTreeSet<String>> {
    let mut out: BTreeMap<(String, String), BTreeSet<String>> = BTreeMap::new();
    for source in &crate::measurement(root).config.sources {
        if !population.admits_config_source(&source.path) {
            continue;
        }
        let Some(member) = source.path.split('/').next() else { continue };
        if !members.contains(member) {
            continue;
        }
        for (key, values) in &source.values {
            let flat = flat_key(key);
            if !flat.ends_with("baseurl") {
                continue;
            }
            out.entry((member.to_string(), flat)).or_default().extend(values.iter().cloned());
        }
    }
    out
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
        for port in source.values.get(SERVER_PORT_KEY).into_iter().flatten() {
            index.claim(member, port);
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
        let third_party = match reading {
            Reading::AsDeclared => self.consumer != self.provider,
            Reading::AsMeasuredByS384 => {
                is_s384_third_party(&self.consumer, &self.provider, self.consumer_serves)
            }
        };
        if third_party {
            Party::ThirdParty
        } else {
            Party::SelfTie
        }
    }
}

/// Everything one pass of the join measured.
#[derive(Debug, Default)]
pub struct Judgement {
    pub pairs: Vec<PortPair>,
    /// `base-url` values a consumer call site actually read, counted once per
    /// `(member, key, value)` — the denominator [CR-124] §6's first report is
    /// taken over.
    ///
    /// Includes values that establish **no** authority. An earlier shape counted
    /// only values `identity::url_target_or_reason` had already parsed, which
    /// made the "carrying a port" share 18 of 18 — a 100% that was structurally
    /// incapable of seeing its own counterexample, because an unparseable value
    /// never became a `TargetRef` and so never reached this set.
    pub base_urls_read: BTreeSet<(String, String, String)>,
    /// The subset of those that parse to an authority carrying a port.
    pub base_urls_with_port: BTreeSet<(String, String, String)>,
    /// Every port a consumer call site referenced, and which members referenced
    /// it — so a port that resolves to nothing can be adjudicated by a human
    /// against the caller that wrote it, rather than as a bare number.
    pub ports_referenced: BTreeMap<String, BTreeSet<String>>,
    /// Every `(consumer, flat base-URL key)` a resolved call site reads,
    /// collected whether or not anything proves that key.
    ///
    /// This is the set `identity::Findings::call_site_base_keys` holds for the
    /// other gate, computed here by [`base_url_sibling_key`] — the ONE rule the
    /// whole join hangs on, and the one the two gates derive independently.
    /// [`assert_traversal_agrees_with_the_identity_gate`] compares the two sets,
    /// so the duplication the module admits to cannot drift silently.
    pub call_site_base_keys: BTreeSet<(String, String)>,
    /// Call sites that resolved a template and whose base-URL sibling key no
    /// admitted configuration source proves — the residue, enumerated.
    pub sites_without_base_url: BTreeSet<String>,
    /// Call sites whose base-URL sibling key **is** committed by an admitted
    /// source, but whose value establishes no authority — so the port signal has
    /// nothing to join on.
    ///
    /// A third bucket, and not a refinement for its own sake. Without it the
    /// estate's one such site — `official-log-ingestion-batch` committing
    /// `base-url: http://localhost:xxxx`, a placeholder port — was reported
    /// under "no admitted source proves this key", which is **false** for it:
    /// the key is proved, the value is unusable. The two are different findings
    /// about the estate and only one of them is about the corpus.
    pub sites_with_unusable_base_url: BTreeSet<String>,
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
        // Party is a per-TEMPLATE property, so one coupling can be third-party at
        // one template and a self-tie at another. Filtering per pair and then
        // collapsing put such a coupling in BOTH columns, and the two printed
        // numbers summed above the true distinct total — the review reproduced
        // it with a fixture. A coupling that is a self-tie anywhere is reported
        // as one, so the two columns partition.
        let produced = self.pairs.iter().filter(|p| p.class != PairClass::TargetServesNothing);
        let mut self_tied: BTreeSet<(&str, &str)> = BTreeSet::new();
        let mut all: BTreeSet<(&str, &str)> = BTreeSet::new();
        for p in produced {
            let coupling = (p.consumer.as_str(), p.provider.as_str());
            all.insert(coupling);
            if p.party(reading) == Party::SelfTie {
                self_tied.insert(coupling);
            }
        }
        match party {
            Party::SelfTie => self_tied,
            Party::ThirdParty => all.difference(&self_tied).copied().collect(),
        }
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
    let committed = base_url_index(root, &corpus.members, population);
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

            let mut key_is_committed = false;
            let mut read_a_base_url = false;
            let mut saw_a_port = false;
            for key in site.key_outcomes.iter().flatten().filter_map(|o| o.key()) {
                let Some(base_flat) = base_url_sibling_key(key) else { continue };
                out.call_site_base_keys.insert((consumer.clone(), base_flat.clone()));
                // Every value an admitted source commits for this key, parsed or
                // not — so a committed-but-unusable base URL lands in its own
                // bucket instead of being reported as an unproven key.
                let key = (consumer.clone(), base_flat.clone());
                for value in committed.get(&key).into_iter().flatten() {
                    key_is_committed = true;
                    let seen = (consumer.clone(), base_flat.clone(), value.clone());
                    if carries_a_port(value) {
                        out.base_urls_with_port.insert(seen.clone());
                    }
                    out.base_urls_read.insert(seen);
                }
                for target in corpus.targets.iter().filter(|t| {
                    t.member == consumer
                        && t.via_flat == base_flat
                        && population.admits_target(t)
                }) {
                    read_a_base_url = true;
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
            if !key_is_committed {
                out.sites_without_base_url.insert(where_);
            } else if !read_a_base_url {
                out.sites_with_unusable_base_url.insert(where_);
            } else if !saw_a_port {
                out.sites_without_port.insert(where_);
            }
        }
    }
    out
}

/// Whether a committed `base-url` value establishes an authority carrying a
/// port — the one thing the port signal can join on.
///
/// Both census populations are computed from this single predicate over the same
/// committed values, so "carrying a port" is necessarily a SUBSET of "read".
/// An earlier shape derived the numerator from parsed `TargetRef`s and the
/// denominator from the same, which reported 18 of 18 — a 100% that could not
/// see its own counterexample, because a value the parser refused never became
/// a `TargetRef` at all. The estate has exactly one:
/// `official-log-ingestion-batch` commits `base-url: http://localhost:xxxx`.
///
/// Delegates to `identity::url_target`, the same parser the references are built
/// with, rather than re-deciding what a URL is.
fn carries_a_port(value: &str) -> bool {
    identity::url_target(value).is_some_and(|(_, _, port)| port.is_some())
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

/// Print the census, in the order [CR-124] §6 raises the questions, so the
/// output reads against the story without a decoder — the shape
/// `identity::report` already uses. The section headings below are this report's
/// own; §6 is a bullet list and numbers nothing.
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

/// How many `base-url` values a resolved call site reads carry a port — the
/// first half of [CR-124] §6's second bullet.
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
    println!(
        "  call sites whose base-URL key IS proved but whose value establishes no authority: {}",
        j.sites_with_unusable_base_url.len(),
    );
    for site in &j.sites_with_unusable_base_url {
        println!("      {site}");
    }
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

/// How the referenced ports resolve, with the ones that resolve to nothing
/// **enumerated for human adjudication** — the second half of [CR-124] §6's
/// second bullet.
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

/// The pairs, split by whether path-only matching would already have bound them
/// **and** by whether they cross a service boundary — [CR-124] §6's third and
/// fourth bullets, both splits at the point the headline appears.
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

/// Every edge identity PRODUCES, with the evidence that bound it — the net-new
/// and already-bound classes. `TargetServesNothing` is a refusal rather than an
/// edge and is reported as a count in the table above, not enumerated here.
/// Grouped by the **S-384
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
         comparable and this run fails rather than printing two numbers side by side.\n  \
         WHAT DIFFERS between the two signals, and why this is not a contradiction, is\n  \
         argued once — in the recorded finding printed below, not restated here."
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
            let consumer_serves =
                s384.providers.serving(&p.normalized).contains(p.consumer.as_str());
            is_s384_third_party(&p.consumer, &p.provider, consumer_serves)
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
        j.target_serves_nothing(Party::ThirdParty, Reading::AsDeclared),
        RECORDED_TARGET_SERVES_NOTHING,
        "the port-target-serves-nothing column moved from {RECORDED_TARGET_SERVES_NOTHING} \
         to {}; the recorded finding quotes it as a figure, so it is pinned",
        j.target_serves_nothing(Party::ThirdParty, Reading::AsDeclared),
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
        // The SET, not just its size. `WithTestTree` is the one non-monotone
        // population — widening the provider side can create a new same-port
        // collision and so REMOVE an edge — and an equal count would hide one
        // edge being swapped for another.
        for (class, party) in [
            (PairClass::NetNewAmbiguous, Party::ThirdParty),
            (PairClass::NetNewAmbiguous, Party::SelfTie),
            (PairClass::AlreadyBoundByPath, Party::ThirdParty),
        ] {
            assert_eq!(
                c.edges(class, party, Reading::AsDeclared),
                j.edges(class, party, Reading::AsDeclared),
                "the `{}` counterfactual reports the same COUNT as the headline for {} / {} \
                 but not the same edges — one edge was swapped for another, which an equal \
                 count hides",
                population.label(),
                class.label(),
                party.label(),
            );
        }
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
    // The sibling-key rule itself, not just the loop around it. The review found
    // that the two previous assertions cover only the OUTER loop, so changing
    // `.base-url` to `.baseurl` in one file would leave both gates green while
    // they compared different key populations — precisely the drift this check
    // exists to catch.
    assert_eq!(
        j.call_site_base_keys, s384.call_site_base_keys,
        "the two gates derived different base-URL sibling keys from the same call sites, so \
         CR-121 section 3.2's rule has drifted between `port_identity::base_url_sibling_key` \
         and `identity::judge`'s inline copy of it",
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

/// Edges whose port target registers no route at the template — a figure the
/// recorded finding quotes twice, so it is pinned for the same reason the
/// already-bound column is.
pub const RECORDED_TARGET_SERVES_NOTHING: usize = 6;

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

    /// Build an index through `PortIndex::claim` — production's own rule — so a
    /// fixture reading `declarations` or `claimants` is reading what the
    /// measurement would have written, not what the fixture wrote.
    fn index_of(claims: &[(&str, &str)]) -> PortIndex {
        let mut index = PortIndex::default();
        for (member, port) in claims {
            index.claim(member, port);
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
        // Anchored on the line that names the METRIC, not merely on the first
        // `>= ` line: a declaration that gained an earlier `>= n …` line would
        // otherwise silently redefine the floor.
        let declared: usize = DECLARED_FLOOR
            .lines()
            .filter(|l| l.contains("call-site edges"))
            .find_map(|l| l.trim().strip_prefix(">= ")?.split_whitespace().next()?.parse().ok())
            .expect("the declaration states its floor as a `>= NN ... call-site edges` line");
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

    // ── The join rules the estate arm alone would leave uncovered ───────────
    //
    // Every rule below was proved uncovered by the review's mutation sweep:
    // breaking it in production left all eighteen fixtures green. These are the
    // CR-124 §4.4 requirement applied to the rules that actually decide the
    // gate, not only to the ones that were convenient to reach.

    #[test]
    fn the_base_url_is_the_sibling_of_the_path_key_under_the_same_prefix() {
        // CR-121 §3.2's rule, and the single rule the whole join hangs on.
        // Replacing `.base-url` with `.baseurl` here left the suite green.
        assert_eq!(
            base_url_sibling_key("official-log-aggregate.api.uri-request-official-log"),
            Some(flat_key("official-log-aggregate.api.base-url")),
        );
        // Relaxed binding: the three spellings of the same key agree.
        assert_eq!(
            base_url_sibling_key("officialLogAggregate.api.uriRequestOfficialLog"),
            base_url_sibling_key("official-log-aggregate.api.uri-request-official-log"),
        );
        // The near miss: it is the SIBLING, not the key itself, and not the
        // grandparent's. A path key one level deeper hangs its base URL one
        // level deeper too.
        assert_ne!(
            base_url_sibling_key("a.api.rest.uri-x"),
            base_url_sibling_key("a.api.uri-x"),
        );
        assert_eq!(base_url_sibling_key("a.api.rest.uri-x"), Some(flat_key("a.api.rest.base-url")));
        // A key with no prefix has no sibling to hang a base URL on.
        assert_eq!(base_url_sibling_key("uri-x"), None);
        assert_eq!(base_url_sibling_key(""), None);
    }

    #[test]
    fn the_s384_party_rule_is_one_function_used_by_both_gates() {
        // Written twice, the two copies could diverge and the asserted
        // reconciliation would compare different rules while staying green.
        assert!(is_s384_third_party("notification-adapter", "notification-api", false));
        assert!(!is_s384_third_party("mailbox-aggregator-api", "official-log-export-api", true));
        assert!(!is_s384_third_party("a", "a", false), "a member bound to itself");
        assert!(!is_s384_third_party("a", "a", true));
        // …and it is exactly what `PortPair::party` answers under that reading.
        for (consumer, provider, serves) in
            [("a", "b", false), ("a", "b", true), ("a", "a", false), ("a", "a", true)]
        {
            let p = pair_serving(consumer, provider, "/v1/x/{}", PairClass::NetNewAmbiguous, serves);
            let expected = if is_s384_third_party(consumer, provider, serves) {
                Party::ThirdParty
            } else {
                Party::SelfTie
            };
            assert_eq!(p.party(Reading::AsMeasuredByS384), expected);
        }
    }

    #[test]
    fn the_key_the_port_index_reads_is_the_members_own_server_port() {
        // The constant's doc argues at length that admitting the actuator's
        // separate port would fabricate edges under NFR-RA-05 — and nothing
        // tested it. Swapping it for `management.server.port` left the suite
        // green.
        assert_eq!(SERVER_PORT_KEY, "server.port");
        assert_ne!(SERVER_PORT_KEY, "management.server.port");
        assert_ne!(SERVER_PORT_KEY, "server.ports");
    }

    #[test]
    fn a_deploy_manifest_is_never_read_for_a_provider_port() {
        // CR-124 §2's "never touches a deploy manifest" was enforced on the
        // consumer side only: `Tree::of` returns `Main` for a Helm values file,
        // so `Headline` admitted one. Latent — `ConfigCorpus::discover` filters
        // on basename — but a guard held by another module's rule is a guard the
        // reader cannot see.
        for path in [
            "mailbox-api/.helm/values.yaml",
            ".helm/values.yaml",
            "mailbox-aggregator-api/deploy-coll-bp/values.yaml",
            "mailbox-api/docker-compose.yml",
        ] {
            assert!(is_deploy_path(path), "{path} is deploy evidence");
            for population in
                [Population::Headline, Population::WithTestTree, Population::WithDeployEvidence]
            {
                assert!(
                    !population.admits_provider_source(path),
                    "{path} must not supply a provider port under {}",
                    population.label(),
                );
            }
        }
        // The near miss: an application source is not deploy evidence merely by
        // sitting near one, and the estate's real provider sources still pass.
        assert!(!is_deploy_path("mailbox-api/src/main/resources/application.yml"));
        assert!(Population::Headline
            .admits_provider_source("mailbox-api/src/main/resources/application.yml"));
    }

    #[test]
    fn a_coupling_that_is_a_self_tie_anywhere_is_reported_as_one() {
        // Party is a per-TEMPLATE property, so filtering per pair and then
        // collapsing put one coupling in BOTH columns and the two printed
        // numbers summed above the true distinct total.
        let j = judgement(vec![
            pair_serving("a", "b", "/v1/x/{}", PairClass::NetNewAmbiguous, true),
            pair_serving("a", "b", "/v1/y/{}", PairClass::AlreadyBoundByPath, false),
        ]);
        let r = Reading::AsMeasuredByS384;
        assert_eq!(j.couplings(Party::SelfTie, r).len(), 1);
        assert_eq!(j.couplings(Party::ThirdParty, r).len(), 0);
        // The two columns partition: their sum is the distinct coupling count.
        assert_eq!(
            j.couplings(Party::SelfTie, r).len() + j.couplings(Party::ThirdParty, r).len(),
            1,
        );
        // …and a refusal is not a coupling at all.
        let with_refusal = judgement(vec![
            pair("a", "b", "/v1/x/{}", PairClass::NetNewAmbiguous),
            pair("a", "c", "/v1/z/{}", PairClass::TargetServesNothing),
        ]);
        assert_eq!(with_refusal.couplings(Party::ThirdParty, r).len(), 1);
    }

    #[test]
    fn the_call_site_count_behind_the_headline_counts_only_net_new_third_party() {
        // Both filters were uncovered: every pair in the earlier fixture was
        // net-new AND third-party, so the function was indistinguishable from
        // "count distinct sites".
        let r = Reading::AsMeasuredByS384;
        let mut bound = pair("a", "c", "/v1/y/{}", PairClass::AlreadyBoundByPath);
        bound.site = "a/src/main/java/Bound.java:2".to_string();
        let mut tie = pair_serving("a", "d", "/v1/z/{}", PairClass::NetNewAmbiguous, true);
        tie.site = "a/src/main/java/Tie.java:3".to_string();
        let j = judgement(vec![pair("a", "b", "/v1/x/{}", PairClass::NetNewAmbiguous), bound, tie]);
        assert_eq!(j.net_new_third_party_sites(r).len(), 1);
        assert_eq!(
            j.net_new_third_party_sites(r).into_iter().collect::<Vec<_>>(),
            vec!["a/src/main/java/X.java:1"],
        );
    }

    #[test]
    fn every_printed_label_says_what_it_means() {
        // The module's whole thesis is that the third-party/self-tie split must
        // survive TO THE PRINTED HEADLINE. Swapping the two label strings broke
        // exactly that and left the suite green.
        assert_eq!(Verdict::decide(None, 16).label(), "VOID");
        assert_eq!(Verdict::decide(Some(0), 16).label(), "FALSIFIED");
        assert_eq!(Verdict::decide(Some(16), 16).label(), "HOLDS");
        assert!(Party::ThirdParty.label().starts_with("third-party"));
        assert!(Party::SelfTie.label().starts_with("SELF-TIE"));
        assert_ne!(Party::ThirdParty.label(), Party::SelfTie.label());
        assert_ne!(Reading::AsDeclared.label(), Reading::AsMeasuredByS384.label());
        assert!(Reading::AsDeclared.label().contains("consumer != provider"));
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
        // Digits alone is not enough, and the join is STRING equality: a
        // leading-zero spelling is a claim no caller can write, and neither 0
        // nor a value past the 16-bit range is a port anything listens on.
        assert!(!is_a_port("0"));
        assert!(!is_a_port("09013"));
        assert!(!is_a_port("99999"));
        assert!(!is_a_port("65536"));
        assert!(is_a_port("65535"), "the top of the range is still a port");
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
    fn the_index_refuses_a_declaration_that_is_not_a_port() {
        // `is_a_port` is covered as a predicate; this covers its CALL SITE,
        // which is the rule. The review proved that deleting the guard from the
        // index left every fixture green — extracting the predicate alone had
        // not closed the hole, because no fixture ever offered the index a
        // value it must refuse.
        let index = index_of(&[
            ("mailbox-api", "9000"),
            ("a", "${PORT}"),
            ("b", "${SERVER_PORT:9000}"),
            ("c", ""),
            ("d", "0"),
            ("e", "09013"),
        ]);
        assert_eq!(index.declarations, 1, "only the real port is a declaration");
        assert_eq!(index.declaring_members, ["mailbox-api".to_string()].into_iter().collect());
        assert_eq!(index.claimants.len(), 1);
        assert_eq!(index.member_for("9000"), Some("mailbox-api"));
        // The consequence the guard exists for: two members holding the same
        // placeholder must not collide and cancel out the ports they declare.
        assert_eq!(index.member_for("${PORT}"), None);
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
    fn a_base_url_the_parser_refuses_is_still_a_base_url_that_was_read() {
        // The defect the review found: an earlier shape derived BOTH census
        // populations from parsed references, so a value the parser refused was
        // invisible to the denominator and the run reported 18 of 18 (100%)
        // carrying a port. The estate's counterexample is real — the run's own
        // fixture below pins that `http://localhost:xxxx` is refused — so the
        // 100% could never have been falsified by the data.
        assert!(carries_a_port("http://localhost:9013"));
        assert!(carries_a_port("http://mailbox-api.pec-services.svc.cluster.local:9000"));
        // …and every shape the estate commits that cannot be joined on:
        assert!(!carries_a_port("http://localhost:xxxx"), "a placeholder port");
        assert!(!carries_a_port("changeit"), "not a URL at all");
        assert!(!carries_a_port("http://localhost"), "a URL with no port");
        assert!(!carries_a_port("https://192.168.54.134:444/postedoc-ws"), "an IP literal");
        assert!(!carries_a_port("http://${SERVICE_HOST}:${PORT}"), "a templated authority");
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
