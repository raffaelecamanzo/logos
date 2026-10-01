//! End-to-end tests for `logos xservice type-refs` and the type-reference
//! stitch in `xservice callers` / `impact` (S-474, [FR-WS-05], [FR-WS-35],
//! [BR-60]), driven through the **real** `logos` binary over members indexed
//! with Java sources and Maven poms.
//!
//! `lib` (`com.acme:lib`) declares `com.acme.lib.Dto`; `app` depends on it and
//! imports it in `App.java` (bound), which `app`'s own `Main.java` depends on;
//! `stray` imports it and builds against nothing (type-only). Beside them,
//! `web` serves a route `api`'s OpenAPI document declares, so the bridge tier
//! the stitch must leave alone is non-empty.
//!
//! [FR-WS-05]: ../../docs/specs/requirements/FR-WS-05.md
//! [FR-WS-35]: ../../docs/specs/requirements/FR-WS-35.md
//! [BR-60]: ../../docs/specs/software-spec.md#327-workspace-federation
#![cfg(feature = "lang-all")]

use std::path::Path;
use std::process::{Command, Output};

use serde_json::{json, Value};
use tempfile::TempDir;

const APP_JAVA: &str = "\
package com.acme.app;

import com.acme.lib.Dto;

public class App {
    public String run(Dto dto) { return dto.toString(); }
}
";

const MAIN_JAVA: &str = "\
package com.acme.app;

import com.acme.app.App;

public class Main {
    public static void main(String[] args) { new App().run(null); }
}
";

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
            ("app", true) => {
                write(&dir, "src/main/java/com/acme/app/App.java", APP_JAVA);
                write(&dir, "src/main/java/com/acme/app/Main.java", MAIN_JAVA);
                write(&dir, "pom.xml", &pom("app", &["lib"]));
            }
            (importer, true) => {
                let class = "Stray";
                write(
                    &dir,
                    &format!("src/main/java/com/acme/{importer}/{class}.java"),
                    &format!("package com.acme.{importer};\n\nimport com.acme.lib.Dto;\n\npublic class {class} {{}}\n"),
                );
                write(&dir, "pom.xml", &pom(importer, &[]));
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

const DTO_NODE: &str = "logos . . . src/main/java/com/acme/lib/`Dto.java`/Dto#";

/// **The `--json` snapshot** ([FR-WS-35]): the one provider, its one imported
/// type with its declaration, and its one importer with file and line —
/// stray's type-only match is not an importer — under the headline
/// `workspace status` carries; the human rendering is the same read-model.
///
/// [FR-WS-35]: ../../docs/specs/requirements/FR-WS-35.md
#[test]
fn type_refs_lists_each_provider_type_with_its_importers_under_the_headline() {
    let tmp = workspace(&MEMBERS, true);
    let answer = logos_json(tmp.path(), &["xservice", "type-refs"]);
    assert_eq!(
        answer["providers"],
        json!([{
            "member": "lib",
            "types": [{
                "fqn": "com.acme.lib.Dto",
                "owner": {
                    "member": "lib",
                    "origin": "source",
                    "declared_in": "src/main/java/com/acme/lib/Dto.java",
                    "symbol": DTO_NODE,
                    "kind": "class"
                },
                "importers": [{
                    "member": "app",
                    "file": "src/main/java/com/acme/app/App.java",
                    "line": 3,
                    "symbol": "logos . . . src/main/java/com/acme/app/`App.java`/",
                    "naming": "exact",
                    "form": "import",
                    "evidence": {"via": "build"}
                }]
            }]
        }]),
        "{answer:#}"
    );
    let status = logos_json(tmp.path(), &["workspace", "status"]);
    assert_eq!(answer["headline"], status["type_reference"], "one headline, two commands");
    assert_eq!(answer["headline"]["type_reference_pairs"], 1);
    assert!(answer.get("scope").is_none() && answer.get("scope_note").is_none(), "{answer:#}");

    let (_, human) = logos_human(tmp.path(), &["xservice", "type-refs"]);
    assert_eq!(human, answer, "human and --json print one read-model");
}

/// **`--repo`** scopes the listing to one provider and keeps the headline
/// workspace-wide; a name that is no member is a `scope_note` and exit 0,
/// never an error ([NFR-CC-04]).
///
/// [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md
#[test]
fn a_repo_naming_a_non_member_returns_a_scope_note_not_an_error() {
    let tmp = workspace(&MEMBERS, true);
    let all = logos_json(tmp.path(), &["xservice", "type-refs"]);
    let lib = logos_json(tmp.path(), &["xservice", "type-refs", "--repo", "lib"]);
    assert_eq!((&lib["scope"], &lib["providers"]), (&json!("lib"), &all["providers"]), "{lib:#}");
    assert_eq!(lib["headline"], all["headline"]);

    let nope = logos_json(tmp.path(), &["xservice", "type-refs", "--repo", "nope"]);
    assert_eq!(nope["providers"], json!([]), "{nope:#}");
    assert_eq!(
        nope["scope_note"],
        "`nope` is not a member the type-reference overlay was built over (not in the workspace)"
    );
    assert_eq!(nope["headline"], all["headline"], "the denominator is still stated");
}

/// **The stitch on the CLI**: `callers` and `impact` on `Dto`'s node carry a
/// `via_type_reference` section tagged `via type reference`, apart from the
/// bridge tier; `impact` reaches `Main.java` through the importing file. A
/// symbol no type reference names prints no such key.
#[test]
fn callers_and_impact_stitch_type_references_apart_from_the_bridge_tier() {
    let tmp = workspace(&MEMBERS, true);
    for tool in ["callers", "impact"] {
        let answer = logos_json(tmp.path(), &["xservice", tool, DTO_NODE]);
        let section = answer["via_type_reference"].as_array().unwrap_or_else(|| panic!("{tool}: {answer:#}"));
        assert_eq!(section.len(), 1, "{tool}: {answer:#}");
        assert_eq!(section[0]["reached"], "via type reference", "{tool}");
        assert_eq!(section[0]["via"]["importer"]["file"], "src/main/java/com/acme/app/App.java", "{tool}");
        assert_eq!(answer["cross_service"], json!([]), "{tool}: nothing bridge-reached");

        let none = logos_json(tmp.path(), &["xservice", tool, "get_user"]);
        assert!(none.get("via_type_reference").is_none(), "{tool}: {none:#}");
    }
    let impact = logos_json(tmp.path(), &["xservice", "impact", DTO_NODE]);
    assert_eq!(
        impact["via_type_reference"][0]["result"]["affected"][0]["file"],
        "src/main/java/com/acme/app/Main.java",
        "{impact:#}"
    );
}
