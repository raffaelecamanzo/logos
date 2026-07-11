//! The **HTTP client-call arm** normalizer and refusal classifier (S-252,
//! CR-061, [FR-WS-08], [ADR-54]).
//!
//! The pluggable invocation-arm contract ([FR-WS-07], S-251) supplies the
//! generic consumer-side interpreter
//! ([`capture_invocation_refs`](crate::extract::config::refs)) and the
//! namespace-generic bridge match loop. This module is the HTTP arm's **only**
//! arm-specific logic: the `render_target` normalizer the interpreter drives its
//! captured sites through, expressed so the exact same judgement also names the
//! coverage **reason** an unbindable call surfaces under ([FR-WS-05]).
//!
//! # Only a statically present path literal ever binds ([NFR-RA-05])
//!
//! An outbound call binds a `Route` provider iff its request path is a **static,
//! absolute, positionally-normalizable** template — the same `route_key`
//! ([FR-CG-09]) shape the provider side reduces to, so a client call and a route
//! meet on one key regardless of parameter-name/syntax drift. Everything else is
//! **refused** — it contributes no reference and no ledger entry (the interpreter
//! drops a `None` render), so a runtime-composed or non-normalizable call is
//! *honestly unbound*, never approximately matched:
//!
//! - a **bare-variable / base-URL-composed / interpolated** path (the static path
//!   literal is absent) → [`ClientCallRefusal::BaseUrlRuntime`];
//! - a **relative** literal (`"users/{id}"`) whose route prefix is composed
//!   elsewhere (a client base URL, an un-joined group/`include()`), or an
//!   absolute-URL literal pointing at an externally-based endpoint →
//!   [`ClientCallRefusal::BaseUrlRuntime`];
//! - an **absolute literal that does not normalize** (a catch-all/regex/mixed
//!   template) → [`ClientCallRefusal::PathNotComposed`].
//!
//! The refusal reasons map 1:1 onto the federation coverage vocabulary
//! ([`UnboundReason`](crate::federation::UnboundReason)) in
//! [`crate::federation::coverage`]; keeping the mapping there (not here) means
//! this low-level resolver module carries no dependency on the federation layer.
//!
//! [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
//! [FR-WS-07]: ../../../docs/specs/requirements/FR-WS-07.md
//! [FR-WS-08]: ../../../docs/specs/requirements/FR-WS-08.md
//! [FR-CG-09]: ../../../docs/specs/requirements/FR-CG-09.md
//! [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
//! [ADR-54]: ../../../docs/specs/architecture/decisions/ADR-54.md

use std::collections::BTreeMap;

use super::route_template::route_key;

/// The HTTP-client crates whose presence in a file's references marks it a
/// plausible **client** file, per language ([FR-WS-08], [NFR-RA-05]).
///
/// The consumer-side twin of the framework provider capture's ledger-gated
/// candidacy ([FR-FW-04]): the outbound-call `.scm` anchor is a broad
/// `<receiver>.<method>(<arg>)` shape, so a collection/registry `.get("/x")` is
/// syntactically indistinguishable from `client.get("/x")`. Rather than
/// fabricate an outbound call from an incidental `/`-shaped string key, the arm
/// captures **only** in a file that actually references one of these crates — a
/// file that uses an undetected client wrapper simply stays honestly unbound
/// (under-capture is safe; over-capture would fabricate a cross-service edge).
///
/// Rust-only today (the only language shipping an `invocations` capability); a
/// new language's arm adds its own detector set here (a follow-up may lift this
/// into the plugin descriptor alongside `framework_detectors`).
///
/// [FR-WS-08]: ../../../docs/specs/requirements/FR-WS-08.md
/// [FR-FW-04]: ../../../docs/specs/requirements/FR-FW-04.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
pub(crate) fn http_client_crates(language: &str) -> &'static [&'static str] {
    match language {
        "rust" => &["reqwest", "hyper", "isahc", "ureq", "awc", "surf"],
        // A language without a declared client-detector set never captures — its
        // `invocations` capture (if any) contributes nothing until its arm lands.
        _ => &[],
    }
}

