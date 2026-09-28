//! Constant folding for a path written as an expression rather than a literal
//! ([FR-FW-05], S-469, [CR-151]).
//!
//! A Spring mapping path such as `value = "/users/{" + USER_ID + "}/x"` names no
//! literal the query can capture, so before this step it was dropped without a
//! trace. The query now captures the expression's **byte range** (an opaque
//! path or prefix) and the facts needed to resolve its names (`@fw.const.*`
//! scopes, fields and constants). This module turns those into one literal, or
//! into a refusal:
//!
//! - the expression is read as a `+` chain of operands, each a string literal, a
//!   simple name, or a `Type.NAME` qualified name, with parentheses grouping;
//! - a name folds only when it binds, by Java's own scoping, to exactly one
//!   compile-time `String` constant declared in an enclosing type — the same
//!   type's `static final String`, or an interface's implicit constant;
//! - anything else is `None`: a method call, a number, a non-final or non-String
//!   field, a constant of another type, a name a supertype might supply.
//!
//! # Exact or nothing ([NFR-RA-05])
//!
//! Every `None` becomes a refusal the pass counts, never a guessed path. The
//! rule can therefore only turn a formerly dropped registration into a counted
//! one or a promoted one; it can never move a route that was already promoted.
//!
//! # Why the expression is read as text
//!
//! The shared interpreter names no grammar node kind of any JVM language — the
//! `no_language_specific_composition_code_exists` test enforces it — so a
//! structural walk over `binary_expression` is not available here. The chain is
//! instead read from the captured source text, which is also what
//! [`is_resolvable_prefix`](super::is_resolvable_prefix) does for the forms no
//! query can decompose. The grammar of the text accepted is deliberately tiny:
//! a token outside it (`?`, `,`, a digit, a comment, an escape) refuses the
//! whole expression, so an unforeseen spelling fails closed.
//!
//! Because every accepted operand is a `String`, every `+` in an accepted chain
//! is concatenation and grouping cannot change the result; that is why
//! parentheses are read and then ignored.
//!
//! [FR-FW-05]: ../../../../docs/specs/requirements/FR-FW-05.md
//! [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
//! [CR-151]: ../../../../docs/requests/CR-151-provider-routes-composed-from-string-constants.md

use std::collections::{HashMap, HashSet};

use super::FoldedConstant;

/// How deep one constant's initializer may refer to another before the fold
/// gives up. Real constants nest one or two levels; the bound is what stops a
/// cyclic pair (`A = B; B = A`), because a branch that fails ends the whole
/// fold.
const MAX_DEPTH: usize = 16;

/// How many operands one fold may read, counting every expansion of every
/// constant. Depth alone does not bound the work: a *valid* tree of constants
/// each concatenating six of the level below expands 6^16 operands within the
/// depth bound, and a candidate file is untrusted input to the indexer. A real
/// mapping path reads a handful; the budget refuses the rest, counted like any
/// other refusal.
const MAX_OPERANDS: usize = 4096;

/// One `@fw.const.scope` match: a type body, as one query match described it.
/// Several matches describe one body (the named boundary, a supertype marker),
/// so they are combined by range in [`Names::new`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ScopeCapture {
    /// The body's byte range: where the type's members are in scope by simple
    /// name.
    pub(super) start: usize,
    pub(super) end: usize,
    /// The declaration's byte range, annotations included — where a qualified
    /// `Type.NAME` names this type. `None` from a match that did not capture it.
    pub(super) decl: Option<(usize, usize)>,
    /// The type's simple name, when this match captured it.
    pub(super) name: Option<String>,
    /// `true` when this scope may declare names the query does not model (see
    /// `@fw.const.scope.opaque`).
    pub(super) opaque: bool,
}

/// One field declarator a type body declares — a constant or not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FieldCapture {
    pub(super) name: String,
    /// Start byte of the declarator's name: which body declares it, and the
    /// key that pairs an `@fw.const.field` with its `@fw.const.name` twin.
    pub(super) at: usize,
    /// The initializer's byte range when the field is a compile-time `String`
    /// constant; `None` for any other field, which only shadows.
    pub(super) value: Option<(usize, usize)>,
}

