//! Tool-domain partition + bounded dispatch (S-167).
//!
//! Asserts each domain exposes **exactly** its tool subset (acceptance: "the
//! tool set is partitioned into the graph / governance / source domains the
//! specialized subagents will own", S-174) and that the bounded dispatcher
//! enforces the partition boundary and the budget halt ([NFR-CC-04]).

use std::sync::Arc;

use agent_core::{
    governance_toolset, graph_toolset, source_toolset, BoundedDispatcher, DispatchError, Sandbox,
    ToolBudget, ToolDomain,
};
use logos_core::Engine;
use rig_core::tool::ToolSet;

fn fixture_engine() -> (Arc<Engine>, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("src")).expect("mkdir src");
    std::fs::write(dir.path().join("src/lib.rs"), "pub fn f() {}\n").expect("fixture");
    (Arc::new(Engine::start(dir.path()).expect("engine")), dir)
}

fn fixture_sandbox() -> (Arc<Sandbox>, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("a.txt"), "one\n").expect("a");
    std::fs::write(dir.path().join("b.txt"), "two\n").expect("b");
    (
        Arc::new(Sandbox::new(dir.path(), std::iter::empty()).expect("sandbox")),
        dir,
    )
}

/// A built set must contain exactly the domain's declared names — no more, no
/// fewer — and none of the other domains' names.
async fn assert_partition(domain: ToolDomain, toolset: &ToolSet) {
    let expected = domain.tool_names();
    let defs = toolset.get_tool_definitions().await.expect("definitions");
    assert_eq!(
        defs.len(),
        expected.len(),
        "{domain:?} exposes exactly {} tools",
        expected.len()
    );
    for name in expected {
        assert!(toolset.contains(name), "{domain:?} is missing {name}");
    }
    // No tool from a *different* domain leaks into this set.
    for other in ToolDomain::ALL {
        if other == domain {
            continue;
        }
        for name in other.tool_names() {
            if expected.contains(name) {
                continue; // (domains are disjoint, but guard anyway)
            }
            assert!(
                !toolset.contains(name),
                "{domain:?} must not expose {other:?}'s tool {name}"
            );
        }
    }
}

#[tokio::test]
async fn each_domain_exposes_exactly_its_subset() {
    let (engine, _dir) = fixture_engine();
    let (sandbox, _sdir) = fixture_sandbox();

    assert_partition(ToolDomain::Graph, &graph_toolset(engine.clone())).await;
    assert_partition(ToolDomain::Governance, &governance_toolset(engine)).await;
    assert_partition(ToolDomain::Source, &source_toolset(sandbox)).await;
}

#[test]
fn the_three_domains_are_disjoint_and_cover_the_expected_counts() {
    assert_eq!(ToolDomain::Graph.tool_names().len(), 8);
    assert_eq!(ToolDomain::Governance.tool_names().len(), 8);
    assert_eq!(ToolDomain::Source.tool_names().len(), 3);

    // Pairwise disjoint.
    for a in ToolDomain::ALL {
        for b in ToolDomain::ALL {
            if a == b {
                continue;
            }
            for name in a.tool_names() {
                assert!(
                    !b.tool_names().contains(name),
                    "{name} appears in both {a:?} and {b:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn bounded_dispatch_refuses_out_of_set_tools_without_charging() {
    let (sandbox, _dir) = fixture_sandbox();
    let budget = ToolBudget::new(2);
    let dispatcher = BoundedDispatcher::new(source_toolset(sandbox), &budget);

    // A graph tool is not in the source set — refused, and the budget is
    // untouched (a misroute should not spend the run's allowance, S-174).
    let err = dispatcher
        .dispatch("search", "{}")
        .await
        .expect_err("an out-of-domain tool is refused");
    assert!(matches!(err, DispatchError::ToolNotFound(_)), "got {err:?}");
    assert_eq!(budget.used(), 0, "a refused misroute charges nothing");
}

#[tokio::test]
async fn a_sandbox_escape_is_classified_turn_fatal_by_a_typed_variant() {
    let (sandbox, _dir) = fixture_sandbox();
    let budget = ToolBudget::new(4);
    let dispatcher = BoundedDispatcher::new(source_toolset(sandbox), &budget);

    // A `read` of a path escaping the project root via `..` is a containment
    // refusal — classified as the TURN-FATAL `Containment` variant (not the
    // recoverable `Tool`), detected structurally from the typed `SandboxError`
    // cause, never a substring match (CR-063, NFR-SE-04/NFR-CC-04).
    let err = dispatcher
        .dispatch(
            "read",
            serde_json::json!({ "path": "../../etc/passwd" }).to_string(),
        )
        .await
        .expect_err("a sandbox escape is refused");
    match err {
        DispatchError::Containment(refusal) => {
            assert!(
                refusal.contains("escapes the project root"),
                "the containment refusal names the escape: {refusal}"
            );
        }
        other => panic!("expected a turn-fatal containment refusal, got {other:?}"),
    }
}

#[tokio::test]
async fn a_benign_missing_file_stays_a_recoverable_tool_error() {
    let (sandbox, _dir) = fixture_sandbox();
    let budget = ToolBudget::new(4);
    let dispatcher = BoundedDispatcher::new(source_toolset(sandbox), &budget);

    // A `read` of a path that simply does not exist is NOT a containment refusal:
    // it stays the recoverable `Tool` variant so the orchestrator keeps CR-060's
    // route-around degradation ([FR-UI-28]). Guards against an over-broad
    // classification turning benign faults turn-fatal.
    let err = dispatcher
        .dispatch(
            "read",
            serde_json::json!({ "path": "does-not-exist.txt" }).to_string(),
        )
        .await
        .expect_err("a missing file is a tool error");
    assert!(
        matches!(err, DispatchError::Tool(_)),
        "a missing file stays recoverable, got {err:?}"
    );
}

#[tokio::test]
async fn bounded_dispatch_halts_honestly_when_the_budget_is_spent() {
    let (sandbox, _dir) = fixture_sandbox();
    let budget = ToolBudget::new(1);
    let dispatcher = BoundedDispatcher::new(source_toolset(sandbox), &budget);

    // First call is within budget.
    dispatcher
        .dispatch("read", serde_json::json!({ "path": "a.txt" }).to_string())
        .await
        .expect("first call within budget");
    assert_eq!(budget.used(), 1);

    // Second call halts at the cap — honestly, naming the bound, never invoking
    // the tool (NFR-CC-04).
    let err = dispatcher
        .dispatch("read", serde_json::json!({ "path": "b.txt" }).to_string())
        .await
        .expect_err("the budget is spent");
    match err {
        DispatchError::BudgetExhausted(exhausted) => {
            assert_eq!(exhausted.limit, 1);
            assert_eq!(exhausted.used, 1);
        }
        other => panic!("expected an honest budget halt, got {other:?}"),
    }
    assert_eq!(budget.used(), 1, "a halted dispatch consumes no extra slot");
}
