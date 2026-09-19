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
//! # Skips when unconfigured, and says so — but only under `--nocapture`
//!
//! Set `LOGOS_REF_WORKSPACE=<path>` to run these (`~` is expanded). With no corpus
//! configured each test prints a `SKIPPED:` line and passes: it must not fail a
//! machine that has no corpus either. Be exact about the limit of that, because the
//! mechanism is weaker than it reads — libtest captures `eprintln!` for a test that
//! PASSES, so a default `cargo test` run shows only `... ok` and the `SKIPPED:` line
//! is invisible. A green summary here therefore does **not** establish that the
//! measurement ran: look for the printed figures, or run with `--nocapture`. Each
//! test name carries `when_one_is_configured` for exactly that reason.
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
//! ## Appendix, 2026-09-15 (S-408): the subscribe refusal figure has moved 0 → 8
//!
//! The finding above stays as recorded — it was correct for the capture that
//! existed on 2026-09-07 and its `0` is not edited. What changed is the arm, not
//! the corpus.
//!
//! S-408 added the Kafka Streams topology form, whose `stream(<operand>)`
//! subscribe carries a refusal slot. The estate's 8 topology files each write one
//! `stream(…)` with a `@ConfigurationProperties` accessor or a qualified constant
//! operand, so the S-339 walk — whose file filter is the broad "declares Kafka
//! wiring" marker, not `@KafkaListener` — now counts **8** recorded
//! `topic-not-literal` subscribe refusals where it counted 0.
//!
//! Read the two figures this way:
//!
//!   - The `0` is still the right figure for the population the finding is ABOUT:
//!     the 16 `@KafkaListener` files, every one of which writes a placeholder
//!     literal and none of which writes a constant. That is unchanged and is
//!     re-asserted by the same test.
//!   - The `8` is a different population — topology sites, a form that did not
//!     exist in the arm when the finding was written — and its own denominator is
//!     16 of 16 in
//!     `the_reference_workspace_leaves_no_streams_topology_site_silent_when_one_is_configured`.
//!
//! The consequence for the sentence "the refusal-recording half of this story has
//! no real-corpus evidence" is that it is now false for the ARM and still true for
//! S-339's own population. The arm's refusal path gained real-corpus evidence
//! twice since: 54 header-form refusals (S-370) and these 16 topology ones.
//!
//! [CR-107]: ../../docs/requests/CR-107-broker-topic-capture-drops-placeholder-and-array-literals.md
//! [`extract`]: logos_core::extract::extract

#![cfg(feature = "lang-java")]

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use logos_core::extract::config::corpus::{canonical_key, source_facts};
use logos_core::extract::broker::{resolve_forwarded_topics, ForwardingOutcome};
use logos_core::extract::config::binding::PropertiesIndex;
use logos_core::extract::{extract, extract_files, FileInput, SymbolContext};
use logos_core::resolve::broker_identity::{topic_identity, TopicIdentity};
use logos_core::graph_store::ConfigDefinition;
use logos_core::model::ArtifactRelation;
use logos_core::plugin::LanguageRegistry;
use logos_core::resolve::binding::placeholder_keys;
use tempfile::TempDir;

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

