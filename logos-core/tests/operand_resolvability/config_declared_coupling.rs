//! **S-411's blocking measurement gate** — does the reference estate's
//! *committed configuration*, by itself, prove enough member-grain coupling to
//! justify building [ADR-65]'s third intake?
//!
//! Two independent floors, both declared in [`config_declared_coupling_floor.txt`]
//! **before this module existed** and both **parsed** out of that declaration by
//! [`the_floors_are_the_ones_declared_before_the_run`] rather than compared with a
//! literal written beside them:
//!
//!   - **>= 12 addressed member pairs** — ordered `(A, B)` pairs where a value A
//!     commits names a host whose first DNS label identifies a runnable member B.
//!   - **>= 10 shared topics** — topic identities the broker arm captures that two
//!     or more members each name in a committed value.
//!
//! Below either floor, that half is recorded FALSIFIED with its evidence and its
//! date, [ADR-65] is marked superseded **in place**, no requirement is created,
//! and S-412 through S-415 and S-418 stay unplanned.
//!
//! # What this gate is not
//!
//! It is **not** a re-run of [S-384] or [S-400]. Both of those measured whether a
//! deploy-evidence identity signal can bind a *call site*, on the metric
//! *net-new third-party call-site edges*, and both fell below a floor of 16.
//! [CR-131] §2.2 says so in terms: *"Cluster B proposes a DIFFERENT relation at a
//! DIFFERENT grain with its OWN metric, not a re-run of either gate."* A
//! member-grain relation carries no route template, so scoring it at call-site
//! grain would reject it on a unit it does not have. The consequence is stated
//! rather than hidden: **12 here is not comparable with 16 there**, and this
//! module never presents it as though it were.
//!
//! What the two gates *are* comparable on is their inputs, so both are reconciled
//! explicitly: [S-384]'s **11 distinct member labels** and [S-400]'s **5 ports
//! identifying exactly one member** are re-derived here from the same corpus and
//! asserted, because a run whose target census silently drifted from theirs would
//! be measuring a different estate while printing the same words.
//!
//! # Everything reusable is called, never re-implemented
//!
//! The identity classifier is [S-384]'s, unchanged: [`identity::Corpus::member_for`]
//! decides which member a host label names, under [FR-WS-20] AC2 **as written** —
//! a same-tier collision resolves to nothing. The URL parser is
//! [`identity::url_target_or_reason`]. The deploy-file taxonomy is
//! [`identity::deploy_role`], the documentation-tree guard [`identity::is_documentation`],
//! the overlay attribution [`identity::overlay_of`]. The application corpus is the
//! shipped [`ConfigCorpus`], and the captured topics are the shipped `brokers.scm`
//! arm's own output, collected by [`count_broker_captures`] in the walk
//! [`crate::measurement`] already performs.
//!
//! [`label_state`] is the one place this module decides something about identity,
//! and it **calls** `member_for` rather than restating its tier loop — the whole
//! of its extra work is telling `None`-because-collision apart from
//! `None`-because-unclaimed, which the split needs and `member_for` does not
//! expose. A hand-mirrored copy of that loop is exactly the divergence that would
//! make one of two measurements quietly wrong.
//!
//! [`count_broker_captures`]: super::configuration_agreement::count_broker_captures
//!
//! # The one walk this module performs, and why it is the instrument
//!
//! [FR-WS-19]'s corpus walk sets `hidden(true)`, so the estate's 33 Helm charts
//! under `.helm/` are never entered ([CR-131] §3.1). Admitting committed deploy
//! overlays means walking with `hidden(false)`, and [ADR-65]'s Consequences name
//! that cost as something the gate must measure rather than assert. So the walk is
//! run twice — once counting as the corpus walk does, once as the overlay-admitting
//! walk would — and [`WalkCost`] reports the difference **with its denominator**.
//!
//! # Two readings of a captured topic, and which one decides
//!
//! [FR-WS-10] in force keys a subscribe by *the placeholder literal as written*,
//! so the arm captures `"${spring.kafka.topics.archive-events}"` and not
//! `archive-events`. Joining a configuration *value* against a configuration
//! *reference* can only ever answer zero — a reading structurally incapable of
//! producing a non-zero number, which is the failure shape [S-400] recorded when a
//! denominator derived from already-parsed references made a share read 18 of 18.
//! Both readings are computed ([`TopicReading`]); the declaration fixes **RESOLVED**
//! as the gate and AS-WRITTEN as the printed cost of the in-force key rule.
//!
//! [ADR-65]: ../../../docs/specs/architecture/decisions/ADR-65.md
//! [CR-131]: ../../../docs/requests/CR-131-cross-service-coupling-from-committed-configuration.md
//! [FR-WS-10]: ../../../docs/specs/requirements/FR-WS-10.md
//! [FR-WS-19]: ../../../docs/specs/requirements/FR-WS-19.md
//! [FR-WS-20]: ../../../docs/specs/requirements/FR-WS-20.md
//! [S-384]: ../../../docs/planning/journal.md#s-384-measure-service-identity-resolvability-across-the-deploy-corpus
//! [S-400]: ../../../docs/planning/journal.md#s-400-measure-whether-a-runtime-port-identifies-the-callee
//! [`config_declared_coupling_floor.txt`]: ./config_declared_coupling_floor.txt

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::{Duration, Instant};

use logos_core::plugin::LanguageRegistry;

use super::configuration_agreement::{
    agreed_value, parse_properties, parse_yaml, workspace_agreement, ConfigCorpus,
};
use super::identity::{self, Corpus, PairClass, Tier};

/// **The floors, as declared before the run** — the tracked declaration this
/// module's constants are parsed out of.
///
/// `include_str!` rather than a path reference, following
/// [`identity::DECLARED_FLOOR`]: a file the build embeds cannot be deleted or
/// renamed without breaking compilation, so the declaration and the run that
/// consumed it cannot drift apart silently.
pub const DECLARED_FLOOR: &str = include_str!("config_declared_coupling_floor.txt");

/// The recorded verdict, reproduced by the run and printed by it.
pub const RECORDED_FINDING: &str = include_str!("config_declared_coupling_finding.txt");

/// Floor for the `addresses(A -> B)` half, declared in [`DECLARED_FLOOR`] and
/// reproduced here so the assertion and the declaration cannot drift.
pub const ADDRESSED_PAIR_FLOOR: usize = 12;

/// Floor for the `declares-topic(A -> T)` half, declared in [`DECLARED_FLOOR`].
pub const SHARED_TOPIC_FLOOR: usize = 10;

/// [S-384]'s distinct **member** labels among its deploy target references — the
/// figure this run reconciles its own target census against.
///
/// [S-384]: ../../../docs/planning/journal.md#s-384-measure-service-identity-resolvability-across-the-deploy-corpus
pub const S384_MEMBER_LABELS: usize = 11;

/// [S-400]'s ports identifying **exactly one** member, of the 11 a consumer
/// references — the second reconciliation the story names.
///
/// [S-400]: ../../../docs/planning/journal.md#s-400-measure-whether-a-runtime-port-identifies-the-callee
pub const S400_UNIQUE_PORTS: usize = 5;

/// Members declaring their own `server.port`, as [S-400] counted them — the
/// index behind its port join, reproduced here so a drift in the shared corpus
/// is a failure rather than a silently different reconciliation.
///
/// [S-400]: ../../../docs/planning/journal.md#s-400-measure-whether-a-runtime-port-identifies-the-callee
pub const S400_PORT_DECLARING_MEMBERS: usize = 30;

/// Distinct `server.port` values those members declare, as [S-400] counted them.
///
/// [S-400]: ../../../docs/planning/journal.md#s-400-measure-whether-a-runtime-port-identifies-the-callee
pub const S400_DISTINCT_PORT_VALUES: usize = 27;

/// The addressed-pair figure this harness measured, pinned so a drift in the
/// identity join, the corpus or the path-only subtraction is a failure rather
/// than a quietly different number.
pub const RECORDED_ADDRESSED_PAIRS: usize = 11;

/// The shared-topic figure this harness measured.
pub const RECORDED_SHARED_TOPICS: usize = 13;

/// Ordered pairs the sequence blind spot would add, at pair grain.
///
/// Pinned because the falsification's margin is SMALLER than it: if a later run
/// moves this number the honesty caveat in the finding moves with it, and a
/// caveat that silently stopped being true is worse than none.
pub const RECORDED_BLIND_SPOT_PAIRS: usize = 2;

/// The AS-WRITTEN reading's answer. Zero is the *expected* value and the
/// declaration says why, so it is pinned: a run in which it became non-zero
/// would mean a member had begun committing a property placeholder as a value,
/// and the two readings would no longer be the disjoint populations the
/// declaration assumes.
pub const RECORDED_SHARED_TOPICS_AS_WRITTEN: usize = 0;

/// Pairs the invocation intake already binds by path alone, subtracted from the
/// headline — the other half of the split, pinned for the reason [S-384] pins
/// its own: a change that moved pairs between the two columns while leaving the
/// total alone would otherwise pass silently, and it is exactly the change that
/// would matter.
///
/// [S-384]: ../../../docs/planning/journal.md#s-384-measure-service-identity-resolvability-across-the-deploy-corpus
pub const RECORDED_PATH_ONLY_PAIRS: usize = 8;

const _: () = {
    assert!(
        RECORDED_ADDRESSED_PAIRS < ADDRESSED_PAIR_FLOOR,
        "the recorded pair finding is FALSIFIED; if that changes, re-decide CR-131 section 8 \
         and ADR-65 rather than relaxing the assertion",
    );
    assert!(
        RECORDED_SHARED_TOPICS >= SHARED_TOPIC_FLOOR,
        "the recorded topic finding HOLDS; if that changes, the half that cleared no longer \
         does and ADR-65's supersession banner must say so",
    );
    assert!(
        RECORDED_ADDRESSED_PAIRS + RECORDED_BLIND_SPOT_PAIRS >= ADDRESSED_PAIR_FLOOR,
        "the finding states that the sequence blind spot covers the gap; if it no longer \
         does, that paragraph is wrong and must be corrected before this constant is",
    );
};

// ── What a host label establishes ───────────────────────────────────────────

/// What a host label resolves to, with the two `None` cases of
/// [`identity::Corpus::member_for`] told apart.
///
/// `member_for` collapses "no member claims this label" and "two members claim it
/// at one tier" into `None`, because for its purposes both mean *no edge*. The
/// split needs them apart — an external is a fact about the estate's boundary, a
/// collision is a fact about [FR-WS-20] AC2 — so this enum asks `member_for`
/// first and then distinguishes the `None`, rather than re-walking the tier
/// ladder beside it.
///
/// [FR-WS-20]: ../../../docs/specs/requirements/FR-WS-20.md
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelState<'a> {
    /// Exactly one member claims it at the first decisive tier that claims it.
    Resolves(&'a str),
    /// Two or more claim it at that tier, so [FR-WS-20] AC2 resolves it to
    /// nothing.
    ///
    /// [FR-WS-20]: ../../../docs/specs/requirements/FR-WS-20.md
    Collision,
    /// No member claims it at any decisive tier: an inferred external.
    Unclaimed,
}

