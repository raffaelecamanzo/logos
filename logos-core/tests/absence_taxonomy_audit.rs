//! **Every absence-reporting site on the three surfaces, enumerated from source
//! and recorded conformant or corrected** ([S-434], [CR-138], [CR-135] §3.3,
//! [FR-EH-04], [FR-UI-04], [NFR-CC-04]).
//!
//! The taxonomy these sites are audited against is stated once, in code, at
//! [`models::quality::absence`]. This file does not restate it; it checks it.
//!
//! # The result, with its denominator and its date
//!
//! **2026-09-20 — 3 non-conformant occurrences, in 1 file, of 68 production
//! occurrences enumerated over 32 production rows across 3 surfaces** (plus 83
//! occurrences inside test scope, enumerated and separated, never truncated
//! away). The three are in [`CORRECTIONS`] and were corrected; every other row
//! was already conformant.
//!
//! **No floor is asserted anywhere in this file** ([CR-138] CRA-05). The
//! assertions below are equalities against a dated record of what was found,
//! not thresholds a future audit must clear. A later audit that enumerates more
//! sites and finds none non-conformant is a delivered result; so was most of
//! this one — five stories had already made 31 of the 32 production rows
//! conformant before the audit ran, and manufacturing corrections to improve
//! that figure would defeat its purpose.
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
//! adjudicates it. A census may be wrong only in the loud direction.
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
//! under [S-422]'s story review. [`the_reference_model_is_unchanged`] pins its
//! source byte-for-byte, and its own suite was run unmodified.
//!
//! [`models::quality::absence`]: logos_core::models::quality::absence
//! [`absence::SENTINELS`]: logos_core::models::quality::absence::SENTINELS
//! [S-422]: ../../docs/planning/journal.md#s-422-the-health-readout-is-internally-consistent-and-never-stale
//! [S-434]: ../../docs/planning/journal.md#s-434-one-absence-taxonomy-audited-across-the-three-reporting-surfaces
//! [S-435]: ../../docs/planning/journal.md#s-435-the-wiki-generation-pass-names-its-own-surface
//! [FR-EH-04]: ../../docs/specs/requirements/FR-EH-04.md
//! [FR-UI-04]: ../../docs/specs/requirements/FR-UI-04.md
//! [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md
//! [CR-135]: ../../docs/requests/CR-135-the-health-readout-is-internally-consistent-and-never-stale.md
//! [CR-138]: ../../docs/requests/CR-138-a-readout-names-the-cause-its-gating-condition-establishes.md

use std::path::{Path, PathBuf};

use logos_core::models::quality::{absence, EvaluatedSetAbsence, SignalAbsence};

/// The date the figures in this module's header were taken. A census is a
/// reading of one tree at one moment, and an undated one invites being read as
/// a standing property.
const AUDITED_ON: &str = "2026-09-20";

/// The three surfaces that report an absence, as workspace-relative roots.
///
/// `logos-core/src/governance` is the readout and the gate — the half of the
/// governance engine a user reads. `cli/src` is the CLI's own printing.
/// `web/ui/src` is the SPA, in full rather than only its Health view: a site
/// nobody named must not be able to hide in a tab nobody audited.
///
/// The API facade (`web/src/api_v1.rs`) is deliberately **not** here. It serves
/// the read-model as JSON and renders no absence text of its own; the SPA is
/// the surface that turns those `null`s into words, and it is scanned.
const SURFACES: [(&str, &str); 3] = [
    ("governance", "logos-core/src/governance"),
    ("cli", "cli/src"),
    ("spa", "web/ui/src"),
];

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