/// The corpus walk, with the five flags that make a published figure reproducible.
///
/// `parents(false)` / `git_global(false)` / `ignore(false)` matches production's
/// admission walk and the S-355/S-365 measurements: the corpus must be the same on
/// every machine, so a developer's global ignore file cannot quietly move a
/// published figure.
///
/// This exists as one function because the flags were written out at each of the
/// three walk sites and the comment explaining them sat only on the first — which
/// is how one copy gets "tidied up" and a published figure silently moves.
fn corpus_walker(root: &std::path::Path) -> ignore::Walk {
    ignore::WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .git_global(false)
        .ignore(false)
        .parents(false)
        .build()
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

    let walker = corpus_walker(&root);

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
    let walker = corpus_walker(&root);
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
///     of which in src/main:                            13
///     of which in test sources:                        41   ← see the note below
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
///
/// MOST OF THE 54 IS TEST CODE — 41 of the sites, 76%. Stated here because the
/// headline "54 refusals" is otherwise easy to read as 54 production publishers.
/// There is no test-path exclusion in admission, so all 41 are indexed, all record
/// `topic-not-literal`, and all land in the FR-WS-05 coverage denominator; a JUnit
/// fixture that built a message with a literal topic would likewise promote a real
/// Producer node. Whether the coverage tier SHOULD exclude test sources is a policy
/// question this story does not own and does not decide — it is recorded so the
/// current answer is visible rather than inferable only from the file counts.
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
    // The src/main vs test split of the SITES (not the files). The headline refusal
    // count is mostly test code and must say so — see the recorded finding.
    let mut main_sites = 0usize;
    // What the arm says.
    let mut captured: BTreeMap<String, usize> = BTreeMap::new();
    let mut refusals: BTreeMap<String, usize> = BTreeMap::new();
    let mut captured_keys: BTreeSet<String> = BTreeSet::new();

    let walker = corpus_walker(&root);

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
        // BOTH setter spellings the query's predicate admits. Counting only
        // `setHeader(` would drift from what the arm recognises the day the estate
        // gains a `setHeaderIfAbsent` site — and would fail the equality below with
        // the *opposite* diagnosis ("a query gap"). The two substrings cannot
        // overlap, so summing them double-counts nothing.
        let sites_here = source.matches("setHeader(KafkaHeaders.TOPIC,").count()
            + source.matches("setHeaderIfAbsent(KafkaHeaders.TOPIC,").count();
        textual_sites += sites_here;
        if rel.contains("/src/main/") {
            main_sites += sites_here;
        }
        textual_parameter_sites += source.matches("setHeader(KafkaHeaders.TOPIC, topic)").count()
            + source
                .matches("setHeaderIfAbsent(KafkaHeaders.TOPIC, topic)")
                .count();

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
         of those sites, {main_sites} are in src/main and {} are test sources\n  \
         captured publish topic keys: {captured_total}\n  \
         recorded topic-not-literal publish refusals: {refused_total} across {} files\n  \
         header-form files that are silent: {}",
        root.display(),
        header_files.len(),
        main_header_files.len(),
        main_header_members.len(),
        textual_sites - main_sites,
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
    // ZERO captured keys is the recorded finding, not a regression: S-365 measured
    // that no header-form site in this estate carries a literal topic operand, so
    // recognition admits nothing here and the arm's honest output is refusals.
    // Asserted because it is what makes (4) below entail "each of the 16 refused",
    // and because if this ever becomes non-zero the recorded finding — and S-371's
    // whole premise — needs re-deriving rather than quietly absorbing the change.
    assert_eq!(
        captured_total, 0,
        "S-365 measured 0 literal topic operands across all {textual_sites} \
         header-form sites; {captured_total} captured now. That is not a failure — \
         it may be a better key rule or a changed corpus — but it invalidates this \
         module's recorded finding, so re-derive it rather than updating this number"
    );

    // (1) THE STATEMENT THAT WAS FALSE: the arm recognises the header form at all.
    //     Before this story, `recognised` was 0 on this estate.
    let recognised = captured_total + refused_total;
    assert!(
        recognised > 0,
        "the corpus carries {textual_sites} header-form publish sites and the arm \
         recognised none — the CR-117 condition, unfixed"
    );

    // (2) FILE-GRAIN reconciliation — the one the ledger cannot collapse. Every file
    //     carrying the header form yields at least one row, so this equality is
    //     immune to the dedup described in (3) and is the assertion to trust first.
    assert_eq!(
        refusals.len() + captured.len(),
        header_files.len(),
        "every file carrying KafkaHeaders.TOPIC must yield at least one row; \
         {} of {} did",
        refusals.len() + captured.len(),
        header_files.len(),
    );

    // (3) SITE-FOR-SITE reconciliation. Sharper than (2) — it would catch a pattern
    //     that matches the common shape and misses a variant, which no fixture can
    //     see — but the two sides are counted at DIFFERENT grains, so its failure
    //     message must name both causes it can have.
    //
    //     `textual_sites` counts call sites. `recognised` counts rows from
    //     `extract()`, which has already run `dedup_sort_refs` — keyed on
    //     `(source, target, form, kind, relation)`, ignoring `line`. Every refusal
    //     shares `target == ""`, so two refused header-form sites in ONE enclosing
    //     declaration reach the ledger as ONE row (documented at
    //     `extract::config::refs::record_refusals`, the shared recorder S-374
    //     promoted out of `extract::broker`, and asserted directly by
    //     `refusals_are_attributed_per_declaration_not_per_line`). The equality
    //     therefore holds only while no method carries two header-form publishes —
    //     true of this estate (its two multi-site files put them in separate test
    //     methods), but a property of the corpus, not of the arm.
    assert_eq!(
        recognised, textual_sites,
        "site-for-site reconciliation failed: {recognised} of {textual_sites}. \
         TWO possible causes, and they need opposite fixes — (a) a query gap: the \
         arm does not recognise a variant the marker counted; or (b) declaration- \
         level dedup: two header-form sites now share one enclosing method, so the \
         ledger legitimately collapses them and the MARKER is what needs \
         re-graining. Check (b) first — assertion (2) above stays green under (b) \
         and fails under (a). A mismatch is investigated, not accepted"
    );

    // (4) The refusal count reconciles against the 16 known method-parameter sites:
    //     every one of them is refused, and they are a subset of the refusals rather
    //     than the whole of them (35 configuration getters refuse too). Paired with
    //     the `captured_total == 0` assertion in (0), the two together do entail
    //     "each of the 16 refused" — without it, a future change that captured 16
    //     and lost 16 refusals would satisfy this inequality.
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

/// **S-408 / [CR-131] §3.2 A1's corpus criterion.** Walks the reference workspace
/// and asserts the one thing the criterion asks for: **no Kafka Streams topology
/// site under `src/main` is silent** — each produces either a bound topic or a
/// recorded `topic-not-literal` refusal ([NFR-CC-04]).
///
/// The figures are reported with their denominator and dated. **No floor is
/// asserted on them**, deliberately and per the criterion: a count of what a
/// capture admits on one estate is a measurement, not a product prediction, and
/// [S-397]'s lesson (a census figure promoted to an acceptance floor, then missed
/// by the product at 44 against a harness's 79) is exactly what an assertion of
/// `>= 14` here would repeat. What IS asserted is the invariant — `silent == 0` —
/// plus the corpus identity, so a changed capture cannot be mistaken for a
/// changed corpus.
///
/// # Recorded finding
///
/// ```text
/// S-408 / CR-131 §3.2 A1, measured 2026-09-15 against ~/source/pec-services
/// (84 members).
///
///   files declaring a Kafka Streams topology:            8   (all 8 in src/main,
///                                                            across 8 members)
///   textual `.stream(` / `.to(` sites in them:          16   (8 + 8)
///   recognised topology sites (captured + refused):     16   ← 16 of 16
///   captured topology topic keys:                        0
///   recorded `topic-not-literal` topology refusals:     16
///   topology files that are silent:                      0   ← the NFR-CC-04 claim
///
/// BEFORE this story the same walk recognised 0 of the 16 and recorded 0
/// refusals: the arm matched annotations and the `KafkaHeaders.TOPIC` header and
/// nothing else, so the estate's entire asynchronous topology mass was ABSENT —
/// not refused. Reproduce it by deleting the four topology patterns from
/// `plugins/java/queries/brokers.scm` and re-running; the `silent` assertion
/// below then fails naming all 8 files.
///
/// CAPTURED IS 0, AND THAT IS THE EXPECTED OUTCOME OF THIS STORY, not a defect.
/// All 16 operands are non-literal — 14 are a `@ConfigurationProperties` accessor
/// (`kafkaTopics.getArchiveEvents()`), 2 are a qualified constant
/// (`Topics.INPUT_TOPIC`) — so recognition alone binds no topic and promotes no
/// Producer/Consumer. S-408 delivers the 16 honest refusals; resolving the
/// accessor operand is S-409, which threads into this arm the same BindingView
/// the HTTP arm already resolves 81 of 96 sites with.
///
/// MEMBER COUNT: 8 here against CR-131 §2.1's 14 sites over 7 members. The
/// difference is `punctuators-poc`, a proof-of-concept module the CR's census did
/// not count; it writes the 2 remaining sites and is the only member using the
/// `KStream`-typed-binding shape rather than one chained expression. Both
/// spellings are captured, which is why the total is 16 and not 14 — stated so
/// the two figures are reconcilable rather than read as a discrepancy.
///
/// RUST IS UNEXERCISED; GO IS EXERCISED, AND FOR THE OTHER HALF.
///
///   rust: 0 .rs files                                → UNEXERCISED
///   go:   262 .go files (1 member, hermodr-mirror)
///         textual `Stream(`/`To(`/`stream(`/`to(`:  0
///         broker rows the Go arm produced:          0   ← 0 of 262, no false positive
///
/// The Rust arm is measured by nothing here and its evidence is fixture-only. The
/// Go arm is NOT: the estate carries a 262-file Go member, which is a real corpus
/// for the half that a fixture cannot answer — whether patterns keyed on verbs as
/// common as `To` and `Stream` over-capture on ordinary code. Receiver-gated, they
/// produce 0 rows over all 262 files. That is the measurement the Rust
/// `brokers.scm` header says it lacks for its own bare-verb patterns, and it is
/// why the gate, not the verb, is what makes this arm shippable.
///
/// It is NOT evidence that the Go arm CAPTURES correctly — the corpus writes no
/// topology to capture, so that half stays fixture-only
/// (`extract::broker::go_capture_tests::the_go_topology_form_is_receiver_gated`,
/// and `…::rust_capture_tests::the_rust_topology_form_is_receiver_gated` for Rust).
///
/// This paragraph is the second thing this measurement corrected in its own first
/// run: it was written asserting `(rust, go) == (0, 0)` files, and the estate
/// answered 262 Go files. The claim "unexercised" was assumed, not measured — the
/// assertion is what caught it, which is the whole reason it is an assertion.
/// ```
///
/// # WHERE ELSE THESE FIGURES ARE WRITTEN DOWN — sweep this list when they move
///
/// The assertions below are the ONLY place the estate figures are checked. They
/// are also restated as prose in five other files, where nothing forces them to
/// move together, so a changed estate fails here and leaves those silently stale.
/// When this test fails on a figure, edit every line in this list before calling
/// it done:
///
/// - `logos-core/src/extract/broker.rs` — `TOPOLOGY_RECEIVER_TYPES` rustdoc (the
///   cross-language collision argument rests on `0 .rs` / `262 .go`), and the
///   `rust_capture_tests` / `go_capture_tests` topology-fixture doc comments.
/// - `logos-core/plugins/go/queries/brokers.scm` — the "what the estate does and
///   does not measure here" header section.
/// - `logos-core/plugins/go/plugin.toml` — the `capabilities` comment.
/// - `logos-core/plugins/rust/queries/brokers.scm` — the "no estate evidence"
///   header section.
/// - `logos-core/src/plugin/grammars.rs` — the Go `brokers.scm` embedded-query
///   comment.
///
/// The list is here rather than the numbers being deleted from those files on
/// purpose: each site needs its figure to make its own local argument (a header
/// that said "see the corpus test" would not let a reader judge the claim it is
/// making). What that costs is this sweep, and naming the cost is cheaper than
/// paying it by discovery — this repository has been bitten twice by a stale
/// hardcoded number whose twin was missed.
///
/// [CR-131]: ../../docs/requests/CR-131-cross-service-coupling-from-committed-configuration.md
/// [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md
#[test]
fn the_reference_workspace_leaves_no_streams_topology_site_silent_when_one_is_configured() {
    let Some(root) = corpus_root() else {
        eprintln!(
            "SKIPPED: set LOGOS_REF_WORKSPACE=<path to the reference workspace> to run the \
             S-408 Kafka Streams topology corpus measurement (this test's docs carry the \
             recorded finding it reproduces)."
        );
        return;
    };

    let registry = LanguageRegistry::load(std::env::temp_dir()).expect("registry loads");
    let java = registry.for_extension("java").expect("java plugin present");
    let go = registry.for_extension("go").expect("go plugin present");

    // Textual markers, derived independently of the capture so the reconciliation
    // is a join and not a tautology.
    let mut topology_files: BTreeSet<String> = BTreeSet::new();
    let mut main_topology_files: BTreeSet<String> = BTreeSet::new();
    let mut main_topology_members: BTreeSet<String> = BTreeSet::new();
    let mut textual_sites = 0usize;
    let mut main_sites = 0usize;
    // The two sibling-language arms this story also ships. Counted, not assumed —
    // and counting them is what caught the claim that both were unexercised: Rust
    // is (0 `.rs` files), Go is NOT (262 `.go` files, one member). The Go arm is
    // therefore run over every Go file and its broker rows counted, which turns an
    // untestable "unexercised" into a real no-false-positive measurement.
    let mut rust_files = 0usize;
    let mut go_files = 0usize;
    let mut go_textual_sites = 0usize;
    let mut go_broker_rows = 0usize;
    // What the arm says.
    let mut captured: BTreeMap<String, usize> = BTreeMap::new();
    let mut refusals: BTreeMap<String, usize> = BTreeMap::new();
    let mut captured_keys: BTreeSet<String> = BTreeSet::new();

    for entry in corpus_walker(&root).flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        let extension = path.extension().and_then(|e| e.to_str());
        if !matches!(extension, Some("rs") | Some("go") | Some("java")) {
            continue;
        }
        let Ok(rel) = path.strip_prefix(&root) else {
            continue;
        };
        let rel = rel.to_string_lossy().to_string();
        let Ok(source) = std::fs::read_to_string(path) else {
            continue;
        };
        if extension == Some("rs") {
            rust_files += 1;
            continue;
        }
        if extension == Some("go") {
            go_files += 1;
            go_textual_sites += source.matches(".Stream(").count()
                + source.matches(".To(").count()
                + source.matches(".stream(").count()
                + source.matches(".to(").count();
            let facts = extract(&FileInput::new(&rel, &source), go, &SymbolContext::default());
            go_broker_rows += facts
                .refs
                .iter()
                .filter(|r| {
                    matches!(
                        r.relation,
                        Some(ArtifactRelation::BrokerPublish)
                            | Some(ArtifactRelation::BrokerSubscribe)
                    )
                })
                .count();
            continue;
        }
        // A topology file names `StreamsBuilder` AND writes at least one link. The
        // type name alone admits the `KafkaConfiguration` beans that configure the
        // factory without declaring any topology — 6 such files on this estate, and
        // counting them would put files with 0 sites into the `silent` denominator
        // and fail this test for the wrong reason.
        let sites_here = source.matches(".stream(").count() + source.matches(".to(").count();
        if !source.contains("StreamsBuilder") || sites_here == 0 {
            continue;
        }
        topology_files.insert(rel.clone());
        textual_sites += sites_here;
        if rel.contains("/src/main/") {
            main_topology_files.insert(rel.clone());
            main_sites += sites_here;
            if let Some(member) = rel.split('/').next() {
                main_topology_members.insert(member.to_string());
            }
        }

        let facts = extract(&FileInput::new(&rel, &source), java, &SymbolContext::default());
        for reference in &facts.refs {
            if !matches!(
                reference.relation,
                Some(ArtifactRelation::BrokerPublish) | Some(ArtifactRelation::BrokerSubscribe)
            ) {
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
    let silent: Vec<&String> = topology_files
        .iter()
        .filter(|f| !captured.contains_key(*f) && !refusals.contains_key(*f))
        .collect();

    eprintln!(
        "S-408 corpus: root={}\n  \
         files declaring a Kafka Streams topology: {} ({} in src/main across {} members)\n  \
         textual `.stream(`/`.to(` sites in them: {textual_sites} ({main_sites} in src/main)\n  \
         captured topology topic keys: {captured_total}\n  \
         recorded topic-not-literal topology refusals: {refused_total} across {} files\n  \
         topology files that are silent: {}\n  \
         sibling-language arms: rust {rust_files} .rs files (0 = UNEXERCISED, never \
         zero captured); go {go_files} .go files writing {go_textual_sites} textual \
         `Stream(`/`To(` site(s) → {go_broker_rows} broker row(s)",
        root.display(),
        topology_files.len(),
        main_topology_files.len(),
        main_topology_members.len(),
        refusals.len(),
        silent.len(),
    );

    // (0) The corpus is the one the finding was recorded against. Asserted so a
    //     changed capture cannot be mistaken for a changed corpus, and vice versa.
    assert!(
        !topology_files.is_empty(),
        "the reference workspace at {} declares no Kafka Streams topology — this is \
         not the corpus the recorded finding was measured against, so a green run \
         here would report a measurement that did not happen",
        root.display(),
    );

    // (1) THE CRITERION: no topology site is silent. Every file that writes a
    //     link either bound a topic or recorded a refusal ([NFR-CC-04]). This is
    //     the assertion; the counts above are reported, never floored.
    assert!(
        silent.is_empty(),
        "{} file(s) declare a Kafka Streams topology but neither captured a topic \
         nor recorded a refusal — silence is the defect: {silent:?}",
        silent.len(),
    );

    // (2) Site-grained, not just file-grained: the arm recognises as many sites as
    //     the text writes. A file-grained check passes while half a file's links go
    //     missing, which is precisely the shape of the gap this story closes.
    assert_eq!(
        captured_total + refused_total,
        textual_sites,
        "the arm recognised {} of {textual_sites} textual topology sites — \
         captured {captured_total}, refused {refused_total}",
        captured_total + refused_total,
    );

    // (3) Never fabricated: a captured key is a literal's own text, never an
    //     operand's source. `captured_keys` is EMPTY on this estate (all 16
    //     operands are accessors or qualified constants), so this guards a future
    //     over-eager key rule rather than measuring a live population — stated so a
    //     reader does not mistake a passing assertion for an exercised one.
    for key in &captured_keys {
        assert!(
            !key.contains('(') && !key.contains('+'),
            "a captured topology topic key is a literal's text, never an operand's \
             source: {key:?}"
        );
    }

    // (4a) RUST: the arm is unexercised, and that is asserted rather than assumed.
    //      The day the corpus gains a `.rs` file, the word "unexercised" in this
    //      module's recorded finding and in `plugins/rust/queries/brokers.scm`
    //      stops being true and must be re-measured rather than carried forward.
    assert_eq!(
        rust_files, 0,
        "the recorded finding states the Rust topology arm is UNEXERCISED on this \
         corpus; it now contains {rust_files} .rs file(s), so that word is stale"
    );

    // (4b) GO: the arm IS exercised — 262 files, one member — and what it measures
    //      is the absence of FALSE POSITIVES. This assertion is the reason the Go
    //      patterns can key on verbs as common as `To`/`Stream` at all: over a real
    //      262-file Go corpus that writes no Kafka Streams topology, the
    //      receiver-gated arm produces zero broker rows. Without the gate the same
    //      corpus is a manufactured coverage denominator.
    //
    //      It is NOT evidence that the Go arm captures correctly — the corpus
    //      writes no topology to capture. That half is fixture-only, and the
    //      recorded finding says so.
    assert!(
        go_files > 0,
        "the recorded finding measures the Go arm's false-positive rate over {go_files} \
         Go files; with none, that figure is unexercised and must not be reported as \
         a measured zero"
    );
    assert_eq!(
        go_broker_rows, 0,
        "the receiver-gated Go arm produced {go_broker_rows} broker row(s) over \
         {go_files} Go files that write no Kafka Streams topology ({go_textual_sites} \
         textual `Stream(`/`To(` site(s)) — every one is a false positive"
    );
}

/// One member's broker-arm reading, before and after the accessor hop.
#[derive(Default, Clone, Copy)]
struct MemberBrokerReading {
    /// Rows whose target is a key — a literal topic, or (after) a resolved
    /// accessor's canonical `${prefix.key}` placeholder.
    resolved: usize,
    /// Keyless rows — the arm's recorded `topic-not-literal` refusals.
    refused: usize,
}

impl MemberBrokerReading {
    /// The denominator every figure below is reported against: the broker rows
    /// this member's files produce at all.
    fn rows(self) -> usize {
        self.resolved + self.refused
    }
}

/// **S-409 / [CR-131] §3.2 A2's corpus criterion.** Walks the reference
/// workspace member by member and reports, **per member with its denominator**,
/// how many broker sites resolve their topic operand to a key before and after
/// the accessor hop reaches this arm ([FR-WS-19]).
///
/// # What "before" and "after" are, precisely
///
/// Both are readings of **this** build; neither re-runs an old binary. They
/// differ by exactly the one input S-409 threads in — the member's
/// `PropertiesIndex`:
///
/// - **before** — [`extract`], the single-file driver, whose index is empty.
///   That is a faithful model of the pre-S-409 binary *for broker rows*, and the
///   reason is structural rather than an approximation:
///   `capture_broker_invocation_arm` took no `properties` argument at all, so a
///   member-scoped index and an empty one produced byte-identical broker output.
/// - **after** — [`extract_files`] over the member's whole Java file set, which
///   is what the pipeline actually calls and what builds the index a
///   `@ConfigurationProperties` accessor needs (its owning class is never in the
///   file that reads it).
///
/// # No floor is asserted on any figure here
///
/// Deliberately, and for the reason [S-397] recorded the hard way: a count of
/// what a capture admits on one estate is a measurement, not a prediction about
/// the product, and a `>= N` here is how a census figure becomes an acceptance
/// floor the product then misses. What IS asserted is the invariant — **no
/// member that recorded broker rows before records none after** — plus the
/// corpus identity, so a changed capture cannot be mistaken for a changed
/// corpus.
///
/// # Recorded finding
///
/// ```text
/// S-409 / CR-131 §3.2 A2, measured 2026-09-15 against ~/source/pec-services
/// (87 top-level entries, 84 enrolled members).
///
///   members producing any broker row:        23
///   broker rows (the denominator):           86   ← unchanged by the hop
///   rows whose operand resolved, before:     16   of 86
///   rows whose operand resolved, after:      59   of 86
///
/// The 43-row move is the accessor hop and nothing else: the two readings are
/// the same walk of the same bytes on the same build, differing only in whether
/// the member's `PropertiesIndex` exists. The 27 rows that still refuse are the
/// operands the chain cannot prove — a qualified constant (`Topics.INPUT_TOPIC`),
/// a method parameter (S-417's two-frame wrapper hop, out of scope), and the rest
/// of FR-WS-19's nine named faults.
///
/// The per-member breakdown is printed by the run itself (stderr, one line per
/// member: `MEMBER  before-resolved/rows -> after-resolved/rows`) and reproduced
/// in the sprint implementation notes. It is deliberately NOT transcribed here:
/// 23 figures copied into prose beside the assertion that prints them is exactly
/// the stale-twin failure this module's S-408 sibling keeps an explicit sweep
/// list for.
///
/// WHAT THIS MEASUREMENT IS, AND WHAT IT IS NOT COMPARABLE WITH. This test walks
/// the estate's **source** through `extract_files`. The estate's coverage
/// harnesses — `coverage_headline_baseline`, `coverage_intake_split`,
/// `config_bound_admission` — instead open the ENROLLED `.logos` stores through
/// `federation::discover` and index nothing, so they report whatever generation
/// the stores were last written at, which is a different question from this one
/// and moves for reasons that have nothing to do with the code under test.
///
/// That distinction is not theoretical. On 2026-09-15 those three harnesses were
/// run on this commit and on its merge base and produced **byte-identical**
/// figures (the HTTP arm's `config-bound` count read 84 in both), which is this
/// story's AC4 evidence. Re-run a few hours later they read 90 — because a
/// CONCURRENT sibling dev session was re-enrolling the shared reference
/// workspace, 8 of the 84 stores rewritten mid-run. Nothing about this branch
/// changed between the two readings.
///
/// The lesson for whoever reads a figure out of those three next: **the reference
/// estate is a shared mutable resource, and a store-backed figure is only
/// comparable against another figure taken on the same store generation.** Take
/// both arms of any comparison back to back, and check whether anything else is
/// enrolling before believing a delta. A source-backed walk like this one has no
/// such hazard, which is the reason this measurement is written as one.
/// ```
///
/// [CR-131]: ../../docs/requests/CR-131-cross-service-coupling-from-committed-configuration.md
/// [FR-WS-19]: ../../docs/specs/requirements/FR-WS-19.md
/// [S-397]: ../../docs/planning/journal.md#s-397-the-accessor-capture-hop-reaches-the-invocation-arm
#[test]
fn the_reference_workspace_reports_its_resolved_broker_sites_before_and_after_the_hop() {
    let Some(root) = corpus_root() else {
        eprintln!(
            "SKIPPED: set LOGOS_REF_WORKSPACE=<path to the reference workspace> to run the \
             S-409 broker accessor-hop corpus measurement (this test's docs carry the \
             recorded finding it reproduces)."
        );
        return;
    };

    let registry = LanguageRegistry::load(std::env::temp_dir()).expect("registry loads");
    let java = registry.for_extension("java").expect("java plugin present");
    let ctx = SymbolContext::default();

    // Group every admitted Java file by its member — the first path segment, which
    // is one clone of the estate. The index is member-scoped because that is the
    // scope the production pass builds it at.
    let mut by_member: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for entry in corpus_walker(&root).flatten() {
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
        let Some(member) = rel.split('/').next().map(str::to_string) else {
            continue;
        };
        let Ok(source) = std::fs::read_to_string(path) else {
            continue;
        };
        by_member.entry(member).or_default().push((rel, source));
    }

    let is_broker = |relation: Option<ArtifactRelation>| {
        matches!(
            relation,
            Some(ArtifactRelation::BrokerPublish) | Some(ArtifactRelation::BrokerSubscribe)
        )
    };

    let mut before: BTreeMap<String, MemberBrokerReading> = BTreeMap::new();
    let mut after: BTreeMap<String, MemberBrokerReading> = BTreeMap::new();
    // The keyed targets each member publishes and subscribes, AFTER the hop — the
    // join the bridge itself makes (`PortableKey::broker`, byte equality over the
    // stored target). Collected so the agreement figure below is read off the same
    // rows the ledger carries, not off a second derivation of them.
    let mut published: BTreeSet<(String, String)> = BTreeSet::new();
    let mut subscribed: BTreeSet<(String, String)> = BTreeSet::new();

    for (member, files) in &by_member {
        let inputs: Vec<FileInput> = files
            .iter()
            .map(|(rel, source)| FileInput::new(rel, source))
            .collect();

        let mut pre = MemberBrokerReading::default();
        for input in &inputs {
            for reference in extract(input, java, &ctx).refs.iter().filter(|r| is_broker(r.relation))
            {
                if reference.target.is_empty() {
                    pre.refused += 1;
                } else {
                    pre.resolved += 1;
                }
            }
        }

        let mut post = MemberBrokerReading::default();
        for facts in extract_files(&inputs, &registry, &ctx) {
            for reference in facts.refs.iter().filter(|r| is_broker(r.relation)) {
                if reference.target.is_empty() {
                    post.refused += 1;
                } else {
                    post.resolved += 1;
                    let side = match reference.relation {
                        Some(ArtifactRelation::BrokerPublish) => &mut published,
                        _ => &mut subscribed,
                    };
                    side.insert((member.clone(), reference.target.clone()));
                }
            }
        }

        if pre.rows() == 0 && post.rows() == 0 {
            continue; // this member writes no broker site at all
        }
        before.insert(member.clone(), pre);
        after.insert(member.clone(), post);
    }

    let sum = |m: &BTreeMap<String, MemberBrokerReading>, f: fn(MemberBrokerReading) -> usize| {
        m.values().copied().map(f).sum::<usize>()
    };
    eprintln!("S-409 corpus: root={}", root.display());
    eprintln!(
        "  per member — resolved/rows, before the accessor hop -> after it \
         (no floor is asserted on any of these):"
    );
    for (member, pre) in &before {
        let post = after[member];
        eprintln!(
            "    {member:<44} {:>4}/{:<4} -> {:>4}/{:<4}",
            pre.resolved,
            pre.rows(),
            post.resolved,
            post.rows(),
        );
    }
    eprintln!(
        "  TOTAL over {} member(s) writing a broker site: resolved {}/{} -> {}/{}",
        before.len(),
        sum(&before, |r| r.resolved),
        sum(&before, MemberBrokerReading::rows),
        sum(&after, |r| r.resolved),
        sum(&after, MemberBrokerReading::rows),
    );

    // ── CROSS-MEMBER KEY AGREEMENT, reported and not asserted ───────────────
    //
    // Resolving an operand is not the same as two members MEETING on it, and the
    // resolved-row count above cannot tell the two apart. Agreement is what
    // [CR-131] §3.2 A2 is ultimately about and what S-410 builds on, so it is
    // measured here rather than left for that story to rediscover.
    //
    // Two figures, because they differ and the difference IS the finding:
    //
    //   byte-equal    — pairs that bind TODAY. `federation::bridge` joins broker
    //                   sites on the stored target's exact bytes.
    //   relaxed-equal — pairs naming the same configuration property under Spring's
    //                   own relaxed binding (`canonical_key`).
    //
    // The gap between them is a spelling gap this story opens and does not close: a
    // resolved ACCESSOR is stored canonically (`${…archivevolumecounters}`, see
    // `accessor::placeholder_for`), while a LITERAL is stored exactly as written
    // (`${…archive-volume-counters}`, unchanged since CR-107 and kept in force by
    // ADR-64's "a topic literal is keyed by its own text exactly as written").
    // Nothing canonicalises a broker target downstream —
    // `federation::coverage::config_bound_keys` resolves placeholders for
    // `HttpClientCall` only — so two spellings of one property sit in two
    // namespaces. Deliberately NOT closed here: doing so means either changing the
    // stored form of the literal rows (an ADR-64 rule change) or canonicalising at
    // the join (which IS S-410's committed-value topic identity). Both are
    // decisions above this task, and the figure is what they should be decided on.
    let byte_equal = published
        .iter()
        .filter(|(pm, pk)| subscribed.iter().any(|(sm, sk)| sm != pm && sk == pk))
        .count();
    let relaxed = |k: &str| -> String {
        match placeholder_keys(k) {
            Some(keys) => keys.iter().map(|k| canonical_key(k)).collect::<Vec<_>>().join("|"),
            None => canonical_key(k),
        }
    };
    let relaxed_equal = published
        .iter()
        .filter(|(pm, pk)| {
            subscribed
                .iter()
                .any(|(sm, sk)| sm != pm && relaxed(sk) == relaxed(pk))
        })
        .count();
    // THE GRAIN, stated because it is the first thing a second measurement
    // disagrees with: these count distinct PUBLISHING `(member, key)` rows that
    // meet at least one other member's subscribe — not publish×subscribe pairs. A
    // pair-grained count of the same estate is larger, because one publish can meet
    // several members' subscribes. Neither is wrong; they answer different
    // questions, and a reader reconciling two figures needs to know which is which.
    eprintln!(
        "  CROSS-MEMBER AGREEMENT over the resolved rows (reported, never floored;\n  \
         grain = distinct PUBLISHING (member, key) meeting >=1 other member's subscribe):\n    \
         distinct (member, key) publishes {} · subscribes {}\n    \
         …meeting another member's subscribe, byte-equal (what binds today): {byte_equal}\n    \
         …the same under Spring relaxed binding (canonical_key):             {relaxed_equal}\n    \
         …lost purely to the accessor/literal spelling difference:           {}",
        published.len(),
        subscribed.len(),
        relaxed_equal.saturating_sub(byte_equal),
    );

    // (0) The corpus is the one the finding was measured against. Asserted so a
    //     green run cannot report a measurement that did not happen.
    assert!(
        !before.is_empty(),
        "the reference workspace at {} produced no broker row at all — this is not \
         the corpus the recorded finding was measured against",
        root.display(),
    );

    // (1) THE INVARIANT: the hop resolves operands, it never silences a site.
    //     Per member, in BOTH directions — the row count cannot fall (a refused row
    //     becomes a resolved row; two resolved rows with distinct keys can only
    //     add), and the resolved count cannot fall either. This is the NFR-CC-04
    //     claim, and it is asserted rather than printed.
    //
    //     `>=` rather than `==` on the rows, deliberately: equality happens to hold
    //     on today's estate (every member's denominator is unchanged) but it is not
    //     structurally guaranteed — two refusals in one declaration dedup to one
    //     keyless row, while the same two resolving to DIFFERENT keys are two rows.
    //     Asserting an equality the mechanism does not promise is how a future
    //     estate goes red for the wrong reason.
    let regressed: Vec<String> = before
        .iter()
        .filter(|(member, pre)| {
            let post = after[*member];
            post.rows() < pre.rows() || post.resolved < pre.resolved
        })
        .map(|(member, pre)| {
            let post = after[member];
            format!(
                "{member} {}/{} -> {}/{}",
                pre.resolved,
                pre.rows(),
                post.resolved,
                post.rows()
            )
        })
        .collect();
    assert!(
        regressed.is_empty(),
        "{} member(s) lost a broker row or a resolved operand across the hop — it \
         must move a row from refused to resolved, never remove one: {regressed:?}",
        regressed.len(),
    );

    // (2) THE HOP ACTUALLY RAN. Strict improvement in the total resolved count,
    //     and this is NOT a floor: it asserts a DIRECTION, never a number, so it
    //     cannot become the census-figure-as-acceptance-floor trap [S-397] recorded.
    //
    //     It exists because assertion (1) alone is invariant under the hop being
    //     completely disabled — `rows()` is `resolved + refused`, so a build that
    //     resolves nothing satisfies it. Demonstrated: with the hop stubbed to
    //     `None`, this test reported green while printing `resolved 16/86 -> 16/86`.
    let resolved_before = sum(&before, |r| r.resolved);
    let resolved_after = sum(&after, |r| r.resolved);
    assert!(
        resolved_after > resolved_before,
        "the accessor hop resolved no additional broker operand on this estate \
         ({resolved_before} -> {resolved_after} of {} rows). Either the hop \
         regressed, or this corpus stopped writing accessor operands — the two are \
         different findings and the per-member table above says which",
        sum(&after, MemberBrokerReading::rows),
    );
}

// ── S-410: topic identity is the committed configured value ──────────────────

/// One member's committed configuration, in the shape the shipped resolver reads
/// it through — canonical key → every definition of it.
///
/// Built here from the member's own config files rather than from an indexed
/// store, for the same reason the rest of this module runs `extract` directly:
/// it measures the capture-and-join path without standing up an 84-member index.
/// The flattener is the **shipped** one
/// ([`source_facts`](logos_core::extract::config::corpus::source_facts)), so the
/// keys and values here are the ones an index would write.
///
/// A plain alias and **not** a newtype, because this is the same concrete type as
/// `federation::bridge::MemberCorpus` and logos-core already ships
/// `impl ConfigLookup for` it. A wrapper here would have re-derived that impl by
/// hand — a hand-mirrored twin of shipped behaviour, which is exactly the drift
/// this harness exists to avoid. (The sibling newtypes in
/// `operand_resolvability/configuration_agreement.rs` and `perf_envelope.rs` wrap
/// `ConfigCorpus` and `Runtime`, foreign shapes with no such impl, so they
/// genuinely need one.)
type HarnessCorpus = BTreeMap<String, Vec<ConfigDefinition>>;

/// One captured broker site, with the topic identity (or identities) the shipped
/// rule gives it against its own member's committed configuration.
struct SiteIdentity {
    /// `true` for a publish (`Producer`), `false` for a subscribe (`Consumer`).
    is_publish: bool,
    /// The enclosing declaration's symbol — the endpoint a bridge edge starts or
    /// ends at, and the grain the promotion pass counts a node at.
    declaration: String,
    /// The operand exactly as the ledger stores it: the BEFORE key.
    operand: String,
    /// The AFTER key(s): the committed value(s), or the operand again when
    /// nothing was admitted. More than one is an overlay disagreement.
    topics: Vec<String>,
    /// Whether the committed sources proved a value at all.
    admitted: bool,
}

impl SiteIdentity {
    /// The keys this site is filed under in the column `committed` selects: the
    /// committed value(s) (`true`) or the stored operand (`false`).
    ///
    /// **One home for the axis the whole measurement turns on.** This selection
    /// was written out at five separate sites across two tests; a BEFORE/AFTER
    /// measurement whose own definition of before and after exists in five copies
    /// is one edit away from reporting two different comparisons as one.
    fn keys(&self, committed: bool) -> &[String] {
        if committed {
            &self.topics
        } else {
            std::slice::from_ref(&self.operand)
        }
    }
}

/// One cross-member fan-out edge, at the grain `broker_edges` emits.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Edge {
    topic: String,
    from_member: String,
    from: String,
    to_member: String,
    to: String,
}

/// Flatten every configuration source under `member_dir` into one corpus.
///
/// Mirrors [`ConfigCorpus::discover`]'s admission (basename decides, per
/// [`config_profile`]) by handing every walked file to the shipped
/// [`source_facts`]; a file that is not a configuration source answers `None`
/// and contributes nothing.
fn member_corpus(root: &std::path::Path, member: &str) -> HarnessCorpus {
    let mut corpus: HarnessCorpus = BTreeMap::new();
    for entry in corpus_walker(&root.join(member)).flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        let Ok(rel) = path.strip_prefix(root) else {
            continue;
        };
        let rel = rel.to_string_lossy().to_string();
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let Some(facts) = source_facts(&rel, &text) else {
            continue;
        };
        for value in facts.values {
            corpus.entry(value.key).or_default().push(ConfigDefinition {
                path: rel.clone(),
                profile: facts.profile.clone(),
                value: value.value,
            });
        }
    }
    corpus
}

/// Print the whole S-410 estate report — the per-member and per-topic tables and
/// the edge deltas — and hand back the `(gained, lost)` edge sets the assertions
/// then judge.
///
/// A separate function because the report is genuinely long and the test that
/// owns it is subject to `max_fn_lines`; printing and asserting are also two
/// different jobs, and keeping the assertions in the test keeps them visible at
/// the end of it rather than buried in the middle of a wall of `eprintln!`.
fn report_topic_identities<'a>(
    root: &std::path::Path,
    identities: &BTreeMap<String, Vec<SiteIdentity>>,
    refused_rows: &BTreeMap<String, usize>,
    corpus_keys: &BTreeMap<String, usize>,
    before: &'a BTreeSet<Edge>,
    after: &'a BTreeSet<Edge>,
) -> (Vec<&'a Edge>, Vec<&'a Edge>) {
    eprintln!("S-410 corpus (measured 2026-09-16): root={}", root.display());
    eprintln!(
        "  Producer/Consumer nodes per member, BEFORE (keyed on the stored operand) \
         and AFTER (keyed on the committed value), at the promotion pass's own \
         grain — one per (declaration, role, topic). The two columns are computed \
         from the two key sets independently rather than asserted equal, because \
         'this story does not move them' is a claim a report should EVIDENCE and \
         not merely state. `keys` is the size of the member's committed corpus.\n    \
         {:<40} {:>13} {:>13} {:>8} {:>6}",
        "member", "producers b/a", "consumers b/a", "refused", "keys"
    );
    // A node's identity at this grain is `(declaration, role, topic)`, so the
    // count is over the DISTINCT triples each key set produces — not over sites.
    // A site resolving to two overlay values is two topics and therefore two
    // nodes AFTER, and one BEFORE; counting sites would hide exactly that.
    let node_count = |rows: &[SiteIdentity], publish: bool, committed: bool| -> usize {
        rows.iter()
            .filter(|s| s.is_publish == publish)
            .flat_map(|s| {
                let keys: &[String] = if committed {
                    &s.topics
                } else {
                    std::slice::from_ref(&s.operand)
                };
                keys.iter().map(move |k| (s.declaration.as_str(), k.as_str()))
            })
            .collect::<BTreeSet<_>>()
            .len()
    };
    let mut producers_total = (0usize, 0usize);
    let mut consumers_total = (0usize, 0usize);
    let mut node_counts_moved: Vec<String> = Vec::new();
    for (member, rows) in identities {
        let p = (node_count(rows, true, false), node_count(rows, true, true));
        let c = (node_count(rows, false, false), node_count(rows, false, true));
        producers_total = (producers_total.0 + p.0, producers_total.1 + p.1);
        consumers_total = (consumers_total.0 + c.0, consumers_total.1 + c.1);
        if p.0 != p.1 || c.0 != c.1 {
            node_counts_moved.push(format!(
                "{member} producers {}->{} consumers {}->{}",
                p.0, p.1, c.0, c.1
            ));
        }
        eprintln!(
            "    {member:<40} {:>6}/{:<6} {:>6}/{:<6} {:>8} {:>6}",
            p.0,
            p.1,
            c.0,
            c.1,
            refused_rows.get(member).copied().unwrap_or(0),
            corpus_keys.get(member).copied().unwrap_or(0),
        );
    }
    eprintln!(
        "    (members whose node count MOVED across the two key sets: {})",
        if node_counts_moved.is_empty() {
            "none".to_string()
        } else {
            node_counts_moved.join(" · ")
        }
    );
    let sites_total: usize = identities.values().map(Vec::len).sum();
    let refused_total: usize = refused_rows.values().sum();
    // The member denominator is stated precisely, because the obvious reading of
    // it disagrees with the one ADR-64's amendment and
    // `…_before_and_after_the_hop` carry. Those count members writing a broker
    // ROW (23 on this estate, refusal-only members included); this table lists
    // only members with at least one KEYED site, because a member whose every row
    // is refused has no identity to resolve. The two are different populations,
    // not a moved figure.
    let refusal_only = refused_rows.keys().filter(|m| !identities.contains_key(*m)).count();
    eprintln!(
        "  TOTAL over {} member(s) writing a KEYED broker site (+{refusal_only} more \
         writing only refusals = {} writing any broker row, the denominator ADR-64's \
         amendment and the S-409 hop table use): producers {}->{}, consumers \
         {}->{}, over {sites_total} keyed site(s) of {} row(s) ({refused_total} \
         refused)",
        identities.len(),
        identities.len() + refusal_only,
        producers_total.0,
        producers_total.1,
        consumers_total.0,
        consumers_total.1,
        sites_total + refused_total,
    );

    let admitted: usize = identities.values().flatten().filter(|s| s.admitted).count();
    eprintln!(
        "  TOPIC IDENTITY, of the {sites_total} keyed site(s):\n    \
         …admitted a committed value (config-bound):        {admitted}\n    \
         …kept the operand as written (literal / unresolved): {}",
        sites_total - admitted,
    );

    eprintln!(
        "  CROSS-MEMBER BRIDGE EDGES (publish endpoint -> subscribe endpoint, \
         cross-member only — the grain `broker_edges` emits):\n    \
         BEFORE, keyed on the stored operand's exact bytes: {}\n    \
         AFTER,  keyed on the committed value where one is committed: {}",
        before.len(),
        after.len(),
    );
    let gained: Vec<&Edge> = after.difference(before).collect();
    let lost: Vec<&Edge> = before.difference(after).collect();
    eprintln!(
        "    gained {} · lost {} · per topic:",
        gained.len(),
        lost.len()
    );
    let mut per_topic: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for e in &gained {
        per_topic.entry(e.topic.as_str()).or_default().0 += 1;
    }
    for e in &lost {
        per_topic.entry(e.topic.as_str()).or_default().1 += 1;
    }
    for (topic, (up, down)) in &per_topic {
        eprintln!("      {topic:<64} +{up} -{down}");
    }

    // PER TOPIC, the other half of the same criterion: how many producers and
    // consumers each identity carries, on both sides of the re-keying. A BEFORE
    // topic is an operand as stored; an AFTER topic is a committed value. They
    // are deliberately listed in ONE table keyed by topic string, so the
    // `${…archiveevents}` row and the `archive-events` row sit together and a
    // reader can see the re-keying rather than infer it from two tables.
    let mut topic_nodes: BTreeMap<&str, [usize; 4]> = BTreeMap::new();
    for rows in identities.values() {
        for site in rows {
            let slot = usize::from(!site.is_publish);
            topic_nodes.entry(site.operand.as_str()).or_default()[slot] += 1;
            for topic in &site.topics {
                topic_nodes.entry(topic.as_str()).or_default()[2 + slot] += 1;
            }
        }
    }
    eprintln!(
        "  PER TOPIC — producers/consumers, BEFORE (the stored operand) then AFTER \
         (the committed value); a topic with a figure on only one side is one the \
         re-keying moved:\n      {:<64} {:>13} {:>13}",
        "topic", "producers b/a", "consumers b/a"
    );
    for (topic, [pb, cb, pa, ca]) in &topic_nodes {
        eprintln!("      {topic:<64} {pb:>6}/{pa:<6} {cb:>6}/{ca:<6}");
    }
    for (label, set) in [("GAINED", &gained), ("LOST  ", &lost)] {
        for e in set.iter().take(20) {
            eprintln!(
                "      {label} {} : {}/{} -> {}/{}",
                e.topic, e.from_member, e.from, e.to_member, e.to
            );
        }
    }

    (gained, lost)
}

