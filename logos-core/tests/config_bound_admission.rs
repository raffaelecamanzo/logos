//! **S-398 T2 — what the SHIPPED pipeline admits on the reference estate**
//! ([FR-WS-19], [NFR-CC-04], [CR-122] §6, [CR-123]).
//!
//! [S-397] T1 wired the accessor capture hop and [S-398] T1 widened it to reach an
//! accessor behind a *qualified* receiver. [S-397] AC2 named the surface the figure
//! must be read from: *"measured through `logos workspace status --json` and
//! **not** through the harness"*, and [S-398] AC5 repeats it.
//!
//! That distinction is the whole reason this file is not part of the
//! `operand_resolvability` harness. The harness resolves the estate's accessors
//! with its own tree-sitter reader and its own filesystem corpus walk; it proves
//! what is *resolvable*. This file reads what the **product** emits, through the
//! same call the CLI makes — `federation::discover` →
//! `EngineRegistry<Engine>(RegistryMode::Lazy)` → [`workspace_status`] → the serde
//! serialization `Output::print` applies for `--json` — and counts rows in the
//! resulting payload. A figure the harness proves is not a figure the product
//! emits; the two ran a factor of nearly two apart before [S-398] T1, and this
//! file records where they stand after it.
//!
//! # The measured finding (2026-09-13, `~/source/pec-services`, 84 members)
//!
//! Measured on the estate re-indexed with merged Iteration-1 state ([S-398] T1,
//! [S-402] T1, [S-400] T1, [S-401] T1), 84 of 84 members read, `covers_all = true`.
//!
//! ```text
//! rows carrying `config-bound` provenance          81      (was 44)
//!   of which bound                                 15      (was  5)
//!   of which ambiguous                             23      (was  9)
//!   of which no-provider-in-workspace              42      (was 30)
//!   of which path-not-composed                      1      (was  0)
//! rows carrying `config-unresolved` provenance      0      (was  0)
//! reference rows in the payload                  1034      (was 1060)
//! the accessor denominator (S-382 reading)         96      (was 108)
//! the harness's upper bound (79 agreed + 2 divergent) 81   (unchanged)
//! ```
//!
//! So: **81 of 96** accessor-denominator sites, and **81 of 81** of what the
//! harness proves resolvable on that denominator. **No floor is asserted on this
//! figure and none should be read into it.** [S-397] AC2's floor of 79 is now
//! exceeded, which is recorded here as an outcome and deliberately *not* re-armed
//! as a criterion for [S-398]: [Sprint 68] was bitten by an inherited census figure
//! becoming a story's acceptance floor, and the defect turned out to be the
//! criterion. `RECORDED_ADMITTED` below is an exact pin on what was measured, in
//! both directions, which is a reproduction claim rather than a floor.
//!
//! **A lower refusal count is not a coverage gain on its own, and this run contains
//! one of each.** Two independent things moved between the 44 and the 81:
//!
//! * **[S-398] T1 — an admission.** `config-bound` rows rose 44 → 81, exactly
//!   **+37**, and the `base-url-runtime` residue fell by the same 37 in the same
//!   four members, site for site: `mailbox-aggregator-api` +19/−19,
//!   `funnel-aggregator-api` +12/−12, `archive-manager` +5/−5,
//!   `notification-adapter` +1/−1. Every row that stopped refusing started
//!   carrying a resolved, committed value. That is a coverage gain.
//! * **[S-402] T1 — a precision correction.** The invocation-intake population
//!   itself fell 186 → 160, **−26**, all of it in `hermodr-mirror`, the estate's one
//!   Go member, whose captured client-call sites went 37 → 11 when the candidacy
//!   gate became receiver-grained. Those 26 were keyless refusals that produced no
//!   reference and no edge, so nothing that bound stopped binding — but the
//!   `base-url-runtime` count falls by 26 for a reason that is not a gain, and the
//!   denominator every egress ratio is computed over falls with it.
//!
//! Netting the two would report 63 fewer refusals as though they were one
//! improvement. They are not, and the two halves are never summed here.
//!
//! # The `base-url-runtime` residue, with its own denominator
//!
//! ```text
//! rows whose reason is `base-url-runtime`           24      (was 87)
//!   of an invocation-intake population of          160      (was 186)
//!   of an HTTP-arm (relation `route`) population of 106     (was 132)
//!   of the egress-resolution denominator of        117      (was 155)
//! ```
//!
//! All 24 are invocation-intake `route` rows, in four members: `hermodr-mirror` 11
//! (Go, the genuine sites [S-402] kept), `mailbox-aggregator-api` 8,
//! `pecserver-facade` 4, `funnel-aggregator-api` 1. `pecserver-facade`'s 4 did not
//! move at all across the change, which is the control: it is the member whose
//! accessors were already unqualified, so [S-398] had nothing to add there.
//!
//! # The shortfall [S-397] T2 recorded, and what closed it
//!
//! [S-397] T2 recorded the gap as **37 sites, every one of them spelling `this.`**:
//! `this.mailboxConfigurationApi.getUriGetMailbox()`. `extract::config::accessor`
//! refused a **qualified receiver** by design — [NFR-RA-05], never guess — where
//! the harness's `operand_name` trims the expression to its last segment and
//! resolves it. [S-398] T1 admitted exactly that shape, gated on the receiver's
//! declared type rather than on the trim, and the estate moved by exactly the 37
//! sites the diagnosis named. The prediction and the outcome agree to the site.
//!
//! Arithmetic, for a reader who wants to reconstruct it: the harness resolves 79
//! agreed + 2 divergent = 81 Java sites, and the payload now carries 81
//! `config-bound` rows of which exactly **2** carry an overlay-divergent key (a
//! `values` list with more than one entry). The totals and the divergent split both
//! agree; site-level identity of the two sets was not independently checked and is
//! not claimed here.
//!
//! # The `.properties` gap is still open, and still costs this estate nothing
//!
//! [S-397] AC5 requires the `.properties` admission residue stated as a numerator
//! over the accessor denominator, so the delivered figure is not read as full
//! coverage. It is measured in
//! `operand_resolvability/configuration_agreement.rs::properties_residue` and is
//! **0 of 96** (it was 0 of 108; the denominator moved, the residue did not): this
//! estate commits every key its accessors read in yaml, and the 3 `.properties`
//! sources the corpus admits define none of them. The gap is still open and still
//! unowned — a residue of zero says what it costs today, not that it is closed.
//!
//! # What a reader should NOT conclude
//!
//! * Not "the pipeline resolves 81 of the estate's couplings". It admits 81 rows
//!   carrying a resolved, committed value; 15 of them bind a provider, and
//!   `resolved_cross_service_edges` is **still 0** — a `config-bound` row is
//!   deliberately excluded from that headline because the bridge still keys a
//!   consumer on its raw ledger target.
//! * Not "egress resolution quadrupled". It reads 0.128 (15 of 117) against 0.032
//!   (5 of 155), and the denominator moved underneath it for a reason that is not
//!   a coverage change.
//! * Not "79 was wrong". 79 is a correct census figure, and it remains one.
//!
//! # VOID is not zero ([NFR-CC-04])
//!
//! A run that cannot see the estate corpus must report **VOID**, never 0 —
//! "measured zero" and "measured nothing" are different findings, and [S-392]'s
//! forwarding gate already tests that distinction. Three gates enforce it here,
//! each failing loudly rather than passing with a zero:
//!
//! * `LOGOS_REF_WORKSPACE` unset → the test **skips** and says so on stdout and
//!   stderr. Be exact about how weak that is: libtest captures BOTH streams for a
//!   test that PASSES, and neither `cargo test --workspace` in CI nor
//!   `scripts/gate.sh` passes `--nocapture`, so **the notice below is never shown
//!   by the runs that matter** — they print `1 passed` over a measurement that did
//!   not happen. Nothing in this repository sets the variable, so that is the
//!   normal case, not the exceptional one. The honest remedy is
//!   `#[ignore = "requires LOGOS_REF_WORKSPACE"]`, which turns `1 passed` into
//!   `0 passed; 1 ignored` — an honest denominator in the default summary. It is
//!   NOT applied here because all six estate-gated harnesses
//!   (S-355/S-365/S-374/S-377/S-392 and this one) share the skip convention, and
//!   changing one of six would leave the other five reading as passes while
//!   implying the whole set had been fixed. Recorded as a defect of the
//!   convention, with its scope named, rather than half-fixed.
//! * `LOGOS_REF_WORKSPACE` set but not a directory, or not a Logos workspace →
//!   **panic**. A typo'd path would otherwise report a green run over nothing.
//! * The workspace opens but no member could be read, or the roster is empty →
//!   **panic with VOID**. This is the case the two above do not cover and the one
//!   [NFR-CC-04] is actually about: the payload is well-formed, every counter
//!   reads 0, and nothing whatsoever was measured.
//!
//! # Read-only
//!
//! It opens each member's existing store and reads it. It indexes nothing and
//! enrols nothing — the re-index this story's figure required was performed once,
//! by hand, before the run, and is recorded in the durable artifact beside this
//! file. A harness that quietly re-indexed would make its own figure
//! unattributable.
//!
//! # `corpus_root` is duplicated — the sixth copy, and the debt is already settled
//!
//! This is the **sixth** byte-identical copy of a nineteen-line `corpus_root`:
//! `operand_resolvability.rs`, `config_corpus.rs`, `broker_topic_corpus.rs`,
//! `coverage_intake_split.rs`, `coverage_headline_baseline.rs`, and this file.
//!
//! `coverage_intake_split.rs` used to arm a revisit "if a fourth measurement
//! arrives". That trigger had already fired two files before this one, so the
//! same change that added this file withdrew it there — read that file's docs
//! for the withdrawal, not for a live trigger. The question is **already
//! settled**, in
//! `coverage_headline_baseline.rs`'s module docs, which examined the inherited
//! "sharing would couple separately-recorded published figures" argument and
//! **rejected it**: `corpus_root` performs no measurement, so a shared locator
//! cannot move a recorded number. Its verdict is the plainer one — N byte-identical
//! copies can drift, a `tests/common/mod.rs` is the idiomatic answer and costs
//! almost nothing, and it is accepted debt until a story has business in those
//! files.
//!
//! That verdict is adopted here rather than re-argued, and the AC2-flavoured
//! version of the rejected argument is **not** made: this arm must be independent
//! of the `operand_resolvability` harness's *reader and corpus walk*, which is
//! what makes it a separate measurement — but both harnesses already read the same
//! `LOGOS_REF_WORKSPACE` variable, so a locator owned by neither would not touch
//! that independence. This file adds the sixth copy for the same reason the fifth
//! was added: lifting a helper out of five unrelated harnesses is a drive-by this
//! task has no business in. The debt is real, it is named, and the count is now
//! stated correctly so the next file to face it inherits a fact rather than an
//! arithmetic error.
//!
//! The full record is the durable artifact
//! `config_bound_admission/config_bound_admission_finding.txt`.
//!
//! [Sprint 68]: ../../docs/planning/sprints/sprint-68.md
//! [CR-122]: ../../docs/requests/CR-122-the-configuration-substrate-reaches-the-product.md
//! [CR-123]: ../../docs/requests/CR-123-invocation-capture-accepts-the-qualified-receiver.md
//! [FR-WS-19]: ../../docs/specs/requirements/FR-WS-19.md
//! [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md
//! [NFR-RA-05]: ../../docs/specs/requirements/NFR-RA-05.md
//! [S-392]: ../../docs/planning/journal.md#s-392-measure-the-one-hop-parameter-forwarding-residue
//! [S-397]: ../../docs/planning/journal.md#s-397-the-accessor-capture-hop-reaches-the-invocation-arm
//! [S-398]: ../../docs/planning/journal.md#s-398-the-accessor-hop-reaches-a-qualified-receiver
//! [S-400]: ../../docs/planning/journal.md#s-400-measure-whether-a-runtime-port-identifies-the-callee
//! [S-401]: ../../docs/planning/journal.md#s-401-a-cross-service-reachability-answer-carries-its-unresolved-residue
//! [S-402]: ../../docs/planning/journal.md#s-402-the-go-client-call-gate-is-receiver-grained