/// A type body with everything it declares.
#[derive(Debug)]
struct Scope {
    start: usize,
    end: usize,
    decl: Option<(usize, usize)>,
    name: Option<String>,
    opaque: bool,
    /// Every declarator of each name in this body: `Some(value)` for a
    /// constant, `None` for any other field.
    declared: HashMap<String, Vec<Option<(usize, usize)>>>,
}

/// The name environment of one file: its type bodies, innermost-resolvable.
pub(super) struct Names<'s> {
    src: &'s str,
    rel: &'s str,
    /// Sorted by `(start, end)`; bodies nest and never partially overlap.
    scopes: Vec<Scope>,
    /// Every field name the file declares anywhere — a field named like a type
    /// obscures that type in a qualified reference (JLS §6.4.2), so the fold
    /// refuses the qualified form rather than decide it.
    field_names: HashSet<String>,
}

impl<'s> Names<'s> {
    /// Combine the captured scopes by range and file each field under the
    /// innermost body containing its name.
    pub(super) fn new(
        src: &'s str,
        rel: &'s str,
        scopes: &[ScopeCapture],
        fields: &[FieldCapture],
    ) -> Self {
        let mut by_range: HashMap<(usize, usize), Scope> = HashMap::new();
        for capture in scopes {
            let scope = by_range
                .entry((capture.start, capture.end))
                .or_insert_with(|| Scope {
                    start: capture.start,
                    end: capture.end,
                    decl: None,
                    name: None,
                    opaque: false,
                    declared: HashMap::new(),
                });
            scope.decl = scope.decl.or(capture.decl);
            if scope.name.is_none() {
                scope.name.clone_from(&capture.name);
            }
            scope.opaque |= capture.opaque;
        }
        let mut scopes: Vec<Scope> = by_range.into_values().collect();
        scopes.sort_by_key(|s| (s.start, s.end));

        // One declarator matches both the every-field pattern and, when it is a
        // constant, the constant pattern: pair them by the name's start byte so
        // the constant reading wins and the declarator is counted once.
        let mut by_at: HashMap<usize, &FieldCapture> = HashMap::new();
        for field in fields {
            by_at
                .entry(field.at)
                .and_modify(|seen| {
                    if seen.value.is_none() {
                        *seen = field;
                    }
                })
                .or_insert(field);
        }
        let mut declarators: Vec<&FieldCapture> = by_at.into_values().collect();
        declarators.sort_by_key(|f| f.at);
        let field_names = declarators.iter().map(|f| f.name.clone()).collect();
        for field in declarators {
            if let Some(index) = innermost(&scopes, field.at) {
                scopes[index]
                    .declared
                    .entry(field.name.clone())
                    .or_default()
                    .push(field.value);
            }
        }
        Names { src, rel, scopes, field_names }
    }

    /// Fold the expression at `start..end` to one literal, recording each
    /// constant it used; `None` when any operand does not fold.
    pub(super) fn fold(&self, start: usize, end: usize) -> Option<(String, Vec<FoldedConstant>)> {
        let mut used = Vec::new();
        let mut budget = MAX_OPERANDS;
        let text = self.fold_range(start, end, 0, &mut budget, &mut used)?;
        let mut seen = HashSet::new();
        used.retain(|c: &FoldedConstant| seen.insert(c.clone()));
        Some((text, used))
    }

    fn fold_range(
        &self,
        start: usize,
        end: usize,
        depth: usize,
        budget: &mut usize,
        used: &mut Vec<FoldedConstant>,
    ) -> Option<String> {
        if depth > MAX_DEPTH {
            return None;
        }
        let text = self.src.get(start..end)?;
        let operands = parse_chain(text, start)?;
        let mut folded = String::new();
        for operand in operands {
            *budget = budget.checked_sub(1)?;
            let (name, value) = match operand {
                Operand::Literal(content) => {
                    folded.push_str(content);
                    continue;
                }
                Operand::Name { name, at } => (name, self.resolve_simple(name, at)?),
                Operand::Qualified { owner, name, at } => {
                    (name, self.resolve_qualified(owner, name, at)?)
                }
            };
            // A constant whose initializer reaches itself never bottoms out:
            // the depth bound refuses it.
            let part = self.fold_range(value.0, value.1, depth + 1, budget, used)?;
            folded.push_str(&part);
            used.push(FoldedConstant {
                name: name.to_string(),
                file: self.rel.to_string(),
            });
        }
        Some(folded)
    }

