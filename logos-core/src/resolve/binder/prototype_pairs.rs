//! A C/C++ prototype paired with its definition in another file (S-820,
//! [CR-254], [FR-EX-29], [FR-RS-38]).
//!
//! Extraction pairs a prototype with a definition of its own file
//! (`extract::definition`); a header's prototype and the definition a source
//! file writes stay two unrelated nodes there, since one file cannot read
//! another. The binder reads both: every callable of an include-roots file
//! records its **parameter shape** (`nodes.callable_signature`), and the
//! include scope ([`super::includes`]) says which file reaches which.
//!
//! **The pair.** A bodiless prototype `P` and a bodied definition `D` in
//! another file are a pair when all of these hold:
//!
//! - `D`'s file reaches `P`'s through its includes, transitively;
//! - they have one scope path and name — the namespaces and classes around
//!   each, an out-of-line member's class path included (`Repo#save()` is
//!   `Repo::save`); a callable nested in a function's body has none;
//! - both record a shape, and the shapes are equal;
//! - `D` is the only definition matching `P`, and `P` the only prototype
//!   matching `D` — two candidates are no pair ([NFR-RA-05]);
//! - `D` is not file-local: its plugin's export rule exported it (a C or C++
//!   `static`, or a definition in an anonymous namespace, is not).
//!
//! **What a pair does.** Both nodes and symbols stay ([ADR-07], [BR-74]).
//!
//! - The prototype records a `References` link to its definition — the link a
//!   same-file pair records at extraction — reconciled by the resolution pass
//!   on every run over the whole graph ([`PairLinks`]), so a full index and a
//!   sync converge on one set ([NFR-RA-06]). No ledger row records one.
//! - A call an include lookup, a qualified call or a typed receiver call lands
//!   on a paired prototype binds its definition
//!   ([`PrototypePairs::through_pair`]), as a same-file pair's prototype binds
//!   nothing.
//!   A C call's link-unit rung ([`super::link_unit`]) is then handed the
//!   definition, and its own name-only substitution serves an unpaired
//!   prototype alone.
//! - A bare-name lookup passes over a paired prototype for its definition
//!   (`navigate`, [FR-NV-15]).
//!
//! **Overloads** ([`PrototypePairs::distinct_overloads`], [CR-254] P5). At one level of
//! an include lookup, the caller's own file decides first. An included
//! prototype of the name whose known shape differs from that of every own-file
//! candidate of the name — compared by type words, qualifiers, attributes and
//! `const` placement aside, so one parameter list spelled two ways is no
//! overload — is another overload, so it joins their candidate set: the binder
//! reads no argument types, and two survivors bind neither. Where a shape is
//! unknown, and for a C caller (C overloads nothing), the own file decides
//! alone, as it did before.
//!
//! Every row of an include-roots file is re-bound on every non-empty sync
//! delta ([`Invalidation::reads_include_scope`](super::Invalidation::reads_include_scope)), so a definition, a
//! header or an include between them gained or lost moves a call as a reindex
//! does.
//!
//! [CR-254]: ../../../../docs/requests/CR-254-cpp-prototype-pairs-with-definition-across-files.md
//! [FR-EX-29]: ../../../../docs/specs/requirements/FR-EX-29.md
//! [FR-RS-38]: ../../../../docs/specs/requirements/FR-RS-38.md
//! [FR-NV-15]: ../../../../docs/specs/requirements/FR-NV-15.md
//! [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
//! [NFR-RA-06]: ../../../../docs/specs/requirements/NFR-RA-06.md
//! [ADR-07]: ../../../../docs/specs/architecture/decisions/ADR-07.md
//! [BR-74]: ../../../../docs/specs/software-spec.md#35-extraction--symbol-ids

use std::collections::{BTreeSet, HashMap, HashSet};

use anyhow::Result;

use crate::graph_store::{BatchWriter, EdgeRow, NodeItemFacts, UnresolvedRefRow};
use crate::model::{EdgeKind, NodeId, NodeKind, RefForm};

use super::{is_class_like, Index, Outcome, Res};

/// The parameter shapes of the include-roots languages' callables, and the
/// cross-file pairs they make. Its own type, never more [`Index`] methods: the
/// index only holds it.
#[derive(Debug, Default)]
pub(super) struct PrototypePairs {
    /// callable → its recorded parameter shape.
    shapes: HashMap<NodeId, String>,
    /// The bodiless callables recording a shape: the prototypes a pair link
    /// can leave from, whose `References` to a callable of their name the
    /// pass alone decides.
    prototypes: HashSet<NodeId>,
    /// prototype → the definition it pairs with, in another file.
    definition_of: HashMap<NodeId, NodeId>,
}