use std::path::PathBuf;

use logos_core::federation::{discover, workspace_status, EngineRegistry, RegistryMode};
use logos_core::Engine;

/// The recorded verdict: printed beside the live run, and pinned on its headline
/// figures by [`the_recorded_artifact_states_the_figures_this_file_pins`].
///
/// **Only the headline figures are pinned, and the distinction is worth keeping.**
/// The artifact is prose; asserting it whole would make every wording change a test
/// failure. What is asserted is that the numbers this file re-records appear in it —
/// which is the failure this arm actually has, a stale artifact sitting beside a
/// freshly re-recorded constant. The doc comment here previously claimed the whole
/// artifact was "pinned by its assertions"; it was not pinned by anything, and
/// falsifying every figure in it left the run green.
const RECORDED_FINDING: &str = include_str!("config_bound_admission/config_bound_admission_finding.txt");

/// The artifact beside this file must state the figures this file pins — corpus-free,
/// so it holds in CI and in a fresh clone.
///
/// A durable finding is re-recorded by hand while the constants are re-recorded in
/// code, and nothing made the two agree: the artifact could keep a superseded
/// numerator beside a live one indefinitely. Each figure is checked as a whole word
/// so `81` cannot be satisfied by `810` or by the `81` inside `1081`.
#[test]
fn the_recorded_artifact_states_the_figures_this_file_pins() {
    // The two MEASURED figures, as whole words. A bare-number search is the right
    // instrument for them precisely because they are large and estate-specific: a
    // stale artifact keeps the old numerator, and the old numerator is absent.
    for (figure, what) in [
        (RECORDED_ADMITTED, "the admitted `config-bound` count"),
        (ACCESSOR_DENOMINATOR, "the accessor denominator"),
    ] {
        let needle = figure.to_string();
        let found = RECORDED_FINDING.match_indices(&needle).any(|(at, _)| {
            let before = RECORDED_FINDING[..at].chars().next_back();
            let after = RECORDED_FINDING[at + needle.len()..].chars().next();
            !before.is_some_and(|c| c.is_ascii_digit()) && !after.is_some_and(|c| c.is_ascii_digit())
        });
        assert!(
            found,
            "config_bound_admission_finding.txt does not state {what} ({figure}) that \
             this file pins. The artifact is the durable dated record of the same \
             measurement — re-record it in the same change as the constant, never after."
        );
    }

    // `CRITERION_FLOOR` is pinned to its own SENTENCE, not as a bare number, and the
    // reason is a measured weakness of the loop above: a small historical literal is
    // searched for in a document full of small numbers, so a mutation to 5 finds the
    // `5` in the before-column of the bucket table and passes. Probed rather than
    // assumed — that mutation ran green before this assertion existed. Tying it to
    // the phrase makes the constant and the narration one thing.
    let phrase = format!("floor of {CRITERION_FLOOR}");
    assert!(
        RECORDED_FINDING.contains(&phrase),
        "the artifact does not contain the phrase \"{phrase}\". CRITERION_FLOOR is a \
         frozen historical literal — the text of S-397 AC2 — so the only thing that can \
         pin it is the sentence it appears in. If the artifact's wording changes, change \
         this phrase with it; if the FLOOR changes, you are rewriting history."
    );
}

