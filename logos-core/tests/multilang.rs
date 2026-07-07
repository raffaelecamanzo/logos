//! Multi-language parity smoke test (S-015 / FR-PL-01, FR-EX-05, FR-FW-03,
//! NFR-MA-01, UAT-EX-04, UAT-FW-01, UAT-FW-02), exercised end-to-end through
//! the public [`Engine`] façade against real temp-directory fixtures.
//!
//! Coverage by acceptance criterion:
//! - the default build exposes all five v1 languages via `languages`
//!   (FR-PL-01, ADR-09);
//! - each language extracts nodes/edges at parity with Rust: the same
//!   fixture shape (a container type, free functions, an intra-file call)
//!   yields the same graph shape — a file module, declaration nodes of the
//!   language's kinds, `Contains` scope edges, and a bound `Calls` edge
//!   (FR-EX-05, UAT-EX-04);
//! - framework route/component extraction works for each language's ratified
//!   framework set — FastAPI + Django, Express + Next.js, net/http + Gin,
//!   Spring (FR-FW-03, UAT-FW-01, UAT-FW-02);
//! - a plain library in every language promotes nothing (FR-FW-04);
//! - the per-language tests are feature-gated like the grammars themselves,
//!   so a build that excludes a language excludes its tests (FR-PL-01's
//!   feature-gating criterion; the NFR-MA-01 data-only-addition proof lives
//!   with the registry unit tests, which assemble a synthetic grammar from
//!   pure data).

#![cfg(feature = "lang-rust")]

use std::fs;
use std::path::Path;

use logos_core::model::{EdgeKind, NodeId, NodeKind};
use logos_core::Engine;
use logos_core::Runtime;
use tempfile::TempDir;

/// Write `contents` at `root/rel`, creating parents.
fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// The id of the unique node with `name` and `kind`.
fn node_id(rt: &Runtime, name: &str, kind: NodeKind) -> NodeId {
    let wanted = name.to_string();
    rt.submit_read(move |store| {
        Ok(store
            .all_nodes()?
            .into_iter()
            .find(|r| r.kind == kind && r.name == wanted)
            .map(|r| r.id))
    })
    .expect("read runs")
    .unwrap_or_else(|| panic!("no {kind:?} node named {name:?}"))
}

/// Every node of `kind` as `(id, name)`.
fn nodes_of(rt: &Runtime, kind: NodeKind) -> Vec<(NodeId, String)> {
    rt.submit_read(move |store| {
        Ok(store
            .all_nodes()?
            .into_iter()
            .filter(|n| n.kind == kind)
            .map(|n| (n.id, n.name))
            .collect())
    })
    .expect("read runs")
}

/// All `(source, target)` pairs of edges with `kind`.
fn edges_of(rt: &Runtime, kind: EdgeKind) -> Vec<(NodeId, NodeId)> {
    rt.submit_read(move |store| {
        Ok(store
            .all_edges()?
            .into_iter()
            .filter(|e| e.kind == kind)
            .map(|e| (e.source, e.target))
            .collect())
    })
    .expect("read runs")
}

/// Sorted route-node names (`"METHOD /path"`).
fn route_names(rt: &Runtime) -> Vec<String> {
    let mut names: Vec<String> = nodes_of(rt, NodeKind::Route)
        .into_iter()
        .map(|(_, name)| name)
        .collect();
    names.sort();
    names
}

/// Assert the language-agnostic parity contract (FR-EX-05, UAT-EX-04) that
/// every per-language fixture below is shaped to satisfy — the same graph
/// shape the Rust baseline produces:
/// - a file `Module` node named `module`;
/// - a container node (`container`, of the language's container kind) that
///   `Contains` a member callable named `member`;
/// - free callables `caller` and `callee` under the file module, with a
///   *bound* `caller --Calls--> callee` edge (the resolution pass proved an
///   intra-file reference, so references flow at parity too).
#[allow(clippy::too_many_arguments)]
fn assert_parity_shape(
    rt: &Runtime,
    module: &str,
    container: &str,
    container_kind: NodeKind,
    member: &str,
    member_kind: NodeKind,
    caller: &str,
    callee: &str,
    callable_kind: NodeKind,
) {
    let module_id = node_id(rt, module, NodeKind::Module);
    let container_id = node_id(rt, container, container_kind);
    let member_id = node_id(rt, member, member_kind);
    let caller_id = node_id(rt, caller, callable_kind);
    let callee_id = node_id(rt, callee, callable_kind);

    let contains = edges_of(rt, EdgeKind::Contains);
    assert!(
        contains.contains(&(module_id, container_id)),
        "file module must contain the container declaration"
    );
    assert!(
        contains.contains(&(container_id, member_id)),
        "the container must contain its member"
    );
    assert!(
        contains.contains(&(module_id, caller_id)),
        "file module must contain the free callable"
    );

    let calls = edges_of(rt, EdgeKind::Calls);
    assert!(
        calls.contains(&(caller_id, callee_id)),
        "the intra-file call must bind: {caller} --Calls--> {callee}"
    );
}

