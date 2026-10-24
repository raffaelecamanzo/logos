//! Dependency-Check and CodeQL install only on explicit opt-in (S-746,
//! [FR-SC-23], [NFR-SE-10]), through the [`Engine`]:
//! - `install --all` and Install & scan never select either;
//! - CodeQL is refused, its licence terms shown, without
//!   `--accept-codeql-licence`, and the acknowledgement is recorded per user
//!   with the install consent and shown in `status`;
//! - Dependency-Check installs with the managed Java runtime, runs on it with
//!   no `java` on the search path, and its SARIF ingests;
//! - `uninstall` removes the Java runtime once no installed tool needs it.
//!
//! No test reaches the internet ([ADR-88]'s fitness function). Each test
//! builds its own artifacts in the builtin manifest's layouts — a zip tree
//! holding a stub `dependency-check.sh` launcher, a tar.gz tree holding a stub
//! `bin/java`, a tar.gz tree holding a stub `codeql` — and hands the engine a
//! manifest pinning them at `https://fixtures.invalid/<file>`. `curl` is a stub
//! serving them; `tar`, `gzip` and `unzip` are the system's. The stub launcher
//! does what the real one does — it runs `$JAVACMD` and fails without it — and
//! the stub Java writes the report.
//!
//! [FR-SC-23]: ../../docs/specs/requirements/FR-SC-23.md
//! [NFR-SE-10]: ../../docs/specs/requirements/NFR-SE-10.md
//! [ADR-88]: ../../docs/specs/architecture/decisions/ADR-88.md
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use logos_core::config::{ConfigError, INSTALL_CONSENT_FILE};
use logos_core::security::{ScanEnvironment, ScanRequest, Selection};
use logos_core::tools::surface::InstallRequest;
use logos_core::tools::{sha256_file, Archive, Layout, Manifest, Platform};
use logos_core::Engine;
use serde_json::Value;
use tempfile::TempDir;

/// Where every fixture artifact claims to come from.
const HOST: &str = "fixtures.invalid";

/// One machine: a project, a tools directory, a user-level directory, a search
/// path holding the stub `curl` and the system unpackers — and no `java` — and
/// a "server" directory of artifacts.
struct World {
    _tmp: TempDir,
    base: PathBuf,
    manifest: Manifest,
}

impl World {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        for dir in ["bin", "rec", "project", "user", "server", "src"] {
            fs::create_dir_all(base.join(dir)).unwrap();
        }
        for program in ["tar", "gzip", "unzip"] {
            let found = on_process_path(program)
                .unwrap_or_else(|| panic!("the suite needs the system {program}"));
            // The unpackers record the default-option variables they see.
            let body = match program {
                "gzip" => {
                    std::os::unix::fs::symlink(found, base.join("bin").join(program)).unwrap();
                    continue;
                }
                "tar" => "${TAR_OPTIONS-unset}",
                _ => "${UNZIP-unset}|${UNZIPOPT-unset}",
            };
            script(
                &base.join("bin").join(program),
                &format!(
                    "#!/bin/sh\nprintf '%s\\n' \"{body}\" >> '{}/{program}.log'\nexec '{}' \"$@\"\n",
                    base.join("rec").display(),
                    found.display()
                ),
            );
        }
        let mut w = World {
            _tmp: tmp,
            base,
            manifest: Manifest::builtin().clone(),
        };
        w.curl_serves();
        fs::write(w.root().join("main.py"), "def main():\n    return 1\n").unwrap();
        git(&w.root(), &["init", "-q", "-b", "main"]);
        git(&w.root(), &["add", "."]);
        git(&w.root(), &["commit", "-q", "-m", "seed"]);
        Engine::start(w.root()).expect("engine").index();
        w.manifest = w.build_manifest();
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

    fn engine(&self) -> Engine {
        Engine::start(self.root())
            .expect("engine")
            .with_scan_environment(ScanEnvironment {
                user_dir: Some(self.user()),
                search_path: Some(self.bin().into_os_string()),
                tools_dir: Some(self.tools()),
                nvd_api_key: None,
            })
            .with_tools_manifest(self.manifest.clone())
    }

