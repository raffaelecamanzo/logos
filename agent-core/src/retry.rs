//! A transparent retry-with-backoff decorator over a `rig` [`CompletionModel`]
//! ([CR-060], [S-240], [ADR-42]).
//!
//! [`RetryingModel`] wraps any `rig` completion model and re-implements the
//! [`CompletionModel`] trait by delegation: each `completion`/`stream` call is
//! re-issued, up to a bounded number of times, when it fails with a *transient*
//! provider fault. It is inserted in **both** `agent-core` model constructors
//! ([`anthropic_completion_model`](crate::anthropic_completion_model),
//! [`openai_compatible_completion_model`](crate::openai_compatible_completion_model)),
//! so the chat-agent and the wiki-agent inherit retry transparently with no
//! orchestration change.
//!
//! # What is retried
//! Retryability is decided by the honest classification the substrate already
//! owns ([`classify_provider_error`] → [`ProviderFailure::is_retryable`]):
//! transport failures, HTTP 429/5xx, and unclassified deserialization hiccups
//! retry; a rejected credential ([`Auth`](crate::ProviderErrorKind::Auth)) is
//! **never** retried (a single attempt). On exhaustion the **original**
//! classified error is returned unchanged, so downstream degradation
//! ([FR-UI-28]) sees exactly the fault it would have seen without the decorator.
//!
//! # Bounded backoff + jitter
//! Between attempts the caller waits `base_delay · 2^(attempt-1)` with jitter —
//! bounded exponential backoff so a burst of retries neither hammers the
//! provider nor synchronizes across callers. The jitter source is
//! dependency-free (a process-seeded hash), so this ui-only crate adds no new
//! dependency edge and the no-networking carve-out ([NFR-SE-01]) is untouched.
//!
//! [CR-060]: ../../docs/requests/CR-060-chat-resilience-recoverable-faults.md
//! [S-240]: ../../docs/planning/journal.md#s-240-provider-call-retry-with-backoff-in-agent-core
//! [ADR-42]: ../../docs/specs/architecture/decisions/ADR-42.md
//! [FR-UI-28]: ../../docs/specs/requirements/FR-UI-28.md
//! [NFR-SE-01]: ../../docs/specs/requirements/NFR-SE-01.md

use std::error::Error as StdError;
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use rig_core::completion::{
    CompletionError, CompletionModel, CompletionRequest, CompletionResponse,
};
use rig_core::streaming::StreamingCompletionResponse;

use crate::provider_error::classify_provider_error;

/// Default retry count — attempts **beyond** the first ([CR-060], [FR-CF-06]).
/// `2` means up to three total tries (the initial call plus two retries).
pub const DEFAULT_MAX_PROVIDER_RETRIES: u32 = 2;

/// Default base backoff delay, in milliseconds ([CR-060], [FR-CF-06]).
pub const DEFAULT_PROVIDER_RETRY_BASE_MS: u64 = 200;

/// The upper bound on the exponential shift, so `2^shift` neither overflows nor
/// produces an absurd delay even if a caller (bypassing config validation)
/// constructs a policy with a very large retry count.
const MAX_BACKOFF_SHIFT: u32 = 16;

/// The bounded retry-with-backoff policy a [`RetryingModel`] applies
/// ([CR-060], [S-240]).
///
/// `max_retries` is the number of attempts **beyond** the first; `0` disables
/// retries entirely (a single attempt — the pre-CR-060 behavior). `base_delay`
/// is the first backoff interval, doubled each subsequent retry (with jitter).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    max_retries: u32,
    base_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_PROVIDER_RETRIES, DEFAULT_PROVIDER_RETRY_BASE_MS)
    }
}

impl RetryPolicy {
    /// Build a policy from a retry count and a base backoff in milliseconds.
    ///
    /// `base_ms` is clamped to a floor of `1` — a zero base delay is rejected at
    /// config load ([`ChatConfig::validate`](../../logos_core/config/struct.ChatConfig.html)),
    /// and this floor keeps a directly-constructed policy from busy-looping.
    pub fn new(max_retries: u32, base_ms: u64) -> Self {
        Self {
            max_retries,
            base_delay: Duration::from_millis(base_ms.max(1)),
        }
    }

    /// A policy that never retries — a single attempt (retries disabled).
    pub fn disabled() -> Self {
        Self::new(0, DEFAULT_PROVIDER_RETRY_BASE_MS)
    }

