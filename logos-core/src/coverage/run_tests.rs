//! Tests for `logos coverage run` ([S-737], [FR-CV-14]): the run over every
//! installed tool, its outcomes, the merged snapshot, the consent it checks
//! first, the custom entry, and what `coverage status` reads back.
//!
//! The parent module's stub harness ([`World`]) is the only source of tools:
//! no real coverage tool runs, and a tool installed on this machine is never
//! found.
//!
//! [S-737]: ../../../docs/planning/journal.md#s-737-logos-coverage-run-runs-every-installed-tool-and-ingests-one-merged-snapshot
//! [FR-CV-14]: ../../../docs/specs/requirements/FR-CV-14.md

use std::fs;

use serde_json::Value;

use crate::coverage::{
    consent_pending, runner_status, CoverageRunSummary, Probes, RunProgress, RunRequest, ToolOutcome, LAST_RUN_FILE,
};
use super::{copy_tree, fixture, install, outcome, write, write_executable, World, ENTRY, PLANTED};
use crate::config::ConfigError;
use crate::history::coverage::status;

impl World {
    /// A world whose project holds every one of `projects`' fixture files.
    fn polyglot(projects: &[&str]) -> Self {
        let w = World::new();
        for project in projects {
            copy_tree(&fixture(project), &w.root);
        }
        w
    }

    /// `coverage run` over this world, each report ingested against the
    /// project's source files.
    fn run(&self, tool: Option<&str>, consent: bool) -> anyhow::Result<CoverageRunSummary> {
        self.run_reporting(tool, consent, |_| {})
    }

    /// [`run`](Self::run), each step handed to `progress`.
    fn run_reporting(
        &self,
        tool: Option<&str>,
        consent: bool,
        progress: impl FnMut(&RunProgress),
    ) -> anyhow::Result<CoverageRunSummary> {
        self.run_with(&RunRequest { tool, consent, entry: ENTRY, dependency_consent: false }, progress)
    }

    /// The runner state `coverage status` reports, as its payload.
    fn runner(&self) -> Value {
        serde_json::to_value(runner_status(&self.root, &self.env(), Probes::Every).unwrap()).unwrap()
    }

    /// A stub of `tool` on the search path that answers its version command,
    /// prints `stderr` and exits `exit` — writing no report.
    fn silent(&self, tool: &str, binary: &str, version: &str, stderr: &str, exit: i32) {
        let args = crate::coverage::registry::tool(tool).unwrap().version_args.join(" ");
        write_executable(
            &self.bin.join(binary),
            &format!("#!/bin/sh\n[ \"$*\" = '{args}' ] && {{ echo '{version}'; exit 0; }}\necho '{stderr}' >&2\nexit {exit}\n"),
        );
    }
}

fn row<'a>(runner: &'a Value, tool: &str) -> &'a Value {
    runner["tools"].as_array().unwrap().iter().find(|t| t["tool"] == tool).unwrap_or_else(|| panic!("{tool}: {runner}"))
}

/// The consent refusal a run returned: exit 2, its message.
fn refusal(err: &anyhow::Error) -> String {
    assert_eq!(crate::error::exit_code(err), 2, "{err:#}");
    match err.downcast_ref::<ConfigError>() {
        Some(ConfigError::InvalidValue { key, message }) if key == "consent" => message.clone(),
        _ => panic!("expected a consent refusal, got {err:#}"),
    }
}

/// The gated verdict, without the provenance that tracks the commit.
fn gated(engine: &crate::Engine) -> String {
    let g = engine.gate(None, false, true).expect("gate");
    serde_json::json!([g.passed, g.signal, g.baseline_signal, g.regressions, g.test_function_count]).to_string()
}

