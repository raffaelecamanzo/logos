//! `logos coverage refresh` under the per-project coverage execution consent,
//! end to end through the shipped binary (S-736, [FR-CV-13]).
//!
//! The binary runs on a git fixture outside every work tree, with
//! `LOGOS_CONFIG_HOME` and `LOGOS_DATA_HOME` pointing at temporary user-level
//! directories, so this machine's own consent records are never read or
//! written. The `refresh_cmd` is a stub that appends to `spawned.log` — the
//! evidence it ran — and writes a conventional `lcov.info`. The MCP side is
//! `mcp/tests/coverage_parity.rs`.
//!
//! [FR-CV-13]: ../../docs/specs/requirements/FR-CV-13.md

#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;
use tempfile::TempDir;

struct World {
    _tmp: TempDir,
    root: PathBuf,
    user: PathBuf,
    data: PathBuf,
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

/// A `refresh_cmd` that appends `tag` to `spawned.log`, then writes an LCOV
/// report covering `src/a.rs`.
fn refresh_cmd(tag: &str) -> String {
    format!("printf {tag} >> spawned.log && printf 'TN:suite\\nSF:src/a.rs\\nDA:1,1\\nend_of_record\\n' > lcov.info")
}

impl World {
    fn new(command: &str) -> Self {
        let tmp = TempDir::new().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let [root, user, data] = ["project", "user", "data"].map(|d| base.join(d));
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/a.rs"), "pub fn a() -> i64 { 0 }\n").unwrap();
        let w = Self { _tmp: tmp, root, user, data };
        w.set_command(command);
        git(&w.root, &["init", "-q", "-b", "main"]);
        git(&w.root, &["add", "."]);
        git(&w.root, &["commit", "-q", "-m", "seed"]);
        assert!(w.logos(&["index"]).status.success());
        w
    }

    fn set_command(&self, command: &str) {
        let escaped = command.replace('\\', "\\\\").replace('"', "\\\"");
        fs::create_dir_all(self.root.join(".logos")).unwrap();
        fs::write(
            self.root.join(".logos/config.toml"),
            format!("[project]\nid = \"coverage-consent-fixture\"\n\n[coverage_ingest]\nrefresh_cmd = \"{escaped}\"\n"),
        )
        .unwrap();
    }

    fn logos(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_logos"))
            .arg("--project")
            .arg(&self.root)
            .args(args)
            .env("LOGOS_CONFIG_HOME", &self.user)
            .env("LOGOS_DATA_HOME", &self.data)
            .env("GIT_CEILING_DIRECTORIES", self.root.parent().unwrap())
            .output()
            .expect("the logos binary runs")
    }