/// The capture slot naming the request's HTTP method (`get`, `POST`, …). Filled
/// by every per-language client-call capture.
pub(crate) const METHOD_SLOT: &str = "method";
/// The capture slot naming the request's path **when it is a static string
/// literal** (`"/users/{id}"`). Absent when the path is composed at runtime.
pub(crate) const PATH_SLOT: &str = "path";
/// A capture slot the per-language dispatch sets (to any value) when the path
/// argument is **not** a static string literal — a bare variable, a
/// `format!`/template, a concatenation. Its mere presence signals a
/// runtime-composed path, so the arm refuses the site without guessing a target.
pub(crate) const DYNAMIC_PATH_SLOT: &str = "path_dynamic";

/// Why an outbound HTTP client call is honestly unbindable ([FR-WS-08],
/// [NFR-RA-05]).
///
/// Both variants map onto a federation coverage
/// [`UnboundReason`](crate::federation::UnboundReason) in
/// [`crate::federation::coverage`]; this enum keeps the classification in the
/// resolver layer without importing the federation layer.
///
/// [FR-WS-08]: ../../../docs/specs/requirements/FR-WS-08.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientCallRefusal {
    /// The request path is composed at runtime — a bare variable, a base-URL
    /// join, an interpolated/`format!` string, or a relative/absolute-URL literal
    /// whose static route prefix is not present. Surfaces as `base-url-runtime`.
    BaseUrlRuntime,
    /// A static, absolute path literal is present but its template does not
    /// positionally normalize (a catch-all/regex/mixed segment). Surfaces as
    /// `path-not-composed` — never approximately matched.
    PathNotComposed,
}

/// Reduce a captured client-call site's `slots` to its `"METHOD /template"` bind
/// target, or the [reason](ClientCallRefusal) it is honestly unbindable
/// ([FR-WS-08], [ADR-54]).
///
/// The single judgement the arm makes. A returned target is the **raw**
/// `"METHOD /template"` string (the method upper-cased, the template verbatim) —
/// byte-identical in shape to a framework `Route` node's name — so the intra-repo
/// `(ArtifactBinding, Path)` route binder and the cross-service bridge both key it
/// through the shared [`route_key`] exactly as they key the provider. It never
/// pre-normalizes the template, so the stored ledger target stays re-normalizable.
///
/// [FR-WS-08]: ../../../docs/specs/requirements/FR-WS-08.md
/// [ADR-54]: ../../../docs/specs/architecture/decisions/ADR-54.md
pub(crate) fn classify_client_call(
    slots: &BTreeMap<String, String>,
) -> Result<String, ClientCallRefusal> {
    // A method is mandatory; a site with none is not a well-formed HTTP call and
    // is refused rather than fabricating one (never-fabricate at the arm grain).
    let Some(method) = slots.get(METHOD_SLOT).map(|m| m.trim()).filter(|m| !m.is_empty()) else {
        return Err(ClientCallRefusal::BaseUrlRuntime);
    };

    // A non-literal path argument (bare variable, format!, concatenation) is
    // signalled by the dynamic-path slot: the URL is composed at runtime, so no
    // static path is present to bind.
    if slots.contains_key(DYNAMIC_PATH_SLOT) {
        return Err(ClientCallRefusal::BaseUrlRuntime);
    }

    let Some(path) = slots.get(PATH_SLOT).map(|p| p.trim()).filter(|p| !p.is_empty()) else {
        // Neither a literal nor an explicit dynamic marker — nothing to compose.
        return Err(ClientCallRefusal::BaseUrlRuntime);
    };

    // A literal that is not an absolute path is a base-URL-relative fragment (or
    // an absolute URL, which contains no leading `/` before its scheme): the
    // route prefix is composed elsewhere, so the site is not workspace-composable.
    if !path.starts_with('/') {
        return Err(ClientCallRefusal::BaseUrlRuntime);
    }

    let candidate = format!("{} {path}", method.to_ascii_uppercase());
    // The path literal is absolute, but its template must still positionally
    // normalize — a catch-all/regex/mixed template is never approximated.
    if route_key(&candidate).is_none() {
        return Err(ClientCallRefusal::PathNotComposed);
    }
    Ok(candidate)
}

