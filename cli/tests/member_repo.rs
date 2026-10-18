//! `--repo` on the CLI twins of the read-only MCP tools (S-711, [FR-WS-42],
//! [CR-225]): a human and an agent ask the same per-member question the same way,
//! and are refused in the same words.
//!
//! Drives the real binary over a two-member workspace — `svc-a` (the default: first
//! in the manifest), `svc-b`, and `svc-c`, whose store is not a database — plus a
//! plain repository:
//!
//! - at the workspace root the default member answers and says so (`member`), and a
//!   symbol only `svc-b` holds is unknown without `--repo` and found with it;
//! - every twin the MCP surface lets take `repo` takes `--repo`, and no other command
//!   does (the write and gate commands act on the repository they run in);
//! - an unknown member, a degraded one and `--repo` with no workspace each refuse,
//!   with the text the MCP `repo` parameter refuses with — compared against the real
//!   `serve --mcp` where this build serves a workspace;
//! - in a plain repository, or inside a member with no `--repo`, nothing changes: the
//!   payload carries no `member`.
//!
//! The byte-for-byte comparison of every twin's `--json` against the build before
//! this story is the implementation notes' evidence, not a test: it needs that build.
//!
//! [FR-WS-42]: ../../docs/specs/requirements/FR-WS-42.md
//! [CR-225]: ../../docs/requests/CR-225-per-member-single-root-tools.md

use std::collections::BTreeSet;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use logos_core::federation::MemberSelectionError;
use serde_json::Value;
use tempfile::TempDir;

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().expect("has parent")).expect("mkdir");
    std::fs::write(path, contents).expect("write fixture");
}

/// `logos <args>` with `GIT_CEILING_DIRECTORIES` fencing git discovery to `ceiling`.
fn logos(ceiling: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_logos"));
    command.args(args).env("GIT_CEILING_DIRECTORIES", ceiling);
    command
}

/// A repository holding the one function `symbol`: `git init`, a commit, then
/// `logos index` unless `index` is false.
fn repo(ceiling: &Path, dir: &Path, symbol: &str, index: bool) {
    write(dir, "src/lib.rs", &format!("pub fn {symbol}() -> u32 {{\n    1\n}}\n"));
    for args in [&["init", "-q"][..], &["add", "-A"], &["commit", "-q", "-m", "init"]] {
        let status = Command::new("git")
            .args(["-c", "maintenance.auto=false", "-c", "gc.auto=0", "-c", "user.email=t@t", "-c", "user.name=t"])
            .args(args)
            .current_dir(dir)
            .env("GIT_CEILING_DIRECTORIES", ceiling)
            .stdout(Stdio::null())
            .status()
            .expect("git runs");
        assert!(status.success(), "git {args:?} in {}", dir.display());
    }
    if index {
        let status = logos(ceiling, &["--project", dir.to_str().expect("utf-8")])
            .arg("index")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("logos index runs");
        assert!(status.success(), "logos index {}", dir.display());
    }
}

struct Fixture {
    tmp: TempDir,
}

impl Fixture {
    fn new() -> Self {
        let tmp = TempDir::new().expect("tempdir");
        let ceiling = tmp.path();
        write(
            &ceiling.join("ws"),
            "logos.workspace.toml",
            "[workspace]\nname = \"w\"\nmembers = [\"svc-a\", \"svc-b\", \"svc-c\"]\n",
        );
        repo(ceiling, &ceiling.join("ws/svc-a"), "alpha_only", true);
        repo(ceiling, &ceiling.join("ws/svc-b"), "beta_only", true);
        repo(ceiling, &ceiling.join("ws/svc-c"), "gamma_only", false);
        write(&ceiling.join("ws/svc-c"), ".logos/logos.db", "this is not a SQLite database\n");
        repo(ceiling, &ceiling.join("solo"), "alpha_only", true);
        Fixture { tmp }
    }

    /// The workspace root, a member, or the plain repository, by relative name.
    fn at(&self, rel: &str) -> String {
        self.tmp.path().join(rel).to_str().expect("utf-8").to_string()
    }

