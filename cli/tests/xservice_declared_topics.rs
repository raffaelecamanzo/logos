//! End-to-end tests for `logos xservice declared-topics` and the
//! `declared_topics` section of `logos workspace status` (S-490, [FR-WS-05],
//! [FR-WS-36]), driven through the **real** `logos` binary over members
//! indexed with Java sources and Spring configuration.
//!
//! The fixture is the reference estate's declared-topic shapes in miniature
//! ([S-488]'s findings):
//!
//! - `orders-api` publishes `orders` and its configuration declares it
//!   (`outbound.topic`) — a declaration **already observed**;
//! - `ledger` has no broker site and declares `orders` twice, in the base
//!   `application.yml` and under the `prod` profile — **net-new, config-only**;
//! - `tools` declares `clickstream` (`kafka.topics`), which no broker site
//!   names — **declared-only**, never a topic.
//!
//! A declaration is never an observed broker edge ([BR-54]): every runtime
//! section of `workspace status` is asserted unmoved beside it.
//!
//! [FR-WS-05]: ../../docs/specs/requirements/FR-WS-05.md
//! [FR-WS-36]: ../../docs/specs/requirements/FR-WS-36.md
//! [BR-54]: ../../docs/specs/software-spec.md#327-workspace-federation
//! [S-488]: ../../docs/planning/journal.md#s-488-measure-the-net-new-config-declared-topic-memberships-beyond-the-observed-broker-edges
#![cfg(feature = "lang-all")]

use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;
use tempfile::TempDir;

const PUBLISHER: &str = "\
package com.acme;
class OrderService {
    private KafkaTemplate<String, String> kafkaTemplate;

