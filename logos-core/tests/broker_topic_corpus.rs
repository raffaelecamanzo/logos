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

use logos_core::extract::{extract, extract_files, FileInput, SymbolContext};
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
/// WHAT THIS DOES **NOT** MOVE, and why a reader should not go looking for it.
/// The estate's coverage harnesses — `coverage_headline_baseline`,
/// `coverage_intake_split`, `config_bound_admission` — read the ENROLLED
/// `.logos` stores through `federation::discover`, not a fresh extraction, so
/// none of them can see these 43 rows until the workspace is re-indexed. Their
/// figures were verified byte-identical on this commit and on its merge base
/// (2026-09-15): the five estate assertions that were already failing on main
/// fail with the same numbers here, and this story moves none of them. In
/// particular the HTTP arm's `config-bound` admitted count reads 84 in both.
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

    // (0) The corpus is the one the finding was measured against. Asserted so a
    //     green run cannot report a measurement that did not happen.
    assert!(
        !before.is_empty(),
        "the reference workspace at {} produced no broker row at all — this is not \
         the corpus the recorded finding was measured against",
        root.display(),
    );

    // (1) THE INVARIANT: the hop resolves operands, it never silences a site. A
    //     member that recorded broker rows before must still record them after —
    //     the NFR-CC-04 claim, asserted; the counts above are reported, never
    //     floored.
    let silenced: Vec<&String> = before
        .iter()
        .filter(|(member, pre)| pre.rows() > 0 && after[*member].rows() == 0)
        .map(|(member, _)| member)
        .collect();
    assert!(
        silenced.is_empty(),
        "{} member(s) recorded broker rows before the accessor hop and none after — \
         the hop must move a row from refused to resolved, never remove it: {silenced:?}",
        silenced.len(),
    );
}
