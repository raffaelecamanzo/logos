//! The include scope of an include-roots language — C++ (S-579, [CR-184]
//! §3.2 (1), [FR-RS-38]): what an `#include` binds, and what a call, a `using`
//! and a base class see through it. C reads its includes the same way (S-584),
//! and a bare C call they leave undecided goes on to its link unit
//! ([`super::link_unit`]).
//!
//! **Includes.** A header is named whole, extension included, so `hash.hpp`
//! beside `hash.cpp` is never an ambiguity. A quoted `#include "x.h"`
//! (recorded `.`-headed, `extract::refs::include_specifier_segments`) is read
//! against the including file's directory first; it and an angle-bracketed
//! `#include <a/b.h>` are then read against the repository's **include roots**
//! ([`include_roots`]): every `include/` and `src/` directory, and every
//! directory holding an `include/` one. Exactly one file across the roots binds
//! ([NFR-RA-05]); one no root holds (`<vector>`) is a system header and stays
//! external. A file's includes reach transitively, as the preprocessor pastes
//! them ([`IncludeScope::visible`]).
//!
//! **Lookup.** A Path-form call (`f()`, `ns::f()`, `Cls::sf()`), a `using` and a
//! base class are looked up the way C++ reads a name, level by level from the
//! innermost scope of the source — its enclosing classes and namespaces, an
//! out-of-line member's own class included — out to the global one, each
//! level reading the path under it ([`Ctx::include_lookup`]). At a level the
//! including file's own declarations decide first — the base lexical rule a
//! definition beside its own header's prototype relied on — then those of every
//! file its includes reach; the first level holding any decides, exactly one or
//! nothing. A rooted `::f` is read at the global level alone. A `using`
//! declaration or directive brings its names in where it is written: one in
//! the file or a namespace at that namespace's level, wherever the namespace
//! is reopened in the file; one in a function or a class inside it alone
//! ([`Ctx::usings_in_scope`]). A namespace holds functions and a class its
//! members: an out-of-line member definition (`void Cls::m() {}`, which S-577
//! keeps under its namespace) is reached only through its class's name, never
//! as a free function, and an included file's member of a class extraction
//! never captured is not reached at all ([`Index::is_stray_member`]).
//!
//! Not read: a preprocessor's macros and conditionals (an include under
//! `#ifdef` counts), `compile_commands.json`, overloads by argument type beyond
//! the argument count, ADL, and a `using` inside a header the file includes.
//!
//! [CR-184]: ../../../../docs/requests/CR-184-cpp-resolves-includes-out-of-line-members-and-relations.md
//! [FR-RS-38]: ../../../../docs/specs/requirements/FR-RS-38.md
//! [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

use crate::extract::refs::is_relative_head;
use crate::graph_store::{NodeRow, UnresolvedRefRow};
use crate::model::{EdgeKind, NodeId, NodeKind, RefForm};

use super::super::rel_path::{dir_segments, fold_path};
use super::{exactly_one, is_class_like, split, Ctx, Index, Outcome, Res, Residue, Want, FULLY_QUALIFIED_HEAD};

/// What the binder knows of the include-roots languages' files.
#[derive(Debug, Default)]
pub(super) struct IncludeScope {
    /// The include roots, project-relative (`""` is the repository root),
    /// sorted and distinct.
    roots: Vec<String>,
    /// file-root module → every other file root its `#include`s reach,
    /// transitively, id-sorted.
    visible: HashMap<NodeId, Vec<NodeId>>,
    /// out-of-line member definition → the class path its name spells
    /// (`["Manifest"]` for `core/Manifest#read().`), for a callable of an
    /// include-roots file whose container is no class.
    qualifiers: HashMap<NodeId, Vec<String>>,
    /// file-root module → the file's `using` declarations and directives, in
    /// ledger order, each with the node it is written in.
    usings: HashMap<NodeId, Vec<Using>>,
}