/// **S-410's estate measurement**, reported and never floored: what the
/// committed-value topic identity does to `Producer` nodes, `Consumer` nodes and
/// cross-member bridge edges, per member and per topic, with denominators.
///
/// # What BEFORE and AFTER mean here
///
/// Both halves run over the **same** captured rows — the arm is not changed by
/// [S-410], only the key those rows meet on:
///
///   - **BEFORE** — a site is keyed by its stored operand's exact bytes, which is
///     what `federation::bridge` joined on until this story. A publish spelled
///     `${…archivevolumecounters}` (the canonical form [S-409] stores a resolved
///     accessor as) and a subscribe spelled `${…archive-volume-counters}` are two
///     keys.
///   - **AFTER** — a site whose operand resolves against its **own** member's
///     committed configuration is keyed by that committed **value**; one whose
///     keys the corpus proves nothing for keeps its operand as written
///     ([FR-WS-10]'s rule in force). Both are decided by the shipped
///     [`topic_identity`], so the *identity* half cannot drift from the product.
///
/// # The edge counts are harness-derived, and that is a real limitation
///
/// The identity half runs through shipped code; the **fan-out join** does not.
/// `federation::broker::broker_edges` is `pub(super)`, so the cross-member pairing
/// below is re-implemented here: same direction (publish → subscribe), same
/// cross-member exclusion, same `#`-guard behaviour (the guard rides inside the
/// key string), and de-duplication equivalent because [`Edge`] carries the topic.
/// No behavioural difference between the two is known — but nothing *prevents*
/// one, and this project has been bitten by exactly that shape before: [S-397]'s
/// harness proved 79 sites where the product admitted 44. So read the edge
/// figures as **"what the shipped identity rule implies under a faithful join"**,
/// not as "what `ContractBridge::edges` returned". The per-member and per-topic
/// node counts above have no such caveat — they are pure `topic_identity`.
///
/// **That risk is not hypothetical, and this story hit it.** The harness collects
/// into a [`BTreeSet`], so a pair meeting under two overlays de-duplicates here
/// by construction — while the product's `match_indexed` emitted one edge per
/// meeting and had to be taught to `dedup`. The two joins disagreed by exactly
/// that, the harness read green, and the defect was found by reasoning about the
/// product rather than by this measurement. Keep the caveat above in mind
/// whenever these figures are quoted.
///
/// Closing it properly means widening `broker_edges`' visibility to drive it from
/// here, which is a change to the federation module's surface that no S-410
/// criterion asks for; recorded rather than taken.
///
/// `Producer`/`Consumer` are counted at the grain the promotion pass promotes
/// them — one per `(declaration symbol, role, topic)` — and that grain is
/// **unchanged** by this story. They are reported all the same, because the
/// acceptance criterion asks for them and "unchanged" is a finding a reader
/// should be able to see rather than take on trust.
///
/// **Amended 2026-09-19 by [S-424].** This paragraph used to give as its reason
/// that "the promotion pass is per-repo and keys on the stored operand". The
/// grain is still unchanged, but the reason is retired: the pass keys on the
/// committed value now, through the same `identify` this finding measures
/// ([FR-WS-27]). What survives is the narrower true statement — this story moved
/// the *topic*, not the *grain*. The figures above are S-410's and are not
/// re-measured here; S-424's own reading is in the finding at the foot of this
/// file.
///
/// [FR-WS-27]: ../../docs/specs/requirements/FR-WS-27.md
/// [S-424]: ../../docs/planning/journal.md#s-424-the-promoted-topic-inventory-keys-on-the-committed-value
///
/// **No floor is asserted on any figure.** The two assertions are a direction and
/// an anti-fabrication invariant, never a number — the
/// census-figure-as-acceptance-floor trap [S-397] recorded.
///
/// # Recorded finding
///
/// ```text
/// S-410 / CR-131 §3.2 A3, measured 2026-09-16 against ~/source/pec-services
/// (84 enrolled members; 23 write a broker ROW, of which 21 write at least one
/// KEYED site — the other 2 write only refusals. 23 is the denominator ADR-64's
/// amendment and `…_before_and_after_the_hop` use; 21 is this table's, because a
/// member whose every row is refused has no identity to resolve).
///
///   broker rows:                                   86   ← reconciles with S-409's
///     …keyed (a literal or a resolved accessor):   59   ←   `resolved 16/86 -> 59/86`
///     …refused (`topic-not-literal`):              27   ← 36 publish + 23 subscribe keyed
///
///   TOPIC IDENTITY over the 59 keyed sites:
///     …admitted a committed value:                 59   ← every one of them
///     …kept the operand as written:                 0
///
///   CROSS-MEMBER BRIDGE EDGES (publish endpoint -> subscribe endpoint):
///     BEFORE, on the stored operand's exact bytes: 13
///     AFTER,  on the committed value:              33
/// ```
///
/// **The 13 "lost" edges are not losses; they are the same couplings re-keyed.**
/// Every one of them is re-expressed under its committed value, exactly one for
/// one: `${…archiveevents}` -4 / `archive-events` +4, `${…mailboxevents}` -4 /
/// `mailbox-events` +4, `${…notifications}` -3 / `notifications` +3,
/// `${…archivereporting}` -1 / `archive-reporting` +1, `${…officiallogevents}`
/// -1 / `official-log-events` +1. So of the 33 gained, **13 are re-keyed and 20
/// are genuinely new** — couplings the product could not see at all before this
/// story, because the two ends spelled one property differently.
///
/// Among the 20 is the pair [ADR-64]'s 2026-09-15 amendment named as the
/// reproduction of the spelling gap: `reporting-archive-data-downsampler`'s
/// `DownsamplerStream#createTopology` publishes `archive-volume-counters` and
/// `reporting-archive-data-projector`'s `ArchiveVolumeCountersConsumer#consume`
/// subscribes to it. It did not bind before this story; it does now.
///
/// **That every one of the 59 admitted is a fact about this estate, not a
/// property of the rule.** The estate externalises every topic it names, so the
/// placeholder-as-written fallback has **no producer here** — its evidence is the
/// fixture suite in `federation::broker` and `federation::coverage`, not this
/// corpus. A reader should not conclude the fallback is dead code from a `0` that
/// describes one workspace's configuration habits.
///
/// [ADR-64]: ../../docs/specs/architecture/decisions/ADR-64.md
/// [FR-WS-10]: ../../docs/specs/requirements/FR-WS-10.md
/// [S-397]: ../../docs/planning/journal.md#s-397-the-accessor-capture-hop-reaches-the-invocation-arm
/// [S-409]: ../../docs/planning/journal.md#s-409-the-accessor-hop-reaches-the-broker-arm
/// [S-410]: ../../docs/planning/journal.md#s-410-topic-identity-is-the-committed-configured-value-so-a-streams-publish-meets-a-subscribe
#[test]
fn the_reference_workspace_reports_its_topic_identities_before_and_after_the_committed_value_when_one_is_configured(
) {
    let Some(root) = corpus_root() else {
        eprintln!(
            "SKIPPED: set LOGOS_REF_WORKSPACE=<path to the reference workspace> to run the \
             S-410 committed-value topic-identity corpus measurement."
        );
        return;
    };

    let registry = LanguageRegistry::load(std::env::temp_dir()).expect("registry loads");
    let ctx = SymbolContext::default();

    // Java sources, grouped by member — the scope the production pass resolves at.
    let mut by_member: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for entry in corpus_walker(&root).flatten() {
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
        let Some(member) = rel.split('/').next().map(str::to_string) else {
            continue;
        };
        let Ok(source) = std::fs::read_to_string(path) else {
            continue;
        };
        by_member.entry(member).or_default().push((rel, source));
    }

    /// One member's broker sites: `(role is publish, declaration symbol, stored
    /// operand)`, de-duplicated at the promotion pass's own grain.
    type Sites = BTreeSet<(bool, String, String)>;

    let mut sites: BTreeMap<String, Sites> = BTreeMap::new();
    let mut refused_rows: BTreeMap<String, usize> = BTreeMap::new();
    for (member, files) in &by_member {
        let inputs: Vec<FileInput> = files
            .iter()
            .map(|(rel, source)| FileInput::new(rel, source))
            .collect();
        for facts in extract_files(&inputs, &registry, &ctx) {
            for reference in facts.refs.iter() {
                let is_publish = match reference.relation {
                    Some(ArtifactRelation::BrokerPublish) => true,
                    Some(ArtifactRelation::BrokerSubscribe) => false,
                    _ => continue,
                };
                if reference.target.trim().is_empty() {
                    *refused_rows.entry(member.clone()).or_default() += 1;
                    continue;
                }
                sites.entry(member.clone()).or_default().insert((
                    is_publish,
                    reference.source.as_str().to_string(),
                    reference.target.clone(),
                ));
            }
        }
    }

    // Resolve every site's identities through the SHIPPED rule, once per member.
    let mut identities: BTreeMap<String, Vec<SiteIdentity>> = BTreeMap::new();
    let mut corpus_keys: BTreeMap<String, usize> = BTreeMap::new();
    for (member, member_sites) in &sites {
        let corpus = member_corpus(&root, member);
        corpus_keys.insert(member.clone(), corpus.len());
        for (is_publish, symbol, operand) in member_sites {
            let (topics, admitted) = match topic_identity(operand, &corpus) {
                TopicIdentity::Committed { topics, .. } => (topics, true),
                // A literal keys as written, and so does an operand the corpus
                // proves nothing for — [FR-WS-10]'s rule in force, preserved by
                // its re-proposed criterion.
                TopicIdentity::Literal | TopicIdentity::Unresolved { .. } => {
                    (vec![operand.clone()], false)
                }
            };
            identities.entry(member.clone()).or_default().push(SiteIdentity {
                is_publish: *is_publish,
                declaration: symbol.clone(),
                operand: operand.clone(),
                topics,
                admitted,
            });
        }
    }

    // The cross-member fan-out, both ways round. A `(member, key)` publish binds
    // every OTHER member's subscribe on the same key; the edge set is the set of
    // such (publish endpoint, subscribe endpoint) pairs, which is the grain
    // `broker_edges` emits one edge at.
    let edges = |committed: bool| -> BTreeSet<Edge> {
        let mut eps: Vec<(&str, &SiteIdentity, &str)> = Vec::new();
        for (member, rows) in &identities {
            for site in rows {
                let keys: &[String] = if committed {
                    &site.topics
                } else {
                    std::slice::from_ref(&site.operand)
                };
                for key in keys {
                    eps.push((member.as_str(), site, key.as_str()));
                }
            }
        }
        let mut out = BTreeSet::new();
        for (pm, publish, pk) in eps.iter().filter(|(_, s, _)| s.is_publish) {
            for (sm, subscribe, sk) in eps.iter().filter(|(_, s, _)| !s.is_publish) {
                if sm != pm && sk == pk {
                    out.insert(Edge {
                        topic: (*pk).to_string(),
                        from_member: (*pm).to_string(),
                        from: publish.declaration.clone(),
                        to_member: (*sm).to_string(),
                        to: subscribe.declaration.clone(),
                    });
                }
            }
        }
        out
    };

    let before = edges(false);
    let after = edges(true);

    let (gained, lost) = report_topic_identities(
        &root,
        &identities,
        &refused_rows,
        &corpus_keys,
        &before,
        &after,
    );

    // (0) The corpus is the one the finding was measured against. Asserted so a
    //     green run cannot report a measurement that did not happen.
    assert!(
        !identities.is_empty(),
        "the reference workspace at {} produced no keyed broker site at all — this \
         is not the corpus this measurement is about",
        root.display(),
    );

    // (0b) THE CAPABILITY ACTUALLY RAN, asserted as a DIRECTION and never as a
    //      number — the census-figure-as-acceptance-floor trap [S-397] recorded.
    //
    //      It exists because every other assertion here is invariant under the
    //      whole story being switched off: with `topic_identity` stubbed to
    //      `Literal`, `after == before`, `gained` and `lost` are empty, nothing is
    //      admitted and therefore nothing can be fabricated — and this test
    //      reported green while printing `13 -> 13`. Demonstrated, not supposed.
    assert!(
        !gained.is_empty(),
        "the committed-value identity bound no cross-member pair this estate's \
         stored operands did not already bind ({} -> {} edges over {} keyed \
         site(s)). Either the capability regressed, or this corpus stopped \
         committing its topic keys — the per-topic table above says which",
        before.len(),
        after.len(),
        identities.values().map(Vec::len).sum::<usize>(),
    );

    // (0c) The QUALITATIVE claim [ADR-64]'s 2026-09-15 amendment makes, which is
    //      the reason this story exists: the pair it names as reproducing the
    //      spelling gap meets AFTER and did not meet BEFORE. A property of the
    //      rule rather than of this estate's size, so it is an assertion and not
    //      a floor — though it is still estate-shaped, which is why it names the
    //      members rather than counting them.
    let names = |set: &BTreeSet<Edge>, topic: &str, from: &str, to: &str| {
        set.iter().any(|e| {
            e.topic == topic && e.from_member == from && e.to_member == to
        })
    };
    const GAP_TOPIC: &str = "archive-volume-counters";
    const GAP_FROM: &str = "reporting-archive-data-downsampler";
    const GAP_TO: &str = "reporting-archive-data-projector";
    if by_member.contains_key(GAP_FROM) && by_member.contains_key(GAP_TO) {
        assert!(
            names(&after, GAP_TOPIC, GAP_FROM, GAP_TO),
            "the pair ADR-64's 2026-09-15 amendment names as the reproduction of \
             the canonical/verbatim spelling gap does not meet on `{GAP_TOPIC}` \
             even after the committed value is read"
        );
        assert!(
            !names(&before, GAP_TOPIC, GAP_FROM, GAP_TO),
            "that pair meets on the stored operand's exact bytes, so this corpus no \
             longer reproduces the gap the amendment recorded — re-record the \
             finding rather than relaxing this assertion"
        );
    }

    // (1) NEVER FABRICATE ([NFR-RA-05]): no admitted identity is itself a
    //     placeholder, and none is blank. A `${…}` identity on the AFTER side
    //     would mean an indirection was admitted as a topic.
    let fabricated: Vec<String> = identities
        .values()
        .flatten()
        .filter(|site| site.admitted)
        .flat_map(|site| site.topics.iter())
        .filter(|topic| topic.trim().is_empty() || topic.contains("${"))
        .cloned()
        .collect();
    assert!(
        fabricated.is_empty(),
        "an admitted topic identity is a committed value, never an indirection or a \
         blank: {fabricated:?}",
    );

    // (2) A DIRECTION, not a number: every BEFORE edge that survives does so
    //     because the two ends agree on a committed value, and no edge is lost
    //     to a *spelling* the committed value would have reconciled. Stated as
    //     the reported `lost` set being explainable — asserted only where it is
    //     structural: a lost edge must have had at least one end that admitted a
    //     committed value, because two ends that both kept their operand as
    //     written key exactly as they did before.
    let admitted_at = |member: &str, declaration: &str| {
        identities[member]
            .iter()
            .any(|site| site.declaration == declaration && site.admitted)
    };
    let unexplained: Vec<&Edge> = lost
        .iter()
        .copied()
        .filter(|e| {
            !admitted_at(&e.from_member, &e.from) && !admitted_at(&e.to_member, &e.to)
        })
        .collect();
    assert!(
        unexplained.is_empty(),
        "an edge was lost although NEITHER end admitted a committed value — two \
         operands kept as written key exactly as they did before, so this can \
         only be a defect in the join: {unexplained:?}",
    );
}

