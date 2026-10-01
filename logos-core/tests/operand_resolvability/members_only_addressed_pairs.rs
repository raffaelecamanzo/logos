//! **S-492's blocking measurement gate** — does the addressed-pair half hold
//! once identity labels come from the workspace manifest's members only?
//!
//! [S-475] measured the addressed half 11 against a floor of 12 on its decisive
//! STRICT reading and printed one sensitivity it was forbidden to count: the
//! label `archive-api` collides at tier 1 with a directory **outside** the
//! manifest (`archive-api-logiclens-fork`), so two aggregator pairs resolve to
//! nothing. [CR-158] re-runs that gate with **one** change — tier 1–3 identity
//! labels are collected from the manifest's members only — against the same
//! metric and the same unrevised floor. This module is that re-run.
//!
//! The floor, the population and the decisive reading are declared in
//! [`members_only_addressed_pairs_floor.txt`], committed before this module
//! existed; [`parse_declaration`] reads all three out of it.
//!
//! # Everything but the registry is S-475's, called
//!
//! The target population is [S-475]'s own [`remeasure`] — its item-scoped
//! reader, [S-411]'s overlay walk, admission rule and application reader — and
//! each of its targets is re-judged here by [S-411]'s [`label_state`] and
//! [`classify_pair`] with [S-411]'s runnable set. The three readings are
//! [S-475]'s [`Reading`] through its [`addressed_pairs`]. The baseline every
//! "gained" and "lost" is read against is [S-475]'s own [`Remeasure`], not a
//! reconstruction of it.
//!
//! Two inputs to that classifier change, both by filtering what [S-384]
//! already built rather than by a second walk:
//!
//! - **the registry** — [`members_only`] keeps a tier 1–3 claim only when its
//!   member is in the manifest. Tier 4–5 claims are kept as [S-384] records
//!   them; the declaration says why, and [`MembersOnly::tier4_strict`] prints
//!   what scoping tier 4 too would have moved.
//! - **the path-only subtraction** — [S-384]'s consumer-site judgement, called
//!   through [`identity::judge_pairs`] with the members-only registry and a
//!   provider index without the fork ([`members_only_providers`]). [S-411]
//!   derives its own `path_only` from the same function; the estate run
//!   asserts that calling it with [S-384]'s unscoped inputs reproduces
//!   [S-411]'s set exactly, so the subtraction cannot have drifted in the
//!   reuse.
//!
//! The members-only registry is the same filter [S-475]'s
//! `manifest_registry_sensitivity` applies inline (there over every tier). It
//! is pinned against that sensitivity on the estate: the pairs this registry
//! adds under [S-411]'s path-only set must be exactly the pairs [S-475]
//! printed.
//!
//! [CR-158]: ../../../docs/requests/CR-158-addressed-pairs-through-a-members-only-identity-registry.md
//! [S-384]: ../../../docs/planning/journal.md#s-384-measure-service-identity-resolvability-across-the-deploy-corpus
//! [S-411]: ../../../docs/planning/journal.md#s-411-measure-config-declared-coupling-over-the-reference-estate
//! [S-475]: ../../../docs/planning/journal.md#s-475-re-measure-the-addressed-pair-gate-through-a-yaml-sequence-reading-corpus
//! [`members_only_addressed_pairs_floor.txt`]: ./members_only_addressed_pairs_floor.txt

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use super::config_declared_coupling::{
    classify_pair, label_state, JudgedTarget, LabelState, PairOutcome, Target,
};
use super::identity::{self, Corpus, Pair, PairClass, Providers, Resolution, Tier};
use super::sequence_addressed_pairs::{addressed_pairs, remeasure, Reading, Remeasure};

/// **The floor, the population and the decisive reading, as declared before
/// the run.** `include_str!` so the file cannot be deleted or renamed without
/// breaking compilation, following [S-475]'s `DECLARED_FLOOR`.
///
/// [S-475]: ../../../docs/planning/journal.md#s-475-re-measure-the-addressed-pair-gate-through-a-yaml-sequence-reading-corpus
pub const DECLARED: &str = include_str!("members_only_addressed_pairs_floor.txt");

/// The recorded verdict, reproduced by the run and printed by it.
pub const RECORDED_FINDING: &str = include_str!("members_only_addressed_pairs_finding.txt");

/// The provider both aggregator pairs address, and the label the fork shares.
pub const FORKED_PROVIDER: &str = "archive-api";

/// The two pairs [S-475]'s sensitivity named, whose fate under the fork-free
/// path-only subtraction [CR-158] §3.2 B2 asks for by name.
///
/// [CR-158]: ../../../docs/requests/CR-158-addressed-pairs-through-a-members-only-identity-registry.md
/// [S-475]: ../../../docs/planning/journal.md#s-475-re-measure-the-addressed-pair-gate-through-a-yaml-sequence-reading-corpus
pub const AGGREGATOR_PAIRS: [(&str, &str); 2] = [
    ("funnel-aggregator-api", FORKED_PROVIDER),
    ("mailbox-aggregator-api", FORKED_PROVIDER),
];

// ── The declaration ─────────────────────────────────────────────────────────

/// What the declaration fixes, parsed — never restated as a constant beside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declaration {
    pub floor: usize,
    pub population: BTreeSet<String>,
    pub decisive: Reading,
}