/// One `using` of an include-roots file (S-579): `using ns::f;` brings `f` in
/// as `ns::f`, `using namespace ns;` every name of `ns` — inside the scope it
/// is written in, its `source`, and nowhere else.
#[derive(Debug)]
struct Using {
    /// The declaration the `using` is written in: the file, a namespace, a
    /// function or a class.
    source: NodeId,
    /// The namespace a directive names, or the declaration a `using`
    /// declaration names (`["ns", "f"]`).
    path: Vec<String>,
    /// The name a declaration brings in (`f`); `None` for a directive.
    name: Option<String>,
}

/// The directory names that make an include root (S-579, [FR-RS-38]).
///
/// [FR-RS-38]: ../../../../docs/specs/requirements/FR-RS-38.md
const ROOT_DIRS: [&str; 2] = ["include", "src"];

/// The include roots of `files` (the project-relative paths of the
/// include-roots languages' files): every ancestor directory named `include`
/// or `src`, and the directory holding an `include` one — sorted, distinct.
/// Read off the indexed files alone, so a cold index and a sync over one tree
/// read the same roots.
pub(super) fn include_roots<'a>(files: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut roots: BTreeSet<String> = BTreeSet::new();
    for file in files {
        let dirs = dir_segments(file);
        for (i, dir) in dirs.iter().enumerate() {
            if !ROOT_DIRS.contains(dir) {
                continue;
            }
            roots.insert(dirs[..=i].join("/"));
            if *dir == "include" {
                roots.insert(dirs[..i].join("/"));
            }
        }
    }
    roots.into_iter().collect()
}

/// The class path an out-of-line member's `symbol` spells before its own
/// descriptor — `core/Manifest#read().` → `["Manifest"]`, `A#B#m(2).` →
/// `["A", "B"]` — or `None` when its name is qualified by no type.
fn qualifier_of(symbol: &str) -> Option<Vec<String>> {
    let callable = symbol.strip_suffix(").")?;
    let head = &callable[..callable.rfind('(')?];
    let mut rest = &head[..head.rfind(['#', '/', '.'])? + 1];
    let mut path: Vec<String> = Vec::new();
    while let Some(scope) = rest.strip_suffix('#') {
        let start = scope.rfind(['#', '/', '.', ' ']).map_or(0, |i| i + 1);
        path.push(scope[start..].trim_matches('`').to_string());
        rest = &scope[..start];
    }
    path.reverse();
    (!path.is_empty()).then_some(path)
}

impl IncludeScope {
    /// The include scope of the graph `ix` is being built from: its roots, the
    /// file each `#include` row binds, their transitive reach, and the
    /// out-of-line members' class paths. Empty for a graph holding no file of
    /// an include-roots language, which binds exactly as before.
    pub(super) fn build(nodes: &[NodeRow], refs: &[UnresolvedRefRow], ix: &Index) -> IncludeScope {
        let in_scope = |n: &NodeRow| n.file_path.as_deref().is_some_and(|p| ix.layout.resolves_includes(p));
        let files: BTreeSet<&str> = nodes.iter().filter(|n| in_scope(n)).filter_map(|n| n.file_path.as_deref()).collect();
        if files.is_empty() {
            return IncludeScope::default();
        }
        let mut scope = IncludeScope {
            roots: include_roots(files.iter().copied()),
            ..IncludeScope::default()
        };
        for n in nodes.iter().filter(|n| in_scope(n) && n.kind == NodeKind::Method) {
            let contained_by_class = ix.parent.get(&n.id).and_then(|p| ix.info.get(p)).is_some_and(|p| is_class_like(p.kind));
            if !contained_by_class {
                if let Some(path) = qualifier_of(n.symbol.as_str()) {
                    scope.qualifiers.insert(n.id, path);
                }
            }
        }
        let mut direct: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        for r in refs.iter().filter(|r| is_include_row(r)) {
            let Some(&source) = ix.by_symbol.get(&r.source_symbol) else { continue };
            let Some(file) = ix.info.get(&source).and_then(|i| i.file_path.as_deref()) else { continue };
            if !ix.layout.resolves_includes(file) {
                continue;
            }
            if let Res::Found(header) = scope.resolve_header(ix, file, &split(&r.target)) {
                direct.entry(ix.file_root_of(source)).or_default().push(header);
            }
        }
        scope.visible = direct.keys().map(|&root| (root, reach(root, &direct))).collect();
        for r in refs.iter().filter(|r| is_using_row(r)) {
            let Some(&source) = ix.by_symbol.get(&r.source_symbol) else { continue };
            if !ix.info.get(&source).and_then(|i| i.file_path.as_deref()).is_some_and(|p| ix.layout.resolves_includes(p)) {
                continue;
            }
            let name = (r.form == RefForm::Path).then(|| r.alias.clone()).flatten();
            let using = Using { source, path: split(&r.target), name };
            scope.usings.entry(ix.file_root_of(source)).or_default().push(using);
        }
        scope
    }