/// **S-417 / [FR-WS-26] AC8 — the two-frame wrapper hop, measured on the estate
/// with its denominator, and no floor asserted on it.**
///
/// The population is exactly the one [S-392] measured and falsified at one frame
/// and [S-416] carried at two: a publish or subscribe site whose topic operand is
/// a bare parameter of the method enclosing it. Every such site was **refused**
/// before this hop existed — that is what a `ForwardingCandidate` is — so the
/// "before" figure is 0 by construction and the only interesting numbers are the
/// after-count, the denominator, and the census of what still refuses.
///
/// # Why the census is the deliverable and the headline is not
///
/// [S-416] measured **8 of 13** production publish sites at two frames against a
/// tracked floor of 7, and this pass is expected to come in **at or below** that:
/// the harness folds a same-unit `static final` constant at a call site and the
/// production rule refuses one ([FR-WS-26] AC4, and the Notes there for why a
/// harness and an admission carry opposite burdens). A gap is therefore a
/// *reconciliation*, not a regression, and the per-reason census below is what
/// makes the two comparable. **No floor is asserted on any figure here** — the
/// gate that governs this story is [S-416]'s, already passed, and re-asserting a
/// measurement as an acceptance floor is the trap [S-397] recorded.
///
/// # What is asserted, and why only this
///
/// One thing: that the walk found the population at all. A run over a corpus that
/// writes no wrapper-parameter operand reports zero for reasons that have nothing
/// to do with this code, and a green zero is indistinguishable from a working
/// hop. Everything else is printed.
///
/// The per-member and per-reason figures live **here and nowhere else**. They are
/// deliberately not transcribed into prose beside the assertion that prints them,
/// for the reason this module's S-409 sibling gives: a number copied into a doc
/// comment is a twin that goes stale on its own schedule.
///
/// ```text
/// cargo build -p logos-core --test broker_topic_corpus --features agents
/// LOGOS_REF_WORKSPACE=~/source/pec-services RAYON_NUM_THREADS=2 \
///   ./target/debug/deps/broker_topic_corpus-<hash> \
///   the_reference_workspace_reports_its_two_frame_wrapper_resolutions --nocapture
/// ```
///
/// [FR-WS-26]: ../../docs/specs/requirements/FR-WS-26.md
/// [S-392]: ../../docs/planning/journal.md#s-392-measure-the-one-hop-parameter-forwarding-residue
/// [S-397]: ../../docs/planning/journal.md#s-397-the-accessor-capture-hop-reaches-the-invocation-arm
/// [S-416]: ../../docs/planning/journal.md#s-416-measure-the-two-frame-wrapper-residue-over-main-tree-call-sites
#[test]
fn the_reference_workspace_reports_its_two_frame_wrapper_resolutions() {
    let Some(root) = corpus_root() else {
        eprintln!(
            "SKIPPED: set LOGOS_REF_WORKSPACE=<path to the reference workspace> to run the \
             S-417 two-frame wrapper-hop corpus measurement."
        );
        return;
    };

    let registry = LanguageRegistry::load(std::env::temp_dir()).expect("registry loads");
    let ctx = SymbolContext::default();

    let mut by_member: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for entry in corpus_walker(&root).flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("java") {
            continue;
        }
        let (Ok(rel), Ok(source)) = (path.strip_prefix(&root), std::fs::read_to_string(path))
        else {
            continue;
        };
        let rel = rel.to_string_lossy().to_string();
        let Some(member) = rel.split('/').next().map(str::to_string) else {
            continue;
        };
        by_member.entry(member).or_default().push((rel, source));
    }

    // Per member: candidates, resolved, and the elapsed cost of the whole
    // `extract_files` pass. The cost denominator is the member's file count, so a
    // per-file figure is derivable rather than asserted.
    let mut rows: Vec<(String, usize, usize, usize, u128, u128)> = Vec::new();
    let mut census: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut chains: BTreeMap<usize, usize> = BTreeMap::new();
    let mut topics: BTreeSet<(String, String)> = BTreeSet::new();

    for (member, files) in &by_member {
        let inputs: Vec<FileInput> = files
            .iter()
            .map(|(rel, source)| FileInput::new(rel, source))
            .collect();
        let started = std::time::Instant::now();
        let mut facts = extract_files(&inputs, &registry, &ctx);
        let elapsed = started.elapsed().as_micros();

        // The hop's OWN cost, measured apart from the extraction it rides on, so
        // "before and after" is a decomposition of one run rather than a
        // comparison against a second build of the estate. The pass is
        // idempotent over the same inputs — it re-reads the same caller files and
        // re-decides the same candidates — so re-running it on the facts
        // `extract_files` just produced times exactly the work the first run did.
        // Its emissions land in a fact set the assertion below never reads.
        let properties = PropertiesIndex::from_sources(
            &registry,
            inputs.iter().map(|i| (i.path.as_str(), i.source.as_str())),
        );
        let hop_started = std::time::Instant::now();
        resolve_forwarded_topics(&mut facts, &inputs, &registry, &properties);
        let hop = hop_started.elapsed().as_micros();

        let (mut candidates, mut resolved) = (0usize, 0usize);
        for candidate in facts.iter().flat_map(|f| f.forwarding.iter()) {
            candidates += 1;
            match candidate.outcome.as_ref() {
                Some(ForwardingOutcome::Resolved(forwarded)) => {
                    resolved += 1;
                    *chains.entry(forwarded.chain.len()).or_default() += 1;
                    topics.insert((member.clone(), forwarded.topic.clone()));
                }
                Some(ForwardingOutcome::Refused(reason)) => {
                    *census.entry(reason.as_str()).or_default() += 1;
                }
                None => panic!("extract_files decides every candidate"),
            }
        }
        if candidates > 0 {
            rows.push((member.clone(), candidates, resolved, inputs.len(), elapsed, hop));
        }
    }

    eprintln!("S-417 corpus: root={}", root.display());
    eprintln!(
        "  per member — resolved/candidates, over the member's Java file count, \
         with the sync cost BEFORE this hop (the extraction alone) and AFTER it \
         (extraction + hop), in microseconds. No floor is asserted on any of \
         these:"
    );
    eprintln!(
        "    {:<40} {:>7} {:>7} {:>10} {:>10} {:>8}",
        "member", "res/cand", "files", "before_us", "after_us", "hop_%"
    );
    for (member, candidates, resolved, files, elapsed, hop) in &rows {
        let after = *elapsed;
        let before = after.saturating_sub(*hop);
        let share = if after == 0 { 0.0 } else { (*hop as f64) * 100.0 / (after as f64) };
        eprintln!(
            "    {member:<40} {:>7} {files:>7} {before:>10} {after:>10} {share:>7.2}%",
            format!("{resolved}/{candidates}"),
        );
    }
    let before_total: u128 = rows.iter().map(|r| r.4.saturating_sub(r.5)).sum();
    let after_total: u128 = rows.iter().map(|r| r.4).sum();
    let files_total: usize = rows.iter().map(|r| r.3).sum();
    eprintln!(
        "  SYNC COST over {files_total} Java file(s) in {} member(s): before {before_total}us \
         -> after {after_total}us ({:+.2}%)",
        rows.len(),
        if before_total == 0 {
            0.0
        } else {
            ((after_total as f64) - (before_total as f64)) * 100.0 / (before_total as f64)
        },
    );
    let candidates: usize = rows.iter().map(|r| r.1).sum();
    let resolved: usize = rows.iter().map(|r| r.2).sum();
    eprintln!(
        "  TOTAL over {} member(s) writing a parameter-passed topic operand: \
         resolved {resolved}/{candidates}",
        rows.len(),
    );
    eprintln!("  frames taken, by chain length: {chains:?}");
    eprintln!("  refusal census (the residue, by cause): {census:?}");
    eprintln!("  distinct (member, topic) admitted by the hop: {}", topics.len());

    assert!(
        candidates > 0,
        "the reference workspace at {} writes no parameter-passed broker topic \
         operand at all — that is not the population S-416 measured, and a zero \
         here says nothing about the hop",
        root.display(),
    );
}

