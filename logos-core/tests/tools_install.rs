//! The tool installer over fixture artifacts (S-743, [FR-SC-21], [NFR-SE-10],
//! [UAT-SC-08]). No test reaches the internet: the manifest is the builtin one
//! with each artifact swapped for a committed fixture under
//! `tests/fixtures/tools/`, and `curl` is a stub on a test search path that
//! copies the fixture its URL names. `tar` is the system one behind a
//! recording wrapper, or a stub that dies or hangs mid-unpack.
//!
//! The fixtures are shell scripts named after the scanners, printing the
//! pinned version. `trivy-tampered.tar.gz` holds a trivy that, if anything
//! ever ran it, would leave `LOGOS-RAN-AN-UNVERIFIED-ARTIFACT` in its working
//! directory; `trivy-truncated.tar.gz` is the first half of `trivy.tar.gz`.
//!
//! [FR-SC-21]: ../../docs/specs/requirements/FR-SC-21.md
//! [NFR-SE-10]: ../../docs/specs/requirements/NFR-SE-10.md
//! [UAT-SC-08]: ../../docs/specs/requirements/UAT-SC-08.md
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use logos_core::security::{CopyKind, Detection, Orchestrator, ScanEnvironment};
use logos_core::tools::{
    FailStage, InstallOutcome, Installer, Layout, Manifest, Platform, Source, UninstallOutcome,
    CURL_NOT_FOUND, STAGING_DIR,
};
use tempfile::TempDir;

/// What the tampered fixture leaves behind if it is ever run.
const SENTINEL: &str = "LOGOS-RAN-AN-UNVERIFIED-ARTIFACT";

/// `(tool, fixture file, its SHA-256, pinned version)`. The hashes were taken
/// with `shasum -a 256` when the fixtures were committed, so the installer's
/// own hash is checked against an independent one.
const FIXTURES: [(&str, &str, &str, &str); 4] = [
    (
        "trivy",
        "trivy.tar.gz",
        "b8aac09a7cd5539c3e64d932e12c7f647193522c7c091ba00be1a9c7a93bb1d3",
        "0.74.0",
    ),
    (
        "gitleaks",
        "gitleaks.tar.gz",
        "be54dfe54065ed86841e312e834bf86169bc56cb5918d4691370542176763605",
        "8.18.4",
    ),
    (
        "osv-scanner",
        "osv-scanner",
        "2e3b29ec02f47474da838f7db433924559c3bb2ce72a3b30c293ddf78069091e",
        "2.7.0",
    ),
    (
        "gosec",
        "gosec.tar.gz",
        "90362facebe767be6fdea61a323ad1ffa7644815324edac748d9a14a7752475a",
        "2.29.0",
    ),
];
const TRIVY_SHA: &str = FIXTURES[0].2;
const TAMPERED_SHA: &str = "b4f0d0ade80581797be2d6ca19e56c01cdf5e3a6a6df2fcb8dd2b3fced6406fe";

/// The environment variable that turns [`crash_child`] into an install.
const CRASH_CHILD_ENV: &str = "LOGOS_TOOLS_CRASH_CHILD";

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tools")
}

/// The builtin manifest's release-binary tools, each artifact for this
/// platform replaced by its fixture; `swap` replaces trivy's file and pinned
/// hash. The Python tools are `tests/tools_python.rs`'s.
fn manifest(swap: Option<(&str, &str)>) -> Manifest {
    let platform = Platform::current().expect("the suite runs on a platform with managed installs");
    let mut manifest = Manifest::builtin().clone();
    manifest.tools.retain(|t| t.python.is_none() && t.is_default());
    for entry in &mut manifest.tools {
        let (_, mut file, mut sha, version) = *FIXTURES.iter().find(|f| f.0 == entry.tool).unwrap();
        assert_eq!(
            entry.version, version,
            "{}: the fixture prints the pinned version",
            entry.tool
        );
        if entry.tool == "trivy" {
            if let Some((f, s)) = swap {
                (file, sha) = (f, s);
            }
        }
        let artifact = entry
            .artifacts
            .iter_mut()
            .find(|a| a.platform == platform)
            .unwrap();
        artifact.url = format!("https://fixtures.invalid/tools/{file}");
        artifact.sha256 = sha.to_string();
        artifact.size = fs::metadata(fixtures().join(file)).unwrap().len();
        if file == "osv-scanner" {
            assert_eq!(artifact.layout, Layout::Binary);
        }
        entry.artifacts = vec![artifact.clone()];
    }
    manifest
}

#[derive(Clone, Copy)]
enum Curl {
    Absent,
    Serves,
    /// Writes an error page and fails as `--fail` does on an HTTP error.
    HttpError,
    /// Writes part of the artifact and is killed by a signal.
    DiesMidway,
    /// Writes part of the artifact, says so, and sleeps.
    HangsMidway,
}

#[derive(Clone, Copy)]
enum Tar {
    /// The system tar, behind a recording wrapper.
    Real,
    /// Writes part of the binary and is killed by a signal.
    DiesMidway,
    /// Writes part of the binary, says so, and sleeps.
    HangsMidway,
}