    /// The one file a header `segs` written in `source_file` names (S-579): a
    /// quoted one (`.`/`..`-headed) under the file's own directory first, then
    /// exactly one under the include roots.
    pub(super) fn resolve_header(&self, ix: &Index, source_file: &str, segs: &[String]) -> Res {
        let spec = segs.join("/");
        if segs.first().is_some_and(|h| is_relative_head(h)) {
            if let Some(path) = fold_path(&dir_segments(source_file), &spec) {
                match file_at_path(ix, &path) {
                    Res::NotFound => {}
                    decided => return decided,
                }
            }
        }
        let mut found: Vec<NodeId> = Vec::new();
        for root in &self.roots {
            let base: Vec<&str> = root.split('/').filter(|s| !s.is_empty()).collect();
            let Some(path) = fold_path(&base, &spec) else { continue };
            match file_at_path(ix, &path) {
                Res::Found(file) => found.push(file),
                Res::NotFound => {}
                Res::Ambiguous => return Res::Ambiguous,
            }
        }
        found.sort();
        found.dedup();
        exactly_one(&found)
    }
}

/// Whether `r` is an `#include` row: an `Imports` `Path` row naming no local
/// binding — a `using` declaration names one, a `using` directive is a `Glob`.
fn is_include_row(r: &UnresolvedRefRow) -> bool {
    r.kind == EdgeKind::Imports && r.form == RefForm::Path && r.alias.is_none()
}

/// Whether `r` is a `using` row: a directive's `Glob`, or a declaration's
/// `Path` row naming the local binding it brings in.
fn is_using_row(r: &UnresolvedRefRow) -> bool {
    r.kind == EdgeKind::Imports && (r.form == RefForm::Glob || (r.form == RefForm::Path && r.alias.is_some()))
}

/// The file root of the include-roots file at the project-relative `path`,
/// matched whole — extension included.
fn file_at_path(ix: &Index, path: &str) -> Res {
    if !ix.layout.resolves_includes(path) {
        return Res::NotFound;
    }
    ix.file_root_at(path)
}

/// Every file root `root`'s includes reach through `direct`, itself excluded,
/// id-sorted — breadth-first, each file once, so an include cycle ends.
fn reach(root: NodeId, direct: &HashMap<NodeId, Vec<NodeId>>) -> Vec<NodeId> {
    let mut seen: HashSet<NodeId> = HashSet::from([root]);
    let mut queue: VecDeque<NodeId> = VecDeque::from([root]);
    while let Some(file) = queue.pop_front() {
        for &next in direct.get(&file).into_iter().flatten() {
            if seen.insert(next) {
                queue.push_back(next);
            }
        }
    }
    seen.remove(&root);
    let mut out: Vec<NodeId> = seen.into_iter().collect();
    out.sort_unstable();
    out
}

