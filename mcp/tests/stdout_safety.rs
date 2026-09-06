//! Process-level acceptance tests for the stdout-safety invariant and
//! host-disconnect cleanup (S-017).
//!
//! These spawn the real `logos-mcp` stdio binary — the invariants under test
//! are *process* properties (what reaches the real stdout, whether the
//! process exits) that an in-process duplex harness cannot observe.
//!
//! Coverage: stdout carries only JSON-RPC framing even at trace log level
//! (FR-MC-04, NFR-RA-01, ADR-13, UAT-MC-02); a malformed request gets a
//! structured JSON-RPC error with the server still alive; a host disconnect
//! leaves no orphaned serve process (FR-MC-06, NFR-RA-12, UAT-MC-04).

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStderr, ChildStdout, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

/// Spawn `logos-mcp <root>` at TRACE log level with piped stdio.
fn spawn_server(root: &std::path::Path) -> Child {
    Command::new(env!("CARGO_BIN_EXE_logos-mcp"))
        .arg(root)
        .env("RUST_LOG", "trace")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn logos-mcp")
}

/// Stream stdout lines to a channel — the pipe must be drained concurrently
/// or a full pipe buffer would block the server and fake a hang.
fn drain_stdout(stdout: ChildStdout) -> Receiver<String> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    rx
}

/// Drain stderr on a thread for the same no-blocking reason; trace level is
/// torrential. Returns a handle yielding the captured text.
fn drain_stderr(stderr: ChildStderr) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut text = String::new();
        let _ = BufReader::new(stderr).read_to_string(&mut text);
        text
    })
}

/// Wait for the child to exit on its own, or fail the test — a hung process
/// here IS the orphaned-process bug (NFR-RA-12). Kills the child on failure
/// so the test suite itself never leaks one.
fn wait_with_deadline(child: &mut Child, deadline: Duration) -> std::process::ExitStatus {
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().expect("try_wait") {
            return status;
        }
        if started.elapsed() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "server did not exit within {deadline:?} after host disconnect \
                 (orphaned process, NFR-RA-12)"
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// One newline-delimited JSON-RPC message.
fn frame(value: serde_json::Value) -> String {
    format!("{value}\n")
}

fn initialize_request(id: u64, client: &str) -> String {
    frame(serde_json::json!({
        "jsonrpc": "2.0", "id": id, "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": client, "version": "0"}
        }
    }))
}