    public void emitOrder(String payload) {
        kafkaTemplate.send(\"orders\", payload);
    }
}
";

const PLAIN: &str = "package com.acme;\nclass Plain { void run() {} }\n";

const MAIN: &str = "src/main/resources/application.yml";
const PROD: &str = "src/main/resources/application-prod.yml";

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

/// The human rendering: the same read-model, pretty-printed ([FR-CL-02]).
///
/// [FR-CL-02]: ../../docs/specs/requirements/FR-CL-02.md
fn logos_human(project: &Path, args: &[&str]) -> (String, Value) {
    let out = logos(project, args);
    assert!(out.status.success(), "logos {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8(out.stdout).expect("utf8 stdout");
    let value = serde_json::from_str(&stdout).expect("the human rendering is the read-model");
    (stdout, value)
}

/// One member of the fixture: its Java source and its committed configuration
/// as `(path, yaml)` pairs.
struct Member {
    name: &'static str,
    java: &'static str,
    config: &'static [(&'static str, &'static str)],
}

const MEMBERS: [Member; 3] = [
    Member { name: "orders-api", java: PUBLISHER, config: &[(MAIN, "outbound:\n  topic: orders\n")] },
    Member {
        name: "ledger",
        java: PLAIN,
        config: &[(MAIN, "ledger:\n  topic: orders\n"), (PROD, "ledger:\n  topic: orders\n")],
    },
    Member { name: "tools", java: PLAIN, config: &[(MAIN, "kafka:\n  topics: clickstream\n")] },
];

/// The members, indexed through the real binary. With `declare = false` every
/// member keeps its Java source and an `application.yml` naming no topic, so
/// the two workspaces differ only in what configuration declares.
fn workspace(declare: bool) -> TempDir {
    workspace_of(&MEMBERS, declare)
}

/// [`workspace`] over any roster; the first member is the workspace default.
fn workspace_of(members: &[Member], declare: bool) -> TempDir {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    for m in members {
        let dir = root.join(m.name);
        std::fs::create_dir_all(&dir).unwrap();
        sh_git(&dir, &["init", "-q", "-b", "main"]);
        write(&dir, "src/main/java/com/acme/Source.java", m.java);
        if declare {
            for (path, yaml) in m.config {
                write(&dir, path, yaml);
            }
        } else {
            write(&dir, MAIN, "app:\n  name: shop\n");
        }
        sh_git(&dir, &["add", "."]);
        sh_git(&dir, &["commit", "-q", "-m", "init"]);
        assert!(logos(&dir, &["index"]).status.success(), "index {}", m.name);
    }
    let names = members.iter().map(|m| format!("\"{}\"", m.name)).collect::<Vec<_>>().join(", ");
    std::fs::write(
        root.join("logos.workspace.toml"),
        format!("[workspace]\nname = \"acme\"\nmembers = [{names}]\ndefault = \"{}\"\n", members[0].name),
    )
    .unwrap();
    tmp
}

/// `(member, file, profile, status)` of every declaration of `topic`.
fn declarations(answer: &Value, topic: &str) -> Vec<(String, String, Value, String)> {
    let entry = answer["topics"]
        .as_array()
        .expect("topics array")
        .iter()
        .find(|t| t["topic"] == topic)
        .unwrap_or_else(|| panic!("topic {topic} in {answer}"));
    entry["declarations"]
        .as_array()
        .expect("declarations")
        .iter()
        .map(|d| {
            (
                d["member"].as_str().unwrap().to_string(),
                d["file"].as_str().unwrap().to_string(),
                d["profile"].clone(),
                d["status"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

/// AC2: per captured topic, the declaring members each with key, file, profile
/// and an `observed` / `net-new` mark; `--repo` scopes the rows while the
/// headline stays workspace-wide; a non-member is a `scope_note`, never an
/// error; the human rendering is the same read-model.
#[test]
fn declared_topics_lists_each_topic_with_its_declaring_members_marked_observed_or_net_new() {
    let tmp = workspace(true);
    let all = logos_json(tmp.path(), &["xservice", "declared-topics"]);

    assert!(all.get("scope").is_none() && all.get("scope_note").is_none(), "{all}");
    assert_eq!(all["topics"].as_array().unwrap().len(), 1, "one captured topic: {all}");
    assert_eq!(
        declarations(&all, "orders"),
        vec![
            ("ledger".to_string(), PROD.to_string(), Value::from("prod"), "net-new".to_string()),
            ("ledger".to_string(), MAIN.to_string(), Value::Null, "net-new".to_string()),
            ("orders-api".to_string(), MAIN.to_string(), Value::Null, "observed".to_string()),
        ],
        "a declaration names member, file, profile and its mark: {all}"
    );
    let first = &all["topics"][0]["declarations"][0];
    assert_eq!(first["key"], "ledger.topic", "the canonical key rides on each row: {first}");
    assert_eq!(first["provenance"], "config-declared");
    assert_eq!(first["site"]["site"], "config-only", "a net-new row says why: {first}");

    assert_eq!(all["declared_only"].as_array().unwrap().len(), 1, "clickstream is no topic: {all}");
    assert_eq!(all["declared_only"][0]["member"], "tools");

    let headline = &all["headline"];
    assert_eq!(headline["declared_topic_pairs"], 2, "{headline}");
    assert_eq!(headline["already_observed"]["pairs"], 1);
    assert_eq!(headline["net_new"]["pairs"], 1);
    assert_eq!(headline["net_new"]["config_only"], 1);
    assert_eq!(headline["declared_only"]["pairs"], 1);
    assert_eq!(headline["declarations_considered"], 4);
    assert_eq!(headline["members"]["read"], 3);

    let one = logos_json(tmp.path(), &["xservice", "declared-topics", "--repo", "ledger"]);
    assert_eq!(one["scope"], "ledger");
    assert!(one.get("scope_note").is_none(), "a member read declaring topics is no note: {one}");
    let members: Vec<_> = declarations(&one, "orders").into_iter().map(|d| d.0).collect();
    assert_eq!(members, ["ledger", "ledger"], "scoped to the declaring member");
    assert_eq!(one["declared_only"], serde_json::json!([]));
    assert_eq!(one["headline"], all["headline"], "the denominator stays workspace-wide under a scope");

    let unknown = logos(tmp.path(), &["xservice", "declared-topics", "--repo", "nope", "--json"]);
    assert_eq!(unknown.status.code(), Some(0), "a non-member is no error");
    let unknown: Value = serde_json::from_slice(&unknown.stdout).expect("JSON");
    assert_eq!(unknown["topics"], serde_json::json!([]));
    let note = unknown["scope_note"].as_str().expect("a scope_note");
    assert!(
        note.starts_with("`nope` is not a member the declared-topic relation was read over"),
        "an empty list is never presented as \"declares nothing\": {note}"
    );

    let (_, human) = logos_human(tmp.path(), &["xservice", "declared-topics"]);
    assert_eq!(human, all, "human and --json print one read-model");
    // The snake-case alias reaches the same command.
    assert_eq!(logos_json(tmp.path(), &["xservice", "declared_topics"]), all);
}

/// AC1: `workspace status`, human and `--json`, carries `declared_topics` with
/// `declared_topic_pairs` beside its denominators, after `topics` — and every
/// runtime figure is unmoved by the declarations ([BR-54]).
///
/// [BR-54]: ../../docs/specs/software-spec.md#327-workspace-federation
#[test]
fn workspace_status_carries_declared_topic_pairs_beside_denominators_after_topics() {
    let with = workspace(true);
    let status = logos_json(with.path(), &["workspace", "status"]);
    let section = &status["declared_topics"];
    assert_eq!(section["declared_topic_pairs"], 2, "{section}");
    assert_eq!(section["declarations_considered"], 4, "its denominator: {section}");
    assert_eq!(section["net_new"]["pairs"], 1);
    assert_eq!(section["already_observed"]["pairs"], 1);
    assert_eq!(section["members"]["read"], 3, "the members read, beside it");
    let summary = section["summary"].as_str().expect("summary");
    assert!(summary.contains("declared"), "the line says what it counts: {summary}");

    let (text, human) = logos_human(with.path(), &["workspace", "status"]);
    assert_eq!(human["declared_topics"], *section, "human and --json carry one section");
    let (topics_at, declared_at) = (text.find("\"topics\"").unwrap(), text.find("\"declared_topics\"").unwrap());
    assert!(topics_at < declared_at, "the section follows `topics`, apart from it");
    // …and sits directly after it: `type_reference` (the Java members') follows
    // it, and `declared_topics` is not pushed down past the sections after.
    let type_reference_at = text.find("\"type_reference\"").expect("the Java members carry type_reference");
    assert!(declared_at < type_reference_at, "after `topics`, before the sections that followed it");

    let without = workspace(false);
    let bare = logos_json(without.path(), &["workspace", "status"]);
    for runtime in ["coverage", "warm_rollup", "degraded_rollup"] {
        assert_eq!(status[runtime], bare[runtime], "`{runtime}` moved when configuration declared topics");
    }
    assert_eq!(status["topics"], bare["topics"], "no observed topic moved: a declaration is no broker edge");
}

/// AC1, the other half: a workspace with no declarations omits the section and
/// prints `workspace status` exactly as it did before the section existed — no
/// key, in either rendering.
#[test]
fn a_workspace_with_no_declarations_prints_status_with_no_declared_topics_section() {
    let tmp = workspace(false);
    let status = logos_json(tmp.path(), &["workspace", "status"]);
    let mut keys: Vec<&str> = status.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    // `type_reference` is S-474's section (the members are Java) — part of the
    // payload before the relation existed.
    assert_eq!(
        keys,
        ["coverage", "degraded_rollup", "members", "reference_resolution", "topics", "type_reference", "warm_rollup", "workspace"]
    );
    let (text, _) = logos_human(tmp.path(), &["workspace", "status"]);
    assert!(!text.contains("declared_topics"), "{text}");

    // The command still answers, and says nothing was declared — never an error.
    let answer = logos_json(tmp.path(), &["xservice", "declared-topics"]);
    assert_eq!(answer["topics"], serde_json::json!([]));
    assert_eq!(answer["headline"]["declared_topic_pairs"], 0);
    assert_eq!(answer["headline"]["declarations_considered"], 0);
}

/// AC4 ([ADR-53]): a member whose engine fails surfaces as a per-member error
/// inside the answer, not an abort — the answer is read over the members that
/// could be, and exits 0 like every `xservice` command.
///
/// [ADR-53]: ../../docs/specs/architecture/decisions/ADR-53.md
#[test]
fn a_member_whose_engine_fails_is_named_inside_the_answer_not_an_abort() {
    let tmp = workspace(true);
    let db = tmp.path().join("tools/.logos/logos.db");
    std::fs::remove_file(&db).expect("clear the store file");
    std::fs::create_dir_all(&db).expect("a directory where the store must be");

    let out = logos(tmp.path(), &["xservice", "declared-topics", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let answer: Value = serde_json::from_slice(&out.stdout).expect("JSON");
    let members = &answer["headline"]["members"];
    assert_eq!(members["unread"], serde_json::json!(["tools"]), "{answer}");
    assert_eq!(members["unread_reasons"]["tools"], "declared-topic facts could not be read");
    assert_eq!(members["read"], 2, "read over the other two: {members}");
    assert_eq!(declarations(&answer, "orders").len(), 3, "the healthy members still answer");

    let scoped = logos(tmp.path(), &["xservice", "declared-topics", "--repo", "tools", "--json"]);
    assert_eq!(scoped.status.code(), Some(0));
    let scoped: Value = serde_json::from_slice(&scoped.stdout).expect("JSON");
    let note = scoped["scope_note"].as_str().expect("a note, not silence");
    assert!(note.contains("declared-topic facts could not be read"), "{note}");
}

/// Break a member so its engine cannot open: `.logos/logos.db` as a directory.
fn break_store(root: &Path, member: &str) {
    let db = root.join(member).join(".logos/logos.db");
    std::fs::remove_file(&db).expect("clear the store file");
    std::fs::create_dir_all(&db).expect("a directory where the store must be");
}

/// AC4 on the **status** path ([ADR-53]): `workspace status` derives its section
/// from the walks it already makes, not through the accessor, so a member whose
/// engine fails is named in `declared_topics.members.unread` there too — beside
/// the declarations of the members that could be read, never counted as a
/// member declaring nothing. The status gates on the degraded member (exit 1),
/// but the payload is still the whole answer.
///
/// [ADR-53]: ../../docs/specs/architecture/decisions/ADR-53.md
#[test]
fn workspace_status_names_a_failed_member_unread_inside_the_declared_topics_section() {
    let tmp = workspace(true);
    let healthy = logos_json(tmp.path(), &["workspace", "status"]);
    assert_eq!(healthy["declared_topics"]["members"]["members"], 3, "the roster is the denominator: {healthy}");
    assert_eq!(healthy["declared_topics"]["members"]["read"], 3);
    assert!(healthy["declared_topics"]["members"].get("unread").is_none(), "nothing unread when healthy");

    break_store(tmp.path(), "tools");
    let out = logos(tmp.path(), &["workspace", "status", "--json"]);
    assert_eq!(out.status.code(), Some(1), "an unopenable member gates the status: {}", String::from_utf8_lossy(&out.stderr));
    let status: Value = serde_json::from_slice(&out.stdout).expect("JSON");
    let members = &status["declared_topics"]["members"];
    assert_eq!(members["members"], 3, "the failed member stays in the denominator: {members}");
    assert_eq!(members["read"], 2, "…and is not counted read: {members}");
    assert_eq!(members["unread"], serde_json::json!(["tools"]), "{members}");
    assert_eq!(members["unread_reasons"]["tools"], "declared-topic facts could not be read");
    assert_eq!(status["declared_topics"]["declared_topic_pairs"], 2, "the readable members still answer");

    // The two surfaces name it identically.
    let answer = logos_json(tmp.path(), &["xservice", "declared-topics"]);
    assert_eq!(status["declared_topics"], answer["headline"], "status and accessor agree over a failed member");
}

/// The omission rule: a member that could not be read does not, **alone**, make
/// the section appear — a workspace where nothing was declared prints status
/// exactly as before the relation existed, even with a member unread. (The
/// `xservice` answer always names it.)
#[test]
fn an_unread_member_alone_does_not_surface_the_status_section() {
    let tmp = workspace(false);
    break_store(tmp.path(), "tools");
    let out = logos(tmp.path(), &["workspace", "status", "--json"]);
    assert_eq!(out.status.code(), Some(1), "{}", String::from_utf8_lossy(&out.stderr));
    let status: Value = serde_json::from_slice(&out.stdout).expect("JSON");
    assert!(status.get("declared_topics").is_none(), "{}", status["declared_topics"]);
    let answer = logos_json(tmp.path(), &["xservice", "declared-topics"]);
    assert_eq!(answer["headline"]["members"]["unread"], serde_json::json!(["tools"]), "{answer}");
}

const GATEWAY: &str = "\
package com.acme;
class Relay {
    private KafkaTemplate<String, String> kafkaTemplate;

    public void audit(String payload) {
        kafkaTemplate.send(\"audit\", payload);
    }
}
";

/// The B3 site split through both real surfaces: a net-new member that **has**
/// a broker site — here a publish to `audit`, not the topic it declares — is
/// `site-uncaptured`, not `config-only`, on the status section and on the
/// accessor alike, and the two agree. (The fixture above has no such member, so
/// its split is only ever `config-only`.)
#[test]
fn a_net_new_member_with_a_broker_site_is_split_by_it_on_status_and_on_the_accessor() {
    let members = [
        Member { name: "orders-api", java: PUBLISHER, config: &[(MAIN, "outbound:\n  topic: orders\n")] },
        Member { name: "gateway", java: GATEWAY, config: &[(MAIN, "gateway:\n  topic: orders\n")] },
    ];
    let tmp = workspace_of(&members, true);
    let answer = logos_json(tmp.path(), &["xservice", "declared-topics"]);
    let net_new = &answer["headline"]["net_new"];
    assert_eq!(net_new["pairs"], 1, "{answer}");
    assert_eq!(net_new["config_only"], 0, "a member with a broker site is not config-only: {net_new}");
    assert_eq!(net_new["site_uncaptured"], 1, "{net_new}");
    let mark = declarations(&answer, "orders")
        .into_iter()
        .find(|d| d.0 == "gateway")
        .expect("gateway declares orders");
    assert_eq!(mark.3, "net-new");
    let row = answer["topics"][0]["declarations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["member"] == "gateway")
        .unwrap();
    assert_eq!(row["site"]["site"], "site-uncaptured", "{row}");

    let status = logos_json(tmp.path(), &["workspace", "status"]);
    assert_eq!(status["declared_topics"], answer["headline"], "the status split is the accessor's");
}