impl Ctx<'_> {
    /// Whether the source's file is of an include-roots language (S-579).
    fn reads_includes(&self) -> bool {
        self.ix
            .info
            .get(&self.source)
            .and_then(|i| i.file_path.as_deref())
            .is_some_and(|p| self.ix.layout.resolves_includes(p))
    }

    /// Bind a row of an include-roots language that its include scope decides
    /// (S-579, [FR-RS-38]), or `None` for one it does not — a receiver call,
    /// an access, a capture-before-delete row — or a file of another language:
    ///
    /// - an `#include` binds the one file it names ([`IncludeScope::resolve_header`]);
    /// - a `using` declaration the one declaration it names, a `using`
    ///   directive the one namespace — never several reopenings of it;
    /// - a Path-form call the one callable [`include_lookup`](Ctx::include_lookup) reaches;
    /// - a type relation this arm sees is not a class's own (a base clause
    ///   error recovery left outside its class): it binds nothing.
    ///
    /// [FR-RS-38]: ../../../../docs/specs/requirements/FR-RS-38.md
    pub(super) fn resolve_include_row(&self, r: &UnresolvedRefRow) -> Option<Outcome> {
        if r.form == RefForm::Symbol || !self.reads_includes() {
            return None;
        }
        let resolved = match (r.kind, r.form) {
            (EdgeKind::Imports, RefForm::Path) if r.alias.is_none() => {
                let source_file = self.ix.info.get(&self.source)?.file_path.as_deref()?;
                self.ix.includes.resolve_header(self.ix, source_file, &split(&r.target))
            }
            (EdgeKind::Imports, RefForm::Glob) => self.include_lookup(&split(&r.target), |k| k == NodeKind::Module),
            (EdgeKind::Imports, RefForm::Path) => self.include_lookup(&split(&r.target), |_| true),
            (EdgeKind::Calls, RefForm::Path) => {
                let want = self.call_want();
                let segs = split(&r.target);
                // A bare call prefers a free function to a same-named method
                // (CR-068 Part B), as its scope walk did before (S-579).
                self.bare_path_call.set(segs.len() == 1);
                let (res, level) = self.include_lookup_at(&segs, |k| want.admits(k));
                // A C call its includes leave undecided reads its link unit (S-584).
                let res = self.across_link_unit(&segs, r.line, res);
                self.bare_path_call.set(false);
                self.class_member(&segs, res, level)
            }
            (EdgeKind::Extends | EdgeKind::Implements | EdgeKind::TypeUses | EdgeKind::Instantiates, _) => {
                return Some(Outcome::Unbound)
            }
            _ => return None,
        };
        Some(match resolved {
            Res::Found(target) => Outcome::Bound {
                source: self.source,
                target,
                kind: r.kind,
                payload: r.payload.clone(),
            },
            _ => Outcome::Unbound,
        })
    }

    /// A qualified call `T::m`, read as C++ reads a qualified name in a class's
    /// scope (S-581, [FR-RS-39]): `T` first — the one class-like declaration
    /// [`include_lookup`](Ctx::include_lookup) reaches — then `m` among its
    /// members and, failing them, its nearest proven base's
    /// ([`type_member`](Ctx::type_member)). The call a typed receiver records
    /// (`s.ship()` on a `SpecialOrder s`) is such a call, and so is a written
    /// `Derived::m()`. `level` is where the lookup of the whole path decided:
    /// an answer at `T`'s own level or nearer stands, and so does every other
    /// outcome but the two `T` hides — nothing found, or a same-named `T::m`
    /// an outer scope declares (`::Order::ship` when the call's namespace
    /// declares an `Order` that inherits `ship`). A bare name, a qualifier
    /// naming a namespace or nothing, and one naming two declarations keep the
    /// lookup's answer.
    ///
    /// [FR-RS-39]: ../../../../docs/specs/requirements/FR-RS-39.md
    fn class_member(&self, segs: &[String], res: Res, level: Option<usize>) -> Res {
        let Some((name, qualifier)) = segs.split_last().filter(|(_, q)| !q.is_empty()) else {
            return res;
        };
        let (Res::Found(ty), Some(ty_level)) = self.include_lookup_at(qualifier, is_class_like) else {
            return res;
        };
        if res != Res::NotFound && level >= Some(ty_level) {
            return res;
        }
        self.type_member(ty, name)
    }

    /// A class's type relation of an include-roots language (S-579), read from
    /// the declaration's enclosing scope through the include scope; `None` for
    /// a file of another language. A base `Want::Class` admits a `struct`, the
    /// same declaration under another default access.
    pub(super) fn resolve_included_type(&self, segs: &[String], want: Want) -> Option<Res> {
        if !self.reads_includes() {
            return None;
        }
        let enclosing = *self.ix.parent.get(&self.source)?;
        self.lexical_start.set(enclosing);
        let res = self.include_lookup(segs, |k| want.admits(k) || (want == Want::Class && k == NodeKind::Struct));
        self.lexical_start.set(self.source);
        Some(res)
    }

    /// Look `segs` up as C++ reads a name (S-579, [FR-RS-38]; the module
    /// docs): a bare name first among what the source's own callables nest
    /// ([`nested_name`](Ctx::nested_name)), then level by level from the
    /// innermost scope of [`lexical_start`](Ctx::lexical_start) out to the
    /// global one, at each the source file's own declarations of `segs` under
    /// the level first, then those of every file its includes reach, each
    /// level adding the names of the `using`s in force there. The first set holding a
    /// callable the call's arguments admit ([`applicable`](Ctx::applicable)) —
    /// or any declaration `admit` takes — decides, exactly one or nothing
    /// ([NFR-RA-05]).
    ///
    /// A row the source file attributes to a namespace or to the file itself
    /// sits in a body extraction did not capture — a destructor, an operator,
    /// a macro's — which may be a member's: there the file's own out-of-line
    /// members are names of their namespace too, as its scope walk read them
    /// before includes were ([`Index::declared_under`]).
    ///
    /// [FR-RS-38]: ../../../../docs/specs/requirements/FR-RS-38.md
    /// [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
    pub(super) fn include_lookup(&self, segs: &[String], admit: impl Fn(NodeKind) -> bool) -> Res {
        self.include_lookup_at(segs, admit).0
    }

    /// [`include_lookup`](Ctx::include_lookup), with the level that decided:
    /// the depth of its scope chain (`0` the global level), `usize::MAX` for a
    /// name the source's own callables nest, and `None` when nothing did. Two
    /// lookups from one source compare by it — a deeper level is a nearer
    /// scope, whose declaration hides an outer one of the name
    /// ([`class_member`](Ctx::class_member), S-581).
    fn include_lookup_at(&self, segs: &[String], admit: impl Fn(NodeKind) -> bool) -> (Res, Option<usize>) {
        let start = self.lexical_start.get();
        let own = self.ix.file_root_of(start);
        // A rooted `::f` / `::Base` (headed `FULLY_QUALIFIED_HEAD`) is read at
        // the global level alone.
        let (rooted, segs) = match segs.split_first() {
            Some((head, rest)) if head == FULLY_QUALIFIED_HEAD => (true, rest),
            _ => (false, segs),
        };
        if segs.is_empty() {
            return (Res::NotFound, None);
        }
        if let ([name], false) = (segs, rooted) {
            if let Some(res) = self.nested_name(start, own, name, &admit) {
                return (res, Some(usize::MAX));
            }
        }
        let included = self.ix.includes.visible.get(&own).map_or(&[][..], Vec::as_slice);
        let chain = if rooted { Vec::new() } else { self.scope_chain(start, own) };
        let uncaptured_body = self.ix.info.get(&start).is_some_and(|i| i.kind == NodeKind::Module);
        let usings = self.usings_in_scope(start, own, &chain, rooted);
        for depth in (0..=chain.len()).rev() {
            let paths = self.level_paths(&chain[..depth], segs, &usings);
            for (files, own_pass) in [(std::slice::from_ref(&own), true), (included, false)] {
                let found = self.declared_in(files, &paths, &admit, own_pass && uncaptured_body, own_pass);
                if !found.is_empty() {
                    return (exactly_one(&found), Some(depth));
                }
            }
        }
        (Res::NotFound, None)
    }

    /// A bare `name` declared under the source's callables — a declaration a
    /// function's body nests — innermost first, up to its first enclosing
    /// namespace or class: the lexical scopes [`scope_chain`](Ctx::scope_chain)
    /// does not name. `None` when none declares it.
    fn nested_name(&self, start: NodeId, own: NodeId, name: &str, admit: &impl Fn(NodeKind) -> bool) -> Option<Res> {
        let mut cursor = Some(start);
        while let Some(scope) = cursor.filter(|&id| id != own && !self.ix.info.get(&id).is_some_and(|i| is_scope(i.kind))) {
            let found = self.admitted(self.ix.members_named(scope, name, Want::Any), admit);
            if !found.is_empty() {
                return Some(exactly_one(&found));
            }
            cursor = self.ix.parent.get(&scope).copied();
        }
        None
    }

    /// The declarations `paths` name under each of `files`, as one candidate
    /// set ([`admitted`](Ctx::admitted)): out-of-line members as names of their
    /// namespace when `members`, and a member of a class extraction never
    /// captured ([`Index::is_stray_member`]) only when the files are the
    /// source's own.
    fn declared_in(
        &self,
        files: &[NodeId],
        paths: &[Vec<String>],
        admit: &impl Fn(NodeKind) -> bool,
        members: bool,
        own: bool,
    ) -> Vec<NodeId> {
        let found: Vec<NodeId> = files
            .iter()
            .flat_map(|&file| paths.iter().flat_map(move |path| self.ix.declared_under(file, path, members)))
            .filter(|&id| own || !self.ix.is_stray_member(id))
            .collect();
        self.admitted(found, admit)
    }

    /// `found` as a candidate set: of a kind `admit` takes, never the
    /// declaration whose header is being read nor one a repository adds to
    /// `std` ([`Index::in_std`]), id-sorted and distinct, then
    /// the callables the call's arguments admit and, for a bare call, a free
    /// function over a same-named method ([`prefer_free_functions`](Ctx::prefer_free_functions)).
    fn admitted(&self, mut found: Vec<NodeId>, admit: &impl Fn(NodeKind) -> bool) -> Vec<NodeId> {
        found.retain(|&id| {
            self.ix.info.get(&id).is_some_and(|i| admit(i.kind)) && !self.is_own_header(id) && !self.ix.in_std(id)
        });
        found.sort_unstable();
        found.dedup();
        self.prefer_free_functions(self.applicable(found))
    }

    /// The paths `segs` names at one level `scope` of the lookup: under the
    /// level itself, and through each `using` in force at that level
    /// ([`usings_in_scope`](Ctx::usings_in_scope)) — a directive's namespace
    /// (`using namespace util;` → `util::segs`), and every declaration naming
    /// the head (`using util::Args;` → `util::Args::rest`), so two that name
    /// it make two candidates.
    fn level_paths(&self, scope: &[String], segs: &[String], usings: &[(usize, &Using)]) -> Vec<Vec<String>> {
        let mut paths = vec![[scope, segs].concat()];
        for (_, using) in usings.iter().filter(|(level, _)| *level == scope.len()) {
            match &using.name {
                None => paths.push([using.path.as_slice(), segs].concat()),
                Some(name) if *name == segs[0] => paths.push([using.path.as_slice(), &segs[1..]].concat()),
                Some(_) => {}
            }
        }
        paths
    }

    /// A bare `name` a block-scope `using ns::f;` brings in (S-594,
    /// [FR-RS-44]): read through that declaration alone — the path each
    /// `using` of the name in force at the innermost level it is read at names,
    /// looked up from the source as a written `ns::f` is, exactly one across
    /// them — never as the bare name, whose levels would reach the caller
    /// itself or a same-named function the `using` hides. `None` for a file of
    /// another language.
    ///
    /// [FR-RS-44]: ../../../../docs/specs/requirements/FR-RS-44.md
    pub(super) fn local_using_lookup(&self, name: &str, want: Want) -> Option<Res> {
        if !self.reads_includes() {
            return None;
        }
        let start = self.lexical_start.get();
        let own = self.ix.file_root_of(start);
        let chain = self.scope_chain(start, own);
        let naming: Vec<(usize, &Using)> = self
            .usings_in_scope(start, own, &chain, false)
            .into_iter()
            .filter(|(_, u)| u.name.as_deref() == Some(name))
            .collect();
        let Some(innermost) = naming.iter().map(|(level, _)| *level).max() else {
            self.note(want, || Residue::NameNotInScope);
            return Some(Res::NotFound);
        };
        let mut found: Vec<NodeId> = Vec::new();
        for (_, using) in naming.iter().filter(|(level, _)| *level == innermost) {
            match self.include_lookup(&using.path, |k| want.admits(k)) {
                Res::Found(id) => found.push(id),
                Res::Ambiguous => return Some(Res::Ambiguous),
                Res::NotFound => {}
            }
        }
        found.sort_unstable();
        found.dedup();
        Some(exactly_one(&found))
    }

    /// The `using`s of the file (root `own`) in force at `start` (its scope
    /// names `chain`), each with the lookup level it is read at. One written in
    /// the file or a namespace is in force wherever that namespace is, a
    /// reopening of it included — its namespace path a prefix of `chain` — and
    /// is read at that namespace's level, the file's being the global one. One
    /// written in a function or a class is in force inside that declaration
    /// alone, at the innermost level. A rooted name reads only the global
    /// level's.
    fn usings_in_scope(&self, start: NodeId, own: NodeId, chain: &[String], rooted: bool) -> Vec<(usize, &Using)> {
        let Some(all) = self.ix.includes.usings.get(&own) else {
            return Vec::new();
        };
        let mut enclosing: HashSet<NodeId> = HashSet::new();
        let mut cursor = Some(start);
        while let Some(id) = cursor {
            enclosing.insert(id);
            cursor = self.ix.parent.get(&id).copied().filter(|_| id != own);
        }
        all.iter()
            .filter_map(|u| {
                if self.ix.info.get(&u.source).is_some_and(|i| i.kind == NodeKind::Module) {
                    let path = self.scope_chain(u.source, own);
                    let in_force = chain.starts_with(&path) && (!rooted || path.is_empty());
                    in_force.then_some((path.len(), u))
                } else {
                    (!rooted && enclosing.contains(&u.source)).then_some((chain.len(), u))
                }
            })
            .collect()
    }

    /// The names of the scopes enclosing `start` in its file (`own` the file's
    /// root), outermost first — its namespaces and classes, `start` itself
    /// when it is one, and an out-of-line member's class path
    /// ([`IncludeScope::qualifiers`]) inside its namespace.
    fn scope_chain(&self, start: NodeId, own: NodeId) -> Vec<String> {
        let mut chain: Vec<String> = Vec::new();
        let mut cursor = Some(start);
        while let Some(id) = cursor.filter(|&id| id != own) {
            if let Some(path) = self.ix.includes.qualifiers.get(&id) {
                chain.extend(path.iter().rev().cloned());
            }
            if let Some(info) = self.ix.info.get(&id).filter(|i| is_scope(i.kind)) {
                chain.push(info.name.clone());
            }
            cursor = self.ix.parent.get(&id).copied();
        }
        chain.reverse();
        chain
    }
}

