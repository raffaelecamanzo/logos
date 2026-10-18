//! The Security page's Install, Update and Install & scan (S-747, [FR-SC-24],
//! [FR-SC-22], [FR-SC-23], [UAT-SC-07]) through the real router, in process:
//! the one consent dialog's plan, the `tool-install` job that installs and then
//! scans, its cancel mid-download, and Update replacing the managed copy.
//!
//! No test reaches the internet ([ADR-88]'s fitness function). Each fixture
//! builds its own artifacts — shell scripts named after the release-binary
//! scanners, printing a version and writing an empty SARIF report — packs them
//! as the builtin manifest's layouts and pins them at
//! `https://fixtures.invalid/<version>/<file>`. The opt-in Dependency-Check
//! and CodeQL entries keep their builtin pins: they are offered, never
//! fetched. `curl` is a stub on the engine's search path that serves those
//! files and records each URL; the cancel test's stub writes half a download
//! and sleeps until it is killed. The engine's user, tools and search
//! directories are temporary, so the process environment is never changed.
//! Every test runs under [`BOUND`].
//!
//! [FR-SC-24]: ../../docs/specs/requirements/FR-SC-24.md
//! [FR-SC-22]: ../../docs/specs/requirements/FR-SC-22.md
//! [FR-SC-23]: ../../docs/specs/requirements/FR-SC-23.md
//! [UAT-SC-07]: ../../docs/specs/requirements/UAT-SC-07.md
//! [ADR-88]: ../../docs/specs/architecture/decisions/ADR-88.md
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
    Router,
};
use http_body_util::BodyExt;
use logos_core::config::{load_scan_consent, INSTALL_CONSENT_FILE};
use logos_core::security::{scanner, ScanEnvironment};
use logos_core::tools::{sha256_file, Layout, Manifest, Platform, CODEQL_LICENCE};
use logos_core::Engine;
use serde_json::{json, Value};
use tempfile::TempDir;
use tower::ServiceExt;
use web::{IntentToken, INTENT_HEADER};

/// The whole of any one test, installs and scanners included.
const BOUND: Duration = Duration::from_secs(120);

/// How long a job or a stub is waited for before the test fails.
const JOB_WAIT: Duration = Duration::from_secs(30);

const ORIGIN: &str = "http://127.0.0.1:4983";

/// Where every fixture artifact claims to come from.
const HOST: &str = "fixtures.invalid";

/// One machine: a project, its user-level directory, a tools directory, a
/// search path holding the stub `curl` (and the real `tar`, `gzip`) and a
/// "server" directory of artifacts.
struct World {
    _tmp: TempDir,
    base: PathBuf,
}

