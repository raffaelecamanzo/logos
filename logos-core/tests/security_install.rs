//! `security install|uninstall|scan --install` through the [`Engine`] (S-744,
//! [FR-SC-22], [FR-SC-05], [FR-SC-08]): the per-user install consent, the
//! precedence that lets a user's own copy win, offline installs, updates, and
//! install-then-scan.
//!
//! No test reaches the internet ([ADR-88]'s fitness function). Each test
//! builds its own artifacts — shell scripts named after the scanners that print
//! a version and write an empty SARIF report — packs them as the builtin
//! manifest's layouts, and hands the engine a manifest pinning them at
//! `https://fixtures.invalid/<version>/<file>`. `curl` is a stub on the test
//! search path that serves those files and records each URL, or one that fails
//! as an unreachable network does. The CLI binary's side of the same flow —
//! exit 2 on a refusal, the user's own copy surviving `uninstall --all` — is
//! `cli/tests/security_surface.rs`.
//!
//! [FR-SC-22]: ../../docs/specs/requirements/FR-SC-22.md
//! [FR-SC-05]: ../../docs/specs/requirements/FR-SC-05.md
//! [FR-SC-08]: ../../docs/specs/requirements/FR-SC-08.md
//! [ADR-88]: ../../docs/specs/architecture/decisions/ADR-88.md
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use logos_core::config::{ConfigError, INSTALL_CONSENT_FILE};
use logos_core::security::{scanner, ScanEnvironment, Selection};
use logos_core::tools::surface::InstallRequest;
use logos_core::tools::{sha256_file, Layout, Manifest, Platform};
use logos_core::Engine;
use serde_json::{json, Value};
use tempfile::TempDir;

/// Where every fixture artifact claims to come from.
const HOST: &str = "fixtures.invalid";

/// One machine: a project, a tools directory, a user-level directory, a search
/// path holding the stub `curl` (and `tar`, `gzip`), and a "server" directory
/// of artifacts.
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
        fs::write(w.root().join("main.py"), "def main():\n    return 1\n").unwrap();
        // A Go file too, so gosec has something to scan (S-769).
        fs::write(w.root().join("main.go"), "package main\n\nfunc main() {}\n").unwrap();
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

    /// The engine over this machine, resolving and installing from `manifest`.
    fn engine(&self, manifest: &Manifest) -> Engine {
        self.engine_with_tools(manifest, self.tools())
    }

    /// [`engine`](Self::engine) with the tools directory at `tools`.
    fn engine_with_tools(&self, manifest: &Manifest, tools: PathBuf) -> Engine {
        Engine::start(self.root())
            .expect("engine")
            .with_scan_environment(ScanEnvironment {
                user_dir: Some(self.user()),
                search_path: Some(self.bin().into_os_string()),
                tools_dir: Some(tools),
            })
            .with_tools_manifest(manifest.clone())
    }

    /// A `curl` that serves `https://fixtures.invalid/<rel>` from the server
    /// directory, recording each URL.
    fn curl_serves(&self) {
        self.curl(&format!(
            "rel=\"${{url#https://{HOST}/}}\"\n/bin/cp '{}/'\"$rel\" \"$out\" || exit 22",
            self.server().display()
        ));
    }

    /// A `curl` that fails as an unreachable network does, recording each URL.
    fn curl_offline(&self) {
        self.curl("printf 'curl: (7) Failed to connect\\n' >&2\nexit 7");
    }

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

    /// Put a user's own copy of `tool` on the search path, printing `version`.
    fn own_copy(&self, tool: &str, version: &str) -> PathBuf {
        let path = self.bin().join(tool);
        script(&path, &scanner_script(tool, version));
        path
    }

    /// The builtin manifest's release-binary tools pinned at `pins` (default:
    /// its own version), each artifact for this platform built here and
    /// served from `server/<version>/`. The Python tools, installed through
    /// `uv`, are `tests/tools_python.rs`'s.
    fn manifest(&self, pins: &[(&str, &str)]) -> Manifest {
        let platform = Platform::current().expect("the suite runs where managed installs exist");
        let mut manifest = Manifest::builtin().clone();
        manifest.tools.retain(|t| t.python.is_none() && t.is_default());
        for entry in &mut manifest.tools {
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
                    let status = Command::new("tar")
                        .arg("-czf")
                        .arg(&file)
                        .arg("-C")
                        .arg(&src)
                        .arg(member)
                        .status()
                        .unwrap();
                    assert!(status.success(), "tar packs {}", entry.tool);
                    file
                }
                Layout::Tree { .. } => unreachable!("the default set holds no tree archive"),
            };
            artifact.url = format!(
                "https://{HOST}/{}/{}",
                entry.version,
                file.file_name().unwrap().to_str().unwrap()
            );
            artifact.sha256 = sha256_file(&file).unwrap();
            artifact.size = fs::metadata(&file).unwrap().len();
            entry.artifacts = vec![artifact];
        }
        manifest
    }

    fn managed(&self, tool: &str, version: &str) -> PathBuf {
        self.tools().join(tool).join(version).join(tool)
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
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|d| d.join(program))
        .find(|p| p.is_file())
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

