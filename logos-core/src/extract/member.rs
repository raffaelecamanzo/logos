//! Which aggregate a C-family member belongs to (S-582, [FR-EX-31]).
//!
//! A header the grammar cannot read whole — its macros are never expanded — is
//! torn by error recovery: libuv's `uv.h` writes `struct uv_handle_s {
//! UV_HANDLE_FIELDS };`, and from there on the file is one ERROR region in
//! which `struct uv_loop_s {` is three loose tokens and its members loose
//! `field_declaration`s beside them. Read naively, every member there is a
//! field of no aggregate, and the struct has none. Two markers of a plugin's
//! own `symbols` query put each member where it belongs ([NFR-MA-01]); a
//! plugin that declares neither extracts exactly what it did before.
//!
//! # The `symbols`-query vocabulary (the `member.` group)
//!
//! - `@member.of` — on a `@symbol.field` match: the field is a member of an
//!   aggregate body. One that no declaration of the file contains, and no torn
//!   head claims, is a member of nothing the file declares — a declaration
//!   recovery stranded outside every body, such as `UV_EXTERN uv_handle_type`
//!   read as a field — and is not extracted ([FR-EX-30]).
//! - `@member.torn` — on a struct match whose head recovery tore from its body
//!   (`NAME "{"` loose in an ERROR). It is a head only when the source writes
//!   `struct` or `union` before the name — read off the text, since recovery
//!   does not always leave the keyword a sibling — and is dropped otherwise. A
//!   head owns the member
//!   declarations that follow it in that region, up to the brace that closes
//!   it: a `}` beside them, or one recovery folded into the last of them
//!   (`UV_LOOP_PRIVATE_FIELDS }`). Anything else ends the body too, so a head
//!   never claims a declaration that is not a member.
//!
//! [FR-EX-31]: ../../../docs/specs/requirements/FR-EX-31.md
//! [FR-EX-30]: ../../../docs/specs/requirements/FR-EX-30.md
//! [NFR-MA-01]: ../../../docs/specs/requirements/NFR-MA-01.md

use std::collections::HashMap;

use tree_sitter::{Node, QueryCapture};

use super::Decl;

/// The marker of a member of an aggregate body (see the module docs).
const MEMBER_CAPTURE: &str = "member.of";

/// The marker of an aggregate head torn from its body (see the module docs).
const TORN_HEAD_CAPTURE: &str = "member.torn";

/// What the `@member.*` markers say about one declaration.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct DeclMemberFacts {
    /// The declaration is a member of an aggregate body (`@member.of`).
    member: bool,
    /// The declaration is an aggregate head torn from its body (`@member.torn`).
    torn_head: bool,
    /// The node id of the torn head that owns this member, set by [`settle`].
    claimed_by: Option<usize>,
}

impl DeclMemberFacts {
    /// The markers among the captures of the match that declared it.
    pub(super) fn of_match(captures: &[QueryCapture<'_>], capture_names: &[&str]) -> Self {
        let marked = |name: &str| captures.iter().any(|c| capture_names[c.index as usize] == name);
        Self {
            member: marked(MEMBER_CAPTURE),
            torn_head: marked(TORN_HEAD_CAPTURE),
            claimed_by: None,
        }
    }

    /// The node id of the torn head that owns this member, when one does.
    pub(super) fn claimed_by(self) -> Option<usize> {
        self.claimed_by
    }
}

/// Drop each torn head the source does not write as a `struct` or `union`, give
/// every other the members that follow it, then drop every member that neither
/// a declaration of the file contains nor a torn head claims. Runs before
/// parents exist, so no index moves; [`super::assign_parents`] places a claimed
/// member at its head.
///
/// Returns how many torn heads it dropped: each was counted a declaration the
/// parse error truncated ([FR-EX-30]), and is none.
///
/// [FR-EX-30]: ../../../docs/specs/requirements/FR-EX-30.md
pub(super) fn settle(decls: &mut Vec<Decl<'_>>, source: &[u8]) -> usize {
    if !decls.iter().any(|d| d.member.member || d.member.torn_head) {
        return 0;
    }
    let before = decls.len();
    decls.retain(|d| !d.member.torn_head || follows_aggregate_keyword(source, d.node.start_byte()));
    let not_heads = before - decls.len();
    let mut by_node: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, decl) in decls.iter().enumerate() {
        by_node.entry(decl.node.id()).or_default().push(i);
    }
    // (declaration index, the torn head that owns it) for every declaration of
    // every torn body.
    let claims: Vec<(usize, usize)> = decls
        .iter()
        .filter(|d| d.member.torn_head)
        .flat_map(|d| torn_body(d.node).into_iter().map(move |member| (member, d.node.id())))
        .flat_map(|(member, head)| by_node.get(&member.id()).into_iter().flatten().map(move |&i| (i, head)))
        .collect();
    for (i, head) in claims {
        if decls[i].member.member {
            decls[i].member.claimed_by = Some(head);
        }
    }
    let keep: Vec<bool> = (0..decls.len()).map(|i| contained(decls, i, &by_node)).collect();
    let mut keep = keep.into_iter();
    decls.retain(|_| keep.next().unwrap_or(true));
    not_heads
}

