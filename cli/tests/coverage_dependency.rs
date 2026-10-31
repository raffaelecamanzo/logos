//! `logos coverage run --dependency-consent`, end to end through the shipped
//! binary (S-749, [FR-CV-18]): a vitest project without its coverage provider
//! is offered the dependency, and only the flag makes its own package manager
//! add it, uncommitted.
//!
//! The binary runs on a git fixture outside every work tree, with
//! `LOGOS_CONFIG_HOME` and `LOGOS_DATA_HOME` pointing at temporary user-level
//! directories and a `PATH` whose only package manager is a stub `pnpm` that
//! records its argv. The full matrix of managers and refusals is
//! `logos-core/src/coverage/deps_tests.rs`.
//!
//! [FR-CV-18]: ../../docs/specs/requirements/FR-CV-18.md

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;
use tempfile::TempDir;

struct World {
    _tmp: TempDir,
    root: PathBuf,
    user: PathBuf,
    data: PathBuf,
    bin: PathBuf,
}

fn executable(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

impl World {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let [root, user, data, bin] = ["project", "user", "data", "bin"].map(|d| base.join(d));
        let w = Self { _tmp: tmp, root, user, data, bin };
        let write = |file: &str, text: &str| {
            let path = w.root.join(file);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        };
        write(".logos/config.toml", "[project]\nid = \"coverage-dependency-fixture\"\n");
        write(".gitignore", "node_modules/\n.logos/\n");
        write("package.json", "{\"name\": \"calc\", \"devDependencies\": {\"vitest\": \"^3.2.4\"}}\n");
        write("pnpm-lock.yaml", "lockfileVersion: '9.0'\n");
        write("src/sum.js", "export const sum = (a, b) => a + b;\n");
        write("node_modules/vitest/package.json", "{\"name\": \"vitest\", \"version\": \"3.2.4\"}\n");
        executable(&w.root.join("node_modules/.bin/vitest"), "#!/bin/sh\nexit 0\n");
        executable(
            &w.bin.join("pnpm"),
            &format!(
                "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done >> '{rec}'\n\
                 /bin/mkdir -p node_modules/@vitest/coverage-v8\n\
                 printf '{{\"name\": \"@vitest/coverage-v8\", \"version\": \"3.2.4\"}}\\n' > node_modules/@vitest/coverage-v8/package.json\n\
                 printf '{{\"devDependencies\": {{\"@vitest/coverage-v8\": \"^3.2.4\", \"vitest\": \"^3.2.4\"}}}}\\n' > package.json\n\
                 printf '  @vitest/coverage-v8: 3.2.4\\n' >> pnpm-lock.yaml\n",
                rec = w.pnpm_record().display()
            ),
        );
        for args in [&["init", "-q", "-b", "main"][..], &["add", "-A"], &["commit", "-q", "-m", "seed"]] {
            w.git(args);
        }
        assert!(w.logos(&["index"]).status.success(), "the fixture indexes");
        w
    }

    fn pnpm_record(&self) -> PathBuf {
        self.bin.parent().unwrap().join("pnpm.argv")
    }

    fn git(&self, args: &[&str]) -> String {
        let out = Command::new("git")
            .current_dir(&self.root)
            .env("GIT_CEILING_DIRECTORIES", self.root.parent().unwrap())
            .args(["-c", "maintenance.auto=false", "-c", "gc.auto=0", "-c", "user.email=t@logos", "-c", "user.name=t"])
            .args(args)
            .output()
            .expect("git runs");
        assert!(out.status.success(), "git {args:?}");
        String::from_utf8(out.stdout).unwrap()
    }

    fn logos(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_logos"))
            .arg("--project")
            .arg(&self.root)
            .arg("--json")
            .args(args)
            .env("LOGOS_CONFIG_HOME", &self.user)
            .env("LOGOS_DATA_HOME", &self.data)
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin.display()))
            .env("GIT_CEILING_DIRECTORIES", self.root.parent().unwrap())
            .output()
            .expect("the logos binary runs")
    }

    fn run(&self, args: &[&str]) -> (Option<i32>, Value) {
        let out = self.logos(args);
        let payload = serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|_| panic!("JSON on stdout: {}", String::from_utf8_lossy(&out.stderr)));
        (out.status.code(), payload)
    }

    fn pnpm_argv(&self) -> String {
        fs::read_to_string(self.pnpm_record()).unwrap_or_default()
    }
}

fn vitest(payload: &Value) -> &Value {
    payload["tools"].as_array().unwrap().iter().find(|t| t["tool"] == "vitest").expect("vitest has an outcome")
}

