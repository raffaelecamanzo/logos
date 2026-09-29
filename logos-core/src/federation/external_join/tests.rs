//! Fixtures over the pure join: every positive shape and every refusal
//! [ADR-68] point 3 names, each pinned by its own fixture.
//!
//! [ADR-68]: ../../../../docs/specs/architecture/decisions/ADR-68.md

use super::*;

use std::cell::Cell;

use crate::extract::config::corpus::canonical_key;
use crate::federation::declared_contracts::{
    DeclaredContractHeadline, DocumentAccounting, NamedExternal, VENDORED_SPEC,
};
use crate::model::LogosSymbol;

const APP: &str = "src/main/resources/application.yml";
const PSS_COPY: &str = "src/main/resources/pec-server/pss.yaml";

fn value(key: &str, value: &str, file: &str) -> CommittedValue {
    CommittedValue { key: canonical_key(key), value: value.to_string(), file: file.to_string() }
}

/// `pecserver-facade`'s shape: the base URL and the path keys share the
/// `pec-server` namespace; the application URL carries no path.
fn facade_app() -> Vec<CommittedValue> {
    vec![
        value("pec-server.base-url", "http://localhost:8082", APP),
        value("pec-server.uri-get-mailbox-path", "/domain/{domain}/user/{user}", APP),
        value("pec-server.allow-self-signed-certificates", "false", APP),
    ]
}

/// The four agreeing overlays and the template committing a placeholder.
fn facade_overlays() -> Vec<CommittedValue> {
    vec![
        value("envFrom.PECSERVER_BASEURL", "https://tinvpecprovis01w:8443/prov", "deploy-coll/values.yaml"),
        value("envFrom.PECSERVER_BASEURL", "https://sinvpecprovis01w:8443/prov", "deploy-svil/values.yaml"),
        value("envFrom.PECSERVER_BASEURL", "_#PLACEHOLDER", "deploy-config/values_TEMPLATE.yaml"),
        value("envFrom.SPRING_PROFILES_ACTIVE", "coll", "deploy-coll/values.yaml"),
    ]
}

fn facade_facts() -> BaseFacts {
    BaseFacts { application: facade_app(), overlays: facade_overlays() }
}

fn keys(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|k| canonical_key(k)).collect()
}

fn ops(list: &[(&str, &str)]) -> BTreeSet<OperationKey> {
    list.iter().map(|(m, t)| ((*m).to_string(), (*t).to_string())).collect()
}

fn external_contract(holder: &str, document: &str, external: &str, name: &str, operations: &[(&str, &str)]) -> DeclaredContract {
    DeclaredContract {
        holder: holder.to_string(),
        document: document.to_string(),
        provenance: VENDORED_SPEC,
        target: ContractTarget::External { external: ExternalId(external.to_string()), name: name.to_string() },
        operations: ops(operations),
    }
}

fn relation(contracts: Vec<DeclaredContract>) -> DeclaredContractRelation {
    let mut externals: BTreeMap<ExternalId, NamedExternal> = BTreeMap::new();
    for c in &contracts {
        if let ContractTarget::External { external, name } = &c.target {
            externals
                .entry(external.clone())
                .or_insert_with(|| NamedExternal {
                    id: external.clone(),
                    name: name.clone(),
                    copies: Vec::new(),
                    declared_by: Vec::new(),
                    stand_ins: Vec::new(),
                })
                .declared_by
                .push(c.holder.clone());
        }
    }
    DeclaredContractRelation {
        headline: DeclaredContractHeadline {
            declared_contract_pairs: 0,
            to_member: 0,
            to_external: 0,
            documents: DocumentAccounting::default(),
            named_externals: 0,
            identity_collisions: 0,
            resolved_ties: 0,
            summary: String::new(),
        },
        contracts,
        externals: externals.into_values().collect(),
        collisions: Vec::new(),
        resolved_ties: Vec::new(),
    }
}

