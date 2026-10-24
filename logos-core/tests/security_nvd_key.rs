//! Dependency-Check's NVD API key never reaches a log line, a payload or a
//! stored row (FR-SC-27, S-794), through the engine every surface calls.
//!
//! A test binary of its own, holding one test: the scanner run event's
//! callsite interest is cached process-wide, so a capturing subscriber is
//! reliable only when no other test in the binary emits the event first.
//!
//! No real scanner runs: Dependency-Check is a stub that echoes the file it is
//! handed on both streams — the worst a scanner could do with the key. The
//! project is a git repository in a temporary directory outside every work
//! tree, and the key is passed in the [`ScanEnvironment`], so the process
//! environment is never changed.
#![cfg(unix)]

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use logos_core::security::{FindingsFilter, NvdApiKey, ScanEnvironment, NVD_KEY_FILE};
use logos_core::Engine;

/// The planted key.
const NVD_KEY: &str = "LOGOSPLANTEDNVDKEY-0123-4567-89ab-cdef01234567";

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn git(root: &Path, args: &[&str]) {
    let out = Command::new("git")
        .current_dir(root)
        .env("GIT_CEILING_DIRECTORIES", root.parent().unwrap())
        .args(["-c", "maintenance.auto=false", "-c", "gc.auto=0", "-c", "user.email=t@logos", "-c", "user.name=t"])
        .args(args)
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

/// Every file under `dir`, recursively, that holds `needle`.
fn files_holding(dir: &Path, needle: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let meta = fs::symlink_metadata(&path).unwrap();
            if meta.is_dir() {
                pending.push(path);
            } else if meta.is_file() && fs::read(&path).unwrap().windows(needle.len()).any(|w| w == needle.as_bytes()) {
                found.push(path);
            }
        }
    }
    found
}

#[test]
fn a_planted_nvd_key_reaches_no_log_line_payload_or_stored_row() {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let [root, bin, rec, user] = ["project", "bin", "rec", "user"].map(|d| base.join(d));
    for dir in [&root, &bin, &rec, &user] {
        fs::create_dir_all(dir).unwrap();
    }
    fs::write(root.join("requirements.txt"), "Django==2.2.0\n").unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "seed"]);
    let report = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sarif/dependency-check.sarif");
    let stub = |exit: i32| {
        let write = if exit == 0 {
            format!("/bin/mkdir -p \"$out\" && /bin/cp '{}' \"$out/dependency-check-report.sarif\"", report.display())
        } else {
            String::new()
        };
        let script = format!(
            "#!/bin/sh\n\
             printf '%s\\n' \"$*\" >> '{argv}'\n\
             case \"$1\" in --version) printf '%s\\n' 'Dependency-Check Core version 13.0.0'; exit 0;; esac\n\
             out=''; pf=''; prev=''\n\
             for a in \"$@\"; do case \"$prev\" in --out) out=\"$a\";; --propertyfile) pf=\"$a\";; esac; prev=\"$a\"; done\n\
             if [ -f \"$pf\" ]; then /bin/cp \"$pf\" '{props}'; /bin/cat \"$pf\"; /bin/cat \"$pf\" >&2; fi\n\
             {write}\n\
             exit {exit}\n",
            argv = rec.join("argv").display(),
            props = rec.join("props").display(),
        );
        let path = bin.join("dependency-check");
        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    };
    let engine = Engine::open(&root).with_scan_environment(ScanEnvironment {
        user_dir: Some(user.clone()),
        search_path: Some(bin.clone().into_os_string()),
        tools_dir: None,
        nvd_api_key: NvdApiKey::new(NVD_KEY),
    });

    let captured = Captured::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .with_max_level(tracing::Level::TRACE)
        .finish();
    let mut payloads = Vec::new();
    tracing::subscriber::with_default(subscriber, || {
        tracing::callsite::rebuild_interest_cache();
        tracing::trace!("capture control");
        for (exit, outcome) in [(1, "failed"), (0, "ran")] {
            stub(exit);
            let scan = serde_json::to_value(engine.security_scan(true).expect("the scan runs")).unwrap();
            let row = scan["scanners"].as_array().unwrap().iter().find(|r| r["scanner"] == "dependency-check").unwrap();
            assert_eq!(row["outcome"], outcome, "{row}");
            assert_eq!(fs::read_to_string(rec.join("props")).unwrap(), format!("nvd.api.key={NVD_KEY}\n"), "the key was handed over");
            assert!(fs::symlink_metadata(root.join(".logos/security").join(NVD_KEY_FILE)).is_err(), "the key file outlived the run");
            // After each run: the failed run's log is gone once the next one runs cleanly.
            assert_eq!(files_holding(&root.join(".logos"), NVD_KEY), Vec::<PathBuf>::new(), "{outcome}: a file under .logos holds the key");
            payloads.push(scan);
            payloads.push(serde_json::to_value(engine.security_status().unwrap()).unwrap());
            payloads.push(serde_json::to_value(engine.security_findings(&FindingsFilter::default()).unwrap()).unwrap());
        }
    });
    let log = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    assert!(log.contains("capture control") && log.contains("security scanner run"), "the capture saw the runs: {log}");
    assert!(!log.contains(NVD_KEY), "a log line holds the key: {log}");
    for payload in &payloads {
        assert!(!payload.to_string().contains(NVD_KEY), "a payload holds the key: {payload}");
    }
    let status = &payloads[4];
    let dc = status["scanners"].as_array().unwrap().iter().find(|r| r["scanner"] == "dependency-check").unwrap();
    assert_eq!(dc["nvd_api_key"], "set", "{dc}");
    assert!(payloads[5]["findings"].as_array().is_some_and(|f| !f.is_empty()), "the ran scan was stored: {}", payloads[5]);
    let argv = fs::read_to_string(rec.join("argv")).unwrap();
    assert!(argv.contains("--propertyfile") && !argv.contains(NVD_KEY), "{argv}");
    drop(engine);
    assert_eq!(files_holding(&root.join(".logos"), NVD_KEY), Vec::<PathBuf>::new(), "a file under .logos holds the key");
}
