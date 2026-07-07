//! The shared **positional route-template normalizer** (S-069, CR-011,
//! [FR-CG-09]).
//!
//! An OpenAPI `ApiOperation` ([FR-CG-03]) and a framework-extracted `route`
//! node ([FR-FW-01]) describe the same HTTP endpoint in two different dialects:
//! the spec writes `/users/{id}`, an Express route writes `/users/:userId`, an
//! axum/FastAPI/Spring route writes `/users/{id}`. They bind iff their path
//! templates denote the **same shape** — the same static segments in the same
//! positions, with parameters in the same positions — *regardless of parameter
//! name or parameter syntax*. This module is the one function both sides run
//! through so that judgement is made in exactly one place ([FR-CG-09] "the
//! positional normalizer is shared between this client and the route nodes'
//! templates").
//!
//! # Positional, name-blind, syntax-aligning
//!
//! [`normalize_template`] maps every parameter segment to a single positional
//! placeholder (`{}`) and leaves static segments verbatim, so:
//!
//! - parameter **names** are ignored — spec `{id}` matches code `{user_id}`;
//! - parameter **syntax** is aligned — axum/OpenAPI/FastAPI/Spring `{id}`,
//!   Express/Rails `:id` all collapse to the same placeholder;
//! - the HTTP method and the static skeleton must still match exactly (that is
//!   the caller's job, over the normalized string).
//!
//! # Honestly unresolved, never approximately matched ([FR-CG-09], [NFR-RA-05])
//!
//! The normalizer is deliberately **minimal**: a segment it cannot positionally
//! align — a catch-all (`{*rest}`, `*splat`, FastAPI's `{p:path}`), a
//! regex-constrained parameter (`{id:[0-9]+}`, Express `:id(\d+)`), or a mixed
//! literal/parameter segment (`v{n}`) — makes the **whole template**
//! non-normalizable, and [`normalize_template`] returns [`None`]. The caller
//! treats a `None` template as a non-candidate, so an operation whose only
//! syntactic match is a catch-all or regex route stays in the `unresolved_refs`
//! ledger rather than binding to a route whose actual matching semantics the
//! normalizer cannot prove equal ([NFR-RA-05]). Approximate matching is exactly
//! what never-fabricate forbids.
//!
//! [FR-CG-03]: ../../../docs/specs/requirements/FR-CG-03.md
//! [FR-CG-09]: ../../../docs/specs/requirements/FR-CG-09.md
//! [FR-FW-01]: ../../../docs/specs/requirements/FR-FW-01.md
//! [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md

/// A `route` node's `name` is the framework registration rendered as
/// `"METHOD /path"` (S-012, [`framework`](super::framework)); an
/// `ApiOperation`'s captured route reference is encoded the same way (S-069).
/// Split it into the upper-cased HTTP method and the raw path template.
///
/// Returns `None` for a string that is not a well-formed `"METHOD /path"`: an
/// empty method, or a path that does not begin with `/` (a route name can never
/// have either, so a malformed one is simply never a candidate — never
/// fabricated).
pub(crate) fn parse_method_and_template(name: &str) -> Option<(String, &str)> {
    let (method, template) = name.split_once(' ')?;
    if method.is_empty() || !template.starts_with('/') {
        return None;
    }
    Some((method.to_ascii_uppercase(), template))
}

/// The canonical `(METHOD, normalized-template)` match key for one endpoint, or
/// `None` when the name is malformed or its template does not normalize cleanly
/// ([`normalize_template`]). The single key both an `ApiOperation`'s route
/// reference and a `route` node's name are reduced to before the exactly-one
/// candidate comparison ([FR-CG-09]).
///
/// [FR-CG-09]: ../../../docs/specs/requirements/FR-CG-09.md
pub(crate) fn route_key(name: &str) -> Option<(String, String)> {
    let (method, template) = parse_method_and_template(name)?;
    Some((method, normalize_template(template)?))
}