    fn json(&self, args: &[&str]) -> Value {
        let mut all = vec!["--json"];
        all.extend_from_slice(args);
        let out = self.logos(&all);
        assert!(out.status.success(), "logos {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        serde_json::from_slice(&out.stdout).expect("JSON on stdout")
    }

    /// The refusal's stderr, asserting exit 2.
    fn refused(&self) -> String {
        let out = self.logos(&["coverage", "refresh"]);
        assert_eq!(out.status.code(), Some(2), "{}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stderr).into_owned()
    }

    fn spawned(&self) -> String {
        fs::read_to_string(self.root.join("spawned.log")).unwrap_or_default()
    }
}

#[test]
fn coverage_refresh_needs_the_consent_that_pins_its_command() {
    let first = refresh_cmd("a");
    let w = World::new(&first);

    // No consent: exit 2 naming the flag, the command verbatim; nothing spawned or recorded.
    let stderr = w.refused();
    assert!(stderr.contains("logos coverage refresh --consent"), "{stderr}");
    assert!(stderr.contains(&format!("verbatim:\n{first}\n")), "{stderr}");
    assert_eq!(w.spawned(), "", "nothing is spawned before consent");
    assert!(!w.user.exists() || fs::read_dir(&w.user).unwrap().next().is_none(), "a refusal records nothing");

    let granted = w.json(&["coverage", "refresh", "--consent"]);
    assert_eq!(granted["consent_recorded"], serde_json::json!(["custom"]));
    let custom = &granted["tools"][0];
    assert_eq!((&custom["tool"], &custom["outcome"]), (&serde_json::json!("custom"), &serde_json::json!("ran")), "{granted}");
    assert_eq!(custom["ingests"][0]["matched_files"], 1, "{granted}");
    assert_eq!(w.json(&["coverage", "refresh"])["consent_recorded"], serde_json::json!([]));
    assert_eq!(w.spawned(), "aa", "later runs do not ask");

    // The committed command changes: asked again, showing the new text.
    let changed = refresh_cmd("b");
    w.set_command(&changed);
    let stderr = w.refused();
    assert!(stderr.contains("refresh_cmd text changed") && stderr.contains(&format!("verbatim:\n{changed}\n")), "{stderr}");
    assert_eq!(w.spawned(), "aa", "the changed command did not run");
    w.json(&["coverage", "refresh", "--consent"]);
    assert_eq!(w.spawned(), "aab");

    // The record lives in the user-level directory, and nowhere in the project.
    assert!(w.user.join("coverage-consent.toml").is_file());
    assert!(!w.user.join("security-consent.toml").exists(), "the security consent is not touched");
    let mut stack = vec![w.root.clone()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.file_name().is_some_and(|n| n == ".git") {
                continue;
            }
            assert!(!path.to_string_lossy().contains("consent"), "a consent record in the project: {}", path.display());
            if path.is_dir() {
                stack.push(path);
            }
        }
    }
}

/// S-737: `coverage refresh` and `coverage run --tool custom` print the run's
/// outcome and exit 1 when the command they started failed — 0 only when
/// every tool started ran cleanly.
#[test]
fn a_failing_custom_command_exits_1_with_its_failed_outcome() {
    let w = World::new("echo 'build broke' >&2; exit 7");
    for args in [&["coverage", "refresh", "--consent"][..], &["coverage", "run", "--tool", "custom"]] {
        let mut all = vec!["--json"];
        all.extend_from_slice(args);
        let out = w.logos(&all);
        assert_eq!(out.status.code(), Some(1), "logos {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        let payload: Value = serde_json::from_slice(&out.stdout).expect("JSON on stdout");
        assert_eq!(payload["tools"][0]["outcome"], "failed", "{payload}");
        assert_eq!(payload["tools"][0]["exit_code"], 7, "{payload}");
    }
}

/// `logos coverage install` without `--consent` (S-748, FR-CV-17): exit 2,
/// naming the pinned `cargo-llvm-cov` download and the `rustup` command, with
/// nothing downloaded, no tools directory written and no `rustup` run. The
/// search path holds only a recording `rustup` stub, so no real tool is found.
#[test]
fn coverage_install_without_consent_exits_2_naming_both_lines_and_runs_nothing() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = TempDir::new().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let [root, user, data, bin] = ["project", "user", "data", "bin"].map(|d| base.join(d));
    for dir in [&root, &user, &bin] {
        fs::create_dir_all(dir).unwrap();
    }
    fs::write(root.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
    let log = base.join("rustup.log");
    let rustup = bin.join("rustup");
    fs::write(&rustup, format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\n", log.display())).unwrap();
    fs::set_permissions(&rustup, fs::Permissions::from_mode(0o755)).unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_logos"))
        .arg("--project")
        .arg(&root)
        .args(["coverage", "install"])
        .env("LOGOS_CONFIG_HOME", &user)
        .env("LOGOS_DATA_HOME", &data)
        .env("PATH", &bin)
        .env("GIT_CEILING_DIRECTORIES", &base)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    for named in ["cargo-llvm-cov 0.9.1 from github.com", "rustup component add llvm-tools-preview", "logos coverage install --consent"] {
        assert!(stderr.contains(named), "names {named:?}: {stderr}");
    }
    assert!(!log.exists(), "no rustup command ran");
    assert!(!data.exists(), "no tools directory was written");
    assert!(fs::read_dir(&user).unwrap().next().is_none(), "no consent was recorded");
}

/// `logos coverage install --consent` exits with the install's own exit code:
/// with no `curl` on the search path the download fails, so the process exits
/// 1 — having recorded the consent and run the stub `rustup`'s add.
#[test]
fn coverage_install_exits_1_when_cargo_llvm_cov_could_not_be_installed() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = TempDir::new().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let [root, user, data, bin] = ["project", "user", "data", "bin"].map(|d| base.join(d));
    for dir in [&root, &user, &bin] {
        fs::create_dir_all(dir).unwrap();
    }
    let log = base.join("rustup.log");
    let rustup = bin.join("rustup");
    fs::write(&rustup, format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexit 0\n", log.display())).unwrap();
    fs::set_permissions(&rustup, fs::Permissions::from_mode(0o755)).unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_logos"))
        .arg("--project")
        .arg(&root)
        .args(["coverage", "install", "--consent"])
        .env("LOGOS_CONFIG_HOME", &user)
        .env("LOGOS_DATA_HOME", &data)
        .env("PATH", &bin)
        .env("GIT_CEILING_DIRECTORIES", &base)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("curl not found"), "{stdout}");
    assert_eq!(fs::read_to_string(&log).unwrap(), "component list --installed\ncomponent add llvm-tools-preview\n");
}
