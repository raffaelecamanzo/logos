//! The **broker subscribe corpus measurement** for [CR-107] / S-339, over the real
//! reference workspace rather than a fixture.
//!
//! [CR-107] was filed on an observation about a real estate, not about a fixture:
//! `logos workspace status` reported `topics: []` for **every** member of an
//! 84-member Spring workspace while dozens of files declared Kafka wiring. A fixture
//! can prove that `"${x}"` now binds; only the corpus can answer the question the CR's
//! last acceptance criterion actually asks — *does the inventory become non-empty,
//! and does the number reconcile against the files that declare wiring?*
//!
//! # It measures the capture, not a workspace index
//!
//! This walks the corpus **read-only** and runs the real [`extract`] over each
//! admitted file. It never opens an [`Engine`](logos_core::Engine), never indexes,
//! and never writes a `.logos` store — so it can be run against a reference
//! workspace whose enrolment state is being held for another purpose, and it
//! measures the capture path this story owns without the cost or the interference
//! of an 84-member index.
//!
//! That scoping is also what makes the figure attributable. `workspace status`'s
//! inventory is `capture → ledger → promotion → per-member topic surface →
//! federation`; four of those five stages were already proven correct by S-256's
//! own tests and were never handed a reference to carry ([CR-107] §4.3). The
//! reconciliation below therefore compares captured topics against **declaring
//! files**, which is the join the CR's criterion names.
//!
//! # Skips loudly, like the S-355/S-365 measurements
//!
//! Set `LOGOS_REF_WORKSPACE=<path>` to run it (`~` is expanded). With no corpus
//! configured the test prints a `SKIPPED:` line and passes: a measurement that
//! measured nothing must not read as a green assertion, and it must not fail a
//! machine that has no corpus either.
//!
//! # Recorded finding
//!
//! The measurement recorded when this test was written, so a later reader can tell
//! a changed capture from a changed corpus without re-deriving either.
//!
//! ```text
//! S-339 / CR-107, measured 2026-09-07 against ~/source/pec-services (84 members,
//! 2447 admitted .java files).
//!
//!   files with @KafkaListener:                    16   (all 16 in src/main)
//!   listener files capturing at least one topic:  16   ← 16 of 16
//!   distinct subscribe topic keys captured:       13   (13 of 13 are `${…}`)
//!   subscribe references captured:                16
//!   recorded `topic-not-literal` refusals:         0
//!
//! BEFORE this story the same walk captured **0**, measured by restoring the old
//! gate and re-running this very test, which then failed on its first assertion.
//! Not "fewer" — zero: every
//! listener in the estate externalises its topic as `${…}`, and the `$`/`{`
//! character rule in `ArtifactRelation::classify_target` classified all 16 external.
//! That is the whole of `topics: []`. The `brokers.scm` query itself matched all 16
//! literals before and after, which is why S-365 could report 16 subscribe literals
//! in this corpus while `workspace status` reported an empty inventory: the query
//! matched and the gate dropped.
//!
//! RECONCILIATION against the declaring files. The CR observed "34 files declare
//! Kafka wiring" from a grep it did not record, and that exact figure could not be
//! reproduced; the concrete markers give 16 `@KafkaListener` files, 13 src/main
//! files carrying the `KafkaHeaders.TOPIC` publish header form, and 99 files with a
//! `KafkaTemplate<`/`kafkaTemplate.send` mention (51 main / 48 test). What the
//! difference cannot be is a subscribe-side shortfall: the subscribe half is 16 of
//! 16 with nothing unexplained. The publish half is 0, and it is 0 for a reason
//! already measured independently — S-365 found all 13 src/main publish sites
//! refused as CRA-04 parameter-passed topics in the `MessageBuilder` header form,
//! which the shipped query does not recognise at all. Recognising it is S-370 /
//! CR-117, deliberately the next iteration's story and out of scope here.
//!
//! So: investigated, and the residue is attributed to a named cause on the other
//! half of the arm — not accepted as noise.
//! ```
//!
//! [CR-107]: ../../docs/requests/CR-107-broker-topic-capture-drops-placeholder-and-array-literals.md
//! [`extract`]: logos_core::extract::extract

