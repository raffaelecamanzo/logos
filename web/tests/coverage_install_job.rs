//! The Coverage page's **Install** and **Add dependency** (S-750, [FR-CV-17],
//! [FR-CV-18], [UAT-CV-07]) through the real router, in process: each starts
//! the member's `coverage-run` job with the install or the addition first,
//! after one consent dialog has answered every line it needs.
//!
//! No test reaches the network, and no real `cargo-llvm-cov`, `rustup`,
//! `curl`, vitest or package manager runs. Every tool is a stub on the
//! test-only search path (vitest: the project's `node_modules/.bin`), and each
//! records its argv under the fixture's record directory, so "nothing ran" is
//! a read of a log:
//! - the managed `cargo-llvm-cov` artifact is built here — a shell stand-in
//!   packed as the builtin manifest's layout and pinned at its SHA-256 — and
//!   served by a stub `curl` from `https://fixtures.invalid/…`; the stand-in
//!   writes the committed Cobertura report only once the stub `rustup` has
//!   added the component;
//! - the stub `pnpm` changes `package.json`, the lockfile and `node_modules`
//!   as the real one would, and the stub vitest writes its report only once
//!   the provider is installed.
//!
//! Stubs that must still be going when the test cancels wait for a release
//! file the test never writes, naming `/bin/sleep` absolutely. Every test runs
//! under [`BOUND`].
//!
//! [FR-CV-17]: ../../docs/specs/requirements/FR-CV-17.md
//! [FR-CV-18]: ../../docs/specs/requirements/FR-CV-18.md
//! [UAT-CV-07]: ../../docs/specs/requirements/UAT-CV-07.md
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
    Router,
};
use http_body_util::BodyExt;
use logos_core::config::{COVERAGE_DEPENDENCY_CONSENT_FILE, INSTALL_CONSENT_FILE, TOOLCHAIN_CONSENT_FILE};
use logos_core::security::ScanEnvironment;
use logos_core::tools::{sha256_file, Layout, Manifest, Platform};
use logos_core::Engine;
use serde_json::{json, Value};
use tempfile::TempDir;
use tower::ServiceExt;
use web::{IntentToken, INTENT_HEADER};

/// The whole of any one test, stubs included.
const BOUND: Duration = Duration::from_secs(120);

/// How long a job, or a stub's start, is waited for before the test fails.
const JOB_WAIT: Duration = Duration::from_secs(30);

const ORIGIN: &str = "http://127.0.0.1:4983";

const INSTALL: &str = "/api/v1/coverage/install";
const DEPENDENCY: &str = "/api/v1/coverage/dependency";
const RUN: &str = "/api/v1/coverage/run";
const CANCEL: &str = "/api/v1/coverage/run/cancel";

/// Where every fixture artifact claims to come from.
const HOST: &str = "fixtures.invalid";

/// The command the dependency consent shows for the pnpm vitest project.
const PNPM_ADD: &str = "pnpm add -D @vitest/coverage-v8@3";

/// What the stub `pnpm` does, as the real one would: install the provider,
/// declare it in `package.json` and record it in the lockfile.
const PNPM_EFFECT: &str = r#"/bin/mkdir -p node_modules/@vitest/coverage-v8
printf '{"name": "@vitest/coverage-v8", "version": "3.2.4"}\n' > node_modules/@vitest/coverage-v8/package.json
printf '{"name": "calc", "private": true, "devDependencies": {"@vitest/coverage-v8": "^3.2.4", "vitest": "^3.2.4"}}\n' > package.json
printf '  @vitest/coverage-v8: 3.2.4\n' >> pnpm-lock.yaml"#;

struct Fixture {
    _tmp: TempDir,
    repo: PathBuf,
    bin: PathBuf,
    user: PathBuf,
    rec: PathBuf,
    tools: PathBuf,
    server: PathBuf,
    intent: IntentToken,
    router: Router,
    /// The engine the router serves, for what a test does beside the page —
    /// holding the project's lock.
    engine: Option<Arc<Engine>>,
}

impl Fixture {
    /// A one-commit copy of the committed coverage fixture project `project`,
    /// with `extra` written over it, whose engine finds tools only in `bin`,
    /// keeps its consent records in `user` and installs into `tools`. `bin`
    /// holds `tar` and `gzip` (the install unpacks with them) and a `curl`
    /// that serves the `server` directory. `prepare` runs on the fixture
    /// before the commit and the index, and returns the manifest the engine
    /// uses (the builtin one when `None`).
    fn new(project: &str, extra: &[(&str, &str)], prepare: impl FnOnce(&Fixture) -> Option<Manifest>) -> Self {
        let tmp = TempDir::new().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let [repo, bin, user, rec, tools, server] =
            ["project", "bin", "user", "rec", "tools", "server"].map(|d| base.join(d));
        for dir in [&repo, &bin, &user, &rec, &server] {
            fs::create_dir_all(dir).unwrap();
        }
        for program in ["tar", "gzip"] {
            if let Some(found) = on_process_path(program) {
                std::os::unix::fs::symlink(found, bin.join(program)).unwrap();
            }
        }
        copy_tree(&fixture_project(project), &repo);
        for (path, body) in extra {
            write(&repo.join(path), body);
        }
        write(&repo.join(".gitignore"), "node_modules/\n.logos/\ntarget/\n");
        let mut f = Self {
            _tmp: tmp,
            repo,
            bin,
            user,
            rec,
            tools,
            server,
            intent: IntentToken::generate(),
            router: Router::new(),
            engine: None,
        };
        f.curl(None);
        let manifest = prepare(&f);
        git(&f.repo, &["init", "-q", "-b", "main"]);
        git(&f.repo, &["add", "."]);
        git(&f.repo, &["commit", "-q", "-m", "seed"]);
        let env = ScanEnvironment {
            user_dir: Some(f.user.clone()),
            search_path: Some(f.bin.clone().into_os_string()),
            tools_dir: Some(f.tools.clone()),
            nvd_api_key: None,
        };
        let mut engine = Engine::start(&f.repo).expect("engine starts").with_scan_environment(env);
        if let Some(manifest) = manifest {
            engine = engine.with_tools_manifest(manifest);
        }
        engine.index();
        let engine = Arc::new(engine);
        f.router = web::router_with_intent(Arc::clone(&engine), f.intent.clone());
        f.engine = Some(engine);
        f
    }

