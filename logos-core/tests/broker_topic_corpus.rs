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
