//! S-701 ([FR-WS-05], [FR-WS-35], [BR-73]) end to end: **a cross-service
//! answer matches the member and the symbol, never the symbol alone** — over
//! members really indexed, where two of them hold the same declarations.
//!
//! The fixture is a fork pair: `svc-a` and `svc-b` both declare
//! `OrderListener#onOrder()`, `Shipper#ship()` and a Kotlin `Dto` at one path.
//! Each fork is its own repository with its own coordinate ([CR-222], S-706), so
//! no canonical symbol is held by both — two live members never can be — but a
//! symbol written without a coordinate ([FR-EX-36]) names each pair in both, each
//! fork resolving it under its own coordinate: that is how one query still
//! matches several members, and it is what these tests query. Only `svc-b`'s are wired:
//! its `onOrder` subscribes to `orders`, which `orders` publishes; its `ship`
//! publishes `shipments`, which `ship` subscribes to. Both also declare
//! `Relay#relay()`, which `svc-a`'s publishes from and `svc-b`'s listens on —
//! one edge whose two ends both answer to the queried symbol. The two Kotlin `Dto`s
//! sit in different packages (`com.acme.a`, `com.acme.b`), which Kotlin allows
//! at one path, and `app` — building against both — imports each from its own
//! file, so both bind, each to its own fork's node. Each fork also makes a client
//! call to a runtime-composed URL, the unresolved egress a scoped residue reports.
//!
//! - `--repo svc-a` finds no cross-service caller or impact through
//!   `svc-b`'s edges; `--repo svc-b` finds them, the far side in any member;
//! - unscoped, every row names its member, the resolved count is broken down
//!   per member and an ambiguity notice names both and `--repo`;
//! - the type-reference tier keeps only the scope's owner, on the node and
//!   the dotted-name paths alike.
//!
//! [FR-WS-05]: ../../docs/specs/requirements/FR-WS-05.md
//! [FR-WS-35]: ../../docs/specs/requirements/FR-WS-35.md
//! [BR-73]: ../../docs/specs/software-spec.md#327-workspace-federation
//! [CR-222]: ../../docs/requests/CR-222-workspace-safe-symbol-identity.md
//! [FR-EX-36]: ../../docs/specs/requirements/FR-EX-36.md
#![cfg(all(feature = "lang-java", feature = "lang-kotlin"))]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use logos_core::federation::query::{self, ReachabilityInputs};
use logos_core::federation::{
    discover, BuildDependencies, ContractBridge, EngineRegistry, RegistryMode, TypeReferenceIndex, TypeReferences,
};
use logos_core::Engine;
use serde_json::{json, Value};

#[path = "support/coordinate.rs"]
mod coordinate;

const J: &str = "src/main/java/com/acme";

/// `orders` publishes the `orders` topic `svc-b` subscribes to.
const PUBLISHER: &str = "package com.acme;
class OrderService {
    private KafkaTemplate<String, String> kafkaTemplate;

    public void emitOrder(String payload) {
        kafkaTemplate.send(\"orders\", payload);
    }
}
";

/// `svc-b`'s listener: subscribed, so its `onOrder` is a bridge provider.
const LISTENER_B: &str = "package com.acme;
class OrderListener {
    @KafkaListener(topics = \"orders\")
    public void onOrder(String msg) {}
}
";

/// `svc-a`'s fork of it: the same file, class and method — the same descriptors —
/// subscribed to nothing, and called inside `svc-a`.
const LISTENER_A: &str = "package com.acme;
class OrderListener {
    public void onOrder(String msg) {}