const PSS: &str = "pecserver-facade:src/main/resources/pec-server/pss.yaml";

/// `pecserver-facade` declares PSS; its copy carries the `/prov` prefix.
fn pss_relation() -> DeclaredContractRelation {
    relation(vec![external_contract(
        "facade",
        PSS_COPY,
        PSS,
        "PSS",
        &[("GET", "/prov/domain/{}/user/{}"), ("POST", "/prov/session/authenticate")],
    )])
}

fn endpoint(member: &str, symbol: &str) -> BridgeEndpoint {
    BridgeEndpoint { member: member.to_string(), symbol: LogosSymbol::parse(symbol).unwrap() }
}

fn config_call(composition: &str, key: &str) -> JoinCall {
    JoinCall {
        target: format!("{} ${{{key}}}", composition.split(' ').next().unwrap_or_default()),
        compositions: vec![composition.to_string()],
        keys: vec![canonical_key(key)],
    }
}

/// Judge one call of `member` over `facts`, through [`derive`].
fn outcome(member: &str, call: JoinCall, relation: &DeclaredContractRelation, facts: BaseFacts) -> JoinOutcome {
    let population = [(endpoint(member, "local call"), call)];
    let joined = derive(&population, relation, |_| facts.clone()).expect("an external is declared");
    joined.rows.into_iter().next().expect("one row judged").outcome
}

// ── Positive shapes ────────────────────────────────────────────────────────

/// **`pecserver-facade`'s PSS call binds under `/prov` from the overlays**:
/// every overlay overriding `pec-server.base-url` agrees, the placeholder
/// template adds nothing, and the provenance names each overlay, the key and
/// the matched operation.
#[test]
fn a_call_binds_the_external_its_member_vendors_under_the_agreeing_overlay_path() {
    let got = outcome(
        "facade",
        config_call("GET /domain/{domain}/user/{user}", "pec-server.uri-get-mailbox-path"),
        &pss_relation(),
        facade_facts(),
    );
    assert_eq!(
        got,
        JoinOutcome::BoundExternal(ExternalBinding {
            external: ExternalId(PSS.into()),
            name: "PSS".into(),
            document: PSS_COPY.into(),
            operation: "GET /prov/domain/{}/user/{}".into(),
            base: BasePathEvidence {
                path: "/prov".into(),
                origin: BaseOrigin::DeployOverlay,
                sources: vec![
                    BaseSource { file: "deploy-coll/values.yaml".into(), key: "envfrom.pecserverbaseurl".into() },
                    BaseSource { file: "deploy-svil/values.yaml".into(), key: "envfrom.pecserverbaseurl".into() },
                ],
            },
        })
    );
}

/// **`notification-adapter`'s call binds under its application-config base
/// URL**: no admitted overlay overrides the key, and the URL's empty path makes
/// the call's own path the operation.
#[test]
fn a_call_binds_under_the_application_config_base_url_when_no_overlay_overrides_it() {
    let notification = relation(vec![external_contract(
        "notification-adapter",
        "src/main/resources/notification-gateway/notification-gateway-api_v1.yaml",
        "notification-adapter:src/main/resources/notification-gateway/notification-gateway-api_v1.yaml",
        "Notification Gateway",
        &[("POST", "/v1/notification/send")],
    )]);
    let facts = BaseFacts {
        application: vec![
            value("notification-gateway.api.base-url", "http://localhost:8083", APP),
            value("notification-gateway.api.uri-send-notification", "/v1/notification/send", APP),
        ],
        overlays: vec![value("envFrom.SPRING_PROFILES_ACTIVE", "coll", "deploy-coll/values.yaml")],
    };
    let JoinOutcome::BoundExternal(binding) = outcome(
        "notification-adapter",
        config_call("POST /v1/notification/send", "notification-gateway.api.uri-send-notification"),
        &notification,
        facts,
    ) else {
        panic!("the call binds");
    };
    assert_eq!(binding.operation, "POST /v1/notification/send");
    assert_eq!(
        binding.base,
        BasePathEvidence {
            path: String::new(),
            origin: BaseOrigin::ApplicationConfig,
            sources: vec![BaseSource { file: APP.into(), key: "notificationgateway.api.baseurl".into() }],
        }
    );
}

