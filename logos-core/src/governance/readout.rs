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
//! Since [CR-140] the violations half also states **what the run evaluated**.
//! A recorded `0` over an empty evaluated set is no longer rendered as a clean
//! check — [FR-GV-03]'s *"clean never means nothing was evaluated"* — and the
//! four states it can be in (no contract, a contract authoring no rules, a
//! clean run over N rules, and a marker that recorded no evaluated set at all)
//! are each named. The line names **no command**: the marker is written by
//! `replace_violations`, which `scan` calls too, so any command name in it is
//! an attribution the record cannot support.
//!
//! Since [CR-138] the **signal** half names its own absence the same way: an
//! absent signal reads the cause the readout's discriminant established
//! ([`SignalAbsence`]) rather than the one cause it used to assume, and carries
//! the figure establishing it. The classification is not made here — it arrives
//! on the read-model, so this stays a pure function of it.
//!
//! [FR-GV-21]: ../../../docs/specs/requirements/FR-GV-21.md
//! [FR-IN-07]: ../../../docs/specs/requirements/FR-IN-07.md
//! [CR-095]: ../../../docs/requests/CR-095-session-start-quality-readout.md
//! [CR-096]: ../../../docs/requests/CR-096-recorded-check-marker.md
//! [FR-GV-03]: ../../../docs/specs/requirements/FR-GV-03.md
//! [CR-138]: ../../../docs/requests/CR-138-a-readout-names-the-cause-its-gating-condition-establishes.md
//! [CR-140]: ../../../docs/requests/CR-140-the-recorded-check-marker-carries-what-it-evaluated.md

use crate::models::quality::{CheckRun, EvaluatedSetAbsence, QualityReadout, SignalAbsence};

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