// ── S-424 / CR-136 / FR-WS-27: the PROMOTED inventory, before and after ──────

/// One ordered member pair the service map can draw as a
/// `publisher → topic → subscriber` hop, and the topic that carries it.
///
/// This is exactly `serviceMapModel.ts`'s own test, transcribed: `topicLinks`
/// folds the per-member inventory onto one node per topic identity, and
/// `drawnThroughATopic(l)` is true for a link when some topic has `l.from` among
/// its producers and `l.to` among its consumers. A pair with no such topic is
/// drawn as a flat service→service line instead.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Hop {
    topic: String,
    producer: String,
    consumer: String,
}

/// One member's promoted node counts at the promotion pass's own grain.
///
/// `Topic` is **repo-scoped** (one node per distinct key in the member), while
/// `Producer`/`Consumer` are **site-scoped** (one per `(declaration, role,
/// topic)`), so the three are counted three different ways — which is why this
/// carries them as a struct rather than a triple that invites the wrong reading.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Promoted {
    topics: usize,
    producers: usize,
    consumers: usize,
}

/// Count what the promotion pass would promote for one member, under the key set
/// `committed` selects: the **committed value** (`true`, the delivered rule) or
/// the **stored operand** (`false`, the rule before [S-424]).
///
/// [S-424]: ../../docs/planning/journal.md#s-424-the-promoted-topic-inventory-keys-on-the-committed-value
fn promoted_counts(rows: &[SiteIdentity], committed: bool) -> Promoted {
    let sites = |publish: bool| -> usize {
        rows.iter()
            .filter(|s| s.is_publish == publish)
            .flat_map(|s| {
                s.keys(committed)
                    .iter()
                    .map(move |k| (s.declaration.as_str(), k.as_str()))
            })
            .collect::<BTreeSet<_>>()
            .len()
    };
    Promoted {
        topics: rows
            .iter()
            .flat_map(|s| s.keys(committed))
            .collect::<BTreeSet<_>>()
            .len(),
        producers: sites(true),
        consumers: sites(false),
    }
}

