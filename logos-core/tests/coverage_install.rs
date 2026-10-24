//! `logos coverage install` through the [`Engine`] (S-748, [FR-CV-17],
//! [UAT-CV-07]): the managed `cargo-llvm-cov`, the `llvm-tools-preview` step
//! run by the user's own `rustup`, the consent both need, and the coverage run
//! they make possible.
//!
//! No test reaches the internet, and no real `cargo-llvm-cov` or `rustup`
//! runs. The managed artifact is built here — a shell script standing in for
//! `cargo-llvm-cov`, packed as the builtin manifest's layout — and served by a
//! stub `curl` from `https://fixtures.invalid/…`. The stand-in writes the
//! committed Cobertura report of the `cargo-llvm-cov` fixture project to the
//! path its argv names, but only once the stub `rustup` has added the
//! component, and only under `CARGO_LLVM_COV_SETUP=no`: run any other way it
//! fails, as the real tool does, or leaves the sentinel. The stub `rustup`
//! records every invocation, so "no `rustup` command ran" is a read of its log.
//!
//! [FR-CV-17]: ../../docs/specs/requirements/FR-CV-17.md
//! [UAT-CV-07]: ../../docs/specs/requirements/UAT-CV-07.md
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use logos_core::config::{ConfigError, INSTALL_CONSENT_FILE, TOOLCHAIN_CONSENT_FILE};
use logos_core::security::{ScanEnvironment, Selection};
use logos_core::tools::surface::{CoverageInstallRequest, InstallRequest};
use logos_core::tools::{sha256_file, Layout, Manifest, Platform, LLVM_TOOLS_ADD};
use logos_core::Engine;
use serde_json::Value;
use tempfile::TempDir;

/// Where every fixture artifact claims to come from.
const HOST: &str = "fixtures.invalid";

/// What a `cargo-llvm-cov` that should never have run leaves behind.
const SENTINEL: &str = "LOGOS-RAN-AN-UNVERIFIED-ARTIFACT";

