//! CommonJS `require` bindings qualify calls, and `module.exports` records
//! exports (S-540, [FR-RS-22], [CR-174]) — exercised end-to-end through the
//! public [`Engine`] façade against real temp-directory fixtures.
//!
//! In a `javascript` file a top-level `const m = require('./x')` makes `m` a
//! qualifier (`m.f()` reads `x::f`, `m()` reads `x`'s default), and `const {
//! f, g: h } = require('./x')` binds `f` and `h` as named imports. A module's
//! `module.exports = X` or `const X = module.exports` records `X` as its export
//! object, whose members are the module's names; `module.exports = { a: b }`
//! exports `b` as `a`. A `require` of an external package binds nothing.
//!
//! The fixtures follow the shapes that dominate NodeBB @ e5c9a417: `const x =
//! require('./…')` (2,183 declarators), `const X = module.exports` (232
//! modules), and `request.js`'s destructured `require('./ssrf')`. Every call
//! fixture asserts its import edge binds first, as `typescript_reexports.rs`
//! does: an import that binds and a call that does not is the state before
//! this story.
//!
//! [FR-RS-22]: ../../docs/specs/requirements/FR-RS-22.md
//! [CR-174]: ../../docs/requests/CR-174-javascript-is-its-own-language-and-its-node-idioms-resolve.md

#![cfg(feature = "lang-typescript")]

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use logos_core::extract::{extract, FileInput, SymbolContext};
use logos_core::model::{EdgeKind, NodeId};
use logos_core::plugin::LanguageRegistry;
use logos_core::Engine;
use logos_core::Runtime;
use tempfile::TempDir;

#[path = "support/coordinate.rs"]
mod coordinate;
#[path = "support/graph_fingerprint.rs"]
mod graph_fingerprint;
use graph_fingerprint::graph_fingerprint;

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// Index a fixture of `(path, contents)` files.
fn index(files: &[(&str, &str)]) -> (TempDir, Engine) {
    let tmp = TempDir::new().unwrap();
    for (rel, contents) in files {
        write(tmp.path(), rel, contents);
    }
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.index();
    (tmp, engine)
}

/// Every edge of `kind` as `(source file:name, target file:name)`, sorted.
fn edges_of(rt: &Runtime, kind: EdgeKind) -> Vec<(String, String)> {
    rt.submit_read(move |store| {
        let label: HashMap<NodeId, String> = store
            .all_nodes()?
            .into_iter()
            .map(|n| {
                let file = n.file_path.unwrap_or_default();
                (n.id, format!("{file}:{}", n.name))
            })
            .collect();
        let mut out: Vec<(String, String)> = store
            .all_edges()?
            .into_iter()
            .filter(|e| e.kind == kind)
            .map(|e| (label[&e.source].clone(), label[&e.target].clone()))
            .collect();
        out.sort();
        Ok(out)
    })
    .expect("read runs")
}

/// The `Calls` edges whose target is the node `file:name`, as source labels.
fn callers_of(rt: &Runtime, target: &str) -> Vec<String> {
    edges_of(rt, EdgeKind::Calls)
        .into_iter()
        .filter(|(_, t)| t == target)
        .map(|(s, _)| s)
        .collect()
}

/// The files the `Imports` edges out of `rel` reach, sorted.
fn imported_files(rt: &Runtime, rel: &str) -> Vec<String> {
    let prefix = format!("{rel}:");
    let mut out: Vec<String> = edges_of(rt, EdgeKind::Imports)
        .into_iter()
        .filter(|(s, _)| s.starts_with(&prefix))
        .map(|(_, t)| t.split(':').next().unwrap().to_string())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    out.sort();
    out
}