/// Whether a node of `kind` is a scope a C++ name is read under: a namespace
/// or a class.
///
/// (A member whose class extraction did not capture — a class template's
/// specialization `struct formatter<std::byte, Char>`, a class a macro hides —
/// sits under the namespace instead: [`Index::is_stray_member`].)
fn is_scope(kind: NodeKind) -> bool {
    kind == NodeKind::Module || is_class_like(kind)
}

impl Index {
    /// Whether `r` binds through the include scope (S-579,
    /// [`Index::ref_affected`]): any row of an include-roots file but a
    /// capture-before-delete one. An `#include`, a `using`, a Path-form call
    /// and a base read the include roots, the files the file's includes
    /// reach, and theirs; a `this->` or in-class call and a member access read
    /// the supertype hierarchy those bases make — none of which the row's
    /// target spells. So it is re-bound on every non-empty delta, the stance
    /// the path-specifier fallback takes for an import.
    pub(super) fn reads_include_scope(&self, r: &UnresolvedRefRow) -> bool {
        r.form != RefForm::Symbol
            && self
                .by_symbol
                .get(&r.source_symbol)
                .and_then(|id| self.info.get(id))
                .and_then(|i| i.file_path.as_deref())
                .is_some_and(|p| self.layout.resolves_includes(p))
    }