/// [S-397] AC2's floor: that criterion asked for **at least** this many.
///
/// Historical. Nothing gates on it at runtime — the reasoning is at the removal
/// site in the test body — and it is kept because the 81-against-79 relation is
/// worth printing beside the measurement. Pinned to its own sentence in the
/// artifact by [`the_recorded_artifact_states_the_figures_this_file_pins`].
const CRITERION_FLOOR: usize = 79;

/// What the shipped pipeline actually admits, re-measured 2026-09-13 over the
/// estate re-indexed with merged Iteration-1 state (was 44 before [S-398] T1).
///
/// Pinned exactly, in both directions. That is a reproduction claim, not a floor:
/// a run that reads a different number has changed the rule, the binary or the
/// corpus, and all three need a human.
const RECORDED_ADMITTED: usize = 81;

/// The accessor denominator the figure is stated over — the S-382 reading's
/// production client-call population.
///
/// **Measured and pinned in another test binary, and deliberately not re-measured
/// here**: AC2 requires this arm to be independent of the `operand_resolvability`
/// harness, so it cannot compute the denominator its own headline is stated over.
/// The owner is
/// `operand_resolvability/configuration_agreement.rs`'s
/// `(s382.denominator, s382.resolved, s382.divergent, s382.no_key) == (96, 79, 2, 15)`
/// assertion, whose failure message lists this constant among the places to
/// re-record with it.
///
/// That pointer is the whole guard, and it is here because the figure has
/// **already moved twice** (111 -> 108 in silence, then 108 -> 96 when [S-402]
/// emptied the Go row, documented at that assertion):
/// unpinned, this constant would keep printing a stale denominator beside a live
/// numerator and stay green. The `const` block at the end of the measurement holds
/// the one relation this binary *can* check — that the admitted figure lies inside
/// it.
const ACCESSOR_DENOMINATOR: usize = 96;

