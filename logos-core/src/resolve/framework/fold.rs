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
//!   compile-time `String` constant — the same type's `static final String` or
//!   an interface's implicit constant, declared in an enclosing type (S-469),
//!   or one another file of the member declares, reached through a single-type
//!   static import or a qualified `Type.NAME` whose type the file imports or
//!   shares a package with (S-470, [`Reach`]);
//! - anything else is `None`: a method call, a number, a non-final or non-String
//!   field, a name a supertype might supply, a wildcard import's name, a
//!   constant of another member or a library, a name two declarations offer.
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

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use super::FoldedConstant;

/// How many constants one chain may pass through — a path naming `A`, whose
/// initializer names `B`, … Real constants nest one or two levels; the bound
/// keeps the recursion shallow on untrusted input. (A cycle, `A = B; B = A`, is
/// refused by the in-progress set in [`Names::constant`], not by this bound.)
const MAX_DEPTH: usize = 16;

/// How many operands one fold may read, counting every expansion of every
/// constant. Depth alone does not bound the result: a *valid* tree of constants
/// each concatenating six of the level below expands 6^16 operands within the
/// depth bound, and a candidate file is untrusted input to the indexer. A real
/// mapping path reads a handful; the budget refuses the rest, counted like any
/// other refusal.
///
/// Counted, not spent: each constant is folded once per file and remembers its
/// own expansion count (see [`Names::constant`]), so the budget decides the
/// same answer however often a constant is used, at the cost of one lookup per
/// use rather than one expansion.
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

/// One import declaration (`@fw.const.import`, S-470): the path it names, split
/// into segments, and whether it is static and whether it is a wildcard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ImportCapture {
    /// `import static a.b.C.X;` → `[a, b, C, X]`; a wildcard's path stops before
    /// the `*`.
    pub(super) path: Vec<String>,
    pub(super) is_static: bool,
    pub(super) wildcard: bool,
}

/// A constant another file of the member declares, already folded in that file
/// ([`Names::member_constant`]) — what [`Reach::lookup`] answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ForeignConstant {
    pub(super) text: String,
    /// Its provenance: the constants its value was built from, then itself.
    pub(super) used: Vec<FoldedConstant>,
    /// Operands its fold read, counted into the asking fold's budget exactly
    /// as the same constant's would be were it declared in the asking file.
    pub(super) operands: usize,
}

/// Where a name this file does not declare may still come from (S-470): the
/// file's imports, its package, and a lookup of one type's constant elsewhere
/// in the member.
///
/// `lookup(type_fqn, name)` answers only for a type declared **exactly once** in
/// the member being indexed; a type of another member or of a library, and a
/// type two files declare, answer `None`. Its caller, not this module, owns how
/// a fully-qualified name becomes a file — through the package-shaped module key
/// ([`PackageLayout`](crate::resolve::package_key::PackageLayout)), the one
/// derivation there is.
pub(super) struct Reach<'r> {
    pub(super) imports: &'r [ImportCapture],
    /// The package this file declares by its location
    /// ([`PackageLayout::package_of`](crate::resolve::package_key::PackageLayout::package_of)),
    /// when its `package` declaration agrees; `None` otherwise — a
    /// non-package-shaped language, or a file whose declaration names another
    /// package — which leaves the same-package rung unavailable.
    pub(super) package: Option<Vec<String>>,
    pub(super) lookup: &'r dyn Fn(&[String], &str) -> Option<ForeignConstant>,
}

/// What an operand's name binds to.
enum Binding {
    /// A constant of this file: its initializer's byte range.
    Local((usize, usize)),
    /// A constant of another file of the member, folded there.
    Foreign(ForeignConstant),
}

/// A type body with everything it declares.
#[derive(Debug)]
struct Scope {
    start: usize,
    end: usize,
    decl: Option<(usize, usize)>,
    name: Option<String>,
    opaque: bool,
    /// The innermost body enclosing this one, as an index into
    /// [`Names::scopes`].
    parent: Option<usize>,
    /// Every declarator of each name in this body: `Some(value)` for a
    /// constant, `None` for any other field.
    declared: HashMap<String, Vec<Option<(usize, usize)>>>,
}

/// What an expression, or one constant's initializer, folds to.
#[derive(Debug)]
struct Folded {
    text: String,
    /// The constants it used, each once, in first-use order.
    used: Vec<FoldedConstant>,
    /// Operands read, every constant's expansion included.
    operands: usize,
    /// Constants on the longest chain below this point.
    height: usize,
}