    /// Bind a simple name the way Java does from byte `at`: the innermost
    /// enclosing type body that declares it wins; a body that does not declare
    /// it but may inherit it ends the search with a refusal, because the
    /// inherited field would hide any outer one.
    fn resolve_simple(&self, name: &str, at: usize) -> Option<(usize, usize)> {
        let mut enclosing: Vec<&Scope> = self
            .scopes
            .iter()
            .filter(|s| s.start <= at && at < s.end)
            .collect();
        enclosing.sort_by_key(|s| std::cmp::Reverse(s.start));
        for scope in enclosing {
            if let Some(declarators) = scope.declared.get(name) {
                return only_constant(declarators);
            }
            if scope.opaque {
                return None;
            }
        }
        None
    }

    /// Bind `Owner.NAME`: `Owner` must be a type declared once in this file
    /// whose declaration encloses `at` (the annotated type itself or a type
    /// around it), not obscured by a field of the same name, and `NAME` must be
    /// a constant its own body declares. A constant it would only inherit is
    /// refused, as is every other type — that reach is [S-470]'s.
    ///
    /// [S-470]: ../../../../docs/planning/journal.md#s-470-a-static-imported-same-member-constant-folds-measured-on-the-reference-estate
    fn resolve_qualified(&self, owner: &str, name: &str, at: usize) -> Option<(usize, usize)> {
        if self.field_names.contains(owner) {
            return None;
        }
        let mut named = self.scopes.iter().filter(|s| s.name.as_deref() == Some(owner));
        let scope = named.next()?;
        if named.next().is_some() {
            return None;
        }
        let (decl_start, decl_end) = scope.decl?;
        if !(decl_start <= at && at < decl_end) {
            return None;
        }
        only_constant(scope.declared.get(name)?)
    }
}

/// The index of the innermost scope whose body contains byte `at`.
fn innermost(scopes: &[Scope], at: usize) -> Option<usize> {
    let upper = scopes.partition_point(|s| s.start <= at);
    (0..upper).rev().find(|&i| at < scopes[i].end)
}

/// A name declared exactly once in a body, as a constant.
fn only_constant(declarators: &[Option<(usize, usize)>]) -> Option<(usize, usize)> {
    match declarators {
        [Some(value)] => Some(*value),
        _ => None,
    }
}

/// One operand of a `+` chain. Names carry their absolute start byte, which is
/// where their scope is resolved from.
#[derive(Debug, PartialEq, Eq)]
enum Operand<'t> {
    /// A string literal's content, quotes stripped.
    Literal(&'t str),
    Name { name: &'t str, at: usize },
    Qualified { owner: &'t str, name: &'t str, at: usize },
}

#[derive(Debug, PartialEq, Eq)]
enum Token<'t> {
    Literal(&'t str),
    Name(&'t str, usize),
    Dot,
    Plus,
    Open,
    Close,
}

/// Read `text` (which starts at absolute byte `base`) as a `+` chain; `None`
/// for anything outside the accepted grammar.
fn parse_chain(text: &str, base: usize) -> Option<Vec<Operand<'_>>> {
    let tokens = tokenize(text, base)?;
    let mut operands = Vec::new();
    let mut at = 0;
    chain(&tokens, &mut at, &mut operands)?;
    (at == tokens.len()).then_some(operands)
}

/// `chain := operand ('+' operand)*`
fn chain<'t>(tokens: &[Token<'t>], at: &mut usize, out: &mut Vec<Operand<'t>>) -> Option<()> {
    operand(tokens, at, out)?;
    while tokens.get(*at) == Some(&Token::Plus) {
        *at += 1;
        operand(tokens, at, out)?;
    }
    Some(())
}

