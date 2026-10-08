//! **Every absence-reporting site on the three surfaces, enumerated from source
//! and recorded conformant or corrected** ([S-434], [CR-138], [CR-135] §3.3,
//! [FR-EH-04], [FR-UI-04], [NFR-CC-04]).
//!
//! The taxonomy these sites are audited against is stated once, in code, at
//! [`models::quality::absence`]. This file does not restate it; it checks it.
//!
//! # The result, with its denominator and its date
//!
//! **2026-09-20 — 3 non-conformant renderings, in 1 file, found while
//! enumerating 85 production sentinel occurrences over 38 production rows
//! across 3 surfaces.** All three are in [`CORRECTIONS`] and were corrected;
//! every other row was already conformant.
//!
//! The numerator and the denominator describe **one** population — the tree as
//! it stood before the correction. An earlier draft of this paragraph mixed
//! them, quoting the 3 against the *post*-correction census, so a reader
//! reconciling "3 in 1 file" against that file's row could not make the figures
//! meet. Correcting the three renderings removes one row and four sentinel
//! occurrences (each rendering carried both an `n/a` and an `empty graph`, and
//! the duplicated pair collapsed into one constant), which leaves the census
//! recorded below at **81 production occurrences over 37 production rows**,
//! beside 106 occurrences inside test scope — enumerated and separated, never
//! truncated away. [`the_audit_reports_its_count_with_its_denominator`] pins
//! both populations and the arithmetic between them.
//!
//! **No floor is asserted anywhere in this file** ([CR-138] CRA-05). The
//! assertions below are equalities against a dated record of what was found,
//! not thresholds a future audit must clear. A later audit that enumerates more
//! sites and finds none non-conformant is a delivered result; so was most of
//! this one — five stories had already made 36 of the 37 production rows
//! conformant before the audit ran, and manufacturing corrections to improve
//! that figure would defeat its purpose.
//!
//! **Addendum, 2026-09-22 ([S-442]) — 2 production rows added, 3 occurrences,
//! both conformant; no correction.** The relational answers' denominator
//! reuses two lexicon words as written — `unindexed` and `n/a`, the wire tags
//! of `DenominatorAbsence` in `models/navigation.rs` — so the walk finds them
//! there, and they are adjudicated below. The census now reads **84 production
//! occurrences over 39 production rows**, beside the same 106 in test scope.
//! The 2026-09-20 result above is left as it was written: the addition is
//! stated as its own delta ([`S442_ADDITION`]), so both readings stay checkable
//! from this one table. The same story fixed the scanner's raw-string opener
//! (see [`strip_comments`]); that fix moved no other row.
//!
//! **Addendum, 2026-09-23 ([S-444]) — 2 production rows added, 2 occurrences,
//! beside 2 in test scope; neither is a site, no correction.** The shipped
//! guidance now states the language scope of its relational claims by naming
//! the two `cross_file_absence` tags a reader should look for — `same-file-only`
//! and `no-resolved-edges` — in the managed `CLAUDE.md` block (`init/mod.rs`),
//! and `cli/src/main.rs`'s `surface_parity` holds the one wording the three
//! guidance texts are checked against. The census now reads **86 production
//! occurrences over 41 production rows**, beside 108 in test scope; the delta
//! is [`S444_ADDITION`], so every earlier reading stays checkable.
//!
//! **Addendum, 2026-09-25 ([S-445]) — 1 production row added, 1 occurrence,
//! conformant; no correction.** Every `stats` usage cell now carries what its
//! calls answered, and a cell that classified nothing names that absence with
//! the lexicon's `none recorded` as written — `OUTCOME_ABSENCE` in
//! `models/outcome.rs` — rather than coining a word or rendering a `0%`. The
//! census now reads **87 production occurrences over 42 production rows**,
//! beside the same 108 in test scope; the delta is [`S445_ADDITION`].
//!
//! **Addendum, 2026-09-26 ([S-306]) — 0 production rows, 0 occurrences; 7
//! test-scope occurrences, no correction.** The Statistics tab's new
//! tool-attribution card renders `stats`'s `outcome_absence` string verbatim
//! (never re-deriving it from `classified_calls`); its fixtures and assertions
//! spell the same `none recorded` word across `StatisticsView.test.tsx` (3)
//! and `statsModel.test.ts` (4). The census still reads **87 production
//! occurrences over 42 production rows**, beside **115** in test scope; the
//! delta is [`S306_ADDITION`].
//!
//! **Addendum, 2026-10-03 ([S-502]) — 0 production rows, 0 occurrences; 1
//! test-scope occurrence, no correction.** Bodied Cohesion (metric-semantics
//! v7) pins that a repo whose classes have no bodied method reports Cohesion
//! as `n/a` — the existing applicability drop-out, asserted once more in
//! `metrics/tests.rs`. The census still reads **87 production occurrences over
//! 42 production rows**, beside **116** in test scope; the delta is
//! [`S502_ADDITION`].
//!
//! **[S-499] (2026-10-03) adds five test-scope occurrences, no production site,
//! no correction.** The Health drill-downs' three offender states pin that an
//! `n/a` dimension keeps its `n/a` rendering whatever the recorded flag says
//! (`HealthView.test.tsx` 3, `healthModel.test.ts` 2) — assertions on the
//! existing `n/a` sites, not new ones. The census reads **87 production
//! occurrences over 42 production rows**, beside **121** in test scope; the
//! delta is [`S499_ADDITION`].
//!
//! [S-499]: ../../docs/planning/journal.md#s-499-health-drill-downs-render-the-persisted-offenders-in-three-honest-states
//!
//! **Addendum, 2026-10-08 (Sprint 93, [CR-203]) — net +2 production rows,
//! +2 occurrences, +24 test-scope occurrences; no correction.** The Health
//! rewrite ([S-615]) moved fifteen production occurrences out of
//! `HealthView.tsx` into the new catalogue `copy/health.copy.ts` (sixteen there:
//! the three `READING_ABSENCE` sentences, the stale notes, the reading actions,
//! the Gate's absent baseline and the ADR-21 drop-out badge), so four
//! `HealthView.tsx` rows left and six `health.copy.ts` rows arrived, each keeping
//! the verdict it had. The band's former `no baseline` wording is now the Gate's
//! `baseline n/a; informational pass`, counted under `n/a`. [S-617] added one
//! `n/a` to `CoverageView.tsx` (the Per-file coverage widget's absent overall
//! percentage). The widget stories' tests ([S-615], [S-616], [S-617]) add 24
//! test-scope assertions on those same sites. The census reads **89 production
//! occurrences over 44 production rows**, beside **145** in test scope; the
//! delta is [`SPRINT93_DELTA`]. Caught by `gate.sh full` on merged main: every
//! session's `fast` tier tested only the packages it touched, and a web-only
//! change never runs this logos-core suite.
//!
//! **Addendum, 2026-10-08 (Sprint 93 HF-1, [CR-206]) — 0 production rows,
//! −5 production occurrences, −11 test-scope occurrences; no correction.** The
//! widget frame dropped its action line, so `copy/health.copy.ts` lost
//! `readingAction`: one `case` label each of `unindexed`, `unscanned`,
//! `no-production-scope`, `moved-past` and `indeterminate`. Every row keeps its
//! verdict — the command an arm may name (R3) now sits in its own sentence
//! (`READING_ABSENCE`, `staleNote`) rather than in a second switch, and the arms
//! that name none still name none. The view tests lost the state arguments that
//! fed the removed action (`HealthView.test.tsx` −9, its `moved-past` row gone)
//! and two Files & Risk action tests (`FilesView.test.tsx` −2 `n/a`). The census
//! reads **84 production occurrences over 44 production rows**, beside **134** in
//! test scope; the removal is [`HF1_REMOVAL`]. Run by the hotfix itself, since
//! the session's `fast` tier does not reach this suite for a web-only change.
//!
//! [CR-203]: ../../docs/requests/CR-203-every-web-widget-explains-itself.md
//! [CR-206]: ../../docs/requests/CR-206-the-widget-frame-drops-the-action-line.md
//! [S-615]: ../../docs/planning/journal.md#s-615-health-explains-its-gate-and-signal-and-gives-each-of-the-ten-dimensions-a-widget
//! [S-616]: ../../docs/planning/journal.md#s-616-files--risk-abbreviates-long-paths-and-statistics-explains-its-figures
//! [S-617]: ../../docs/planning/journal.md#s-617-every-remaining-web-widget-states-what-it-shows-why-it-matters-and-what-to-do
//!
//! # What is enumerated, and what this cannot catch
//!
//! The walk covers [`SURFACES`] in full — every `.rs`, `.ts` and `.tsx` file
//! under them, **including test modules and test files**, because a census that
//! stops reading at the first `#[cfg(test)] mod tests` is the failure class
//! this file exists to avoid (Sprint 72 review, appendix 5.9: a helper that
//! truncated there let a site past that point pass a guard it should have
//! failed). Test-scope occurrences are *classified*, not skipped, and
//! [`a_new_site_is_detected_even_after_a_test_module`] proves the classifier
//! does not have that defect rather than asserting it.
//!
//! The matcher is the closed sentinel lexicon [`absence::SENTINELS`], matched
//! case-insensitively on a whole-token boundary against the comment-stripped
//! source. It therefore catches every site that *uses* the product's absence
//! vocabulary and **cannot** catch one that reports an absence in words nobody
//! has used before. No lexical census can; what this one does is make the
//! lexicon the contract, so a new spelling is a deliberate act rather than an
//! oversight, and put the lexicon in the product rather than in the test.
//!
//! It also over-captures deliberately: a sentinel in a type alias, a `case`
//! label, a JSX text node or a plain identifier is reported, and the census
//! adjudicates it as `NOT A SITE` with the reason. A census may be wrong only
//! in the loud direction.
//!
//! # The structural arm beside this one
//!
//! The limit above is closed for one class by a second arm, in
//! `relational_denominator_audit.rs` ([S-443]). A relational answer that finds
//! nothing — `{"affected":[],"warnings":[]}` — reports its absence in **no**
//! words, so this census cannot see it however wide its roots. That arm checks
//! shape instead: it enumerates the navigation answer types from their
//! definitions in `models/navigation.rs` and fails any that neither carries
//! `resolution_denominator` nor is exempted there with a reason.
//!
//! The division of the class between the two arms — which absence each owns,
//! and where a new surface or answer type is enrolled — is tabled once, in that
//! file's header, so the two cannot drift. The seam is the denominator's own
//! wording: that arm proves the field is there; this file proves what it says
//! is in the lexicon, through the `models/navigation.rs` rows of [`CENSUS`] and
//! `the_resolution_denominator_speaks_only_the_lexicon`.
//!
//! One more limit, and it is about *enforcement* rather than about the scan.
//! `scripts/gate.sh fast` scopes its test leg to the cargo packages a branch
//! touched, so this test runs on the session gate only when `logos-core` is in
//! that set. A branch that edits only `web/ui/src` or only `cli/src` — the
//! branches most likely to add an absence site — is covered by `gate.sh full`
//! and by CI's workspace run instead. No placement fixes that under a
//! package-scoped fast tier, so it is recorded rather than worked around.
//!
//! # Why the scanner is not inside a scanned surface
//!
//! It would find its own lexicon and its own census table and report them as
//! sites — the self-reference [S-435]'s sibling harness records hitting. Living
//! in `logos-core/tests/` removes it by construction, and
//! [`the_taxonomy_module_is_not_a_reporting_site`] closes the one hole that
//! leaves: `models/quality.rs`, which holds the lexicon, is not on a scanned
//! surface, so it is checked separately for sentinels outside that declaration.
//!
//! # `signalAbsence` is the reference model, not a finding
//!
//! Its classification was settled by [CR-135] §3.3 and re-confirmed unchanged
//! under [S-422]'s story review. [`the_reference_model_is_unchanged`] pins the
//! whole declaration, reasoning included, as a substring of the file — which is
//! a statement about its *text*, not about whether the page still calls it. Its
//! behaviour is defended by its own suite, which was run unmodified.
//!
//! [`models::quality::absence`]: logos_core::models::quality::absence
//! [`absence::SENTINELS`]: logos_core::models::quality::absence::SENTINELS
//! [S-422]: ../../docs/planning/journal.md#s-422-the-health-readout-is-internally-consistent-and-never-stale
//! [S-434]: ../../docs/planning/journal.md#s-434-one-absence-taxonomy-audited-across-the-three-reporting-surfaces
//! [S-435]: ../../docs/planning/journal.md#s-435-the-wiki-generation-pass-names-its-own-surface
//! [S-442]: ../../docs/planning/journal.md#s-442-a-relational-answer-states-the-resolution-denominator-it-was-computed-over
//! [S-443]: ../../docs/planning/journal.md#s-443-the-absence-audit-gains-a-structural-arm-over-the-relational-result-types
//! [S-444]: ../../docs/planning/journal.md#s-444-the-shipped-guidance-states-the-language-scope-its-relational-claims-hold-on
//! [S-445]: ../../docs/planning/journal.md#s-445-a-telemetry-event-records-what-the-call-answered-with-its-denominator
//! [S-502]: ../../docs/planning/journal.md#s-502-cohesion-and-focus-count-bodied-methods-and-metric-semantics-move-to-v7
//! [FR-EH-04]: ../../docs/specs/requirements/FR-EH-04.md
//! [FR-UI-04]: ../../docs/specs/requirements/FR-UI-04.md
//! [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md
//! [CR-135]: ../../docs/requests/CR-135-the-health-readout-is-internally-consistent-and-never-stale.md
//! [CR-138]: ../../docs/requests/CR-138-a-readout-names-the-cause-its-gating-condition-establishes.md

use std::path::{Path, PathBuf};

use logos_core::models::navigation::{DenominatorAbsence, ResolutionDenominator};
use logos_core::models::quality::{absence, CrossFileAbsence, EvaluatedSetAbsence, SignalAbsence};

/// The date the figures in this module's header were taken. A census is a
/// reading of one tree at one moment, and an undated one invites being read as
/// a standing property.
const AUDITED_ON: &str = "2026-09-20";

