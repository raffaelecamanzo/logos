//! The bounded **prior-turn window** the planner and the Synthesizer are shown on a
//! follow-up turn ([S-483], [FR-UI-20] AC-2).
//!
//! Without it a follow-up such as "and for mailbox-manager?" is answered as if
//! nothing had been said: the planner's prompt is the current question plus this
//! turn's scratchpad, and the Synthesizer grounds on this turn only. A
//! [`ConversationWindow`] carries the thread's earlier user/assistant messages —
//! read from the thread store, never from the working-memory summary — rendered
//! oldest first and bounded by a turn count and a character ceiling.
//!
//! Truncation is **whole turns, oldest first**, and the rendered block states how
//! many earlier turns were omitted — a window never silently pretends to be the
//! whole conversation ([NFR-CC-04]). An empty window renders to nothing, so the
//! first turn of a thread is byte-identical to a turn that never had a window.
//!
//! [S-483]: ../../../docs/planning/journal.md#s-483-follow-up-turns-see-prior-turns
//! [FR-UI-20]: ../../../docs/specs/requirements/FR-UI-20.md
//! [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md

use std::fmt::Write as _;

/// One earlier turn of the thread: the user's message and the assistant's answer,
/// if the turn produced one (a halted or failed turn persists no answer).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PriorTurn {
    /// The user's message.
    pub user: String,
    /// The assistant's answer; `None` for a turn that produced none.
    pub assistant: Option<String>,
}

impl PriorTurn {
    /// A prior turn from the user's message and the assistant's answer, if any.
    pub fn new(user: impl Into<String>, assistant: Option<String>) -> Self {
        Self {
            user: user.into(),
            assistant,
        }
    }

    /// The characters this turn contributes to the window's ceiling.
    fn chars(&self) -> usize {
        self.user.chars().count()
            + self
                .assistant
                .as_deref()
                .map_or(0, |answer| answer.chars().count())
    }
}

/// The bounded window of a thread's earlier turns, oldest first.
///
/// Build one with [`ConversationWindow::bounded`]; [`Default`] is the empty window
/// (a thread's first turn), which renders to the empty string.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConversationWindow {
    turns: Vec<PriorTurn>,
    omitted: usize,
}

impl ConversationWindow {
    /// Bound `turns` (oldest first) to at most `max_turns` turns and `max_chars`
    /// characters of message text.
    ///
    /// The **newest** turns are kept: the window grows backwards from the most
    /// recent turn and stops at the first one that would breach either bound, so
    /// the oldest turns go first and what is kept is always contiguous. Turns are
    /// kept whole — a turn is never cut mid-message — so a turn larger than
    /// `max_chars` on its own is omitted and counted.
    pub fn bounded(turns: Vec<PriorTurn>, max_turns: usize, max_chars: usize) -> Self {
        let total = turns.len();
        let mut kept = 0;
        let mut chars = 0usize;
        for turn in turns.iter().rev().take(max_turns) {
            chars = chars.saturating_add(turn.chars());
            if chars > max_chars {
                break;
            }
            kept += 1;
        }
        let omitted = total - kept;
        let turns = turns.into_iter().skip(omitted).collect();
        Self { turns, omitted }
    }

    /// Whether the window has nothing to show — no turns kept **and** none omitted.
    /// An empty window leaves a prompt exactly as it was before windows existed.
    pub fn is_empty(&self) -> bool {
        self.turns.is_empty() && self.omitted == 0
    }

    /// The kept turns, oldest first.
    pub fn turns(&self) -> &[PriorTurn] {
        &self.turns
    }

    /// How many earlier turns the bounds dropped.
    pub fn omitted(&self) -> usize {
        self.omitted
    }