/// Every hop the service map would draw, over the whole workspace's inventory
/// keyed as `committed` selects.
fn hops(identities: &BTreeMap<String, Vec<SiteIdentity>>, committed: bool) -> BTreeSet<Hop> {
    // topic identity → (producing members, consuming members). Keyed by the topic
    // ALONE, which is what makes a shared identity a coupling (FR-WS-11).
    let mut by_topic: BTreeMap<String, (BTreeSet<&str>, BTreeSet<&str>)> = BTreeMap::new();
    for (member, rows) in identities {
        for site in rows {
            for key in site.keys(committed) {
                let entry = by_topic.entry(key.clone()).or_default();
                if site.is_publish {
                    entry.0.insert(member.as_str());
                } else {
                    entry.1.insert(member.as_str());
                }
            }
        }
    }
    let mut out = BTreeSet::new();
    for (topic, (producers, consumers)) in by_topic {
        for p in &producers {
            for c in &consumers {
                // A self-coupling is not a CROSS-service edge: the bridge emits
                // none and `buildServiceMap` drops one. Excluded here for the
                // same reason, so the figure is comparable with the bridge's.
                if p != c {
                    out.insert(Hop {
                        topic: topic.clone(),
                        producer: (*p).to_string(),
                        consumer: (*c).to_string(),
                    });
                }
            }
        }
    }
    out
}