/// The three surfaces that report an absence, as workspace-relative roots.
///
/// `logos-core/src` is where the words are **written** — the readout, the gate,
/// and every other read-model that renders a figure it does not have. `cli/src`
/// is the CLI's own printing. `web/ui/src` is the SPA, in full rather than only
/// its Health view: a site nobody named must not be able to hide in a tab
/// nobody audited.
///
/// # This root was `logos-core/src/governance` first, and that was too narrow
///
/// Review enumerated the class mechanically instead of trusting the boundary,
/// and found live absence renderings the walk could not see: `logos status`'s
/// *"unindexed: run `logos index` …"* (`navigate/mod.rs`), which names both a
/// cause and a command and is R1/R3's own subject matter — and is the very
/// command [CR-138] §2 reproduced its contradiction against; `FRESHNESS_NA`
/// (`history/coverage/mod.rs`), whose TypeScript **mirror** was already a
/// census row while its source of truth was not; and the doc-symlink notice in
/// `config/discovery.rs`. Worse, review reintroduced the exact wording this
/// audit removed into `logos-core/src/metrics/mod.rs` and the suite stayed
/// green: `the_corrected_wording_is_gone_from_every_surface` is bounded by
/// these roots, so its claim to close "the sibling call site nobody edited"
/// held only inside them.
///
/// A narrower root is a hand-listed boundary, and a hand-listed boundary is the
/// one input on which "a site nobody named cannot be missed" rests.
///
/// # The two exclusions, both stated rather than silent
///
/// `logos-core/src/models/quality.rs` holds [`absence::SENTINELS`], so walking
/// it would report the lexicon as twenty sites — the self-reference [S-435]'s
/// sibling harness records hitting. It is checked separately, and more
/// strictly, by [`the_taxonomy_module_is_not_a_reporting_site`].
///
/// The API facade (`web/src/api_v1.rs`) serves the read-model as JSON and
/// renders no absence text of its own; the SPA is the surface that turns those
/// `null`s into words, and it is scanned. `mcp/src` states the `n/a` contract
/// in tool *descriptions* rather than rendering a figure, and is likewise out.
///
/// [S-435]: ../../docs/planning/journal.md#s-435-the-wiki-generation-pass-names-its-own-surface
/// [CR-138]: ../../docs/requests/CR-138-a-readout-names-the-cause-its-gating-condition-establishes.md
const SURFACES: [(&str, &str); 3] = [
    ("core", "logos-core/src"),
    ("cli", "cli/src"),
    ("spa", "web/ui/src"),
];

/// The one file inside a surface the walk does not read, and why.
///
/// Held as a named constant rather than inlined so the exclusion is a value the
/// census can point at, and so there is exactly one of it.
const LEXICON_HOME: &str = "logos-core/src/models/quality.rs";

/// What the audit corrected: `(file, before, after, why)`.
///
/// One file, three occurrences. `governance::gate` renders the absent signal on
/// the same CLI surface as the readout and reached it by a different path, so
/// [CR-138]'s narrowing passed it by: both verdicts attributed
/// `aggregate_signal == None` to an empty graph, which is one of the **two**
/// causes [`SignalAbsence`] separates — [FR-EH-04] AC2's failure, word for
/// word, one renderer over.
///
/// The correction removes the attribution rather than adding a classification:
/// [`SignalAbsence::classify`] needs the store's node and file counts, which the
/// gate does not read, and R2 forbids naming a cause without the figures that
/// establish it. Behaviourally pinned by
/// `logos-core/tests/quality_readout_scope.rs`'s
/// `the_gate_verdict_names_no_cause_for_an_absent_signal`, on the [CR-138] §2
/// reproduction store.
///
/// [FR-EH-04]: ../../docs/specs/requirements/FR-EH-04.md
/// [CR-138]: ../../docs/requests/CR-138-a-readout-names-the-cause-its-gating-condition-establishes.md
const CORRECTIONS: [(&str, &str, &str, &str); 3] = [
    (
        "logos-core/src/governance/mod.rs",
        "signal or baseline is n/a (empty graph) — informational pass",
        "signal or baseline is n/a — informational pass",
        "`gate_from_snapshot`, the read-only verdict behind `Engine::latest_gate`: \
         names no cause, because this condition establishes none (R1)",
    ),
    (
        "logos-core/src/governance/mod.rs",
        "signal or baseline is n/a (empty graph) — informational pass",
        "signal or baseline is n/a — informational pass",
        "`governance::gate`, the persisting verdict behind `Engine::gate`: the same \
         sentence, and it was a second literal. Now one \
         `SIGNAL_OR_BASELINE_ABSENT` constant, because two paths rendering one \
         condition are a pair that can drift (R5)",
    ),
    (
        "logos-core/src/governance/mod.rs",
        "signal is n/a (empty graph) — cannot satisfy threshold",
        "signal is n/a — cannot satisfy threshold",
        "`gate --threshold`'s floor: `aggregate_signal == None` cannot satisfy a \
         floor whatever the reason, and the reason was never established (R1)",
    ),
];

/// `healthModel.ts`'s reference model, pinned as source text.
///
/// Not a paraphrase and not a behavioural echo: the acceptance criterion is
/// that this code is **unchanged**, so the check is the source itself. A
/// re-worded doc comment moves this, which is the intended sensitivity — the
/// claim being made is about the whole declaration, reasoning included.
///
/// What it does **not** claim, because a `contains` cannot: that the page still
/// calls it. Review bypassed the classifier at its call site
/// (`signalAbsence({ ...status, indexed: true }, …)`) with this test green; the
/// SPA's own suite caught that, which is the division of labour intended here —
/// this guards the reviewed decision's text, `HealthView.test.tsx` guards its
/// use.
const SIGNAL_ABSENCE_REFERENCE_MODEL: &str = r#"export type SignalAbsence = "unindexed" | "unscanned" | "no-production-scope";

/**
 * Classify the absence behind a null signal or an empty metric grid.
 *
 * `status.indexed` is a whole-graph fact (`files > 0 || nodes > 0`, test nodes
 * included); the metric snapshot's emptiness is a *production*-subgraph fact. The
 * two are different questions, so `indexed` alone cannot tell `unscanned` from
 * `no-production-scope` — `evolution.snapshots` supplies the missing bit, being
 * non-empty exactly when a `metric_snapshots` row exists.
 */
export function signalAbsence(status: StatusInfo, evolution: EvolutionReport): SignalAbsence {
  if (!status.indexed) return "unindexed";
  if (evolution.snapshots.length === 0) return "unscanned";
  return "no-production-scope";
}
"#;

/// A synthetic Rust source whose second absence site sits **after** a
/// `#[cfg(test)]` module — the exact shape that made a comparable census report
/// clean by construction (Sprint 72 review, appendix 5.9).
const FIXTURE_WITH_A_SITE_AFTER_THE_TEST_MODULE: &str = r#"
fn already_declared() -> String {
    "n/a (empty graph)".to_string()
}

#[cfg(test)]
mod tests {
    #[test]
    fn t() {
        assert_eq!(already_declared(), "n/a (empty graph)");
    }
}

fn a_second_unrecorded_site() -> String {
    "none recorded (no rule check has run)".to_string()
}
"#;