    /// The number of attempts beyond the first.
    pub fn max_retries(&self) -> u32 {
        self.max_retries
    }

    /// The base backoff delay (the first retry interval, before jitter).
    pub fn base_delay(&self) -> Duration {
        self.base_delay
    }

    /// Given that a call failed with `err` after `prior_retries` retries already
    /// spent, the backoff delay to wait before trying again — or `None` when the
    /// caller should stop and surface the original error.
    ///
    /// `None` means either the retry budget is exhausted (`prior_retries >=
    /// max_retries`) or the fault is not retryable ([`ProviderFailure::is_retryable`]);
    /// in both cases the caller returns the original classified error unchanged.
    pub fn retry_after(
        &self,
        prior_retries: u32,
        err: &(dyn StdError + 'static),
    ) -> Option<Duration> {
        if prior_retries >= self.max_retries {
            return None;
        }
        if !classify_provider_error(err).is_retryable() {
            return None;
        }
        Some(self.backoff_delay(prior_retries + 1))
    }

    /// The deterministic exponential ceiling for a 1-indexed `attempt`, before
    /// jitter: `base_delay · 2^(attempt-1)`, saturating and shift-capped.
    fn exponential_ceiling(&self, attempt: u32) -> Duration {
        let shift = attempt.saturating_sub(1).min(MAX_BACKOFF_SHIFT);
        self.base_delay.saturating_mul(1u32 << shift)
    }

    /// The backoff delay for a 1-indexed `attempt` with **equal jitter**: half
    /// the exponential ceiling plus a random fraction of the other half. Equal
    /// jitter keeps a floor (never a zero-wait busy retry) while still
    /// desynchronizing concurrent callers.
    fn backoff_delay(&self, attempt: u32) -> Duration {
        let ceiling = self.exponential_ceiling(attempt);
        let half = ceiling / 2;
        half + half.mul_f64(jitter_fraction())
    }
}

/// A process-seeded pseudo-random fraction in `[0.0, 1.0)` for backoff jitter.
///
/// Dependency-free: each call seeds [`std::collections::hash_map::RandomState`]
/// (randomly seeded per process) and folds in a monotonic sequence number, so
/// successive calls differ. Jitter needs no cryptographic quality — only that
/// concurrent callers do not retry in lockstep — so this is sufficient and adds
/// no dependency to the no-networking-guarded tree ([NFR-SE-01]).
fn jitter_fraction() -> f64 {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u64(seq);
    let bits = hasher.finish();
    // Map the top 53 bits to [0, 1) — the standard f64 uniform construction.
    ((bits >> 11) as f64) / ((1u64 << 53) as f64)
}

/// A `rig` [`CompletionModel`] wrapped with a bounded retry-with-backoff policy
/// ([CR-060], [S-240]).
///
/// Delegates every trait method to the inner model, re-issuing a failed
/// `completion`/`stream` call for transient faults per [`RetryPolicy`]. Cloning
/// is cheap (the inner model is itself `Clone`, as the trait requires) and the
/// policy is `Copy`.
#[derive(Debug, Clone)]
pub struct RetryingModel<M> {
    inner: M,
    policy: RetryPolicy,
}

impl<M> RetryingModel<M> {
    /// Wrap `inner` with `policy`.
    pub fn new(inner: M, policy: RetryPolicy) -> Self {
        Self { inner, policy }
    }

    /// The retry policy in effect.
    pub fn policy(&self) -> RetryPolicy {
        self.policy
    }

    /// A borrow of the wrapped model.
    pub fn inner(&self) -> &M {
        &self.inner
    }

    /// Drive one provider call under the bounded retry policy: invoke `call`,
    /// and on a retryable transient fault wait the backoff and re-invoke, up to
    /// the policy's budget; return the *original* error on exhaustion. Shared by
    /// [`completion`](CompletionModel::completion) and
    /// [`stream`](CompletionModel::stream) so the retry discipline has a single
    /// source of truth (no drift between two hand-copied loops).
    ///
    /// `call` re-issues the (cloned) request each attempt; it captures only
    /// shared borrows of `self` and the request, so a plain `FnMut() -> Fut` (no
    /// lending closure) is sufficient and awaiting each future to completion
    /// before the next call keeps at most one in flight.
    async fn run_with_retry<T, Fut>(
        &self,
        mut call: impl FnMut() -> Fut,
    ) -> Result<T, CompletionError>
    where
        Fut: std::future::Future<Output = Result<T, CompletionError>>,
    {
        let mut prior_retries = 0u32;
        loop {
            match call().await {
                Ok(value) => return Ok(value),
                Err(err) => match self.policy.retry_after(prior_retries, &err) {
                    Some(delay) => {
                        tokio::time::sleep(delay).await;
                        prior_retries += 1;
                    }
                    None => return Err(err),
                },
            }
        }
    }
}

impl<M: CompletionModel> CompletionModel for RetryingModel<M> {
    type Response = M::Response;
    type StreamingResponse = M::StreamingResponse;
    type Client = M::Client;

