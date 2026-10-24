//! Stopping `logos serve --ui` cancels its running scan and install jobs
//! (S-759, [FR-SC-15], [FR-SC-24]): a real serve, stopped by SIGINT or
//! SIGTERM mid-download or mid-scan, leaves no `curl` or scanner process
//! running and no partial download in staging, and exits cleanly.
//!
//! No test reaches the internet: `curl` and `gitleaks` are stubs on a search
//! path of their own, each writing its own and its child's pid and then
//! hanging until it is killed. The user, config and tools directories are
//! temporary, passed through the serve's environment.
//!
//! [FR-SC-15]: ../../docs/specs/requirements/FR-SC-15.md
//! [FR-SC-24]: ../../docs/specs/requirements/FR-SC-24.md
#![cfg(unix)]

use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use logos_core::tools::{Manifest, STAGING_DIR};
use serde_json::{json, Value};
use tempfile::TempDir;

/// How long a stub, a listener or a stopping serve is waited for.
const WAIT: Duration = Duration::from_secs(60);

/// The coverage-data path an instrumented build was given, to pass on through a
/// cleared environment. Without it the spawned `logos` writes a
/// `default_*.profraw` into its working directory, which is this crate.
fn profile_file() -> Option<(&'static str, std::ffi::OsString)> {
    std::env::var_os("LLVM_PROFILE_FILE").map(|path| ("LLVM_PROFILE_FILE", path))
}

/// One machine: a committed project, a search path holding the stubs, and the
/// user's config and tools directories.
struct Machine {
    _tmp: TempDir,
    base: PathBuf,
}

