//! CLI-vs-MCP payload equality for the four security tools (S-667, [FR-SC-06],
//! [FR-SC-08], [NFR-CC-01]).
//!
//! Each CLI command prints one `Engine` method's read-model with `serde_json`;
//! its MCP twin returns the same method's read-model through `Content::json`.
//! As `coverage_parity.rs` does for coverage, this compares the engine-direct
//! value (the CLI's payload) with the MCP payload over one store state. The CLI
//! binary's own behaviour — exit codes, the consent refusal — is
//! `cli/tests/security_surface.rs`.
//!
//! Both engines look for scanners in a stub directory and for consent in a
//! temporary user-level directory ([`ScanEnvironment`]), so no scanner on this
//! machine is found and the process environment is never changed. The fixture
//! is a git repository outside every work tree.
//!
//! [FR-SC-06]: ../../docs/specs/requirements/FR-SC-06.md
//! [FR-SC-08]: ../../docs/specs/requirements/FR-SC-08.md
//! [NFR-CC-01]: ../../docs/specs/requirements/NFR-CC-01.md

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use logos_core::security::{FindingsFilter, NvdApiKey, ScanEnvironment, ScanRequest};
use logos_core::Engine;
use mcp::LogosMcp;
use rmcp::{
    model::{CallToolRequestParams, ErrorCode},
    service::{RoleClient, RunningService, ServiceError},
    ServiceExt,
};
use serde_json::{json, Value};
use tempfile::TempDir;

/// Printed by the failing stub; must reach no payload.
const PLANTED: &str = "LOGOSPLANTEDSECRET0123456789abcdef";

/// `app/db.py` with a function around each line `semgrep.sarif` names (12 and
/// 18), so each finding binds to an enclosing symbol. Python resolves no call
/// across files here, so reachability is `unresolved` (S-669).
const DB_PY: &str = r#"import sqlite3


def connect():
    return sqlite3.connect("app.db")


def find(user_id):
    conn = connect()
    cursor = conn.cursor()
    # look the user up
    cursor.execute("SELECT * FROM users WHERE id = %s" % user_id)
    return cursor.fetchone()


def delete(uid):
    conn = connect()
    return conn.execute(f"DELETE FROM users WHERE id = {uid}")
"#;

const CLEAN_BANDIT: &str = r#"{"version":"2.1.0","runs":[{"tool":{"driver":{"name":"Bandit","rules":[]}},"results":[]}]}"#;

struct Fixture {
    _tmp: TempDir,
    root: PathBuf,
    env: ScanEnvironment,
    clean: PathBuf,
}

fn git(root: &Path, args: &[&str]) {
    let out = Command::new("git")
        .current_dir(root)
        .env("GIT_CEILING_DIRECTORIES", root.parent().unwrap())
        .args(["-c", "maintenance.auto=false", "-c", "gc.auto=0", "-c", "user.email=t@logos", "-c", "user.name=t"])
        .args(args)
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?}");
}

