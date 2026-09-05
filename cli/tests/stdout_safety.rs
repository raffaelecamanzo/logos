//! The stdout-safety invariant, end to end through the real binary (S-019,
//! [NFR-RA-01], [FR-OB-02], [UAT-MC-02] precursor).
//!
//! Even at `RUST_LOG=trace`, stdout must carry **only** the serialised
//! read-model — every log line belongs to stderr. A single stray stdout byte
//! would corrupt an MCP JSON-RPC stream ([RK-02]); the CLI shares the exact
//! same `observability::init` seam, so proving it here proves the seam. The
//! full `serve --mcp` trace-level proof lands with S-017's server.
//!
//! Also drives the telemetry round-trip as a user would: a command records
//! usage, `stats --json` reports it ([FR-OB-04]).
//!
//! [NFR-RA-01]: ../../docs/specs/requirements/NFR-RA-01.md
//! [FR-OB-02]: ../../docs/specs/requirements/FR-OB-02.md
//! [RK-02]: ../../docs/specs/software-spec.md#8-risk-register

use std::fs;
use std::process::Command;

use tempfile::TempDir;

/// Run the `logos` binary in `root` with `RUST_LOG=trace` and the given args.
fn logos(root: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_logos"))
        .args(args)
        .arg("--project")
        .arg(root)
        .env("RUST_LOG", "trace")
        .output()
        .expect("logos binary runs")
}

/// At trace level, stdout is exactly one JSON document — all tracing output
/// (which must be present at this level) lands on stderr (NFR-RA-01).
#[test]
fn stdout_carries_only_the_read_model_even_at_trace_level() {
    let dir = TempDir::new().expect("temp dir");
    fs::create_dir_all(dir.path().join(".logos")).expect("pre-create .logos");

    let out = logos(dir.path(), &["languages", "--json"]);
    assert!(out.status.success(), "languages exits 0");

    let stdout = String::from_utf8(out.stdout).expect("stdout is UTF-8");
    serde_json::from_str::<serde_json::Value>(stdout.trim())
        .expect("stdout parses as a single JSON document — no log contamination");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("languages"),
        "trace-level tracing output appears on stderr (the emission point fired): {stderr}"
    );
}

/// The telemetry round-trip through the binary: a command records usage into
/// `.logos/telemetry.db`, then `stats --json` reports counts and the saved
/// estimate (FR-OB-03, FR-OB-04).
#[test]
fn stats_reports_usage_recorded_by_prior_commands() {
    let dir = TempDir::new().expect("temp dir");
    fs::create_dir_all(dir.path().join(".logos")).expect("pre-create .logos");

    // Two commands' worth of telemetry, flushed at each process exit.
    assert!(logos(dir.path(), &["languages", "--json"]).status.success());
    assert!(logos(dir.path(), &["languages", "--json"]).status.success());
    assert!(
        dir.path().join(".logos").join("telemetry.db").is_file(),
        "telemetry.db materialised next to (not inside) logos.db"
    );

    let out = logos(dir.path(), &["stats", "--json"]);
    assert!(out.status.success(), "stats exits 0");
    let stdout = String::from_utf8(out.stdout).expect("stdout is UTF-8");
    let stats: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("stats --json is clean JSON");

    assert_eq!(stats["window_days"], 7, "FR-OB-04 default window");
    let calls_total = stats["calls_total"].as_u64().expect("calls_total present");
    assert!(
        calls_total >= 2,
        "both languages runs recorded, got {calls_total}"
    );
    assert!(
        stats["calls_by_tool"]
            .as_array()
            .expect("calls_by_tool present")
            .iter()
            .any(|u| u["tool"] == "languages" && u["surface"] == "cli"),
        "usage is broken down by tool and surface: {stats}"
    );
    // Percentile and estimate fields are present and well-typed (the math is
    // unit-tested in logos-core); `languages` is not a navigation tool, so the
    // estimate may legitimately be zero here.
    for field in [
        "latency_p50_ms",
        "latency_p95_ms",
        "latency_p99_ms",
        "reads_saved_estimate",
        "tokens_saved_estimate",
    ] {
        assert!(stats[field].as_u64().is_some(), "{field} present: {stats}");
    }

    // ── FR-OB-11, asserted through the real binary ────────────────────────
    //
    // "`logos stats --json` reports per-tool counts split by dev/`main` origin,
    // and a class for every tool." The temp dir is not a git worktree, so both
    // runs stamp `origin = "main"`.
    let cross_tab = stats["calls_by_tool_origin"]
        .as_array()
        .expect("calls_by_tool_origin present");
    let cell = cross_tab
        .iter()
        .find(|c| c["tool"] == "languages")
        .unwrap_or_else(|| panic!("the cross-tab names the tool that ran: {stats}"));
    assert_eq!(cell["origin"], "main", "outside a worktree, calls are main: {stats}");
    assert!(cell["calls"].as_u64().is_some_and(|n| n >= 2), "{stats}");
    // `languages` reports Logos's own grammar registry — a `read-model` class,
    // yet a counted event (the two classifications are independent axes).
    assert_eq!(cell["class"], "read-model", "{stats}");

    let by_class = stats["calls_by_class"]
        .as_array()
        .expect("calls_by_class present");
    assert!(
        by_class
            .iter()
            .any(|c| c["class"] == "read-model" && c["origin"] == "main"),
        "the class × origin dogfood table is populated: {stats}"
    );

    // Every tool the read-model reports carries a class — the label rides the
    // existing raw-plus-rollup shape, so this holds for aged-out days too.
    for usage in stats["calls_by_tool"].as_array().expect("calls_by_tool present") {
        assert!(
            usage["class"].as_str().is_some_and(|c| !c.is_empty()),
            "{} carries a class: {stats}",
            usage["tool"]
        );
    }

    // "The payload states its own coverage limits" — an unlabelled figure is
    // what NFR-CC-04 forbids.
    let coverage = &stats["attribution_coverage"];
    assert_eq!(coverage["raw_events_only"], true, "{stats}");
    assert_eq!(coverage["legacy_null_origin_folds_into_main"], true, "{stats}");
    assert_eq!(coverage["requested_window_days"], 7, "{stats}");
    assert_eq!(coverage["covered_window_days"], 7, "{stats}");
    assert_eq!(coverage["truncated_by_retention"], false, "{stats}");
    assert!(
        coverage["notes"].as_array().is_some_and(|n| n.len() >= 3),
        "the limits are stated as prose too: {stats}"
    );

    // A window past raw retention names the window it actually covers.
    let out = logos(dir.path(), &["stats", "--json", "--window", "365"]);
    assert!(out.status.success(), "stats --window 365 exits 0");
    let wide: serde_json::Value =
        serde_json::from_str(String::from_utf8(out.stdout).unwrap().trim()).expect("clean JSON");
    let coverage = &wide["attribution_coverage"];
    assert_eq!(coverage["requested_window_days"], 365, "{wide}");
    assert_eq!(coverage["covered_window_days"], 90, "{wide}");
    assert_eq!(coverage["truncated_by_retention"], true, "{wide}");
}