// ── Refusals, one fixture each ─────────────────────────────────────────────

/// **Suffix-only is refused.** Without the overlays the base path is the
/// application URL's empty one, and the call's path is only the tail of the
/// operation — CR-147 §2.1's "suffix inference", never admitted.
#[test]
fn a_suffix_only_match_is_refused() {
    let facts = BaseFacts { application: facade_app(), overlays: Vec::new() };
    assert_eq!(
        outcome(
            "facade",
            config_call("GET /domain/{domain}/user/{user}", "pec-server.uri-get-mailbox-path"),
            &pss_relation(),
            facts,
        ),
        JoinOutcome::Refused(JoinRefusal::SuffixOnly {
            operation: "GET /prov/domain/{}/user/{}".into(),
            base: String::new(),
        })
    );
}

/// **Overlays committing disagreeing base paths are refused, the
/// disagreement named** — each path with its file and key.
#[test]
fn overlays_committing_disagreeing_base_paths_are_refused_naming_each() {
    let mut facts = facade_facts();
    facts.overlays.push(value("envFrom.PECSERVER_BASEURL", "https://p:8443/pss", "deploy-prod/values.yaml"));
    let got = outcome(
        "facade",
        config_call("GET /domain/{domain}/user/{user}", "pec-server.uri-get-mailbox-path"),
        &pss_relation(),
        facts,
    );
    let key = "envfrom.pecserverbaseurl".to_string();
    assert_eq!(
        got,
        JoinOutcome::Refused(JoinRefusal::BasePathsDisagree {
            paths: vec![
                BasePath { path: "/prov".into(), file: "deploy-coll/values.yaml".into(), key: key.clone() },
                BasePath { path: "/prov".into(), file: "deploy-svil/values.yaml".into(), key: key.clone() },
                BasePath { path: "/pss".into(), file: "deploy-prod/values.yaml".into(), key },
            ],
        })
    );
}

/// **An external the calling member does not itself declare is refused**,
/// naming it: `other` commits the same `/prov` base and calls the same path,
/// but only `facade` vendors PSS — and a member declaring nothing at all that
/// no external could match is refused for that.
#[test]
fn an_external_the_calling_member_does_not_itself_declare_is_refused() {
    let got = outcome(
        "other",
        config_call("GET /domain/{domain}/user/{user}", "pec-server.uri-get-mailbox-path"),
        &pss_relation(),
        facade_facts(),
    );
    assert_eq!(
        got,
        JoinOutcome::Refused(JoinRefusal::ExternalNotDeclaredByMember {
            external: ExternalId(PSS.into()),
            name: "PSS".into(),
            operation: "GET /prov/domain/{}/user/{}".into(),
        })
    );
    assert_eq!(
        outcome("other", config_call("GET /orders", "shop.uri-orders"), &pss_relation(), facade_facts()),
        JoinOutcome::Refused(JoinRefusal::NoDeclaredExternal)
    );
}