/// Every `Calls` ledger row of `rel` but its `require(…)` calls, as `target
/// (bound|unbound)`, sorted.
fn call_rows(rt: &Runtime, rel: &'static str) -> Vec<String> {
    rt.submit_read(move |store| {
        let in_file: HashSet<String> = store
            .all_nodes()?
            .into_iter()
            .filter(|n| n.file_path.as_deref() == Some(rel))
            .map(|n| n.symbol.as_str().to_string())
            .collect();
        let mut out: Vec<String> = store
            .unresolved_refs()?
            .into_iter()
            .filter(|r| r.kind == EdgeKind::Calls && in_file.contains(r.source_symbol.as_str()) && r.target != "require")
            .map(|r| format!("{} ({})", r.target, if r.resolved { "bound" } else { "unbound" }))
            .collect();
        out.sort();
        Ok(out)
    })
    .expect("read runs")
}

/// Every `Imports` row `path` records, as `target alias exported`, sorted —
/// one in-memory file through the embedded plugin.
fn import_rows(path: &str, source: &str) -> Vec<String> {
    let tmp = TempDir::new().unwrap();
    let reg = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");
    let plugin = reg.for_path(path).unwrap_or_else(|| panic!("a plugin claims {path}"));
    let facts = extract(&FileInput::new(path, source), plugin, &SymbolContext::default());
    assert!(facts.warnings.is_empty(), "{path}: {:?}", facts.warnings);
    let mut out: Vec<String> = facts
        .refs
        .iter()
        .filter(|r| r.kind == EdgeKind::Imports)
        .map(|r| format!("{} {} {}", r.target, r.alias.as_deref().unwrap_or("-"), r.exported == Some(true)))
        .collect();
    out.sort();
    out
}

/// Every `Calls` row target `path` records but the `require(…)` calls
/// themselves, sorted.
fn call_targets(path: &str, source: &str) -> Vec<String> {
    let tmp = TempDir::new().unwrap();
    let reg = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");
    let plugin = reg.for_path(path).unwrap_or_else(|| panic!("a plugin claims {path}"));
    let facts = extract(&FileInput::new(path, source), plugin, &SymbolContext::default());
    let mut out: Vec<String> = facts
        .refs
        .iter()
        .filter(|r| r.kind == EdgeKind::Calls && r.target != "require")
        .map(|r| r.target.clone())
        .collect();
    out.sort();
    out
}

/// NodeBB's `src/ssrf.js`, cut to its export shape.
const SSRF: &str = "'use strict';\n\
const allowList = new Set();\n\
async function checkHostname(rawHostname) {\n  return { ok: allowList.has(rawHostname) };\n}\n\
async function check(url) {\n  return await checkHostname(new URL(url).hostname);\n}\n\
function lookup(hostname, options, callback) {\n  callback(null, hostname);\n}\n\
module.exports = {\n  allowList,\n  checkHostname,\n  check,\n  lookup,\n};\n";

/// NodeBB's `src/request.js:11` and its `check` wrapper (`:196`–`:205`).
const REQUEST: &str = "'use strict';\n\
const nconf = require('nconf');\n\
const { checkHostname, lookup: resolveHost, allowList } = require('./ssrf');\n\
async function check(url) {\n  const { hostname } = new URL(url);\n\
  if (allowList.has(hostname)) {\n    return { ok: true };\n  }\n\
  resolveHost(hostname, {}, () => {});\n  return await checkHostname(hostname);\n}\n\
exports.check = check;\n";

#[test]
fn a_destructured_require_binds_its_named_functions_across_files() {
    let (_tmp, engine) = index(&[("src/ssrf.js", SSRF), ("src/request.js", REQUEST)]);
    let rt = engine.runtime().unwrap();
    assert_eq!(imported_files(rt, "src/request.js"), ["src/ssrf.js"], "the require binds first");
    assert_eq!(callers_of(rt, "src/ssrf.js:checkHostname"), ["src/request.js:check", "src/ssrf.js:check"]);
    assert_eq!(
        callers_of(rt, "src/ssrf.js:lookup"),
        ["src/request.js:check"],
        "`lookup: resolveHost` binds the module's `lookup` under its local name"
    );
}