/// The answer for one expression or constant.
enum Outcome {
    Folded(Rc<Folded>),
    /// Does not fold, whoever asks: memoizable.
    Refused,
    /// Cut short because the *asking* chain grew past [`MAX_DEPTH`] — true of
    /// that chain, not of the constant, so never memoized.
    TooDeep,
}

/// The name environment of one file: its type bodies, innermost-resolvable.
pub(super) struct Names<'s> {
    src: &'s str,
    rel: &'s str,
    /// Where a name the file does not declare may come from; `None` limits the
    /// fold to this file (S-469's reach).
    reach: Option<Reach<'s>>,
    /// Sorted by `(start, end)`; bodies nest and never partially overlap.
    scopes: Vec<Scope>,
    /// Scope indices by type name, for the qualified form.
    by_name: HashMap<String, Vec<usize>>,
    /// Every field name the file declares anywhere — a field named like a type
    /// obscures that type in a qualified reference (JLS §6.4.2), so the fold
    /// refuses the qualified form rather than decide it.
    field_names: HashSet<String>,
    /// Each constant's fold, keyed by its initializer's start byte. A pure
    /// function of the constant, so one entry serves every path using it.
    memo: RefCell<HashMap<usize, Option<Rc<Folded>>>>,
    /// Constants whose fold is in progress: meeting one again is a cycle.
    active: RefCell<HashSet<usize>>,
    /// Constants actually folded, as opposed to answered from the memo.
    #[cfg(test)]
    expansions: std::cell::Cell<usize>,
}

impl<'s> Names<'s> {
    /// Combine the captured scopes by range, link each to its enclosing body,
    /// and file each field under the innermost body containing its name.
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
                    parent: None,
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

        // Bodies nest, so one sweep with a stack of the open ones finds each
        // body's parent: the innermost still open when it starts.
        let mut open: Vec<usize> = Vec::new();
        for index in 0..scopes.len() {
            while open.last().is_some_and(|&top| scopes[top].end <= scopes[index].start) {
                open.pop();
            }
            scopes[index].parent = open.last().copied();
            open.push(index);
        }