    /// Whether `id` is a method with no class around it: one a namespace holds
    /// whose name spells no class path — the member of a class extraction did
    /// not capture (a template specialization, whose name is no plain
    /// identifier; a class a macro hides). A name looked up from another file
    /// never reaches a class member that way, so the included files offer none
    /// ([`Ctx::include_lookup`]); the including file's own pass still does, as
    /// its lexical scope chain did before includes were read.
    fn is_stray_member(&self, id: NodeId) -> bool {
        self.info.get(&id).is_some_and(|i| i.kind == NodeKind::Method)
            && !self.includes.qualifiers.contains_key(&id)
            && self
                .parent
                .get(&id)
                .and_then(|p| self.info.get(p))
                .is_some_and(|p| p.kind == NodeKind::Module)
    }

    /// Whether `id` is the top-level namespace `std`, or declared in it (S-579
    /// review): the standard reserves it to the implementation, so what a
    /// repository declares there — nlohmann's `namespace std { void
    /// swap(basic_json&, basic_json&) }` — specializes or overloads a standard
    /// entity whose overload set the repository never holds whole. A
    /// `std::swap(a, b)` names that set, not the one overload a file adds, and
    /// `using namespace std;` the standard's namespace, not a file's block of it.
    fn in_std(&self, id: NodeId) -> bool {
        let mut cursor = Some(id);
        while let Some(scope) = cursor {
            let parent = self.parent.get(&scope).copied();
            let top_level = parent.is_some_and(|p| self.file_root_of(p) == p && self.file_root_of(scope) == p);
            if top_level && self.info.get(&scope).is_some_and(|i| i.kind == NodeKind::Module && i.name == "std") {
                return true;
            }
            cursor = parent;
        }
        false
    }

