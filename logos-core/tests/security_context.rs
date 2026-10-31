//! A finding in graph context, end to end over a committed fixture project
//! (S-669, [FR-SC-13], [UAT-SC-03]): its enclosing symbol, whether a `route`
//! reaches it over call edges, and its blast radius, each with a confidence
//! label read from the per-language resolution denominator.
//!
//! The fixture (`tests/fixtures/security-context/`) holds an Axum crate and a
//! Spring controller — each calling into a second file, so each language has a
//! cross-file `calls` figure, the shape a real handler has — a one-file Python
//! script, whose language therefore resolves no call across files, and a
//! `Cargo.lock`, which the index excludes by default. Two reports point into
//! it: a Semgrep one into code and an osv-scanner one into the lockfile. The
//! project is rebuilt in a temporary git repository outside every work tree.
//!
//! [FR-SC-13]: ../../docs/specs/requirements/FR-SC-13.md
//! [UAT-SC-03]: ../../docs/specs/requirements/UAT-SC-03.md

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use logos_core::security::{FindingsFilter, ScanEnvironment};
use logos_core::Engine;
use serde_json::Value;
use tempfile::TempDir;

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/security-context")
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn git(root: &Path, args: &[&str]) {
    let out = Command::new("git")
        .current_dir(root)
        .env("GIT_CEILING_DIRECTORIES", root.parent().unwrap())
        .args(["-c", "maintenance.auto=false", "-c", "gc.auto=0", "-c", "user.email=t@logos", "-c", "user.name=t"])
        .args(args)
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

struct Project {
    _tmp: TempDir,
    root: PathBuf,
    env: ScanEnvironment,
}

/// The fixture project, committed, indexed, with both reports ingested.
fn project() -> Project {
    let tmp = TempDir::new().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let (root, user, bin) = (base.join("project"), base.join("user"), base.join("bin"));
    for dir in ["axum", "spring", "scripts"] {
        copy_tree(&fixture_dir().join(dir), &root.join(dir));
    }
    for dir in [&root.join(".logos"), &user, &bin] {
        fs::create_dir_all(dir).unwrap();
    }
    fs::write(root.join(".logos/config.toml"), "[project]\nid = \"security-context-fixture\"\n").unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "seed"]);
    let env = ScanEnvironment {
        user_dir: Some(user),
        search_path: Some(bin.into_os_string()),
        tools_dir: None,
        nvd_api_key: None,
    };
    let p = Project { _tmp: tmp, root, env };
    let engine = p.engine();
    engine.index();
    for report in ["semgrep.sarif", "osv-scanner.sarif"] {
        engine.security_ingest(&fixture_dir().join(report)).expect("the report ingests");
    }
    p
}

/// A project from inline `files`, committed and indexed, with one Semgrep
/// report holding a finding at each `(path, line)` of `findings` ingested.
fn inline_project(files: &[(&str, &str)], findings: &[(&str, i64)]) -> Project {
    let tmp = TempDir::new().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let (root, user, bin) = (base.join("project"), base.join("user"), base.join("bin"));
    for (path, body) in files {
        let file = root.join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, body).unwrap();
    }
    for dir in [&root.join(".logos"), &user, &bin] {
        fs::create_dir_all(dir).unwrap();
    }
    fs::write(root.join(".logos/config.toml"), "[project]\nid = \"security-context-inline\"\n").unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "seed"]);
    let results: Vec<Value> = findings
        .iter()
        .map(|(path, line)| {
            serde_json::json!({
                "ruleId": "inline.rule",
                "message": {"text": "An inline finding."},
                "locations": [{"physicalLocation": {
                    "artifactLocation": {"uri": path},
                    "region": {"startLine": line, "endLine": line}
                }}]
            })
        })
        .collect();
    let report = base.join("inline.sarif");
    let sarif = serde_json::json!({"version": "2.1.0", "runs": [{
        "tool": {"driver": {"name": "Semgrep OSS", "semanticVersion": "1.85.0", "rules": [{"id": "inline.rule"}]}},
        "results": results
    }]});
    fs::write(&report, sarif.to_string()).unwrap();
    let env = ScanEnvironment {
        user_dir: Some(user),
        search_path: Some(bin.into_os_string()),
        tools_dir: None,
        nvd_api_key: None,
    };
    let p = Project { _tmp: tmp, root, env };
    let engine = p.engine();
    engine.index();
    engine.security_ingest(&report).expect("the report ingests");
    p
}