/// Every absence-reporting occurrence on the three surfaces, as
/// `(surface, file, sentinel, production occurrences, test occurrences,
/// verdict)`.
///
/// Compared for **equality** with the walk, in both directions: an occurrence
/// the table does not declare fails, and a declared row the walk no longer
/// finds fails too. The second direction is what stops the table rotting into a
/// list of sites that used to exist.
///
/// The key is `(file, sentinel)` rather than `(file, enclosing function)`. Two
/// reasons, and the first is the load-bearing one: the walk crosses Rust and
/// TypeScript, and "the enclosing function" needs a different, separately
/// wrong-able parser in each. The second is that line numbers and function
/// names drift under refactoring while the pair here does not, so the table
/// moves when the *absences* move rather than when the code around them does.
/// The occurrence counts carry what the key drops: a second `n/a` added to a
/// file that already has one moves its count and fails.
const CENSUS: [(&str, &str, &str, usize, usize, &str); 84] = [
    (
        "core",
        "logos-core/src/config/discovery.rs",
        "unindexed",
        7,
        4,
        "NOT A SITE — six are identifiers (`UnindexedDocSymlink`, `unindexed_doc_symlinks`) and one is the FR-IX-11 diagnostic \"documentation directory-symlink … exists but is unindexed\", which reports a CONFIGURATION fault rather than a figure the surface would otherwise have. The matcher over-captures identifiers deliberately: a census may be wrong only in the loud direction",
    ),
    (
        "core",
        "logos-core/src/federation/query.rs",
        "empty graph",
        0,
        2,
        "NO PRODUCTION SITE — 2 test occurrence(s) asserting sites declared elsewhere in this table; enumerated, never truncated away",
    ),
    (
        "core",
        "logos-core/src/governance/mod.rs",
        "n/a",
        2,
        0,
        "CORRECTED — the gate's two absent-signal verdicts. Both read \"n/a (empty graph)\" over a condition (`aggregate_signal == None` on either side) that has the two causes `SignalAbsence` separates, so both stated a cause the condition never established (R1, FR-EH-04 AC2). See CORRECTIONS",
    ),
    (
        "core",
        "logos-core/src/governance/mod.rs",
        "no baseline",
        2,
        0,
        "CONFORMANT — the gate's no-baseline informational pass, in its two paths. R3: `gate --save` is exactly the command this condition identifies. R4: an informational pass, never a silent zero",
    ),
    (
        "core",
        "logos-core/src/governance/readout.rs",
        "at an unknown age",
        2,
        0,
        "CONFORMANT — `render_age`'s two degradations, a negative and an implausible stamp. R4: the age is named unusable, never rendered as a plausible phrase. The cross-surface twin of `dashboardModel.ts`, pinned byte-identical below",
    ),
    (
        "core",
        "logos-core/src/governance/readout.rs",
        "empty graph",
        1,
        5,
        "CONFORMANT — `SignalAbsence::EmptyGraph`'s clause, and the one honest use of the phrase on this surface: the readout reads the store's own counts, so its condition does establish the cause (R1)",
    ),
    (
        "core",
        "logos-core/src/governance/readout.rs",
        "evaluated set unknown",
        2,
        4,
        "CONFORMANT — `EvaluatedSetAbsence::Unrecorded`, plus the `(None, None)` arm that names the set unknown and attributes no cause. R1 in its strictest form: an unestablished cause is reported absent, not guessed",
    ),
    (
        "core",
        "logos-core/src/governance/readout.rs",
        "n/a",
        5,
        10,
        "CONFORMANT — the signal cell's three arms (no cause, empty graph, no production scope) and the baseline/delta clauses. R1 throughout; the `NoProductionScope` arm carries its two figures (R2)",
    ),
    (
        "core",
        "logos-core/src/governance/readout.rs",
        "no baseline",
        1,
        1,
        "CONFORMANT — the summary channel's baseline clause. The full channel renders a different sentence naming `gate --save`, and `render_signal` records why that fork is deliberately not consolidated — R5's stated carve-out, recorded at the site",
    ),
    (
        "core",
        "logos-core/src/governance/readout.rs",
        "no pass is stated",
        1,
        1,
        "CONFORMANT — R4 at its sharpest: a run that evaluated nothing and found nothing is that state, never \"clean\" and never a bare 0 (FR-GV-03)",
    ),
    (
        "core",
        "logos-core/src/governance/readout.rs",
        "no rules contract",
        1,
        2,
        "CONFORMANT — `EvaluatedSetAbsence::NoContract`. R3: names no command, because the marker is written by `replace_violations`, which `scan` calls too",
    ),
    (
        "core",
        "logos-core/src/governance/readout.rs",
        "none recorded",
        1,
        6,
        "CONFORMANT — no marker is absence of knowledge, and R4 forbids reading it as a pass (BR-41). \"rule check\" is the activity, not a command (R3)",
    ),
    (
        "core",
        "logos-core/src/governance/readout.rs",
        "not comparable",
        2,
        2,
        "CONFORMANT — the baseline and delta clauses in both channels. R4: an incomparable baseline is named, never rendered as a delta of 0",
    ),
    (
        "core",
        "logos-core/src/governance/tests.rs",
        "unindexed",
        0,
        1,
        "NO PRODUCTION SITE — 1 test occurrence(s) asserting sites declared elsewhere in this table; enumerated, never truncated away",
    ),
    (
        "core",
        "logos-core/src/graph_store/schema.rs",
        "n/a",
        1,
        0,
        "NOT A SITE — a SQL comment inside the DDL string literal, documenting that a NULL signal column is the ADR-12 sentinel. It renders nothing to a user; the stripper cannot see into a second comment language nested in a Rust string, and reporting it is the safe direction",
    ),
    (
        "core",
        "logos-core/src/graph_store/tests.rs",
        "empty graph",
        0,
        1,
        "NO PRODUCTION SITE — 1 test occurrence(s) asserting sites declared elsewhere in this table; enumerated, never truncated away",
    ),
    (
        "core",
        "logos-core/src/history/coverage/mod.rs",
        "n/a",
        1,
        0,
        "CONFORMANT — `FRESHNESS_NA`, the never-covered-file sentinel (FR-CV-05), and the SOURCE OF TRUTH whose TypeScript mirror (`api/types.ts`'s `Freshness`) this census already carried. Enumerating the mirror and not its origin is what the narrower root cost, and R4 holds at both ends: never a shifted number, never a guessed 0",
    ),
    (
        "core",
        "logos-core/src/history/hotspot.rs",
        "n/a",
        0,
        4,
        "NO PRODUCTION SITE — 4 test occurrence(s) asserting sites declared elsewhere in this table; enumerated, never truncated away",
    ),
    (
        "core",
        "logos-core/src/history/temporal.rs",
        "n/a",
        0,
        1,
        "NO PRODUCTION SITE — 1 test occurrence(s) asserting sites declared elsewhere in this table; enumerated, never truncated away",
    ),
    (
        "core",
        "logos-core/src/history/tests.rs",
        "n/a",
        0,
        1,
        "NO PRODUCTION SITE — 1 test occurrence(s) asserting sites declared elsewhere in this table; enumerated, never truncated away",
    ),
    (
        "core",
        "logos-core/src/hydrate/tests.rs",
        "empty graph",
        0,
        1,
        "NO PRODUCTION SITE — 1 test occurrence(s) asserting sites declared elsewhere in this table; enumerated, never truncated away",
    ),
    (
        "core",
        "logos-core/src/init/mod.rs",
        "no-resolved-edges",
        1,
        0,
        "NOT A SITE — the managed CLAUDE.md block's \"Where the relational claims hold\" paragraph, naming the `cross_file_absence` tags a reader should look for on a relational answer's `resolution_denominator`. Guidance prose reporting no absence of its own; it speaks the lexicon's spelling, pinned to the serialised tag by `surface_parity`'s vocabulary check (S-444, FR-IN-09 AC 4)",
    ),
    (
        "core",
        "logos-core/src/init/mod.rs",
        "same-file-only",
        1,
        0,
        "NOT A SITE — the managed CLAUDE.md block's \"Where the relational claims hold\" paragraph, naming the `cross_file_absence` tags a reader should look for on a relational answer's `resolution_denominator`. Guidance prose reporting no absence of its own; it speaks the lexicon's spelling, pinned to the serialised tag by `surface_parity`'s vocabulary check (S-444, FR-IN-09 AC 4)",
    ),
    (
        "core",
        "logos-core/src/metrics/tests.rs",
        "empty graph",
        0,
        4,
        "NO PRODUCTION SITE — 4 test occurrence(s) asserting sites declared elsewhere in this table; enumerated, never truncated away",
    ),
    (
        "core",
        "logos-core/src/metrics/tests.rs",
        "n/a",
        0,
        3,
        "NO PRODUCTION SITE — 3 test occurrence(s) asserting sites declared elsewhere in this table; enumerated, never truncated away",
    ),
    (
        "core",
        "logos-core/src/models/navigation.rs",
        "n/a",
        1,
        0,
        "CONFORMANT — `DenominatorAbsence::NotAvailable`'s wire tag, a `#[serde(rename)]`: a relational answer that read no resolution denominator — the ADR-14 degraded path, or a failed denominator read beside a successful answer, whose `warnings` say why. R1: names no cause. R4: a defaulted answer states this, never an empty row list that reads as a measured absence (S-442, FR-NV-14)",
    ),
    (
        "core",
        "logos-core/src/models/navigation.rs",
        "unindexed",
        2,
        0,
        "CONFORMANT — `DenominatorAbsence::Unindexed`, the variant and its one construction in `ResolutionDenominator::measured`, serialised by the kebab-case derive as the lexicon's own spelling. R1: its condition — no anchor of the answer resolved to an indexed node or file — establishes exactly that. R3: names no command, unlike the Health page's `unindexed`, because a misspelt symbol is as likely as an unindexed one and the answer's `suggestions` already speak to it (S-442, FR-NV-14)",
    ),
    (
        "core",
        "logos-core/src/models/outcome.rs",
        "none recorded",
        1,
        0,
        "CONFORMANT — `OUTCOME_ABSENCE`, what a `stats` usage cell with `classified_calls == 0` carries in `outcome_absence` in place of an answered rate. R1: the condition establishes that no outcome was recorded and nothing about why — no vocabulary, pre-v4 rows, or rolled-up pre-v4 days — so it names no cause. R4: never a `0%`; the two counts ship and no rate does. R5: derived once, at serialisation, from the count itself (S-445, FR-OB-14)",
    ),
    (
        "core",
        "logos-core/src/navigate/branch.rs",
        "unindexed",
        1,
        0,
        "NOT A SITE — coverage prose (\"an unindexed language, an excluded path …\") stating what the graph does not reach. Adjacent to the taxonomy and not an instance of it: the taxonomy governs a figure the surface does not have, and this qualifies a figure it does",
    ),
    (
        "core",
        "logos-core/src/navigate/mod.rs",
        "unindexed",
        3,
        0,
        "CONFORMANT, and the row is mixed — the verdict adjudicates all three. One is a site: `logos status`'s \"unindexed: run `logos index` …\", which R1 satisfies (the condition is exactly \"nothing is indexed\") and R3 permits to name a command for the same reason HealthView's `unindexed` arm may. It is also the readout `CR-138` §2 reproduced its contradiction against. The other two are the coverage prose recorded on `navigate/branch.rs`",
    ),
    (
        "core",
        "logos-core/src/pipeline/tests.rs",
        "empty graph",
        0,
        1,
        "NO PRODUCTION SITE — 1 test occurrence(s) asserting sites declared elsewhere in this table; enumerated, never truncated away",
    ),
    (
        "core",
        "logos-core/src/resolve/tests.rs",
        "unindexed",
        0,
        1,
        "NO PRODUCTION SITE — 1 test occurrence(s) asserting sites declared elsewhere in this table; enumerated, never truncated away",
    ),
    (
        "cli",
        "cli/src/main.rs",
        "no rules contract",
        1,
        0,
        "CONFORMANT — `report_check`'s FR-GV-22 exit-4 line. A second spelling of `EvaluatedSetAbsence::NoContract`'s condition on a second surface, which R5 permits: R5 binds within a surface, and this line is the CLI's own, printed before any read-model rendering",
    ),
    (
        "cli",
        "cli/src/main.rs",
        "no-resolved-edges",
        0,
        1,
        "NO PRODUCTION SITE — 1 test occurrence(s) asserting sites declared elsewhere in this table; enumerated, never truncated away",
    ),
    (
        "cli",
        "cli/src/main.rs",
        "nothing was evaluated",
        1,
        0,
        "CONFORMANT — R4: the absent contract is named instead of rendered as a zero violation count (NFR-CC-04), which is the whole reason this arm exists rather than printing the report",
    ),
    (
        "cli",
        "cli/src/main.rs",
        "same-file-only",
        0,
        1,
        "NO PRODUCTION SITE — 1 test occurrence(s) asserting sites declared elsewhere in this table; enumerated, never truncated away",
    ),
    (
        "spa",
        "web/ui/src/api/types.ts",
        "n/a",
        1,
        0,
        "CONFORMANT — the `Freshness` union declares the sentinel as a value of the read-model; it renders nothing itself. Enumerated because a vocabulary declaration is where a fifth spelling would first appear",
    ),
    (
        "spa",
        "web/ui/src/copy/health.copy.ts",
        "indeterminate",
        1,
        0,
        "CONFORMANT — moved from `HealthView.tsx` with the Health rewrite (S-615, 2026-10-08): `staleNote`'s third arm. Neither `current` nor a date, and `detail` names the fact that is missing; no command, because none establishes currency (R1, R3, R4). HF-1 (CR-206, 2026-10-08) removed `readingAction`, whose `noAction` arm was the second occurrence",
    ),
    (
        "spa",
        "web/ui/src/copy/health.copy.ts",
        "moved-past",
        1,
        0,
        "CONFORMANT — moved from `HealthView.tsx` (S-615): `staleNote` claims only that the graph was indexed or synced since the snapshot, never that the figures changed, and since HF-1 (CR-206) its own sentence names `logos scan`, which records a current snapshot — the command `readingAction` named before the action line was removed. R1: exactly what the timestamp pair establishes and no more; R3",
    ),
    (
        "spa",
        "web/ui/src/copy/health.copy.ts",
        "n/a",
        6,
        0,
        "CONFORMANT — moved from `HealthView.tsx` (S-615): the three `READING_ABSENCE` sentences, the Gate's absent baseline (`baseline n/a; informational pass`, which replaces the band's former `no baseline` wording), `NO_APPLICABLE_CONSTRUCT` and the offender badge of an ADR-21 drop-out. R4: a named `n/a` and an informational pass, never a fabricated zero or a delta from an absent baseline",
    ),
    (
        "spa",
        "web/ui/src/copy/health.copy.ts",
        "no-production-scope",
        1,
        0,
        "CONFORMANT — moved from `HealthView.tsx` (S-615): the `READING_ABSENCE` key, whose sentence names no command (HF-1, CR-206, removed `readingAction`'s `noAction` arm). R3: no command changes the state — the conclusion `SignalAbsence::NoProductionScope` follows",
    ),
    (
        "spa",
        "web/ui/src/copy/health.copy.ts",
        "unindexed",
        1,
        0,
        "CONFORMANT — moved from `HealthView.tsx` (S-615): the `READING_ABSENCE` key, whose sentence names `logos index` since HF-1 (CR-206) folded `readingAction`'s arm into it. R3: may name it, because this condition is exactly \"nothing is indexed\"",
    ),
    (
        "spa",
        "web/ui/src/copy/health.copy.ts",
        "unscanned",
        1,
        0,
        "CONFORMANT — moved from `HealthView.tsx` (S-615): the `READING_ABSENCE` key, whose sentence names `logos scan` since HF-1 (CR-206) folded `readingAction`'s arm into it. R3: may name it, for the same reason",
    ),
    (
        "spa",
        "web/ui/src/copy/text.test.tsx",
        "no baseline",
        0,
        1,
        "NO PRODUCTION SITE — 1 test occurrence(s) asserting sites declared elsewhere in this table; enumerated, never truncated away",
    ),
    (
        "spa",
        "web/ui/src/views/analytics/CoverageView.test.tsx",
        "n/a",
        0,
        1,
        "NO PRODUCTION SITE — 1 test occurrence(s) asserting sites declared elsewhere in this table; enumerated, never truncated away",
    ),
    (
        "spa",
        "web/ui/src/views/analytics/CoverageView.tsx",
        "n/a",
        3,
        0,
        "CONFORMANT — an absent freshness percentage, an absent `HEAD`, and (S-617, 2026-10-08) the Per-file coverage widget's absent overall percentage. R4: never a shifted number, never a guessed 0% (FR-CV-05)",
    ),
    (
        "spa",
        "web/ui/src/views/analytics/FilesView.test.tsx",
        "n/a",
        0,
        4,
        "NO PRODUCTION SITE — 4 test occurrence(s) asserting sites declared elsewhere in this table; enumerated, never truncated away",
    ),
    (
        "spa",
        "web/ui/src/views/analytics/analyticsModel.test.ts",
        "n/a",
        0,
        4,
        "NO PRODUCTION SITE — 4 test occurrence(s) asserting sites declared elsewhere in this table; enumerated, never truncated away",
    ),
    (
        "spa",
        "web/ui/src/views/analytics/analyticsModel.ts",
        "n/a",
        1,
        0,
        "CONFORMANT — the `NA` constant, the SPA's one spelling of the sentinel (R5 within this surface)",
    ),
    (
        "spa",
        "web/ui/src/views/config/ConfigView.tsx",
        "n/a",
        1,
        0,
        "CONFORMANT — an apply outcome carrying no signal. R4",
    ),
    (
        "spa",
        "web/ui/src/views/dashboard/DashboardView.test.tsx",
        "empty graph",
        0,
        1,
        "NO PRODUCTION SITE — 1 test occurrence(s) asserting sites declared elsewhere in this table; enumerated, never truncated away",
    ),
    (
        "spa",
        "web/ui/src/views/dashboard/DashboardView.test.tsx",
        "no rules contract",
        0,
        1,
        "NO PRODUCTION SITE — 1 test occurrence(s) asserting sites declared elsewhere in this table; enumerated, never truncated away",
    ),
    (
        "spa",
        "web/ui/src/views/dashboard/DashboardView.test.tsx",
        "unindexed",
        0,
        1,
        "NO PRODUCTION SITE — 1 test occurrence(s) asserting sites declared elsewhere in this table; enumerated, never truncated away",
    ),
    (
        "spa",
        "web/ui/src/views/dashboard/dashboardModel.test.ts",
        "at an unknown age",
        0,
        3,
        "NO PRODUCTION SITE — 3 test occurrence(s) asserting sites declared elsewhere in this table; enumerated, never truncated away",
    ),
    (
        "spa",
        "web/ui/src/views/dashboard/dashboardModel.ts",
        "at an unknown age",
        2,
        0,
        "CONFORMANT — S-433's two degradations, adopted from the CLI's wording rather than invented. The cross-surface twin of `readout.rs`'s pair, pinned byte-identical below",
    ),
    (
        "spa",
        "web/ui/src/views/graph/GraphView.test.tsx",
        "unindexed",
        0,
        1,
        "NO PRODUCTION SITE — 1 test occurrence(s) asserting sites declared elsewhere in this table; enumerated, never truncated away",
    ),
    (
        "spa",
        "web/ui/src/views/health/HealthView.test.tsx",
        "at an unknown age",
        0,
        1,
        "NO PRODUCTION SITE — 1 test occurrence(s) asserting sites declared elsewhere in this table; enumerated, never truncated away",
    ),
    (
        "spa",
        "web/ui/src/views/health/HealthView.test.tsx",
        "de-indexed",
        0,
        1,
        "NO PRODUCTION SITE — 1 test occurrence(s) asserting sites declared elsewhere in this table; enumerated, never truncated away",
    ),
    (
        "spa",
        "web/ui/src/views/health/HealthView.test.tsx",
        "empty graph",
        0,
        2,
        "NO PRODUCTION SITE — 2 test occurrence(s) asserting sites declared elsewhere in this table; enumerated, never truncated away",
    ),
    (
        "spa",
        "web/ui/src/views/health/HealthView.test.tsx",
        "indeterminate",
        0,
        3,
        "NO PRODUCTION SITE — 3 test occurrence(s) asserting sites declared elsewhere in this table; enumerated, never truncated away",
    ),
    (
        "spa",
        "web/ui/src/views/health/HealthView.test.tsx",
        "n/a",
        0,
        7,
        "NO PRODUCTION SITE — 7 test occurrence(s) asserting sites declared elsewhere in this table; enumerated, never truncated away",
    ),
    (
        "spa",
        "web/ui/src/views/health/HealthView.test.tsx",
        "no baseline",
        0,
        1,
        "NO PRODUCTION SITE — 1 test occurrence(s) asserting sites declared elsewhere in this table; enumerated, never truncated away",
    ),
    (
        "spa",
        "web/ui/src/views/health/HealthView.test.tsx",
        "no-production-scope",
        0,
        2,
        "NO PRODUCTION SITE — 2 test occurrence(s) asserting sites declared elsewhere in this table; enumerated, never truncated away",
    ),
    (
        "spa",
        "web/ui/src/views/health/HealthView.test.tsx",
        "unindexed",
        0,
        1,
        "NO PRODUCTION SITE — 1 test occurrence(s) asserting sites declared elsewhere in this table; enumerated, never truncated away",
    ),
    (
        "spa",
        "web/ui/src/views/health/HealthView.test.tsx",
        "unscanned",
        0,
        2,
        "NO PRODUCTION SITE — 2 test occurrence(s) asserting sites declared elsewhere in this table; enumerated, never truncated away",
    ),
    (
        "spa",
        "web/ui/src/views/health/HealthView.tsx",
        "indeterminate",
        1,
        0,
        "CONFORMANT — the stale-tone switch only (S-615 moved the sentence and the action to `health.copy.ts`): `indeterminate` renders muted, because nothing is established. R1, R4",
    ),
    (
        "spa",
        "web/ui/src/views/health/HealthView.tsx",
        "moved-past",
        1,
        0,
        "CONFORMANT — the stale-tone switch only (S-615 moved the sentence and the action to `health.copy.ts`): the same caution tone as `de-indexed`, never a staleness proof. R1",
    ),
    (
        "spa",
        "web/ui/src/views/health/HealthView.tsx",
        "n/a",
        3,
        0,
        "CONFORMANT — the dimension table's three muted `n/a` badges for an ADR-21 drop-out (score, normalized, raw). R4: a muted `n/a`, never a fabricated zero",
    ),
    (
        "spa",
        "web/ui/src/views/health/healthModel.test.ts",
        "at an unknown age",
        0,
        1,
        "NO PRODUCTION SITE — 1 test occurrence(s) asserting sites declared elsewhere in this table; enumerated, never truncated away",
    ),
    (
        "spa",
        "web/ui/src/views/health/healthModel.test.ts",
        "de-indexed",
        0,
        4,
        "NO PRODUCTION SITE — 4 test occurrence(s) asserting sites declared elsewhere in this table; enumerated, never truncated away",
    ),
    (
        "spa",
        "web/ui/src/views/health/healthModel.test.ts",
        "indeterminate",
        0,
        11,
        "NO PRODUCTION SITE — 11 test occurrence(s) asserting sites declared elsewhere in this table; enumerated, never truncated away",
    ),
    (
        "spa",
        "web/ui/src/views/health/healthModel.test.ts",
        "moved-past",
        0,
        8,
        "NO PRODUCTION SITE — 8 test occurrence(s) asserting sites declared elsewhere in this table; enumerated, never truncated away",
    ),
    (
        "spa",
        "web/ui/src/views/health/healthModel.test.ts",
        "n/a",
        0,
        6,
        "NO PRODUCTION SITE — 6 test occurrence(s) asserting sites declared elsewhere in this table; enumerated, never truncated away",
    ),
    (
        "spa",
        "web/ui/src/views/health/healthModel.test.ts",
        "no baseline",
        0,
        2,
        "NO PRODUCTION SITE — 2 test occurrence(s) asserting sites declared elsewhere in this table; enumerated, never truncated away",
    ),
    (
        "spa",
        "web/ui/src/views/health/healthModel.ts",
        "indeterminate",
        9,
        0,
        "CONFORMANT — `SnapshotCurrency`'s third arm and the three sentences naming the missing fact. R1, R4; `date: null` by type, so no date can reach the renderer",
    ),
    (
        "spa",
        "web/ui/src/views/health/healthModel.ts",
        "moved-past",
        2,
        0,
        "CONFORMANT — the arm and its discriminant. The known over-report is recorded in code with its direction and its reason (NFR-RA-05)",
    ),
    (
        "spa",
        "web/ui/src/views/health/healthModel.ts",
        "n/a",
        1,
        0,
        "CONFORMANT — `optSignal`'s honest sentinel. R1: no cause is named, because this seam cannot tell the causes apart and says so",
    ),
    (
        "spa",
        "web/ui/src/views/health/healthModel.ts",
        "no-production-scope",
        2,
        0,
        "CONFORMANT — THE REFERENCE MODEL, asserted unchanged. Settled by CR-135 §3.3, re-confirmed under S-422's story review, and pinned as source text by `the_reference_model_is_unchanged`, its behaviour by its own unmodified suite",
    ),
    (
        "spa",
        "web/ui/src/views/health/healthModel.ts",
        "unindexed",
        2,
        0,
        "CONFORMANT — THE REFERENCE MODEL, asserted unchanged (see the `no-production-scope` row)",
    ),
    (
        "spa",
        "web/ui/src/views/health/healthModel.ts",
        "unscanned",
        2,
        0,
        "CONFORMANT — THE REFERENCE MODEL, asserted unchanged. Its middle arm is the one a persisted snapshot can reach and a computing readout cannot, which is R0 in one line",
    ),
    (
        "spa",
        "web/ui/src/views/statistics/StatisticsView.test.tsx",
        "none recorded",
        0,
        3,
        "NO PRODUCTION SITE — 3 test occurrence(s) asserting sites declared elsewhere in this table; enumerated, never truncated away",
    ),
    (
        "spa",
        "web/ui/src/views/statistics/WorkspaceStatisticsView.test.tsx",
        "none recorded",
        0,
        1,
        "NO PRODUCTION SITE — 1 test occurrence(s) asserting sites declared elsewhere in this table; enumerated, never truncated away",
    ),
    (
        "spa",
        "web/ui/src/views/statistics/statsModel.test.ts",
        "none recorded",
        0,
        4,
        "NO PRODUCTION SITE — 4 test occurrence(s) asserting sites declared elsewhere in this table; enumerated, never truncated away",
    ),
    (
        "spa",
        "web/ui/src/views/workspace/WorkspaceView.tsx",
        "n/a",
        1,
        0,
        "CONFORMANT — a cross-service entry with no file to name. R4: `n/a`, never a fabricated path; the surrounding empty states already distinguish \"unknown\" from \"absent\"",
    ),
];