        let mut by_name: HashMap<String, Vec<usize>> = HashMap::new();
        for (index, scope) in scopes.iter().enumerate() {
            if let Some(name) = &scope.name {
                by_name.entry(name.clone()).or_default().push(index);
            }
        }

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
        Names {
            src,
            rel,
            reach: None,
            scopes,
            by_name,
            field_names,
            memo: RefCell::new(HashMap::new()),
            active: RefCell::new(HashSet::new()),
            #[cfg(test)]
            expansions: std::cell::Cell::new(0),
        }
    }

    /// Let a name this file does not declare resolve through its imports and
    /// package to a constant of another file of the member (S-470).
    pub(super) fn with_reach(mut self, reach: Reach<'s>) -> Self {
        self.reach = Some(reach);
        self
    }

    /// The constant `name` that the top-level type `type_name` of this file
    /// declares, folded here — what another file of the member reaches through
    /// a static import or a qualified `Type.NAME` (S-470). `None` unless the
    /// file declares that type once at top level and its own body declares
    /// `name` once, as a constant that folds.
    ///
    /// Its caller builds these names with no [`Reach`], so a constant built
    /// from a third file's constant through *this* file's imports is refused
    /// rather than chased: a cross-file fold goes one file deep, which also
    /// makes a cycle between files impossible.
    pub(super) fn member_constant(&self, type_name: &str, name: &str) -> Option<ForeignConstant> {
        let mut top_level = self
            .by_name
            .get(type_name)?
            .iter()
            .filter(|&&index| self.scopes[index].parent.is_none());
        let (Some(&index), None) = (top_level.next(), top_level.next()) else {
            return None;
        };
        let value = only_constant(self.scopes[index].declared.get(name)?)?;
        let Outcome::Folded(folded) = self.constant(value, 1) else {
            return None;
        };
        Some(self.contribution(&folded, name))
    }

    /// What the folded constant `name` of this file contributes where it is
    /// used: its text, its provenance — the constants it was built from, then
    /// itself — and the operands its fold read. The one reading of a constant
    /// both for a use in this file and for a use from another (S-470).
    fn contribution(&self, folded: &Folded, name: &str) -> ForeignConstant {
        let mut used = folded.used.clone();
        let own = FoldedConstant {
            name: name.to_string(),
            file: self.rel.to_string(),
        };
        if !used.contains(&own) {
            used.push(own);
        }
        ForeignConstant {
            text: folded.text.clone(),
            used,
            operands: folded.operands,
        }
    }

    /// Fold the expression at `start..end` to one literal, recording each
    /// constant it used; `None` when any operand does not fold.
    pub(super) fn fold(&self, start: usize, end: usize) -> Option<(String, Vec<FoldedConstant>)> {
        match self.expression(start, end, 0) {
            Outcome::Folded(folded) => Some((folded.text.clone(), folded.used.clone())),
            Outcome::Refused | Outcome::TooDeep => None,
        }
    }

    /// Fold one expression; `chain` is how many constants the asking path has
    /// already passed through.
    fn expression(&self, start: usize, end: usize, chain: usize) -> Outcome {
        let Some(operands) = self.src.get(start..end).and_then(|text| parse_chain(text, start))
        else {
            return Outcome::Refused;
        };
        let mut folded = Folded {
            text: String::new(),
            used: Vec::new(),
            operands: 0,
            height: 0,
        };
        for operand in operands {
            folded.operands += 1;
            let (name, value) = match operand {
                Operand::Literal(content) => {
                    folded.text.push_str(content);
                    continue;
                }
                Operand::Name { name, at } => (name, self.resolve_simple(name, at)),
                Operand::Qualified { owner, name, at } => {
                    (name, self.resolve_qualified(owner, name, at))
                }
            };
            let (constant, height) = match value {
                None => return Outcome::Refused,
                Some(Binding::Local(value)) => match self.constant(value, chain + 1) {
                    Outcome::Folded(constant) => (self.contribution(&constant, name), constant.height),
                    other => return other,
                },
                // Folded in its own file, whose depth bound it met there; here
                // it is one constant deep.
                Some(Binding::Foreign(constant)) => (constant, 1),
            };
            folded.text.push_str(&constant.text);
            folded.operands += constant.operands;
            folded.height = folded.height.max(height);
            for used in constant.used {
                if !folded.used.contains(&used) {
                    folded.used.push(used);
                }
            }
            if folded.operands > MAX_OPERANDS {
                return Outcome::Refused;
            }
        }
        Outcome::Folded(Rc::new(folded))
    }

    /// Fold one constant's initializer, once per file. The result — text,
    /// provenance, expansion count, height — depends on the constant alone, so
    /// it is remembered; only a fold cut short by the asking chain's depth is
    /// not, because that says nothing about the constant.
    fn constant(&self, value: (usize, usize), chain: usize) -> Outcome {
        if let Some(known) = self.memo.borrow().get(&value.0) {
            return known.clone().map_or(Outcome::Refused, Outcome::Folded);
        }
        if chain > MAX_DEPTH {
            return Outcome::TooDeep;
        }
        // Met again while its own fold is under way: its value reaches itself.
        if !self.active.borrow_mut().insert(value.0) {
            return Outcome::Refused;
        }
        #[cfg(test)]
        self.expansions.set(self.expansions.get() + 1);
        let outcome = match self.expression(value.0, value.1, chain) {
            Outcome::Folded(init) if init.height < MAX_DEPTH => Outcome::Folded(Rc::new(Folded {
                text: init.text.clone(),
                used: init.used.clone(),
                operands: init.operands,
                height: init.height + 1,
            })),
            Outcome::Folded(_) => Outcome::Refused,
            other => other,
        };
        self.active.borrow_mut().remove(&value.0);
        match &outcome {
            Outcome::Folded(folded) => {
                self.memo.borrow_mut().insert(value.0, Some(Rc::clone(folded)));
            }
            Outcome::Refused => {
                self.memo.borrow_mut().insert(value.0, None);
            }
            Outcome::TooDeep => {}
        }
        outcome
    }

    /// Bind a simple name the way Java does from byte `at`: the innermost
    /// enclosing type body that declares it wins; a body that does not declare
    /// it but may inherit it ends the search with a refusal, because the
    /// inherited field would hide any outer one.
    ///
    /// A name no enclosing body declares — and no body on the way might
    /// inherit — is then looked up among the file's **single-type static
    /// imports** (S-470), which is where Java looks next: exactly one import
    /// naming it, of a type the member declares once. A static wildcard never
    /// supplies it, and two imports of the name from different types are two
    /// visible declarations; both refuse.
    fn resolve_simple(&self, name: &str, at: usize) -> Option<Binding> {
        let mut current = innermost(&self.scopes, at);
        while let Some(index) = current {
            let scope = &self.scopes[index];
            if let Some(declarators) = scope.declared.get(name) {
                return only_constant(declarators).map(Binding::Local);
            }
            if scope.opaque {
                return None;
            }
            current = scope.parent;
        }
        let reach = self.reach.as_ref()?;
        let owner = only_one(
            reach
                .imports
                .iter()
                .filter(|i| i.is_static && !i.wildcard && i.path.last().map(String::as_str) == Some(name))
                .map(|i| &i.path[..i.path.len() - 1]),
        )?;
        (reach.lookup)(owner, name).map(Binding::Foreign)
    }

    /// Bind `Owner.NAME`.
    ///
    /// When `Owner` is a type this file declares, it must be declared once,
    /// its declaration must enclose `at` (the annotated type itself or a type
    /// around it), and `NAME` must be a constant its own body declares; a
    /// constant it would only inherit is refused (S-469).
    ///
    /// Otherwise `Owner` is a type of another file of the member (S-470),
    /// named the way Java finds a type name: a single-type `import …Owner;`,
    /// else the file's own package. It is refused wherever `Owner` might name
    /// something else first — a field of this file (JLS §6.4.2), anything a
    /// static import brings in, or a member type an enclosing body might
    /// inherit.
    fn resolve_qualified(&self, owner: &str, name: &str, at: usize) -> Option<Binding> {
        if self.field_names.contains(owner) {
            return None;
        }
        if let Some(declared) = self.by_name.get(owner) {
            let [index] = declared.as_slice() else {
                return None;
            };
            let scope = &self.scopes[*index];
            let (decl_start, decl_end) = scope.decl?;
            if !(decl_start <= at && at < decl_end) {
                return None;
            }
            return only_constant(scope.declared.get(name)?).map(Binding::Local);
        }
        let reach = self.reach.as_ref()?;
        if self.may_inherit_around(at)
            || reach
                .imports
                .iter()
                .any(|i| i.is_static && (i.wildcard || i.path.last().map(String::as_str) == Some(owner)))
        {
            return None;
        }
        let imported = reach
            .imports
            .iter()
            .filter(|i| !i.is_static && !i.wildcard && i.path.last().map(String::as_str) == Some(owner))
            .map(|i| i.path.as_slice());
        let fqn = match only_one(imported.clone()) {
            Some(path) => path.to_vec(),
            None if imported.count() == 0 => {
                let mut fqn = reach.package.clone()?;
                fqn.push(owner.to_string());
                fqn
            }
            None => return None,
        };
        (reach.lookup)(&fqn, name).map(Binding::Foreign)
    }

    /// `true` when a body enclosing `at` may inherit members this file does not
    /// model — a supertype's member type named like an imported one would
    /// shadow the import.
    fn may_inherit_around(&self, at: usize) -> bool {
        std::iter::successors(innermost(&self.scopes, at), |&index| self.scopes[index].parent)
            .any(|index| self.scopes[index].opaque)
    }
}

