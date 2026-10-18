//! `logos offenders` end to end through the shipped binary (S-676,
//! [FR-QM-19]): the uncapped listing past the display cap, byte-identical
//! when run twice, "not recorded" before any scan, and exit 2 for a bad
//! dimension or glob. The MCP side of the same payload is
//! `mcp/tests/offenders_parity.rs`.
//!
//! [FR-QM-19]: ../../docs/specs/requirements/FR-QM-19.md

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;
use tempfile::TempDir;

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// `name` nesting `depth` `if`s.
fn nested(name: &str, depth: usize) -> String {
    let open: String = (0..depth).map(|i| format!("if x > {i} {{\n")).collect();
    format!("pub fn {name}(x: i32) -> i32 {{\n{open}return x;\n{}0\n}}\n", "}\n".repeat(depth))
}

/// The binary over `root`, never escaping into an enclosing repository.
fn logos(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_logos"))
        .arg("--project")
        .arg(root)
        .args(args)
        .env("GIT_CEILING_DIRECTORIES", root.parent().unwrap())
        .output()
        .expect("logos runs")
}

/// Fourteen deep functions under `src/a` and two in `src/ab`.
fn fixture() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let a: String = (0..14).map(|i| nested(&format!("a{i:02}"), 4 + i % 3)).collect();
    write(root, "src/lib.rs", "pub mod a;\npub mod ab;\n");
    write(root, "src/a/mod.rs", &a);
    write(root, "src/ab/mod.rs", &format!("{}{}", nested("ab0", 5), nested("ab1", 4)));
    let out = logos(root, &["index"]);
    assert!(out.status.success(), "index: {}", String::from_utf8_lossy(&out.stderr));
    tmp
}

#[test]
fn offenders_lists_every_match_byte_identically_and_refuses_bad_tokens_with_exit_2() {
    let tmp = fixture();
    let root = tmp.path();

    let never = logos(root, &["--json", "offenders"]);
    assert!(never.status.success(), "{}", String::from_utf8_lossy(&never.stderr));
    let never: Value = serde_json::from_slice(&never.stdout).unwrap();
    assert_eq!(never["recorded"], false, "{never}");
    assert_eq!(never["not_recorded_because"], "never_scanned", "{never}");

    let scan = logos(root, &["--json", "scan"]);
    assert!(scan.status.success(), "scan: {}", String::from_utf8_lossy(&scan.stderr));

    let args = ["--json", "offenders", "--path", "src/a/**", "--dimension", "nesting"];
    let (first, second) = (logos(root, &args), logos(root, &args));
    assert!(first.status.success(), "{}", String::from_utf8_lossy(&first.stderr));
    assert_eq!(first.stdout, second.stdout, "a rerun prints the same bytes");
    let listing: Value = serde_json::from_slice(&first.stdout).unwrap();
    let entries = listing["offenders"].as_array().unwrap();
    assert_eq!(entries.len(), 14, "all fourteen, past the display cap: {listing}");
    assert!(entries.iter().all(|o| o["file"].as_str().unwrap().starts_with("src/a/")), "{listing}");
    assert!(
        entries.iter().all(|o| o["value"].as_i64() >= o["threshold"].as_i64() && o["threshold"] == 4),
        "{listing}"
    );

    for bad in [&["offenders", "--dimension", "modularity"][..], &["offenders", "--path", "../x/**"]] {
        let out = logos(root, bad);
        assert_eq!(out.status.code(), Some(2), "logos {bad:?}: {}", String::from_utf8_lossy(&out.stderr));
    }
}
