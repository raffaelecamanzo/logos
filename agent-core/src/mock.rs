//! A mock `rig` [`CompletionModel`] for offline tests (ADR-41).
//!
//! ADR-41 is explicit that **Logos builds the mock provider** — a `rig`
//! `CompletionModel` implementation returning scripted assistant/tool-call
//! responses. Owning it here (rather than leaning on `rig`'s internal
//! `test-utils` feature) means the chat-agent and wiki-agent test suites get a
//! stable, first-party mock under the normal `agent-core` API, and the
//! ui-build zero-real-egress proof ([UAT-UI-07], NFR-SE-07) drives a full
//! round-trip through `rig`'s machinery with **no** HTTP client involved.
//!
//! The mock is cloneable and consumes one scripted [`MockTurn`] per
//! completion/stream call; when the script is exhausted it returns an honest
//! provider error rather than fabricating a response (NFR-CC-04). It records a
//! request count so tests can assert the mock — not a real provider — served
//! the turn.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use rig_core::completion::{
    AssistantContent, CompletionError, CompletionModel, CompletionRequest, CompletionResponse,
    GetTokenUsage, Usage,
};
use rig_core::message::{ToolCall, ToolFunction};
use rig_core::streaming::{RawStreamingChoice, StreamingCompletionResponse, StreamingResult};
use rig_core::OneOrMany;
use serde::{Deserialize, Serialize};

/// One scripted turn the mock will return.
#[derive(Debug, Clone)]
pub enum MockTurn {
    /// A plain assistant text message.
    Text(String),
    /// A plain assistant text message delivered as several streamed chunks: each
    /// `String` is one `stream()` delta (token-by-token), while `completion()`
    /// returns them concatenated. Lets a test exercise multi-delta answer
    /// streaming ([FR-UI-19]) deterministically offline.
    TextChunks(Vec<String>),
    /// A tool call: `(call_id, tool_name, arguments)`.
    ToolCall {
        /// Provider-style tool-call id.
        id: String,
        /// The tool name the planner/agent should dispatch.
        name: String,
        /// The JSON arguments for the call.
        arguments: serde_json::Value,
    },
    /// An honest provider error (e.g. to exercise failure handling).
    Error(String),
    /// A request that never resolves — a stalled/dead-air provider connection. The
    /// completion/stream call records the request, then awaits forever, so a test can
    /// exercise a caller's liveness timeout (e.g. the wiki-agent's per-page synthesis
    /// timeout, S-222/CR-056) deterministically offline.
    Hang,
}

impl MockTurn {
    /// Convenience constructor for a text turn.
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text(text.into())
    }

    /// Convenience constructor for a multi-chunk streamed text turn — one delta
    /// per item under `stream()`, concatenated under `completion()`.
    pub fn text_chunks<I, S>(chunks: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self::TextChunks(chunks.into_iter().map(Into::into).collect())
    }

    /// Convenience constructor for a tool-call turn.
    pub fn tool_call(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: serde_json::Value,
    ) -> Self {
        Self::ToolCall {
            id: id.into(),
            name: name.into(),
            arguments,
        }
    }

    /// The assistant content for a non-error turn, or the error message.
    fn into_content(self) -> Result<AssistantContent, String> {
        match self {
            MockTurn::Text(text) => Ok(AssistantContent::text(text)),
            MockTurn::TextChunks(chunks) => Ok(AssistantContent::text(chunks.concat())),
            MockTurn::ToolCall {
                id,
                name,
                arguments,
            } => Ok(AssistantContent::ToolCall(ToolCall::new(
                id,
                ToolFunction::new(name, arguments),
            ))),
            MockTurn::Error(message) => Err(message),
            // Intercepted before this point by `completion`/`stream` (which await
            // forever); it never carries content.
            MockTurn::Hang => unreachable!("MockTurn::Hang is handled before into_content"),
        }
    }
}

/// The mock's raw-response type. `rig`'s `CompletionModel` requires the raw
/// `Response`/`StreamingResponse` associated types to be `Serialize +
/// DeserializeOwned` (and the streaming one to report token usage); this
/// carries a zero-valued [`Usage`] — the documented sentinel for "no provider
/// metrics".
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MockRawResponse {
    /// Token usage (always zero — the mock makes no real call).
    pub usage: Usage,
}

impl GetTokenUsage for MockRawResponse {
    fn token_usage(&self) -> Usage {
        self.usage
    }
}

#[derive(Default)]
struct MockState {
    turns: Mutex<VecDeque<MockTurn>>,
    requests: AtomicUsize,
}

