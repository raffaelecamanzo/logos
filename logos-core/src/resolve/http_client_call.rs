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
//! **refused** — it contributes no reference, so a runtime-composed or
//! non-normalizable call is *honestly unbound*, never approximately matched:
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
//! # A refusal is recorded, not silent (S-374, [CR-120])
//!
//! "No reference" is not "no trace". Refusing used to mean the site left nothing
//! whatsoever — the caller discarded this classifier's reason with
//! [`Result::ok`] and the shared interpreter skipped the site — so
//! [FR-WS-08] AC2's "appears under a runtime-composition coverage reason" was
//! unmet and an estate whose client paths are all composed at runtime read like
//! one with no outbound calls at all. `extract::capture_http_client_call_arm`
//! now keeps the reason: a [`BaseUrlRuntime`](ClientCallRefusal::BaseUrlRuntime)
//! site leaves one **keyless** ledger row (empty target — no fabricated
//! template, inert to binding and promotion) which the coverage tier reports as
//! `base-url-runtime`. The reference-emitting contract below is unchanged:
//! [`render_client_call_target`] still returns `None`, and a refused site still
//! contributes no reference and no edge.
//!
//! A [`PathNotComposed`](ClientCallRefusal::PathNotComposed) site is still
//! dropped without a row — the coverage tier tells the two HTTP refusals apart by
//! whether the stored target is empty, so that reason needs a non-keyless row
//! and a mechanism of its own. Which populations are recorded and which stay
//! invisible is enumerated once, on
//! `extract::capture_http_client_call_arm`; this module's job is only to name
//! the reason.
//!
//! [CR-120]: ../../../docs/requests/CR-120-invocation-arms-report-their-own-refusals.md
//!
//! [FR-WS-05]: ../../../docs/specs/requirements/FR-WS-05.md
//! [FR-WS-07]: ../../../docs/specs/requirements/FR-WS-07.md
//! [FR-WS-08]: ../../../docs/specs/requirements/FR-WS-08.md
//! [FR-CG-09]: ../../../docs/specs/requirements/FR-CG-09.md
//! [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
//! [ADR-54]: ../../../docs/specs/architecture/decisions/ADR-54.md

use std::collections::BTreeMap;

use super::binding::placeholder_keys;
use super::route_template::route_key;

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
    /// whose static route prefix is not present.
    ///
    /// Recorded as a keyless ledger row by the capture arm and surfaced as
    /// `base-url-runtime` (S-374): this is the variant with a production
    /// producer, and the reason a declined call site is now visible rather than
    /// merely absent.
    BaseUrlRuntime,
    /// A static, absolute path literal is present but its template does not
    /// positionally normalize (a catch-all/regex/mixed segment). Surfaces as
    /// `path-not-composed` — never approximately matched.
    ///
    /// The variant **without** a producer reachable from an index run. The
    /// capture arm drops such a site without a row (for the reason given in this
    /// module's docs), and the coverage tier maps to it only from a *stored* HTTP
    /// target that fails `route_key` — which the arm never stores, because
    /// accepting the target is that same test. So it is reachable from this
    /// classifier and from a row written by an older binary, not from anything
    /// this one records. Stated rather than left to be inferred, on the sprint
    /// that removed a sibling variant for exactly this.
    PathNotComposed,
}

/// What the arm resolved a captured call site's path to (S-382, [ADR-64]).
///
/// Two admissions, and the distinction is the whole of [ADR-64]'s
/// committed-evidence line at this grain: a [`Literal`](ClientCallPath::Literal)
/// is proven by the call site itself and binds today; a
/// [`ConfigBound`](ClientCallPath::ConfigBound) is proven only once the committed
/// configuration is read, so it is carried to resolution rather than bound here
/// — and rather than refused, which is what it was before S-382.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ClientCallPath {
    /// A static, absolute, positionally-normalizable `"METHOD /template"` — the
    /// pre-S-382 admission, unchanged.
    Literal(String),
    /// A `"METHOD /template"` whose template carries one or more `${…}`
    /// placeholders naming configuration keys. **Not yet a target**: it keys
    /// nothing until [`binding`](super::binding) resolves the placeholders
    /// against the committed corpus, and it is stored verbatim so the
    /// resolution reads the same bytes the source commits.
    ///
    /// Deliberately **not** required to be absolute. `${orders.base}/orders/{id}`
    /// begins with its configuration prefix, and demanding a leading `/` here
    /// would refuse exactly the shape this variant exists to admit; whether the
    /// *resolved* composition names a route is decided after substitution, by
    /// the same [`route_key`] test a literal passes.
    ConfigBound(String),
}