fn stub(bin: &Path, name: &str, report: Option<&Path>, exit: i32, stderr: &str) {
    let copy = report.map_or(String::new(), |r| format!("if [ -n \"$out\" ]; then /bin/cp '{}' \"$out\"; fi", r.display()));
    let script = format!(
        "#!/bin/sh\n\
         case \"$1\" in --version|version|-version) printf '%s\\n' '{name} 1.2.3'; exit 0;; esac\n\
         out=''; prev=''\n\
         for a in \"$@\"; do case \"$prev\" in --output|--output-file|--report-path) out=\"$a\";; esac; prev=\"$a\"; done\n\
         {copy}\n\
         printf '%s\\n' '{stderr}' >&2\n\
         exit {exit}\n"
    );
    let path = bin.join(name);
    fs::write(&path, script).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// An indexed one-commit project; semgrep and bandit run, gitleaks fails with a
/// planted secret, every other scanner is absent.
fn fixture() -> Fixture {
    let tmp = TempDir::new().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let (root, bin, user) = (base.join("project"), base.join("bin"), base.join("user"));
    for dir in [&root.join("app"), &root.join(".logos"), &bin, &user] {
        fs::create_dir_all(dir).unwrap();
    }
    fs::write(root.join("app/db.py"), DB_PY).unwrap();
    // One ignored directory on disk, so a scanner that reads it states how (FR-SC-26).
    fs::write(root.join(".gitignore"), "/build/\n").unwrap();
    fs::create_dir_all(root.join("build")).unwrap();
    fs::write(root.join("build/gen.py"), "x = 1\n").unwrap();
    fs::write(root.join(".logos/config.toml"), "[project]\nid = \"security-parity-fixture\"\n").unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "seed"]);
    let clean = base.join("clean-bandit.sarif");
    fs::write(&clean, CLEAN_BANDIT).unwrap();
    let semgrep = Path::new(env!("CARGO_MANIFEST_DIR")).join("../logos-core/tests/fixtures/sarif/semgrep.sarif");
    stub(&bin, "semgrep", Some(&semgrep), 0, "");
    stub(&bin, "bandit", Some(&clean), 0, "");
    stub(&bin, "gitleaks", None, 3, &format!("error: password={PLANTED}"));
    let env = ScanEnvironment {
        user_dir: Some(user),
        search_path: Some(bin.into_os_string()),
        tools_dir: None,
        nvd_api_key: None,
    };
    Engine::start(&root).expect("engine").index();
    Fixture { _tmp: tmp, root, env, clean }
}

impl Fixture {
    fn engine(&self) -> Engine {
        Engine::start(&self.root).expect("engine").with_scan_environment(self.env.clone())
    }

    /// The CLI's payload: the engine method's read-model, serialised.
    fn direct<T: serde::Serialize>(&self, call: impl FnOnce(&Engine) -> anyhow::Result<T>) -> Value {
        serde_json::to_value(call(&self.engine()).expect("the engine call succeeds")).unwrap()
    }
}

type Client = RunningService<RoleClient, ()>;

async fn boot(f: &Fixture) -> (Client, tokio::task::JoinHandle<()>) {
    let engine = f.engine();
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let server = tokio::spawn(async move {
        if let Ok(running) = LogosMcp::new(engine, &logos_ext::ExtensionHost::new()).serve(server_io).await {
            let _ = running.waiting().await;
        }
    });
    (().serve(client_io).await.expect("client initialize"), server)
}

async fn call(client: &Client, tool: &str, args: Value) -> Result<Value, ServiceError> {
    let Value::Object(args) = args else { panic!("arguments are an object") };
    let result = client.call_tool(CallToolRequestParams::new(tool.to_string()).with_arguments(args)).await?;
    assert_ne!(result.is_error, Some(true), "{tool} reported a tool error");
    let text = result.content.first().unwrap().as_text().unwrap();
    Ok(serde_json::from_str(&text.text).expect("JSON content"))
}

fn row<'a>(payload: &'a Value, scanner: &str) -> &'a Value {
    payload["scanners"].as_array().unwrap().iter().find(|r| r["scanner"] == scanner).unwrap()
}

/// Each listed finding's enclosing symbol and reachability `(confidence,
/// verdict)` — its graph context (S-669).
fn contexts(payload: &Value) -> Vec<(Option<&str>, Option<&str>, Option<&str>)> {
    payload["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            let c = &f["context"];
            (c["enclosing"]["symbol"]["name"].as_str(), c["reachability"]["confidence"].as_str(), c["reachability"]["verdict"].as_str())
        })
        .collect()
}

/// What `contexts` reads on the fixture: each finding bound to its function,
/// and no route claimed either way, Python resolving no call across files.
const BOUND_UNRESOLVED: [(Option<&str>, Option<&str>, Option<&str>); 2] = [
    (Some("find"), Some("unresolved"), Some("unknown")),
    (Some("delete"), Some("unresolved"), Some("unknown")),
];

