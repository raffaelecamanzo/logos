//! Gin and net/http handlers link to their routes, and Go 1.22 patterns carry
//! their method (S-526, CR-172, FR-FW-03, FR-EX-12, NFR-RA-05) — exercised end
//! to end through the public [`Engine`] façade against temp-directory fixtures.
//!
//! - A route handler written as a method value of the registering method's own
//!   receiver (`s.PullHandler` inside `func (s *Server) …`) links `RoutesTo` the
//!   method of the receiver's type, bound through the type S-509 records — never
//!   to a same-named method of another type.
//! - A method-value handler whose receiver the file does not prove (a parameter,
//!   a field of the receiver) promotes the route, links nothing, and is counted
//!   in `handlers_receiver_unproven`; a handler written through an imported
//!   package (`http.NotFound`) is not that case and is not counted.
//! - A Go 1.22 `ServeMux` pattern (`"GET /api/status"`) names its method and its
//!   path apart: `GET /api/status`, not `ANY GET /api/status`.
//!
//! Fixtures are written inline into temp directories: a fixture tree checked
//! into this repository would be indexed into its own graph.

#![cfg(feature = "lang-go")]

use std::fs;
use std::path::Path;

use logos_core::model::{EdgeKind, NodeId, NodeKind};
use logos_core::Engine;
use logos_core::Runtime;
use tempfile::TempDir;

#[path = "support/graph_fingerprint.rs"]
mod graph_fingerprint;
use graph_fingerprint::graph_fingerprint;

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// The id of the unique node with `name`, `kind` in the file at `file`.
fn node_in(rt: &Runtime, name: &str, kind: NodeKind, file: &str) -> NodeId {
    let (wanted, wanted_file) = (name.to_string(), file.to_string());
    rt.submit_read(move |store| {
        Ok(store
            .all_nodes()?
            .into_iter()
            .find(|n| n.kind == kind && n.name == wanted && n.file_path.as_deref() == Some(&wanted_file))
            .map(|n| n.id))
    })
    .expect("read runs")
    .unwrap_or_else(|| panic!("no {kind:?} node named {name:?} in {file}"))
}

/// The id of the unique `kind` node `name` declared at `line` of `file` — a
/// method name two types share is told apart by where it is declared.
fn node_at(rt: &Runtime, name: &str, kind: NodeKind, file: &str, line: i64) -> NodeId {
    let (wanted, wanted_file) = (name.to_string(), file.to_string());
    rt.submit_read(move |store| {
        Ok(store
            .all_nodes()?
            .into_iter()
            .find(|n| {
                n.kind == kind
                    && n.name == wanted
                    && n.file_path.as_deref() == Some(&wanted_file)
                    && n.start_line == Some(line)
            })
            .map(|n| n.id))
    })
    .expect("read runs")
    .unwrap_or_else(|| panic!("no {kind:?} node named {name:?} at {file}:{line}"))
}

fn route_id(rt: &Runtime, name: &str) -> NodeId {
    let wanted = name.to_string();
    rt.submit_read(move |store| {
        Ok(store
            .all_nodes()?
            .into_iter()
            .find(|n| n.kind == NodeKind::Route && n.name == wanted)
            .map(|n| n.id))
    })
    .expect("read runs")
    .unwrap_or_else(|| panic!("no route named {name:?}"))
}

fn route_names(rt: &Runtime) -> Vec<String> {
    let mut names: Vec<String> = rt
        .submit_read(|store| {
            Ok(store
                .all_nodes()?
                .into_iter()
                .filter(|n| n.kind == NodeKind::Route)
                .map(|n| n.name)
                .collect())
        })
        .expect("read runs");
    names.sort();
    names
}