impl Project {
    fn engine(&self) -> Engine {
        Engine::start(&self.root).expect("engine").with_scan_environment(self.env.clone())
    }

    fn findings(&self) -> Value {
        serde_json::to_value(self.engine().security_findings(&FindingsFilter::default()).unwrap()).unwrap()
    }
}

/// The listed finding at `path:line`.
fn at<'a>(findings: &'a Value, path: &str, line: i64) -> &'a Value {
    findings["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["report_path"] == path && f["start_line"] == line)
        .unwrap_or_else(|| panic!("no finding at {path}:{line} in {findings}"))
}

/// `(confidence, verdict, route name, hops)` of a finding's reachability.
fn reach(finding: &Value) -> (&str, &str, Option<&str>, Option<u64>) {
    let r = &finding["context"]["reachability"];
    (
        r["confidence"].as_str().unwrap(),
        r["verdict"].as_str().unwrap(),
        r["route"]["name"].as_str(),
        r["route"]["hops"].as_u64(),
    )
}

fn enclosing_name(finding: &Value) -> Option<&str> {
    finding["context"]["enclosing"]["symbol"]["name"].as_str()
}

#[test]
fn a_finding_in_a_spring_or_axum_handler_is_reachable_and_resolved() {
    let p = project();
    let findings = p.findings();

    let axum = at(&findings, "axum/src/main.rs", 11);
    assert_eq!(enclosing_name(axum), Some("show_user"));
    assert_eq!(axum["context"]["enclosing"]["confidence"], "resolved");
    assert_eq!(reach(axum), ("resolved", "reachable", Some("GET /users/:id"), Some(1)));

    let spring = at(&findings, "spring/src/main/java/com/acme/users/UserController.java", 19);
    assert_eq!(enclosing_name(spring), Some("show"));
    assert_eq!(reach(spring), ("resolved", "reachable", Some("GET /api/users/{id}"), Some(1)));
    // Calls resolve across files in both languages, so each blast radius is a
    // resolved claim too.
    for handler in [axum, spring] {
        assert_eq!(handler["context"]["blast_radius"]["confidence"], "resolved", "{handler}");
    }

    // A route reaches through calls too, and names how far: the handler calls
    // into a second file.
    let called = at(&findings, "axum/src/db.rs", 2);
    assert_eq!(enclosing_name(called), Some("find_user"));
    assert_eq!(reach(called), ("resolved", "reachable", Some("GET /users/:id"), Some(2)));
    // Where calls resolve and no route reaches, the graph says so.
    let orphan = at(&findings, "axum/src/db.rs", 7);
    assert_eq!(enclosing_name(orphan), Some("purge"));
    assert_eq!(reach(orphan), ("resolved", "unreachable", None, None));
    // A route's own span is its registration: a finding there sits in the
    // function that registers it, never in the route.
    let registration = at(&findings, "axum/src/main.rs", 7);
    assert_eq!(enclosing_name(registration), Some("app"));
    assert_eq!(registration["context"]["enclosing"]["symbol"]["kind"], "function");
    // The label is read from the language's own denominator row, carried along.
    assert_eq!(orphan["context"]["resolution"]["language"], "rust");
    assert!(orphan["context"]["resolution"]["calls"]["cross_file_edges"].as_u64().unwrap() > 0);
}

#[test]
fn a_finding_in_a_language_the_graph_does_not_resolve_is_unresolved_never_unreachable() {
    let p = project();
    let findings = p.findings();

    let script = at(&findings, "scripts/report.py", 5);
    // The span lookup still binds it: that claim needs no call resolution.
    assert_eq!(enclosing_name(script), Some("render"));
    assert_eq!(script["context"]["enclosing"]["confidence"], "resolved");
    // No route reaches it, and none could be found: unknown, not unreachable.
    assert_eq!(reach(script), ("unresolved", "unknown", None, None));
    assert_eq!(script["context"]["blast_radius"]["confidence"], "unresolved");
    let resolution = &script["context"]["resolution"];
    assert_eq!(resolution["language"], "python");
    assert!(resolution["calls"]["cross_file_edges"].is_null(), "{resolution}");

    // Over the whole listing, `unreachable` is only ever a resolved claim.
    for f in findings["findings"].as_array().unwrap() {
        if f["context"]["reachability"]["verdict"] == "unreachable" {
            assert_eq!(f["context"]["reachability"]["confidence"], "resolved", "{f}");
        }
    }
}