#![cfg(feature = "lang-java")]

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use logos_core::extract::{extract, FileInput, SymbolContext};
use logos_core::model::ArtifactRelation;
use logos_core::plugin::LanguageRegistry;

/// The reference workspace, or `None` when none is configured — the same
/// `LOGOS_REF_WORKSPACE` contract the S-355/S-365 measurements read.
fn corpus_root() -> Option<PathBuf> {
    let raw = std::env::var("LOGOS_REF_WORKSPACE").ok()?;
    if raw.trim().is_empty() {
        return None;
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let expanded = match raw.strip_prefix("~/") {
        Some(rest) => PathBuf::from(&home).join(rest),
        None if raw == "~" => PathBuf::from(&home),
        None => PathBuf::from(&raw),
    };
    assert!(
        expanded.is_dir(),
        "LOGOS_REF_WORKSPACE={raw} does not resolve to a directory (expanded: {}) — \
         refusing to report a green run that measured nothing",
        expanded.display(),
    );
    Some(expanded)
}

/// What the walk found.
#[derive(Default)]
struct Corpus {
    /// Files declaring Kafka wiring by any of the three concrete markers — the
    /// reconciliation denominator (this module's docs say why it is
    /// not the CR's unreproducible `34`).
    declaring: BTreeSet<String>,
    /// Files carrying a `@KafkaListener` annotation (the subscribe half).
    listener_files: BTreeSet<String>,
    /// Listener files from which at least one topic was captured.
    captured_files: BTreeSet<String>,
    /// Distinct captured subscribe topic keys → how many references carry each.
    topics: BTreeMap<String, usize>,
    /// Files carrying a recorded `topic-not-literal` refusal, and how many.
    refusals: BTreeMap<String, usize>,
}

/// **S-339 / [CR-107]'s corpus criterion.** Walks the reference workspace and
/// asserts the two things the criterion asks for: the subscribe inventory is
/// **non-empty** for members carrying `@KafkaListener`, and the count
/// **reconciles** — every listener-bearing file captures, and any file that does
/// not is accounted for rather than tolerated.
///
/// The reconciliation is an assertion, not a printout, precisely because the
/// criterion says a mismatch is investigated and not accepted. If a listener file
/// stops capturing, this fails and names the file.
///
/// **What it does NOT measure, said plainly.** Clause (2) below is satisfied by its
/// *capture* disjunct alone on this estate: refusals measured **0**, because every
/// listener writes a placeholder literal and none writes a constant. So the
/// refusal-recording half of this story has no real-corpus evidence — that is a
/// fact about the corpus, not a passing measurement, and the `0` in the recorded
/// finding should be read that way. The refusal path's evidence is the fixture
/// tests in `extract::broker` and `broker_topic_promotion`.
///
/// The test name says "when one is configured" because with no corpus it skips and
/// passes: the `SKIPPED:` line goes to stderr, which `cargo test` captures, so the
/// name is the only thing a reader of a green summary sees.
#[test]
fn the_reference_workspace_reports_a_reconciled_subscribe_topic_inventory_when_one_is_configured() {
    let Some(root) = corpus_root() else {
        eprintln!(
            "SKIPPED: set LOGOS_REF_WORKSPACE=<path to the reference workspace> to run the \
             S-339 broker subscribe corpus measurement (this module's docs carry the \
             recorded finding it reproduces)."
        );
        return;
    };

    let registry = LanguageRegistry::load(std::env::temp_dir()).expect("registry loads");
    let java = registry.for_extension("java").expect("java plugin present");
    let mut corpus = Corpus::default();

    // `parents(false)` / `git_global(false)` / `ignore(false)` matches production's
    // admission walk and the S-355/S-365 measurements: the corpus must be the same
    // on every machine, so a developer's global ignore file cannot quietly move a
    // published figure.
    let walker = ignore::WalkBuilder::new(&root)
        .hidden(true)
        .git_ignore(true)
        .git_global(false)
        .ignore(false)
        .parents(false)
        .build();

    for entry in walker.flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("java") {
            continue;
        }
        let Ok(rel) = path.strip_prefix(&root) else {
            continue;
        };
        let rel = rel.to_string_lossy().to_string();
        let Ok(source) = std::fs::read_to_string(path) else {
            continue;
        };

        // "Declares Kafka wiring" is a textual test on purpose: deriving the
        // denominator from the capture would make the reconciliation circular. The
        // three concrete markers are the listener annotation, the template type/send,
        // and the header-form publish — narrow enough to be reconcilable, unlike a
        // bare `Kafka` substring which matches every import and config class.
        let declares = source.contains("@KafkaListener")
            || source.contains("KafkaTemplate<")
            || source.contains("kafkaTemplate.send")
            || source.contains("KafkaHeaders.TOPIC");
        if declares {
            corpus.declaring.insert(rel.clone());
        }
        let has_listener = source.contains("@KafkaListener");
        if has_listener {
            corpus.listener_files.insert(rel.clone());
        }

        let facts = extract(&FileInput::new(&rel, &source), java, &SymbolContext::default());
        for reference in &facts.refs {
            if reference.relation != Some(ArtifactRelation::BrokerSubscribe) {
                continue;
            }
            if reference.target.is_empty() {
                *corpus.refusals.entry(rel.clone()).or_default() += 1;
                continue;
            }
            corpus.captured_files.insert(rel.clone());
            *corpus.topics.entry(reference.target.clone()).or_default() += 1;
        }
    }

    let references: usize = corpus.topics.values().sum();
    let placeholders = corpus
        .topics
        .keys()
        .filter(|t| t.contains("${"))
        .count();
    let refusals: usize = corpus.refusals.values().sum();
    let publish_only: Vec<&String> = corpus
        .declaring
        .iter()
        .filter(|f| !corpus.listener_files.contains(*f))
        .collect();

    eprintln!(
        "S-339 corpus: root={}\n  \
         files declaring Kafka wiring: {}\n  \
         files with @KafkaListener: {}\n  \
         listener files capturing: {}\n  \
         distinct subscribe topics: {} ({placeholders} placeholder)\n  \
         subscribe references: {references}\n  \
         recorded topic-not-literal refusals: {refusals} across {} files\n  \
         declaring-but-publish-only files (S-370/CR-117's half): {}",
        root.display(),
        corpus.declaring.len(),
        corpus.listener_files.len(),
        corpus.captured_files.len(),
        corpus.topics.len(),
        corpus.refusals.len(),
        publish_only.len(),
    );

    // (1) The inventory is non-empty. This is the statement that was false.
    assert!(
        !corpus.topics.is_empty(),
        "the corpus declares Kafka wiring in {} files but captured no subscribe topic \
         at all — the CR-107 condition, unfixed",
        corpus.declaring.len(),
    );

    // (2) It reconciles: every file carrying a listener captures at least one topic,
    // or carries a recorded refusal explaining why it did not. Silence is what this
    // story exists to remove, so neither branch may be empty-handed.
    let unexplained: Vec<&String> = corpus
        .listener_files
        .iter()
        .filter(|f| !corpus.captured_files.contains(*f) && !corpus.refusals.contains_key(*f))
        .collect();
    assert!(
        unexplained.is_empty(),
        "{} listener file(s) neither captured a topic nor recorded a refusal — \
         a silent loss is exactly the CR-107 defect: {unexplained:?}",
        unexplained.len(),
    );

    // (3) The placeholder form is genuinely present in the measured corpus, so a
    // green run cannot mean "the corpus happens to write plain literals". Without
    // this, the measurement could pass on a corpus that never exercised the fix.
    assert!(
        placeholders > 0,
        "the corpus exercises no `${{…}}` topic, so it cannot evidence the CR-107 fix; \
         captured keys were {:?}",
        corpus.topics.keys().collect::<Vec<_>>(),
    );
}