/// The workspace root — this crate's parent, which is where `cli/` and
/// `web/ui/` live. Asserted rather than assumed: a walk rooted one directory
/// wrong reports an empty census, which would compare equal to an empty table
/// and mean nothing.
fn workspace_root() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("logos-core has a parent")
        .to_path_buf();
    for (_, surface) in SURFACES {
        assert!(
            root.join(surface).is_dir(),
            "{} is not under {} — the walk would silently scan nothing",
            surface,
            root.display()
        );
    }
    root
}

/// One source, comment-blanked, with a per-byte map of what sits inside a
/// string literal.
///
/// Two consumers need two different views of the same bytes and they must agree
/// on offsets, so one pass produces both:
/// - `code` — comments blanked, **string bodies intact**, every line kept. The
///   text a user reads lives in string literals and JSX text nodes, so the
///   sentinel scan needs them.
/// - `in_string` — one entry per **byte** of `code`. The brace walk needs the
///   opposite view: a `{` inside a string literal is text, not a block.
///
/// Keeping them the same length is the whole point. An earlier version returned
/// only `code`, lower-cased it for matching, and compared the resulting offsets
/// against spans computed on the un-lowered string — two coordinate systems that
/// agree only while every character folds to its own byte length.
struct Stripped {
    code: String,
    in_string: Vec<bool>,
}

impl Stripped {
    fn push(&mut self, c: char, in_string: bool) {
        self.code.push(c);
        self.in_string.resize(self.code.len(), in_string);
    }
}

/// Comments blanked, **every line kept**, and string literals left intact.
///
/// Prose *about* an absence is not an absence site, so comments go; the text a
/// user actually reads lives in string literals and JSX text nodes, so those
/// stay. Nothing is truncated: the test module is blanked nowhere, and
/// [`rust_test_spans`] classifies it instead.
///
/// # Five things this gets right that the obvious version does not
///
/// **`//` is only a comment outside a string.** `line.split_once("//")` is
/// quote-blind, and a line carrying a URL — `let u = "https://x"; f("n/a")` —
/// truncates at the URL's own slashes and loses the site after it. That is a
/// *silent miss*, the one direction a census must never fail in. Reproduced by
/// [S-435]'s review on the sibling harness before it was fixed there.
///
/// **`'` is a string delimiter in TypeScript and a lifetime in Rust.** Treating
/// it as a quote in Rust makes `&'a str` open a string that never closes, after
/// which every `//` on the rest of the file is kept and doc prose is reported
/// as a site. The first draft of this scanner did exactly that and booked a
/// rustdoc line in `models/quality.rs` as a rendering. So the delimiter set is
/// per language.
///
/// **A backtick template literal legally spans lines; `"` and `'` do not.** So
/// quote state resets at each newline for the per-line delimiters — which is
/// what bounds the damage of an unbalanced apostrophe in JSX prose to one line —
/// and is carried across lines for a backtick and for a `/* */` block. Review
/// reproduced the miss: a template literal whose second line carried a URL was
/// truncated at that URL and the sentinel after it vanished.
///
/// **A Rust raw string is not a `"`-delimited string.** `r#"a "quote here"#`
/// contains an odd number of `"`, so a `"`-counting scanner leaves the rest of
/// the line marked as string — which now also corrupts `in_string` and can move
/// a brace out of the block walk. Raw strings are matched by their hash count,
/// and they may span lines.
///
/// **An `r` that ends an identifier opens nothing.** A Rust `"…"` string may
/// continue onto the next line after a `\`, and this scanner resets `"` state
/// at each newline, so a continuation line is read as code — and one ending
/// `…answer",` put `r` before `"`, which the raw-string check took for `r"`.
/// Everything to the next `"` was then read as raw-string code, comments
/// included. [S-442]'s first run of this audit found it: two `//` comments in
/// `navigate/mod.rs`, forty lines below such a continuation, were booked as a
/// production `n/a` and `unindexed`. A raw string opens only where `r` starts
/// a token, or follows a `b`/`c` prefix that does — so byte and C raw strings
/// (`br"…"`, `cr#"…"#`) still open, which the first version of this fix broke.
///
/// [S-435]: ../../docs/planning/journal.md#s-435-the-wiki-generation-pass-names-its-own-surface
/// [S-442]: ../../docs/planning/journal.md#s-442-a-relational-answer-states-the-resolution-denominator-it-was-computed-over
fn strip_comments(source: &str, rust: bool) -> Stripped {
    /// What the scanner is in the middle of, between characters.
    enum State {
        Code,
        /// A `"`/`'` string, which ends at its delimiter or at the newline.
        Line(char),
        /// A backtick template literal, which survives newlines.
        Template,
        /// A Rust raw string, closed by `"` followed by this many `#`.
        Raw(usize),
        Block,
    }
    let per_line: &[char] = if rust { &['"'] } else { &['"', '\''] };
    let mut out = Stripped {
        code: String::with_capacity(source.len()),
        in_string: Vec::with_capacity(source.len()),
    };
    let mut state = State::Code;
    for (n, line) in source.lines().enumerate() {
        if n > 0 {
            out.push('\n', false);
        }
        // A `"`/`'` string cannot cross a newline in either language; a template
        // literal and a block comment can.
        if matches!(state, State::Line(_)) {
            state = State::Code;
        }
        let mut chars = line.chars().peekable();
        // The two characters before `c` on this line, for the raw-string
        // opener's token boundary: `r` opens one at a token start, or after a
        // `b`/`c` prefix that is itself at a token start (`br"…"`, `cr#"…"#`).
        let (mut prev2, mut prev) = (' ', ' ');
        while let Some(c) = chars.next() {
            let at_boundary = |ch: char| !(ch.is_alphanumeric() || ch == '_');
            let starts_token =
                at_boundary(prev) || (matches!(prev, 'b' | 'c') && at_boundary(prev2));
            (prev2, prev) = (prev, c);
            match state {
                State::Block => {
                    if c == '*' && chars.peek() == Some(&'/') {
                        chars.next();
                        state = State::Code;
                        out.push(' ', false);
                        out.push(' ', false);
                    } else {
                        out.push(' ', false);
                    }
                }
                State::Raw(hashes) => {
                    out.push(c, true);
                    if c == '"' {
                        let mut seen = 0;
                        while seen < hashes && chars.peek() == Some(&'#') {
                            chars.next();
                            out.push('#', true);
                            seen += 1;
                        }
                        if seen == hashes {
                            state = State::Code;
                        }
                    }
                }
                State::Line(delimiter) => {
                    out.push(c, true);
                    if c == '\\' {
                        if let Some(escaped) = chars.next() {
                            out.push(escaped, true);
                        }
                    } else if c == delimiter {
                        state = State::Code;
                    }
                }
                State::Template => {
                    out.push(c, true);
                    if c == '\\' {
                        if let Some(escaped) = chars.next() {
                            out.push(escaped, true);
                        }
                    } else if c == '`' {
                        state = State::Code;
                    }
                }
                State::Code => {
                    // A raw string opener, before `r` could be read as a letter.
                    if rust && starts_token && c == 'r' && matches!(chars.peek(), Some('"' | '#')) {
                        let mut lookahead = chars.clone();
                        let mut hashes = 0usize;
                        while lookahead.peek() == Some(&'#') {
                            lookahead.next();
                            hashes += 1;
                        }
                        if lookahead.peek() == Some(&'"') {
                            out.push(c, false);
                            for _ in 0..hashes {
                                chars.next();
                                out.push('#', false);
                            }
                            chars.next();
                            out.push('"', false);
                            state = State::Raw(hashes);
                            continue;
                        }
                    }
                    if per_line.contains(&c) {
                        state = State::Line(c);
                        out.push(c, false);
                    } else if !rust && c == '`' {
                        state = State::Template;
                        out.push(c, false);
                    } else if c == '/' && chars.peek() == Some(&'/') {
                        break;
                    } else if c == '/' && chars.peek() == Some(&'*') {
                        chars.next();
                        state = State::Block;
                        out.push(' ', false);
                        out.push(' ', false);
                    } else {
                        out.push(c, false);
                    }
                }
            }
        }
    }
    out
}

/// The byte just past each `#[cfg(...)]` attribute whose predicate gates on
/// **`test`**, in a comment-stripped Rust source.
///
/// Matching the literal string `#[cfg(test)]` is not enough, and the audit's
/// own scope extension is what proved it: `logos-core/src/pipeline/mod.rs`
/// declares its test module as `#[cfg(all(test, feature = "lang-rust"))]`, so a
/// literal match missed it and every assertion in `pipeline/tests.rs` entered
/// the census as a **production** absence site. Compound predicates are
/// ordinary Rust and this walks any of them.
///
/// The `test` token is required to be a whole token **outside a string**, so
/// `feature = "test-utils"` does not gate a module on `test` and is not read as
/// if it did.
fn cfg_test_attributes(stripped: &Stripped) -> Vec<usize> {
    const OPEN: &str = "#[cfg(";
    let code = &stripped.code;
    let mut ends = Vec::new();
    for (at, _) in code.match_indices(OPEN) {
        let predicate_start = at + OPEN.len();
        let mut depth = 1usize;
        let mut close = None;
        for (offset, c) in code[predicate_start..].char_indices() {
            if stripped.in_string[predicate_start + offset] {
                continue;
            }
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(predicate_start + offset);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(close) = close else { continue };
        if code[close..].chars().nth(1) != Some(']') {
            continue;
        }
        let gates_on_test = code[predicate_start..close]
            .match_indices("test")
            .any(|(offset, _)| {
                !stripped.in_string[predicate_start + offset]
                    && whole_token(code, predicate_start + offset, predicate_start + offset + 4)
            });
        if gates_on_test {
            ends.push(close + 2);
        }
    }
    ends
}

/// Byte spans of the `#[cfg(test)] mod … { … }` blocks in a comment-stripped
/// Rust source, brace-matched **outside string literals**.
///
/// **Spans, never a truncation point.** Matching the closing brace is what
/// makes a site *after* a test module production code again;
/// [`a_new_site_is_detected_even_after_a_test_module`] proves it on a fixture
/// shaped exactly like the Sprint 72 defect.
///
/// **Braces inside string literals are text.** [`strip_comments`] deliberately
/// keeps string bodies — the rendered text lives there — so a walk that counted
/// every `{` would take `assert_eq!("{", "{")` for an opened block. Review
/// reproduced all three consequences on the real tree: a lone `{` in a test
/// module made the walk run off the end and abort the whole audit with an
/// "unbalanced braces" panic that blamed a perfectly balanced file; a lone `}`
/// ended the span early and re-reported thirty test assertions in `readout.rs`
/// as production sites; and a `{` in one test module beside a `}` in the next
/// **silently** swallowed the production code between them. That last is the
/// Sprint 72 §5.9 failure class arriving by a different door. So the walk reads
/// `in_string` and skips those bytes.
///
/// A `#[cfg(test)] mod tests;` **declaration** carries no block, and reading the
/// next `{` in the file as its body would classify hundreds of lines of
/// production code as test scope. The first draft of this function did that to
/// `governance/mod.rs` and hid a real site. The guard is that what sits between
/// the attribute and the `{` must be exactly `mod <identifier>`, which a
/// declaration never is — its `;` falls inside that span, and so does every line
/// between it and whatever brace comes next.
///
/// A separate `;`-before-`{` check stood here first. The falsifiability sweep
/// removed it and every test stayed green: the head check already rejects
/// `"tests;"` as a name, so the two conditions were one, and the spare was a
/// dead conjunct of exactly the kind `recorded_clean_over` records shipping.
fn rust_test_spans(stripped: &Stripped) -> Vec<(usize, usize)> {
    let code = &stripped.code;
    let mut spans = Vec::new();
    for after in cfg_test_attributes(stripped) {
        let tail = &code[after..];
        let Some(offset) = tail
            .char_indices()
            .find(|&(i, c)| c == '{' && !stripped.in_string[after + i])
            .map(|(i, _)| i)
        else {
            continue;
        };
        let head = tail[..offset].trim();
        let Some(name) = head.strip_prefix("mod ") else {
            continue;
        };
        let name = name.trim();
        // `head` is trimmed, so a successful `mod ` strip always leaves a
        // non-empty remainder — an emptiness check here would be a dead
        // conjunct, which review confirmed by removing it over the whole tree
        // and over thirteen adversarial heads with no change in outcome.
        if !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
            continue;
        }
        let open = after + offset;
        let mut depth = 0usize;
        let mut end = open;
        for (offset, c) in code[open..].char_indices() {
            if stripped.in_string[open + offset] {
                continue;
            }
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = open + offset + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        assert!(
            end > open,
            "the scanner could not brace-match the `#[cfg(test)] mod` block at \
             byte {open}: the walk reached the end of the file with {depth} \
             brace(s) still open. This is a scanner limitation, not a claim \
             about the source — report it rather than reformatting the file"
        );
        spans.push((open, end));
    }
    spans
}

/// Is this `.rs` file test-only — declared by its parent under
/// `#[cfg(test)] mod <stem>;`?
///
/// Read from the declaration rather than from the filename, so `tests.rs` is
/// test scope because the module tree says so and not because of what it is
/// called. The occurrences are still enumerated; only their column changes.
///
/// The declaration may carry a visibility (`pub mod helpers;` is an ordinary
/// test-only helper module), so one is stripped before the comparison; matching
/// `mod <stem>` exactly missed those and put a test module's occurrences in the
/// production census, where the citation check would then demand a taxonomy
/// reference from it.
///
/// The sibling-file candidate is built by **appending** `.rs` to the directory
/// name rather than by `Path::with_extension`, which replaces the last
/// dot-suffix: for `b.v2/inner.rs` that produced `b.rs`, an unrelated file, and
/// if it happened to declare a module of the same name the production file
/// vanished from the census silently.
fn rust_file_is_test_only(path: &Path) -> bool {
    let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
        return false;
    };
    let Some(parent) = path.parent() else {
        return false;
    };
    let mut candidates = vec![parent.join("mod.rs")];
    if let Some(directory) = parent.file_name().and_then(|n| n.to_str()) {
        candidates.push(parent.with_file_name(format!("{directory}.rs")));
    }
    for declarer in candidates {
        if declarer == path || !declarer.is_file() {
            continue;
        }
        let Ok(source) = std::fs::read_to_string(&declarer) else {
            continue;
        };
        let stripped = strip_comments(&source, true);
        let code = &stripped.code;
        for after in cfg_test_attributes(&stripped) {
            let tail = &code[after..];
            // An inline `mod x { … }` needs no guard of its own: its first `;`
            // lies inside the block, so the slice up to it carries a `{` and
            // cannot equal `mod <stem>`. A separate brace check stood here and
            // review proved it dead over the whole tree and over seven
            // adversarial tails — the same spare-conjunct shape the sweep had
            // already removed from `rust_test_spans`, left standing in its
            // sibling sixty lines below.
            let Some(end) = tail.find(';') else { continue };
            let declaration = tail[..end].trim();
            let declaration = declaration
                .strip_prefix("pub(crate)")
                .or_else(|| declaration.strip_prefix("pub"))
                .unwrap_or(declaration)
                .trim_start();
            if declaration == format!("mod {stem}") {
                return true;
            }
        }
    }
    false
}