/// One isolated machine: a tools directory, a search path holding only the
/// stubs, a recording directory, and an empty project to detect from.
struct Rig {
    _tmp: TempDir,
    base: PathBuf,
}

impl Rig {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let rig = Rig { _tmp: tmp, base };
        rig.setup();
        rig
    }

    fn setup(&self) {
        for dir in [
            self.bin(),
            self.rec(),
            self.root(),
            self.user(),
            self.scanners(),
        ] {
            fs::create_dir_all(dir).unwrap();
        }
        // GNU tar runs `gzip` for `-z`; the system one is linked in.
        if let Some(gzip) = on_process_path("gzip") {
            std::os::unix::fs::symlink(gzip, self.bin().join("gzip")).unwrap();
        }
        self.curl(Curl::Serves);
        self.tar(Tar::Real);
    }

    fn bin(&self) -> PathBuf {
        self.base.join("bin")
    }
    fn rec(&self) -> PathBuf {
        self.base.join("rec")
    }
    fn tools(&self) -> PathBuf {
        self.base.join("tools")
    }
    fn root(&self) -> PathBuf {
        self.base.join("project")
    }
    fn user(&self) -> PathBuf {
        self.base.join("user")
    }
    /// An empty search path for detection: no scanner on `PATH`.
    fn scanners(&self) -> PathBuf {
        self.base.join("no-scanners")
    }

    fn installer(&self, manifest: Manifest) -> Installer {
        Installer::new(self.tools())
            .with_manifest(manifest)
            .with_search_path(Some(self.bin().into_os_string()))
    }

    fn curl(&self, mode: Curl) {
        let path = self.bin().join("curl");
        let partial = format!(
            "/usr/bin/head -c 64 '{}/'\"$name\" > \"$out\"",
            fixtures().display()
        );
        let body = match mode {
            Curl::Absent => {
                let _ = fs::remove_file(&path);
                return;
            }
            Curl::Serves => format!(
                "/bin/cp '{}/'\"$name\" \"$out\" || exit 22",
                fixtures().display()
            ),
            Curl::HttpError => "printf '<html>not found</html>' > \"$out\"\nprintf 'curl: (22) The requested URL returned error: 404\\n' >&2\nexit 22".to_string(),
            Curl::DiesMidway => format!("{partial}\nkill -9 $$"),
            Curl::HangsMidway => self.hang("curl", &partial),
        };
        write_script(
            &path,
            &format!(
                "#!/bin/sh\nout=; url=\nwhile [ $# -gt 0 ]; do\n  case \"$1\" in\n    --output) out=\"$2\"; shift 2 ;;\n    *) url=\"$1\"; shift ;;\n  esac\ndone\nprintf '%s\\n' \"$url\" >> '{rec}/curl.log'\nname=\"${{url##*/}}\"\n{body}\n",
                rec = self.rec().display()
            ),
        );
    }

    fn tar(&self, mode: Tar) {
        let partial = "printf 'partial' > \"$dir/trivy\"";
        let body = match mode {
            Tar::Real => format!(
                "exec '{}' \"$@\"",
                on_process_path("tar").expect("a system tar").display()
            ),
            Tar::DiesMidway => format!("{}\n{partial}\nkill -9 $$", Self::ARGS_C),
            Tar::HangsMidway => format!("{}\n{}", Self::ARGS_C, self.hang("tar", partial)),
        };
        write_script(
            &self.bin().join("tar"),
            &format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}/tar.log'\n{body}\n",
                self.rec().display()
            ),
        );
    }

    /// Finds `-C <dir>` among a stub tar's arguments.
    const ARGS_C: &'static str =
        "dir=\nwhile [ $# -gt 0 ]; do case \"$1\" in -C) dir=\"$2\"; shift 2 ;; *) shift ;; esac; done";

    /// Do `partial`, record the pid, signal readiness, then sleep as that pid.
    fn hang(&self, name: &str, partial: &str) -> String {
        let rec = self.rec();
        format!(
            "{partial}\necho $$ > '{pid}'\n: > '{ready}'\nexec /bin/sleep 60",
            pid = rec.join(format!("{name}.pid")).display(),
            ready = rec.join("ready").display(),
        )
    }

    fn log(&self, name: &str) -> Vec<String> {
        fs::read_to_string(self.rec().join(format!("{name}.log")))
            .map(|t| t.lines().map(str::to_string).collect())
            .unwrap_or_default()
    }

    fn detect(&self) -> Vec<(&'static str, Detection)> {
        let env = ScanEnvironment {
            user_dir: Some(self.user()),
            search_path: Some(self.scanners().into_os_string()),
            tools_dir: Some(self.tools()),
            nvd_api_key: None,
        };
        Orchestrator::open_in(&self.root(), &env)
            .unwrap()
            .detect()
            .into_iter()
            .map(|s| (s.scanner, s.detection))
            .collect()
    }

    /// Every file and directory under `dir`, relative, sorted.
    fn tree(&self, dir: &Path) -> Vec<String> {
        let mut out = Vec::new();
        walk(dir, dir, &mut out);
        out.sort();
        out
    }
}

