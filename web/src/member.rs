//! The per-request **member scope** ([FR-UI-29], [FR-WS-06], [ADR-52]) — the
//! server half of the workspace UI's member/service selector (S-250).
//!
//! The SPA's selector scopes *every existing view* to the chosen member. It does
//! that by riding one optional query param, `?repo=<member>`, on the ordinary
//! `/api/v1/*` read-model endpoints — the same `?repo=` vocabulary the
//! `/api/v1/workspace/*` fan-out already speaks (S-249). This extractor is where
//! that param becomes an [`Engine`]:
//!
//! - **Single-root** ([`Backing::Single`]) — the one engine, always. A `?repo=`
//!   is inert there, exactly as any unrecognised query param has always been, so
//!   the single-root request path is **byte-for-byte** what it was ([ADR-52]).
//! - **Workspace, unscoped** — the warmed default member (what every shared
//!   `/api/v1/*` handler already answered from under a federated backing).
//! - **Workspace, `?repo=<member>`** — that member's engine, resolved through
//!   [`EngineRegistry::engine_for`] (lazily started + cached, [NFR-PE-10]).
//! - **Workspace, unknown member** — an honest `404`: the workspace has no such
//!   member, and no view is served a *different* member's figures under the
//!   member the user selected ([NFR-RA-05]).
//!
//! Member resolution can start an engine, so it runs on the blocking pool via the
//! same `spawn_blocking` hop every read-model crosses ([ADR-03]) — the serve
//! loop is never blocked on a cold member's first request.
//!
//! [FR-UI-29]: ../../docs/specs/requirements/FR-UI-29.md
//! [FR-WS-06]: ../../docs/specs/requirements/FR-WS-06.md
//! [NFR-PE-10]: ../../docs/specs/requirements/NFR-PE-10.md
//! [NFR-RA-05]: ../../docs/specs/requirements/NFR-RA-05.md
//! [ADR-03]: ../../docs/specs/architecture/decisions/ADR-03.md
//! [ADR-52]: ../../docs/specs/architecture/decisions/ADR-52.md

use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    async_trait,
    extract::{rejection::QueryRejection, FromRef, FromRequestParts, Query},
    http::{request::Parts, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use logos_core::federation::Backing;
use logos_core::Engine;

use crate::api_v1::ApiError;

/// The query param the SPA's member selector rides on — shared verbatim with the
/// `/api/v1/workspace/*` fan-out's own `?repo=` scoping (S-249).
pub(crate) const REPO_PARAM: &str = "repo";

/// The [`Engine`] a request is scoped to: the single-root engine, the workspace's
/// default member, or the `?repo=`-selected member (see the module docs).
///
/// Extracted in place of `State<Arc<Engine>>` by every `/api/v1/*` read handler,
/// so a handler body is unchanged — it still just holds an `Arc<Engine>`.
pub(crate) struct MemberEngine(pub(crate) Arc<Engine>);

/// An honest error body at `status` ([NFR-RA-05]).
fn refuse(status: StatusCode, error: String) -> Response {
    (status, Json(ApiError { error })).into_response()
}

/// The `404` for a `?repo=` naming a member this workspace does not have — the
/// selector's counterpart to the fan-out's "not a workspace" `404`.
///
/// Reserved for **genuine absence**. A member that *is* in the manifest but whose
/// engine cannot start is a `500` ([`member_unavailable`]) — answering `404` there
/// would assert the member does not exist while the very same message admitted that
/// it does, and would send the SPA hunting for a typo instead of a broken store.
fn unknown_member(member: &str) -> Response {
    refuse(
        StatusCode::NOT_FOUND,
        format!("no workspace member `{member}` in this workspace"),
    )
}

/// The `500` for a member that exists but whose engine failed to start (a locked or
/// corrupt store, an I/O fault) — the failure chain verbatim, never masked as a
/// missing member ([NFR-RA-05]).
fn member_unavailable(member: &str, err: &anyhow::Error) -> Response {
    refuse(
        StatusCode::INTERNAL_SERVER_ERROR,
        format!("workspace member `{member}` could not be started: {err:#}"),
    )
}

#[async_trait]
impl<S> FromRequestParts<S> for MemberEngine
where
    S: Send + Sync,
    Arc<Engine>: FromRef<S>,
    Arc<Backing<Engine>>: FromRef<S>,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let default: Arc<Engine> = FromRef::from_ref(state);
        let backing: Arc<Backing<Engine>> = FromRef::from_ref(state);
        // The single-root fast path, taken BEFORE the query is even looked at: a
        // plain repo must pay nothing at all for federation — not a registry, not an
        // allocation, not a query-string parse ([ADR-52]). The one engine IS the
        // root, so `?repo=` is inert there.
        let Some(registry) = backing.as_federated() else {
            return Ok(Self(default));
        };
        let member = match requested_member(parts) {
            Ok(None) => return Ok(Self(default)), // unscoped → the warmed default member
            Ok(Some(member)) => member,
            // The request named a scope we cannot read. Answering from the default
            // member would hand back a DIFFERENT member's figures under a `200` —
            // the one thing this extractor exists to prevent.
            Err(err) => {
                return Err(refuse(
                    StatusCode::BAD_REQUEST,
                    format!("the query string could not be parsed: {err}"),
                ))
            }
        };
        // Absence is decided against the manifest roster — cheaply, and without
        // starting anything — so it is never confused with a start failure below.
        if !registry.members().iter().any(|m| m.name == member) {
            return Err(unknown_member(&member));
        }
        let backing = Arc::clone(&backing);
        let resolved = {
            let member = member.clone();
            tokio::task::spawn_blocking(move || {
                backing
                    .as_federated()
                    .expect("federated backing checked before spawn")
                    .engine_for(&member)
            })
            .await
            // A panic crossing the pool is a core bug — re-raise it rather than
            // mask it as an unavailable member (mirrors `crate::bridge`).
            .unwrap_or_else(|err| std::panic::resume_unwind(err.into_panic()))
        };
        resolved
            .map(Self)
            .map_err(|err| member_unavailable(&member, &err))
    }
}

