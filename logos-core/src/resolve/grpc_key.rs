//! The shared **gRPC method-key normalizer** (S-253, CR-061, [FR-WS-09]).
//!
//! A generated-stub gRPC call (`client.GetUser(ctx, req)`, a consumer) and an
//! enriched `ProtoService` method (`example.v1.UserService/GetUser`, a provider)
//! denote the same RPC endpoint from two sides of a service boundary. They bind
//! iff they reduce to the **same fully-qualified key** — the `route_key`
//! ([`super::route_template`]) analogue for the [`Grpc`](crate::model::BridgeNamespace::Grpc)
//! invocation namespace. This module is the one function the consumer capture
//! runs through, so the qualification judgement is made in exactly one place
//! ([ADR-54]).
//!
//! # The key shape: `package.Service/Method`
//!
//! The proto wire identity of an RPC is `/package.Service/Method`; this
//! normalizer builds the un-slashed-prefix form `package.Service/Method` that
//! both the stub-call consumer and the proto-service provider key on. The
//! `package` is the file-level proto package (`example.v1`), a dotted chain of
//! proto identifiers; `Service` and `Method` are single proto identifiers.
//!
//! # Honestly unqualifiable, never approximately matched ([FR-WS-09], [NFR-RA-05])
//!
//! Every part must be a **statically-captured proto identifier**. A stub call
//! whose package, service, or method the capture could not pin to a literal
//! identifier — a dynamically-selected method, a call on a client whose service
//! could not be traced, a missing package — cannot be fully qualified, so
//! [`grpc_key`] returns [`None`]. The caller ([`capture_invocation_refs`](crate::extract))
//! treats a `None` as a non-candidate: the call emits no reference and stays an
//! honest coverage miss rather than binding on a partial key. Approximate
//! matching is exactly what never-fabricate forbids.
//!
//! [FR-WS-09]: ../../../docs/specs/requirements/FR-WS-09.md
//! [ADR-54]: ../../../docs/specs/architecture/decisions/ADR-54.md
//! [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md

// The normalizer is the reuse foundation the gRPC stub-call capture drives sites
// through; its production caller is the per-language `.scm` code-path capture (a
// coordinated cross-arm follow-up shared with the S-252 HTTP client-call arm — the
// same `extract_one` invocation hook), so today it is exercised by the arm's unit
// tests (here + `extract::config::refs`) rather than a production call site. This
// mirrors the S-251 `capture_invocation_refs` scaffolding it is a normalizer for.
#![allow(dead_code)]

use std::collections::BTreeMap;

/// The fully-qualified `package.Service/Method` key for one gRPC endpoint, or
/// `None` when any part is not a statically-captured proto identifier (an
/// unqualifiable call — never approximately matched, [NFR-RA-05]).
///
/// `package` is a dotted chain of proto identifiers (`example.v1`); `service`
/// and `method` are single proto identifiers. Every part is validated so a
/// runtime-composed or partially-captured target is refused before it can reach
/// the ledger.
///
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
pub(crate) fn grpc_key(package: &str, service: &str, method: &str) -> Option<String> {
    if !is_ident_path(package) || !is_ident(service) || !is_ident(method) {
        return None;
    }
    Some(format!("{package}.{service}/{method}"))
}

/// Build the [`grpc_key`] from a captured invocation site's named slots
/// (`package`/`service`/`method`), or `None` when a slot is absent or its value
/// is not a static proto identifier.
///
/// This is the `render_target` normalizer the generic consumer-side capture
/// interpreter ([`capture_invocation_refs`](crate::extract)) drives the gRPC
/// stub-call sites through: the `.scm` names the three captures, this reduces
/// them to the portable key, and an unqualifiable site is refused by returning
/// `None`.
pub(crate) fn grpc_key_from_slots(slots: &BTreeMap<String, String>) -> Option<String> {
    grpc_key(
        slots.get("package").map(String::as_str)?,
        slots.get("service").map(String::as_str)?,
        slots.get("method").map(String::as_str)?,
    )
}

/// `true` if `s` is a single proto identifier: a non-empty run of ASCII
/// letters, digits, and underscores that does not start with a digit. Anything
/// else — an empty token, a call expression, a quoted string, a dotted path, a
/// runtime interpolation — is rejected, so only a statically-pinned name
/// qualifies.
fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c == '_' || c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

/// `true` if `s` is a dotted chain of proto identifiers (`example.v1`,
/// `com.acme.orders`) — every `.`-delimited segment is a valid [`is_ident`]. An
/// empty path, a leading/trailing/doubled dot, or any non-identifier segment is
/// rejected: a package that is not fully captured cannot fully qualify a call.
fn is_ident_path(s: &str) -> bool {
    !s.is_empty() && s.split('.').all(is_ident)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The happy path: three static parts fold into the `package.Service/Method`
    /// key both the stub-call consumer and the proto-service provider match on.
    #[test]
    fn qualified_parts_build_the_fully_qualified_key() {
        assert_eq!(
            grpc_key("example.v1", "UserService", "GetUser"),
            Some("example.v1.UserService/GetUser".to_string())
        );
        // A single-segment package qualifies too.
        assert_eq!(
            grpc_key("billing", "Ledger", "Post"),
            Some("billing.Ledger/Post".to_string())
        );
        // A deep package chain.
        assert_eq!(
            grpc_key("com.acme.orders.v2", "OrderService", "Place"),
            Some("com.acme.orders.v2.OrderService/Place".to_string())
        );
    }

    /// Any part that is not a statically-captured proto identifier makes the call
    /// unqualifiable — `None`, never an approximate key ([NFR-RA-05]).
    #[test]
    fn an_unqualifiable_part_refuses_the_key() {
        // An empty package (no `package` statement captured) cannot fully qualify.
        assert_eq!(grpc_key("", "UserService", "GetUser"), None);
        // A dynamically-selected method (a captured expression, not an identifier).
        assert_eq!(grpc_key("example.v1", "UserService", "methods[i]"), None);
        assert_eq!(grpc_key("example.v1", "UserService", ""), None);
        // A service traced only to a call expression, not a literal name.
        assert_eq!(grpc_key("example.v1", "newClient()", "GetUser"), None);
        // A package with a malformed segment (leading dot, doubled dot, digit-led).
        assert_eq!(grpc_key(".example.v1", "S", "M"), None);
        assert_eq!(grpc_key("example..v1", "S", "M"), None);
        assert_eq!(grpc_key("1example", "S", "M"), None);
        // A quoted / interpolated method target.
        assert_eq!(grpc_key("example.v1", "UserService", "\"GetUser\""), None);
    }

    /// The slots-based `render_target` reads the three named captures; a missing
    /// slot (a capture the `.scm` did not bind) is itself an unqualifiable site.
    #[test]
    fn slots_render_target_reads_the_three_captures() {
        let mut slots = BTreeMap::new();
        slots.insert("package".to_string(), "example.v1".to_string());
        slots.insert("service".to_string(), "UserService".to_string());
        slots.insert("method".to_string(), "GetUser".to_string());
        assert_eq!(
            grpc_key_from_slots(&slots),
            Some("example.v1.UserService/GetUser".to_string())
        );

        // A missing `method` capture → unqualifiable.
        slots.remove("method");
        assert_eq!(grpc_key_from_slots(&slots), None);

        // An empty map (a language whose capture bound nothing) → unqualifiable.
        assert_eq!(grpc_key_from_slots(&BTreeMap::new()), None);
    }
}