/// A cloneable, scripted [`CompletionModel`] for offline tests.
#[derive(Clone, Default)]
pub struct MockCompletionModel {
    state: Arc<MockState>,
}

impl MockCompletionModel {
    /// Build a mock from a sequence of scripted turns; each completion/stream
    /// call consumes the next one in order.
    pub fn new(turns: impl IntoIterator<Item = MockTurn>) -> Self {
        Self {
            state: Arc::new(MockState {
                turns: Mutex::new(turns.into_iter().collect()),
                requests: AtomicUsize::new(0),
            }),
        }
    }

    /// Build a mock that returns a single text completion.
    pub fn text(text: impl Into<String>) -> Self {
        Self::new([MockTurn::text(text)])
    }

    /// The number of completion/stream requests served so far. Tests assert
    /// this is non-zero to prove the mock — not a real provider — handled the
    /// turn.
    pub fn request_count(&self) -> usize {
        self.state.requests.load(Ordering::SeqCst)
    }

    fn next_turn(&self) -> Option<MockTurn> {
        self.state
            .turns
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .pop_front()
    }

    fn record_request(&self) {
        self.state.requests.fetch_add(1, Ordering::SeqCst);
    }
}

impl CompletionModel for MockCompletionModel {
    type Response = MockRawResponse;
    type StreamingResponse = MockRawResponse;
    type Client = ();

    fn make(_client: &Self::Client, _model: impl Into<String>) -> Self {
        Self::default()
    }

    async fn completion(
        &self,
        _request: CompletionRequest,
    ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
        self.record_request();
        let Some(turn) = self.next_turn() else {
            return Err(CompletionError::ProviderError(
                "mock completion model exhausted: no scripted turn remaining".to_string(),
            ));
        };
        if let MockTurn::Hang = turn {
            // A stalled/dead-air provider: never resolve, so the caller's liveness
            // timeout is what ends the wait.
            std::future::pending::<()>().await;
        }
        let content = turn
            .into_content()
            .map_err(CompletionError::ProviderError)?;
        Ok(CompletionResponse {
            choice: OneOrMany::one(content),
            usage: Usage::new(),
            raw_response: MockRawResponse::default(),
            message_id: None,
        })
    }

    async fn stream(
        &self,
        _request: CompletionRequest,
    ) -> Result<StreamingCompletionResponse<Self::StreamingResponse>, CompletionError> {
        self.record_request();
        let Some(turn) = self.next_turn() else {
            return Err(CompletionError::ProviderError(
                "mock completion model exhausted: no scripted streaming turn remaining".to_string(),
            ));
        };
        if let MockTurn::Hang = turn {
            // A stalled/dead-air provider: never resolve, so the caller's liveness
            // timeout is what ends the wait.
            std::future::pending::<()>().await;
        }

        // Map the scripted turn to a sequence of streamed chunks terminated by
        // the final-response marker (so the aggregated response carries usage).
        let chunks: Vec<Result<RawStreamingChoice<MockRawResponse>, CompletionError>> = match turn {
            MockTurn::Text(text) => vec![
                Ok(RawStreamingChoice::Message(text)),
                Ok(RawStreamingChoice::FinalResponse(MockRawResponse::default())),
            ],
            // One streamed delta per chunk, then the final-response marker — the
            // multi-delta path the token-by-token answer streaming exercises.
            MockTurn::TextChunks(chunks) => {
                let mut out: Vec<Result<RawStreamingChoice<MockRawResponse>, CompletionError>> =
                    chunks
                        .into_iter()
                        .map(|chunk| Ok(RawStreamingChoice::Message(chunk)))
                        .collect();
                out.push(Ok(RawStreamingChoice::FinalResponse(
                    MockRawResponse::default(),
                )));
                out
            }
            MockTurn::ToolCall {
                id,
                name,
                arguments,
            } => vec![
                Ok(RawStreamingChoice::ToolCall(
                    rig_core::streaming::RawStreamingToolCall::new(id, name, arguments),
                )),
                Ok(RawStreamingChoice::FinalResponse(MockRawResponse::default())),
            ],
            MockTurn::Error(message) => vec![Err(CompletionError::ProviderError(message))],
            // Handled above by awaiting forever; never reaches the chunk mapping.
            MockTurn::Hang => unreachable!("MockTurn::Hang is handled before the chunk mapping"),
        };

        let stream: StreamingResult<Self::StreamingResponse> =
            Box::pin(futures::stream::iter(chunks));
        Ok(StreamingCompletionResponse::stream(stream))
    }
}
