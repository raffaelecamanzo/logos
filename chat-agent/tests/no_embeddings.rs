//! No-embedding / no-vector-store fitness function (S-175, [FR-UI-20], [ADR-41]).
//!
//! [FR-UI-20] constrains the multi-step memory to v1: a **per-turn scratchpad +
//! per-thread working/conversation memory** over plain `chat.db` relational rows —
//! **no embedding model, no vector index, no RAG store**. This test enforces that
//! invariant *structurally*: it resolves the `chat-agent` crate's dependency tree
//! (the crate that owns the memory store and pulls in `agent-core`'s `rig` stack)
//! and fails the build if any known embedding / vector-store / ANN / RAG crate has
//! entered it. A regression — someone wiring `rig`'s `lancedb`/`qdrant` vector
//! integration, an ANN index, or an embedding runtime into the chat feature — is
//! caught at `cargo test` time, the same way [`no_network_deps`] guards the
//! offline invariant.
//!
//! It is the structural twin of `logos-core/tests/no_network_deps.rs`; the
//! denylist and matching semantics are this story's, the scan machinery is the
//! same `cargo tree` feature-resolution approach.
//!
//! [no_network_deps]: ../../logos-core/tests/no_network_deps.rs
//! [FR-UI-20]: ../../docs/specs/requirements/FR-UI-20.md
//! [ADR-41]: ../../docs/specs/architecture/decisions/ADR-41.md

use std::process::Command;

/// Known embedding / vector-store / ANN / RAG crates that must never enter the
/// chat-agent dependency tree (v1 has no semantic memory, [FR-UI-20]).
///
/// *Atomic* crate names matched exactly, across four families:
/// - **`rig` vector-store / RAG adapters** — every `rig-<store>` integration is a
///   vector index or RAG store (`rig-lancedb`, `rig-qdrant`, `rig-mongodb`,
///   `rig-neo4j`, `rig-sqlite`, `rig-postgres`, `rig-surrealdb`, `rig-milvus`,
///   `rig-scylladb`, `rig-fastembed`). The framework-internal crates `rig-core`
///   (the `Agent`/`Tool` core the orchestrator is built on) and `rig-derive` (its
///   proc-macro) are deliberately **absent** — they are not vector stores.
/// - **managed vector DBs** (`lancedb`, `qdrant-client`, `milvus-sdk-rust`,
///   `sqlite-vec`, `sqlite-vss`, `pgvector`).
/// - **approximate-nearest-neighbour indexes** (`hnsw`, `hnsw_rs`,
///   `instant-distance`, `usearch`, `faiss`, `arroy`, `annoy`).
/// - **embedding / tokenizer / model runtimes** that signal a RAG pipeline
///   (`fastembed`, `tokenizers`, `tiktoken-rs`, `ort`, `rust-bert`, `llm`).
///
/// Representative of what a contributor would plausibly introduce, not an
/// exhaustive enumeration; add new families here as they become relevant.
const DENIED_EXACT: &[&str] = &[
    // rig vector-store / RAG adapters (NOT rig-core / rig-derive, the framework).
    "rig-lancedb",
    "rig-qdrant",
    "rig-mongodb",
    "rig-neo4j",
    "rig-sqlite",
    "rig-postgres",
    "rig-surrealdb",
    "rig-milvus",
    "rig-scylladb",
    "rig-fastembed",
    // managed vector DBs.
    "lancedb",
    "qdrant-client",
    "milvus-sdk-rust",
    "sqlite-vec",
    "sqlite-vss",
    "pgvector",
    // approximate-nearest-neighbour indexes.
    "hnsw",
    "hnsw_rs",
    "instant-distance",
    "usearch",
    "faiss",
    "arroy",
    "annoy",
    // embedding / tokenizer / model runtimes.
    "fastembed",
    "tokenizers",
    "tiktoken-rs",
    "ort",
    "rust-bert",
    "llm",
];

