//! Rails routes are extracted inside `draw` and link to controller actions
//! (S-567, CR-179, FR-FW-03, FR-FW-01, NFR-RA-05, NFR-RA-06) — exercised end to
//! end through the public [`Engine`] façade against the SHIPPED Ruby
//! `frameworks.scm` and `plugin.toml`, never a query override.
//!
//! The fixtures reproduce the shapes the 2026-10-03 inspection found: lobsters'
//! `get "/x" => "c#a"` lines (122 of its ~178), its `root to:`, its `namespace
//! :mod` block whose controllers live in `Mod`, and its `resources` blocks; and
//! rack's test-client `get("/cgi/test", …)` calls, which became three false
//! routes. Fixtures are written inline into temp directories, like every
//! sibling suite: a fixture tree checked into this repository would be indexed
//! into its own graph.

#![cfg(feature = "lang-ruby")]

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use logos_core::model::{EdgeKind, NodeId, NodeKind};
use logos_core::{Engine, Runtime};
use tempfile::TempDir;

const ROUTES: &str = r#"Rails.application.routes.draw do
  root to: "home#index", as: "root"

  get "/rss" => "home#index", :format => "rss"
  get "/top(/:length)" => "home#top", :as => "top"
  post "/login", to: "login#login"
  get "/upvoted", to: redirect("/upvoted/stories")
  get "/missing" => "nowhere#index"
  get "/singleton" => "reports#weekly"

  resources :stories, except: [:index] do
    post "upvote"
  end

  namespace :mod do
    get "flagged_stories/:period" => "flagged#flagged_stories"
    get "/" => "activities#index"
  end
end
"#;

const CONTROLLERS: &[(&str, &str)] = &[
    (
        "app/controllers/home_controller.rb",
        "\
class HomeController < ApplicationController
  def index
  end

  def top
  end
end
",
    ),
    (
        "app/controllers/login_controller.rb",
        "\
class LoginController < ApplicationController
  def login
  end
end
",
    ),
    (
        // A class method is no action: the route names nothing.
        "app/controllers/reports_controller.rb",
        "\
class ReportsController < ApplicationController
  def self.weekly
  end
end
",
    ),
    (
        // The compact form lobsters writes its moderation controllers in.
        "app/controllers/mod/flagged_controller.rb",
        "\
class Mod::FlaggedController < ApplicationController
  def flagged_stories
  end
end
",
    ),
    (
        "app/controllers/mod/activities_controller.rb",
        "\
module Mod
  class ActivitiesController < ApplicationController
    def index
    end
  end
end
",
    ),
    (
        // A top-level controller of the same name as the namespaced one: the
        // namespace decides which `index` the route names.
        "app/controllers/activities_controller.rb",
        "\
class ActivitiesController < ApplicationController
  def index
  end
end
",
    ),
];

/// rack's shape: a test client's request calls in a file that names `Rails`
/// (the ledger gate admits it), outside any `draw` block.
const RACK_TEST: &str = r#"Rails.logger
class FilesTest < Minitest::Test
  def test_cgi
    get("/cgi/test", "HTTP_IF_MODIFIED_SINCE" => "x")
    post("/", "HTTP_IF_NONE_MATCH" => "1234")
    get "/plain"
  end
end
"#;

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn fixture(root: &Path, routes: &str) {
    write(root, "config/routes.rb", routes);
    for (rel, src) in CONTROLLERS {
        write(root, rel, src);
    }
    write(root, "test/spec_files.rb", RACK_TEST);
}

/// Every route-node name, sorted.
fn route_names(rt: &Runtime) -> Vec<String> {
    rt.submit_read(|store| {
        let mut names: Vec<String> = store
            .all_nodes()?
            .into_iter()
            .filter(|n| n.kind == NodeKind::Route)
            .map(|n| n.name)
            .collect();
        names.sort();
        Ok(names)
    })
    .expect("read runs")
}

/// Every `RoutesTo` edge as `(route name, "<handler file>#<handler name>")`,
/// sorted — the file tells two same-named actions apart, and ids differ between
/// two indexes where names do not.
fn route_handlers(rt: &Runtime) -> Vec<(String, String)> {
    rt.submit_read(|store| {
        let nodes: HashMap<NodeId, (String, Option<String>)> =
            store.all_nodes()?.into_iter().map(|n| (n.id, (n.name, n.file_path))).collect();
        let mut pairs: Vec<(String, String)> = store
            .all_edges()?
            .into_iter()
            .filter(|e| e.kind == EdgeKind::RoutesTo)
            .map(|e| {
                let (handler, file) = &nodes[&e.target];
                (nodes[&e.source].0.clone(), format!("{}#{handler}", file.clone().unwrap_or_default()))
            })
            .collect();
        pairs.sort();
        Ok(pairs)
    })
    .expect("read runs")
}

fn pair(route: &str, handler: &str) -> (String, String) {
    (route.to_string(), handler.to_string())
}

