//! The `rules.toml [review]` policy through the shipped binary (S-754,
//! [FR-GV-25], [FR-GV-01]): an invalid table is exit 2 on every command that
//! loads the contract, naming the key; a valid one fails `gate --json` with
//! exit 1 and the gating entry, and `review --json` carries the bytes the Stop
//! hook reads. A re-bless past `drift_budget` without `--reason` is exit 2
//! too (S-755). A trend warning (S-756) moves no exit code of `scan`, `gate`
//! or `review`, and `gate --json` carries it as a structured `trend` too
//! (S-785). The verdicts themselves are `logos-core/tests/review_policy.rs`.
//!
//! [FR-GV-25]: ../../docs/specs/requirements/FR-GV-25.md
//! [FR-GV-01]: ../../docs/specs/requirements/FR-GV-01.md

#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;
use tempfile::TempDir;

fn git(root: &Path, args: &[&str]) {
    let out = Command::new("git")
        .current_dir(root)
        .env("GIT_CEILING_DIRECTORIES", root.parent().unwrap())
        .args([
            "-c", "maintenance.auto=false", "-c", "gc.auto=0", "-c", "user.email=t@logos", "-c", "user.name=t",
            "-c", "commit.gpgsign=false", "-c", "core.hooksPath=",
        ])
        .args(args)
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

/// `name` nesting `depth` `if`s.
fn nested(name: &str, depth: usize) -> String {
    let open: String = (0..depth).map(|i| format!("    if x > {i} {{\n")).collect();
    format!("pub fn {name}(x: i32) -> i32 {{\n{open}return x;\n{}    0\n}}\n", "    }\n".repeat(depth))
}

struct World {
    _tmp: TempDir,
    root: PathBuf,
    user: PathBuf,
}

impl World {
    /// A Rust repository on `main`, indexed, with the gate's baseline saved.
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let (root, user) = (base.join("project"), base.join("user"));
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(&user).unwrap();
        fs::write(root.join(".gitignore"), ".logos/\n").unwrap();
        fs::write(root.join("src/lib.rs"), nested("alpha", 2)).unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        git(&root, &["add", "."]);
        git(&root, &["commit", "-q", "-m", "seed"]);
        let w = Self { _tmp: tmp, root, user };
        assert!(w.logos(&["index"]).status.success());
        assert_eq!(w.logos(&["gate", "--save"]).status.code(), Some(0));
        w
    }

    fn rules(&self, text: &str) {
        fs::write(self.root.join(".logos/rules.toml"), text).unwrap();
    }

    fn logos(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_logos"))
            .arg("--project")
            .arg(&self.root)
            .args(args)
            .env("LOGOS_CONFIG_HOME", &self.user)
            .env("LOGOS_DATA_HOME", &self.user)
            .env("GIT_CEILING_DIRECTORIES", self.root.parent().unwrap())
            .output()
            .expect("the logos binary runs")
    }
}

/// [FR-GV-25] acceptance 5: an unknown key, a distributional dimension, an
/// unknown dimension and a negative budget each exit 2 from `check`, `gate`
/// and `session-end`, naming what is wrong; the near miss loads.
#[test]
fn an_invalid_review_table_exits_2_naming_the_key() {
    let w = World::new();
    for (table, says) in [
        ("[review]\ntrend_windw = 2\n", "trend_windw"),
        ("[review]\ngate_dimensions = [\"equality\"]\n", "review.gate_dimensions[0]"),
        ("[review]\ngate_dimensions = [\"nesting\", \"depth\"]\n", "review.gate_dimensions[1]"),
        ("[review]\ngate_dimensions = [\"complexity\"]\n", "review.gate_dimensions[0]"),
        ("[review]\ndrift_budget = -1\n", "review.drift_budget"),
        ("[review]\ndrift_budget = \"reprt\"\n", "drift_budget"),
        ("[review]\naccept = \"human\"\n", "accept"),
        ("[review]\ntrend_window = 0\n", "review.trend_window"),
        ("[review]\ntrend_window = -1\n", "trend_window"),
    ] {
        w.rules(table);
        for command in [&["check"][..], &["gate"], &["session-end"]] {
            let out = w.logos(command);
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert_eq!(out.status.code(), Some(2), "{command:?} over {table:?}: {stderr}");
            assert!(stderr.contains(says), "{command:?} over {table:?} names {says}: {stderr}");
        }
    }
    w.rules("[review]\ngate_dimensions = [\"nesting\"]\ndrift_budget = 2\naccept = \"owner\"\ntrend_window = 1\n");
    for command in [&["check"][..], &["gate"], &["session-end"]] {
        let out = w.logos(command);
        assert_ne!(out.status.code(), Some(2), "{command:?}: {}", String::from_utf8_lossy(&out.stderr));
    }
}