#[test]
fn a_require_binding_qualifies_a_member_call() {
    // NodeBB's `src/events.js:7` and `:266`, and `src/batch.js:13`.
    let batch = "'use strict';\nconst db = require('./database');\n\
                 exports.processSortedSet = async function (setKey, process, options) {\n  return db.getSortedSetRange(setKey);\n};\n";
    let events = "'use strict';\nconst batch = require('./batch');\n\
                  async function deleteAll() {\n  await batch.processSortedSet('events:time', async (eids) => eids, { batch: 500 });\n}\n\
                  exports.deleteAll = deleteAll;\n";
    let (_tmp, engine) = index(&[("src/batch.js", batch), ("src/events.js", events)]);
    let rt = engine.runtime().unwrap();
    assert_eq!(imported_files(rt, "src/events.js"), ["src/batch.js"], "the require binds first");
    assert_eq!(callers_of(rt, "src/batch.js:processSortedSet"), ["src/events.js:deleteAll"]);
    assert_eq!(call_rows(rt, "src/events.js"), [".::batch::processSortedSet (bound)"]);
}

#[test]
fn the_export_object_s_members_are_the_module_s_names() {
    // NodeBB's `src/user/index.js`: `const User = module.exports;` then
    // `User.exists = async function …`; and `module.exports = X` after `X`'s
    // members are assigned.
    let user = "'use strict';\nconst User = module.exports;\n\
                User.exists = async function (uids) {\n  return uids;\n};\n";
    let pubsub = "'use strict';\nconst PubSub = {};\nPubSub.publish = function (event, data) {\n  return data;\n};\n\
                  module.exports = PubSub;\n";
    let posts = "'use strict';\nconst user = require('../user');\nconst pubsub = require('../pubsub');\n\
                 exports.create = async function (data) {\n  pubsub.publish('post:new', data);\n  return await user.exists([data.uid]);\n};\n";
    let (_tmp, engine) =
        index(&[("src/user/index.js", user), ("src/pubsub.js", pubsub), ("src/posts/create.js", posts)]);
    let rt = engine.runtime().unwrap();
    assert_eq!(imported_files(rt, "src/posts/create.js"), ["src/pubsub.js", "src/user/index.js"]);
    assert_eq!(callers_of(rt, "src/user/index.js:exists"), ["src/posts/create.js:create"]);
    assert_eq!(callers_of(rt, "src/pubsub.js:publish"), ["src/posts/create.js:create"]);
}

#[test]
fn a_module_that_exports_two_objects_exports_neither_s_members() {
    // `module.exports` is overwritten: at runtime `x.get` is undefined, and
    // the tree does not say which object a requiring file sees.
    let x = "const Legacy = {};\nLegacy.get = function () {};\nmodule.exports = Legacy;\n\
             const Modern = {};\nModern.put = function () {};\nmodule.exports = Modern;\n";
    let main = "const x = require('./x');\nexports.run = function () {\n  x.put();\n  return x.get();\n};\n";
    // One object, aliased twice, is still one.
    let y = "const Y = module.exports;\nY.get = function () {};\nmodule.exports = Y;\n";
    let other = "const y = require('./y');\nexports.run = function () {\n  return y.get();\n};\n";
    let (_tmp, engine) = index(&[("src/x.js", x), ("src/main.js", main), ("src/y.js", y), ("src/other.js", other)]);
    let rt = engine.runtime().unwrap();
    assert_eq!(imported_files(rt, "src/main.js"), ["src/x.js"]);
    assert!(callers_of(rt, "src/x.js:get").is_empty());
    assert!(callers_of(rt, "src/x.js:put").is_empty());
    assert_eq!(callers_of(rt, "src/y.js:get"), ["src/other.js:run"]);
}