/// Denied crate-name *prefixes* — families published as many sub-crates.
///
/// `candle-` covers the candle ML stack (`candle-core`, `candle-nn`,
/// `candle-transformers`) used to run embedding models locally. The `rig-` family
/// is matched **exactly** (see [`DENIED_EXACT`]) rather than by prefix, so the
/// framework-internal `rig-core` / `rig-derive` are not false-positives.
const DENIED_PREFIXES: &[&str] = &["candle-"];

/// Returns `true` if `name` is a denylisted embedding / vector-store crate:
/// an exact match against [`DENIED_EXACT`] OR a prefix match against
/// [`DENIED_PREFIXES`].
fn is_denied(name: &str) -> bool {
    DENIED_EXACT.contains(&name) || DENIED_PREFIXES.iter().any(|p| name.starts_with(p))
}

/// Resolve the crate names in the `chat-agent` dependency tree (normal + build
/// edges, dev excluded) via `cargo tree`. `--offline` keeps it network-free (the
/// registry cache is warm after the build that precedes `cargo test`).
fn chat_agent_tree_crates() -> Vec<String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let output = Command::new(cargo)
        .args([
            "tree",
            "--package",
            "chat-agent",
            "--edges",
            "normal,build",
            "--prefix",
            "none",
            "--format",
            "{p}",
            "--color",
            "never",
            "--offline",
        ])
        .output()
        .expect("`cargo tree` runs (the no-embedding fitness gate must resolve the tree)");

    assert!(
        output.status.success(),
        "`cargo tree` failed; the no-embedding fitness gate could not resolve the tree:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );

    let stdout = String::from_utf8(output.stdout).expect("`cargo tree` output is UTF-8");
    let mut names: Vec<String> = stdout
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_string)
        .collect();
    names.sort_unstable();
    names.dedup();
    names
}

/// Asserts the chat-agent dependency tree contains no embedding/vector/RAG crate
/// — the multi-step memory stays a plain relational store ([FR-UI-20]).
#[test]
fn chat_agent_graph_has_no_embedding_or_vector_crate() {
    let offenders: Vec<String> = chat_agent_tree_crates()
        .into_iter()
        .filter(|name| is_denied(name))
        .collect();

    assert!(
        offenders.is_empty(),
        "no-embedding invariant violated (FR-UI-20): the chat-agent dependency \
         tree contains embedding/vector-store crate(s) {offenders:?}. The v1 \
         multi-step memory is a per-turn scratchpad + per-thread working memory \
         over plain `chat.db` relational rows — NO embeddings, NO vector index, \
         NO RAG. If semantic recall is genuinely wanted it is a deferred \
         follow-up (CR-046 §3.3) and must be added deliberately via a new CR/ADR, \
         not silently."
    );
}

/// Pins the matching semantics of [`is_denied`] directly, so a regression that
/// silently broke it (wrong negation, a typo'd prefix, an emptied list, or a
/// `rig-` rule that swallowed the framework core) is caught even when the tree
/// happens to be clean.
#[test]
fn is_denied_matches_expected_names() {
    // Exact matches — vector DBs, ANN indexes, embedding runtimes.
    assert!(is_denied("lancedb"));
    assert!(is_denied("qdrant-client"));
    assert!(is_denied("hnsw"));
    assert!(is_denied("fastembed"));
    assert!(is_denied("tokenizers"));

    // Exact matches — rig vector-store adapters.
    assert!(is_denied("rig-lancedb"));
    assert!(is_denied("rig-qdrant"));

    // Prefix matches — the candle ML stack.
    assert!(is_denied("candle-core"));
    assert!(is_denied("candle-transformers"));

    // The rig FRAMEWORK crates are NOT vector integrations — must not match.
    assert!(!is_denied("rig-core"));
    assert!(!is_denied("rig-derive"));

    // Prefix boundary: a prefix family matches only WITH its trailing hyphen, so a
    // typo dropping the `-` from `candle-` cannot silently widen the denylist —
    // bare `candle` and near-misses must NOT match.
    assert!(!is_denied("candle"));
    assert!(!is_denied("candid"));

    // Must NOT match: the crates the memory store actually uses, and near-misses.
    assert!(!is_denied("rusqlite"));
    assert!(!is_denied("serde"));
    assert!(!is_denied("serde_json"));
    assert!(!is_denied("anyhow"));
}