    /// The window as a prompt block — empty for an empty window.
    ///
    /// The header says these are context, not grounding: a codebase claim must
    /// still rest on this turn's observations, never on an earlier answer
    /// ([NFR-CC-04]).
    pub fn render(&self) -> String {
        if self.is_empty() {
            return String::new();
        }
        let mut out = String::from(
            "Earlier in this conversation (oldest first). This is context for the current \
             question only — ground every claim about the codebase in observations gathered \
             this turn, not in these earlier answers:\n",
        );
        if self.omitted > 0 {
            let _ = writeln!(
                out,
                "[{} earlier turn(s) omitted from this window]",
                self.omitted
            );
        }
        for turn in &self.turns {
            let _ = writeln!(out, "User: {}", turn.user);
            match &turn.assistant {
                Some(answer) => {
                    let _ = writeln!(out, "Assistant: {answer}");
                }
                None => out.push_str("Assistant: (no answer was produced for this turn)\n"),
            }
        }
        out.truncate(out.trim_end().len());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(n: usize) -> PriorTurn {
        PriorTurn::new(format!("question {n}"), Some(format!("answer {n}")))
    }

    #[test]
    fn an_empty_window_renders_nothing_so_a_first_turn_is_unchanged() {
        let window = ConversationWindow::default();
        assert!(window.is_empty());
        assert_eq!(window.render(), "");
        assert!(ConversationWindow::bounded(Vec::new(), 6, 16_000).is_empty());
    }

    /// The header is the [NFR-CC-04] safeguard: earlier answers are context, never
    /// grounding. A render that lost it would pass every ordering test.
    ///
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    #[test]
    fn the_render_states_the_window_is_context_and_not_grounding() {
        let text = ConversationWindow::bounded(vec![turn(1)], 6, 16_000).render();
        assert!(text.starts_with("Earlier in this conversation (oldest first)."), "{text}");
        assert!(text.contains("context for the current question only"), "{text}");
        assert!(
            text.contains("ground every claim about the codebase in observations gathered this turn"),
            "{text}"
        );
    }

    #[test]
    fn turns_render_oldest_first_with_both_speakers() {
        let window = ConversationWindow::bounded(vec![turn(1), turn(2)], 6, 16_000);
        let text = window.render();
        let positions: Vec<usize> = ["question 1", "answer 1", "question 2", "answer 2"]
            .iter()
            .map(|needle| text.find(needle).unwrap_or_else(|| panic!("{needle} in {text}")))
            .collect();
        assert!(positions.windows(2).all(|w| w[0] < w[1]), "{text}");
        assert!(!text.contains("omitted"), "nothing was dropped: {text}");
        assert!(!text.ends_with('\n'), "no trailing newline: {text:?}");
    }

    #[test]
    fn the_turn_count_drops_the_oldest_and_states_how_many() {
        let window = ConversationWindow::bounded(vec![turn(1), turn(2), turn(3)], 1, 16_000);
        assert_eq!(window.omitted(), 2);
        assert_eq!(window.turns(), [turn(3)]);
        let text = window.render();
        assert!(text.contains("[2 earlier turn(s) omitted from this window]"), "{text}");
        assert!(text.contains("question 3") && !text.contains("question 2"), "{text}");
    }

    #[test]
    fn the_character_ceiling_drops_the_oldest_whole_turns() {
        // Each turn is 10 + 8 = 18 chars ("question N" + "answer N").
        let three = vec![turn(1), turn(2), turn(3)];
        let window = ConversationWindow::bounded(three.clone(), 6, 36);
        assert_eq!(window.turns(), [turn(2), turn(3)], "36 chars fit exactly two turns");
        assert_eq!(window.omitted(), 1);

        let tight = ConversationWindow::bounded(three, 6, 35);
        assert_eq!(tight.turns(), [turn(3)], "one char short drops the next-oldest too");
        assert_eq!(tight.omitted(), 2);
    }

    #[test]
    fn a_ceiling_below_the_newest_turn_keeps_nothing_but_still_states_the_omission() {
        let window = ConversationWindow::bounded(vec![turn(1), turn(2)], 6, 5);
        assert!(window.turns().is_empty());
        assert_eq!(window.omitted(), 2);
        assert!(!window.is_empty(), "an omission is itself something to say");
        assert!(window.render().contains("[2 earlier turn(s) omitted"), "{}", window.render());
    }

    #[test]
    fn the_ceiling_counts_characters_not_bytes() {
        // Four 3-byte characters: 4 chars, 12 bytes.
        let multibyte = PriorTurn::new("日本語日", None);
        assert_eq!(multibyte.chars(), 4);
        let window = ConversationWindow::bounded(vec![multibyte], 6, 4);
        assert_eq!(window.omitted(), 0, "4 characters fit a 4-character ceiling");
    }

    #[test]
    fn an_unanswered_turn_says_so_rather_than_inventing_an_answer() {
        let window =
            ConversationWindow::bounded(vec![PriorTurn::new("what is X?", None)], 6, 16_000);
        let text = window.render();
        assert!(text.contains("User: what is X?"), "{text}");
        assert!(text.contains("Assistant: (no answer was produced for this turn)"), "{text}");
    }
}