#[tokio::test(flavor = "multi_thread")]
async fn the_four_security_tools_return_the_cli_payload_and_no_secret() {
    let f = fixture();

    // Before any scan: `never_run`, on both surfaces, with the consent pending.
    let cli_status = f.direct(|e| e.security_status());
    assert_eq!(row(&cli_status, "semgrep")["state"], "never_run");
    assert_eq!(cli_status["consent"]["pending"], json!(["semgrep", "gitleaks", "bandit"]));
    let (client, server) = boot(&f).await;
    assert_eq!(call(&client, "security_status", json!({})).await.unwrap(), cli_status);

    // The first scan without consent is refused, naming the CLI command: the
    // user gives consent on the command line, never an MCP client (FR-SC-07).
    let Err(ServiceError::McpError(refused)) = call(&client, "security_scan", json!({})).await else {
        panic!("the first scan must be refused without consent");
    };
    assert!(refused.message.contains("logos security scan --consent"), "{}", refused.message);
    assert!(!refused.message.contains("consent: true"), "no MCP route to consent is offered: {}", refused.message);
    // An MCP client offering consent itself is refused, and nothing is recorded.
    let Err(ServiceError::McpError(self_consent)) = call(&client, "security_scan", json!({"consent": true})).await else {
        panic!("an MCP client cannot record consent");
    };
    assert_eq!(self_consent.code, ErrorCode::INVALID_PARAMS, "{}", self_consent.message);
    assert!(self_consent.message.contains("logos security scan --consent"), "{}", self_consent.message);
    assert_eq!(f.direct(|e| e.security_status())["consent"]["pending"], json!(["semgrep", "gitleaks", "bandit"]), "nothing recorded");
    assert_eq!(f.direct(|e| e.security_status())["consent"]["consented"], json!([]), "nothing recorded");
    client.cancel().await.ok();
    server.abort();

    // Consent and scan once; then a re-scan on each surface is the same payload.
    f.direct(|e| e.security_scan(true));
    let cli_scan = f.direct(|e| e.security_scan(false));
    let (client, server) = boot(&f).await;
    let mcp_scan = call(&client, "security_scan", json!({})).await.unwrap();
    assert_eq!(mcp_scan, cli_scan, "security_scan");
    assert_eq!(row(&mcp_scan, "gitleaks")["outcome"], "failed");
    // S-769: the failure's one-line cause, masked, rides on the MCP payload.
    assert_eq!(row(&mcp_scan, "gitleaks")["cause"], "error: password=[masked]");
    assert_eq!(row(&mcp_scan, "trivy")["outcome"], "not_installed");
    assert_eq!(row(&mcp_scan, "semgrep")["outcome"], "ran");
    // FR-SC-26's coverage fact rides on the MCP payload too.
    assert_eq!(row(&mcp_scan, "semgrep")["ignored_paths"], "excluded");

    // `fresh` with zero findings, on both surfaces, beside a scanner with two.
    let mcp_status = call(&client, "security_status", json!({})).await.unwrap();
    let bandit = row(&mcp_status, "bandit");
    assert_eq!((bandit["state"].as_str(), bandit["findings"].as_u64()), (Some("fresh"), Some(0)));
    assert_eq!(row(&mcp_status, "semgrep")["findings"], 2);
    assert_eq!(row(&mcp_status, "semgrep")["ignored_paths"], "excluded");
    // FR-SC-26: the kind of each note rides on the MCP payload — the CLI's twin
    // is equal above — and an excluded scanner states none.
    assert_eq!((row(&mcp_status, "bandit")["coverage_kind"].as_str(), row(&mcp_scan, "bandit")["coverage_kind"].as_str()), (Some("unproven"), Some("unproven")));
    assert_eq!(row(&mcp_status, "gitleaks")["coverage_kind"], "history");
    assert!(row(&mcp_status, "semgrep").get("coverage_kind").is_none());

    let filters = json!({"scanner": "semgrep", "severity": "medium"});
    let mcp_findings = call(&client, "security_findings", filters).await.unwrap();
    // Every filter set, each to a value only its own slot accepts, so a swapped
    // pair of tokens cannot pass for the same filter.
    let every = json!({"severity": "medium", "scanner": "semgrep", "category": "A05", "path": "app", "suppression": "active", "edition": "2025"});
    let mcp_every = call(&client, "security_findings", every).await.unwrap();
    let mcp_ingest = call(&client, "security_ingest", json!({"report": f.clean.display().to_string()})).await.unwrap();
    let Err(ServiceError::McpError(bad)) = call(&client, "security_findings", json!({"edition": "1999"})).await else {
        panic!("an unknown edition is the caller's mistake");
    };
    assert!(bad.message.contains("1999"), "{}", bad.message);
    assert_eq!(bad.code, ErrorCode::INVALID_PARAMS, "a bad filter token is the caller's mistake");
    client.cancel().await.ok();
    server.abort();

    assert_eq!(f.direct(|e| e.security_status()), mcp_status, "security_status");
    let filter = FindingsFilter::parse(Some("medium"), Some("semgrep"), None, None, None, None, None).unwrap();
    assert_eq!(f.direct(|e| e.security_findings(&filter)), mcp_findings, "security_findings");
    assert_eq!(mcp_findings["findings"].as_array().unwrap().len(), 2);
    assert_eq!(contexts(&mcp_findings), BOUND_UNRESOLVED, "the graph context rides on the MCP payload");
    let every = FindingsFilter::parse(Some("medium"), Some("semgrep"), Some("A05"), Some("app"), Some("active"), Some("2025"), None).unwrap();
    assert_eq!(f.direct(|e| e.security_findings(&every)), mcp_every, "every filter, same slots");
    assert_eq!(mcp_every["findings"].as_array().unwrap().len(), 2, "{mcp_every}");
    assert_eq!(f.direct(|e| e.security_ingest(&f.clean)), mcp_ingest, "security_ingest");
    assert_eq!(mcp_ingest["already_ingested"], true);

    for payload in [&cli_status, &cli_scan, &mcp_scan, &mcp_status, &mcp_findings, &mcp_ingest] {
        assert!(!payload.to_string().contains(PLANTED), "a planted secret reached a payload: {payload}");
    }
}