    /// A `curl` that serves `https://fixtures.invalid/<file>` from the server
    /// directory, recording each URL; a file that is not there fails as an
    /// HTTP error does.
    fn curl_serves(&self) {
        script(
            &self.bin().join("curl"),
            &format!(
                "#!/bin/sh\nout=; url=\nwhile [ $# -gt 0 ]; do\n  case \"$1\" in\n    --output) out=\"$2\"; shift 2 ;;\n    *) url=\"$1\"; shift ;;\n  esac\ndone\nprintf '%s\\n' \"$url\" >> '{rec}/curl.log'\n/bin/cp '{server}/'\"${{url#https://{HOST}/}}\" \"$out\" || exit 22\n",
                rec = self.rec().display(),
                server = self.server().display()
            ),
        );
    }

    fn downloads(&self) -> Vec<String> {
        lines(&self.rec().join("curl.log"))
    }

    /// The builtin manifest's Trivy (one default tool), Dependency-Check,
    /// CodeQL and Java runtime, each artifact for this platform built here in
    /// the builtin layout, at the builtin pins.
    fn build_manifest(&self) -> Manifest {
        let platform = Platform::current().expect("the suite runs where managed installs exist");
        let mut manifest = Manifest::builtin().clone();
        manifest
            .tools
            .retain(|t| ["trivy", "dependency-check", "codeql"].contains(&t.tool.as_str()));
        let rec = self.rec();
        for entry in manifest.tools.iter_mut().chain([&mut manifest.java]) {
            let mut artifact = entry.artifact(platform).unwrap().clone();
            let src = self.base.join("src").join(&entry.tool);
            let file = match &artifact.layout {
                Layout::TarGz { member } => {
                    script(&src.join(member), &trivy(&entry.version));
                    self.pack_tar(&src, member, &entry.tool)
                }
                Layout::Tree { archive, root } => {
                    let tree = src.join(root);
                    let body = match entry.tool.as_str() {
                        "dependency-check" => LAUNCHER.to_string(),
                        "codeql" => codeql(&entry.version),
                        "java" => java(&rec, &self.manifest_dc_version()),
                        other => panic!("no fixture for {other}"),
                    };
                    script(&tree.join(&entry.binary), &body);
                    // Something beside the executable, as the real trees have.
                    fs::write(tree.join("NOTICE"), "fixture\n").unwrap();
                    let top = root.split('/').next().unwrap();
                    match archive {
                        Archive::TarGz => self.pack_tar(&src, top, &entry.tool),
                        Archive::Zip => self.pack_zip(&src, top, &entry.tool),
                    }
                }
                Layout::Binary => unreachable!("no raw binary among these"),
            };
            artifact.url = format!(
                "https://{HOST}/{}",
                file.file_name().unwrap().to_str().unwrap()
            );
            artifact.sha256 = sha256_file(&file).unwrap();
            artifact.size = fs::metadata(&file).unwrap().len();
            entry.artifacts = vec![artifact];
        }
        manifest
    }

    fn manifest_dc_version(&self) -> String {
        Manifest::builtin()
            .tool("dependency-check")
            .unwrap()
            .version
            .clone()
    }

    fn pack_tar(&self, src: &Path, member: &str, tool: &str) -> PathBuf {
        let file = self.server().join(format!("{tool}.tar.gz"));
        let status = Command::new("tar")
            .arg("-czf")
            .arg(&file)
            .arg("-C")
            .arg(src)
            .arg(member)
            .status()
            .unwrap();
        assert!(status.success(), "tar packs {tool}");
        file
    }

