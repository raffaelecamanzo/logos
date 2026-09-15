//! The **path-neutral composer** contract test (S-405, [CR-129], [FR-WS-08]
//! AC2, [NFR-RA-05]).
//!
//! A URI *composer* is a fluent chain a client call hands its path to instead of
//! passing the path directly — Spring's `.uri(builder -> builder.path(…)…)`
//! being the shape [FR-WS-08]'s normative Java row writes. The chain's links are
//! left-nested calls of arbitrary length, which a tree-sitter pattern cannot
//! spell: a pattern's nesting is fixed, while the reference estate's own chains
//! run from 4 links (`path … queryParam … queryParam … build`) to 9 on the same
//! shape. So the **shape** stays in the plugin's query data
//! ([FR-WS-07]) — which link supplies the path, which links are path-neutral,
//! which link is the terminal, each declared by its own capture name and its own
//! text predicate — and this module does the one thing a query cannot: reconcile
//! those matches by byte range into a single answer.
//!
//! # The contract test ([CR-129] AC1)
//!
//! A link is path-neutral **iff it provably cannot alter the path template**
//! [FR-CG-09] matches a provider route on. The query declares that as a test
//! over the link's name, never as a list of admitted method names: see
//! `plugins/java/queries/invocations.scm` pattern 5b, which is where the
//! vocabulary and the reasoning behind it live. Nothing in this module knows
//! any method name, in any language.
//!
//! The reconciliation is what keeps the test honest, and it fails **closed**:
//! every link between the path link and the chain's root must have been matched
//! as path-neutral (or, for the root alone, as the terminal). A link no query
//! pattern classified is a link nothing proved harmless, so the composer refuses
//! and the call records the refusal it already recorded.
//!
//! # Stated ceiling: the chain is walked by PARENT
//!
//! [`Composers::operand`] walks from the path link to the chain root through
//! `Node::parent`, so it assumes a grammar where each link of a fluent chain is
//! the direct parent of the link it is called on — true of `tree-sitter-java`'s
//! `method_invocation`, whose `object:` field holds the inner call. A grammar
//! that wraps the receiver in a separate node (JavaScript's `member_expression`
//! between two `call_expression`s) would see that wrapper as an unclassified
//! link and refuse. That is the safe direction, and it is a ceiling rather than
//! a defect: [CR-129] §3.3 scopes every other language's composer idiom out, and
//! a port is its own story with its own descriptor data.
//!
//! [CR-129]: ../../../docs/requests/CR-129-path-neutral-composer-link-in-a-uribuilder-lambda.md
//! [FR-CG-09]: ../../../docs/specs/requirements/FR-CG-09.md
//! [FR-WS-07]: ../../../docs/specs/requirements/FR-WS-07.md
//! [FR-WS-08]: ../../../docs/specs/requirements/FR-WS-08.md
//! [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md

use std::ops::Range;

use tree_sitter::{Node, Query, QueryCursor, StreamingIterator};

/// The composer chain a call's path is composed in — the node whose subtree the
/// links below are reconciled within. Bound by the **site** pattern, beside the
/// call's `@invoke.http.method`.
pub const COMPOSER: &str = "invoke.http.composer";
/// The name the composer's innermost receiver must be — a lambda's own
/// parameter, so a `path(…)` call on some other object in the same expression is
/// not read as the builder's.
pub const COMPOSER_RECEIVER: &str = "invoke.http.composer.receiver";
/// The whole link that supplies the path template.
const COMPOSER_PATH: &str = "invoke.http.composer.path";
/// That link's receiver, compared against [`COMPOSER_RECEIVER`].
const COMPOSER_ANCHOR: &str = "invoke.http.composer.anchor";
/// That link's single argument — the operand the arm judges exactly as it judges
/// a directly-passed one.
const COMPOSER_OPERAND: &str = "invoke.http.composer.operand";
/// A link the query proved path-neutral.
const COMPOSER_NEUTRAL: &str = "invoke.http.composer.neutral";
/// The chain's terminal, admitted only as the chain's outermost link.
const COMPOSER_TERMINAL: &str = "invoke.http.composer.terminal";

/// One `path(<operand>)` link: the link itself, its receiver, and its operand.
#[derive(Debug, Clone, Copy)]
struct PathLink<'t> {
    link: Node<'t>,
    anchor: Node<'t>,
    operand: Node<'t>,
}

