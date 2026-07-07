//! Cyclomatic complexity via a declarative keyword walk ([FR-EX-03], [ADR-08]).
//!
//! There is no `complexity.scm` query yet (the v1 Rust plugin ships only
//! `symbols`), so this is the architecture's sanctioned *fallback keyword walk*
//! ([extraction-engine](../../../docs/specs/architecture/components/extraction-engine.md)):
//! cyclomatic complexity is `1 + (number of decision points)`, where a decision
//! point is any anonymous token node whose kind is one of the plugin's declared
//! `complexity_keywords` (`if`, `match`, `while`, `&&`, `?`, …).
//!
//! Matching on **anonymous token-node kinds** — not raw source text — is what
//! keeps the count language-correct and deterministic: a keyword appearing
//! inside a string literal, a comment, or an identifier is a *named* node (or a
//! child of one) and never an anonymous `if`/`match`/`&&` token, so it is not
//! miscounted. The keyword set is the descriptor's declarative, on-disk-tunable
//! contract ([NFR-MA-05]), so the metric tracks whatever the plugin declares.
//!
//! The walk covers the whole declaration subtree, including nested closures and
//! nested items — a deliberate simplification for the keyword-walk fallback; a
//! language-precise `complexity.scm` can refine this later without touching the
//! call site.
//!
//! [FR-EX-03]: ../../../docs/specs/requirements/FR-EX-03.md
//! [ADR-08]: ../../../docs/specs/architecture/decisions/ADR-08.md
//! [NFR-MA-05]: ../../../docs/specs/requirements/NFR-MA-05.md

use tree_sitter::Node;

/// Cyclomatic complexity of `node`'s subtree: `1 + decision points`.
///
/// A decision point is any anonymous (non-named) descendant token whose kind is
/// in `keywords`. The base of 1 is the single entry path every function has.
pub(crate) fn cyclomatic_complexity(node: Node<'_>, keywords: &[String]) -> u32 {
    let mut decision_points = 0u32;
    // Iterative pre-order walk over every descendant (named and anonymous). A
    // manual stack avoids recursion blowing the native stack on deeply nested
    // functions and keeps the traversal allocation-light.
    let mut stack: Vec<Node<'_>> = vec![node];
    while let Some(current) = stack.pop() {
        let child_count = current.child_count();
        for i in 0..child_count {
            let Some(child) = current.child(i) else {
                continue;
            };
            if !child.is_named() && keywords.iter().any(|kw| kw == child.kind()) {
                // `saturating_add` so a pathologically large generated function
                // can never overflow-panic (debug) or wrap (release).
                decision_points = decision_points.saturating_add(1);
            }
            stack.push(child);
        }
    }
    decision_points.saturating_add(1)
}

#[cfg(all(test, feature = "lang-rust"))]
mod tests {
    use super::*;
    use tree_sitter::Parser;

    /// The Rust plugin's declared complexity keywords (plugins/rust/plugin.toml).
    fn rust_keywords() -> Vec<String> {
        [
            "if", "else", "match", "while", "for", "loop", "&&", "||", "?",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    fn parse(src: &str) -> tree_sitter::Tree {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .unwrap();
        parser.parse(src, None).unwrap()
    }

    /// Find the first `function_item` node in the tree (pre-order).
    fn first_function(tree: &tree_sitter::Tree) -> Node<'_> {
        let mut stack = vec![tree.root_node()];
        while let Some(n) = stack.pop() {
            if n.kind() == "function_item" {
                return n;
            }
            // Push children in reverse so we pop in source order (not required
            // for correctness here, but keeps "first" intuitive).
            for i in (0..n.child_count()).rev() {
                if let Some(c) = n.child(i) {
                    stack.push(c);
                }
            }
        }
        panic!("no function_item in tree");
    }

    #[test]
    fn straight_line_function_has_complexity_one() {
        let tree = parse("fn f() { let x = 1; let y = x + 2; }");
        let cc = cyclomatic_complexity(first_function(&tree), &rust_keywords());
        assert_eq!(cc, 1, "no decision points → CC of 1");
    }

    #[test]
    fn each_decision_keyword_adds_one() {
        // if (+1), else (+1), && (+1): base 1 → 4.
        let src = "fn f(a: bool, b: bool) { if a && b { g(); } else { h(); } }";
        let tree = parse(src);
        let cc = cyclomatic_complexity(first_function(&tree), &rust_keywords());
        assert_eq!(cc, 4, "if + else + && over base 1");
    }

    #[test]
    fn match_loop_and_try_are_counted() {
        // match (+1), while (+1), ? (+1) → base 1 = 4.
        let src = r#"
fn f(n: u32) -> Option<u32> {
    match n { 0 => {}, _ => {} }
    while takes()? { let _ = 1; }
    Some(n)
}
"#;
        let tree = parse(src);
        let cc = cyclomatic_complexity(first_function(&tree), &rust_keywords());
        assert_eq!(cc, 4, "match + while + ? over base 1");
    }

    #[test]
    fn keyword_inside_string_or_comment_is_not_counted() {
        // The words "if"/"match" appear in a string and a comment but are not
        // anonymous keyword tokens, so they must not inflate CC.
        let src = r#"
fn f() {
    // if match while
    let s = "if match && ||";
    let _ = s;
}
"#;
        let tree = parse(src);
        let cc = cyclomatic_complexity(first_function(&tree), &rust_keywords());
        assert_eq!(
            cc, 1,
            "keywords in strings/comments are not decision points"
        );
    }

    #[test]
    fn an_empty_keyword_set_yields_base_complexity() {
        let tree = parse("fn f(a: bool) { if a { g(); } }");
        let cc = cyclomatic_complexity(first_function(&tree), &[]);
        assert_eq!(cc, 1, "no declared keywords → only the base path counts");
    }
}