/// The trimmed, non-empty `?repo=` value, or `None` — an absent, blank, or
/// whitespace-only param is "unscoped", never a member named `""`.
///
/// An unparsable query string is an `Err`, **not** a `None`: silently treating it as
/// "unscoped" would resolve the default member and answer `200` with the wrong
/// member's figures for a request that explicitly named one.
fn requested_member(parts: &Parts) -> Result<Option<String>, QueryRejection> {
    let Query(q) = Query::<HashMap<String, String>>::try_from_uri(&parts.uri)?;
    Ok(crate::api_v1::opt_param(&q, REPO_PARAM))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;

    fn parts_for(uri: &str) -> Parts {
        Request::builder().uri(uri).body(()).unwrap().into_parts().0
    }

    /// The parsed member, panicking on the (separately-tested) parse-failure arm.
    fn member_of(uri: &str) -> Option<String> {
        requested_member(&parts_for(uri)).expect("the query string parses")
    }

    #[test]
    fn no_repo_param_is_unscoped() {
        assert_eq!(member_of("/api/v1/health"), None);
        assert_eq!(member_of("/api/v1/health?untested=1"), None);
    }

    #[test]
    fn a_blank_repo_param_is_unscoped_not_a_member_named_empty() {
        assert_eq!(member_of("/api/v1/health?repo="), None);
        assert_eq!(member_of("/api/v1/health?repo=%20"), None);
    }

    #[test]
    fn a_named_repo_is_the_requested_member_trimmed_and_decoded() {
        assert_eq!(member_of("/api/v1/health?repo=api").as_deref(), Some("api"));
        assert_eq!(
            member_of("/api/v1/health?repo=services%2Fapi&untested=1").as_deref(),
            Some("services/api"),
        );
    }

    /// The `?repo=` normalisation is the SAME one the `/api/v1/workspace/*` fan-out
    /// applies (`api_v1::opt_param`) — one param, one rule. If these two ever drifted,
    /// `/api/v1/health?repo=%20api` and `/api/v1/workspace/search?repo=%20api` would
    /// disagree about which member they mean.
    #[test]
    fn the_repo_rule_is_shared_with_the_fan_out() {
        let q: HashMap<String, String> =
            [(REPO_PARAM.to_string(), " api ".to_string())].into_iter().collect();
        assert_eq!(crate::api_v1::opt_param(&q, REPO_PARAM).as_deref(), Some("api"));
        assert_eq!(member_of("/api/v1/health?repo=%20api%20").as_deref(), Some("api"));
    }
}