impl ClientCallPath {
    /// The stored ledger target — the raw string either admission contributes.
    pub(crate) fn target(&self) -> &str {
        match self {
            Self::Literal(target) | Self::ConfigBound(target) => target,
        }
    }
}

/// Reduce a captured client-call site's `slots` to its `"METHOD /template"` bind
/// target, or the [reason](ClientCallRefusal) it is honestly unbindable
/// ([FR-WS-08], [ADR-54], [ADR-64]).
///
/// The single judgement the arm makes. A returned
/// [`Literal`](ClientCallPath::Literal) is the **raw** `"METHOD /template"`
/// string (the method upper-cased, the template verbatim) — byte-identical in
/// shape to a framework `Route` node's name — so the intra-repo
/// `(ArtifactBinding, Path)` route binder and the cross-service bridge both key it
/// through the shared [`route_key`] exactly as they key the provider. It never
/// pre-normalizes the template, so the stored ledger target stays re-normalizable.
///
/// # A configuration placeholder is no longer a refusal (S-382, [ADR-64])
///
/// A path literal carrying a `${…}` placeholder used to fall through to the
/// absolute-path test and be refused as
/// [`BaseUrlRuntime`](ClientCallRefusal::BaseUrlRuntime) — which was true of the
/// *call site* and false of the *repository*: the value is committed, in a file
/// the index already holds. It is now admitted as
/// [`ConfigBound`](ClientCallPath::ConfigBound) and resolved downstream. The
/// placeholder test runs **before** the absoluteness and normalization tests
/// precisely because neither is answerable until the substitution is made.
///
/// **This widens what is captured, never what is believed.** A config-bound path
/// creates no edge here, and the composition it resolves to must still pass the
/// same [`route_key`] test a literal passes.
///
/// [FR-WS-08]: ../../../docs/specs/requirements/FR-WS-08.md
/// [ADR-54]: ../../../docs/specs/architecture/decisions/ADR-54.md
/// [ADR-64]: ../../../docs/specs/architecture/decisions/ADR-64.md
pub(crate) fn classify_client_call(
    slots: &BTreeMap<String, String>,
) -> Result<ClientCallPath, ClientCallRefusal> {
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

    let candidate = format!("{} {path}", method.to_ascii_uppercase());

    // A `${…}` placeholder names a configuration key, and the value it names is
    // committed (S-382, ADR-64). Decided FIRST: neither the absoluteness test
    // below nor `route_key` is answerable before the substitution is made, so
    // running either first would refuse the shape this admission exists for.
    if placeholder_keys(path).is_some() {
        return Ok(ClientCallPath::ConfigBound(candidate));
    }

    // A literal that is not an absolute path is a base-URL-relative fragment (or
    // an absolute URL, which contains no leading `/` before its scheme): the
    // route prefix is composed elsewhere, so the site is not workspace-composable.
    if !path.starts_with('/') {
        return Err(ClientCallRefusal::BaseUrlRuntime);
    }

    // The path literal is absolute, but its template must still positionally
    // normalize — a catch-all/regex/mixed template is never approximated.
    if route_key(&candidate).is_none() {
        return Err(ClientCallRefusal::PathNotComposed);
    }
    Ok(ClientCallPath::Literal(candidate))
}