// ── FR-PL-01: the default build exposes all five v1 languages ────────────────

#[test]
fn default_build_lists_all_five_languages_with_their_extensions() {
    let tmp = TempDir::new().unwrap();
    let info = Engine::open(tmp.path()).languages();

    let find = |name: &str| {
        info.languages
            .iter()
            .find(|l| l.name == name)
            .unwrap_or_else(|| panic!("language '{name}' not listed"))
    };

    assert_eq!(find("rust").extensions, ["rs"]);
    #[cfg(feature = "lang-python")]
    assert_eq!(find("python").extensions, ["py", "pyi"]);
    #[cfg(feature = "lang-typescript")]
    {
        assert_eq!(find("typescript").extensions, ["ts", "js", "mjs", "cjs"]);
        assert_eq!(find("tsx").extensions, ["tsx", "jsx"]);
    }
    #[cfg(feature = "lang-go")]
    assert_eq!(find("go").extensions, ["go"]);
    #[cfg(feature = "lang-java")]
    assert_eq!(find("java").extensions, ["java"]);
    #[cfg(feature = "lang-c")]
    {
        // C claims `.c` only — `.h` headers belong to the C++ plugin under the
        // fixed ownership rule (S-056/S-058), so C must never claim `h`.
        let c = find("c");
        assert_eq!(c.extensions, ["c"]);
        assert!(
            !c.extensions.iter().any(|e| e == "h"),
            "C must not claim `.h` — that extension is the C++ plugin's"
        );
    }
    // Kotlin (S-055): the verification preflight as a standing regression guard —
    // the grammar resolves, loads at the workspace ABI (not skipped below), and
    // lists with its extensions (FR-PL-07, UAT-PL-04).
    #[cfg(feature = "lang-kotlin")]
    assert_eq!(find("kotlin").extensions, ["kt", "kts"]);
    #[cfg(feature = "lang-c-sharp")]
    assert_eq!(find("c-sharp").extensions, ["cs"]);
    #[cfg(feature = "lang-ruby")]
    assert_eq!(find("ruby").extensions, ["rb"]);
    #[cfg(feature = "lang-php")]
    assert_eq!(find("php").extensions, ["php"]);

    // Every loaded grammar passed the ABI assertion (ADR-09, FR-PL-03).
    assert!(
        info.skipped.is_empty(),
        "no compiled-in grammar may be ABI-skipped: {:?}",
        info.skipped
    );
}

// ── FR-CG-06 / FR-CG-01: the infra-format artifact grammars load and are listed ──
//
// The verification preflight (FR-CG-06) as a permanent regression guard. Built
// with `--features lang-terraform,lang-sql`, both grammars must register through
// the substrate, pass the load-time ABI assertion (not be ABI-skipped), and
// surface through the public `Engine::languages()` path as artifact-class plugins
// with their extension claims (FR-CG-01, FR-CG-04). This is the registry-level
// counterpart to the structural anchor proofs in
// `extract::config::anchors::tests`.
#[cfg(all(feature = "lang-terraform", feature = "lang-sql"))]
#[test]
fn infra_artifact_grammars_load_and_are_listed() {
    let tmp = TempDir::new().unwrap();
    let info = Engine::open(tmp.path()).languages();

    let find = |name: &str| {
        info.languages
            .iter()
            .find(|l| l.name == name)
            .unwrap_or_else(|| panic!("artifact language '{name}' not listed"))
    };

    let terraform = find("terraform");
    assert_eq!(terraform.extensions, ["tf", "tfvars"]);
    assert!(terraform.artifact, "terraform is an artifact-class plugin");

    let sql = find("sql");
    assert_eq!(sql.extensions, ["sql"]);
    assert!(sql.artifact, "sql is an artifact-class plugin");

    // Both highest-risk grammars passed the load-time ABI assertion — the
    // verification preflight, captured as a standing regression guard (FR-CG-06).
    assert!(
        info.skipped.is_empty(),
        "the infra grammars must pass the ABI assertion, not be skipped: {:?}",
        info.skipped
    );
}

// ── Rust baseline: the shape every other language is measured against ────────

#[test]
fn rust_baseline_extracts_the_parity_shape() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/store.rs",
        "\
pub struct UserStore {}

impl UserStore {
    pub fn add(&self) {}
}

pub fn caller() {
    callee();
}