/// **The [FR-WS-27] AC7 estate measurement.** `Topic`, `Producer` and `Consumer`
/// node counts and the couplings drawn as hops, reported **before and after, per
/// member, with denominators**, from one run.
///
/// # What "before" and "after" mean here, precisely
///
/// The bridge has keyed a broker site on its **committed value** since [S-410].
/// Only the promotion pass changes in this story, so:
///
/// - **BEFORE** = the bridge on committed values × the inventory on **stored
///   operands**. That is the shipped state [CR-136] was filed on, not a
///   hypothetical.
/// - **AFTER** = the bridge on committed values × the inventory on committed
///   values, which is one identity for both.
///
/// The consequence the measurement is for is the hop count: a coupling can be
/// drawn as `publisher → topic → subscriber` only when ONE topic node carries
/// both ends, and a split identity gives it two.
///
/// # No floor is asserted on any figure ([CR-136] CRA-01)
///
/// The assertions are a direction and an anti-fabrication invariant, never a
/// number — the census-figure-as-acceptance-floor trap [S-397] recorded. The
/// pre-change reading is a measurement of the state before the change.
///
/// # The denominator is 81, not 84
///
/// The reference workspace enrols 84 members, three of which
/// (`official-log-export-reporting-adapter`, `official-log-metrics-adapter`,
/// `pecserver-reporting-adapter`) are **empty directories** — 0 files of any
/// kind. 81 is therefore the denominator for any per-member figure, and that is a
/// property of the estate rather than a defect to chase.
///
/// # Recorded finding
///
/// This block is the **code-side home** of the figures; the docs-side home is
/// [CR-136]'s delivery appendix. Every other file that mentions them carries a
/// dated pointer rather than a copy, because a number kept in five places is a
/// number that disagrees with itself in four.
///
/// ```text
/// S-424 / CR-136 / FR-WS-27 AC7, measured 2026-09-19 against ~/source/pec-services
/// on logos 1.4.13.
///
///   DENOMINATORS: 84 members enrolled · 81 with any walked file (the other three —
///   official-log-export-reporting-adapter, official-log-metrics-adapter,
///   pecserver-reporting-adapter — hold nothing but a `.git` directory) · 22 write a
///   keyed broker site, which is the per-member table's denominator.
///
///   PROMOTED NODES, summed over the 22 members (BEFORE keyed on the stored
///   operand, AFTER on the committed value):
///     Topic      57 -> 47
///     Producer   43 -> 43   ← unchanged, and evidenced rather than asserted
///     Consumer   23 -> 23   ← likewise
///
///   DISTINCT topic identities across the workspace — one service-map node each:
///     32 -> 19
///
///   COUPLINGS DRAWN AS A publisher->topic->subscriber HOP:
///     13 -> 33, over 13 -> 31 distinct ordered member pairs.
///     Of the 31 couplings the corrected inventory makes drawable, 13 were
///     already drawn as a hop before the change and the other 18 fell back to a
///     flat service->service line. (The AFTER side of that ratio is 31 of 31 BY
///     CONSTRUCTION — the denominator is the after-column itself — so the
///     measured figure is the BEFORE one. Named rather than printed as a result.)
///
///   COST, on `mailbox-manager` (the member with the most promoted topics),
///   copied to a temporary directory and indexed there — 177 indexable files, 15
///   broker sources on the incremental leg, 6 distinct configuration keys:
///     cold index      1890 ms
///     incremental     674 ms
///     the added read  77 us (median of 3) for all 6 keys, one pooled connection
/// ```
///
/// **Two figures CR-136 recorded at filing did not reproduce, and are corrected
/// here rather than reconciled away.** [CR-136] §2 read the estate as *"32 topic
/// names for 16 real topics"* and *"zero couplings drawn as the hop"*, with *"all
/// 43 resolved broker couplings"* falling back to a flat line. Measured:
///
/// - **19 distinct committed values, not 16.** The CR's own arithmetic did not
///   close either — 32 names less 12 duplicate spellings is 20, not 16 — so this
///   is a correction to a filing-time reading, not a change in behaviour.
/// - **13 couplings were already drawn as hops, not zero.** A coupling whose two
///   ends spell the operand *identically* met on one placeholder-keyed topic node
///   even before this story; only the differently-spelled ones were split. The
///   figure the CR's sentence describes is the 18 that were **not** drawn as hops.
/// - **31 resolved cross-member couplings, not 43.** 43 is the estate's `Producer`
///   node count, which is a count of publish sites rather than of couplings.
///
/// [CR-136] CRA-01 states outright that the filing reading is a measurement of
/// the state before the change and **never an acceptance criterion**, which is why
/// these three departures cost the delivery nothing. No floor is asserted here
/// either.
///
/// # These are HARNESS figures, and the difference has bitten this arm before
///
/// [`promoted_counts`] and [`hops`] model the promotion pass from
/// [`topic_identity`]; they do **not** call `desired_set`, `broker_refs` or
/// `identify`. Three product gates are therefore absent from the model: the
/// enclosing declaration must resolve to a node in the graph (`node_by_symbol`),
/// the topic and site symbols must encode, and the site grain is
/// `(declaration, role, topic)` rather than `(declaration, topic)`. So the
/// `Producer`/`Consumer` columns are an **upper bound** on what the pass
/// promotes, not a reading of it.
///
/// The S-410 finding 600 lines above carries the same warning for the same
/// reason, and records that the hazard already bit once on this very arm:
/// S-397's harness proved 79 sites where the product admitted 44. Read these as
/// what the shipped identity rule implies under a faithful join — and read the
/// **product-side** assertion in
/// [`the_reference_workspace_reports_its_promotion_pass_cost_when_one_is_configured`]
/// for the one figure here that a real `Engine::index` produced.
///
/// [CR-136]: ../../docs/requests/CR-136-promoted-topic-identity-is-the-committed-value.md
/// [FR-WS-27]: ../../docs/specs/requirements/FR-WS-27.md
/// [S-397]: ../../docs/planning/journal.md#s-397-the-accessor-capture-hop-reaches-the-invocation-arm
/// [S-410]: ../../docs/planning/journal.md#s-410-topic-identity-is-the-committed-configured-value-so-a-streams-publish-meets-a-subscribe
#[test]
fn the_reference_workspace_reports_its_promoted_topic_inventory_before_and_after_when_one_is_configured(
) {
    let Some(root) = corpus_root() else {
        eprintln!(
            "SKIPPED: set LOGOS_REF_WORKSPACE=<path to the reference workspace> to run the \
             S-424 promoted-inventory corpus measurement."
        );
        return;
    };

    let registry = LanguageRegistry::load(std::env::temp_dir()).expect("registry loads");
    let ctx = SymbolContext::default();

    // Java sources grouped by member, and the member roster with its denominators.
    let mut by_member: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    let mut members_enrolled: BTreeSet<String> = BTreeSet::new();
    let mut members_with_files: BTreeSet<String> = BTreeSet::new();
    for entry in std::fs::read_dir(&root).expect("the workspace root reads").flatten() {
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with('.') {
                members_enrolled.insert(name);
            }
        }
    }
    for entry in corpus_walker(&root).flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        let Ok(rel) = path.strip_prefix(&root) else {
            continue;
        };
        let rel = rel.to_string_lossy().to_string();
        let Some(member) = rel.split('/').next().map(str::to_string) else {
            continue;
        };
        members_with_files.insert(member.clone());
        if path.extension().and_then(|e| e.to_str()) != Some("java") {
            continue;
        }
        let Ok(source) = std::fs::read_to_string(path) else {
            continue;
        };
        by_member.entry(member).or_default().push((rel, source));
    }

    // Every member's broker sites, de-duplicated at the promotion pass's grain.
    type Sites = BTreeSet<(bool, String, String)>;
    let mut sites: BTreeMap<String, Sites> = BTreeMap::new();
    let mut refused_rows: BTreeMap<String, usize> = BTreeMap::new();
    for (member, files) in &by_member {
        let inputs: Vec<FileInput> = files
            .iter()
            .map(|(rel, source)| FileInput::new(rel, source))
            .collect();
        for facts in extract_files(&inputs, &registry, &ctx) {
            for reference in facts.refs.iter() {
                let is_publish = match reference.relation {
                    Some(ArtifactRelation::BrokerPublish) => true,
                    Some(ArtifactRelation::BrokerSubscribe) => false,
                    _ => continue,
                };
                if reference.target.trim().is_empty() {
                    *refused_rows.entry(member.clone()).or_default() += 1;
                    continue;
                }
                sites.entry(member.clone()).or_default().insert((
                    is_publish,
                    reference.source.as_str().to_string(),
                    reference.target.clone(),
                ));
            }
        }
    }

    // Resolve every site through the SHIPPED rule, once per member, against that
    // member's own corpus and no other ([ADR-64]'s within-reach rule).
    let mut identities: BTreeMap<String, Vec<SiteIdentity>> = BTreeMap::new();
    for (member, member_sites) in &sites {
        let corpus = member_corpus(&root, member);
        for (is_publish, symbol, operand) in member_sites {
            let (topics, admitted) = match topic_identity(operand, &corpus) {
                TopicIdentity::Committed { topics, .. } => (topics, true),
                TopicIdentity::Literal | TopicIdentity::Unresolved { .. } => {
                    (vec![operand.clone()], false)
                }
            };
            identities.entry(member.clone()).or_default().push(SiteIdentity {
                is_publish: *is_publish,
                declaration: symbol.clone(),
                operand: operand.clone(),
                topics,
                admitted,
            });
        }
    }

    let hops_before = hops(&identities, false);
    let hops_after = hops(&identities, true);
    // The couplings the CORRECTED inventory makes drawable, at member-pair grain.
    //
    // Named for what it is. It is `pairs(hops_after)`, so the AFTER column of the
    // "drawn as a hop" figure below is this set intersected with itself — 100% by
    // construction, on any estate. That is not a measurement and the report must
    // not print it as one; the informative half is the BEFORE column, which says
    // how many of these couplings the placeholder-keyed inventory already carried.
    //
    // It is NOT the bridge's own edge set: `broker_edges` is never invoked here.
    // The two coincide today because both join on the committed value, which is
    // exactly what this story delivered — but "coincide because one function keys
    // both" is a claim the unit suite pins, not something this harness observes.
    let drawable_after: BTreeSet<(String, String)> = pairs(&hops_after);
    let totals = report_promoted_inventory(
        &root,
        &identities,
        &refused_rows,
        (&members_enrolled, &members_with_files),
        (&hops_before, &hops_after),
        &drawable_after,
    );

    // (0) The corpus is the one the finding was measured against — a green run
    //     must not be able to report a measurement that did not happen.
    assert!(
        !identities.is_empty(),
        "the reference workspace at {} produced no keyed broker site at all — this \
         is not the corpus this measurement is about",
        root.display(),
    );

    // (0b) THE CAPABILITY ACTUALLY RAN, as a DIRECTION and never as a number. With
    //      the committed-value rule switched off (`topic_identity` stubbed to
    //      `Literal`) every figure below is invariant: after == before, no topic
    //      collapses and no hop appears. This is the assertion that fails then.
    assert!(
        totals.topics_after < totals.topics_before,
        "no topic identity collapsed ({} -> {}) — on an estate that externalises \
         its topics this means the committed-value rule did not run",
        totals.topics_before,
        totals.topics_after,
    );
    assert!(
        hops_after.len() > hops_before.len(),
        "no coupling became drawable as a publisher->topic->subscriber hop \
         ({} -> {}) — which is the consequence [FR-WS-27] exists for",
        hops_before.len(),
        hops_after.len(),
    );

    // (1) NEVER FABRICATE. Every AFTER hop's topic must be a value some member
    //     actually commits, or the operand as written — never a string this
    //     measurement composed. Checked against the union of admitted values and
    //     written operands, which is the whole of what the rule may produce.
    let admissible: BTreeSet<&str> = identities
        .values()
        .flatten()
        .flat_map(|s| {
            s.topics
                .iter()
                .map(String::as_str)
                .chain(std::iter::once(s.operand.as_str()))
        })
        .collect();
    for hop in &hops_after {
        assert!(
            admissible.contains(hop.topic.as_str()),
            "hop topic {:?} is neither a committed value nor a written operand — \
             fabricated",
            hop.topic
        );
    }

    // (2) A hop is never a self-coupling: the bridge emits none and the map draws
    //     none, so a figure that counted them would not be comparable with either.
    for hop in hops_before.iter().chain(hops_after.iter()) {
        assert_ne!(
            hop.producer, hop.consumer,
            "a member coupled to itself is not a CROSS-service hop: {hop:?}"
        );
    }
}