/// UAT-CV-05: on a Rust + vitest project one run yields two *ran* outcomes
/// and one merged snapshot, `coverage status` lists both tools with the
/// overall %, and the gate reads the same before and after the run (BR-28).
#[test]
fn a_rust_and_vitest_run_ingests_both_reports_into_one_snapshot() {
    let w = World::polyglot(&["cargo-llvm-cov", "vitest"]);
    install(&w, "cargo-llvm-cov", 0);
    install(&w, "vitest", 0);
    w.commit();
    let engine = crate::Engine::start(&w.root).unwrap().with_scan_environment(w.env());
    engine.index();
    engine.gate(None, true, true).expect("gate --save");
    let before = gated(&engine);

    let summary = engine.coverage_run(None, true).expect("the run");
    assert_eq!(summary.consent_recorded, ["cargo-llvm-cov", "vitest"]);
    for tool in ["cargo-llvm-cov", "vitest"] {
        let ToolOutcome::Ran { ingests, .. } = outcome(&summary, tool) else {
            panic!("{tool}: {summary:?}")
        };
        assert_eq!(Some(ingests[0].snapshot_id), summary.snapshot_id, "{tool} lands in the one snapshot");
        assert_eq!(ingests[0].matched_files, 1, "{tool}: {ingests:?}");
    }
    assert!(matches!(outcome(&summary, "go"), ToolOutcome::NotApplicable));
    assert_eq!(summary.tools.len(), 7, "one outcome per registered tool, no custom entry configured");
    assert_eq!(summary.exit_code(), 0);
    assert_eq!(gated(&engine), before, "a coverage run never moves the gate");

    let status = serde_json::to_value(engine.coverage_status().unwrap()).unwrap();
    assert_eq!(status["report_count"], 2, "{status}");
    assert_eq!(status["total_files"], 2, "{status}");
    // src/lib.rs covers 3 of its 6 lines, src/sum.js 2 of 4: 5 of 10.
    assert_eq!(status["overall_coverage_bp"], 5000, "{status}");
    let runner = &status["runner"];
    for tool in ["cargo-llvm-cov", "vitest"] {
        assert_eq!(row(runner, tool)["state"], "ran", "{runner}");
        assert_eq!(row(runner, tool)["last_run"]["head_sha"], summary.head_sha.clone().unwrap().as_str());
    }
    assert_eq!(row(runner, "vitest")["origin"], "project");
    assert_eq!(row(runner, "jest")["state"], "not_applicable");
    assert_eq!(runner["consent"]["consented"], serde_json::json!(["cargo-llvm-cov", "vitest"]));
    assert_eq!(runner["consent"]["pending"], serde_json::json!([]));
    // The Dashboard's read carries the evidence alone.
    let evidence = serde_json::to_value(engine.coverage_evidence().unwrap()).unwrap();
    assert!(evidence.get("runner").is_none(), "{evidence}");
    assert_eq!(evidence["overall_coverage_bp"], 5000);
}

/// FR-CV-14: one tool missing and one failing are reported — *not installed*,
/// and *failed* with a masked excerpt of its stderr and the whole of it in a
/// log — and the rest is ingested. The secret the failing build printed
/// reaches the log only: no payload, no status, no record, no store.
#[test]
fn a_missing_and_a_failing_tool_are_reported_and_the_rest_ingest() {
    let w = World::polyglot(&["cargo-llvm-cov", "vitest", "go"]);
    w.silent("cargo-llvm-cov", "cargo-llvm-cov", "cargo-llvm-cov 0.6.13", &format!("error: token={PLANTED} rejected"), 2);
    install(&w, "go", 0);
    // vitest is declared but its dependencies were never installed.
    w.commit();

    let summary = w.run(None, true).expect("the run");
    let ToolOutcome::Failed { exit_code, stderr, log, .. } = outcome(&summary, "cargo-llvm-cov") else {
        panic!("{summary:?}")
    };
    assert_eq!(*exit_code, Some(2));
    assert!(stderr.starts_with("error: token=") && stderr.contains(crate::history::security::mask::MASK), "{stderr}");
    assert_eq!(log.as_deref(), Some(".logos/coverage/cargo-llvm-cov.failed.log"));
    let logged = fs::read_to_string(w.root.join(".logos/coverage/cargo-llvm-cov.failed.log")).unwrap();
    assert!(logged.contains(PLANTED), "the log keeps the whole output: {logged}");
    assert!(
        matches!(outcome(&summary, "vitest"), ToolOutcome::NotInstalled { hint: Some(h) } if h.contains("dependencies")),
        "{summary:?}"
    );
    let ToolOutcome::Ran { ingests, .. } = outcome(&summary, "go") else { panic!("{summary:?}") };
    assert_eq!(ingests[0].matched_files, 1);
    assert_eq!(summary.exit_code(), 1, "a tool that started failed");

    let runner = w.runner();
    assert_eq!(row(&runner, "cargo-llvm-cov")["state"], "failed");
    assert_eq!(row(&runner, "cargo-llvm-cov")["last_run"]["exit_code"], 2);
    assert_eq!(row(&runner, "cargo-llvm-cov")["last_run"]["log"], ".logos/coverage/cargo-llvm-cov.failed.log");
    assert_eq!(row(&runner, "vitest")["state"], "not_installed");
    assert_eq!(row(&runner, "go")["state"], "ran");
    assert_eq!(status(&w.root).unwrap().report_count, 1, "go's report was ingested");

    let payloads = [serde_json::to_string(&summary).unwrap(), runner.to_string()];
    for payload in &payloads {
        assert!(!payload.contains(PLANTED), "the secret reached a payload: {payload}");
    }
    let logos = w.root.join(".logos");
    for entry in fs::read_dir(&logos).unwrap().chain(fs::read_dir(logos.join("coverage")).unwrap()) {
        let path = entry.unwrap().path();
        if path.is_file() && !path.ends_with("cargo-llvm-cov.failed.log") {
            let bytes = fs::read(&path).unwrap();
            assert!(!String::from_utf8_lossy(&bytes).contains(PLANTED), "the secret reached {}", path.display());
        }
    }

    // A clean run of the failed tool clears its log.
    install(&w, "cargo-llvm-cov", 0);
    w.run(Some("cargo-llvm-cov"), false).expect("consent is recorded");
    assert!(!w.root.join(".logos/coverage/cargo-llvm-cov.failed.log").exists());
    assert_eq!(row(&w.runner(), "cargo-llvm-cov")["state"], "ran");
}