#[test]
fn a_bare_call_of_a_require_binding_calls_the_exported_object() {
    let slugify = "'use strict';\nfunction slugify(str) {\n  return str;\n}\nmodule.exports = slugify;\n";
    let topics = "'use strict';\nconst slugify = require('./slugify');\n\
                  exports.create = function (title) {\n  return slugify(title);\n};\n";
    let (_tmp, engine) = index(&[("src/slugify.js", slugify), ("src/topics.js", topics)]);
    let rt = engine.runtime().unwrap();
    assert_eq!(imported_files(rt, "src/topics.js"), ["src/slugify.js"]);
    assert_eq!(callers_of(rt, "src/slugify.js:slugify"), ["src/topics.js:create"]);
}

#[test]
fn an_object_export_names_its_value_under_its_key() {
    let helpers = "'use strict';\nfunction buildCategories(cids) {\n  return cids;\n}\n\
                   module.exports = { getCategories: buildCategories };\n";
    let controller = "'use strict';\nconst { getCategories } = require('./helpers');\nconst helpers = require('./helpers');\n\
                      exports.list = function () {\n  helpers.getCategories([2]);\n  return getCategories([1]);\n};\n";
    let (_tmp, engine) = index(&[("src/helpers.js", helpers), ("src/controller.js", controller)]);
    let rt = engine.runtime().unwrap();
    assert_eq!(imported_files(rt, "src/controller.js"), ["src/helpers.js"]);
    assert_eq!(callers_of(rt, "src/helpers.js:buildCategories"), ["src/controller.js:list"]);
    let rows = call_rows(rt, "src/controller.js");
    assert!(
        rows.iter().all(|r| r == ".::helpers::getCategories (bound)"),
        "the named import and the qualifier read alike: {rows:?}"
    );
}

#[test]
fn a_require_of_an_external_package_stays_unbound() {
    // A workspace `Router` and `json` a fabricated binding would reach.
    let router = "'use strict';\nfunction Router() {}\nfunction json() {}\nmodule.exports = { Router, json };\n";
    let app = "'use strict';\nconst express = require('express');\n\
               exports.start = function () {\n  const app = express();\n  app.use(express.json());\n  return express.Router();\n};\n";
    let (_tmp, engine) = index(&[("src/router.js", router), ("src/app.js", app)]);
    let rt = engine.runtime().unwrap();
    assert!(imported_files(rt, "src/app.js").is_empty(), "`require('express')` binds nothing");
    assert!(callers_of(rt, "src/router.js:Router").is_empty());
    assert!(callers_of(rt, "src/router.js:json").is_empty());
    let rows = call_rows(rt, "src/app.js");
    for target in ["express::Router (unbound)", "express::json (unbound)", "express::default (unbound)"] {
        assert!(rows.iter().any(|r| r == target), "{target} in {rows:?}");
    }
}

#[test]
fn a_binding_that_shadows_a_require_keeps_the_call_bare() {
    let batch = "exports.processSortedSet = async function () {};\n";
    // A parameter named `batch`, a `require` inside a function body, and one
    // inside a block at file scope: the name is not the file's, so no call
    // reads through the import.
    let caller = "const batch = require('./batch');\n\
                  exports.run = function (batch) {\n  return batch.processSortedSet();\n};\n\
                  exports.lazy = function () {\n  const meta = require('./meta');\n  return meta.processSortedSet();\n};\n\
                  if (process.env.NODE_ENV) {\n  const plugins = require('./plugins');\n}\n\
                  exports.global = function () {\n  return plugins.processSortedSet();\n};\n";
    let (_tmp, engine) = index(&[
        ("src/batch.js", batch),
        ("src/meta.js", batch),
        ("src/plugins.js", batch),
        ("src/caller.js", caller),
    ]);
    let rt = engine.runtime().unwrap();
    assert_eq!(imported_files(rt, "src/caller.js"), ["src/batch.js", "src/meta.js", "src/plugins.js"]);
    assert!(callers_of(rt, "src/batch.js:processSortedSet").is_empty());
    assert!(callers_of(rt, "src/meta.js:processSortedSet").is_empty());
    assert!(callers_of(rt, "src/plugins.js:processSortedSet").is_empty());
    assert!(call_rows(rt, "src/caller.js").iter().all(|r| !r.contains("::")));
}

