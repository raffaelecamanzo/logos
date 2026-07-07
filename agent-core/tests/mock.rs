//! Direct unit coverage for the mock `CompletionModel`'s non-text paths
//! (S-166 review-fix): the `ToolCall` and `Error` turn variants and the
//! exhaustion behavior, across both `completion` and `stream`. These are the
//! seams the planner/subagent roster ([S-173], [S-174]) will depend on, so a
//! silent regression in the tool-call wiring or the honest-failure contract
//! ([NFR-CC-04]) must be caught here.
//!
//! [S-173]: docs/planning/journal.md
//! [S-174]: docs/planning/journal.md
//! [NFR-CC-04]: docs/specs/requirements/NFR-CC-04.md

use agent_core::rig::completion::{AssistantContent, CompletionError, CompletionModel};
use agent_core::rig::streaming::StreamedAssistantContent;
use agent_core::{MockCompletionModel, MockTurn};
use futures::StreamExt;

#[tokio::test]
async fn completion_returns_a_scripted_tool_call() {
    let args = serde_json::json!({ "name": "Engine" });
    let model = MockCompletionModel::new([MockTurn::tool_call("call-1", "node", args.clone())]);
    let request = model.completion_request("find Engine").build();

    let response = model.completion(request).await.expect("the mock completes");
    match response.choice.first() {
        AssistantContent::ToolCall(tc) => {
            assert_eq!(tc.id, "call-1");
            assert_eq!(tc.function.name, "node");
            assert_eq!(tc.function.arguments, args);
        }
        other => panic!("expected a tool call, got {other:?}"),
    }
}

#[tokio::test]
async fn stream_emits_a_scripted_tool_call() {
    let args = serde_json::json!({ "pattern": "fn main" });
    let model = MockCompletionModel::new([MockTurn::tool_call("call-2", "grep", args.clone())]);
    let request = model.completion_request("grep main").build();

    let mut stream = model.stream(request).await.expect("the mock streams");
    let mut saw_tool_call = false;
    while let Some(chunk) = stream.next().await {
        if let Ok(StreamedAssistantContent::ToolCall { tool_call, .. }) = chunk {
            assert_eq!(tool_call.function.name, "grep");
            assert_eq!(tool_call.function.arguments, args);
            saw_tool_call = true;
        }
    }
    assert!(
        saw_tool_call,
        "the streamed turn yielded the scripted tool call"
    );
}

#[tokio::test]
async fn completion_surfaces_a_scripted_error_turn() {
    let model = MockCompletionModel::new([MockTurn::Error("provider exploded".to_string())]);
    let request = model.completion_request("x").build();

    let err = model
        .completion(request)
        .await
        .expect_err("an error turn surfaces as an error");
    match err {
        CompletionError::ProviderError(msg) => {
            assert!(
                msg.contains("provider exploded"),
                "the scripted message is preserved: {msg}"
            );
        }
        other => panic!("expected a ProviderError, got {other:?}"),
    }
}

#[tokio::test]
async fn completion_on_an_exhausted_mock_fails_honestly() {
    // No scripted turns: the mock must refuse rather than fabricate a response
    // (NFR-CC-04).
    let model = MockCompletionModel::new([]);
    let request = model.completion_request("x").build();

    let err = model
        .completion(request)
        .await
        .expect_err("an exhausted mock returns an error, never a fabricated turn");
    match err {
        CompletionError::ProviderError(msg) => {
            assert!(
                msg.contains("exhausted"),
                "the error names exhaustion: {msg}"
            );
        }
        other => panic!("expected a ProviderError, got {other:?}"),
    }
}

#[tokio::test]
async fn stream_on_an_exhausted_mock_fails_honestly() {
    let model = MockCompletionModel::new([]);
    let request = model.completion_request("x").build();

    // `StreamingCompletionResponse` is not `Debug`, so match rather than
    // `expect_err`.
    match model.stream(request).await {
        Err(CompletionError::ProviderError(msg)) => {
            assert!(
                msg.contains("exhausted"),
                "the error names exhaustion: {msg}"
            );
        }
        Err(other) => panic!("expected a ProviderError, got {other:?}"),
        Ok(_) => panic!("an exhausted mock stream must not produce a stream"),
    }
}