/// Parse the three declared terms. A field missing, doubled or malformed, or a
/// member list whose length disagrees with its own `POPULATION:` line, is an
/// error: a declaration that cannot be read must not default to anything.
pub fn parse_declaration(text: &str) -> Result<Declaration, String> {
    let field = |name: &str| -> Result<&str, String> {
        let mut hits = text.lines().filter_map(|l| l.strip_prefix(name)).map(str::trim);
        let first = hits.next().ok_or_else(|| format!("no `{name}` line"))?;
        match hits.next() {
            Some(_) => Err(format!("more than one `{name}` line")),
            None => Ok(first),
        }
    };
    let number = |name: &str| -> Result<usize, String> {
        field(name)?.parse().map_err(|e| format!("`{name}` is not a count: {e}"))
    };
    let floor = number("FLOOR:")?;
    let size = number("POPULATION:")?;
    let decisive = match field("DECISIVE:")? {
        "STRICT" => Reading::Strict,
        "MANIFEST" => Reading::Manifest,
        "DIRECTORY" => Reading::Directory,
        other => return Err(format!("`DECISIVE:` names no reading: `{other}`")),
    };
    let mut inside = false;
    let mut population = BTreeSet::new();
    for line in text.lines() {
        if line.starts_with("MANIFEST MEMBERS — BEGIN") {
            inside = true;
        } else if line.starts_with("MANIFEST MEMBERS — END") {
            inside = false;
        } else if inside && !line.trim().is_empty() {
            population.insert(line.trim().to_string());
        }
    }
    if population.len() != size {
        return Err(format!(
            "the member list holds {} names where `POPULATION:` declares {size}",
            population.len()
        ));
    }
    Ok(Declaration {
        floor,
        population,
        decisive,
    })
}

/// The committed declaration, parsed. Panics on a malformed file: every arm of
/// this gate, the estate-blind one included, reads its floor from here.
pub fn declaration() -> Declaration {
    parse_declaration(DECLARED).unwrap_or_else(|e| {
        panic!("members_only_addressed_pairs_floor.txt does not parse: {e}")
    })
}

/// HOLDS iff the decisive reading reaches the declared floor.
pub fn verdict(n: usize, floor: usize) -> &'static str {
    if n >= floor {
        "HOLDS"
    } else {
        "FALSIFIED"
    }
}

/// The headline line, or VOID where no estate was measured ([NFR-CC-04]).
///
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
pub fn headline(strict: Option<usize>, floor: usize) -> String {
    match strict {
        None => "VOID: set LOGOS_REF_WORKSPACE=<path to the reference workspace> to run the \
                 S-492 members-only addressed-pair gate. A run that sees no estate reports VOID, \
                 never zero (see members_only_addressed_pairs_floor.txt)."
            .to_string(),
        Some(n) => format!(
            "VERDICT: addressed member pairs on the decisive STRICT reading through a \
             members-only identity registry {n} against a floor of {floor} declared before \
             the run  =>  {}",
            verdict(n, floor)
        ),
    }
}

// ── The one change: a members-only registry ─────────────────────────────────

/// Whether a claim at this tier is collected from manifest members only — tiers
/// 1–3, [CR-158] §3.2 A's scope sentence.
///
/// [CR-158]: ../../../docs/requests/CR-158-addressed-pairs-through-a-members-only-identity-registry.md
pub fn is_member_scoped(tier: Tier) -> bool {
    matches!(tier, Tier::Deploy | Tier::Container | Tier::Directory)
}

/// The identity corpus with every claim at a `scoped` tier dropped unless its
/// member is in `members`. The claims at the other tiers, the target references
/// and the Spring key census are carried unchanged; [S-384]'s judgement reads
/// only the claims and the targets.
///
/// [S-384]: ../../../docs/planning/journal.md#s-384-measure-service-identity-resolvability-across-the-deploy-corpus
pub fn members_only(
    corpus: &Corpus,
    members: &BTreeSet<String>,
    scoped: impl Fn(Tier) -> bool,
) -> Corpus {
    Corpus {
        members: corpus.members.clone(),
        claims: corpus
            .claims
            .iter()
            .filter(|c| !scoped(c.tier) || members.contains(&c.member))
            .cloned()
            .collect(),
        targets: corpus.targets.clone(),
        spring_by_flat: corpus.spring_by_flat.clone(),
        ..Corpus::default()
    }
}

/// [S-384]'s provider index without the routes a non-member serves — the
/// fork-free half of the path-only subtraction.
///
/// [S-384]: ../../../docs/planning/journal.md#s-384-measure-service-identity-resolvability-across-the-deploy-corpus
pub fn members_only_providers(providers: &Providers, members: &BTreeSet<String>) -> Providers {
    Providers {
        by_member: providers
            .by_member
            .iter()
            .filter(|(m, _)| members.contains(*m))
            .map(|(m, routes)| (m.clone(), routes.clone()))
            .collect(),
        ..Providers::default()
    }
}

/// The `(consumer, provider)` pairs path-only matching already binds, as
/// [S-411] derives its own `path_only` from [S-384]'s judged pairs.
///
/// [S-384]: ../../../docs/planning/journal.md#s-384-measure-service-identity-resolvability-across-the-deploy-corpus
/// [S-411]: ../../../docs/planning/journal.md#s-411-measure-config-declared-coupling-over-the-reference-estate
pub fn path_only_of(pairs: &[Pair]) -> BTreeSet<(String, String)> {
    pairs
        .iter()
        .filter(|p| p.class == PairClass::AlreadyBoundByPath)
        .map(|p| (p.consumer.clone(), p.provider.clone()))
        .collect()
}

/// Re-judge targets, in order, through [S-411]'s classifier under one
/// registry and one path-only set.
///
/// [S-411]: ../../../docs/planning/journal.md#s-411-measure-config-declared-coupling-over-the-reference-estate
pub fn judge_all<'a>(
    targets: impl Iterator<Item = &'a JudgedTarget>,
    registry: &Corpus,
    runnable: &BTreeSet<String>,
    path_only: &BTreeSet<(String, String)>,
) -> Vec<JudgedTarget> {
    targets
        .map(|t| {
            let state = label_state(registry, &t.target.label);
            let (outcome, provider) = classify_pair(&t.target.member, state, runnable, path_only);
            JudgedTarget {
                target: t.target.clone(),
                outcome,
                provider,
            }
        })
        .collect()
}