    /// `logos --json --project <rel> <args>`.
    fn run(&self, rel: &str, args: &[&str]) -> Output {
        logos(self.tmp.path(), &["--json", "--project", &self.at(rel)])
            .args(args)
            .output()
            .expect("logos runs")
    }

    /// The JSON payload of a command that must succeed.
    fn json(&self, rel: &str, args: &[&str]) -> Value {
        let out = self.run(rel, args);
        assert!(out.status.success(), "logos {args:?} at {rel}: {}", String::from_utf8_lossy(&out.stderr));
        serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("logos {args:?} at {rel} printed no JSON: {e}"))
    }
}

/// [CR-225] §6: a symbol only the non-default member holds is unknown without
/// `--repo` and found with it — and each answer names the member it came from.
#[test]
fn node_is_unknown_on_the_default_member_and_found_with_repo() {
    let fx = Fixture::new();

    let unscoped = fx.json("ws", &["node", "beta_only"]);
    assert_eq!(unscoped["member"], "svc-a", "the default member answers, and says so: {unscoped}");
    assert!(unscoped["node"].is_null(), "beta_only is not in the default member: {unscoped}");

    let scoped = fx.json("ws", &["node", "beta_only", "--repo", "svc-b"]);
    assert_eq!(scoped["member"], "svc-b");
    assert_eq!(scoped["node"]["name"], "beta_only", "found in svc-b: {scoped}");
    // The member stamp is first, and the answer's own fields follow unchanged.
    assert_eq!(scoped.as_object().unwrap().keys().next().map(String::as_str), Some("member"));
    assert_eq!(scoped["query"], "beta_only");

    // Inside a member the manifest is found up-tree, so a sibling is reachable too.
    let from_member = fx.json("ws/svc-a", &["node", "beta_only", "--repo", "svc-b"]);
    assert_eq!(from_member, scoped, "the same question, asked from another directory");
}

/// The twins of the listed tools — each with the arguments it needs, and the
/// symbol only `svc-b` holds where it takes one.
const TWINS: &[&[&str]] = &[
    &["search", "beta_only"],
    &["query", "beta_only"],
    &["query", "beta_only", "--callers"],
    &["query", "beta_only", "--callees"],
    &["context", "change", "beta_only", "--no-code"],
    &["explore", "beta_only"],
    &["node", "beta_only"],
    &["callers", "beta_only"],
    &["callees", "beta_only"],
    &["impact", "beta_only"],
    &["impact-intersection", "--item", "x=beta_only"],
    &["precedent", "src/lib.rs"],
    &["branch-overlap", "--ref", "HEAD"],
    &["status"],
    &["health", "--no-reconcile"],
    &["hotspots"],
    &["dsm", "--no-reconcile"],
    &["evolution"],
    &["coverage", "status"],
    &["security", "status"],
    &["security", "findings"],
    &["offenders"],
    &["review", "--base", "HEAD"],
    &["improvements"],
];

/// Every twin takes `--repo` and answers for the member it names, saying so.
#[test]
fn every_listed_twin_answers_for_the_member_repo_names() {
    let fx = Fixture::new();
    for twin in TWINS {
        let mut args = twin.to_vec();
        args.extend(["--repo", "svc-b"]);
        let payload = fx.json("ws", &args);
        assert_eq!(payload["member"], "svc-b", "logos {twin:?} --repo svc-b: {payload}");

        // Unscoped, at the workspace root: the default member, said aloud.
        let unscoped = fx.json("ws", twin);
        assert_eq!(unscoped["member"], "svc-a", "logos {twin:?} at the workspace root: {unscoped}");
    }
}

/// `--repo` is on exactly the twins of the read-only tools — and on no write or
/// gate command, whose MCP twins say they act on the default member. The `xservice`
/// and `workspace` groups carried `--repo` before this story and keep their own.
#[test]
fn only_the_twins_of_the_listed_tools_take_repo() {
    let command = logos_cli::command(&logos_cli::ExtensionHost::new());
    let mut with_repo = BTreeSet::new();
    for sub in command.get_subcommands().filter(|sub| !["xservice", "workspace"].contains(&sub.get_name())) {
        let mut visit = vec![(sub.get_name().to_string(), sub)];
        while let Some((path, cmd)) = visit.pop() {
            if cmd.get_arguments().any(|arg| arg.get_long() == Some("repo")) {
                with_repo.insert(path.clone());
            }
            visit.extend(cmd.get_subcommands().map(|child| (format!("{path} {}", child.get_name()), child)));
        }
    }
    let expected: BTreeSet<String> = TWINS.iter().map(|twin| twin_path(twin)).collect();
    assert_eq!(with_repo, expected, "the commands taking `--repo`");
}