/// Resolve a host label against the [S-384] identity corpus.
///
/// [S-384]: ../../../docs/planning/journal.md#s-384-measure-service-identity-resolvability-across-the-deploy-corpus
pub fn label_state<'a>(corpus: &'a Corpus, label: &str) -> LabelState<'a> {
    match corpus.member_for(label) {
        Some(member) => LabelState::Resolves(member),
        None if corpus.claims.iter().any(|c| c.tier.is_decisive() && c.label == label) => {
            LabelState::Collision
        }
        None => LabelState::Unclaimed,
    }
}

/// Which bucket of the declaration's six-way split one target-valued committed
/// value falls in. Total and mutually exclusive by construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PairOutcome {
    /// The label resolves to the committing member itself.
    SelfTie,
    /// Same-tier identity collision: resolves to nothing ([FR-WS-20] AC2).
    ///
    /// [FR-WS-20]: ../../../docs/specs/requirements/FR-WS-20.md
    SameTierCollision,
    /// No member claims the label at a decisive tier: an inferred external.
    NoMemberLabel,
    /// It resolves to a member that runs nothing — a manifest, chart or
    /// document store, never a deployable.
    TargetNotRunnable,
    /// The invocation intake already binds this member pair on the path alone,
    /// so the relation reports coupling the product already reports.
    PathOnlyMatched,
    /// What remains — **the headline**.
    Addressed,
}

impl PairOutcome {
    pub fn label(self) -> &'static str {
        match self {
            Self::SelfTie => "self-tie",
            Self::SameTierCollision => "same-tier identity collision",
            Self::NoMemberLabel => "label resolves to no member (external)",
            Self::TargetNotRunnable => "target member is not runnable",
            Self::PathOnlyMatched => "already matched by path alone",
            Self::Addressed => "ADDRESSED PAIR",
        }
    }

    pub const ALL: [Self; 6] = [
        Self::SelfTie,
        Self::SameTierCollision,
        Self::NoMemberLabel,
        Self::TargetNotRunnable,
        Self::PathOnlyMatched,
        Self::Addressed,
    ];
}

/// **The pair half of the gate, in one function.**
///
/// A free function, and pinned unconditionally by fixtures, for the reason
/// [`identity::classify`] is one: the estate arm *skips* wherever
/// `LOGOS_REF_WORKSPACE` is unset, so a classifier exercised only there could be
/// replaced by a constant with the default `cargo test` run still green.
///
/// The order of the tests is the declaration's, and it is total: a label that
/// establishes no member cannot be a self-tie, and a member that runs nothing
/// cannot have a route the path already matched.
pub fn classify_pair(
    consumer: &str,
    state: LabelState<'_>,
    runnable: &BTreeSet<String>,
    path_only: &BTreeSet<(String, String)>,
) -> (PairOutcome, Option<String>) {
    let provider = match state {
        LabelState::Collision => return (PairOutcome::SameTierCollision, None),
        LabelState::Unclaimed => return (PairOutcome::NoMemberLabel, None),
        LabelState::Resolves(m) => m.to_string(),
    };
    if provider == consumer {
        return (PairOutcome::SelfTie, Some(provider));
    }
    if !runnable.contains(&provider) {
        return (PairOutcome::TargetNotRunnable, Some(provider));
    }
    if path_only.contains(&(consumer.to_string(), provider.clone())) {
        return (PairOutcome::PathOnlyMatched, Some(provider));
    }
    (PairOutcome::Addressed, Some(provider))
}

// ── Target-valued committed values ──────────────────────────────────────────

/// Which of the declaration's two source sets a committed value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SourceSet {
    /// `application*.{yml,yaml,properties}`, as the shipped [`ConfigCorpus`]
    /// admits them.
    Application,
    /// A committed deploy overlay: Helm values, `Chart.yaml`, Compose, or a raw
    /// Kubernetes manifest.
    Deploy,
}

impl SourceSet {
    pub fn label(self) -> &'static str {
        match self {
            Self::Application => "application config",
            Self::Deploy => "deploy overlay",
        }
    }
}

/// How a committed value established a host — a URL, or the bare `host` beside a
/// `port` that [CR-131] §3.2 B2 admits as an equal target identity.
///
/// [CR-131]: ../../../docs/requests/CR-131-cross-service-coupling-from-committed-configuration.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TargetForm {
    Url,
    HostBesidePort,
}

/// One target-valued committed value: who commits it, where, and what host label
/// it names.
#[derive(Debug, Clone)]
pub struct Target {
    pub member: String,
    pub label: String,
    pub form: TargetForm,
    pub source: SourceSet,
    pub overlay: String,
    pub via_key: String,
    pub file: String,
    pub value: String,
}

/// The first DNS label of a **bare host** value, or `None`.
///
/// Deliberately stricter than [`identity::url_target_or_reason`], because it has
/// no scheme to lean on: without one, `changeit` and `true` are host-shaped
/// strings. A bare host is admitted only when it carries a dot or a dash — the
/// shapes a DNS name or a Kubernetes service name actually takes — is not an IP
/// literal, and carries no template syntax.
///
/// The near miss this rejects, and the reason the dot-or-dash rule is here rather
/// than a bare character-class test: `enabled: true` beside a `port` key would
/// otherwise be a host label named `true`, and it is the *sibling-port* rule
/// that puts such a key in scope at all.
pub fn bare_host_label(value: &str) -> Option<String> {
    let host = value.trim();
    // One allow-list, not an allow-list behind a blacklist. An earlier draft
    // rejected `{}$()*/: @` first and then applied this rule; the falsifiability
    // sweep deleted the blacklist and left every fixture green, because every
    // character in it already fails here. A redundant guard is not a second
    // safety net — it is a rule a reader has to check for divergence.
    if host.is_empty() {
        return None;
    }
    if !host.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') {
        return None;
    }
    if !host.contains('.') && !host.contains('-') {
        return None;
    }
    // An IP literal is an address, not an identity — `identity::url_target_or_reason`
    // refuses it for the same reason and this is that rule, applied where there
    // is no scheme to carry it.
    if host.split('.').all(|o| !o.is_empty() && o.chars().all(|c| c.is_ascii_digit())) {
        return None;
    }
    let label = host.split('.').next().unwrap_or(host);
    (!label.is_empty()).then(|| label.to_string())
}

/// The keys of one flattened source whose value is a bare host **beside a
/// sibling `port`**.
///
/// The sibling rule is the whole admission: a `host` key alone is ambiguous with
/// a hundred other uses of the word, and [CR-131] §3.2 B2 admits the pair, not
/// the key.
///
/// [CR-131]: ../../../docs/requests/CR-131-cross-service-coupling-from-committed-configuration.md
pub fn host_keys_with_sibling_port(
    values: &BTreeMap<String, BTreeSet<String>>,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (key, vals) in values {
        let leaf = key.rsplit('.').next().unwrap_or(key);
        if leaf != "host" && leaf != "hostname" {
            continue;
        }
        let parent = key.strip_suffix(leaf).unwrap_or("");
        if !values.contains_key(&format!("{parent}port")) {
            continue;
        }
        for v in vals {
            if let Some(label) = bare_host_label(v) {
                out.push((key.clone(), label));
            }
        }
    }
    out
}

// ── The deploy-overlay walk, and what it costs ──────────────────────────────

/// The cost of admitting deploy overlays and hidden chart directories, **with
/// its denominator** — [ADR-65]'s Consequences name this as a figure the gate
/// owes rather than asserts.
///
/// [ADR-65]: ../../../docs/specs/architecture/decisions/ADR-65.md
#[derive(Debug, Default)]
pub struct WalkCost {
    /// Entries a `hidden(true)` traversal visits — the shipped corpus walk's
    /// population, and the denominator every other figure here is read against.
    pub corpus_entries: usize,
    pub corpus_traversal: Duration,
    /// Entries a `hidden(false)` traversal visits.
    pub overlay_entries: usize,
    pub overlay_traversal: Duration,
    /// Deploy-shaped files actually read and parsed.
    pub files_read: usize,
    pub bytes_read: u64,
    pub read_and_parse: Duration,
    /// Deploy-shaped files that sit inside a hidden directory — the population
    /// `hidden(true)` cannot reach at all.
    pub files_in_hidden: usize,
    /// Deploy-shaped files skipped as documentation, examples or tutorials.
    pub documentation_files: usize,
}

impl WalkCost {
    /// Entries the overlay walk visits that the corpus walk does not.
    pub fn extra_entries(&self) -> usize {
        self.overlay_entries.saturating_sub(self.corpus_entries)
    }
}

/// Every committed value the deploy overlays carry, by member, plus the walk cost
/// of collecting them.
#[derive(Default)]
pub struct DeployCorpus {
    /// member -> (canonical key, value, overlay, file) for every scalar the
    /// admitted deploy files commit.
    pub values: Vec<Target>,
    /// Every committed scalar, as `(member, value)` — the join population for the
    /// topic half. A `Vec` of rows rather than a map, because the topic half
    /// needs the via-key for adjudication and the pair half needs the file.
    pub scalars: Vec<Scalar>,
    pub cost: WalkCost,
    /// A textual **ceiling**, never an extraction: `host:`-shaped lines inside a
    /// YAML sequence item in an admitted deploy file.
    ///
    /// The shipped `parse_yaml` binds no scalar under a sequence, deliberately —
    /// its own doc comment records the `routes:` list that once fabricated a
    /// `spring.cloud.gateway.routes.uri` key — so the estate's structured
    /// gateway upstream table (`proxy.upstreams[].host`) is invisible to this
    /// measurement. Counting the lines bounds what a sequence-reading corpus
    /// would add without pretending to have read them.
    pub sequence_host_ceiling: Vec<SequenceHost>,
}

/// One `host:`-shaped line the shipped parser cannot bind, kept with the member
/// that commits it so the blind spot can be BOUNDED rather than merely counted.
#[derive(Debug, Clone)]
pub struct SequenceHost {
    pub member: String,
    pub file: String,
    pub value: String,
}

/// One committed scalar, at the grain the topic join needs.
#[derive(Debug, Clone)]
pub struct Scalar {
    pub member: String,
    pub key: String,
    pub value: String,
    pub source: SourceSet,
    pub file: String,
}

