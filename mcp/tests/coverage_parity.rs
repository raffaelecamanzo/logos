//! CLI-vs-MCP payload-equality for the coverage evidence-tier surfaces (S-051,
//! CR-007, [FR-CV-06], [FR-CV-07]).
//!
//! Both surfaces delegate to the **one** `api` entrypoint per command
//! (`Engine::coverage_status` / `Engine::hotspots`, [NFR-CC-01]) and serialise its
//! read-model — the CLI via `serde_json`, the MCP twin via `Content::json`. This
//! proves the wire payloads are byte-identical for the same store state over a
//! real git-history + indexed + coverage-ingested fixture. (The full 25-tool
//! `logos:*` listing is asserted in `protocol.rs`.)

use std::path::Path;
use std::process::Command;

use logos_core::Engine;
use mcp::LogosMcp;
use rmcp::{
    model::CallToolRequestParams,
    service::{RoleClient, RunningService},
    ServiceExt,
};
use serde_json::{json, Map, Value};

fn sh_git(cwd: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["-c", "user.email=dev@logos", "-c", "user.name=Logos Dev"])
        .args(args)
        .output()
        .expect("git is on PATH");
    assert!(out.status.success(), "git {args:?} failed");
}

fn commit(cwd: &Path, rel: &str, contents: &str, msg: &str) {
    let path = cwd.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
    sh_git(cwd, &["add", rel]);
    sh_git(cwd, &["commit", "-q", "-m", msg]);
}

fn branchy(name: &str, ifs: usize) -> String {
    let body: String = (0..ifs)
        .map(|i| format!("    if x == {i} {{ return {i}; }}\n"))
        .collect();
    format!("pub fn {name}(x: i64) -> i64 {{\n{body}    x\n}}\n")
}

/// An indexed git fixture with two churny, complex files — one covered, one not —
/// and a coverage snapshot already ingested.
fn fixture() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);
    for n in 0..3 {
        commit(
            repo,
            "src/hot.rs",
            &format!("{}// rev {n}\n", branchy("hot", 6)),
            &format!("hot v{n}"),
        );
    }
    for n in 0..2 {
        commit(
            repo,
            "src/tested.rs",
            &format!("{}// rev {n}\n", branchy("tested", 4)),
            &format!("tested v{n}"),
        );
    }

    {
        // Ingest coverage through a transient engine, then drop it so the server
        // owns the store with a single writer at a time.
        let engine = Engine::start(repo).expect("engine starts");
        engine.index();
        let lines: String = (1..=5).map(|n| format!("DA:{n},1\n")).collect();
        let report = repo.join("coverage.info");
        std::fs::write(
            &report,
            format!("TN:suite\nSF:src/tested.rs\n{lines}end_of_record\n"),
        )
        .unwrap();
        engine
            .coverage_ingest(&report, None)
            .expect("coverage ingest");
    }
    tmp
}

type Client = RunningService<RoleClient, ()>;

async fn boot(root: &Path) -> (Client, tokio::task::JoinHandle<()>) {
    boot_engine(Engine::start(root).expect("engine start")).await
}

async fn boot_engine(engine: Engine) -> (Client, tokio::task::JoinHandle<()>) {
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let server = tokio::spawn(async move {
        if let Ok(running) = LogosMcp::new(engine, &logos_ext::ExtensionHost::new()).serve(server_io).await {
            let _ = running.waiting().await;
        }
    });
    let client = ().serve(client_io).await.expect("client initialize");
    (client, server)
}

/// The JSON payload an MCP tool returns (the `Content::json` text re-parsed).
async fn mcp_payload(
    client: &Client,
    tool: &'static str,
    args: Option<Map<String, Value>>,
) -> Value {
    let mut params = CallToolRequestParams::new(tool);
    if let Some(map) = args {
        params = params.with_arguments(map);
    }
    let result = client.call_tool(params).await.expect("tool call");
    assert_ne!(result.is_error, Some(true), "{tool} must succeed");
    let text = result.content.first().unwrap().as_text().unwrap();
    serde_json::from_str(&text.text).expect("valid JSON")
}