/// Every label that collides at the tier [`Corpus::member_for`] decides it at,
/// with that tier and its claimants — the collision census.
pub fn collisions(corpus: &Corpus) -> BTreeMap<String, (Tier, BTreeSet<String>)> {
    let tiers: Vec<(Tier, BTreeMap<&str, BTreeSet<&str>>)> = Tier::ALL
        .into_iter()
        .filter(|t| t.is_decisive())
        .map(|t| (t, corpus.labels_at(t)))
        .collect();
    let labels: BTreeSet<&str> = tiers.iter().flat_map(|(_, m)| m.keys().copied()).collect();
    labels
        .into_iter()
        .filter_map(|label| {
            let (tier, claimants) = tiers.iter().find_map(|(t, m)| Some((*t, m.get(label)?)))?;
            (claimants.len() > 1).then(|| {
                let names = claimants.iter().map(|s| s.to_string()).collect();
                (label.to_string(), (tier, names))
            })
        })
        .collect()
}

// ── The re-measurement ──────────────────────────────────────────────────────

/// Everything the re-run measures.
pub struct MembersOnly {
    pub declaration: Declaration,
    /// [S-475]'s own run — the targets, the readings' population and the
    /// baseline.
    ///
    /// [S-475]: ../../../docs/planning/journal.md#s-475-re-measure-the-addressed-pair-gate-through-a-yaml-sequence-reading-corpus
    pub s475: Remeasure,
    /// [S-384]'s own findings — the unscoped corpus and provider index.
    ///
    /// [S-384]: ../../../docs/planning/journal.md#s-384-measure-service-identity-resolvability-across-the-deploy-corpus
    pub s384: &'static identity::Findings,
    /// The members-only registry.
    pub registry: Corpus,
    /// [S-384]'s collision census over its unscoped corpus — the "before".
    ///
    /// [S-384]: ../../../docs/planning/journal.md#s-384-measure-service-identity-resolvability-across-the-deploy-corpus
    pub collisions_before: BTreeMap<String, (Tier, BTreeSet<String>)>,
    /// The consumer-site judgement taken without the fork — the bindings the
    /// path-only subtraction is made of.
    pub site_pairs: Vec<Pair>,
    pub path_only: BTreeSet<(String, String)>,
    /// [S-411]'s path-only set re-derived through the same call over [S-384]'s
    /// unscoped inputs — must equal [S-411]'s own.
    ///
    /// [S-384]: ../../../docs/planning/journal.md#s-384-measure-service-identity-resolvability-across-the-deploy-corpus
    /// [S-411]: ../../../docs/planning/journal.md#s-411-measure-config-declared-coupling-over-the-reference-estate
    pub reproduced_s411_path_only: BTreeSet<(String, String)>,
    /// S-475's targets re-judged — same order as [`Remeasure::targets`].
    pub targets: Vec<JudgedTarget>,
    /// STRICT pairs under the members-only registry and S-411's path-only set:
    /// what S-475's sensitivity predicted, before the fork-free subtraction.
    pub registry_only_strict: BTreeSet<(String, String)>,
    /// STRICT pairs had tier 4 been scoped too (path-only re-taken under that
    /// registry). A report, never counted.
    pub tier4_strict: BTreeSet<(String, String)>,
}

impl MembersOnly {
    /// Distinct addressed ordered pairs under one reading — S-475's rule.
    pub fn pairs(&self, reading: Reading) -> BTreeSet<(&str, &str)> {
        addressed_pairs(self.targets.iter(), reading, &self.s475.manifest)
    }

    /// Each S-475 judged target beside its members-only re-judgement.
    pub fn before_after(&self) -> impl Iterator<Item = (&JudgedTarget, &JudgedTarget)> {
        self.s475.targets().zip(&self.targets)
    }

    /// The targets proving one pair, under this run's judgement.
    pub fn evidence(&self, a: &str, b: &str) -> Vec<&JudgedTarget> {
        self.targets
            .iter()
            .filter(|t| t.target.member == a && t.provider.as_deref() == Some(b))
            .collect()
    }

    /// The members-only registry's collision census — the "after".
    pub fn collisions_after(&self) -> BTreeMap<String, (Tier, BTreeSet<String>)> {
        collisions(&self.registry)
    }

    /// `(references, distinct labels)` in the no-member bucket.
    pub fn no_member(targets: &[&JudgedTarget]) -> (usize, BTreeSet<String>) {
        let bucket: Vec<_> = targets
            .iter()
            .filter(|t| t.outcome == PairOutcome::NoMemberLabel)
            .collect();
        (bucket.len(), bucket.iter().map(|t| t.target.label.clone()).collect())
    }

    /// Whether the fork-free path-only subtraction kept one aggregator pair on
    /// STRICT, removed it, or never reached it, with the site bindings behind
    /// the answer.
    pub fn fate(&self, a: &str, b: &str) -> (Fate, Vec<&Pair>) {
        let bindings = self
            .site_pairs
            .iter()
            .filter(|p| p.consumer == a && p.provider == b)
            .collect();
        let outcomes: BTreeSet<PairOutcome> =
            self.evidence(a, b).iter().map(|t| t.outcome).collect();
        let fate = if self.pairs(Reading::Strict).contains(&(a, b)) {
            Fate::Kept
        } else if outcomes.contains(&PairOutcome::PathOnlyMatched) {
            Fate::Removed
        } else {
            Fate::NotReached(outcomes)
        };
        (fate, bindings)
    }
}

/// What the fork-free path-only subtraction did to one pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fate {
    /// Addressed on STRICT: no path-only binding subtracts it.
    Kept,
    /// Every target resolving to it is already bound by path alone.
    Removed,
    /// The registry never resolves it to an addressed or path-only target;
    /// the buckets it did fall in.
    NotReached(BTreeSet<PairOutcome>),
}