/// Walk the estate for committed deploy overlays, timing both traversals.
///
/// `hidden(false)`, unlike [`ConfigCorpus::discover`]: the charts this gate exists
/// to read live under `.helm/`. `.git` is excluded explicitly instead — the one
/// hidden directory that carries no evidence and costs the whole walk if entered.
/// That is [`identity::walk_deploy`]'s configuration, reproduced because the cost
/// of *this* traversal is one of the figures the story asks for and a walk behind
/// a `OnceLock` another gate already consumed cannot be timed.
fn walk_overlays(root: &Path, members: &BTreeSet<String>) -> DeployCorpus {
    let mut out = DeployCorpus::default();

    let started = Instant::now();
    let corpus_walk = ignore::WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .git_global(false)
        .ignore(false)
        .parents(false)
        .build();
    out.cost.corpus_entries =
        corpus_walk.flatten().filter(|e| e.file_type().is_some_and(|t| t.is_file())).count();
    out.cost.corpus_traversal = started.elapsed();

    let started = Instant::now();
    let mut read_and_parse = Duration::ZERO;
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_global(false)
        .ignore(false)
        .parents(false)
        .filter_entry(|e| e.file_name() != std::ffi::OsStr::new(".git"))
        .build();
    for entry in walker.flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        out.cost.overlay_entries += 1;
        let Ok(rel) = entry.path().strip_prefix(root) else { continue };
        let rel = rel.to_string_lossy().replace('\\', "/");
        let Some(member) = rel.split('/').next().map(str::to_string) else { continue };
        if !members.contains(&member) {
            continue;
        }
        if identity::is_documentation(&rel) {
            if identity::deploy_role(&rel).is_some() {
                out.cost.documentation_files += 1;
            }
            continue;
        }
        if identity::deploy_role(&rel).is_none() {
            continue;
        }
        let at = Instant::now();
        let Ok(text) = std::fs::read_to_string(entry.path()) else { continue };
        out.cost.files_read += 1;
        out.cost.bytes_read += text.len() as u64;
        if rel.split('/').any(|seg| seg.starts_with('.') && seg.len() > 1) {
            out.cost.files_in_hidden += 1;
        }
        let values = if rel.ends_with(".properties") {
            parse_properties(&text)
        } else {
            parse_yaml(&text)
        };
        read_and_parse += at.elapsed();
        for value in sequence_host_lines(&text) {
            out.sequence_host_ceiling.push(SequenceHost {
                member: member.clone(),
                file: rel.clone(),
                value,
            });
        }
        collect_overlay(&mut out, &member, &rel, &values);
    }
    out.cost.overlay_traversal = started.elapsed();
    out.cost.read_and_parse = read_and_parse;
    out
}

/// Fold one admitted deploy file's flattened values into the corpus.
fn collect_overlay(
    out: &mut DeployCorpus,
    member: &str,
    rel: &str,
    values: &BTreeMap<String, BTreeSet<String>>,
) {
    let overlay = identity::overlay_of(rel, member);
    for (key, vals) in values {
        for value in vals {
            out.scalars.push(Scalar {
                member: member.to_string(),
                key: key.clone(),
                value: value.clone(),
                source: SourceSet::Deploy,
                file: rel.to_string(),
            });
            if let Ok((label, _, _)) = identity::url_target_or_reason(value) {
                out.values.push(Target {
                    member: member.to_string(),
                    label,
                    form: TargetForm::Url,
                    source: SourceSet::Deploy,
                    overlay: overlay.clone(),
                    via_key: key.clone(),
                    file: rel.to_string(),
                    value: value.clone(),
                });
            }
        }
    }
    for (key, label) in host_keys_with_sibling_port(values) {
        let value = values.get(&key).and_then(|v| v.iter().next()).cloned().unwrap_or_default();
        out.values.push(Target {
            member: member.to_string(),
            label,
            form: TargetForm::HostBesidePort,
            source: SourceSet::Deploy,
            overlay: overlay.clone(),
            via_key: key,
            file: rel.to_string(),
            value,
        });
    }
}

/// Lines of the shape `- host: x` or a `host:` key indented inside a sequence
/// item — the textual ceiling [`DeployCorpus::sequence_host_ceiling`] explains.
///
/// Textual and deliberately a ceiling: it exists to bound what the shipped
/// parser's sequence skip costs, never to extract a host.
pub fn sequence_host_lines(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_sequence_at: Option<usize> = None;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        if let Some(seq) = in_sequence_at {
            if indent <= seq && !trimmed.starts_with('-') {
                in_sequence_at = None;
            }
        }
        let body = if trimmed.starts_with('-') {
            in_sequence_at = Some(indent);
            trimmed.trim_start_matches('-').trim()
        } else if in_sequence_at.is_some() {
            trimmed
        } else {
            continue;
        };
        for prefix in ["host:", "hostname:"] {
            if let Some(value) = body.strip_prefix(prefix) {
                out.push(value.trim().trim_matches(['"', '\'']).to_string());
            }
        }
    }
    out
}

// ── Topics ──────────────────────────────────────────────────────────────────

/// The two readings of a captured topic literal — see this module's docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TopicReading {
    /// The literal's own text, [FR-WS-10]'s key rule in force.
    ///
    /// [FR-WS-10]: ../../../docs/specs/requirements/FR-WS-10.md
    AsWritten,
    /// The committed value a `${key}` placeholder resolves to — **the gate**.
    Resolved,
}

impl TopicReading {
    /// The identity a captured topic has **under this reading** — the single
    /// definition of what each reading means.
    ///
    /// One function rather than the `match reading` that stood at each of the two
    /// places `judge_topics` needs it. The falsifiability sweep is why: with the
    /// two spelled out separately, swapping ONE of them left every fixture green,
    /// because the join population and the judged identity disagreed and the
    /// lookup simply missed. That is the hand-mirrored-twin defect — two copies of
    /// one rule, one of which can drift silently — and a gate whose whole subject
    /// is which of two readings decides cannot afford it.
    pub fn identity_of(self, topic: &CapturedTopic) -> Option<&str> {
        match self {
            Self::AsWritten => Some(topic.as_written.as_str()),
            Self::Resolved => topic.resolved.as_deref(),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::AsWritten => "AS-WRITTEN (FR-WS-10's key in force)",
            Self::Resolved => "RESOLVED (the committed value) — THE GATE",
        }
    }
}

/// Which bucket of the declaration's three-way topic split one captured identity
/// falls in. `Dangling` is split in two so a falsification can say whether the
/// join broke at resolution or at declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TopicOutcome {
    /// Two or more distinct members declare it — **the headline**.
    BothSidesDeclared,
    /// Exactly one member declares it.
    OneSideOnly,
    /// The placeholder resolved to no agreed committed value.
    DanglingUnresolved,
    /// It resolved, but no member commits a value equal to it.
    DanglingNoDeclarer,
}

impl TopicOutcome {
    pub fn label(self) -> &'static str {
        match self {
            Self::BothSidesDeclared => "BOTH-SIDES-DECLARED (>= 2 members)",
            Self::OneSideOnly => "one-side-only (1 member)",
            Self::DanglingUnresolved => "dangling: placeholder did not resolve",
            Self::DanglingNoDeclarer => "dangling: no member commits the value",
        }
    }

    pub const ALL: [Self; 4] = [
        Self::BothSidesDeclared,
        Self::OneSideOnly,
        Self::DanglingUnresolved,
        Self::DanglingNoDeclarer,
    ];
}

/// **The topic half of the gate, in one function** — pinned by fixtures for the
/// same reason [`classify_pair`] is.
pub fn classify_topic(identity: Option<&str>, declarers: usize) -> TopicOutcome {
    match (identity, declarers) {
        (None, _) => TopicOutcome::DanglingUnresolved,
        (Some(_), 0) => TopicOutcome::DanglingNoDeclarer,
        (Some(_), 1) => TopicOutcome::OneSideOnly,
        (Some(_), _) => TopicOutcome::BothSidesDeclared,
    }
}

/// The configuration key a captured topic literal refers to, or `None` when the
/// literal is a topic name rather than a reference.
///
/// `${key}` and `${key:default}`. The default is deliberately **not** used as a
/// fallback identity: the declaration admits only the value the workspace's
/// committed configuration agrees on, and a default written at the reference site
/// is not a committed configuration value.
pub fn placeholder_key(literal: &str) -> Option<&str> {
    let inner = literal.strip_prefix("${")?.strip_suffix('}')?;
    let key = inner.split(':').next().unwrap_or(inner).trim();
    (!key.is_empty()).then_some(key)
}

/// One captured topic identity and everything the split needs about it.
#[derive(Debug, Clone)]
pub struct CapturedTopic {
    /// The literal as the arm captured it.
    pub as_written: String,
    /// The committed value it resolves to, under [`TopicReading::Resolved`].
    pub resolved: Option<String>,
    pub publishes: usize,
    pub subscribes: usize,
}

// ── The measurement ─────────────────────────────────────────────────────────

/// One judged target-valued committed value.
#[derive(Debug, Clone)]
pub struct JudgedTarget {
    pub target: Target,
    pub outcome: PairOutcome,
    pub provider: Option<String>,
}

/// One judged captured topic, under one reading.
#[derive(Debug, Clone)]
pub struct JudgedTopic {
    pub topic: CapturedTopic,
    pub identity: Option<String>,
    pub outcome: TopicOutcome,
    pub declarers: BTreeSet<String>,
    /// One `(member, key, file)` example per declarer, so the bucket is evidence
    /// rather than a bare count ([NFR-CC-04]).
    ///
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    pub evidence: Vec<(String, String, String)>,
    /// `(member, key, source set)` per declarer, so the split by source set is a
    /// group-by rather than a second pass over the scalars.
    pub by_source: Vec<(String, String, SourceSet)>,
}

/// Everything the run measures, so the report and the assertions read one object
/// rather than recomputing.
pub struct Judgement {
    pub members: BTreeSet<String>,
    pub runnable: BTreeSet<String>,
    pub targets: Vec<JudgedTarget>,
    pub topics_resolved: Vec<JudgedTopic>,
    pub topics_as_written: Vec<JudgedTopic>,
    pub cost: WalkCost,
    pub sequence_host_ceiling: Vec<SequenceHost>,
    pub path_only: BTreeSet<(String, String)>,
    /// Application-config scalars, kept for the census denominators.
    pub application_scalars: usize,
    pub deploy_scalars: usize,
    /// Distinct ports a consumer references, and how many name exactly one
    /// member — [S-400]'s reconciliation, re-derived from this run's own targets.
    ///
    /// [S-400]: ../../../docs/planning/journal.md#s-400-measure-whether-a-runtime-port-identifies-the-callee
    pub deploy_member_labels: BTreeSet<String>,
    pub deploy_external_labels: BTreeSet<String>,
    /// `server.port` value -> the members declaring it, as [S-400] built its
    /// index: a port two members claim identifies neither.
    ///
    /// [S-400]: ../../../docs/planning/journal.md#s-400-measure-whether-a-runtime-port-identifies-the-callee
    pub port_owners: BTreeMap<String, BTreeSet<String>>,
    /// The bounded sequence blind spot, as `(consumer, provider, file)` rows.
    /// Computed once at judgement time, where the identity corpus is in scope.
    pub blind_spot: BTreeSet<(String, String, String)>,
}