#[test]
fn only_the_dependency_consent_flag_adds_the_provider_and_nothing_is_committed() {
    let w = World::new();
    let head = w.git(&["rev-parse", "HEAD"]);

    // The execution consent alone: the dependency is offered, nothing is added.
    let (code, offered) = w.run(&["coverage", "run", "--consent"]);
    assert_eq!(code, Some(0), "{offered}");
    let dependency = &offered["dependencies"][0];
    assert_eq!(dependency["outcome"], "pending", "{offered}");
    assert_eq!(dependency["command"], "pnpm add -D @vitest/coverage-v8@3");
    assert_eq!(dependency["files"], serde_json::json!(["package.json", "pnpm-lock.yaml"]));
    let statement = dependency["consent"].as_str().unwrap();
    assert!(statement.contains("lifecycle scripts") && statement.contains("logos coverage run --consent --dependency-consent"), "{statement}");
    assert_eq!(vitest(&offered)["outcome"], "not_installed");
    assert_eq!(w.pnpm_argv(), "", "no package manager ran");
    assert_eq!(w.git(&["status", "--porcelain"]), "");

    // The flag alone: vitest would now start, so the execution consent is
    // asked for it first, and nothing is added.
    let refused = w.logos(&["coverage", "run", "--dependency-consent"]);
    assert_eq!(refused.status.code(), Some(2), "{}", String::from_utf8_lossy(&refused.stderr));
    assert!(String::from_utf8_lossy(&refused.stderr).contains("for vitest —"));
    assert_eq!(w.pnpm_argv(), "", "no package manager ran");

    // The command the statement names: pnpm adds it at the vitest major, and vitest starts.
    let (_, added) = w.run(&["coverage", "run", "--consent", "--dependency-consent"]);
    assert_eq!(added["dependencies"][0]["outcome"], "added", "{added}");
    assert_eq!(w.pnpm_argv(), "add\n-D\n@vitest/coverage-v8@3\n");
    assert_ne!(vitest(&added)["outcome"], "not_installed", "{added}");
    assert_eq!(w.git(&["status", "--porcelain"]), " M package.json\n M pnpm-lock.yaml\n");
    assert_eq!(w.git(&["rev-parse", "HEAD"]), head, "nothing is committed");
    assert!(w.user.join("coverage-dependency-consent.toml").is_file(), "the consent is user-level");
}

/// Sprint 108 review (S-749 × S-750 × the stop signal): a `coverage run
/// --dependency-consent` stopped by SIGINT or SIGTERM while the package
/// manager is adding the provider puts `package.json`, the lockfile and the
/// provider's own entry back before it exits 128 + the signal, as a cancelled
/// addition from the page does — the signal must not end the process before
/// the addition's put-back runs.
#[test]
fn a_dependency_addition_stopped_by_a_signal_leaves_no_half_applied_change() {
    for (signal, code) in [("INT", 130), ("TERM", 143)] {
        let w = World::new();
        let marker = w.bin.parent().unwrap().join("pnpm.changed");
        // Changes the files as the real manager would, says so, then waits to be killed.
        executable(
            &w.bin.join("pnpm"),
            &format!(
                "#!/bin/sh\n/bin/mkdir -p node_modules/@vitest/coverage-v8\n\
                 printf '{{}}\\n' > node_modules/@vitest/coverage-v8/package.json\n\
                 printf '{{\"half\": true}}\\n' > package.json\n\
                 printf 'half\\n' >> pnpm-lock.yaml\n\
                 : > '{marker}'\n\
                 n=0; while [ $n -lt 1200 ]; do /bin/sleep 0.05; n=$((n+1)); done\n",
                marker = marker.display()
            ),
        );
        let before = |file: &str| fs::read(w.root.join(file)).unwrap();
        let (manifest, lock) = (before("package.json"), before("pnpm-lock.yaml"));
        let mut run = Command::new(env!("CARGO_BIN_EXE_logos"))
            .arg("--project")
            .arg(&w.root)
            .args(["coverage", "run", "--consent", "--dependency-consent"])
            .env("LOGOS_CONFIG_HOME", &w.user)
            .env("LOGOS_DATA_HOME", &w.data)
            .env("PATH", format!("{}:/usr/bin:/bin", w.bin.display()))
            .env("GIT_CEILING_DIRECTORIES", w.root.parent().unwrap())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("the logos binary runs");
        let deadline = std::time::Instant::now()
            + logos_core::perf::scaled(std::time::Duration::from_secs(60));
        while !marker.exists() {
            assert!(std::time::Instant::now() < deadline, "SIG{signal}: the stub manager never ran");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let sent = Command::new("kill").arg(format!("-{signal}")).arg(run.id().to_string()).status().unwrap();
        assert!(sent.success());
        let status = run.wait().unwrap();
        assert_eq!(status.code(), Some(code), "SIG{signal}: exits 128 + the signal: {status:?}");
        assert_eq!(before("package.json"), manifest, "SIG{signal}: package.json is put back");
        assert_eq!(before("pnpm-lock.yaml"), lock, "SIG{signal}: the lockfile is put back");
        assert!(!w.root.join("node_modules/@vitest/coverage-v8").exists(), "SIG{signal}: the provider is removed");
        assert_eq!(w.git(&["status", "--porcelain"]), "", "SIG{signal}: nothing half-applied");
    }
}
