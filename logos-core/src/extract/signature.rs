//! A callable's **signature** and a generic type's **type parameters**, read off
//! the parse tree from the captures a plugin's `symbols` query declares (S-830,
//! [CR-260] G1, [FR-RS-18]) — the reading Go's method-set pass matches a
//! method against an interface's spec by. No language is named here
//! ([NFR-MA-01]): a plugin that declares no capture records nothing, and its
//! callables keep whatever `NodeFact::callable_signature` another reading
//! gives them (C/C++'s [`super::definition::parameter_shape`], S-820).
//!
//! # The vocabulary
//!
//! - `@signature.parameters` — a callable's parameter list.
//! - `@signature.results` — its result: a single type, or a list of the
//!   parameter list's kind (`(n int, err error)`).
//! - `@signature.type_parameters` — the type parameters a declaration binds:
//!   a generic type's type-parameter list, or the type arguments a method's
//!   generic receiver names (`func (s *Set[T]) Add(v T)` binds `T`).
//!
//! A parameter or result capture belongs to the declaration that is its
//! **parent** — the list is that declaration's own field — and to no other: a
//! method spec of an interface written inside a parameter's type is no
//! declaration, and its result is never its enclosing function's. A
//! type-parameter capture belongs to its nearest enclosing declaration
//! ([`super::arity::owning_decl`]), as a receiver's sits inside its list.
//!
//! # The recorded text
//!
//! A callable records `(P)(R)`: its parameter types, then its result types,
//! each list comma-separated, without names. `Read(p []byte) (n int, err
//! error)` records `([]byte)(int,error)`, `Close() error` records `()(error)`
//! and `Log(f string, args ...any)` records `(string,...any)()` — a variadic
//! `...T` stays apart from `[]T`. A parameter declaring several names (`a, b
//! int`) is one type per name. A type is its source text with every blank
//! dropped, save one space between two words (`chan int`, `<-chan int`), and
//! with the parameter names of a function type it contains dropped as well
//! (`func(n int) error` is `func(int)error`). The type parameters a
//! declaration binds come first, by name, inside `[…]`: a generic interface
//! records `[T,U]` alone, and a method of a generic receiver `[T](T)(bool)` —
//! a `T` in its types is that parameter, never a type the package declares.
//!
//! [CR-260]: ../../../docs/requests/CR-260-go-interface-matching-by-signature-and-package.md
//! [FR-RS-18]: ../../../docs/specs/requirements/FR-RS-18.md
//! [NFR-MA-01]: ../../../docs/specs/requirements/NFR-MA-01.md

use std::collections::{HashMap, HashSet};

use tree_sitter::Node;

use super::Decl;

/// The capture-name prefix of the vocabulary.
const GROUP: &str = "signature.";

/// The signature captures of one file, gathered during the declaration walk
/// ([`note`](Self::note)) and resolved against the declarations once it ends
/// ([`per_decl`](Self::per_decl)).
#[derive(Default)]
pub(super) struct SignatureCaptures<'tree> {
    parameters: Vec<Node<'tree>>,
    results: Vec<Node<'tree>>,
    type_parameters: Vec<Node<'tree>>,
}

impl<'tree> SignatureCaptures<'tree> {
    /// Record `capture` when it belongs to the vocabulary; `false` for any
    /// other capture, which the caller then reads itself.
    pub(super) fn note(&mut self, capture: &str, node: Node<'tree>) -> bool {
        let Some(name) = capture.strip_prefix(GROUP) else {
            return false;
        };
        match name {
            "parameters" => self.parameters.push(node),
            "results" => self.results.push(node),
            "type_parameters" => self.type_parameters.push(node),
            _ => {}
        }
        true
    }