/// One machine: a Rust project, a tools directory, a user-level directory, a
/// search path holding the stubs, and a "server" directory of artifacts.
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
        w.curl_serves();
        copy_tree(&fixture_project(), &w.root());
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
    /// The stub `rustup`'s toolchain: the component is there once this file is.
    fn component(&self) -> PathBuf {
        self.rec().join("llvm-tools-installed")
    }

    /// The engine over this machine, its search path `search` (default: the
    /// stub directory).
    fn engine(&self, manifest: &Manifest, search: Option<std::ffi::OsString>) -> Engine {
        Engine::start(self.root())
            .expect("engine")
            .with_scan_environment(ScanEnvironment {
                user_dir: Some(self.user()),
                search_path: Some(search.unwrap_or_else(|| self.bin().into_os_string())),
                tools_dir: Some(self.tools()),
                nvd_api_key: None,
            })
            .with_tools_manifest(manifest.clone())
    }

    /// A `curl` that serves `https://fixtures.invalid/<rel>` from the server
    /// directory, recording each URL.
    fn curl_serves(&self) {
        script(
            &self.bin().join("curl"),
            &format!(
                "#!/bin/sh\nout=; url=\nwhile [ $# -gt 0 ]; do\n  case \"$1\" in\n    --output) out=\"$2\"; shift 2 ;;\n    *) url=\"$1\"; shift ;;\n  esac\ndone\n\
                 printf '%s\\n' \"$url\" >> '{rec}/curl.log'\nrel=\"${{url#https://{HOST}/}}\"\n/bin/cp '{server}/'\"$rel\" \"$out\" || exit 22\n",
                rec = self.rec().display(),
                server = self.server().display()
            ),
        );
    }

    /// A stub `rustup` at `path`: records `cwd` and argv of each run to
    /// `rec/<log>`; lists the component once [`component`](Self::component)
    /// exists, and `component add llvm-tools-preview` creates it.
    fn rustup_at(&self, path: &Path, log: &str) {
        self.rustup_scripted(path, log, "", &format!(": > '{}'; exit 0", self.component().display()));
    }

    /// The stub `rustup` with `list_first` run before its listing (an `exit`
    /// there fails the listing) and `add` as its `component add`.
    fn rustup_scripted(&self, path: &Path, log: &str, list_first: &str, add: &str) {
        script(
            path,
            &format!(
                "#!/bin/sh\n{{ printf 'cwd=%s\\n' \"$(pwd -P)\"; printf 'arg=%s\\n' \"$@\"; printf -- '--\\n'; }} >> '{rec}/{log}'\n\
                 case \"$*\" in\n\
                 'component list --installed') {list_first}\n printf 'cargo-x86_64-unknown-linux-gnu\\nrust-std-x86_64-unknown-linux-gnu\\n'; \
                 [ -f '{component}' ] && printf 'llvm-tools-x86_64-unknown-linux-gnu\\n'; exit 0 ;;\n\
                 'component add llvm-tools-preview') {add} ;;\n\
                 esac\nexit 1\n",
                rec = self.rec().display(),
                component = self.component().display()
            ),
        );
    }

    fn rustup(&self) {
        self.rustup_at(&self.bin().join("rustup"), "rustup.log");
    }

    fn downloads(&self) -> Vec<String> {
        lines(&self.rec().join("curl.log"))
    }

    /// Each `rustup` invocation recorded in `log`, as its lines.
    fn rustup_runs(&self, log: &str) -> Vec<Vec<String>> {
        fs::read_to_string(self.rec().join(log))
            .unwrap_or_default()
            .split("--\n")
            .filter(|s| !s.is_empty())
            .map(|s| s.lines().map(str::to_string).collect())
            .collect()
    }

    /// The builtin manifest, its `cargo-llvm-cov` artifact for this platform
    /// built here — the stand-in script below, packed as the manifest's
    /// `.tar.gz` — and pinned at its SHA-256, or at `pin` when given.
    fn manifest(&self, pin: Option<&str>) -> Manifest {
        let platform = Platform::current().expect("the suite runs where managed installs exist");
        let mut manifest = Manifest::builtin().clone();
        let entry = &mut manifest.coverage[0];
        assert_eq!(entry.tool, "cargo-llvm-cov");
        let mut artifact = entry.artifact(platform).unwrap().clone();
        let Layout::TarGz { member } = &artifact.layout else {
            panic!("cargo-llvm-cov is a .tar.gz release")
        };
        let src = self.base.join("src");
        fs::create_dir_all(&src).unwrap();
        script(&src.join(member), &self.llvm_cov_script(&entry.version));
        let file = self.server().join("cargo-llvm-cov.tar.gz");
        let packed = Command::new("tar").arg("-czf").arg(&file).arg("-C").arg(&src).arg(member).status().unwrap();
        assert!(packed.success());
        artifact.url = format!("https://{HOST}/cargo-llvm-cov.tar.gz");
        artifact.sha256 = pin.map_or_else(|| sha256_file(&file).unwrap(), str::to_string);
        artifact.size = fs::metadata(&file).unwrap().len();
        entry.artifacts = vec![artifact];
        manifest
    }

    /// The stand-in `cargo-llvm-cov` at `version`: answers its version
    /// command; leaves [`SENTINEL`] in its working directory when run without
    /// `CARGO_LLVM_COV_SETUP=no`; fails, as the real one does, while the
    /// component is missing; else writes the fixture report to the path after
    /// `--output-path`. Records each run's argv.
    fn llvm_cov_script(&self, version: &str) -> String {
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{rec}/cargo-llvm-cov.log'\n\
             [ \"$*\" = 'llvm-cov --version' ] && {{ echo 'cargo-llvm-cov {version}'; exit 0; }}\n\
             [ \"${{CARGO_LLVM_COV_SETUP-}}\" = no ] || {{ : > {SENTINEL}; : > '{rec}/{SENTINEL}'; exit 3; }}\n\
             [ -f '{component}' ] || {{ echo 'error: failed to find llvm-tools-preview, please install llvm-tools-preview with `rustup component add llvm-tools-preview --toolchain stable`' >&2; exit 1; }}\n\
             out=; prev=\nfor a in \"$@\"; do [ \"$prev\" = --output-path ] && out=\"$a\"; prev=\"$a\"; done\n\
             /bin/cp '{report}' \"$out\"\n",
            rec = self.rec().display(),
            component = self.component().display(),
            report = fixture_project().join("fixture-report.xml").display(),
        )
    }

    fn managed(&self, version: &str) -> PathBuf {
        self.tools().join("cargo-llvm-cov").join(version).join("cargo-llvm-cov")
    }
}