/// Is the span `start..end` a whole token, or part of a longer word?
///
/// `n/a` inside `en/africa` is not the sentinel, and `unindexed` inside
/// `reunindexed` is not either. `-` and `/` are deliberately **not** word
/// characters: `no-production-scope` and `n/a` contain them, so treating them
/// as boundaries is what lets the sentinels match at all.
fn whole_token(code: &str, start: usize, end: usize) -> bool {
    let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
    !word(code[..start].chars().next_back()) && !word(code[end..].chars().next())
}

/// Match `sentinel` at `at`, tolerating any run of whitespace where it has a
/// single space; the end offset, or `None`.
///
/// Nine of the twenty sentinels contain a space, and both languages on these
/// surfaces routinely split a rendered sentence across source lines — Rust with
/// a `\` continuation inside a `format!`, TSX with a formatter wrapping a JSX
/// text node. `readout.rs` and `governance/mod.rs` are written almost entirely
/// in continued strings. A plain substring match requires exactly one space and
/// so **loses the site**, which is the silent direction; review reproduced it on
/// `"violations none \` + newline + `recorded (…)"`, a string rustc renders with
/// one space.
///
/// A `\` is consumed as whitespace for the same reason: it is the continuation
/// marker, and what it joins is one rendered sentence.
fn match_sentinel_at(code: &str, at: usize, sentinel: &str) -> Option<usize> {
    let mut cursor = at;
    for want in sentinel.chars() {
        if want == ' ' {
            let consumed: usize = code[cursor..]
                .chars()
                .take_while(|c| c.is_whitespace() || *c == '\\')
                .map(char::len_utf8)
                .sum();
            if consumed == 0 {
                return None;
            }
            cursor += consumed;
            continue;
        }
        let got = code[cursor..].chars().next()?;
        if got != want {
            return None;
        }
        cursor += got.len_utf8();
    }
    Some(cursor)
}

/// Every sentinel occurrence in one comment-stripped source, as
/// `(start, end, sentinel)`, in `SENTINELS` order.
///
/// **One matcher, and it is the one the audit runs.** This body used to be
/// written out five times — once in `occurrences`, twice in the near-miss probe,
/// once in the fixture test and once in the taxonomy-module check. The probe
/// therefore exercised its own copy, so a change made here was covered by no
/// near-miss case at all while the test's doc claimed it probed "the matcher".
///
/// Folding is `to_ascii_lowercase`, never `to_lowercase`. The lexicon is pure
/// ASCII so nothing is lost, and ASCII folding is **length-preserving** — which
/// is what keeps these offsets in the same coordinate system as
/// [`rust_test_spans`]'s. `to_lowercase` is not: a Kelvin sign folds 3 bytes to
/// 1 and a dotted capital I folds 2 to 3, and review reproduced both directions
/// moving a site across a span boundary.
fn sentinel_hits(code: &str) -> Vec<(usize, usize, &'static str)> {
    let lower = code.to_ascii_lowercase();
    debug_assert_eq!(lower.len(), code.len(), "ASCII folding preserves length");
    let mut hits = Vec::new();
    for sentinel in absence::SENTINELS {
        let first = sentinel.split(' ').next().expect("a non-empty sentinel");
        for (at, _) in lower.match_indices(first) {
            let Some(end) = match_sentinel_at(&lower, at, sentinel) else {
                continue;
            };
            if whole_token(&lower, at, end) {
                hits.push((at, end, *sentinel));
            }
        }
    }
    hits
}

/// Every sentinel occurrence in one file, as `(sentinel, in test scope)`.
fn occurrences(path: &Path) -> Vec<(&'static str, bool)> {
    let source = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    let rust = path.extension().and_then(|e| e.to_str()) == Some("rs");
    let stripped = strip_comments(&source, rust);
    let whole_file_is_test = if rust {
        rust_file_is_test_only(path)
    } else {
        path.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.contains(".test."))
    };
    let spans = if whole_file_is_test || !rust {
        Vec::new()
    } else {
        rust_test_spans(&stripped)
    };
    sentinel_hits(&stripped.code)
        .into_iter()
        .map(|(at, _, sentinel)| {
            let in_test =
                whole_file_is_test || spans.iter().any(|&(start, end)| at >= start && at < end);
            (sentinel, in_test)
        })
        .collect()
}

/// Source extensions the walk reads. Every other extension under a surface is
/// **refused**, never skipped — see [`sources`].
const WALKED: [&str; 3] = ["rs", "ts", "tsx"];

/// Extensions a bundler on these surfaces would happily build, and which the
/// walk does not read.
///
/// Encountering one is a hard failure rather than a silent skip. Review proved
/// why: a `.mts` file placed under `web/ui/src/views/health/` carrying three
/// unadjudicated sentinels — including a verbatim reintroduction of the wording
/// this audit removed — left the whole suite green. Vite resolves `.mts`,
/// `.mjs`, `.jsx` and `.cjs` by default, so such a file ships. Refusing is what
/// turns "the walk did not look there" into a decision someone has to make.
const REFUSED: [&str; 8] = ["mts", "cts", "mjs", "cjs", "js", "jsx", "svelte", "vue"];