/// `healthModel.ts`'s reference model, pinned byte-for-byte.
///
/// Not a paraphrase and not a behavioural echo: the acceptance criterion is
/// that this code is **unchanged**, so the check is the source itself. A
/// re-worded doc comment moves this, which is the intended sensitivity — the
/// claim being made is about the whole declaration, reasoning included.
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
const CENSUS: [(&str, &str, &str, usize, usize, &str); 51] = [
    (
        "governance",
        "logos-core/src/governance/mod.rs",
        "n/a",
        2,
        0,
        "CORRECTED — the gate's two absent-signal verdicts. Both read \"n/a (empty graph)\" over a condition (`aggregate_signal == None` on either side) that has the two causes `SignalAbsence` separates, so both stated a cause the condition never established (R1, FR-EH-04 AC2). See CORRECTIONS",
    ),
    (
        "governance",
        "logos-core/src/governance/mod.rs",
        "no baseline",
        2,
        0,
        "CONFORMANT — the gate's no-baseline informational pass, in its two paths. R3: `gate --save` is exactly the command this condition identifies. R4: an informational pass, never a silent zero",
    ),
    (
        "governance",
        "logos-core/src/governance/readout.rs",
        "at an unknown age",
        2,
        0,
        "CONFORMANT — `render_age`'s two degradations, a negative and an implausible stamp. R4: the age is named unusable, never rendered as a plausible phrase. The cross-surface twin of `dashboardModel.ts`, pinned byte-identical below",
    ),
    (
        "governance",
        "logos-core/src/governance/readout.rs",
        "empty graph",
        1,
        5,
        "CONFORMANT — `SignalAbsence::EmptyGraph`'s clause, and the one honest use of the phrase on this surface: the readout reads the store's own counts, so its condition does establish the cause (R1)",
    ),
    (
        "governance",
        "logos-core/src/governance/readout.rs",
        "evaluated set unknown",
        2,
        4,
        "CONFORMANT — `EvaluatedSetAbsence::Unrecorded`, plus the `(None, None)` arm that names the set unknown and attributes no cause. R1 in its strictest form: an unestablished cause is reported absent, not guessed",
    ),
    (
        "governance",
        "logos-core/src/governance/readout.rs",
        "n/a",
        5,
        10,
        "CONFORMANT — the signal cell's three arms (no cause, empty graph, no production scope) and the baseline/delta clauses. R1 throughout; the `NoProductionScope` arm carries its two figures (R2)",
    ),
    (
        "governance",
        "logos-core/src/governance/readout.rs",
        "no baseline",
        1,
        1,
        "CONFORMANT — the summary channel's baseline clause. The full channel renders a different sentence naming `gate --save`, and `render_signal` records why that fork is deliberately not consolidated — R5's stated carve-out, recorded at the site",
    ),
    (
        "governance",
        "logos-core/src/governance/readout.rs",
        "no pass is stated",
        1,
        1,
        "CONFORMANT — R4 at its sharpest: a run that evaluated nothing and found nothing is that state, never \"clean\" and never a bare 0 (FR-GV-03)",
    ),
    (
        "governance",
        "logos-core/src/governance/readout.rs",
        "no rules contract",
        1,
        2,
        "CONFORMANT — `EvaluatedSetAbsence::NoContract`. R3: names no command, because the marker is written by `replace_violations`, which `scan` calls too",
    ),
    (
        "governance",
        "logos-core/src/governance/readout.rs",
        "none recorded",
        1,
        6,
        "CONFORMANT — no marker is absence of knowledge, and R4 forbids reading it as a pass (BR-41). \"rule check\" is the activity, not a command (R3)",
    ),
    (
        "governance",
        "logos-core/src/governance/readout.rs",
        "not comparable",
        2,
        2,
        "CONFORMANT — the baseline and delta clauses in both channels. R4: an incomparable baseline is named, never rendered as a delta of 0",
    ),
    (
        "governance",
        "logos-core/src/governance/tests.rs",
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
        "nothing was evaluated",
        1,
        0,
        "CONFORMANT — R4: the absent contract is named instead of rendered as a zero violation count (NFR-CC-04), which is the whole reason this arm exists rather than printing the report",
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
        2,
        0,
        "CONFORMANT — an absent freshness percentage and an absent `HEAD`. R4: never a shifted number, never a guessed 0 (FR-CV-05)",
    ),
    (
        "spa",
        "web/ui/src/views/analytics/FilesView.test.tsx",
        "n/a",
        0,
        3,
        "NO PRODUCTION SITE — 3 test occurrence(s) asserting sites declared elsewhere in this table; enumerated, never truncated away",
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
        2,
        "NO PRODUCTION SITE — 2 test occurrence(s) asserting sites declared elsewhere in this table; enumerated, never truncated away",
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
        "web/ui/src/views/health/HealthView.tsx",
        "indeterminate",
        2,
        0,
        "CONFORMANT — S-436's third arm rendered: neither `current` nor a date, and `detail` names the fact that is missing (R1, R4)",
    ),
    (
        "spa",
        "web/ui/src/views/health/HealthView.tsx",
        "moved-past",
        2,
        0,
        "CONFORMANT — the claim is that the graph moved past the snapshot, never that the figures are proven stale. R1: exactly what the timestamp pair establishes and no more",
    ),
    (
        "spa",
        "web/ui/src/views/health/HealthView.tsx",
        "n/a",
        9,
        0,
        "CONFORMANT — the three `signalAbsence` sentences and the ADR-21 metric drop-outs. R4: a muted `n/a`, never a fabricated zero",
    ),
    (
        "spa",
        "web/ui/src/views/health/HealthView.tsx",
        "no baseline",
        1,
        0,
        "CONFORMANT — the gate band's absent baseline. R4",
    ),
    (
        "spa",
        "web/ui/src/views/health/HealthView.tsx",
        "no-production-scope",
        2,
        0,
        "CONFORMANT — R3: this arm names no command, because no command changes the state — the conclusion `SignalAbsence::NoProductionScope` follows",
    ),
    (
        "spa",
        "web/ui/src/views/health/HealthView.tsx",
        "unindexed",
        2,
        0,
        "CONFORMANT — R3: names `logos index`, and may, because this condition is exactly \"nothing is indexed\"",
    ),
    (
        "spa",
        "web/ui/src/views/health/HealthView.tsx",
        "unscanned",
        2,
        0,
        "CONFORMANT — R3: names `logos scan`, and may, for the same reason",
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
        3,
        "NO PRODUCTION SITE — 3 test occurrence(s) asserting sites declared elsewhere in this table; enumerated, never truncated away",
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
        "CONFORMANT — THE REFERENCE MODEL, asserted unchanged. Settled by CR-135 §3.3, re-confirmed under S-422's story review, and pinned byte-for-byte by `the_reference_model_is_unchanged`",
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
/// # Four things this gets right that the obvious version does not
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
/// [S-435]: ../../docs/planning/journal.md#s-435-the-wiki-generation-pass-names-its-own-surface
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
        while let Some(c) = chars.next() {
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
                    if rust && c == 'r' && matches!(chars.peek(), Some('"' | '#')) {
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
    const ATTR: &str = "#[cfg(test)]";
    let code = &stripped.code;
    let mut spans = Vec::new();
    for (at, _) in code.match_indices(ATTR) {
        let after = at + ATTR.len();
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
    const ATTR: &str = "#[cfg(test)]";
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
        let code = strip_comments(&source, true).code;
        for (at, _) in code.match_indices(ATTR) {
            let tail = &code[at + ATTR.len()..];
            // Whichever comes first ends the candidate: a `;` makes it the
            // declaration this looks for, a `{` makes it an inline module.
            let Some(end) = tail.find(';') else { continue };
            if tail.find('{').is_some_and(|brace| brace < end) {
                continue;
            }
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
/// Nine of the sixteen sentinels contain a space, and both languages on these
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
        "#[cfg(test)]\npub mod helpers;\n\n#[cfg(test)]\nmod tests;\n\npub mod live;\n",
    )
    .expect("write");
    for leaf in ["helpers", "tests", "live"] {
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
         module is not one; and `b.v2/inner.rs` is not declared by `b.rs`"
    );
}

/// **A brace inside a string literal is text, not a block.**
///
/// [`strip_comments`] keeps string bodies because the rendered text lives
/// there, so the brace walk has to be told which braces are code. Review
/// reproduced all three consequences of not telling it, and every one of them
/// arrives from ordinary Rust that carries no sentinel at all.
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
/// Nine of the sixteen sentinels contain a space, and `readout.rs` and
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
                    || verdict.starts_with("NO PRODUCTION SITE"),
                "{file} / {sentinel} carries no adjudication: {verdict}"
            );
            assert_eq!(
                *production == 0,
                verdict.starts_with("NO PRODUCTION SITE"),
                "{file} / {sentinel}: the verdict and the production count must \
                 agree about whether there is a site to adjudicate"
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
#[test]
fn the_audit_reports_its_count_with_its_denominator() {
    let production_rows = CENSUS.iter().filter(|r| r.3 > 0).count();
    let production_occurrences: usize = CENSUS.iter().map(|r| r.3).sum();
    let test_occurrences: usize = CENSUS.iter().map(|r| r.4).sum();
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
        (3, 1, 68, 32, 83, 3, "2026-09-20"),
        "the module header states: on 2026-09-20, 3 non-conformant occurrences \
         in 1 file, of 68 production occurrences over 32 production rows across \
         3 surfaces, beside 83 test-scope occurrences. Change the header and \
         this tuple together — a count without its denominator says nothing, \
         and an undated one is read as a standing property"
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
    for (surface, file, _, production, _, _) in CENSUS {
        if production == 0 {
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
        cited, 32,
        "one check per production row — a denominator, so a census that \
         silently stopped producing rows cannot pass this by checking nothing"
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
    let cli = std::fs::read_to_string(root.join("logos-core/src/governance/readout.rs"))
        .expect("the readout source");
    let spa = std::fs::read_to_string(root.join("web/ui/src/views/dashboard/dashboardModel.ts"))
        .expect("the Dashboard model");
    for phrase in [
        "at an unknown age (recorded ahead of now — check the clock)",
        "at an unknown age (the recorded time is implausibly old — check the store)",
    ] {
        assert!(
            cli.contains(phrase) && spa.contains(phrase),
            "{phrase:?} is one condition's wording and must be byte-identical \
             on both surfaces (S-433)"
        );
    }
}

/// **The lexicon's own home is not a reporting site.**
///
/// `models/quality.rs` is deliberately off the scanned surfaces — it holds
/// [`absence::SENTINELS`], so scanning it would report the lexicon as sixteen
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
