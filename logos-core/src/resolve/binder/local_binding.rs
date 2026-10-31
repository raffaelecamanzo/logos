//! A bare call a local binding shadows (S-594, [CR-191], [FR-RS-44]).
//!
//! Extraction marks a bare call when a local binding of its name is in force
//! at it (`extract::local_shadow`), and this rung decides the marked row before
//! every other — the lexical scope, an include scope, an import of the file —
//! so the call never falls through the binding to a member or a same-named
//! module function:
//!
//! - **a local value** ([`LOCAL_VALUE_ALIAS`]) binds nothing: it calls
//!   whatever the parameter or variable holds. Its reason is
//!   [`Residue::LocalBinding`];
//! - **a local import** ([`LOCAL_IMPORT_ALIAS`]) resolves through that import
//!   alone: a block-scope `using` through the path it names
//!   ([`Ctx::local_using_lookup`]), any other through the file's imports of the
//!   name, which bind only when every one of them names the same declaration,
//!   and no rung past them ([`Ctx::resolve_local_import_call`]).
//!
//! What the rung reads — the file's imports of the name (every one of them),
//! its `using`s and its include scope — is what [`Index::ref_affected`](super::Index::ref_affected)
//! selects a row by: its target's name, the alias chase and every expansion of
//! its head, and the include-scope clause. A local value reads nothing, so no
//! delta can change its outcome.
//!
//! [CR-191]: ../../../../docs/requests/CR-191-a-local-binding-shadows-the-receiver-and-the-callee.md
//! [FR-RS-44]: ../../../../docs/specs/requirements/FR-RS-44.md

use super::{Ctx, Outcome, Res, Residue, Want, LOCAL_IMPORT_ALIAS, LOCAL_VALUE_ALIAS, MAX_ALIAS_DEPTH};
use crate::graph_store::UnresolvedRefRow;
use crate::model::{EdgeKind, RefForm};

impl Ctx<'_> {
    /// Decide `r` when extraction marked it a bare call through a local
    /// binding; `None` for every other row.
    pub(super) fn resolve_local_binding_call(&self, r: &UnresolvedRefRow) -> Option<Outcome> {
        if r.kind != EdgeKind::Calls || r.form != RefForm::Path || r.target.contains("::") {
            return None;
        }
        let want = self.call_want();
        let resolved = match r.alias.as_deref()? {
            LOCAL_VALUE_ALIAS => {
                self.note(want, || Residue::LocalBinding);
                return Some(Outcome::Unbound);
            }
            LOCAL_IMPORT_ALIAS => self.resolve_local_import_call(&r.target, want),
            _ => return None,
        };
        Some(match resolved {
            Res::Found(target) => Outcome::Bound {
                source: self.source,
                target,
                // A class a declared call constructs (S-521).
                kind: if self.constructs(want, target) { EdgeKind::Instantiates } else { EdgeKind::Calls },
                payload: r.payload.clone(),
            },
            _ => Outcome::Unbound,
        })
    }

    /// A bare `name` a local import binds, resolved through the import alone.
    /// An include-roots file reads its `using` in force at the call
    /// ([`Ctx::local_using_lookup`]); any other reads the file's imports of the
    /// name, as the alias rung reads a file-level import — but every one of
    /// them ([`agreeing`](Ctx::agreeing)): the file's import table is not
    /// scoped, so two functions importing the name from two modules bind
    /// neither, never the first one's. No later rung is read: past its import,
    /// a bare call would reach the workspace's same-named function the import
    /// hides. Unbound, it records why: imports that disagree are
    /// `overload-ambiguous`, an import whose path reaches nothing here
    /// `external-type`, and a name no import the binder reads names
    /// `name-not-in-scope`.
    fn resolve_local_import_call(&self, name: &str, want: Want) -> Res {
        let resolved = match self.local_using_lookup(name, want) {
            Some(resolved) => resolved,
            None => match self.scope().and_then(|s| s.alias_expansions.get(name)) {
                Some(expansions) => self.agreeing(expansions, want),
                None => {
                    self.note(want, || Residue::NameNotInScope);
                    return Res::NotFound;
                }
            },
        };
        match resolved {
            Res::Ambiguous => self.note(want, || Residue::OverloadAmbiguous),
            Res::NotFound => self.note(want, || Residue::ExternalType { candidates: Vec::new() }),
            Res::Found(_) => {}
        }
        resolved
    }

    /// The one declaration every path of `expansions` names; [`Res::Ambiguous`]
    /// when two name different ones, or one names nothing here while another
    /// names something — which import is in force at the call is not the
    /// table's to say.
    fn agreeing(&self, expansions: &[Vec<String>], want: Want) -> Res {
        let mut named: Option<Res> = None;
        for path in expansions {
            let res = self.resolve_path(path, want, MAX_ALIAS_DEPTH - 1);
            match named {
                Some(seen) if seen != res => return Res::Ambiguous,
                _ => named = Some(res),
            }
        }
        named.unwrap_or(Res::NotFound)
    }
}