impl World {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let w = World { _tmp: tmp, base };
        for dir in [w.bin(), w.rec(), w.root(), w.user(), w.server()] {
            fs::create_dir_all(dir).unwrap();
        }
        for program in ["tar", "gzip"] {
            if let Some(found) = on_process_path(program) {
                std::os::unix::fs::symlink(found, w.bin().join(program)).unwrap();
            }
        }
        w.curl(&format!(
            "rel=\"${{url#https://{HOST}/}}\"\n/bin/cp '{}/'\"$rel\" \"$out\" || exit 22",
            w.server().display()
        ));
        fs::write(w.root().join("main.py"), "def main():\n    return 1\n").unwrap();
        git(&w.root(), &["init", "-q", "-b", "main"]);
        git(&w.root(), &["add", "."]);
        git(&w.root(), &["commit", "-q", "-m", "seed"]);
        Engine::start(w.root()).expect("engine").index();
        w
    }

    fn bin(&self) -> PathBuf {
        self.base.join("bin")
    }
    fn rec(&self) -> PathBuf {
        self.base.join("rec")
    }
    fn root(&self) -> PathBuf {
        self.base.join("project")
    }
    fn user(&self) -> PathBuf {
        self.base.join("user")
    }
    fn tools(&self) -> PathBuf {
        self.base.join("tools")
    }
    fn server(&self) -> PathBuf {
        self.base.join("server")
    }

    /// The router over this machine, installing from `manifest`, and its
    /// engine.
    fn page(&self, manifest: &Manifest) -> Page {
        self.page_at(&self.root(), manifest)
    }

    /// [`page`](Self::page) for the project at `root`, sharing this machine's
    /// user, tools and search directories.
    fn page_at(&self, root: &Path, manifest: &Manifest) -> Page {
        let engine = Engine::start(root)
            .expect("engine")
            .with_scan_environment(ScanEnvironment {
                user_dir: Some(self.user()),
                search_path: Some(self.bin().into_os_string()),
                tools_dir: Some(self.tools()),
            })
            .with_tools_manifest(manifest.clone());
        let engine = Arc::new(engine);
        let intent = IntentToken::generate();
        let router = web::router_with_intent(Arc::clone(&engine), intent.clone());
        Page { router, intent, engine }
    }

    /// A stub `curl` that records each URL, then runs `body` with `$url` and
    /// `$out` (its `--output`) set.
    fn curl(&self, body: &str) {
        script(
            &self.bin().join("curl"),
            &format!(
                "#!/bin/sh\nout=; url=\nwhile [ $# -gt 0 ]; do\n  case \"$1\" in\n    --output) out=\"$2\"; shift 2 ;;\n    *) url=\"$1\"; shift ;;\n  esac\ndone\nprintf '%s\\n' \"$url\" >> '{}/curl.log'\n{body}\n",
                self.rec().display()
            ),
        );
    }

    fn downloads(&self) -> Vec<String> {
        fs::read_to_string(self.rec().join("curl.log"))
            .map(|t| t.lines().map(str::to_string).collect())
            .unwrap_or_default()
    }

    /// The builtin manifest's release-binary default tools pinned at `pins`
    /// (default: their own version), each artifact for this platform built
    /// here and served from `server/<version>/`, and its opt-in tools as
    /// built in. The Python tools are left out: their `uv` route is
    /// `logos-core/tests/tools_python.rs`'s.
    fn manifest(&self, pins: &[(&str, &str)]) -> Manifest {
        let platform = Platform::current().expect("the suite runs where managed installs exist");
        let mut manifest = Manifest::builtin().clone();
        manifest.tools.retain(|t| t.python.is_none());
        for entry in manifest.tools.iter_mut().filter(|t| t.is_default()) {
            if let Some((_, version)) = pins.iter().find(|(t, _)| *t == entry.tool) {
                entry.version = version.to_string();
            }
            let mut artifact = entry.artifact(platform).unwrap().clone();
            let dir = self.server().join(&entry.version);
            fs::create_dir_all(&dir).unwrap();
            let file = match &artifact.layout {
                Layout::Binary => {
                    let file = dir.join(&entry.binary);
                    script(&file, &scanner_script(&entry.tool, &entry.version));
                    file
                }
                Layout::TarGz { member } => {
                    let src = self.base.join("src").join(&entry.tool).join(&entry.version);
                    fs::create_dir_all(&src).unwrap();
                    script(&src.join(member), &scanner_script(&entry.tool, &entry.version));
                    let file = dir.join(format!("{}.tar.gz", entry.tool));
                    let status =
                        Command::new("tar").arg("-czf").arg(&file).arg("-C").arg(&src).arg(member).status().unwrap();
                    assert!(status.success(), "tar packs {}", entry.tool);
                    file
                }
                Layout::Tree { .. } => unreachable!("the default set holds no tree archive"),
            };
            artifact.url = format!("https://{HOST}/{}/{}", entry.version, file.file_name().unwrap().to_str().unwrap());
            artifact.sha256 = sha256_file(&file).unwrap();
            artifact.size = fs::metadata(&file).unwrap().len();
            entry.artifacts = vec![artifact];
        }
        manifest
    }

    /// The tool directories in the tools directory, `<tool>/<version>`
    /// sorted, the staging directory excluded.
    fn installed(&self) -> Vec<String> {
        let mut found = Vec::new();
        for tool in fs::read_dir(self.tools()).into_iter().flatten().flatten() {
            let name = tool.file_name().to_string_lossy().into_owned();
            if name == ".staging" {
                continue;
            }
            for version in fs::read_dir(tool.path()).unwrap().flatten() {
                found.push(format!("{name}/{}", version.file_name().to_string_lossy()));
            }
        }
        found.sort();
        found
    }

    /// Every file left under the staging directory.
    fn staging(&self) -> Vec<PathBuf> {
        fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
            for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
                if entry.path().is_dir() {
                    walk(&entry.path(), out);
                } else {
                    out.push(entry.path());
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.tools().join(".staging"), &mut out);
        out
    }
}

/// One served Security page.
struct Page {
    router: Router,
    intent: IntentToken,
    engine: Arc<Engine>,
}

impl Page {
    async fn send(&self, req: Request<Body>) -> (StatusCode, Value) {
        let resp = self.router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
        (status, body)
    }

    /// A same-origin, intent-bearing `POST`.
    async fn post(&self, path: &str, body: &Value) -> (StatusCode, Value) {
        let req = Request::builder()
            .method(Method::POST)
            .uri(path)
            .header(header::HOST, "127.0.0.1:4983")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ORIGIN, ORIGIN)
            .header(INTENT_HEADER, self.intent.as_str())
            .body(Body::from(body.to_string()))
            .unwrap();
        self.send(req).await
    }

    async fn get(&self, path: &str) -> (StatusCode, Value) {
        let req =
            Request::builder().method(Method::GET).uri(path).header(header::HOST, "127.0.0.1:4983").body(Body::empty()).unwrap();
        self.send(req).await
    }

    /// The member's install job, once it is no longer running.
    async fn ended(&self) -> Value {
        let deadline = Instant::now() + JOB_WAIT;
        loop {
            let (status, body) = self.get("/api/v1/security/install").await;
            assert_eq!(status, StatusCode::OK, "{body}");
            if body["job"]["state"] != "running" {
                return body["job"].clone();
            }
            assert!(Instant::now() < deadline, "the install is still running: {body}");
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// The scanner's status row.
    fn row(&self, scanner: &str) -> Value {
        let status = serde_json::to_value(self.engine.security_status().unwrap()).unwrap();
        status["scanners"].as_array().unwrap().iter().find(|r| r["scanner"] == scanner).unwrap().clone()
    }
}

/// A scanner that prints `version` and, given its output flag, writes an empty
/// SARIF report under its registry driver name.
fn scanner_script(tool: &str, version: &str) -> String {
    let driver = scanner(tool).unwrap().driver;
    format!(
        "#!/bin/sh\n\
         case \"$1\" in --version|version|-version) printf '%s\\n' '{tool} version {version}'; exit 0;; esac\n\
         out=''; prev=''\n\
         for a in \"$@\"; do case \"$prev\" in --output|--report-path|-out) out=\"$a\";; esac; prev=\"$a\"; done\n\
         if [ -n \"$out\" ]; then printf '%s' '{{\"version\":\"2.1.0\",\"runs\":[{{\"tool\":{{\"driver\":{{\"name\":\"{driver}\",\"rules\":[]}}}},\"results\":[]}}]}}' > \"$out\"; fi\n\
         exit 0\n"
    )
}

fn script(path: &Path, body: &str) {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, body).unwrap();
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755)).unwrap();
    fs::rename(&tmp, path).unwrap();
}