#[test]
fn a_lockfile_finding_is_unbound_with_its_manifest_line() {
    let p = project();
    let findings = p.findings();

    let lock = at(&findings, "axum/Cargo.lock", 7);
    let context = &lock["context"];
    assert_eq!(context["line"], 7, "it keeps its manifest line");
    assert_eq!(context["enclosing"]["confidence"], "unbound");
    assert_eq!(context["enclosing"]["unbound_because"], "not_indexed");
    assert!(context["enclosing"]["symbol"].is_null());
    assert_eq!(reach(lock), ("unbound", "unknown", None, None));
    assert_eq!(context["blast_radius"], serde_json::json!({"confidence": "unbound"}));
    assert!(context["resolution"].is_null());
}

#[test]
fn blast_radius_is_impact_on_the_same_symbol() {
    let p = project();
    let findings = p.findings();
    let engine = p.engine();

    let mut compared = 0;
    for f in findings["findings"].as_array().unwrap() {
        let Some(symbol) = f["context"]["enclosing"]["symbol"]["symbol"].as_str() else {
            continue;
        };
        let impact = engine.impact(symbol, None);
        assert_eq!(impact.resolved.as_ref().map(|r| r.symbol.as_str()), Some(symbol));
        let radius = &f["context"]["blast_radius"];
        assert_eq!(radius["depth"], impact.depth, "{symbol}");
        assert_eq!(radius["upstream"], impact.upstream.len(), "{symbol}");
        assert_eq!(radius["downstream"], impact.downstream.len(), "{symbol}");
        assert_eq!(radius["docs"], impact.docs.len(), "{symbol}");
        compared += 1;
    }
    assert_eq!(compared, 6, "every finding in code was compared");
    // The comparison is not 0 == 0: the handler both depends and is depended on.
    let handler = &at(&findings, "axum/src/main.rs", 11)["context"]["blast_radius"];
    assert_eq!((handler["upstream"].as_u64(), handler["downstream"].as_u64()), (Some(1), Some(1)));
}

#[test]
fn a_finding_whose_file_changed_since_the_scan_is_never_bound() {
    let p = project();
    // The line the report named now holds something else.
    let db = p.root.join("axum/src/db.rs");
    fs::write(&db, format!("// moved\n{}", fs::read_to_string(&db).unwrap())).unwrap();
    let findings = p.findings();

    let moved = at(&findings, "axum/src/db.rs", 2);
    assert_eq!(moved["freshness"], "stale");
    assert_eq!(moved["context"]["enclosing"]["unbound_because"], "stale");
    assert_eq!(reach(moved), ("unbound", "unknown", None, None));
    // An untouched file still binds.
    assert_eq!(reach(at(&findings, "axum/src/main.rs", 11)).1, "reachable");
}

#[test]
fn a_filtered_listing_carries_each_kept_findings_own_context() {
    let p = project();
    let filter = FindingsFilter::parse(None, None, None, Some("scripts"), None, None, None).unwrap();
    let listed = serde_json::to_value(p.engine().security_findings(&filter).unwrap()).unwrap();

    let kept = listed["findings"].as_array().unwrap();
    assert_eq!(kept.len(), 1, "{listed}");
    assert_eq!(enclosing_name(&kept[0]), Some("render"));
    assert_eq!(listed["total"], 9, "the total still counts every stored finding");
}

#[test]
fn a_file_holding_one_function_binds_to_the_function_not_its_module() {
    // The function spans the whole file, exactly as the file's module does.
    let p = inline_project(
        &[
            ("src/main.rs", "mod token;\n\nfn main() {\n    let _ = token::sign(\"k\");\n}\n"),
            ("src/token.rs", "pub fn sign(secret: &str) -> String {\n    format!(\"{secret}:signed\")\n}\n"),
        ],
        &[("src/token.rs", 2)],
    );
    let findings = p.findings();
    let sign = at(&findings, "src/token.rs", 2);
    assert_eq!(sign["context"]["enclosing"]["symbol"]["kind"], "function", "{sign}");
    assert_eq!(enclosing_name(sign), Some("sign"));
    // Its caller is counted: the blast radius is the function's, not the module's.
    assert_eq!(sign["context"]["blast_radius"]["upstream"], 1, "{sign}");
}