/// MCP cannot install a scanner ([FR-SC-22], [ADR-88] §6): no tool installs,
/// uninstalls or records install consent, and `security_status` names the
/// CLI command the user runs instead — on both surfaces, one payload. The
/// instructions say so inside the 2048 characters a truncating host delivers.
///
/// [FR-SC-22]: ../../docs/specs/requirements/FR-SC-22.md
/// [ADR-88]: ../../docs/specs/architecture/decisions/ADR-88.md
#[tokio::test(flavor = "multi_thread")]
async fn no_mcp_tool_installs_and_security_status_names_the_cli_install_command() {
    let f = fixture();
    let server = LogosMcp::new(f.engine(), &logos_ext::ExtensionHost::new());
    let installers: Vec<String> = server
        .list_tools()
        .into_iter()
        .map(|t| t.name.to_string())
        .filter(|name| name.contains("install"))
        .collect();
    assert!(installers.is_empty(), "an MCP tool installs: {installers:?}");

    let cli_status = f.direct(|e| e.security_status());
    let (client, server) = boot(&f).await;
    let mcp_status = call(&client, "security_status", json!({})).await.unwrap();
    client.cancel().await.ok();
    server.abort();
    assert_eq!(mcp_status, cli_status);
    // A missing scanner Logos installs names the command, and so does a PATH
    // copy below the pinned minimum. CodeQL's names its licence flag, and its
    // row the acknowledgement: none yet (S-746).
    assert_eq!(row(&mcp_status, "trivy")["install"], "logos security install trivy");
    assert_eq!(row(&mcp_status, "trivy")["installed"], Value::Null);
    assert_eq!(row(&mcp_status, "checkov")["install"], "logos security install checkov");
    assert_eq!(row(&mcp_status, "codeql")["install"], "logos security install codeql --accept-codeql-licence");
    assert_eq!(row(&mcp_status, "codeql")["licence"], json!({"accepted": [], "pinned_accepted": false}));
    let semgrep = row(&mcp_status, "semgrep");
    assert_eq!(
        semgrep["install"], "logos security install semgrep",
        "a PATH copy below the pinned minimum (1.85.0) names the command"
    );
    assert_eq!(
        (semgrep["installed"]["copy"].as_str(), semgrep["installed"]["version"].as_str()),
        (Some("path"), Some("1.2.3")),
        "the copy in use and its probed version: {semgrep}"
    );

    let line = "`security_status` names\n  `logos security install`";
    let at = mcp::INSTRUCTIONS.find(line).expect("the instructions name the install command");
    assert!(at + line.len() <= 2048, "the install line ends at {} — past what a host delivers", at + line.len());
}

