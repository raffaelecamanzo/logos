//! The four built-in, prompt-only chat presets ([S-625], [FR-UI-49], [ADR-41]).
//!
//! A preset steers a turn by **text alone**: it appends one focus clause to the
//! planner preamble and one to the Synthesizer preamble, on whichever roster runs
//! the turn — the member roster's or the workspace roster's own. Nothing else
//! varies with it. The tool set, the subagent roster, the budget tree and the
//! read-only sandbox are the same under every preset ([BR-36]), so a preset never
//! widens what a turn may do or what reaches the provider beyond the preamble
//! ([NFR-SE-07]).
//!
//! [`Preset::Default`] appends nothing, so a Default turn's preambles are
//! byte-identical to the preambles before presets existed.
//!
//! The clauses compose with the first turn's title trailer ([S-623]): the trailer
//! rides the Synthesizer's step context and follows whatever preamble the roster
//! hands it, so a preset preamble is followed by the trailer clause, never
//! replaced by it.
//!
//! [S-625]: ../../../docs/planning/journal.md#s-625-a-chat-turn-runs-under-a-prompt-only-preset-chosen-in-the-composer
//! [S-623]: ../../../docs/planning/journal.md#s-623-the-synthesizer-titles-a-new-conversation-and-a-conversation-can-be-renamed
//! [FR-UI-49]: ../../../docs/specs/requirements/FR-UI-49.md
//! [ADR-41]: ../../../docs/specs/architecture/decisions/ADR-41.md
//! [BR-36]: ../../../docs/specs/software-spec.md#326-web-ui
//! [NFR-SE-07]: ../../../docs/specs/requirements/NFR-SE-07.md

/// A built-in chat preset — a fixed enum, so an unknown value is refused at the
/// turn boundary rather than mapped to something ([FR-UI-49]).
///
/// [FR-UI-49]: ../../../docs/specs/requirements/FR-UI-49.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Preset {
    /// The unsteered turn: no clause is appended.
    #[default]
    Default,
    /// Lead with what a change to the code in question would reach.
    ImpactAnalysis,
    /// Orient a reader who is new to the code.
    Onboarding,
    /// Diagnose a fault from the symptom the reader names.
    Troubleshooting,
}

impl Preset {
    /// Every preset, Default first — the order the composer menu lists them in.
    pub const ALL: [Preset; 4] = [
        Preset::Default,
        Preset::ImpactAnalysis,
        Preset::Onboarding,
        Preset::Troubleshooting,
    ];

    /// The value the turn POST's `preset` field carries.
    pub fn wire_name(self) -> &'static str {
        match self {
            Preset::Default => "default",
            Preset::ImpactAnalysis => "impact_analysis",
            Preset::Onboarding => "onboarding",
            Preset::Troubleshooting => "troubleshooting",
        }
    }

    /// The preset a `preset` field names — an exact match on [`Self::wire_name`],
    /// so a near miss (`Default`, `impact-analysis`, ` onboarding`) is `None`.
    pub fn from_wire(value: &str) -> Option<Preset> {
        Preset::ALL.into_iter().find(|preset| preset.wire_name() == value)
    }

    /// The clause appended to the planner preamble; `None` for Default.
    pub fn planner_clause(self) -> Option<&'static str> {
        match self {
            Preset::Default => None,
            Preset::ImpactAnalysis => Some(IMPACT_ANALYSIS_PLANNER_CLAUSE),
            Preset::Onboarding => Some(ONBOARDING_PLANNER_CLAUSE),
            Preset::Troubleshooting => Some(TROUBLESHOOTING_PLANNER_CLAUSE),
        }
    }

    /// The clause appended to the Synthesizer preamble; `None` for Default.
    pub fn synthesizer_clause(self) -> Option<&'static str> {
        match self {
            Preset::Default => None,
            Preset::ImpactAnalysis => Some(IMPACT_ANALYSIS_SYNTHESIZER_CLAUSE),
            Preset::Onboarding => Some(ONBOARDING_SYNTHESIZER_CLAUSE),
            Preset::Troubleshooting => Some(TROUBLESHOOTING_SYNTHESIZER_CLAUSE),
        }
    }
}

/// `base` followed by `clause` as its own paragraph — the one way a preset clause
/// joins a preamble, shared by the planner and the Synthesizer of both rosters.
pub(crate) fn with_clause(base: String, clause: Option<&str>) -> String {
    match clause {
        Some(clause) => format!("{base}\n\n{clause}"),
        None => base,
    }
}