/// A Python `self.x = …` declares its class's Field on a line of `__init__`
/// (S-543): a finding on that line still sits in `__init__`, the method that runs
/// it, never in the field.
#[cfg(feature = "lang-python")]
#[test]
fn a_finding_on_a_python_self_write_binds_to_its_method() {
    let p = inline_project(
        &[(
            "app/views.py",
            "import subprocess\n\n\nclass Runner:\n    def __init__(self, request):\n        self.cmd = request.args[\"cmd\"]\n        self.timeout = 5\n\n    def run(self):\n        return subprocess.call(self.cmd, shell=True)\n",
        )],
        &[("app/views.py", 6), ("app/views.py", 10)],
    );
    let findings = p.findings();
    let write = at(&findings, "app/views.py", 6);
    assert_eq!(enclosing_name(write), Some("__init__"), "{write}");
    assert_eq!(write["context"]["enclosing"]["symbol"]["kind"], "method", "{write}");
    assert_eq!(enclosing_name(at(&findings, "app/views.py", 10)), Some("run"));
}

/// `(verdict, confidence, unresolved_because)` of the finding at `path:line`.
fn open_at<'a>(findings: &'a Value, path: &str, line: i64) -> (&'a str, &'a str, Option<&'a str>) {
    let r = &at(findings, path, line)["context"]["reachability"];
    (r["verdict"].as_str().unwrap(), r["confidence"].as_str().unwrap(), r["unresolved_because"].as_str())
}

#[test]
fn a_method_behind_an_interface_is_never_called_unreachable() {
    // The default Spring shape: the handler calls the interface, and the call
    // binds to the interface's method, never to the implementation's.
    let p = inline_project(
        &[
            (
                "src/main/java/com/acme/UserController.java",
                "package com.acme;\n\nimport org.springframework.web.bind.annotation.GetMapping;\n\
                 import org.springframework.web.bind.annotation.RestController;\n\n@RestController\n\
                 public class UserController {\n    private final UserService service;\n\n\
                 \x20   public UserController(UserService service) {\n        this.service = service;\n    }\n\n\
                 \x20   @GetMapping(\"/users\")\n    public String show(String id) {\n        return service.find(id);\n    }\n}\n",
            ),
            ("src/main/java/com/acme/UserService.java", "package com.acme;\n\npublic interface UserService {\n    String find(String id);\n}\n"),
            (
                "src/main/java/com/acme/UserServiceImpl.java",
                "package com.acme;\n\npublic class UserServiceImpl implements UserService {\n    public String find(String id) {\n\
                 \x20       return \"SELECT \" + id;\n    }\n}\n",
            ),
        ],
        &[("src/main/java/com/acme/UserServiceImpl.java", 5)],
    );
    let findings = p.findings();
    let path = "src/main/java/com/acme/UserServiceImpl.java";
    assert_eq!(enclosing_name(at(&findings, path, 5)), Some("find"));
    assert_eq!(open_at(&findings, path, 5), ("unknown", "unresolved", Some("dispatch")));
    // Callers through the interface are counted against the interface's
    // member, so the blast radius says it may be short.
    assert_eq!(at(&findings, path, 5)["context"]["blast_radius"]["confidence"], "unresolved");
}

/// An Axum crate with a cross-file call (so Rust resolves calls across files)
/// and `extra` added to its router chain.
fn axum_with(extra: &str) -> Vec<(&'static str, String)> {
    vec![
        (
            "src/main.rs",
            format!(
                "mod db;\n\nuse axum::routing::get;\nuse axum::Router;\n\nfn app() -> Router {{\n    Router::new()\n\
                 \x20       .route(\"/users\", get(list_users)){extra}\n}}\n\nasync fn list_users() -> String {{\n\
                 \x20   db::find(\"a\")\n}}\n\nasync fn guard() -> String {{\n    db::find(\"b\")\n}}\n\n\
                 fn main() {{\n    let _ = app();\n}}\n"
            ),
        ),
        (
            "src/db.rs",
            "pub fn find(id: &str) -> String {\n    format!(\"SELECT {id}\")\n}\n\n\
             pub fn idle() -> String {\n    String::new()\n}\n"
                .to_string(),
        ),
    ]
}

fn axum_project(extra: &str) -> Project {
    let files = axum_with(extra);
    let files: Vec<(&str, &str)> = files.iter().map(|(p, b)| (*p, b.as_str())).collect();
    // `guard`'s body, and `idle`'s.
    inline_project(&files, &[("src/main.rs", 17), ("src/db.rs", 6)])
}