    /// Construct via the inner model's `make`, with the default retry policy.
    ///
    /// The production path never uses this — the two `agent-core` constructors
    /// wrap an already-built model with a config-resolved [`RetryPolicy`] — but
    /// the trait requires it, so it yields a sensible default-policy decorator.
    fn make(client: &Self::Client, model: impl Into<String>) -> Self {
        Self::new(M::make(client, model), RetryPolicy::default())
    }

    /// A completion call with bounded retry: re-issue a cloned request for
    /// transient faults, waiting the policy's backoff between attempts, and
    /// return the original error on exhaustion.
    async fn completion(
        &self,
        request: CompletionRequest,
    ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
        self.run_with_retry(|| self.inner.completion(request.clone()))
            .await
    }

    /// A streaming call with the same bounded retry discipline applied to the
    /// stream *setup* (a fault surfaced when opening the stream). Once the stream
    /// is established, per-chunk faults flow through untouched — a partially
    /// emitted answer is not cleanly re-issuable.
    async fn stream(
        &self,
        request: CompletionRequest,
    ) -> Result<StreamingCompletionResponse<Self::StreamingResponse>, CompletionError> {
        self.run_with_retry(|| self.inner.stream(request.clone()))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MockCompletionModel, MockTurn};
    use rig_core::completion::AssistantContent;
    use rig_core::streaming::{RawStreamingChoice, StreamingResult};
    use rig_core::OneOrMany;

    /// A scripted mock that fails the first `fail_first` calls with a given error,
    /// then succeeds with a text turn — the "fail N times then succeed" harness
    /// the story's testing plan calls for.
    ///
    /// It counts every call so a test can assert exactly how many attempts the
    /// decorator made.
    #[derive(Clone)]
    struct ScriptedModel {
        state: std::sync::Arc<ScriptState>,
    }

    struct ScriptState {
        error: CompletionError,
        fail_first: u32,
        calls: std::sync::atomic::AtomicU32,
    }

    impl ScriptedModel {
        fn new(error: CompletionError, fail_first: u32) -> Self {
            Self {
                state: std::sync::Arc::new(ScriptState {
                    error,
                    fail_first,
                    calls: std::sync::atomic::AtomicU32::new(0),
                }),
            }
        }

        fn calls(&self) -> u32 {
            self.state.calls.load(Ordering::SeqCst)
        }
    }

    impl CompletionModel for ScriptedModel {
        type Response = crate::MockRawResponse;
        type StreamingResponse = crate::MockRawResponse;
        type Client = ();

        fn make(_client: &Self::Client, _model: impl Into<String>) -> Self {
            unreachable!("the scripted mock is constructed directly")
        }

        async fn completion(
            &self,
            _request: CompletionRequest,
        ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
            let n = self.state.calls.fetch_add(1, Ordering::SeqCst);
            if n < self.state.fail_first {
                return Err(clone_error(&self.state.error));
            }
            Ok(CompletionResponse {
                choice: OneOrMany::one(AssistantContent::text("recovered")),
                usage: rig_core::completion::Usage::new(),
                raw_response: crate::MockRawResponse::default(),
                message_id: None,
            })
        }

        async fn stream(
            &self,
            _request: CompletionRequest,
        ) -> Result<
            StreamingCompletionResponse<Self::StreamingResponse>,
            CompletionError,
        > {
            // Same fail-N-then-succeed script as `completion`, so the retry loop
            // shared by both methods is exercised on the stream *setup* path too.
            let n = self.state.calls.fetch_add(1, Ordering::SeqCst);
            if n < self.state.fail_first {
                return Err(clone_error(&self.state.error));
            }
            let chunks: Vec<
                Result<RawStreamingChoice<crate::MockRawResponse>, CompletionError>,
            > = vec![
                Ok(RawStreamingChoice::Message("recovered".to_string())),
                Ok(RawStreamingChoice::FinalResponse(
                    crate::MockRawResponse::default(),
                )),
            ];
            let stream: StreamingResult<crate::MockRawResponse> =
                Box::pin(futures::stream::iter(chunks));
            Ok(StreamingCompletionResponse::stream(stream))
        }
    }