impl Machine {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let m = Machine { _tmp: tmp, base };
        for dir in [m.bin(), m.project(), m.home(), m.config(), m.tools()] {
            fs::create_dir_all(dir).unwrap();
        }
        fs::write(m.project().join("main.py"), "def main():\n    return 1\n").unwrap();
        for args in [&["init", "-q", "-b", "main"][..], &["add", "."], &["commit", "-q", "-m", "seed"]] {
            let status = Command::new("git")
                .args(args)
                .current_dir(m.project())
                .env("GIT_CEILING_DIRECTORIES", &m.base)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?}");
        }
        m
    }

    fn bin(&self) -> PathBuf {
        self.base.join("bin")
    }
    fn project(&self) -> PathBuf {
        self.base.join("project")
    }
    fn home(&self) -> PathBuf {
        self.base.join("home")
    }
    fn config(&self) -> PathBuf {
        self.base.join("config")
    }
    fn tools(&self) -> PathBuf {
        self.base.join("tools")
    }
    /// Where a hanging stub writes `<its pid> <its child's pid>`.
    fn pids(&self, stub: &str) -> PathBuf {
        self.base.join(format!("{stub}.pids"))
    }

    /// A stub on the search path that runs `prelude`, then records its pid and
    /// its child's and waits on a child that sleeps until it is killed.
    fn hanging_stub(&self, name: &str, prelude: &str) {
        let path = self.bin().join(name);
        fs::write(
            &path,
            format!(
                "#!/bin/sh\n{prelude}\n/bin/sleep 60 &\necho $$ $! > '{pids}.tmp'\n/bin/mv '{pids}.tmp' '{pids}'\nwait\n",
                pids = self.pids(name).display()
            ),
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// `logos serve --ui` on a free port, once it listens; with `mcp`,
    /// `serve --mcp --ui` with its MCP input a pipe held open, as a host
    /// holds it.
    fn serve(&self, mcp: bool) -> Served {
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let mut command = Command::new(env!("CARGO_BIN_EXE_logos"));
        command
            .arg("--project")
            .arg(self.project())
            .args(["serve", "--ui", "--port", &port.to_string()])
            .args(mcp.then_some("--mcp"))
            .env_clear()
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin().display()))
            .env("HOME", self.home())
            .env("LOGOS_CONFIG_HOME", self.config())
            .env("LOGOS_DATA_HOME", self.tools())
            .env("GIT_CEILING_DIRECTORIES", &self.base)
            .envs(profile_file())
            // The runner logs each job its shutdown cancels.
            .env("RUST_LOG", "logos::jobs=info")
            .stdin(if mcp { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut child = command.spawn().expect("logos serve --ui starts");
        let mcp_input = child.stdin.take();
        let mut served =
            Served { child: Some(child), _mcp_input: mcp_input, addr: ([127, 0, 0, 1], port).into(), intent: String::new() };
        let deadline = Instant::now() + WAIT;
        while TcpStream::connect_timeout(&served.addr, Duration::from_millis(200)).is_err() {
            if let Some(status) = served.child().try_wait().unwrap() {
                panic!("serve --ui exited ({status}): {}", served.stderr());
            }
            assert!(Instant::now() < deadline, "serve --ui did not listen within {WAIT:?}");
            std::thread::sleep(Duration::from_millis(100));
        }
        let (code, shell) = served.request("GET", "/", None);
        assert_eq!(code, 200, "the shell is served");
        let marker = "<meta name=\"logos-intent\" content=\"";
        let at = shell.find(marker).expect("the shell carries the intent token") + marker.len();
        served.intent = shell[at..].split('"').next().unwrap().to_string();
        served
    }

    /// The `<pid> <child pid>` a hanging stub recorded, once it did.
    fn hung(&self, stub: &str, served: &mut Served) -> [u32; 2] {
        let deadline = Instant::now() + WAIT;
        loop {
            if let Ok(line) = fs::read_to_string(self.pids(stub)) {
                let pids: Vec<u32> = line.split_whitespace().map(|p| p.parse().unwrap()).collect();
                return [pids[0], pids[1]];
            }
            if let Some(status) = served.child().try_wait().unwrap() {
                panic!("serve exited ({status}) before {stub} ran: {}", served.stderr());
            }
            assert!(Instant::now() < deadline, "{stub} never ran");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Every file left under the staging directory.
    fn staging(&self) -> Vec<PathBuf> {
        fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
            for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
                if entry.path().is_dir() {
                    walk(&entry.path(), out);
                } else {
                    out.push(entry.path());
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.tools().join(STAGING_DIR), &mut out);
        out
    }
}

/// A running `logos serve --ui`, killed if a failing assertion drops it.
struct Served {
    child: Option<Child>,
    /// The MCP input under `--mcp`, never written and kept open.
    _mcp_input: Option<ChildStdin>,
    addr: SocketAddr,
    intent: String,
}

impl Served {
    fn child(&mut self) -> &mut Child {
        self.child.as_mut().expect("running")
    }

    fn stderr(&mut self) -> String {
        let mut text = String::new();
        if let Some(stderr) = self.child().stderr.as_mut() {
            let _ = stderr.read_to_string(&mut text);
        }
        text
    }

    /// One bounded HTTP/1.0 request: the status code and the body. A `POST`
    /// carries the same-origin and intent-token proof.
    fn request(&self, method: &str, path: &str, body: Option<&Value>) -> (u16, String) {
        let mut stream = TcpStream::connect_timeout(&self.addr, Duration::from_secs(5)).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
        let host = self.addr.to_string();
        let mut head = format!("{method} {path} HTTP/1.0\r\nHost: {host}\r\n");
        let payload = body.map(Value::to_string).unwrap_or_default();
        if body.is_some() {
            head.push_str(&format!(
                "Origin: http://{host}\r\nContent-Type: application/json\r\nx-logos-intent: {}\r\nContent-Length: {}\r\n",
                self.intent,
                payload.len()
            ));
        }
        write!(stream, "{head}\r\n{payload}").unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).unwrap();
        let response = String::from_utf8_lossy(&response).into_owned();
        let (head, body) = response.split_once("\r\n\r\n").expect("a head and a body");
        let code = head.split_whitespace().nth(1).and_then(|c| c.parse().ok()).expect("a status code");
        (code, body.to_string())
    }

    fn post(&self, path: &str, body: &Value) -> (u16, Value) {
        let (code, text) = self.request("POST", path, Some(body));
        (code, serde_json::from_str(&text).unwrap_or(Value::String(text)))
    }

    /// Send `signal` and wait for the serve to exit.
    fn stop(mut self, signal: &str) -> (ExitStatus, String) {
        let pid = self.child().id().to_string();
        assert!(Command::new("/bin/kill").args([signal, &pid]).status().unwrap().success());
        let deadline = Instant::now() + WAIT;
        let status = loop {
            if let Some(status) = self.child().try_wait().unwrap() {
                break status;
            }
            assert!(Instant::now() < deadline, "serve did not stop within {WAIT:?} of {signal}");
            std::thread::sleep(Duration::from_millis(50));
        };
        let stderr = self.stderr();
        self.child = None;
        (status, stderr)
    }
}

impl Drop for Served {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Whether `pid` has gone, waited for: an orphan is reaped by init shortly
/// after it dies.
fn gone(pid: u32) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    let alive = || {
        Command::new("/bin/kill").args(["-0", &pid.to_string()]).stderr(Stdio::null()).status().unwrap().success()
    };
    while alive() {
        if Instant::now() > deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    true
}

/// Kill whatever a failed test left of a stub, so nothing outlives the suite.
fn reap(pids: [u32; 2]) {
    for pid in pids {
        let _ = Command::new("/bin/kill").args(["-9", &pid.to_string()]).stderr(Stdio::null()).status();
    }
}

/// Ctrl-C during a download: the serve cancels the install, which kills
/// `curl` and its child and removes the partial download, then exits 0.
#[test]
fn sigint_during_a_download_leaves_no_curl_and_no_partial_download() {
    let m = Machine::new();
    m.hanging_stub(
        "curl",
        "out=\nwhile [ $# -gt 0 ]; do case \"$1\" in --output) out=\"$2\"; shift 2 ;; *) shift ;; esac; done\nprintf 'partial' > \"$out\"",
    );
    let mut served = m.serve(false);

    let (code, asked) = served.post("/api/v1/security/install", &json!({ "tools": ["gitleaks"] }));
    assert_eq!(code, 409, "the first start asks: {asked}");
    let consent: Vec<String> = asked["plan"]["lines"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|l| (l["line"] == "tool" && l["offered"] == false) || l["line"] == "support")
        .map(|l| format!("{} {}", l["tool"].as_str().unwrap(), l["version"].as_str().unwrap()))
        .collect();
    let (code, started) =
        served.post("/api/v1/security/install", &json!({ "tools": ["gitleaks"], "consent": consent }));
    assert_eq!(code, 200, "{started}");
    let pids = m.hung("curl", &mut served);
    let partial = m.staging().into_iter().find(|p| fs::read_to_string(p).is_ok_and(|t| t == "partial"));
    assert!(partial.is_some(), "the half download sits in staging: {:?}", m.staging());

    let (status, stderr) = served.stop("-INT");
    let curl_gone = pids.map(gone);
    reap(pids);
    assert_eq!(curl_gone, [true, true], "neither curl {} nor its child {} outlived serve", pids[0], pids[1]);
    assert!(m.staging().is_empty(), "no partial download is left: {:?}", m.staging());
    assert!(stderr.contains("job cancel at shutdown"), "serve cancelled the install itself: {stderr}");
    assert!(status.success(), "serve exits cleanly on SIGINT ({status}): {stderr}");
}

/// SIGTERM during a scan: the serve cancels the scan, which kills the
/// scanner and its child, then exits 0.
#[test]
fn sigterm_during_a_scan_leaves_no_scanner_running() {
    let m = Machine::new();
    let version = &Manifest::builtin().tool("gitleaks").expect("gitleaks is managed").version;
    m.hanging_stub(
        "gitleaks",
        &format!("case \"$1\" in version|--version) printf '%s\\n' '{version}'; exit 0 ;; esac"),
    );
    let mut served = m.serve(false);

    let (code, started) = served.post("/api/v1/security/scan", &json!({ "consent": ["gitleaks"] }));
    assert_eq!(code, 200, "{started}");
    let pids = m.hung("gitleaks", &mut served);

    let (status, stderr) = served.stop("-TERM");
    let scanner_gone = pids.map(gone);
    reap(pids);
    assert_eq!(scanner_gone, [true, true], "neither the scanner {} nor its child {} outlived serve", pids[0], pids[1]);
    assert!(stderr.contains("job cancel at shutdown"), "serve cancelled the scan itself: {stderr}");
    assert!(status.success(), "serve exits cleanly on SIGTERM ({status}): {stderr}");
}

/// Ctrl-C to `serve --mcp --ui` while its MCP host still holds the input
/// open, during a scan: the scanner is killed and serve exits, rather than
/// waiting for the input to close.
#[test]
fn sigint_stops_serve_mcp_ui_with_its_input_open_and_leaves_no_scanner_running() {
    let m = Machine::new();
    let version = &Manifest::builtin().tool("gitleaks").expect("gitleaks is managed").version;
    m.hanging_stub(
        "gitleaks",
        &format!("case \"$1\" in version|--version) printf '%s\\n' '{version}'; exit 0 ;; esac"),
    );
    let mut served = m.serve(true);

    let (code, started) = served.post("/api/v1/security/scan", &json!({ "consent": ["gitleaks"] }));
    assert_eq!(code, 200, "{started}");
    let pids = m.hung("gitleaks", &mut served);

    let (status, stderr) = served.stop("-INT");
    let scanner_gone = pids.map(gone);
    reap(pids);
    assert_eq!(scanner_gone, [true, true], "neither the scanner {} nor its child {} outlived serve", pids[0], pids[1]);
    assert!(stderr.contains("job cancel at shutdown"), "serve cancelled the scan itself: {stderr}");
    assert!(status.success(), "serve --mcp --ui exits cleanly on SIGINT ({status}): {stderr}");
}

/// Sprint 107 review D1: a scan the MCP surface runs is no job, but stopping
/// `serve` kills its scanner too — under `serve --mcp --ui`, and under `serve
/// --mcp` alone, which now stops on SIGTERM as well — so the freed project
/// lock admits no scan beside an orphan. SIGKILL is not covered.
#[test]
fn sigterm_during_an_mcp_scan_leaves_no_scanner_running() {
    for ui in [true, false] {
        let m = Machine::new();
        let env = |command: &mut Command| {
            command
                .env_clear()
                .env("PATH", format!("{}:/usr/bin:/bin", m.bin().display()))
                .env("HOME", m.home())
                .env("LOGOS_CONFIG_HOME", m.config())
                .env("LOGOS_DATA_HOME", m.tools())
                .env("GIT_CEILING_DIRECTORIES", &m.base)
                .envs(profile_file());
        };
        let version = &Manifest::builtin().tool("gitleaks").expect("gitleaks is managed").version;
        let answer = format!("case \"$1\" in version|--version) printf '%s\\n' '{version}'; exit 0 ;; esac");
        // The user's consent, given at a terminal by a scan that ends at once:
        // an MCP client cannot give it.
        let quick = m.bin().join("gitleaks");
        fs::write(&quick, format!("#!/bin/sh\n{answer}\nexit 0\n")).unwrap();
        fs::set_permissions(&quick, fs::Permissions::from_mode(0o755)).unwrap();
        for args in [&["index"][..], &["security", "scan", "--consent", "--scanner", "gitleaks"]] {
            let mut command = Command::new(env!("CARGO_BIN_EXE_logos"));
            command.arg("--project").arg(m.project()).args(args);
            env(&mut command);
            let out = command.output().unwrap();
            assert!(out.status.success(), "logos {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        }
        m.hanging_stub("gitleaks", &answer);

        let mut served = if ui {
            m.serve(true)
        } else {
            let mut command = Command::new(env!("CARGO_BIN_EXE_logos"));
            command.arg("--project").arg(m.project()).args(["serve", "--mcp"]);
            env(&mut command);
            let mut child =
                command.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().expect("serve --mcp starts");
            let input = child.stdin.take();
            Served { child: Some(child), _mcp_input: input, addr: ([127, 0, 0, 1], 0).into(), intent: String::new() }
        };
        let input = served._mcp_input.as_mut().expect("the MCP input");
        for message in [
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}}),
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "security_scan", "arguments": {"scanners": ["gitleaks"]}}}),
        ] {
            writeln!(input, "{message}").unwrap();
        }
        input.flush().unwrap();
        let pids = m.hung("gitleaks", &mut served);

        let (status, stderr) = served.stop("-TERM");
        let scanner_gone = pids.map(gone);
        reap(pids);
        assert_eq!(scanner_gone, [true, true], "ui={ui}: neither the MCP scan's scanner {} nor its child {} outlived serve", pids[0], pids[1]);
        assert!(status.success(), "ui={ui}: serve exits cleanly on SIGTERM ({status}): {stderr}");
    }
}