/// [UAT-GV-15] step 1 through the binary: a branch adding a 6-deep function
/// fails `gate --json` with exit 1 and the entry, value 6, threshold 4, trust
/// `resolved`.
#[test]
fn a_gating_entry_is_exit_1_with_the_entry_in_the_json() {
    let w = World::new();
    w.rules("[review]\n");
    git(&w.root, &["checkout", "-q", "-b", "deep"]);
    fs::write(w.root.join("src/lib.rs"), format!("{}{}", nested("alpha", 2), nested("beta_depth_six", 6))).unwrap();
    git(&w.root, &["commit", "-qam", "deep"]);

    let out = w.logos(&["--json", "gate"]);
    assert_eq!(out.status.code(), Some(1), "{}", String::from_utf8_lossy(&out.stderr));
    let gate: Value = serde_json::from_slice(&out.stdout).unwrap();
    let entry = &gate["gating_entries"][0];
    assert_eq!(
        (&entry["name"], &entry["after"], &entry["threshold"], &entry["trust"], &entry["status"]),
        (&Value::from("beta_depth_six"), &Value::from(6), &Value::from(4), &Value::from("resolved"), &Value::from("new")),
        "{gate}"
    );
    assert_eq!(gate["gating_entries"].as_array().map(Vec::len), Some(1), "{gate}");

    // The Stop hook reads `review --json` by these exact bytes (stop.sh
    // `leg_review` / `review_entries`): the policy object opening with its
    // dimensions, and the entry's flag.
    let out = w.logos(&["--json", "review"]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let review = String::from_utf8_lossy(&out.stdout);
    assert!(review.contains(r#""policy":{"gate_dimensions":"#), "{review}");
    assert_eq!(review.matches(r#""gating":true"#).count(), 1, "{review}");
}

/// [UAT-GV-16] steps 2 and 3 through the binary (S-755, [FR-GV-16]): a
/// re-bless past `drift_budget` without `--reason` exits 2 and names the flag;
/// `--reason` without `--save` is a usage error; with it the save exits 0 and
/// carries the reason and the attribution of the change it blessed.
///
/// [FR-GV-16]: ../../docs/specs/requirements/FR-GV-16.md
#[test]
fn a_rebless_past_the_budget_exits_2_without_a_reason() {
    let w = World::new();
    w.rules("[review]\ndrift_budget = 0\n");
    git(&w.root, &["checkout", "-q", "-b", "deep"]);
    fs::write(w.root.join("src/lib.rs"), format!("{}{}", nested("alpha", 2), nested("beta_depth_six", 6))).unwrap();
    git(&w.root, &["commit", "-qam", "deep"]);

    let refused = w.logos(&["--json", "gate", "--save"]);
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert_eq!(refused.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("past drift_budget 0") && stderr.contains("--reason"), "{stderr}");

    let unsaved = w.logos(&["gate", "--reason", "why"]);
    assert_eq!(unsaved.status.code(), Some(2), "--reason requires --save");

    let out = w.logos(&["--json", "gate", "--save", "--reason", "the deep branch is intrinsic"]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let saved: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(saved["baseline_reason"], "the deep branch is intrinsic", "{saved}");
    assert!(saved["attribution"]["change"].as_i64().is_some_and(|c| c < 0), "{saved}");
    assert!(saved["attribution"]["contributions"].as_array().is_some_and(|c| !c.is_empty()), "{saved}");
}

/// `name` with `n` sequential `if`s: cyclomatic complexity `n + 1`.
fn branches(name: &str, n: usize) -> String {
    let body: String = (0..n).map(|i| format!("    if x == {i} {{\n        return {i};\n    }}\n")).collect();
    format!("pub fn {name}(x: i32) -> i32 {{\n{body}    x\n}}\n")
}

/// The trend warnings of a `--json` answer.
fn trend(answer: &Value) -> Vec<String> {
    answer["warnings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|w| w.contains("review.trend_window"))
        .map(str::to_string)
        .collect()
}

/// [FR-GV-27] acceptance 2 through the binary (S-756): with Equality falling
/// at each `scan`, the warning reaches the JSON of `scan`, `gate` and
/// `review`, and each exits as it does under a window the run does not fill.
///
/// [FR-GV-27]: ../../docs/specs/requirements/FR-GV-27.md
#[test]
fn a_trend_warning_moves_no_exit_code() {
    let w = World::new();
    w.rules("[review]\n");
    // Uniform, as the one seed function is, so replacing it is no decline.
    let base: String = (0..40).map(|i| branches(&format!("f{i}"), 2)).collect();
    for extra in 0..3 {
        let added: String = (0..extra).map(|i| branches(&format!("g{i}"), 6 + i)).collect();
        fs::write(w.root.join("src/lib.rs"), format!("{base}{added}")).unwrap();
        let out = w.logos(&["--json", "scan"]);
        assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
        let scan: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(trend(&scan).len(), usize::from(extra == 2), "scan {extra}: {scan}");
    }

    let exits = |window: &str| {
        w.rules(&format!("[review]\ntrend_window = {window}\n"));
        ["gate", "review"].map(|command| {
            let out = w.logos(&["--json", command]);
            let answer: Value = serde_json::from_slice(&out.stdout).unwrap();
            (out.status.code(), trend(&answer).len())
        })
    };
    let warned = exits("2");
    let quiet = exits("99");
    assert_eq!(warned.map(|(_, n)| n), [1, 1], "{warned:?}");
    assert_eq!(quiet.map(|(_, n)| n), [0, 0], "{quiet:?}");
    assert_eq!(warned.map(|(code, _)| code), quiet.map(|(code, _)| code), "the same exit codes");
}

/// [CR-243] through the binary (S-785): `gate --json` carries the structured
/// `trend` — the dimension, the Equality each `scan --json` reported, oldest
/// first, and the window — beside the warning spelled from it; without the
/// table the key is absent.
///
/// [CR-243]: ../../docs/requests/CR-243-governance-signals-reach-every-surface.md
#[test]
fn gate_json_carries_the_structured_trend_beside_its_warning() {
    let w = World::new();
    w.rules("[review]\n");
    let base: String = (0..40).map(|i| branches(&format!("f{i}"), 2)).collect();
    let mut values = Vec::new();
    for extra in 0..3 {
        let added: String = (0..extra).map(|i| branches(&format!("g{i}"), 6 + i)).collect();
        fs::write(w.root.join("src/lib.rs"), format!("{base}{added}")).unwrap();
        let out = w.logos(&["--json", "scan"]);
        let scan: Value = serde_json::from_slice(&out.stdout).unwrap();
        values.push(scan["metrics"]["equality"]["normalized"].clone());
    }

    let out = w.logos(&["--json", "gate"]);
    let gate: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(gate["trend"], serde_json::json!([{"dimension": "equality", "values": values, "window": 2}]), "{gate}");
    // The warning's values are the trend's, compared as printed: a parsed JSON
    // float may read back one bit off, so the text is the evidence.
    let warning = &trend(&gate)[0];
    let chain = warning.split(": ").nth(1).and_then(|c| c.split(" (").next()).unwrap_or_default();
    let printed: Vec<String> = chain
        .split(" → ")
        .map(|v| serde_json::to_string(&v.parse::<f64>().expect("a number")).unwrap())
        .collect();
    assert_eq!(printed.len(), 3, "{warning}");
    let raw: String = String::from_utf8(out.stdout).unwrap().split_whitespace().collect();
    assert!(raw.contains(&format!("\"values\":[{}],\"window\":2", printed.join(","))), "{warning} vs {raw}");

    fs::remove_file(w.root.join(".logos/rules.toml")).unwrap();
    let out = w.logos(&["--json", "gate"]);
    let gate: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(gate.get("trend").is_none() && trend(&gate).is_empty(), "{gate}");
}