/// Whether declaration `i` stays: a non-member always does; a member does when
/// a torn head claims it or a declaration that stays encloses it — a member
/// nested in a stranded one is stranded with it, so the walk climbs past
/// enclosing members to the first declaration that stays on its own account.
fn contained(decls: &[Decl<'_>], i: usize, by_node: &HashMap<usize, Vec<usize>>) -> bool {
    let stays = |d: &Decl<'_>| !d.member.member || d.member.claimed_by.is_some();
    if stays(&decls[i]) {
        return true;
    }
    let mut ancestor = decls[i].node.parent();
    while let Some(node) = ancestor {
        if by_node.get(&node.id()).and_then(|js| js.first()).is_some_and(|&j| stays(&decls[j])) {
            return true;
        }
        ancestor = node.parent();
    }
    false
}

/// The member declarations a torn head owns: its siblings after the `{` that
/// follows it, while they are member declarations — or `#if`/`#ifdef` blocks
/// of them, either branch — up to the closing brace.
fn torn_body(head: Node<'_>) -> Vec<Node<'_>> {
    let mut body = Vec::new();
    let mut sibling = head.next_sibling();
    // The opening brace (comments may sit between) — the pattern anchored it.
    while let Some(node) = sibling {
        sibling = node.next_sibling();
        if node.kind() == "{" {
            break;
        }
    }
    while let Some(node) = sibling {
        let closed = match node.kind() {
            "comment" => false,
            "field_declaration" => {
                body.push(node);
                closes(node)
            }
            kind if kind.starts_with("preproc_if") => conditional_members(node, &mut body),
            _ => break,
        };
        if closed {
            break;
        }
        sibling = node.next_sibling();
    }
    body
}

/// Collect the member declarations of an `#if`/`#ifdef` block in a torn body,
/// every branch and nested block; `true` when one of them holds the body's
/// closing brace.
fn conditional_members<'t>(block: Node<'t>, body: &mut Vec<Node<'t>>) -> bool {
    let mut closed = false;
    for child in children(block) {
        if child.kind() == "field_declaration" {
            body.push(child);
            closed |= closes(child);
        } else if child.kind().starts_with("preproc_") {
            closed |= conditional_members(child, body);
        }
    }
    closed
}

/// Whether recovery folded the body's closing brace into `member`: a `}` in
/// an ERROR inside it (`UV_LOOP_PRIVATE_FIELDS }`). A nested aggregate's own
/// braces sit in its `field_declaration_list`, never in an ERROR.
fn closes(member: Node<'_>) -> bool {
    children(member).any(|child| child.is_error() && children(child).any(|t| t.kind() == "}"))
}

/// Every child of `node`, named or not.
fn children<'t>(node: Node<'t>) -> impl Iterator<Item = Node<'t>> {
    (0..node.child_count()).filter_map(move |k| node.child(k))
}

/// Whether the source writes `struct` or `union` — as a whole word — right
/// before byte `at`, whitespace aside.
fn follows_aggregate_keyword(source: &[u8], at: usize) -> bool {
    let before = source[..at.min(source.len())].trim_ascii_end();
    ["struct", "union"].iter().any(|keyword| {
        before.ends_with(keyword.as_bytes())
            && before[..before.len() - keyword.len()]
                .last()
                .is_none_or(|&b| !(b.is_ascii_alphanumeric() || b == b'_'))
    })
}

#[cfg(test)]
mod tests {
    use super::follows_aggregate_keyword;

    /// The keyword is read as a whole word, whitespace aside: a name that ends
    /// in `struct` is no keyword (S-582).
    #[test]
    fn the_aggregate_keyword_is_a_whole_word() {
        let at = |source: &str| follows_aggregate_keyword(source.as_bytes(), source.len());
        assert!(at("struct "));
        assert!(at("x;\nunion\n  "));
        assert!(at("};\tstruct"));
        assert!(!at("my_struct "));
        assert!(!at("xunion "));
        assert!(!at("struct_ "));
        assert!(!at(""));
    }
}
