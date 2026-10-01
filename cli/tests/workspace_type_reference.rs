//! End-to-end tests for the `type_reference` section of `logos workspace
//! status` (S-473, [FR-WS-35], [BR-60]), driven through the **real** `logos`
//! binary over members indexed with Java sources and Maven poms.
//!
//! `lib` (`com.acme:lib`) declares `com.acme.lib.Dto`; `app` depends on it and
//! imports it (bound); `stray` imports it and builds against nothing
//! (type-only). Beside them, `web` serves a route `api`'s OpenAPI document
//! declares, so the runtime figures the overlay must never move are
//! non-trivial.
//!
//! [FR-WS-35]: ../../docs/specs/requirements/FR-WS-35.md
//! [BR-60]: ../../docs/specs/software-spec.md#327-workspace-federation
#![cfg(feature = "lang-all")]

use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;
use tempfile::TempDir;

const OPENAPI_YAML: &str = "\
openapi: 3.0.3
info:
  title: User API
  version: 1.0.0
paths:
  /users/{user_id}:
    get:
      summary: Get a user
";

const AXUM_MAIN: &str = r#"
use axum::routing::get;
use axum::Router;

async fn get_user() {}

fn app() -> Router {
    Router::new().route("/users/{id}", get(get_user))
}
"#;

fn pom(artifact: &str, dependencies: &[&str]) -> String {
    let deps: String = dependencies
        .iter()
        .map(|d| format!("    <dependency><groupId>com.acme</groupId><artifactId>{d}</artifactId></dependency>\n"))
        .collect();
    format!(
        "<project>\n  <groupId>com.acme</groupId>\n  <artifactId>{artifact}</artifactId>\n  \
         <version>1</version>\n  <dependencies>\n{deps}  </dependencies>\n</project>\n"
    )
}

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().expect("has parent")).expect("mkdir");
    std::fs::write(path, contents).expect("write fixture");
}

fn sh_git(cwd: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["-c", "user.email=test@logos", "-c", "user.name=logos-test"])
        .args(args)
        .output()
        .expect("git is on PATH");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

fn logos(project: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_logos"))
        .arg("--project")
        .arg(project)
        .args(args)
        .output()
        .expect("the logos binary runs")
}