/// Run the re-measurement.
pub fn measure(root: &Path) -> MembersOnly {
    let declaration = declaration();
    let s475 = remeasure(root);
    let s384 = identity::findings(root);
    let members = &declaration.population;
    let runnable = &s475.s411.runnable;

    let registry = members_only(&s384.corpus, members, is_member_scoped);
    let providers = members_only_providers(&s384.providers, members);
    let site_pairs = identity::judge_pairs(root, &registry, &providers, Resolution::Literal);
    let path_only = path_only_of(&site_pairs);
    let reproduced_s411_path_only = path_only_of(&identity::judge_pairs(
        root,
        &s384.corpus,
        &s384.providers,
        Resolution::Literal,
    ));
    let targets = judge_all(s475.targets(), &registry, runnable, &path_only);

    let strict_of = |judged: &[JudgedTarget]| -> BTreeSet<(String, String)> {
        addressed_pairs(judged.iter(), Reading::Strict, &s475.manifest)
            .into_iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
    };
    let registry_only_strict =
        strict_of(&judge_all(s475.targets(), &registry, runnable, &s475.s411.path_only));
    let tier4 = members_only(&s384.corpus, members, Tier::is_decisive);
    let tier4_path_only =
        path_only_of(&identity::judge_pairs(root, &tier4, &providers, Resolution::Literal));
    let tier4_strict = strict_of(&judge_all(s475.targets(), &tier4, runnable, &tier4_path_only));

    MembersOnly {
        declaration,
        collisions_before: collisions(&s384.corpus),
        s475,
        s384,
        registry,
        site_pairs,
        path_only,
        reproduced_s411_path_only,
        targets,
        registry_only_strict,
        tier4_strict,
    }
}

// ── The report ──────────────────────────────────────────────────────────────

fn report(m: &MembersOnly) {
    println!("\n=== S-492 · the addressed-pair gate through a members-only identity registry ===");
    report_readings(m);
    for reading in Reading::ALL {
        report_gained_and_lost(m, reading);
    }
    report_aggregators(m);
    report_collisions(m);
    report_split(m);
    report_cross_checks(m);
}

fn report_readings(m: &MembersOnly) {
    let floor = m.declaration.floor;
    println!("\n  THE THREE READINGS (floor {floor}, declared before the run)\n");
    println!("    {:<58} {:>6} {:>7}  verdict", "reading", "pairs", "S-475");
    for reading in Reading::ALL {
        let n = m.pairs(reading).len();
        let was = m.s475.pairs(reading).len();
        println!("    {:<58} {n:>6} {was:>7}  {}", reading.label(), verdict(n, floor));
    }
    println!("\n    the STRICT pairs, enumerated:");
    for (a, b) in m.pairs(Reading::Strict) {
        println!("      {a:<36} -> {b}");
    }
}

fn report_gained_and_lost(m: &MembersOnly, reading: Reading) {
    let now = m.pairs(reading);
    let before = m.s475.pairs(reading);
    println!("\n  {} — versus S-475's {}", reading.label(), before.len());
    println!("    gained:");
    for (a, b) in now.difference(&before) {
        let evidence: Vec<_> = m
            .before_after()
            .filter(|(_, t)| t.outcome == PairOutcome::Addressed)
            .filter(|(_, t)| t.target.member == *a && t.provider.as_deref() == Some(*b))
            .collect();
        let overlays: BTreeSet<&str> =
            evidence.iter().map(|(_, t)| t.target.overlay.as_str()).collect();
        println!("      {a} -> {b}    overlays {overlays:?}");
        for (was, t) in evidence {
            println!(
                "          {} = {:?}  [{}]  {}   (S-475: {})",
                t.target.via_key,
                t.target.value,
                t.target.source.label(),
                t.target.file,
                was.outcome.label()
            );
        }
    }
    println!("    lost:");
    for (a, b) in before.difference(&now) {
        let fell: BTreeSet<&str> = m
            .before_after()
            .filter(|(was, _)| was.outcome == PairOutcome::Addressed)
            .filter(|(was, _)| was.target.member == *a && was.provider.as_deref() == Some(*b))
            .map(|(_, t)| t.outcome.label())
            .collect();
        println!("      {a} -> {b}    now: {fell:?}");
    }
}

fn report_aggregators(m: &MembersOnly) {
    println!("\n  THE TWO AGGREGATOR PAIRS UNDER THE FORK-FREE PATH-ONLY SUBTRACTION");
    for (a, b) in AGGREGATOR_PAIRS {
        let (fate, bindings) = m.fate(a, b);
        println!("\n    {a} -> {b}    {fate:?}");
        for t in m.evidence(a, b) {
            println!(
                "        target  {} = {:?}  {}  [{}]  => {}",
                t.target.via_key,
                t.target.value,
                t.target.file,
                t.target.overlay,
                t.outcome.label()
            );
        }
        for p in &bindings {
            println!(
                "        site    {}  {}  via {}  overlay {}  serving {}  => {}",
                p.site,
                p.normalized,
                p.via_key,
                p.overlay,
                p.serving,
                p.class.label()
            );
        }
        let templates: BTreeSet<&str> = bindings.iter().map(|p| p.normalized.as_str()).collect();
        for template in templates {
            println!(
                "        serving {template} in S-384's unscoped index: {:?}",
                m.s384.providers.serving(template)
            );
        }
        if bindings.is_empty() {
            println!("        site    (no consumer site of {a} resolves a template to {b})");
        }
    }
}