    void replay() { onOrder(\"\"); }
}
";

/// `svc-b`'s publisher of `shipments`: `ship()` is a bridge consumer endpoint.
const SHIPPER_B: &str = "package com.acme;
class Shipper {
    private KafkaTemplate<String, String> kafkaTemplate;

    public void ship(String payload) {
        kafkaTemplate.send(\"shipments\", payload);
    }
}
";

/// `svc-a`'s fork of it: the same descriptors, publishing nothing.
const SHIPPER_A: &str = "package com.acme;
class Shipper {
    public void ship(String payload) {}
}
";

/// `ship` subscribes to `shipments`.
const SHIP_LISTENER: &str = "package com.acme;
class ShipListener {
    @KafkaListener(topics = \"shipments\")
    public void onShipment(String msg) {}
}
";

/// A client call whose base URL is joined at runtime: one unresolved egress
/// site (`base-url-runtime`) per fork.
const REPLAYER: &str = "package com.acme;

import org.springframework.web.client.RestClient;

public class Replayer {
    private RestClient restClient;
    private String baseUrl;

    public String replay(String id) {
        return restClient.get().uri(baseUrl + \"/orders/\" + id).retrieve().body(String.class);
    }
}
";

/// `svc-a`'s relay publishes `loop`…
const RELAY_A: &str = "package com.acme;
class Relay {
    private KafkaTemplate<String, String> kafkaTemplate;

    public void relay(String payload) {
        kafkaTemplate.send(\"loop\", payload);
    }
}
";

/// …and `svc-b`'s, the same descriptors, listens to it: one edge whose two ends
/// both answer to the queried symbol, in different members — a fork calling its original.
const RELAY_B: &str = "package com.acme;
class Relay {
    @KafkaListener(topics = \"loop\")
    public void relay(String payload) {}
}
";

/// The two members holding every shared symbol, in name order.
const FORKS: [&str; 2] = ["svc-a", "svc-b"];

const MEMBERS: [&str; 5] = ["orders", "svc-a", "svc-b", "ship", "app"];

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().expect("has parent")).expect("mkdir");
    fs::write(path, contents).expect("write fixture");
}

fn git_init(dir: &Path) {
    fs::create_dir_all(dir).expect("mkdir member");
    let status = Command::new("git")
        .args(["-c", "maintenance.auto=false", "-c", "gc.auto=0", "init", "-q"])
        .current_dir(dir)
        .status()
        .expect("git runs");
    assert!(status.success(), "git init {}", dir.display());
}

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

fn index_member(root: &Path) {
    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    let _ = engine.sync(&[] as &[PathBuf]);
}

/// Write and index the workspace.
fn workspace(root: &Path) {
    for name in MEMBERS {
        let dir = root.join(name);
        git_init(&dir);
        match name {
            "orders" => write(&dir, &format!("{J}/OrderService.java"), PUBLISHER),
            "ship" => write(&dir, &format!("{J}/ShipListener.java"), SHIP_LISTENER),
            "app" => {
                let uses = |package: &str, class: &str| {
                    format!("package com.acme.app;\n\nimport com.acme.{package}.Dto;\n\npublic class {class} {{}}\n")
                };
                write(&dir, &format!("{J}/app/UsesA.java"), &uses("a", "UsesA"));
                write(&dir, &format!("{J}/app/UsesB.java"), &uses("b", "UsesB"));
                write(&dir, "pom.xml", &pom("app", &FORKS));
            }
            fork => {
                let wired = fork == "svc-b";
                let package = if wired { "b" } else { "a" };
                write(&dir, &format!("{J}/OrderListener.java"), if wired { LISTENER_B } else { LISTENER_A });
                write(&dir, &format!("{J}/Shipper.java"), if wired { SHIPPER_B } else { SHIPPER_A });
                write(&dir, &format!("{J}/Replayer.java"), REPLAYER);
                write(&dir, &format!("{J}/Relay.java"), if wired { RELAY_B } else { RELAY_A });
                write(&dir, "src/main/kotlin/Dto.kt", &format!("package com.acme.{package}\n\nclass Dto\n"));
                write(&dir, "pom.xml", &pom(fork, &[]));
            }
        }
        coordinate::pin_as(&dir, &member_coordinate(name));
        index_member(&dir);
    }
    let list: Vec<String> = MEMBERS.iter().map(|m| format!("\"{m}\"")).collect();
    fs::write(
        root.join("logos.workspace.toml"),
        format!("[workspace]\nname = \"forks\"\nmembers = [{}]\n", list.join(", ")),
    )
    .expect("write manifest");
}

struct Fixture {
    tmp: tempfile::TempDir,
    registry: EngineRegistry<Engine>,
    inputs: ReachabilityInputs,
    index: std::sync::Arc<TypeReferenceIndex>,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    workspace(tmp.path());
    let federation = discover(tmp.path()).expect("manifest parses").expect("a workspace");
    let registry = EngineRegistry::<Engine>::new(federation, RegistryMode::Lazy);
    let inputs = query::reachability_inputs(&ContractBridge::new(), &registry);
    let index = TypeReferences::new().index(&registry, &BuildDependencies::new());
    Fixture { tmp, registry, inputs, index }
}

/// The coordinate a member is indexed under (S-703): its own, forks included —
/// two members sharing one would leave the later degraded (S-706).
fn member_coordinate(name: &str) -> String {
    format!("example.com/{name}")
}

/// `path`'s `scope` written without a coordinate ([FR-EX-36]): the one string
/// both forks answer to, each under its own coordinate.
///
/// [FR-EX-36]: ../../docs/specs/requirements/FR-EX-36.md
fn shared(path: &str, scope: &str) -> String {
    let (dir, file) = path.rsplit_once('/').expect("a path with a file");
    format!("logos . . . {dir}/`{file}`/{scope}")
}

/// The canonical symbol `member` holds for the coordinate-less `shared`.
fn held_by(member: &str, shared: &str) -> String {
    let descriptors = shared.strip_prefix("logos . . . ").expect("a symbol without a coordinate");
    format!("logos . {} . {descriptors}", member_coordinate(member))
}

fn on_order() -> String {
    shared(&format!("{J}/OrderListener.java"), "OrderListener#onOrder().")
}

fn ship() -> String {
    shared(&format!("{J}/Shipper.java"), "Shipper#ship().")
}

fn relay() -> String {
    shared(&format!("{J}/Relay.java"), "Relay#relay().")
}

const DTO: &str = "logos . . . src/main/kotlin/`Dto.kt`/Dto#";

impl Fixture {
    fn callers(&self, symbol: &str, repo: Option<&str>) -> Value {
        self.callers_over(&self.index, symbol, repo)
    }

    fn impact(&self, symbol: &str, repo: Option<&str>) -> Value {
        self.impact_over(&self.index, symbol, repo)
    }

    /// `callers` stitched over `index` rather than the fixture's own.
    fn callers_over(&self, index: &TypeReferenceIndex, symbol: &str, repo: Option<&str>) -> Value {
        let answer = query::xservice_callers(&self.registry, &self.inputs, symbol, None, repo)
            .with_type_references(&self.registry, index);
        serde_json::to_value(answer).expect("serializes")
    }

    /// `impact` stitched over `index` rather than the fixture's own.
    fn impact_over(&self, index: &TypeReferenceIndex, symbol: &str, repo: Option<&str>) -> Value {
        let answer = query::xservice_impact(&self.registry, &self.inputs, symbol, None, repo)
            .with_type_references(&self.registry, index, None);
        serde_json::to_value(answer).expect("serializes")
    }
}

fn rows(answer: &Value, key: &str) -> Vec<Value> {
    answer.get(key).and_then(Value::as_array).cloned().unwrap_or_default()
}

/// The `(owner member, importer file)` of every type-reference entry in `key`.
fn type_refs(answer: &Value, key: &str) -> Vec<(String, String)> {
    rows(answer, key)
        .iter()
        .map(|row| {
            let via = &row["via"];
            (via["owner"]["member"].as_str().unwrap().to_string(), via["importer"]["file"].as_str().unwrap().to_string())
        })
        .collect()
}

fn summary(answer: &Value) -> &str {
    answer["unresolved_egress"]["summary"].as_str().unwrap_or_else(|| panic!("a residue line: {answer:#}"))
}

/// **The fixture is what it claims** — every assertion below rests on it: each
/// fork resolves the queried symbols under its own coordinate and never holds the
/// other's canonical one, and only `svc-b`'s are bridge endpoints, but for
/// `relay`, whose edge runs from one fork to the other.
#[test]
fn the_fixture_binds_only_svc_b_while_both_forks_hold_the_symbols() {
    let f = fixture();
    let edges: Vec<(&str, &str, &str)> = f
        .inputs
        .edges
        .iter()
        .map(|e| (e.from.member.as_str(), e.to.member.as_str(), e.relation.as_str()))
        .collect();
    assert_eq!(
        edges,
        [("orders", "svc-b", "broker-topic"), ("svc-a", "svc-b", "broker-topic"), ("svc-b", "ship", "broker-topic")],
        "{edges:?}"
    );
    assert_eq!(f.inputs.edges[0].to.symbol.as_str(), held_by("svc-b", &on_order()));
    let relayed = &f.inputs.edges[1];
    assert_eq!(
        (relayed.from.symbol.as_str(), relayed.to.symbol.as_str()),
        (held_by("svc-a", &relay()).as_str(), held_by("svc-b", &relay()).as_str())
    );
    assert_eq!(f.inputs.edges[2].from.symbol.as_str(), held_by("svc-b", &ship()));
    for symbol in [on_order(), ship(), relay(), DTO.to_string()] {
        for (fork, other) in [(FORKS[0], FORKS[1]), (FORKS[1], FORKS[0])] {
            let engine = f.registry.engine_for(fork).unwrap();
            let held = held_by(fork, &symbol);
            assert_eq!(engine.callers(&symbol, None).resolved.map(|r| r.symbol), Some(held.clone()), "{fork} holds {symbol}");
            let foreign = held_by(other, &symbol);
            assert_eq!(engine.callers(&foreign, None).resolved.map(|r| r.symbol), None, "{fork} never holds {foreign}");
        }
    }
    let owners: Vec<(&str, Option<&str>)> =
        f.index.references.iter().map(|r| (r.owner.member.as_str(), r.owner.symbol.as_deref())).collect();
    let (dto_a, dto_b) = (held_by("svc-a", DTO), held_by("svc-b", DTO));
    assert_eq!(owners, [("svc-a", Some(dto_a.as_str())), ("svc-b", Some(dto_b.as_str()))], "both Dtos bind, each its own node");
}

/// **Scoped callers** ([BR-73], AC 1): `--repo svc-a` reports no cross-service
/// caller and a resolved count of 0 — the line says whose symbol it counted —
/// while `--repo svc-b` reports `svc-b`'s caller, which lies in `orders`.
#[test]
fn scoped_callers_report_only_the_scoped_members_cross_service_callers() {
    let f = fixture();
    let a = f.callers(&on_order(), Some("svc-a"));
    assert_eq!(rows(&a, "cross_service"), Vec::<Value>::new(), "svc-b's caller is not svc-a's: {a:#}");
    assert!(
        summary(&a).starts_with("no resolved cross-service callers of svc-a's symbol, from any member; "),
        "{}",
        summary(&a)
    );
    assert_eq!(a["unresolved_egress"]["scope"], "svc-a", "the residue stays the member's own");
    assert!(a.get("ambiguity_warning").is_none() && a.get("resolved_by_member").is_none(), "a scope chose: {a:#}");
    assert_eq!(a["members"][0]["result"]["total"], 1, "svc-a's own intra-repo caller is still reported");

    let b = f.callers(&on_order(), Some("svc-b"));
    let cross = rows(&b, "cross_service");
    assert_eq!(cross.len(), 1, "{b:#}");
    assert_eq!((&cross[0]["from"]["member"], &cross[0]["to"]["member"]), (&json!("orders"), &json!("svc-b")));
    assert!(summary(&b).starts_with("1 resolved cross-service caller of svc-b's symbol, from any member; "));
}

/// **Scoped impact** ([BR-73], AC 2), from both ends of an edge: `--repo svc-a`
/// stitches nothing through an edge whose near endpoint is `svc-b`'s — on the
/// provider end (`onOrder`) and the consumer end (`ship`) alike.
#[test]
fn scoped_impact_stitches_no_far_side_through_another_members_endpoint() {
    let f = fixture();
    for (symbol, far) in [(on_order(), "orders"), (ship(), "ship")] {
        let a = f.impact(&symbol, Some("svc-a"));
        assert_eq!(rows(&a, "cross_service"), Vec::<Value>::new(), "{symbol}: {a:#}");
        assert!(summary(&a).starts_with("no resolved cross-service impacts of svc-a's symbol, from any member; "));

        let b = f.impact(&symbol, Some("svc-b"));
        let cross = rows(&b, "cross_service");
        assert_eq!(cross.len(), 1, "{symbol}: {b:#}");
        assert_eq!(cross[0]["member"], far, "the far side lies in any member");
    }
}

/// **Unscoped, grouped and flagged** ([BR-73], AC 3): every cross-service row
/// names its member, the resolved count is broken down per member holding the
/// symbol — `svc-a` at zero — and the notice names both members and `--repo`.
/// The composed line names the same breakdown.
#[test]
fn unscoped_answers_over_a_shared_symbol_are_grouped_per_member_and_flagged() {
    let f = fixture();
    let breakdown = json!([{"member": "svc-a", "resolved": 0}, {"member": "svc-b", "resolved": 1}]);
    let line = "1 resolved cross-service {noun} across the 2 members holding the symbol (svc-a 0, svc-b 1); ";
    for (verb, answer) in [("caller", f.callers(&on_order(), None)), ("impact", f.impact(&on_order(), None))] {
        assert_eq!(answer["resolved_by_member"], breakdown, "{verb}: {answer:#}");
        let warning = answer["ambiguity_warning"].as_str().unwrap_or_else(|| panic!("{verb}: {answer:#}"));
        assert!(warning.contains("svc-a, svc-b") && warning.contains("--repo"), "{warning}");
        assert!(summary(&answer).starts_with(&line.replace("{noun}", verb)), "{}", summary(&answer));
        let cross = rows(&answer, "cross_service");
        assert_eq!(cross.len(), 1);
        let provider = if verb == "caller" { &cross[0]["to"]["member"] } else { &cross[0]["via"]["to"]["member"] };
        assert_eq!(provider, "svc-b", "the row names its provider member");
    }
    // A canonical symbol names one member's: the same answer, never grouped.
    let canonical = held_by("svc-b", &on_order());
    for answer in [f.callers(&canonical, None), f.impact(&canonical, None)] {
        assert_eq!(rows(&answer, "cross_service").len(), 1, "{answer:#}");
        assert!(answer.get("ambiguity_warning").is_none() && answer.get("resolved_by_member").is_none(), "{answer:#}");
    }
}

/// **Type references take the scope on the node path** ([BR-73], AC 5): the
/// `Dto` symbol both forks answer to (each its own node) reaches `UsesA` for
/// `svc-a` and `UsesB` for `svc-b`; unscoped, both, grouped by owner member
/// under the notice.
#[test]
fn type_references_on_the_shared_node_are_scoped_to_their_owner_member() {
    let f = fixture();
    let want = |owner: &str, file: &str| vec![(owner.to_string(), format!("{J}/app/{file}"))];
    for (verb, a, b, all) in [
        ("callers", f.callers(DTO, Some("svc-a")), f.callers(DTO, Some("svc-b")), f.callers(DTO, None)),
        ("impact", f.impact(DTO, Some("svc-a")), f.impact(DTO, Some("svc-b")), f.impact(DTO, None)),
    ] {
        assert_eq!(type_refs(&a, "via_type_reference"), want("svc-a", "UsesA.java"), "{verb}");
        assert_eq!(type_refs(&b, "via_type_reference"), want("svc-b", "UsesB.java"), "{verb}");
        let mut both = want("svc-a", "UsesA.java");
        both.extend(want("svc-b", "UsesB.java"));
        assert_eq!(type_refs(&all, "via_type_reference"), both, "{verb}: grouped by owner, roster order");
        assert!(all["ambiguity_warning"].as_str().is_some_and(|w| w.contains("svc-a, svc-b")), "{verb}: {all:#}");
        for answer in [&a, &b, &all] {
            assert!(answer.get("type_reference_unattributed").is_none(), "every owner is a member: {answer:#}");
        }
    }
}

/// **…and on the dotted-name path** ([BR-73], AC 5): `com.acme.a.Dto` is
/// `svc-a`'s, so `--repo svc-b` reaches nothing through it. One member owns a
/// bound dotted name, so the unscoped answer carries no notice.
#[test]
fn type_references_on_the_dotted_name_path_are_scoped_to_their_owner_member() {
    let f = fixture();
    let fqn = "com.acme.a.Dto";
    let mine = vec![("svc-a".to_string(), format!("{J}/app/UsesA.java"))];
    assert_eq!(type_refs(&f.callers(fqn, Some("svc-a")), "via_type_reference"), mine);
    assert_eq!(type_refs(&f.callers(fqn, Some("svc-b")), "via_type_reference"), Vec::new());
    assert_eq!(type_refs(&f.impact(fqn, Some("svc-b")), "via_type_reference"), Vec::new());
    let all = f.callers(fqn, None);
    assert_eq!(type_refs(&all, "via_type_reference"), mine);
    assert!(all.get("ambiguity_warning").is_none(), "{all:#}");
}

/// **An edge whose two ends both answer to the queried symbol is stitched from
/// each near side** ([BR-73]): `svc-a`'s `relay` publishes to `svc-b`'s, at the
/// same descriptors. Each `--repo` answer reaches the other fork, so the unscoped answer
/// carries both stitches and credits each member with one — each holder's share
/// equals its own scoped answer, never only the edge's first end.
#[test]
fn an_edge_whose_both_ends_hold_the_symbol_is_stitched_from_each_near_side() {
    let f = fixture();
    let far_of = |answer: &Value| -> Vec<String> {
        rows(answer, "cross_service").iter().map(|row| row["member"].as_str().unwrap().to_string()).collect()
    };
    assert_eq!(far_of(&f.impact(&relay(), Some("svc-a"))), ["svc-b"]);
    assert_eq!(far_of(&f.impact(&relay(), Some("svc-b"))), ["svc-a"]);
    let all = f.impact(&relay(), None);
    assert_eq!(far_of(&all), ["svc-b", "svc-a"], "one stitch per near side: {all:#}");
    assert_eq!(
        all["resolved_by_member"],
        json!([{"member": "svc-a", "resolved": 1}, {"member": "svc-b", "resolved": 1}]),
        "each holder's share is its own scoped answer"
    );
    let callers = f.callers(&relay(), None);
    assert_eq!(
        callers["resolved_by_member"],
        json!([{"member": "svc-a", "resolved": 0}, {"member": "svc-b", "resolved": 1}]),
        "callers count the provider end only: {callers:#}"
    );
}

/// **A reference no workspace member owns reaches both read-models, under any
/// scope, never dropped** ([CR-221] CRA-02). The index builder names a roster
/// member on every owner it makes, so the case is an index and a roster that
/// disagree: here `svc-b`'s `Dto` reference is re-owned by `gone`. Impact
/// still reaches its importer.
///
/// [CR-221]: ../../docs/requests/CR-221-cross-service-answers-match-member-and-symbol.md
#[test]
fn a_reference_owned_by_no_member_is_reported_apart_by_both_read_models() {
    let f = fixture();
    let mut index = (*f.index).clone();
    let orphan = index.references.iter_mut().find(|r| r.owner.member == "svc-b").expect("svc-b's Dto reference");
    orphan.owner.member = "gone".to_string();
    let uses_b = vec![("gone".to_string(), format!("{J}/app/UsesB.java"))];
    for scope in [Some("svc-a"), Some("svc-b"), None] {
        let callers = f.callers_over(&index, DTO, scope);
        assert_eq!(type_refs(&callers, "type_reference_unattributed"), uses_b, "callers, {scope:?}: {callers:#}");
        let impact = f.impact_over(&index, DTO, scope);
        assert_eq!(type_refs(&impact, "type_reference_unattributed"), uses_b, "impact, {scope:?}");
        let entry = &impact["type_reference_unattributed"][0];
        assert_eq!(entry["member"], "app", "the importer's reach is computed: {entry:#}");
        assert!(entry["result"]["changed"].is_array(), "{entry:#}");
        for answer in [&callers, &impact] {
            assert!(
                type_refs(answer, "via_type_reference").iter().all(|(owner, _)| owner != "gone"),
                "never attributed: {answer:#}"
            );
        }
    }
}

/// **A bare name that resolves in both forks does not make them holders**
/// ([BR-73] governs one symbol string): `onOrder` resolves to the shared
/// listener in each fork, but no bridge row matches a bare name, and the
/// answer is neither grouped nor flagged.
#[test]
fn a_bare_name_resolving_in_both_forks_is_not_a_shared_symbol() {
    let f = fixture();
    for fork in FORKS {
        let seed = f.registry.engine_for(fork).unwrap().callers("onOrder", None);
        assert_eq!(seed.resolved.map(|r| r.symbol), Some(held_by(fork, &on_order())), "guard: {fork} resolves the bare name");
    }
    for answer in [f.callers("onOrder", None), f.impact("onOrder", None)] {
        assert!(
            answer.get("ambiguity_warning").is_none() && answer.get("resolved_by_member").is_none(),
            "{answer:#}"
        );
    }
}

/// **A far member that will not start contributes no share** ([ADR-53]): with
/// `orders`' store corrupted after the bridge read, the unscoped impact of the
/// shared listener stitches nothing, and the breakdown and the composed line
/// agree with that — each fork at zero — while `orders` is named unread.
///
/// [ADR-53]: ../../docs/specs/architecture/decisions/ADR-53.md
#[test]
fn a_far_member_that_will_not_start_adds_nothing_to_the_breakdown() {
    let f = fixture();
    f.registry.evict_to_capacity(0);
    let store = f.tmp.path().join("orders/.logos");
    for sidecar in ["logos.db-wal", "logos.db-shm"] {
        let _ = fs::remove_file(store.join(sidecar));
    }
    fs::write(store.join("logos.db"), b"this is not a sqlite database").expect("corrupt the far store");

    let impact = f.impact(&on_order(), None);
    assert_eq!(rows(&impact, "cross_service"), Vec::<Value>::new(), "{impact:#}");
    assert_eq!(
        impact["resolved_by_member"],
        json!([{"member": "svc-a", "resolved": 0}, {"member": "svc-b", "resolved": 0}])
    );
    assert!(
        summary(&impact).starts_with("no resolved cross-service impacts across the 2 members holding the symbol (svc-a 0, svc-b 0); "),
        "{}",
        summary(&impact)
    );
    assert!(impact["member_reads"]["unread"]["orders"].is_string(), "{impact:#}");
}