/// **An uncommitted (environment-only) base path is refused**, naming the key
/// and the file committing the indirection — and an overriding overlay that
/// commits no URL path is the same refusal: the application URL does not come
/// back in its place.
#[test]
fn an_environment_only_base_path_is_refused() {
    let env_only = BaseFacts {
        application: vec![
            value("pec-server.base-url", "${PECSERVER_BASEURL}", APP),
            value("pec-server.uri-get-mailbox-path", "/domain/{domain}/user/{user}", APP),
        ],
        overlays: Vec::new(),
    };
    let call = || config_call("GET /domain/{domain}/user/{user}", "pec-server.uri-get-mailbox-path");
    assert_eq!(
        outcome("facade", call(), &pss_relation(), env_only),
        JoinOutcome::Refused(JoinRefusal::BasePathUncommitted {
            sources: vec![BaseSource { file: APP.into(), key: "pecserver.baseurl".into() }],
        })
    );
    let placeholder_only = BaseFacts {
        application: facade_app(),
        overlays: vec![value("envFrom.PECSERVER_BASEURL", "_#PLACEHOLDER", "deploy-config/values_TEMPLATE.yaml")],
    };
    assert!(matches!(
        outcome("facade", call(), &pss_relation(), placeholder_only),
        JoinOutcome::Refused(JoinRefusal::BasePathUncommitted { .. })
    ));
}

/// A literal target names no key, so it proves no base path — even when its
/// path is exactly an operation's.
#[test]
fn a_literal_call_proves_no_base_path() {
    let literal = JoinCall {
        target: "POST /prov/session/authenticate".into(),
        compositions: vec!["POST /prov/session/authenticate".into()],
        keys: Vec::new(),
    };
    assert_eq!(
        outcome("facade", literal, &pss_relation(), facade_facts()),
        JoinOutcome::Refused(JoinRefusal::NoBaseKey)
    );
}

/// The method is part of the operation, and a path one segment off the suffix
/// boundary reaches nothing.
#[test]
fn a_near_miss_is_no_match() {
    let call = |c: &str| config_call(c, "pec-server.uri-get-mailbox-path");
    for near in ["DELETE /domain/{d}/user/{u}", "GET /main/{d}/user/{u}", "GET /domain/{d}/users/{u}"] {
        assert_eq!(
            outcome("facade", call(near), &pss_relation(), facade_facts()),
            JoinOutcome::Refused(JoinRefusal::NoMatch),
            "{near}"
        );
    }
    // A DELETE operation elsewhere in the copy makes the DELETE call reachable,
    // so the exact comparison itself must hold the method: under `/prov` the
    // path equals only the GET operation's, and that is not this call's.
    let with_delete = relation(vec![external_contract(
        "facade",
        PSS_COPY,
        PSS,
        "PSS",
        &[("GET", "/prov/domain/{}/user/{}"), ("DELETE", "/prov/archive/domain/{}/user/{}")],
    )]);
    assert_eq!(
        outcome("facade", call("DELETE /domain/{d}/user/{u}"), &with_delete, facade_facts()),
        JoinOutcome::Refused(JoinRefusal::NoMatch)
    );
}

/// Two compositions (two profiles) binding two distinct operations name both
/// and bind neither.
#[test]
fn compositions_binding_two_operations_bind_neither() {
    let two = JoinCall {
        target: "GET ${pec-server.uri-get-mailbox-path}".into(),
        compositions: vec!["GET /domain/{d}/user/{u}".into(), "POST /session/authenticate".into()],
        keys: vec![canonical_key("pec-server.uri-get-mailbox-path")],
    };
    assert_eq!(
        outcome("facade", two, &pss_relation(), facade_facts()),
        JoinOutcome::Refused(JoinRefusal::SeveralOperations {
            operations: vec!["GET /prov/domain/{}/user/{}".into(), "POST /prov/session/authenticate".into()],
        })
    );
}

// ── The population, the denominator and the reads ──────────────────────────