/// `logos <args> --json`, asserting exit 0 and one machine-clean JSON line.
fn logos_json(project: &Path, args: &[&str]) -> Value {
    let mut full = args.to_vec();
    full.push("--json");
    let out = logos(project, &full);
    assert!(out.status.success(), "logos {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8(out.stdout).expect("utf8 stdout");
    assert_eq!(stdout.trim().lines().count(), 1, "--json is one line: {stdout}");
    serde_json::from_str(stdout.trim()).expect("--json stdout is JSON")
}

/// The human rendering: the same read-model, pretty-printed.
fn logos_human(project: &Path, args: &[&str]) -> (String, Value) {
    let out = logos(project, args);
    assert!(out.status.success(), "logos {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8(out.stdout).expect("utf8 stdout");
    let value = serde_json::from_str(&stdout).expect("the human rendering is the read-model");
    (stdout, value)
}

/// Index `members` through the real binary; `jvm = false` gives the JVM
/// members one Rust file and no pom instead.
fn workspace(members: &[&str], jvm: bool) -> TempDir {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    for name in members {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        sh_git(&dir, &["init", "-q", "-b", "main"]);
        match (*name, jvm) {
            ("api", _) => write(&dir, "api/openapi.yaml", OPENAPI_YAML),
            ("web", _) => write(&dir, "src/main.rs", AXUM_MAIN),
            ("lib", true) => {
                write(&dir, "src/main/java/com/acme/lib/Dto.java", "package com.acme.lib;\n\npublic class Dto {}\n");
                write(&dir, "pom.xml", &pom("lib", &[]));
            }
            (importer, true) => {
                let class = if importer == "app" { "App" } else { "Stray" };
                write(
                    &dir,
                    &format!("src/main/java/com/acme/{importer}/{class}.java"),
                    &format!("package com.acme.{importer};\n\nimport com.acme.lib.Dto;\n\npublic class {class} {{}}\n"),
                );
                let deps: &[&str] = if importer == "app" { &["lib"] } else { &[] };
                write(&dir, "pom.xml", &pom(importer, deps));
            }
            _ => write(&dir, "src/lib.rs", "pub fn f() {}\n"),
        }
        sh_git(&dir, &["add", "."]);
        sh_git(&dir, &["commit", "-q", "-m", "init"]);
        assert!(logos(&dir, &["index"]).status.success(), "index {name}");
    }
    let list = members.iter().map(|m| format!("\"{m}\"")).collect::<Vec<_>>().join(", ");
    std::fs::write(
        root.join("logos.workspace.toml"),
        format!("[workspace]\nname = \"acme\"\nmembers = [{list}]\ndefault = \"api\"\n"),
    )
    .unwrap();
    tmp
}

const MEMBERS: [&str; 5] = ["lib", "app", "stray", "api", "web"];

/// `workspace status`, human and `--json`, carries `type_reference_pairs`
/// beside the rows considered, bound, ambiguous-owner and type-only and the
/// members read, as its own section after the build section — and the
/// runtime sections carry none of it ([BR-51], [BR-60]).
///
/// [BR-51]: ../../docs/specs/software-spec.md#327-workspace-federation
/// [BR-60]: ../../docs/specs/software-spec.md#327-workspace-federation
#[test]
fn workspace_status_states_type_reference_pairs_beside_the_denominators_in_both_renderings() {
    let tmp = workspace(&MEMBERS, true);
    let status = logos_json(tmp.path(), &["workspace", "status"]);
    let section = &status["type_reference"];
    assert_eq!(section["type_reference_pairs"], 1, "app→lib: {section}");
    let rows = &section["rows"];
    for (bucket, want) in [("considered", 2), ("bound", 1), ("type_only", 1), ("ambiguous_owner", 0)] {
        assert_eq!(rows[bucket], want, "{bucket}: {rows}");
    }
    assert_eq!((&section["members"]["read"], &section["members"]["members"]), (&5.into(), &5.into()));
    assert_eq!(
        section["type_only"],
        serde_json::json!([{"from": "stray", "to": "lib", "types": ["com.acme.lib.Dto"], "references": 1}])
    );
    let summary = section["summary"].as_str().unwrap();
    assert!(
        summary.starts_with("1 member pairs (1 build · 0 collision-backed) bind 1 of 2 unresolved"),
        "{summary}"
    );
    assert!(summary.contains("never a coupling"), "{summary}");
    for runtime in ["coverage", "build_dependency"] {
        assert!(
            !status[runtime].to_string().contains("type_reference"),
            "no type-reference figure inside `{runtime}`: {}",
            status[runtime]
        );
    }

    let (text, human) = logos_human(tmp.path(), &["workspace", "status"]);
    assert_eq!(human["type_reference"], *section, "human and --json carry one section");
    let (build_at, types_at) =
        (text.find("\"build_dependency\"").unwrap(), text.find("\"type_reference\"").unwrap());
    assert!(build_at < types_at, "the type section follows the build one, apart from it");
}

/// A workspace with no Java/Kotlin/Avro member prints `workspace status` with
/// no `type_reference` key in either rendering.
#[test]
fn a_workspace_with_no_java_kotlin_or_avro_member_prints_no_type_reference_section() {
    let tmp = workspace(&MEMBERS, false);
    let status = logos_json(tmp.path(), &["workspace", "status"]);
    assert!(status.get("type_reference").is_none(), "{status}");
    let (text, _) = logos_human(tmp.path(), &["workspace", "status"]);
    assert!(!text.contains("type_reference"), "{text}");
}