/// lobsters' shapes become routes, each `controller#action` linked to its
/// controller's action — the namespace's module included — and nothing else.
#[test]
fn rails_routes_inside_draw_link_to_their_controller_actions() {
    let tmp = TempDir::new().unwrap();
    fixture(tmp.path(), ROUTES);
    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    let result = engine.index();

    assert_eq!(
        route_names(rt),
        [
            "GET /",
            "GET /missing",
            "GET /mod",
            "GET /mod/flagged_stories/:period",
            "GET /rss",
            "GET /singleton",
            "GET /top(/:length)",
            "GET /upvoted",
            "POST /login",
        ],
        "rack's test-client calls are no routes, and `resources` stays unexpanded"
    );
    assert_eq!(
        route_handlers(rt),
        [
            pair("GET /", "app/controllers/home_controller.rb#index"),
            pair("GET /mod", "app/controllers/mod/activities_controller.rb#index"),
            pair("GET /mod/flagged_stories/:period", "app/controllers/mod/flagged_controller.rb#flagged_stories"),
            pair("GET /rss", "app/controllers/home_controller.rb#index"),
            pair("GET /top(/:length)", "app/controllers/home_controller.rb#top"),
            pair("POST /login", "app/controllers/login_controller.rb#login"),
        ],
        "a redirect, a missing controller and a class method name no action"
    );
    assert_eq!(
        result.framework.routes_not_composed, 1,
        "the registration inside `resources` is refused and counted"
    );
}

/// The handler link is re-proven on every run: editing the routes file and
/// syncing it gives the graph a fresh index gives (NFR-RA-06).
#[test]
fn syncing_the_routes_file_equals_a_fresh_index() {
    let edited = ROUTES.replace(r#""home#top""#, r#""login#login""#);

    let live = TempDir::new().unwrap();
    fixture(live.path(), ROUTES);
    let engine = Engine::start(live.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    engine.index();
    write(live.path(), "config/routes.rb", &edited);
    let synced = engine.sync(&[std::path::PathBuf::from("config/routes.rb")]);
    assert!(synced.warnings.is_empty(), "{:?}", synced.warnings);

    let fresh = TempDir::new().unwrap();
    fixture(fresh.path(), &edited);
    let cold = Engine::start(fresh.path()).expect("engine starts");
    cold.index();
    let cold_rt = cold.runtime().unwrap();

    assert_eq!(route_names(rt), route_names(cold_rt));
    assert_eq!(route_handlers(rt), route_handlers(cold_rt));
    assert!(
        route_handlers(rt).contains(&pair("GET /top(/:length)", "app/controllers/login_controller.rb#login")),
        "the edit reached the graph: {:?}",
        route_handlers(rt)
    );
}

/// Syncing a controller that gains the action links the route that named it.
#[test]
fn syncing_a_controller_that_gains_the_action_links_its_route() {
    let live = TempDir::new().unwrap();
    fixture(live.path(), ROUTES);
    let engine = Engine::start(live.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    engine.index();
    assert!(!route_handlers(rt).iter().any(|(route, _)| route == "GET /missing"));

    write(
        live.path(),
        "app/controllers/nowhere_controller.rb",
        "class NowhereController < ApplicationController\n  def index\n  end\nend\n",
    );
    engine.sync(&[std::path::PathBuf::from("app/controllers/nowhere_controller.rb")]);
    assert!(
        route_handlers(rt).contains(&pair("GET /missing", "app/controllers/nowhere_controller.rb#index")),
        "{:?}",
        route_handlers(rt)
    );
}

/// The action is exactly one instance method of a class: a class reopened in
/// another file pools its methods, an action two declarations both define
/// names nothing, and a module spelled like a controller is no controller
/// (NFR-RA-05).
#[test]
fn an_action_binds_only_one_instance_method_of_a_class() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "config/routes.rb",
        r#"Rails.application.routes.draw do
  get "/split" => "split#show"
  get "/dup" => "dups#show"
  get "/module" => "pages#show"
end
"#,
    );
    let sources = [
        ("app/controllers/split_controller.rb", "class SplitController < ApplicationController\n  def index\n  end\nend\n"),
        ("app/controllers/split_show.rb", "class SplitController\n  def show\n  end\nend\n"),
        ("app/controllers/dups_controller.rb", "class DupsController < ApplicationController\n  def show\n  end\nend\n"),
        ("app/controllers/dups_again.rb", "class DupsController\n  def show\n  end\nend\n"),
        ("app/controllers/pages_controller.rb", "module PagesController\n  def show\n  end\nend\n"),
    ];
    for (rel, src) in sources {
        write(tmp.path(), rel, src);
    }
    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    engine.index();

    assert_eq!(route_names(rt), ["GET /dup", "GET /module", "GET /split"]);
    assert_eq!(
        route_handlers(rt),
        [pair("GET /split", "app/controllers/split_show.rb#show")],
        "the reopened class's action links; the doubled action and the module's method do not"
    );
}
