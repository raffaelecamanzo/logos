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
//! [FR-IN-07]: ../../../docs/specs/requirements/FR-IN-07.md
//! [CR-095]: ../../../docs/requests/CR-095-session-start-quality-readout.md

use crate::models::quality::QualityReadout;

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

/// Render the one-line, user-visible summary of a readout ([CR-095]).
///
/// Every absent value is named rather than defaulted: an empty graph reads
/// `signal n/a`, not `signal 0`; an unrecorded check reads `violations none
/// recorded`, not `0 violations`.
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
    parts.push(match readout.violation_count {
        Some(count) => format!("{count} violation(s)"),
        None => "violations none recorded".to_string(),
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
    match (&readout.violations, readout.violation_count) {
        (Some(messages), Some(total)) => {
            out.push_str(&format!("  rule violations: {total} (as of the last `logos check`)\n"));
            for message in messages {
                out.push_str(&format!("    - {message}\n"));
            }
            // Never a silent cap: a reader must not mistake the listed subset
            // for the whole set.
            let dropped = total.saturating_sub(messages.len());
            if dropped > 0 {
                out.push_str(&format!("    … {dropped} more not shown\n"));
            }
        }
        _ => out.push_str(
            "  rule violations: none recorded (a clean `logos check`, or none has run)\n",
        ),
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
            warnings: Vec::new(),
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
}