/// Collect stdout lines until a response exists for every expected id,
/// asserting EVERY line is JSON-RPC framing (the NFR-RA-01 invariant).
fn collect_responses(
    rx: &Receiver<String>,
    expected_ids: &[serde_json::Value],
    deadline: Duration,
) -> HashMap<serde_json::Value, serde_json::Value> {
    let started = Instant::now();
    let mut responses = HashMap::new();
    while expected_ids.iter().any(|id| !responses.contains_key(id)) {
        let remaining = deadline.checked_sub(started.elapsed()).unwrap_or_else(|| {
            panic!(
                "timed out awaiting responses; got ids {:?}",
                responses.keys()
            )
        });
        let line = match rx.recv_timeout(remaining) {
            Ok(line) => line,
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {
                panic!(
                    "stdout closed before all responses arrived; got ids {:?}",
                    responses.keys()
                )
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let value: serde_json::Value = serde_json::from_str(&line).unwrap_or_else(|e| {
            panic!("non-JSON-RPC bytes on stdout (NFR-RA-01 violated): {e}\nline: {line}")
        });
        assert_eq!(
            value["jsonrpc"], "2.0",
            "stdout line is JSON but not JSON-RPC framing: {line}"
        );
        responses.insert(value["id"].clone(), value);
    }
    responses
}

fn tool_count(response: &serde_json::Value) -> usize {
    response["result"]["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("tools/list response carries a tools array: {response}"))
        .len()
}

#[test]
fn stdout_carries_only_jsonrpc_even_at_trace_level() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut child = spawn_server(dir.path());
    let mut stdin = child.stdin.take().expect("stdin");
    let stdout = drain_stdout(child.stdout.take().expect("stdout"));
    let stderr = drain_stderr(child.stderr.take().expect("stderr"));

    // Drive a real session: handshake, list, a tool call, a MALFORMED line,
    // then prove the server is still alive — all with trace logging live.
    for message in [
        initialize_request(1, "uat-mc-02"),
        frame(serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"})),
        frame(serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"})),
        frame(serde_json::json!({
            "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": {"name": "status", "arguments": {}}
        })),
        "this line is not JSON-RPC {{{\n".to_string(),
        frame(serde_json::json!({"jsonrpc": "2.0", "id": 4, "method": "tools/list"})),
    ] {
        stdin.write_all(message.as_bytes()).expect("write request");
    }
    stdin.flush().expect("flush");

    // Read every expected response BEFORE hanging up, so none race teardown.
    use serde_json::json;
    let expected = [
        json!(1),
        json!(2),
        json!(3),
        serde_json::Value::Null,
        json!(4),
    ];
    let responses = collect_responses(&stdout, &expected, Duration::from_secs(60));

    // Host disconnect: stdin EOF. No orphan may remain (UAT-MC-04).
    drop(stdin);
    let status = wait_with_deadline(&mut child, Duration::from_secs(60));
    assert!(
        status.success(),
        "server must exit cleanly on host disconnect, got {status}"
    );

    // Any straggler stdout lines written during teardown still obey the invariant.
    while let Ok(line) = stdout.try_recv() {
        if line.trim().is_empty() {
            continue;
        }
        let value: serde_json::Value = serde_json::from_str(&line)
            .unwrap_or_else(|e| panic!("non-JSON-RPC teardown bytes on stdout: {e}\nline: {line}"));
        assert_eq!(value["jsonrpc"], "2.0");
    }

    // The session exercised the surface end-to-end.
    let init = &responses[&json!(1)];
    assert_eq!(init["result"]["serverInfo"]["name"], "logos");
    assert!(
        init["result"]["instructions"]
            .as_str()
            .is_some_and(|i| !i.is_empty()),
        "server-instructions must ride the initialize response (FR-MC-03)"
    );
    assert_eq!(
        tool_count(&responses[&json!(2)]),
        28,
        "all 28 logos tools register (FR-MC-01)"
    );
    assert_ne!(
        responses[&json!(3)]["result"]["isError"],
        json!(true),
        "the status tool call must succeed"
    );

    // The malformed line produced a STRUCTURED parse error (id null)…
    assert_eq!(
        responses[&serde_json::Value::Null]["error"]["code"],
        -32700,
        "malformed input → JSON-RPC parse error"
    );
    // …and the server survived it to answer the next request (FR-MC-06).
    assert_eq!(
        tool_count(&responses[&json!(4)]),
        28,
        "server alive after malformed input"
    );

    // Trace logs flowed — to stderr, where they belong (ADR-13).
    let stderr = stderr.join().expect("stderr drain thread");
    assert!(
        !stderr.trim().is_empty(),
        "trace-level logs must appear on stderr (proves logging was live during the run)"
    );
}

#[test]
fn eof_before_initialize_is_a_clean_disconnect_not_a_fault() {
    // `logos serve --mcp < /dev/null`: a host that closes stdin before the
    // initialize handshake is a disconnect like any other — exit 0, zero
    // bytes on stdout (FR-MC-06, NFR-RA-12; the sprint-review headline check).
    let dir = tempfile::tempdir().expect("tempdir");
    let mut child = spawn_server(dir.path());
    let stdin = child.stdin.take().expect("stdin");
    let stdout = drain_stdout(child.stdout.take().expect("stdout"));
    let _stderr = drain_stderr(child.stderr.take().expect("stderr"));

    drop(stdin); // EOF before any frame is sent

    let status = wait_with_deadline(&mut child, Duration::from_secs(60));
    assert!(
        status.success(),
        "pre-handshake EOF must wind down cleanly, got {status}"
    );
    while let Ok(line) = stdout.try_recv() {
        assert!(
            line.trim().is_empty(),
            "no bytes may reach stdout in a session with no requests: {line}"
        );
    }
}

#[test]
fn host_disconnect_mid_call_leaves_no_orphaned_process() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut child = spawn_server(dir.path());
    let mut stdin = child.stdin.take().expect("stdin");
    let _stdout = drain_stdout(child.stdout.take().expect("stdout"));
    let _stderr = drain_stderr(child.stderr.take().expect("stderr"));

    // Handshake, then fire a tool call and hang up WITHOUT reading the
    // response — the abrupt mid-call disconnect of UAT-MC-04.
    for message in [
        initialize_request(1, "uat-mc-04"),
        frame(serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"})),
        frame(serde_json::json!({
            "jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": {"name": "context", "arguments": {"task": "anything at all"}}
        })),
    ] {
        stdin.write_all(message.as_bytes()).expect("write request");
    }
    stdin.flush().expect("flush");
    drop(stdin); // mid-call hangup

    // No orphan: the process must wind down by itself (FR-MC-06, NFR-RA-12).
    let status = wait_with_deadline(&mut child, Duration::from_secs(60));
    assert!(status.success(), "clean teardown expected, got {status}");
}