/// The `render_target` normalizer the HTTP arm hands to
/// [`capture_invocation_refs`](crate::extract::config::refs::capture_invocation_refs):
/// `Some("METHOD /template")` for a static, normalizable call; `None` for any
/// runtime-composed or non-normalizable one — contributing no reference and no
/// ledger entry ([NFR-RA-05]).
///
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
pub(crate) fn render_client_call_target(slots: &BTreeMap<String, String>) -> Option<String> {
    classify_client_call(slots).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slots(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    /// A static, absolute, normalizable call renders its raw `"METHOD /template"`
    /// target (method upper-cased, template verbatim) — the exact shape a
    /// framework `Route` node carries, so both sides meet on one `route_key`.
    #[test]
    fn a_static_absolute_call_renders_its_method_template_target() {
        let s = slots(&[(METHOD_SLOT, "get"), (PATH_SLOT, "/users/{id}")]);
        assert_eq!(classify_client_call(&s), Ok("GET /users/{id}".to_string()));
        assert_eq!(render_client_call_target(&s).as_deref(), Some("GET /users/{id}"));
        // The stored target re-normalizes cleanly (it was NOT pre-normalized), so
        // the intra-repo route binder and the bridge can key it via `route_key`.
        assert_eq!(
            route_key("GET /users/{id}"),
            Some(("GET".to_string(), "/users/{}".to_string()))
        );
    }

    /// Acceptance (2) `base-url-runtime`: a runtime-composed path — a bare
    /// variable (no literal captured, dynamic-path slot set) — is refused, and the
    /// normalizer returns `None` (no reference, no ledger entry).
    #[test]
    fn a_bare_variable_path_is_base_url_runtime() {
        // The per-language dispatch could not extract a literal, so it set the
        // dynamic-path marker instead of a `path` slot.
        let s = slots(&[(METHOD_SLOT, "get"), (DYNAMIC_PATH_SLOT, "url")]);
        assert_eq!(
            classify_client_call(&s),
            Err(ClientCallRefusal::BaseUrlRuntime)
        );
        assert_eq!(render_client_call_target(&s), None);

        // A missing path slot entirely (no literal, no marker) is likewise refused.
        let bare = slots(&[(METHOD_SLOT, "get")]);
        assert_eq!(
            classify_client_call(&bare),
            Err(ClientCallRefusal::BaseUrlRuntime)
        );
    }

    /// Acceptance (2) `base-url-runtime`: a base-URL-composed path — a relative
    /// literal whose route prefix lives on a client base URL — is refused. An
    /// absolute-URL literal (a hard-coded external base) is likewise refused.
    #[test]
    fn a_base_url_composed_or_absolute_literal_is_base_url_runtime() {
        for path in ["users/{id}", "v1/users", "https://api.example.com/users/{id}"] {
            let s = slots(&[(METHOD_SLOT, "get"), (PATH_SLOT, path)]);
            assert_eq!(
                classify_client_call(&s),
                Err(ClientCallRefusal::BaseUrlRuntime),
                "{path} has no workspace-composable absolute route prefix"
            );
            assert_eq!(render_client_call_target(&s), None);
        }
    }

    /// Acceptance (2) `path-not-composed`: an absolute literal that does not
    /// positionally normalize (catch-all/regex/mixed) is refused — never
    /// approximately matched ([NFR-RA-05]).
    #[test]
    fn a_non_normalizable_absolute_literal_is_path_not_composed() {
        for path in [
            "/files/{*rest}",  // catch-all
            "/files/{p:path}", // typed/catch-all
            "/users/{id:[0-9]+}", // regex-constrained
            "/v{version}/users",  // mixed literal + parameter segment
        ] {
            let s = slots(&[(METHOD_SLOT, "get"), (PATH_SLOT, path)]);
            assert_eq!(
                classify_client_call(&s),
                Err(ClientCallRefusal::PathNotComposed),
                "{path} must be path-not-composed (never approximated)"
            );
            assert_eq!(render_client_call_target(&s), None);
        }
    }

    /// The method is upper-cased so a lower-cased client idiom (`client.get`)
    /// keys equal to an upper-cased route method — the drift `route_key` erases.
    #[test]
    fn the_method_is_upper_cased_for_a_stable_key() {
        let lower = slots(&[(METHOD_SLOT, "post"), (PATH_SLOT, "/orders")]);
        let upper = slots(&[(METHOD_SLOT, "POST"), (PATH_SLOT, "/orders")]);
        assert_eq!(classify_client_call(&lower), classify_client_call(&upper));
        assert_eq!(classify_client_call(&lower), Ok("POST /orders".to_string()));
    }
}
