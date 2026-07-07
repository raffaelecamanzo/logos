//! Provider-resolution fitness tests (S-166, ADR-41, [NFR-SE-07]).
//!
//! `rig` must resolve both an Anthropic-native and an OpenAI-compatible
//! provider, and the OpenAI-compatible client must honor a configurable
//! `base_url` defaulting to OpenRouter. Resolving a client only *constructs* an
//! in-memory `reqwest` handle — it opens no connection — so these run offline
//! with a throwaway key and never dial.

use agent_core::{
    anthropic_completion_model, openai_compatible_completion_model, resolve_anthropic,
    resolve_openai_compatible, ProviderConfig, RetryPolicy,
};

#[test]
fn openai_compatible_resolves_and_defaults_to_openrouter() {
    let cfg = ProviderConfig::openai_compatible("openai/gpt-4o-mini", "sk-test-key");
    let client =
        resolve_openai_compatible(&cfg).expect("rig resolves the OpenAI-compatible client");
    assert_eq!(
        client.base_url(),
        "https://openrouter.ai/api/v1",
        "the OpenAI-compatible provider defaults to OpenRouter",
    );
}

#[test]
fn openai_compatible_honors_a_configured_base_url() {
    let cfg = ProviderConfig::openai_compatible("local/model", "sk-test-key")
        .with_base_url("http://127.0.0.1:11434/v1");
    let client = resolve_openai_compatible(&cfg).expect("rig resolves a custom-endpoint client");
    assert_eq!(
        client.base_url(),
        "http://127.0.0.1:11434/v1",
        "an explicit base_url overrides the OpenRouter default",
    );
}

#[test]
fn anthropic_resolves_to_its_native_endpoint() {
    let cfg = ProviderConfig::anthropic("claude-sonnet-4-6", "sk-ant-test");
    let client = resolve_anthropic(&cfg).expect("rig resolves the native Anthropic client");
    // rig normalizes the Anthropic base URL; assert it points at the native host.
    assert!(
        client.base_url().contains("api.anthropic.com"),
        "the Anthropic provider points at its native endpoint: {}",
        client.base_url(),
    );
}

#[test]
fn completion_model_helpers_resolve_for_both_providers() {
    // Both helpers now wrap the resolved model in the retry decorator (CR-060,
    // S-240); resolution is still egress-free and applies the passed policy
    // verbatim — assert the returned RetryingModel actually carries it, so a
    // constructor that ignored or hard-coded the argument would fail.
    let policy = RetryPolicy::new(4, 125);

    let openai = ProviderConfig::openai_compatible("openai/gpt-4o-mini", "sk-test");
    let openai_model = openai_compatible_completion_model(&openai, policy)
        .expect("the OpenAI-compatible completion-model helper resolves a model handle");
    assert_eq!(
        openai_model.policy(),
        policy,
        "the constructor stores the passed retry policy",
    );

    let anthropic = ProviderConfig::anthropic("claude-sonnet-4-6", "sk-ant-test");
    let anthropic_model = anthropic_completion_model(&anthropic, policy)
        .expect("the Anthropic completion-model helper resolves a model handle");
    assert_eq!(
        anthropic_model.policy(),
        policy,
        "the constructor stores the passed retry policy",
    );
}
