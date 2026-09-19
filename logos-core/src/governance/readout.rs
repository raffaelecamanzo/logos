//! The report tier's session-start payload rendering ([FR-IN-07], [CR-095]).
//!
//! Renders a [`QualityReadout`] — the non-persisting read
//! [`quality_readout`](super::quality_readout) produces — into the JSON object an
//! agent host consumes at session start.
//!
//! # Why this lives in `governance` and not in the hook module
//!
//! It first landed beside the hook *materializer* ([`crate::wiki::hook`]), on the
//! reasoning that both are "the hook". They are not the same concern: that module
//! writes a script and merges a settings entry — pure install-time filesystem I/O
//! that never touches a read-model — while this projects a governance read-model
//! for one consumer. Co-locating them was the only reason `wiki` depended on
//! `models` at all. The payload is a rendering of the readout, so it belongs with
//! the readout, and `wiki::hook` goes back to owning only the artifacts it writes.
//!
//! Since [CR-096] the violations half is **dated**: it renders the age and the
//! `HEAD` of the [FR-GV-21] run marker the readout carries, states a *recorded*
//! clean check, and names a tree that has moved since the run. Every one of
//! those is rendered from the read-model alone — no clock is read and no
//! subprocess is spawned here — so each case is a constructible fixture.
//!
//! [FR-GV-21]: ../../../docs/specs/requirements/FR-GV-21.md
//! [FR-IN-07]: ../../../docs/specs/requirements/FR-IN-07.md
//! [CR-095]: ../../../docs/requests/CR-095-session-start-quality-readout.md
//! [CR-096]: ../../../docs/requests/CR-096-recorded-check-marker.md

use crate::models::quality::{CheckRun, QualityReadout};

/// How many violation messages the readout lists before truncating. The total is
/// always reported alongside, and a truncated list says what it dropped — a
/// silent cap would let a reader treat the visible subset as the whole set.
const READOUT_MESSAGE_CAP: usize = 20;

/// The agent host's session-start hook payload ([CR-095]).
///
/// Built and serialized **in the binary**, not assembled by the hook script.
/// The script is a three-line launcher precisely because this is not a job for
/// shell: the host discards the entire readout if the JSON is malformed, so
/// every escaping edge — a control character in a rule message, a backslash in a
/// Windows path, a quote, a newline — has to be handled correctly, and
/// `serde_json` already does that. Hand-rolled `sed`/`awk` escaping in the
/// script got each of those wrong.
///
/// `hook_event_name` is a fixed `&'static str` rather than a field a caller
/// supplies: the host's output parser rejects the whole payload when the event
/// name does not match the event that fired, so making it unspellable-wrong is
/// worth more than the flexibility.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct HookPayload {
    /// The one-line readout the host renders to the user.
    #[serde(rename = "systemMessage")]
    pub system_message: String,
    #[serde(rename = "hookSpecificOutput")]
    pub hook_specific_output: HookSpecificOutput,
}

/// The event-scoped half of [`HookPayload`] — the agent-visible context.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct HookSpecificOutput {
    /// Always `"SessionStart"`; see [`HookPayload`].
    #[serde(rename = "hookEventName")]
    pub hook_event_name: &'static str,
    /// The full readout, handed to the agent as session context.
    #[serde(rename = "additionalContext")]
    pub additional_context: String,
}

/// The conventional 7-char short form of a commit SHA for human-facing text.
/// Shorter strings pass through unchanged (a test fixture or an unusual ref).
///
/// Mirrors the private helper of the same name in the coverage read-model,
/// whose artifact-vs-`HEAD` staleness prompt is the sibling this whole
/// rendering follows. Duplicated rather than shared because that one is a
/// module-private detail of an unrelated read-model; a 7-char truncation is
/// not a seam worth cutting across two components to reuse.
fn short_sha(sha: &str) -> String {
    sha.chars().take(7).collect()
}

