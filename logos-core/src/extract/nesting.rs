//! Per-function maximum nesting depth via a declarative block-kind walk
//! (CR-005, [FR-EX-07], [ADR-08], [ADR-21]).
//!
//! The companion of [`super::complexity`]: where cyclomatic complexity counts
//! decision-point *tokens*, nesting depth measures how deeply the function's
//! control-flow constructs are *stacked*. Both read a declarative descriptor
//! field ([NFR-MA-05]) so a new language tunes the metric without a core change
//! ([NFR-MA-01]) — here the field is `nesting_block_kinds`, the tree-sitter node
//! kinds that introduce one nesting level (`if_expression`, `for_statement`, …).
//!
//! # The depth contract ([FR-EX-07])
//!
//! Depth `0` is a flat body — no nested control-flow construct. Each
//! block-structure node entered on a path increments the depth by one, and the
//! result is the maximum over every path through the function's subtree. So a
//! function whose deepest point is `if { while { match { if { … } } } }` scores
//! `4`. The function node itself is depth `0`; only the declared block kinds
//! count, never the function's own body block — keeping the count language-
//! correct and hand-verifiable regardless of how a grammar wraps a body.
//!
//! # Determinism ([NFR-RA-06])
//!
//! A pure function of the parse tree and the declared kind set: the same source
//! yields the same depth on every run and every release target. The walk is
//! iterative on an explicit stack — **never** native recursion — because AST
//! depth is input-controlled and a stack overflow would abort the whole index
//! run ([FR-IX-04]); the same hazard and mitigation as [`super::complexity`] and
//! [`super::shape`].
//!
//! [FR-EX-07]: ../../../docs/specs/requirements/FR-EX-07.md
//! [FR-IX-04]: ../../../docs/specs/requirements/FR-IX-04.md
//! [NFR-MA-01]: ../../../docs/specs/requirements/NFR-MA-01.md
//! [NFR-MA-05]: ../../../docs/specs/requirements/NFR-MA-05.md
//! [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
//! [ADR-08]: ../../../docs/specs/architecture/decisions/ADR-08.md
//! [ADR-21]: ../../../docs/specs/architecture/decisions/ADR-21.md

use tree_sitter::Node;

/// The maximum block-structure nesting depth within `node`'s subtree.
///
/// `block_kinds` is the language's declared `nesting_block_kinds`: each
/// descendant whose kind is in the set adds one level to the running depth, and
/// the returned value is the deepest such stack on any path. `0` for a flat
/// body or an empty kind set.
pub(crate) fn max_nesting_depth(node: Node<'_>, block_kinds: &[String]) -> u32 {
    let mut max_depth = 0u32;
    // Each frame carries the depth already accrued *at* that node (the count of
    // block-kind ancestors, the node itself included when it is a block kind).
    // The root function node is depth 0 and is never itself a block kind.
    let mut stack: Vec<(Node<'_>, u32)> = vec![(node, 0)];
    while let Some((current, depth)) = stack.pop() {
        let mut cursor = current.walk();
        for child in current.children(&mut cursor) {
            let child_depth = if block_kinds.iter().any(|k| k == child.kind()) {
                // `saturating_add` so a pathologically deep generated function
                // can never overflow-panic (debug) or wrap (release).
                let d = depth.saturating_add(1);
                max_depth = max_depth.max(d);
                d
            } else {
                depth
            };
            stack.push((child, child_depth));
        }
    }
    max_depth
}

#[cfg(all(test, feature = "lang-rust"))]
mod tests {
    use super::*;
    use tree_sitter::Parser;

    /// The Rust plugin's declared nesting block kinds (plugins/rust/plugin.toml).
    fn rust_block_kinds() -> Vec<String> {
        [
            "if_expression",
            "match_expression",
            "while_expression",
            "loop_expression",
            "for_expression",
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

    /// First `function_item` node in the tree (pre-order).
    fn first_function(tree: &tree_sitter::Tree) -> Node<'_> {
        let mut stack = vec![tree.root_node()];
        while let Some(n) = stack.pop() {
            if n.kind() == "function_item" {
                return n;
            }
            for i in (0..n.child_count()).rev() {
                if let Some(c) = n.child(i) {
                    stack.push(c);
                }
            }
        }
        panic!("no function_item in tree");
    }

    fn depth_of(src: &str) -> u32 {
        let tree = parse(src);
        max_nesting_depth(first_function(&tree), &rust_block_kinds())
    }

    #[test]
    fn flat_function_is_depth_zero() {
        assert_eq!(depth_of("fn f() { let x = 1; let y = x + 2; }"), 0);
    }

    #[test]
    fn a_single_if_is_depth_one() {
        assert_eq!(depth_of("fn f(a: bool) { if a { g(); } }"), 1);
    }

    #[test]
    fn four_levels_of_nesting_yield_exactly_four() {
        // if { while { match { if { … } } } } — a hand-verifiable depth of 4.
        let src = r#"
fn f(a: bool, n: u32) {
    if a {
        while a {
            match n {
                0 => { if a { g(); } }
                _ => {}
            }
        }
    }
}
"#;
        assert_eq!(depth_of(src), 4);
    }

    #[test]
    fn sibling_branches_take_the_deepest_path_not_the_sum() {
        // Two siblings of depth 1 and 2 → the function's depth is the max (2),
        // never the sum.
        let src = r#"
fn f(a: bool) {
    if a { g(); }
    if a { while a { g(); } }
}
"#;
        assert_eq!(depth_of(src), 2);
    }

    #[test]
    fn for_and_loop_count_as_nesting_levels() {
        let src = r#"
fn f() {
    for _ in 0..3 {
        loop { break; }
    }
}
"#;
        assert_eq!(depth_of(src), 2);
    }

    #[test]
    fn an_empty_block_kind_set_is_always_zero() {
        let tree = parse("fn f(a: bool) { if a { while a { g(); } } }");
        assert_eq!(max_nesting_depth(first_function(&tree), &[]), 0);
    }
}