impl Judgement {
    /// Distinct ordered member pairs in one bucket — **the grain the floor is
    /// read at**.
    pub fn pairs(&self, outcome: PairOutcome) -> BTreeSet<(&str, &str)> {
        self.targets
            .iter()
            .filter(|t| t.outcome == outcome)
            .filter_map(|t| Some((t.target.member.as_str(), t.provider.as_deref()?)))
            .collect()
    }

    /// **The figure the pair gate is read off.**
    pub fn addressed_pairs(&self) -> usize {
        self.pairs(PairOutcome::Addressed).len()
    }

    /// Target-valued committed values in one bucket — the reference-grain census
    /// beside the pair-grain headline.
    pub fn references(&self, outcome: PairOutcome) -> usize {
        self.targets.iter().filter(|t| t.outcome == outcome).count()
    }

    /// **The figure the topic gate is read off** — distinct topic IDENTITIES
    /// two or more members declare.
    ///
    /// Deduped on the identity, never on the captured literal, because the
    /// declaration fixes the unit: *"The grain is the TOPIC, not the (member,
    /// topic) row and not the member pair."* `topics_resolved` carries one row
    /// per distinct captured LITERAL, and two literals can resolve to one
    /// identity — `${spring.kafka.topics.orders}` and a bare `orders` name the
    /// same topic — so counting rows would report one topic twice.
    ///
    /// It changes nothing on today's estate, where each of the 13 identities is
    /// captured under exactly one literal, and that is precisely why it was
    /// worth fixing now: the population this gate measures is the one [S-408],
    /// [S-409] and [S-410] set out to WIDEN, and a second spelling of an
    /// already-captured topic is the first thing that widening introduces.
    ///
    /// [S-408]: ../../../docs/planning/journal.md#s-408-a-kafka-streams-topology-link-is-a-broker-publish-or-subscribe-site
    /// [S-409]: ../../../docs/planning/journal.md#s-409-the-accessor-hop-reaches-the-broker-arm
    /// [S-410]: ../../../docs/planning/journal.md#s-410-topic-identity-is-the-committed-configured-value-so-a-streams-publish-meets-a-subscribe
    pub fn shared_topics(&self) -> usize {
        self.topics(TopicOutcome::BothSidesDeclared)
            .iter()
            .filter_map(|t| t.identity.as_deref())
            .collect::<BTreeSet<_>>()
            .len()
    }

    pub fn topics(&self, outcome: TopicOutcome) -> Vec<&JudgedTopic> {
        self.topics_resolved.iter().filter(|t| t.outcome == outcome).collect()
    }

    /// Which source set proves each `declares-topic` row — a relation the estate
    /// only commits in deploy overlays would carry a different delivery cost from
    /// one its application configuration already proves.
    pub fn declaration_source_split(&self) -> (usize, usize) {
        let mut app = 0;
        let mut deploy = 0;
        for t in &self.topics_resolved {
            for (_, _, source) in &t.by_source {
                match source {
                    SourceSet::Application => app += 1,
                    SourceSet::Deploy => deploy += 1,
                }
            }
        }
        (app, deploy)
    }

    /// A **CEILING** on what the shipped parser's sequence skip costs the pair
    /// half: the ordered pairs the `host:` lines inside YAML sequences WOULD add
    /// if a sequence-reading corpus admitted every one of them, judged by the
    /// same classifier.
    ///
    /// A ceiling, never an extraction. It exists for one reason: the pair
    /// headline falsified by a small margin, and a reader with a named blind
    /// spot owes the adjudicator an answer to "could the blind spot have
    /// covered the gap?" — a question a bare line count cannot answer.
    pub fn blind_spot_pairs(&self, corpus: &Corpus) -> BTreeSet<(String, String, String)> {
        let already = self.pairs(PairOutcome::Addressed);
        self.sequence_host_ceiling
            .iter()
            .filter_map(|h| {
                let label = bare_host_label(&h.value)?;
                let state = label_state(corpus, &label);
                let (outcome, provider) =
                    classify_pair(&h.member, state, &self.runnable, &self.path_only);
                (outcome == PairOutcome::Addressed)
                    .then(|| Some((h.member.clone(), provider?, h.file.clone())))
                    .flatten()
            })
            .filter(|(a, b, _)| !already.contains(&(a.as_str(), b.as_str())))
            .collect()
    }

    /// Declarations reached through a key whose last segment names a topic, and
    /// those reached through any other key — the accidental-match guard for the
    /// topic half, where the join is on a VALUE and a key that happens to carry
    /// a topic-shaped string would be indistinguishable without it.
    /// The blind spot at PAIR grain — the same unit the floor is read at. The
    /// row set carries one entry per overlay, so its length is not this figure.
    pub fn blind_spot_pairs_count(&self) -> usize {
        self.blind_spot.iter().map(|(a, b, _)| (a, b)).collect::<BTreeSet<_>>().len()
    }

    pub fn declaration_key_split(&self) -> (usize, usize) {
        let mut topic_shaped = 0;
        let mut other = 0;
        for t in self.topics(TopicOutcome::BothSidesDeclared) {
            for (_, key, _) in &t.by_source {
                let leaf = key.rsplit('.').next().unwrap_or(key);
                if leaf.contains("topic") || key.contains(".topics.") {
                    topic_shaped += 1;
                } else {
                    other += 1;
                }
            }
        }
        (topic_shaped, other)
    }

    /// The AS-WRITTEN reading's answer, at the SAME grain as [`shared_topics`]:
    /// the two are printed side by side, so counting them in different units
    /// would make the comparison meaningless.
    ///
    /// [`shared_topics`]: Judgement::shared_topics
    pub fn shared_topics_as_written(&self) -> usize {
        self.topics_as_written
            .iter()
            .filter(|t| t.outcome == TopicOutcome::BothSidesDeclared)
            .filter_map(|t| t.identity.as_deref())
            .collect::<BTreeSet<_>>()
            .len()
    }
}

/// The measurement, computed once per test binary.
pub(crate) fn judgement(root: &Path) -> &'static Judgement {
    static ONCE: std::sync::OnceLock<Judgement> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| judge(root))
}

fn judge(root: &Path) -> Judgement {
    let s384 = identity::findings(root);
    let m = crate::measurement(root);
    let members = s384.corpus.members.clone();
    let overlays = walk_overlays(root, &members);
    let runnable = runnable_members(root, &members, &m.config);

    let path_only: BTreeSet<(String, String)> = s384
        .pairs
        .iter()
        .filter(|p| p.class == PairClass::AlreadyBoundByPath)
        .map(|p| (p.consumer.clone(), p.provider.clone()))
        .collect();

    let mut targets: Vec<Target> = overlays.values.clone();
    let mut application_scalars = 0usize;
    let mut scalars: Vec<Scalar> = overlays.scalars.clone();
    for source in &m.config.sources {
        let member = source.path.split('/').next().unwrap_or("").to_string();
        if !members.contains(&member) {
            continue;
        }
        let overlay =
            format!("application:{}", source.profile.as_deref().unwrap_or("<none>"));
        for (key, values) in &source.values {
            for value in values {
                application_scalars += 1;
                scalars.push(Scalar {
                    member: member.clone(),
                    key: key.clone(),
                    value: value.clone(),
                    source: SourceSet::Application,
                    file: source.path.clone(),
                });
                if let Ok((label, _, _)) = identity::url_target_or_reason(value) {
                    targets.push(Target {
                        member: member.clone(),
                        label,
                        form: TargetForm::Url,
                        source: SourceSet::Application,
                        overlay: overlay.clone(),
                        via_key: key.clone(),
                        file: source.path.clone(),
                        value: value.clone(),
                    });
                }
            }
        }
        for (key, label) in host_keys_with_sibling_port(&source.values) {
            let value =
                source.values.get(&key).and_then(|v| v.iter().next()).cloned().unwrap_or_default();
            targets.push(Target {
                member: member.clone(),
                label,
                form: TargetForm::HostBesidePort,
                source: SourceSet::Application,
                overlay: overlay.clone(),
                via_key: key,
                file: source.path.clone(),
                value,
            });
        }
    }

    let judged: Vec<JudgedTarget> = targets
        .into_iter()
        .map(|target| {
            let state = label_state(&s384.corpus, &target.label);
            let (outcome, provider) =
                classify_pair(&target.member, state, &runnable, &path_only);
            JudgedTarget { target, outcome, provider }
        })
        .collect();

    let captured = captured_topics(m, &m.config);
    let topics_resolved =
        judge_topics(&captured, &scalars, TopicReading::Resolved);
    let topics_as_written =
        judge_topics(&captured, &scalars, TopicReading::AsWritten);

    let deploy_member_labels = s384
        .corpus
        .targets
        .iter()
        .filter(|t| t.is_deploy())
        .filter(|t| matches!(label_state(&s384.corpus, &t.label), LabelState::Resolves(_)))
        .map(|t| t.label.clone())
        .collect();
    let deploy_external_labels = s384
        .corpus
        .targets
        .iter()
        .filter(|t| t.is_deploy())
        .filter(|t| matches!(label_state(&s384.corpus, &t.label), LabelState::Unclaimed))
        .map(|t| t.label.clone())
        .collect();

    let mut port_owners: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for source in &m.config.sources {
        let member = source.path.split('/').next().unwrap_or("").to_string();
        if !members.contains(&member) {
            continue;
        }
        for port in source.values.get("server.port").into_iter().flatten() {
            if port.chars().all(|c| c.is_ascii_digit()) && !port.is_empty() {
                port_owners.entry(port.clone()).or_default().insert(member.clone());
            }
        }
    }

    let mut out = Judgement {
        members,
        runnable,
        targets: judged,
        topics_resolved,
        topics_as_written,
        cost: overlays.cost,
        sequence_host_ceiling: overlays.sequence_host_ceiling,
        path_only,
        application_scalars,
        deploy_scalars: overlays.scalars.len(),
        deploy_member_labels,
        deploy_external_labels,
        port_owners,
        blind_spot: BTreeSet::new(),
    };
    out.blind_spot = out.blind_spot_pairs(&s384.corpus);
    out
}