/// The command path of a `TWINS` row: its leading words up to the first argument.
fn twin_path(twin: &[&str]) -> String {
    match twin {
        ["coverage" | "security", sub, ..] => format!("{} {sub}", twin[0]),
        [name, ..] => (*name).to_string(),
        [] => unreachable!("a twin names a command"),
    }
}

/// `scan`, `check`, `gate`, `verify`, the session gate and the coverage writers
/// refuse `--repo` as an unknown argument (usage, exit 2) — they take none.
#[test]
fn write_and_gate_commands_refuse_repo() {
    let fx = Fixture::new();
    for args in [&["scan"][..], &["check"], &["gate"], &["verify"], &["session-start"], &["session-end"], &["doctor"], &["coverage", "refresh"], &["security", "scan"], &["security", "ingest", "r.sarif"]] {
        let mut args = args.to_vec();
        args.extend(["--repo", "svc-b"]);
        let out = fx.run("ws", &args);
        assert_eq!(out.status.code(), Some(2), "logos {args:?} must refuse --repo: {}", String::from_utf8_lossy(&out.stderr));
    }
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).trim_end().to_string()
}

/// An unknown member, `--repo` with no workspace and a degraded member each refuse,
/// the first two as usage faults (exit 2) with the MCP parameter's words, the third
/// as the member's own failure with its reason.
#[test]
fn repo_refuses_an_unknown_member_a_degraded_one_and_no_workspace() {
    let fx = Fixture::new();

    let unknown = fx.run("ws", &["node", "x", "--repo", "nope"]);
    assert_eq!(unknown.status.code(), Some(2));
    let expected = MemberSelectionError::UnknownMember {
        requested: "nope".into(),
        members: vec!["svc-a".into(), "svc-b".into(), "svc-c".into()],
    };
    assert_eq!(stderr(&unknown), format!("error: {expected}"));
    assert!(unknown.stdout.is_empty(), "a refusal prints no payload");

    let alone = fx.run("solo", &["node", "x", "--repo", "svc-b"]);
    assert_eq!(alone.status.code(), Some(2));
    assert_eq!(stderr(&alone), format!("error: {}", MemberSelectionError::NoWorkspace { requested: "svc-b".into() }));

    // Refused before the (unindexed) single-root engine is asked for: the reason
    // is the workspace's absence, not a missing index.
    let unindexed = TempDir::new().expect("tempdir");
    let nowhere = logos(unindexed.path(), &["--json", "--project", unindexed.path().to_str().unwrap(), "callers", "x", "--repo", "a"])
        .output()
        .expect("logos runs");
    assert_eq!(nowhere.status.code(), Some(2));
    assert!(stderr(&nowhere).contains("no workspace is loaded"), "{}", stderr(&nowhere));

    let degraded = fx.run("ws", &["node", "x", "--repo", "svc-c"]);
    assert_eq!(degraded.status.code(), Some(3), "the member's own start failure, as a broken store is anywhere");
    assert!(
        stderr(&degraded).starts_with("error: workspace member \"svc-c\" is degraded: "),
        "names the member and the reason: {}",
        stderr(&degraded)
    );
    assert!(stderr(&degraded).contains("not a database"), "{}", stderr(&degraded));
}

/// Outside a workspace — a plain repository, or a member's own directory with no
/// `--repo` — a twin answers for the repository it runs in, with no `member`.
#[test]
fn without_a_workspace_or_repo_the_payload_carries_no_member() {
    let fx = Fixture::new();
    for rel in ["solo", "ws/svc-a"] {
        for twin in TWINS {
            let payload = fx.json(rel, twin);
            assert!(
                payload.get("member").is_none(),
                "logos {twin:?} at {rel} must not name a member: {payload}"
            );
        }
    }
    // A blank `--repo` is no `--repo`, as it is on the MCP surface.
    assert!(fx.json("solo", &["node", "alpha_only", "--repo", "  "]).get("member").is_none());
}