fn on_process_path(program: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join(program)).find(|p| p.is_file())
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

/// The plan's lines of kind `line`.
fn lines<'a>(plan: &'a Value, line: &str) -> Vec<&'a Value> {
    plan["lines"].as_array().unwrap().iter().filter(|l| l["line"] == line).collect()
}

/// The `<tool> <version>` of every tool and support line the action installs:
/// what an accepting dialog sends back.
fn accepted(plan: &Value) -> Vec<String> {
    plan["lines"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|l| (l["line"] == "tool" && l["offered"] == false) || l["line"] == "support")
        .map(|l| format!("{} {}", l["tool"].as_str().unwrap(), l["version"].as_str().unwrap()))
        .collect()
}

/// `(phase, tool-or-scanner)` of every replayed event.
fn steps(job: &Value) -> Vec<(String, String)> {
    job["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            let event = &e["event"];
            let name = event.get("tool").or_else(|| event.get("scanner")).and_then(Value::as_str).unwrap_or("");
            (event["phase"].as_str().unwrap().to_string(), name.to_string())
        })
        .collect()
}

async fn bounded(test: impl std::future::Future<Output = ()>) {
    tokio::time::timeout(BOUND, test).await.expect("the test ran past its bound");
}

/// Install & scan on a machine with none of the scanners: the first start
/// asks, with nothing fetched or recorded — every default tool with its
/// version, source and size, Dependency-Check and CodeQL offered unticked
/// (the Java runtime's size, the CodeQL licence a line of its own), and the
/// project's network consent. Accepting runs one `tool-install` job that
/// installs the default tools and then scans them, the opt-in pair untouched.
#[tokio::test]
async fn install_and_scan_asks_once_then_installs_the_default_scanners_and_scans_them() {
    bounded(async {
        let w = World::new();
        let manifest = w.manifest(&[]);
        let page = w.page(&manifest);
        let defaults: Vec<&str> = manifest.tools.iter().filter(|t| t.is_default()).map(|t| t.tool.as_str()).collect();
        assert_eq!(defaults.len(), 4, "the fixture installs the four release-binary scanners");

        let (status, asked) = page.post("/api/v1/security/install", &json!({ "scan": true })).await;
        assert_eq!(status, StatusCode::CONFLICT, "{asked}");
        let plan = &asked["plan"];
        let tools = lines(plan, "tool");
        let named: Vec<&str> = tools.iter().filter(|l| l["offered"] == false).map(|l| l["tool"].as_str().unwrap()).collect();
        assert_eq!(named.len(), 4, "{plan}");
        for tool in &defaults {
            let line = tools.iter().find(|l| l["tool"] == *tool).unwrap_or_else(|| panic!("{tool} is not named: {plan}"));
            let entry = manifest.tool(tool).unwrap();
            assert_eq!(
                (line["version"].as_str(), line["source"].as_str(), line["size"].as_u64(), line["recorded"].as_bool()),
                (Some(entry.version.as_str()), Some(HOST), entry.download_size(Platform::current().unwrap()), Some(false)),
                "{line}"
            );
        }
        let mut offered: Vec<&str> = tools.iter().filter(|l| l["offered"] == true).map(|l| l["tool"].as_str().unwrap()).collect();
        offered.sort_unstable();
        assert_eq!(offered, ["codeql", "dependency-check"], "the opt-in pair is offered, unticked: {plan}");
        let java = lines(plan, "support");
        assert_eq!(java.len(), 1, "{plan}");
        assert_eq!((java[0]["tool"].as_str(), java[0]["for_tools"].clone()), (Some("java"), json!(["dependency-check"])));
        assert!(java[0]["size"].as_u64().unwrap() > 10_000_000, "the Java runtime's size is stated: {}", java[0]);
        let licence = lines(plan, "licence");
        assert_eq!(licence.len(), 1, "{plan}");
        assert_eq!((licence[0]["tool"].as_str(), licence[0]["terms"].as_str()), (Some("codeql"), Some(CODEQL_LICENCE.terms)));
        let scan_consent: Vec<&str> = plan["scan_consent"].as_array().unwrap().iter().map(|s| s.as_str().unwrap()).collect();
        for tool in defaults.iter().chain(&["dependency-check", "codeql"]) {
            assert!(scan_consent.contains(tool), "{tool} needs the project's network consent: {plan}");
        }
        assert!(w.downloads().is_empty(), "nothing is fetched before the consent");
        assert!(fs::read_dir(w.user()).unwrap().next().is_none(), "nothing is recorded before the consent");

        // The install lines answered but not the network consent: asked again.
        let (status, again) = page.post("/api/v1/security/install", &json!({ "scan": true, "consent": accepted(plan) })).await;
        assert_eq!(status, StatusCode::CONFLICT, "the scan's network consent is needed too: {again}");
        assert!(w.downloads().is_empty() && fs::read_dir(w.user()).unwrap().next().is_none(), "nothing is fetched or recorded");

        // The dialog accepted as shown: the default lines and their scanners,
        // the opt-in lines left unticked.
        let answer = json!({
            "scan": true,
            "consent": accepted(plan),
            "scan_consent": defaults,
        });
        let (status, started) = page.post("/api/v1/security/install", &answer).await;
        assert_eq!(status, StatusCode::OK, "{started}");
        assert_eq!((started["joined"].as_bool(), started["job"]["kind"].as_str()), (Some(false), Some("tool-install")));
        let job = page.ended().await;
        assert_eq!(job["state"], "completed", "{job}");

        let installed: Vec<(String, String)> = job["result"]["install"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| (t["tool"].as_str().unwrap().to_string(), t["outcome"].as_str().unwrap().to_string()))
            .collect();
        assert_eq!(installed, defaults.iter().map(|t| (t.to_string(), "installed".to_string())).collect::<Vec<_>>());
        for tool in &defaults {
            let run = job["result"]["scan"]["scanners"].as_array().unwrap().iter().find(|r| r["scanner"] == *tool).unwrap();
            assert_eq!(run["outcome"], "ran", "{tool}: {run}");
            let row = page.row(tool);
            assert_eq!(row["installed"]["copy"], "managed", "{row}");
        }
        // The steps streamed in order: each tool's install, then the scan.
        let steps = steps(&job);
        let first_scan = steps.iter().position(|(phase, _)| !phase.starts_with("install_")).expect("the scan reported its steps");
        assert!(steps[first_scan..].iter().all(|(phase, _)| phase == "started" || phase == "finished"), "{steps:?}");
        for tool in &defaults {
            assert!(steps.contains(&("install_started".to_string(), tool.to_string())), "{steps:?}");
            assert!(steps.contains(&("install_finished".to_string(), tool.to_string())), "{steps:?}");
        }

        assert_eq!(w.downloads().len(), 4, "only the four default tools are fetched: {:?}", w.downloads());
        assert!(!w.tools().join("dependency-check").exists() && !w.tools().join("codeql").exists() && !w.tools().join("java").exists());
        let record = fs::read_to_string(w.user().join(INSTALL_CONSENT_FILE)).unwrap();
        assert!(!record.contains("codeql") && !record.contains("dependency-check"), "{record}");
        assert_eq!(page.row("codeql")["state"], "not_installed");
        // The project's network consent names the four scanners the dialog
        // asked for, never an opt-in tool left unticked.
        let consented = load_scan_consent(Some(&w.user())).unwrap().consented(&w.root());
        let mut expected: Vec<&str> = defaults.clone();
        expected.sort_unstable();
        assert_eq!(consented.iter().map(String::as_str).collect::<Vec<_>>(), expected);
    })
    .await;
}