/// The per-member table and roll-up for [`the_reference_workspace_reports_its_promoted_topic_inventory_before_and_after_when_one_is_configured`],
/// returning the totals its assertions then judge.
///
/// Split out for the same two reasons the [S-410] reporter is: the test owning it
/// is subject to `max_fn_lines`, and printing and asserting are different jobs —
/// keeping the assertions in the test keeps them visible at the end of it rather
/// than buried in a wall of `eprintln!`.
///
/// [S-410]: ../../docs/planning/journal.md#s-410-topic-identity-is-the-committed-configured-value-so-a-streams-publish-meets-a-subscribe
fn report_promoted_inventory(
    root: &std::path::Path,
    identities: &BTreeMap<String, Vec<SiteIdentity>>,
    refused_rows: &BTreeMap<String, usize>,
    roster: (&BTreeSet<String>, &BTreeSet<String>),
    hops: (&BTreeSet<Hop>, &BTreeSet<Hop>),
    drawable_after: &BTreeSet<(String, String)>,
) -> InventoryTotals {
    let (enrolled_set, with_files_set) = roster;
    // **Intersected, not counted separately.** The walk keys a file on its first
    // path segment, so a loose file at the workspace ROOT contributes its own file
    // name to `with_files_set` and is not a member at all. Taking `.len()` of that
    // set printed 84-of-84 on an estate where three members are empty — the two
    // sets were the same size for unrelated reasons, which is exactly the kind of
    // coincidence a denominator must not be read through.
    let enrolled = enrolled_set.len();
    let with_files = enrolled_set.intersection(with_files_set).count();
    // NAMED, not just counted: "3 members are empty" is a claim a reader cannot
    // check, and a denominator that silently moves is how a published figure
    // stops reconciling with the one beside it.
    let empty: Vec<&str> = enrolled_set
        .difference(with_files_set)
        .map(String::as_str)
        .collect();
    let (before, after) = hops;
    eprintln!(
        "S-424 / CR-136 promoted-inventory measurement: root={}",
        root.display()
    );
    eprintln!(
        "  DENOMINATORS: {enrolled} members enrolled · {with_files} with any walked \
         file at all (the other {}: {}) · {} write a keyed broker site, which is \
         this table's denominator.",
        empty.len(),
        if empty.is_empty() {
            "none".to_string()
        } else {
            empty.join(", ")
        },
        identities.len(),
    );
    eprintln!(
        "  BEFORE = the inventory keyed on the STORED OPERAND (the state CR-136 was \
         filed on) · AFTER = keyed on the COMMITTED VALUE. The bridge has been on \
         committed values since S-410 in both columns; only the inventory moves.\n    \
         {:<44} {:>11} {:>13} {:>13} {:>7}",
        "member", "topics b/a", "producers b/a", "consumers b/a", "refused"
    );
    let mut totals = InventoryTotals::default();
    for (member, rows) in identities {
        let b = promoted_counts(rows, false);
        let a = promoted_counts(rows, true);
        totals.topics_before += b.topics;
        totals.topics_after += a.topics;
        totals.producers_before += b.producers;
        totals.producers_after += a.producers;
        totals.consumers_before += b.consumers;
        totals.consumers_after += a.consumers;
        eprintln!(
            "    {member:<44} {:>5}/{:<5} {:>6}/{:<6} {:>6}/{:<6} {:>7}",
            b.topics,
            a.topics,
            b.producers,
            a.producers,
            b.consumers,
            a.consumers,
            refused_rows.get(member).copied().unwrap_or(0),
        );
    }
    // The workspace-wide topic node count is NOT the sum of the per-member column:
    // a topic is repo-scoped, so two members naming one topic are two nodes in the
    // graph and one node on the service map. Both are reported, named apart.
    let distinct = |committed: bool| -> usize {
        identities
            .values()
            .flatten()
            .flat_map(|s| s.keys(committed))
            .collect::<BTreeSet<_>>()
            .len()
    };
    totals.distinct_before = distinct(false);
    totals.distinct_after = distinct(true);
    eprintln!(
        "  TOTALS (sum of per-member promoted nodes): topics {} -> {} · producers {} -> {} \
         · consumers {} -> {}",
        totals.topics_before,
        totals.topics_after,
        totals.producers_before,
        totals.producers_after,
        totals.consumers_before,
        totals.consumers_after,
    );
    eprintln!(
        "  DISTINCT topic identities across the workspace (what the service map \
         draws one node each for): {} -> {}",
        totals.distinct_before, totals.distinct_after
    );
    eprintln!(
        "  COUPLINGS DRAWN AS A publisher->topic->subscriber HOP: {} -> {}",
        before.len(),
        after.len()
    );
    eprintln!(
        "    …over {} -> {} distinct ordered MEMBER PAIRS (a pair coupled on two \
         topics is two hops and one line)",
        pairs(before).len(),
        pairs(after).len()
    );
    // CR-136's question, answered with the denominator it is actually measured
    // against and with the tautology named rather than dressed up as a result.
    eprintln!(
        "  Of the {} couplings the CORRECTED inventory makes drawable, {} were \
         already drawn as a hop before the change; the other {} fell back to a \
         flat service->service line. AFTER is {} of {} — equal to its own \
         denominator BY CONSTRUCTION (the denominator is the after-column), so \
         the measured figure here is the BEFORE one.",
        drawable_after.len(),
        drawable_after.intersection(&pairs(before)).count(),
        drawable_after.len() - drawable_after.intersection(&pairs(before)).count(),
        drawable_after.len(),
        drawable_after.len(),
    );
    for hop in after.iter().take(60) {
        eprintln!(
            "      {} --[{}]--> {}",
            hop.producer, hop.topic, hop.consumer
        );
    }
    if after.len() > 60 {
        eprintln!("      … and {} more", after.len() - 60);
    }
    totals
}

/// A hop set projected onto the ordered member pairs it couples — what the
/// service map draws one line (or one pair of hops) for.
fn pairs(set: &BTreeSet<Hop>) -> BTreeSet<(String, String)> {
    set.iter()
        .map(|h| (h.producer.clone(), h.consumer.clone()))
        .collect()
}

/// The roll-up [`report_promoted_inventory`] hands back.
#[derive(Debug, Default)]
struct InventoryTotals {
    topics_before: usize,
    topics_after: usize,
    producers_before: usize,
    producers_after: usize,
    consumers_before: usize,
    consumers_after: usize,
    distinct_before: usize,
    distinct_after: usize,
}

/// The estate member the cost measurement runs over: the one with the most
/// promoted topics in the table above, so the added read is measured where it has
/// the most keys to look up rather than on a member that names one.
const COST_MEMBER: &str = "mailbox-manager";

/// **[FR-WS-27] AC4 / [NFR-PE-02] — the cost of the added committed-configuration
/// read, on cold index and on incremental sync, with its file-count denominator.**
///
/// The member is **copied into a temporary directory** and indexed there. It is
/// never indexed in place: the reference workspace's own enrolment is held for
/// other measurements, and writing a `.logos` store into it would disturb them.
///
/// # How "before" is obtained, stated rather than implied
///
/// AFTER is measured wall clock: a real `Engine::index` and a real
/// `Engine::sync` of the member's broker sources, with the delivered code.
///
/// BEFORE is **AFTER minus the added read**, and the added read is measured too —
/// not modelled. Everything the promotion pass did before this story it still
/// does, unchanged and in the same order; the whole of what this story adds to the
/// pass is one gated `config_definitions` lookup per distinct broker
/// configuration key, issued on one pooled connection. So the delta is that read,
/// and this test times exactly it, against the very store the index just built,
/// over the very key set the ledger names.
///
/// That decomposition is reported as a decomposition. The alternative — an
/// environment escape in `resolve::topics` that suppresses the read so a test can
/// run the old codepath — would put test scaffolding on a production path to save
/// one subtraction, and this project does not pay that price.
///
/// # No budget is asserted, and why that is the right call here
///
/// The figures are **reported**. A wall-clock threshold asserted on a developer
/// machine is a flaky test, not a performance gate; [NFR-PE-02]'s envelope is
/// enforced by `tests/perf_envelope.rs` over its own fixtures. What this test owes
/// is the number, dated and with its denominator.
///
/// [FR-WS-27]: ../../docs/specs/requirements/FR-WS-27.md
/// [NFR-PE-02]: ../../docs/specs/requirements/NFR-PE-02.md
#[test]
fn the_reference_workspace_reports_its_promotion_pass_cost_when_one_is_configured() {
    let Some(root) = corpus_root() else {
        eprintln!(
            "SKIPPED: set LOGOS_REF_WORKSPACE=<path to the reference workspace> to run the \
             S-424 promotion-pass cost measurement."
        );
        return;
    };
    let member = root.join(COST_MEMBER);
    assert!(
        member.is_dir(),
        "the cost measurement's member {COST_MEMBER} is absent from {} — refusing to \
         report a green run that measured nothing",
        root.display(),
    );

    let tmp = TempDir::new().unwrap();
    let dest = tmp.path().join(COST_MEMBER);
    copy_tree(&member, &dest);
    let files = count_files(&dest);
    let dirty = broker_source_files(&dest);

    let engine = logos_core::Engine::start(&dest).expect("engine starts");
    let cold_started = std::time::Instant::now();
    engine.index();
    let cold_ms = cold_started.elapsed().as_millis();

    let inc_started = std::time::Instant::now();
    engine.sync(&dirty);
    let inc_ms = inc_started.elapsed().as_millis();

    let rt = engine.runtime().unwrap();
    let mut topics: Vec<String> = rt
        .submit_read(|store| {
            Ok(store
                .all_nodes()?
                .into_iter()
                .filter(|n| n.kind == logos_core::model::NodeKind::Topic)
                .map(|n| n.name)
                .collect::<Vec<_>>())
        })
        .expect("read runs");
    topics.sort();

    // The added read, measured against the store the index just built, over the
    // key set the ledger names — which is the pass's own gate, re-derived here
    // from the same `placeholder_keys` the shipped predicate uses so this cannot
    // measure a key set the pass would not ask for.
    let keys: Vec<String> = rt
        .submit_read(|store| {
            let mut keys: Vec<String> = Vec::new();
            for row in store.unresolved_refs()? {
                // Matched through `from_wire` on the ENUM, never against a
                // hardcoded wire token: a literal here would go stale silently
                // the day a token is renamed and would then measure zero keys
                // while reporting a cost.
                let broker = matches!(
                    row.payload.as_deref().and_then(ArtifactRelation::from_wire),
                    Some(ArtifactRelation::BrokerPublish) | Some(ArtifactRelation::BrokerSubscribe)
                );
                if !broker {
                    continue;
                }
                for key in placeholder_keys(&row.target).unwrap_or_default() {
                    if !keys.contains(&key) {
                        keys.push(key);
                    }
                }
            }
            Ok(keys)
        })
        .expect("read runs");
    // Three passes, and the MEDIAN reported: a single timing of a sub-millisecond
    // read is noise, and the mean is skewed by the first (cold page cache) run.
    let mut samples: Vec<u128> = (0..3)
        .map(|_| {
            let keys = keys.clone();
            let started = std::time::Instant::now();
            rt.submit_read(move |store| {
                for key in &keys {
                    let _ = store.config_definitions(key)?;
                }
                Ok(())
            })
            .expect("read runs");
            started.elapsed().as_micros()
        })
        .collect();
    samples.sort_unstable();
    let added_us = samples[1];

    eprintln!(
        "S-424 / FR-WS-27 promotion-pass cost: member={COST_MEMBER} from {}",
        root.display()
    );
    eprintln!("  DENOMINATOR: {files} indexable files · {} broker sources re-synced on the incremental leg · {} distinct broker configuration keys read", dirty.len(), keys.len());
    eprintln!("  AFTER  (measured): cold index {cold_ms} ms · incremental sync {inc_ms} ms");
    eprintln!(
        "  ADDED (measured): the committed-configuration read costs {added_us} us \
         (median of 3) for all {} keys, on one pooled connection",
        keys.len()
    );
    // Reported in µs on both sides, because the subtraction is invisible in ms:
    // an 83 µs read truncates to 0 and the decomposition then prints AFTER twice,
    // which reads as "the added read is free" rather than "the added read is
    // three orders of magnitude below the figure it is subtracted from".
    eprintln!(
        "  BEFORE (AFTER minus ADDED): cold index {} us · incremental sync {} us",
        (cold_ms * 1000).saturating_sub(added_us),
        (inc_ms * 1000).saturating_sub(added_us),
    );
    eprintln!("  Promoted topic names: {topics:?}");

    // The measurement is only about cost if the pass actually promoted something
    // and actually read a corpus — otherwise it is timing a no-op.
    assert!(
        !topics.is_empty(),
        "{COST_MEMBER} promoted no topic at all — this is not the member the cost \
         measurement is about"
    );
    assert!(
        !keys.is_empty(),
        "{COST_MEMBER} named no broker configuration key, so the added read never \
         ran and there is no cost here to report"
    );
    // The incremental leg must have had something to sync, or "incremental sync
    // N ms" is the cost of syncing nothing. The dirty set is selected by a
    // literal source grep, so a field rename in the estate would silently empty
    // it and the figure would quietly become a no-op rather than fail.
    assert!(
        !dirty.is_empty(),
        "{COST_MEMBER} yielded no broker source for the incremental leg — the \
         sync figure above would be the cost of syncing nothing"
    );

    // **The one product-side assertion in this file.** Everything above is wall
    // clock, and everything in the sibling inventory harness is modelled from
    // `topic_identity`. These names came out of a real `Engine::index` over real
    // estate sources, through `desired_set` and `broker_refs`, so they are the
    // shipped pass's own output — and on this member every operand is a `${…}`
    // placeholder that only committed configuration can resolve.
    //
    // Revert `broker_refs` to `row.target.trim()` and this is the assertion in
    // the estate suite that fails; without it, both estate harnesses stay green
    // while reporting a capability the product no longer has.
    for name in &topics {
        assert!(
            !name.contains("${"),
            "a promoted topic on {COST_MEMBER} still carries its placeholder: \
             {name:?} — the committed-value rule did not reach the real pass"
        );
    }
}

/// Copy a directory tree, skipping any `.logos` store and `.git` directory — the
/// two things a measurement must never carry into its temporary copy.
fn copy_tree(from: &std::path::Path, to: &std::path::Path) {
    for entry in ignore::WalkBuilder::new(from)
        .hidden(false)
        .git_ignore(false)
        .parents(false)
        .build()
        .flatten()
    {
        let path = entry.path();
        let Ok(rel) = path.strip_prefix(from) else {
            continue;
        };
        if rel
            .components()
            .any(|c| matches!(c.as_os_str().to_str(), Some(".git") | Some(".logos")))
        {
            continue;
        }
        let dest = to.join(rel);
        if entry.file_type().is_some_and(|t| t.is_dir()) {
            std::fs::create_dir_all(&dest).unwrap();
        } else if entry.file_type().is_some_and(|t| t.is_file()) {
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            let _ = std::fs::copy(path, &dest);
        }
    }
}

/// The indexable-file denominator: every file the walk admits.
fn count_files(root: &std::path::Path) -> usize {
    corpus_walker(root)
        .flatten()
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .count()
}

/// The dirty set for the incremental leg: every Java source naming a broker
/// template call or listener, project-relative, as `Engine::sync` takes them.
fn broker_source_files(root: &std::path::Path) -> Vec<PathBuf> {
    corpus_walker(root)
        .flatten()
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .filter_map(|e| {
            let path = e.path();
            if path.extension().and_then(|x| x.to_str()) != Some("java") {
                return None;
            }
            let text = std::fs::read_to_string(path).ok()?;
            (text.contains("kafkaTemplate") || text.contains("@KafkaListener"))
                .then(|| path.strip_prefix(root).ok().map(std::path::Path::to_path_buf))
                .flatten()
        })
        .collect()
}