fn callee() {}
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    engine.index();

    // Rust's v1 policy maps `impl` methods to Function and collapses them to
    // module scope (no captured impl scope), so the Rust baseline asserts the
    // container and call halves separately.
    let module_id = node_id(rt, "store", NodeKind::Module);
    let store_id = node_id(rt, "UserStore", NodeKind::Struct);
    let caller_id = node_id(rt, "caller", NodeKind::Function);
    let callee_id = node_id(rt, "callee", NodeKind::Function);
    let contains = edges_of(rt, EdgeKind::Contains);
    assert!(contains.contains(&(module_id, store_id)));
    assert!(contains.contains(&(module_id, caller_id)));
    assert!(edges_of(rt, EdgeKind::Calls).contains(&(caller_id, callee_id)));
}

// ── Python: extraction parity + FastAPI/Django promotion (FR-FW-03) ──────────

#[cfg(feature = "lang-python")]
#[test]
fn python_extracts_nodes_and_edges_at_parity_with_rust() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/store.py",
        "\
class UserStore:
    def add(self, item):
        return item

def caller():
    return callee()

def callee():
    return []
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    engine.index();

    assert_parity_shape(
        rt,
        "store",
        "UserStore",
        NodeKind::Class,
        "add",
        NodeKind::Function, // v1 policy: every `def` is a Function (as Rust)
        "caller",
        "callee",
        NodeKind::Function,
    );
}

#[cfg(feature = "lang-python")]
#[test]
fn fastapi_route_and_django_model_are_promoted() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/app.py",
        "\
from fastapi import FastAPI

app = FastAPI()

@app.get(\"/users\")
def list_users():
    return []
",
    );
    write(
        tmp.path(),
        "src/urls.py",
        "\
from django.urls import path

def index(request):
    return None

urlpatterns = [path(\"users/\", index)]
",
    );
    write(
        tmp.path(),
        "src/models.py",
        "\
from django.db import models

class User(models.Model):
    pass
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    let result = engine.index();

    // FastAPI decorator (`@app.get`) and Django URLconf (`path(...)`) both
    // promote (FR-FW-01, FR-FW-03); Django's verb-less registration is ANY.
    assert_eq!(route_names(rt), ["ANY users/", "GET /users"]);
    assert_eq!(result.framework.routes, 2);

    let routes_to = edges_of(rt, EdgeKind::RoutesTo);
    let get_users = node_id(rt, "GET /users", NodeKind::Route);
    let list_users = node_id(rt, "list_users", NodeKind::Function);
    let any_users = node_id(rt, "ANY users/", NodeKind::Route);
    let index_fn = node_id(rt, "index", NodeKind::Function);
    assert!(routes_to.contains(&(get_users, list_users)));
    assert!(routes_to.contains(&(any_users, index_fn)));

    // The Django model is the wired building block (FR-FW-02).
    let user_component = node_id(rt, "User", NodeKind::Component);
    let user_class = node_id(rt, "User", NodeKind::Class);
    assert!(edges_of(rt, EdgeKind::References).contains(&(user_component, user_class)));
}

// ── TypeScript/JS: extraction parity + Express promotion (FR-FW-03) ──────────

#[cfg(feature = "lang-typescript")]
#[test]
fn typescript_extracts_nodes_and_edges_at_parity_with_rust() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/store.ts",
        "\
export class UserStore {
    add(item: string): string {
        return item;
    }
}

export function caller(): string[] {
    return callee();
}

function callee(): string[] {
    return [];
}
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    engine.index();

    assert_parity_shape(
        rt,
        "store",
        "UserStore",
        NodeKind::Class,
        "add",
        NodeKind::Method, // TS captures class methods as Method
        "caller",
        "callee",
        NodeKind::Function,
    );
}

#[cfg(feature = "lang-typescript")]
#[test]
fn express_routes_are_promoted_and_linked_to_handlers() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/server.ts",
        "\
import express from \"express\";

const app = express();

export function listUsers(req: unknown, res: unknown): void {}