fn report_collisions(m: &MembersOnly) {
    let after = m.collisions_after();
    let target_labels: BTreeSet<&str> =
        m.targets.iter().map(|t| t.target.label.as_str()).collect();
    let mark = |label: &str| if target_labels.contains(label) { "  [a target label]" } else { "" };
    println!(
        "\n  COLLISIONS — {} before (S-384's corpus) · {} after (members-only)",
        m.collisions_before.len(),
        after.len()
    );
    println!("    removed:");
    for (label, (tier, claimants)) in &m.collisions_before {
        if !after.contains_key(label) {
            let now = match label_state(&m.registry, label) {
                LabelState::Resolves(member) => format!("resolves to {member}"),
                LabelState::Unclaimed => "resolves to no member".to_string(),
                LabelState::Collision => "still collides at another tier".to_string(),
            };
            println!("      {label:<40} tier {}  {claimants:?}  -> {now}{}", tier.label(), mark(label));
        }
    }
    println!("    kept:");
    for (label, (tier, claimants)) in &after {
        let new = if m.collisions_before.contains_key(label) { "" } else { "  NEW" };
        println!("      {label:<40} tier {}  {claimants:?}{new}{}", tier.label(), mark(label));
    }
}

fn report_split(m: &MembersOnly) {
    let before: Vec<&JudgedTarget> = m.s475.targets().collect();
    let after: Vec<&JudgedTarget> = m.targets.iter().collect();
    println!("\n  THE SIX-WAY SPLIT, BY REFERENCE (S-475 -> members-only)");
    for outcome in PairOutcome::ALL {
        let count = |ts: &[&JudgedTarget]| ts.iter().filter(|t| t.outcome == outcome).count();
        println!("    {:<44} {:>6} -> {}", outcome.label(), count(&before), count(&after));
    }
    let (n_before, labels_before) = MembersOnly::no_member(&before);
    let (n_after, labels_after) = MembersOnly::no_member(&after);
    println!(
        "\n  THE NO-MEMBER BUCKET: {n_before} references / {} labels before -> {n_after} / {} after",
        labels_before.len(),
        labels_after.len()
    );
    println!(
        "    labels entering: {:?}",
        labels_after.difference(&labels_before).collect::<Vec<_>>()
    );
    println!(
        "    labels leaving:  {:?}",
        labels_before.difference(&labels_after).collect::<Vec<_>>()
    );
}

fn report_cross_checks(m: &MembersOnly) {
    let s475_sensitivity: BTreeSet<(String, String)> =
        m.s475.sensitivity.iter().map(|(a, b, _)| (a.clone(), b.clone())).collect();
    println!("\n  CROSS-CHECKS ON THE REUSE");
    println!(
        "    S-411's path-only re-derived through identity::judge_pairs   {} / {} pairs, equal: {}",
        m.reproduced_s411_path_only.len(),
        m.s475.s411.path_only.len(),
        m.reproduced_s411_path_only == m.s475.s411.path_only
    );
    println!(
        "    pairs the registry adds under S-411's path-only              {:?}",
        m.registry_only_strict
            .iter()
            .filter(|p| !m.s475.pairs(Reading::Strict).contains(&(p.0.as_str(), p.1.as_str())))
            .collect::<Vec<_>>()
    );
    println!("    S-475's printed sensitivity                                  {s475_sensitivity:?}");
    println!(
        "    fork-free path-only set: {} pairs (S-411's: {})",
        m.path_only.len(),
        m.s475.s411.path_only.len()
    );
    for (a, b) in m.path_only.difference(&m.s475.s411.path_only) {
        println!("      + {a} -> {b}");
    }
    for (a, b) in m.s475.s411.path_only.difference(&m.path_only) {
        println!("      - {a} -> {b}");
    }
    let strict: BTreeSet<(String, String)> =
        m.pairs(Reading::Strict).into_iter().map(|(a, b)| (a.into(), b.into())).collect();
    println!(
        "\n  SENSITIVITY — tier 4 scoped too (NEVER counted): STRICT {} (added {:?}, lost {:?})",
        m.tier4_strict.len(),
        m.tier4_strict.difference(&strict).collect::<Vec<_>>(),
        strict.difference(&m.tier4_strict).collect::<Vec<_>>()
    );
}

// ── The gate ────────────────────────────────────────────────────────────────

/// **S-492's blocking gate.** Reports VOID — and asserts nothing — when no
/// estate is configured, so `cargo test --workspace` stays green without one.
#[test]
fn measure_members_only_addressed_pairs_over_the_reference_workspace() {
    let floor = declaration().floor;
    let Some(root) = crate::corpus_root() else {
        eprintln!("{}", headline(None, floor));
        return;
    };
    let m = measure(&root);
    report(&m);
    println!("\n{RECORDED_FINDING}");

    assert_the_population_is_the_declared_one(&m);
    assert_the_reuse_is_faithful(&m);

    let strict = m.pairs(Reading::Strict).len();
    println!(
        "\n{}\n  beside it: MANIFEST {} ({}) · DIRECTORY {} ({})",
        headline(Some(strict), floor),
        m.pairs(Reading::Manifest).len(),
        verdict(m.pairs(Reading::Manifest).len(), floor),
        m.pairs(Reading::Directory).len(),
        verdict(m.pairs(Reading::Directory).len(), floor),
    );
    assert_the_recorded_verdict(&m);
}

fn assert_the_population_is_the_declared_one(m: &MembersOnly) {
    let declared = &m.declaration.population;
    assert_eq!(
        &m.s475.manifest,
        declared,
        "the live manifest's members differ from the population declared before the run \
         (only in manifest: {:?}; only in declaration: {:?})",
        m.s475.manifest.difference(declared).collect::<Vec<_>>(),
        declared.difference(&m.s475.manifest).collect::<Vec<_>>(),
    );
    assert_eq!(m.declaration.decisive, Reading::Strict);
}