fn fixture_project() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/coverage/projects/cargo-llvm-cov")
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

fn script(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, body).unwrap();
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755)).unwrap();
    fs::rename(&tmp, path).unwrap();
}

fn lines(path: &Path) -> Vec<String> {
    fs::read_to_string(path).map(|t| t.lines().map(str::to_string).collect()).unwrap_or_default()
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

fn install(consent: bool) -> CoverageInstallRequest {
    CoverageInstallRequest { from: None, consent }
}

/// The exit-2 refusal's message.
fn refusal<T: std::fmt::Debug>(result: anyhow::Result<T>) -> String {
    let err = result.expect_err("the call must be refused");
    let config = err.downcast_ref::<ConfigError>().unwrap_or_else(|| panic!("not a config error: {err:#}"));
    assert_eq!(config.exit_code(), 2);
    config.to_string()
}

fn json<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap()
}

/// `coverage status`'s row for `cargo-llvm-cov`.
fn status_row(engine: &Engine) -> Value {
    let status = json(&engine.coverage_status().unwrap());
    status["runner"]["tools"].as_array().unwrap().iter().find(|t| t["tool"] == "cargo-llvm-cov").unwrap().clone()
}

/// [FR-CV-17] AC1, [UAT-CV-07] step 1: a Rust project with neither tool
/// reports `cargo-llvm-cov` missing with the install command; the install is
/// refused until consented, naming the download and the `rustup` command,
/// with nothing downloaded and no `rustup` run; after one consent both are
/// installed — the component by `rustup`, from the filesystem root — the copy
/// reads `managed`, and `coverage run` ingests its Cobertura report.
#[test]
fn neither_tool_reports_the_install_command_then_one_consent_installs_both_and_a_run_ingests() {
    let w = World::new();
    w.rustup();
    let manifest = w.manifest(None);
    let engine = w.engine(&manifest, None);
    let entry = &manifest.coverage[0];

    let row = status_row(&engine);
    assert_eq!(row["state"], "not_installed", "{row}");
    assert!(row["hint"].as_str().unwrap().contains("logos coverage install"), "{row}");

    let refused = refusal(engine.coverage_install(&install(false)));
    let mb = format!("({:.1} MB)", entry.artifacts[0].size as f64 / 1_000_000.0);
    for named in [
        format!("cargo-llvm-cov {} from {HOST} {mb}", entry.version),
        format!("`{LLVM_TOOLS_ADD}` on your active Rust toolchain"),
        "logos coverage install --consent".to_string(),
        "Nothing was downloaded and no rustup command ran".to_string(),
    ] {
        assert!(refused.contains(&named), "names {named:?}: {refused}");
    }
    assert!(w.downloads().is_empty(), "nothing was downloaded");
    assert!(w.rustup_runs("rustup.log").is_empty(), "no rustup command ran before the consent");
    assert!(!w.tools().exists() && !w.user().join(INSTALL_CONSENT_FILE).exists());

    let done = engine.coverage_install(&install(true)).unwrap();
    assert_eq!(done.exit_code(), 0, "{done:?}");
    let payload = json(&done);
    assert_eq!(
        payload["consent_recorded"],
        serde_json::json!([format!("cargo-llvm-cov {}", entry.version), LLVM_TOOLS_ADD])
    );
    assert_eq!(payload["tools"][0]["outcome"], "installed", "{payload}");
    assert_eq!(payload["llvm_tools"]["outcome"], "added", "{payload}");
    assert_eq!(w.downloads(), [format!("https://{HOST}/cargo-llvm-cov.tar.gz")]);
    assert!(w.managed(&entry.version).is_file());
    let runs = w.rustup_runs("rustup.log");
    let argvs: Vec<Vec<&str>> =
        runs.iter().map(|run| run.iter().filter_map(|l| l.strip_prefix("arg=")).collect()).collect();
    assert_eq!(argvs, [vec!["component", "list", "--installed"], vec!["component", "add", "llvm-tools-preview"]]);
    assert!(runs.iter().all(|run| run.contains(&"cwd=/".to_string())), "rustup runs from the filesystem root: {runs:?}");
    assert!(fs::read_to_string(w.user().join(TOOLCHAIN_CONSENT_FILE)).unwrap().contains("llvm-tools-preview"));
    let recorded = logos_core::config::load_install_consent(Some(&w.user())).unwrap();
    assert!(recorded.covers("cargo-llvm-cov", &entry.version), "the install consent is on disk, not only in the payload");

    let row = status_row(&engine);
    assert_eq!((row["state"].as_str(), row["origin"].as_str()), (Some("never_run"), Some("managed")), "{row}");
    assert_eq!(row["version"], entry.version.as_str());

    let summary = engine.coverage_run(None, true).unwrap();
    let ran = json(&summary)["tools"].as_array().unwrap().iter().find(|t| t["tool"] == "cargo-llvm-cov").unwrap().clone();
    assert_eq!(ran["outcome"], "ran", "{ran}");
    assert_eq!(ran["ingests"][0]["format"], "cobertura", "{ran}");
    assert_eq!(ran["ingests"][0]["matched_files"], 1, "{ran}");
    assert!(!w.rec().join(SENTINEL).exists() && !w.root().join(SENTINEL).exists());

    // Again: everything is in place, so nothing is asked, fetched or added.
    let again = json(&engine.coverage_install(&install(false)).unwrap());
    assert_eq!(again["kept"][0]["origin"], "managed", "{again}");
    assert_eq!(again["tools"], serde_json::json!([]));
    assert_eq!(again["llvm_tools"]["outcome"], "already_present", "{again}");
    assert_eq!(w.downloads().len(), 1);
}

