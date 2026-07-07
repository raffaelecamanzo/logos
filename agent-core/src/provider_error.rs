//! Honest classification of a provider-call failure for the chat error frame
//! ([S-199], [FR-UI-24], [NFR-CC-04]).
//!
//! A `rig` provider error is a layered [`std::error::Error`]: `PromptError` wraps
//! [`CompletionError`](rig_core::completion::CompletionError) wraps
//! [`http_client::Error`](rig_core::http_client::Error) wraps the `reqwest`
//! transport error wraps the OS error. `thiserror`'s `#[error("…: {0}")]` only
//! renders **one** layer down, so a bare `e.to_string()` keeps the framing
//! ("HttpError: Http client error: error sending request") but **drops the
//! legible root cause** ("Connection refused (os error 61)") that lives two more
//! `source()` hops in. [`classify_provider_error`] walks the whole
//! [`Error::source`] chain so the surfaced message carries the cause, and labels
//! it **transport vs HTTP-status vs auth** so the Chat surface can render an
//! actionable error instead of one opaque line.
//!
//! # The API key never appears here ([NFR-SE-07])
//! `rig` does not echo the key into its error bodies, and this module only ever
//! reads the error chain — it never touches the [`ProviderConfig`](crate::ProviderConfig).
//! So no rendered [`ProviderFailure`] can leak the secret.
//!
//! [S-199]: ../../docs/planning/journal.md#s-199-chat-provider-error-surfacing-and-configuration-preflight
//! [FR-UI-24]: ../../docs/specs/requirements/FR-UI-24.md
//! [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md
//! [NFR-SE-07]: ../../docs/specs/requirements/NFR-SE-07.md

use std::error::Error as StdError;
use std::fmt;

use rig_core::completion::CompletionError;
use rig_core::http_client::Error as HttpClientError;

/// How a provider call failed — the axis the Chat surface renders distinctly
/// ([FR-UI-24]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderErrorKind {
    /// The endpoint could not be reached or the exchange did not complete: DNS
    /// failure, connection refused, TLS error, or timeout. The request never got
    /// an HTTP response. Maps from [`CompletionError::HttpError`].
    Transport,
    /// The provider rejected the credentials — HTTP 401/403, or a body naming an
    /// authentication/authorization problem.
    Auth,
    /// The provider returned a non-success HTTP status that is not an auth
    /// rejection (rate limit, bad request, server error, …). Maps from
    /// [`CompletionError::ProviderError`] / a status-carrying transport error.
    HttpStatus,
    /// Any other failure (request building, response parsing, an unrecognized
    /// shape). Surfaced honestly rather than mislabeled.
    Other,
}

impl ProviderErrorKind {
    /// A short human label for the kind, used as the message's lead-in.
    fn label(self) -> &'static str {
        match self {
            ProviderErrorKind::Transport => "transport error",
            ProviderErrorKind::Auth => "authentication error",
            ProviderErrorKind::HttpStatus => "provider returned an error response",
            ProviderErrorKind::Other => "provider call failed",
        }
    }
}

/// A classified provider-call failure carrying the **full** source chain, the
/// HTTP status where one is present, and the response body where the provider
/// returned one ([S-199]). Renders to a single legible line ([FR-UI-24]); never
/// carries the API key ([NFR-SE-07]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderFailure {
    /// The classified kind — transport vs auth vs HTTP-status vs other.
    pub kind: ProviderErrorKind,
    /// The rendered source chain: each distinct [`Error::source`] layer's
    /// `Display`, joined `: ` outermost-to-root, so the legible cause is present
    /// even when `thiserror`'s one-layer `Display` would have dropped it.
    pub detail: String,
    /// The HTTP status code, when the error carried a structured one (some
    /// transport-layer status errors do; rig's provider-body errors usually do
    /// not — the status is folded into [`body`](Self::body)).
    pub status: Option<u16>,
    /// The provider's response body, when one was returned (a non-success HTTP
    /// status from the provider). Carries the provider's own error JSON/text.
    pub body: Option<String>,
}