    /// The Rust project with no coverage tooling: a stub `rustup` without the
    /// component, and the builtin manifest's `cargo-llvm-cov` artifact built
    /// here and served — its download held when `held_download`.
    fn rust(held_download: bool) -> Self {
        Self::new("cargo-llvm-cov", &[], |f| {
            f.rustup();
            if held_download {
                f.curl(Some(&f.held()));
            }
            Some(f.manifest())
        })
    }

    /// The Rust project whose `rustup` cannot add the component.
    fn rust_component_fails() -> Self {
        Self::new("cargo-llvm-cov", &[], |f| {
            let body = "case \"$*\" in\n'component list --installed') exit 0 ;;\n\
                        'component add llvm-tools-preview') echo 'error: toolchain is not installed' >&2; exit 1 ;;\nesac\nexit 1";
            f.stub(&f.bin.join("rustup"), "rustup", body);
            Some(f.manifest())
        })
    }

    /// The Rust project, its `cargo-llvm-cov` artifact pinned at a SHA-256
    /// that is not its own.
    fn rust_mispinned() -> Self {
        Self::new("cargo-llvm-cov", &[], |f| {
            f.rustup();
            let mut manifest = f.manifest();
            manifest.coverage[0].artifacts[0].sha256 = "0".repeat(64);
            Some(manifest)
        })
    }