/// Render an age in seconds as the coarse human phrase the readout uses —
/// `4 minutes ago`, `6 days ago`.
///
/// Deliberately coarse: the readout exists to tell a live finding from an
/// archaeological one, and a reader acts on "days" and "minutes", not on
/// seconds. The largest unit that fits wins, truncating rather than rounding,
/// so the phrase never overstates how recent a run was.
///
/// An age that cannot be true is **named, not rendered**. Two cases, both of
/// which mean the stored timestamp is wrong rather than old:
/// - **Negative** — the marker is stamped after now (a store carried between
///   machines, a clock stepped back). Rendering it as "just now" would invent
///   the most reassuring reading of a fact the readout cannot establish.
/// - **Implausibly large** — a run older than [`IMPLAUSIBLE_AGE_SECS`] predates
///   any plausible project. It arrives from a corrupted or hand-edited row (the
///   producer saturates rather than overflowing on one), and "106751991167300
///   days ago" is a fabricated precision, not a fact.
fn render_age(seconds: i64) -> String {
    const MINUTE: i64 = 60;
    const HOUR: i64 = 60 * MINUTE;
    const DAY: i64 = 24 * HOUR;
    /// A century. Past this the stored value is wrong, not merely old — no
    /// `check` run predates the tool by decades.
    const IMPLAUSIBLE_AGE_SECS: i64 = 100 * 365 * DAY;

    if seconds < 0 {
        return "at an unknown age (recorded ahead of now — check the clock)".to_string();
    }
    if seconds > IMPLAUSIBLE_AGE_SECS {
        return "at an unknown age (the recorded time is implausibly old — check the store)"
            .to_string();
    }
    let (count, unit) = match seconds {
        s if s < MINUTE => return "just now".to_string(),
        s if s < HOUR => (s / MINUTE, "minute"),
        s if s < DAY => (s / HOUR, "hour"),
        s => (s / DAY, "day"),
    };
    let plural = if count == 1 { "" } else { "s" };
    format!("{count} {unit}{plural} ago")
}

/// The `at HEAD <sha>, <age>` clause naming when and against what a run was
/// measured, plus the moved-tree clause when `HEAD` has since changed.
///
/// A `None` `commit_sha` omits the `at HEAD` half rather than rendering a
/// placeholder: the run genuinely has no recorded `HEAD` (no git, no commits,
/// or a record recovered from pre-marker rows), and inventing one would be the
/// fabrication this readout exists to refuse.
fn render_provenance(check: &CheckRun) -> String {
    let mut clause = match &check.commit_sha {
        Some(sha) => format!("at HEAD {}, {}", short_sha(sha), render_age(check.age_seconds)),
        None => render_age(check.age_seconds),
    };
    // Staleness is a property of the tree, not just of the clock: with only an
    // age, a check against the current tree and one against a since-changed
    // tree render identically ([CR-096] §3.2).
    if check.tree_moved {
        clause.push_str("; measured against a different tree");
        if let Some(head) = &check.head_sha {
            clause.push_str(&format!(" — HEAD is now {}", short_sha(head)));
        }
    }
    clause
}

/// Whether this record licenses stating a **clean** check ([BR-41]).
///
/// Both halves are required, and the conjunction is the point: the marker must
/// record a run that found nothing (`Some(0)` — the state an empty table cannot
/// express), *and* the rows it wrote must actually be absent. A marker claiming
/// findings over an empty table, or the reverse, is a store that disagrees with
/// itself; `quality_readout` warns about it and this returns `false`, so the
/// disagreement is never resolved in favour of the most reassuring reading.
fn is_recorded_clean(readout: &QualityReadout, check: &CheckRun) -> bool {
    check.recorded_count == Some(0) && readout.violation_count.is_none()
}

/// The count to lead the violations line with: the rows when there are any,
/// otherwise whatever the marker recorded. They agree on every store one
/// transaction wrote; when they do not, `quality_readout` has already raised a
/// warning and this prefers the larger, less flattering figure.
fn headline_count(readout: &QualityReadout, check: Option<&CheckRun>) -> Option<i64> {
    let rows = readout.violation_count.map(|c| c as i64);
    let recorded = check.and_then(|c| c.recorded_count);
    match (rows, recorded) {
        (Some(rows), Some(recorded)) => Some(rows.max(recorded)),
        (rows, recorded) => rows.or(recorded),
    }
}