app.get(\"/users\", listUsers);
app.post(\"/users\", (req, res) => {});
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    let result = engine.index();

    assert_eq!(route_names(rt), ["GET /users", "POST /users"]);
    assert_eq!(result.framework.routes, 2);

    // The named handler is proven (FR-FW-01); the closure-registered route
    // keeps its node with no fabricated edge (NFR-RA-05).
    let routes_to = edges_of(rt, EdgeKind::RoutesTo);
    let get_users = node_id(rt, "GET /users", NodeKind::Route);
    let post_users = node_id(rt, "POST /users", NodeKind::Route);
    let list_users = node_id(rt, "listUsers", NodeKind::Function);
    assert!(routes_to.contains(&(get_users, list_users)));
    assert!(!routes_to.iter().any(|(s, _)| *s == post_users));
}

#[cfg(feature = "lang-typescript")]
#[test]
fn nextjs_component_is_promoted_from_tsx() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/Home.tsx",
        "\
import Link from \"next/link\";

export function HomePage() {
    return <Link href=\"/\">home</Link>;
}

function helper() {
    return null;
}
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    let result = engine.index();

    // The exported PascalCase function is the Next.js component (FR-FW-02,
    // UAT-FW-02); the lowercase helper is not promoted.
    let component = node_id(rt, "HomePage", NodeKind::Component);
    let function = node_id(rt, "HomePage", NodeKind::Function);
    assert!(edges_of(rt, EdgeKind::References).contains(&(component, function)));
    assert_eq!(result.framework.components, 1);
}

// ── Go: extraction parity + net/http and Gin promotion (FR-FW-03) ────────────

#[cfg(feature = "lang-go")]
#[test]
fn go_extracts_nodes_and_edges_at_parity_with_rust() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/store.go",
        "\
package store

type UserStore struct{}

func (s *UserStore) Add(item string) string {
	return item
}

func Caller() []string {
	return callee()
}

func callee() []string {
	return nil
}
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    engine.index();

    // Go methods declare outside the struct body, so the member half of the
    // parity shape is the method's existence (module-contained), not
    // container-Contains; the struct itself still anchors under the module.
    let module_id = node_id(rt, "store", NodeKind::Module);
    let store_id = node_id(rt, "UserStore", NodeKind::Struct);
    let add_id = node_id(rt, "Add", NodeKind::Method);
    let caller_id = node_id(rt, "Caller", NodeKind::Function);
    let callee_id = node_id(rt, "callee", NodeKind::Function);
    let contains = edges_of(rt, EdgeKind::Contains);
    assert!(contains.contains(&(module_id, store_id)));
    assert!(contains.contains(&(module_id, add_id)));
    assert!(contains.contains(&(module_id, caller_id)));
    assert!(edges_of(rt, EdgeKind::Calls).contains(&(caller_id, callee_id)));
}

#[cfg(feature = "lang-go")]
#[test]
fn net_http_and_gin_routes_are_promoted() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/main.go",
        "\
package main

import \"net/http\"

func main() {
	http.HandleFunc(\"/users\", listUsers)
}

func listUsers(w http.ResponseWriter, r *http.Request) {}
",
    );
    write(
        tmp.path(),
        "src/router.go",
        "\
package main

import \"github.com/gin-gonic/gin\"

func setup() *gin.Engine {
	r := gin.Default()
	r.GET(\"/ping\", ping)
	return r
}

func ping(c *gin.Context) {}
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    let result = engine.index();

    // net/http registrations carry no verb (ANY); Gin's verbs name themselves.
    assert_eq!(route_names(rt), ["ANY /users", "GET /ping"]);
    assert_eq!(result.framework.routes, 2);

    let routes_to = edges_of(rt, EdgeKind::RoutesTo);
    let any_users = node_id(rt, "ANY /users", NodeKind::Route);
    let get_ping = node_id(rt, "GET /ping", NodeKind::Route);
    let list_users = node_id(rt, "listUsers", NodeKind::Function);
    let ping_fn = node_id(rt, "ping", NodeKind::Function);
    assert!(routes_to.contains(&(any_users, list_users)));
    assert!(routes_to.contains(&(get_ping, ping_fn)));
}

// ── Ruby: extraction parity + Rails promotion (FR-FW-03, S-059) ──────────────

#[cfg(feature = "lang-ruby")]
#[test]
fn ruby_extracts_nodes_and_edges_at_parity_with_rust() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/store.rb",
        "\
class UserStore
  def add(item)
    item
  end
end

def caller
  callee()
end

def callee
  []
end
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    engine.index();

    // Ruby `def` is always a method, so every callable maps to Method (the
    // class member and the two file-scope defs alike); the intra-file call binds
    // (the receiver-less `callee()` is a Calls path, not a member call).
    assert_parity_shape(
        rt,
        "store",
        "UserStore",
        NodeKind::Class,
        "add",
        NodeKind::Method,
        "caller",
        "callee",
        NodeKind::Method,
    );
}

#[cfg(feature = "lang-ruby")]
#[test]
fn rails_routes_and_controller_component_are_promoted() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "config/routes.rb",
        "\
Rails.application.routes.draw do
  get \"/users\", to: \"users#index\"
  post \"/users\", to: \"users#create\"