impl ProviderFailure {
    /// Whether this failure is worth a bounded retry ([CR-060], [S-240]).
    ///
    /// Retry the faults that a second attempt can plausibly clear, and never the
    /// one it cannot:
    ///
    /// - [`Transport`](ProviderErrorKind::Transport) and
    ///   [`Other`](ProviderErrorKind::Other) → **retry**. A dropped connection,
    ///   a timeout, or an unclassified deserialization hiccup (rig's
    ///   `ApiResponse` `JsonError` on a 2xx gateway body) is exactly the
    ///   transient class a retry recovers from.
    /// - [`HttpStatus`](ProviderErrorKind::HttpStatus) → **retry on 429 / 5xx**
    ///   (rate-limit and server-side faults), and **retry when the status is
    ///   unknown** — rig folds some non-success statuses into the response body
    ///   ([`status`](Self::status) is then `None`), and a body-folded status is
    ///   most often a 429/5xx, so the safe default is to retry it. A *known*
    ///   non-retryable status (a 4xx client error such as 400/404) is **not**
    ///   retried — a second identical request would fail identically.
    /// - [`Auth`](ProviderErrorKind::Auth) → **never**. A rejected credential is
    ///   deterministic; retrying only wastes attempts and hammers the provider.
    ///
    /// On exhaustion the caller returns the *original* classified error
    /// unchanged, so downstream degradation ([FR-UI-28]) is unaffected.
    ///
    /// [CR-060]: ../../docs/requests/CR-060-chat-resilience-recoverable-faults.md
    /// [FR-UI-28]: ../../docs/specs/requirements/FR-UI-28.md
    pub fn is_retryable(&self) -> bool {
        match self.kind {
            ProviderErrorKind::Transport | ProviderErrorKind::Other => true,
            ProviderErrorKind::Auth => false,
            ProviderErrorKind::HttpStatus => match self.status {
                // A structured status: retry only the transient ones.
                Some(code) => code == 429 || (500..=599).contains(&code),
                // rig folded the status into the body (or it is otherwise
                // unknown) — retry, since the common body-folded case is a
                // 429/5xx.
                None => true,
            },
        }
    }
}

impl fmt::Display for ProviderFailure {
    /// `<kind label>[ (HTTP <status>)]: <source chain>` — a single honest line
    /// the Chat surface renders ([FR-UI-24]). The body, when present, is already
    /// part of `detail` (it is the provider error's `Display`), so it is not
    /// repeated.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.kind.label())?;
        if let Some(status) = self.status {
            write!(f, " (HTTP {status})")?;
        }
        write!(f, ": {}", self.detail)
    }
}

