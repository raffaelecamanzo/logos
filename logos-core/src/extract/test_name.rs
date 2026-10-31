//! The name of a test a macro declares (S-580, [CR-184], [FR-EX-06]): C++'s
//! GoogleTest `TEST(Suite, Name) { … }` and Catch2's `TEST_CASE("name", "[tag]")
//! { … }`, which name the test by their arguments rather than by the macro.
//!
//! The `symbols` query marks the declaration and what names it, and records no
//! row of its own for any of them:
//!
//! - `@test.decl` — the declaration node: a GoogleTest `function_definition`,
//!   or a Catch2 test's body block, which is a sibling of the `TEST_CASE(…)`
//!   statement that heads it and so the only node holding both the body's
//!   references and its span;
//! - `@test.suite` — a GoogleTest suite, a `type_identifier` argument;
//! - `@test.name` — a GoogleTest test name (`type_identifier`) or a Catch2 test
//!   name (`string_literal`);
//! - `@test.head` — the Catch2 statement that heads the block, whose line the
//!   test starts on.
//!
//! A GoogleTest test is named `Suite.Name`, as the runner prints it; a Catch2
//! test by its string. Every other declaration keeps the name its own capture
//! gave. The names are applied after the walk ([`TestNames::apply`]), keyed by
//! the declaration node, so which pattern named the declaration first is no
//! matter.
//!
//! [CR-184]: ../../../docs/requests/CR-184-cpp-resolves-includes-out-of-line-members-and-relations.md
//! [FR-EX-06]: ../../../docs/specs/requirements/FR-EX-06.md

use std::collections::{HashMap, HashSet};

use tree_sitter::{Node, QueryCapture};

use super::{match_capture, Decl};

const DECL: &str = "test.decl";
const SUITE: &str = "test.suite";
const NAME: &str = "test.name";
const HEAD: &str = "test.head";

/// What the `@test.*` captures say of the declarations of one file.
#[derive(Default)]
pub(super) struct TestNames {
    /// declaration node id → (its name, the line it starts on, when a head says so).
    named: HashMap<usize, (String, Option<u32>)>,
    /// The declaration nodes a macro marks whose name no argument spells.
    unnamed: HashSet<usize>,
}

impl TestNames {
    /// Take `capture` when it belongs to the vocabulary; `false` for any other,
    /// which the caller reads itself. The match's other captures are read at its
    /// `@test.decl`.
    pub(super) fn note(
        &mut self,
        capture: &str,
        node: Node<'_>,
        captures: &[QueryCapture<'_>],
        capture_names: &[&str],
        source: &[u8],
    ) -> bool {
        match capture {
            DECL => {
                let text = |name| match_capture(captures, capture_names, name).and_then(|n| written_name(n, source));
                if let Some(name) = text(NAME) {
                    let name = match text(SUITE) {
                        Some(suite) => format!("{suite}.{name}"),
                        None => name,
                    };
                    let head = match_capture(captures, capture_names, HEAD).map(|n| n.start_position().row as u32 + 1);
                    self.named.entry(node.id()).or_insert((name, head));
                } else {
                    self.unnamed.insert(node.id());
                }
                true
            }
            SUITE | NAME | HEAD => true,
            _ => false,
        }
    }

    /// Rename every declaration a test macro names, and start a Catch2 test at
    /// the statement that heads its block. A Catch2 block whose string is empty
    /// names nothing and is no declaration: its own capture named it for the
    /// block's text.
    pub(super) fn apply(self, decls: &mut Vec<Decl<'_>>) {
        if self.named.is_empty() && self.unnamed.is_empty() {
            return;
        }
        decls.retain(|d| !(d.node.kind() == "compound_statement" && self.unnamed.contains(&d.node.id())));
        for decl in decls {
            if let Some((name, head)) = self.named.get(&decl.node.id()) {
                decl.name.clone_from(name);
                if let Some(line) = head.filter(|&line| line < decl.start_line) {
                    decl.start_line = line;
                }
            }
        }
    }
}

/// The name a captured argument spells: an identifier as written, a string
/// literal without its quotes (and its encoding prefix). `None` for a string
/// that is empty, or an argument that is no name.
fn written_name(node: Node<'_>, source: &[u8]) -> Option<String> {
    let text = node.utf8_text(source).ok()?.trim();
    let name = if node.kind() == "string_literal" {
        let (open, close) = (text.find('"')?, text.rfind('"')?);
        text.get(open + 1..close.max(open + 1))?.trim()
    } else {
        text
    };
    (!name.is_empty()).then(|| name.to_string())
}

#[cfg(all(test, feature = "lang-cpp"))]
mod tests {
    use super::*;
    use tree_sitter::Parser;

    fn string_literal_name(src: &str) -> Option<String> {
        let mut parser = Parser::new();
        parser.set_language(&tree_sitter_cpp::LANGUAGE.into()).unwrap();
        let tree = parser.parse(src, None).unwrap();
        let root = tree.root_node();
        let mut stack = vec![root];
        while let Some(n) = stack.pop() {
            if n.kind() == "string_literal" {
                return written_name(n, src.as_bytes());
            }
            let mut cursor = n.walk();
            stack.extend(n.children(&mut cursor));
        }
        None
    }

    #[test]
    fn a_string_is_named_by_its_content() {
        assert_eq!(string_literal_name("f(\"adds numbers\");").as_deref(), Some("adds numbers"));
        assert_eq!(string_literal_name("f(u8\"wide\");").as_deref(), Some("wide"));
        assert_eq!(string_literal_name("f(\"a \\\"quoted\\\" word\");").as_deref(), Some("a \\\"quoted\\\" word"));
    }

    #[test]
    fn an_empty_string_names_nothing() {
        assert_eq!(string_literal_name("f(\"\");"), None);
        assert_eq!(string_literal_name("f(\"  \");"), None);
    }
}
