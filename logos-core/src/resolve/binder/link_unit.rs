//! The link-unit rung of a language whose files make one link unit — a C call
//! across translation units (S-584, [CR-185] §3.2 (3), [FR-RS-40]).
//!
//! A C bare call is read in two steps:
//!
//! 1. **Through includes** ([`Ctx::include_lookup`], S-579): the caller's own
//!    file first, then every file its includes reach. A definition found there
//!    decides — the caller's own function (a `static` one included), a `static
//!    inline` function or a macro a header supplies. A **prototype** found there
//!    only names the function: it is paired with its definition by step 2. (A
//!    header C reads declares no prototype node, its `symbols` query capturing
//!    definitions alone (S-582); one read as C++ — a repository with as many
//!    C++ sources as C — does, bodiless.) Two declarations at one level stop
//!    the walk unbound, as every include-scope lookup does.
//! 2. **Link unit**: failing that, the one non-`static` definition of the name
//!    among the files of the language ([`PackageLayout::links_units`]: C's
//!    `.c` files, and its `.h` ones where the repository's headers are read
//!    as C — a single-header library's implementation, xxhash's — never a
//!    header read as C++). C overloads nothing, so no argument count narrows
//!    it ([FR-RS-43]). A `static` function — a header's `static inline` one
//!    too — is its own file's alone, and two non-`static` definitions — the
//!    platform variants a build selects between, libuv's `uv_loop_init` in
//!    `src/unix/loop.c` and `src/win/core.c` — bind neither ([NFR-RA-05]).
//!
//! A call through a function pointer names no function: a name a parameter
//! or local binds at the call (the S-593 local-binding facts, by line), or a
//! variable the file or its includes declare, binds nothing at the second
//! step, nor — for a parameter or local — at the first.
//!
//! Not read: a build file (`Makefile`, CMake, `compile_commands.json`), so a
//! definition no binary links with the caller is still a candidate — the
//! exactly-one rule is what keeps a guess out ([CR-185] §7).
//!
//! A row of an include-roots language is re-bound on every non-empty sync
//! delta ([`Index::reads_include_scope`]), so a definition gained, lost or made
//! `static` in a file the caller never names moves the call on sync as on a
//! reindex.
//!
//! [CR-185]: ../../../../docs/requests/CR-185-c-resolves-includes-prototypes-headers-and-macros.md
//! [FR-RS-40]: ../../../../docs/specs/requirements/FR-RS-40.md
//! [FR-RS-43]: ../../../../docs/specs/requirements/FR-RS-43.md
//! [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
//! [`PackageLayout::links_units`]: crate::resolve::package_key::PackageLayout::links_units

use crate::model::{NodeId, NodeKind};

use super::{exactly_one, Ctx, Res};

impl Ctx<'_> {
    /// The bare call `segs` once its includes have answered `through_includes`
    /// (S-584; the module docs): that answer when the source's language makes
    /// no link unit, the call is qualified, or the includes found a definition
    /// or two declarations; else — nothing found, or a prototype — the one
    /// non-`static` definition of the name the link unit holds
    /// ([`link_unit_definition`](Ctx::link_unit_definition)).
    ///
    /// A name a parameter or local binds at the call's `line` — a
    /// function-pointer `cb(x)` — or a variable the file or its includes
    /// declare is called through, never bound: nothing.
    pub(super) fn across_link_unit(&self, segs: &[String], line: Option<i64>, through_includes: Res) -> Res {
        let [name] = segs else {
            return through_includes;
        };
        if !self.links_units() {
            return through_includes;
        }
        if self.bound_locally(name, line) {
            return Res::NotFound;
        }
        match through_includes {
            Res::Found(id) if !self.is_prototype(id) => Res::Found(id),
            Res::Ambiguous => Res::Ambiguous,
            Res::Found(_) | Res::NotFound if self.include_lookup(segs, |k| k == NodeKind::Variable) != Res::NotFound => {
                Res::NotFound
            }
            Res::Found(_) | Res::NotFound => self.link_unit_definition(name),
        }
    }

    /// Whether a parameter or local of the source's file binds `name` at
    /// `line` (S-593 facts, [`Index::with_link_unit_locals`](super::Index::with_link_unit_locals)):
    /// declared at or before it, in a scope spanning it. Read by line, so a
    /// binding later on the call's own line counts too — a call left unbound,
    /// never one bound through a pointer.
    fn bound_locally(&self, name: &str, line: Option<i64>) -> bool {
        let (Some(line), Some(path)) = (line.and_then(|l| u32::try_from(l).ok()), self.source_file()) else {
            return false;
        };
        self.ix
            .link_unit_locals
            .get(&(path.to_string(), name.to_string()))
            .is_some_and(|scopes| scopes.iter().any(|&(at, start, end)| at <= line && start <= line && line <= end))
    }

    /// Whether the source's file is of a language whose files make one link
    /// unit (S-584).
    fn links_units(&self) -> bool {
        self.source_file().is_some_and(|p| self.ix.layout.links_units(p))
    }

    /// The project-relative path of the source's file.
    fn source_file(&self) -> Option<&str> {
        self.ix.info.get(&self.source).and_then(|i| i.file_path.as_deref())
    }

    /// Whether `id` is a function with no body — a prototype a header read as
    /// C++ declares. The store records a bodiless node among the members no
    /// type inherits ([`Index::uninherited`](super::Index)), and no free
    /// function is marked uninherited on any other ground.
    fn is_prototype(&self, id: NodeId) -> bool {
        self.ix.info.get(&id).is_some_and(|i| i.kind == NodeKind::Function) && self.ix.uninherited.contains(&id)
    }

    /// The one non-`static`, bodied function `name` among the link unit's
    /// files. Nothing when the index was given no linkage facts
    /// ([`Index::with_file_local`](super::Index::with_file_local)): a `static`
    /// it cannot tell apart is never offered to another file.
    fn link_unit_definition(&self, name: &str) -> Res {
        let Some(file_local) = &self.ix.file_local else {
            return Res::NotFound;
        };
        let mut found: Vec<NodeId> = self
            .ix
            .by_name
            .get(name)
            .into_iter()
            .flatten()
            .copied()
            .filter(|id| {
                !file_local.contains(id)
                    && !self.is_prototype(*id)
                    && self.ix.info.get(id).is_some_and(|i| {
                        i.kind == NodeKind::Function
                            && i.file_path.as_deref().is_some_and(|p| self.ix.layout.links_units(p))
                    })
            })
            .collect();
        found.sort_unstable();
        found.dedup();
        exactly_one(&found)
    }
}