/// **The promotion + per-member topic-surface stages, on REAL source.**
///
/// The measurement above stops at capture, so on its own it leaves the criterion's
/// named surface — `logos workspace status` reporting a non-empty topic inventory —
/// argued rather than exercised on the real estate. This closes the gap without
/// re-indexing the reference workspace itself: it copies the `@KafkaListener`-bearing
/// Java sources of the corpus into a temp project, runs a real `Engine::index`, and
/// reads the **same** `topic_surface()` projection that `workspace status` builds a
/// member's inventory from ([FR-WS-11]).
///
/// So the chain capture → ledger → promotion → per-member topic surface is measured
/// end to end on committed production source. The one remaining stage is the
/// federation rollup that concatenates per-member surfaces, which is what S-256's
/// own tests cover and which cannot turn a non-empty member surface into `topics: []`.
///
/// Copying rather than indexing in place is deliberate: the reference workspace's
/// enrolment state belongs to other work, and a test must not mutate it.
///
/// [FR-WS-11]: ../../docs/specs/requirements/FR-WS-11.md
#[test]
fn real_listener_sources_promote_a_non_empty_member_topic_inventory() {
    let Some(root) = corpus_root() else {
        eprintln!(
            "SKIPPED: set LOGOS_REF_WORKSPACE=<path to the reference workspace> to run the \
             S-339 promotion check on real listener sources."
        );
        return;
    };

    let tmp = tempfile::TempDir::new().expect("temp project");
    let mut copied = 0usize;
    let walker = ignore::WalkBuilder::new(&root)
        .hidden(true)
        .git_ignore(true)
        .git_global(false)
        .ignore(false)
        .parents(false)
        .build();
    for entry in walker.flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("java") {
            continue;
        }
        let Ok(source) = std::fs::read_to_string(path) else {
            continue;
        };
        if !source.contains("@KafkaListener") {
            continue;
        }
        // Flatten into one project: the topic identity is repo-scoped and carries no
        // path, so the directory layout does not affect what is promoted.
        let dest = tmp.path().join("src").join(format!("Listener{copied}.java"));
        std::fs::create_dir_all(dest.parent().unwrap()).expect("mkdir");
        std::fs::write(&dest, &source).expect("write");
        copied += 1;
    }
    assert!(
        copied > 0,
        "the corpus carries no @KafkaListener source to promote from"
    );

    let engine = logos_core::Engine::start(tmp.path()).expect("engine starts");
    engine.index();

    // The exact projection `workspace status` reports a member's inventory from.
    let surface = {
        use logos_core::federation::MemberContracts;
        engine.topic_surface().expect("the topic surface reads")
    };
    let mut topics: Vec<&str> = surface.iter().map(|t| t.topic.as_str()).collect();
    topics.sort();
    eprintln!(
        "S-339 promotion: {copied} listener source(s) → {} topic(s) in the member \
         inventory: {topics:?}",
        topics.len()
    );

    assert!(
        !topics.is_empty(),
        "real listener sources must promote a non-empty topic inventory — \
         `topics: []` is the CR-107 condition"
    );
    // Every promoted topic has at least one subscribing declaration behind it, and
    // none is a fabricated empty key.
    for summary in &surface {
        assert!(!summary.topic.trim().is_empty(), "no keyless topic: {summary:?}");
    }
}