fn walk(base: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let path = entry.unwrap().path();
        out.push(path.strip_prefix(base).unwrap().display().to_string());
        if fs::symlink_metadata(&path).unwrap().is_dir() {
            walk(base, &path, out);
        }
    }
}

fn on_process_path(program: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|d| d.join(program))
        .find(|p| p.is_file())
}

/// Replace `path` with an executable script, by rename, so a script still
/// open elsewhere is never written in place.
fn write_script(path: &Path, body: &str) {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, body).unwrap();
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755)).unwrap();
    fs::rename(&tmp, path).unwrap();
}

fn pinned(rig: &Rig, tool: &str) -> PathBuf {
    let (_, _, _, version) = *FIXTURES.iter().find(|f| f.0 == tool).unwrap();
    rig.tools().join(tool).join(version).join(tool)
}

#[test]
fn the_four_scanners_install_from_fixture_artifacts_and_detection_reports_them_managed() {
    let rig = Rig::new();
    let installer = rig.installer(manifest(None));
    let names = ["trivy", "gitleaks", "osv-scanner", "gosec"];
    let report = installer.install(&names, &Source::Network).unwrap();

    for (tool, (name, _, _, version)) in report.tools.iter().zip(FIXTURES) {
        assert_eq!((tool.tool.as_str(), tool.version.as_str()), (name, version));
        assert_eq!(
            tool.outcome,
            InstallOutcome::Installed {
                binary: pinned(&rig, name)
            },
            "{name}"
        );
        assert_eq!(
            installer.resolve(name).map(|m| m.binary),
            Some(pinned(&rig, name))
        );
        let mode = fs::metadata(pinned(&rig, name))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            mode, 0o755,
            "{name}: executable, writable by its owner only"
        );
    }
    assert_eq!(rig.log("curl").len(), 4, "one download per tool");
    // Each tool directory holds its binary and nothing else; staging is gone.
    assert_eq!(
        rig.tree(&rig.tools()),
        [
            "gitleaks",
            "gitleaks/8.18.4",
            "gitleaks/8.18.4/gitleaks",
            "gosec",
            "gosec/2.29.0",
            "gosec/2.29.0/gosec",
            "osv-scanner",
            "osv-scanner/2.7.0",
            "osv-scanner/2.7.0/osv-scanner",
            "trivy",
            "trivy/0.74.0",
            "trivy/0.74.0/trivy",
        ]
    );

    let detected = rig.detect();
    for (scanner, detection) in &detected {
        match FIXTURES.iter().find(|f| f.0 == *scanner) {
            Some((name, _, _, version)) => assert_eq!(
                *detection,
                Detection::Installed {
                    binary: pinned(&rig, name),
                    version: Some(version.to_string()),
                    copy: CopyKind::Managed,
                },
                "{scanner}"
            ),
            None => assert_eq!(*detection, Detection::NotInstalled, "{scanner}"),
        }
    }
    assert!(
        rig.tree(&rig.root()).is_empty(),
        "nothing is written in the project"
    );

    // A second install finds every pinned copy and fetches nothing.
    let again = installer.install(&names, &Source::Network).unwrap();
    assert!(
        again
            .tools
            .iter()
            .all(|t| matches!(t.outcome, InstallOutcome::AlreadyPresent { .. })),
        "{again:?}"
    );
    assert_eq!(rig.log("curl").len(), 4);
}

/// The fitness test ([NFR-SE-10], [UAT-SC-08] steps 1, 2 and 4).
#[test]
fn a_tampered_or_truncated_artifact_is_deleted_reported_failed_with_the_checksum_reason_and_never_run(
) {
    let rig = Rig::new();
    let cases = [
        ("tampered", "trivy-tampered.tar.gz", Source::Network),
        ("truncated", "trivy-truncated.tar.gz", Source::Network),
        (
            "tampered, offline",
            "trivy-tampered.tar.gz",
            Source::Directory(PathBuf::new()),
        ),
    ];
    for (case, file, source) in cases {
        let source = match source {
            // The offline directory holds the tampered file under trivy's name.
            Source::Directory(_) => {
                let offline = rig.base.join("offline");
                fs::create_dir_all(&offline).unwrap();
                fs::copy(fixtures().join(file), offline.join("trivy.tar.gz")).unwrap();
                Source::Directory(offline)
            }
            network => network,
        };
        let file = if matches!(source, Source::Directory(_)) {
            "trivy.tar.gz"
        } else {
            file
        };
        let report = rig
            .installer(manifest(Some((file, TRIVY_SHA))))
            .install(&["trivy"], &source)
            .unwrap();
        let InstallOutcome::Failed { stage, reason } = &report.tools[0].outcome else {
            panic!("{case}: {report:?}");
        };
        assert_eq!(*stage, FailStage::Checksum, "{case}: {reason}");
        assert!(reason.starts_with("checksum mismatch"), "{case}: {reason}");
        assert!(
            reason.contains(TRIVY_SHA),
            "{case}: names the pinned hash: {reason}"
        );
        // Deleted: nothing at all is left in the tools directory.
        assert!(
            rig.tree(&rig.tools()).is_empty(),
            "{case}: {:?}",
            rig.tree(&rig.tools())
        );
        assert!(rig.log("tar").is_empty(), "{case}: nothing was unpacked");
        let trivy = rig
            .detect()
            .into_iter()
            .find(|(s, _)| *s == "trivy")
            .unwrap()
            .1;
        assert_eq!(trivy, Detection::NotInstalled, "{case}");
        assert!(
            !rig.tree(&rig.base).iter().any(|p| p.ends_with(SENTINEL)),
            "{case}: nothing from the artifact ran"
        );
    }
    assert_eq!(
        rig.log("curl").len(),
        2,
        "the offline case downloads nothing"
    );

    // Control: the sentinel does catch a run. Pinned to its own hash, the
    // tampered trivy installs, and detection's version probe runs it.
    let report = rig
        .installer(manifest(Some(("trivy-tampered.tar.gz", TAMPERED_SHA))))
        .install(&["trivy"], &Source::Network)
        .unwrap();
    assert!(
        matches!(report.tools[0].outcome, InstallOutcome::Installed { .. }),
        "{report:?}"
    );
    rig.detect();
    assert!(
        rig.root().join(SENTINEL).is_file(),
        "the control run leaves the sentinel"
    );
}

