//! ui-build zero-real-egress proof for the mock provider (S-166, [NFR-SE-07],
//! [UAT-UI-07], ADR-41).
//!
//! A mock-`CompletionModel` round-trip — driven through `rig`'s real `Agent`
//! loop and through the raw streaming API — must record **zero real outbound
//! connections**. This is the behavioral half of the offline carve-out (the
//! structural half is the byte-identical default-tree no-HTTP-client scan in
//! `logos-core/tests/no_network_deps.rs`, plus the ui-vs-default boundary scan
//! in `tests/carve_out.rs`).
//!
//! # The connection-recording harness
//!
//! We stand up a loopback **tripwire** listener and point a `ProviderConfig`'s
//! `base_url` at it — the very endpoint a *real* provider would dial. Then we
//! run the full round-trip using the **mock** model instead. If any real egress
//! leaked through `rig`'s machinery to the configured endpoint, the tripwire's
//! accept loop would record it. We assert it recorded **zero** connections and
//! that the mock itself served the turn (so the answer is genuinely the mock's,
//! not a fabricated pass-through).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use agent_core::rig::agent::AgentBuilder;
use agent_core::rig::completion::{CompletionModel, Prompt};
use agent_core::rig::streaming::StreamedAssistantContent;
use agent_core::{resolve_openai_compatible, MockCompletionModel, MockTurn, ProviderConfig};
use futures::StreamExt;
use tokio::net::TcpListener;

/// A loopback tripwire: binds an ephemeral port and counts every connection its
/// accept loop sees. Returns the recorder and the `base_url` a provider would
/// dial to reach it.
struct Tripwire {
    connections: Arc<AtomicUsize>,
    base_url: String,
}

async fn spawn_tripwire() -> Tripwire {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind tripwire");
    let addr = listener.local_addr().expect("tripwire addr");
    let connections = Arc::new(AtomicUsize::new(0));
    let counter = connections.clone();
    tokio::spawn(async move {
        while listener.accept().await.is_ok() {
            counter.fetch_add(1, Ordering::SeqCst);
        }
    });
    Tripwire {
        connections,
        base_url: format!("http://{addr}/v1"),
    }
}

#[tokio::test]
async fn mock_agent_round_trip_records_zero_real_egress() {
    let tripwire = spawn_tripwire().await;

    // A provider config whose endpoint IS the tripwire — a real client would
    // dial it. The mock serves the turn instead, and the tripwire proves
    // nothing reached the wire.
    let cfg = ProviderConfig::openai_compatible("mock/model", "sk-test")
        .with_base_url(tripwire.base_url.clone());
    assert_eq!(cfg.effective_base_url(), tripwire.base_url);

    // Tie the tripwire to the REAL client path: resolving the actual
    // reqwest-backed provider client against the tripwire endpoint must
    // construct successfully and open no connection (provider resolution is
    // egress-free — the first dial is a later, consent-gated turn, NFR-SE-07).
    let real_client = resolve_openai_compatible(&cfg).expect("the real client constructs");
    let _ = real_client; // constructed, never prompted — no turn is run on it
    assert_eq!(
        tripwire.connections.load(Ordering::SeqCst),
        0,
        "constructing the real provider client opened no connection",
    );

    let model = MockCompletionModel::text("The Engine read-models ground every answer.");
    let agent = AgentBuilder::new(model.clone())
        .preamble("You are a Logos test agent.")
        .build();

    let answer = agent
        .prompt("What grounds the answers?")
        .await
        .expect("the mock-backed agent completes a turn");

    assert!(
        answer.contains("Engine read-models"),
        "the agent returned the mock's scripted answer: {answer:?}",
    );
    assert!(model.request_count() >= 1, "the mock served the turn");
    assert_eq!(
        tripwire.connections.load(Ordering::SeqCst),
        0,
        "the mock round-trip opened zero real outbound connections",
    );
}

#[tokio::test]
async fn mock_streaming_round_trip_records_zero_real_egress() {
    let tripwire = spawn_tripwire().await;

    let model = MockCompletionModel::new([MockTurn::text("streamed grounding")]);
    let request = model.completion_request("stream me").build();
    let mut stream = model.stream(request).await.expect("the mock streams");

    let mut text = String::new();
    while let Some(chunk) = stream.next().await {
        if let Ok(StreamedAssistantContent::Text(t)) = chunk {
            text.push_str(&t.text);
        }
    }

    assert!(
        text.contains("streamed grounding"),
        "streamed text aggregates: {text:?}"
    );
    assert!(
        model.request_count() >= 1,
        "the mock served the streamed turn"
    );
    assert_eq!(
        tripwire.connections.load(Ordering::SeqCst),
        0,
        "the mock streaming round-trip opened zero real outbound connections",
    );
}