    /// The declarations `path` names under `scope` (S-579): down its
    /// namespaces and classes by every segment but the last, then the members
    /// of the last name. A namespace holds functions: the out-of-line member
    /// definitions it holds (`void Cls::m() {}`) are reached through the class
    /// path their name spells, and as one of its own names only when
    /// `members` ([`Ctx::include_lookup`]'s uncaptured body).
    fn declared_under(&self, scope: NodeId, path: &[String], members: bool) -> Vec<NodeId> {
        let Some((name, heads)) = path.split_last() else {
            return Vec::new();
        };
        let namespace = self.info.get(&scope).is_some_and(|i| i.kind == NodeKind::Module);
        let out_of_line = |id: &NodeId| self.includes.qualifiers.get(id);
        let mut found: Vec<NodeId> = Vec::new();
        if namespace {
            found.extend(
                self.members_named(scope, name, Want::Any)
                    .into_iter()
                    .filter(|id| match out_of_line(id) {
                        Some(class) => class.as_slice() == heads || (members && heads.is_empty()),
                        None => heads.is_empty(),
                    }),
            );
        } else if heads.is_empty() {
            found.extend(self.members_named(scope, name, Want::Any));
        }
        if let Some((head, rest)) = heads.split_first() {
            for inner in self.members_named(scope, head, Want::Any) {
                if self.info.get(&inner).is_some_and(|i| is_scope(i.kind)) {
                    found.extend(self.declared_under(inner, &[rest, std::slice::from_ref(name)].concat(), members));
                }
            }
        }
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn include_roots_are_include_and_src_dirs_and_what_holds_an_include_dir() {
        let roots = include_roots([
            "src/ccache/hash.hpp",
            "src/ccache/core/manifest.cpp",
            "include/nlohmann/json.hpp",
            "third_party/fmt/include/fmt/core.h",
            "tests/src/unit.cpp",
            "tools/gen/main.cpp",
            "main.cpp",
        ]);
        assert_eq!(
            roots,
            ["", "include", "src", "tests/src", "third_party/fmt", "third_party/fmt/include"]
        );
        // A near miss names no root: `includes/`, `source/`, `my_src/`.
        assert!(include_roots(["includes/a.h", "source/b.cpp", "my_src/c.cpp"]).is_empty());
    }

    #[test]
    fn an_out_of_line_member_spells_its_class_path() {
        let q = |s: &str| qualifier_of(s);
        assert_eq!(q("logos . r . src/`manifest.cpp`/core/Manifest#read()."), Some(vec!["Manifest".to_string()]));
        assert_eq!(q("logos . r . `a.cpp`/A#B#m(2)."), Some(vec!["A".to_string(), "B".to_string()]));
        // A free function, or a namespace member, spells none.
        assert_eq!(q("logos . r . src/`a.cpp`/util/parse()."), None);
        assert_eq!(q("logos . r . `a.cpp`/run()."), None);
        // A type is no callable.
        assert_eq!(q("logos . r . `a.cpp`/A#"), None);
    }
}