#[test]
fn a_require_binding_the_file_binds_again_decides_nothing() {
    // A `let` reassigned in a block, a `var` redeclared in one, and names a
    // file-scope `for`, block or `catch` binds again: where the call is made,
    // the name may hold another module or a local value.
    let get = "exports.get = function () {};\n";
    let reassigned = "let store = require('./memory');\nif (process.env.REDIS) {\n  store = require('./redis');\n}\n\
                      exports.run = function () {\n  return store.get();\n};\n";
    let redeclared = "var db = require('./memory');\nif (process.env.REDIS) {\n  var db = require('./redis');\n}\n\
                      exports.run = function () {\n  return db.get();\n};\n";
    let shadowed = "const batch = require('./memory');\nconst { get } = require('./redis');\nconst list = [];\n\
                    for (const batch of list) {\n  batch.get();\n}\nfor (const get of list) {\n  get();\n}\n\
                    if (list.length) {\n  const batch = list[0];\n  batch.get();\n}\n\
                    try {\n  list.pop();\n} catch (batch) {\n  batch.get();\n}\n";
    for (name, caller) in [("reassigned", reassigned), ("redeclared", redeclared), ("shadowed", shadowed)] {
        let (_tmp, engine) = index(&[("src/memory.js", get), ("src/redis.js", get), ("src/main.js", caller)]);
        let rt = engine.runtime().unwrap();
        assert!(!imported_files(rt, "src/main.js").is_empty(), "{name}: the requires bind");
        assert!(callers_of(rt, "src/memory.js:get").is_empty(), "{name}: {:?}", call_rows(rt, "src/main.js"));
        assert!(callers_of(rt, "src/redis.js:get").is_empty(), "{name}: {:?}", call_rows(rt, "src/main.js"));
    }
}

#[test]
fn an_assignment_to_a_function_s_own_binding_leaves_the_require_in_force() {
    // NodeBB's `src/user/notifications.js`: the module is required as
    // `notifications`, and one function reuses the name for its own `let`.
    let notifications = "exports.push = function () {};\n";
    let user = "const notifications = require('./notifications');\n\
                exports.list = function () {\n  let notifications = [];\n  notifications = notifications.filter(Boolean);\n  return notifications;\n};\n\
                exports.send = function () {\n  return notifications.push();\n};\n";
    let (_tmp, engine) = index(&[("src/notifications.js", notifications), ("src/user.js", user)]);
    let rt = engine.runtime().unwrap();
    assert_eq!(callers_of(rt, "src/notifications.js:push"), ["src/user.js:send"]);
}

#[test]
fn require_declarators_bind_exactly_the_names_they_destructure() {
    let source = "const { a, b: c, d = 1, e: f = 2, ...rest } = require('./m');\n\
                  var legacy = require('./legacy');\nlet { tail } = require('./t');\n\
                  const { g: { h } } = require('./n');\n\
                  const prop = require('./p').value;\n\
                  require('./q')(exports);\n\
                  exports.run = function () {\n  a(); c(); d(); f(); rest(); h(); prop(); value(); legacy.run(); tail();\n};\n";
    assert_eq!(
        call_targets("src/x.js", source),
        [".::legacy::run", ".::m::a", ".::m::b", ".::m::d", ".::m::e", ".::t::tail", "h", "prop", "rest", "value"],
    );
}

