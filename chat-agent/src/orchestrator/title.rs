//! The Synthesizer's conversation-title trailer ([S-623], [FR-UI-48]).
//!
//! On the turn that opens a conversation, the Synthesizer is asked to end its
//! answer with one line, `TITLE: <title>`. That costs no extra provider request
//! ([BR-36], [NFR-SE-07]): the title rides the answer the turn was making anyway.
//! The line is meta-data, never answer text, so it must not reach the reader —
//! not even for the instant between two `answer_delta` frames that split it.
//!
//! Three pieces live here:
//! - [`TITLE_TRAILER_CLAUSE`] — the instruction, a clause appended to whichever
//!   Synthesizer preamble the turn runs under, so a preamble that varies (a
//!   workspace's, or a preset's) composes with it rather than replacing it;
//! - [`TrailerHoldback`] — the [`AnswerSink`] that streams the answer but
//!   withholds any suffix that could still turn out to be the trailer;
//! - [`trailer_title`] — whether a trailer is well formed, which decides between
//!   the Synthesizer's title and the store's deterministic fallback
//!   ([`title_from_first_message`](crate::db::title_from_first_message)).
//!
//! [S-623]: ../../../docs/planning/journal.md#s-623-the-synthesizer-titles-a-new-conversation-and-a-conversation-can-be-renamed
//! [FR-UI-48]: ../../../docs/specs/requirements/FR-UI-48.md
//! [BR-36]: ../../../docs/specs/software-spec.md#326-web-ui
//! [NFR-SE-07]: ../../../docs/specs/requirements/NFR-SE-07.md

use std::sync::Mutex;

use super::step::AnswerSink;
use crate::db::normalize_thread_title;

/// The most words a Synthesizer title may run to ([FR-UI-48]). A longer trailer is
/// malformed: the conversation keeps its cleaned-first-message title.
pub const TITLE_MAX_WORDS: usize = 6;

/// The instruction that asks the Synthesizer for the trailer on a conversation's
/// first turn ([FR-UI-48]). A **separate clause**, appended after the role's own
/// preamble, so any Synthesizer preamble composes with it.
pub const TITLE_TRAILER_CLAUSE: &str = "\
This answer opens a new conversation. After the answer, end your reply with one final \
line of the form `TITLE: <title>`: a title of at most six words, in the language of \
the user's question, naming what the conversation is about. Write nothing after that \
line, and do not mention the title anywhere else.";

/// The trailer's key, compared ASCII-case-insensitively.
const TRAILER_KEY: &str = "title:";

/// A line's head once leading whitespace and markdown emphasis are dropped — so
/// `**TITLE:** …` is recognised as the trailer a model meant it to be.
fn line_head(line: &str) -> &str {
    line.trim_start_matches(|c: char| c.is_whitespace() || c == '*' || c == '_')
}

/// Whether `head` starts with the first `n` bytes of the trailer key.
fn key_prefix(head: &str, n: usize) -> bool {
    head.as_bytes()
        .get(..n)
        .is_some_and(|bytes| bytes.eq_ignore_ascii_case(&TRAILER_KEY.as_bytes()[..n]))
}

/// Whether `line` is a trailer line: its head is `TITLE:` (any case).
fn is_trailer_line(line: &str) -> bool {
    key_prefix(line_head(line), TRAILER_KEY.len())
}

/// Whether `line` — the last, possibly incomplete, line of what has streamed —
/// could still turn out to be a trailer line: it already is one, or its head is
/// a proper prefix of the key (`""`, `"TIT"`, `"**Ti"`).
fn could_become_trailer(line: &str) -> bool {
    let head = line_head(line);
    is_trailer_line(line) || (head.len() < TRAILER_KEY.len() && key_prefix(head, head.len()))
}