#[tokio::test]
async fn cli_and_mcp_coverage_status_payloads_are_identical() {
    let tmp = fixture();
    let root = tmp.path();

    let cli = {
        let engine = Engine::start(root).expect("engine");
        serde_json::to_value(engine.coverage_status().expect("coverage status")).unwrap()
    };

    let (client, server) = boot(root).await;
    let mcp = mcp_payload(&client, "coverage_status", None).await;
    assert_eq!(
        cli, mcp,
        "CLI and MCP coverage_status payloads must be byte-identical (FR-CV-06)"
    );
    // Provenance + freshness are present on a populated payload.
    assert!(
        mcp["head_sha"].is_string(),
        "HEAD provenance present: {mcp}"
    );
    assert_eq!(mcp["fresh_files"].as_u64(), Some(1));
    // The overall line-coverage aggregate rides the payload in parity: the lone
    // fresh file (src/tested.rs) has 5/5 lines covered → 100% (FR-CV-06, CR-021).
    assert_eq!(mcp["overall_coverage_bp"].as_i64(), Some(10_000));

    client.cancel().await.ok();
    server.abort();
}

#[tokio::test]
async fn cli_and_mcp_untested_hotspots_payloads_are_identical() {
    let tmp = fixture();
    let root = tmp.path();

    // Warm the lazy mine so neither compared call carries the first-mine notice.
    Engine::start(root)
        .expect("engine")
        .hotspots(None, true, false)
        .expect("warm-up");

    let cli = {
        let engine = Engine::start(root).expect("engine");
        serde_json::to_value(engine.hotspots(None, true, false).expect("hotspots")).unwrap()
    };

    let (client, server) = boot(root).await;
    let args = Map::from_iter([("untested".to_string(), Value::Bool(true))]);
    let mcp = mcp_payload(&client, "hotspots", Some(args)).await;
    assert_eq!(
        cli, mcp,
        "CLI and MCP --untested hotspot payloads must be byte-identical (FR-CV-07)"
    );

    // The uncovered hotspot leads; the fresh-covered file is filtered out.
    let paths: Vec<&str> = mcp["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap())
        .collect();
    assert_eq!(
        paths.first(),
        Some(&"src/hot.rs"),
        "uncovered hotspot leads: {paths:?}"
    );
    assert!(
        !paths.contains(&"src/tested.rs"),
        "fresh-covered file excluded under --untested: {paths:?}"
    );
    assert_eq!(mcp["coverage_basis"], "coverage");

    client.cancel().await.ok();
    server.abort();
}

#[tokio::test]
async fn mcp_coverage_ingest_returns_the_ingest_summary() {
    // A fresh indexed git repo with NO prior ingest, so the MCP `coverage_ingest`
    // tool exercises a SUCCESSFUL write path (protocol.rs only covers its error
    // path) and serialises the `IngestSummary` read-model via the same `api`
    // chokepoint the CLI uses (FR-CV-06, NFR-CC-01).
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    sh_git(root, &["init", "-q", "-b", "main"]);
    commit(root, "src/lib.rs", "pub fn a() -> i64 { 0 }\n", "lib");
    let report = root.join("coverage.info");
    std::fs::write(&report, "TN:suite\nSF:src/lib.rs\nDA:1,1\nend_of_record\n").unwrap();
    Engine::start(root).expect("engine").index();

    let (client, server) = boot(root).await;
    let args = Map::from_iter([(
        "report".to_string(),
        Value::String(report.to_string_lossy().into_owned()),
    )]);
    let mcp = mcp_payload(&client, "coverage_ingest", Some(args)).await;

    // The summary read-model (not an error) came back with the expected shape.
    assert_eq!(
        mcp["format"], "lcov",
        "auto-detected the LCOV format: {mcp}"
    );
    assert_eq!(mcp["matched_files"].as_u64(), Some(1), "src/lib.rs bound");
    assert!(
        mcp["snapshot_id"].as_i64().is_some(),
        "a snapshot was opened"
    );
    assert_eq!(
        mcp["already_ingested"], false,
        "a fresh ingest, not a no-op"
    );
    assert!(mcp["head_sha"].as_str().is_some_and(|s| !s.is_empty()));

    client.cancel().await.ok();
    server.abort();
}

/// [FR-CV-13] / S-736: MCP `coverage_refresh` without the project's coverage
/// execution consent spawns nothing and returns the instruction — the
/// terminal command, and the command text verbatim — which an MCP client
/// cannot act on itself. Once the user has consented, the same MCP call runs.
#[tokio::test]
async fn mcp_coverage_refresh_spawns_nothing_without_consent_and_returns_the_instruction() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (root, user) = (tmp.path().join("project"), tmp.path().join("user"));
    std::fs::create_dir_all(root.join(".logos")).unwrap();
    sh_git(&root, &["init", "-q", "-b", "main"]);
    commit(&root, "src/lib.rs", "pub fn a() -> i64 { 0 }\n", "lib");
    let command = "printf m >> spawned.log && printf 'TN:suite\\nSF:src/lib.rs\\nDA:1,1\\nend_of_record\\n' > lcov.info";
    std::fs::write(
        root.join(".logos/config.toml"),
        format!("[coverage_ingest]\nrefresh_cmd = \"{}\"\n", command.replace('\\', "\\\\")),
    )
    .unwrap();
    let env = logos_core::security::ScanEnvironment { user_dir: Some(user.clone()), search_path: None, tools_dir: None, nvd_api_key: None };
    Engine::start(&root).expect("engine").index();

    let (client, server) = boot_engine(Engine::start(&root).expect("engine").with_scan_environment(env.clone())).await;
    let refused = client
        .call_tool(CallToolRequestParams::new("coverage_refresh"))
        .await
        .expect_err("no consent: the call is refused");
    let message = match refused {
        rmcp::ServiceError::McpError(data) => data.message.into_owned(),
        other => panic!("a non-MCP error: {other}"),
    };
    assert!(message.contains("`logos coverage refresh --consent`"), "{message}");
    assert!(message.contains("an MCP client cannot"), "{message}");
    assert!(message.contains(&format!("verbatim:\n{command}\n")), "{message}");
    assert!(!root.join("spawned.log").exists(), "nothing was spawned");
    assert!(!user.exists(), "nothing was recorded");
    client.cancel().await.ok();
    server.abort();

    // The user consents, as `logos coverage refresh --consent` records it.
    logos_core::coverage::consent::require(Some(&user), &root, &[], Some(command), true, "logos coverage refresh")
        .expect("the user consents");
    let (client, server) = boot_engine(Engine::start(&root).expect("engine").with_scan_environment(env)).await;
    let mcp = mcp_payload(&client, "coverage_refresh", None).await;
    assert_eq!(mcp["consent_recorded"], serde_json::json!([]), "{mcp}");
    assert_eq!((&mcp["tools"][0]["tool"], &mcp["tools"][0]["outcome"]), (&json!("custom"), &json!("ran")), "{mcp}");
    assert_eq!(std::fs::read_to_string(root.join("spawned.log")).unwrap(), "m");
    client.cancel().await.ok();
    server.abort();
}