/// The denominator is every reference the coverage tier hands over; the
/// accounting sums to it; a member's facts are read once, and only for a member
/// some row of which could match.
#[test]
fn the_denominator_is_every_handed_reference_and_facts_are_read_lazily() {
    let population = [
        (endpoint("facade", "local a"), config_call("GET /domain/{d}/user/{u}", "pec-server.uri-get-mailbox-path")),
        (endpoint("facade", "local b"), config_call("POST /session/authenticate", "pec-server.uri-session")),
        (endpoint("shop", "local c"), config_call("GET /orders", "shop.uri-orders")),
    ];
    let reads = Cell::new(0);
    let joined = derive(&population, &pss_relation(), |member| {
        assert_eq!(member, "facade", "`shop` could match nothing, so it is never read");
        reads.set(reads.get() + 1);
        facade_facts()
    })
    .unwrap();
    assert_eq!(reads.get(), 1, "one read per member, not per row");
    let h = &joined.headline;
    assert_eq!((h.bound_external, h.no_provider_rows), (2, 3));
    assert_eq!(h.accounting.no_declared_external, 1);
    let a = h.accounting;
    assert_eq!(
        a.bound_external
            + a.no_declared_external
            + a.external_not_declared_by_member
            + a.no_base_key
            + a.base_path_uncommitted
            + a.base_paths_disagree
            + a.suffix_only
            + a.no_match
            + a.several_operations,
        h.no_provider_rows,
        "every row is in exactly one bucket"
    );
    assert_eq!(
        h.summary,
        "2 of 3 invocation no-provider-in-workspace REST rows bound to a named external their own member \
         declares (refused: 1 no declared external); declared by vendored specs, never a cross-service edge, \
         and outside egress_resolution"
    );
    assert_eq!(joined.rows_of(&endpoint("facade", "local b")).count(), 1);
    assert_eq!(joined.rows_of(&endpoint("facade", "local z")).count(), 0);
}

/// With no external declared by any member the join has nothing to bind to
/// and publishes nothing, so such a payload is unchanged.
#[test]
fn no_declared_external_publishes_no_join() {
    let population = [(endpoint("facade", "local a"), JoinCall::default())];
    let to_member = DeclaredContract {
        target: ContractTarget::Member { member: "agg".into(), document: "v1.yaml".into(), shared: 1, total: 1 },
        ..external_contract("web", "spec/agg.yaml", "x", "x", &[("GET", "/mail")])
    };
    assert!(derive(&population, &relation(vec![to_member]), |_| BaseFacts::default()).is_none());
    assert!(derive(&population, &relation(Vec::new()), |_| BaseFacts::default()).is_none());
}

/// The wire form: a bound row is `state: bound-external` with its evidence
/// flattened beside `from`; a refused one is `state: refused` with its reason.
#[test]
fn the_wire_form_names_the_state_and_the_reason() {
    let population = [
        (endpoint("facade", "local a"), config_call("GET /domain/{d}/user/{u}", "pec-server.uri-get-mailbox-path")),
        (endpoint("facade", "local b"), config_call("GET /nowhere", "pec-server.uri-nowhere")),
    ];
    let joined = derive(&population, &pss_relation(), |_| facade_facts()).unwrap();
    let json = serde_json::to_value(&joined).unwrap();
    let bound = &json["rows"][0];
    assert_eq!(bound["state"], "bound-external");
    assert_eq!(bound["target"], "GET ${pec-server.uri-get-mailbox-path}");
    assert_eq!(bound["external"], PSS);
    assert_eq!(bound["operation"], "GET /prov/domain/{}/user/{}");
    assert_eq!(bound["base"]["origin"], "deploy-overlay");
    assert_eq!(bound["base"]["sources"][0]["file"], "deploy-coll/values.yaml");
    assert_eq!(json["rows"][1]["state"], "refused");
    assert_eq!(json["rows"][1]["reason"], "no-match");
    assert_eq!(json["headline"]["bound_external"], 1);
    assert_eq!(json["headline"]["no_provider_rows"], 2);
}

// ── The base-path reading ──────────────────────────────────────────────────

/// A call key's own value is never its base, and a key naming no namespace
/// has no base key.
#[test]
fn the_base_url_key_is_another_key_of_the_calls_namespace() {
    let facts = BaseFacts {
        application: vec![value("pec-server.uri-full", "http://h/prov/x", APP)],
        overlays: Vec::new(),
    };
    assert_eq!(base_reading(&keys(&["pec-server.uri-full"]), &facts), BaseReading::NoKey);
    assert_eq!(base_reading(&keys(&["toplevel"]), &facade_facts()), BaseReading::NoKey);
    assert_eq!(base_reading(&BTreeSet::new(), &facade_facts()), BaseReading::NoKey);
    // Another namespace's URL is not this call's base.
    assert_eq!(base_reading(&keys(&["shop.uri-orders"]), &facade_facts()), BaseReading::NoKey);
}