/// The workspace is read only for a command that asked for it: a manifest that
/// will not parse, up-tree of a repository, does not reach a twin run there with
/// no `--repo` — it answered before `--repo` existed and still does — while
/// `--repo` fails loud on it, as every command that reads the manifest does.
#[test]
fn a_broken_manifest_up_tree_reaches_only_a_command_that_asked_for_the_workspace() {
    let tmp = TempDir::new().expect("tempdir");
    let member = tmp.path().join("svc");
    repo(tmp.path(), &member, "alpha_only", true);
    write(tmp.path(), "logos.workspace.toml", "[workspace\nthis is not toml\n");
    let run = |args: &[&str]| {
        logos(tmp.path(), &["--json", "--project", member.to_str().unwrap()]).args(args).output().expect("logos runs")
    };

    let plain = run(&["node", "alpha_only"]);
    assert!(plain.status.success(), "{}", stderr(&plain));
    assert!(serde_json::from_slice::<Value>(&plain.stdout).unwrap().get("member").is_none());

    let asked = run(&["node", "alpha_only", "--repo", "svc"]);
    assert_eq!(asked.status.code(), Some(2), "a malformed manifest is a usage fault: {}", stderr(&asked));

    // A repository that itself holds a manifest — broken, or naming no git member —
    // keeps answering from its own index: a workspace with no default member to
    // answer for is not one the unscoped twin hands the question to.
    for (name, manifest) in [("broken", "[workspace\nthis is not toml\n"), ("empty", "[workspace]\nname = \"w\"\nmembers = []\n")] {
        let dir = tmp.path().join(name);
        repo(tmp.path(), &dir, "alpha_only", true);
        write(&dir, "logos.workspace.toml", manifest);
        let own = logos(tmp.path(), &["--json", "--project", dir.to_str().unwrap(), "node", "alpha_only"]).output().expect("logos runs");
        assert!(own.status.success(), "{name}: {}", stderr(&own));
        let payload: Value = serde_json::from_slice(&own.stdout).expect("JSON");
        assert_eq!(payload["node"]["name"], "alpha_only", "{name}: answered from the repository's own index");
        assert!(payload.get("member").is_none(), "{name}: {payload}");
    }
}

/// A member that was never indexed is refused as an unindexed repository is —
/// exit 3, the remedy named — and nothing is written into it: a read does not
/// create a store in a repository nobody asked to index. Reached by `--repo`, and
/// by the default member at the workspace root.
#[test]
fn an_unindexed_member_is_refused_and_left_untouched() {
    let tmp = TempDir::new().expect("tempdir");
    let ws = tmp.path().join("ws");
    write(&ws, "logos.workspace.toml", "[workspace]\nname = \"w\"\nmembers = [\"fresh\", \"other\"]\n");
    repo(tmp.path(), &ws.join("fresh"), "fresh_only", false);
    repo(tmp.path(), &ws.join("other"), "other_only", true);
    let run = |args: &[&str]| {
        logos(tmp.path(), &["--json", "--project", ws.to_str().unwrap()]).args(args).output().expect("logos runs")
    };

    for args in [&["node", "fresh_only", "--repo", "fresh"][..], &["dsm", "--repo", "fresh"], &["node", "fresh_only"]] {
        let out = run(args);
        assert_eq!(out.status.code(), Some(3), "logos {args:?}: {}", stderr(&out));
        assert!(stderr(&out).contains("no Logos index found"), "{}", stderr(&out));
        assert!(!ws.join("fresh/.logos").exists(), "logos {args:?} created a store in the member");
    }
    assert!(run(&["node", "other_only", "--repo", "other"]).status.success(), "an indexed sibling still answers");
}