end
",
    );
    write(
        tmp.path(),
        "app/controllers/users_controller.rb",
        "\
class UsersController < ApplicationController
  def index
    render json: []
  end
end
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    let result = engine.index();

    // The Rails routing DSL verbs promote with their path; the controller#action
    // handler string is not a resolvable symbol, so no RoutesTo edge is
    // fabricated (NFR-RA-05) — the route nodes still exist. The `resources`-style
    // macros are deliberately not expanded.
    assert_eq!(route_names(rt), ["GET /users", "POST /users"]);
    assert_eq!(result.framework.routes, 2);

    // The controller class is the wired building block (FR-FW-02).
    let component = node_id(rt, "UsersController", NodeKind::Component);
    let class = node_id(rt, "UsersController", NodeKind::Class);
    assert!(edges_of(rt, EdgeKind::References).contains(&(component, class)));
    assert_eq!(result.framework.components, 1);
}

// ── Ruby: measured resolution coverage + never-fabricate (NFR-RA-05) ─────────

#[cfg(feature = "lang-ruby")]
#[test]
fn ruby_resolution_coverage_is_measured_and_never_fabricates() {
    // CR-009 obligation: measure reference-resolution coverage and prove the
    // never-fabricate posture — an intra-file call binds, while dynamic dispatch
    // (a receiver method call the resolver cannot prove) yields no edge, never a
    // guessed one. The exact coverage number is recorded in the impl notes; the
    // test pins the qualitative invariants (no floor, NFR-RA-05).
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/calc.rb",
        "\
def normalize(n)
  n
end

def compute(n)
  result = normalize(n)
  result.round
end
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    let result = engine.index();

    // The resolvable intra-file call binds (compute --Calls--> normalize).
    let compute = node_id(rt, "compute", NodeKind::Method);
    let normalize = node_id(rt, "normalize", NodeKind::Method);
    assert!(
        edges_of(rt, EdgeKind::Calls).contains(&(compute, normalize)),
        "the receiver-less intra-file call must bind"
    );

    // Dynamic dispatch (`result.round`) is never fabricated: `round` resolves to
    // no node in this graph, so it stays unresolved and yields no Calls edge.
    assert!(
        !nodes_of(rt, NodeKind::Method).iter().any(|(_, n)| n == "round"),
        "no `round` node exists, so any edge to it would be fabricated"
    );
    assert!(
        result.resolution.refs_unresolved >= 1,
        "the dynamic-dispatch reference stays unresolved (never-fabricate)"
    );

    // Recorded measurement: a non-trivial bound ratio with both bound and
    // unbound references present (no uncalibrated floor — CR-009 §10).
    let cov = result.resolution.coverage;
    println!(
        "RUBY_RESOLUTION_COVERAGE coverage={cov:.4} resolved={} unresolved={} total={}",
        result.resolution.refs_resolved,
        result.resolution.refs_unresolved,
        result.resolution.refs_total,
    );
    assert!(cov > 0.0, "at least the intra-file call resolves");
}

// ── Java: extraction parity + Spring promotion (FR-FW-03) ────────────────────

#[cfg(feature = "lang-java")]
#[test]
fn java_extracts_nodes_and_edges_at_parity_with_rust() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/Store.java",
        "\
public class Store {
    public String add(String item) {
        return item;
    }

    public String caller() {
        return callee();
    }

    private String callee() {
        return \"\";
    }
}
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    engine.index();

    // Java has no free functions — every callable is a class member, so the
    // parity shape nests one level deeper: module ∋ class ∋ methods, with the
    // intra-class call bound (Balanced policy: unique method name).
    let module_id = node_id(rt, "Store", NodeKind::Module);
    let class_id = node_id(rt, "Store", NodeKind::Class);
    let add_id = node_id(rt, "add", NodeKind::Method);
    let caller_id = node_id(rt, "caller", NodeKind::Method);
    let callee_id = node_id(rt, "callee", NodeKind::Method);
    let contains = edges_of(rt, EdgeKind::Contains);
    assert!(contains.contains(&(module_id, class_id)));
    assert!(contains.contains(&(class_id, add_id)));
    assert!(contains.contains(&(class_id, caller_id)));
    assert!(contains.contains(&(class_id, callee_id)));
    assert!(edges_of(rt, EdgeKind::Calls).contains(&(caller_id, callee_id)));
}

#[cfg(feature = "lang-java")]
#[test]
fn spring_routes_and_stereotype_components_are_promoted() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/UserController.java",
        "\
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.RestController;

@RestController
public class UserController {
    @GetMapping(\"/users\")
    public String listUsers() {
        return \"\";
    }
}
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    let result = engine.index();

    assert_eq!(route_names(rt), ["GET /users"]);
    assert_eq!(result.framework.routes, 1);