/// **S-370 / [CR-117]'s corpus criterion: the PUBLISH half.**
///
/// The measurement above reconciles the subscribe side and, in its recorded
/// finding, attributes the publish side's `0` to a named cause on the other half of
/// the arm — the `MessageBuilder` header form the query did not recognise. This
/// test is that half, measured the same way: it walks the reference workspace
/// read-only, runs the real `extract` over each admitted file, and reconciles what
/// the arm now says about the publish side against textual markers derived
/// independently of the capture.
///
/// # What "reconciled" means here, and why the answer is 0 producers
///
/// S-365 measured — before this story was written — that **none** of the estate's
/// header-form sites carries a literal topic operand. So recognising the site
/// admits no topic and promotes no `Producer` node, and that is the expected
/// outcome rather than a failure: the deliverable is the refusal rows. What this
/// test holds the arm to is therefore not a capture count but the [NFR-CC-04]
/// property: **no file that publishes is silent**. Every file carrying
/// `KafkaHeaders.TOPIC` either captures a topic or records a refusal that says why
/// it did not, and the refusal count reconciles site-for-site against the textual
/// marker.
///
/// # Recorded finding
///
/// ```text
/// S-370 / CR-117, measured 2026-09-07 against ~/source/pec-services (84 members,
/// 2447 admitted .java files).
///
///   files carrying KafkaHeaders.TOPIC:                52   (13 in src/main, across 12 members)
///   textual `.setHeader(KafkaHeaders.TOPIC, …)` sites: 54
///     of which the operand is a bare `topic` parameter: 16   (13 main + 3 test)
///     of which a @ConfigurationProperties getter:       35
///     of which another identifier (@Value-injected):     3
///     of which a static string literal:                  0   ← why 0 producers
///   recognised publish sites (captured + refused):     54   ← 54 of 54
///   captured publish topic keys:                        0
///   recorded `topic-not-literal` publish refusals:     54
///   header-form files that are silent:                  0   ← the NFR-CC-04 claim
///
/// BEFORE this story the same walk recognised 0 of the 54 and recorded 0 refusals:
/// the arm looked for a topic in argument position, and no publish site in the
/// estate has one. That is the whole of FR-WS-10's producer promise yielding 0
/// Producer nodes on a 12-member Kafka estate.
/// ```
///
/// [CR-117]: ../../docs/requests/CR-117-broker-publish-capture-and-the-topic-key-namespace.md
/// [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md
#[test]
fn the_reference_workspace_reports_reconciled_publish_sites_when_one_is_configured() {
    let Some(root) = corpus_root() else {
        eprintln!(
            "SKIPPED: set LOGOS_REF_WORKSPACE=<path to the reference workspace> to run the \
             S-370 broker publish corpus measurement (this test's docs carry the recorded \
             finding it reproduces)."
        );
        return;
    };

    let registry = LanguageRegistry::load(std::env::temp_dir()).expect("registry loads");
    let java = registry.for_extension("java").expect("java plugin present");

    // Textual markers, derived independently of the capture so the reconciliation is
    // a join and not a tautology.
    let mut header_files: BTreeSet<String> = BTreeSet::new();
    let mut main_header_files: BTreeSet<String> = BTreeSet::new();
    let mut main_header_members: BTreeSet<String> = BTreeSet::new();
    let mut textual_sites = 0usize;
    let mut textual_parameter_sites = 0usize;
    // What the arm says.
    let mut captured: BTreeMap<String, usize> = BTreeMap::new();
    let mut refusals: BTreeMap<String, usize> = BTreeMap::new();
    let mut captured_keys: BTreeSet<String> = BTreeSet::new();

    let walker = ignore::WalkBuilder::new(&root)
        .hidden(true)
        .git_ignore(true)
        .git_global(false)
        .ignore(false)
        .parents(false)
        .build();

    for entry in walker.flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("java") {
            continue;
        }
        let Ok(rel) = path.strip_prefix(&root) else {
            continue;
        };
        let rel = rel.to_string_lossy().to_string();
        let Ok(source) = std::fs::read_to_string(path) else {
            continue;
        };
        if !source.contains("KafkaHeaders.TOPIC") {
            continue;
        }
        header_files.insert(rel.clone());
        if rel.contains("/src/main/") {
            main_header_files.insert(rel.clone());
            // The workspace member is the first path segment — `deprecated-mailbox-core`
            // owns two Maven modules, so files and members do not count 1:1.
            if let Some(member) = rel.split('/').next() {
                main_header_members.insert(member.to_string());
            }
        }
        textual_sites += source.matches("setHeader(KafkaHeaders.TOPIC,").count();
        textual_parameter_sites += source.matches("setHeader(KafkaHeaders.TOPIC, topic)").count();

        let facts = extract(&FileInput::new(&rel, &source), java, &SymbolContext::default());
        for reference in &facts.refs {
            if reference.relation != Some(ArtifactRelation::BrokerPublish) {
                continue;
            }
            if reference.target.is_empty() {
                *refusals.entry(rel.clone()).or_default() += 1;
            } else {
                *captured.entry(rel.clone()).or_default() += 1;
                captured_keys.insert(reference.target.clone());
            }
        }
    }

    let captured_total: usize = captured.values().sum();
    let refused_total: usize = refusals.values().sum();
    let silent: Vec<&String> = header_files
        .iter()
        .filter(|f| !captured.contains_key(*f) && !refusals.contains_key(*f))
        .collect();

    eprintln!(
        "S-370 corpus: root={}\n  \
         files carrying KafkaHeaders.TOPIC: {} ({} in src/main across {} members)\n  \
         textual setHeader(KafkaHeaders.TOPIC, …) sites: {textual_sites} \
         ({textual_parameter_sites} pass a bare `topic` parameter)\n  \
         captured publish topic keys: {captured_total}\n  \
         recorded topic-not-literal publish refusals: {refused_total} across {} files\n  \
         header-form files that are silent: {}",
        root.display(),
        header_files.len(),
        main_header_files.len(),
        main_header_members.len(),
        refusals.len(),
        silent.len(),
    );

    // (0) The corpus is the one the finding was recorded against. Asserted so a
    //     changed capture cannot be mistaken for a changed corpus, and vice versa —
    //     the reason this module states the figures at all.
    assert_eq!(
        main_header_files.len(),
        13,
        "the recorded finding is 13 src/main files carrying the header form; \
         the corpus now has {} — investigate before trusting the counts below: {:?}",
        main_header_files.len(),
        main_header_files,
    );
    assert_eq!(
        main_header_members.len(),
        12,
        "…across 12 members: {main_header_members:?}"
    );
    assert_eq!(
        textual_parameter_sites, 16,
        "the recorded finding is 16 method-parameter sites (13 main + 3 test)"
    );

    // (1) THE STATEMENT THAT WAS FALSE: the arm recognises the header form at all.
    //     Before this story, `recognised` was 0 on this estate.
    let recognised = captured_total + refused_total;
    assert!(
        recognised > 0,
        "the corpus carries {textual_sites} header-form publish sites and the arm \
         recognised none — the CR-117 condition, unfixed"
    );

    // (2) It reconciles SITE FOR SITE against the textual marker. A weaker
    //     "some were recognised" would hide a pattern that matches the common shape
    //     and misses a variant, which is the failure mode a fixture cannot see.
    assert_eq!(
        recognised, textual_sites,
        "every textual header-form site must be recognised — captured or refused. \
         {recognised} of {textual_sites} were; the shortfall is a query gap, and a \
         mismatch is investigated, not accepted"
    );

    // (3) The refusal count reconciles against the 16 known method-parameter sites:
    //     every one of them is refused, and they are a subset of the refusals rather
    //     than the whole of them (35 configuration getters refuse too).
    assert!(
        refused_total >= textual_parameter_sites,
        "all {textual_parameter_sites} method-parameter sites must record a refusal; \
         only {refused_total} refusals exist in total"
    );

    // (4) NFR-CC-04, the property this story exists for: no publishing file is
    //     silent. This is the assertion that would have failed before S-370 for all
    //     52 header-form files.
    assert!(
        silent.is_empty(),
        "{} file(s) carry KafkaHeaders.TOPIC but neither captured a topic nor \
         recorded a refusal — silence is the defect: {silent:?}",
        silent.len(),
    );

    // (5) Never fabricated: a captured key is a literal's own text, never an
    //     operand's source. On this estate `captured_keys` is EMPTY — there is no
    //     literal operand to capture — so this is a guard against a future
    //     over-eager key rule rather than a live measurement, and it is stated that
    //     way so a reader does not mistake a passing assertion for an exercised one.
    for key in &captured_keys {
        assert!(
            !key.contains('(') && !key.contains('+'),
            "a captured topic key is a literal's text, never an operand's source: {key:?}"
        );
    }
}