#[test]
fn a_quoted_computed_or_numeric_key_binds_and_exports_nothing() {
    // Only an identifier key names an export: `'a-b'`, `[k]` and `0` do not.
    assert_eq!(
        call_targets("src/x.js", "const { 'a-b': q, [k]: r, 0: z } = require('./m');\nexports.run = function () {\n  q(); r(); z();\n};\n"),
        ["q", "r", "z"],
    );
    assert!(import_rows("src/x.js", "function f() {}\nmodule.exports = { 'a-b': f, [k]: f, 0: f };\n").is_empty());
}

#[test]
fn only_module_exports_itself_is_the_export_object() {
    assert!(import_rows("src/x.js", "function f() {}\nconst Parent = module.parent;\nmodule.id = f;\n").is_empty());
}

#[test]
fn a_var_alias_of_module_exports_is_the_export_object() {
    assert_eq!(
        import_rows("src/x.js", "var Legacy = module.exports;\nLegacy.run = function () {};\n"),
        ["Legacy module.exports true"],
    );
}

#[test]
fn module_exports_statements_record_the_export_object_and_renamed_properties() {
    let source = "const { lookup } = require('./dns');\n\
                  const Meta = module.exports;\n\
                  function render() {}\nfunction parse() {}\n\
                  module.exports = { render, toHtml: render, lookup, resolve: lookup, version: '1', run() {} };\n\
                  module.exports = parse;\n\
                  module.exports = function () {};\n\
                  module.exports.extra = parse;\n\
                  if (render) {\n  module.exports = render;\n}\n";
    assert_eq!(
        import_rows("src/meta.js", source),
        [
            ".::dns dns false", // the require itself
            ".::dns::lookup lookup true",
            ".::dns::lookup resolve true",
            "Meta module.exports true",
            "parse module.exports true",
            "render toHtml true",
        ],
    );
}

#[test]
fn a_typescript_file_reads_no_commonjs_binding() {
    // The same text in a `.ts` file: TypeScript declares no `commonjs`, so the
    // call keeps its receiver-method form and no `module.exports` row exists.
    let source = "const batch = require('./batch');\nconst { check } = require('./ssrf');\n\
                  export function run() {\n  check();\n  return batch.processSortedSet();\n}\n\
                  const Meta = module.exports;\nmodule.exports = { run: check };\n";
    assert_eq!(call_targets("src/x.ts", source), ["check", "processSortedSet"]);
    assert_eq!(import_rows("src/x.ts", source), [".::batch batch false", ".::ssrf ssrf false"]);
    assert_eq!(
        call_targets("src/x.js", source),
        [".::batch::processSortedSet", ".::ssrf::check"],
        "the JavaScript reading of the same text, for contrast"
    );
}

/// Sync ≡ reindex (NFR-RA-06): moving a function on and off the export
/// object re-binds the call through the untouched importer, and the synced
/// graph is the one a cold index of the edited tree builds.
#[test]
fn an_export_object_edit_rebinds_on_sync_and_matches_a_full_index() {
    let user = "const User = module.exports;\nUser.exists = async function (uids) {\n  return uids;\n};\n";
    let edited = "const Users = {};\nUsers.exists = async function (uids) {\n  return uids;\n};\n\
                  function exists() {}\nmodule.exports = { exists };\n";
    let caller = "const user = require('./user');\nexports.run = () => user.exists([1]);\n";

    let live = TempDir::new().unwrap();
    write(live.path(), "src/user.js", user);
    write(live.path(), "src/main.js", caller);
    coordinate::pin(live.path());
    let engine = Engine::start(live.path()).expect("engine starts");
    engine.index();
    let rt = engine.runtime().unwrap();
    assert_eq!(callers_of(rt, "src/user.js:exists"), ["src/main.js:run"]);
    write(live.path(), "src/user.js", edited);
    engine.sync(&["src/user.js".into()]);
    let bound: Vec<(String, String)> =
        edges_of(rt, EdgeKind::Calls).into_iter().filter(|(s, _)| s == "src/main.js:run").collect();
    assert_eq!(bound.len(), 1, "the call binds the one top-level `exists`: {bound:?}");

    let fresh = TempDir::new().unwrap();
    write(fresh.path(), "src/user.js", edited);
    write(fresh.path(), "src/main.js", caller);
    coordinate::pin(fresh.path());
    let cold = Engine::start(fresh.path()).expect("engine starts");
    cold.index();
    assert_eq!(graph_fingerprint(rt), graph_fingerprint(cold.runtime().unwrap()));
}

