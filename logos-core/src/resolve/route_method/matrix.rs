//! The **shared** wildcard-method fixture matrix (S-349, [CR-109],
//! [FR-CG-09] AC3).
//!
//! [FR-CG-09] requires that "the intra-repo binder and the cross-member bridge
//! agree on every fixture case — no input binds at one site and not the other",
//! and [ADR-52] is the reason: one classifier, no drift between "why did this
//! bind" and "why didn't this bind". A matrix written twice would drift the
//! moment one copy gained a case, so it is written **once, here**, and driven by
//! all three candidate-selection sites:
//!
//! | Site | Driver |
//! |---|---|
//! | intra-repo binder ([`resolve::binder`](crate::resolve)) | `resolve::tests` |
//! | cross-member bridge ([`federation::bridge`](crate::federation)) | `bridge::tests` |
//! | coverage read-model ([`federation::coverage`](crate::federation)) | `coverage::tests` |
//!
//! Each case names the providers of **one** normalized template (the bridge and
//! coverage put each in its own member; the binder puts them all in one graph)
//! and the consumer that meets them. The binder and the bridge can only observe
//! *whether* a binding happened; the coverage read-model additionally observes
//! *why* it did not, which is what pins the ambiguous-vs-no-provider distinction
//! the acceptance criteria require to be reported rather than absorbed.
//!
//! [CR-109]: ../../../../docs/requests/CR-109-wildcard-method-route-matching.md
//! [FR-CG-09]: ../../../../docs/specs/requirements/FR-CG-09.md
//! [ADR-52]: ../../../../docs/specs/architecture/decisions/ADR-52.md

use super::WILDCARD_METHOD;

/// What every candidate-selection site must conclude for one matrix case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Expect {
    /// The consumer binds the provider at this index of [`Case::providers`].
    Binds(usize),
    /// Two or more candidates survive the ranking: no edge, reported `ambiguous`
    /// ([NFR-RA-05](../../../../docs/specs/requirements/NFR-RA-05.md)).
    Ambiguous,
    /// No provider of this endpoint anywhere: no edge, reported
    /// `no-provider-in-workspace`. This is also where a method mismatch lands —
    /// the bucket exists but holds nothing that serves the consumer.
    NoProvider,
    /// The consumer's own target does not normalize, so it never reaches
    /// candidate selection at all: no edge, reported `path-not-composed`.
    NotComposed,
}

impl Expect {
    /// The provider index this case binds, or `None` when it binds nothing — the
    /// question the intra-repo binder and the bridge can both answer.
    pub(crate) fn bound(self) -> Option<usize> {
        match self {
            Expect::Binds(i) => Some(i),
            _ => None,
        }
    }
}

/// One row of the matrix: the providers of a template, the consumer meeting
/// them, and the outcome every site must agree on.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Case {
    /// Identifies the case in an assertion message.
    pub(crate) name: &'static str,
    /// Framework `Route` provider names (`"METHOD /template"`), each hosted by
    /// its own member at the cross-member sites.
    pub(crate) providers: &'static [&'static str],
    /// The consumer's rendered target (`"METHOD /template"`) — an OpenAPI
    /// operation intra-repo, an `ApiOperation` contract node across members.
    pub(crate) consumer: &'static str,
    /// The outcome required at every site.
    pub(crate) expect: Expect,
}

/// The wildcard-method fixture matrix ([CR-109] §6).
///
/// [CR-109]: ../../../../docs/requests/CR-109-wildcard-method-route-matching.md
pub(crate) const MATRIX: &[Case] = &[
    // ── The recovery: a wildcard provider serves a concrete consumer ─────────
    Case {
        name: "a wildcard provider serves a concrete consumer",
        providers: &["ANY /v1/x"],
        consumer: "GET /v1/x",
        expect: Expect::Binds(0),
    },
    // ── The regression guard: exact-method precedence ────────────────────────
    Case {
        name: "an exact-method provider outranks a wildcard sibling",
        providers: &["ANY /v1/x", "GET /v1/x"],
        consumer: "GET /v1/x",
        expect: Expect::Binds(1),
    },
    Case {
        name: "precedence is independent of the order providers are indexed in",
        providers: &["GET /v1/x", "ANY /v1/x"],
        consumer: "GET /v1/x",
        expect: Expect::Binds(0),
    },
    Case {
        name: "an incompatible concrete provider never displaces a serving wildcard",
        providers: &["ANY /v1/x", "POST /v1/x"],
        consumer: "GET /v1/x",
        expect: Expect::Binds(0),
    },
    // ── Ranking is total on one axis only, never a general tie-break ─────────
    Case {
        name: "two wildcard providers of one template stay ambiguous",
        providers: &["ANY /v1/x/{id}", "ANY /v1/x/{userId}"],
        consumer: "GET /v1/x/{id}",
        expect: Expect::Ambiguous,
    },
    Case {
        name: "two exact-method providers stay ambiguous exactly as before",
        providers: &["GET /v1/x/{id}", "GET /v1/x/{userId}"],
        consumer: "GET /v1/x/{id}",
        expect: Expect::Ambiguous,
    },
    // ── A method mismatch still never binds ─────────────────────────────────
    Case {
        name: "a concrete method mismatch still never binds",
        providers: &["POST /v1/x"],
        consumer: "GET /v1/x",
        expect: Expect::NoProvider,
    },
    Case {
        name: "a wildcard provider of a different template is not a candidate",
        providers: &["ANY /v1/y"],
        consumer: "GET /v1/x",
        expect: Expect::NoProvider,
    },
    Case {
        name: "the wildcard widens the provider side only, never the consumer's",
        providers: &["GET /v1/x"],
        consumer: "ANY /v1/x",
        expect: Expect::NoProvider,
    },
    // ── Express `app.all` / `app.use` mounts, beyond the JVM ────────────────
    Case {
        name: "an Express app.all mount is a wildcard provider across the :id dialect",
        providers: &["ANY /widgets/:id"],
        consumer: "GET /widgets/{id}",
        expect: Expect::Binds(0),
    },
    Case {
        name: "an Express app.all mount yields to an exact-method Express route",
        providers: &["ANY /widgets/:id", "DELETE /widgets/:widgetId"],
        consumer: "DELETE /widgets/{id}",
        expect: Expect::Binds(1),
    },
    // ── Non-normalizable templates stay unresolved regardless of wildcarding ─
    Case {
        name: "a non-normalizable wildcard provider is never a candidate",
        providers: &["ANY /files/{*rest}"],
        consumer: "GET /files/{path}",
        expect: Expect::NoProvider,
    },
    Case {
        name: "a non-normalizable consumer never reaches a wildcard provider",
        providers: &["ANY /files/{path}"],
        consumer: "GET /files/{*rest}",
        expect: Expect::NotComposed,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    /// The matrix is self-consistent: every `Binds` index addresses a real
    /// provider, and every case exercising the wildcard actually uses the shared
    /// token rather than a hand-typed copy of it.
    #[test]
    fn the_matrix_is_well_formed_and_speaks_the_shared_wildcard_token() {
        assert!(
            MATRIX
                .iter()
                .any(|c| c.providers.iter().any(|p| p.starts_with(WILDCARD_METHOD))),
            "the matrix must exercise the wildcard token the extractor emits"
        );
        for case in MATRIX {
            if let Expect::Binds(i) = case.expect {
                assert!(
                    i < case.providers.len(),
                    "{}: Binds({i}) addresses no provider",
                    case.name
                );
            }
        }
    }
}