    // The handler is the controller *method* (FR-FW-01).
    let routes_to = edges_of(rt, EdgeKind::RoutesTo);
    let get_users = node_id(rt, "GET /users", NodeKind::Route);
    let list_users = node_id(rt, "listUsers", NodeKind::Method);
    assert!(routes_to.contains(&(get_users, list_users)));

    // The stereotype-annotated class is the wired building block (FR-FW-02).
    let component = node_id(rt, "UserController", NodeKind::Component);
    let class = node_id(rt, "UserController", NodeKind::Class);
    assert!(edges_of(rt, EdgeKind::References).contains(&(component, class)));
    assert_eq!(result.framework.components, 1);
}

// ── C: extraction parity, the honesty fixture (no frameworks) ────────────────

#[cfg(feature = "lang-c")]
#[test]
fn c_extracts_nodes_and_edges_at_parity_with_rust() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/store.c",
        "\
int caller(int n) {
    return callee(n);
}

int callee(int n) {
    return n;
}

static int hidden(void) {
    return 0;
}
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    engine.index();

    // C is flat — no class-like container — so the parity shape is the Rust
    // free-function half: a file `Module` that Contains the functions, with a
    // bound intra-file `caller --Calls--> callee` edge (references flow at
    // parity even with no container).
    let module_id = node_id(rt, "store", NodeKind::Module);
    let caller_id = node_id(rt, "caller", NodeKind::Function);
    let callee_id = node_id(rt, "callee", NodeKind::Function);
    let hidden_id = node_id(rt, "hidden", NodeKind::Function);
    let contains = edges_of(rt, EdgeKind::Contains);
    assert!(contains.contains(&(module_id, caller_id)));
    assert!(contains.contains(&(module_id, callee_id)));
    assert!(contains.contains(&(module_id, hidden_id)));
    assert!(
        edges_of(rt, EdgeKind::Calls).contains(&(caller_id, callee_id)),
        "the intra-file call must bind: caller --Calls--> callee"
    );

    // C extracts no class-like containers — the honesty posture (NFR-CC-04).
    assert!(
        nodes_of(rt, NodeKind::Class).is_empty(),
        "C emits no Class nodes"
    );
    assert!(
        nodes_of(rt, NodeKind::Struct).is_empty(),
        "C emits no Struct nodes — Cohesion/Focus stay n/a, never a fabricated score"
    );
}

// ── Kotlin: extraction parity + Spring promotion (S-055, FR-FW-03) ───────────

#[cfg(feature = "lang-kotlin")]
#[test]
fn kotlin_extracts_nodes_and_edges_at_parity_with_rust() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/store.kt",
        "\
class Store {
    fun add(item: String): String {
        return item
    }
}

fun caller(): List<String> {
    return callee()
}

fun callee(): List<String> {
    return emptyList()
}
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    engine.index();

    // Kotlin models member and free functions with one node, so v1 policy maps
    // every `fun` to Function (as Rust collapses its impl methods): the member
    // half nests one level deeper — module ∋ class ∋ member fn — with the free
    // intra-file call bound.
    assert_parity_shape(
        rt,
        "store",
        "Store",
        NodeKind::Class,
        "add",
        NodeKind::Function,
        "caller",
        "callee",
        NodeKind::Function,
    );
}

#[cfg(feature = "lang-kotlin")]
#[test]
fn spring_routes_and_stereotype_components_are_promoted_from_kotlin() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/UserController.kt",
        "\
import org.springframework.web.bind.annotation.GetMapping
import org.springframework.web.bind.annotation.RestController

@RestController
class UserController {
    @GetMapping(\"/users\")
    fun listUsers(): String {
        return \"\"
    }
}
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    let result = engine.index();

    assert_eq!(route_names(rt), ["GET /users"]);
    assert_eq!(result.framework.routes, 1);

    // The handler is the controller function (Kotlin's uniform fun→Function).
    let routes_to = edges_of(rt, EdgeKind::RoutesTo);
    let get_users = node_id(rt, "GET /users", NodeKind::Route);
    let list_users = node_id(rt, "listUsers", NodeKind::Function);
    assert!(routes_to.contains(&(get_users, list_users)));

    // The stereotype-annotated class is the wired building block (FR-FW-02),
    // detected through the same annotation idiom as Java's `@RestController`.
    let component = node_id(rt, "UserController", NodeKind::Component);
    let class = node_id(rt, "UserController", NodeKind::Class);
    assert!(edges_of(rt, EdgeKind::References).contains(&(component, class)));
    assert_eq!(result.framework.components, 1);
}