/// [FR-CV-17] AC2: a user's own `cargo-llvm-cov` on `PATH` at the minimum is
/// used — by the install, which keeps it, and by the run — and never
/// downloaded; one below the minimum is not kept.
#[test]
fn a_users_own_cargo_llvm_cov_is_used_and_never_downloaded() {
    let w = World::new();
    w.rustup();
    let manifest = w.manifest(None);
    let engine = w.engine(&manifest, None);
    let entry = &manifest.coverage[0];
    let own = w.bin().join("cargo-llvm-cov");

    script(&own, &w.llvm_cov_script("0.5.9"));
    let refused = refusal(engine.coverage_install(&install(false)));
    assert!(refused.contains(&format!("cargo-llvm-cov {} from", entry.version)), "below the minimum is not kept: {refused}");

    script(&own, &w.llvm_cov_script(&entry.min_version));
    let done = json(&engine.coverage_install(&install(true)).unwrap());
    assert_eq!(done["kept"][0]["origin"], "path", "{done}");
    assert_eq!(done["kept"][0]["binary"], own.to_str().unwrap());
    assert_eq!(done["kept"][0]["version"], entry.min_version.as_str());
    assert_eq!(done["tools"], serde_json::json!([]), "nothing installed");
    assert_eq!(done["consent_recorded"], serde_json::json!([LLVM_TOOLS_ADD]), "only the rustup line was asked for");
    assert!(w.downloads().is_empty(), "never downloaded");
    assert!(!w.tools().join("cargo-llvm-cov").exists());

    assert_eq!(status_row(&engine)["origin"], "path");
    let summary = json(&engine.coverage_run(Some("cargo-llvm-cov"), true).unwrap());
    assert_eq!(summary["tools"][0]["outcome"], "ran", "{summary}");
    let runs = lines(&w.rec().join("cargo-llvm-cov.log"));
    assert!(runs.iter().any(|r| r.starts_with("llvm-cov --workspace --cobertura")), "the user's copy ran: {runs:?}");
}

/// [FR-CV-17] AC3: without `rustup` — and a `rustup` inside the project is no
/// `rustup` — the component is reported with the manual command, nothing is
/// asked for it, nothing runs, and the toolchain is as it was.
#[test]
fn without_rustup_the_component_is_reported_with_the_manual_command_and_nothing_changes() {
    let w = World::new();
    // Only a project-supplied `rustup` is on the search path.
    let planted = w.root().join("bin");
    w.rustup_at(&planted.join("rustup"), "planted.log");
    let search = std::env::join_paths([w.bin(), planted]).unwrap();
    let manifest = w.manifest(None);
    let engine = w.engine(&manifest, Some(search));

    let refused = refusal(engine.coverage_install(&install(false)));
    assert!(!refused.contains("rustup component add"), "no rustup line without rustup: {refused}");

    let done = engine.coverage_install(&install(true)).unwrap();
    assert_eq!(done.exit_code(), 0, "the managed copy is in place: {done:?}");
    let payload = json(&done);
    assert_eq!(payload["tools"][0]["outcome"], "installed");
    let step = &payload["llvm_tools"];
    assert_eq!(step["outcome"], "no_rustup", "{payload}");
    assert_eq!(step["command"], LLVM_TOOLS_ADD);
    assert!(step["reason"].as_str().unwrap().contains("nothing was changed"));
    assert!(w.rustup_runs("planted.log").is_empty(), "the project's rustup never ran");
    assert!(!w.component().exists(), "the toolchain is as it was");
    assert!(!w.user().join(TOOLCHAIN_CONSENT_FILE).exists(), "no toolchain consent was asked for or recorded");
}