/// The estate engaged, it is the estate S-475 measured, and every reused piece
/// reproduces what it reproduced for its owner — so "unchanged" is a checked
/// statement, not a claim.
fn assert_the_reuse_is_faithful(m: &MembersOnly) {
    use super::sequence_addressed_pairs::{
        RECORDED_DIRECTORY_PAIRS, RECORDED_MANIFEST_PAIRS, RECORDED_STRICT_PAIRS,
    };
    assert!(m.s475.s411.members.len() >= 80, "not the estate");
    assert!(m.s475.collapsed.is_empty(), "S-475's reader collapsed a key: {:?}", m.s475.collapsed);
    assert_eq!(
        (
            m.s475.pairs(Reading::Strict).len(),
            m.s475.pairs(Reading::Manifest).len(),
            m.s475.pairs(Reading::Directory).len(),
        ),
        (RECORDED_STRICT_PAIRS, RECORDED_MANIFEST_PAIRS, RECORDED_DIRECTORY_PAIRS),
        "S-475's own readings moved — the estate drifted since S-475 and the baseline is not \
         the one its finding records",
    );
    assert_eq!(
        m.reproduced_s411_path_only, m.s475.s411.path_only,
        "identity::judge_pairs over S-384's unscoped inputs no longer reproduces S-411's \
         path-only set — the subtraction drifted in the reuse",
    );
    let added: BTreeSet<(String, String)> = m
        .registry_only_strict
        .iter()
        .filter(|p| !m.s475.pairs(Reading::Strict).contains(&(p.0.as_str(), p.1.as_str())))
        .cloned()
        .collect();
    let predicted: BTreeSet<(String, String)> =
        m.s475.sensitivity.iter().map(|(a, b, _)| (a.clone(), b.clone())).collect();
    assert_eq!(
        added, predicted,
        "under S-411's path-only set the members-only registry must add exactly the pairs \
         S-475's sensitivity printed",
    );
    assert!(!m.site_pairs.is_empty(), "the consumer-site judgement produced no pair");
}

/// The three readings this run measured, pinned so a drift in the registry,
/// the subtraction or the estate is a failure rather than a quietly different
/// verdict. See `members_only_addressed_pairs_finding.txt`.
pub const RECORDED_STRICT_PAIRS: usize = 11;
pub const RECORDED_MANIFEST_PAIRS: usize = 12;
pub const RECORDED_DIRECTORY_PAIRS: usize = 13;

/// `(collisions before, collisions after)` in the identity census — the
/// registry removes exactly one, `archive-api`, and keeps the rest.
pub const RECORDED_COLLISIONS: (usize, usize) = (22, 21);

/// `(same-tier collision, path-only-matched)` references after the change —
/// S-475 read `(75, 12)`: the two aggregator references move from one
/// subtraction to the other.
pub const RECORDED_SPLIT_AFTER: (usize, usize) = (73, 14);

/// `(references, distinct labels)` in the no-member bucket, before and after
/// — unchanged.
pub const RECORDED_NO_MEMBER: (usize, usize) = (393, 39);

/// The fork-free path-only set: S-411's 8 and the two aggregator pairs.
pub const RECORDED_PATH_ONLY: usize = 10;

const _: () = {
    assert!(
        RECORDED_STRICT_PAIRS < super::sequence_addressed_pairs::ADDRESSED_PAIR_FLOOR,
        "the recorded finding is FALSIFIED on the decisive reading; if that changes, \
         re-decide CR-158 and ADR-65 rather than relaxing the assertion",
    );
};

/// The recorded finding, pinned: the three readings, nothing gained or lost
/// against S-475, both aggregator pairs removed by path alone, and every
/// census figure the finding quotes.
fn assert_the_recorded_verdict(m: &MembersOnly) {
    let got = (
        m.pairs(Reading::Strict).len(),
        m.pairs(Reading::Manifest).len(),
        m.pairs(Reading::Directory).len(),
    );
    assert_eq!(
        got,
        (RECORDED_STRICT_PAIRS, RECORDED_MANIFEST_PAIRS, RECORDED_DIRECTORY_PAIRS),
        "(STRICT, MANIFEST, DIRECTORY) moved from the recorded figures to {got:?} without the \
         finding being re-recorded — re-decide CR-158 and ADR-65, do not relax this",
    );
    for reading in Reading::ALL {
        assert_eq!(m.pairs(reading), m.s475.pairs(reading), "{reading:?} gained or lost a pair");
    }
    for (a, b) in AGGREGATOR_PAIRS {
        let (fate, bindings) = m.fate(a, b);
        assert_eq!(fate, Fate::Removed, "{a} -> {b}");
        assert!(
            !bindings.is_empty()
                && bindings.iter().all(|p| p.class == PairClass::AlreadyBoundByPath
                    && p.normalized == "/v1/users/{}/archives/{}"
                    && p.serving == 1),
            "{a} -> {b}: the recorded binding is the sole-provider archive route",
        );
        assert_eq!(
            m.s384.providers.serving("/v1/users/{}/archives/{}"),
            [FORKED_PROVIDER, "archive-api-logiclens-fork"].into_iter().collect(),
            "with the fork the route is ambiguous — the reason S-411's path-only never held it",
        );
    }
    let after = m.collisions_after();
    let removed: Vec<&String> =
        m.collisions_before.keys().filter(|l| !after.contains_key(*l)).collect();
    assert_eq!(removed, [FORKED_PROVIDER], "the registry removes exactly one collision");
    assert_eq!((m.collisions_before.len(), after.len()), RECORDED_COLLISIONS);
    let count = |o: PairOutcome| m.targets.iter().filter(|t| t.outcome == o).count();
    assert_eq!(
        (count(PairOutcome::SameTierCollision), count(PairOutcome::PathOnlyMatched)),
        RECORDED_SPLIT_AFTER,
    );
    let before: Vec<&JudgedTarget> = m.s475.targets().collect();
    let after_targets: Vec<&JudgedTarget> = m.targets.iter().collect();
    for side in [&before, &after_targets] {
        let (n, labels) = MembersOnly::no_member(side);
        assert_eq!((n, labels.len()), RECORDED_NO_MEMBER, "the no-member bucket moved");
    }
    assert_eq!(m.path_only.len(), RECORDED_PATH_ONLY);
    assert!(m.s475.s411.path_only.is_subset(&m.path_only));
    let strict: BTreeSet<(String, String)> =
        m.pairs(Reading::Strict).into_iter().map(|(a, b)| (a.into(), b.into())).collect();
    assert_eq!(m.tier4_strict, strict, "scoping tier 4 too moves the decisive reading");
}