    /// `CompletionError` is not `Clone`; reproduce the variants the tests use.
    fn clone_error(err: &CompletionError) -> CompletionError {
        match err {
            CompletionError::ProviderError(body) => {
                CompletionError::ProviderError(body.clone())
            }
            CompletionError::ResponseError(msg) => {
                CompletionError::ResponseError(msg.clone())
            }
            other => CompletionError::ResponseError(other.to_string()),
        }
    }

    /// A tiny base delay so the backoff sleeps are negligible in tests.
    fn fast_policy(max_retries: u32) -> RetryPolicy {
        RetryPolicy::new(max_retries, 1)
    }

    /// A transient fault classified as retryable — a body-folded provider error
    /// (no structured status), which [`ProviderFailure::is_retryable`] retries.
    fn transient_error() -> CompletionError {
        CompletionError::ProviderError(
            "{\"error\":{\"message\":\"upstream temporarily unavailable\"}}".to_string(),
        )
    }

    /// A deserialization-shaped fault (the `ApiResponse` JsonError on a 2xx body)
    /// classified as `Other` — retryable.
    fn deser_error() -> CompletionError {
        CompletionError::ResponseError(
            "failed to deserialize the provider response".to_string(),
        )
    }

    /// An auth fault — a body naming an invalid key, classified as `Auth`.
    fn auth_error() -> CompletionError {
        CompletionError::ProviderError(
            "{\"error\":{\"message\":\"Incorrect API key provided\",\"code\":401}}".to_string(),
        )
    }

    #[tokio::test]
    async fn recovers_within_the_retry_budget() {
        // Two transient failures then success, with a budget of two retries → the
        // third attempt succeeds and the recovered response is returned.
        let inner = ScriptedModel::new(transient_error(), 2);
        let model = RetryingModel::new(inner.clone(), fast_policy(2));
        let response = model.completion(request()).await.expect("recovers");
        assert!(matches!(
            response.choice.first(),
            AssistantContent::Text(_)
        ));
        assert_eq!(inner.calls(), 3, "initial attempt + two retries");
    }

    #[tokio::test]
    async fn an_unclassified_deserialization_hiccup_is_retried() {
        let inner = ScriptedModel::new(deser_error(), 1);
        let model = RetryingModel::new(inner.clone(), fast_policy(2));
        assert!(model.completion(request()).await.is_ok());
        assert_eq!(inner.calls(), 2, "one failure, one retry, then success");
    }

    #[tokio::test]
    async fn auth_failures_are_never_retried() {
        // Even with a generous budget, an auth fault is a single attempt.
        let inner = ScriptedModel::new(auth_error(), 5);
        let model = RetryingModel::new(inner.clone(), fast_policy(3));
        let err = model.completion(request()).await.expect_err("auth is fatal");
        assert!(matches!(err, CompletionError::ProviderError(_)));
        assert_eq!(inner.calls(), 1, "auth is never retried");
    }

    #[tokio::test]
    async fn exhaustion_returns_the_original_error_unchanged() {
        // More failures than the budget can cover → the loop gives up and returns
        // the last (identical) provider error, unchanged, for downstream
        // degradation to classify itself.
        let inner = ScriptedModel::new(transient_error(), 10);
        let model = RetryingModel::new(inner.clone(), fast_policy(2));
        let err = model
            .completion(request())
            .await
            .expect_err("budget exhausts");
        match err {
            CompletionError::ProviderError(body) => {
                assert!(
                    body.contains("upstream temporarily unavailable"),
                    "the original error body survives: {body}"
                );
            }
            other => panic!("expected the original ProviderError, got {other:?}"),
        }
        assert_eq!(inner.calls(), 3, "initial attempt + two retries, then give up");
    }

    #[tokio::test]
    async fn a_zero_retry_policy_is_a_single_attempt() {
        let inner = ScriptedModel::new(transient_error(), 10);
        let model = RetryingModel::new(inner.clone(), RetryPolicy::disabled());
        assert!(model.completion(request()).await.is_err());
        assert_eq!(inner.calls(), 1, "disabled → no retry");
    }

    #[tokio::test]
    async fn a_first_try_success_makes_no_retry() {
        let inner = ScriptedModel::new(transient_error(), 0);
        let model = RetryingModel::new(inner.clone(), fast_policy(2));
        assert!(model.completion(request()).await.is_ok());
        assert_eq!(inner.calls(), 1, "success on the first attempt, no retry");
    }