/// Classify a `rig` provider error into a [`ProviderFailure`], walking the whole
/// [`Error::source`] chain so the legible root cause and the transport-vs-status
/// distinction survive ([S-199], [FR-UI-24], [NFR-CC-04]).
///
/// Accepts any `&dyn Error` so both the planner's `PromptError` and a subagent's
/// [`CompletionError`] flow through one path.
pub fn classify_provider_error(err: &(dyn StdError + 'static)) -> ProviderFailure {
    let detail = render_source_chain(err);

    // The structured signal — the first `CompletionError`/`http_client::Error` in
    // the chain — drives the kind; the chain string carries the legible cause.
    let completion = find_in_chain::<CompletionError>(err);
    let http = find_in_chain::<HttpClientError>(err);

    let status = http.and_then(http_status);
    let body = completion.and_then(provider_body);

    let kind = classify_kind(completion, http, status, body.as_deref(), &detail);

    ProviderFailure {
        kind,
        detail,
        status,
        body,
    }
}

/// Decide the kind from the structured layers, falling back to body/chain
/// heuristics when rig folded the status into a string.
fn classify_kind(
    completion: Option<&CompletionError>,
    http: Option<&HttpClientError>,
    status: Option<u16>,
    body: Option<&str>,
    detail: &str,
) -> ProviderErrorKind {
    // A structured status is the strongest signal.
    if let Some(status) = status {
        return if is_auth_status(status) {
            ProviderErrorKind::Auth
        } else {
            ProviderErrorKind::HttpStatus
        };
    }

    // The provider answered with a non-success body (rig's `ProviderError`) — an
    // HTTP-status failure whose code lives in the body text; sub-classify auth.
    if let Some(body) = body {
        return if looks_like_auth(body) {
            ProviderErrorKind::Auth
        } else {
            ProviderErrorKind::HttpStatus
        };
    }

    // No HTTP response at all: a transport-layer `CompletionError::HttpError`, or
    // an `http_client` error that is not a status error.
    if matches!(completion, Some(CompletionError::HttpError(_))) || http.is_some() {
        return ProviderErrorKind::Transport;
    }

    // Last-resort heuristics on the rendered chain (e.g. a `RequestError` box that
    // wrapped a transport failure, so neither layer downcast above).
    if looks_like_auth(detail) {
        ProviderErrorKind::Auth
    } else if looks_like_transport(detail) {
        ProviderErrorKind::Transport
    } else {
        ProviderErrorKind::Other
    }
}

/// The provider's response body for a non-success status, if this is rig's
/// [`CompletionError::ProviderError`] (the body-carrying HTTP-status variant).
fn provider_body(err: &CompletionError) -> Option<String> {
    match err {
        CompletionError::ProviderError(body) if !body.trim().is_empty() => Some(body.clone()),
        _ => None,
    }
}

/// The structured HTTP status from an `http_client` status error, if present.
fn http_status(err: &HttpClientError) -> Option<u16> {
    match err {
        HttpClientError::InvalidStatusCode(code)
        | HttpClientError::InvalidStatusCodeWithMessage(code, _) => Some(code.as_u16()),
        _ => None,
    }
}

/// Whether a structured status is an authentication/authorization rejection.
fn is_auth_status(status: u16) -> bool {
    status == 401 || status == 403
}

/// Whether a body/chain reads as an auth rejection — used when rig folded the
/// status into a string and the numeric code is unavailable structurally.
fn looks_like_auth(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    // Provider-auth-specific phrasings only. `"permission denied"` is deliberately
    // excluded: it is an atypical wording for a provider auth rejection and would
    // risk misclassifying an unrelated message — the typed 401/403 path and these
    // markers cover the real cases (review-fix S-199).
    const MARKERS: [&str; 8] = [
        "unauthorized",
        "authentication",
        "invalid api key",
        "invalid_api_key",
        "incorrect api key",
        "no auth credentials",
        "\"code\":401",
        "\"code\":403",
    ];
    MARKERS.iter().any(|m| lower.contains(m))
}

/// Whether a rendered chain reads as a transport failure — the fallback when no
/// structured layer downcast (e.g. a boxed `RequestError`).
fn looks_like_transport(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    const MARKERS: [&str; 7] = [
        "connection refused",
        "dns error",
        "timed out",
        "error sending request",
        "tcp connect",
        "tls",
        "certificate",
    ];
    MARKERS.iter().any(|m| lower.contains(m))
}

/// Render the **full** [`Error::source`] chain into one legible line: each
/// layer's `Display`, joined `: `, outermost first. A layer is appended only when
/// it adds text the previous layer's `Display` did not already contain — so
/// `thiserror`'s `{0}` nesting (where the outer `Display` already embeds the
/// inner one) does not duplicate, while a deeper `source()` that reveals a new
/// cause (the OS error under a `reqwest` error) is kept.
fn render_source_chain(err: &(dyn StdError + 'static)) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut current: Option<&(dyn StdError + 'static)> = Some(err);
    while let Some(e) = current {
        let rendered = e.to_string();
        let trimmed = rendered.trim();
        if !trimmed.is_empty()
            && parts
                .last()
                .is_none_or(|last: &String| !last.contains(trimmed))
        {
            parts.push(trimmed.to_string());
        }
        current = e.source();
    }
    if parts.is_empty() {
        "an unknown provider error occurred".to_string()
    } else {
        parts.join(": ")
    }
}

/// The first error of concrete type `T` in the [`Error::source`] chain, if any.
fn find_in_chain<'a, T: StdError + 'static>(err: &'a (dyn StdError + 'static)) -> Option<&'a T> {
    let mut current: Option<&'a (dyn StdError + 'static)> = Some(err);
    while let Some(e) = current {
        if let Some(found) = e.downcast_ref::<T>() {
            return Some(found);
        }
        current = e.source();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // A small error type with a controllable source, to prove the chain walk
    // recovers a cause that the outer `Display` does not embed (the reqwest →
    // OS-error shape, without depending on a live socket).
    #[derive(Debug)]
    struct Layered {
        display: String,
        source: Option<Box<dyn StdError + 'static>>,
    }
    impl fmt::Display for Layered {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(&self.display)
        }
    }
    impl StdError for Layered {
        fn source(&self) -> Option<&(dyn StdError + 'static)> {
            self.source.as_deref()
        }
    }

    #[test]
    fn renders_the_full_chain_recovering_a_hidden_root_cause() {
        // Outer `Display` stops at "error sending request"; the OS cause is two
        // hops down and would be lost by a bare `to_string()`.
        let root = Layered {
            display: "Connection refused (os error 61)".to_string(),
            source: None,
        };
        let mid = Layered {
            display: "client error (Connect)".to_string(),
            source: Some(Box::new(root)),
        };
        let outer = Layered {
            display: "error sending request for url (https://host/v1)".to_string(),
            source: Some(Box::new(mid)),
        };
        let chain = render_source_chain(&outer);
        assert!(chain.contains("error sending request"));
        assert!(
            chain.contains("Connection refused (os error 61)"),
            "the chain walk must recover the hidden root cause: {chain}"
        );
    }

    #[test]
    fn does_not_duplicate_thiserror_nested_display() {
        // The outer `Display` already embeds the inner one (the `{0}` idiom).
        let inner = Layered {
            display: "ProviderError: rate limited".to_string(),
            source: None,
        };
        let outer = Layered {
            display: "CompletionError: ProviderError: rate limited".to_string(),
            source: Some(Box::new(inner)),
        };
        let chain = render_source_chain(&outer);
        assert_eq!(chain, "CompletionError: ProviderError: rate limited");
    }

    #[test]
    fn transport_error_is_classified_from_the_http_error_variant() {
        let err = CompletionError::HttpError(HttpClientError::StreamEnded);
        let failure = classify_provider_error(&err);
        assert_eq!(failure.kind, ProviderErrorKind::Transport);
        assert!(failure.status.is_none());
        assert!(failure.body.is_none());
    }

    #[test]
    fn provider_body_is_classified_as_http_status_and_carried() {
        let err = CompletionError::ProviderError(
            "{\"error\":{\"message\":\"rate limit exceeded\",\"type\":\"rate_limit\"}}".to_string(),
        );
        let failure = classify_provider_error(&err);
        assert_eq!(failure.kind, ProviderErrorKind::HttpStatus);
        assert_eq!(
            failure.body.as_deref(),
            Some("{\"error\":{\"message\":\"rate limit exceeded\",\"type\":\"rate_limit\"}}")
        );
        // The body is part of the rendered detail, so the surface shows the cause.
        assert!(failure.detail.contains("rate limit exceeded"));
    }

    #[test]
    fn an_auth_body_is_classified_as_auth_not_generic_http_status() {
        let err = CompletionError::ProviderError(
            "{\"error\":{\"message\":\"Incorrect API key provided\",\"code\":401}}".to_string(),
        );
        let failure = classify_provider_error(&err);
        assert_eq!(failure.kind, ProviderErrorKind::Auth);
    }

    #[test]
    fn a_structured_http_status_drives_the_kind_and_renders_in_display() {
        // The path rig takes when a transport surfaces a typed status (vs folding it
        // into a `ProviderError` body): 401/403 → Auth, other non-2xx → HttpStatus,
        // and the numeric code is carried and rendered as `(HTTP n)` (S-199 review).
        let unauthorized = CompletionError::HttpError(
            HttpClientError::InvalidStatusCodeWithMessage(
                http::StatusCode::UNAUTHORIZED,
                "missing bearer token".to_string(),
            ),
        );
        let failure = classify_provider_error(&unauthorized);
        assert_eq!(failure.kind, ProviderErrorKind::Auth);
        assert_eq!(failure.status, Some(401));
        assert!(
            failure.to_string().contains("(HTTP 401)"),
            "the structured status is rendered: {failure}"
        );

        let server_error =
            CompletionError::HttpError(HttpClientError::InvalidStatusCode(
                http::StatusCode::INTERNAL_SERVER_ERROR,
            ));
        let failure = classify_provider_error(&server_error);
        assert_eq!(failure.kind, ProviderErrorKind::HttpStatus);
        assert_eq!(failure.status, Some(500));
    }

    #[test]
    fn display_leads_with_the_kind_label() {
        let err = CompletionError::ProviderError("upstream is on fire".to_string());
        let rendered = classify_provider_error(&err).to_string();
        assert!(
            rendered.starts_with("provider returned an error response"),
            "the kind label leads the message: {rendered}"
        );
        assert!(rendered.contains("upstream is on fire"));
    }

    #[test]
    fn retryability_is_by_classified_kind() {
        // A helper to build a failure of a given kind/status directly.
        let failure = |kind, status| ProviderFailure {
            kind,
            detail: String::new(),
            status,
            body: None,
        };

        // Transport and Other are always retryable (transient by nature).
        assert!(failure(ProviderErrorKind::Transport, None).is_retryable());
        assert!(failure(ProviderErrorKind::Other, None).is_retryable());

        // Auth is never retryable (a rejected credential is deterministic).
        assert!(!failure(ProviderErrorKind::Auth, Some(401)).is_retryable());
        assert!(!failure(ProviderErrorKind::Auth, None).is_retryable());

        // HttpStatus: 429 and any 5xx retry; a known 4xx does not.
        assert!(failure(ProviderErrorKind::HttpStatus, Some(429)).is_retryable());
        assert!(failure(ProviderErrorKind::HttpStatus, Some(500)).is_retryable());
        assert!(failure(ProviderErrorKind::HttpStatus, Some(503)).is_retryable());
        assert!(!failure(ProviderErrorKind::HttpStatus, Some(400)).is_retryable());
        assert!(!failure(ProviderErrorKind::HttpStatus, Some(404)).is_retryable());

        // HttpStatus with no structured status (rig folded it into the body) —
        // the body-folded/unknown case retries.
        assert!(failure(ProviderErrorKind::HttpStatus, None).is_retryable());
    }

    #[test]
    fn a_body_folded_provider_error_is_retryable() {
        // rig's `ProviderError` carries a non-success body with no structured
        // status — the common transient case (a 429/5xx text) must retry.
        let err = CompletionError::ProviderError(
            "{\"error\":{\"message\":\"rate limit exceeded\"}}".to_string(),
        );
        assert!(classify_provider_error(&err).is_retryable());
    }

    #[test]
    fn an_auth_body_is_not_retryable() {
        let err = CompletionError::ProviderError(
            "{\"error\":{\"message\":\"Incorrect API key provided\",\"code\":401}}".to_string(),
        );
        assert!(!classify_provider_error(&err).is_retryable());
    }
}