/// CR-234 CRA-01: a failing suite that still writes its report reads *ran —
/// tests failed*, and the report is ingested.
#[test]
fn a_failing_suite_that_writes_a_report_reads_tests_failed_and_is_ingested() {
    let w = World::polyglot(&["cargo-llvm-cov"]);
    install(&w, "cargo-llvm-cov", 101);
    w.commit();
    let summary = w.run(None, true).expect("the run");
    let ToolOutcome::TestsFailed { exit_code, reports, ingests } = outcome(&summary, "cargo-llvm-cov") else {
        panic!("{summary:?}")
    };
    assert_eq!((*exit_code, reports.as_slice()), (Some(101), [".logos/coverage/rust.xml".to_string()].as_slice()));
    assert_eq!(ingests[0].matched_files, 1);
    assert_eq!(summary.exit_code(), 1, "the suite failed");
    assert_eq!(serde_json::to_value(&summary).unwrap()["tools"][0]["outcome"], "tests_failed");
    assert_eq!(row(&w.runner(), "cargo-llvm-cov")["state"], "tests_failed");
    assert_eq!(status(&w.root).unwrap().report_count, 1);
}

/// FR-CV-13: without consent nothing starts — not even a version probe — and
/// the refusal names exactly the tools that would start; status lists them as
/// pending until the consent is given.
#[test]
fn no_tool_starts_without_consent_and_the_consent_names_what_would_start() {
    let w = World::polyglot(&["cargo-llvm-cov", "vitest", "go"]);
    install(&w, "cargo-llvm-cov", 0);
    install(&w, "vitest", 0);
    w.commit();
    let runner = w.runner();
    assert_eq!(runner["consent"]["pending"], serde_json::json!(["cargo-llvm-cov", "vitest"]));
    assert_eq!(row(&runner, "cargo-llvm-cov")["state"], "never_run");
    assert_eq!(row(&runner, "go")["state"], "not_installed");

    let message = refusal(&w.run(None, false).expect_err("no consent yet"));
    assert!(message.contains("for cargo-llvm-cov, vitest —"), "go is not installed, so not named: {message}");
    assert!(message.contains(&format!("`{ENTRY} --consent`")), "{message}");
    let before = w.invocations("cargo-llvm-cov").len();
    let refused = refusal(&w.run(None, false).expect_err("still no consent"));
    assert_eq!(refused, message);
    assert_eq!(w.invocations("cargo-llvm-cov").len(), before, "a refused run starts nothing, not even a probe");
    assert!(w.argvs("vitest").is_empty(), "a project copy is never run before consent");
    assert!(!w.root.join(".logos/coverage").join(LAST_RUN_FILE).exists());

    w.run(None, true).expect("consented");
    assert_eq!(w.runner()["consent"]["pending"], serde_json::json!([]));
    assert!(w.argvs("cargo-llvm-cov").iter().any(|a| a.first().is_some_and(|x| x == "llvm-cov") && !a.contains(&"--version".to_string())), "the consented run started it");
}

/// `--tool` runs that tool alone and leaves every other tool's last run as it
/// was; a name that is not registered, or `custom` with no command, starts
/// nothing and exits 2; a named tool that cannot start exits 1.
#[test]
fn a_named_tool_runs_alone_and_a_bad_name_starts_nothing() {
    let w = World::polyglot(&["cargo-llvm-cov", "go"]);
    install(&w, "cargo-llvm-cov", 0);
    install(&w, "go", 0);
    w.commit();
    w.run(None, true).expect("the full run");
    let first = row(&w.runner(), "cargo-llvm-cov")["last_run"].clone();

    let summary = w.run(Some("go"), false).expect("go alone");
    assert_eq!(summary.tools.iter().map(|t| t.tool).collect::<Vec<_>>(), ["go"]);
    assert_eq!(summary.exit_code(), 0);
    assert_eq!(row(&w.runner(), "cargo-llvm-cov")["last_run"], first, "cargo-llvm-cov's last run is kept");
    assert_eq!(w.argvs("go").iter().filter(|a| a.first().is_some_and(|x| x == "test")).count(), 2);

    let invocations = w.invocations("go").len() + w.invocations("cargo-llvm-cov").len();
    let unknown = w.run(Some("rustc"), true).expect_err("not registered");
    assert_eq!(crate::error::exit_code(&unknown), 2, "{unknown:#}");
    assert!(
        matches!(unknown.downcast_ref::<ConfigError>(), Some(ConfigError::InvalidValue { key, .. }) if key == "tool"),
        "{unknown:#}"
    );
    let no_command = w.run(Some("custom"), true).expect_err("no refresh_cmd");
    assert_eq!(crate::error::exit_code(&no_command), 3, "a missing command, not a usage error: {no_command:#}");
    assert!(no_command.to_string().contains("refresh_cmd"), "{no_command:#}");
    let after = w.invocations("go").len() + w.invocations("cargo-llvm-cov").len();
    assert_eq!(after, invocations, "a refused run starts nothing, not even a version probe");
    let missing = w.run(Some("vitest"), true).expect("vitest is reported");
    assert!(matches!(missing.tools[0].outcome, ToolOutcome::NotApplicable));
    assert_eq!(missing.exit_code(), 1, "the tool named did not start");
}