/// The four `(bucket, reason)` pairs the 81 admitted rows fall into, in the order
/// [`admission`] tallies them — a `BTreeMap`, so ascending by the pair rather than
/// by count.
///
/// **Keyed on the reason as well as the bucket, and that is what makes it a
/// diagnosis.** `CoverageState::bucket` folds every non-ambiguous unbound reason —
/// `no-provider-in-workspace`, `base-url-runtime`, `path-not-composed`,
/// `config-key-missing` — into the single string `"unbound"`. Pinned on the bucket
/// alone, this file's central claim (*the shortfall was capture, not resolution*)
/// would have been prose only: a generation in which those 42 rows became
/// `config-key-missing` refusals would have passed with an unchanged split. A bound
/// row names no reason, so its slot is `None`.
///
/// The fourth pair is new since [S-397] T2: one admitted row now refuses under
/// `path-not-composed`, where before every admitted row keyed.
///
/// **That word arrives by the composed-template path, not by the arm's
/// stored-target convention, and the difference is worth stating because the
/// obvious reading is the wrong one.** A `config-bound` row is classified by
/// `federation::coverage::record_config_bound`, whose unbound reason is
/// `composed_refusal(&template).map_or(PathNotComposed, …)` — so the word here is
/// the **fallback default** of that `map_or`: every proven composition failed
/// `consumer_portable_key`, and the arm's own refusal test found nothing to say
/// about the template (it is a valid rooted client-call path). It is *not*
/// `client_call_refusal`'s "a non-empty stored target that will not key", which
/// `coverage.rs` documents as unreachable for a store this binary writes and which
/// a reader would otherwise take this row for — i.e. for a stale-binary artefact
/// rather than for what it is.
///
/// One row is not a trend and is recorded rather than chased.
const RECORDED_BUCKETS: [(&str, Option<&str>, usize); 4] = [
    ("ambiguous", Some("ambiguous"), 23),
    ("bound", None, 15),
    ("unbound", Some("no-provider-in-workspace"), 42),
    ("unbound", Some("path-not-composed"), 1),
];