/// What a pair is matched on: the scope path, the name and the shape.
type PairKey = (Vec<String>, String, String);

impl PrototypePairs {
    /// `res`, its prototype replaced by the definition it pairs with: a call
    /// that lands on a paired prototype binds the definition (S-820).
    pub(super) fn through_pair(&self, res: Res) -> Res {
        match res {
            Res::Found(id) => Res::Found(self.definition_of.get(&id).copied().unwrap_or(id)),
            other => other,
        }
    }

    /// The prototypes `included` names that are other overloads of the own
    /// file's candidates `own` (the module docs, [CR-254] P5): of the name of
    /// an own candidate, every own candidate of that name recording a shape,
    /// and the prototype recording one none of them does — compared by their
    /// parameters' type words ([`overload_key`]), since a header and its source
    /// may spell one parameter list several ways (`std::filesystem::path` and
    /// `fs::path`, `Foo const&` and `const Foo&`, `unsigned` and `unsigned
    /// int`, `char buf[]` and `char*`, an attribute on one side), and a
    /// spelling is no overload. Empty whenever a shape is unknown; `included`
    /// is read only when an own candidate records one. `ix` names the
    /// candidates.
    ///
    /// [CR-254]: ../../../../docs/requests/CR-254-cpp-prototype-pairs-with-definition-across-files.md
    pub(super) fn distinct_overloads(
        &self,
        ix: &Index,
        own: &[NodeId],
        included: impl FnOnce() -> Vec<NodeId>,
    ) -> Vec<NodeId> {
        let name = |id: &NodeId| ix.info.get(id).map(|i| i.name.as_str());
        let shape = |id: &NodeId| self.shapes.get(id).map(|s| overload_key(s));
        if !own.iter().any(|o| self.shapes.contains_key(o)) {
            return Vec::new();
        }
        included()
            .iter()
            .copied()
            .filter(|p| self.prototypes.contains(p))
            .filter(|p| {
                let theirs = shape(p);
                let rivals: Vec<Option<Vec<Vec<String>>>> = own.iter().filter(|o| name(o) == name(p)).map(shape).collect();
                !rivals.is_empty() && rivals.iter().all(|s| s.is_some() && *s != theirs)
            })
            .collect()
    }

    /// The shapes and pairs of `facts` over the index `ix` (the module docs).
    pub(super) fn build(ix: &Index, facts: &[NodeItemFacts]) -> PrototypePairs {
        let mut pairs = PrototypePairs::default();
        let mut declared: HashMap<PairKey, (Vec<NodeId>, Vec<NodeId>)> = HashMap::new();
        for (id, shape) in facts.iter().filter_map(|f| Some((f.id, f.callable_signature.as_ref()?))) {
            let Some(info) = ix.info.get(&id) else { continue };
            let callable = matches!(info.kind, NodeKind::Function | NodeKind::Method);
            if !callable || !info.file_path.as_deref().is_some_and(|p| ix.layout.resolves_includes(p)) {
                continue;
            }
            pairs.shapes.insert(id, shape.clone());
            // A bodiless callable is recorded among the members no type
            // inherits; a C/C++ callable is marked so on no other ground.
            let prototype = ix.facts.uninherited.contains(&id);
            if prototype {
                pairs.prototypes.insert(id);
            }
            let Some(key) = Self::pair_scope(ix, id).map(|scope| (scope, info.name.clone(), shape.clone())) else { continue };
            let entry = declared.entry(key).or_default();
            match (prototype, &ix.facts.file_local) {
                (true, _) => entry.0.push(id),
                (false, Some(file_local)) if !file_local.contains(&id) => entry.1.push(id),
                _ => {}
            }
        }
        for (prototypes, definitions) in declared.values() {
            for &p in prototypes {
                let Some(d) = only(definitions.iter().copied().filter(|&d| Self::reaches(ix, d, p))) else { continue };
                if only(prototypes.iter().copied().filter(|&q| Self::reaches(ix, d, q))) == Some(p) {
                    pairs.definition_of.insert(p, d);
                }
            }
        }
        pairs
    }