/// Every composer-vocabulary node a file's `invocations` query matched, indexed
/// by nothing — the populations are small (one entry per fluent link in a
/// client-gated file) and the reconciliation is a containment test, which no
/// index makes cheaper at this size.
#[derive(Debug, Default)]
pub struct Composers<'t> {
    path_links: Vec<PathLink<'t>>,
    neutral: Vec<Range<usize>>,
    terminal: Vec<Range<usize>>,
}

impl<'t> Composers<'t> {
    /// Collect the vocabulary in one pass over `root`.
    ///
    /// Returns empty — and runs no pass at all — for a query that declares no
    /// composer captures, which is every language but Java today. The whole cost
    /// of this module for such a language is one `capture_names` scan.
    pub fn collect(query: &Query, root: Node<'t>, source: &'t [u8]) -> Self {
        let names = query.capture_names();
        if !names.contains(&COMPOSER_PATH) {
            return Self::default();
        }
        let mut found = Self::default();
        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(query, root, source);
        while let Some(m) = matches.next() {
            let (mut link, mut anchor, mut operand) = (None, None, None);
            for cap in m.captures {
                match names[cap.index as usize] {
                    COMPOSER_PATH => link = Some(cap.node),
                    COMPOSER_ANCHOR => anchor = Some(cap.node),
                    COMPOSER_OPERAND => operand = Some(cap.node),
                    COMPOSER_NEUTRAL => found.neutral.push(cap.node.byte_range()),
                    COMPOSER_TERMINAL => found.terminal.push(cap.node.byte_range()),
                    _ => {}
                }
            }
            if let (Some(link), Some(anchor), Some(operand)) = (link, anchor, operand) {
                found.path_links.push(PathLink {
                    link,
                    anchor,
                    operand,
                });
            }
        }
        found
    }

    /// The operand `composer` composes its path from, or `None` when the chain
    /// is not one this arm can prove path-neutral end to end.
    ///
    /// `None` is not a refusal of its own: the call site that asked already
    /// carries the wider match's `base-url-runtime` candidate, so a composer
    /// this declines records exactly the row it recorded before the composer was
    /// looked at ([FR-WS-05]).
    ///
    /// [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
    pub fn operand(
        &self,
        composer: Node<'t>,
        receiver: Node<'t>,
        source: &[u8],
    ) -> Option<Node<'t>> {
        let chain = composer.byte_range();
        let receiver = receiver.utf8_text(source).ok()?.trim();

        // EXACTLY ONE path link in the chain. A second one anywhere inside it —
        // chained (`path(a).path(b)`) or nested in another link's argument — is
        // a template composed from two operands, and this arm reads one.
        let mut path_link: Option<PathLink<'t>> = None;
        for candidate in &self.path_links {
            let range = candidate.link.byte_range();
            if range.start < chain.start || range.end > chain.end {
                continue;
            }
            if path_link.is_some() {
                return None;
            }
            path_link = Some(*candidate);
        }
        let path_link = path_link?;

        // It must be the chain's INNERMOST link, and be called on the name the
        // site pattern says the composer belongs to. A left-nested chain shares
        // its start offset with every one of its links, so the innermost link is
        // the one that also ends first — equivalently, the only one that starts
        // where the chain starts and is a descendant of all the others.
        if path_link.link.start_byte() != chain.start {
            return None;
        }
        if path_link.anchor.utf8_text(source).ok()?.trim() != receiver {
            return None;
        }

        // Every remaining link, from the path link up to and including the
        // chain's root, must have been classified by the query. `build()` counts
        // only as the root: a call AFTER the terminal operates on what the
        // terminal returned — a `URI`, not the builder — so the builder's
        // contract proves nothing about it (`build().normalize()` rewrites the
        // path it was handed).
        let mut node = path_link.link;
        while node.byte_range() != chain {
            let parent = node.parent()?;
            let range = parent.byte_range();
            if range.start != chain.start || range.end > chain.end {
                return None; // not a link of this chain
            }
            let neutral = self.neutral.contains(&range);
            let terminal = range == chain && self.terminal.contains(&range);
            if !neutral && !terminal {
                return None;
            }
            node = parent;
        }
        Some(path_link.operand)
    }
}