/// Render the one-line, user-visible summary of a readout ([CR-095]).
///
/// Every absent value is named rather than defaulted: an empty graph reads
/// `signal n/a`, not `signal 0`; a check nobody ran reads `violations none
/// recorded (no check has run)`, not `0 violations`. A check that demonstrably
/// ran and found nothing is stated as clean — with its age, and never bare
/// ([CR-096], [BR-41]).
fn render_summary(readout: &QualityReadout) -> String {
    let mut parts = vec![match readout.signal {
        Some(signal) => format!("signal {signal}"),
        None => "signal n/a (empty graph)".to_string(),
    }];
    match (readout.baseline_signal, readout.delta) {
        (Some(baseline), Some(delta)) => {
            parts.push(format!("baseline {baseline}"));
            parts.push(format!("delta {delta}"));
        }
        (Some(baseline), None) => parts.push(format!("baseline {baseline} (not comparable)")),
        (None, _) => parts.push("no baseline saved".to_string()),
    }
    parts.push(match (&readout.check, headline_count(readout, readout.check.as_ref())) {
        // A recorded clean run: the assertion [FR-IN-07] was previously
        // forbidden from making, and it never appears unqualified — the age it
        // was measured at rides with it on the same line.
        (Some(check), _) if is_recorded_clean(readout, check) => {
            let mut part = format!("clean check {}", render_age(check.age_seconds));
            if check.tree_moved {
                part.push_str(" (different tree)");
            }
            part
        }
        (Some(check), Some(count)) => {
            let mut part = format!("{count} violation(s), checked {}", render_age(check.age_seconds));
            if check.tree_moved {
                part.push_str(" (different tree)");
            }
            part
        }
        // No marker and no rows: nothing knows of a run. Absence of the marker
        // is absence of knowledge, never a clean bill of health ([BR-41]).
        _ => "violations none recorded (no check has run)".to_string(),
    });
    format!("logos quality report: {}", parts.join(" · "))
}

/// Render the full multi-line readout handed to the agent ([CR-095]).
fn render_readout(readout: &QualityReadout) -> String {
    let mut out = String::from("logos quality report (session start)\n");
    match readout.signal {
        Some(signal) => out.push_str(&format!("  signal:   {signal}\n")),
        None => out.push_str("  signal:   n/a (empty graph)\n"),
    }
    match readout.baseline_signal {
        Some(baseline) => {
            out.push_str(&format!("  baseline: {baseline}\n"));
            match readout.delta {
                Some(delta) => out.push_str(&format!("  delta:    {delta}\n")),
                None => out.push_str("  delta:    n/a (baseline not comparable)\n"),
            }
        }
        None => out.push_str("  baseline: n/a (none saved — bless one with `logos gate --save`)\n"),
    }

    // The violations half is as of the last recorded `check_rules` run, and says
    // so — it is deliberately not re-evaluated, because that would be a write.
    // Since [CR-096] it also says *when* that run was and what tree it saw, so
    // the staleness is quantified rather than merely labelled.
    // Computed ONCE and reused below. The headline and the "not shown" note
    // must be two views of ONE number: when they were derived separately — the
    // headline from `headline_count`, the note from `violation_count` — a
    // marker recording 9 over a table holding 1 printed "rule violations: 9",
    // listed one, and added no note at all, hiding 8 findings behind a heading
    // that named them. That is precisely the silent cap the cap exists to
    // prevent, reintroduced by splitting the source.
    let headline = headline_count(readout, readout.check.as_ref());
    match (&readout.check, headline) {
        (Some(check), _) if is_recorded_clean(readout, check) => out.push_str(&format!(
            "  rule violations: 0 — clean `logos check` {}\n",
            render_provenance(check)
        )),
        (Some(check), Some(total)) => {
            out.push_str(&format!(
                "  rule violations: {total} (as of `logos check` {})\n",
                render_provenance(check)
            ));
        }
        // No marker and no rows. The disjunction [CR-095] had to render here
        // ("a clean check, or none has run") is now resolved: the marker's
        // absence means no run happened.
        _ => out.push_str("  rule violations: none recorded (no `logos check` has run)\n"),
    }
    // Listed under whichever line was rendered above — a recorded-clean run has
    // nothing to list, and a store that disagrees with itself still shows the
    // rows it actually holds.
    if let Some(messages) = &readout.violations {
        for message in messages {
            out.push_str(&format!("    - {message}\n"));
        }
        // Never a silent cap: a reader must not mistake the listed subset for
        // the whole set. Counted against the headline, so every finding the
        // heading claims is either listed or explicitly accounted for.
        let dropped = headline
            .unwrap_or_default()
            .saturating_sub(messages.len() as i64);
        if dropped > 0 {
            out.push_str(&format!("    … {dropped} more not shown\n"));
        }
    }

    if !readout.freshness.is_empty() {
        out.push_str(&format!("  freshness: {}\n", readout.freshness));
    }
    for warning in &readout.warnings {
        out.push_str(&format!("  note: {warning}\n"));
    }
    out
}