#[test]
fn a_download_or_an_unpack_that_dies_leaves_no_partial_tool_and_a_second_run_completes_it() {
    let rig = Rig::new();
    let installer = rig.installer(manifest(None));

    rig.curl(Curl::DiesMidway);
    let report = installer.install(&["trivy"], &Source::Network).unwrap();
    assert!(
        matches!(&report.tools[0].outcome, InstallOutcome::Failed { stage: FailStage::Download, reason } if reason.contains("curl was killed")),
        "{report:?}"
    );
    assert!(
        rig.tree(&rig.tools()).is_empty(),
        "{:?}",
        rig.tree(&rig.tools())
    );

    rig.curl(Curl::Serves);
    rig.tar(Tar::DiesMidway);
    let report = installer.install(&["trivy"], &Source::Network).unwrap();
    assert!(
        matches!(&report.tools[0].outcome, InstallOutcome::Failed { stage: FailStage::Unpack, reason } if reason.contains("tar was killed")),
        "{report:?}"
    );
    assert!(
        rig.tree(&rig.tools()).is_empty(),
        "{:?}",
        rig.tree(&rig.tools())
    );
    assert_eq!(installer.resolve("trivy"), None);

    rig.tar(Tar::Real);
    let report = installer.install(&["trivy"], &Source::Network).unwrap();
    assert_eq!(
        report.tools[0].outcome,
        InstallOutcome::Installed {
            binary: pinned(&rig, "trivy")
        }
    );
    assert!(fs::read_to_string(pinned(&rig, "trivy"))
        .unwrap()
        .contains("Version: 0.74.0"));
}

/// The install the crash and staging tests run in a process of its own: run
/// only as their child process.
#[test]
#[ignore = "the child process of the crash and staging tests below"]
fn crash_child() {
    let Some(base) = std::env::var_os(CRASH_CHILD_ENV) else {
        return;
    };
    let base = PathBuf::from(base);
    let installer = Installer::new(base.join("tools"))
        .with_manifest(manifest(None))
        .with_search_path(Some(base.join("bin").into_os_string()));
    let report = installer.install(&["trivy"], &Source::Network);
    // Reached only if the parent failed to kill this process.
    panic!("the crash child was not killed: {report:?}");
}

/// Kill a whole install process — not just its `curl` or `tar` — at the
/// point the stubs hang, and check what it leaves behind.
fn kill_an_install(rig: &Rig) {
    let mut child = start_an_install(rig);
    child.kill().unwrap();
    child.wait().unwrap();
    // The hanging stub runs in a process group of its own; it goes too.
    for name in ["curl", "tar"] {
        if let Ok(pid) = fs::read_to_string(rig.rec().join(format!("{name}.pid"))) {
            let _ = Command::new("/bin/kill").args(["-9", pid.trim()]).status();
        }
    }
}