#[cfg(unix)]
/// A Go project (`go.mod`, one indexed file) and a stub `go` on a search path
/// of its own that answers its version and writes a coverprofile — no real
/// tool runs — with the user's coverage consent recorded for it.
fn go_project() -> (tempfile::TempDir, std::path::PathBuf, logos_core::security::ScanEnvironment) {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().expect("tempdir");
    let (root, bin, user) = (tmp.path().join("project"), tmp.path().join("bin"), tmp.path().join("user"));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&bin).unwrap();
    sh_git(&root, &["init", "-q", "-b", "main"]);
    commit(&root, "go.mod", "module example.com/calc\n\ngo 1.23\n", "mod");
    commit(&root, "calc.go", "package calc\n\nfunc Add(a, b int) int {\n\treturn a + b\n}\n", "calc");
    let go = bin.join("go");
    std::fs::write(
        &go,
        "#!/bin/sh\n[ \"$1\" = version ] && { echo 'go version go1.23.1 linux/amd64'; exit 0; }\n\
         for a in \"$@\"; do case $a in -coverprofile=*) printf 'mode: set\\nexample.com/calc/calc.go:3.24,5.2 1 1\\n' > \"${a#-coverprofile=}\";; esac; done\n",
    )
    .unwrap();
    std::fs::set_permissions(&go, std::fs::Permissions::from_mode(0o755)).unwrap();
    logos_core::coverage::consent::require(Some(&user), &root, &["go"], None, true, "logos coverage run").expect("consent");
    Engine::start(&root).expect("engine").index();
    let env = logos_core::security::ScanEnvironment { user_dir: Some(user), search_path: Some(bin.into_os_string()), tools_dir: None, nvd_api_key: None };
    (tmp, root, env)
}