/// The reference workspace, or `None` when none is configured — the same
/// `LOGOS_REF_WORKSPACE` contract the S-355/S-365/S-374/S-377 measurements read.
///
/// A variable that is **set but does not resolve to a directory** panics rather
/// than skipping: the two cases are indistinguishable to a reader of a green test
/// run, and a typo'd or un-checked-out corpus path would otherwise report success
/// while measuring nothing.
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

/// One reference row's three discriminators, read off the `--json` payload the
/// way a consumer reads them rather than off the Rust types.
///
/// Deliberately from `serde_json::Value` and not from `CrossServiceCoverage`:
/// AC2 names `logos workspace status --json`, and a count taken off the Rust
/// struct would pass even if the field never reached the wire. `provenance` is
/// `#[serde(flatten)]`ed onto the row, so its absence here is a serialization
/// defect, not a missing value — which is why it is read as an `Option` and
/// asserted present rather than defaulted.
#[derive(Debug, PartialEq, Eq)]
struct Row {
    provenance: Option<String>,
    intake: String,
    relation: String,
    bucket: String,
    /// The unbound reason, absent on a bound row. Read because `bucket` alone
    /// cannot tell `no-provider-in-workspace` from `config-key-missing`, and the
    /// difference between them is this task's whole diagnosis.
    reason: Option<String>,
}

/// What [S-397] AC2 asks the payload for.
#[derive(Debug, Default)]
struct Admission {
    /// Every reference row in the payload.
    references: usize,
    /// Rows carrying `config-bound` provenance — **the figure**.
    config_bound: usize,
    /// Rows carrying `config-unresolved` provenance: an accessor whose key the
    /// committed sources do not admit. Counted because zero of them is itself a
    /// finding — the shortfall is not keys going missing.
    config_unresolved: usize,
    /// The `config-bound` rows split by display bucket **and unbound reason**.
    by_bucket: Vec<(String, Option<String>, usize)>,
    /// Rows carrying no `provenance` key at all. Must be zero: the field is not
    /// optional ([FR-WS-19] AC6).
    missing_provenance: usize,
    /// Members the coverage walk actually read, and the roster size.
    members_read: u64,
    members_total: u64,
    covers_all: bool,
}

