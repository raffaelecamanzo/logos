//! A `serve --mcp` whose host disconnects while a coverage run goes leaves no
//! orphan (S-738, [FR-CV-15]): the real `logos-mcp` stdio binary — the same
//! `mcp::serve_stdio` that `logos serve --mcp` runs — is started on a Maven
//! project, `coverage_run` starts the job, and stdin closes. The process must
//! exit, and the tool, its forked test JVM and the build daemon that detached
//! from it must all be gone.
//!
//! A process property, so a process-level test: the stub `mvn` is found on a
//! `PATH` whose first entry is the stub directory, the consent lives in a
//! temporary `LOGOS_CONFIG_HOME`, and nothing reaches the network.
//!
//! [FR-CV-15]: ../../docs/specs/requirements/FR-CV-15.md
#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn git(cwd: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["-c", "maintenance.auto=false", "-c", "gc.auto=0", "-c", "user.email=t@logos", "-c", "user.name=t"])
        .args(args)
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

fn frame(value: serde_json::Value) -> String {
    format!("{value}\n")
}

/// Every pid in the process table.
fn process_table() -> Vec<u32> {
    let out = Command::new("ps").args(["-A", "-o", "pid="]).output().expect("ps runs");
    String::from_utf8_lossy(&out.stdout).lines().filter_map(|line| line.trim().parse().ok()).collect()
}

fn wait_for(what: &str, deadline: Duration, mut done: impl FnMut() -> bool) {
    let started = Instant::now();
    while !done() {
        assert!(started.elapsed() < logos_core::perf::scaled(deadline), "{what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_host_disconnect_during_a_coverage_run_leaves_no_orphan() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let [root, bin, user, data, rec] = ["project", "bin", "user", "data", "rec"].map(|d| base.join(d));
    for dir in [&root, &bin, &user, &data, &rec] {
        std::fs::create_dir_all(dir).unwrap();
    }
    git(&root, &["init", "-q", "-b", "main"]);
    std::fs::write(root.join("pom.xml"), "<project><modelVersion>4.0.0</modelVersion></project>\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "seed"]);
    // mvn: a forked test JVM and a detached build daemon, each pid written
    // with the tool's own, then it waits on the JVM.
    let pids = rec.join("mvn.pids");
    let mvn = bin.join("mvn");
    std::fs::write(
        &mvn,
        format!(
            "#!/bin/sh\n[ \"$1\" = --version ] && {{ echo 'Apache Maven 3.9.9'; exit 0; }}\n\
             /bin/sleep 60 &\njvm=$!\n( /bin/sleep 60 & echo $! > '{p}.daemon' )\n\
             echo $$ $jvm $(/bin/cat '{p}.daemon') > '{p}.tmp' && /bin/mv '{p}.tmp' '{p}'\nwait $jvm\n",
            p = pids.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&mvn, std::fs::Permissions::from_mode(0o755)).unwrap();
    logos_core::coverage::consent::require(Some(&user), &root, &["maven"], None, true, "logos coverage run")
        .expect("the user consents");

    let mut server = Command::new(env!("CARGO_BIN_EXE_logos-mcp"))
        .arg(&root)
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .env(logos_core::config::USER_CONFIG_HOME_ENV, &user)
        .env("LOGOS_DATA_HOME", &data)
        .env_remove("RUST_LOG")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn logos-mcp");
    let mut stdin = server.stdin.take().unwrap();
    let mut stdout = BufReader::new(server.stdout.take().unwrap());
    let mut send = |value: serde_json::Value| stdin.write_all(frame(value).as_bytes()).unwrap();
    send(serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}}
    }));
    send(serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    send(serde_json::json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": {"name": "coverage_run", "arguments": {}}
    }));
    let started = Instant::now();
    let answer = loop {
        let mut line = String::new();
        assert!(stdout.read_line(&mut line).unwrap() > 0, "the server closed stdout before answering");
        let message: serde_json::Value = serde_json::from_str(&line).unwrap();
        if message["id"] == 2 {
            break message;
        }
        assert!(
            started.elapsed() < logos_core::perf::scaled(Duration::from_secs(60)),
            "no answer to coverage_run"
        );
    };
    let text = answer["result"]["content"][0]["text"].as_str().unwrap_or_else(|| panic!("{answer}"));
    let started: serde_json::Value = serde_json::from_str(text).unwrap();
    assert_eq!((&started["job"]["kind"], &started["job"]["state"]), (&"coverage-run".into(), &"running".into()), "{started}");

    wait_for("mvn never started", Duration::from_secs(30), || pids.exists());
    let running: Vec<u32> =
        std::fs::read_to_string(&pids).unwrap().split_whitespace().map(|p| p.parse().unwrap()).collect();
    assert_eq!(running.len(), 3, "mvn, its JVM and its daemon: {running:?}");

    drop(send);
    drop(stdin);
    let exited = Instant::now();
    let status = loop {
        if let Some(status) = server.try_wait().unwrap() {
            break status;
        }
        if exited.elapsed() > Duration::from_secs(30) {
            let _ = server.kill();
            panic!("the server did not exit after its host disconnected");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(status.success(), "{status}");
    wait_for(
        &format!("the coverage run outlived the server: {running:?}"),
        Duration::from_secs(10),
        || !process_table().iter().any(|p| running.contains(p)),
    );
}