/// Cancelling during the install kills the download running, leaves no
/// partial tool and no staging, starts no further tool, and the job's last
/// step names the tools that landed. While the install runs, Run scan is
/// refused rather than run beside it.
#[tokio::test]
async fn cancelling_mid_download_leaves_no_partial_tool_and_reports_what_was_installed() {
    bounded(async {
        let w = World::new();
        let manifest = w.manifest(&[]);
        let page = w.page(&manifest);
        let defaults: Vec<&str> = manifest.tools.iter().filter(|t| t.is_default()).map(|t| t.tool.as_str()).collect();
        // The second tool's download writes half the file and hangs.
        let hanging = defaults[1];
        w.curl(&format!(
            "rel=\"${{url#https://{HOST}/}}\"\ncase \"$url\" in\n  *{hanging}*) printf 'partial' > \"$out\"; /bin/sleep 60 & echo $$ $! > '{rec}/curl.pids'; wait ;;\n  *) /bin/cp '{server}/'\"$rel\" \"$out\" || exit 22 ;;\nesac",
            rec = w.rec().display(),
            server = w.server().display()
        ));

        let (_, asked) = page.post("/api/v1/security/install", &json!({ "scan": true })).await;
        let answer = json!({ "scan": true, "consent": accepted(&asked["plan"]), "scan_consent": defaults });
        let (status, started) = page.post("/api/v1/security/install", &answer).await;
        assert_eq!(status, StatusCode::OK, "{started}");
        let deadline = Instant::now() + JOB_WAIT;
        let (leader, sleeper) = loop {
            if let Some(line) = fs::read_to_string(w.rec().join("curl.pids")).ok().filter(|s| s.ends_with('\n')) {
                let pids: Vec<u32> = line.split_whitespace().map(|p| p.parse().unwrap()).collect();
                break (pids[0], pids[1]);
            }
            assert!(Instant::now() < deadline, "the download never started hanging");
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        assert_eq!(w.staging().len(), 1, "the half download sits in staging: {:?}", w.staging());

        let (status, refused) = page.post("/api/v1/security/scan", &json!({})).await;
        assert_eq!(status, StatusCode::CONFLICT, "a scan beside the install is refused: {refused}");
        assert!(refused["error"].as_str().unwrap().contains("being installed"), "{refused}");
        // A second install — another action, or the same — is refused, never
        // joined: the server would answer for work it does not do.
        for second in [json!({ "scan": true }), json!({ "tools": ["gosec"], "consent": accepted(&asked["plan"]) })] {
            let (status, refused) = page.post("/api/v1/security/install", &second).await;
            assert_eq!(status, StatusCode::CONFLICT, "{second}: {refused}");
            assert!(refused["error"].as_str().unwrap().contains("already running"), "{refused}");
            assert!(refused.get("plan").is_none(), "{refused}");
        }
        let (_, current) = page.get("/api/v1/security/install").await;
        assert_eq!(current["job"]["id"], started["job"]["id"], "no other install job took the slot: {current}");

        let (status, cancelling) = page.post("/api/v1/security/install/cancel", &json!({})).await;
        assert_eq!((status, cancelling["job"]["id"].clone()), (StatusCode::OK, started["job"]["id"].clone()), "{cancelling}");
        let job = page.ended().await;
        assert_eq!(job["state"], "cancelled", "{job}");

        // The download was killed: neither the hanging curl nor its child runs.
        let deadline = Instant::now() + Duration::from_secs(10);
        let alive = |pid: &u32| {
            Command::new("kill").args(["-0", &pid.to_string()]).stderr(Stdio::null()).status().unwrap().success()
        };
        while [leader, sleeper].iter().any(alive) {
            assert!(Instant::now() < deadline, "the download is still running");
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        let events = job["events"].as_array().unwrap();
        let last = &events.last().unwrap()["event"];
        let first = manifest.tool(defaults[0]).unwrap();
        assert_eq!(last, &json!({ "phase": "cancelled", "installed": [format!("{} {}", first.tool, first.version)] }), "{job}");
        let interrupted = events
            .iter()
            .find(|e| e["event"]["phase"] == "install_finished" && e["event"]["tool"] == hanging)
            .expect("the interrupted tool settles");
        assert_eq!(interrupted["event"]["outcome"], "cancelled", "{interrupted}");
        assert_eq!(w.installed(), [format!("{}/{}", first.tool, first.version)], "no partial tool is left");
        assert!(w.staging().is_empty(), "no download is left in staging: {:?}", w.staging());
        assert_eq!(w.downloads().len(), 2, "no tool starts after the cancel: {:?}", w.downloads());
        let begun: Vec<String> = steps(&job).into_iter().filter(|(phase, _)| phase == "install_started").map(|(_, tool)| tool).collect();
        assert_eq!(begun, [defaults[0], hanging], "no tool's install begins after the cancel");
        assert_eq!(page.row(hanging)["state"], "not_installed");
        assert!(!steps(&job).iter().any(|(phase, _)| phase == "started"), "no scan follows a cancelled install");
    })
    .await;
}

/// A Run scan refused because an install is running records nothing — not the
/// network consent its dialog accepted either — as the install start refuses
/// before it records.
#[tokio::test]
async fn a_scan_refused_beside_an_install_records_no_consent() {
    bounded(async {
        let w = World::new();
        let manifest = w.manifest(&[]);
        // An installed scanner this project has not consented to.
        script(&w.bin().join("bandit"), &scanner_script("bandit", "1.7.10"));
        let page = w.page(&manifest);
        let tool = manifest.tools.iter().find(|t| t.is_default()).unwrap().tool.clone();
        w.curl("/bin/sleep 60 & wait");
        let (_, asked) = page.post("/api/v1/security/install", &json!({ "tools": [tool] })).await;
        let (status, started) =
            page.post("/api/v1/security/install", &json!({ "tools": [tool], "consent": accepted(&asked["plan"]) })).await;
        assert_eq!(status, StatusCode::OK, "{started}");

        let (status, refused) = page.post("/api/v1/security/scan", &json!({ "consent": ["bandit"] })).await;
        assert_eq!(status, StatusCode::CONFLICT, "{refused}");
        assert!(refused["error"].as_str().unwrap().contains("being installed"), "{refused}");
        let consented = load_scan_consent(Some(&w.user())).unwrap().consented(&w.root());
        assert!(consented.is_empty(), "a refused scan records no consent: {consented:?}");

        page.post("/api/v1/security/install/cancel", &json!({})).await;
        assert_eq!(page.ended().await["state"], "cancelled");
    })
    .await;
}

/// A managed copy older than the pin reads `update available`; the row's
/// Update asks for the new version's consent, then replaces the managed copy
/// and removes the old one. A row's Install of CodeQL asks for its licence as
/// a line of its own and is refused until it is ticked, with nothing fetched.
#[tokio::test]
async fn update_replaces_the_managed_copy_and_codeql_waits_for_its_licence_line() {
    bounded(async {
        let w = World::new();
        let old = w.manifest(&[]);
        let trivy = old.tool("trivy").unwrap().version.clone();
        let first = w.page(&old);
        let (_, asked) = first.post("/api/v1/security/install", &json!({ "tools": ["trivy"] })).await;
        let (status, _) = first.post("/api/v1/security/install", &json!({ "tools": ["trivy"], "consent": accepted(&asked["plan"]) })).await;
        assert_eq!(status, StatusCode::OK);
        let done = first.ended().await;
        assert_eq!(done["state"], "completed", "{done}");

        let page = w.page(&w.manifest(&[("trivy", "99.0.0")]));
        let row = page.row("trivy");
        assert_eq!(
            (row["installed"]["copy"].as_str(), row["installed"]["version"].as_str(), row["installed"]["update_available"].as_str()),
            (Some("managed"), Some(trivy.as_str()), Some("99.0.0")),
            "{row}"
        );
        let update = json!({ "tools": ["trivy"], "update": true });
        let (status, asked) = page.post("/api/v1/security/install", &update).await;
        assert_eq!(status, StatusCode::CONFLICT, "the new version is asked for: {asked}");
        assert_eq!(accepted(&asked["plan"]), ["trivy 99.0.0"]);
        let (status, _) =
            page.post("/api/v1/security/install", &json!({ "tools": ["trivy"], "update": true, "consent": ["trivy 99.0.0"] })).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(page.ended().await["state"], "completed");
        assert_eq!(w.installed(), ["trivy/99.0.0"], "the managed copy is replaced");
        let row = page.row("trivy");
        assert_eq!((row["installed"]["version"].as_str(), row["installed"]["update_available"].clone()), (Some("99.0.0"), Value::Null));

        let downloads = w.downloads().len();
        let codeql = json!({ "tools": ["codeql"] });
        let (status, asked) = page.post("/api/v1/security/install", &codeql).await;
        assert_eq!(status, StatusCode::CONFLICT, "{asked}");
        let tool = lines(&asked["plan"], "tool");
        assert_eq!((tool.len(), tool[0]["tool"].as_str(), tool[0]["offered"].as_bool()), (1, Some("codeql"), Some(false)));
        let licence = lines(&asked["plan"], "licence");
        assert_eq!((licence.len(), licence[0]["flag"].as_str()), (1, Some(CODEQL_LICENCE.flag)));
        // The install consent ticked, the licence line not: asked again.
        let (status, again) = page.post("/api/v1/security/install", &json!({ "tools": ["codeql"], "consent": accepted(&asked["plan"]) })).await;
        assert_eq!(status, StatusCode::CONFLICT, "{again}");
        assert_eq!(lines(&again["plan"], "licence").len(), 1);
        assert_eq!(w.downloads().len(), downloads, "nothing is fetched while the licence is unanswered");
        let record = fs::read_to_string(w.user().join(INSTALL_CONSENT_FILE)).unwrap();
        assert!(!record.contains("codeql"), "{record}");

        // Ticking Dependency-Check in Install & scan makes its line, its Java
        // runtime's line and its network consent needed: answering only the
        // default lines is asked again.
        let (_, asked) = page.post("/api/v1/security/install", &json!({ "scan": true })).await;
        let defaults: Vec<String> =
            asked["plan"]["scan_consent"].as_array().unwrap().iter().map(|s| s.as_str().unwrap().to_string()).collect();
        let ticked = json!({ "scan": true, "opt_in": ["dependency-check"], "consent": accepted(&asked["plan"]), "scan_consent": defaults });
        let (status, again) = page.post("/api/v1/security/install", &ticked).await;
        assert_eq!(status, StatusCode::CONFLICT, "{again}");
        assert_eq!(w.downloads().len(), downloads, "nothing is fetched for a ticked line left unanswered");
        // A tool's line accepted without its support line's — Dependency-Check
        // without the Java runtime it brings — is asked again, with nothing
        // fetched or recorded for either.
        let dc = Manifest::builtin().tool("dependency-check").unwrap().version.clone();
        let (status, again) =
            page.post("/api/v1/security/install", &json!({ "tools": ["dependency-check"], "consent": [format!("dependency-check {dc}")] })).await;
        assert_eq!(status, StatusCode::CONFLICT, "{again}");
        let java = lines(&again["plan"], "support");
        assert_eq!((java.len(), java[0]["tool"].as_str(), java[0]["for_tools"].clone()), (1, Some("java"), json!(["dependency-check"])), "{again}");
        assert_eq!(w.downloads().len(), downloads, "nothing is fetched while the runtime's line is unanswered");
        let record = fs::read_to_string(w.user().join(INSTALL_CONSENT_FILE)).unwrap();
        assert!(!record.contains("dependency-check") && !record.contains("java"), "{record}");
        // Only an offered tool can be ticked.
        let (status, refused) = page.post("/api/v1/security/install", &json!({ "scan": true, "opt_in": ["trivy"] })).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{refused}");
        assert!(refused["error"].as_str().unwrap().contains("is not an opt-in tool this install offers"), "{refused}");
    })
    .await;
}

/// Two members' installs in one serve never write the shared tools directory
/// at once: each member has its own `tool-install` job, and the downloads run
/// one after the other. The stub `curl` takes a `mkdir` lock for its download
/// and records any download that starts while another holds it.
#[tokio::test]
async fn two_members_installs_in_one_serve_run_one_at_a_time() {
    bounded(async {
        let w = World::new();
        let manifest = w.manifest(&[]);
        let other = w.base.join("other");
        fs::create_dir_all(&other).unwrap();
        fs::write(other.join("main.py"), "def main():\n    return 2\n").unwrap();
        git(&other, &["init", "-q", "-b", "main"]);
        git(&other, &["add", "."]);
        git(&other, &["commit", "-q", "-m", "seed"]);
        w.curl(&format!(
            "rel=\"${{url#https://{HOST}/}}\"\n/bin/mkdir '{rec}/busy' 2>/dev/null || echo \"$url\" >> '{rec}/overlap'\n/bin/sleep 1\n/bin/cp '{server}/'\"$rel\" \"$out\" || exit 22\n/bin/rmdir '{rec}/busy' 2>/dev/null\nexit 0",
            rec = w.rec().display(),
            server = w.server().display()
        ));
        let (first, second) = (w.page(&manifest), w.page_at(&other, &manifest));
        for (page, tool) in [(&first, "trivy"), (&second, "gitleaks")] {
            let (_, asked) = page.post("/api/v1/security/install", &json!({ "tools": [tool] })).await;
            assert_eq!(accepted(&asked["plan"]).len(), 1, "{asked}");
        }
        let body = |tool: &str| json!({ "tools": [tool], "consent": [format!("{tool} {}", manifest.tool(tool).unwrap().version)] });
        let (trivy, gitleaks) = (body("trivy"), body("gitleaks"));
        let ((a, started_a), (b, started_b)) = tokio::join!(
            first.post("/api/v1/security/install", &trivy),
            second.post("/api/v1/security/install", &gitleaks)
        );
        assert_eq!((a, b), (StatusCode::OK, StatusCode::OK), "{started_a} {started_b}");
        let done = first.ended().await;
        assert_eq!(done["state"], "completed", "{done}");
        assert_eq!(second.ended().await["state"], "completed");
        assert_eq!(w.downloads().len(), 2, "{:?}", w.downloads());
        let overlap = fs::read_to_string(w.rec().join("overlap")).unwrap_or_default();
        assert!(overlap.is_empty(), "a download started while another was writing: {overlap}");
        let installed = w.installed();
        assert!(installed.iter().any(|t| t.starts_with("trivy/")) && installed.iter().any(|t| t.starts_with("gitleaks/")), "{installed:?} {done}");
    })
    .await;
}

/// A member's install waiting for another member's to finish is cancelled at
/// once: it ends `cancelled` while the other still downloads, having fetched
/// nothing.
#[tokio::test]
async fn an_install_waiting_for_another_members_is_cancelled_at_once() {
    bounded(async {
        let w = World::new();
        let manifest = w.manifest(&[]);
        let other = w.base.join("other");
        fs::create_dir_all(&other).unwrap();
        fs::write(other.join("main.py"), "def main():\n    return 2\n").unwrap();
        git(&other, &["init", "-q", "-b", "main"]);
        git(&other, &["add", "."]);
        git(&other, &["commit", "-q", "-m", "seed"]);
        // Trivy's download hangs until it is killed.
        w.curl(&format!(
            "case \"$url\" in\n  *trivy*) /bin/sleep 60 & echo $$ $! > '{rec}/curl.pids'; wait ;;\n  *) exit 22 ;;\nesac",
            rec = w.rec().display()
        ));
        let (first, second) = (w.page(&manifest), w.page_at(&other, &manifest));
        let consent = |tool: &str| json!({ "tools": [tool], "consent": [format!("{tool} {}", manifest.tool(tool).unwrap().version)] });
        assert_eq!(first.post("/api/v1/security/install", &consent("trivy")).await.0, StatusCode::OK);
        let deadline = Instant::now() + JOB_WAIT;
        while !fs::read_to_string(w.rec().join("curl.pids")).is_ok_and(|s| s.ends_with('\n')) {
            assert!(Instant::now() < deadline, "trivy's download never started");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(second.post("/api/v1/security/install", &consent("gitleaks")).await.0, StatusCode::OK);

        let (status, _) = second.post("/api/v1/security/install/cancel", &json!({})).await;
        assert_eq!(status, StatusCode::OK);
        let cancelled = Instant::now();
        let deadline = cancelled + Duration::from_secs(5);
        let job = loop {
            let (_, body) = second.get("/api/v1/security/install").await;
            if body["job"]["state"] != "running" {
                break body["job"].clone();
            }
            assert!(Instant::now() < deadline, "the waiting install is still running 5 s after its cancel: {body}");
            tokio::time::sleep(Duration::from_millis(25)).await;
        };
        assert_eq!(job["state"], "cancelled", "{job}");
        let (_, running) = first.get("/api/v1/security/install").await;
        assert_eq!(running["job"]["state"], "running", "the other member's install still downloads");
        assert_eq!(w.downloads().len(), 1, "the cancelled install fetched nothing: {:?}", w.downloads());

        first.post("/api/v1/security/install/cancel", &json!({})).await;
        assert_eq!(first.ended().await["state"], "cancelled");
    })
    .await;
}

/// Install & scan over a machine that already has some of the scanners: an
/// installed scanner the project's network consent does not name is asked
/// for, and is scanned; once every consent is recorded, a later Install & scan
/// of a removed tool asks nothing and starts at once.
#[tokio::test]
async fn install_and_scan_again_asks_only_what_is_missing() {
    bounded(async {
        let w = World::new();
        let manifest = w.manifest(&[]);
        let page = w.page(&manifest);
        let trivy = format!("trivy {}", manifest.tool("trivy").unwrap().version);
        let (_, asked) = page.post("/api/v1/security/install", &json!({ "tools": ["trivy"] })).await;
        assert_eq!(page.post("/api/v1/security/install", &json!({ "tools": ["trivy"], "consent": accepted(&asked["plan"]) })).await.0, StatusCode::OK);
        assert_eq!(page.ended().await["state"], "completed");

        let (status, asked) = page.post("/api/v1/security/install", &json!({ "scan": true })).await;
        assert_eq!(status, StatusCode::CONFLICT, "{asked}");
        let plan = &asked["plan"];
        assert!(plan["scan_consent"].as_array().unwrap().contains(&json!("trivy")), "an installed, unconsented scanner is asked for: {plan}");
        assert_eq!(plan["kept"][0]["tool"], "trivy", "{plan}");
        assert!(!accepted(plan).contains(&trivy), "a kept tool is not installed again: {plan}");
        let defaults: Vec<&str> = manifest.tools.iter().filter(|t| t.is_default()).map(|t| t.tool.as_str()).collect();
        let answer = json!({ "scan": true, "consent": accepted(plan), "scan_consent": defaults });
        assert_eq!(page.post("/api/v1/security/install", &answer).await.0, StatusCode::OK);
        let job = page.ended().await;
        assert_eq!(job["state"], "completed", "{job}");
        let run = job["result"]["scan"]["scanners"].as_array().unwrap().iter().find(|r| r["scanner"] == "trivy").unwrap();
        assert_eq!(run["outcome"], "ran", "{run}");

        // Every consent is recorded now: a removed tool is installed again,
        // and scanned, on one click.
        let gitleaks = manifest.tool("gitleaks").unwrap();
        fs::remove_dir_all(w.tools().join("gitleaks")).unwrap();
        let (status, started) = page.post("/api/v1/security/install", &json!({ "scan": true })).await;
        assert_eq!(status, StatusCode::OK, "nothing is asked again: {started}");
        let job = page.ended().await;
        assert_eq!(job["state"], "completed", "{job}");
        assert_eq!(job["result"]["install"]["tools"][0]["tool"], gitleaks.tool.as_str(), "{job}");
        assert_eq!(job["result"]["install"]["tools"][0]["outcome"], "installed", "{job}");
    })
    .await;
}

/// While a scan runs, an install is refused and starts nothing. A cancel that
/// lands during Install & scan's scan still reports every tool the install
/// left in place.
#[tokio::test]
async fn an_install_waits_out_a_scan_and_a_scan_phase_cancel_reports_what_was_installed() {
    bounded(async {
        let w = World::new();
        let manifest = w.manifest(&[]);
        let page = w.page(&manifest);
        let line = |tool: &str| format!("{tool} {}", manifest.tool(tool).unwrap().version);
        assert_eq!(page.post("/api/v1/security/install", &json!({ "tools": ["trivy"], "consent": [line("trivy")] })).await.0, StatusCode::OK);
        let installed = page.ended().await;
        assert_eq!(installed["state"], "completed", "{installed}");
        // The managed Trivy now hangs when it scans, until it is killed.
        let version = &manifest.tool("trivy").unwrap().version;
        script(
            &w.tools().join("trivy").join(version).join("trivy"),
            &format!(
                "#!/bin/sh\ncase \"$1\" in --version|version) printf '%s\\n' 'trivy version {version}'; exit 0;; esac\n/bin/sleep 60 & echo $$ $! > '{}/trivy.pids'\nwait\n",
                w.rec().display()
            ),
        );
        let sleeping = || async {
            let deadline = Instant::now() + JOB_WAIT;
            while !fs::read_to_string(w.rec().join("trivy.pids")).is_ok_and(|s| s.ends_with('\n')) {
                assert!(Instant::now() < deadline, "trivy never started scanning");
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            fs::remove_file(w.rec().join("trivy.pids")).unwrap();
        };

        let (status, scan) = page.post("/api/v1/security/scan", &json!({ "consent": ["trivy"] })).await;
        assert_eq!(status, StatusCode::OK, "{scan}");
        sleeping().await;
        let downloads = w.downloads().len();
        // A fully answered Install & scan, refused before it records anything.
        let defaults: Vec<&str> = manifest.tools.iter().filter(|t| t.is_default()).map(|t| t.tool.as_str()).collect();
        let missing: Vec<String> = defaults.iter().filter(|t| **t != "trivy").map(|t| line(t)).collect();
        let answered = json!({ "scan": true, "consent": missing, "scan_consent": defaults });
        let (status, refused) = page.post("/api/v1/security/install", &answered).await;
        assert_eq!(status, StatusCode::CONFLICT, "{refused}");
        assert!(refused["error"].as_str().unwrap().contains("a scan is running"), "{refused}");
        assert!(refused.get("plan").is_none(), "{refused}");
        let consented = load_scan_consent(Some(&w.user())).unwrap().consented(&w.root());
        assert_eq!(consented.iter().map(String::as_str).collect::<Vec<_>>(), ["trivy"], "a refused install records no consent");
        let (_, current) = page.get("/api/v1/security/install").await;
        assert_eq!(current["job"]["id"], installed["id"], "no install job started: {current}");
        assert_eq!(w.downloads().len(), downloads, "nothing is fetched beside a scan");
        page.post("/api/v1/security/scan/cancel", &json!({})).await;
        let deadline = Instant::now() + JOB_WAIT;
        while page.get("/api/v1/security/scan").await.1["job"]["state"] == "running" {
            assert!(Instant::now() < deadline, "the scan is still running");
            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        let (_, asked) = page.post("/api/v1/security/install", &json!({ "scan": true })).await;
        let answer = json!({ "scan": true, "consent": accepted(&asked["plan"]), "scan_consent": defaults });
        assert_eq!(page.post("/api/v1/security/install", &answer).await.0, StatusCode::OK);
        sleeping().await;
        page.post("/api/v1/security/install/cancel", &json!({})).await;
        let job = page.ended().await;
        assert_eq!(job["state"], "cancelled", "{job}");
        let landed: Vec<String> = defaults.iter().filter(|t| **t != "trivy").map(|t| line(t)).collect();
        let last = &job["events"].as_array().unwrap().last().unwrap()["event"];
        assert_eq!(last, &json!({ "phase": "cancelled", "installed": landed }), "{job}");
        assert!(steps(&job).contains(&("started".to_string(), "trivy".to_string())), "the cancel landed during the scan");
    })
    .await;
}