    /// The pnpm vitest project, its vitest 3.2.4 installed without a coverage
    /// provider: `pnpm` runs `effect`, then `then` (empty, or [`held`]).
    fn vitest(lockfile: bool, then: impl FnOnce(&Fixture) -> String) -> Self {
        let package = r#"{"name": "calc", "private": true, "type": "module", "devDependencies": {"vitest": "^3.2.4"}}
"#;
        let mut extra = vec![("package.json", package), ("node_modules/vitest/package.json", r#"{"version": "3.2.4"}"#)];
        let lock = fs::read_to_string(fixture_project("deps/pnpm-vitest").join("pnpm-lock.yaml")).unwrap();
        if lockfile {
            extra.push(("pnpm-lock.yaml", &lock));
        }
        Self::new("vitest", &extra, |f| {
            f.vitest_stub();
            let tail = then(f);
            f.stub(&f.bin.join("pnpm"), "pnpm", &format!("{PNPM_EFFECT}\n{tail}"));
            None
        })
    }

    /// A stub at `path` that records `cwd` and argv to `rec/<name>.runs`,
    /// then runs `body`.
    fn stub(&self, path: &Path, name: &str, body: &str) {
        let script = format!(
            "#!/bin/sh\nprintf '%s|%s\\n' \"$(pwd -P)\" \"$*\" >> '{rec}/{name}.runs'\n{body}\n",
            rec = self.rec.display()
        );
        write(path, &script);
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// A stub body that waits for a release file nobody writes: the stub
    /// goes on until a cancel kills it.
    fn held(&self) -> String {
        format!("while [ ! -f '{}/release' ]; do /bin/sleep 0.05; done", self.rec.display())
    }

    /// The project's vitest: writes the fixture report into the reports
    /// directory its argv names, but only with the provider installed.
    fn vitest_stub(&self) {
        let body = format!(
            "[ -f node_modules/@vitest/coverage-v8/package.json ] || {{ echo 'no coverage provider' >&2; exit 1; }}\n\
             for a in \"$@\"; do case $a in --coverage.reportsDirectory=*) d=\"${{a#--coverage.reportsDirectory=}}\";; esac; done\n\
             /bin/mkdir -p \"$d\" && /bin/cp '{report}' \"$d/cobertura-coverage.xml\"",
            report = fixture_project("vitest").join("fixture-report.xml").display()
        );
        self.stub(&self.repo.join("node_modules/.bin/vitest"), "vitest", &body);
    }

    /// A `curl` serving `https://fixtures.invalid/<rel>` from the server
    /// directory, recording each URL — after `hold`, when given.
    fn curl(&self, hold: Option<&str>) {
        let body = format!(
            "out=; url=\nwhile [ $# -gt 0 ]; do\n  case \"$1\" in\n    --output) out=\"$2\"; shift 2 ;;\n    *) url=\"$1\"; shift ;;\n  esac\ndone\n\
             {hold}\nrel=\"${{url#https://{HOST}/}}\"\n/bin/cp '{server}/'\"$rel\" \"$out\" || exit 22",
            hold = hold.unwrap_or(""),
            server = self.server.display()
        );
        self.stub(&self.bin.join("curl"), "curl", &body);
    }

    /// The stub `rustup`'s toolchain holds the component once this file is.
    fn component(&self) -> PathBuf {
        self.rec.join("llvm-tools-installed")
    }

    /// A stub `rustup`: lists the component once it is there, and
    /// `component add llvm-tools-preview` adds it.
    fn rustup(&self) {
        let body = format!(
            "case \"$*\" in\n\
             'component list --installed') printf 'cargo-x86_64-unknown-linux-gnu\\n'; \
             [ -f '{c}' ] && printf 'llvm-tools-x86_64-unknown-linux-gnu\\n'; exit 0 ;;\n\
             'component add llvm-tools-preview') : > '{c}'; exit 0 ;;\nesac\nexit 1",
            c = self.component().display()
        );
        self.stub(&self.bin.join("rustup"), "rustup", &body);
    }

    /// The builtin manifest, its `cargo-llvm-cov` artifact for this platform
    /// built here — a stand-in packed as the manifest's `.tar.gz` — and
    /// pinned at its SHA-256. The stand-in fails while the component is
    /// missing, and otherwise copies the fixture report to `--output-path`.
    fn manifest(&self) -> Manifest {
        let platform = Platform::current().expect("the suite runs where managed installs exist");
        let mut manifest = Manifest::builtin().clone();
        let entry = &mut manifest.coverage[0];
        assert_eq!(entry.tool, "cargo-llvm-cov");
        let mut artifact = entry.artifact(platform).unwrap().clone();
        let Layout::TarGz { member } = &artifact.layout else { panic!("cargo-llvm-cov is a .tar.gz release") };
        let src = self.rec.join("src");
        let body = format!(
            "[ \"$*\" = 'llvm-cov --version' ] && {{ echo 'cargo-llvm-cov {version}'; exit 0; }}\n\
             [ -f '{component}' ] || {{ echo 'error: failed to find llvm-tools-preview' >&2; exit 1; }}\n\
             out=; prev=\nfor a in \"$@\"; do [ \"$prev\" = --output-path ] && out=\"$a\"; prev=\"$a\"; done\n\
             /bin/cp '{report}' \"$out\"",
            version = entry.version,
            component = self.component().display(),
            report = fixture_project("cargo-llvm-cov").join("fixture-report.xml").display(),
        );
        self.stub(&src.join(member), "cargo-llvm-cov", &body);
        let file = self.server.join("cargo-llvm-cov.tar.gz");
        let packed = Command::new("tar").arg("-czf").arg(&file).arg("-C").arg(&src).arg(member).status().unwrap();
        assert!(packed.success());
        artifact.url = format!("https://{HOST}/cargo-llvm-cov.tar.gz");
        artifact.sha256 = sha256_file(&file).unwrap();
        artifact.size = fs::metadata(&file).unwrap().len();
        entry.artifacts = vec![artifact];
        manifest
    }

    /// Each recorded run of stub `name`: `cwd|argv`.
    fn runs(&self, name: &str) -> Vec<String> {
        fs::read_to_string(self.rec.join(format!("{name}.runs")))
            .map(|s| s.lines().map(str::to_string).collect())
            .unwrap_or_default()
    }

    /// Wait until stub `name` has started.
    async fn started(&self, name: &str) {
        let deadline = Instant::now() + logos_core::perf::scaled(JOB_WAIT);
        while self.runs(name).is_empty() {
            assert!(Instant::now() < deadline, "{name} never started");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    fn file(&self, path: &str) -> Vec<u8> {
        fs::read(self.repo.join(path)).unwrap_or_default()
    }

    /// The project's `git status --porcelain` lines.
    fn status(&self) -> Vec<String> {
        let out = Command::new("git").arg("-C").arg(&self.repo).args(["status", "--porcelain"]).output().unwrap();
        String::from_utf8_lossy(&out.stdout).lines().map(str::to_string).collect()
    }

    fn head(&self) -> String {
        let out = Command::new("git").arg("-C").arg(&self.repo).args(["rev-parse", "HEAD"]).output().unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// Whether the user-level consent record `file` exists.
    fn recorded(&self, file: &str) -> bool {
        self.user.join(file).exists()
    }

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
        self.send(post(path, Some(self.intent.as_str()), &body.to_string())).await
    }

    async fn get(&self, path: &str) -> (StatusCode, Value) {
        self.send(get(path)).await
    }

    /// The Coverage page's row for `tool`.
    async fn row(&self, tool: &str) -> Value {
        let (status, body) = self.get("/api/v1/coverage").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let tools = body["coverage"]["runner"]["tools"].as_array().expect("the runner's rows").clone();
        tools.into_iter().find(|t| t["tool"] == tool).unwrap_or_else(|| panic!("no {tool} row"))
    }

    /// The member's coverage job, once it is no longer running.
    async fn ended(&self) -> Value {
        let deadline = Instant::now() + logos_core::perf::scaled(JOB_WAIT);
        loop {
            let (status, body) = self.get(RUN).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            if body["job"]["state"] != "running" {
                return body["job"].clone();
            }
            assert!(Instant::now() < deadline, "the job is still running: {body}");
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
}

/// The job's events of `phase`, in order.
fn events(job: &Value, phase: &str) -> Vec<Value> {
    job["events"].as_array().unwrap().iter().map(|e| e["event"].clone()).filter(|e| e["phase"] == phase).collect()
}

/// The phases of the job's events, in order.
fn phases(job: &Value) -> Vec<String> {
    job["events"].as_array().unwrap().iter().map(|e| e["event"]["phase"].as_str().unwrap().to_string()).collect()
}

/// What the dialog accepts: every line of the `409`'s question, as shown.
fn accept_all(question: &Value) -> Value {
    let install: Vec<String> = question["install"]
        .as_array()
        .unwrap()
        .iter()
        .map(|line| match line["line"].as_str().unwrap() {
            "tool" => format!("{} {}", line["tool"].as_str().unwrap(), line["version"].as_str().unwrap()),
            _ => line["command"].as_str().unwrap().to_string(),
        })
        .collect();
    let dependency: Vec<&Value> = question["dependencies"].as_array().unwrap().iter().map(|d| &d["command"]).collect();
    json!({ "consent": question["consent_required"]["tools"], "install": install, "dependency": dependency })
}

fn fixture_project(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../logos-core/tests/fixtures/coverage/projects").join(name)
}

fn copy_tree(from: &Path, to: &Path) {
    for entry in fs::read_dir(from).unwrap().flatten() {
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            fs::create_dir_all(&target).unwrap();
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).unwrap();
        }
    }
}

fn write(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn on_process_path(program: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join(program)).find(|p| p.is_file())
}

fn git(cwd: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .env("GIT_CEILING_DIRECTORIES", cwd.parent().unwrap())
        .args(["-c", "maintenance.auto=false", "-c", "gc.auto=0", "-c", "user.email=t@logos", "-c", "user.name=t"])
        .args(args)
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

fn get(path: &str) -> Request<Body> {
    Request::builder().method(Method::GET).uri(path).header(header::HOST, "127.0.0.1:4983").body(Body::empty()).unwrap()
}

fn post(path: &str, intent: Option<&str>, body: &str) -> Request<Body> {
    let mut builder = Request::builder()
        .method(Method::POST)
        .uri(path)
        .header(header::HOST, "127.0.0.1:4983")
        .header(header::ORIGIN, ORIGIN)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(token) = intent {
        builder = builder.header(INTENT_HEADER, token);
    }
    builder.body(Body::from(body.to_string())).unwrap()
}

async fn bounded(test: impl std::future::Future<Output = ()>) {
    tokio::time::timeout(BOUND, test).await.expect("the test ran past its bound");
}

/// AC1, [UAT-CV-07] step 1 from the page: a Rust project with neither
/// `cargo-llvm-cov` nor `llvm-tools-preview` offers **Install**; the first
/// start asks — the download, the `rustup` command and the run's execution
/// consent in one question — with nothing downloaded, run or recorded; a
/// partial answer is asked again; the whole answer installs both, then runs
/// coverage and ingests it, with no terminal used.
#[tokio::test]
async fn a_rust_project_with_no_tooling_runs_coverage_from_the_page_after_one_consent() {
    bounded(async {
        let f = Fixture::rust(false);
        let row = f.row("cargo-llvm-cov").await;
        assert_eq!((row["state"].as_str(), row["offer"].as_str()), (Some("not_installed"), Some("install")), "{row}");

        let (status, question) = f.post(INSTALL, &json!({})).await;
        assert_eq!(status, StatusCode::CONFLICT, "{question}");
        let lines = question["install"].as_array().unwrap();
        assert_eq!(lines.len(), 2, "the download and the rustup step: {question}");
        assert_eq!((lines[0]["line"].as_str(), lines[0]["tool"].as_str()), (Some("tool"), Some("cargo-llvm-cov")));
        assert!(lines[0]["text"].as_str().unwrap().contains(" from fixtures.invalid ("), "{question}");
        assert_eq!(lines[1]["command"], "rustup component add llvm-tools-preview", "{question}");
        assert_eq!(question["consent_required"]["tools"], json!(["cargo-llvm-cov"]), "the run's consent is asked too");
        assert_eq!(question["dependencies"], json!([]));
        let nothing_yet = |f: &Fixture| {
            assert!(f.runs("curl").is_empty() && f.runs("rustup").is_empty() && f.runs("cargo-llvm-cov").is_empty());
            assert!(!f.tools.exists(), "nothing installed");
            for file in [INSTALL_CONSENT_FILE, TOOLCHAIN_CONSENT_FILE, "coverage-consent.toml"] {
                assert!(!f.recorded(file), "{file} recorded before the whole answer");
            }
        };
        nothing_yet(&f);

        let mut partial = accept_all(&question);
        partial["install"].as_array_mut().unwrap().pop();
        let (status, again) = f.post(INSTALL, &partial).await;
        assert_eq!(status, StatusCode::CONFLICT, "an unaccepted rustup line is asked again: {again}");
        nothing_yet(&f);

        let (status, started) = f.post(INSTALL, &accept_all(&question)).await;
        assert_eq!(status, StatusCode::OK, "{started}");
        assert_eq!(started["job"]["kind"], "coverage-run");
        let job = f.ended().await;
        assert_eq!(job["state"], "completed", "{job}");
        let order = phases(&job);
        let at = |phase: &str| order.iter().position(|p| p == phase).unwrap_or_else(|| panic!("no {phase}: {order:?}"));
        assert!(at("install_finished") < at("component_finished") && at("component_finished") < at("started"), "{order:?}");
        assert_eq!(events(&job, "install_finished")[0]["outcome"], "installed", "{job}");
        assert_eq!(events(&job, "component_finished")[0]["outcome"], "added", "{job}");
        let ran = events(&job, "finished").into_iter().find(|e| e["tool"] == "cargo-llvm-cov").unwrap();
        assert_eq!(ran["outcome"], "ran", "{job}");
        assert_eq!(ran["ingests"][0]["format"], "cobertura", "{ran}");
        assert!(ran["ingests"][0]["matched_files"].as_u64().unwrap() >= 1, "{ran}");
        assert_eq!(f.runs("curl").len(), 1, "one download");
        let rustup: Vec<String> = f.runs("rustup").into_iter().map(|r| r.split_once('|').unwrap().1.to_string()).collect();
        assert_eq!(rustup, ["component list --installed", "component add llvm-tools-preview"]);
        for file in [INSTALL_CONSENT_FILE, TOOLCHAIN_CONSENT_FILE] {
            assert!(f.recorded(file), "{file} not recorded");
            assert!(!f.repo.join(file).exists() && !f.repo.join(".logos").join(file).exists(), "{file} in the repository");
        }

        let row = f.row("cargo-llvm-cov").await;
        assert_eq!((row["state"].as_str(), row["origin"].as_str()), (Some("ran"), Some("managed")), "{row}");
        assert!(row.get("offer").is_none(), "an installed tool offers nothing: {row}");
    })
    .await;
}

/// AC2, [UAT-CV-07] step 2–3 from the page: a pnpm vitest project offers
/// **Add dependency**; its question names the exact command and the files it
/// changes before anything changes; once accepted, pnpm adds the provider,
/// vitest runs and ingests, and the change stays in the working tree.
#[tokio::test]
async fn a_vitest_consent_shows_the_exact_command_and_files_before_anything_changes() {
    bounded(async {
        let f = Fixture::vitest(true, |_| String::new());
        let before = (f.file("package.json"), f.file("pnpm-lock.yaml"), f.head());
        let row = f.row("vitest").await;
        assert_eq!((row["state"].as_str(), row["offer"].as_str()), (Some("not_installed"), Some("add_dependency")), "{row}");

        let (status, question) = f.post(DEPENDENCY, &json!({ "tool": "vitest" })).await;
        assert_eq!(status, StatusCode::CONFLICT, "{question}");
        assert_eq!(
            question["dependencies"],
            json!([{
                "tool": "vitest",
                "package": "@vitest/coverage-v8@3",
                "manager": "pnpm",
                "command": PNPM_ADD,
                "files": ["package.json", "pnpm-lock.yaml"],
            }]),
        );
        assert_eq!(question["consent_required"]["tools"], json!(["vitest"]), "the tool the addition enables");
        assert_eq!(question["install"], json!([]));
        assert!(f.runs("pnpm").is_empty() && f.runs("vitest").is_empty(), "nothing ran before the answer");
        assert_eq!((f.file("package.json"), f.file("pnpm-lock.yaml"), f.head()), before, "nothing changed");
        assert!(f.status().is_empty(), "{:?}", f.status());
        assert!(!f.recorded(COVERAGE_DEPENDENCY_CONSENT_FILE));

        // The run's consent alone does not accept the command: asked again, nothing changed.
        let (status, again) = f.post(DEPENDENCY, &json!({ "tool": "vitest", "consent": ["vitest"] })).await;
        assert_eq!(status, StatusCode::CONFLICT, "{again}");
        assert!(f.runs("pnpm").is_empty() && f.status().is_empty(), "nothing ran or changed");
        assert!(!f.recorded(COVERAGE_DEPENDENCY_CONSENT_FILE) && !f.recorded("coverage-consent.toml"), "nothing recorded");

        let (status, started) = f.post(DEPENDENCY, &json!({ "tool": "vitest", "consent": ["vitest"], "dependency": [PNPM_ADD] })).await;
        assert_eq!(status, StatusCode::OK, "{started}");
        let job = f.ended().await;
        assert_eq!(job["state"], "completed", "{job}");
        let order = phases(&job);
        assert_eq!(&order[..2], ["dependency_started", "dependency_finished"], "the addition comes first: {order:?}");
        assert_eq!(events(&job, "dependency_started")[0]["command"], PNPM_ADD);
        assert_eq!(events(&job, "dependency_finished")[0]["outcome"], "added", "{job}");
        let vitest = events(&job, "finished").into_iter().find(|e| e["tool"] == "vitest").unwrap();
        assert_eq!(vitest["outcome"], "ran", "{vitest}");
        let pnpm = f.runs("pnpm");
        assert_eq!(pnpm, [format!("{}|add -D @vitest/coverage-v8@3", f.repo.display())], "run once, in the project");
        assert_eq!(f.status(), [" M package.json", " M pnpm-lock.yaml"], "left uncommitted");
        assert_eq!(f.head(), before.2, "nothing committed");
        assert!(f.recorded(COVERAGE_DEPENDENCY_CONSENT_FILE));
    })
    .await;
}

/// AC3, the addition: a cancel while pnpm runs kills it, puts `package.json`
/// and the lockfile back byte for byte, removes the provider it had installed,
/// and no tool runs after it.
#[tokio::test]
async fn cancel_during_the_addition_leaves_no_half_applied_dependency_change() {
    bounded(async {
        let f = Fixture::vitest(true, Fixture::held);
        let before = (f.file("package.json"), f.file("pnpm-lock.yaml"));
        let (_, question) = f.post(DEPENDENCY, &json!({ "tool": "vitest" })).await;
        let mut answer = accept_all(&question);
        answer["tool"] = json!("vitest");
        let (status, started) = f.post(DEPENDENCY, &answer).await;
        assert_eq!(status, StatusCode::OK, "{started}");
        f.started("pnpm").await;
        let deadline = Instant::now() + logos_core::perf::scaled(JOB_WAIT);
        while !f.repo.join("node_modules/@vitest/coverage-v8/package.json").exists() || f.file("package.json") == before.0 {
            assert!(Instant::now() < deadline, "pnpm never changed the project");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        let (status, _) = f.post(CANCEL, &json!({})).await;
        assert_eq!(status, StatusCode::OK);
        let job = f.ended().await;
        assert_eq!(job["state"], "cancelled", "{job}");
        let finished = &events(&job, "dependency_finished")[0];
        assert_eq!(finished["outcome"], "cancelled", "{finished}");
        assert_eq!(finished["restored"], json!(["package.json", "pnpm-lock.yaml", "node_modules/@vitest/coverage-v8"]));
        assert_eq!((f.file("package.json"), f.file("pnpm-lock.yaml")), before, "the files hold what they held");
        assert!(!f.repo.join("node_modules/@vitest/coverage-v8").exists(), "the half-installed provider is gone");
        assert!(f.status().is_empty(), "{:?}", f.status());
        assert!(f.runs("vitest").is_empty(), "no tool ran after the cancel");
        assert_eq!(f.row("vitest").await["state"], "not_installed", "the page reads it as before");
    })
    .await;
}

/// AC3, the install: a cancel while `cargo-llvm-cov` downloads kills it,
/// leaves no partial tool, runs no `rustup` and no coverage tool.
#[tokio::test]
async fn cancel_during_the_install_leaves_no_partial_install() {
    bounded(async {
        let f = Fixture::rust(true);
        let (_, question) = f.post(INSTALL, &json!({})).await;
        let (status, started) = f.post(INSTALL, &accept_all(&question)).await;
        assert_eq!(status, StatusCode::OK, "{started}");
        f.started("curl").await;

        let (status, _) = f.post(CANCEL, &json!({})).await;
        assert_eq!(status, StatusCode::OK);
        let job = f.ended().await;
        assert_eq!(job["state"], "cancelled", "{job}");
        assert_eq!(events(&job, "install_finished")[0]["outcome"], "cancelled", "{job}");
        let version = &question["install"][0]["version"];
        let managed = f.tools.join("cargo-llvm-cov").join(version.as_str().unwrap());
        assert!(!managed.exists(), "a partial tool at {}", managed.display());
        let left: Vec<PathBuf> = walk(&f.tools).into_iter().filter(|p| p.is_file()).collect();
        assert!(left.is_empty(), "nothing of the download stays: {left:?}");
        assert!(f.runs("rustup").is_empty(), "no rustup after the cancel: {:?}", f.runs("rustup"));
        assert!(events(&job, "started").is_empty(), "no coverage tool ran: {job}");
        assert_eq!(f.row("cargo-llvm-cov").await["state"], "not_installed");
    })
    .await;
}

/// [FR-CV-18]'s refusal from the page: a vitest project with no lockfile is
/// refused with the command to run by hand, before any question, and nothing
/// in it changes.
#[tokio::test]
async fn a_project_with_no_lockfile_is_refused_with_the_command_to_run_by_hand() {
    bounded(async {
        let f = Fixture::vitest(false, |_| String::new());
        let (status, body) = f.post(DEPENDENCY, &json!({ "tool": "vitest" })).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(body["hand_command"], "npm install --save-dev @vitest/coverage-v8@3", "{body}");
        assert!(body["error"].as_str().unwrap().contains("no lockfile"), "{body}");
        assert!(f.runs("pnpm").is_empty() && f.status().is_empty());
        for file in [COVERAGE_DEPENDENCY_CONSENT_FILE, "coverage-consent.toml"] {
            assert!(!f.recorded(file), "{file}");
        }
        let (_, job) = f.get(RUN).await;
        assert_eq!(job["job"], Value::Null, "no job started");
    })
    .await;
}

/// The routes' own refusals: each is a start that names its shape, needs the
/// page's intent token, and never joins a run already going.
#[tokio::test]
async fn the_routes_refuse_a_wrong_shape_a_missing_intent_and_a_run_already_going() {
    bounded(async {
        let f = Fixture::vitest(true, Fixture::held);
        let (status, body) = f.post(INSTALL, &json!({ "tool": "vitest" })).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        let (status, body) = f.post(DEPENDENCY, &json!({})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        let (status, _) = f.send(post(DEPENDENCY, None, r#"{"tool": "vitest"}"#)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "no intent token, no start");

        let answer = json!({ "tool": "vitest", "consent": ["vitest"], "dependency": [PNPM_ADD] });
        let (status, started) = f.post(DEPENDENCY, &answer).await;
        assert_eq!(status, StatusCode::OK, "{started}");
        f.started("pnpm").await;
        let (status, body) = f.post(DEPENDENCY, &answer).await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert!(body["error"].as_str().unwrap().contains("a coverage run is going"), "{body}");
        assert_eq!(f.runs("pnpm").len(), 1, "the second start started nothing");
        f.post(CANCEL, &json!({})).await;
        f.ended().await;
    })
    .await;
}

/// Every path under `dir`, recursively; empty when it does not exist.
fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path.clone());
            }
            found.push(path);
        }
    }
    found
}

/// An install that does not complete ends the job failed, naming what went
/// wrong, and no coverage tool runs: a download whose SHA-256 is not the
/// pinned one is never run, and the run that would need it never starts.
#[tokio::test]
async fn an_install_that_fails_ends_the_job_failed_and_no_tool_runs() {
    bounded(async {
        let f = Fixture::rust_mispinned();
        let (_, question) = f.post(INSTALL, &json!({})).await;
        let (status, started) = f.post(INSTALL, &accept_all(&question)).await;
        assert_eq!(status, StatusCode::OK, "{started}");
        let job = f.ended().await;
        assert_eq!(job["state"], "failed", "{job}");
        let error = job["error"].as_str().unwrap();
        assert!(error.starts_with("the install did not complete, so no coverage tool ran: cargo-llvm-cov "), "{error}");
        assert!(error.contains("checksum"), "the stage is named: {error}");
        let finished = &events(&job, "install_finished")[0];
        assert_eq!((finished["outcome"].as_str(), finished["stage"].as_str()), (Some("failed"), Some("checksum")), "{finished}");
        assert!(events(&job, "started").is_empty() && f.runs("cargo-llvm-cov").is_empty(), "no tool ran: {job}");
    })
    .await;
}

/// The question is decided on recorded state only, as the page reads it: a
/// `cargo-llvm-cov` on `PATH` is the copy in use whatever its version, so
/// Install asks to download nothing — only the `rustup` step — and its version
/// is never probed before the answer.
#[tokio::test]
async fn a_cargo_llvm_cov_on_path_is_never_downloaded_and_never_probed_before_the_answer() {
    bounded(async {
        let f = Fixture::new("cargo-llvm-cov", &[], |f| {
            f.rustup();
            f.stub(&f.bin.join("cargo-llvm-cov"), "path-cargo-llvm-cov", "echo 'cargo-llvm-cov 0.9.1'");
            Some(f.manifest())
        });
        let row = f.row("cargo-llvm-cov").await;
        assert_eq!((row["state"].as_str(), row["origin"].as_str()), (Some("never_run"), Some("path")), "{row}");
        assert!(row.get("offer").is_none(), "{row}");
        let (status, question) = f.post(INSTALL, &json!({})).await;
        assert_eq!(status, StatusCode::CONFLICT, "{question}");
        let lines: Vec<&str> = question["install"].as_array().unwrap().iter().map(|l| l["line"].as_str().unwrap()).collect();
        assert_eq!(lines, ["toolchain"], "nothing to download: {question}");
        assert!(f.runs("path-cargo-llvm-cov").is_empty(), "the PATH copy was probed before the answer");
        assert!(f.runs("rustup").is_empty() && f.runs("curl").is_empty());
    })
    .await;
}

/// [FR-CV-18] for pytest-cov from the page: the project `.venv`'s pytest
/// without its plugin offers **Add dependency**, and its question names the
/// uv command and the files it changes before anything runs or changes.
#[tokio::test]
async fn a_uv_project_s_pytest_without_pytest_cov_is_offered_the_uv_command() {
    bounded(async {
        let lock = fs::read_to_string(fixture_project("deps/uv").join("uv.lock")).unwrap();
        let pyproject = fs::read_to_string(fixture_project("deps/uv").join("pyproject.toml")).unwrap();
        let f = Fixture::new("pytest-cov", &[("uv.lock", &lock), ("pyproject.toml", &pyproject)], |f| {
            f.stub(&f.repo.join(".venv/bin/pytest"), "pytest", "exit 0");
            f.stub(&f.bin.join("uv"), "uv", "exit 0");
            None
        });
        let row = f.row("pytest-cov").await;
        assert_eq!((row["state"].as_str(), row["offer"].as_str()), (Some("not_installed"), Some("add_dependency")), "{row}");

        let (status, question) = f.post(DEPENDENCY, &json!({ "tool": "pytest-cov" })).await;
        assert_eq!(status, StatusCode::CONFLICT, "{question}");
        let ask = &question["dependencies"][0];
        assert_eq!(ask["command"], "uv add --dev pytest-cov", "{question}");
        assert_eq!(ask["files"], json!(["pyproject.toml", "uv.lock"]), "{question}");
        assert_eq!(question["consent_required"]["tools"], json!(["pytest-cov"]));
        assert!(f.runs("uv").is_empty() && f.runs("pytest").is_empty(), "nothing ran before the answer");
        assert!(f.status().is_empty(), "{:?}", f.status());
    })
    .await;
}

/// AC4: the public manual's coverage page describes managed install and the
/// dependency consent — what each offer does, what the one dialog shows, that
/// nothing runs before it, where the answer is kept, and what a cancel leaves.
#[test]
fn the_manual_describes_managed_install_and_the_dependency_consent() {
    let manual = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs/howto/usage.md")).unwrap();
    let start = manual.find("### Setting up a coverage tool from the Coverage page").expect("the section");
    let section = &manual[start..];
    let section = &section[..section[4..].find("\n### ").map_or(section.len(), |end| end + 4)];
    for needle in [
        "**Install** appears on `cargo-llvm-cov`",
        "checks its SHA-256 before anything from it",
        "`rustup component add llvm-tools-preview`",
        "**Add dependency** appears on a vitest that has no coverage provider",
        "pytest-cov is missing",
        "the **exact command**, such as\n  `pnpm add -D @vitest/coverage-v8@3`",
        "the **files it changes**",
        "may run dependency lifecycle scripts",
        "Nothing is downloaded, run or changed until you accept it",
        "never in the repository",
        "Logos never commits",
        "A cancelled install leaves no partial tool behind",
        "puts back the manifest and the lockfile",
        "the command to\nrun by hand",
    ] {
        assert!(section.contains(needle), "the manual's coverage section lacks {needle:?}");
    }
}

/// S-750, review: the run's execution consent is one of the lines the dialog
/// answers. A start that accepts the download and the `rustup` step — or the
/// dependency command — but not the run itself is asked again, and nothing at
/// all is recorded, downloaded or run.
#[tokio::test]
async fn an_answer_without_the_run_consent_records_nothing() {
    bounded(async {
        let f = Fixture::rust(false);
        let (_, question) = f.post(INSTALL, &json!({})).await;
        let mut answer = accept_all(&question);
        answer["consent"] = json!([]);
        let (status, again) = f.post(INSTALL, &answer).await;
        assert_eq!(status, StatusCode::CONFLICT, "{again}");
        assert_eq!(again["consent_required"]["tools"], json!(["cargo-llvm-cov"]));
        for file in [INSTALL_CONSENT_FILE, TOOLCHAIN_CONSENT_FILE, "coverage-consent.toml"] {
            assert!(!f.recorded(file), "{file} recorded without the run's consent");
        }
        assert!(f.runs("curl").is_empty() && f.runs("rustup").is_empty());

        let f = Fixture::vitest(true, |_| String::new());
        let (status, again) = f.post(DEPENDENCY, &json!({ "tool": "vitest", "dependency": [PNPM_ADD] })).await;
        assert_eq!(status, StatusCode::CONFLICT, "{again}");
        assert!(!f.recorded(COVERAGE_DEPENDENCY_CONSENT_FILE) && !f.recorded("coverage-consent.toml"), "recorded");
        assert!(f.runs("pnpm").is_empty() && f.status().is_empty());
    })
    .await;
}

/// S-750, review: what the user consented to before — in a terminal or on the
/// page — is not asked again. With the dependency consent and the run consent
/// recorded, Add dependency starts at once; with the install and toolchain
/// consents recorded, Install asks only for the run.
#[tokio::test]
async fn a_recorded_consent_is_not_asked_again() {
    bounded(async {
        let f = Fixture::vitest(true, |_| String::new());
        logos_core::config::record_coverage_dependency_consent(Some(&f.user), &f.repo, &[PNPM_ADD]).unwrap();
        logos_core::coverage::consent::require(Some(&f.user), &f.repo, &["vitest"], None, true, "logos coverage run").unwrap();
        let (status, started) = f.post(DEPENDENCY, &json!({ "tool": "vitest" })).await;
        assert_eq!(status, StatusCode::OK, "nothing left to ask: {started}");
        assert_eq!(f.ended().await["state"], "completed");
        assert_eq!(f.runs("pnpm").len(), 1);

        let f = Fixture::rust(false);
        let version = Manifest::builtin().coverage[0].version.clone();
        logos_core::config::record_install_consent(Some(&f.user), &[("cargo-llvm-cov", version.as_str())], &[]).unwrap();
        logos_core::config::record_toolchain_consent(Some(&f.user), &[logos_core::tools::LLVM_TOOLS]).unwrap();
        let (status, question) = f.post(INSTALL, &json!({})).await;
        assert_eq!(status, StatusCode::CONFLICT, "{question}");
        assert_eq!(question["install"], json!([]), "the download and the rustup step are recorded");
        assert_eq!(question["consent_required"]["tools"], json!(["cargo-llvm-cov"]), "only the run is asked");
    })
    .await;
}

/// S-750, review: Add dependency for a tool that is missing no dependency, or
/// for no registered tool at all, is refused with the reason, and no job
/// starts and nothing is recorded, whatever the answer accepted.
#[tokio::test]
async fn add_dependency_for_a_tool_missing_none_is_refused() {
    bounded(async {
        let f = Fixture::rust(false);
        for tool in ["cargo-llvm-cov", "nope"] {
            let answer = json!({ "tool": tool, "consent": [tool], "dependency": ["anything"] });
            let (status, body) = f.post(DEPENDENCY, &answer).await;
            assert!(!status.is_success() && status != StatusCode::CONFLICT, "{tool}: {status} {body}");
            let error = body["error"].as_str().unwrap();
            assert!(error.contains("not missing a coverage dev dependency") || error.contains("not a registered coverage tool"), "{error}");
        }
        assert!(f.get(RUN).await.1["job"].is_null(), "no job started");
        assert!(!f.recorded("coverage-consent.toml") && !f.recorded(COVERAGE_DEPENDENCY_CONSENT_FILE));
    })
    .await;
}

/// S-750, review: an Install while a scan or install of the project holds its
/// lock is refused naming the holder, before any question — nothing is asked,
/// recorded or downloaded — and asked as usual once the lock is free.
#[tokio::test]
async fn an_install_while_the_project_lock_is_held_is_refused() {
    bounded(async {
        let f = Fixture::rust(false);
        let lock_file = f.engine.as_ref().unwrap().security_lock_file().unwrap();
        let holder = logos_core::jobs::lock::Holder::here(logos_core::jobs::JobKind::SecurityScan, None);
        let held = logos_core::jobs::lock::ProjectLock::take(&lock_file, &holder).unwrap().expect("the lock is free");
        let (status, body) = f.post(INSTALL, &json!({})).await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert!(body["error"].as_str().unwrap().contains("already running on this project"), "{body}");
        assert!(body.get("install").is_none(), "no question while the lock is held: {body}");
        assert!(f.runs("curl").is_empty() && !f.recorded(INSTALL_CONSENT_FILE));
        drop(held);
        let (status, question) = f.post(INSTALL, &json!({})).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(question["install"].as_array().unwrap().len(), 2, "{question}");
    })
    .await;
}

/// S-750, review: an install whose tool lands but whose `rustup component
/// add` fails ends the job failed, naming the command and `rustup`'s reason,
/// and `cargo-llvm-cov` — which would fail without the component — never runs.
#[tokio::test]
async fn a_failed_component_step_ends_the_job_failed_and_no_tool_runs() {
    bounded(async {
        let f = Fixture::rust_component_fails();
        let (_, question) = f.post(INSTALL, &json!({})).await;
        let (status, started) = f.post(INSTALL, &accept_all(&question)).await;
        assert_eq!(status, StatusCode::OK, "{started}");
        let job = f.ended().await;
        assert_eq!(job["state"], "failed", "{job}");
        let error = job["error"].as_str().unwrap();
        assert!(error.contains("rustup component add llvm-tools-preview: "), "{error}");
        assert!(error.contains("toolchain is not installed"), "rustup's reason is named: {error}");
        assert_eq!(events(&job, "install_finished")[0]["outcome"], "installed");
        assert_eq!(events(&job, "component_finished")[0]["outcome"], "failed", "{job}");
        assert!(events(&job, "started").is_empty() && f.runs("cargo-llvm-cov").is_empty(), "no tool ran");
    })
    .await;
}