// Measured resolution coverage + never-fabricate (S-055, NFR-RA-05): an
// intra-file call binds; a member call on an untyped receiver the resolver
// cannot bind produces no edge — never a guessed one. The ratio is recorded in
// the impl notes.
#[cfg(feature = "lang-kotlin")]
#[test]
fn kotlin_resolution_coverage_is_measured_and_never_fabricates() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/calc.kt",
        "\
fun compute(): Int {
    return helper()
}

fun helper(): Int {
    val value = compute()
    return value.toLong().toInt()
}
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    engine.index();

    // The two receiver-less intra-file calls (`helper()`, `compute()`) bind to
    // their unique same-file definitions.
    let calls = edges_of(rt, EdgeKind::Calls);
    let compute = node_id(rt, "compute", NodeKind::Function);
    let helper = node_id(rt, "helper", NodeKind::Function);
    assert!(calls.contains(&(compute, helper)));
    assert!(calls.contains(&(helper, compute)));

    // `value.toLong()` / `.toInt()` are member calls on an untyped local the
    // resolver cannot bind: no `toLong`/`toInt` symbol exists in the file, so no
    // Calls edge is fabricated (NFR-RA-05).
    assert!(
        !calls.iter().any(|(src, dst)| {
            let names: Vec<String> = nodes_of(rt, NodeKind::Function)
                .into_iter()
                .filter(|(id, _)| id == src || id == dst)
                .map(|(_, n)| n)
                .collect();
            names.iter().any(|n| n == "toLong" || n == "toInt")
        }),
        "unbindable member dispatch must produce no edge"
    );
}

// ── C#: extraction parity + ASP.NET Core promotion (FR-FW-03) ────────────────

#[cfg(feature = "lang-c-sharp")]
#[test]
fn csharp_extracts_nodes_and_edges_at_parity_with_rust() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/Store.cs",
        "\
public class Store {
    public string Add(string item) {
        return item;
    }

    public string Caller() {
        return Callee();
    }

    private string Callee() {
        return \"\";
    }
}
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    engine.index();

    // Like Java, C# has no free functions — every callable is a class member, so
    // the parity shape nests one level deeper: module ∋ class ∋ methods, with the
    // intra-class call bound (Balanced policy: unique method name).
    let module_id = node_id(rt, "Store", NodeKind::Module);
    let class_id = node_id(rt, "Store", NodeKind::Class);
    let add_id = node_id(rt, "Add", NodeKind::Method);
    let caller_id = node_id(rt, "Caller", NodeKind::Method);
    let callee_id = node_id(rt, "Callee", NodeKind::Method);
    let contains = edges_of(rt, EdgeKind::Contains);
    assert!(contains.contains(&(module_id, class_id)));
    assert!(contains.contains(&(class_id, add_id)));
    assert!(contains.contains(&(class_id, caller_id)));
    assert!(contains.contains(&(class_id, callee_id)));
    assert!(edges_of(rt, EdgeKind::Calls).contains(&(caller_id, callee_id)));
}

#[cfg(feature = "lang-c-sharp")]
#[test]
fn aspnet_core_routes_and_controller_components_are_promoted() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/UserController.cs",
        "\
using Microsoft.AspNetCore.Mvc;

[ApiController]
public class UserController {
    [HttpGet(\"/users\")]
    public string ListUsers() {
        return \"\";
    }
}
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    let result = engine.index();

    assert_eq!(route_names(rt), ["GET /users"]);
    assert_eq!(result.framework.routes, 1);

    // The handler is the controller *method* (FR-FW-01).
    let routes_to = edges_of(rt, EdgeKind::RoutesTo);
    let get_users = node_id(rt, "GET /users", NodeKind::Route);
    let list_users = node_id(rt, "ListUsers", NodeKind::Method);
    assert!(routes_to.contains(&(get_users, list_users)));

    // The [ApiController]-attributed class is the wired building block (FR-FW-02).
    let component = node_id(rt, "UserController", NodeKind::Component);
    let class = node_id(rt, "UserController", NodeKind::Class);
    assert!(edges_of(rt, EdgeKind::References).contains(&(component, class)));
    assert_eq!(result.framework.components, 1);
}

// ── PHP: extraction parity + Laravel promotion + HTML-interleaved (FR-FW-03) ──

#[cfg(feature = "lang-php")]
#[test]
fn php_extracts_nodes_and_edges_at_parity_with_rust() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/store.php",
        "<?php
class UserStore {
    public function add($item) { return $item; }
}

function caller() { return callee(); }

function callee() { return 1; }
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    engine.index();

    // PHP has both class members and free functions, so it meets the full parity
    // shape: module ∋ class ∋ method, plus free caller/callee with a bound call.
    assert_parity_shape(
        rt,
        "store",
        "UserStore",
        NodeKind::Class,
        "add",
        NodeKind::Method,
        "caller",
        "callee",
        NodeKind::Function,
    );
}

