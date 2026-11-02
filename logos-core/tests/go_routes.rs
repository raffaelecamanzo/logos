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
    let result = engine.pipeline().index();

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
    let result = engine.pipeline().index();

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
    engine.pipeline().index();

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
    let result = engine.pipeline().index();

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
    let result = engine.pipeline().index();

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
    engine.pipeline().index();

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
    engine_a.pipeline().index();
    assert_eq!(routes_to(engine_a.runtime().unwrap()).len(), 2, "both routes link before the edit");

    let root_a = tmp_a.path().canonicalize().expect("canonicalize root");
    fs::write(tmp_a.path().join("server/routes.go"), &after).unwrap();
    engine_a.pipeline().sync(&[root_a.join("server/routes.go")]);
    assert_eq!(route_names(engine_a.runtime().unwrap()), ["GET /api/pull", "POST /api/status"]);
    let chat = node_in(engine_a.runtime().unwrap(), "ChatHandler", NodeKind::Method, "server/handlers.go");
    assert!(
        routes_to(engine_a.runtime().unwrap()).iter().all(|&(_, target)| target == chat),
        "both routes follow the edit to ChatHandler"
    );

    let tmp_b = tree(&after);
    let engine_b = Engine::start(tmp_b.path()).expect("engine starts");
    engine_b.pipeline().index();
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
    engine_a.pipeline().index();
    let root_a = tmp_a.path().canonicalize().expect("canonicalize root");
    fs::write(tmp_a.path().join("server/handlers.go"), &edited).unwrap();
    engine_a.pipeline().sync(&[root_a.join("server/handlers.go")]);
    assert_eq!(routes_to(engine_a.runtime().unwrap()).len(), 2, "the links survive the handler file's re-extraction");

    let tmp_b = tree(&edited);
    let engine_b = Engine::start(tmp_b.path()).expect("engine starts");
    engine_b.pipeline().index();
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
    engine_a.pipeline().index();
    assert_eq!(routes_to(engine_a.runtime().unwrap()).len(), 2, "both routes link before the edit");
    let root_a = tmp_a.path().canonicalize().expect("canonicalize root");
    fs::write(tmp_a.path().join("server/handlers.go"), &edited).unwrap();
    engine_a.pipeline().sync(&[root_a.join("server/handlers.go")]);
    assert_eq!(routes_to(engine_a.runtime().unwrap()).len(), 0, "the renamed handler is no longer linked");

    let tmp_b = tree(&edited);
    let engine_b = Engine::start(tmp_b.path()).expect("engine starts");
    engine_b.pipeline().index();
    let diff = fingerprint_difference(
        (tmp_a.path(), &graph_fingerprint(engine_a.runtime().unwrap())),
        (tmp_b.path(), &graph_fingerprint(engine_b.runtime().unwrap())),
    );
    assert!(diff.is_empty(), "sync-to-state must equal index-of-state: {diff:#?}");
}

// ── Gin route groups (S-821, CR-255, FR-FW-03, FR-FW-05, NFR-RA-05) ──────────

/// The `RoutesTo` edges as `(route name, handler name)` pairs, sorted.
fn route_links(rt: &Runtime) -> Vec<(String, String)> {
    let mut links: Vec<(String, String)> = rt
        .submit_read(|store| {
            let nodes: std::collections::HashMap<NodeId, String> =
                store.all_nodes()?.into_iter().map(|n| (n.id, n.name)).collect();
            Ok(store
                .all_edges()?
                .into_iter()
                .filter(|e| e.kind == EdgeKind::RoutesTo)
                .map(|e| (nodes[&e.source].clone(), nodes[&e.target].clone()))
                .collect())
        })
        .expect("read runs");
    links.sort();
    links
}

/// The story's fixture: a root route, a group bound to a local, and a group
/// whose routes sit in the `{ … }` block Gin code writes. The closure handler
/// is the registration only the handler-less pattern reads.
const GIN_GROUPS: &str = "\
package main

import \"github.com/gin-gonic/gin\"

func root(c *gin.Context) {}
func list(c *gin.Context) {}

