//! The declaration facts one **associated-item lookup** reads (S-606, [CR-202]
//! F1 and F3, [FR-EX-34]): every impl block's header, each callable's receiver
//! mode, each enum's variant names, and which declarations are required
//! signatures.
//!
//! All of them are read off the parse tree while it is in hand, from captures a
//! plugin's own `symbols` query declares — no language is named here
//! ([NFR-MA-01]). A plugin that declares none records none of these facts, and
//! an unknown fact never filters a candidate.
//!
//! # The `symbols`-query vocabulary (the `item.` group)
//!
//! - `@item.impl` — an impl block (the whole block, so an empty one counts);
//!   its companions in the **same match**:
//!   - `@item.impl.self` — the block's self type;
//!   - `@item.impl.referent` — the type a reference self type refers to (`T`
//!     of `&T` / `&mut T`): it replaces `.self` and sets the reference flag;
//!   - `@item.impl.trait` — the trait a trait impl implements;
//!   - `@item.impl.target` — the `type Target` of an `impl Deref`.
//!
//!   Several patterns may each name one block; their companions are merged.
//! - `@item.variants` — an enum's variant list, and `@item.variant` — one
//!   variant's name. The enum declaration owning the list records the names in
//!   declaration order, an empty list as no names.
//! - `@item.receiver.<mode>` — a callable's receiver parameter, by its
//!   [`ReceiverMode`] token (`value`, `ref`, `mut`, `typed`). A parameter
//!   captured as several modes takes the strongest — `typed` > `mut` > `ref` >
//!   `value` — so a query may capture every receiver `value` and refine it.
//! - `@item.signature` — a declaration that is a required signature of its
//!   container: a member its implementors supply, with no body of its own (a
//!   Rust trait's `fn m(&self);`).
//!
//! # Type paths
//!
//! A self type, trait or `Deref` target is recorded by [`item_path`]: a path
//! keeps its segments as written with its generic arguments stripped
//! (`crate::a::A<M>` → `crate::a::A`), and anything else — a primitive, tuple,
//! slice, `str`, `dyn Tr` — is recorded as written, its whitespace collapsed.
//!
//! [CR-202]: ../../../docs/requests/CR-202-one-rust-associated-item-lookup.md
//! [FR-EX-34]: ../../../docs/specs/requirements/FR-EX-34.md
//! [NFR-MA-01]: ../../../docs/specs/requirements/NFR-MA-01.md

use std::collections::{BTreeMap, HashMap, HashSet};

use tree_sitter::Node;

use crate::model::ReceiverMode;

use super::arity::owning_decl;
use super::Decl;

/// The capture-name prefix of the vocabulary.
const GROUP: &str = "item.";

/// One impl block's header (F1): what the block implements, for whom.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImplBlockFact {
    /// 1-based first line of the block.
    pub start_line: u32,
    /// 1-based last line of the block.
    pub end_line: u32,
    /// The self type as written ([`item_path`]): `X`, `a::X`, `crate::m::X`,
    /// `std::io::Error`, `()`, `str`, `[u8]`. For a reference self type, the
    /// type it refers to.
    pub self_type: String,
    /// `true` when the self type is a reference — `&T` or `&mut T`.
    pub self_ref: bool,
    /// The trait a trait impl implements ([`item_path`]); `None` for an
    /// inherent impl.
    pub trait_path: Option<String>,
    /// The `type Target` of an `impl Deref` ([`item_path`]); `None` otherwise.
    pub deref_target: Option<String>,
}

/// The per-declaration facts of this vocabulary.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct DeclItemFacts {
    /// The mode of the receiver the declaration's parameters write, when one is
    /// captured.
    pub(super) receiver: Option<ReceiverMode>,
    /// The enum's variant names in declaration order, space-joined; `Some("")`
    /// for an enum with none.
    pub(super) variants: Option<String>,
    /// Whether the declaration is a required signature.
    pub(super) signature: bool,
}

/// The companions one impl block's matches captured.
#[derive(Default)]
struct BlockCaptures<'tree> {
    self_type: Option<Node<'tree>>,
    referent: Option<Node<'tree>>,
    trait_path: Option<Node<'tree>>,
    target: Option<Node<'tree>>,
}