/// [FR-SC-17] parity: a scan narrowed by scanner and path is one payload on
/// both surfaces, and a path outside the root — exit 2 on the CLI — is refused
/// on MCP with the same message, before any scanner starts. Like the consent
/// refusal, it is the engine's error, so it travels as the tool's failure.
///
/// [FR-SC-17]: ../../docs/specs/requirements/FR-SC-17.md
#[tokio::test(flavor = "multi_thread")]
async fn a_scoped_scan_is_one_payload_on_both_surfaces_and_an_outside_path_is_refused() {
    let f = fixture();
    f.direct(|e| e.security_scan(true));
    let request = ScanRequest::new(vec!["semgrep".into()], vec!["app/db.py".into()]);
    // The second run of the same scoped report is the no-op MCP's run repeats.
    f.direct(|e| e.security_scan_selected(false, &request, |_| {}));
    let cli = f.direct(|e| e.security_scan_selected(false, &request, |_| {}));
    let (client, server) = boot(&f).await;
    let mcp = call(&client, "security_scan", json!({"scanners": ["semgrep"], "paths": ["app/db.py"]})).await.unwrap();
    assert_eq!(mcp, cli, "security_scan, scoped");
    assert_eq!(row(&mcp, "semgrep")["scope"], json!({"kind": "paths", "paths": ["app/db.py"]}));
    assert_eq!(row(&mcp, "bandit")["outcome"], "not_selected");

    let Err(ServiceError::McpError(outside)) = call(&client, "security_scan", json!({"paths": ["../outside"]})).await else {
        panic!("a path outside the root must be refused");
    };
    assert!(outside.message.contains("\"../outside\" resolves outside the project root"), "{}", outside.message);
    let Err(ServiceError::McpError(unknown)) = call(&client, "security_scan", json!({"scanners": ["nmap"]})).await else {
        panic!("an unregistered scanner must be refused");
    };
    assert!(unknown.message.contains("\"nmap\" is not a registered scanner"), "{}", unknown.message);
    client.cancel().await.ok();
    server.abort();
}

/// A secret found only in history is one payload on both surfaces ([S-767]):
/// its `historical` binding, its commit with the short spelling every surface
/// shows, and its unbound graph context.
///
/// [S-767]: ../../docs/planning/journal.md#s-767-a-secret-found-only-in-history-is-anchored-to-its-commit
#[tokio::test(flavor = "multi_thread")]
async fn a_secret_found_only_in_history_is_one_payload_on_both_surfaces() {
    let f = fixture();
    fs::write(f.root.join("app/legacy.py"), "API_KEY = \"lgs_l3gacy_R8wY2pQ5nM4kT7\"\n").unwrap();
    git(&f.root, &["add", "app/legacy.py"]);
    git(&f.root, &["commit", "-q", "-m", "legacy client"]);
    let out = Command::new("git").current_dir(&f.root).args(["rev-parse", "HEAD"]).output().unwrap();
    let commit = String::from_utf8_lossy(&out.stdout).trim().to_string();
    git(&f.root, &["rm", "-q", "app/legacy.py"]);
    git(&f.root, &["commit", "-q", "-m", "drop the legacy client"]);
    let report = f.root.parent().unwrap().join("history.sarif");
    let result = json!({
        "ruleId": "generic-api-key",
        "message": { "text": "generic-api-key has detected secret for file app/legacy.py." },
        "locations": [{ "physicalLocation": {
            "artifactLocation": { "uri": "app/legacy.py" },
            "region": { "startLine": 1, "startColumn": 1, "endLine": 1, "endColumn": 37, "snippet": { "text": "REDACTED" } }
        } }],
        "partialFingerprints": { "commitSha": commit, "email": "", "author": "", "date": "2026-10-01T09:00:00Z", "commitMessage": "" }
    });
    let body = json!({ "version": "2.1.0", "runs": [{ "tool": { "driver": { "name": "gitleaks", "semanticVersion": "v8.18.4" } }, "results": [result] }] });
    fs::write(&report, body.to_string()).unwrap();
    assert_eq!(f.direct(|e| e.security_ingest(&report))["historical"], 1);

    let (client, server) = boot(&f).await;
    let mcp = call(&client, "security_findings", json!({"scanner": "gitleaks"})).await.unwrap();
    client.cancel().await.ok();
    server.abort();
    let filter = FindingsFilter::parse(None, Some("gitleaks"), None, None, None, None, None).unwrap();
    assert_eq!(f.direct(|e| e.security_findings(&filter)), mcp, "security_findings");
    let finding = &mcp["findings"][0];
    assert_eq!(finding["binding"], "historical", "{mcp}");
    assert_eq!(finding["anchor"], json!({ "commit": commit, "short_commit": &commit[..7], "path": "app/legacy.py" }));
    assert_eq!(finding["context"]["enclosing"]["unbound_because"], "historical");
}