    fn pack_zip(&self, src: &Path, top: &str, tool: &str) -> PathBuf {
        let file = self.server().join(format!("{tool}.zip"));
        let zip =
            on_process_path("zip").expect("the suite needs the system zip to build a fixture");
        let status = Command::new(zip)
            .current_dir(src)
            .args(["-q", "-r"])
            .arg(&file)
            .arg(top)
            .status()
            .unwrap();
        assert!(status.success(), "zip packs {tool}");
        file
    }

    fn version(&self, tool: &str) -> String {
        match tool {
            "java" => self.manifest.java.version.clone(),
            _ => self.manifest.tool(tool).unwrap().version.clone(),
        }
    }

    fn request(&self, tools: &[&str], consent: bool, accept: &[&str]) -> InstallRequest {
        let tools = match tools {
            [] => Selection::All,
            names => Selection::Only(names.iter().map(|n| n.to_string()).collect()),
        };
        InstallRequest {
            tools,
            update: false,
            from: None,
            consent,
            accept_licences: accept.iter().map(|n| n.to_string()).collect(),
        }
    }

    fn installed(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(self.tools())
            .map(|entries| {
                entries
                    .flatten()
                    .map(|e| e.file_name().into_string().unwrap())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }
}

/// Dependency-Check's launcher, as the real one behaves: `$JAVACMD`, else
/// `$JAVA_HOME/bin/java`, else `java` from `PATH`, else the real error.
const LAUNCHER: &str = "#!/bin/sh\n\
    if [ -z \"$JAVACMD\" ]; then\n\
      if [ -n \"$JAVA_HOME\" ]; then JAVACMD=\"$JAVA_HOME/bin/java\"; else JAVACMD=`command -v java`; fi\n\
    fi\n\
    if [ ! -x \"$JAVACMD\" ]; then echo 'Error: JAVA_HOME is not defined correctly.' >&2; exit 1; fi\n\
    exec \"$JAVACMD\" -classpath lib org.owasp.dependencycheck.App \"$@\"\n";

/// A stub Java that runs Dependency-Check: it records the `JAVA_HOME` it ran
/// with, prints the version, and writes one finding's SARIF to `--out`.
fn java(rec: &Path, version: &str) -> String {
    let sarif = r#"{"version":"2.1.0","runs":[{"tool":{"driver":{"name":"dependency-check","rules":[]}},"results":[{"ruleId":"CVE-2026-0001","level":"error","message":{"text":"a vulnerable dependency"},"locations":[{"physicalLocation":{"artifactLocation":{"uri":"main.py"},"region":{"startLine":1}}}]}]}]}"#;
    format!(
        "#!/bin/sh\n\
         printf '%s\\n' \"$JAVA_HOME\" >> '{rec}/java.log'\n\
         shift 3\n\
         case \"$1\" in --version) printf '%s\\n' 'dependency-check-cli version {version}'; exit 0;; esac\n\
         out=; prev=\n\
         for a in \"$@\"; do [ \"$prev\" = --out ] && out=\"$a\"; prev=\"$a\"; done\n\
         printf '%s' '{sarif}' > \"$out/dependency-check-report.sarif\"\n",
        rec = rec.display()
    )
}

fn codeql(version: &str) -> String {
    format!("#!/bin/sh\nprintf '%s\\n' '{version}'\n")
}

fn trivy(version: &str) -> String {
    format!("#!/bin/sh\nprintf '%s\\n' 'Version: {version}'\n")
}

fn script(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn lines(path: &Path) -> Vec<String> {
    fs::read_to_string(path)
        .map(|t| t.lines().map(str::to_string).collect())
        .unwrap_or_default()
}

fn on_process_path(program: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|d| d.join(program))
        .find(|p| p.is_file())
}

fn git(root: &Path, args: &[&str]) {
    let out = Command::new("git")
        .current_dir(root)
        .env("GIT_CEILING_DIRECTORIES", root.parent().unwrap())
        .args([
            "-c",
            "maintenance.auto=false",
            "-c",
            "gc.auto=0",
            "-c",
            "user.email=t@logos",
            "-c",
            "user.name=t",
        ])
        .args(args)
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?}");
}

/// The exit-2 refusal's message, or a panic naming what came back instead.
fn refusal<T: std::fmt::Debug>(result: anyhow::Result<T>) -> String {
    let err = result.expect_err("the call must be refused");
    let config = err
        .downcast_ref::<ConfigError>()
        .unwrap_or_else(|| panic!("not a config error (exit 2): {err:#}"));
    assert_eq!(config.exit_code(), 2);
    config.to_string()
}

fn json<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap()
}