/// Every walked source under `dir`, recursively and in sorted order.
///
/// Symlinked directories are not followed. `docs/` in this repository is a
/// directory symlink, so the pattern is in the house style, and a cycle would
/// make this loop forever with no panic and no timeout — a hang is the least
/// diagnosable failure a census can have.
fn sources(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(next) = stack.pop() {
        let entries = std::fs::read_dir(&next)
            .unwrap_or_else(|e| panic!("reading {}: {e}", next.display()));
        for entry in entries {
            let path = entry
                .unwrap_or_else(|e| panic!("reading an entry of {}: {e}", next.display()))
                .path();
            let kind = std::fs::symlink_metadata(&path)
                .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
            if kind.file_type().is_symlink() {
                continue;
            }
            if kind.is_dir() {
                stack.push(path);
                continue;
            }
            let extension = path.extension().and_then(|e| e.to_str()).unwrap_or_default();
            assert!(
                !REFUSED.contains(&extension),
                "{} is a source file on an audited surface that the walk does \
                 not read. Add its extension to WALKED and adjudicate its \
                 sites, or say in SURFACES why the surface excludes it — a \
                 file the census cannot see is the one place an absence can \
                 hide",
                path.display()
            );
            if WALKED.contains(&extension) {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// The walk, as census rows: `(surface, file, sentinel, production, test)`.
fn walk() -> Vec<(String, String, String, usize, usize)> {
    let root = workspace_root();
    let mut rows: Vec<(String, String, String, usize, usize)> = Vec::new();
    for (surface, dir) in SURFACES {
        for path in sources(&root.join(dir)) {
            let relative = path
                .strip_prefix(&root)
                .expect("under the workspace root")
                .to_string_lossy()
                .replace('\\', "/");
            if relative == LEXICON_HOME {
                continue;
            }
            let mut per_sentinel: Vec<(&str, usize, usize)> = Vec::new();
            for (sentinel, in_test) in occurrences(&path) {
                match per_sentinel.iter_mut().find(|(s, _, _)| *s == sentinel) {
                    Some(entry) => {
                        if in_test {
                            entry.2 += 1;
                        } else {
                            entry.1 += 1;
                        }
                    }
                    None => per_sentinel.push((
                        sentinel,
                        usize::from(!in_test),
                        usize::from(in_test),
                    )),
                }
            }
            per_sentinel.sort_by_key(|(s, _, _)| *s);
            for (sentinel, production, test) in per_sentinel {
                rows.push((
                    surface.to_string(),
                    relative.clone(),
                    sentinel.to_string(),
                    production,
                    test,
                ));
            }
        }
    }
    rows
}

// ── the matcher, probed on the side deletions cannot reach ──────────────────

/// **The matcher, run against what it must ADMIT and what it must REFUSE.**
///
/// Every case here was written from a near miss rather than from the rule, and
/// two of them are defects this scanner actually had. A mutation sweep cannot
/// find either: deleting part of a matcher can only shrink what it accepts, so
/// nothing a sweep removes makes a wrongly-admitted case start failing.
#[test]
fn the_matcher_rejects_its_near_misses() {
    // `sentinel_hits` is the function `occurrences` runs, not a copy of it.
    // The earlier version of this test re-implemented the scan inline and so
    // probed its own duplicate: a change to the real matcher was covered by no
    // near-miss case, while this doc claimed it probed "the matcher".
    let sites = |src: &str, rust: bool| -> Vec<&'static str> {
        sentinel_hits(&strip_comments(src, rust).code)
            .into_iter()
            .map(|(_, _, sentinel)| sentinel)
            .collect()
    };
    let rust_sites = |src: &str| sites(src, true);
    let ts_sites = |src: &str| sites(src, false);

    // ADMIT: a rendered sentinel, in a string literal and in a JSX text node.
    assert_eq!(rust_sites(r#"fn f() { "n/a (empty graph)" }"#).len(), 2);
    assert_eq!(ts_sites("const c = <>n/a — nothing indexed yet</>;"), ["n/a"]);

    // REFUSE: prose about a sentinel is not a site, in either comment syntax.
    assert!(
        rust_sites("/// renders `n/a` when absent\nfn f() {}").is_empty(),
        "a rustdoc line naming the sentinel is prose"
    );
    assert!(
        ts_sites("/**\n * renders n/a, never a zero\n */\nconst x = 1;").is_empty(),
        "a JSDoc block spanning lines is prose — and it is the shape this \
         surface's files are written in, so failing to strip it would report \
         every doc paragraph as a site"
    );

    // REFUSE: a sentinel inside a longer word.
    assert!(
        ts_sites(r#"const s = "en/africa"; const t = "reunindexed";"#).is_empty(),
        "`n/a` inside `en/africa` and `unindexed` inside `reunindexed` are not \
         the sentinels — the LEFT boundary"
    );
    assert!(
        ts_sites(r#"const s = "n/african"; const t = "unindexedly";"#).is_empty(),
        "…and the RIGHT boundary, which no left-boundary case reaches: both of \
         these start cleanly at a quote and run on into a longer word"
    );

    // REFUSE: `unknown` in a TypeScript type position. The lexicon carries the
    // phrase `at an unknown age`, never the bare word, precisely because
    // `Record<string, unknown>` is ordinary TypeScript and appears everywhere.
    assert!(
        ts_sites("function g(v: unknown): Record<string, unknown> { return {}; }").is_empty(),
        "an `unknown` type annotation reports no figure and is not a site"
    );

    // ADMIT, and this one was a silent miss before it was fixed: a `//` inside
    // a string literal is not a comment start. Quote-blind stripping truncates
    // the line at the URL and loses every site after it.
    assert_eq!(
        ts_sites(r#"const u = "https://example.com"; const v = "n/a";"#),
        ["n/a"],
        "a URL's `//` inside a string must not truncate the line"
    );

    // ADMIT, and this one review found: a backtick template literal legally
    // spans lines, so per-line quote state truncated the continuation at the
    // URL's own slashes and lost the sentinel after it — a silent miss.
    assert_eq!(
        ts_sites("const msg = `see\nhttps://x.example and n/a`;"),
        ["n/a"],
        "a template literal carries its string state across the newline"
    );

    // REFUSE, and this one too: a Rust raw string holds an odd number of `\"`,
    // so a quote-counting scanner leaves the rest of the line marked as string
    // — which both admits comment prose and, since the mask feeds the brace
    // walk, can move a brace out of a test block.
    assert!(
        rust_sites("const S: &str = r#\"a \"quote here\"#; // renders n/a").is_empty(),
        "a raw string is closed by its hash count, not by its next quote"
    );

    // ADMIT, and this one this scanner got wrong: `'` is a lifetime in Rust,
    // not a quote. Treating it as one opens a string that never closes, after
    // which no `//` is stripped and doc prose is reported as a rendering.
    // The apostrophe count on the line is what matters: quote state is per
    // line, so an even number closes by accident and proves nothing. One
    // lifetime and a trailing comment is the shape that actually reproduced.
    assert!(
        rust_sites("fn f<'a>(x: &str) -> &str { x } // renders `n/a` when absent").is_empty(),
        "a lifetime must not open a string literal — with `'` as a delimiter \
         the string never closes, the trailing `//` is never stripped, and the \
         comment is booked as a rendering. The first draft did this to a \
         rustdoc line in models/quality.rs"
    );
    // …while `'` IS a string delimiter in TypeScript, so the same protection
    // against `//` applies there.
    assert_eq!(
        ts_sites(r#"const u = 'https://example.com'; const v = 'n/a';"#),
        ["n/a"],
        "single quotes delimit strings in TypeScript"
    );
}

/// **A source file the walk does not read is refused, never skipped.**
///
/// Review placed a `.mts` file under `web/ui/src/views/health/` carrying three
/// unadjudicated sentinels — one of them a verbatim reintroduction of the
/// wording this audit removed — and the whole suite stayed green. Vite resolves
/// `.mts`, `.mjs`, `.jsx` and `.cjs` by default, so such a file ships. A file
/// the census cannot see is the one place an absence can hide, so the walk
/// refuses rather than skips.
#[test]
fn a_source_extension_the_walk_does_not_read_is_refused() {
    let tmp = tempfile::TempDir::new().expect("temp root");
    std::fs::write(tmp.path().join("kept.ts"), "export const A = \"n/a\";\n").expect("write");
    assert_eq!(
        sources(tmp.path())
            .iter()
            .filter_map(|p| p.file_name().and_then(|n| n.to_str()))
            .collect::<Vec<_>>(),
        ["kept.ts"],
        "a walked extension is read, and a stylesheet beside it is not a source"
    );

    std::fs::write(tmp.path().join("hidden.mts"), "export const B = \"n/a\";\n").expect("write");
    let refused = std::panic::catch_unwind(|| sources(tmp.path()));
    assert!(
        refused.is_err(),
        "a `.mts` file on an audited surface must fail the walk loudly, not \
         drop out of the census in silence"
    );
}

/// **A test-only module keeps its scope when the declaration carries a
/// visibility, and a dotted directory name does not borrow another file's.**
///
/// `rust_file_is_test_only` reads the module tree rather than the filename.
/// Matching `mod <stem>` exactly missed `pub mod helpers;` and put a test
/// module's occurrences into the *production* census, where the citation check
/// would then demand a taxonomy reference from it. And `with_extension` turned
/// `b.v2/inner.rs` into a lookup of `b.rs` — an unrelated file, whose unrelated
/// declaration made a production file vanish from the census silently.
#[test]
fn a_test_module_is_recognised_by_its_declaration_not_its_name() {
    let tmp = tempfile::TempDir::new().expect("temp root");
    let root = tmp.path();

    std::fs::create_dir_all(root.join("a")).expect("mkdir");
    std::fs::write(
        root.join("a/mod.rs"),
        // `live_helper` is the near miss: a test-only module whose name has
        // `live` as a PREFIX. The comparison must be an equality, not a
        // containment — relaxing it to `contains` classified the production
        // `live.rs` as test scope and every site in it left the census in
        // silence. A deletion-only sweep cannot find that: it is about what the
        // matcher wrongly ADMITS.
        "#[cfg(test)]\npub mod helpers;\n\n#[cfg(test)]\nmod tests;\n\n\
         #[cfg(test)]\nmod live_helper;\n\npub mod live;\n",
    )
    .expect("write");
    for leaf in ["helpers", "tests", "live", "live_helper"] {
        std::fs::write(root.join(format!("a/{leaf}.rs")), "// fixture\n").expect("write");
    }

    // A dotted directory beside an unrelated `b.rs` that declares `inner`.
    std::fs::create_dir_all(root.join("b.v2")).expect("mkdir");
    std::fs::write(root.join("b.v2/inner.rs"), "// fixture\n").expect("write");
    std::fs::write(root.join("b.rs"), "#[cfg(test)]\nmod inner;\n").expect("write");

    assert_eq!(
        [
            rust_file_is_test_only(&root.join("a/helpers.rs")),
            rust_file_is_test_only(&root.join("a/tests.rs")),
            rust_file_is_test_only(&root.join("a/live.rs")),
            rust_file_is_test_only(&root.join("b.v2/inner.rs")),
        ],
        [true, true, false, false],
        "a `pub mod` declaration is still a test-only declaration; a production \
         module is not one even when a test-only sibling's name extends its \
         own; and `b.v2/inner.rs` is not declared by `b.rs`"
    );
}

/// **A brace inside a string literal is text, not a block.**
///
/// [`strip_comments`] keeps string bodies because the rendered text lives
/// there, so the brace walk has to be told which braces are code. Review
/// reproduced all three consequences of not telling it, and every one of them
/// arrives from ordinary Rust that carries no sentinel at all.
/// **An identifier ending in `r` before a quote opens no raw string** — the
/// near miss of the raw-string opener, one character from `r"`. The source is
/// the shape [S-442] tripped on: a `"` string continued onto a second line, read
/// as code, ending `answer"`, with a comment below it. That comment is prose;
/// the real raw string after it is still a site.
///
/// [S-442]: ../../docs/planning/journal.md#s-442-a-relational-answer-states-the-resolution-denominator-it-was-computed-over
#[test]
fn an_identifier_ending_in_r_before_a_quote_opens_no_raw_string() {
    let source = r#"fn f() -> String {
    format!("one \
     answer")
}
// n/a is prose in a comment
fn g() -> &'static str { r"unscanned" }
"#;
    let code = strip_comments(source, true).code;
    let hits: Vec<&str> = sentinel_hits(&code)
        .into_iter()
        .map(|(_, _, sentinel)| sentinel)
        .collect();
    assert_eq!(
        hits,
        ["unscanned"],
        "only the raw string's sentinel is a site; the comment below `answer\"` is not"
    );

    // The positive side, at every token boundary a raw string can follow —
    // punctuation, `=`, and a `b`/`c` prefix — and the negative side of the
    // prefix: `x_r"` is an identifier ending in `r`, not a raw string. Each
    // raw string holds a `"` that a plain string would close on, then a `//`
    // that a plain string's close would expose as a comment.
    for (source, expected) in [
        (r##"fn f() { g(r#"a" // unscanned"#) }"##, vec!["unscanned"]),
        (r##"fn f() { let s =r#"a" // unscanned"#; }"##, vec!["unscanned"]),
        (r##"fn f() -> &'static [u8] { br#"a" // unscanned"# }"##, vec!["unscanned"]),
        (r##"fn f() { let s = cr#"a" // unscanned"#; }"##, vec!["unscanned"]),
        // `_` is an identifier character: a continuation line ending `x_r"`
        // opens nothing, so the comment below it stays prose.
        (
            "fn f() -> String {\n    format!(\"one \\\n     x_r\")\n}\n// unscanned\nfn g() {}\n",
            vec![],
        ),
    ] {
        let code = strip_comments(source, true).code;
        let hits: Vec<&str> = sentinel_hits(&code)
            .into_iter()
            .map(|(_, _, sentinel)| sentinel)
            .collect();
        assert_eq!(hits, expected, "{source}");
    }
}

#[test]
fn a_brace_in_a_string_literal_is_not_a_block() {
    // (1) The loud one: a lone `{` in a test module made the walk run off the
    // end of the file and abort the audit with a panic that blamed a perfectly
    // balanced source.
    let unbalanced_looking = strip_comments(
        "#[cfg(test)]\nmod tests {\n    fn t() { assert_eq!(\"{\", \"{\"); }\n}\n",
        true,
    );
    assert_eq!(
        rust_test_spans(&unbalanced_looking).len(),
        1,
        "the braces in this source are balanced; only the ones in the string \
         literal are not, and they are text"
    );

    // (2) The silent one, and it is the Sprint 72 §5.9 failure class arriving
    // by a different door: a stray `{` in one test module beside a stray `}` in
    // the next swallowed the production site between them.
    let swallowing = strip_comments(
        "#[cfg(test)]\nmod a {\n    fn t() { let s = \"{\"; }\n}\n\n\
         fn prod() -> &'static str { \"unscanned\" }\n\n\
         #[cfg(test)]\nmod b {\n    fn u() { let s = \"}\"; }\n}\n",
        true,
    );
    let spans = rust_test_spans(&swallowing);
    let production: Vec<&str> = sentinel_hits(&swallowing.code)
        .into_iter()
        .filter(|(at, _, _)| !spans.iter().any(|&(start, end)| *at >= start && *at < end))
        .map(|(_, _, sentinel)| sentinel)
        .collect();
    assert_eq!(
        production,
        ["unscanned"],
        "the production site between two test modules must stay production — \
         it disappearing is a silent miss, which is the one direction a census \
         may never fail in"
    );

    // (3) The mis-classifying one: a stray `}` ended the span early and
    // re-reported every later test occurrence as production.
    let early_close = strip_comments(
        "#[cfg(test)]\nmod tests {\n    fn t() { let s = \"}\"; }\n    \
         fn u() -> &'static str { \"none recorded\" }\n}\n",
        true,
    );
    let spans = rust_test_spans(&early_close);
    assert!(
        sentinel_hits(&early_close.code)
            .into_iter()
            .all(|(at, _, _)| spans.iter().any(|&(start, end)| at >= start && at < end)),
        "every occurrence inside the module stays inside its span"
    );
}

/// **ASCII folding keeps the sentinel offsets and the span offsets in one
/// coordinate system.**
///
/// `to_lowercase` is not length-preserving: U+212A KELVIN SIGN folds 3 bytes to
/// 1, and U+0130 folds 2 bytes to 3. The sentinel offsets came from the folded
/// string and the spans from the unfolded one, so once enough of either
/// character sits before a site, the two coordinate systems disagree by more
/// than the distance to the nearest span boundary and the site crosses it.
/// Review reproduced both directions. No file on the three surfaces contains
/// such a character today — which is exactly why this is a fixture rather than
/// a live failure, and why it would have shipped.
///
/// The runs are long on purpose: the defect is a *drift* between two offset
/// systems, so a fixture whose drift is smaller than the gap it has to cross
/// passes under the broken code and proves nothing. The first draft of this
/// test used three characters and survived the mutation.
#[test]
fn a_character_that_folds_to_a_different_length_does_not_move_a_site() {
    // Shrinking fold: the production site's folded offset slides *back* into
    // the test span, and a real site drops out of the production census.
    let shrinking = "\u{212A}".repeat(40);
    let source =
        format!("#[cfg(test)]\nmod t {{ fn x() {{ let _ = \"{shrinking}\"; }} }}\nfn p() -> &'static str {{ \"unscanned\" }}\n");
    let stripped = strip_comments(&source, true);
    let spans = rust_test_spans(&stripped);
    let production: Vec<&str> = sentinel_hits(&stripped.code)
        .into_iter()
        .filter(|(at, _, _)| !spans.iter().any(|&(start, end)| *at >= start && *at < end))
        .map(|(_, _, sentinel)| sentinel)
        .collect();
    assert_eq!(
        production,
        ["unscanned"],
        "a production site after a test module full of shrinking characters \
         must stay production"
    );

    // Growing fold: a test occurrence's folded offset slides *past* the span's
    // end and is reported as a production site nobody wrote.
    let growing = "\u{0130}".repeat(40);
    let source =
        format!("#[cfg(test)]\nmod t {{ fn x() {{ let _ = \"{growing}\"; let _ = \"unscanned\"; }} }}\n");
    let stripped = strip_comments(&source, true);
    let spans = rust_test_spans(&stripped);
    assert!(
        sentinel_hits(&stripped.code)
            .into_iter()
            .all(|(at, _, _)| spans.iter().any(|&(start, end)| at >= start && at < end)),
        "an occurrence inside a test module full of growing characters must \
         stay inside its span"
    );
}

/// **A sentinel the formatter wrapped is still one site.**
///
/// Nine of the twenty sentinels contain a space, and `readout.rs` and
/// `governance/mod.rs` are written almost entirely in `\`-continued `format!`
/// strings. A plain substring match needs exactly one space, so a reflow of the
/// wrong line makes a conformant site invisible — under-capture, the silent
/// direction. No live site is split today; one edit would do it.
#[test]
fn a_sentinel_split_across_lines_is_still_one_site() {
    let continued = strip_comments(
        "fn m() -> String { format!(\"violations none \\\n        recorded (no rule \
         check has run)\") }",
        true,
    );
    assert_eq!(
        sentinel_hits(&continued.code)
            .into_iter()
            .map(|(_, _, sentinel)| sentinel)
            .collect::<Vec<_>>(),
        ["none recorded"],
        "a Rust backslash-continuation joins one rendered sentence; rustc \
         renders exactly one space there. (`no rule check` is not in the \
         lexicon — the rendered line says `none recorded (no rule check has \
         run)` and `none recorded` is the sentinel it uses.)"
    );

    let wrapped = strip_comments("const t = <p>violations none\n  recorded here</p>;", false);
    assert_eq!(
        sentinel_hits(&wrapped.code)
            .into_iter()
            .map(|(_, _, sentinel)| sentinel)
            .collect::<Vec<_>>(),
        ["none recorded"],
        "…and a JSX text node the formatter wrapped renders the same sentence"
    );
}

/// **A `#[cfg(test)] mod tests;` declaration is not a test block.**
///
/// The guard is the strict `mod <identifier>` head check; a declaration's `;`
/// falls inside the head and fails it.
///
/// Reading the next `{` in the file as its body classifies everything after it
/// as test scope — which is the same silent-miss failure as truncation, wearing
/// the opposite sign. This scanner did it to `governance/mod.rs` and hid a real
/// production site in a file the audit was specifically walking.
#[test]
fn a_module_declaration_is_not_a_test_block() {
    let code = strip_comments(
        "#[cfg(test)]\nmod tests;\n\nfn renders() -> &'static str { \"n/a\" }\n",
        true,
    );
    assert!(
        rust_test_spans(&code).is_empty(),
        "`mod tests;` declares a module in another FILE and opens no block here"
    );

    let inline = strip_comments(
        "fn renders() -> &'static str { \"n/a\" }\n#[cfg(test)]\nmod tests {\n    fn t() {}\n}\n",
        true,
    );
    assert_eq!(
        rust_test_spans(&inline).len(),
        1,
        "…while an inline `mod tests {{ }}` is one span"
    );

    // `#[cfg(test)]` on anything that is not a module opens no test scope
    // either, and this is the case the `mod ` prefix alone rejects — the head
    // check's two halves answer different inputs and neither is spare.
    let on_a_fn = strip_comments(
        "#[cfg(test)]\nfn helper() {}\nfn renders() -> &'static str { \"n/a\" }\n",
        true,
    );
    assert!(
        rust_test_spans(&on_a_fn).is_empty(),
        "`#[cfg(test)] fn helper()` is a test-only helper, not a module whose \
         braces enclose a region — treating its body as a span would classify \
         everything up to the matching brace as test scope"
    );
}

/// **The census is demonstrated to detect an added site, including one past a
/// `#[cfg(test)]` module** — not merely observed to run clean today.
///
/// A guard that reports clean by construction is the failure class this file
/// exists to close (Sprint 72 review, appendix 5.9), so the detection is proven
/// on a fixture: the scan finds the added site, *and* the comparison the real
/// test performs rejects the fixture against a table that declares only the
/// first site. If the span walk ever regains the truncation defect, the second
/// site vanishes, the fixture collapses onto that baseline, and this fires.
#[test]
fn a_new_site_is_detected_even_after_a_test_module() {
    let stripped = strip_comments(FIXTURE_WITH_A_SITE_AFTER_THE_TEST_MODULE, true);
    let spans = rust_test_spans(&stripped);
    let found: Vec<(&str, bool)> = sentinel_hits(&stripped.code)
        .into_iter()
        .map(|(at, _, sentinel)| {
            (
                sentinel,
                spans.iter().any(|&(start, end)| at >= start && at < end),
            )
        })
        .collect();

    assert!(
        found.contains(&("none recorded", false)),
        "the site added AFTER the test module is production scope — truncating \
         there is what made a comparable guard false-green: {found:?}"
    );
    assert!(
        found.contains(&("n/a", false)) && found.contains(&("n/a", true)),
        "and the sentinel either side of the module is classified both ways, \
         so the span is a span and not a cut: {found:?}"
    );

    // The comparison, not only the extractor. The like-for-like baseline is
    // what this fixture's table would say if only its FIRST function were
    // declared; the added site must break equality against THAT.
    let mut production: Vec<&str> = found
        .iter()
        .filter(|(_, in_test)| !in_test)
        .map(|(s, _)| *s)
        .collect();
    production.sort_unstable();
    assert_ne!(
        production,
        vec!["empty graph", "n/a"],
        "a source carrying an unrecorded second site must not compare equal to \
         the table that declares only the first"
    );
}

// ── the audit ──────────────────────────────────────────────────────────────

/// **The three surfaces, walked from disk, match the census exactly.**
#[test]
fn every_absence_reporting_site_is_enumerated_and_recorded() {
    let walked = walk();

    // Anti-vacuity, and not the same claim as the equality below: a walk that
    // matched nothing reports an empty set, which would compare equal to an
    // empty table and mean nothing.
    assert!(
        !walked.is_empty(),
        "the walk found no absence site on any surface — it cannot have read them"
    );

    let declared: Vec<(String, String, String, usize, usize)> = CENSUS
        .iter()
        .map(|(surface, file, sentinel, production, test, verdict)| {
            assert!(
                verdict.starts_with("CONFORMANT")
                    || verdict.starts_with("CORRECTED")
                    || verdict.starts_with("NOT A SITE")
                    || verdict.starts_with("NO PRODUCTION SITE"),
                "{file} / {sentinel} carries no adjudication: {verdict}"
            );
            // `NOT A SITE` is an adjudication, not an omission: the matcher
            // over-captures on purpose — an identifier, a `case` label or a SQL
            // comment nested in a Rust string can spell a sentinel — and a
            // census may be wrong only in the loud direction. What it may never
            // do is leave a capture unjudged.
            assert_eq!(
                *production == 0,
                verdict.starts_with("NO PRODUCTION SITE"),
                "{file} / {sentinel}: the verdict and the production count must \
                 agree about whether there is a site to adjudicate"
            );
            // The verdict against CORRECTIONS, so the classification is a
            // checked fact rather than free prose. Without this, relabelling
            // the one CORRECTED row "CONFORMANT — nothing was ever wrong here"
            // left the suite green while CORRECTIONS still listed three
            // corrections in that same file: the table contradicted itself and
            // the audit reported clean. That is S-435's review finding — "the
            // classification prose carried no test weight" — recurring in the
            // story written to learn from it.
            // Keyed on the SENTINEL, not just the file: `governance/mod.rs`
            // carries a corrected `n/a` row beside a conformant `no baseline`
            // row, so "this file has a correction" is too coarse to
            // adjudicate either.
            assert_eq!(
                verdict.starts_with("CORRECTED"),
                CORRECTIONS
                    .iter()
                    .any(|(corrected, _, after, _)| corrected == file
                        && after.contains(sentinel)),
                "{file} / {sentinel}: a CORRECTED verdict needs a CORRECTIONS \
                 entry whose corrected wording carries this sentinel, and a \
                 row the corrections reach cannot be recorded conformant"
            );
            (
                (*surface).to_string(),
                (*file).to_string(),
                (*sentinel).to_string(),
                *production,
                *test,
            )
        })
        .collect();

    assert_eq!(
        walked, declared,
        "every absence-reporting occurrence on the three surfaces is enumerated \
         and recorded. A row the walk finds and the census does not is a site \
         nobody adjudicated; a row the census declares and the walk does not is \
         a record of a site that has moved. Either way, update CENSUS with what \
         the site is and which taxonomy rule it keeps ({AUDITED_ON} reading)"
    );
}

/// **The count, its denominator and its date — derived, then compared with what
/// the header states.**
///
/// The header is prose and prose drifts; this is the same three figures read
/// off the census, so a row added without touching the header fails here.
///
/// These are equalities against a dated record, **not floors** ([CR-138]
/// CRA-05). Nothing in this file requires a future audit to find any particular
/// number of non-conformant sites, and *all sites already conformant* is a
/// delivered result.
///
/// [CR-138]: ../../docs/requests/CR-138-a-readout-names-the-cause-its-gating-condition-establishes.md
/// How the correction changed the census: `(rows removed, production
/// occurrences removed)`.
///
/// Stated so the pre-correction denominator and the post-correction one are
/// tied by arithmetic rather than by two independent hand counts. Each of the
/// three corrected renderings carried an `n/a` **and** an `empty graph`, and
/// the two byte-identical ones collapsed into a single constant, so six
/// occurrences became two and the `empty graph` row left the file entirely.
const CORRECTION_DELTA: (usize, usize) = (1, 4);

/// What [S-442] added to the census on 2026-09-22: `(production rows added,
/// production occurrences added)` — the two `models/navigation.rs` rows, one
/// `n/a` and two `unindexed`.
///
/// Stated as a delta for the same reason as [`CORRECTION_DELTA`]: the
/// 2026-09-20 figures in the header stay tied to this table by arithmetic
/// rather than surviving only as prose about a tree that no longer exists.
///
/// [S-442]: ../../docs/planning/journal.md#s-442-a-relational-answer-states-the-resolution-denominator-it-was-computed-over
const S442_ADDITION: (usize, usize) = (2, 3);

/// What [S-444] added to the census on 2026-09-23: `(production rows added,
/// production occurrences added, test occurrences added)` — the managed
/// block's `same-file-only` and `no-resolved-edges`, and the one test-scope
/// copy of each in `surface_parity`'s `RELATIONAL_SCOPE`.
///
/// The first delta to carry test-scope occurrences, so it has the third field
/// [`S442_ADDITION`] never needed.
///
/// [S-444]: ../../docs/planning/journal.md#s-444-the-shipped-guidance-states-the-language-scope-its-relational-claims-hold-on
const S444_ADDITION: (usize, usize, usize) = (2, 2, 2);

/// What [S-445] added to the census on 2026-09-25: `(production rows added,
/// production occurrences added, test occurrences added)` — the one
/// `models/outcome.rs` row, `OUTCOME_ABSENCE`'s `none recorded`. Its tests name
/// the constant rather than the literal, so it adds no test-scope occurrence.
///
/// [S-445]: ../../docs/planning/journal.md#s-445-a-telemetry-event-records-what-the-call-answered-with-its-denominator
const S445_ADDITION: (usize, usize, usize) = (1, 1, 0);

/// What [S-306] added to the census on 2026-09-26: `(production rows added,
/// production occurrences added, test occurrences added)` — no production
/// site, only test-scope fixtures/assertions pinning `stats`'s
/// `OUTCOME_ABSENCE` string verbatim in the new Statistics-tab attribution
/// card's tests (`StatisticsView.test.tsx` 3, `statsModel.test.ts` 4).
///
/// [S-306]: ../../docs/planning/journal.md#s-306-statistics-tab-attribution-view-with-stated-coverage-limits
const S306_ADDITION: (usize, usize, usize) = (0, 0, 7);

/// What [S-502] added to the census on 2026-10-03: `(production rows added,
/// production occurrences added, test occurrences added)` — no production
/// site, one test-scope assertion that a repo with no class carrying a bodied
/// method reports Cohesion `n/a` (`metrics/tests.rs`).
///
/// [S-502]: ../../docs/planning/journal.md#s-502-cohesion-and-focus-count-bodied-methods-and-metric-semantics-move-to-v7
const S502_ADDITION: (usize, usize, usize) = (0, 0, 1);

/// What [S-499] added to the census on 2026-10-03: `(production rows added,
/// production occurrences added, test occurrences added)` — no production
/// site, five test-scope assertions that an `n/a` dimension keeps its `n/a`
/// rendering beside the three offender states (`HealthView.test.tsx` 3,
/// `healthModel.test.ts` 2).
///
/// [S-499]: ../../docs/planning/journal.md#s-499-health-drill-downs-render-the-persisted-offenders-in-three-honest-states
const S499_ADDITION: (usize, usize, usize) = (0, 0, 5);

/// What Sprint 93 ([CR-203]) changed in the census on 2026-10-08, as a NET
/// `(production rows, production occurrences, test occurrences)`: the Health
/// sites moved from `HealthView.tsx` to `copy/health.copy.ts` (−4 rows, +6
/// rows; −15, +16 occurrences), one new `CoverageView.tsx` `n/a`, and 24 new
/// test-scope assertions. Net, because a move is not an addition: it is the
/// figure that keeps every earlier reading checkable.
///
/// [CR-203]: ../../docs/requests/CR-203-every-web-widget-explains-itself.md
const SPRINT93_DELTA: (usize, usize, usize) = (2, 2, 24);

/// What Sprint 93's hotfix HF-1 ([CR-206]) REMOVED from the census on
/// 2026-10-08: `(production rows, production occurrences, test occurrences)` —
/// `readingAction`'s five `case` labels in `copy/health.copy.ts` (no row left,
/// each still counted once in its absence or stale sentence), and eleven
/// test-scope occurrences that fed or asserted the removed action line. A
/// removal, so it is ADDED back to reach the earlier readings.
///
/// [CR-206]: ../../docs/requests/CR-206-the-widget-frame-drops-the-action-line.md
const HF1_REMOVAL: (usize, usize, usize) = (0, 5, 11);

#[test]
fn the_audit_reports_its_count_with_its_denominator() {
    let production_rows_now = CENSUS.iter().filter(|r| r.3 > 0).count();
    let production_occurrences_now: usize = CENSUS.iter().map(|r| r.3).sum();
    let test_occurrences: usize = CENSUS.iter().map(|r| r.4).sum();
    assert_eq!(
        (production_occurrences_now, production_rows_now, test_occurrences),
        (84, 44, 134),
        "the census as it stands after Sprint 93 HF-1's addendum, 2026-10-08: 84 \
         production occurrences over 44 production rows, beside 134 test-scope \
         occurrences"
    );
    // The reading before HF-1: Sprint 93's addendum, 2026-10-08.
    let (production_occurrences_now, production_rows_now, test_occurrences) = (
        production_occurrences_now + HF1_REMOVAL.1,
        production_rows_now + HF1_REMOVAL.0,
        test_occurrences + HF1_REMOVAL.2,
    );
    assert_eq!(
        (production_occurrences_now, production_rows_now, test_occurrences),
        (89, 44, 145),
        "the census as it stood after Sprint 93's addendum, 2026-10-08: 89 production \
         occurrences over 44 production rows, beside 145 test-scope occurrences"
    );
    // The 2026-09-20 reading the header and the tuple below state.
    let production_rows = production_rows_now
        - S445_ADDITION.0
        - S444_ADDITION.0
        - S442_ADDITION.0
        - S306_ADDITION.0
        - S502_ADDITION.0
        - S499_ADDITION.0
        - SPRINT93_DELTA.0;
    let production_occurrences = production_occurrences_now
        - S445_ADDITION.1
        - S444_ADDITION.1
        - S442_ADDITION.1
        - S306_ADDITION.1
        - S502_ADDITION.1
        - S499_ADDITION.1
        - SPRINT93_DELTA.1;
    let test_occurrences = test_occurrences
        - S445_ADDITION.2
        - S444_ADDITION.2
        - S306_ADDITION.2
        - S502_ADDITION.2
        - S499_ADDITION.2
        - SPRINT93_DELTA.2;
    let corrected_files: std::collections::BTreeSet<&str> =
        CORRECTIONS.iter().map(|(file, _, _, _)| *file).collect();

    assert_eq!(
        (
            CORRECTIONS.len(),
            corrected_files.len(),
            production_occurrences,
            production_rows,
            test_occurrences,
            SURFACES.len(),
            AUDITED_ON,
        ),
        (3, 1, 81, 37, 106, 3, "2026-09-20"),
        "the census as it now stands: 81 production occurrences over 37 \
         production rows across 3 surfaces, beside 106 test-scope occurrences, \
         read on 2026-09-20. Change the header and this tuple together — a \
         count without its denominator says nothing, and an undated one is \
         read as a standing property"
    );

    // The numerator's own population: the 3 were counted on the tree BEFORE the
    // correction, so the denominator they are quoted against is that tree's.
    assert_eq!(
        (
            production_rows + CORRECTION_DELTA.0,
            production_occurrences + CORRECTION_DELTA.1,
        ),
        (38, 85),
        "3 non-conformant renderings of 85 production occurrences over 38 \
         production rows — one population, which is what makes it a ratio"
    );

    for (file, before, after, why) in CORRECTIONS {
        assert!(
            !why.is_empty() && before != after,
            "{file}: a correction records what changed and why"
        );
    }
}

/// **The corrected wording is gone from every surface, not just from the site
/// the audit happened to open.**
///
/// The third blind spot of an author-run sweep: a mutation can only break code
/// that exists, never reveal the sibling call site nobody edited. So the
/// removed phrasing is searched for across all three surfaces rather than
/// checked where it was found.
#[test]
fn the_corrected_wording_is_gone_from_every_surface() {
    let root = workspace_root();
    for (_, dir) in SURFACES {
        for path in sources(&root.join(dir)) {
            let source = std::fs::read_to_string(&path).expect("a readable source");
            let rust = path.extension().and_then(|e| e.to_str()) == Some("rs");
            let code = strip_comments(&source, rust).code;
            for (_, before, _, _) in CORRECTIONS {
                assert!(
                    !code.contains(before),
                    "{} still renders {before:?} — the correction reached one \
                     site and not its siblings",
                    path.display()
                );
            }
        }
    }

    // …and the replacement is actually in place, so the check above cannot be
    // satisfied by deleting the message altogether.
    let gate = std::fs::read_to_string(root.join("logos-core/src/governance/mod.rs"))
        .expect("the gate source");
    for (_, _, after, _) in CORRECTIONS {
        assert!(
            gate.contains(after),
            "the corrected wording {after:?} is not in the gate source"
        );
    }
}

/// **Each surface file that reports an absence references the one taxonomy.**
///
/// Deliverable (b) as a checkable fact: the statement lives in one place and
/// the surfaces point at it. A new absence-reporting file fails here until it
/// does — which is the moment its author reads the rules.
#[test]
fn the_three_surfaces_reference_the_one_taxonomy() {
    let root = workspace_root();
    let mut cited = 0usize;
    for (surface, file, _, production, _, verdict) in CENSUS {
        // A file whose only captures are identifiers or nested prose reports no
        // absence, so it has no taxonomy to reference.
        if production == 0 || verdict.starts_with("NOT A SITE") {
            continue;
        }
        let source = std::fs::read_to_string(root.join(file))
            .unwrap_or_else(|e| panic!("reading {file}: {e}"));
        assert!(
            source.contains(absence::TAXONOMY_REFERENCE),
            "{surface}: {file} reports an absence but does not reference \
             `{}` — the taxonomy is stated once and referenced, never restated",
            absence::TAXONOMY_REFERENCE
        );
        cited += 1;
    }
    assert_eq!(
        cited,
        CENSUS
            .iter()
            .filter(|(_, _, _, production, _, verdict)| *production > 0
                && !verdict.starts_with("NOT A SITE"))
            .count(),
        "one check per adjudicated production row — a denominator, so a census \
         that silently stopped producing rows cannot pass this by checking \
         nothing. Derived rather than hardcoded: the figure is pinned once, in \
         the dated tuple, which is this file's own discipline"
    );
    assert!(
        cited > 0,
        "…and the loop ran at all — an empty census would satisfy any equality"
    );
}

/// **`signalAbsence` is unchanged** ([CR-135] §3.3, re-confirmed under
/// [S-422]'s story review).
///
/// The audit's conclusion was that this classification is the reference model
/// the Rust vocabulary follows, not a defect — so the deliverable here is the
/// assertion, not a change. Had the audit concluded otherwise, that would have
/// been an explicit reversal of a decision taken under review, raised as a
/// deferred finding rather than folded in.
///
/// [S-422]: ../../docs/planning/journal.md#s-422-the-health-readout-is-internally-consistent-and-never-stale
/// [CR-135]: ../../docs/requests/CR-135-the-health-readout-is-internally-consistent-and-never-stale.md
#[test]
fn the_reference_model_is_unchanged() {
    let source =
        std::fs::read_to_string(workspace_root().join("web/ui/src/views/health/healthModel.ts"))
            .expect("the Health model");
    assert!(
        source.contains(SIGNAL_ABSENCE_REFERENCE_MODEL),
        "`SignalAbsence` and `signalAbsence` differ from the text CR-135 §3.3 \
         settled. Changing them is a reversal of a decision taken under review: \
         raise it, do not fold it in"
    );
}

/// **No new absence state, on any surface** ([CR-135] §7).
///
/// The two Rust enums are matched **exhaustively**, so an added arm does not
/// fail an assertion — it fails to compile, which is the stronger guard. The
/// two TypeScript unions have no compiler to lend, so their arity is read from
/// source.
#[test]
fn no_new_absence_state_on_any_surface() {
    let signal_cause = |absence: &SignalAbsence| match absence {
        SignalAbsence::EmptyGraph => "empty-graph",
        SignalAbsence::NoProductionScope { .. } => "no-production-scope",
    };
    let evaluated_cause = |absence: EvaluatedSetAbsence| match absence {
        EvaluatedSetAbsence::NoContract => "no-contract",
        EvaluatedSetAbsence::NoRulesAuthored => "no-rules-authored",
        EvaluatedSetAbsence::Unrecorded => "unrecorded",
    };
    let cross_file_cause = |absence: CrossFileAbsence| match absence {
        CrossFileAbsence::SameFileOnly { .. } => "same-file-only",
        CrossFileAbsence::NoResolvedEdges { .. } => "no-resolved-edges",
        CrossFileAbsence::NoReferencesRecorded => "no-references-recorded",
    };
    assert_eq!(
        (
            signal_cause(&SignalAbsence::EmptyGraph),
            signal_cause(&SignalAbsence::NoProductionScope {
                indexed_nodes: 1,
                test_functions: 0,
            }),
        ),
        ("empty-graph", "no-production-scope"),
        "`SignalAbsence` answers one question with two arms (R0)"
    );
    assert_eq!(
        [
            evaluated_cause(EvaluatedSetAbsence::NoContract),
            evaluated_cause(EvaluatedSetAbsence::NoRulesAuthored),
            evaluated_cause(EvaluatedSetAbsence::Unrecorded),
        ],
        ["no-contract", "no-rules-authored", "unrecorded"],
        "`EvaluatedSetAbsence` answers a different question with three (R0)"
    );
    assert_eq!(
        [
            cross_file_cause(CrossFileAbsence::SameFileOnly { same_file_edges: 1 }),
            cross_file_cause(CrossFileAbsence::NoResolvedEdges {
                references: 1,
                bound: 0,
            }),
            cross_file_cause(CrossFileAbsence::NoReferencesRecorded),
        ],
        [
            "same-file-only",
            "no-resolved-edges",
            "no-references-recorded"
        ],
        "`CrossFileAbsence` answers a third question — why a language's relation \
         class has no cross-file edge — with three (R0, S-441)"
    );
    let denominator_cause = |absence: DenominatorAbsence| match absence {
        DenominatorAbsence::Unindexed => "unindexed",
        DenominatorAbsence::NoLanguageRecorded { .. } => "no-language-recorded",
        DenominatorAbsence::NotAvailable => "n/a",
    };
    assert_eq!(
        [
            denominator_cause(DenominatorAbsence::Unindexed),
            denominator_cause(DenominatorAbsence::NoLanguageRecorded { anchors: 1 }),
            denominator_cause(DenominatorAbsence::NotAvailable),
        ],
        ["unindexed", "no-language-recorded", "n/a"],
        "`DenominatorAbsence` answers a fourth question — why a relational answer \
         carries no language row — with three (R0, S-442)"
    );

    let model =
        std::fs::read_to_string(workspace_root().join("web/ui/src/views/health/healthModel.ts"))
            .expect("the Health model");
    let arity = |declaration: &str| -> usize {
        let at = model
            .find(declaration)
            .unwrap_or_else(|| panic!("{declaration} is declared"));
        let tail = &model[at + declaration.len()..];
        tail[..tail.find(';').expect("a terminated declaration")]
            .split('|')
            .count()
    };
    assert_eq!(
        (
            arity("export type SignalAbsence ="),
            arity("export type SnapshotCurrency ="),
        ),
        (3, 3),
        "the Health page classifies an absent signal three ways and a \
         non-current one three ways, and adds no fourth to either (CR-135 §7)"
    );

    // `SnapshotCurrency`'s three arms are named interfaces discriminated by a
    // `cause` field, so a fourth state can be added INSIDE an arm without the
    // alias line moving. Review did exactly that — `cause: "moved-past" |
    // "drifted"` — and the pipe count stayed 3 while the Health page began
    // classifying a non-current snapshot four ways, which is what CR-135 §7
    // forbids and what this test claims to prevent. So the discriminants are
    // counted where they are declared.
    let mut causes: Vec<&str> = model
        .match_indices("readonly cause:")
        .map(|(at, marker)| {
            let tail = &model[at + marker.len()..];
            &tail[..tail.find(';').expect("a terminated field")]
        })
        .flat_map(|declaration| declaration.split('|'))
        .map(|arm| arm.trim().trim_matches('"'))
        .filter(|arm| *arm != "undefined")
        .collect();
    causes.sort_unstable();
    assert_eq!(
        causes,
        ["indeterminate", "moved-past"],
        "exactly two tagged causes, plus the untagged de-index arm whose \
         absent `cause` is its discriminant (CR-135 §3.2). A third tag is a \
         fourth Health state however it is spelled"
    );
}

/// **One condition, one wording — including across the language boundary.**
///
/// R5 binds within a surface, so the taxonomy cannot make these two literals
/// one. They are the same sentence in two languages: `render_age`'s two
/// degradations and the phrases `dashboardModel.ts` adopted from them under
/// S-433. Nothing checked that they stayed identical, and a pair nobody checks
/// is a pair that drifts — so this is the check, and the audit found the gap
/// rather than the drift.
#[test]
fn one_condition_has_one_wording_across_the_language_boundary() {
    let root = workspace_root();
    // Comment-stripped, like every other file-reading check in this module.
    // Reading raw source made this satisfiable by a COMMENT: review reworded
    // `render_age`'s literal, left the old sentence behind as a `//` line, and
    // the two surfaces genuinely drifted with this test — the only defender of
    // the cross-boundary property — still green. The Rust side has no other
    // defender: `readout.rs` and `read_only_accessors.rs` both assert only
    // `contains("implausibly old")`, which survives the rewording.
    let cli = strip_comments(
        &std::fs::read_to_string(root.join("logos-core/src/governance/readout.rs"))
            .expect("the readout source"),
        true,
    )
    .code;
    let spa = strip_comments(
        &std::fs::read_to_string(root.join("web/ui/src/views/dashboard/dashboardModel.ts"))
            .expect("the Dashboard model"),
        false,
    )
    .code;
    for phrase in [
        "at an unknown age (recorded ahead of now — check the clock)",
        "at an unknown age (the recorded time is implausibly old — check the store)",
    ] {
        assert!(
            cli.contains(phrase) && spa.contains(phrase),
            "{phrase:?} is one condition's wording and must be byte-identical \
             on both surfaces, as a rendered literal and not as prose about \
             one (S-433)"
        );
    }
}

/// **The lexicon's own home is not a reporting site.**
///
/// `models/quality.rs` is deliberately off the scanned surfaces — it holds
/// [`absence::SENTINELS`], so scanning it would report the lexicon as twenty
/// sites, the self-reference [S-435]'s sibling harness records hitting. That
/// exclusion is the one hole in the walk, so it is closed here rather than
/// assumed: the file is a serde read-model and renders nothing, and the only
/// sentinels in it are the declaration itself.
///
/// [S-435]: ../../docs/planning/journal.md#s-435-the-wiki-generation-pass-names-its-own-surface
#[test]
fn the_taxonomy_module_is_not_a_reporting_site() {
    let source = std::fs::read_to_string(workspace_root().join("logos-core/src/models/quality.rs"))
        .expect("the quality read-model");
    let code = strip_comments(&source, true).code;
    let lower = code.to_ascii_lowercase();
    let declaration = lower
        .find("pub const sentinels")
        .expect("the lexicon is declared here");
    let end = lower[declaration..]
        .find("];")
        .expect("a terminated declaration")
        + declaration;

    for (at, _, sentinel) in sentinel_hits(&code) {
        assert!(
            at >= declaration && at < end,
            "models/quality.rs carries {sentinel:?} outside the SENTINELS \
             declaration — it has become a rendering surface and must be \
             added to SURFACES rather than left unwalked"
        );
    }
}

/// **The resolution denominator speaks only the lexicon** ([S-441], [FR-RS-09],
/// [S-442], [FR-NV-14]).
///
/// [`CrossFileAbsence`]'s tags are serde-derived, so they are not source
/// literals and the lexical census above cannot see them — the blind spot its
/// own header names, one type over. They are pinned here structurally instead:
/// every tag the type can serialise is an [`absence::SENTINELS`] spelling, and
/// the three spellings [S-441] added are exactly the tags the arms produce.
///
/// The relational answers' [`DenominatorAbsence`] is pinned the same way, from
/// its one classifying constructor and its `n/a` state: every tag is a lexicon
/// spelling, and `no-language-recorded` — the one wording [S-442] added — is
/// produced by an arm. Its other two tags reuse lexicon words and are census
/// rows besides, since one of them is written as a literal.
///
/// What that cannot see is a **fourth** spelling added to the lexicon with no
/// producer — the lexicon does not mark which entries are resolution ones, so
/// no reverse walk over it is possible. The lexicon's size is pinned instead,
/// so any addition, orphan or not, fails here and is reviewed rather than
/// accepted. [S-442] attaches this denominator to the relational answers; this
/// is what keeps it in the closed vocabulary when it does.
///
/// [S-441]: ../../docs/planning/journal.md#s-441-resolution-coverage-is-reported-per-language-with-its-denominator
/// [S-442]: ../../docs/planning/journal.md#s-442-a-relational-answer-states-the-resolution-denominator-it-was-computed-over
/// [FR-RS-09]: ../../docs/specs/requirements/FR-RS-09.md
/// [FR-NV-14]: ../../docs/specs/requirements/FR-NV-14.md
#[test]
fn the_resolution_denominator_speaks_only_the_lexicon() {
    // Exhaustive by construction: the classifier is the only producer, and
    // these four inputs reach each of its three arms (the fourth is the
    // figure, which serialises no tag at all).
    let tags: Vec<String> = [(1, 0, 1, 0), (1, 0, 0, 0), (0, 0, 0, 0), (1, 1, 1, 1)]
        .into_iter()
        .filter_map(|(references, bound, same, cross)| {
            CrossFileAbsence::classify(references, bound, same, cross).1
        })
        .map(|absence| {
            serde_json::to_value(absence).expect("serialises")["cause"]
                .as_str()
                .expect("a string tag")
                .to_string()
        })
        .collect();
    assert_eq!(tags.len(), 3, "three arms, three tags: {tags:?}");
    for tag in &tags {
        assert!(
            absence::SENTINELS.contains(&tag.as_str()),
            "{tag:?} is a named state the closed lexicon does not carry"
        );
    }
    for spelling in [
        "same-file-only",
        "no-resolved-edges",
        "no-references-recorded",
    ] {
        assert!(
            tags.iter().any(|t| t == spelling),
            "{spelling:?} is in the lexicon but no arm produces it"
        );
    }
    // The answer-level denominator: every arm, reached through the only
    // constructors that produce them.
    let answer_tags: Vec<String> = [
        ResolutionDenominator::measured(Vec::new(), &[]),
        ResolutionDenominator::measured(Vec::new(), &[None]),
        ResolutionDenominator::not_available(),
    ]
    .into_iter()
    .map(|denominator| {
        serde_json::to_value(denominator.absence.expect("no row, so an absence"))
            .expect("serialises")["cause"]
            .as_str()
            .expect("a string tag")
            .to_string()
    })
    .collect();
    assert_eq!(answer_tags, ["unindexed", "no-language-recorded", "n/a"]);
    for tag in &answer_tags {
        assert!(
            absence::SENTINELS.contains(&tag.as_str()),
            "{tag:?} is a relational answer's named state the closed lexicon does not carry"
        );
    }

    assert_eq!(
        absence::SENTINELS.len(),
        20,
        "the closed lexicon holds twenty spellings (sixteen before S-441, \
         nineteen before S-442). An addition is a new absence wording on some \
         surface: name its producer and its census row, then change this figure \
         and the prose counts in this file together"
    );
}