/// The code scope ([FR-SC-28], S-795) is one parameter on both surfaces and one
/// payload: no scope lists production and states what it hid, `test` and `all`
/// narrow exactly and intersect with the other filters, each finding carries
/// its `code_scope` and reason, and a bad token is the caller's mistake.
///
/// [FR-SC-28]: ../../docs/specs/requirements/FR-SC-28.md
#[tokio::test(flavor = "multi_thread")]
async fn the_code_scope_is_one_parameter_and_one_payload_on_both_surfaces() {
    let f = fixture();
    let report = f.root.parent().unwrap().join("scoped.sarif");
    let result = |uri: &str, line: i64| {
        json!({"ruleId": "custom.rule", "level": "error", "message": {"text": "tainted"},
            "locations": [{"physicalLocation": {"artifactLocation": {"uri": uri}, "region": {"startLine": line}}}]})
    };
    let sarif = json!({"version": "2.1.0", "runs": [{"tool": {"driver": {"name": "Custom SAST", "version": "1", "rules": []}},
        "results": [result("app/db.py", 12), result("tests/test_db.py", 3), result("app/testdata/seed.py", 1)]}]});
    fs::write(&report, sarif.to_string()).unwrap();
    f.direct(|e| e.security_ingest(&report));

    let (client, server) = boot(&f).await;
    let mut mcp = Vec::new();
    let cases: [(Option<&str>, Option<&str>); 5] =
        [(None, None), (Some("test"), None), (Some("all"), None), (Some("test"), Some("tests")), (None, Some("tests"))];
    for (scope, path) in cases {
        let mut args = json!({"scanner": "Custom SAST"});
        if let Some(scope) = scope {
            args["scope"] = json!(scope);
        }
        if let Some(path) = path {
            args["path"] = json!(path);
        }
        mcp.push(call(&client, "security_findings", args).await.unwrap());
    }
    let Err(ServiceError::McpError(bad)) = call(&client, "security_findings", json!({"scope": "tests"})).await else {
        panic!("an unknown scope is the caller's mistake");
    };
    assert_eq!(bad.code, ErrorCode::INVALID_PARAMS, "{}", bad.message);
    assert!(bad.message.contains("production, test, all"), "{}", bad.message);
    client.cancel().await.ok();
    server.abort();

    for ((scope, path), mcp) in cases.into_iter().zip(&mcp) {
        let filter = FindingsFilter::parse(None, Some("Custom SAST"), None, path, None, None, scope).unwrap();
        assert_eq!(&f.direct(|e| e.security_findings(&filter)), mcp, "scope {scope:?}, path {path:?}");
    }
    let listed = |payload: &Value| -> (Vec<(String, String, Value)>, u64) {
        let rows = payload["findings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| (f["report_path"].as_str().unwrap().to_string(), f["code_scope"].as_str().unwrap().to_string(), f["code_scope_reason"].clone()))
            .collect();
        (rows, payload["hidden_by_scope"].as_u64().unwrap())
    };
    let production = ("app/db.py".to_string(), "production".to_string(), Value::Null);
    let fixture_row = ("app/testdata/seed.py".to_string(), "test".to_string(), json!("test_path"));
    let test_row = ("tests/test_db.py".to_string(), "test".to_string(), json!("test_path"));
    assert_eq!(listed(&mcp[0]), (vec![production.clone()], 2), "production by default, the hidden two stated");
    assert_eq!(mcp[0]["scope"], "production");
    assert_eq!(listed(&mcp[1]), (vec![fixture_row.clone(), test_row.clone()], 1));
    assert_eq!(listed(&mcp[2]), (vec![production, fixture_row, test_row.clone()], 0));
    assert_eq!(listed(&mcp[3]), (vec![test_row], 0), "scope and path intersect");
    assert_eq!(listed(&mcp[4]), (vec![], 1), "the path kept one, the default scope hid it, and says so");
}

/// FR-SC-08, FR-SC-27: `nvd_api_key` reads `not_set`, then `set` with a key
/// in the scanning process's environment, on the Dependency-Check row alone —
/// the same payload on both surfaces, and never the key.
#[tokio::test(flavor = "multi_thread")]
async fn the_nvd_api_key_state_is_one_payload_on_both_surfaces_and_never_the_key() {
    let mut f = fixture();
    for (key, state) in [(None, "not_set"), (NvdApiKey::new(PLANTED), "set")] {
        f.env.nvd_api_key = key;
        let cli_status = f.direct(|e| e.security_status());
        assert_eq!(row(&cli_status, "dependency-check")["nvd_api_key"], state, "{cli_status}");
        let carrying: Vec<&str> = cli_status["scanners"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r.get("nvd_api_key").is_some())
            .map(|r| r["scanner"].as_str().unwrap())
            .collect();
        assert_eq!(carrying, ["dependency-check"]);
        let (client, server) = boot(&f).await;
        let mcp_status = call(&client, "security_status", json!({})).await.unwrap();
        assert_eq!(mcp_status, cli_status);
        assert!(!mcp_status.to_string().contains(PLANTED), "{mcp_status}");
        client.cancel().await.ok();
        server.abort();
    }
}

/// Every `(category id, state.findings, production_findings)` of a payload's
/// `categories`, covered or not.
fn category_counts(categories: &Value) -> Vec<(String, Option<u64>, Option<u64>)> {
    categories
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (c["id"].as_str().unwrap().to_string(), c["state"]["findings"].as_u64(), c["production_findings"].as_u64()))
        .collect()
}