/// Members that commit at least one file the plugin registry claims as source,
/// outside a documentation tree — the declaration's "runnable".
///
/// Read off [`ConfigCorpus::files`], the roster the corpus walk already stashed,
/// so this costs no traversal of its own.
fn runnable_members(
    root: &Path,
    members: &BTreeSet<String>,
    config: &ConfigCorpus,
) -> BTreeSet<String> {
    let Ok(registry) = LanguageRegistry::load(root) else {
        return BTreeSet::new();
    };
    let mut out = BTreeSet::new();
    for rel in config.files() {
        let Some(member) = rel.split('/').next() else { continue };
        if !members.contains(member) || out.contains(member) {
            continue;
        }
        if identity::is_documentation(rel) {
            continue;
        }
        if registry.for_path(rel).is_some() {
            out.insert(member.to_string());
        }
    }
    out
}

/// Every topic identity the shipped `brokers.scm` captured in the walk
/// [`crate::measurement`] performed, with its resolved value.
fn captured_topics(m: &crate::Measurement, config: &ConfigCorpus) -> Vec<CapturedTopic> {
    let mut by_text: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for stats in m.broker.values() {
        for text in &stats.publish_topic_texts {
            by_text.entry(text.clone()).or_default().0 += 1;
        }
        for text in &stats.subscribe_topic_texts {
            by_text.entry(text.clone()).or_default().1 += 1;
        }
    }
    by_text
        .into_iter()
        .filter(|(text, _)| !text.is_empty())
        .map(|(as_written, (publishes, subscribes))| {
            let resolved = match placeholder_key(&as_written) {
                None => Some(as_written.clone()),
                Some(key) => agreed_value(&workspace_agreement(config, key)).map(str::to_string),
            };
            CapturedTopic { as_written, resolved, publishes, subscribes }
        })
        .collect()
}

/// Judge every captured topic under one reading.
fn judge_topics(
    captured: &[CapturedTopic],
    scalars: &[Scalar],
    reading: TopicReading,
) -> Vec<JudgedTopic> {
    let wanted: BTreeSet<&str> =
        captured.iter().filter_map(|t| reading.identity_of(t)).collect();
    let mut declarers: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    let mut evidence: BTreeMap<&str, Vec<(String, String, String)>> = BTreeMap::new();
    let mut by_source: BTreeMap<&str, Vec<(String, String, SourceSet)>> = BTreeMap::new();
    for s in scalars {
        let Some(topic) = wanted.get(s.value.as_str()) else { continue };
        if declarers.entry(topic).or_default().insert(s.member.as_str()) {
            evidence.entry(topic).or_default().push((
                s.member.clone(),
                s.key.clone(),
                s.file.clone(),
            ));
            by_source.entry(topic).or_default().push((
                s.member.clone(),
                s.key.clone(),
                s.source,
            ));
        }
    }
    captured
        .iter()
        .map(|topic| {
            let identity = reading.identity_of(topic).map(str::to_string);
            let who: BTreeSet<String> = identity
                .as_deref()
                .and_then(|t| declarers.get(t))
                .map(|d| d.iter().map(|m| (*m).to_string()).collect())
                .unwrap_or_default();
            let ev = identity
                .as_deref()
                .and_then(|t| evidence.get(t))
                .cloned()
                .unwrap_or_default();
            let src = identity
                .as_deref()
                .and_then(|t| by_source.get(t))
                .cloned()
                .unwrap_or_default();
            JudgedTopic {
                topic: topic.clone(),
                outcome: classify_topic(identity.as_deref(), who.len()),
                identity,
                declarers: who,
                evidence: ev,
                by_source: src,
            }
        })
        .collect()
}

// ── The report ──────────────────────────────────────────────────────────────

/// Print the full census. One function per acceptance criterion, so the printed
/// output can be read against the story without a decoder — and so the combined
/// printer stays inside the project's `max_fn_lines` rule.
fn report(j: &Judgement) {
    println!("\n=== S-411 · config-declared coupling over the reference estate ===");
    println!(
        "members {} · runnable {} · committed scalars {} application + {} deploy overlay",
        j.members.len(),
        j.runnable.len(),
        j.application_scalars,
        j.deploy_scalars,
    );
    report_pairs(j);
    report_topics(j);
    report_reconciliation(j);
    report_walk_cost(j);
}

fn report_pairs(j: &Judgement) {
    println!("\n  THE PAIR HALF — ordered member pairs from committed target values\n");
    println!("    {:<40} {:>10} {:>10}", "bucket", "refs", "pairs");
    for outcome in PairOutcome::ALL {
        let pairs = j.pairs(outcome);
        let shown = if matches!(
            outcome,
            PairOutcome::SameTierCollision | PairOutcome::NoMemberLabel
        ) {
            String::from("--")
        } else {
            pairs.len().to_string()
        };
        println!(
            "    {:<40} {:>10} {:>10}",
            outcome.label(),
            j.references(outcome),
            shown,
        );
    }
    println!(
        "\n    by form: {} URL · {} bare host beside a port",
        j.targets.iter().filter(|t| t.target.form == TargetForm::Url).count(),
        j.targets.iter().filter(|t| t.target.form == TargetForm::HostBesidePort).count(),
    );
    println!(
        "    by source set: {} application config · {} deploy overlay",
        j.targets.iter().filter(|t| t.target.source == SourceSet::Application).count(),
        j.targets.iter().filter(|t| t.target.source == SourceSet::Deploy).count(),
    );
    println!("\n    the addressed pairs, enumerated:");
    for (a, b) in j.pairs(PairOutcome::Addressed) {
        let via = j
            .targets
            .iter()
            .find(|t| {
                t.outcome == PairOutcome::Addressed
                    && t.target.member == a
                    && t.provider.as_deref() == Some(b)
            })
            .map(|t| format!("{}  [{}]  {}", t.target.via_key, t.target.source.label(), t.target.file))
            .unwrap_or_default();
        println!("      {a} -> {b}    via {via}");
    }
    println!(
        "\n    the path-only-matched pairs subtracted from the headline ({} pairs the \n             invocation intake binds by path alone, of which {} are named by a committed value):",
        j.path_only.len(),
        j.pairs(PairOutcome::PathOnlyMatched).len(),
    );
    for (a, b) in j.pairs(PairOutcome::PathOnlyMatched) {
        println!("      {a} -> {b}");
    }
    report_pairs_by_overlay(j);
}

/// Per-overlay attribution of the addressed pairs — a pair proved only in one
/// overlay is a different fact from one every overlay of its member commits, and
/// [FR-WS-19] AC2's profile-tag rule is what the delivery story would carry it
/// under.
///
/// [FR-WS-19]: ../../../docs/specs/requirements/FR-WS-19.md
fn report_pairs_by_overlay(j: &Judgement) {
    let mut by_overlay: BTreeMap<&str, BTreeSet<(&str, &str)>> = BTreeMap::new();
    for t in j.targets.iter().filter(|t| t.outcome == PairOutcome::Addressed) {
        if let Some(provider) = t.provider.as_deref() {
            by_overlay
                .entry(t.target.overlay.as_str())
                .or_default()
                .insert((t.target.member.as_str(), provider));
        }
    }
    println!("\n    addressed pairs by the overlay that proves them:");
    for (overlay, pairs) in &by_overlay {
        println!("      {:<44} {} pair(s)", overlay, pairs.len());
    }
}

fn report_topics(j: &Judgement) {
    println!("\n  THE TOPIC HALF — captured topic identities and who declares them\n");
    println!("    captured topic identities (distinct literals) {}", j.topics_resolved.len());
    println!("    {:<44} {:>10}", TopicReading::Resolved.label(), "topics");
    for outcome in TopicOutcome::ALL {
        println!("    {:<44} {:>10}", outcome.label(), j.topics(outcome).len());
    }
    println!(
        "\n    {}: {} both-sides-declared",
        TopicReading::AsWritten.label(),
        j.shared_topics_as_written(),
    );
    let (app, deploy) = j.declaration_source_split();
    println!(
        "    declarations by source set: {app} application config · {deploy} deploy overlay",
    );
    let (topic_shaped, other) = j.declaration_key_split();
    println!(
        "    declarations by key shape:  {topic_shaped} through a topic-named key · {other} \
         through some other key\n    (the accidental-match guard: the join is on a VALUE, so a \
         key that merely\n     happens to carry a topic-shaped string would otherwise be \
         indistinguishable)",
    );
    println!("\n    the shared topics, enumerated:");
    for t in j.topics(TopicOutcome::BothSidesDeclared) {
        println!(
            "      {:<34} {} members   from {:?}",
            t.identity.as_deref().unwrap_or("?"),
            t.declarers.len(),
            t.topic.as_written,
        );
        for (member, key, file) in &t.evidence {
            println!("          {member}  {key}  ({file})");
        }
    }
    println!("\n    the dangling identities, enumerated:");
    for outcome in [TopicOutcome::DanglingUnresolved, TopicOutcome::DanglingNoDeclarer] {
        for t in j.topics(outcome) {
            println!(
                "      {:<44} {}  [pub {} / sub {}]",
                t.topic.as_written,
                outcome.label(),
                t.topic.publishes,
                t.topic.subscribes,
            );
        }
    }
}

fn report_reconciliation(j: &Judgement) {
    println!("\n  RECONCILIATION AGAINST S-384 AND S-400\n");
    println!(
        "    distinct member labels among DEPLOY target references  {}   (S-384 recorded {})",
        j.deploy_member_labels.len(),
        S384_MEMBER_LABELS,
    );
    println!(
        "    distinct external labels among the same references     {}",
        j.deploy_external_labels.len(),
    );
    let (referenced, unique) = port_census(j);
    println!(
        "    members declaring their own `server.port`               {}   (S-400 recorded \
         {S400_PORT_DECLARING_MEMBERS})",
        j.port_owners.values().flatten().collect::<BTreeSet<_>>().len(),
    );
    println!(
        "    distinct `server.port` values declared                  {}   (S-400 recorded \
         {S400_DISTINCT_PORT_VALUES})",
        j.port_owners.len(),
    );
    println!(
        "    distinct ports referenced by an application-committed URL {referenced}",
    );
    println!(
        "    of which identifying exactly one member                {unique}   (S-400 recorded \
         {S400_UNIQUE_PORTS})",
    );
    println!(
        "\n    The referenced-port population here is WIDER than S-400's: that gate read the"
    );
    println!("    19 base-url values a RESOLVED CALL SITE reads, this one reads every");
    println!("    application-committed URL value. So S-400's {S400_UNIQUE_PORTS} is a FLOOR for");
    println!("    this figure, not an equality, and the reconciliation is that the index behind");
    println!("    both — members' own `server.port` — is unchanged.");
    println!("\n    S-384 and S-400 measured CALL-SITE EDGES and fell below a floor of 16 each.");
    println!(
        "    This gate measures a member-grain RELATION against floors of \
         {ADDRESSED_PAIR_FLOOR} pairs and {SHARED_TOPIC_FLOOR} topics."
    );
    println!("    The two are NOT comparable (CR-131 section 2.2) and neither number may be");
    println!("    read against the other.");
}