/// The byte offset in `text` where the suffix that may still be the trailer
/// begins — everything before it is answer text, whatever streams next.
///
/// The withheld suffix is trailing whitespace, and, when the last non-blank line
/// could still become a trailer line, that line and the whitespace before it. A
/// line that has gone past the key without matching it can never become one, so
/// ordinary prose streams with no delay.
fn held_from(text: &str) -> usize {
    let body = text.trim_end();
    let line_start = body.rfind('\n').map_or(0, |i| i + 1);
    if could_become_trailer(&body[line_start..]) {
        text[..line_start].trim_end().len()
    } else {
        body.len()
    }
}

/// Split `answer` into the answer proper and its trailer line, when its last
/// non-blank line is one. Without a trailer `answer` comes back unchanged. A
/// reply that is nothing but a trailer keeps it as its answer: stripping it would
/// leave no answer at all, and the conversation keeps its fallback title.
#[must_use]
pub fn split_trailer(answer: &str) -> (&str, Option<&str>) {
    let body = answer.trim_end();
    let line_start = body.rfind('\n').map_or(0, |i| i + 1);
    let last = &body[line_start..];
    let rest = answer[..line_start].trim_end();
    if is_trailer_line(last) && !rest.trim().is_empty() {
        (rest, Some(last))
    } else {
        (answer, None)
    }
}

/// The quote and emphasis pairs a model wraps a title in. One is removed only as a
/// pair around the whole value — never half of one, so a title that merely starts
/// or ends with a quoted name keeps both its marks.
const WRAPPERS: &[(&str, &str)] = &[
    ("**", "**"),
    ("__", "__"),
    ("*", "*"),
    ("_", "_"),
    ("\"", "\""),
    ("'", "'"),
    ("`", "`"),
    ("“", "”"),
    ("‘", "’"),
];

/// `value` without one pair of [`WRAPPERS`] around the whole of it, if it has one.
fn unwrap_pair(value: &str) -> &str {
    WRAPPERS
        .iter()
        .find_map(|(open, close)| {
            (value.len() >= open.len() + close.len())
                .then(|| value.strip_prefix(open)?.strip_suffix(close))
                .flatten()
        })
        .unwrap_or(value)
}

/// The title a trailer line names, or `None` when it is malformed: empty, more
/// than [`TITLE_MAX_WORDS`] words, or longer than the store's title cap. The
/// surrounding quotes, emphasis and a closing full stop a model tends to add are
/// not part of the title; they come off as whole pairs, in any nesting, until
/// none is left.
#[must_use]
pub fn trailer_title(line: &str) -> Option<String> {
    if !is_trailer_line(line) {
        return None;
    }
    let mut value = &line_head(line)[TRAILER_KEY.len()..];
    // `**TITLE:** …`: the emphasis that opened before the key closes after it.
    if line.trim_start().starts_with(['*', '_']) {
        value = value.trim_start_matches(['*', '_']);
    }
    loop {
        let before = value;
        value = unwrap_pair(value.trim().trim_end_matches('.').trim_end());
        if value == before {
            break;
        }
    }
    let title = normalize_thread_title(value)?;
    (title.split(' ').count() <= TITLE_MAX_WORDS).then_some(title)
}

/// What [`TrailerHoldback`] has received and how much of it went out.
#[derive(Default)]
struct Held {
    /// Everything streamed so far.
    text: String,
    /// The byte length of `text` already forwarded.
    emitted: usize,
}

/// An [`AnswerSink`] that forwards the Synthesizer's answer while withholding any
/// suffix that could still be the `TITLE:` trailer ([FR-UI-48]). A trailer split
/// across two deltas is therefore never forwarded, in part or whole.
/// [`finish`](Self::finish) releases what turned out to be answer text and
/// returns the answer without its trailer.
pub(crate) struct TrailerHoldback<'a> {
    inner: &'a dyn AnswerSink,
    held: Mutex<Held>,
}

impl<'a> TrailerHoldback<'a> {
    /// Hold back the trailer on its way to `inner`.
    pub(crate) fn new(inner: &'a dyn AnswerSink) -> Self {
        Self {
            inner,
            held: Mutex::new(Held::default()),
        }
    }