/// [FR-CV-17] AC4, [NFR-SE-10]: an artifact whose SHA-256 is not the pin is
/// deleted and never run — nothing lands in the tools directory, the stand-in
/// leaves no sentinel — and the install exits 1.
///
/// [NFR-SE-10]: ../../docs/specs/requirements/NFR-SE-10.md
#[test]
fn a_checksum_mismatch_never_executes_the_download() {
    let w = World::new();
    w.rustup();
    let manifest = w.manifest(Some(&"0".repeat(64)));
    let engine = w.engine(&manifest, None);

    let done = engine.coverage_install(&install(true)).unwrap();
    let payload = json(&done);
    assert_eq!(payload["tools"][0]["outcome"], "failed", "{payload}");
    assert_eq!(payload["tools"][0]["stage"], "checksum", "{payload}");
    assert!(payload["tools"][0]["reason"].as_str().unwrap().contains("nothing from it was run"));
    assert_eq!(done.exit_code(), 1);
    assert_eq!(w.downloads().len(), 1, "it was fetched");
    assert!(!w.tools().join("cargo-llvm-cov").exists(), "nothing was installed");
    assert!(lines(&w.rec().join("cargo-llvm-cov.log")).is_empty(), "the download never ran");
    assert!(!w.rec().join(SENTINEL).exists());
    assert_eq!(status_row(&engine)["state"], "not_installed");
}

/// The security surfaces never list, install or remove a coverage tool: it is
/// no tool `security install` knows, and `--all` names none.
#[test]
fn the_security_install_never_touches_the_coverage_tool() {
    let w = World::new();
    let engine = w.engine(&w.manifest(None), None);
    let named = InstallRequest {
        tools: Selection::Only(vec!["cargo-llvm-cov".to_string()]),
        update: false,
        from: None,
        consent: true,
        accept_licences: Vec::new(),
    };
    let refused = refusal(engine.security_install(&named));
    assert!(refused.contains("\"cargo-llvm-cov\" is not a tool Logos installs"), "{refused}");
    let all = InstallRequest { tools: Selection::All, consent: false, ..named };
    let refused = refusal(engine.security_install(&all));
    assert!(refused.contains("trivy") && !refused.contains("cargo-llvm-cov"), "{refused}");
    assert!(refusal(engine.security_uninstall(&Selection::Only(vec!["cargo-llvm-cov".to_string()]))).contains("is not a tool"));
    assert!(w.downloads().is_empty());
}

/// [FR-CV-13] under [FR-CV-17]: a managed copy is found without a version
/// probe, so the execution consent a run checks — found with no probe —
/// still names it: once installed, `coverage run` without `--consent` is
/// refused naming `cargo-llvm-cov`, and the copy never starts.
///
/// [FR-CV-13]: ../../docs/specs/requirements/FR-CV-13.md
#[test]
fn a_managed_copy_still_needs_the_run_consent() {
    let w = World::new();
    w.rustup();
    let manifest = w.manifest(None);
    let engine = w.engine(&manifest, None);
    assert_eq!(engine.coverage_install(&install(true)).unwrap().exit_code(), 0);
    let before = lines(&w.rec().join("cargo-llvm-cov.log"));

    let refused = refusal(engine.coverage_run(Some("cargo-llvm-cov"), false));
    assert!(refused.contains("cargo-llvm-cov") && refused.contains("--consent"), "{refused}");
    assert_eq!(lines(&w.rec().join("cargo-llvm-cov.log")), before, "the managed copy never started");
}