/// Read the `--json` payload and count what AC2 asks for.
fn admission(payload: &serde_json::Value) -> Admission {
    let coverage = &payload["coverage"];
    let references = coverage["references"].as_array().expect("`references` is an array");
    let mut out = Admission {
        references: references.len(),
        members_read: coverage["members_read"].as_u64().expect("`members_read` is a number"),
        members_total: coverage["members_total"].as_u64().expect("`members_total` is a number"),
        covers_all: coverage["covers_all_members"].as_bool().expect("`covers_all_members` is a bool"),
        ..Admission::default()
    };
    let mut buckets: std::collections::BTreeMap<(String, Option<String>), usize> =
        std::collections::BTreeMap::new();
    for reference in references {
        let row = Row {
            provenance: reference["provenance"].as_str().map(str::to_string),
            intake: reference["intake"].as_str().unwrap_or_default().to_string(),
            relation: reference["relation"].as_str().unwrap_or_default().to_string(),
            bucket: reference["bucket"].as_str().unwrap_or_default().to_string(),
            reason: reference["reason"].as_str().map(str::to_string),
        };
        match row.provenance.as_deref() {
            None => out.missing_provenance += 1,
            Some("config-bound") => {
                out.config_bound += 1;
                *buckets.entry((row.bucket.clone(), row.reason.clone())).or_default() += 1;
                // Every admitted row is a captured call site on the HTTP key.
                // Asserted per row rather than in aggregate, so a payload that
                // admitted a contract-surface row under this provenance names
                // the row that did it.
                assert_eq!(
                    (row.intake.as_str(), row.relation.as_str()),
                    ("invocation", "route"),
                    "a `config-bound` row must be a captured HTTP client call; this one is \
                     {row:?}",
                );
            }
            Some("config-unresolved") => out.config_unresolved += 1,
            Some(_) => {}
        }
    }
    out.by_bucket = buckets.into_iter().map(|((b, r), n)| (b, r, n)).collect();
    out
}