/// Application profiles that commit different base paths disagree, too.
#[test]
fn application_profiles_committing_different_base_paths_disagree() {
    let mut application = facade_app();
    application.push(value("pec-server.base-url", "http://h:1/v2", "src/main/resources/application-prod.yml"));
    let facts = BaseFacts { application, overlays: Vec::new() };
    assert!(matches!(
        base_reading(&keys(&["pec-server.uri-get-mailbox-path"]), &facts),
        BaseReading::Disagree(paths) if paths.len() == 2
    ));
}

// ── The admitted files ─────────────────────────────────────────────────────

/// Helm values and Compose files are overlays; a documentation tree's chart,
/// a raw manifest, an application config file and a non-YAML file are not.
#[test]
fn a_deploy_overlay_is_a_values_or_compose_file_outside_a_documentation_tree() {
    for yes in [
        "deploy-coll/values.yaml",
        "deploy-config/values_TEMPLATE.yaml",
        "charts/api/prod-values.yml",
        "docker-compose.yml",
        "ci/docker-compose.override.yaml",
    ] {
        assert!(is_deploy_overlay(yes), "{yes}");
    }
    for no in [
        "docs/charts/values.yaml",
        "examples/values.yaml",
        "a/documentation/values.yaml",
        "k8s/deployment.yaml",
        "src/main/resources/application.yml",
        "values.json",
        "values.yaml.bak",
    ] {
        assert!(!is_deploy_overlay(no), "{no}");
    }
}

/// **The overlay read opens only files the discovery walk admitted, and adds
/// no key to the configuration corpus.** A hidden `.helm/values.yaml` — the
/// `notification-adapter` shape — is never read, a documentation tree's chart
/// is never read, and the corpus the same walk builds holds exactly the keys
/// it held before the overlays were committed.
#[test]
fn the_overlay_read_opens_only_walk_admitted_files_and_adds_no_corpus_key() {
    let tmp = tempfile::Builder::new().prefix("s459-").tempdir().unwrap();
    let root = tmp.path();
    let write = |rel: &str, text: &str| {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    write(APP, "pec-server:\n  base-url: http://localhost:8082\n  uri-get: /domain/{d}\n");
    let census = |root: &Path| -> BTreeSet<String> {
        ConfigCorpus::discover(root).sources.iter().flat_map(|s| s.values.keys().cloned()).collect()
    };
    let before = census(root);

    write("deploy-coll/values.yaml", "envFrom:\n  PECSERVER_BASEURL: 'https://t:8443/prov'\n");
    write(".helm/values.yaml", "envFrom:\n  PECSERVER_BASEURL: 'http://hidden/api'\n");
    write("docs/example/values.yaml", "envFrom:\n  PECSERVER_BASEURL: 'http://doc/x'\n");
    let facts = BaseFacts::read(root);

    let overlay_files: BTreeSet<&str> = facts.overlays.iter().map(|v| v.file.as_str()).collect();
    assert_eq!(overlay_files, BTreeSet::from(["deploy-coll/values.yaml"]));
    assert!(facts.application.iter().any(|v| v.key == "pecserver.baseurl" && v.file == APP));
    assert_eq!(census(root), before, "the corpus gained no key");
    assert!(!before.contains("envfrom.pecserverbaseurl"));
    assert!(matches!(
        base_reading(&keys(&["pec-server.uri-get"]), &facts),
        BaseReading::One { ref path, origin: BaseOrigin::DeployOverlay, .. } if path == "/prov"
    ));
}