/// The port census [S-400] recorded, re-derived from this run's own application
/// targets so a silent drift in the shared corpus is visible here.
///
/// [S-400] read the port of an application-committed base URL against members'
/// own `server.port`; that is the population reproduced, and the figure this run
/// reconciles is *ports identifying exactly one member*, not an edge count.
///
/// [S-400]: ../../../docs/planning/journal.md#s-400-measure-whether-a-runtime-port-identifies-the-callee
fn port_census(j: &Judgement) -> (usize, usize) {
    let mut referenced: BTreeSet<String> = BTreeSet::new();
    for t in &j.targets {
        // S-400 read the port of an APPLICATION-committed base URL, so that is
        // the source set reproduced. The key population is wider — every URL
        // value, not the 19 a resolved call site reads — which is why S-400's
        // figure reconciles as a floor rather than as an equality.
        if t.target.source != SourceSet::Application {
            continue;
        }
        if let Ok((_, _, Some(port))) = identity::url_target_or_reason(&t.target.value) {
            referenced.insert(port);
        }
    }
    let unique = referenced
        .iter()
        .filter(|p| j.port_owners.get(*p).is_some_and(|owners| owners.len() == 1))
        .count();
    (referenced.len(), unique)
}

fn report_walk_cost(j: &Judgement) {
    let c = &j.cost;
    println!("\n  WALK COST OF ADMITTING DEPLOY OVERLAYS AND HIDDEN CHART DIRECTORIES\n");
    println!(
        "    corpus walk  hidden(true)   {:>7} files   {:>8.2}s   <- the denominator",
        c.corpus_entries,
        c.corpus_traversal.as_secs_f64(),
    );
    println!(
        "    overlay walk hidden(false)  {:>7} files   {:>8.2}s",
        c.overlay_entries,
        c.overlay_traversal.as_secs_f64(),
    );
    println!(
        "    extra files the overlay walk visits   {:>7}   ({:.1}% of the corpus walk)",
        c.extra_entries(),
        pct(c.extra_entries(), c.corpus_entries),
    );
    println!(
        "    deploy-shaped files read and parsed   {:>7}   {:>8.2}s   {} KiB",
        c.files_read,
        c.read_and_parse.as_secs_f64(),
        c.bytes_read / 1024,
    );
    println!(
        "    of those, inside a hidden directory   {:>7}   ({:.1}% of files read)",
        c.files_in_hidden,
        pct(c.files_in_hidden, c.files_read),
    );
    println!("    deploy-shaped files skipped as documentation {:>4}", c.documentation_files);
    println!(
        "    overlay traversal net of read and parse {:>7.2}s  against the corpus walk's \
         {:.2}s",
        (c.overlay_traversal.saturating_sub(c.read_and_parse)).as_secs_f64(),
        c.corpus_traversal.as_secs_f64(),
    );
    println!(
        "\n    THE FILE COUNTS ARE THE COST FIGURE; the seconds are ONE run's wall clock on"
    );
    println!("    one machine and move by 2x with the page cache alone (measured: the same");
    println!("    traversal read 4.49s cold-cached and 9.65s warm in two consecutive runs of");
    println!("    this harness). Counts are deterministic and reproduce; times do not, and a");
    println!("    figure that does not reproduce must not be quoted as a budget.");
    let rows = &j.blind_spot;
    let blind: BTreeMap<(&str, &str), Vec<&str>> =
        rows.iter().fold(BTreeMap::new(), |mut acc, (a, b, file)| {
            acc.entry((a.as_str(), b.as_str())).or_default().push(file.as_str());
            acc
        });
    println!(
        "\n    CEILING, never an extraction: {} `host:`-shaped lines sit inside a YAML\n    \
         sequence item in an admitted deploy file. The shipped `parse_yaml` binds no\n    \
         scalar under a sequence, so the estate's `proxy.upstreams[].host` table is\n    \
         invisible to this measurement. That is a limit of the reader, not an estate fact.",
        j.sequence_host_ceiling.len(),
    );
    println!(
        "    Judged by the SAME classifier and AT THE SAME GRAIN — the ordered member pair —"
    );
    println!(
        "    those {} lines would add at most {} pair(s) beyond the {} the headline counts.",
        rows.len(),
        blind.len(),
        j.addressed_pairs(),
    );
    for ((a, b), files) in &blind {
        println!("      {a} -> {b}   (ceiling only, in {} overlay(s):)", files.len());
        for file in files {
            println!("          {file}");
        }
    }
    if j.addressed_pairs() < ADDRESSED_PAIR_FLOOR
        && j.addressed_pairs() + blind.len() >= ADDRESSED_PAIR_FLOOR
    {
        println!(
            "\n    THE BLIND SPOT COVERS THE GAP. {} + {} >= {ADDRESSED_PAIR_FLOOR}. The pair",
            j.addressed_pairs(),
            blind.len(),
        );
        println!("    half is FALSIFIED as the declaration defines the corpus, and the margin is");
        println!("    smaller than a NAMED limitation of the reader. That is recorded here at the");
        println!("    point the figure is printed, never left to the prose, because a gate whose");
        println!("    verdict is inside its own reader's blind spot is weak evidence and must say");
        println!("    so. What would settle it is a sequence-reading corpus, measured — not this");
        println!("    ceiling, which is an upper bound and admits every line without judging it.");
    }
}

fn pct(n: usize, d: usize) -> f64 {
    if d == 0 {
        0.0
    } else {
        n as f64 * 100.0 / d as f64
    }
}

// ── The gate ────────────────────────────────────────────────────────────────

/// **S-411's blocking gate.** Skips — loudly — when no corpus is configured, so
/// `cargo test --workspace` stays green on a machine without one.
///
/// Per [NFR-CC-04], a run that sees no estate reports VOID and asserts nothing:
/// zero addressed pairs is not the finding, there is no finding.
///
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
#[test]
fn measure_config_declared_coupling_over_the_reference_workspace() {
    let Some(root) = crate::corpus_root() else {
        eprintln!(
            "VOID: set LOGOS_REF_WORKSPACE=<path to the reference workspace> to run the \
             S-411 config-declared coupling gate. A run that sees no estate reports VOID, \
             never zero (see config_declared_coupling_floor.txt)."
        );
        return;
    };
    let j = judgement(&root);
    report(j);
    println!("\n{RECORDED_FINDING}");

    assert_estate_engaged(j, &root);

    let pairs = j.addressed_pairs();
    let topics = j.shared_topics();
    println!(
        "\nVERDICT (pairs):  addressed member pairs {pairs} against a floor of \
         {ADDRESSED_PAIR_FLOOR} declared before the run  =>  {}",
        verdict(pairs, ADDRESSED_PAIR_FLOOR),
    );
    println!(
        "VERDICT (topics): shared topics {topics} against a floor of {SHARED_TOPIC_FLOOR} \
         declared before the run  =>  {}",
        verdict(topics, SHARED_TOPIC_FLOOR),
    );

    assert_the_recorded_verdict(j);
    assert_the_recorded_reconciliation(j);
}

/// The recorded finding, pinned. S-411 measured the pair half FALSIFIED and the
/// topic half HOLDING; the assertions pin both verdicts and both splits so a
/// change that flips either has to be decided rather than absorbed.
///
/// Re-open CR-131 §8 and re-decide the CR before relaxing any of these — do not
/// edit them to make the suite green.
fn assert_the_recorded_verdict(j: &Judgement) {
    let pairs = j.addressed_pairs();
    assert!(
        pairs < ADDRESSED_PAIR_FLOOR,
        "S-411's recorded finding is that committed configuration proves \
         {RECORDED_ADDRESSED_PAIRS} addressed member pairs, below the floor of \
         {ADDRESSED_PAIR_FLOOR} declared before the run; this run found {pairs}. If that is \
         real, CR-131 cluster B's gate has re-opened: re-decide CR-131 §8, lift ADR-65's \
         supersession banner, and plan S-412..S-415 and S-418 — do not relax this.",
    );
    assert_eq!(
        pairs, RECORDED_ADDRESSED_PAIRS,
        "the addressed-pair figure moved from the recorded {RECORDED_ADDRESSED_PAIRS} to \
         {pairs} without the finding being re-recorded",
    );
    // The split is the criterion, so the subtracted half is pinned too.
    assert_eq!(
        j.pairs(PairOutcome::PathOnlyMatched).len(),
        RECORDED_PATH_ONLY_PAIRS,
        "the path-only-matched half of the split moved from {RECORDED_PATH_ONLY_PAIRS} to {}",
        j.pairs(PairOutcome::PathOnlyMatched).len(),
    );
    let topics = j.shared_topics();
    assert!(
        topics >= SHARED_TOPIC_FLOOR,
        "S-411's recorded finding is that the topic half HOLDS at {RECORDED_SHARED_TOPICS} \
         against a floor of {SHARED_TOPIC_FLOOR}; this run found {topics}. The half that \
         cleared no longer clears — re-record the finding and correct ADR-65's banner.",
    );
    assert_eq!(
        topics, RECORDED_SHARED_TOPICS,
        "the shared-topic figure moved from the recorded {RECORDED_SHARED_TOPICS} to {topics} \
         without the finding being re-recorded",
    );
    assert_eq!(
        j.shared_topics_as_written(),
        RECORDED_SHARED_TOPICS_AS_WRITTEN,
        "the AS-WRITTEN reading answered {} rather than the recorded \
         {RECORDED_SHARED_TOPICS_AS_WRITTEN}; the declaration's reason for making RESOLVED \
         decisive was that the two populations are disjoint, and they no longer are",
        j.shared_topics_as_written(),
    );
}