/// Sync ≡ reindex through a re-export the call never spells: `user.has()`
/// reaches `impl.js`'s `exists` only through `module.exports = { has: exists
/// }`, so renaming it dirties no token of the caller's row — only the
/// re-selection of every call through an import keeps the caller current.
#[test]
fn a_re_exported_definition_renamed_behind_a_module_rebinds_on_sync_and_matches_a_full_index() {
    let implementation = "function exists(uids) {\n  return uids;\n}\n";
    let edited = "function check(uids) {\n  return uids;\n}\n";
    let user = "const { exists } = require('./impl');\nmodule.exports = { has: exists };\n";
    let caller = "const user = require('./user');\nexports.run = () => user.has([1]);\n";

    let live = TempDir::new().unwrap();
    write(live.path(), "src/impl.js", implementation);
    write(live.path(), "src/user.js", user);
    write(live.path(), "src/main.js", caller);
    coordinate::pin(live.path());
    let engine = Engine::start(live.path()).expect("engine starts");
    engine.index();
    let rt = engine.runtime().unwrap();
    assert_eq!(callers_of(rt, "src/impl.js:exists"), ["src/main.js:run"]);
    write(live.path(), "src/impl.js", edited);
    engine.sync(&["src/impl.js".into()]);
    assert!(callers_of(rt, "src/impl.js:check").is_empty(), "nothing exports `check` as `has`");

    let fresh = TempDir::new().unwrap();
    write(fresh.path(), "src/impl.js", edited);
    write(fresh.path(), "src/user.js", user);
    write(fresh.path(), "src/main.js", caller);
    coordinate::pin(fresh.path());
    let cold = Engine::start(fresh.path()).expect("engine starts");
    cold.index();
    assert_eq!(graph_fingerprint(rt), graph_fingerprint(cold.runtime().unwrap()));
}

/// A `require` specifier resolves as an `import` one does (S-531): through the
/// file's `jsconfig.json` alias and a workspace package's `package.json`.
#[test]
fn a_require_through_an_alias_or_a_workspace_package_binds() {
    let (_tmp, engine) = index(&[
        ("package.json", r#"{ "private": true, "workspaces": ["apps/*", "packages/*"] }"#),
        ("apps/web/jsconfig.json", r#"{ "compilerOptions": { "baseUrl": ".", "paths": { "@/*": ["./src/*"] } } }"#),
        ("apps/web/src/lib/format.js", "exports.formatDate = function (d) {\n  return d;\n};\n"),
        ("packages/ui/package.json", r#"{ "name": "@acme/ui", "main": "./index.js" }"#),
        ("packages/ui/index.js", "function Button() {\n  return 1;\n}\nmodule.exports = { Button };\n"),
        (
            "apps/web/src/app.js",
            "const format = require('@/lib/format');\nconst { Button } = require('@acme/ui');\n\
             exports.App = function () {\n  Button();\n  return format.formatDate('x');\n};\n",
        ),
    ]);
    let rt = engine.runtime().unwrap();
    assert_eq!(imported_files(rt, "apps/web/src/app.js"), ["apps/web/src/lib/format.js", "packages/ui/index.js"]);
    assert_eq!(callers_of(rt, "apps/web/src/lib/format.js:formatDate"), ["apps/web/src/app.js:App"]);
    assert_eq!(callers_of(rt, "packages/ui/index.js:Button"), ["apps/web/src/app.js:App"]);
}