#[test]
fn a_callable_handed_off_by_value_is_never_called_unreachable() {
    // `from_fn(guard)` hands the guard to the router as a value: no call edge
    // reaches it, only the handoff marker.
    let p = axum_project("\n        .layer(axum::middleware::from_fn(guard))");
    let findings = p.findings();
    assert_eq!(enclosing_name(at(&findings, "src/main.rs", 17)), Some("guard"));
    assert_eq!(open_at(&findings, "src/main.rs", 17), ("unknown", "unresolved", Some("value_reference")));
    // The rule is about the guard, not the project: an uncalled function
    // beside it is still unreachable.
    assert_eq!(open_at(&findings, "src/db.rs", 6), ("unreachable", "resolved", None));
}

#[test]
fn a_route_with_no_proven_handler_leaves_no_symbol_unreachable() {
    // A closure handler: the route has no edge to a named handler, so it
    // could reach anything.
    let p = axum_project("\n        .route(\"/purge\", get(|| async { String::new() }))");
    let findings = p.findings();
    assert_eq!(open_at(&findings, "src/db.rs", 6), ("unknown", "unresolved", Some("unproven_route")));
    assert_eq!(open_at(&findings, "src/main.rs", 17), ("unknown", "unresolved", Some("unproven_route")));
}

#[test]
fn a_project_with_no_routes_calls_nothing_unreachable() {
    // A framework the graph does not promote has no route to walk from.
    let p = inline_project(
        &[
            ("src/main.rs", "mod db;\n\nfn main() {\n    let _ = db::find(\"a\");\n}\n"),
            ("src/db.rs", "pub fn find(id: &str) -> String {\n    format!(\"SELECT {id}\")\n}\n\npub fn idle() -> String {\n    String::new()\n}\n"),
        ],
        &[("src/db.rs", 6)],
    );
    let findings = p.findings();
    assert_eq!(at(&findings, "src/db.rs", 6)["context"]["resolution"]["language"], "rust");
    assert_eq!(open_at(&findings, "src/db.rs", 6), ("unknown", "unresolved", Some("no_routes")));
}

#[test]
fn a_finding_outside_any_code_symbol_or_with_no_location_is_unbound_and_says_why() {
    let p = project();
    let findings = p.findings();

    // An IaC finding: the Dockerfile is indexed, but only as config nodes,
    // which never enclose a finding.
    let docker = at(&findings, "axum/Dockerfile", 2);
    assert_eq!(docker["binding"], "bound", "the file itself is indexed");
    assert_eq!(docker["context"]["enclosing"]["unbound_because"], "no_symbol");
    assert_eq!(docker["context"]["line"], 2);
    assert_eq!(reach(docker), ("unbound", "unknown", None, None));

    // A result with no physical location at all.
    let nowhere = findings["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["report_path"].is_null())
        .expect("the location-less result is listed");
    assert_eq!(nowhere["context"]["enclosing"]["unbound_because"], "no_location");
    assert!(nowhere["context"]["line"].is_null());
    assert_eq!(nowhere["context"]["blast_radius"], serde_json::json!({"confidence": "unbound"}));
}

#[test]
fn a_listing_where_nothing_binds_still_says_why_each_finding_is_unbound() {
    // Only the lockfile finding is kept: no listed finding has an indexed
    // file to bind in.
    let p = project();
    let filter = FindingsFilter::parse(None, Some("osv-scanner"), None, None, None, None, None).unwrap();
    let listed = serde_json::to_value(p.engine().security_findings(&filter).unwrap()).unwrap();

    let kept = listed["findings"].as_array().unwrap();
    assert_eq!(kept.len(), 1, "{listed}");
    let context = &kept[0]["context"];
    assert_eq!(context["enclosing"]["unbound_because"], "not_indexed");
    assert_eq!(context["line"], 7);
    assert!(context["resolution"].is_null());
}

// ── Code scope by the graph (S-795, FR-SC-28) ────────────────────────────────

/// `src/keys.rs`: a production function at line 2 and, from line 6, an inline
/// test module whose `#[test]` fn holds a fake key at line 9 — the shape of
/// this repository's `mask.rs:214`.
const KEYS_RS: &str = r#"pub fn mask(secret: &str) -> String {
    secret.chars().map(|_| '*').collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_key_is_masked() {
        let key = "AKIAIOSFODNN7EXAMPLE";
        assert_eq!(super::mask(key).len(), key.len());
    }
}
"#;