/// A `rustup component add` that fails fails the step — with what `rustup`
/// printed — and the install exits 1, the managed copy still installed.
#[test]
fn a_failed_rustup_add_fails_the_step_and_the_install_exits_1() {
    let w = World::new();
    let stderr = "error: toolchain 'stable-x86_64-unknown-linux-gnu' is not installed";
    w.rustup_scripted(&w.bin().join("rustup"), "rustup.log", "", &format!("echo \"{stderr}\" >&2; exit 1"));
    let engine = w.engine(&w.manifest(None), None);

    let done = engine.coverage_install(&install(true)).unwrap();
    let payload = json(&done);
    assert_eq!(payload["tools"][0]["outcome"], "installed", "{payload}");
    assert_eq!(payload["llvm_tools"]["outcome"], "failed", "{payload}");
    let reason = payload["llvm_tools"]["reason"].as_str().unwrap();
    assert!(reason.contains("rustup exited with 1") && reason.contains(stderr), "{reason}");
    assert_eq!(done.exit_code(), 1);
    assert!(!w.component().exists());
}

/// A `rustup component list` that fails reads as the component missing:
/// the add runs, and it is what tells.
#[test]
fn a_failed_listing_reads_as_missing_and_the_component_is_added() {
    let w = World::new();
    let add = format!(": > '{}'; exit 0", w.component().display());
    w.rustup_scripted(&w.bin().join("rustup"), "rustup.log", "echo 'error: no default toolchain' >&2; exit 1", &add);
    let engine = w.engine(&w.manifest(None), None);

    let done = json(&engine.coverage_install(&install(true)).unwrap());
    assert_eq!(done["llvm_tools"]["outcome"], "added", "{done}");
    assert!(w.component().exists());
    assert_eq!(w.rustup_runs("rustup.log").len(), 2, "listed, then added");
}

/// A user-level override that cannot run is the user's choice: it is kept
/// — nothing is downloaded beside it, since it would never be used — and the
/// install exits 0, while `coverage status` reports it not installed with the
/// override's hint.
#[test]
fn an_override_that_cannot_run_is_kept_and_nothing_is_downloaded_beside_it() {
    let w = World::new();
    w.rustup();
    let broken = w.base.join("own/cargo-llvm-cov");
    fs::create_dir_all(broken.parent().unwrap()).unwrap();
    fs::write(&broken, "not a program").unwrap();
    fs::write(
        w.user().join("config.toml"),
        format!("[coverage_ingest.tools.cargo-llvm-cov]\npath = \"{}\"\n", broken.display()),
    )
    .unwrap();
    let engine = w.engine(&w.manifest(None), None);

    let done = engine.coverage_install(&install(true)).unwrap();
    let payload = json(&done);
    assert_eq!(payload["kept"][0]["origin"], "override", "{payload}");
    assert_eq!(payload["kept"][0]["binary"], broken.to_str().unwrap());
    assert_eq!(payload["kept"][0]["version"], Value::Null);
    assert_eq!(payload["tools"], serde_json::json!([]));
    assert_eq!(done.exit_code(), 0);
    assert!(w.downloads().is_empty() && !w.tools().join("cargo-llvm-cov").exists());
    let row = status_row(&engine);
    assert_eq!(row["state"], "not_installed", "{row}");
    assert!(row["hint"].as_str().unwrap().contains("override"), "{row}");
}

/// With the user's own copy kept, only the component step runs, and it needs
/// neither a tools directory nor a platform with managed installs: with none
/// resolvable, the install still adds the component and writes no tools
/// directory anywhere.
#[test]
fn with_a_copy_kept_the_component_step_needs_no_tools_directory() {
    let w = World::new();
    w.rustup();
    let manifest = w.manifest(None);
    let own = w.bin().join("cargo-llvm-cov");
    script(&own, &w.llvm_cov_script(&manifest.coverage[0].min_version));
    let engine = Engine::start(w.root())
        .expect("engine")
        .with_scan_environment(ScanEnvironment {
            user_dir: Some(w.user()),
            search_path: Some(w.bin().into_os_string()),
            tools_dir: None,
            nvd_api_key: None,
        })
        .with_tools_manifest(manifest);

    let done = engine.coverage_install(&install(true)).unwrap();
    let payload = json(&done);
    assert_eq!(payload["kept"][0]["origin"], "path", "{payload}");
    assert_eq!(payload["llvm_tools"]["outcome"], "added", "{payload}");
    assert_eq!(done.exit_code(), 0);
    assert!(w.downloads().is_empty() && !w.tools().exists());
}