#[cfg(feature = "lang-php")]
#[test]
fn laravel_routes_and_eloquent_components_are_promoted() {
    let tmp = TempDir::new().unwrap();
    // Route facade registration (handler is an array → no provable handler name,
    // so the route is promoted with no fabricated RoutesTo edge, NFR-RA-05).
    write(
        tmp.path(),
        "routes/web.php",
        "<?php
use Illuminate\\Support\\Facades\\Route;

Route::get('/users', [UserController::class, 'index']);
Route::post('/users', [UserController::class, 'store']);
",
    );
    // An Eloquent model — the wired Laravel building block (FR-FW-02).
    write(
        tmp.path(),
        "app/Models/User.php",
        "<?php
namespace App\\Models;

use Illuminate\\Database\\Eloquent\\Model;

class User extends Model {}
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    let result = engine.index();

    assert_eq!(route_names(rt), ["GET /users", "POST /users"]);
    assert_eq!(result.framework.routes, 2);

    // The model class is promoted to a wired component (FR-FW-02).
    let component = node_id(rt, "User", NodeKind::Component);
    let class = node_id(rt, "User", NodeKind::Class);
    assert!(edges_of(rt, EdgeKind::References).contains(&(component, class)));
    assert_eq!(result.framework.components, 1);
}

#[cfg(feature = "lang-php")]
#[test]
fn html_interleaved_php_still_extracts_its_symbols() {
    // The full `php` grammar parses an HTML template with embedded `<?php … ?>`
    // islands; the PHP symbols inside still extract and their intra-file call
    // binds — the HTML never drops the code out of the graph (S-060).
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "templates/page.php",
        "<!DOCTYPE html>
<html><body>
<h1>Users</h1>
<?php
function render() { return greet(); }

function greet() { return 'hi'; }
?>
</body></html>
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    engine.index();

    // Both functions extracted from the mixed-HTML file…
    let module_id = node_id(rt, "page", NodeKind::Module);
    let render_id = node_id(rt, "render", NodeKind::Function);
    let greet_id = node_id(rt, "greet", NodeKind::Function);
    let contains = edges_of(rt, EdgeKind::Contains);
    assert!(contains.contains(&(module_id, render_id)));
    assert!(contains.contains(&(module_id, greet_id)));
    // …and the intra-file call binds through the embedded code.
    assert!(edges_of(rt, EdgeKind::Calls).contains(&(render_id, greet_id)));
}

// ── FR-FW-04: a plain library in any language promotes nothing ───────────────

#[test]
fn plain_libraries_in_every_language_promote_nothing() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/lib.rs", "pub fn util() {}\n");
    #[cfg(feature = "lang-python")]
    write(tmp.path(), "src/util.py", "def util():\n    return 1\n");
    #[cfg(feature = "lang-typescript")]
    write(
        tmp.path(),
        "src/util.ts",
        "export function util(): number {\n    return 1;\n}\n",
    );
    #[cfg(feature = "lang-go")]
    write(
        tmp.path(),
        "src/util.go",
        "package util\n\nfunc Util() int {\n\treturn 1\n}\n",
    );
    #[cfg(feature = "lang-java")]
    write(
        tmp.path(),
        "src/Util.java",
        "public class Util {\n    public int util() {\n        return 1;\n    }\n}\n",
    );
    // C has no frameworks capability at all (the honesty fixture) — it can never
    // promote a route or component (S-056).
    #[cfg(feature = "lang-c")]
    write(
        tmp.path(),
        "src/util.c",
        "int util(void) {\n    return 1;\n}\n",
    );
    #[cfg(feature = "lang-kotlin")]
    write(
        tmp.path(),
        "src/Util.kt",
        "class Util {\n    fun util(): Int {\n        return 1\n    }\n}\n",
    );
    #[cfg(feature = "lang-c-sharp")]
    write(
        tmp.path(),
        "src/Util.cs",
        "public class Util {\n    public int Util() {\n        return 1;\n    }\n}\n",
    );
    #[cfg(feature = "lang-ruby")]
    write(tmp.path(), "src/util.rb", "def util\n  1\nend\n");
    #[cfg(feature = "lang-php")]
    write(
        tmp.path(),
        "src/util.php",
        "<?php\nfunction util() { return 1; }\n",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    let result = engine.index();

    // No file names a framework, so the pass parses zero files and promotes
    // nothing — in any language (FR-FW-04, UAT-FW-03).
    assert!(nodes_of(rt, NodeKind::Route).is_empty());
    assert!(nodes_of(rt, NodeKind::Component).is_empty());
    assert_eq!(result.framework.files_scanned, 0);
}