/// `operand := literal | name | name '.' name | '(' chain ')'`
fn operand<'t>(tokens: &[Token<'t>], at: &mut usize, out: &mut Vec<Operand<'t>>) -> Option<()> {
    match tokens.get(*at)? {
        Token::Literal(content) => {
            *at += 1;
            out.push(Operand::Literal(content));
        }
        Token::Open => {
            *at += 1;
            chain(tokens, at, out)?;
            if tokens.get(*at) != Some(&Token::Close) {
                return None;
            }
            *at += 1;
        }
        Token::Name(first, first_at) => {
            *at += 1;
            if tokens.get(*at) == Some(&Token::Dot) {
                let Some(Token::Name(second, _)) = tokens.get(*at + 1) else {
                    return None;
                };
                *at += 2;
                // `a.b.C.X` is a fully qualified reference: not a shape this
                // fold resolves.
                if tokens.get(*at) == Some(&Token::Dot) {
                    return None;
                }
                out.push(Operand::Qualified { owner: first, name: second, at: *first_at });
            } else {
                out.push(Operand::Name { name: first, at: *first_at });
            }
        }
        Token::Dot | Token::Plus | Token::Close => return None,
    }
    Some(())
}

fn tokenize(text: &str, base: usize) -> Option<Vec<Token<'_>>> {
    let bytes = text.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        match c {
            b' ' | b'\t' | b'\r' | b'\n' => i += 1,
            b'+' => {
                tokens.push(Token::Plus);
                i += 1;
            }
            b'.' => {
                tokens.push(Token::Dot);
                i += 1;
            }
            b'(' => {
                tokens.push(Token::Open);
                i += 1;
            }
            b')' => {
                tokens.push(Token::Close);
                i += 1;
            }
            b'"' => {
                // A text block (`"""`) is not a one-line literal; its
                // indentation rules are not read here.
                if bytes.get(i + 1) == Some(&b'"') && bytes.get(i + 2) == Some(&b'"') {
                    return None;
                }
                let content_start = i + 1;
                let close = text[content_start..].find('"')? + content_start;
                let content = &text[content_start..close];
                // An escape sequence means the source form is not the value.
                if content.contains('\\') || content.contains('\n') {
                    return None;
                }
                tokens.push(Token::Literal(content));
                i = close + 1;
            }
            _ if c == b'_' || c == b'$' || c.is_ascii_alphabetic() => {
                let start = i;
                while i < bytes.len()
                    && (bytes[i] == b'_' || bytes[i] == b'$' || bytes[i].is_ascii_alphanumeric())
                {
                    i += 1;
                }
                tokens.push(Token::Name(&text[start..i], base + start));
            }
            // A digit, a char literal, an operator other than `+`, a comment,
            // a non-ASCII identifier: outside the accepted grammar.
            _ => return None,
        }
    }
    Some(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chain_of_literals_and_names_is_read_in_order() {
        let got = parse_chain(r#""/users/{" + USER_ID + "}/x""#, 100).expect("parses");
        assert_eq!(
            got,
            vec![
                Operand::Literal("/users/{"),
                Operand::Name { name: "USER_ID", at: 113 },
                Operand::Literal("}/x"),
            ]
        );
    }

    #[test]
    fn parentheses_group_and_a_qualified_name_is_one_operand() {
        let got = parse_chain(r#"(C.BASE + ("/a"))"#, 0).expect("parses");
        assert_eq!(
            got,
            vec![
                Operand::Qualified { owner: "C", name: "BASE", at: 1 },
                Operand::Literal("/a"),
            ]
        );
    }

    #[test]
    fn anything_outside_the_chain_grammar_is_refused() {
        for text in [
            r#"Paths.get()"#,          // a call
            r#""/a" + 1"#,             // a number
            r#""/a" + 'b'"#,           // a char
            r#"a.b.C.X"#,              // fully qualified
            r#"FLAG ? "/a" : "/b""#,   // a conditional
            r#""/a\tb""#,              // an escape
            r#""""x""""#,              // a text block
            r#""/a" + /* c */ B"#,     // a comment
            r#""/a" +"#,               // dangling operator
            r#"(String) B"#,           // a cast
            r#"("/a""#,                // unbalanced
            "",                        // nothing
        ] {
            assert_eq!(parse_chain(text, 0), None, "{text:?} must not parse");
        }
    }
}
