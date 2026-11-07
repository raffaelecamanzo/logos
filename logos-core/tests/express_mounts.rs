//! An Express mount composes wherever it is written (S-853, CR-270 §3.2 B,
//! FR-FW-03, NFR-RA-05, NFR-RA-06).
//!
//! `app.use('/api', router)` in a file that names no framework is no framework
//! candidate, so before S-853 the pass never read it: the router it mounts,
//! exported by a candidate module, had its registrations refused and counted in
//! `routes_not_composed`. The mount facts are now read in every indexed
//! JavaScript and TypeScript file, while the full framework pass still runs on
//! candidates only. Each test here builds a tree whose mounting file imports no
//! framework, and asserts it stays out of `files_scanned`.

#![cfg(feature = "lang-typescript")]

use std::fs;
use std::path::Path;

use logos_core::model::{EdgeKind, NodeId, NodeKind};
use logos_core::models::pipeline::FrameworkStats;
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

fn index_tree(files: &[(&str, &str)]) -> (TempDir, Engine, FrameworkStats) {
    let tmp = TempDir::new().unwrap();
    for (rel, contents) in files {
        write(tmp.path(), rel, contents);
    }
    let engine = Engine::start(tmp.path()).expect("engine starts");
    let stats = engine.pipeline().index().framework;
    (tmp, engine, stats)
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

/// `(route, handler)` for every `RoutesTo` edge, sorted.
fn route_links(rt: &Runtime) -> Vec<(String, String)> {
    let (nodes, edges) = rt
        .submit_read(|store| Ok((store.all_nodes()?, store.all_edges()?)))
        .expect("read runs");
    let names: std::collections::HashMap<NodeId, String> = nodes.into_iter().map(|n| (n.id, n.name)).collect();
    let mut links: Vec<(String, String)> = edges
        .into_iter()
        .filter(|e| e.kind == EdgeKind::RoutesTo)
        .map(|e| (names[&e.source].clone(), names[&e.target].clone()))
        .collect();
    links.sort();
    links
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

/// The router module: a candidate (it requires `express`), two registrations,
/// the router exported as the module.
const ITEMS_JS: &str = "\
'use strict';

const router = require('express').Router();

function list(req, res) {}

router.get('/items', list);
router.get('/', list);

module.exports = router;
";

/// The application module: a candidate, which mounts nothing itself.
const APP_JS: &str = "\
'use strict';

const express = require('express');

module.exports = express();
";

/// The mounting file: it requires no framework and binds no `app`/`router`
/// parameter, so it is no candidate. `prefix` is written as the mount's first
/// argument verbatim.
fn server_js(prefix: &str) -> String {
    format!(
        "\
'use strict';

const app = require('./app');
const items = require('./items');

app.use({prefix}, items);

module.exports = app;
"
    )
}

/// The candidates of every tree here: the mounting file is never one of them.
const CANDIDATES: u64 = 2;

/// The story's case (AC 1). Without the mounting file the router is mounted
/// nowhere, and both its registrations are refused; with it they compose under
/// `/api`, and the refusal count drops by exactly the routes that composed.
#[test]
fn a_mount_in_a_file_with_no_framework_import_composes_the_router_it_imports() {
    let (_tmp, engine, unmounted) = index_tree(&[("src/items.js", ITEMS_JS), ("src/app.js", APP_JS)]);
    assert_eq!(route_names(engine.runtime().unwrap()), Vec::<String>::new());
    assert_eq!(unmounted.routes_not_composed, 2, "both registrations wait for a mount");

    let server = server_js("'/api'");
    let (_tmp, engine, mounted) =
        index_tree(&[("src/items.js", ITEMS_JS), ("src/app.js", APP_JS), ("src/server.js", &server)]);
    let rt = engine.runtime().unwrap();

    assert_eq!(mounted.files_scanned, CANDIDATES, "the mounting file is no candidate");
    assert_eq!(route_names(rt), ["GET /api", "GET /api/items"]);
    assert_eq!(
        route_links(rt),
        [("GET /api".to_string(), "list".to_string()), ("GET /api/items".to_string(), "list".to_string())]
    );
    assert_eq!(mounted.routes_not_composed, 0);
    assert_eq!(
        unmounted.routes_not_composed - mounted.routes_not_composed,
        route_names(rt).len() as u64,
        "routes_not_composed drops by the count of the routes the mount composed"
    );
}

/// The same tree in TypeScript's `import` / `export default` spelling.
#[test]
fn a_typescript_mount_in_a_file_with_no_framework_import_composes() {
    let items = "\
import { Router } from 'express';

const router = Router();

export function list(req: unknown, res: unknown): void {}

router.get('/items', list);

export default router;
";
    let app = "import express from 'express';\n\nexport default express();\n";
    let server = "import app from './app';\nimport items from './items';\n\napp.use('/api', items);\n";
    let (_tmp, engine, stats) = index_tree(&[("src/items.ts", items), ("src/app.ts", app), ("src/server.ts", server)]);
    let rt = engine.runtime().unwrap();

    assert_eq!(stats.files_scanned, CANDIDATES);
    assert_eq!(route_names(rt), ["GET /api/items"]);
    assert_eq!(route_links(rt), [("GET /api/items".to_string(), "list".to_string())]);
    assert_eq!(stats.routes_not_composed, 0);
}

/// The router module most Express code writes also calls `use` itself, for its
/// own middleware: its text is admitted by the mount read's text check, yet it
/// is a candidate, so the full pass reads it and the mount read must not read it
/// a second time — a second copy under the same path would take its exports,
/// and its held registrations would find no mount.
#[test]
fn a_candidate_router_that_calls_use_itself_still_composes_from_a_non_candidate_mount() {
    let items = ITEMS_JS.replace(
        "function list(req, res) {}\n",
        "function list(req, res) {}\nfunction auth(req, res, next) { next(); }\n\nrouter.use(auth);\n",
    );
    assert_ne!(items, ITEMS_JS, "the fixture's handler line moved: the replace did nothing");
    let server = server_js("'/api'");
    let (_tmp, engine, stats) =
        index_tree(&[("src/items.js", &items), ("src/app.js", APP_JS), ("src/server.js", &server)]);
    let rt = engine.runtime().unwrap();

    assert_eq!(stats.files_scanned, CANDIDATES);
    // `router.use(auth)` names no path, so it is no registration of its own.
    assert_eq!(route_names(rt), ["GET /api", "GET /api/items"]);
    assert_eq!(stats.routes_not_composed, 0);
}

/// AC 2: a prefix the mount does not write as a literal — a variable, a
/// template, an expression — stays refused and counted, once per registration,
/// wherever the mount is written.
#[test]
fn a_computed_prefix_in_a_non_candidate_file_stays_refused_and_counted() {
    for prefix in ["process.env.PREFIX", "`/${base}`", "base + '/api'"] {
        let server = server_js(prefix);
        let (_tmp, engine, stats) =
            index_tree(&[("src/items.js", ITEMS_JS), ("src/app.js", APP_JS), ("src/server.js", &server)]);

        assert_eq!(route_names(engine.runtime().unwrap()), Vec::<String>::new(), "{prefix}");
        assert_eq!(stats.routes_not_composed, 2, "{prefix}");
    }
}

/// AC 2: a `.use(str, ident)` in a non-candidate file whose identifier is no
/// router the mount reader recognises composes nothing, and the router mounted
/// nowhere stays refused. Each line is one step from the story's mount: an
/// imported module that exports no router, a name nothing binds, a method one
/// character off, and the mount itself in a comment and a string.
#[test]
fn a_non_express_use_in_a_non_candidate_file_composes_nothing() {
    let backend = "'use strict';\n\nmodule.exports = { load() {} };\n";
    let server = "\
'use strict';

const i18n = require('./i18n');
const backend = require('./backend');
const items = require('./items');

i18n.use('/api', backend);
i18n.use('/api', unbound);
i18n.usex('/api', items);
// i18n.use('/api', items);
const text = \"i18n.use('/api', items)\";
";
    let (_tmp, engine, stats) = index_tree(&[
        ("src/items.js", ITEMS_JS),
        ("src/app.js", APP_JS),
        ("src/backend.js", backend),
        ("src/i18n.js", backend),
        ("src/server.js", server),
    ]);

    assert_eq!(stats.files_scanned, CANDIDATES);
    assert_eq!(route_names(engine.runtime().unwrap()), Vec::<String>::new());
    assert_eq!(stats.routes_not_composed, 2, "the router is still mounted nowhere");
}

/// AC 3 (NFR-RA-06): the mounting file is no candidate, so syncing it alone must
/// still move the router's routes, exactly as a fresh index of the edited tree
/// does — routes, edges and the whole graph fingerprint, ledger included.
#[test]
fn a_synced_edit_of_the_mounting_file_equals_a_fresh_index() {
    let at_api = server_js("'/api'");
    let at_v2 = server_js("'/v2'");
    let computed = server_js("process.env.PREFIX");
    let unmounted = "'use strict';\n\nconst app = require('./app');\n\nmodule.exports = app;\n";
    let (tmp, engine, _) =
        index_tree(&[("src/items.js", ITEMS_JS), ("src/app.js", APP_JS), ("src/server.js", &at_api)]);
    let rt = engine.runtime().unwrap();
    assert_eq!(route_names(rt), ["GET /api", "GET /api/items"]);
    let root = tmp.path().canonicalize().expect("canonicalize root");

    for (contents, expected_routes, expected_refused) in [
        (at_v2.as_str(), vec!["GET /v2", "GET /v2/items"], 0),
        (computed.as_str(), vec![], 2),
        (unmounted, vec![], 2),
        (at_api.as_str(), vec!["GET /api", "GET /api/items"], 0),
    ] {
        write(tmp.path(), "src/server.js", contents);
        let synced = engine.pipeline().sync(&[root.join("src/server.js")]).framework;
        assert_eq!(route_names(rt), expected_routes, "after editing to {contents}");
        assert_eq!(synced.routes_not_composed, expected_refused, "after editing to {contents}");

        let fresh = TempDir::new().unwrap();
        for rel in ["src/items.js", "src/app.js", "src/server.js"] {
            write(fresh.path(), rel, &fs::read_to_string(tmp.path().join(rel)).unwrap());
        }
        let cold = Engine::start(fresh.path()).expect("engine starts");
        let indexed = cold.pipeline().index().framework;
        assert_eq!(synced.routes_not_composed, indexed.routes_not_composed, "after editing to {contents}");
        assert_eq!(route_links(rt), route_links(cold.runtime().unwrap()), "after editing to {contents}");
        let diff = fingerprint_difference(
            (tmp.path(), &graph_fingerprint(rt)),
            (fresh.path(), &graph_fingerprint(cold.runtime().unwrap())),
        );
        assert!(diff.is_empty(), "sync-to-state must equal index-of-state after {contents}: {diff:#?}");
    }
}