    /// The signature of each declaration, by index into `decls` (the module
    /// docs' text): `None` for a declaration owning no captured list.
    pub(super) fn per_decl(&self, decls: &[Decl<'tree>], source: &[u8]) -> Vec<Option<String>> {
        let mut out: Vec<Option<String>> = vec![None; decls.len()];
        if self.parameters.is_empty() && self.type_parameters.is_empty() {
            return out;
        }
        let decl_at: HashMap<usize, usize> = decls.iter().enumerate().map(|(i, d)| (d.node.id(), i)).collect();
        let own = |nodes: &[Node<'tree>]| -> HashMap<usize, Node<'tree>> {
            let mut by_decl = HashMap::new();
            for node in nodes {
                if let Some(&idx) = node.parent().and_then(|p| decl_at.get(&p.id())) {
                    by_decl.entry(idx).or_insert(*node);
                }
            }
            by_decl
        };
        let lists: HashSet<&str> = self.parameters.iter().map(Node::kind).collect();
        let reader = Reader { source, lists: &lists };
        let mut bound: HashMap<usize, Node<'tree>> = HashMap::new();
        for list in &self.type_parameters {
            if let Some(idx) = super::arity::owning_decl(*list, &decl_at) {
                bound.entry(idx).or_insert(*list);
            }
        }
        let results = own(&self.results);
        let params = own(&self.parameters);
        for (idx, out) in out.iter_mut().enumerate() {
            let names = bound.get(&idx).map(|list| format!("[{}]", reader.names(*list)));
            let callable = params.get(&idx).map(|list| {
                let result = results.get(&idx).map_or_else(String::new, |r| reader.result(*r));
                format!("({})({result})", reader.list(*list))
            });
            if names.is_some() || callable.is_some() {
                *out = Some(format!("{}{}", names.unwrap_or_default(), callable.unwrap_or_default()));
            }
        }
        out
    }
}

/// Renders the captured nodes of one file.
struct Reader<'s> {
    source: &'s [u8],
    /// The kinds of the captured parameter lists: a node of one of them inside
    /// a type is a function type's own parameter or result list.
    lists: &'s HashSet<&'s str>,
}

impl Reader<'_> {
    /// A result: a list of the parameter list's kind, else one type.
    fn result(&self, node: Node<'_>) -> String {
        if self.lists.contains(node.kind()) {
            self.list(node)
        } else {
            self.ty(node)
        }
    }

    /// A list's types, comma-separated: each child's `type` field once per
    /// name its `name` fields give (once when it gives none), after the tokens
    /// written between the names and the type (`...`). A child with no `type`
    /// field is a type itself.
    fn list(&self, list: Node<'_>) -> String {
        let mut types: Vec<String> = Vec::new();
        let mut cursor = list.walk();
        for child in list.named_children(&mut cursor).filter(|c| !c.is_extra()) {
            let Some(ty) = child.child_by_field_name("type") else {
                types.push(self.ty(child));
                continue;
            };
            let mut names = child.walk();
            let named = child.children_by_field_name("name", &mut names).count();
            let mut tokens = child.walk();
            let prefix: String = child
                .children(&mut tokens)
                .filter(|c| !c.is_named() && c.end_byte() <= ty.start_byte() && c.kind() != ",")
                .map(|c| self.text(c))
                .collect();
            let text = format!("{prefix}{}", self.ty(ty));
            types.extend(std::iter::repeat_n(text, named.max(1)));
        }
        types.join(",")
    }

    /// The names a type-parameter list binds, comma-separated: each child's
    /// `name` fields, or the child's own text when it has none (a receiver's
    /// type argument, `T`).
    fn names(&self, list: Node<'_>) -> String {
        let mut names: Vec<String> = Vec::new();
        let mut cursor = list.walk();
        for child in list.named_children(&mut cursor).filter(|c| !c.is_extra()) {
            let mut fields = child.walk();
            let named: Vec<String> = child.children_by_field_name("name", &mut fields).map(|n| self.text(n)).collect();
            if named.is_empty() {
                names.push(self.text(child));
            } else {
                names.extend(named);
            }
        }
        names.join(",")
    }

    /// A type's text, blanks squeezed, a nested list rendered as `(…)`.
    fn ty(&self, node: Node<'_>) -> String {
        let mut out = String::new();
        self.push_type(node, &mut out);
        out
    }

    fn push_type(&self, node: Node<'_>, out: &mut String) {
        if self.lists.contains(node.kind()) {
            push_word(out, &format!("({})", self.list(node)));
            return;
        }
        if node.child_count() == 0 {
            push_word(out, &self.text(node));
            return;
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor).filter(|c| !c.is_extra()) {
            self.push_type(child, out);
        }
    }

    fn text(&self, node: Node<'_>) -> String {
        node.utf8_text(self.source).unwrap_or_default().split_whitespace().collect::<Vec<_>>().join(" ")
    }
}

/// Append `piece`, one space before it when it and `out` meet at two word
/// characters (`chan` `int`), none otherwise (`*` `T`, `map[` `string`).
fn push_word(out: &mut String, piece: &str) {
    let word = |c: char| c.is_alphanumeric() || c == '_';
    if out.chars().next_back().is_some_and(word) && piece.chars().next().is_some_and(word) {
        out.push(' ');
    }
    out.push_str(piece);
}

#[cfg(all(test, feature = "lang-go"))]
#[path = "signature_tests.rs"]
mod tests;
