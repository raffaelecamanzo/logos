//! Cross-member end-to-end for the **Java** HTTP client-call arm (S-341,
//! [CR-108], [FR-WS-08], [FR-WS-12], [ADR-54]).
//!
//! The Rust twin of this test (`xservice_http_client_call.rs`, S-252) proves the
//! bridge; this one proves that Java's `invocations.scm` feeds it. Two member
//! repositories are indexed by their own [`Engine`] over the real
//! `tree-sitter-java` grammar: a Spring `RestClient` fluent call in `web`, and a
//! Spring `@RestController` route in `api`. It pins the two acceptance criteria
//! a fixture-only test cannot reach:
//!
//! - a static Java client call in member A binds a `Route` provider in member B
//!   through the shared positional `route_key` ([FR-CG-09]) — across the
//!   `{id}`/`{userId}` param-name drift, so consumer and provider provably meet
//!   on one key rather than on matching text;
//! - the resulting edge carries the **`invocation`** intake, so it seeds an
//!   app-wide reachability live root ([FR-WS-12], [CR-083]) — a contract-surface
//!   edge would not.
//!
//! [CR-083]: ../../docs/requests/CR-083-reachability-invocation-edge-roots.md
//! [CR-108]: ../../docs/requests/CR-108-per-language-http-client-call-capture.md
//! [FR-CG-09]: ../../docs/specs/requirements/FR-CG-09.md
//! [FR-WS-08]: ../../docs/specs/requirements/FR-WS-08.md
//! [FR-WS-12]: ../../docs/specs/requirements/FR-WS-12.md
//! [ADR-54]: ../../docs/specs/architecture/decisions/ADR-54.md
#![cfg(feature = "lang-java")]

use std::fs;
use std::path::{Path, PathBuf};

use logos_core::federation::{
    cross_service_coverage, BridgeIntake, ContractBridge, EngineRegistry, Federation, Member,
    RegistryMode,
};
use logos_core::Engine;

/// The consumer: a Spring `RestClient` fluent chain making a static
/// `GET /users/{id}`. The `org.springframework.web.client` import is what makes
/// the file a client-call candidate under the arm's ledger gate.
const JAVA_CLIENT: &str = r#"package com.example.web;

import org.springframework.web.client.RestClient;

public class UserGateway {
    private RestClient restClient;

    public String fetchUser(String id) {
        return restClient.get().uri("/users/{id}").retrieve().body(String.class);
    }
}
"#;

/// The consumer with a runtime-composed path — the base URL is joined at
/// runtime, so the arm refuses it (`base-url-runtime`): no reference, no bind,
/// even though a matching provider exists ([NFR-RA-05]).
const JAVA_CLIENT_COMPOSED: &str = r#"package com.example.web;

import org.springframework.web.client.RestClient;

public class UserGateway {
    private RestClient restClient;
    private String baseUrl;

    public String fetchUser(String id) {
        return restClient.get().uri(baseUrl + "/users/" + id).retrieve().body(String.class);
    }
}
"#;

/// The provider: a Spring controller registering `GET /users/{userId}` — the
/// param name drifts from the consumer's `{id}`, which the positional
/// `route_key` erases.
const SPRING_CONTROLLER: &str = r#"package com.example.api;

import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.RestController;

@RestController
public class UserController {

    @GetMapping("/users/{userId}")
    public String getUser(String userId) {
        return userId;
    }
}
"#;

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().expect("has parent")).expect("mkdir");
    fs::write(path, contents).expect("write fixture");
}

/// Index a member repo into its own `.logos/logos.db`, then drop the engine so
/// the store is closed before the registry re-opens it.
fn index_member(root: &Path) {
    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    let _ = engine.sync(&[] as &[PathBuf]);
}

fn member(name: &str, root: &Path) -> Member {
    Member {
        name: name.to_string(),
        root: root.to_path_buf(),
    }
}

fn federation(root: &Path, members: Vec<Member>) -> Federation {
    Federation {
        name: "pec".to_string(),
        root: root.to_path_buf(),
        members,
        default: None,
        links: Vec::new(),
        governance: Default::default(),
        warm_concurrency: None,
    }
}

/// A static Java client call in `web` binds the Spring route in `api` across the
/// param-name drift, and the edge carries the `invocation` intake ([FR-WS-12]).
#[test]
fn a_static_java_client_call_binds_a_spring_route_in_another_member() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    let web = root.join("web");
    let api = root.join("api");
    write(&web, "src/UserGateway.java", JAVA_CLIENT);
    write(&api, "src/UserController.java", SPRING_CONTROLLER);
    index_member(&web);
    index_member(&api);

    let registry = EngineRegistry::<Engine>::new(
        federation(root, vec![member("web", &web), member("api", &api)]),
        RegistryMode::Lazy,
    );
    let edges = ContractBridge::new().edges(&registry);

    assert_eq!(
        edges.len(),
        1,
        "the Java client call binds its cross-member Spring route: {edges:?}"
    );
    let edge = &edges[0];
    assert_eq!(edge.relation, "route");
    assert_eq!(edge.from.member, "web", "the call site is in member `web`");
    assert_eq!(edge.to.member, "api", "the Spring route is in member `api`");
    assert_eq!(
        edge.intake,
        BridgeIntake::Invocation,
        "an HTTP client call is an invocation, so it seeds an app-wide \
         reachability live root — a contract-surface edge would not (FR-WS-12)"
    );

    // The coverage read-model reports the same call as `bound`.
    let coverage = cross_service_coverage(&registry);
    assert_eq!(coverage.bound, 1, "the Java client call is bound");
    assert_eq!(coverage.ambiguous, 0);
}

/// A runtime-composed Java client call never binds, even against a provider that
/// would otherwise match — no approximate edge is fabricated ([NFR-RA-05]).
#[test]
fn a_runtime_composed_java_client_call_never_binds() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    let web = root.join("web");
    let api = root.join("api");
    write(&web, "src/UserGateway.java", JAVA_CLIENT_COMPOSED);
    write(&api, "src/UserController.java", SPRING_CONTROLLER);
    index_member(&web);
    index_member(&api);

    let registry = EngineRegistry::<Engine>::new(
        federation(root, vec![member("web", &web), member("api", &api)]),
        RegistryMode::Lazy,
    );

    let edges = ContractBridge::new().edges(&registry);
    assert!(
        edges.is_empty(),
        "a base-url-runtime call never binds — no approximate edge: {edges:?}"
    );
}