/// The lines `a` has and `b` lacks, and the reverse, each under its own temp
/// root's name — empty when the two fingerprints agree.
fn fingerprint_difference(a: (&Path, &str), b: (&Path, &str)) -> Vec<String> {
    let strip = |(root, fp): (&Path, &str)| {
        let name = root.file_name().unwrap().to_string_lossy().into_owned();
        fp.replace(&name, "<root>")
    };
    let (fa, fb) = (strip(a), strip(b));
    let (la, lb): (Vec<&str>, Vec<&str>) = (fa.lines().collect(), fb.lines().collect());
    let only = |x: &[&str], y: &[&str], tag: &str| -> Vec<String> {
        x.iter().filter(|l| !y.contains(l)).map(|l| format!("{tag} {l}")).collect()
    };
    let mut diff = only(&la, &lb, "synced only:");
    diff.extend(only(&lb, &la, "fresh only:"));
    diff
}

fn routes_to(rt: &Runtime) -> Vec<(NodeId, NodeId)> {
    rt.submit_read(|store| {
        Ok(store
            .all_edges()?
            .into_iter()
            .filter(|e| e.kind == EdgeKind::RoutesTo)
            .map(|e| (e.source, e.target))
            .collect())
    })
    .expect("read runs")
}

const SERVER: &str = "\
package server

import \"github.com/gin-gonic/gin\"

type Server struct{}

// A same-named method of another type, in the registering file.
type Twin struct{}

func (t *Twin) PullHandler(c *gin.Context) {}

func (s *Server) GenerateRoutes() *gin.Engine {
	r := gin.Default()
	r.GET(\"/api/pull\", s.PullHandler)
	r.POST(\"/api/chat\", s.ChatHandler)
	return r
}

func (s *Server) PullHandler(c *gin.Context) {}

func (s *Server) ChatHandler(c *gin.Context) {}
";

/// A second type in the same package whose method shares a name with one of the
/// server's: a name-only guess would have two candidates.
const OTHER: &str = "\
package server

import \"github.com/gin-gonic/gin\"

type Other struct{}

func (o *Other) PullHandler(c *gin.Context) {}
";

#[test]
fn a_method_value_handler_links_to_its_receiver_types_method() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "server/routes.go", SERVER);
    write(tmp.path(), "server/other.go", OTHER);

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    let result = engine.index();

    assert_eq!(route_names(rt), ["GET /api/pull", "POST /api/chat"]);
    let edges = routes_to(rt);
    let pull = node_at(rt, "PullHandler", NodeKind::Method, "server/routes.go", 19);
    let chat = node_in(rt, "ChatHandler", NodeKind::Method, "server/routes.go");
    let other_pull = node_in(rt, "PullHandler", NodeKind::Method, "server/other.go");
    let twin_pull = node_at(rt, "PullHandler", NodeKind::Method, "server/routes.go", 10);
    assert!(edges.contains(&(route_id(rt, "GET /api/pull"), pull)), "{edges:?}");
    assert!(edges.contains(&(route_id(rt, "POST /api/chat"), chat)), "{edges:?}");
    assert!(
        !edges.iter().any(|&(_, target)| target == other_pull || target == twin_pull),
        "a same-named method of another type is never the handler: {edges:?}"
    );
    assert_eq!(result.framework.handlers_receiver_unproven, 0);
}

/// Two types whose methods share a name and whose receivers share a name, in
/// one file, with a free function on either side: the registering method's own
/// receiver is the only proof, and it holds inside that method alone.
const SAME_FILE: &str = "\
package server

import \"github.com/gin-gonic/gin\"

type A struct{}
type B struct{}