fn outcomes(payload: &Value) -> Vec<(String, String)> {
    payload["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| {
            (
                t["tool"].as_str().unwrap().to_string(),
                t["outcome"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

fn status_row(engine: &Engine, tool: &str) -> Value {
    let status = json(&engine.security_status().unwrap());
    status["scanners"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["scanner"] == tool)
        .unwrap()
        .clone()
}

/// `install --all` and Install & scan never select Dependency-Check or
/// CodeQL: neither is in the consent they ask for, neither is fetched, and
/// neither (nor the Java runtime) lands in the tools directory.
#[test]
fn install_all_and_install_and_scan_never_select_the_opt_in_pair() {
    let w = World::new();
    let engine = w.engine();

    let refused = refusal(engine.security_install(&w.request(&[], false, &[])));
    assert!(
        refused.contains(&format!("trivy {} from {HOST} (", w.version("trivy"))),
        "{refused}"
    );
    for opt_in in ["dependency-check", "codeql", "java"] {
        assert!(
            !refused.contains(opt_in),
            "--all does not ask for {opt_in}: {refused}"
        );
    }

    let installed = json(&engine.security_install(&w.request(&[], true, &[])).unwrap());
    assert_eq!(
        outcomes(&installed),
        [("trivy".to_string(), "installed".to_string())]
    );
    assert_eq!(
        installed["consent_recorded"],
        serde_json::json!([format!("trivy {}", w.version("trivy"))])
    );
    let scanned = json(&engine.security_scan_install(true).unwrap());
    assert!(
        outcomes(&scanned["install"]).is_empty(),
        "nothing else is installed for a scan: {scanned}"
    );
    assert_eq!(scanned["install"]["kept"][0]["tool"], "trivy", "{scanned}");

    assert_eq!(
        w.downloads().len(),
        1,
        "only trivy was fetched: {:?}",
        w.downloads()
    );
    assert_eq!(w.installed(), ["trivy"]);
    let consent = fs::read_to_string(w.user().join(INSTALL_CONSENT_FILE)).unwrap();
    assert!(
        !consent.contains("codeql") && !consent.contains("dependency-check"),
        "{consent}"
    );
    // Each is still offered by name.
    assert_eq!(
        status_row(&engine, "dependency-check")["install"],
        "logos security install dependency-check"
    );
    // …and marked opt-in, so a page never offers `scan --install` for it.
    for opt_in in ["dependency-check", "codeql"] {
        assert_eq!(status_row(&engine, opt_in)["opt_in"], true, "{opt_in}");
    }
    assert!(status_row(&engine, "trivy").get("opt_in").is_none(), "absent on the default set");
}

/// CodeQL is refused with its licence terms until `--accept-codeql-licence`;
/// the acknowledgement is recorded only when the install goes ahead, per
/// version and with the install consent, is shown in `status`, and is not
/// asked for again.
#[test]
fn codeql_is_refused_without_its_licence_and_the_acknowledgement_is_recorded_and_shown() {
    let w = World::new();
    let engine = w.engine();
    let version = w.version("codeql");
    let consent_file = w.user().join(INSTALL_CONSENT_FILE);

    let before = status_row(&engine, "codeql");
    assert_eq!(
        before["install"],
        "logos security install codeql --accept-codeql-licence"
    );
    assert_eq!(
        before["licence"],
        serde_json::json!({"accepted": [], "pinned_accepted": false})
    );
    assert!(
        status_row(&engine, "trivy").get("licence").is_none(),
        "only a licensed scanner carries one"
    );

    // Even with the install consent given, the licence is asked for first.
    let refused = refusal(engine.security_install(&w.request(&["codeql"], true, &[])));
    for shown in [
        "GitHub CodeQL Terms and Conditions",
        "open-source codebases",
        "academic research",
        "Nothing was downloaded",
        "`logos security install codeql --accept-codeql-licence --consent`",
    ] {
        assert!(refused.contains(shown), "shows `{shown}`: {refused}");
    }
    assert!(
        w.downloads().is_empty() && !consent_file.exists(),
        "a refusal fetches and records nothing"
    );

    // The licence alone records nothing while the install consent is missing.
    let refused = refusal(engine.security_install(&w.request(&["codeql"], false, &["codeql"])));
    assert!(
        refused.contains(&format!("codeql {version} from {HOST} (")),
        "{refused}"
    );
    assert!(
        refused.contains("--accept-codeql-licence --consent"),
        "{refused}"
    );
    assert!(w.downloads().is_empty() && !consent_file.exists());

    let installed = json(
        &engine
            .security_install(&w.request(&["codeql"], true, &["codeql"]))
            .unwrap(),
    );
    assert_eq!(
        outcomes(&installed),
        [("codeql".to_string(), "installed".to_string())]
    );
    assert_eq!(
        installed["licence_recorded"],
        serde_json::json!([format!("codeql {version}")])
    );
    assert_eq!(
        installed["consent_recorded"],
        serde_json::json!([format!("codeql {version}")])
    );
    let record = fs::read_to_string(&consent_file).unwrap();
    assert!(
        record.contains("[licence]") && record.contains(&format!("codeql = [\"{version}\"]")),
        "{record}"
    );
    assert!(w
        .tools()
        .join("codeql")
        .join(&version)
        .join("codeql")
        .is_file());
    assert!(
        w.tools()
            .join("codeql")
            .join(&version)
            .join("NOTICE")
            .is_file(),
        "the whole tree is installed"
    );
    assert_eq!(w.installed(), ["codeql"], "CodeQL needs no Java runtime");

    let after = status_row(&engine, "codeql");
    assert_eq!(
        after["licence"],
        serde_json::json!({"accepted": [version], "pinned_accepted": true})
    );
    assert_eq!(after["installed"]["copy"], "managed", "{after}");
    assert_eq!(after["installed"]["version"], version, "{after}");
    assert!(after["install"].is_null(), "{after}");

    // Once accepted, a reinstall asks for neither.
    engine
        .security_uninstall(&Selection::Only(vec!["codeql".to_string()]))
        .unwrap();
    assert_eq!(
        status_row(&engine, "codeql")["install"],
        "logos security install codeql"
    );
    let again = json(
        &engine
            .security_install(&w.request(&["codeql"], false, &[]))
            .unwrap(),
    );
    assert_eq!(
        outcomes(&again),
        [("codeql".to_string(), "installed".to_string())]
    );
    assert_eq!(again["licence_recorded"], serde_json::json!([]));
}

/// Dependency-Check installs with the managed Java runtime — a consent line of
/// its own — and runs on it with no `java` on the search path: its version
/// probe and its scan both reach the managed runtime, and its SARIF ingests.
#[test]
fn dependency_check_runs_on_the_managed_java_runtime_and_its_sarif_ingests() {
    let w = World::new();
    let engine = w.engine();
    let (dc, jre) = (w.version("dependency-check"), w.version("java"));
    assert!(!w.bin().join("java").exists(), "the machine has no Java");

    let refused = refusal(engine.security_install(&w.request(&["dependency-check"], false, &[])));
    for line in [
        format!("dependency-check {dc} from {HOST} ("),
        format!("java {jre} from {HOST} ("),
    ] {
        assert!(refused.contains(&line), "names `{line}`: {refused}");
    }
    assert!(w.downloads().is_empty());

    let installed = json(
        &engine
            .security_install(&w.request(&["dependency-check"], true, &[]))
            .unwrap(),
    );
    assert_eq!(
        outcomes(&installed),
        [("dependency-check".to_string(), "installed".to_string())]
    );
    assert_eq!(
        installed["consent_recorded"],
        serde_json::json!([format!("dependency-check {dc}"), format!("java {jre}")])
    );
    assert_eq!(w.downloads().len(), 2, "{:?}", w.downloads());
    assert_eq!(w.installed(), ["dependency-check", "java"]);
    let home = w.tools().join("java").join(&jre);
    assert!(home.join("bin").join("java").is_file());
    assert!(w
        .tools()
        .join("dependency-check")
        .join(&dc)
        .join("bin")
        .join("dependency-check.sh")
        .is_file());

    let row = status_row(&engine, "dependency-check");
    assert_eq!(row["installed"]["copy"], "managed", "{row}");
    assert_eq!(
        row["installed"]["version"], dc,
        "the probe ran on the managed Java: {row}"
    );

    let scan = json(
        &engine
            .security_scan_selected(
                true,
                &ScanRequest::new(vec!["dependency-check".to_string()], Vec::new()),
                |_| {},
            )
            .unwrap(),
    );
    let run = scan["scanners"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["scanner"] == "dependency-check")
        .unwrap();
    assert_eq!(run["outcome"], "ran", "{scan}");
    assert_eq!(scan["findings"], 1, "the report ingested: {scan}");
    let homes = lines(&w.rec().join("java.log"));
    assert!(
        homes.len() >= 2,
        "the probe and the scan both ran Java: {homes:?}"
    );
    assert!(
        homes.iter().all(|h| Path::new(h) == home),
        "always the managed runtime: {homes:?}"
    );
}

/// The Java runtime goes once no installed tool needs it — not while one
/// does, and not for a tool that never needed it; a Dependency-Check whose
/// runtime is gone needs only the runtime again.
#[test]
fn uninstall_removes_the_java_runtime_only_once_no_installed_tool_needs_it() {
    let w = World::new();
    let engine = w.engine();
    let all = w.request(&["dependency-check", "codeql", "trivy"], true, &["codeql"]);
    engine.security_install(&all).unwrap();
    assert_eq!(
        w.installed(),
        ["codeql", "dependency-check", "java", "trivy"]
    );

    let only = |tool: &str| Selection::Only(vec![tool.to_string()]);
    engine.security_uninstall(&only("codeql")).unwrap();
    engine.security_uninstall(&only("trivy")).unwrap();
    assert_eq!(
        w.installed(),
        ["dependency-check", "java"],
        "Dependency-Check still needs it"
    );

    // A runtime removed by hand leaves Dependency-Check to be installed, and
    // only the runtime is fetched for it.
    fs::remove_dir_all(w.tools().join("java")).unwrap();
    assert_eq!(
        status_row(&engine, "dependency-check")["state"],
        "not_installed"
    );
    let fetched = w.downloads().len();
    let again = json(
        &engine
            .security_install(&w.request(&["dependency-check"], false, &[]))
            .unwrap(),
    );
    assert_eq!(
        outcomes(&again),
        [(
            "dependency-check".to_string(),
            "already_present".to_string()
        )]
    );
    assert_eq!(
        w.downloads()[fetched..].len(),
        1,
        "only the runtime: {:?}",
        w.downloads()
    );
    assert!(w.downloads().last().unwrap().ends_with(".tar.gz"));
    assert_eq!(w.installed(), ["dependency-check", "java"]);

    let removed = json(&engine.security_uninstall(&Selection::All).unwrap());
    assert!(
        outcomes(&removed).contains(&("dependency-check".to_string(), "removed".to_string())),
        "--all covers the opt-in tools: {removed}"
    );
    assert!(
        w.installed().is_empty(),
        "the runtime went with the last tool: {:?}",
        w.installed()
    );
}

/// A Dependency-Check install that fails after the runtime came in keeps
/// nothing it fetched for it.
#[test]
fn a_failed_dependency_check_install_leaves_no_java_runtime_behind() {
    let mut w = World::new();
    let dc = w
        .manifest
        .tools
        .iter_mut()
        .find(|t| t.tool == "dependency-check")
        .unwrap();
    dc.artifacts[0].sha256 = "0".repeat(64);
    let engine = w.engine();
    let report = json(
        &engine
            .security_install(&w.request(&["dependency-check"], true, &[]))
            .unwrap(),
    );
    assert_eq!(report["tools"][0]["outcome"], "failed", "{report}");
    assert_eq!(report["tools"][0]["stage"], "checksum", "{report}");
    assert_eq!(w.downloads().len(), 2, "the runtime was fetched first");
    assert!(w.installed().is_empty(), "{:?}", w.installed());
}

/// A tree archive is checked before anything moves into place: one without
/// the root directory, or whose root lacks the executable, fails that tool at
/// `unpack` and leaves nothing; and every unpack runs with the unpackers'
/// default-option variables emptied, so the environment adds no option.
#[test]
fn a_tree_archive_without_its_root_or_executable_fails_at_unpack_and_leaves_nothing() {
    let mut w = World::new();
    let codeql = w.manifest.tools.iter_mut().find(|t| t.tool == "codeql").unwrap();
    let Layout::Tree { root, .. } = &mut codeql.artifacts[0].layout else {
        panic!("codeql is a tree")
    };
    *root = "codeql-missing".to_string();
    let dc = w.manifest.tools.iter_mut().find(|t| t.tool == "dependency-check").unwrap();
    dc.binary = "bin/missing.sh".to_string();
    let engine = w.engine();
    let report = json(
        &engine
            .security_install(&w.request(&["dependency-check", "codeql"], true, &["codeql"]))
            .unwrap(),
    );
    let reasons: Vec<(&str, &str, &str)> = report["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| {
            (
                t["tool"].as_str().unwrap(),
                t["stage"].as_str().unwrap_or_default(),
                t["reason"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        reasons,
        [
            ("dependency-check", "unpack", "dependency-check.zip holds no regular file dependency-check/bin/missing.sh"),
            ("codeql", "unpack", "codeql.tar.gz holds no directory codeql-missing"),
        ],
        "{report}"
    );
    assert!(w.installed().is_empty(), "no tool, no runtime, no staging: {:?}", w.installed());
    assert!(lines(&w.rec().join("unzip.log")).iter().all(|l| l == "|"), "{:?}", lines(&w.rec().join("unzip.log")));
    let tar = lines(&w.rec().join("tar.log"));
    assert!(!tar.is_empty() && tar.iter().all(|l| l.is_empty()), "{tar:?}");
}

/// A user's own copies stay theirs: a Dependency-Check on `PATH` at the
/// minimum runs on the user's own Java — never with the managed runtime's
/// variables, even when a managed copy and the runtime are installed beside it
/// — and a CodeQL on `PATH` is kept, so no licence is asked for or recorded.
#[test]
fn a_users_own_copies_run_on_their_own_java_and_ask_no_licence() {
    let w = World::new();
    let engine = w.engine();
    engine
        .security_install(&w.request(&["dependency-check"], true, &[]))
        .unwrap();
    assert_eq!(w.installed(), ["dependency-check", "java"]);

    // The user's own copies, put on `PATH` afterwards.
    let (dc, codeql_version) = (w.version("dependency-check"), w.version("codeql"));
    let sarif = r#"{"version":"2.1.0","runs":[{"tool":{"driver":{"name":"dependency-check","rules":[]}},"results":[]}]}"#;
    script(
        &w.bin().join("dependency-check"),
        &format!(
            "#!/bin/sh\n\
             printf '%s|%s\\n' \"$JAVA_HOME\" \"$JAVACMD\" >> '{rec}/own-dc.log'\n\
             case \"$1\" in --version) printf '%s\\n' 'dependency-check-cli version {dc}'; exit 0;; esac\n\
             out=; prev=\n\
             for a in \"$@\"; do [ \"$prev\" = --out ] && out=\"$a\"; prev=\"$a\"; done\n\
             printf '%s' '{sarif}' > \"$out/dependency-check-report.sarif\"\n",
            rec = w.rec().display()
        ),
    );
    script(&w.bin().join("codeql"), &codeql(&codeql_version));

    let row = status_row(&engine, "dependency-check");
    assert_eq!(row["installed"]["copy"], "path", "{row}");
    let scan = json(
        &engine
            .security_scan_selected(
                true,
                &ScanRequest::new(vec!["dependency-check".to_string()], Vec::new()),
                |_| {},
            )
            .unwrap(),
    );
    let run = scan["scanners"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["scanner"] == "dependency-check")
        .unwrap();
    assert_eq!(run["outcome"], "ran", "{scan}");
    let seen = lines(&w.rec().join("own-dc.log"));
    assert!(seen.len() >= 2, "the probe and the scan both ran it: {seen:?}");
    let runtime = w.tools().join("java");
    assert!(
        seen.iter().all(|l| !l.contains(runtime.to_str().unwrap())),
        "the user's own copy never gets the managed runtime: {seen:?}"
    );

    // Beside a missing tool, so the install goes ahead past the kept copy.
    let kept = json(
        &engine
            .security_install(&w.request(&["codeql", "trivy"], true, &[]))
            .unwrap(),
    );
    assert_eq!(outcomes(&kept), [("trivy".to_string(), "installed".to_string())]);
    assert_eq!(kept["kept"][0]["tool"], "codeql", "{kept}");
    assert_eq!(kept["kept"][0]["copy"], "path", "{kept}");
    assert_eq!(kept["licence_recorded"], serde_json::json!([]));
    let record = fs::read_to_string(w.user().join(INSTALL_CONSENT_FILE)).unwrap();
    assert!(!record.contains("licence"), "nothing to accept for a kept copy: {record}");
}

/// A tree whose root, or whose executable, is a symbolic link — pointing
/// outside the tree — is refused at `unpack`: nothing is installed through it.
#[test]
fn a_tree_whose_root_or_executable_is_a_link_is_refused_at_unpack() {
    for (case, root_is_link) in [("root", true), ("executable", false)] {
        let mut w = World::new();
        let outside = w.base.join("outside");
        script(&outside.join("codeql"), &codeql("planted"));
        let src = w.base.join("src").join(format!("linked-{case}"));
        fs::create_dir_all(&src).unwrap();
        if root_is_link {
            std::os::unix::fs::symlink(&outside, src.join("codeql")).unwrap();
        } else {
            fs::create_dir_all(src.join("codeql")).unwrap();
            std::os::unix::fs::symlink(outside.join("codeql"), src.join("codeql").join("codeql")).unwrap();
        }
        let file = w.pack_tar(&src, "codeql", &format!("codeql-linked-{case}"));
        let entry = w.manifest.tools.iter_mut().find(|t| t.tool == "codeql").unwrap();
        let artifact = &mut entry.artifacts[0];
        artifact.url = format!("https://{HOST}/{}", file.file_name().unwrap().to_str().unwrap());
        artifact.sha256 = sha256_file(&file).unwrap();
        artifact.size = fs::metadata(&file).unwrap().len();

        let report = json(
            &w.engine()
                .security_install(&w.request(&["codeql"], true, &["codeql"]))
                .unwrap(),
        );
        let reason = match root_is_link {
            true => format!("codeql-linked-{case}.tar.gz holds no directory codeql"),
            false => format!("codeql-linked-{case}.tar.gz holds no regular file codeql/codeql"),
        };
        assert_eq!(report["tools"][0]["stage"], "unpack", "{case}: {report}");
        assert_eq!(report["tools"][0]["reason"], reason, "{case}: {report}");
        assert!(w.installed().is_empty(), "{case}: {:?}", w.installed());
    }
}