    /// Close the stream over the Synthesizer's full `answer`: forward the withheld
    /// text that is answer after all, and return the answer without its trailer
    /// plus the trailer's title when it is well formed.
    pub(crate) fn finish(&self, answer: &str) -> (String, Option<String>) {
        let (body, trailer) = split_trailer(answer);
        let held = self.held.lock().unwrap_or_else(|p| p.into_inner());
        // Forward only a continuation of what already went out: the deltas and the
        // full answer are the same text from a real Synthesizer, and a stand-in that
        // reports something else must not get a mangled tail.
        if let Some(rest) = body
            .get(held.emitted..)
            .filter(|_| body.starts_with(&held.text[..held.emitted]))
            .filter(|rest| !rest.is_empty())
        {
            self.inner.answer_delta(rest);
        }
        (body.to_string(), trailer.and_then(trailer_title))
    }
}

impl AnswerSink for TrailerHoldback<'_> {
    fn answer_delta(&self, delta: &str) {
        let mut held = self.held.lock().unwrap_or_else(|p| p.into_inner());
        held.text.push_str(delta);
        let cut = held_from(&held.text).max(held.emitted);
        if cut > held.emitted {
            self.inner.answer_delta(&held.text[held.emitted..cut]);
            held.emitted = cut;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Collects forwarded deltas.
    #[derive(Default)]
    struct Collect(Mutex<Vec<String>>);

    impl AnswerSink for Collect {
        fn answer_delta(&self, delta: &str) {
            self.0.lock().unwrap().push(delta.to_string());
        }
    }

    impl Collect {
        fn joined(&self) -> String {
            self.0.lock().unwrap().concat()
        }
    }

    /// Stream `chunks` through a holdback and finish over their concatenation.
    fn stream(chunks: &[&str]) -> (Vec<String>, String, Option<String>) {
        let sink = Collect::default();
        let holdback = TrailerHoldback::new(&sink);
        for chunk in chunks {
            holdback.answer_delta(chunk);
        }
        let (answer, title) = holdback.finish(&chunks.concat());
        let deltas = sink.0.lock().unwrap().clone();
        (deltas, answer, title)
    }

    #[test]
    fn a_trailer_split_across_two_deltas_is_never_forwarded() {
        let (deltas, answer, title) = stream(&[
            "Engine has three callers.\n\nTI",
            "TLE: Engine callers overview",
        ]);
        assert_eq!(deltas.concat(), "Engine has three callers.");
        assert!(
            deltas.iter().all(|d| !d.contains("TI")),
            "no part of the trailer leaks: {deltas:?}"
        );
        assert_eq!(answer, "Engine has three callers.");
        assert_eq!(title.as_deref(), Some("Engine callers overview"));
    }

    #[test]
    fn a_trailer_split_mid_key_and_mid_value_is_withheld_at_every_step() {
        let sink = Collect::default();
        let holdback = TrailerHoldback::new(&sink);
        for chunk in ["The answer.", "\n", "T", "itl", "e", ": Bin", "der map\n"] {
            holdback.answer_delta(chunk);
            assert_eq!(
                sink.joined(),
                "The answer.",
                "only the answer has gone out after {chunk:?}"
            );
        }
        let (answer, title) = holdback.finish("The answer.\nTitle: Binder map\n");
        assert_eq!(sink.joined(), "The answer.");
        assert_eq!(answer, "The answer.");
        assert_eq!(title.as_deref(), Some("Binder map"));
    }

    #[test]
    fn ordinary_prose_streams_without_delay() {
        let sink = Collect::default();
        let holdback = TrailerHoldback::new(&sink);
        holdback.answer_delta("Tim");
        assert_eq!(
            sink.joined(),
            "Tim",
            "a line past the key is released at once"
        );
        holdback.answer_delta("e is short.\nThe end");
        assert_eq!(sink.joined(), "Time is short.\nThe end");
    }

    #[test]
    fn a_title_line_that_is_not_last_is_answer_text() {
        let (deltas, answer, title) = stream(&["Title: a heading\n", "and the body follows."]);
        assert_eq!(deltas.concat(), "Title: a heading\nand the body follows.");
        assert_eq!(answer, "Title: a heading\nand the body follows.");
        assert_eq!(title, None);
    }

    #[test]
    fn without_a_trailer_the_answer_streams_whole_and_unchanged() {
        let (deltas, answer, title) = stream(&["Just an answer.", "\n\n"]);
        assert_eq!(
            deltas.concat(),
            "Just an answer.\n\n",
            "held whitespace is released at the end"
        );
        assert_eq!(answer, "Just an answer.\n\n");
        assert_eq!(title, None);
    }

    #[test]
    fn a_malformed_or_over_long_trailer_is_stripped_but_names_nothing() {
        let (_, answer, title) = stream(&["Answer.\nTITLE: one two three four five six seven"]);
        assert_eq!(
            answer, "Answer.",
            "an over-long trailer still never reaches the answer"
        );
        assert_eq!(title, None);
        let (_, answer, title) = stream(&["Answer.\nTITLE:   "]);
        assert_eq!(answer, "Answer.");
        assert_eq!(title, None, "an empty trailer names nothing");
    }

    #[test]
    fn exactly_six_words_is_a_title() {
        assert_eq!(
            trailer_title("TITLE: one two three four five six").as_deref(),
            Some("one two three four five six")
        );
    }

    #[test]
    fn decoration_around_the_trailer_is_not_part_of_the_title() {
        assert_eq!(
            trailer_title("**TITLE:** \"Binder map.\"").as_deref(),
            Some("Binder map")
        );
        assert_eq!(
            trailer_title("title: `Gate drift`").as_deref(),
            Some("Gate drift")
        );
        // Pairs nest and may sit inside or outside a closing full stop.
        for (line, title) in [
            ("TITLE: \"Binder map\".", "Binder map"),
            ("TITLE: **Gate drift**.", "Gate drift"),
            ("TITLE: *\"Binder map\"*", "Binder map"),
            ("TITLE: “Risk in alpha.”", "Risk in alpha"),
        ] {
            assert_eq!(trailer_title(line).as_deref(), Some(title), "{line}");
        }
    }

    #[test]
    fn a_mark_that_is_not_a_whole_pair_stays_in_the_title() {
        for (line, title) in [
            ("TITLE: `Engine::run` callers", "`Engine::run` callers"),
            (
                "TITLE: Callers of `Engine::run`",
                "Callers of `Engine::run`",
            ),
            ("TITLE: \"Foo\" bar baz", "\"Foo\" bar baz"),
        ] {
            assert_eq!(trailer_title(line).as_deref(), Some(title), "{line}");
        }
    }

    #[test]
    fn a_near_miss_key_is_not_a_trailer() {
        assert_eq!(trailer_title("TITLES: nope"), None);
        assert_eq!(trailer_title("TITLE nope"), None);
        assert_eq!(trailer_title("SUBTITLE: nope"), None);
        let (answer, trailer) = split_trailer("Answer.\nTITLE nope");
        assert_eq!((answer, trailer), ("Answer.\nTITLE nope", None));
    }

    #[test]
    fn a_reply_that_is_only_a_trailer_keeps_it() {
        assert_eq!(split_trailer("TITLE: lonely"), ("TITLE: lonely", None));
    }

    #[test]
    fn multi_byte_text_never_splits_a_character() {
        let (deltas, answer, title) = stream(&["日本語の答え。\n", "TITLE: 日本語"]);
        assert_eq!(deltas.concat(), "日本語の答え。");
        assert_eq!(answer, "日本語の答え。");
        assert_eq!(title.as_deref(), Some("日本語"));
    }
}