/// Reduce a path template to its **positional** form — parameter segments become
/// a bare `{}` placeholder, static segments stay verbatim — or `None` if any
/// segment cannot be positionally aligned (see the module docs).
///
/// The leading/trailing/`//` slash structure is preserved (an empty segment
/// stays empty), so `/users` and `/users/` are deliberately *distinct* shapes —
/// the normalizer never silently equates templates whose skeletons differ.
pub(crate) fn normalize_template(template: &str) -> Option<String> {
    if !template.starts_with('/') {
        return None;
    }
    let mut out = String::with_capacity(template.len());
    for (i, segment) in template.split('/').enumerate() {
        if i > 0 {
            out.push('/');
        }
        match classify_segment(segment)? {
            Segment::Placeholder => out.push_str("{}"),
            Segment::Literal => out.push_str(segment),
        }
    }
    Some(out)
}

/// What one `/`-delimited path segment normalizes to.
enum Segment {
    /// A clean single parameter (`{id}`, `:id`) — its name is irrelevant.
    Placeholder,
    /// A static segment, kept verbatim (including the empty segment).
    Literal,
}

/// Classify one path segment, or `None` when it is a parameter shape the
/// normalizer refuses to positionally align (catch-all, regex-constrained, or a
/// segment that mixes a literal with parameter/metacharacter syntax).
fn classify_segment(segment: &str) -> Option<Segment> {
    // A `{name}` parameter (OpenAPI, axum, FastAPI, Spring): clean iff the inner
    // text is a plain identifier. `{*rest}` (catch-all), `{p:path}`/`{id:[0-9]+}`
    // (typed/regex) carry `*`/`:` and are therefore non-normalizable.
    if let Some(inner) = segment.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
        return is_plain_param_name(inner).then_some(Segment::Placeholder);
    }
    // A `:name` parameter (Express, Rails): clean iff the rest is a plain
    // identifier. `:id(\d+)` (regex) carries `(` and is non-normalizable.
    if let Some(inner) = segment.strip_prefix(':') {
        return is_plain_param_name(inner).then_some(Segment::Placeholder);
    }
    // A wildcard / catch-all introduced bare (`*`, `*splat`, Express `*`): never
    // positionally alignable.
    if segment.starts_with('*') {
        return None;
    }
    // A static segment must be a pure literal: any parameter/regex
    // metacharacter inside it (`v{n}`, `file.(json|yaml)`) is a shape the
    // normalizer will not approximate — the whole template is non-normalizable.
    if segment.bytes().any(is_template_metachar) {
        return None;
    }
    Some(Segment::Literal)
}