    /// The scope path a pair is matched on: the names of the namespaces and
    /// classes around `id` in its file, outermost first, an out-of-line member's
    /// class path innermost. `None` for a callable a function's body nests, which
    /// no other file names.
    fn pair_scope(ix: &Index, id: NodeId) -> Option<Vec<String>> {
        let root = ix.file_root_of(id);
        let mut path: Vec<String> = ix.includes.qualifier(id).map(|q| q.iter().rev().cloned().collect()).unwrap_or_default();
        let mut cursor = ix.parent.get(&id).copied();
        while let Some(scope) = cursor.filter(|&s| s != root) {
            let info = ix.info.get(&scope)?;
            if !(info.kind == NodeKind::Module || is_class_like(info.kind)) {
                return None;
            }
            path.push(info.name.clone());
            cursor = ix.parent.get(&scope).copied();
        }
        path.reverse();
        Some(path)
    }

    /// Whether definition `d`'s file reaches prototype `p`'s through its includes,
    /// another file.
    fn reaches(ix: &Index, d: NodeId, p: NodeId) -> bool {
        let (from, to) = (ix.file_root_of(d), ix.file_root_of(p));
        from != to && ix.includes.reaches(from, to)
    }
}

/// `shape` with every qualifier a name is written with dropped — `ns::`,
/// `Cls::`, a rooted `::` — so `const std::filesystem::path&,` reads
/// `const path&,`.
fn unqualified(shape: &str) -> String {
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut out = String::with_capacity(shape.len());
    let mut chars = shape.chars().peekable();
    while let Some(c) = chars.next() {
        if c == ':' && chars.peek() == Some(&':') {
            chars.next();
            while out.chars().next_back().is_some_and(word) {
                out.pop();
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// What two shapes are told apart by when the overload rule reads them: per
/// parameter (and the declarator's qualifiers, last), the type words alone —
/// every qualifier ([`unqualified`]) and `[[…]]` attribute dropped, and the
/// words a spelling may add or move (`const`, `volatile`, `struct`, `class`,
/// `enum`, `union`, `typename`, `register`, and the `int` of `unsigned int`,
/// `long int`, `short int`); `*`, `&` and `[]` are no words. Two shapes of one
/// key may still be two overloads (`f(char*)` and `f(char&)`): the rule then
/// keeps today's own-file reading, never unbinding a call.
fn overload_key(shape: &str) -> Vec<Vec<String>> {
    const SPELLING: [&str; 8] = ["const", "volatile", "struct", "class", "enum", "union", "typename", "register"];
    const INT_PREFIXES: [&str; 4] = ["unsigned", "signed", "long", "short"];
    let mut bare = String::with_capacity(shape.len());
    let mut attribute = 0usize;
    let mut chars = unqualified(shape).chars().collect::<Vec<char>>().into_iter().peekable();
    while let Some(c) = chars.next() {
        match c {
            '[' if chars.peek() == Some(&'[') => {
                chars.next();
                attribute += 1;
            }
            ']' if attribute > 0 && chars.peek() == Some(&']') => {
                chars.next();
                attribute -= 1;
            }
            _ if attribute > 0 => {}
            _ => bare.push(c),
        }
    }
    let mut params: Vec<String> = vec![String::new()];
    let mut depth = 0usize;
    for c in bare.chars() {
        match c {
            '<' | '(' => depth += 1,
            '>' | ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                params.push(String::new());
                continue;
            }
            _ => {}
        }
        params.last_mut().expect("one segment").push(c);
    }
    params
        .iter()
        .map(|param| {
            let words: Vec<&str> = param
                .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .filter(|w| !w.is_empty() && !SPELLING.contains(w))
                .collect();
            let sized = words.iter().any(|w| INT_PREFIXES.contains(w));
            words.into_iter().filter(|w| !(sized && *w == "int")).map(str::to_string).collect()
        })
        .collect()
}

/// The one item of `items`, `None` for none or several.
fn only(mut items: impl Iterator<Item = NodeId>) -> Option<NodeId> {
    let first = items.next()?;
    items.next().is_none().then_some(first)
}

/// The resolution pass's verdict on the pair links for one run (the
/// `method_set::Plan` shape): the links it wants, the committed ones it
/// retracts, and the capture-before-delete rows it spends.
#[derive(Debug, Default)]
pub(crate) struct PairLinks {
    /// Every `(prototype, definition)` pair, sorted.
    desired: BTreeSet<(NodeId, NodeId)>,
    /// Every committed `References` from a prototype to a callable of its
    /// name that is not a pair — what the run deletes.
    retract: Vec<(NodeId, NodeId)>,
    /// The capture-before-delete rows of a `References` out of an owned
    /// prototype: spent without restoring anything, as a fresh index holds
    /// none.
    pub(crate) captures: HashSet<i64>,
}

impl PairLinks {
    /// The pair links this run wants and the committed ones it retracts
    /// ([`PairLinks`]), over the snapshot's `edges` and ledger `refs`.
    pub(crate) fn of(ix: &Index, edges: &[EdgeRow], refs: &[UnresolvedRefRow]) -> PairLinks {
        let prototypes = &ix.pairs.prototypes;
        let desired: BTreeSet<(NodeId, NodeId)> = ix.pairs.definition_of.iter().map(|(&p, &d)| (p, d)).collect();
        let same_named = |e: &EdgeRow| {
            let name = |id| ix.info.get(&id).map(|i| (i.name.as_str(), matches!(i.kind, NodeKind::Function | NodeKind::Method)));
            matches!((name(e.source), name(e.target)), (Some((a, true)), Some((b, true))) if a == b)
        };
        let retract = edges
            .iter()
            .filter(|e| e.kind == EdgeKind::References && prototypes.contains(&e.source) && same_named(e))
            .map(|e| (e.source, e.target))
            .filter(|link| !desired.contains(link))
            .collect();
        let captures = refs
            .iter()
            .filter(|r| r.form == RefForm::Symbol && r.kind == EdgeKind::References)
            .filter(|r| ix.by_symbol.get(&r.source_symbol).is_some_and(|id| prototypes.contains(id)))
            .map(|r| r.id)
            .collect();
        PairLinks { desired, retract, captures }
    }

    /// Turn every capture-before-delete row of an owned prototype's
    /// `References` [`Outcome::Unbound`]: the pass decides the link.
    pub(crate) fn spend_captures(&self, outcomes: &mut [(i64, bool, Outcome)]) {
        for (id, _, outcome) in outcomes {
            if self.captures.contains(id) {
                *outcome = Outcome::Unbound;
            }
        }
    }

    /// Reconcile the committed links to the verdict inside the resolution
    /// pass's write batch ([ADR-02]): delete what it retracts, insert what it
    /// wants, idempotently. The count of links inserted. Run last in the
    /// batch, so a link the pass retracted as no longer produced by any row —
    /// none produces one — is inserted again when it is still a pair.
    ///
    /// [ADR-02]: ../../../../docs/specs/architecture/decisions/ADR-02.md
    pub(crate) fn commit(&self, w: &BatchWriter<'_>) -> Result<u64> {
        crate::resolve::method_set::reconcile(w, EdgeKind::References, &self.retract, &self.desired)
    }
}

#[cfg(test)]
mod tests {
    use super::unqualified;

    #[test]
    fn an_overload_key_reads_type_words_alone() {
        use super::overload_key;
        let same = |a: &str, b: &str| overload_key(a) == overload_key(b);
        assert!(same("const Foo&,", "Foo const&,"), "an east const");
        assert!(same("unsigned,", "unsigned int,"));
        assert!(same("char*,", "char[],"));
        assert!(same("[[maybe_unused]] int,", "int,"));
        assert!(same("const std::filesystem::path&,", "const fs::path&,"));
        assert!(same("std::function<void(int,const char*)>,", "std::function<void(int,char const*)>,"));
        // What still tells two overloads apart.
        assert!(!same("int,", "double,"), "the to_text case");
        assert!(!same("int,", "int,int,"), "an arity");
        assert!(!same("unsigned,", "int,"), "a near miss: the int a sizing word makes redundant, only");
        assert!(!same("std::vector<int>,", "std::vector<long>,"));
    }

    #[test]
    fn a_shape_reads_unqualified_names() {
        assert_eq!(unqualified("const std::filesystem::path&,"), "const path&,");
        assert_eq!(unqualified("const fs::path&,core::Statistic,int64_t,"), "const path&,Statistic,int64_t,");
        assert_eq!(unqualified("std::vector<std::string>,::FILE*,const"), "vector<string>,FILE*,const");
        // A near miss: one `:` is no qualifier.
        assert_eq!(unqualified("int:3,"), "int:3,");
        assert_eq!(unqualified(""), "");
    }
}