func before(r *gin.Engine, s *B) { r.GET(\"/before\", s.H) }
func (s *A) Routes(r *gin.Engine) { r.GET(\"/a\", s.H) }
func (s *B) Routes(r *gin.Engine) { r.GET(\"/b\", s.H) }
func after(r *gin.Engine, s *A) { r.GET(\"/after\", s.H) }

func (s *A) H(c *gin.Context) {}
func (s *B) H(c *gin.Context) {}
";

#[test]
fn a_receiver_proves_a_handler_only_inside_its_own_method() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "server/routes.go", SAME_FILE);

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    let result = engine.index();

    let a_h = node_at(rt, "H", NodeKind::Method, "server/routes.go", 13);
    let b_h = node_at(rt, "H", NodeKind::Method, "server/routes.go", 14);
    let edges = routes_to(rt);
    assert_eq!(
        edges.iter().copied().collect::<std::collections::BTreeSet<_>>(),
        [(route_id(rt, "GET /a"), a_h), (route_id(rt, "GET /b"), b_h)].into_iter().collect(),
        "each registering method links its own type's H, and nothing else links"
    );
    // The free functions' `s` is a parameter, before and after the methods.
    assert_eq!(result.framework.handlers_receiver_unproven, 2);
}

/// Two methods declared on one line share a line range, so the range names no
/// method: the registration gets no edge rather than the other type's `H`.
#[test]
fn methods_sharing_a_line_range_name_no_registering_method() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "server/routes.go",
        "\
package server

import \"github.com/gin-gonic/gin\"

type A struct{}
type B struct{}

func (a *A) Aaa(r *gin.Engine) { r.GET(\"/x\", a.H) }; func (b *B) Zzz() {}

func (b *B) H(c *gin.Context) {}
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    engine.index();

    let edges = routes_to(rt);
    let x = route_id(rt, "GET /x");
    assert!(!edges.iter().any(|&(source, _)| source == x), "{edges:?}");
}

#[test]
fn a_method_value_handler_whose_receiver_is_not_proven_stays_unlinked_and_counted() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "server/routes.go", SERVER);
    write(
        tmp.path(),
        "server/wire.go",
        "\
package server

import (
	\"net/http\"

	\"github.com/foo/go-bar\"
	\"github.com/gin-gonic/gin\"
	\"github.com/labstack/echo/v4\"
	\"gopkg.in/yaml.v3\"
)

// srv is a parameter: nothing in the file says what type it has.
func wire(r *gin.Engine, srv *Server) {
	r.GET(\"/wired/pull\", srv.PullHandler)
	r.GET(\"/wired/missing\", http.NotFound)
	// Packages whose import path ends in a version, a version suffix or a prefix.
	r.GET(\"/wired/echo\", echo.NotFoundHandler)
	r.GET(\"/wired/yaml\", yaml.Handler)
	r.GET(\"/wired/bar\", bar.Handler)
}

type Holder struct {
	deps *Server
}

// A field of the receiver is not the receiver.
func (h *Holder) Routes(r *gin.Engine) {
	r.GET(\"/held/pull\", h.deps.PullHandler)
}

// A parameter of a method is not that method's receiver either.
func (h *Holder) Attach(r *gin.Engine, srv *Server) {
	r.GET(\"/held/param\", srv.PullHandler)
}
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    let result = engine.index();

    assert_eq!(
        route_names(rt),
        [
            "GET /api/pull",
            "GET /held/param",
            "GET /held/pull",
            "GET /wired/bar",
            "GET /wired/echo",
            "GET /wired/missing",
            "GET /wired/pull",
            "GET /wired/yaml",
            "POST /api/chat"
        ]
    );
    let edges = routes_to(rt);
    for unlinked in ["GET /wired/pull", "GET /held/pull", "GET /held/param", "GET /wired/missing"] {
        let id = route_id(rt, unlinked);
        assert!(
            !edges.iter().any(|&(source, _)| source == id),
            "{unlinked} must stay unlinked: {edges:?}"
        );
    }
    // `srv.PullHandler` (twice) and `h.deps.PullHandler`; `http.NotFound` and
    // the three versioned or prefixed packages are imported packages'
    // functions, not receivers the file failed to prove.
    assert_eq!(result.framework.handlers_receiver_unproven, 3);
}

#[test]
fn a_receiver_of_another_method_does_not_prove_a_handler_outside_it() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "server/routes.go", SERVER);
    write(
        tmp.path(),
        "server/free.go",
        "\
package server

import \"github.com/gin-gonic/gin\"

// `s` here is a parameter of a free function, not the receiver of `GenerateRoutes`.
func attach(r *gin.Engine, s *Server) {
	r.GET(\"/free/pull\", s.PullHandler)
}
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    let result = engine.index();

    let edges = routes_to(rt);
    let free = route_id(rt, "GET /free/pull");
    assert!(!edges.iter().any(|&(source, _)| source == free), "{edges:?}");
    assert_eq!(result.framework.handlers_receiver_unproven, 1);
}

#[test]
fn a_go_122_pattern_names_its_method_and_its_path_apart() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "main.go",
        "\
package main

import \"net/http\"

func routes() *http.ServeMux {
	mux := http.NewServeMux()
	mux.HandleFunc(\"GET /api/status\", status)
	mux.Handle(\"POST /api/items\", http.HandlerFunc(items))
	mux.HandleFunc(\"DELETE   /api/items/{id}\", remove)
	mux.HandleFunc(\"GET example.com/api/host\", host)
	mux.HandleFunc(\"/legacy\", legacy)
	mux.HandleFunc(\"GETX /api/near\", near)
	mux.HandleFunc(\"get /api/lower\", lower)
	mux.HandleFunc(\" GET /api/trim \", trimmed)
	mux.HandleFunc(`PUT /api/raw`, raw)
	return mux
}

func status(w http.ResponseWriter, r *http.Request) {}
func items(w http.ResponseWriter, r *http.Request) {}
func remove(w http.ResponseWriter, r *http.Request) {}
func host(w http.ResponseWriter, r *http.Request) {}
func legacy(w http.ResponseWriter, r *http.Request) {}
func near(w http.ResponseWriter, r *http.Request) {}
func lower(w http.ResponseWriter, r *http.Request) {}
func trimmed(w http.ResponseWriter, r *http.Request) {}
func raw(w http.ResponseWriter, r *http.Request) {}
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    engine.index();

    // Only a leading upper-case standard verb followed by whitespace is a
    // method: `GETX` and `get` are part of the path text, as they were.
    assert_eq!(
        route_names(rt),
        [
            "ANY /legacy",
            "ANY GETX /api/near",
            "ANY get /api/lower",
            "DELETE /api/items/{id}",
            "GET /api/status",
            "GET /api/trim",
            "GET example.com/api/host",
            "POST /api/items",
            "PUT /api/raw",
        ]
    );
    // The handler still links under the split name.
    let status = node_in(rt, "status", NodeKind::Function, "main.go");
    let remove = node_in(rt, "remove", NodeKind::Function, "main.go");
    let edges = routes_to(rt);
    assert!(edges.contains(&(route_id(rt, "GET /api/status"), status)), "{edges:?}");
    assert!(edges.contains(&(route_id(rt, "DELETE /api/items/{id}"), remove)), "{edges:?}");
}

const HANDLERS: &str = "\
package server

import \"github.com/gin-gonic/gin\"

func (s *Server) PullHandler(c *gin.Context) {}

func (s *Server) ChatHandler(c *gin.Context) {}
";

fn routes_file(pull: &str, status: &str) -> String {
    format!(
        "\
package server

import (
	\"net/http\"

	\"github.com/gin-gonic/gin\"
)

type Server struct{{}}

func (s *Server) Routes(r *gin.Engine, mux *http.ServeMux) {{
	r.GET(\"/api/pull\", s.{pull})
	mux.HandleFunc(\"{status}\", s.{pull})
}}
"
    )
}

/// NFR-RA-06: a registering file edited to a different handler and a different
/// verb, synced alone, is exactly the graph a fresh index of the edited tree
/// builds — the route nodes, their names and their `RoutesTo` edges.
#[test]
fn a_synced_registration_edit_equals_a_fresh_index_of_the_edited_tree() {
    let tree = |routes: &str| {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "server/routes.go", routes);
        write(tmp.path(), "server/handlers.go", HANDLERS);
        tmp
    };
    let before = routes_file("PullHandler", "GET /api/status");
    let after = routes_file("ChatHandler", "POST /api/status");

    let tmp_a = tree(&before);
    let engine_a = Engine::start(tmp_a.path()).expect("engine starts");
    engine_a.index();
    assert_eq!(routes_to(engine_a.runtime().unwrap()).len(), 2, "both routes link before the edit");

    let root_a = tmp_a.path().canonicalize().expect("canonicalize root");
    fs::write(tmp_a.path().join("server/routes.go"), &after).unwrap();
    engine_a.sync(&[root_a.join("server/routes.go")]);
    assert_eq!(route_names(engine_a.runtime().unwrap()), ["GET /api/pull", "POST /api/status"]);
    let chat = node_in(engine_a.runtime().unwrap(), "ChatHandler", NodeKind::Method, "server/handlers.go");
    assert!(
        routes_to(engine_a.runtime().unwrap()).iter().all(|&(_, target)| target == chat),
        "both routes follow the edit to ChatHandler"
    );

    let tmp_b = tree(&after);
    let engine_b = Engine::start(tmp_b.path()).expect("engine starts");
    engine_b.index();
    let diff = fingerprint_difference(
        (tmp_a.path(), &graph_fingerprint(engine_a.runtime().unwrap())),
        (tmp_b.path(), &graph_fingerprint(engine_b.runtime().unwrap())),
    );
    assert!(diff.is_empty(), "sync-to-state must equal index-of-state: {diff:#?}");
}

/// The same property for the handler's file: a method added to the declaring
/// file, synced alone, leaves the graph a fresh index of the edited tree builds.
#[test]
fn a_synced_handler_file_edit_equals_a_fresh_index_of_the_edited_tree() {
    let routes = routes_file("PullHandler", "GET /api/status");
    let edited = format!("{HANDLERS}\nfunc (s *Server) ExtraHandler(c *gin.Context) {{}}\n");
    let tree = |handlers: &str| {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "server/routes.go", &routes);
        write(tmp.path(), "server/handlers.go", handlers);
        tmp
    };
    let tmp_a = tree(HANDLERS);
    let engine_a = Engine::start(tmp_a.path()).expect("engine starts");
    engine_a.index();
    let root_a = tmp_a.path().canonicalize().expect("canonicalize root");
    fs::write(tmp_a.path().join("server/handlers.go"), &edited).unwrap();
    engine_a.sync(&[root_a.join("server/handlers.go")]);
    assert_eq!(routes_to(engine_a.runtime().unwrap()).len(), 2, "the links survive the handler file's re-extraction");

    let tmp_b = tree(&edited);
    let engine_b = Engine::start(tmp_b.path()).expect("engine starts");
    engine_b.index();
    let diff = fingerprint_difference(
        (tmp_a.path(), &graph_fingerprint(engine_a.runtime().unwrap())),
        (tmp_b.path(), &graph_fingerprint(engine_b.runtime().unwrap())),
    );
    assert!(diff.is_empty(), "sync-to-state must equal index-of-state: {diff:#?}");
}

/// The handler a route links to is deleted by a sync of its file alone: the route
/// keeps its node, loses its edge, and leaves no ledger row a fresh index of the
/// edited tree would not hold.
#[test]
fn a_synced_handler_deletion_equals_a_fresh_index_of_the_edited_tree() {
    let routes = routes_file("PullHandler", "GET /api/status");
    let edited = HANDLERS.replace("PullHandler", "PullHandlerV2");
    let tree = |handlers: &str| {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "server/routes.go", &routes);
        write(tmp.path(), "server/handlers.go", handlers);
        tmp
    };
    let tmp_a = tree(HANDLERS);
    let engine_a = Engine::start(tmp_a.path()).expect("engine starts");
    engine_a.index();
    assert_eq!(routes_to(engine_a.runtime().unwrap()).len(), 2, "both routes link before the edit");
    let root_a = tmp_a.path().canonicalize().expect("canonicalize root");
    fs::write(tmp_a.path().join("server/handlers.go"), &edited).unwrap();
    engine_a.sync(&[root_a.join("server/handlers.go")]);
    assert_eq!(routes_to(engine_a.runtime().unwrap()).len(), 0, "the renamed handler is no longer linked");

    let tmp_b = tree(&edited);
    let engine_b = Engine::start(tmp_b.path()).expect("engine starts");
    engine_b.index();
    let diff = fingerprint_difference(
        (tmp_a.path(), &graph_fingerprint(engine_a.runtime().unwrap())),
        (tmp_b.path(), &graph_fingerprint(engine_b.runtime().unwrap())),
    );
    assert!(diff.is_empty(), "sync-to-state must equal index-of-state: {diff:#?}");
}