/// `true` for a non-empty parameter name made only of identifier characters
/// (ASCII alphanumeric or `_`) — the conservative shape every supported
/// framework uses for a plain, unconstrained path parameter. Anything richer
/// (a `:`-typed converter, a `(`-regex, a `-`/`.` separator) is rejected so the
/// segment falls through to non-normalizable rather than being approximated.
fn is_plain_param_name(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// `true` for a byte that marks a segment as a parameter/regex/catch-all shape
/// the normalizer refuses to treat as a plain static literal.
fn is_template_metachar(b: u8) -> bool {
    matches!(
        b,
        b'{' | b'}' | b':' | b'*' | b'(' | b')' | b'[' | b']' | b'\\' | b'?' | b'+'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_method_and_template_from_a_route_name() {
        assert_eq!(
            parse_method_and_template("GET /users/{id}"),
            Some(("GET".to_string(), "/users/{id}"))
        );
        // The method is upper-cased so a lower-cased OpenAPI operation method
        // ("get") and an upper-cased route method ("GET") compare equal.
        assert_eq!(
            parse_method_and_template("get /users"),
            Some(("GET".to_string(), "/users"))
        );
        // Malformed: no space, empty method, or a non-`/` path.
        assert_eq!(parse_method_and_template("GET"), None);
        assert_eq!(parse_method_and_template(" /users"), None);
        assert_eq!(parse_method_and_template("GET users"), None);
    }

    #[test]
    fn parameter_names_are_ignored_so_drifted_names_normalize_equal() {
        // The {id}-vs-{user_id} parameter-name drift the acceptance calls out.
        assert_eq!(
            normalize_template("/users/{id}"),
            normalize_template("/users/{user_id}")
        );
        assert_eq!(
            normalize_template("/users/{id}").as_deref(),
            Some("/users/{}")
        );
    }

    #[test]
    fn axum_express_fastapi_spring_parameter_syntaxes_all_align() {
        // The framework fixture matrix at the normalizer grain: every dialect's
        // single-parameter syntax collapses to the same positional template, so
        // an OpenAPI `{id}` operation matches a route in any of them.
        let openapi = normalize_template("/users/{id}/posts/{postId}");
        for equivalent in [
            "/users/{id}/posts/{postId}",      // axum / FastAPI / Spring / OpenAPI
            "/users/{userId}/posts/{post_id}", // name drift, still `{}`
            "/users/:id/posts/:postId",        // Express / Rails
            "/users/:user/posts/:p",           // Express, drifted names
        ] {
            assert_eq!(
                normalize_template(equivalent),
                openapi,
                "{equivalent} must align with the OpenAPI template"
            );
        }
        assert_eq!(openapi.as_deref(), Some("/users/{}/posts/{}"));
    }

    #[test]
    fn static_templates_normalize_to_themselves_and_distinguish_skeletons() {
        assert_eq!(normalize_template("/health").as_deref(), Some("/health"));
        assert_eq!(normalize_template("/").as_deref(), Some("/"));
        // A trailing slash is a different skeleton — never silently equated.
        assert_ne!(normalize_template("/users"), normalize_template("/users/"));
        // Different static skeletons of equal length never collide.
        assert_ne!(
            normalize_template("/users/{id}"),
            normalize_template("/orders/{id}")
        );
        // A method difference is the caller's concern, but two methods over the
        // same template share a normalized template (the caller keys on both).
        assert_eq!(
            route_key("GET /users/{id}").map(|(_, t)| t),
            route_key("POST /users/{userId}").map(|(_, t)| t)
        );
    }

    #[test]
    fn catch_all_templates_are_non_normalizable() {
        // axum `{*rest}`, FastAPI `{path:path}`, and a bare `*`/`*splat`
        // wildcard each refuse to normalize — they stay honestly unresolved.
        for catch_all in [
            "/files/{*rest}",
            "/files/{path:path}",
            "/files/*",
            "/files/*splat",
            "/assets/*/x",
        ] {
            assert_eq!(
                normalize_template(catch_all),
                None,
                "{catch_all} must be non-normalizable (catch-all)"
            );
        }
    }

    #[test]
    fn regex_constrained_parameters_are_non_normalizable() {
        // A typed/regex parameter cannot be positionally aligned with a plain
        // `{id}` without asserting a matching semantics we cannot prove equal.
        for constrained in [
            "/users/{id:[0-9]+}", // Spring / FastAPI typed
            "/users/:id(\\d+)",   // Express regex
            "/v{version}/users",  // mixed literal + parameter segment
            "/users/{id}.json",   // parameter fused with a literal suffix
        ] {
            assert_eq!(
                normalize_template(constrained),
                None,
                "{constrained} must be non-normalizable (regex/mixed)"
            );
        }
    }

    #[test]
    fn route_key_is_none_for_a_malformed_or_non_normalizing_name() {
        assert_eq!(route_key("not a route"), None); // path lacks a leading `/`
        assert_eq!(route_key("GET /files/{*rest}"), None); // non-normalizing
        assert_eq!(
            route_key("GET /users/{id}"),
            Some(("GET".to_string(), "/users/{}".to_string()))
        );
    }
}