/// The scope of the listed finding at `path:line`, by `report_path`.
fn scope_at(findings: &Value, path: &str, line: i64) -> (String, Value) {
    let f = at(findings, path, line);
    (f["code_scope"].as_str().unwrap().to_string(), f["code_scope_reason"].clone())
}

/// The engine classifies each finding by its enclosing node in this graph,
/// else by the security path rule: a key in a `#[test]` fn in `src/` reads
/// `test_symbol`; the production fn beside it in the same file reads
/// production; a fixture Dockerfile and an OSV lockfile named by its
/// absolute path under the root read `test_path`. The default listing hides
/// the test-code ones and says so, and `security status` counts production
/// alone and states the rest.
#[test]
fn the_engine_reads_each_findings_code_scope_from_its_graph_and_its_path() {
    let p = inline_project(
        &[("src/keys.rs", KEYS_RS), ("tests/fixtures/app/Dockerfile", "FROM rust\nUSER root\n"), ("Cargo.toml", "[package]\nname = \"keys\"\n")],
        &[("src/keys.rs", 2), ("src/keys.rs", 9), ("tests/fixtures/app/Dockerfile", 2)],
    );
    let lockfile = p.root.join("tests/fixtures/app/Cargo.lock");
    fs::write(&lockfile, "version = 3\n").unwrap();
    let osv = p.root.parent().unwrap().join("osv.sarif");
    let uri = format!("file://{}", lockfile.display());
    let sarif = serde_json::json!({"version": "2.1.0", "runs": [{
        "tool": {"driver": {"name": "osv-scanner", "version": "1.9.2", "rules": [{"id": "GHSA-xxxx"}]}},
        "results": [{"ruleId": "GHSA-xxxx", "message": {"text": "a vulnerable package"},
            "locations": [{"physicalLocation": {"artifactLocation": {"uri": uri}, "region": {"startLine": 1}}}]}]
    }]});
    fs::write(&osv, sarif.to_string()).unwrap();
    p.engine().security_ingest(&osv).expect("the osv report ingests");

    let all = FindingsFilter::parse(None, None, None, None, None, None, Some("all")).unwrap();
    let listed = serde_json::to_value(p.engine().security_findings(&all).unwrap()).unwrap();
    let production = ("production".to_string(), Value::Null);
    assert_eq!(scope_at(&listed, "src/keys.rs", 2), production, "outside the test module");
    assert_eq!(scope_at(&listed, "src/keys.rs", 9), ("test".to_string(), Value::from("test_symbol")), "{listed}");
    assert_eq!(scope_at(&listed, "tests/fixtures/app/Dockerfile", 2), ("test".to_string(), Value::from("test_path")));
    let osv_row = listed["findings"].as_array().unwrap().iter().find(|f| f["scanner_name"] == "osv-scanner").unwrap();
    assert_eq!((osv_row["code_scope"].as_str(), osv_row["code_scope_reason"].as_str()), (Some("test"), Some("test_path")), "{osv_row}");

    let default = serde_json::to_value(p.engine().security_findings(&FindingsFilter::parse(None, None, None, None, None, None, None).unwrap()).unwrap()).unwrap();
    let kept: Vec<i64> = default["findings"].as_array().unwrap().iter().map(|f| f["start_line"].as_i64().unwrap()).collect();
    assert_eq!((kept, default["hidden_by_scope"].as_u64()), (vec![2], Some(3)), "{default}");

    let status = serde_json::to_value(p.engine().security_status().unwrap()).unwrap();
    let semgrep = status["scanners"].as_array().unwrap().iter().find(|r| r["scanner"] == "semgrep").unwrap();
    assert_eq!((semgrep["findings"].as_u64(), semgrep["test"].as_u64()), (Some(1), Some(2)), "{semgrep}");

    // Files & Risk classifies by the same graph: the production file with an
    // inline test module counts its production finding alone.
    let board = serde_json::to_value(p.engine().hotspots_with_findings(None, false, false, false).unwrap()).unwrap();
    let keys = board["files"].as_array().unwrap().iter().find(|h| h["path"] == "src/keys.rs").expect("src/keys.rs is ranked");
    assert_eq!((keys["findings"]["count"].as_u64(), keys["findings"]["test"].as_u64()), (Some(1), Some(1)), "{keys}");
}