/// The `item.` captures of one file, gathered during the declaration walk
/// ([`note`](Self::note)) and resolved once it ends.
#[derive(Default)]
pub(super) struct AssocCaptures<'tree> {
    /// Impl node id → (the block node, its companions).
    blocks: HashMap<usize, (Node<'tree>, BlockCaptures<'tree>)>,
    variant_lists: Vec<Node<'tree>>,
    variants: Vec<Node<'tree>>,
    receivers: Vec<(Node<'tree>, ReceiverMode)>,
    signatures: HashSet<usize>,
}

impl<'tree> AssocCaptures<'tree> {
    /// Record `capture` when it belongs to the vocabulary; `false` for any
    /// other capture, which the caller then reads itself. `companion` finds a
    /// capture of the same match by name, for an impl block's header.
    pub(super) fn note(
        &mut self,
        capture: &str,
        node: Node<'tree>,
        companion: impl Fn(&str) -> Option<Node<'tree>>,
    ) -> bool {
        let Some(name) = capture.strip_prefix(GROUP) else {
            return false;
        };
        match name {
            "impl" => {
                let (_, block) = self.blocks.entry(node.id()).or_insert_with(|| (node, BlockCaptures::default()));
                let merge = |slot: &mut Option<Node<'tree>>, role: &str| {
                    if slot.is_none() {
                        *slot = companion(role);
                    }
                };
                merge(&mut block.self_type, "item.impl.self");
                merge(&mut block.referent, "item.impl.referent");
                merge(&mut block.trait_path, "item.impl.trait");
                merge(&mut block.target, "item.impl.target");
            }
            "variants" => self.variant_lists.push(node),
            "variant" => self.variants.push(node),
            "signature" => {
                self.signatures.insert(node.id());
            }
            _ => {
                let mode = name
                    .strip_prefix("receiver.")
                    .and_then(|m| ReceiverMode::ALL.into_iter().find(|r| r.as_str() == m));
                if let Some(mode) = mode {
                    self.receivers.push((node, mode));
                }
            }
        }
        true
    }

    /// The facts of each declaration, by index into `decls`.
    pub(super) fn per_decl(&self, decls: &[Decl<'tree>], source: &[u8]) -> Vec<DeclItemFacts> {
        let mut out = vec![DeclItemFacts::default(); decls.len()];
        let decl_at: HashMap<usize, usize> = decls.iter().enumerate().map(|(i, d)| (d.node.id(), i)).collect();
        for (i, decl) in decls.iter().enumerate() {
            out[i].signature = self.signatures.contains(&decl.node.id());
        }

        // Each declaration's receiver: the strongest mode any capture gives it.
        for &(node, mode) in &self.receivers {
            if let Some(i) = owning_decl(node, &decl_at) {
                let slot = &mut out[i].receiver;
                if slot.is_none_or(|m| m.as_i32() < mode.as_i32()) {
                    *slot = Some(mode);
                }
            }
        }

        // Each enum's variant names, in declaration order.
        let list_ids: HashSet<usize> = self.variant_lists.iter().map(Node::id).collect();
        let mut names: HashMap<usize, BTreeMap<usize, String>> = HashMap::new();
        for &variant in &self.variants {
            let mut ancestor = variant.parent();
            while let Some(n) = ancestor {
                if list_ids.contains(&n.id()) {
                    break;
                }
                ancestor = n.parent();
            }
            let (Some(list), Ok(text)) = (ancestor, variant.utf8_text(source)) else {
                continue;
            };
            names.entry(list.id()).or_default().insert(variant.start_byte(), text.trim().to_string());
        }
        for &list in &self.variant_lists {
            if let Some(i) = owning_decl(list, &decl_at) {
                let joined: Vec<String> = names.get(&list.id()).map(|m| m.values().cloned().collect()).unwrap_or_default();
                out[i].variants = Some(joined.join(" "));
            }
        }
        out
    }

    /// Every captured impl block's header, in source order. A block whose self
    /// type no capture names records nothing.
    pub(super) fn impl_blocks(&self, source: &[u8]) -> Vec<ImplBlockFact> {
        let text = |n: Option<Node<'_>>| n.and_then(|n| n.utf8_text(source).ok()).map(item_path);
        let mut blocks: Vec<(usize, ImplBlockFact)> = self
            .blocks
            .values()
            .filter_map(|(node, c)| {
                let self_ref = c.referent.is_some();
                let self_type = text(c.referent.or(c.self_type)).filter(|t| !t.is_empty())?;
                Some((
                    node.start_byte(),
                    ImplBlockFact {
                        start_line: node.start_position().row as u32 + 1,
                        end_line: node.end_position().row as u32 + 1,
                        self_type,
                        self_ref,
                        trait_path: text(c.trait_path).filter(|t| !t.is_empty()),
                        deref_target: text(c.target).filter(|t| !t.is_empty()),
                    },
                ))
            })
            .collect();
        blocks.sort_by_key(|(start, _)| *start);
        blocks.into_iter().map(|(_, b)| b).collect()
    }
}