fn request(tools: Selection, consent: bool) -> InstallRequest {
    InstallRequest {
        tools,
        update: false,
        from: None,
        consent,
        accept_licences: Vec::new(),
    }
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

/// `(tool, outcome)` per installed or attempted tool.
fn outcomes(payload: &Value) -> Vec<(String, String)> {
    payload["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| (t["tool"].as_str().unwrap().to_string(), t["outcome"].as_str().unwrap().to_string()))
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

/// Refuse → exit 2 naming tools, versions, hosts and sizes, nothing fetched →
/// consent → only the missing tools installed; the user's own Trivy at the
/// minimum version used, never downloaded, reported `path`, and surviving
/// `uninstall --all` ([FR-SC-22]'s acceptance criteria 1–3).
#[test]
fn consent_then_only_the_missing_tools_install_and_a_users_own_copy_wins_and_survives_uninstall() {
    let w = World::new();
    let manifest = w.manifest(&[]);
    let engine = w.engine(&manifest);
    let trivy_min = &manifest.tool("trivy").unwrap().min_version;
    let own = w.own_copy("trivy", trivy_min);

    let refused = refusal(engine.security_install(&request(Selection::All, false)));
    for tool in ["gitleaks", "osv-scanner", "gosec"] {
        let entry = manifest.tool(tool).unwrap();
        let mb = format!("({:.1} MB)", entry.artifacts[0].size as f64 / 1_000_000.0);
        let line = format!("{tool} {} from {HOST} {mb}", entry.version);
        assert!(refused.contains(&line), "names `{line}`: {refused}");
    }
    assert!(!refused.contains("trivy"), "the user's own trivy is not asked for: {refused}");
    assert!(refused.contains("logos security install --all --consent"), "{refused}");
    assert!(w.downloads().is_empty(), "a refusal downloads nothing");
    assert!(!w.tools().exists(), "a refusal writes no tool");
    assert!(!w.user().join(INSTALL_CONSENT_FILE).exists(), "a refusal records nothing");

    let installed = json(&engine.security_install(&request(Selection::All, true)).unwrap());
    assert_eq!(
        installed["consent_recorded"],
        json!(["gitleaks 8.18.4", "osv-scanner 1.9.2", "gosec 2.21.4"])
    );
    assert_eq!(
        outcomes(&installed),
        [("gitleaks", "installed"), ("osv-scanner", "installed"), ("gosec", "installed")]
            .map(|(t, o)| (t.to_string(), o.to_string()))
    );
    let kept = &installed["kept"][0];
    assert_eq!((kept["tool"].as_str(), kept["copy"].as_str()), (Some("trivy"), Some("path")), "{installed}");
    assert_eq!(kept["version"].as_str(), Some(trivy_min.as_str()));
    let downloads = w.downloads();
    assert_eq!(downloads.len(), 3, "{downloads:?}");
    assert!(downloads.iter().all(|u| !u.contains("trivy")), "trivy is never downloaded: {downloads:?}");

    let trivy = status_row(&engine, "trivy");
    assert_eq!(trivy["installed"]["copy"], "path");
    assert_eq!(trivy["installed"]["binary"], json!(own));
    assert_eq!(trivy["install"], Value::Null);
    let gosec = status_row(&engine, "gosec");
    assert_eq!(
        (gosec["installed"]["copy"].as_str(), gosec["installed"]["version"].as_str()),
        (Some("managed"), Some("2.21.4"))
    );
    assert_eq!(gosec["installed"]["update_available"], Value::Null);

    // Consented and present: a second install asks nothing and fetches nothing.
    let again = json(&engine.security_install(&request(Selection::All, false)).unwrap());
    assert_eq!((again["consent_recorded"].clone(), again["tools"].clone()), (json!([]), json!([])));
    assert_eq!(again["kept"].as_array().unwrap().len(), 4);
    assert_eq!(w.downloads().len(), 3);

    let removed = json(&engine.security_uninstall(&Selection::All).unwrap());
    assert_eq!(
        outcomes(&removed),
        [("trivy", "not_installed"), ("gitleaks", "removed"), ("osv-scanner", "removed"), ("gosec", "removed")]
            .map(|(t, o)| (t.to_string(), o.to_string()))
    );
    assert_eq!(removed["kept"].as_array().unwrap().len(), 1, "{removed}");
    assert_eq!(removed["kept"][0]["copy"], "path");
    assert!(own.is_file(), "uninstall --all never touches the user's own copy");
    assert_eq!(status_row(&engine, "trivy")["installed"]["copy"], "path");
    assert_eq!(status_row(&engine, "gosec")["install"], "logos security install gosec");
    for tool in ["gitleaks", "osv-scanner", "gosec"] {
        assert!(!w.tools().join(tool).exists(), "{tool}'s managed copy is gone");
    }

    // The consent was recorded per user and version: reinstalling asks nothing.
    let reinstalled = json(&engine.security_install(&request(Selection::Only(vec!["gosec".into()]), false)).unwrap());
    assert_eq!(reinstalled["consent_recorded"], json!([]));
    assert_eq!(outcomes(&reinstalled), [("gosec".to_string(), "installed".to_string())]);
}

/// A `PATH` copy below the minimum is installed over and loses to the managed
/// copy ([FR-SC-05]: it does not win over a managed copy); without one it is
/// still used.
#[test]
fn a_path_copy_below_the_minimum_is_replaced_by_a_managed_install() {
    let w = World::new();
    let manifest = w.manifest(&[]);
    let engine = w.engine(&manifest);
    let old = w.own_copy("trivy", "0.60.0");

    let before = status_row(&engine, "trivy");
    assert_eq!(
        (before["installed"]["copy"].as_str(), before["installed"]["version"].as_str()),
        (Some("path"), Some("0.60.0")),
        "with no managed copy, the user's older copy still runs"
    );
    assert_eq!(before["install"], "logos security install trivy", "below the minimum, an install is offered");
    let refused = refusal(engine.security_install(&request(Selection::Only(vec!["trivy".into()]), false)));
    assert!(refused.contains("trivy 0.74.0") && refused.contains("logos security install trivy --consent"), "{refused}");

    let installed = json(&engine.security_install(&request(Selection::Only(vec!["trivy".into()]), true)).unwrap());
    assert_eq!(outcomes(&installed), [("trivy".to_string(), "installed".to_string())]);
    let after = status_row(&engine, "trivy");
    assert_eq!(after["installed"]["copy"], "managed");
    assert_eq!(after["install"], Value::Null);
    assert_eq!(after["installed"]["binary"], json!(w.managed("trivy", "0.74.0")));
    assert!(old.is_file(), "the user's copy is never removed");
}

/// `--from <dir>` installs with the network unreachable, through the same
/// verification: a tampered artifact in the directory fails the checksum and
/// nothing from it is installed ([FR-SC-22] AC 4, [NFR-SE-10]).
///
/// [NFR-SE-10]: ../../docs/specs/requirements/NFR-SE-10.md
#[test]
fn install_from_a_directory_needs_no_network_and_verifies_each_artifact() {
    let w = World::new();
    let manifest = w.manifest(&[]);
    let engine = w.engine(&manifest);
    w.curl_offline();
    let offline = w.base.join("offline");
    fs::create_dir_all(&offline).unwrap();
    for entry in &manifest.tools {
        let name = entry.artifacts[0].url.rsplit('/').next().unwrap();
        let from = w.server().join(&entry.version).join(name);
        fs::copy(&from, offline.join(name)).unwrap();
    }
    // gosec's artifact is swapped for trivy's: a valid archive, the wrong hash.
    let gosec_name = manifest.tool("gosec").unwrap().artifacts[0].url.rsplit('/').next().unwrap().to_string();
    fs::copy(offline.join("trivy.tar.gz"), offline.join(&gosec_name)).unwrap();

    let from = InstallRequest { from: Some(offline.clone()), ..request(Selection::All, false) };
    let refused = refusal(engine.security_install(&from));
    assert!(refused.contains(&format!("trivy 0.74.0 from {}", offline.display())), "{refused}");
    assert!(refused.contains(&format!("--from {} --consent", offline.display())), "{refused}");

    let installed = json(&engine.security_install(&InstallRequest { consent: true, ..from }).unwrap());
    assert_eq!(
        outcomes(&installed),
        [("trivy", "installed"), ("gitleaks", "installed"), ("osv-scanner", "installed"), ("gosec", "failed")]
            .map(|(t, o)| (t.to_string(), o.to_string()))
    );
    let gosec = &installed["tools"][3];
    assert_eq!(gosec["stage"], "checksum", "{gosec}");
    assert!(!w.tools().join("gosec").exists(), "nothing from the tampered artifact is installed");
    assert!(w.downloads().is_empty(), "no download was attempted: {:?}", w.downloads());
    assert_eq!(status_row(&engine, "trivy")["installed"]["copy"], "managed");
}

/// A manifest pinning a newer version: `status` shows `update_available`, the
/// older managed copy keeps running, a plain install leaves it, and
/// `install --update` asks consent for the new version, then replaces the copy
/// and removes the old one ([FR-SC-22] AC 5).
#[test]
fn a_newer_pin_shows_update_available_and_install_update_replaces_the_managed_copy_after_consent() {
    let w = World::new();
    let v1 = w.manifest(&[]);
    w.engine(&v1).security_install(&request(Selection::Only(vec!["trivy".into()]), true)).unwrap();
    let v2 = w.manifest(&[("trivy", "0.75.0")]);
    let engine = w.engine(&v2);

    let row = status_row(&engine, "trivy");
    assert_eq!(
        (row["installed"]["copy"].as_str(), row["installed"]["version"].as_str()),
        (Some("managed"), Some("0.74.0")),
        "the older managed copy still runs: {row}"
    );
    assert_eq!(row["installed"]["update_available"], "0.75.0");

    let plain = json(&engine.security_install(&request(Selection::Only(vec!["trivy".into()]), false)).unwrap());
    assert_eq!(plain["tools"], json!([]), "nothing updates unless asked: {plain}");
    assert_eq!(w.downloads().len(), 1);

    let update = InstallRequest { update: true, ..request(Selection::Only(vec!["trivy".into()]), false) };
    let refused = refusal(engine.security_install(&update));
    assert!(refused.contains("trivy 0.75.0 from fixtures.invalid"), "the new version is consented to anew: {refused}");
    assert!(refused.contains("logos security install trivy --update --consent"), "{refused}");
    assert_eq!(w.downloads().len(), 1, "a refusal downloads nothing");

    let updated = json(&engine.security_install(&InstallRequest { consent: true, ..update.clone() }).unwrap());
    assert_eq!(updated["consent_recorded"], json!(["trivy 0.75.0"]));
    assert_eq!(outcomes(&updated), [("trivy".to_string(), "installed".to_string())]);
    assert!(w.managed("trivy", "0.75.0").is_file());
    assert!(!w.tools().join("trivy").join("0.74.0").exists(), "the replaced copy is removed");
    let row = status_row(&engine, "trivy");
    assert_eq!(
        (row["installed"]["version"].as_str(), row["installed"]["update_available"].clone()),
        (Some("0.75.0"), Value::Null)
    );

    // At the pin, `--update` has nothing to do: the copy is kept, unfetched.
    let downloads = w.downloads().len();
    let again = json(&engine.security_install(&update).unwrap());
    assert_eq!(again["tools"], json!([]), "{again}");
    assert_eq!((again["kept"][0]["copy"].as_str(), again["kept"][0]["version"].as_str()), (Some("managed"), Some("0.75.0")));
    assert_eq!(w.downloads().len(), downloads);
}

/// `scan --install --consent` installs the missing default scanners and scans
/// in one call; without consent it is refused before anything is downloaded
/// or recorded ([FR-SC-22] AC 6).
#[test]
fn scan_install_installs_the_missing_scanners_then_scans_them() {
    let w = World::new();
    let manifest = w.manifest(&[]);
    let engine = w.engine(&manifest);
    // A project that disables gosec does not get it installed for its scan.
    fs::create_dir_all(w.root().join(".logos")).unwrap();
    fs::write(
        w.root().join(".logos/config.toml"),
        "[security_scan.gosec]\nenabled = false\n",
    )
    .unwrap();

    let refused = refusal(engine.security_scan_install(false));
    assert!(refused.contains("logos security scan --install --consent"), "{refused}");
    assert!(!refused.contains("gosec"), "a disabled scanner is not asked for: {refused}");
    assert!(w.downloads().is_empty());
    assert!(fs::read_dir(w.user()).unwrap().next().is_none(), "nothing is recorded");

    let done = json(&engine.security_scan_install(true).unwrap());
    assert_eq!(
        outcomes(&done["install"]),
        [("trivy", "installed"), ("gitleaks", "installed"), ("osv-scanner", "installed")]
            .map(|(t, o)| (t.to_string(), o.to_string()))
    );
    let scan = &done["scan"];
    assert_eq!(scan["consent_recorded"], json!(["trivy", "gitleaks", "osv-scanner"]));
    for tool in ["trivy", "gitleaks", "osv-scanner"] {
        let run = scan["scanners"].as_array().unwrap().iter().find(|r| r["scanner"] == tool).unwrap();
        assert_eq!(run["outcome"], "ran", "{tool}: {run}");
    }
    let gosec = scan["scanners"].as_array().unwrap().iter().find(|r| r["scanner"] == "gosec").unwrap();
    assert_eq!(gosec["outcome"], "disabled");
    assert!(!w.tools().join("gosec").exists());
}

/// A newer managed copy, left by a later release sharing the tools directory,
/// is not an update for an older pin: it keeps running, status offers no
/// `update_available`, and `--update` neither downgrades nor deletes it.
#[test]
fn an_older_pin_never_downgrades_or_removes_a_newer_managed_copy() {
    let w = World::new();
    let newer = w.manifest(&[("trivy", "0.75.0")]);
    w.engine(&newer).security_install(&request(Selection::Only(vec!["trivy".into()]), true)).unwrap();
    let older = w.manifest(&[]);
    let engine = w.engine(&older);

    let row = status_row(&engine, "trivy");
    assert_eq!(
        (row["installed"]["copy"].as_str(), row["installed"]["version"].as_str()),
        (Some("managed"), Some("0.75.0"))
    );
    assert_eq!(row["installed"]["update_available"], Value::Null, "a downgrade is not an update: {row}");

    let update = InstallRequest { update: true, ..request(Selection::Only(vec!["trivy".into()]), true) };
    let updated = json(&engine.security_install(&update).unwrap());
    assert_eq!(updated["tools"], json!([]), "nothing is installed over a newer copy: {updated}");
    assert_eq!(updated["kept"][0]["version"], "0.75.0");
    assert_eq!(w.downloads().len(), 1, "only the newer release's own download");
    assert!(w.managed("trivy", "0.75.0").is_file(), "the newer copy is never removed");
}

/// A tools directory inside the project — named directly, or through a
/// symlink that resolves into it — is refused with exit 2 before anything is
/// fetched or removed: Logos never installs a tool into a repository
/// ([NFR-SE-08]).
///
/// [NFR-SE-08]: ../../docs/specs/requirements/NFR-SE-08.md
#[test]
fn a_tools_directory_inside_the_project_is_refused_before_anything_is_written() {
    let w = World::new();
    let manifest = w.manifest(&[]);
    let inside = w.root().join(".tools");
    fs::create_dir_all(w.root().join("vendor")).unwrap();
    let link = w.base.join("tools-link");
    std::os::unix::fs::symlink(w.root().join("vendor"), &link).unwrap();

    for tools in [inside.clone(), link] {
        let engine = w.engine_with_tools(&manifest, tools.clone());
        for refused in [
            refusal(engine.security_install(&request(Selection::All, true))),
            refusal(engine.security_uninstall(&Selection::All)),
        ] {
            assert!(refused.contains("is inside the project"), "{}: {refused}", tools.display());
        }
    }
    assert!(!inside.exists(), "nothing is created inside the project");
    assert!(fs::read_dir(w.root().join("vendor")).unwrap().next().is_none());
    assert!(w.downloads().is_empty(), "nothing is fetched");
    assert!(!w.user().join(INSTALL_CONSENT_FILE).exists(), "no consent is recorded either");
}

/// A `PATH` copy whose version cannot be read is not shown to meet the
/// minimum ([FR-SC-05]): it runs while there is nothing else, `install` asks
/// to install over it, and the managed copy then wins.
#[test]
fn a_path_copy_whose_version_cannot_be_read_is_installed_over() {
    let w = World::new();
    let manifest = w.manifest(&[]);
    let engine = w.engine(&manifest);
    let silent = w.bin().join("trivy");
    script(&silent, "#!/bin/sh\nexit 0\n");

    let before = status_row(&engine, "trivy");
    assert_eq!(
        (before["installed"]["copy"].as_str(), before["installed"]["version"].clone()),
        (Some("path"), Value::Null)
    );
    let refused = refusal(engine.security_install(&request(Selection::Only(vec!["trivy".into()]), false)));
    assert!(refused.contains("trivy 0.74.0 from fixtures.invalid"), "{refused}");
    engine.security_install(&request(Selection::Only(vec!["trivy".into()]), true)).unwrap();
    assert_eq!(status_row(&engine, "trivy")["installed"]["copy"], "managed");
    assert!(silent.is_file());
}

/// A user-level `path` override that cannot run is still the user's choice:
/// nothing would ever run a managed copy beside it, so `install` keeps it and
/// downloads nothing, and status names no install command for it.
#[test]
fn a_broken_override_is_kept_and_no_install_is_offered_for_it() {
    let w = World::new();
    let manifest = w.manifest(&[]);
    let engine = w.engine(&manifest);
    let missing = w.base.join("nowhere/trivy");
    fs::write(w.user().join("config.toml"), format!("[security_scan.trivy]\npath = \"{}\"\n", missing.display())).unwrap();

    let row = status_row(&engine, "trivy");
    assert_eq!((row["state"].as_str(), row["install"].clone()), (Some("not_installed"), Value::Null), "{row}");
    let installed = json(&engine.security_install(&request(Selection::Only(vec!["trivy".into()]), false)).unwrap());
    assert_eq!(installed["tools"], json!([]), "{installed}");
    assert_eq!((installed["kept"][0]["copy"].as_str(), installed["kept"][0]["binary"].clone()), (Some("override"), json!(missing)));
    assert!(w.downloads().is_empty() && !w.tools().exists(), "nothing is downloaded for a copy that would never run");
}

/// A runnable user-level `path` override wins whatever its version: it is
/// kept as `override` and its tool is never downloaded.
#[test]
fn a_user_override_is_kept_and_never_downloaded() {
    let w = World::new();
    let manifest = w.manifest(&[]);
    let engine = w.engine(&manifest);
    let own = w.base.join("own/trivy");
    fs::create_dir_all(own.parent().unwrap()).unwrap();
    script(&own, &scanner_script("trivy", "0.60.0"));
    fs::write(w.user().join("config.toml"), format!("[security_scan.trivy]\npath = \"{}\"\n", own.display())).unwrap();

    let installed = json(&engine.security_install(&request(Selection::All, true)).unwrap());
    let kept = &installed["kept"][0];
    assert_eq!(
        (kept["tool"].as_str(), kept["copy"].as_str(), kept["version"].as_str()),
        (Some("trivy"), Some("override"), Some("0.60.0")),
        "{installed}"
    );
    assert_eq!(installed["tools"].as_array().unwrap().len(), 3);
    assert!(w.downloads().iter().all(|u| !u.contains("trivy")), "{:?}", w.downloads());
    assert_eq!(status_row(&engine, "trivy")["installed"]["copy"], "override");
}

/// With every scanner already usable, `scan --install` installs nothing, so it
/// needs no tools directory: it scans instead of refusing.
#[test]
fn scan_install_with_nothing_missing_needs_no_tools_directory() {
    let w = World::new();
    let manifest = w.manifest(&[]);
    for entry in &manifest.tools {
        w.own_copy(&entry.tool, &entry.version);
    }
    let engine = Engine::start(w.root())
        .expect("engine")
        .with_scan_environment(ScanEnvironment {
            user_dir: Some(w.user()),
            search_path: Some(w.bin().into_os_string()),
            tools_dir: None,
        })
        .with_tools_manifest(manifest);

    let done = json(&engine.security_scan_install(true).unwrap());
    assert_eq!(done["install"]["tools"], json!([]));
    assert_eq!(done["install"]["kept"].as_array().unwrap().len(), 4, "{done}");
    let ran = done["scan"]["scanners"].as_array().unwrap().iter().filter(|r| r["outcome"] == "ran").count();
    assert_eq!(ran, 4, "{done}");
    assert!(w.downloads().is_empty());
}

/// S-758: an install run where Logos has never run — a repository with no
/// `.logos/` — installs as before and leaves no lock file, or any `.logos/`,
/// behind; in a project that has one, the install holds the project's lock,
/// which is free again once it returns.
#[test]
fn an_install_outside_a_logos_project_writes_nothing_into_it() {
    let w = World::new();
    let manifest = w.manifest(&[]);
    let plain = w.base.join("plain");
    fs::create_dir_all(&plain).unwrap();
    fs::write(plain.join("README"), "not a Logos project\n").unwrap();
    git(&plain, &["init", "-q", "-b", "main"]);
    let engine = Engine::open(&plain)
        .with_scan_environment(ScanEnvironment {
            user_dir: Some(w.user()),
            search_path: Some(w.bin().into_os_string()),
            tools_dir: Some(w.tools()),
        })
        .with_tools_manifest(manifest.clone());

    let done = json(&engine.security_install(&request(Selection::Only(vec!["gitleaks".into()]), true)).unwrap());
    assert_eq!(outcomes(&done), vec![("gitleaks".to_string(), "installed".to_string())], "{done}");
    assert!(!plain.join(".logos").exists(), "the install wrote no .logos/ into a repository Logos never ran in");

    let done = json(&w.engine(&manifest).security_install(&request(Selection::Only(vec!["trivy".into()]), true)).unwrap());
    assert_eq!(outcomes(&done), vec![("trivy".to_string(), "installed".to_string())], "{done}");
    let lock = w.root().join(".logos/security").join(logos_core::security::LOCK_FILE);
    assert!(lock.is_file(), "in a Logos project the install took the project lock");
    assert_eq!(logos_core::jobs::lock::peek(&lock).unwrap(), None, "and released it");
}

/// S-758: the lock file is never written through a symlinked `.logos`. A scan
/// is refused before it takes the lock, naming the link; an install, which
/// writes only to the tools directory, runs unlocked as it did before, since
/// no scan can run there.
#[test]
fn a_symlinked_logos_gets_no_lock_file_written_through_it() {
    let w = World::new();
    let manifest = w.manifest(&[]);
    let linked = w.base.join("linked");
    let elsewhere = w.base.join("elsewhere");
    fs::create_dir_all(&linked).unwrap();
    fs::create_dir_all(&elsewhere).unwrap();
    fs::write(linked.join("main.py"), "def main():\n    return 1\n").unwrap();
    git(&linked, &["init", "-q", "-b", "main"]);
    std::os::unix::fs::symlink(&elsewhere, linked.join(".logos")).unwrap();
    let engine = Engine::open(&linked)
        .with_scan_environment(ScanEnvironment {
            user_dir: Some(w.user()),
            search_path: Some(w.bin().into_os_string()),
            tools_dir: Some(w.tools()),
        })
        .with_tools_manifest(manifest.clone());

    let scan = engine.security_scan(true).expect_err("a scan through a symlinked .logos is refused");
    assert!(scan.to_string().contains("is a symbolic link"), "{scan:#}");
    let done = json(&engine.security_install(&request(Selection::Only(vec!["gitleaks".into()]), true)).unwrap());
    assert_eq!(outcomes(&done), vec![("gitleaks".to_string(), "installed".to_string())], "{done}");
    let written: Vec<_> = fs::read_dir(&elsewhere).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert!(written.is_empty(), "nothing was written through the link: {written:?}");
}
