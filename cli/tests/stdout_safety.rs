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
}