/// The one distinct item `items` yields; `None` for none or for two different
/// ones. A repeated import is one import (Java admits the duplicate).
fn only_one<T: PartialEq>(mut items: impl Iterator<Item = T>) -> Option<T> {
    let first = items.next()?;
    items.all(|other| other == first).then_some(first)
}

/// The index of the innermost scope whose body contains byte `at`: the last
/// body starting at or before it, or the nearest of its parents that is still
/// open there.
fn innermost(scopes: &[Scope], at: usize) -> Option<usize> {
    let mut current = scopes.partition_point(|s| s.start <= at).checked_sub(1);
    while let Some(index) = current {
        if at < scopes[index].end {
            return Some(index);
        }
        current = scopes[index].parent;
    }
    None
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

    /// Each constant is folded once per file, however many paths use it — the
    /// work a hostile file can cause grows with its constants, not with
    /// (paths x constants x scopes). The provenance stays one entry per
    /// constant, in first-use order.
    #[test]
    fn a_constant_is_folded_once_however_many_paths_use_it() {
        let src = r#"A = "/a"; B = A + A + A + A; B + B"#;
        let at = |needle: &str| src.find(needle).expect("in fixture");
        let field = |name: &str, value: &str| FieldCapture {
            name: name.to_string(),
            at: at(&format!("{name} =")),
            value: Some((at(value), at(value) + value.len())),
        };
        let names = Names::new(
            src,
            "src/C.java",
            &[ScopeCapture { start: 0, end: src.len(), decl: None, name: None, opaque: false }],
            &[field("A", r#""/a""#), field("B", "A + A + A + A")],
        );
        let path = (at("B + B"), src.len());
        for _ in 0..100 {
            let (text, used) = names.fold(path.0, path.1).expect("folds");
            assert_eq!(text, "/a".repeat(8));
            let names: Vec<&str> = used.iter().map(|c| c.name.as_str()).collect();
            assert_eq!(names, ["A", "B"]);
        }
        assert_eq!(names.expansions.get(), 2, "A and B, once each");
    }

    // ── The reach into other files of the member (S-470) ────────────────────

    fn import(path: &str, is_static: bool, wildcard: bool) -> ImportCapture {
        ImportCapture {
            path: path.split('.').map(str::to_string).collect(),
            is_static,
            wildcard,
        }
    }

    /// The one foreign constant the fixtures' lookup knows: `a.b.G.X`.
    fn g_x() -> ForeignConstant {
        ForeignConstant {
            text: "x".to_string(),
            used: vec![FoldedConstant {
                name: "X".to_string(),
                file: "src/main/java/a/b/G.java".to_string(),
            }],
            operands: 1,
        }
    }

    /// Fold `path`, written inside the body of `class H { <body> <path> }` in
    /// package `a.c` with `imports`; the lookup answers only `a.b.G.X`.
    /// Returns the fold and every `(type.name)` the lookup was asked for.
    fn fold_with_imports(
        body: &[(&str, &str)],
        imports: &[ImportCapture],
        opaque: bool,
        path: &str,
    ) -> (Option<(String, Vec<FoldedConstant>)>, Vec<String>) {
        let mut src = String::from("class H { ");
        let mut fields = Vec::new();
        for (name, value) in body {
            fields.push(FieldCapture {
                name: name.to_string(),
                at: src.len(),
                value: Some((src.len() + name.len() + 3, src.len() + name.len() + 3 + value.len())),
            });
            src.push_str(&format!("{name} = {value}; "));
        }
        let path_at = src.len();
        src.push_str(path);
        let path_end = src.len();
        src.push_str(" }");
        let asked = RefCell::new(Vec::new());
        let lookup = |fqn: &[String], name: &str| {
            asked.borrow_mut().push(format!("{}.{name}", fqn.join(".")));
            (fqn == ["a", "b", "G"] && name == "X").then(g_x)
        };
        let names = Names::new(
            &src,
            "src/main/java/a/c/H.java",
            &[ScopeCapture { start: 0, end: src.len(), decl: None, name: Some("H".into()), opaque }],
            &fields,
        )
        .with_reach(Reach {
            imports,
            package: Some(vec!["a".into(), "c".into()]),
            lookup: &lookup,
        });
        let folded = names.fold(path_at, path_end);
        (folded, asked.into_inner())
    }

    const G_X: &str = "import static a.b.G.X";

    #[test]
    fn a_single_static_import_supplies_a_name_the_file_does_not_declare() {
        let (folded, asked) =
            fold_with_imports(&[], &[import("a.b.G.X", true, false)], false, r#""/{" + X + "}""#);
        let (text, used) = folded.expect("folds");
        assert_eq!(text, "/{x}");
        // Provenance names the declaring file, not the handler's.
        assert_eq!(used, g_x().used);
        assert_eq!(asked, ["a.b.G.X"], "{G_X}");
    }

    #[test]
    fn the_files_own_declaration_shadows_a_static_import_and_is_not_looked_up() {
        let (folded, asked) = fold_with_imports(
            &[("X", r#""own""#)],
            &[import("a.b.G.X", true, false)],
            false,
            "X",
        );
        assert_eq!(folded.map(|(t, _)| t).as_deref(), Some("own"));
        assert!(asked.is_empty(), "{asked:?}");
    }

    /// Each shape that must refuse without folding a guessed declaration.
    #[test]
    fn a_name_the_imports_do_not_prove_is_refused() {
        for (why, imports, opaque, path) in [
            ("static wildcard only", vec![import("a.b.G", true, true)], false, "X"),
            ("no import at all", vec![], false, "X"),
            (
                "two types' static imports",
                vec![import("a.b.G.X", true, false), import("a.b.K.X", true, false)],
                false,
                "X",
            ),
            ("a body that may inherit X", vec![import("a.b.G.X", true, false)], true, "X"),
            ("a non-static import of a member", vec![import("a.b.G.X", false, false)], false, "X"),
            // Qualified:
            ("G obscured by a static import", vec![import("a.b.G", false, false), import("z.Q.G", true, false)], false, "G.X"),
            ("G possibly a static-wildcard field", vec![import("a.b.G", false, false), import("z.Q", true, true)], false, "G.X"),
            ("two single-type imports of G", vec![import("a.b.G", false, false), import("z.G", false, false)], false, "G.X"),
            ("an inherited member type G", vec![import("a.b.G", false, false)], true, "G.X"),
        ] {
            let (folded, _) = fold_with_imports(&[], &imports, opaque, path);
            assert_eq!(folded, None, "{why}");
        }
    }

    #[test]
    fn a_repeated_static_import_is_one_import() {
        let imports = [import("a.b.G.X", true, false), import("a.b.G.X", true, false)];
        let (folded, _) = fold_with_imports(&[], &imports, false, "X");
        assert_eq!(folded.map(|(t, _)| t).as_deref(), Some("x"));
    }

    /// `G.X` names the single-type-imported `a.b.G`; without the import, `G`
    /// is the file's own package's `a.c.G`, which the lookup does not know.
    #[test]
    fn a_qualified_type_is_named_by_its_single_type_import_else_by_the_package() {
        let (folded, asked) = fold_with_imports(&[], &[import("a.b.G", false, false)], false, "G.X");
        assert_eq!(folded.map(|(t, _)| t).as_deref(), Some("x"));
        assert_eq!(asked, ["a.b.G.X"]);

        let (folded, asked) = fold_with_imports(&[], &[import("a.b.Other", false, false)], false, "G.X");
        assert_eq!(folded, None);
        assert_eq!(asked, ["a.c.G.X"], "the same package, not a guess");

        // A field named `G` obscures the type (JLS 6.4.2): no lookup at all.
        let (folded, asked) =
            fold_with_imports(&[("G", r#""g""#)], &[import("a.b.G", false, false)], false, "G.X");
        assert_eq!(folded, None);
        assert!(asked.is_empty(), "{asked:?}");
    }

    /// The declaring side: a top-level type's own constant, folded with the
    /// file's own constants and naming itself last in its provenance.
    #[test]
    fn a_member_constant_is_a_top_level_types_own_folded_constant() {
        let src = r#"class G { P = "e"; X = P + "mail"; class N { Y = "n"; } }"#;
        let at = |needle: &str| src.find(needle).expect("in fixture");
        let field = |name: &str, value: &str| FieldCapture {
            name: name.to_string(),
            at: at(&format!("{name} =")),
            value: Some((at(value), at(value) + value.len())),
        };
        let names = Names::new(
            src,
            "src/main/java/a/b/G.java",
            &[
                ScopeCapture { start: 0, end: src.len(), decl: None, name: Some("G".into()), opaque: false },
                ScopeCapture { start: at("class N"), end: at(" }") + 2, decl: None, name: Some("N".into()), opaque: false },
            ],
            &[field("P", r#""e""#), field("X", r#"P + "mail""#), field("Y", r#""n""#)],
        );
        let x = names.member_constant("G", "X").expect("folds");
        assert_eq!(x.text, "email");
        // `P + "mail"` reads two operands and `P`'s initializer one: what a
        // use of `X` in its own file adds to the asking fold's budget too.
        assert_eq!(x.operands, 3);
        let names_used: Vec<&str> = x.used.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names_used, ["P", "X"]);
        assert!(x.used.iter().all(|c| c.file == "src/main/java/a/b/G.java"));
        // A nested type is not a top-level type another file imports by FQN,
        // and a constant only a nested body declares is not the type's.
        assert_eq!(names.member_constant("N", "Y"), None);
        assert_eq!(names.member_constant("G", "Y"), None);
        assert_eq!(names.member_constant("Missing", "X"), None);
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