/// Each category's production count ([FR-SC-28], S-796 D2): one figure on the
/// CLI payload, the MCP payload and the web page's, on one fixture holding
/// findings in production code, in test paths, in a test symbol, and a
/// suppressed one. `state.findings` still counts every scope; the new field is
/// the part in production code, suppressed findings in both — what the page's
/// OWASP list shows under its default scope — and it equals the count taken
/// over the page's own findings list, in both editions.
///
/// [FR-SC-28]: ../../docs/specs/requirements/FR-SC-28.md
#[tokio::test(flavor = "multi_thread")]
async fn the_production_count_per_category_is_one_figure_on_the_cli_mcp_and_web_payloads() {
    let f = fixture();
    // A test symbol outside a test path: only the graph classifies it.
    fs::write(f.root.join("app/checks.py"), "def test_lookup():\n    return 1\n").unwrap();
    Engine::start(&f.root).expect("engine").index();

    let semgrep = Path::new(env!("CARGO_MANIFEST_DIR")).join("../logos-core/tests/fixtures/sarif/semgrep.sarif");
    let mut sarif: Value = serde_json::from_str(&fs::read_to_string(semgrep).unwrap()).unwrap();
    let results = sarif["runs"][0]["results"].as_array_mut().unwrap();
    let template = results[0].clone();
    // Two production findings (db.py:12 and :18) and three more of the first
    // one's CWE-89 rule: a test path, a fixture path and a test symbol.
    for (n, (uri, line)) in [("tests/test_db.py", 3), ("app/testdata/seed.py", 1), ("app/checks.py", 2)].into_iter().enumerate() {
        let mut twin = template.clone();
        twin["locations"][0]["physicalLocation"]["artifactLocation"]["uri"] = json!(uri);
        twin["locations"][0]["physicalLocation"]["region"]["startLine"] = json!(line);
        twin["locations"][0]["physicalLocation"]["region"]["endLine"] = json!(line);
        twin["fingerprints"] = json!({"matchBasedId/v1": format!("{n:0>8}-twin-{uri}")});
        results.push(twin);
    }
    let report = f.root.parent().unwrap().join("production-count.sarif");
    fs::write(&report, sarif.to_string()).unwrap();
    f.direct(|e| e.security_ingest(&report));

    // Suppress the finding at db.py:18, a production one.
    let all = FindingsFilter::parse(None, None, None, None, None, None, Some("all")).unwrap();
    let before = f.direct(|e| e.security_findings(&all));
    let listed = before["findings"].as_array().unwrap();
    assert_eq!(listed.len(), 5, "{before}");
    let scopes: Vec<(&str, &str)> =
        listed.iter().map(|x| (x["report_path"].as_str().unwrap(), x["code_scope"].as_str().unwrap())).collect();
    assert_eq!(scopes.iter().filter(|(_, s)| *s == "production").count(), 2, "{scopes:?}");
    let suppressed = listed.iter().find(|x| x["start_line"] == 18).unwrap()["fingerprint"].as_str().unwrap().to_string();
    fs::write(
        f.root.join(".logos/security-ignore.toml"),
        format!("[[ignore]]\nfingerprint = \"{suppressed}\"\nreason = \"fixture: never deployed\"\n"),
    )
    .unwrap();

    let (client, server) = boot(&f).await;
    let mcp = call(&client, "security_findings", json!({})).await.unwrap();
    client.cancel().await.ok();
    server.abort();
    let cli = f.direct(|e| e.security_findings(&FindingsFilter::parse(None, None, None, None, None, None, None).unwrap()));
    let page = f.direct(|e| e.security_page());

    // The fixture holds a suppressed production finding: the page counts it apart, and the
    // production figure below still includes it.
    assert_eq!(page["counts"]["excluded"]["suppressed"], 1, "{}", page["counts"]);

    // The figures are over every stored finding, whatever the other filters keep.
    for (severity, suppression) in [(Some("critical"), None), (None, Some("active")), (None, Some("suppressed"))] {
        let filter = FindingsFilter::parse(severity, None, None, None, suppression, None, None).unwrap();
        let filtered = f.direct(|e| e.security_findings(&filter));
        assert_eq!(filtered["categories"], cli["categories"], "severity {severity:?}, suppression {suppression:?}");
    }

    // The three payloads state the same categories, field for field.
    assert_eq!(cli["categories"], mcp["categories"], "CLI and MCP");
    assert_eq!(cli["categories"], page["findings"]["categories"], "CLI and web findings");
    assert_eq!(cli["categories"], page["editions"][0]["categories"], "CLI and web edition 2025");

    // One injection finding in each scope the fixture plants: five stored,
    // two in production (one of them suppressed), three in test code.
    let counts = category_counts(&cli["categories"]);
    let injection = counts.iter().find(|(id, ..)| id == "A05:2025").unwrap();
    assert_eq!(injection, &("A05:2025".to_string(), Some(5), Some(2)), "{counts:?}");
    assert!(counts.iter().all(|(_, covered, production)| production.is_some() == covered.is_some()), "{counts:?}");
    let not_covered = cli["categories"].as_array().unwrap().iter().find(|c| c["state"]["state"] == "not_covered").unwrap();
    assert!(not_covered.get("production_findings").is_none(), "a not-covered category states no count: {not_covered}");

    // The page's own figure, taken over its findings list under Production,
    // is the field's, in each edition (the second shares the first's findings).
    let page_findings = page["findings"]["findings"].as_array().unwrap();
    assert_eq!(page_findings.len(), 5, "the page lists every scope");
    let in_production = page_findings.iter().filter(|x| x["code_scope"] == "production" && x["owasp"] == json!(["A05:2025"])).count();
    assert_eq!(in_production as u64, injection.2.unwrap());
    let injection_2021 = category_counts(&page["editions"][1]["categories"]).into_iter().find(|(id, ..)| id == "A03:2021").unwrap();
    assert_eq!(injection_2021, ("A03:2021".to_string(), Some(5), Some(2)));
    assert_eq!(page["counts"]["excluded"]["test"], 3, "{}", page["counts"]);
}