/// An install of trivy in a process of its own, returned once it reached the
/// point the stubs hang.
fn start_an_install(rig: &Rig) -> std::process::Child {
    let exe = std::env::current_exe().unwrap();
    let mut child = Command::new(exe)
        .args([
            "--exact",
            "crash_child",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CRASH_CHILD_ENV, &rig.base)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let ready = rig.rec().join("ready");
    let start = Instant::now();
    while !ready.exists() {
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "the install never reached the hang"
        );
        if let Some(status) = child.try_wait().unwrap() {
            panic!("the crash child exited before the hang: {status}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    child
}

fn assert_killed_install_left_no_tool_then_completes(rig: &Rig, residue: &str) {
    let installer = rig.installer(manifest(None));
    let staging = rig.tools().join(STAGING_DIR).join("trivy-0.74.0");
    assert!(
        staging.join(residue).is_file(),
        "the install was killed mid-way: {:?}",
        rig.tree(&rig.tools())
    );
    assert!(
        !rig.tools().join("trivy").exists(),
        "no tool directory: {:?}",
        rig.tree(&rig.tools())
    );
    assert_eq!(installer.resolve("trivy"), None);
    let trivy = rig
        .detect()
        .into_iter()
        .find(|(s, _)| *s == "trivy")
        .unwrap()
        .1;
    assert_eq!(trivy, Detection::NotInstalled);

    rig.curl(Curl::Serves);
    rig.tar(Tar::Real);
    let report = installer.install(&["trivy"], &Source::Network).unwrap();
    assert_eq!(
        report.tools[0].outcome,
        InstallOutcome::Installed {
            binary: pinned(rig, "trivy")
        }
    );
    assert!(fs::read_to_string(pinned(rig, "trivy"))
        .unwrap()
        .contains("Version: 0.74.0"));
    assert_eq!(
        rig.tree(&rig.tools()),
        ["trivy", "trivy/0.74.0", "trivy/0.74.0/trivy"],
        "the earlier staging is discarded"
    );
}

#[test]
fn an_install_killed_mid_download_leaves_no_tool_and_the_next_run_completes_it() {
    let rig = Rig::new();
    rig.curl(Curl::HangsMidway);
    kill_an_install(&rig);
    assert_killed_install_left_no_tool_then_completes(&rig, "trivy.tar.gz");
}

#[test]
fn an_install_killed_mid_unpack_leaves_no_tool_and_the_next_run_completes_it() {
    let rig = Rig::new();
    rig.tar(Tar::HangsMidway);
    kill_an_install(&rig);
    assert_killed_install_left_no_tool_then_completes(&rig, "extract/trivy");
}

/// A verified archive whose binary is a symlink yields no tool: the installed
/// file must be a regular file inside the tools directory.
#[test]
fn an_archive_whose_binary_is_a_symlink_is_refused_at_unpack() {
    let rig = Rig::new();
    let src = rig.base.join("src");
    fs::create_dir_all(&src).unwrap();
    // The link names a file the user owns, so following it would succeed.
    let victim = rig.base.join("victim");
    fs::write(&victim, "#!/bin/sh\n").unwrap();
    fs::set_permissions(&victim, fs::Permissions::from_mode(0o644)).unwrap();
    std::os::unix::fs::symlink(&victim, src.join("trivy")).unwrap();
    let archive = rig.base.join("offline").join("trivy.tar.gz");
    fs::create_dir_all(archive.parent().unwrap()).unwrap();
    let status = Command::new(on_process_path("tar").unwrap())
        .arg("-czf")
        .arg(&archive)
        .arg("-C")
        .arg(&src)
        .arg("trivy")
        .status()
        .unwrap();
    assert!(status.success());
    let sha = logos_core::tools::sha256_file(&archive).unwrap();
    let report = rig
        .installer(manifest(Some(("trivy.tar.gz", &sha))))
        .install(
            &["trivy"],
            &Source::Directory(archive.parent().unwrap().to_path_buf()),
        )
        .unwrap();
    assert!(
        matches!(&report.tools[0].outcome, InstallOutcome::Failed { stage: FailStage::Unpack, reason } if reason.contains("no regular file")),
        "{report:?}"
    );
    assert!(
        rig.tree(&rig.tools()).is_empty(),
        "{:?}",
        rig.tree(&rig.tools())
    );
    let mode = fs::metadata(&victim).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o644, "the link's target was never made executable");
}

#[test]
fn curl_absent_from_the_path_reports_curl_not_found_naming_the_package() {
    let rig = Rig::new();
    rig.curl(Curl::Absent);
    let report = rig
        .installer(manifest(None))
        .install(&["trivy", "osv-scanner"], &Source::Network)
        .unwrap();
    for tool in &report.tools {
        assert_eq!(
            tool.outcome,
            InstallOutcome::Failed {
                stage: FailStage::Download,
                reason: CURL_NOT_FOUND.to_string()
            },
            "{}",
            tool.tool
        );
    }
    assert!(CURL_NOT_FOUND.starts_with("curl not found"));
    assert!(CURL_NOT_FOUND.contains("`curl` package") && CURL_NOT_FOUND.contains("--from <dir>"));
    assert!(
        rig.tree(&rig.tools()).is_empty(),
        "{:?}",
        rig.tree(&rig.tools())
    );
    // A relative search-path entry is never used to find curl, even one that
    // names the stub curl from this process's working directory.
    rig.curl(Curl::Serves);
    let cwd = std::env::current_dir().unwrap().canonicalize().unwrap();
    let up = "../".repeat(cwd.components().count() - 1);
    let relative = PathBuf::from(up).join(rig.bin().strip_prefix("/").unwrap());
    assert!(
        relative.is_relative() && relative.join("curl").is_file(),
        "{}",
        relative.display()
    );
    let report = rig
        .installer(manifest(None))
        .with_search_path(Some(relative.into_os_string()))
        .install(&["trivy"], &Source::Network)
        .unwrap();
    assert!(
        matches!(&report.tools[0].outcome, InstallOutcome::Failed { reason, .. } if reason == CURL_NOT_FOUND),
        "{report:?}"
    );
}

/// An install run for a project never finds `curl` in it, even on an absolute
/// search-path entry (a direnv `bin/`, `node_modules/.bin`).
#[test]
fn an_install_for_a_project_never_runs_a_curl_the_project_holds() {
    let rig = Rig::new();
    let repo_bin = rig.root().join("bin");
    fs::create_dir_all(&repo_bin).unwrap();
    let planted = rig.base.join("planted-curl-ran");
    write_script(
        &repo_bin.join("curl"),
        &format!(
            "#!/bin/sh
: > '{}'
exit 22
",
            planted.display()
        ),
    );
    let search = std::env::join_paths([repo_bin, rig.bin()]).unwrap();
    let report = rig
        .installer(manifest(None))
        .with_search_path(Some(search))
        .for_project(&rig.root())
        .install(&["trivy"], &Source::Network)
        .unwrap();
    assert_eq!(
        report.tools[0].outcome,
        InstallOutcome::Installed {
            binary: pinned(&rig, "trivy")
        }
    );
    assert!(!planted.exists(), "the project's curl never ran");
    assert_eq!(rig.log("curl").len(), 1, "the user's curl served it");
}

/// A symlink in place of a tool's directory or of `.staging` is replaced,
/// never followed: nothing outside the tools directory is deleted or written.
#[test]
fn an_install_never_deletes_or_writes_through_a_symlinked_tool_or_staging_directory() {
    let rig = Rig::new();
    let outside = rig.base.join("outside");
    fs::create_dir_all(outside.join("2.29.0")).unwrap();
    fs::create_dir_all(outside.join("gosec-2.29.0")).unwrap();
    fs::write(outside.join("2.29.0/precious.txt"), "keep").unwrap();
    fs::write(outside.join("gosec-2.29.0/precious.txt"), "keep").unwrap();
    fs::create_dir_all(rig.tools()).unwrap();
    std::os::unix::fs::symlink(&outside, rig.tools().join("gosec")).unwrap();
    std::os::unix::fs::symlink(&outside, rig.tools().join(STAGING_DIR)).unwrap();

    let report = rig
        .installer(manifest(None))
        .install(&["gosec"], &Source::Network)
        .unwrap();
    assert_eq!(
        report.tools[0].outcome,
        InstallOutcome::Installed {
            binary: pinned(&rig, "gosec")
        }
    );
    assert_eq!(
        rig.tree(&outside),
        [
            "2.29.0",
            "2.29.0/precious.txt",
            "gosec-2.29.0",
            "gosec-2.29.0/precious.txt"
        ],
        "nothing outside was touched"
    );
    assert_eq!(
        rig.tree(&rig.tools()),
        ["gosec", "gosec/2.29.0", "gosec/2.29.0/gosec"]
    );
}

/// A symlink in place of a staging lock file — the install's own, or one the
/// sweep reaches — is removed, never followed: no file is created outside the
/// tools directory, and the install goes ahead.
#[test]
fn an_install_never_creates_a_file_through_a_symlinked_lock_file() {
    let rig = Rig::new();
    let outside = rig.base.join("outside");
    fs::create_dir_all(&outside).unwrap();
    let staging = rig.tools().join(STAGING_DIR);
    fs::create_dir_all(&staging).unwrap();
    std::os::unix::fs::symlink(outside.join("own"), staging.join("trivy-0.74.0.lock")).unwrap();
    std::os::unix::fs::symlink(outside.join("swept"), staging.join("gosec-2.29.0.lock")).unwrap();

    let report = rig
        .installer(manifest(None))
        .install(&["trivy"], &Source::Network)
        .unwrap();
    assert_eq!(
        report.tools[0].outcome,
        InstallOutcome::Installed {
            binary: pinned(&rig, "trivy")
        }
    );
    assert!(rig.tree(&outside).is_empty(), "nothing was created outside: {:?}", rig.tree(&outside));
    assert_eq!(
        rig.tree(&rig.tools()),
        ["trivy", "trivy/0.74.0", "trivy/0.74.0/trivy"]
    );
}

/// An offline artifact that is not a regular file fails at once: a FIFO is
/// never opened, so the install cannot block on it.
#[test]
fn an_offline_artifact_that_is_not_a_regular_file_fails_without_blocking() {
    let rig = Rig::new();
    let offline = rig.base.join("offline");
    fs::create_dir_all(&offline).unwrap();
    let status = Command::new("/usr/bin/mkfifo")
        .arg(offline.join("trivy.tar.gz"))
        .status()
        .unwrap();
    assert!(status.success());
    let report = rig
        .installer(manifest(None))
        .install(&["trivy"], &Source::Directory(offline))
        .unwrap();
    assert!(
        matches!(&report.tools[0].outcome, InstallOutcome::Failed { stage: FailStage::Download, reason } if reason.ends_with("is not a regular file")),
        "{report:?}"
    );
    assert!(
        rig.tree(&rig.tools()).is_empty(),
        "{:?}",
        rig.tree(&rig.tools())
    );
}

/// What interrupted installs left in staging — the tool's own at an earlier
/// version, another tool's, a lock file with no entry — is discarded by the
/// next install of any tool, as no running install holds it (S-759).
#[test]
fn the_next_install_discards_the_staging_no_running_install_holds() {
    let rig = Rig::new();
    let staging = rig.tools().join(STAGING_DIR);
    for leftover in ["trivy-0.73.0", "gosec-2.29.0"] {
        fs::create_dir_all(staging.join(leftover)).unwrap();
        fs::write(staging.join(leftover).join("partial"), "x").unwrap();
    }
    fs::write(staging.join("gosec-2.29.0.lock"), "").unwrap();
    fs::write(staging.join("osv-scanner-2.7.0.lock"), "").unwrap();
    rig.installer(manifest(None))
        .install(&["trivy"], &Source::Network)
        .unwrap();
    assert_eq!(
        rig.tree(&rig.tools()),
        ["trivy", "trivy/0.74.0", "trivy/0.74.0/trivy"]
    );
}

/// After a process is killed mid-download — as a `serve` stopped by SIGKILL
/// is — the next install, of another tool, clears the staging it left: the
/// killed process's lock went with it (S-759).
#[test]
fn an_install_of_another_tool_clears_the_staging_a_killed_install_left() {
    let rig = Rig::new();
    rig.curl(Curl::HangsMidway);
    kill_an_install(&rig);
    let left = rig.tools().join(STAGING_DIR).join("trivy-0.74.0");
    assert!(
        left.join("trivy.tar.gz").is_file(),
        "the killed install left its download: {:?}",
        rig.tree(&rig.tools())
    );

    rig.curl(Curl::Serves);
    let report = rig
        .installer(manifest(None))
        .install(&["gosec"], &Source::Network)
        .unwrap();
    assert_eq!(
        report.tools[0].outcome,
        InstallOutcome::Installed {
            binary: pinned(&rig, "gosec")
        }
    );
    assert_eq!(
        rig.tree(&rig.tools()),
        ["gosec", "gosec/2.29.0", "gosec/2.29.0/gosec"],
        "the killed install's staging is gone"
    );
}

/// The staging of an install still running in another process is left
/// alone, and an install of the same tool there fails that tool rather than
/// write over it.
#[test]
fn the_staging_of_an_install_running_in_another_process_is_left_alone() {
    let rig = Rig::new();
    rig.curl(Curl::HangsMidway);
    let mut running = start_an_install(&rig);
    rig.curl(Curl::Serves);
    let installer = rig.installer(manifest(None));

    let report = installer.install(&["gosec", "trivy"], &Source::Network).unwrap();
    let live = rig.tools().join(STAGING_DIR).join("trivy-0.74.0");
    let still_there = live.join("trivy.tar.gz").is_file();
    running.kill().unwrap();
    running.wait().unwrap();
    if let Ok(pid) = fs::read_to_string(rig.rec().join("curl.pid")) {
        let _ = Command::new("/bin/kill").args(["-9", pid.trim()]).status();
    }

    assert_eq!(
        report.tools[0].outcome,
        InstallOutcome::Installed {
            binary: pinned(&rig, "gosec")
        }
    );
    assert_eq!(
        report.tools[1].outcome,
        InstallOutcome::Failed {
            stage: FailStage::Install,
            reason: "another Logos process is using the staging of trivy 0.74.0: try again once it ends".to_string()
        }
    );
    assert!(still_there, "the running install's download was left alone");
}

/// A managed copy that cannot be removed fails that tool alone, with the
/// reason; the others are still removed.
#[test]
fn an_uninstall_that_cannot_remove_a_copy_fails_that_tool_and_continues() {
    let rig = Rig::new();
    let installer = rig.installer(manifest(None));
    installer
        .install(&["trivy", "gosec"], &Source::Network)
        .unwrap();
    let locked = rig.tools().join("trivy");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();
    let root_bypasses_modes = {
        use std::os::unix::fs::MetadataExt;
        fs::metadata(&rig.base).unwrap().uid() == 0
    };
    let report = installer.uninstall(&["trivy", "gosec"]).unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    if root_bypasses_modes {
        return;
    }
    assert!(
        matches!(&report[0].outcome, UninstallOutcome::Failed { reason } if reason.starts_with("could not remove") && reason.contains("trivy")),
        "{report:?}"
    );
    assert_eq!(report[1].outcome, UninstallOutcome::Removed);
    assert_eq!(installer.resolve("gosec"), None);
}

/// A non-runnable file at the pinned path is no managed copy: it is not
/// resolved, not detected, and the next install replaces it.
#[test]
fn a_pinned_path_holding_a_non_runnable_file_is_replaced_by_the_next_install() {
    let rig = Rig::new();
    let junk = pinned(&rig, "trivy");
    fs::create_dir_all(junk.parent().unwrap()).unwrap();
    fs::write(&junk, "#!/bin/sh\necho junk\n").unwrap();
    fs::set_permissions(&junk, fs::Permissions::from_mode(0o644)).unwrap();
    let installer = rig.installer(manifest(None));
    assert_eq!(installer.resolve("trivy"), None);
    let report = installer.install(&["trivy"], &Source::Network).unwrap();
    assert_eq!(
        report.tools[0].outcome,
        InstallOutcome::Installed {
            binary: pinned(&rig, "trivy")
        }
    );
    assert!(fs::read_to_string(&junk)
        .unwrap()
        .contains("Version: 0.74.0"));
    assert_eq!(
        rig.log("curl").len(),
        1,
        "it was fetched, not taken as present"
    );
}

/// A dangling symlink at the pinned path is replaced by an install, and one in
/// place of a tool's directory is removed by an uninstall — the link itself,
/// never what it names.
#[test]
fn dangling_symlinks_in_the_tools_directory_are_replaced_and_removed() {
    let rig = Rig::new();
    let dest = pinned(&rig, "trivy");
    fs::create_dir_all(dest.parent().unwrap().parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(rig.base.join("nowhere"), dest.parent().unwrap()).unwrap();
    let installer = rig.installer(manifest(None));
    let report = installer.install(&["trivy"], &Source::Network).unwrap();
    assert_eq!(
        report.tools[0].outcome,
        InstallOutcome::Installed { binary: dest }
    );

    std::os::unix::fs::symlink(rig.base.join("nowhere"), rig.tools().join("gosec")).unwrap();
    let report = installer.uninstall(&["gosec"]).unwrap();
    assert_eq!(report[0].outcome, UninstallOutcome::Removed);
    assert!(fs::symlink_metadata(rig.tools().join("gosec")).is_err());
}

/// An HTTP error is a download failure carrying curl's exit code and stderr;
/// the error page it wrote is never hashed as the artifact.
#[test]
fn an_http_error_fails_the_download_with_curls_exit_code_and_stderr() {
    let rig = Rig::new();
    rig.curl(Curl::HttpError);
    let report = rig
        .installer(manifest(None))
        .install(&["trivy"], &Source::Network)
        .unwrap();
    assert_eq!(
        report.tools[0].outcome,
        InstallOutcome::Failed {
            stage: FailStage::Download,
            reason: "curl exited with 22: curl: (22) The requested URL returned error: 404"
                .to_string()
        }
    );
    assert!(
        rig.tree(&rig.tools()).is_empty(),
        "{:?}",
        rig.tree(&rig.tools())
    );
}

/// An offline directory without the artifact fails the download; nothing is
/// fetched from the network instead.
#[test]
fn an_offline_directory_without_the_artifact_fails_the_download() {
    let rig = Rig::new();
    let offline = rig.base.join("offline");
    fs::create_dir_all(&offline).unwrap();
    let report = rig
        .installer(manifest(None))
        .install(&["trivy"], &Source::Directory(offline.clone()))
        .unwrap();
    assert!(
        matches!(&report.tools[0].outcome, InstallOutcome::Failed { stage: FailStage::Download, reason } if reason.contains(&offline.join("trivy.tar.gz").display().to_string())),
        "{report:?}"
    );
    assert!(rig.log("curl").is_empty());
    assert!(
        rig.tree(&rig.tools()).is_empty(),
        "{:?}",
        rig.tree(&rig.tools())
    );
}

/// `tar` comes only from the search path, as `curl` does: without one there,
/// a verified archive fails at unpack and nothing is installed.
#[test]
fn tar_absent_from_the_path_fails_the_unpack() {
    let rig = Rig::new();
    let empty = rig.base.join("empty-bin");
    fs::create_dir_all(&empty).unwrap();
    let report = rig
        .installer(manifest(None))
        .with_search_path(Some(empty.into_os_string()))
        .install(&["trivy"], &Source::Directory(fixtures()))
        .unwrap();
    assert_eq!(
        report.tools[0].outcome,
        InstallOutcome::Failed {
            stage: FailStage::Unpack,
            reason: "tar not found on PATH".to_string()
        }
    );
    assert!(
        rig.tree(&rig.tools()).is_empty(),
        "{:?}",
        rig.tree(&rig.tools())
    );
}

#[test]
fn uninstall_removes_the_managed_copy_and_never_a_users_own() {
    let rig = Rig::new();
    let installer = rig.installer(manifest(None));
    installer.install(&["trivy"], &Source::Network).unwrap();
    let own = rig.scanners().join("trivy");
    write_script(&own, "#!/bin/sh\necho 'Version: 0.80.0'\n");

    let removed = installer.uninstall(&["trivy", "gosec"]).unwrap();
    assert_eq!(
        removed
            .iter()
            .map(|r| (r.tool.as_str(), r.outcome.clone()))
            .collect::<Vec<_>>(),
        [
            ("trivy", UninstallOutcome::Removed),
            ("gosec", UninstallOutcome::NotInstalled)
        ]
    );
    assert_eq!(installer.resolve("trivy"), None);
    assert!(rig.tree(&rig.tools()).is_empty());
    assert!(own.is_file(), "the user's own copy is untouched");
    let trivy = rig
        .detect()
        .into_iter()
        .find(|(s, _)| *s == "trivy")
        .unwrap()
        .1;
    assert!(
        matches!(
            trivy,
            Detection::Installed {
                copy: CopyKind::Path,
                ..
            }
        ),
        "{trivy:?}"
    );
}