/// FR-CV-14 / FR-CV-10: the custom entry runs after the registered tools,
/// through `sh -c`, under the consent that pins its text; an artifact an
/// earlier run left is never credited to a command that wrote none.
#[test]
fn the_custom_entry_runs_last_and_an_old_artifact_is_never_credited() {
    let w = World::polyglot(&["go"]);
    install(&w, "go", 0);
    let command = "printf 'TN:t\\nSF:calc.go\\nDA:3,1\\nend_of_record\\n' > lcov.info";
    w.repo_config(&format!("[coverage_ingest]\nrefresh_cmd = \"{}\"\n", command.replace('\\', "\\\\")));
    w.commit();

    let message = refusal(&w.run(None, false).expect_err("no consent"));
    assert!(message.contains("for go, custom —") && message.contains(&format!("verbatim:\n{command}\n")), "{message}");
    // The custom command's consent is asked for only by a run that starts it.
    crate::coverage::consent::require(Some(&w.user), &w.root, &["go"], None, true, ENTRY).unwrap();
    w.run(Some("go"), false).expect("a run of go alone needs no consent for the custom command");
    let message = refusal(&w.run(None, false).expect_err("the custom command is not consented"));
    assert!(message.contains("for custom —"), "only the custom command is asked for: {message}");
    let summary = w.run(None, true).expect("the run");
    assert_eq!(summary.consent_recorded, ["custom"]);
    assert_eq!(summary.tools.last().unwrap().tool, "custom", "the custom entry runs last");
    let ToolOutcome::Ran { reports, ingests } = outcome(&summary, "custom") else { panic!("{summary:?}") };
    assert_eq!((reports.as_slice(), ingests[0].matched_files), (["lcov.info".to_string()].as_slice(), 1));
    assert_eq!(ingests[0].snapshot_id, summary.snapshot_id.unwrap(), "both reports merge into one snapshot");
    assert_eq!(row(&w.runner(), "custom")["state"], "ran");

    // A command that writes nothing finds lcov.info still there: not credited.
    w.repo_config("[coverage_ingest]\nrefresh_cmd = \"true\"\n");
    let consent = w.runner()["consent"].clone();
    assert_eq!((&consent["command_changed"], &consent["pending"]), (&Value::from(true), &serde_json::json!(["custom"])), "{consent}");
    let summary = w.run(Some("custom"), true).expect("re-consented");
    let ToolOutcome::Failed { stderr, .. } = outcome(&summary, "custom") else { panic!("{summary:?}") };
    assert!(stderr.contains("wrote no coverage artifact"), "{stderr}");
    assert_eq!(row(&w.runner(), "custom")["state"], "failed");
}

/// A cancelled run kills what runs and starts nothing after it: every tool
/// still to start reads *cancelled*, and so does its last run.
#[test]
fn a_cancelled_run_starts_nothing_more() {
    let w = World::polyglot(&["cargo-llvm-cov", "go"]);
    install(&w, "cargo-llvm-cov", 0);
    install(&w, "go", 0);
    w.commit();
    crate::coverage::consent::require(Some(&w.user), &w.root, &["cargo-llvm-cov", "go"], None, true, ENTRY).unwrap();
    let summary = crate::jobs::foreground(|| {
        crate::jobs::this_run().expect("a foreground run").cancel();
        w.run(None, false).expect("the run")
    });
    for tool in ["cargo-llvm-cov", "go"] {
        assert!(matches!(outcome(&summary, tool), ToolOutcome::Cancelled), "{summary:?}");
        assert_eq!(row(&w.runner(), tool)["state"], "cancelled");
    }
    assert!(w.argvs("go").iter().all(|a| a.first().is_some_and(|x| x == "version")), "{:?}", w.argvs("go"));
    assert_eq!(summary.snapshot_id, None);
    assert_eq!(summary.exit_code(), 1);
}