const IMPACT_ANALYSIS_PLANNER_CLAUSE: &str = "\
Preset — Impact analysis: the user is weighing a change. Plan to establish what the \
code in question reaches: its callers and dependents, the modules and tests that \
depend on it, and the quality signal around it. Gather that reach before you \
finalize.";

const IMPACT_ANALYSIS_SYNTHESIZER_CLAUSE: &str = "\
Preset — Impact analysis: lead with the reach of a change to the code in question. \
List what depends on it, nearest first, with the location of each; then what would \
need re-testing; then the riskiest dependents and why. Where the observations do not \
establish part of the reach, say which part is unknown.";

const ONBOARDING_PLANNER_CLAUSE: &str = "\
Preset — Onboarding: the user is new to this code. Plan for orientation: the main \
modules and entry points, how they connect, and the conventions the code follows. \
Prefer broad overview steps to deep detail on a single symbol.";

const ONBOARDING_SYNTHESIZER_CLAUSE: &str = "\
Preset — Onboarding: explain for a newcomer. Start from the big picture, define each \
project term the first time it appears, and end with where to start reading, naming \
the files. Prefer a short guided tour to an exhaustive list.";

const TROUBLESHOOTING_PLANNER_CLAUSE: &str = "\
Preset — Troubleshooting: the user is chasing a fault. Plan to locate the code path \
the symptom points to, read the source involved — the error sites and the conditions \
that lead to them — and trace the callers that reach it.";

const TROUBLESHOOTING_SYNTHESIZER_CLAUSE: &str = "\
Preset — Troubleshooting: answer as a diagnosis. Rank the likely causes by the \
evidence in the observations, name where each would occur (file and symbol), and \
give the next check that would confirm or rule it out. Never state a root cause the \
observations do not support.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_wire_name_round_trips_and_is_distinct() {
        for preset in Preset::ALL {
            assert_eq!(Preset::from_wire(preset.wire_name()), Some(preset));
        }
        let mut names: Vec<_> = Preset::ALL.iter().map(|p| p.wire_name()).collect();
        names.dedup();
        assert_eq!(names.len(), Preset::ALL.len());
    }

    #[test]
    fn a_near_miss_names_no_preset() {
        for near in [
            "", "Default", "DEFAULT", " default", "default ", "impact-analysis", "impactanalysis",
            "impact_analysi", "onboarding2", "troubleshoot", "custom",
        ] {
            assert_eq!(Preset::from_wire(near), None, "{near:?}");
        }
    }

    #[test]
    fn default_appends_nothing_and_every_other_preset_appends_a_distinct_clause_to_both() {
        assert_eq!(Preset::Default.planner_clause(), None);
        assert_eq!(Preset::Default.synthesizer_clause(), None);
        let mut seen = std::collections::HashSet::new();
        for preset in &Preset::ALL[1..] {
            let planner = preset.planner_clause().expect("a planner clause");
            let synthesizer = preset.synthesizer_clause().expect("a Synthesizer clause");
            assert!(seen.insert(planner), "{preset:?}'s planner clause is its own");
            assert!(seen.insert(synthesizer), "{preset:?}'s Synthesizer clause is its own");
        }
    }

    /// Each preset steers toward ITS OWN focus: the clause pair is pinned to the
    /// preset by its name and a word of its intent, so swapping two presets' clauses
    /// fails here (every other test reads the expected text back from the accessors).
    #[test]
    fn each_preset_carries_its_own_clauses() {
        for (preset, name, planner_intent, synthesizer_intent) in [
            (Preset::ImpactAnalysis, "Impact analysis", "what the code in question reaches", "reach of a change"),
            (Preset::Onboarding, "Onboarding", "orientation", "for a newcomer"),
            (Preset::Troubleshooting, "Troubleshooting", "chasing a fault", "as a diagnosis"),
        ] {
            let head = format!("Preset — {name}:");
            let planner = preset.planner_clause().expect("a planner clause");
            let synthesizer = preset.synthesizer_clause().expect("a Synthesizer clause");
            assert!(planner.starts_with(&head), "{preset:?}: {planner}");
            assert!(planner.contains(planner_intent), "{preset:?}: {planner}");
            assert!(synthesizer.starts_with(&head), "{preset:?}: {synthesizer}");
            assert!(synthesizer.contains(synthesizer_intent), "{preset:?}: {synthesizer}");
        }
    }

    #[test]
    fn with_clause_appends_a_paragraph_or_nothing() {
        assert_eq!(with_clause("base".into(), None), "base");
        assert_eq!(with_clause("base".into(), Some("clause")), "base\n\nclause");
    }
}