/// The denominator a **clean** result may be stated over, or `None` when this
/// record licenses no clean result at all ([BR-41], [FR-GV-03], [CR-140] §3.2).
///
/// Three conditions, and the conjunction is the point:
/// - the marker records a run that found nothing (`Some(0)` — the state an
///   empty table cannot express);
/// - the rows it wrote are actually absent. A marker claiming findings over an
///   empty table, or the reverse, is a store that disagrees with itself;
///   `quality_readout` warns about it and this returns `None`, so the
///   disagreement is never resolved in favour of the most reassuring reading;
/// - **a non-empty evaluated set was recorded**. [FR-GV-03] defines clean as
///   *"a contract was evaluated and held; it never means nothing was
///   evaluated"*, so a `0` over no denominator — an absent contract, a contract
///   authoring no rules, or a marker that recorded no denominator at all — is a
///   vacuous run. This third condition is what `notes/sprint-test-72.md`
///   Finding 1a reproduced the absence of: the same binary printing *"nothing
///   was evaluated"* and *"clean check"* in one breath.
///
/// # Why this returns the denominator rather than a `bool`
///
/// So that the licence and the figure are **one** decision. The first draft
/// returned `bool` with the third condition spelled `checked_rules.is_some()`,
/// and the caller then re-read `checked_rules` to get the number — which made
/// the conjunct dead: deleting it changed no rendered line, because the
/// caller's own match on `checked_rules` was already doing the work. A
/// falsifiability mutation dropping that condition survived the whole suite.
/// Returning the figure makes stating a clean result impossible without the
/// denominator it is clean over, rather than merely checked twice.
///
/// [FR-GV-03]: ../../../docs/specs/requirements/FR-GV-03.md
/// [CR-140]: ../../../docs/requests/CR-140-the-recorded-check-marker-carries-what-it-evaluated.md
fn recorded_clean_over(readout: &QualityReadout, check: &CheckRun) -> Option<u32> {
    if check.recorded_count != Some(0) || readout.violation_count.is_some() {
        return None;
    }
    check.checked_rules
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

/// What the last run evaluated: the denominator, or the named absence of one
/// ([CR-140] §3.2, [FR-IN-07]).
///
/// One helper for both channels and for every arm of the violations line, for
/// the reason [`render_signal_absence`] consolidates the signal's clause: a
/// sentence two channels derive separately is a pair that can drift, and on
/// this very line that drift shipped.
///
/// **No arm names a command.** The marker is written by `replace_violations`,
/// which both `check` and `scan` call, so naming one would attribute the run to
/// a command that may never have been invoked — `notes/sprint-test-72.md`
/// Finding 1b, reproduced through `init` / `index` / `scan` with no `check` at
/// any point. [FR-IN-07]'s appended criterion drops the name rather than
/// correcting it: the `HEAD`, the age and the denominator are the actionable
/// halves.
///
/// The `(None, None)` case — no figure and no cause — is reachable only from a
/// record assembled without the discriminant, and it names the set unknown
/// without attributing a reason, exactly as an unclassified [`SignalAbsence`]
/// names no cause.
///
/// [FR-IN-07]: ../../../docs/specs/requirements/FR-IN-07.md
/// [CR-140]: ../../../docs/requests/CR-140-the-recorded-check-marker-carries-what-it-evaluated.md
fn render_evaluated_set(check: &CheckRun) -> String {
    match (check.checked_rules, check.evaluated_absence.as_ref()) {
        (Some(rules), _) => format!("{rules} rule(s) evaluated"),
        (None, Some(EvaluatedSetAbsence::NoContract)) => "no rules contract authored".to_string(),
        (None, Some(EvaluatedSetAbsence::NoRulesAuthored)) => {
            "a rules contract present, authoring no rules".to_string()
        }
        (None, Some(EvaluatedSetAbsence::Unrecorded)) => {
            "evaluated set unknown (this marker predates its recording)".to_string()
        }
        (None, None) => "evaluated set unknown".to_string(),
    }
}

/// The violations verdict — **one** spelling, both channels ([CR-140] §3.2).
///
/// `provenance` is the channel's own trailing clause: the full readout carries
/// the `HEAD` and the age ([`render_provenance`]), the one-line summary carries
/// the age alone. Everything before it — which state the run was in, and what
/// it evaluated — is decided here exactly once. The two channels previously
/// built that sentence separately; consolidating it is the same treatment
/// `headline_count` already gives the number, and for the same reason recorded
/// there.
///
/// The five states, and why each is its own ([FR-IN-07], [FR-GV-03]):
/// - **no run known of** — the marker's absence is absence of knowledge, never
///   a pass ([BR-41]);
/// - **clean over N > 0** — the only genuinely clean state, and the figure
///   carries its denominator ([NFR-CC-04]);
/// - **nothing found, but no denominator recorded** — an absent contract, a
///   contract authoring no rules, or a marker predating the evaluated set. It
///   is rendered as that state: never as clean, and never as a bare `0`, which
///   is the favourable reading of an absent fact this whole line exists to
///   stop;
/// - **findings over N > 0** — the count with its denominator;
/// - **findings with no denominator** — the count is reported, the denominator
///   named absent rather than assumed.
///
/// [BR-41]: ../../../docs/specs/software-spec.md#4-cross-cutting-non-functional-requirements
/// [FR-GV-03]: ../../../docs/specs/requirements/FR-GV-03.md
/// [FR-IN-07]: ../../../docs/specs/requirements/FR-IN-07.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
/// [CR-140]: ../../../docs/requests/CR-140-the-recorded-check-marker-carries-what-it-evaluated.md
fn render_violations(
    readout: &QualityReadout,
    headline: Option<i64>,
    provenance: &dyn Fn(&CheckRun) -> String,
) -> String {
    let Some(check) = readout.check.as_ref() else {
        // "rule check" is the activity, not a command — nothing here names one.
        return "none recorded (no rule check has run)".to_string();
    };
    let when = provenance(check);
    let evaluated = render_evaluated_set(check);
    // The clean licence and the denominator it is clean over arrive as ONE
    // value — see `recorded_clean_over`. Re-deriving the figure here from
    // `check.checked_rules` is what made the licence's own third condition dead
    // in the first draft.
    match (recorded_clean_over(readout, check), check.checked_rules, headline) {
        (Some(rules), _, _) => format!("0 of {rules} rule(s) evaluated — clean, {when}"),
        // Not clean and nothing to count: the state is the evaluated set
        // itself. "no pass is stated" rather than "0 violations" — the
        // difference [FR-GV-03] turns on.
        (None, _, None | Some(0)) => format!("{evaluated} — no pass is stated, {when}"),
        (None, Some(rules), Some(total)) => format!("{total} of {rules} rule(s) evaluated, {when}"),
        (None, None, Some(total)) => format!("{total} recorded, {evaluated}, {when}"),
    }
}

/// The `n/a` clause for an absent signal, naming the cause the readout's own
/// discriminant established and carrying the figure that establishes it
/// ([CR-138], [FR-EH-04]).
///
/// One helper for both channels, and for the same reason `headline_count` is
/// computed once below: a summary and a full readout that derive the same
/// sentence separately are a pair that can drift, and this one is the sentence
/// a user reads to decide whether their project is indexed at all.
///
/// The `None` case names **no** cause. It is reachable only from a readout
/// assembled without the discriminant, and defaulting it to the familiar
/// "empty graph" is precisely what this change removes — an unestablished cause
/// is reported as absent, not as the most likely one ([NFR-CC-04]).
///
/// [FR-EH-04]: ../../../docs/specs/requirements/FR-EH-04.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
/// [CR-138]: ../../../docs/requests/CR-138-a-readout-names-the-cause-its-gating-condition-establishes.md
fn render_signal_absence(absence: Option<&SignalAbsence>) -> String {
    match absence {
        None => "n/a".to_string(),
        Some(SignalAbsence::EmptyGraph) => "n/a (empty graph)".to_string(),
        // The figures ride with the cause on the same line, never a bare
        // "no production code": `indexed_nodes` is what rules out an empty
        // graph, so a reader can check the claim against `logos status`
        // without a second command.
        Some(SignalAbsence::NoProductionScope {
            indexed_nodes,
            test_functions,
        }) => format!(
            "n/a (no production code — {indexed_nodes} node(s) indexed, \
             {test_functions} test function(s) excluded)"
        ),
    }
}

/// The signal cell — the figure, or the clause naming its absence.
///
/// One spelling for both channels, for the same reason
/// [`render_signal_absence`] consolidates the clause and `headline_count`
/// computes its number once: two channels that derive one cell separately are a
/// pair that can drift, and on the violations line that drift shipped.
///
/// The *baseline* fork below is deliberately not consolidated this way — there
/// the two channels render genuinely different text (`no baseline saved` versus
/// a full sentence naming `gate --save`), so a shared helper would have to
/// re-introduce the split it was meant to remove.
fn render_signal(readout: &QualityReadout) -> String {
    match readout.signal {
        Some(signal) => signal.to_string(),
        None => render_signal_absence(readout.signal_absence.as_ref()),
    }
}

/// Render the one-line, user-visible summary of a readout ([CR-095]).
///
/// Every absent value is named rather than defaulted: an absent signal reads
/// `signal n/a` with the cause the readout established ([`render_signal_absence`]),
/// not `signal 0`; a check nobody ran reads `violations none recorded (no rule
/// check has run)`, not `0 violations`; and a run that evaluated nothing reads
/// as *that*, not as clean ([`render_violations`]). A check that demonstrably
/// ran over a non-empty evaluated set and found nothing is stated as clean —
/// with its denominator and its age, and never bare ([CR-096], [CR-140],
/// [BR-41]).
///
/// [CR-140]: ../../../docs/requests/CR-140-the-recorded-check-marker-carries-what-it-evaluated.md
fn render_summary(readout: &QualityReadout) -> String {
    let mut parts = vec![format!("signal {}", render_signal(readout))];
    match (readout.baseline_signal, readout.delta) {
        (Some(baseline), Some(delta)) => {
            parts.push(format!("baseline {baseline}"));
            parts.push(format!("delta {delta}"));
        }
        (Some(baseline), None) => parts.push(format!("baseline {baseline} (not comparable)")),
        (None, _) => parts.push("no baseline saved".to_string()),
    }
    // The one-line channel's provenance: the age, and the moved-tree flag in
    // its compact form — the summary has no room for two shas.
    let summary_provenance = |check: &CheckRun| {
        let mut when = render_age(check.age_seconds);
        if check.tree_moved {
            when.push_str(" (different tree)");
        }
        when
    };
    parts.push(format!(
        "violations {}",
        render_violations(
            readout,
            headline_count(readout, readout.check.as_ref()),
            &summary_provenance,
        )
    ));
    format!("logos quality report: {}", parts.join(" · "))
}

/// Render the full multi-line readout handed to the agent ([CR-095]).
fn render_readout(readout: &QualityReadout) -> String {
    let mut out = String::from("logos quality report (session start)\n");
    out.push_str(&format!("  signal:   {}\n", render_signal(readout)));
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
    // The state, the evaluated set and the verdict come from the SAME helper
    // the summary uses ([CR-140] §3.2) — see `render_violations`. The
    // disjunction [CR-095] had to render here ("a clean check, or none has
    // run") stays resolved by the marker's presence; what the helper adds is
    // that a marker with no evaluated set no longer resolves it *favourably*.
    out.push_str(&format!(
        "  rule violations: {}\n",
        render_violations(readout, headline, &render_provenance)
    ));
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
            // A present signal has no absence to explain; the pair is set
            // together here exactly as `quality_readout` sets it.
            signal_absence: None,
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
    /// `HEAD` that has **not** moved since — over a contract authoring
    /// [`FIXTURE_RULES`] rules.
    fn marker(count: i64, age: i64) -> CheckRun {
        CheckRun {
            ran_at: 1_700_000_000,
            age_seconds: age,
            commit_sha: Some("ff657f5aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string()),
            head_sha: Some("ff657f5aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string()),
            tree_moved: false,
            recorded_count: Some(count),
            checked_rules: Some(FIXTURE_RULES),
            evaluated_absence: None,
        }
    }

    /// The evaluated set the default fixture marker records.
    ///
    /// Deliberately **not** equal to any violation count used below: a
    /// denominator that happened to equal its numerator would be satisfied by a
    /// rendering that printed the count twice, which is the conflation
    /// [S-437]'s T1 review already caught once on the write side.
    ///
    /// [S-437]: ../../../docs/planning/journal.md#s-437-the-recorded-check-marker-carries-what-it-evaluated
    const FIXTURE_RULES: u32 = 7;

    /// A marker recording a run that produced **no denominator** — the three
    /// states [CR-140] requires be told apart from a clean one and from each
    /// other. `count` is the violation total it recorded.
    fn vacuous_marker(absence: EvaluatedSetAbsence, count: i64, age: i64) -> CheckRun {
        CheckRun {
            checked_rules: None,
            evaluated_absence: Some(absence),
            ..marker(count, age)
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
        assert!(
            summary.contains("1 of 7 rule(s) evaluated"),
            "summary names the count AND its denominator (CR-140 §3.2): {summary}"
        );
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

    /// Nothing absent is ever defaulted: an absent signal reads `n/a`, not `0`;
    /// no baseline reads "none saved", not a delta against zero; and an
    /// unrecorded check reads "none recorded", never a truthful-looking "0
    /// violations" — which would assert a passing check that may never have run.
    ///
    /// The readout here is a bare `Default`, so it carries no absence
    /// discriminant either; that the signal line then names no *cause* is
    /// `an_unclassified_absence_names_no_cause`'s assertion, not this one's.
    #[test]
    fn payload_never_fabricates_an_absent_value() {
        let empty = QualityReadout::default();
        let payload = round_trip(&empty);
        let summary = payload["systemMessage"].as_str().unwrap();
        let context = payload["hookSpecificOutput"]["additionalContext"].as_str().unwrap();

        assert!(
            summary.contains("signal n/a"),
            "an unclassified absence is n/a: {summary}"
        );
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
            context.contains(
                "rule violations: 0 of 7 rule(s) evaluated — clean, at HEAD ff657f5, \
                 4 minutes ago"
            ),
            "the clean check is stated, dated and carries its denominator: {context}"
        );
        assert!(
            summary.contains("violations 0 of 7 rule(s) evaluated — clean, 4 minutes ago"),
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
            context.contains("rule violations: 1 of 7 rule(s) evaluated, at HEAD ff657f5, 6 days ago"),
            "the count carries its denominator, its age and its HEAD: {context}"
        );
        assert!(context.contains("- max_cc: foo is 31"), "the message is still listed: {context}");
        assert!(
            summary.contains("violations 1 of 7 rule(s) evaluated, 6 days ago"),
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
            context.contains("clean, at HEAD ff657f5, 4 minutes ago; \
                              measured against a different tree — HEAD is now a1b2c3d"),
            "a clean check is never left looking current when the tree has moved: {context}"
        );
        assert!(
            summary.contains("0 of 7 rule(s) evaluated — clean, 4 minutes ago (different tree)"),
            "{summary}"
        );
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
            context.contains("rule violations: none recorded (no rule check has run)"),
            "absence of the marker is absence of knowledge: {context}"
        );
        assert!(summary.contains("violations none recorded (no rule check has run)"), "{summary}");
        for fabricated in ["clean", "0 violation", "different tree"] {
            assert!(
                !context.contains(fabricated) && !summary.contains(fabricated),
                "nothing is asserted about a run that never happened ({fabricated}): {context}"
            );
        }
    }

    /// A store written before the marker migration has violation rows but no
    /// marker. The rows carry their own run time, so the findings are dated
    /// immediately — but with no marker there is no recorded `HEAD` and no
    /// recorded evaluated set, so the readout attributes them to no tree, names
    /// the denominator unknown, and asserts no clean check.
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
                checked_rules: None,
                evaluated_absence: Some(EvaluatedSetAbsence::Unrecorded),
            }),
            ..full_readout()
        };
        let (summary, context) = channels(&readout);
        assert!(
            context.contains(
                "rule violations: 1 recorded, evaluated set unknown \
                 (this marker predates its recording), 6 days ago"
            ),
            "the rows date themselves with no marker present, and the denominator \
             they were measured against is named unknown: {context}"
        );
        // The summary too, not only the agent-facing channel: this arm —
        // findings recorded over NO denominator — is the one state whose
        // wording was pinned on `additionalContext` alone, so a regression in
        // the line a human reads would have shipped unseen.
        assert!(
            summary.contains(
                "violations 1 recorded, evaluated set unknown \
                 (this marker predates its recording), 6 days ago"
            ),
            "and the user-visible line says the same thing: {summary}"
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
            context.contains("rule violations: 0 of 7 rule(s) evaluated — clean, just now"),
            "a clean run with no recorded HEAD is still stated, dated and denominated: {context}"
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

    // ── The [CR-138] two-arm absence ([FR-EH-04], S-432) ────────────────────

    /// An absent signal with no cause established names **no** cause.
    ///
    /// Reachable only from a readout assembled without the discriminant — a
    /// bare `Default`, which is what the fabrication test above builds. It must
    /// not fall back to "empty graph": that is the defect this change removes,
    /// in its purest form, since the fallback would be asserting a store fact
    /// nothing here ever read.
    #[test]
    fn an_unclassified_absence_names_no_cause() {
        let (summary, context) = channels(&QualityReadout::default());
        assert!(summary.contains("signal n/a ·"), "unqualified, not guessed: {summary}");
        assert!(context.contains("signal:   n/a\n"), "and the same in full: {context}");
        for channel in [&summary, &context] {
            assert!(
                !channel.contains("empty graph") && !channel.contains("production"),
                "a cause nothing established is not invented: {channel}"
            );
        }
    }

    /// The empty-graph arm renders the phrase the readout has always used —
    /// the one case in which it was always true.
    #[test]
    fn the_empty_graph_arm_is_unchanged() {
        let readout = QualityReadout {
            signal: None,
            signal_absence: Some(SignalAbsence::EmptyGraph),
            ..full_readout()
        };
        let (summary, context) = channels(&readout);
        assert!(summary.contains("signal n/a (empty graph)"), "{summary}");
        assert!(context.contains("signal:   n/a (empty graph)"), "{context}");
    }

    /// The production-scope arm names that scope and carries **both** figures
    /// into **both** channels. The [CR-138] reproduction is the fixture shape:
    /// nine nodes indexed, three of them test functions, no production code.
    #[test]
    fn the_production_scope_arm_names_the_scope_and_carries_its_figures() {
        let readout = QualityReadout {
            signal: None,
            signal_absence: Some(SignalAbsence::NoProductionScope {
                indexed_nodes: 9,
                test_functions: 3,
            }),
            ..full_readout()
        };
        let (summary, context) = channels(&readout);
        for channel in [&summary, &context] {
            assert!(
                !channel.contains("empty graph"),
                "a populated store is never an empty graph (FR-EH-04 AC2): {channel}"
            );
            assert!(
                channel.contains("no production code — 9 node(s) indexed, 3 test function(s) \
                                  excluded"),
                "the cause and both establishing figures, on one line: {channel}"
            );
        }
        // The summary stays one line: the figures ride with the cause, they do
        // not wrap it onto a second.
        assert!(!summary.contains('\n'), "still one line: {summary:?}");
    }

    /// Zero excluded test functions is reported as the count it is. An empty
    /// production scope over a graph of nothing but derived vertices excludes no
    /// test, and rendering "every symbol is a test" there would be a fabricated
    /// explanation of a real absence ([NFR-CC-04]).
    #[test]
    fn a_production_scope_emptied_without_tests_still_reports_its_counts() {
        let readout = QualityReadout {
            signal: None,
            signal_absence: Some(SignalAbsence::NoProductionScope {
                indexed_nodes: 4,
                test_functions: 0,
            }),
            ..full_readout()
        };
        let (summary, _) = channels(&readout);
        assert!(
            summary.contains("4 node(s) indexed, 0 test function(s) excluded"),
            "the counts are the counts: {summary}"
        );
    }

    /// Both channels render the absence through one helper, so they cannot
    /// drift into describing one absence two ways — the failure `headline_count`
    /// already exists to prevent for the violations line.
    #[test]
    fn both_channels_render_one_absence_identically() {
        for absence in [
            None,
            Some(SignalAbsence::EmptyGraph),
            Some(SignalAbsence::NoProductionScope {
                indexed_nodes: 9,
                test_functions: 3,
            }),
        ] {
            let readout = QualityReadout {
                signal: None,
                signal_absence: absence.clone(),
                ..full_readout()
            };
            let (summary, context) = channels(&readout);
            let clause = render_signal_absence(absence.as_ref());
            assert!(
                summary.contains(&format!("signal {clause}")),
                "summary carries the one clause: {summary}"
            );
            assert!(
                context.contains(&format!("signal:   {clause}")),
                "and so does the full readout: {context}"
            );
        }
    }

    // ── The [CR-140] evaluated set ([FR-IN-07], [FR-GV-03], S-437 T2) ───────
    //
    // Four states, each its own. The fixtures are the `notes/sprint-test-72.md`
    // Finding 1 transcripts, not invented shapes: the exit-`4` run with no
    // contract, the `logos init` default contract authoring none, a real
    // contract that holds, and an existing install whose marker predates
    // migration 21.

    /// The `rule violations:` line of the full readout, and the violations
    /// segment of the one-line summary — the two renderings of one state.
    ///
    /// Scoped to that line deliberately: the assertions below include "this
    /// line names no command", and the baseline and freshness lines legitimately
    /// name `logos gate --save`. Asserting over the whole readout would make
    /// that test pass or fail for reasons that have nothing to do with it.
    fn violation_lines(readout: &QualityReadout) -> (String, String) {
        let (summary, context) = channels(readout);
        let summary_part = summary
            .split(" · ")
            .find(|part| part.starts_with("violations "))
            .unwrap_or_else(|| panic!("the summary carries a violations segment: {summary}"))
            .to_string();
        let context_line = context
            .lines()
            .find(|line| line.trim_start().starts_with("rule violations:"))
            .unwrap_or_else(|| panic!("the readout carries a violations line: {context}"))
            .trim()
            .to_string();
        (summary_part, context_line)
    }

    /// A readout whose last run found nothing and recorded `absence` as its
    /// evaluated set — the shape every vacuous arm below is built from.
    fn vacuous_readout(absence: EvaluatedSetAbsence) -> QualityReadout {
        QualityReadout {
            violations: None,
            violation_count: None,
            check: Some(vacuous_marker(absence, 0, 4 * 60)),
            ..full_readout()
        }
    }

    /// **Arm (a)** — a run with no rules contract at all: the exit-`4` state
    /// whose own stdout says *"nothing was evaluated"*.
    ///
    /// `notes/sprint-test-72.md` Finding 1a is the whole fixture: the same
    /// binary printed *"no rules contract found — nothing was evaluated"* and
    /// then rendered ``rule violations: 0 — clean `logos check` at HEAD
    /// 7e4535d, just now``. [FR-GV-03] says clean *"never means nothing was
    /// evaluated"*, so the readout names the state instead.
    #[test]
    fn a_run_with_no_contract_is_named_never_rendered_as_a_clean_check() {
        let (summary, context) = violation_lines(&vacuous_readout(EvaluatedSetAbsence::NoContract));
        for line in [&summary, &context] {
            assert!(
                line.contains("no rules contract authored"),
                "the state is named: {line}"
            );
            assert!(
                line.contains("no pass is stated"),
                "and no pass is claimed over it: {line}"
            );
            assert!(
                !line.contains("clean"),
                "a run that evaluated nothing is never clean (FR-GV-03): {line}"
            );
            assert!(
                !line.contains("violations: 0") && !line.contains("violations 0"),
                "and never a bare zero (FR-GV-22, CR-140 CRA-01): {line}"
            );
        }
    }

    /// **Arm (b)** — a present contract authoring **zero** rules: exit `0`, and
    /// what `logos init` writes by default, so the ordinary state of a fresh
    /// project rather than an edge case.
    ///
    /// Distinguishable from arm (a) is the assertion that matters: both record
    /// `checked_rules = 0`, and only `rules_present` separates them, which is
    /// exactly why T1 stored two fields rather than deriving one.
    #[test]
    fn a_contract_authoring_zero_rules_is_its_own_state_not_a_clean_check() {
        let (summary, context) =
            violation_lines(&vacuous_readout(EvaluatedSetAbsence::NoRulesAuthored));
        for line in [&summary, &context] {
            assert!(
                line.contains("a rules contract present, authoring no rules"),
                "a configured project that enforces nothing says so: {line}"
            );
            assert!(
                !line.contains("no rules contract authored"),
                "and is never collapsed into the unconfigured state (CRA-02): {line}"
            );
            assert!(!line.contains("clean"), "still not a pass: {line}");
        }
    }

    /// **Arm (c)** — the one genuinely clean state: a run over N > 0 rules that
    /// found nothing, stated **with N** so the figure carries its denominator
    /// ([NFR-CC-04]).
    ///
    /// The denominator is read from the record rather than hardcoded twice, and
    /// `FIXTURE_RULES` is deliberately not equal to any violation count in this
    /// module — a rendering that printed the numerator twice would satisfy an
    /// assertion built on a fixture where the two coincide.
    #[test]
    fn a_clean_run_states_the_clean_result_with_its_denominator() {
        let readout = QualityReadout {
            violations: None,
            violation_count: None,
            check: Some(marker(0, 4 * 60)),
            ..full_readout()
        };
        let (summary, context) = violation_lines(&readout);
        for line in [&summary, &context] {
            assert!(
                line.contains(&format!("0 of {FIXTURE_RULES} rule(s) evaluated")),
                "the clean figure carries its denominator (NFR-CC-04, CRA-03): {line}"
            );
            assert!(line.contains("clean"), "and it is stated as clean: {line}");
        }
        assert_ne!(
            i64::from(FIXTURE_RULES),
            0,
            "a zero denominator would make this test agree with the vacuous arms"
        );
    }

    /// **Arm (d)** — a marker written **before** migration 21 records no
    /// evaluated set, and renders as *evaluated set unknown*: never clean, and
    /// never zero.
    ///
    /// An existing install is the common case, so this arm is the one the whole
    /// story turns on — rendering an absent fact favourably is precisely the
    /// defect being removed, and reintroducing it here would be the most
    /// natural way to fail ([CR-140] CRA-05).
    #[test]
    fn a_marker_predating_the_evaluated_set_renders_as_unknown_never_clean_never_zero() {
        let (summary, context) = violation_lines(&vacuous_readout(EvaluatedSetAbsence::Unrecorded));
        for line in [&summary, &context] {
            assert!(
                line.contains("evaluated set unknown"),
                "an unrecorded evaluated set is named unknown (CRA-05): {line}"
            );
            assert!(
                !line.contains("clean"),
                "an unknown evaluated set licenses no pass: {line}"
            );
            assert!(
                !line.contains("violations: 0") && !line.contains("violations 0"),
                "and is never rendered as a zero: {line}"
            );
        }
    }

    /// The four states, plus *never checked*, are **pairwise distinguishable**
    /// on both channels ([CR-140] CRA-01/02/03/05).
    ///
    /// Each test above pins one arm's wording; this pins that no two of them
    /// render the same line. A rendering that collapsed two arms into one
    /// sentence would pass every individual test that only asserts a substring
    /// is present, and fail here.
    #[test]
    fn every_evaluated_set_state_renders_differently_from_every_other() {
        let states: Vec<(&str, QualityReadout)> = vec![
            ("never checked", QualityReadout::default()),
            (
                "clean over N",
                QualityReadout {
                    violations: None,
                    violation_count: None,
                    check: Some(marker(0, 4 * 60)),
                    ..full_readout()
                },
            ),
            ("no contract", vacuous_readout(EvaluatedSetAbsence::NoContract)),
            ("zero rules", vacuous_readout(EvaluatedSetAbsence::NoRulesAuthored)),
            ("unrecorded", vacuous_readout(EvaluatedSetAbsence::Unrecorded)),
        ];
        let rendered: Vec<(&str, (String, String))> = states
            .iter()
            .map(|(name, readout)| (*name, violation_lines(readout)))
            .collect();
        for (i, (left_name, left)) in rendered.iter().enumerate() {
            for (right_name, right) in rendered.iter().skip(i + 1) {
                assert_ne!(
                    left.0, right.0,
                    "`{left_name}` and `{right_name}` render the same summary segment"
                );
                assert_ne!(
                    left.1, right.1,
                    "`{left_name}` and `{right_name}` render the same readout line"
                );
            }
        }
    }

    /// The violations line **names no command**, in every state ([FR-IN-07],
    /// [CR-140] CRA-04).
    ///
    /// Pinned by name so the attribution cannot silently return. It is not a
    /// wording preference: the marker is written by `replace_violations`, which
    /// `scan` calls as well as `check`, and `notes/sprint-test-72.md` Finding 1b
    /// reproduced a readout naming ``logos check`` after an ``init`` / ``index``
    /// / ``scan`` sequence in which ``logos check`` was never invoked. T1 added
    /// an `operation` column that *could* name it; [FR-IN-07]'s appended
    /// criterion says not to.
    #[test]
    fn the_violations_line_names_no_command() {
        let states = [
            QualityReadout::default(),
            full_readout(),
            QualityReadout {
                violations: None,
                violation_count: None,
                check: Some(marker(0, 4 * 60)),
                ..full_readout()
            },
            vacuous_readout(EvaluatedSetAbsence::NoContract),
            vacuous_readout(EvaluatedSetAbsence::NoRulesAuthored),
            vacuous_readout(EvaluatedSetAbsence::Unrecorded),
            QualityReadout {
                check: Some(vacuous_marker(EvaluatedSetAbsence::Unrecorded, 1, 60)),
                ..full_readout()
            },
        ];
        for readout in &states {
            let (summary, context) = violation_lines(readout);
            for line in [&summary, &context] {
                for command in ["logos check", "logos scan", "logos gate", "`logos", "check_rules"]
                {
                    assert!(
                        !line.contains(command),
                        "the violations line names no command, and named `{command}`: {line}"
                    );
                }
            }
        }
        // The rest of the readout is untouched by that rule — the absent
        // baseline still says how to create one. Without this, deleting every
        // backtick from the whole rendering would pass the loop above.
        let (_, whole) = channels(&QualityReadout::default());
        assert!(
            whole.contains("bless one with `logos gate --save`"),
            "only the violations line drops its command name: {whole}"
        );
    }

    /// Both channels render one evaluated set through one helper, so they
    /// cannot drift into describing one state two ways — the sibling of
    /// `both_channels_render_one_absence_identically`, and of the drift that
    /// actually shipped on this line before `headline_count` consolidated its
    /// number.
    #[test]
    fn both_channels_render_one_evaluated_set_identically() {
        for absence in [
            EvaluatedSetAbsence::NoContract,
            EvaluatedSetAbsence::NoRulesAuthored,
            EvaluatedSetAbsence::Unrecorded,
        ] {
            let readout = vacuous_readout(absence);
            let clause = render_evaluated_set(readout.check.as_ref().expect("a marker"));
            let (summary, context) = violation_lines(&readout);
            assert!(summary.contains(&clause), "summary carries the one clause: {summary}");
            assert!(context.contains(&clause), "and so does the full readout: {context}");
        }
    }

    /// A recorded run's denominator and the reason it is missing are produced
    /// **together**, so a figure and a cause for its absence can never both be
    /// present — the invariant [`QualityReadout::signal`] /
    /// [`QualityReadout::signal_absence`] already carries, applied to the
    /// second figure on the same readout.
    #[test]
    fn a_denominator_and_a_reason_for_its_absence_are_never_both_present() {
        for (checked_rules, rules_present) in [
            (Some(3), Some(true)),
            (Some(0), Some(true)),
            (Some(0), Some(false)),
            (None, None),
            (None, Some(true)),
            (Some(0), None),
        ] {
            let (figure, absence) = EvaluatedSetAbsence::classify(checked_rules, rules_present);
            assert_ne!(
                figure.is_some(),
                absence.is_some(),
                "exactly one of the two is present for ({checked_rules:?}, {rules_present:?}): \
                 {figure:?} / {absence:?}"
            );
        }
    }

    /// The two facts a marker records are **not** one fact: `checked_rules`
    /// alone cannot separate an unconfigured project from a configured one that
    /// enforces nothing, and `rules_present` is what does ([CR-140] §3.1).
    ///
    /// Asserted on the classifier rather than only through the rendering, so a
    /// rendering that happened to differ for another reason could not stand in
    /// for the discriminant actually working.
    #[test]
    fn rules_present_is_what_separates_the_two_vacuous_states() {
        let (no_figure, absent) = EvaluatedSetAbsence::classify(Some(0), Some(false));
        let (also_none, present) = EvaluatedSetAbsence::classify(Some(0), Some(true));
        assert_eq!(
            (no_figure, also_none),
            (None, None),
            "both evaluated nothing, so neither yields a denominator"
        );
        assert_eq!(absent, Some(EvaluatedSetAbsence::NoContract));
        assert_eq!(present, Some(EvaluatedSetAbsence::NoRulesAuthored));
        assert_ne!(absent, present, "the count cannot separate them; rules_present does");
    }

    /// A marker whose evaluated-set columns are `NULL` classifies as
    /// **unrecorded** — not as either vacuous state, and never as a zero
    /// ([CR-140] CRA-05).
    ///
    /// Pinned on the classifier directly rather than only through a store,
    /// because the rendering tests above construct their records by hand and so
    /// never exercise this mapping: a mutation collapsing `Unrecorded` into
    /// `NoRulesAuthored` passed every one of them, and only the end-to-end
    /// pre-migration fixture caught it. A `NULL` is a fact nobody recorded, and
    /// an existing install is the common case — the two reasons this arm may
    /// never be resolved into a state somebody did record.
    #[test]
    fn a_null_evaluated_set_classifies_as_unrecorded_not_as_a_state_someone_recorded() {
        // Both columns NULL, which is what migration 21 leaves on every marker
        // written before it — the three columns are added by one statement, so
        // in practice they are NULL together.
        for columns in [(None, None), (None, Some(true)), (None, Some(false)), (Some(0), None)] {
            let (figure, absence) = EvaluatedSetAbsence::classify(columns.0, columns.1);
            assert_eq!(
                (figure, absence),
                (None, Some(EvaluatedSetAbsence::Unrecorded)),
                "a NULL column is unknown, never a recorded state, for {columns:?}"
            );
        }
        // And a recorded zero is NOT unrecorded: the distinction only means
        // something if the two really are different inputs.
        assert_eq!(
            EvaluatedSetAbsence::classify(Some(0), Some(true)).1,
            Some(EvaluatedSetAbsence::NoRulesAuthored),
            "a recorded zero over a present contract is a state somebody did record"
        );
    }

    /// A `checked_rules` no run could have produced is named unusable, never
    /// rendered as a state somebody recorded.
    ///
    /// Migration 21 put a `CHECK` on `rules_present` and **none** on
    /// `checked_rules`, so a corrupted or hand-edited row can carry a negative
    /// count. Falling through to the zero-rules arms would state *"a rules
    /// contract present, authoring no rules"* — a specific, plausible sentence
    /// about a row that records nothing usable, which is the same class of
    /// fabrication this whole line exists to remove. The over-large end is
    /// clamped for the same reason, and both are asserted here so the two ends
    /// of one posture cannot drift apart.
    #[test]
    fn an_unusable_rule_count_is_named_unknown_not_rendered_as_a_recorded_state() {
        for corrupt in [-1_i64, -5, i64::MIN] {
            for rules_present in [Some(true), Some(false), None] {
                let (figure, absence) = EvaluatedSetAbsence::classify(Some(corrupt), rules_present);
                assert_eq!(
                    (figure, absence),
                    (None, Some(EvaluatedSetAbsence::Unrecorded)),
                    "a negative count records no evaluated set \
                     ({corrupt}, {rules_present:?})"
                );
            }
        }
        // And the rendering says so, rather than naming a contract state.
        let readout = QualityReadout {
            violations: None,
            violation_count: None,
            check: Some(CheckRun {
                checked_rules: EvaluatedSetAbsence::classify(Some(-5), Some(true)).0,
                evaluated_absence: EvaluatedSetAbsence::classify(Some(-5), Some(true)).1,
                ..marker(0, 60)
            }),
            ..full_readout()
        };
        let (summary, context) = violation_lines(&readout);
        for channel in [&summary, &context] {
            assert!(
                channel.contains("evaluated set unknown"),
                "an unusable count renders as unknown: {channel}"
            );
            assert!(
                !channel.contains("authoring no rules") && !channel.contains("clean"),
                "and never as a contract state or a pass: {channel}"
            );
        }

        // The other end of the same posture: a count wider than the producer's
        // own `u32` saturates rather than wrapping into a small, plausible one.
        assert_eq!(
            EvaluatedSetAbsence::classify(Some(i64::from(u32::MAX) + 1), Some(true)),
            (Some(u32::MAX), None),
            "an over-large count saturates; it never wraps"
        );
    }
}