/// `health --repo` projects the selected member's verdict onto the exit code: a
/// member with a dangling edge exits 1 and says which member, its healthy sibling 0.
#[test]
fn health_repo_exits_with_the_selected_members_verdict() {
    let fx = Fixture::new();
    let db = fx.tmp.path().join("ws/svc-b/.logos/logos.db");
    let sql = Command::new("sqlite3")
        .arg(&db)
        .arg("PRAGMA foreign_keys=off; DELETE FROM nodes WHERE name='beta_only';")
        .status();
    if !sql.map(|s| s.success()).unwrap_or(false) {
        eprintln!("sqlite3 unavailable: cannot corrupt a member store; skipping the unhealthy half");
        return;
    }
    let unhealthy = fx.run("ws", &["health", "--no-reconcile", "--repo", "svc-b"]);
    assert_eq!(unhealthy.status.code(), Some(1), "{}", stderr(&unhealthy));
    let report: Value = serde_json::from_slice(&unhealthy.stdout).expect("JSON");
    assert_eq!((report["member"].as_str(), report["ok"].as_bool()), (Some("svc-b"), Some(false)), "{report}");
    assert!(fx.run("ws", &["health", "--no-reconcile", "--repo", "svc-a"]).status.success(), "the healthy member exits 0");
}

/// At the workspace root the default member answers the read-only twins, but
/// the write and gate commands do not borrow it: there is still no index there
/// for them to act on, so they refuse as they did.
#[test]
fn the_workspace_root_answers_reads_but_gates_nothing() {
    let fx = Fixture::new();
    let gate = fx.run("ws", &["doctor"]);
    assert_eq!(gate.status.code(), Some(3), "{}", stderr(&gate));
    assert!(stderr(&gate).contains("no Logos index found"), "{}", stderr(&gate));
}

/// What the real `serve --mcp` refuses a bad `repo` with: each probe's protocol
/// error message, in order. The slim build serves one repository, so only the
/// default build answers for a workspace.
#[cfg(feature = "ui")]
fn mcp_refusals(fx: &Fixture, rel: &str, repos: &[&str]) -> Vec<String> {
    use std::io::{BufRead, BufReader, Write};
    use std::time::{Duration, Instant};

    let mut child = logos(fx.tmp.path(), &["--project", &fx.at(rel), "serve", "--mcp"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn logos serve --mcp");
    let mut stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Ok(value) = serde_json::from_str::<Value>(&line) {
                if tx.send(value).is_err() {
                    break;
                }
            }
        }
    });
    let mut messages = vec![
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "s-711", "version": "0"}}}),
        serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
    ];
    for (id, repo) in (2u64..).zip(repos) {
        messages.push(serde_json::json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": {"name": "node", "arguments": {"symbol": "x", "repo": repo}}}));
    }
    for message in &messages {
        writeln!(stdin, "{message}").expect("write request");
    }
    stdin.flush().expect("flush");

    let mut refusals = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(60);
    while refusals.len() < repos.len() {
        let left = deadline.saturating_duration_since(Instant::now());
        let value = rx.recv_timeout(left).expect("a response within 60s");
        if value["id"].as_u64() >= Some(2) {
            let message = value["error"]["message"].as_str().unwrap_or_else(|| panic!("expected a refusal: {value}"));
            refusals.push(message.to_string());
        }
    }
    drop(stdin);
    let _ = child.wait();
    refusals
}

/// The CLI refuses with the MCP probe's text: an unknown member and `repo` with no
/// workspace word for word, a degraded member with the same cause after the tool's
/// own prefix.
#[cfg(feature = "ui")]
#[test]
fn the_cli_refuses_in_the_words_the_mcp_probe_does() {
    let fx = Fixture::new();
    let cli = |rel: &str, repo: &str| stderr(&fx.run(rel, &["node", "x", "--repo", repo])).trim_start_matches("error: ").to_string();

    let mcp = mcp_refusals(&fx, "ws", &["nope", "svc-c"]);
    assert_eq!(cli("ws", "nope"), mcp[0], "unknown member");
    assert_eq!(format!("logos:node failed: {}", cli("ws", "svc-c")), mcp[1], "degraded member");

    let mcp = mcp_refusals(&fx, "solo", &["svc-b"]);
    assert_eq!(cli("solo", "svc-b"), mcp[0], "no workspace");
}