/// Why an **already-composed** `"METHOD /template"` does not bind, judged by the
/// same rule [`classify_client_call`] applies to a captured site (S-382).
///
/// The configuration resolver composes a template *after* capture — substituting
/// a committed value into a `${…}` placeholder — so the result must be judged
/// again, and by this arm's rule rather than by the ledger convention
/// `client_call_refusal` uses. That convention reads an **empty** stored target
/// as `base-url-runtime` and any non-empty one as `path-not-composed`, which is
/// correct for a *stored* row and wrong for a composed template: a key holding
/// `https://orders:8080` composes a non-empty absolute URL whose route prefix is
/// not present, which is `base-url-runtime` by this arm's own definition.
///
/// Implemented by feeding the composition back through [`classify_client_call`]
/// rather than re-stating its two tests, so the composed and the captured paths
/// can never disagree about the same string.
///
/// Returns [`None`] when the composition **does** bind — the caller then has a
/// target, not a refusal.
pub(crate) fn composed_refusal(target: &str) -> Option<ClientCallRefusal> {
    let (method, path) = target.split_once(' ')?;
    let slots: BTreeMap<String, String> = [
        (METHOD_SLOT.to_string(), method.to_string()),
        (PATH_SLOT.to_string(), path.to_string()),
    ]
    .into_iter()
    .collect();
    classify_client_call(&slots).err()
}

/// The `render_target` normalizer the HTTP arm hands to
/// [`capture_invocation_refs`](crate::extract::config::refs::capture_invocation_refs):
/// `Some("METHOD /template")` for a static, normalizable call; `None` for any
/// runtime-composed or non-normalizable one — contributing no reference
/// ([NFR-RA-05]).
///
/// This is the arm's **reference** contract and it is unchanged by S-374; the
/// arm's caller re-runs [`classify_client_call`] to keep the refusal *reason*
/// this discards, which is what makes a declined site visible without making it
/// bindable.
///
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
pub(crate) fn render_client_call_target(slots: &BTreeMap<String, String>) -> Option<String> {
    classify_client_call(slots).ok().map(|path| path.target().to_string())
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
        assert_eq!(
            classify_client_call(&s),
            Ok(ClientCallPath::Literal("GET /users/{id}".to_string()))
        );
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
    /// normalizer returns `None`, so no reference. Since S-374 the capture arm
    /// keeps the reason and records a keyless ledger row for such a site; that
    /// half is asserted at the arm, not here (see this module's docs).
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
        assert_eq!(
            classify_client_call(&lower),
            Ok(ClientCallPath::Literal("POST /orders".to_string()))
        );
    }

    /// S-382 / AC1. A path literal carrying a `${…}` placeholder is **admitted**
    /// as config-bound rather than refused as `base-url-runtime`, and its target
    /// is stored verbatim so the resolution reads the bytes the source commits.
    ///
    /// The three shapes that matter are the leading placeholder (the whole route
    /// prefix is configured), the interior one (a configured path segment), and
    /// the placeholder carrying an inline default.
    #[test]
    fn a_placeholder_bearing_path_is_config_bound_not_base_url_runtime() {
        for (path, target) in [
            ("${orders.base}/orders/{id}", "GET ${orders.base}/orders/{id}"),
            ("/api/${orders.version}/orders", "GET /api/${orders.version}/orders"),
            ("${orders.base:/orders}/{id}", "GET ${orders.base:/orders}/{id}"),
        ] {
            let s = slots(&[(METHOD_SLOT, "get"), (PATH_SLOT, path)]);
            assert_eq!(
                classify_client_call(&s),
                Ok(ClientCallPath::ConfigBound(target.to_string())),
                "{path} names a committed configuration key"
            );
            // It reaches the ledger, which is what makes it resolvable at all —
            // before S-382 the arm emitted nothing for this shape.
            assert_eq!(render_client_call_target(&s).as_deref(), Some(target));
        }
    }

    /// S-382, the near miss that decides the whole change: a **route parameter**
    /// is not a configuration placeholder. `/users/{id}` carries no `$`, so it
    /// stays a literal and keeps binding exactly as it did.
    #[test]
    fn a_route_parameter_is_not_a_configuration_placeholder() {
        let s = slots(&[(METHOD_SLOT, "get"), (PATH_SLOT, "/users/{id}/orders/{orderId}")]);
        assert!(matches!(classify_client_call(&s), Ok(ClientCallPath::Literal(_))));
        // And an unterminated `${` fabricates no key: it falls through to the
        // absoluteness test and refuses, exactly as it did before S-382.
        let broken = slots(&[(METHOD_SLOT, "get"), (PATH_SLOT, "${orders.base/id")]);
        assert_eq!(classify_client_call(&broken), Err(ClientCallRefusal::BaseUrlRuntime));
    }
}