    #[tokio::test]
    async fn stream_setup_recovers_within_the_retry_budget() {
        // The stream() path shares the retry loop with completion(): two transient
        // setup failures then success, with a budget of two retries, recovers.
        let inner = ScriptedModel::new(transient_error(), 2);
        let model = RetryingModel::new(inner.clone(), fast_policy(2));
        assert!(
            model.stream(request()).await.is_ok(),
            "the stream setup recovers on the third attempt"
        );
        assert_eq!(inner.calls(), 3, "initial attempt + two retries");
    }

    #[tokio::test]
    async fn stream_setup_exhaustion_returns_the_original_error() {
        let inner = ScriptedModel::new(transient_error(), 10);
        let model = RetryingModel::new(inner.clone(), fast_policy(2));
        // `StreamingCompletionResponse` is not `Debug`, so match rather than
        // `expect_err`; assert the original provider error surfaces on exhaustion.
        match model.stream(request()).await {
            Err(CompletionError::ProviderError(body)) => {
                assert!(body.contains("upstream temporarily unavailable"));
            }
            Err(other) => panic!("expected the original ProviderError, got {other:?}"),
            Ok(_) => panic!("stream setup should not have succeeded"),
        }
        assert_eq!(inner.calls(), 3, "initial attempt + two retries, then give up");
    }

    #[tokio::test]
    async fn stream_setup_never_retries_auth() {
        let inner = ScriptedModel::new(auth_error(), 5);
        let model = RetryingModel::new(inner.clone(), fast_policy(3));
        assert!(model.stream(request()).await.is_err());
        assert_eq!(inner.calls(), 1, "auth is never retried on the stream path");
    }

    #[tokio::test]
    async fn the_real_mock_model_wraps_and_delegates() {
        // The decorator is transparent over the first-party mock: a scripted text
        // turn round-trips through the wrapper unchanged.
        let mock = MockCompletionModel::new([MockTurn::text("hello")]);
        let model = RetryingModel::new(mock.clone(), RetryPolicy::default());
        let response = model.completion(request()).await.expect("delegates");
        assert!(matches!(
            response.choice.first(),
            AssistantContent::Text(_)
        ));
        assert_eq!(mock.request_count(), 1);
    }

    #[test]
    fn retry_after_reports_the_decision_by_kind_and_budget() {
        let policy = fast_policy(2);
        // Retryable within budget → Some.
        assert!(policy.retry_after(0, &transient_error()).is_some());
        assert!(policy.retry_after(1, &transient_error()).is_some());
        // Budget exhausted → None even for a retryable fault.
        assert!(policy.retry_after(2, &transient_error()).is_none());
        // Non-retryable (auth) → None even with budget remaining.
        assert!(policy.retry_after(0, &auth_error()).is_none());
    }

    #[test]
    fn backoff_grows_exponentially_and_stays_bounded() {
        let policy = RetryPolicy::new(5, 100);
        // Equal jitter: the delay is in [ceiling/2, ceiling]. Ceiling doubles per
        // attempt (100 → 200 → 400 ms), so each attempt's floor exceeds the
        // previous attempt's floor.
        let d1 = policy.backoff_delay(1);
        let d2 = policy.backoff_delay(2);
        let d3 = policy.backoff_delay(3);
        assert!(d1 >= Duration::from_millis(50) && d1 <= Duration::from_millis(100));
        assert!(d2 >= Duration::from_millis(100) && d2 <= Duration::from_millis(200));
        assert!(d3 >= Duration::from_millis(200) && d3 <= Duration::from_millis(400));
    }

    #[test]
    fn base_ms_is_floored_to_avoid_a_zero_wait() {
        // A directly-constructed zero base delay is floored to 1ms (config load
        // rejects 0 outright; this is the defense-in-depth floor).
        let policy = RetryPolicy::new(2, 0);
        assert_eq!(policy.base_delay(), Duration::from_millis(1));
    }

    fn request() -> CompletionRequest {
        CompletionRequest {
            model: None,
            preamble: None,
            chat_history: OneOrMany::one(rig_core::message::Message::user("hi")),
            documents: Vec::new(),
            tools: Vec::new(),
            temperature: None,
            max_tokens: None,
            tool_choice: None,
            additional_params: None,
            output_schema: None,
        }
    }
}