#[cfg(unix)]
/// `coverage_status` once its `job` — the coverage run `coverage_run` started
/// (S-738) — is no longer running.
async fn ended_status(client: &Client) -> Value {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        let status = mcp_payload(client, "coverage_status", None).await;
        if status["job"]["state"] != "running" {
            return status;
        }
        assert!(std::time::Instant::now() < deadline, "the coverage run is still running: {}", status["job"]);
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

#[cfg(unix)]
/// [FR-CV-14] / S-737, S-738: MCP `coverage_run` starts the run as a job and
/// returns at once; the job's result, once `coverage_status` reads it
/// completed, is the payload the CLI's `coverage run` prints — one outcome per
/// tool, Go's report ingested. The extended `coverage status` (each tool's
/// state, the consent) is byte-identical on both surfaces after it, but for
/// the `job` the CLI, which runs no job, never carries.
#[tokio::test]
async fn cli_and_mcp_coverage_run_and_extended_status_payloads_are_identical() {
    let (_tmp, root, env) = go_project();
    let engine = || Engine::start(&root).expect("engine").with_scan_environment(env.clone());
    // A first run ingests the report; the two compared runs then find it
    // already ingested at this HEAD, as each other would.
    engine().coverage_run(None, false).expect("the first run");
    let cli = serde_json::to_value(engine().coverage_run(None, false).expect("the CLI run")).unwrap();

    let (client, server) = boot_engine(engine()).await;
    let started = mcp_payload(&client, "coverage_run", None).await;
    assert_eq!((&started["joined"], &started["job"]["kind"]), (&json!(false), &json!("coverage-run")), "{started}");
    let mut status = ended_status(&client).await;
    let job = status.as_object_mut().unwrap().remove("job").expect("coverage_status reports the job");
    assert_eq!((&job["id"], &job["state"]), (&started["job"]["id"], &json!("completed")), "{job}");
    let mcp = job["result"].clone();
    assert_eq!(cli, mcp, "CLI and MCP coverage_run payloads must be byte-identical (FR-CV-14)");
    let go = mcp["tools"].as_array().unwrap().iter().find(|t| t["tool"] == "go").expect("go's outcome").clone();
    assert_eq!(go["outcome"], "ran", "{mcp}");
    assert_eq!(go["ingests"][0]["matched_files"], 1, "{mcp}");
    assert_eq!(mcp["tools"].as_array().unwrap().len(), 7, "one outcome per registered tool: {mcp}");
    let phases: Vec<(&str, &str)> = job["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e["event"]["phase"].as_str().unwrap(), e["event"]["tool"].as_str().unwrap()))
        .collect();
    let go_started = phases.iter().position(|p| *p == ("started", "go")).expect("go's start is an event");
    assert_eq!(phases[go_started + 1], ("finished", "go"), "{phases:?}");

    client.cancel().await.ok();
    server.abort();
    let cli_status = serde_json::to_value(engine().coverage_status().expect("status")).unwrap();
    assert!(cli_status.get("job").is_none(), "the CLI runs no job: {cli_status}");
    assert_eq!(cli_status, status, "CLI and MCP coverage_status payloads must be byte-identical (FR-CV-06)");
    let row = status["runner"]["tools"].as_array().unwrap().iter().find(|t| t["tool"] == "go").unwrap().clone();
    assert_eq!((&row["state"], &row["version"], &row["origin"]), (&json!("ran"), &json!("1.23.1"), &json!("path")), "{status}");
    assert_eq!(status["runner"]["consent"]["consented"], json!(["go"]));
    assert_eq!(status["report_count"], 1, "{status}");
}

#[cfg(unix)]
/// FR-CV-13: MCP `coverage_run` cannot grant the consent. Without it no job
/// starts, nothing runs and the refusal names the terminal command; `tool`
/// selects one tool.
#[tokio::test]
async fn mcp_coverage_run_refuses_without_consent_and_runs_the_named_tool() {
    let (_tmp, root, mut env) = go_project();
    let elsewhere = tempfile::tempdir().expect("a user dir with no consent");
    let consented = env.user_dir.replace(elsewhere.path().to_path_buf());
    let (client, server) = boot_engine(Engine::start(&root).expect("engine").with_scan_environment(env.clone())).await;
    let refused = client.call_tool(CallToolRequestParams::new("coverage_run")).await.expect_err("no consent");
    let message = match refused {
        rmcp::ServiceError::McpError(data) => data.message.into_owned(),
        other => panic!("a non-MCP error: {other}"),
    };
    assert!(message.contains("`logos coverage run --consent`") && message.contains("an MCP client cannot"), "{message}");
    assert!(!root.join(".logos/coverage/go.out").exists(), "nothing ran");
    let status = mcp_payload(&client, "coverage_status", None).await;
    assert!(status.get("job").is_none(), "no job started: {status}");
    client.cancel().await.ok();
    server.abort();

    env.user_dir = consented;
    let (client, server) = boot_engine(Engine::start(&root).expect("engine").with_scan_environment(env)).await;
    let args = Map::from_iter([("tool".to_string(), json!("go"))]);
    mcp_payload(&client, "coverage_run", Some(args)).await;
    let mcp = ended_status(&client).await["job"]["result"].clone();
    assert_eq!(mcp["tools"].as_array().unwrap().len(), 1, "{mcp}");
    assert_eq!((&mcp["tools"][0]["tool"], &mcp["tools"][0]["outcome"]), (&json!("go"), &json!("ran")), "{mcp}");
    client.cancel().await.ok();
    server.abort();
}

#[cfg(unix)]
/// S-738: the server runs its jobs on the runner it is handed — under `serve
/// --mcp --ui`, the web surface's — so a run started on that runner is the one
/// `coverage_status` reports and `coverage_run` joins.
#[tokio::test]
async fn coverage_run_joins_a_run_on_the_runner_the_server_is_handed() {
    let (_tmp, root, env) = go_project();
    let engine = Engine::start(&root).expect("engine").with_scan_environment(env);
    let member = engine.root().to_path_buf();
    let shared = std::sync::Arc::new(logos_core::jobs::AnalysisJobs::new());
    let (release, hold) = std::sync::mpsc::channel::<()>();
    let started = shared
        .start(logos_core::jobs::JobKind::CoverageRun, &member, move |_| {
            hold.recv().ok();
            Ok(json!("started by the web surface"))
        })
        .unwrap();
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let server_jobs = std::sync::Arc::clone(&shared);
    let server = tokio::spawn(async move {
        let mcp = LogosMcp::new(engine, &logos_ext::ExtensionHost::new()).with_jobs(server_jobs);
        if let Ok(running) = mcp.serve(server_io).await {
            let _ = running.waiting().await;
        }
    });
    let client = ().serve(client_io).await.expect("client initialize");

    let status = mcp_payload(&client, "coverage_status", None).await;
    assert_eq!(status["job"]["id"], json!(started.job.id), "coverage_status reports the shared runner's job: {}", status["job"]);
    let joined = mcp_payload(&client, "coverage_run", None).await;
    assert_eq!((&joined["joined"], &joined["job"]["id"]), (&json!(true), &json!(started.job.id)), "{joined}");
    release.send(()).unwrap();
    client.cancel().await.ok();
    server.abort();
}

#[cfg(unix)]
/// [FR-CV-14] AC / S-738: a second `coverage_run` while a run is going returns
/// the running job's id — `joined`, nothing started twice — and the run ends
/// completed once its tool does.
#[tokio::test]
async fn a_second_coverage_run_during_a_run_returns_the_running_jobs_id() {
    use std::os::unix::fs::PermissionsExt;
    let (tmp, root, env) = go_project();
    let (release, runs) = (tmp.path().join("release"), tmp.path().join("go.runs"));
    let go = tmp.path().join("bin").join("go");
    std::fs::write(
        &go,
        format!(
            "#!/bin/sh\n[ \"$1\" = version ] && {{ echo 'go version go1.23.1 linux/amd64'; exit 0; }}\n\
             echo run >> '{runs}'\nwhile [ ! -f '{release}' ]; do /bin/sleep 0.05; done\n\
             for a in \"$@\"; do case $a in -coverprofile=*) printf 'mode: set\\nexample.com/calc/calc.go:3.24,5.2 1 1\\n' > \"${{a#-coverprofile=}}\";; esac; done\n",
            runs = runs.display(),
            release = release.display(),
        ),
    )
    .unwrap();
    std::fs::set_permissions(&go, std::fs::Permissions::from_mode(0o755)).unwrap();
    let (client, server) = boot_engine(Engine::start(&root).expect("engine").with_scan_environment(env)).await;

    let first = mcp_payload(&client, "coverage_run", None).await;
    assert_eq!((&first["joined"], &first["job"]["state"]), (&json!(false), &json!("running")), "{first}");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !runs.exists() {
        assert!(std::time::Instant::now() < deadline, "go never started");
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let second = mcp_payload(&client, "coverage_run", None).await;
    assert_eq!(second["joined"], true, "the second call joins: {second}");
    assert_eq!(second["job"]["id"], first["job"]["id"], "the running job's id");

    std::fs::write(&release, "").unwrap();
    let job = ended_status(&client).await["job"].clone();
    assert_eq!((&job["id"], &job["state"]), (&first["job"]["id"], &json!("completed")), "{job}");
    assert_eq!(std::fs::read_to_string(&runs).unwrap().lines().count(), 1, "the joined call started no second run");
    client.cancel().await.ok();
    server.abort();
}