/// A type path as recorded ([module docs](self)): a path's `::` segments as
/// written, generic arguments stripped and whitespace dropped
/// (`crate :: a :: A < M >` → `crate::a::A`, `::std::io::Error` kept global);
/// anything that is not a path — `()`, `[u8]`, `&str`, `dyn Tr`, `(A, B)`, a
/// qualified `<T as A>::Out`, whose bracket heads the path rather than
/// following a segment — as written, each run of whitespace collapsed to one
/// space.
pub(crate) fn item_path(text: &str) -> String {
    let as_written = || text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.trim_start().starts_with('<') {
        return as_written();
    }
    let mut stripped = String::with_capacity(text.len());
    let mut depth = 0usize;
    let mut prev = ' ';
    for c in text.chars() {
        match c {
            '<' => depth += 1,
            // `->` inside a function type is no closing bracket.
            '>' if prev != '-' => depth = depth.saturating_sub(1),
            c if depth == 0 => stripped.push(c),
            _ => {}
        }
        prev = c;
    }
    let segments: Vec<&str> = stripped.split("::").map(str::trim).collect();
    let is_ident = |s: &str| {
        s.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_')
            && s.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '#')
    };
    let global = segments.len() > 1 && segments[0].is_empty();
    let rest = if global { &segments[1..] } else { &segments[..] };
    if depth == 0 && !rest.is_empty() && rest.iter().all(|s| is_ident(s)) {
        return segments.join("::");
    }
    as_written()
}

#[cfg(test)]
mod tests {
    use super::item_path;

    /// A path keeps its segments, its generics stripped at any depth; anything
    /// else is kept as written. Near misses of a path stay as written.
    #[test]
    fn a_type_path_strips_generics_and_anything_else_is_kept_as_written() {
        let cases = [
            ("X", "X"),
            ("a::X", "a::X"),
            ("crate::m::X<T>", "crate::m::X"),
            ("A<B<C>, D>", "A"),
            ("std :: io :: Error", "std::io::Error"),
            ("::std::io::Error", "::std::io::Error"),
            ("r#type", "r#type"),
            ("()", "()"),
            ("str", "str"),
            ("[u8]", "[u8]"),
            ("[T;  N]", "[T; N]"),
            ("(A,\n B)", "(A, B)"),
            ("dyn Tr", "dyn Tr"),
            ("dyn  Tr + Send", "dyn Tr + Send"),
            ("fn(u8) -> u8", "fn(u8) -> u8"),
            ("Box<dyn Fn() -> u8>", "Box"),
            ("1X", "1X"),
            ("a::", "a::"),
            ("Vec<u8", "Vec<u8"),
            ("<T as A>::Out", "<T as A>::Out"),
            ("< T  as A >::Out", "< T as A >::Out"),
            ("<Vec<u8>>::Item", "<Vec<u8>>::Item"),
        ];
        for (text, want) in cases {
            assert_eq!(item_path(text), want, "{text:?}");
        }
    }
}