/// The measurement [S-397] AC2 and [S-398] AC5 name, through the surface they name.
///
/// The verdict is **asserted**, not printed. Without assertions a regression that
/// moved the figure — in either direction — would pass silently, and this figure
/// is what says whether the qualified-receiver mechanism [Sprint 68] named is
/// still reaching the product.
///
/// **The remedy for a red run here is to RECORD the new figure** — in this file's
/// docs, in the durable artifact beside it, and in the story's notes — never to
/// bend the pipeline to reproduce this one. Say what moved it, and say which part
/// of the move is an admission and which part is a population correction: a lower
/// refusal count is not a coverage gain on its own.
#[test]
fn measure_config_bound_admission_over_the_reference_workspace_when_one_is_configured() {
    let Some(root) = corpus_root() else {
        println!(
            "SKIPPED: LOGOS_REF_WORKSPACE is unset, so S-397 AC2 was NOT measured. This \
             test reports `ok` having measured NOTHING. Run `LOGOS_REF_WORKSPACE=<path> \
             cargo test -p logos-core --test config_bound_admission` to measure."
        );
        eprintln!(
            "SKIPPED: set LOGOS_REF_WORKSPACE=<path to the reference workspace> to run the \
             S-397 AC2 admission measurement (see this file's module docs for the recorded \
             finding). This run measured nothing."
        );
        return;
    };

    let federation = discover(&root)
        .expect("the workspace manifest parses")
        .unwrap_or_else(|| {
            panic!(
                "LOGOS_REF_WORKSPACE={} is not a Logos workspace: no logos.workspace.toml \
                 found up-tree. Enrol it with `logos init --workspace --yes` first — this \
                 harness deliberately does not.",
                root.display()
            )
        });
    let registry = EngineRegistry::<Engine>::new(federation, RegistryMode::Lazy);

    // The `--json` payload, produced exactly as `Output::print` produces it for
    // `logos workspace status --json` (cli/src/main.rs): `serde_json` over the
    // value `federation::query::workspace_status` returns.
    let payload = serde_json::to_value(workspace_status(&registry)).expect("the payload serializes");
    let a = admission(&payload);

    println!(
        "S-398 T2 — `config-bound` admission over {} ({} of {} members read, covers_all={}):\n\
         \x20 references                          {:>5}   (1060 before S-402)\n\
         \x20 carrying `config-bound`             {:>5}   <- the figure (44 before S-398 T1)\n\
         \x20 carrying `config-unresolved`        {:>5}\n\
         \x20 rows with NO provenance             {:>5}\n\
         \x20 by bucket                           {:?}\n\
         \x20 S-397's historical floor            {:>5}   (exceeded; not a criterion here)\n\
         \x20 accessor denominator (S-382)        {:>5}   (108 before S-402)\n\
         \x20 same payload, pre-hop index gen         0   (two generations back, \
         before the accessor hop existed at all)",
        root.display(),
        a.members_read,
        a.members_total,
        a.covers_all,
        a.references,
        a.config_bound,
        a.config_unresolved,
        a.missing_provenance,
        a.by_bucket,
        CRITERION_FLOOR,
        ACCESSOR_DENOMINATOR,
    );
    println!("\n--- recorded finding ---\n{RECORDED_FINDING}");

    // ── VOID, not zero (NFR-CC-04) ──────────────────────────────────────
    //
    // The payload is well-formed whether or not a single member opened, and
    // every counter in it reads 0 either way. This is the case the corpus_root
    // guards above cannot reach, and the one a zero would be misreported as.
    assert!(
        a.members_total > 0 && a.members_read > 0,
        "VOID, not 0: the workspace at {} declares {} members and the coverage walk read \
         {}, so this run could not see the estate corpus at all. A zero admitted figure \
         from such a run is `measured nothing`, which is a different finding from \
         `measured zero` — do not record it as the latter (NFR-CC-04, the distinction \
         S-392's gate tests).",
        root.display(),
        a.members_total,
        a.members_read,
    );
    assert!(
        a.covers_all,
        "VOID, not 0: {} of {} members read, so the payload's counters are a partial \
         reading of the estate and no admitted figure taken over them is comparable to \
         the recorded one. Fix the unreadable members and re-run (NFR-CC-04).",
        a.members_read,
        a.members_total,
    );
    assert!(
        a.references > 0,
        "VOID, not 0: the coverage tier classified no cross-boundary reference at all \
         over {} members. Nothing was measured (NFR-CC-04).",
        a.members_read,
    );

    // Census floors, not equalities: the estate can only grow and a re-clone must
    // not redden the run.
    //
    // **Placed here, with the VOID gates, and the position is the whole point.**
    // Behind the exact pin below it could not do its job: any corpus collapse it
    // exists to catch also moves `config_bound` off its recorded value, so the pin
    // fires first — and the pin's message says to RECORD the new figure, which is
    // the opposite of the remedy for a broken harness. It survived only in the
    // contrived case where the reference count fell while the admitted count, the
    // bucket split and the unresolved count all held exactly.
    assert!(
        a.members_read >= 40 && a.references >= 900,
        "the recorded finding measured 1034 references over 84 members (it was 1060 \
         before S-402 corrected the Go client-call gate); this run saw {} over {}. A \
         collapsed corpus is a broken harness, not a new finding: do NOT record the \
         figures below as a new measurement — find out why the estate shrank first.",
        a.references,
        a.members_read,
    );

    // ── The payload shape AC2's count rests on ──────────────────────────
    assert_eq!(
        a.missing_provenance, 0,
        "`provenance` is not optional (FR-WS-19 AC6): {} of {} rows carry none, so an \
         admitted value is indistinguishable from an observed one on exactly those rows \
         — which is the boundary ADR-64 draws, and the count below would be taken over a \
         payload that cannot answer the question.",
        a.missing_provenance, a.references,
    );

    // ── The figure ──────────────────────────────────────────────────────
    //
    // **The ordered floor guard that stood here until S-398 T2 is GONE, and its
    // removal is deliberate rather than a tidy-up.** It read
    // `assert!(a.config_bound < CRITERION_FLOOR)` and existed for one reason: while
    // the recorded figure was 44 and the floor 79, a *rise* past the floor would
    // otherwise have surfaced as an opaque `44 != 81` pin mismatch instead of as
    // the sentence a reader needed. It did its job — that is exactly how this
    // run's 81 was surfaced — and it cannot do it again, because the recorded
    // figure now sits ABOVE the floor and the same assertion would fire on every
    // green run.
    //
    // Its symmetric successor would be `a.config_bound >= CRITERION_FLOOR`, and
    // that is NOT written here on purpose: it would turn a census figure into a
    // standing acceptance floor on the product, which is the precise error
    // [Sprint 68] recorded. The exact pin below carries the whole guard now, in
    // both directions, and its message carries the reading.
    assert_eq!(
        a.config_bound, RECORDED_ADMITTED,
        "S-398 T2's recorded finding is that the shipped pipeline admits \
         {RECORDED_ADMITTED} `config-bound` client-call rows on the reference estate \
         (of an accessor denominator of {ACCESSOR_DENOMINATOR}; S-397's historical floor \
         of {CRITERION_FLOOR} is exceeded and is not a criterion here). This run read \
         {}. If the corpus has been re-indexed or re-enrolled, or the accessor hop has \
         widened again, RECORD the new figure here, in the artifact beside this file, \
         AND in the `config-bound` table under \"What this emits on a real estate \
         today\" in docs/howto/commands.md, which restates this numerator in prose no \
         test reads — do not bend the pipeline to reproduce this one. State with it \
         which part of the move is an admission and which part is a change to the \
         captured population: the two are not the same finding and must not be netted.",
        a.config_bound,
    );
    assert_eq!(
        a.by_bucket,
        RECORDED_BUCKETS
            .map(|(bucket, reason, n)| (bucket.to_string(), reason.map(str::to_string), n))
            .to_vec(),
        "the admitted rows' (bucket, reason) split moved. The headline can hold while the \
         split moves, and the split is the more informative half — it is what carries \
         this file's diagnosis rather than its prose. Two readings to keep apart: if the \
         42 `no-provider-in-workspace` rows became `config-key-missing`, the estate's \
         committed sources have stopped defining keys the accessors resolve and the \
         shortfall is no longer capture-only; if `bound` moved, the estate's own topology \
         changed. Note that a `config-bound` row REACHES `resolved_cross_service_edges` \
         when it binds — S-403 T1 removed the exclusion that made the headline a proxy \
         for the bridge's own edge count (CR-127) — so a move in `bound` moves the \
         published headline with it. The estate's figures are restated in prose in \
         SEVEN places outside the baseline artifact, and nothing tests any of them \
         — re-record them all in the same change, or they go quietly wrong on the \
         next re-index: docs/howto/commands.md (the worked examples and the \
         config-bound paragraph), logos-core/src/federation/coverage.rs (the \
         `by_intake` and `resolved_cross_service_edges` field docs), \
         logos-core/src/federation/reach.rs (the `resolved_cross_service_edges` \
         and `spec_conformance_measured` field docs), mcp/src/server.rs (both tool \
         descriptions), web/src/api_v1.rs (the workspace-status doc) and \
         web/ui/src/api/types.ts (the `resolved_edges_summary` example).",
    );
    assert_eq!(
        a.config_unresolved, 0,
        "the recorded finding is that NO admitted accessor names a key the committed \
         sources fail to define: the shortfall against the floor is capture, not \
         resolution. This run read {} `config-unresolved` rows, which moves the diagnosis \
         — record it, because it is the figure the `.properties` residue would show up in.",
        a.config_unresolved,
    );

    const {
        // **The relations this binary can check WITHOUT the corpus, and they are the
        // whole of what a default `cargo test` sees.** The test body above runs only
        // under LOGOS_REF_WORKSPACE, which nothing in this repository sets, so any
        // constant not checked here could be edited to any value and CI would stay
        // green.
        //
        // Each of the three is a claim about this file's own RECORDED CONSTANTS —
        // about whether the sentences it prints are self-consistent — and none is a
        // claim about the product. That distinction is why re-arming
        // `a.config_bound >= CRITERION_FLOOR` at RUNTIME would be wrong (see the
        // comment on the removed runtime guard) while pinning the recorded 81
        // against the recorded 79 here is not: the second says only "this file must
        // not narrate a relation it does not hold", which is exactly the defect
        // found when `CRITERION_FLOOR` was left with nothing constraining it —
        // setting it to 5 compiled, ran green, and printed
        // "S-397's historical floor 5 (exceeded; not a criterion here)".
        assert!(
            RECORDED_ADMITTED <= ACCESSOR_DENOMINATOR,
            "the admitted figure must be inside the denominator it is stated over",
        );
        // `<=`, not `<`: a numerator may legitimately equal its denominator — an
        // estate in which every accessor-denominator site admits reads 96 of 96, and
        // this file already records an 81-of-81 against the harness's upper bound.
        // The strict form held at 81/96 and was loosened deliberately, not by
        // accident.
        assert!(
            CRITERION_FLOOR <= RECORDED_ADMITTED,
            "this file's docs, its printed table and its pin message all say S-397's \
             historical floor is EXCEEDED. If a re-record ever takes the admitted \
             figure back below it, that narration becomes false while every runtime \
             assertion stays green — rewrite the narration in the same change rather \
             than relaxing this.",
        );
    }

}