func main() {
	r := gin.Default()
	r.GET(\"/\", root)
	api := r.Group(\"/api\")
	api.GET(\"/\", list)
	api.GET(\"/items\", list)
	api.GET(\"/inline\", func(c *gin.Context) {})
	v1 := r.Group(\"/v1\")
	{
		v1.GET(\"/\", list)
	}
	r.Run()
}
";

/// Before S-821: `GET /` and `GET /items` only — each group route promoted as if
/// absolute, and the two `GET(\"/\")` on a group lost to the root's `GET /`.
#[test]
fn gin_group_routes_carry_their_group_prefix() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "main.go", GIN_GROUPS);

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    let result = engine.pipeline().index();

    assert_eq!(route_names(rt), ["GET /", "GET /api/", "GET /api/inline", "GET /api/items", "GET /v1/"]);
    assert_eq!(
        route_links(rt),
        [
            ("GET /".to_string(), "root".to_string()),
            ("GET /api/".to_string(), "list".to_string()),
            ("GET /api/items".to_string(), "list".to_string()),
            ("GET /v1/".to_string(), "list".to_string()),
        ]
    );
    assert_eq!(result.framework.routes_not_composed, 0);
}

/// A group called inline and a group of a group, bound or chained; middleware
/// returns the group it was given, and a group written without a leading slash
/// is joined from the root as Gin joins it, and so is a raw-string path. Every
/// binding form the query reads carries its group: the second name of a `:=`,
/// a `var` (alone or in a list), and a package-level `var` used above its
/// declaration.
#[test]
fn chained_and_nested_gin_groups_compose_every_prefix() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "main.go",
        "\
package main

import \"github.com/gin-gonic/gin\"

func h(c *gin.Context) {}
func auth() gin.HandlerFunc { return nil }

func early() { pkg.GET(\"/p\", h) }

func main() {
	r := gin.New()
	r.Group(\"/api\").GET(\"/x\", h)
	api := r.Group(\"/api\")
	api.Group(\"/v1\").GET(\"/y\", h)
	v2 := r.Group(\"v2\")
	v2.POST(\"items\", h)
	r.Group(\"/m\").Use(auth()).GET(\"/z\", h)
	r.Group(`/raw`).GET(\"/r\", h)
	a, b := r.Group(\"/a\"), r.Group(\"/b\")
	a.GET(\"/q\", h)
	b.GET(\"/q\", h)
	var v3 = r.Group(\"/v3\")
	v3.GET(\"/s\", h)
	var (
		w = r.Group(\"/w\")
	)
	w.GET(\"/t\", h)
}

var pkg = gin.New().Group(\"/pkg\")
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    let result = engine.pipeline().index();

    assert_eq!(
        route_names(rt),
        [
            "GET /a/q",
            "GET /api/v1/y",
            "GET /api/x",
            "GET /b/q",
            "GET /m/z",
            "GET /pkg/p",
            "GET /raw/r",
            "GET /v3/s",
            "GET /w/t",
            "POST /v2/items"
        ]
    );
    assert_eq!(route_links(rt).len(), 10, "every composed route keeps its handler: {:?}", route_links(rt));
    assert_eq!(result.framework.routes_not_composed, 0);
}

/// A registration on a group the file cannot prove promotes no route and is
/// counted once: a group whose path is not a written literal (with a named or a
/// closure handler), a parameter typed
/// as a router group, a name assigned a group after its declaration, and a call
/// on a group that may return anything.
#[test]
fn a_registration_on_an_unprovable_gin_group_is_refused_and_counted() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "main.go",
        "\
package main

import \"github.com/gin-gonic/gin\"

func h(c *gin.Context) {}

func register(rg *gin.RouterGroup) {
	rg.GET(\"/param\", h)
}

func main() {
	r := gin.Default()
	prefix := \"/dyn\"
	r.Group(prefix).GET(\"/opaque\", h)
	r.Group(prefix).GET(\"/closure\", func(c *gin.Context) {})
	var g *gin.RouterGroup
	g = r.Group(\"/later\")
	g.GET(\"/assigned\", h)
	r.Group(\"/api\").Other().GET(\"/foreign\", h)
	r.GET(\"/kept\", h)
	register(r.Group(\"/api\"))
}
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    let result = engine.pipeline().index();

    assert_eq!(route_names(rt), ["GET /kept"]);
    assert_eq!(result.framework.routes_not_composed, 5);
}

/// A receiver chain that crosses no `Group` call keeps its written path:
/// net/http's package functions, a `ServeMux`, a router kept in a field, an
/// engine parameter, and parameters whose type is one character away from a
/// router group (`*MyRouterGroup`, `*gin.RouterGroupX`).
#[test]
fn receivers_that_cross_no_gin_group_keep_their_routes() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "server/routes.go",
        "\
package server

import (
	\"net/http\"

	\"github.com/gin-gonic/gin\"
)

type Server struct {
	router *gin.Engine
}

func health(w http.ResponseWriter, r *http.Request) {}
func h(c *gin.Context) {}

func (s *Server) Routes(mux *http.ServeMux, e *gin.Engine) {
	http.HandleFunc(\"/health\", health)
	mux.HandleFunc(\"/mux\", health)
	s.router.GET(\"/field\", h)
	e.POST(\"/param\", h)
	gin.Default().GET(\"/inline\", h)
}

// Types one character away from a router group are not one.
type MyRouterGroup struct{}

func (m *MyRouterGroup) GET(p string, f gin.HandlerFunc) {}

func near(a *MyRouterGroup, b *gin.RouterGroupX, c MyRouterGroup) {
	a.GET(\"/near\", h)
	b.GET(\"/nearx\", h)
	c.GET(\"/nearc\", h)
}
",
    );
    // A file whose only registration has a closure handler: the handler-less
    // pattern alone states its receiver's default.
    write(
        tmp.path(),
        "server/closures.go",
        "\
package server

import \"github.com/gin-gonic/gin\"

func Closures() {
	e := gin.Default()
	e.GET(\"/only\", func(c *gin.Context) {})
}
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    let result = engine.pipeline().index();

    assert_eq!(
        route_names(rt),
        [
            "ANY /health",
            "ANY /mux",
            "GET /field",
            "GET /inline",
            "GET /near",
            "GET /nearc",
            "GET /nearx",
            "GET /only",
            "POST /param"
        ]
    );
    assert_eq!(route_links(rt).len(), 8, "{:?}", route_links(rt));
    assert_eq!(result.framework.routes_not_composed, 0);
}

/// NFR-RA-06: a group's literal prefix edited and its file synced alone leaves
/// the route set and edges a fresh index of the edited tree has.
#[test]
fn a_synced_group_prefix_edit_equals_a_fresh_index_of_the_edited_tree() {
    let edited = GIN_GROUPS.replace("r.Group(\"/api\")", "r.Group(\"/api/v2\")");
    let tree = |main: &str| {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "main.go", main);
        tmp
    };
    let tmp_a = tree(GIN_GROUPS);
    let engine_a = Engine::start(tmp_a.path()).expect("engine starts");
    engine_a.pipeline().index();
    let root_a = tmp_a.path().canonicalize().expect("canonicalize root");
    fs::write(tmp_a.path().join("main.go"), &edited).unwrap();
    engine_a.pipeline().sync(&[root_a.join("main.go")]);
    assert_eq!(
        route_names(engine_a.runtime().unwrap()),
        ["GET /", "GET /api/v2/", "GET /api/v2/inline", "GET /api/v2/items", "GET /v1/"]
    );

    let tmp_b = tree(&edited);
    let engine_b = Engine::start(tmp_b.path()).expect("engine starts");
    engine_b.pipeline().index();
    assert_eq!(route_links(engine_a.runtime().unwrap()), route_links(engine_b.runtime().unwrap()));
    let diff = fingerprint_difference(
        (tmp_a.path(), &graph_fingerprint(engine_a.runtime().unwrap())),
        (tmp_b.path(), &graph_fingerprint(engine_b.runtime().unwrap())),
    );
    assert!(diff.is_empty(), "sync-to-state must equal index-of-state: {diff:#?}");
}

/// Gin's `Handle(httpMethod, relativePath, …)` names its verb first. Read as a
/// path-first registration, the verb joined onto a group would be the route
/// `ANY /api/GET`, which nothing serves: it promotes nothing. A path-first
/// `Handle` (net/http) and a Go 1.22 pattern that opens with a verb keep their
/// routes.
#[test]
fn a_gin_handle_naming_its_verb_first_promotes_no_route() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "main.go",
        "\
package main

import (
	\"net/http\"

	\"github.com/gin-gonic/gin\"
)

func h(c *gin.Context) {}
func f(w http.ResponseWriter, r *http.Request) {}

func main() {
	r := gin.Default()
	api := r.Group(\"/api\")
	api.Handle(\"GET\", \"/items\", h)
	r.Handle(\"POST\", \"/root\", h)
	mux := http.NewServeMux()
	mux.Handle(\"/m\", http.HandlerFunc(f))
	mux.HandleFunc(\"GET /c\", f)
}
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    let result = engine.pipeline().index();

    assert_eq!(route_names(rt), ["ANY /m", "GET /c"]);
    assert_eq!(result.framework.routes_not_composed, 0);
}

/// Package-level groups built from one another compose through every level, in
/// whichever order the file declares them: a package-level variable is visible
/// across the file, so `v1`'s initializer sees `api`, `w`'s sees `v2` declared
/// after it, and a function above the `var ( … )` block sees `v1`.
#[test]
fn package_level_groups_built_from_each_other_compose_every_prefix() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "main.go",
        "\
package main

import \"github.com/gin-gonic/gin\"

func h(c *gin.Context) {}

func early() { v1.GET(\"/early\", h) }

var (
	router = gin.Default()
	api    = router.Group(\"/api\")
	v1     = api.Group(\"/v1\")
)

var w = v2.Group(\"/w\")

var v2 = router.Group(\"/v2\")

func main() {
	v1.GET(\"/x\", h)
	w.GET(\"/y\", h)
}
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    let result = engine.pipeline().index();

    assert_eq!(route_names(rt), ["GET /api/v1/early", "GET /api/v1/x", "GET /v2/w/y"]);
    assert_eq!(result.framework.routes_not_composed, 0);
}

/// A name assigned after its declaration is whatever it was last given: one
/// given a group through a chain or through another name, or as any name of a
/// multiple assignment (the fourth, whose value is not paired with it,
/// included), is refused and counted; one only ever given a plain router, in
/// any of the first three positions, keeps its routes.
#[test]
fn an_assigned_name_is_refused_unless_every_value_it_is_given_is_a_plain_router() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "main.go",
        "\
package main

import \"github.com/gin-gonic/gin\"

func h(c *gin.Context) {}
func auth() gin.HandlerFunc { return nil }

var router *gin.Engine

func main() {
	router = gin.New()
	router.GET(\"/kept\", h)
	r := gin.Default()
	var api *gin.RouterGroup
	api = r.Group(\"/api\").Use(auth())
	api.GET(\"/chained\", h)
	var a, b *gin.RouterGroup
	a, b = r.Group(\"/a\"), r.Group(\"/b\")
	a.GET(\"/p\", h)
	b.GET(\"/q\", h)
	var w, x, y, z *gin.RouterGroup
	w, x, y, z = r.Group(\"/w\"), r.Group(\"/x\"), r.Group(\"/y\"), r.Group(\"/z\")
	z.GET(\"/fourth\", h)
	grp := r.Group(\"/grp\")
	var al *gin.RouterGroup
	al = grp
	al.GET(\"/alias\", h)
	var n, m int
	var r2, r3 *gin.Engine
	n, r2 = 1, gin.New()
	n, m, r3 = 1, 2, gin.New()
	r2.GET(\"/second\", h)
	r3.GET(\"/third\", h)
}
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    let result = engine.pipeline().index();

    assert_eq!(route_names(rt), ["GET /kept", "GET /second", "GET /third"]);
    assert_eq!(result.framework.routes_not_composed, 5);
}

/// A group bound by an `if`, `switch` or `for` initializer, or as the third name
/// of one `:=`, carries its prefix inside the statement that binds it; a type
/// switch's alias may be any value, so a registration on it is refused and
/// counted.
#[test]
fn a_group_bound_by_an_initializer_or_a_third_name_carries_its_prefix() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "main.go",
        "\
package main

import \"github.com/gin-gonic/gin\"

func h(c *gin.Context) {}

func main() {
	r := gin.Default()
	if g := r.Group(\"/if\"); g != nil {
		g.GET(\"/a\", h)
	}
	if ok, g2 := true, r.Group(\"/if2\"); ok {
		g2.GET(\"/e\", h)
	}
	switch s := r.Group(\"/sw\"); {
	default:
		s.GET(\"/b\", h)
	}
	for f := r.Group(\"/for\"); ; {
		f.GET(\"/c\", h)
		break
	}
	for i, f2 := 0, r.Group(\"/for2\"); i < 1; i++ {
		f2.GET(\"/g\", h)
	}
	x, y, z := r.Group(\"/x\"), r.Group(\"/y\"), r.Group(\"/z\")
	z.GET(\"/d\", h)
	_, _ = x, y
	var v interface{} = r
	switch t := v.(type) {
	case *gin.Engine:
		t.GET(\"/alias\", h)
	}
}
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    let result = engine.pipeline().index();

    assert_eq!(
        route_names(rt),
        ["GET /for/c", "GET /for2/g", "GET /if/a", "GET /if2/e", "GET /sw/b", "GET /z/d"]
    );
    assert_eq!(result.framework.routes_not_composed, 1);
}

/// A parameter, a method's receiver and a `range` variable are bindings of their
/// own: a group of the same name outside them lends them no prefix. A plain
/// parameter or receiver keeps its routes; a `range` variable may be any element,
/// so a registration on it is refused and counted.
#[test]
fn a_parameter_receiver_or_range_variable_shadows_a_group_of_its_name() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "main.go",
        "\
package main

import \"github.com/gin-gonic/gin\"

func h(c *gin.Context) {}

var srv = gin.New().Group(\"/pkg\")

type S struct{}

func (srv *S) GET(p string, f gin.HandlerFunc) {}
func (srv *S) routes() { srv.GET(\"/m\", h) }

func main() {
	r := gin.Default()
	api := r.Group(\"/api\")
	api.GET(\"/in\", h)
	mount := func(api gin.IRouter) { api.GET(\"/health\", h) }
	mount(r)
	g := r.Group(\"/g\")
	for _, g := range []*gin.Engine{r} {
		g.GET(\"/each\", h)
	}
	_ = g
}
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    let result = engine.pipeline().index();

    assert_eq!(route_names(rt), ["GET /api/in", "GET /health", "GET /m"]);
    assert_eq!(result.framework.routes_not_composed, 1);
}

/// Gin's registration methods return the group they were called on, so a chain
/// of registrations off one group keeps that group's prefix on every link (each
/// listed verb is called on the result of another, so each is read as a link).
#[test]
fn registrations_chained_off_a_group_keep_its_prefix() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "main.go",
        "\
package main

import \"github.com/gin-gonic/gin\"

func h(c *gin.Context) {}

func main() {
	r := gin.Default()
	r.Group(\"/c\").GET(\"/a\", h).POST(\"/b\", h).PUT(\"/d\", h).DELETE(\"/e\", h).PATCH(\"/f\", h).Any(\"/g\", h).GET(\"/z\", h)
}
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    let result = engine.pipeline().index();

    assert_eq!(
        route_names(rt),
        ["ANY /c/g", "DELETE /c/e", "GET /c/a", "GET /c/z", "PATCH /c/f", "POST /c/b", "PUT /c/d"]
    );
    assert_eq!(result.framework.routes_not_composed, 0);
}

/// An assignment re-binds the variable its own position sees. ollama's
/// `server/routes.go` assigns a response `r = t` inside another function, from a
/// type switch's alias; its route table's `r := gin.Default()` is another
/// variable, and keeps every route. Before this rule, that one assignment
/// refused 36 of ollama's routes.
#[test]
fn an_assignment_to_another_functions_variable_says_nothing_about_this_router() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "server/routes.go",
        "\
package server

import \"github.com/gin-gonic/gin\"

type Resp struct{}

func h(c *gin.Context) {}

func collect(ch chan interface{}) {
	var r Resp
	for rr := range ch {
		switch t := rr.(type) {
		case Resp:
			r = t
		}
	}
	_ = r
}

func routes() {
	r := gin.Default()
	r.GET(\"/x\", h)
	g := r.Group(\"/api\")
	g.GET(\"/y\", h)
}
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    let result = engine.pipeline().index();

    assert_eq!(route_names(rt), ["GET /api/y", "GET /x"]);
    assert_eq!(result.framework.routes_not_composed, 0);
}