/// S-738: a run hands each step to `progress` as it happens — a tool that
/// starts is announced, then settled exactly as the summary lists it; a tool
/// that is not run is settled without a start.
#[test]
fn each_tool_is_announced_then_settled_as_the_summary_lists_it() {
    let w = World::polyglot(&["go"]);
    install(&w, "go", 0);
    w.commit();
    let mut steps = Vec::new();
    let summary = w.run_reporting(None, true, |step| steps.push(step.clone())).expect("the run");
    let started: Vec<&str> = steps
        .iter()
        .filter_map(|step| match step {
            RunProgress::Started { tool } => Some(*tool),
            _ => None,
        })
        .collect();
    assert_eq!(started, ["go"], "only the installed tool starts: {steps:?}");
    let finished: Vec<_> = steps
        .iter()
        .filter_map(|step| match step {
            RunProgress::Finished(run) => Some(run.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(finished, summary.tools, "each settled part is the summary's own");
    let go_start = steps.iter().position(|s| *s == RunProgress::Started { tool: "go" }).unwrap();
    assert!(matches!(&steps[go_start + 1], RunProgress::Finished(run) if run.tool == "go"), "{steps:?}");
    let wire = serde_json::to_value(&steps[go_start]).unwrap();
    assert_eq!(wire, serde_json::json!({ "phase": "started", "tool": "go" }));
    let wire = serde_json::to_value(&steps[go_start + 1]).unwrap();
    assert_eq!((&wire["phase"], &wire["tool"], &wire["outcome"]), (&"finished".into(), &"go".into(), &"ran".into()), "{wire}");
}

/// S-738: what a run's consent does not cover yet is read without starting a
/// process — the installed tools, then the custom command verbatim — and is
/// empty once the run recorded it; a bad tool name is the run's own error.
#[test]
fn the_pending_consent_is_read_without_starting_anything() {
    let w = World::polyglot(&["go"]);
    install(&w, "go", 0);
    w.repo_config("[coverage_ingest]\nrefresh_cmd = \"make cov\"\n");
    w.commit();
    let pending = consent_pending(&w.root, &w.env(), None).expect("pending");
    assert_eq!(pending.tools, ["go", "custom"]);
    assert_eq!(pending.refresh_cmd.as_deref(), Some("make cov"));
    assert!(w.argvs("go").is_empty(), "no process started: {:?}", w.argvs("go"));
    assert_eq!(consent_pending(&w.root, &w.env(), Some("go")).unwrap().tools, ["go"]);
    assert!(consent_pending(&w.root, &w.env(), Some("nope")).is_err(), "an unregistered tool is refused");

    crate::coverage::consent::require(Some(&w.user), &w.root, &["go"], Some("make cov"), true, ENTRY).unwrap();
    assert!(consent_pending(&w.root, &w.env(), None).unwrap().is_empty());
}

/// A last-run record that is not a regular file Logos wrote — a symlink, or
/// text that is not one — reads as no run, never as an error.
#[test]
fn a_record_that_is_not_one_reads_as_never_run() {
    let w = World::polyglot(&["go"]);
    install(&w, "go", 0);
    let record = w.root.join(".logos/coverage").join(LAST_RUN_FILE);
    write(&record, "{not json");
    assert_eq!(row(&w.runner(), "go")["state"], "never_run");
    fs::remove_file(&record).unwrap();
    let elsewhere = w.root.join("planted.json");
    write(&elsewhere, r#"{"go": {"outcome": "ran", "exit_code": 0, "head_sha": null, "finished_at": 1, "log": null}}"#);
    std::os::unix::fs::symlink(&elsewhere, &record).unwrap();
    assert_eq!(row(&w.runner(), "go")["state"], "never_run", "a symlinked record is not read");
}

/// A tool that prints its error to stdout alone — cargo-llvm-cov and pytest
/// often do — and writes no report reads *failed* with that text as its
/// cause, masked.
#[test]
fn stdout_is_the_cause_when_stderr_is_empty() {
    let w = World::polyglot(&["go"]);
    write_executable(
        &w.bin.join("go"),
        &format!("#!/bin/sh\n[ \"$1\" = version ] && {{ echo 'go version go1.23.1 linux/amd64'; exit 0; }}\necho 'build failed: password={PLANTED}'\nexit 1\n"),
    );
    w.commit();
    let summary = w.run(None, true).expect("the run");
    let ToolOutcome::Failed { stderr, .. } = outcome(&summary, "go") else { panic!("{summary:?}") };
    assert!(stderr.starts_with("build failed: password="), "the stdout cause: {stderr:?}");
    assert!(!stderr.contains(PLANTED), "{stderr}");
}

impl World {
    /// A `cargo-llvm-cov` stub that answers its version command and fails
    /// writing no report: `stdout` and `stderr` as shell commands, run in
    /// that order, stdout's first.
    fn failing_with(&self, stdout: &str, stderr: &str) {
        let args = crate::coverage::registry::tool("cargo-llvm-cov").unwrap().version_args.join(" ");
        write_executable(
            &self.bin.join("cargo-llvm-cov"),
            &format!(
                "#!/bin/sh\n[ \"$*\" = '{args}' ] && {{ echo 'cargo-llvm-cov 0.6.13'; exit 0; }}\n\
                 {stdout}\n{{ {stderr}; }} >&2\nexit 101\n"
            ),
        );
    }
}

/// HF-8: a failed run whose output outgrows the bound keeps the END of it —
/// where a test runner lists what failed — in the log, with one line stating
/// what was left out, and the payload's excerpt names the failing test: the
/// tail of stdout, then stderr's, not stderr's alone. A secret printed there
/// is masked in the excerpt.
#[test]
fn a_failed_run_far_past_the_bound_keeps_the_failure_in_its_log_and_its_cause() {
    use crate::jobs::spawn::{STDERR_CAP, STDOUT_CAP};
    let w = World::polyglot(&["cargo-llvm-cov"]);
    let flood = |n: usize| format!("/usr/bin/yes 'filler line of test output' | /usr/bin/head -c {n}");
    w.failing_with(
        &format!(
            "echo 'running 1 test'; {}; echo; echo 'failures:'; echo '    FAILED marker-test'; \
             echo 'password={PLANTED}'; echo; echo 'test result: FAILED. 0 passed; 1 failed'",
            flood(STDOUT_CAP * 4)
        ),
        &format!("{}; echo; echo 'error: test failed, rerun with `-p marker`'", flood(STDERR_CAP * 4)),
    );
    w.commit();
    let summary = w.run(None, true).expect("the run");
    let ToolOutcome::Failed { stderr, log, .. } = outcome(&summary, "cargo-llvm-cov") else { panic!("{summary:?}") };
    assert!(stderr.contains("FAILED marker-test"), "the cause names the failing test: {stderr}");
    assert!(stderr.contains("test result: FAILED") && stderr.contains("error: test failed"), "{stderr}");
    assert!(!stderr.contains(PLANTED), "the excerpt is masked: {stderr}");
    let logged = fs::read_to_string(w.root.join(log.as_deref().unwrap())).unwrap();
    assert!(logged.contains("running 1 test"), "the head is kept");
    assert!(logged.contains("FAILED marker-test") && logged.contains("test result: FAILED"), "the end of stdout is kept");
    assert!(logged.contains("error: test failed, rerun with"), "the end of stderr is kept");
    assert_eq!(logged.matches("bytes omitted").count(), 2, "one note per stream cut: {}", logged.len());
    // Both streams, within their bounds, and the two labels.
    assert!(logged.len() <= STDOUT_CAP + STDERR_CAP + "stdout:\n\nstderr:\n".len(), "{} bytes", logged.len());
}

/// HF-8: with text on both streams the excerpt is the end of each, in its own
/// budget and labelled — stdout's last six lines, stderr's last four — so
/// the line counts and the order are the contract, and one stream's long
/// lines never push the other's out of the payload.
#[test]
fn the_cause_is_the_last_lines_of_each_stream_each_in_its_own_budget() {
    let w = World::polyglot(&["cargo-llvm-cov"]);
    let long = "e".repeat(300);
    w.failing_with(
        "i=1; while [ $i -le 10 ]; do echo \"o$i\"; i=$((i+1)); done",
        &format!("echo e1; echo {long}; echo {long}; echo {long}; echo {long}; echo e6"),
    );
    w.commit();
    let summary = w.run(None, true).expect("the run");
    let ToolOutcome::Failed { stderr, .. } = outcome(&summary, "cargo-llvm-cov") else { panic!("{summary:?}") };
    assert!(
        stderr.starts_with("stdout:\no5\no6\no7\no8\no9\no10\nstderr:\n"),
        "stdout's last six lines come first, whole, whatever stderr printed: {stderr:?}"
    );
    assert!(stderr.ends_with("\ne6"), "stderr's last line is last: {stderr:?}");
    assert!(stderr.len() <= 1024, "within the payload's excerpt: {} bytes", stderr.len());
    let after = stderr.split("\nstderr:\n").nth(1).unwrap();
    assert!(after.starts_with('…'), "stderr's long lines are cut from the front, marked: {after:?}");
}

/// HF-8: a secret that spans lines is masked before the cause is cut to its
/// last lines, so the lines of it that survive the cut are not left in the
/// excerpt for want of the `BEGIN` line that was cut away.
#[test]
fn a_multi_line_secret_is_masked_before_the_cause_is_cut() {
    let w = World::polyglot(&["cargo-llvm-cov"]);
    w.failing_with(
        "echo '-----BEGIN RSA PRIVATE KEY-----'; i=1; while [ $i -le 8 ]; do echo \"PEMBODY$i\"; i=$((i+1)); done; \
         echo '-----END RSA PRIVATE KEY-----'; echo 'FAILED marker-test'",
        "echo 'error: test failed'",
    );
    w.commit();
    let summary = w.run(None, true).expect("the run");
    let ToolOutcome::Failed { stderr, .. } = outcome(&summary, "cargo-llvm-cov") else { panic!("{summary:?}") };
    assert!(stderr.contains("FAILED marker-test"), "{stderr}");
    assert!(!stderr.contains("PEMBODY"), "a line of the key reached the excerpt: {stderr}");
    assert!(stderr.contains(crate::history::security::mask::MASK), "{stderr}");
}

/// HF-8: output within the bound is kept whole — no omission note — in the
/// log and in the excerpt.
#[test]
fn a_failed_run_within_the_bound_is_logged_whole_with_no_omission_note() {
    let w = World::polyglot(&["cargo-llvm-cov"]);
    w.failing_with(
        "echo 'running 1 test'; echo 'FAILED marker-test'",
        "echo 'error: test failed, rerun with `-p marker`'",
    );
    w.commit();
    let summary = w.run(None, true).expect("the run");
    let ToolOutcome::Failed { stderr, log, .. } = outcome(&summary, "cargo-llvm-cov") else { panic!("{summary:?}") };
    let logged = fs::read_to_string(w.root.join(log.as_deref().unwrap())).unwrap();
    assert_eq!(
        logged,
        "stdout:\nrunning 1 test\nFAILED marker-test\nstderr:\nerror: test failed, rerun with `-p marker`"
    );
    assert_eq!(*stderr, logged, "a short output is its own excerpt");
}

/// A run that starts nothing — no tool applies here — reports each tool's
/// reason, exits 0, and writes nothing into the project.
#[test]
fn a_run_that_starts_nothing_writes_nothing() {
    let w = World::new();
    w.commit_empty();
    let summary = w.run(None, false).expect("nothing needs consent");
    assert!(summary.tools.iter().all(|t| matches!(t.outcome, ToolOutcome::NotApplicable)), "{summary:?}");
    assert_eq!((summary.exit_code(), summary.snapshot_id), (0, None));
    assert!(!w.root.join(".logos").exists(), "a run that started nothing wrote into the project");
}

impl World {
    /// [`World::commit`] over a project with one file and no ecosystem marker.
    fn commit_empty(&self) {
        write(&self.root.join("README.md"), "nothing to cover\n");
        self.commit();
    }
}

/// The custom entry credits only what its command wrote: a command that
/// removes the newest artifact and writes nothing leaves an older one behind,
/// and that one is not its report either (review finding, S-737).
#[test]
fn an_older_artifact_left_behind_is_not_the_custom_entrys_report() {
    let w = World::polyglot(&["go"]);
    write(&w.root.join("coverage/lcov.info"), "TN:t\nSF:calc.go\nDA:3,1\nend_of_record\n");
    let day_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(86_400);
    fs::File::options().write(true).open(w.root.join("coverage/lcov.info")).unwrap().set_modified(day_ago).unwrap();
    write(&w.root.join("lcov.info"), "TN:t\nSF:calc.go\nDA:3,1\nend_of_record\n");
    w.repo_config("[coverage_ingest]\nrefresh_cmd = \"/bin/rm -f lcov.info\"\n");
    w.commit();
    let summary = w.run(Some("custom"), true).expect("the run");
    let ToolOutcome::Failed { stderr, .. } = outcome(&summary, "custom") else { panic!("{summary:?}") };
    assert!(stderr.contains("wrote no coverage artifact"), "{stderr}");
}

/// A clean run clears its tool's failure log only inside a real
/// `.logos/coverage/`: through a committed `.logos` symlink it removes nothing
/// outside the project (review finding, S-737).
#[test]
fn a_clean_run_never_removes_a_log_through_a_symlinked_logos() {
    let w = World::polyglot(&["go"]);
    let outside = w.tmp.path().join("outside");
    write(&outside.join("coverage/custom.failed.log"), "not the project's\n");
    write(&outside.join("config.toml"), "[coverage_ingest]\nrefresh_cmd = \"printf 'TN:t\\\\nSF:calc.go\\\\nDA:3,1\\\\nend_of_record\\\\n' > lcov.info\"\n");
    std::os::unix::fs::symlink(&outside, w.root.join(".logos")).unwrap();
    w.commit();
    let summary = w.run(Some("custom"), true).expect("the run");
    assert!(matches!(outcome(&summary, "custom"), ToolOutcome::Ran { .. }), "{summary:?}");
    assert!(outside.join("coverage/custom.failed.log").exists(), "a file outside the project was removed");
}

/// A failure's cause printed on stdout is kept when stderr carries only a
/// warning: the log holds both streams, labelled, and so does the excerpt
/// when they are short (review finding, S-737).
#[test]
fn a_cause_on_stdout_survives_a_warning_on_stderr() {
    let w = World::polyglot(&["go"]);
    w.repo_config(
        "[coverage_ingest]\nrefresh_cmd = \"echo 'warning: deprecated flag' >&2; echo 'REAL_CAUSE: test xyz failed'; exit 1\"\n",
    );
    w.commit();
    let summary = w.run(Some("custom"), true).expect("the run");
    let ToolOutcome::Failed { stderr, log, .. } = outcome(&summary, "custom") else { panic!("{summary:?}") };
    assert_eq!(stderr, "stdout:\nREAL_CAUSE: test xyz failed\nstderr:\nwarning: deprecated flag");
    let logged = fs::read_to_string(w.root.join(log.as_deref().unwrap())).unwrap();
    assert!(logged.contains("REAL_CAUSE") && logged.contains("warning: deprecated flag"), "{logged}");
}

/// The web page's read (`Probes::Never`) starts no process: a tool on the
/// search path still reads installed — `never_run`, its version unknown — and
/// the consent's pending list is the same as a probing read's (ADR-38).
#[test]
fn an_unprobed_status_read_starts_nothing() {
    let w = World::polyglot(&["cargo-llvm-cov"]);
    install(&w, "cargo-llvm-cov", 0);
    let unprobed = serde_json::to_value(runner_status(&w.root, &w.env(), Probes::Never).unwrap()).unwrap();
    assert!(w.invocations("cargo-llvm-cov").is_empty(), "{:?}", w.invocations("cargo-llvm-cov"));
    let row = row(&unprobed, "cargo-llvm-cov");
    assert_eq!((&row["state"], &row["origin"]), (&Value::from("never_run"), &Value::from("path")), "{unprobed}");
    assert!(row.get("version").is_none(), "{unprobed}");
    assert_eq!(unprobed["consent"], w.runner()["consent"], "the same pending consent either way");
    assert_eq!(w.invocations("cargo-llvm-cov").len(), 1, "the probing read asked once");
}

/// A report that does not ingest makes its tool *failed* with the tool's own
/// exit status, the cause in its log and nothing stored for it; a later
/// failing suite whose report ingests clears that log (review findings, S-737).
#[test]
fn a_report_that_does_not_ingest_fails_its_tool() {
    let w = World::polyglot(&["go"]);
    write_executable(
        &w.bin.join("go"),
        "#!/bin/sh\n[ \"$1\" = version ] && { echo 'go version go1.23.1 linux/amd64'; exit 0; }\n\
         for a in \"$@\"; do case $a in -coverprofile=*) echo 'not a coverprofile' > \"${a#-coverprofile=}\";; esac; done\n",
    );
    w.commit();
    let summary = w.run(None, true).expect("the run");
    let ToolOutcome::Failed { exit_code, log, ingests, .. } = outcome(&summary, "go") else { panic!("{summary:?}") };
    assert_eq!((*exit_code, log.as_deref(), ingests.len()), (Some(0), Some(".logos/coverage/go.failed.log"), 0));
    assert_eq!((summary.exit_code(), summary.snapshot_id), (1, None));
    assert_eq!(status(&w.root).unwrap().report_count, 0, "nothing was stored");
    assert_eq!(row(&w.runner(), "go")["state"], "failed");

    install(&w, "go", 1);
    let summary = w.run(None, false).expect("consented");
    assert!(matches!(outcome(&summary, "go"), ToolOutcome::TestsFailed { .. }), "{summary:?}");
    assert!(!w.root.join(".logos/coverage/go.failed.log").exists(), "a failing suite whose report ingests clears the log");
}

/// A per-module tool whose first report ingests and whose second does not
/// reads *failed*, and still names the ingest that is in the store — the
/// summary's snapshot is that one (review finding, S-737).
#[test]
fn a_partial_ingest_is_named_in_the_failed_outcome() {
    use std::os::unix::fs::PermissionsExt;
    let w = World::polyglot(&["maven"]);
    let core = fixture("maven").join("fixture-report-core.xml");
    write_executable(
        &w.bin.join("mvn"),
        &format!(
            "#!/bin/sh\n[ \"$1\" = --version ] && {{ echo 'Apache Maven 3.9.9'; exit 0; }}\n\
             /bin/mkdir -p core/target/site/jacoco zz/target/site/jacoco\n\
             /bin/cp '{}' core/target/site/jacoco/jacoco.xml\n: > zz/target/site/jacoco/jacoco.xml\n\
             /bin/chmod 000 zz/target/site/jacoco/jacoco.xml\n",
            core.display()
        ),
    );
    w.commit();
    let summary = w.run(Some("maven"), true).expect("the run");
    let unreadable = w.root.join("zz/target/site/jacoco/jacoco.xml");
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o644)).unwrap();
    let ToolOutcome::Failed { exit_code, ingests, .. } = outcome(&summary, "maven") else { panic!("{summary:?}") };
    assert_eq!((*exit_code, ingests.len()), (Some(0), 1), "{summary:?}");
    assert_eq!(summary.snapshot_id, Some(ingests[0].snapshot_id));
    assert_eq!(status(&w.root).unwrap().report_count, 1);
}