/// Build the session-start hook payload from a quality readout ([FR-IN-07],
/// [CR-095]).
#[must_use]
pub fn session_start_payload(readout: &QualityReadout) -> HookPayload {
    HookPayload {
        system_message: render_summary(readout),
        hook_specific_output: HookSpecificOutput {
            hook_event_name: "SessionStart",
            additional_context: render_readout(readout),
        },
    }
}

/// The readout's message cap ([`READOUT_MESSAGE_CAP`]), for the [`Engine`] method
/// that assembles a readout to feed [`session_start_payload`].
///
/// [`Engine`]: crate::Engine
#[must_use]
pub fn message_cap() -> usize {
    READOUT_MESSAGE_CAP
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    // ── The session-start payload ([CR-095]) ─────────────────────────────────
    //
    // Rendering is tested here, in Rust, against constructed `QualityReadout`
    // values rather than through the script: the script no longer computes
    // anything, and these are the cases a shell fake could never reach —
    // control characters, backslashes, a cap overflow, an absent signal.

    /// A readout with everything present, for tests that vary one axis.
    fn full_readout() -> QualityReadout {
        QualityReadout {
            signal: Some(8234),
            baseline_signal: Some(8100),
            delta: Some(134),
            freshness: "assumed-fresh (no reconcile)".to_string(),
            violations: Some(vec!["max_cc: foo is 31".to_string()]),
            violation_count: Some(1),
            check: Some(marker(1, 6 * 86_400)),
            warnings: Vec::new(),
        }
    }

    /// A [FR-GV-21] marker recording `count` violations `age` seconds ago, at a
    /// `HEAD` that has **not** moved since.
    fn marker(count: i64, age: i64) -> CheckRun {
        CheckRun {
            ran_at: 1_700_000_000,
            age_seconds: age,
            commit_sha: Some("ff657f5aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string()),
            head_sha: Some("ff657f5aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string()),
            tree_moved: false,
            recorded_count: Some(count),
        }
    }

    /// The same marker, but `HEAD` has moved on since the run.
    fn marker_on_a_moved_tree(count: i64, age: i64) -> CheckRun {
        CheckRun {
            head_sha: Some("a1b2c3d9999999999999999999999999999999999".to_string()),
            tree_moved: true,
            ..marker(count, age)
        }
    }

    /// Serialize a payload and read it back — what the host actually does.
    fn round_trip(readout: &QualityReadout) -> Value {
        let json = serde_json::to_string(&session_start_payload(readout)).expect("serialise");
        serde_json::from_str(&json)
            .unwrap_or_else(|e| panic!("the host must be able to parse the payload ({e}): {json}"))
    }

    /// The payload's shape is the one the host's parser demands, and both
    /// channels are populated: the one-line `systemMessage` the user sees and the
    /// full `additionalContext` the agent gets.
    #[test]
    fn payload_carries_both_channels_under_the_exact_event_name() {
        let payload = round_trip(&full_readout());
        assert_eq!(
            payload["hookSpecificOutput"]["hookEventName"], "SessionStart",
            "the host's parser rejects any other event name"
        );
        let summary = payload["systemMessage"].as_str().expect("systemMessage is a string");
        let context = payload["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .expect("additionalContext is a string");

        assert!(summary.contains("signal 8234"), "summary names the signal: {summary}");
        assert!(summary.contains("baseline 8100"), "summary names the baseline: {summary}");
        assert!(summary.contains("delta 134"), "summary names the delta: {summary}");
        assert!(summary.contains("1 violation"), "summary names the count: {summary}");
        assert!(!summary.contains('\n'), "the user-visible line is one line: {summary:?}");

        assert!(context.contains("signal:   8234"), "{context}");
        assert!(context.contains("baseline: 8100"), "{context}");
        assert!(context.contains("delta:    134"), "{context}");
        assert!(context.contains("rule violations: 1"), "{context}");
        assert!(context.contains("max_cc: foo is 31"), "the message is listed: {context}");
        assert!(context.contains("assumed-fresh"), "freshness line: {context}");
    }

    /// A regressed signal renders its negative delta rather than being read as a
    /// failure. `gate` exits 1 on a regression and `check` on an error violation,
    /// both by design ([FR-GV-03]) — a regressed-but-readable graph is healthy
    /// data, not an unavailable one.
    #[test]
    fn a_regression_renders_as_a_negative_delta_not_a_failure() {
        let readout = QualityReadout {
            signal: Some(7900),
            baseline_signal: Some(8100),
            delta: Some(-200),
            ..full_readout()
        };
        let payload = round_trip(&readout);
        let context = payload["hookSpecificOutput"]["additionalContext"].as_str().unwrap();
        assert!(context.contains("signal:   7900"), "the regressed signal: {context}");
        assert!(context.contains("delta:    -200"), "a negative delta: {context}");
        for absent in ["unavailable", "n/a"] {
            assert!(
                !context.contains(absent),
                "a regression is not a degradation ({absent}): {context}"
            );
        }
    }

    /// Nothing absent is ever defaulted: an empty graph reads `n/a`, not `0`; no
    /// baseline reads "none saved", not a delta against zero; and an unrecorded
    /// check reads "none recorded", never a truthful-looking "0 violations" —
    /// which would assert a passing check that may never have run.
    #[test]
    fn payload_never_fabricates_an_absent_value() {
        let empty = QualityReadout::default();
        let payload = round_trip(&empty);
        let summary = payload["systemMessage"].as_str().unwrap();
        let context = payload["hookSpecificOutput"]["additionalContext"].as_str().unwrap();

        assert!(summary.contains("signal n/a"), "an empty graph is n/a: {summary}");
        assert!(summary.contains("no baseline saved"), "{summary}");
        assert!(summary.contains("violations none recorded"), "{summary}");
        assert!(
            !summary.contains("signal 0") && !summary.contains("0 violation"),
            "never a zeroed readout rendered as fact: {summary}"
        );
        assert!(context.contains("signal:   n/a"), "{context}");
        assert!(context.contains("baseline: n/a"), "{context}");
        assert!(
            context.contains("bless one with `logos gate --save`"),
            "an absent baseline says how to create one: {context}"
        );
        assert!(context.contains("rule violations: none recorded"), "{context}");
        assert!(!context.contains("delta:"), "no delta without a baseline: {context}");
        // An empty freshness string contributes no dangling label.
        assert!(!context.contains("freshness:"), "{context}");
    }

    /// A baseline that exists but is not comparable (a different metric version
    /// or threshold set) is named as such, with no delta invented across the
    /// incompatibility — and the warning explaining it is surfaced.
    #[test]
    fn an_incomparable_baseline_yields_no_delta() {
        let readout = QualityReadout {
            delta: None,
            warnings: vec!["baseline scored under different thresholds".to_string()],
            ..full_readout()
        };
        let payload = round_trip(&readout);
        let summary = payload["systemMessage"].as_str().unwrap();
        let context = payload["hookSpecificOutput"]["additionalContext"].as_str().unwrap();
        assert!(summary.contains("baseline 8100 (not comparable)"), "{summary}");
        assert!(!summary.contains("delta"), "no delta across an incomparability: {summary}");
        assert!(context.contains("delta:    n/a (baseline not comparable)"), "{context}");
        assert!(
            context.contains("note: baseline scored under different thresholds"),
            "the reason is surfaced, not swallowed: {context}"
        );
    }

    /// A capped violation list says what it dropped. A silent cap would let a
    /// reader treat the visible subset as the whole set — the count is always the
    /// pre-cap total.
    #[test]
    fn a_capped_violation_list_says_what_it_dropped() {
        let listed: Vec<String> = (0..READOUT_MESSAGE_CAP).map(|i| format!("v{i}")).collect();
        let readout = QualityReadout {
            violations: Some(listed),
            violation_count: Some(READOUT_MESSAGE_CAP + 7),
            ..full_readout()
        };
        let payload = round_trip(&readout);
        let context = payload["hookSpecificOutput"]["additionalContext"].as_str().unwrap();
        assert!(
            context.contains(&format!("rule violations: {}", READOUT_MESSAGE_CAP + 7)),
            "the count is the pre-cap total: {context}"
        );
        assert!(context.contains("… 7 more not shown"), "the cap is named: {context}");
        assert!(context.contains("- v0") && context.contains("- v19"), "{context}");

        // An uncapped list adds no phantom "more not shown" line.
        let exact = QualityReadout {
            violations: Some(vec!["only".to_string()]),
            violation_count: Some(1),
            ..full_readout()
        };
        let context = round_trip(&exact)["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(!context.contains("not shown"), "{context}");
    }

    /// The bytes a rule message can actually contain — a quote, a backslash, a
    /// newline, a raw control character, a non-BMP char — survive into a payload
    /// the host can parse. This is the whole reason the payload moved into the
    /// binary: a malformed payload makes the host discard the readout silently,
    /// and the shell predecessor got every one of these wrong.
    #[test]
    fn payload_survives_the_bytes_a_rule_message_can_contain() {
        let nasty = "bad \"x\" import\\path\n\tline\u{7}bell \u{1f600} é";
        let readout = QualityReadout {
            violations: Some(vec![nasty.to_string()]),
            violation_count: Some(1),
            freshness: nasty.to_string(),
            warnings: vec![nasty.to_string()],
            ..full_readout()
        };
        // `round_trip` panics if the host could not parse this.
        let payload = round_trip(&readout);
        let context = payload["hookSpecificOutput"]["additionalContext"].as_str().unwrap();
        assert!(
            context.contains(nasty),
            "the message survives serialisation byte-for-byte: {context:?}"
        );
    }

    // ── The [CR-096] dated / clean / unchecked readout ([UAT-GV-13]) ─────────

    /// The context and summary of a readout, in one call — every case below
    /// asserts on both channels, because a fact stated to the agent and
    /// withheld from the user (or the reverse) is half a readout.
    fn channels(readout: &QualityReadout) -> (String, String) {
        let payload = round_trip(readout);
        (
            payload["systemMessage"].as_str().expect("systemMessage").to_string(),
            payload["hookSpecificOutput"]["additionalContext"]
                .as_str()
                .expect("additionalContext")
                .to_string(),
        )
    }

    /// A marker recording a clean run lets the readout **state the clean
    /// check** — the assertion [FR-IN-07] previously forbade outright — and it
    /// is never stated bare: the `HEAD` and the age it was measured at ride on
    /// the same line, so a reader can tell a current pass from an old one.
    #[test]
    fn a_recorded_clean_run_is_stated_as_clean_with_its_head_and_age() {
        let readout = QualityReadout {
            violations: None,
            violation_count: None,
            check: Some(marker(0, 4 * 60)),
            ..full_readout()
        };
        let (summary, context) = channels(&readout);

        assert!(
            context.contains("rule violations: 0 — clean `logos check` at HEAD ff657f5, 4 minutes ago"),
            "the clean check is stated, dated and attributed: {context}"
        );
        assert!(
            summary.contains("clean check 4 minutes ago"),
            "the user-visible line states it too: {summary}"
        );
        assert!(
            !context.contains("none recorded") && !summary.contains("none recorded"),
            "a recorded clean run is knowledge, not absence of it: {context}"
        );
        assert!(
            !context.contains("different tree"),
            "HEAD has not moved, so no staleness clause is invented: {context}"
        );
    }

    /// A marker recording findings dates them: the count, the run's age and the
    /// `HEAD` it was measured at, replacing [CR-095]'s undated "as of the last
    /// `logos check`".
    #[test]
    fn recorded_violations_are_dated_and_attributed_to_a_head() {
        let (summary, context) = channels(&full_readout());
        assert!(
            context.contains("rule violations: 1 (as of `logos check` at HEAD ff657f5, 6 days ago)"),
            "the count carries its age and HEAD: {context}"
        );
        assert!(context.contains("- max_cc: foo is 31"), "the message is still listed: {context}");
        assert!(
            summary.contains("1 violation(s), checked 6 days ago"),
            "{summary}"
        );
        assert!(
            !context.contains("as of the last `logos check`"),
            "the undated phrasing is gone: {context}"
        );
    }

    /// The case a timestamp alone cannot draw, and the whole reason `HEAD` is
    /// recorded: a run that is not merely old but was measured against a
    /// **different tree**. Without this clause, a check against the current
    /// tree and one against a since-changed tree render identically.
    #[test]
    fn a_moved_head_says_the_finding_was_measured_against_a_different_tree() {
        let violations = QualityReadout {
            check: Some(marker_on_a_moved_tree(1, 6 * 86_400)),
            ..full_readout()
        };
        let (summary, context) = channels(&violations);
        assert!(
            context.contains("measured against a different tree — HEAD is now a1b2c3d"),
            "the moved tree is named, with the HEAD it moved to: {context}"
        );
        assert!(summary.contains("(different tree)"), "{summary}");

        // The clean case carries the same qualification — a recorded pass over
        // a tree that has since moved is exactly the over-trusted readout.
        let clean = QualityReadout {
            violations: None,
            violation_count: None,
            check: Some(marker_on_a_moved_tree(0, 4 * 60)),
            ..full_readout()
        };
        let (summary, context) = channels(&clean);
        assert!(
            context.contains("clean `logos check` at HEAD ff657f5, 4 minutes ago; \
                              measured against a different tree — HEAD is now a1b2c3d"),
            "a clean check is never left looking current when the tree has moved: {context}"
        );
        assert!(summary.contains("clean check 4 minutes ago (different tree)"), "{summary}");
    }

    /// No marker and no rows: nothing knows of a run, and the readout says
    /// exactly that. [CR-095] had to render a disjunction here ("a clean
    /// `logos check`, or none has run") because an empty table could not tell
    /// the two apart; the marker's absence now resolves it ([BR-41]).
    #[test]
    fn an_absent_marker_reads_as_no_check_has_run_never_as_clean() {
        let readout = QualityReadout::default();
        let (summary, context) = channels(&readout);
        assert!(
            context.contains("rule violations: none recorded (no `logos check` has run)"),
            "absence of the marker is absence of knowledge: {context}"
        );
        assert!(summary.contains("violations none recorded (no check has run)"), "{summary}");
        for fabricated in ["clean", "0 violation", "different tree"] {
            assert!(
                !context.contains(fabricated) && !summary.contains(fabricated),
                "nothing is asserted about a run that never happened ({fabricated}): {context}"
            );
        }
    }

    /// A store written before the marker migration has violation rows but no
    /// marker. The rows carry their own run time, so the findings are dated
    /// immediately — but with no marker there is no recorded `HEAD`, so the
    /// readout attributes them to no tree and asserts no clean check.
    #[test]
    fn a_pre_migration_record_dates_its_rows_but_claims_no_head_and_no_clean_run() {
        let readout = QualityReadout {
            check: Some(CheckRun {
                ran_at: 1_700_000_000,
                age_seconds: 6 * 86_400,
                commit_sha: None,
                head_sha: Some("a1b2c3d9999999999999999999999999999999999".to_string()),
                tree_moved: false,
                recorded_count: None,
            }),
            ..full_readout()
        };
        let (_, context) = channels(&readout);
        assert!(
            context.contains("rule violations: 1 (as of `logos check` 6 days ago)"),
            "the rows date themselves with no marker present: {context}"
        );
        assert!(
            !context.contains("at HEAD"),
            "no HEAD was ever recorded for this run, so none is claimed: {context}"
        );
        assert!(
            !context.contains("different tree"),
            "a tree comparison needs a recorded HEAD to compare against: {context}"
        );
    }

    /// A marker whose `commit_sha` is NULL — a tree with no resolvable `HEAD`
    /// at run time — omits the tree comparison rather than rendering a
    /// placeholder, while still dating the run.
    #[test]
    fn a_null_commit_sha_omits_the_tree_comparison_rather_than_faking_one() {
        let readout = QualityReadout {
            violations: None,
            violation_count: None,
            check: Some(CheckRun {
                commit_sha: None,
                ..marker(0, 30)
            }),
            ..full_readout()
        };
        let (_, context) = channels(&readout);
        assert!(
            context.contains("rule violations: 0 — clean `logos check` just now"),
            "a clean run with no recorded HEAD is still stated, and still dated: {context}"
        );
        for placeholder in ["at HEAD", "different tree", "unknown", "None", "null"] {
            assert!(
                !context.contains(placeholder),
                "no placeholder stands in for the absent sha ({placeholder}): {context}"
            );
        }
    }

    /// The age phrase takes the largest unit that fits and **truncates**, so it
    /// never overstates how recent a run was; a negative age (a marker stamped
    /// after now) is named rather than clamped into a plausible "just now".
    #[test]
    fn the_age_phrase_truncates_downwards_and_names_a_skewed_clock() {
        for (seconds, expected) in [
            (0_i64, "just now"),
            (59, "just now"),
            (60, "1 minute ago"),
            (119, "1 minute ago"),
            (3_599, "59 minutes ago"),
            (3_600, "1 hour ago"),
            (86_399, "23 hours ago"),
            (86_400, "1 day ago"),
            (6 * 86_400 + 86_399, "6 days ago"),
        ] {
            assert_eq!(render_age(seconds), expected, "age of {seconds}s");
        }
        let skewed = render_age(-5);
        assert!(
            skewed.contains("recorded ahead of now"),
            "a skewed clock is named, not rendered as a plausible age: {skewed}"
        );
        assert!(!skewed.contains("ago"), "and is not phrased as an age at all: {skewed}");

        // The other end: a saturated age from a corrupted row is named too,
        // rather than rendered as a confident count of a hundred million days.
        let absurd = render_age(i64::MAX);
        assert!(
            absurd.contains("implausibly old"),
            "an impossible age is named, not counted: {absurd}"
        );
        assert!(!absurd.contains("ago"), "and is not phrased as an age: {absurd}");
        // The boundary still renders normally — a century is implausible, a
        // decade is merely stale.
        assert_eq!(render_age(10 * 365 * 24 * 60 * 60), "3650 days ago");
    }

    /// A marker and the rows it supposedly wrote disagreeing is a store that
    /// disagrees with itself. The readout never resolves that in favour of the
    /// reassuring reading: a marker claiming a clean run over a table holding
    /// findings renders the findings, not "clean".
    #[test]
    fn a_marker_disagreeing_with_its_rows_never_renders_as_clean() {
        let readout = QualityReadout {
            violations: Some(vec!["max_cc: foo is 31".to_string()]),
            violation_count: Some(1),
            check: Some(marker(0, 60)),
            ..full_readout()
        };
        let (summary, context) = channels(&readout);
        assert!(
            !context.contains("clean") && !summary.contains("clean"),
            "rows exist, so no clean check is asserted: {context}"
        );
        assert!(context.contains("rule violations: 1 "), "the stored rows are reported: {context}");

        // The reverse disagreement — a marker claiming findings over an empty
        // table — leads with the marker's larger figure rather than silently
        // reporting the flattering empty one.
        let reversed = QualityReadout {
            violations: None,
            violation_count: None,
            check: Some(marker(3, 60)),
            ..full_readout()
        };
        let (_, context) = channels(&reversed);
        assert!(
            context.contains("rule violations: 3 "),
            "the marker's count is reported when the rows are gone: {context}"
        );
        assert!(!context.contains("clean"), "and never as clean: {context}");

        // Partial row loss — the marker counted 9, only 1 row survives. The
        // line leads with the larger figure: under-reporting a breach is the
        // failure that costs something, over-reporting one is merely noisy.
        let partial = QualityReadout {
            violations: Some(vec!["max_cc: foo is 31".to_string()]),
            violation_count: Some(1),
            check: Some(marker(9, 60)),
            ..full_readout()
        };
        let (_, context) = channels(&partial);
        assert!(
            context.contains("rule violations: 9 "),
            "the larger, less flattering of the two disagreeing counts leads: {context}"
        );
        // And the heading is reconciled with what is listed. A heading claiming
        // 9 above a single listed finding, with no note, hides 8 of them behind
        // a number that names them.
        assert!(
            context.contains("… 8 more not shown"),
            "every finding the heading claims is listed or accounted for: {context}"
        );
    }

    /// A short or unusual `HEAD` string passes through the 7-char shortening
    /// unchanged rather than panicking on a slice boundary — the readout must
    /// survive whatever a ref resolves to.
    #[test]
    fn a_short_or_multibyte_sha_survives_shortening() {
        assert_eq!(short_sha("abc"), "abc");
        assert_eq!(short_sha("ff657f5aaaa"), "ff657f5");
        // Char-wise, not byte-wise: a byte slice would panic mid-codepoint.
        assert_eq!(short_sha("ééééééééé"), "ééééééé");
    }
}