/// The reconciliation S-411's acceptance criteria require, asserted rather than
/// printed: a run whose target census had silently drifted from [S-384]'s and
/// [S-400]'s would be measuring a different estate while printing the same words.
///
/// Equalities, not floors, and deliberately: these are reproductions of two
/// recorded measurements over the same corpus. If the estate grows they move, and
/// when they move the comparison is void and has to be re-recorded rather than
/// quietly widened.
///
/// [S-384]: ../../../docs/planning/journal.md#s-384-measure-service-identity-resolvability-across-the-deploy-corpus
/// [S-400]: ../../../docs/planning/journal.md#s-400-measure-whether-a-runtime-port-identifies-the-callee
fn assert_the_recorded_reconciliation(j: &Judgement) {
    assert_eq!(
        j.deploy_member_labels.len(),
        S384_MEMBER_LABELS,
        "this run resolved {} distinct MEMBER labels among deploy target references against \
         S-384's recorded {S384_MEMBER_LABELS}; the two gates are no longer reading the same \
         estate and the reconciliation S-411 AC3 requires is void",
        j.deploy_member_labels.len(),
    );
    let members_with_port: BTreeSet<&String> = j.port_owners.values().flatten().collect();
    assert_eq!(
        (members_with_port.len(), j.port_owners.len()),
        (S400_PORT_DECLARING_MEMBERS, S400_DISTINCT_PORT_VALUES),
        "the `server.port` index behind S-400's port reconciliation moved from \
         {S400_PORT_DECLARING_MEMBERS} members / {S400_DISTINCT_PORT_VALUES} distinct values \
         to {} / {}",
        members_with_port.len(),
        j.port_owners.len(),
    );
    let (_, unique) = port_census(j);
    assert!(
        unique >= S400_UNIQUE_PORTS,
        "this run found {unique} ports identifying exactly one member, below S-400's \
         recorded {S400_UNIQUE_PORTS} over a NARROWER population — a wider population \
         cannot resolve fewer ports, so the join has drifted",
    );
    assert_eq!(
        j.blind_spot_pairs_count(),
        RECORDED_BLIND_SPOT_PAIRS,
        "the sequence blind spot would add {} pairs rather than the recorded \
         {RECORDED_BLIND_SPOT_PAIRS}; the finding's statement that it covers the \
         falsification's margin has to be re-checked before this constant is changed",
        j.blind_spot_pairs_count(),
    );
}

fn verdict(measured: usize, floor: usize) -> &'static str {
    if measured >= floor {
        "HOLDS"
    } else {
        "FALSIFIED"
    }
}

/// The corpus must have engaged at all. A run that walked nothing reports "0
/// addressed pairs" exactly like a run that walked everything and found none, and
/// only these separate them.
fn assert_estate_engaged(j: &Judgement, root: &Path) {
    assert!(
        j.members.len() >= 80,
        "the workspace at {} yielded {} members — point LOGOS_REF_WORKSPACE at the \
         reference estate rather than at a single repository",
        root.display(),
        j.members.len(),
    );
    assert!(
        j.runnable.len() >= 60,
        "only {} of {} members were classified runnable; the plugin registry did not load \
         or the file roster is empty, so every pair would fall in `target is not runnable` \
         for a reason that is about this harness and not about the estate",
        j.runnable.len(),
        j.members.len(),
    );
    assert!(
        j.cost.files_read >= 100,
        "the overlay walk read {} deploy-shaped files — the hidden-directory walk did not \
         engage and the deploy half of the corpus is empty",
        j.cost.files_read,
    );
    assert!(
        j.cost.files_in_hidden > 0,
        "no deploy-shaped file was read from inside a hidden directory, so the one thing \
         this gate adds to the shipped corpus walk did nothing",
    );
    assert!(
        !j.topics_resolved.is_empty(),
        "the broker arm captured no topic literal anywhere in the estate, so the topic half \
         is vacuous rather than falsified",
    );
    assert!(
        j.application_scalars >= 1000,
        "the application corpus yielded {} scalars — ConfigCorpus::discover did not engage",
        j.application_scalars,
    );
}

// ── Fixtures ────────────────────────────────────────────────────────────────
//
// The estate measurement runs only where a corpus is configured, so every rule it
// depends on is pinned here too, and each matcher is probed with the near miss it
// must reject rather than only with the case it must accept.

#[cfg(test)]
mod fixtures {
    use super::*;

    fn corpus_of(claims: &[(&str, Tier, &str)]) -> Corpus {
        let mut c = Corpus::default();
        for (member, tier, label) in claims {
            c.members.insert((*member).to_string());
            c.claim(member, *tier, label, "fixture");
        }
        c
    }