// ── Fixtures ────────────────────────────────────────────────────────────────
//
// The estate arm runs only where a corpus is configured, so the members-only
// rule is pinned here on every `cargo test`, each with the near miss it must
// reject. S-475's own fixtures run unchanged in its module.

#[cfg(test)]
mod fixtures {
    use super::*;

    const FORK: &str = "archive-api-logiclens-fork";

    fn set(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    fn corpus(claims: &[(&str, Tier, &str)]) -> Corpus {
        let mut c = Corpus::default();
        for (member, tier, label) in claims {
            c.members.insert(member.to_string());
            c.claim(member, *tier, label, "fixture");
        }
        c
    }

    fn scoped(c: &Corpus, members: &[&str]) -> Corpus {
        members_only(c, &set(members), is_member_scoped)
    }

    #[test]
    fn the_declaration_parses_floor_population_and_decisive_reading() {
        use super::super::sequence_addressed_pairs::{declared_members, ADDRESSED_PAIR_FLOOR};
        let d = declaration();
        assert_eq!(d.floor, 12);
        assert_eq!(d.floor, ADDRESSED_PAIR_FLOOR, "the floor is CR-131's, unrevised");
        assert_eq!(d.decisive, Reading::Strict);
        assert_eq!(d.population.len(), 83);
        assert_eq!(d.population, declared_members(), "the same 83 S-475 declared");
        assert!(!d.population.contains(FORK) && d.population.contains("e2e-tests"));
    }

    #[test]
    fn a_declaration_missing_doubling_or_miscounting_a_field_is_refused() {
        let ok = "FLOOR: 12\nPOPULATION: 2\nDECISIVE: STRICT\n\
                  MANIFEST MEMBERS — BEGIN (2)\n    a\n    b\nMANIFEST MEMBERS — END\n";
        let d = parse_declaration(ok).expect("parses");
        assert_eq!((d.floor, d.population, d.decisive), (12, set(&["a", "b"]), Reading::Strict));
        for (broken, why) in [
            (ok.replace("FLOOR: 12\n", ""), "no `FLOOR:`"),
            (ok.replace("FLOOR: 12", "FLOOR: 12\nFLOOR: 11"), "more than one"),
            (ok.replace("FLOOR: 12", "FLOOR: twelve"), "not a count"),
            (ok.replace("POPULATION: 2", "POPULATION: 3"), "holds 2 names"),
            (ok.replace("DECISIVE: STRICT", "DECISIVE: STRICTEST"), "names no reading"),
            // Indented, as a quoted line would be: not a declared field.
            (ok.replace("DECISIVE: STRICT", "    DECISIVE: STRICT"), "no `DECISIVE:`"),
        ] {
            let err = parse_declaration(&broken).expect_err(why);
            assert!(err.contains(why), "{err} — expected {why}");
        }
    }

    // ── Rule A: the three always-run cases ──────────────────────────────────

    #[test]
    fn a_non_member_owning_a_label_makes_no_collision() {
        let c = corpus(&[
            ("archive-api", Tier::Deploy, "archive-api"),
            (FORK, Tier::Deploy, "archive-api"),
        ]);
        // The near miss: the unscoped corpus is S-475's classifier, and there
        // the fork's chart name collides with the member's.
        assert_eq!(label_state(&c, "archive-api"), LabelState::Collision);
        let r = scoped(&c, &["archive-api"]);
        assert_eq!(label_state(&r, "archive-api"), LabelState::Resolves("archive-api"));
        assert!(collisions(&r).is_empty() && collisions(&c).contains_key("archive-api"));
    }

    #[test]
    fn two_members_owning_one_label_keep_their_collision() {
        let c = corpus(&[
            ("a", Tier::Deploy, "shared"),
            ("b", Tier::Deploy, "shared"),
            (FORK, Tier::Deploy, "shared"),
        ]);
        let r = scoped(&c, &["a", "b"]);
        assert_eq!(label_state(&r, "shared"), LabelState::Collision);
        assert_eq!(collisions(&r)["shared"], (Tier::Deploy, set(&["a", "b"])));
        let (outcome, provider) =
            classify_pair("c", label_state(&r, "shared"), &set(&["a", "b"]), &BTreeSet::new());
        assert_eq!((outcome, provider), (PairOutcome::SameTierCollision, None));
        // The near miss: one member and the non-member — no longer a collision.
        assert_eq!(label_state(&scoped(&c, &["a"]), "shared"), LabelState::Resolves("a"));
    }

    #[test]
    fn a_label_only_a_non_member_owns_resolves_to_no_member() {
        let c = corpus(&[(FORK, Tier::Deploy, "fork-only"), (FORK, Tier::Directory, FORK)]);
        assert_eq!(label_state(&c, "fork-only"), LabelState::Resolves(FORK));
        let r = scoped(&c, &["archive-api"]);
        for label in ["fork-only", FORK] {
            assert_eq!(label_state(&r, label), LabelState::Unclaimed, "{label}");
            let (outcome, _) =
                classify_pair("agg", label_state(&r, label), &set(&[FORK]), &BTreeSet::new());
            assert_eq!(outcome, PairOutcome::NoMemberLabel);
        }
    }

    #[test]
    fn every_tier_one_to_three_is_scoped_and_tiers_four_and_five_are_kept() {
        assert!(is_member_scoped(Tier::Deploy));
        assert!(is_member_scoped(Tier::Container));
        assert!(is_member_scoped(Tier::Directory));
        assert!(!is_member_scoped(Tier::Application));
        assert!(!is_member_scoped(Tier::Artifact));
        let c = corpus(&[
            (FORK, Tier::Container, "compose-name"),
            (FORK, Tier::Application, "spring-name"),
            (FORK, Tier::Artifact, "artifact-id"),
        ]);
        let r = scoped(&c, &["m"]);
        let kept: BTreeSet<Tier> = r.claims.iter().map(|c| c.tier).collect();
        assert_eq!(kept, [Tier::Application, Tier::Artifact].into_iter().collect());
        assert_eq!(label_state(&r, "compose-name"), LabelState::Unclaimed);
        assert_eq!(label_state(&r, "spring-name"), LabelState::Resolves(FORK));
        // The tier-4 sensitivity's registry drops it too.
        let all = members_only(&c, &set(&["m"]), Tier::is_decisive);
        assert_eq!(label_state(&all, "spring-name"), LabelState::Unclaimed);
    }

    #[test]
    fn the_registry_keeps_target_references_and_the_spring_census() {
        let mut c = corpus(&[(FORK, Tier::Deploy, "x")]);
        c.spring_by_flat.insert("ab".into(), set(&["a-b"]));
        let r = scoped(&c, &["m"]);
        assert_eq!(r.spring_by_flat, c.spring_by_flat);
        assert_eq!(r.members, c.members);
        assert!(r.claims.is_empty());
    }

    #[test]
    fn a_route_only_the_fork_duplicates_is_bound_by_path_without_it() {
        let route = |t: &str| -> BTreeMap<String, BTreeSet<String>> {
            [(t.to_string(), set(&["GET"]))].into_iter().collect()
        };
        let full = Providers {
            by_member: [
                ("archive-api".to_string(), route("/v1/docs/{}")),
                (FORK.to_string(), route("/v1/docs/{}")),
            ]
            .into_iter()
            .collect(),
            ..Providers::default()
        };
        assert_eq!(
            identity::classify(&full.serving("/v1/docs/{}"), "archive-api"),
            PairClass::NetNewAmbiguous,
            "the near miss: with the fork's duplicate the template is ambiguous",
        );
        let fork_free = members_only_providers(&full, &set(&["archive-api"]));
        assert_eq!(
            identity::classify(&fork_free.serving("/v1/docs/{}"), "archive-api"),
            PairClass::AlreadyBoundByPath,
        );
    }

    #[test]
    fn only_already_bound_pairs_enter_the_path_only_set() {
        let pair = |c: &str, p: &str, class| Pair {
            consumer: c.into(),
            provider: p.into(),
            overlay: "o".into(),
            template: "t".into(),
            normalized: "n".into(),
            via_key: "k".into(),
            label: p.into(),
            serving: 1,
            class,
            site: "s".into(),
        };
        let pairs = [
            pair("agg", "a", PairClass::AlreadyBoundByPath),
            pair("agg", "b", PairClass::NetNewAmbiguous),
            pair("agg", "c", PairClass::TargetServesNothing),
        ];
        assert_eq!(path_only_of(&pairs), [("agg".into(), "a".into())].into_iter().collect());
    }

    #[test]
    fn the_strict_reading_decides_against_the_parsed_floor_and_a_blind_run_is_void() {
        let floor = declaration().floor;
        assert_eq!(verdict(floor - 1, floor), "FALSIFIED");
        assert_eq!(verdict(floor, floor), "HOLDS");
        let void = headline(None, floor);
        assert!(void.starts_with("VOID"), "{void}");
        assert!(!void.contains("HOLDS") && !void.contains("FALSIFIED") && !void.contains(" 0 "));
        assert!(headline(Some(floor), floor).ends_with("HOLDS"));
        assert!(headline(Some(floor - 1), floor).ends_with("FALSIFIED"));
    }

    #[test]
    fn the_collision_census_agrees_with_the_classifier() {
        let c = corpus(&[
            ("a", Tier::Deploy, "x"),
            ("b", Tier::Deploy, "x"),
            // Collides at tier 2, but tier 1 decides first: not a collision.
            ("a", Tier::Deploy, "y"),
            ("b", Tier::Container, "y"),
            ("c", Tier::Container, "y"),
            ("a", Tier::Artifact, "z"),
            ("b", Tier::Artifact, "z"),
        ]);
        let census = collisions(&c);
        assert_eq!(census.keys().collect::<Vec<_>>(), ["x"]);
        for label in ["x", "y", "z", "a"] {
            assert_eq!(
                census.contains_key(label),
                label_state(&c, label) == LabelState::Collision,
                "{label}"
            );
        }
    }

    #[test]
    fn judge_all_rejudges_in_order_under_the_given_registry() {
        let c = corpus(&[("p", Tier::Deploy, "p"), (FORK, Tier::Deploy, "p")]);
        let t = |member: &str, label: &str| JudgedTarget {
            target: Target {
                member: member.into(),
                label: label.into(),
                form: super::super::config_declared_coupling::TargetForm::Url,
                source: super::super::config_declared_coupling::SourceSet::Deploy,
                overlay: "o".into(),
                via_key: "k".into(),
                file: "f".into(),
                value: "v".into(),
                port: None,
            },
            outcome: PairOutcome::SameTierCollision,
            provider: None,
        };
        let before = [t("q", "p"), t("p", "p"), t("q", "nobody")];
        let run = set(&["p", "q"]);
        let judged = judge_all(before.iter(), &scoped(&c, &["p", "q"]), &run, &BTreeSet::new());
        let got: Vec<_> = judged.iter().map(|j| (j.outcome, j.provider.as_deref())).collect();
        assert_eq!(
            got,
            [
                (PairOutcome::Addressed, Some("p")),
                (PairOutcome::SelfTie, Some("p")),
                (PairOutcome::NoMemberLabel, None),
            ]
        );
        let path_only = [("q".to_string(), "p".to_string())].into_iter().collect();
        let judged = judge_all(before.iter(), &scoped(&c, &["p", "q"]), &run, &path_only);
        assert_eq!(judged[0].outcome, PairOutcome::PathOnlyMatched);
    }
}