    fn runnable(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|n| (*n).to_string()).collect()
    }

    /// An empty [`Judgement`] a fixture fills one field of, so a rule about one
    /// reported figure can be pinned without an estate.
    fn probe_judgement() -> Judgement {
        Judgement {
            members: BTreeSet::new(),
            runnable: BTreeSet::new(),
            targets: Vec::new(),
            topics_resolved: Vec::new(),
            topics_as_written: Vec::new(),
            cost: WalkCost::default(),
            sequence_host_ceiling: Vec::new(),
            path_only: BTreeSet::new(),
            application_scalars: 0,
            deploy_scalars: 0,
            deploy_member_labels: BTreeSet::new(),
            deploy_external_labels: BTreeSet::new(),
            port_owners: BTreeMap::new(),
            blind_spot: BTreeSet::new(),
        }
    }

    // ── The floors are the declared ones ────────────────────────────────────

    #[test]
    fn the_floors_are_the_ones_declared_before_the_run() {
        // Reads the DECLARATION, not the constants. Asserting
        // `ADDRESSED_PAIR_FLOOR == 12` would compare a value with the literal
        // written a few hundred lines above it, which no mutation can falsify
        // and which says nothing about what was declared before the run.
        //
        // Requires the metric's floor line to be UNIQUE, and that is the whole
        // guard. An earlier version took the FIRST line that both named the
        // metric and began `>= `, under a comment claiming that a later `>= n`
        // line could not silently redefine a floor. It could — review mutated
        // the declaration to read
        //
        //     >= 12 ADDRESSED MEMBER PAIRS was the pilot illustration, superseded below.
        //     >= 20 ADDRESSED MEMBER PAIRS on the pec-services estate.
        //
        // and this test still certified the constant 12 against a declaration
        // that said 20. That is the one failure this file exists to make
        // impossible, because every other assertion here trusts the floor it
        // parses. Two matching lines is now itself the failure, so a declaration
        // cannot carry a second reading of its own floor at all.
        let declared = |metric: &str| -> usize {
            let hits: Vec<usize> = DECLARED_FLOOR
                .lines()
                .filter(|l| l.contains(metric))
                .filter_map(|l| {
                    l.trim().strip_prefix(">= ")?.split_whitespace().next()?.parse().ok()
                })
                .collect();
            assert_eq!(
                hits.len(),
                1,
                "the declaration must state the floor for `{metric}` on exactly ONE \
                 `>= NN {metric}` line; found {} ({hits:?}). A declaration that states its \
                 own floor twice has no floor.",
                hits.len(),
            );
            hits[0]
        };
        assert_eq!(
            declared("ADDRESSED MEMBER PAIRS"),
            ADDRESSED_PAIR_FLOOR,
            "ADDRESSED_PAIR_FLOOR is {ADDRESSED_PAIR_FLOOR} but the floor declared before \
             the run was {}. The declaration is the record; change the constant only by \
             re-deciding CR-131, never to make a run clear it.",
            declared("ADDRESSED MEMBER PAIRS"),
        );
        assert_eq!(
            declared("SHARED TOPICS"),
            SHARED_TOPIC_FLOOR,
            "SHARED_TOPIC_FLOOR is {SHARED_TOPIC_FLOOR} but the floor declared before the \
             run was {}",
            declared("SHARED TOPICS"),
        );
        assert!(
            DECLARED_FLOOR.contains("2026-09-15T07:08:24Z"),
            "the declaration must carry the UTC timestamp that makes it a floor rather than \
             a result",
        );
        // The terms the floors turn on must be fixed in the declaration, or the
        // units they name exist only in this file's comments.
        for term in [
            "ADDRESSED MEMBER PAIR",
            "SHARED TOPIC",
            "RUNNABLE",
            "COMMITTED VALUE",
            "PATH-ONLY-MATCHED",
            "CAPTURED TOPIC IDENTITY",
        ] {
            assert!(
                DECLARED_FLOOR.contains(term),
                "the declaration must fix the term `{term}`, or the metric is defined only \
                 in the measurement code it is supposed to constrain",
            );
        }
    }

    // ── label_state ─────────────────────────────────────────────────────────

    #[test]
    fn a_label_one_member_claims_resolves_to_it() {
        let c = corpus_of(&[("mailbox-api", Tier::Deploy, "mailbox-api")]);
        assert_eq!(label_state(&c, "mailbox-api"), LabelState::Resolves("mailbox-api"));
    }

    #[test]
    fn a_same_tier_collision_is_a_collision_and_not_an_external() {
        // The fork case S-384 measured: two members claim one chart name.
        let c = corpus_of(&[
            ("archive-api", Tier::Deploy, "archive-api"),
            ("archive-api-logiclens-fork", Tier::Deploy, "archive-api"),
        ]);
        assert_eq!(label_state(&c, "archive-api"), LabelState::Collision);
    }

    #[test]
    fn a_label_no_member_claims_is_unclaimed() {
        let c = corpus_of(&[("mailbox-api", Tier::Deploy, "mailbox-api")]);
        assert_eq!(label_state(&c, "keycloak-http"), LabelState::Unclaimed);
    }

    #[test]
    fn a_tier_five_claim_alone_is_never_decisive() {
        // FR-WS-20 AC6: an artifact id is recorded and never decides. A label
        // claimed ONLY at tier 5 must read as unclaimed, not as a resolution.
        let c = corpus_of(&[("archive-api", Tier::Artifact, "api")]);
        assert_eq!(label_state(&c, "api"), LabelState::Unclaimed);
    }

    #[test]
    fn label_state_never_disagrees_with_the_s384_classifier() {
        // The one guard against this module growing a second reading of the tier
        // ladder: whatever `member_for` resolves, `label_state` must resolve
        // identically, and only its `None` may be refined.
        let c = corpus_of(&[
            ("a", Tier::Deploy, "alpha"),
            ("b", Tier::Deploy, "beta"),
            ("c", Tier::Deploy, "beta"),
            ("d", Tier::Directory, "delta"),
        ]);
        for label in ["alpha", "beta", "delta", "absent"] {
            match (c.member_for(label), label_state(&c, label)) {
                (Some(m), LabelState::Resolves(s)) => assert_eq!(m, s),
                (None, LabelState::Collision | LabelState::Unclaimed) => {}
                (a, b) => panic!("member_for({label}) = {a:?} but label_state = {b:?}"),
            }
        }
    }

    // ── classify_pair ───────────────────────────────────────────────────────

    #[test]
    fn every_pair_bucket_is_reachable_and_the_order_is_the_declared_one() {
        let path_only: BTreeSet<(String, String)> =
            [("agg".to_string(), "bound".to_string())].into_iter().collect();
        let run = runnable(&["agg", "callee", "bound"]);
        let cases: [(&str, LabelState<'_>, PairOutcome); 6] = [
            ("agg", LabelState::Resolves("agg"), PairOutcome::SelfTie),
            ("agg", LabelState::Collision, PairOutcome::SameTierCollision),
            ("agg", LabelState::Unclaimed, PairOutcome::NoMemberLabel),
            ("agg", LabelState::Resolves("charts"), PairOutcome::TargetNotRunnable),
            ("agg", LabelState::Resolves("bound"), PairOutcome::PathOnlyMatched),
            ("agg", LabelState::Resolves("callee"), PairOutcome::Addressed),
        ];
        for (consumer, state, expected) in cases {
            assert_eq!(
                classify_pair(consumer, state, &run, &path_only).0,
                expected,
                "classifying {state:?} for {consumer}",
            );
        }
    }

    #[test]
    fn a_self_tie_is_a_self_tie_even_when_the_member_is_runnable() {
        // The near miss: `runnable` contains the consumer, so an order that
        // tested runnability first would still say Addressed.
        let run = runnable(&["agg"]);
        assert_eq!(
            classify_pair("agg", LabelState::Resolves("agg"), &run, &BTreeSet::new()).0,
            PairOutcome::SelfTie,
        );
    }

    #[test]
    fn path_only_matching_is_directional() {
        // `(a, b)` bound by path does not make `(b, a)` bound by path.
        let path_only: BTreeSet<(String, String)> =
            [("a".to_string(), "b".to_string())].into_iter().collect();
        let run = runnable(&["a", "b"]);
        assert_eq!(
            classify_pair("b", LabelState::Resolves("a"), &run, &path_only).0,
            PairOutcome::Addressed,
        );
    }

    // ── bare_host_label and the sibling-port rule ───────────────────────────

    #[test]
    fn a_bare_kubernetes_service_name_yields_its_first_label() {
        assert_eq!(
            bare_host_label("mailbox-api.pec-services.svc.cluster.local").as_deref(),
            Some("mailbox-api"),
        );
        assert_eq!(bare_host_label("mailbox-api").as_deref(), Some("mailbox-api"));
    }

    #[test]
    fn a_bare_host_that_is_not_a_name_establishes_nothing() {
        // Each of these sits one step from matching, and each would fabricate a
        // host label if the rule were a bare character class.
        // Every one of these sits one character from matching, and each probes a
        // different clause: a bare word, a word with no dot or dash, a number, an
        // IP literal, a template, a URL, an email, an authority with a port, and
        // the empty string.
        for near_miss in [
            "true",
            "changeit",
            "9090",
            "192.168.54.134",
            "${HOST}",
            "http://mailbox-api.svc",
            "user@mailbox-api.svc",
            "mailbox-api.svc:9000",
            "mailbox api",
            "",
        ] {
            assert_eq!(
                bare_host_label(near_miss),
                None,
                "{near_miss:?} must not read as a host label",
            );
        }
    }

    #[test]
    fn a_host_key_is_admitted_only_beside_a_sibling_port() {
        let with_port: BTreeMap<String, BTreeSet<String>> = [
            ("proxy.host".to_string(), ["mailbox-api.svc".to_string()].into_iter().collect()),
            ("proxy.port".to_string(), ["9000".to_string()].into_iter().collect()),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            host_keys_with_sibling_port(&with_port),
            vec![("proxy.host".to_string(), "mailbox-api".to_string())],
        );

        let without_port: BTreeMap<String, BTreeSet<String>> = [(
            "proxy.host".to_string(),
            ["mailbox-api.svc".to_string()].into_iter().collect(),
        )]
        .into_iter()
        .collect();
        assert!(host_keys_with_sibling_port(&without_port).is_empty());
    }

    #[test]
    fn a_port_under_a_different_parent_is_not_a_sibling() {
        // The near miss the `parent` prefix exists for: `a.host` beside `b.port`.
        let values: BTreeMap<String, BTreeSet<String>> = [
            ("a.host".to_string(), ["mailbox-api.svc".to_string()].into_iter().collect()),
            ("b.port".to_string(), ["9000".to_string()].into_iter().collect()),
        ]
        .into_iter()
        .collect();
        assert!(host_keys_with_sibling_port(&values).is_empty());
    }

    // ── placeholder_key and the two readings ────────────────────────────────

    #[test]
    fn a_property_placeholder_yields_its_key_and_a_plain_topic_does_not() {
        assert_eq!(
            placeholder_key("${spring.kafka.topics.archive-events}"),
            Some("spring.kafka.topics.archive-events"),
        );
        assert_eq!(placeholder_key("${a.b:fallback}"), Some("a.b"));
        assert_eq!(placeholder_key("archive-events"), None);
        // Near misses: an unterminated placeholder and a bare `$`.
        assert_eq!(placeholder_key("${unterminated"), None);
        assert_eq!(placeholder_key("$notaplaceholder"), None);
        assert_eq!(placeholder_key("${}"), None);
    }

    #[test]
    fn every_topic_bucket_is_reachable() {
        assert_eq!(classify_topic(None, 0), TopicOutcome::DanglingUnresolved);
        assert_eq!(classify_topic(Some("t"), 0), TopicOutcome::DanglingNoDeclarer);
        assert_eq!(classify_topic(Some("t"), 1), TopicOutcome::OneSideOnly);
        assert_eq!(classify_topic(Some("t"), 2), TopicOutcome::BothSidesDeclared);
        assert_eq!(classify_topic(Some("t"), 9), TopicOutcome::BothSidesDeclared);
    }

    #[test]
    fn an_unresolved_placeholder_is_dangling_whatever_its_declarer_count_says() {
        // The guard against the two readings crossing: a topic whose identity is
        // unknown cannot be shared, even if the count passed in is large.
        assert_eq!(classify_topic(None, 7), TopicOutcome::DanglingUnresolved);
    }

    // ── judge_topics ────────────────────────────────────────────────────────

    fn scalar(member: &str, key: &str, value: &str) -> Scalar {
        Scalar {
            member: member.to_string(),
            key: key.to_string(),
            value: value.to_string(),
            source: SourceSet::Application,
            file: format!("{member}/src/main/resources/application.yml"),
        }
    }

    #[test]
    fn a_topic_two_members_commit_is_shared_under_the_resolved_reading_only() {
        let captured = vec![CapturedTopic {
            as_written: "${spring.kafka.topics.archive-events}".to_string(),
            resolved: Some("archive-events".to_string()),
            publishes: 0,
            subscribes: 3,
        }];
        let scalars = [
            scalar("a", "spring.kafka.topics.archiveevents", "archive-events"),
            scalar("b", "spring.kafka.topics.archiveevents", "archive-events"),
        ];

        let resolved = judge_topics(&captured, &scalars, TopicReading::Resolved);
        assert_eq!(resolved[0].outcome, TopicOutcome::BothSidesDeclared);
        assert_eq!(resolved[0].declarers.len(), 2);

        // The same run under FR-WS-10's in-force key rule: nobody commits the
        // PLACEHOLDER, so the reading answers zero. This is the asymmetry the
        // declaration fixes, and it is asserted rather than described.
        let as_written = judge_topics(&captured, &scalars, TopicReading::AsWritten);
        assert_eq!(as_written[0].outcome, TopicOutcome::DanglingNoDeclarer);
    }

    #[test]
    fn two_literals_resolving_to_one_identity_are_one_shared_topic() {
        // The grain the declaration fixes: "The grain is the TOPIC, not the
        // (member, topic) row and not the member pair." Counting captured
        // LITERALS here would answer 2 — and would do so silently, because on
        // today's estate each identity has exactly one literal and the two
        // counts coincide.
        let captured = vec![
            CapturedTopic {
                as_written: "${spring.kafka.topics.orders}".to_string(),
                resolved: Some("orders-v1".to_string()),
                publishes: 0,
                subscribes: 1,
            },
            CapturedTopic {
                as_written: "orders-v1".to_string(),
                resolved: Some("orders-v1".to_string()),
                publishes: 1,
                subscribes: 0,
            },
        ];
        let scalars = [
            scalar("a", "spring.kafka.topics.orders", "orders-v1"),
            scalar("b", "spring.kafka.topics.orders", "orders-v1"),
        ];
        let j = Judgement {
            topics_resolved: judge_topics(&captured, &scalars, TopicReading::Resolved),
            ..probe_judgement()
        };
        assert_eq!(j.topics(TopicOutcome::BothSidesDeclared).len(), 2, "two captured literals");
        assert_eq!(j.shared_topics(), 1, "but ONE topic identity, which is the declared grain");
    }

    #[test]
    fn one_member_committing_a_topic_twice_is_still_one_declarer() {
        // The grain guard: declarers are distinct MEMBERS, so two profiles of one
        // member cannot manufacture a shared topic.
        let captured = vec![CapturedTopic {
            as_written: "orders".to_string(),
            resolved: Some("orders".to_string()),
            publishes: 1,
            subscribes: 1,
        }];
        let scalars = [
            scalar("a", "spring.kafka.topics.orders", "orders"),
            Scalar { file: "a/src/main/resources/application-prod.yml".to_string(), ..scalar("a", "spring.kafka.topics.orders", "orders") },
        ];
        let judged = judge_topics(&captured, &scalars, TopicReading::Resolved);
        assert_eq!(judged[0].outcome, TopicOutcome::OneSideOnly);
        assert_eq!(judged[0].evidence.len(), 1);
    }

    // ── the sequence ceiling ────────────────────────────────────────────────

    #[test]
    fn the_sequence_ceiling_counts_hosts_the_shipped_parser_cannot_bind() {
        let text = "proxy:\n  upstreams:\n    - host: mailbox-api.svc\n      port: 9000\n    \
                    - host: reporting-api.svc\n      port: 9010\n";
        assert_eq!(
            sequence_host_lines(text),
            vec!["mailbox-api.svc".to_string(), "reporting-api.svc".to_string()],
        );
        // And the shipped parser really does bind neither, which is the whole
        // reason the ceiling exists — asserted, not assumed.
        let parsed = parse_yaml(text);
        assert!(
            !parsed.keys().any(|k| k.ends_with("host")),
            "parse_yaml bound a host under a sequence; the ceiling's premise has changed",
        );
    }

    #[test]
    fn a_mapping_host_is_not_counted_toward_the_sequence_ceiling() {
        // The near miss: a `host:` key the shipped parser DOES bind must not be
        // double-counted as something the parser missed.
        let text = "proxy:\n  host: mailbox-api.svc\n  port: 9000\n";
        assert!(sequence_host_lines(text).is_empty());
        assert!(parse_yaml(text).contains_key("proxy.host"));
    }
}
