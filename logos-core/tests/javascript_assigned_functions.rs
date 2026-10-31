//! JavaScript functions defined by assignment, `exports` or prototype are
//! symbols (S-539, [FR-EX-18], [CR-174]).
//!
//! Node/CommonJS JavaScript declares most of its functions by assigning them:
//! `User.exists = async function …`, `exports.processSortedSet = …`,
//! `PubSub.prototype.publish = function …`, and `{ load: function … }` in an
//! object literal. Before S-539 none of them was a symbol. The `javascript`
//! plugin now layers its own `symbols.scm` over the shared TypeScript-family
//! query, and each form is a declaration whose owner is read by **name**: the
//! declaration of `X` the file holds, or the file when it holds none.
//!
//! Capture-level checks run one in-memory file through the embedded plugin; the
//! fixtures follow the shapes that dominate NodeBB (`const User =
//! module.exports;` then `User.y = async function`, and the `module.exports =
//! function (User) { User.y = … }` mixin) and preact (`BaseComponent.prototype`).
//! The identity check compares against the shared query alone, loaded as an
//! on-disk override, so every symbol the graph held before is pinned.
//!
//! [FR-EX-18]: ../../docs/specs/requirements/FR-EX-18.md
//! [CR-174]: ../../docs/requests/CR-174-javascript-is-its-own-language-and-its-node-idioms-resolve.md
#![cfg(feature = "lang-typescript")]

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use logos_core::extract::{extract, Facts, FileInput, SymbolContext};
use logos_core::model::{EdgeKind, NodeKind};
use logos_core::plugin::LanguageRegistry;
use logos_core::Engine;

#[path = "support/coordinate.rs"]
mod coordinate;
#[path = "support/graph_fingerprint.rs"]
mod graph_fingerprint;
use graph_fingerprint::graph_fingerprint;

/// The shared TypeScript-family `symbols` query alone: what a `.js` file was
/// extracted with before S-539 layered its own patterns over it.
const SHARED_SYMBOLS: &str = include_str!("../plugins/tsx/queries/symbols.scm");

const PREFIX: &str = "logos . . . ";

/// A symbol minus its prefix, with its path descriptor unquoted
/// (``a.js`/User.` reads `a.js/User.`).
fn bare(symbol: &str) -> String {
    symbol.trim_start_matches(PREFIX).replace('`', "")
}

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// Extract `source` as `path` under the registry loaded from `root` (whose
/// `.logos/plugins/` overrides, if any, apply).
fn extract_under(root: &Path, path: &str, source: &str) -> Facts {
    let reg = LanguageRegistry::load(root).expect("embedded grammars load");
    let plugin = reg.for_path(path).unwrap_or_else(|| panic!("a plugin claims {path}"));
    let facts = extract(&FileInput::new(path, source), plugin, &SymbolContext::default());
    assert!(facts.warnings.is_empty(), "{path}: {:?}", facts.warnings);
    facts
}

fn extract_js(source: &str) -> Facts {
    let tmp = tempfile::tempdir().expect("tempdir");
    extract_under(tmp.path(), "a.js", source)
}

/// `symbol:kind` of every node but the file module, minus the symbol prefix.
fn nodes(facts: &Facts) -> BTreeSet<String> {
    facts
        .nodes
        .iter()
        .filter(|n| n.kind != NodeKind::Module)
        .map(|n| format!("{}:{}", bare(n.symbol.as_str()), n.kind.as_str()))
        .collect()
}

/// [`nodes`], each at the line its declaration starts: which declaration holds
/// a symbol, so two that trade symbols do not read as unchanged.
fn nodes_at_lines(facts: &Facts) -> BTreeSet<String> {
    facts
        .nodes
        .iter()
        .filter(|n| n.kind != NodeKind::Module)
        .map(|n| format!("{}:{}@{}", bare(n.symbol.as_str()), n.kind.as_str(), n.start_line))
        .collect()
}

/// The `Contains` edges as `parent -> child`, minus the symbol prefix.
fn contains(facts: &Facts) -> BTreeSet<String> {
    facts
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Contains)
        .map(|e| format!("{} -> {}", bare(e.source.as_str()), bare(e.target.as_str())))
        .collect()
}

fn node<'f>(facts: &'f Facts, name: &str) -> &'f logos_core::extract::NodeFact {
    facts
        .nodes
        .iter()
        .find(|n| n.name == name)
        .unwrap_or_else(|| panic!("no node {name}: {:?}", nodes(facts)))
}

fn set(of: &[&str]) -> BTreeSet<String> {
    of.iter().map(|s| s.to_string()).collect()
}

/// NodeBB `src/user/index.js`: the module object bound to a `const`, and its
/// functions assigned to it at file level.
#[test]
fn a_function_assigned_to_a_declared_object_is_a_function_of_that_object() {
    let facts = extract_js(
        "const User = module.exports;\n\
         \n\
         User.exists = async function (uids) {\n  return check(uids);\n};\n\
         User.existsBySlug = async (userslug) => check(userslug);\n",
    );
    assert_eq!(
        nodes(&facts),
        set(&["a.js/User.:constant", "a.js/User.exists().:function", "a.js/User.existsBySlug().:function"]),
    );
    let edges = contains(&facts);
    assert!(edges.contains("a.js/User. -> a.js/User.exists()."), "{edges:?}");
    assert!(edges.contains("a.js/User. -> a.js/User.existsBySlug()."), "{edges:?}");

    let exists = node(&facts, "exists");
    assert_eq!((exists.start_line, exists.end_line), (3, 5), "the node spans the assignment");
    assert!(!exists.exported, "an object that is not `exports` exports nothing");
    let params = exists.params.expect("the function's parameter list is its own");
    assert_eq!((params.min, params.max), (1, Some(1)));
    assert_eq!(node(&facts, "existsBySlug").params.map(|p| (p.min, p.max)), Some((1, Some(1))));

    // The body's call is the function's own reference, not the file's.
    let sources: BTreeSet<String> = facts
        .refs
        .iter()
        .filter(|r| r.kind == EdgeKind::Calls && r.target == "check")
        .map(|r| bare(r.source.as_str()))
        .collect();
    assert_eq!(sources, set(&["a.js/User.exists().", "a.js/User.existsBySlug()."]));
}

/// NodeBB `src/user/data.js`: the owner is the mixin's parameter, which the
/// file declares no symbol for, so the function is module-level.
#[test]
fn a_function_assigned_to_an_undeclared_object_is_module_level() {
    let facts = extract_js(
        "module.exports = function (User) {\n\
         \tUser.getUsersFields = async function (uids, fields) {\n\t\treturn [];\n\t};\n\
         };\n\
         window.boot = () => {};\n",
    );
    assert_eq!(nodes(&facts), set(&["a.js/getUsersFields().:function", "a.js/boot().:function"]));
    assert!(contains(&facts).contains("a.js/ -> a.js/getUsersFields()."), "{:?}", contains(&facts));
    assert_eq!(node(&facts, "getUsersFields").params.map(|p| (p.min, p.max)), Some((2, Some(2))));
}

/// The owner is looked up by name from where the assignment is written,
/// outwards, so a function declared inside another owns what is assigned to it
/// there, and a file-level object owns what a nested function assigns to it.
#[test]
fn the_owner_is_the_nearest_declaration_of_its_name_outwards() {
    let facts = extract_js(
        "const Topic = {};\n\
         function setup() {\n  function Topic() {}\n  Topic.init = function () {};\n  return Topic;\n}\n\
         function later() {\n  Topic.refresh = () => {};\n}\n",
    );
    let edges = contains(&facts);
    assert!(edges.contains("a.js/setup().Topic(). -> a.js/setup().Topic().init()."), "{edges:?}");
    assert!(edges.contains("a.js/Topic. -> a.js/Topic.refresh()."), "{edges:?}");
}

/// An owner is a declaration the assignment can see: one written in another
/// function's body — two bodies' local `Widget`s, a local `Foo` read at file
/// level, an `X` declared inside the assigned function itself — never owns
/// it, whatever scope the graph files that body's declarations under.
#[test]
fn a_declaration_local_to_another_body_never_owns() {
    let facts = extract_js(
        "exports.init = function () {\n  function Widget() {}\n  Widget.prototype.render = function () {};\n};\n\
         exports.other = function () {\n  function Widget() {}\n  Widget.prototype.render = function () {};\n};\n\
         exports.setup = function () {\n  var Foo = function () {};\n};\n\
         Foo.prototype.bar = function () {};\n\
         X.y = function () {\n  function X() {}\n};\n",
    );
    let edges = contains(&facts);
    assert!(edges.contains("a.js/Widget(). -> a.js/Widget().render()."), "{edges:?}");
    assert!(edges.contains("a.js/Widget(1). -> a.js/Widget(1).render()."), "{edges:?}");
    assert!(edges.contains("a.js/ -> a.js/bar()."), "{edges:?}");
    assert!(edges.contains("a.js/ -> a.js/y()."), "{edges:?}");
}

/// A parameter or a local the graph does not capture shadows a declaration of
/// its name further out: the assignment is to the parameter or local, so the
/// function is module-level, never under the outer declaration.
#[test]
fn a_parameter_or_local_shadowing_the_owner_leaves_the_function_module_level() {
    let facts = extract_js(
        "const User = require('./user');\n\
         module.exports = function (User) {\n  User.getFields = async function (uids) {};\n};\n\
         const config = {};\n\
         function build() {\n  const config = {};\n  config.get = function () {};\n  return config;\n}\n\
         function wire() {\n  config.reload = () => {};\n}\n",
    );
    let edges = contains(&facts);
    assert!(edges.contains("a.js/ -> a.js/getFields()."), "{edges:?}");
    assert!(edges.contains("a.js/ -> a.js/get()."), "{edges:?}");
    assert!(edges.contains("a.js/config. -> a.js/config.reload()."), "an unshadowed name still owns: {edges:?}");
}

/// NodeBB `src/batch.js` and the `module.exports.y` spelling: an exported
/// function of the module, at file level.
#[test]
fn an_exports_assignment_is_an_exported_module_level_function() {
    let facts = extract_js(
        "exports.processSortedSet = async function (setKey, process, options) {\n  return setKey;\n};\n\
         module.exports.processArray = (array, process) => array.map(process);\n",
    );
    assert_eq!(
        nodes(&facts),
        set(&["a.js/processSortedSet().:function", "a.js/processArray().:function"]),
    );
    assert!(node(&facts, "processSortedSet").exported);
    assert!(node(&facts, "processArray").exported);
    let params = |name| node(&facts, name).params.map(|p| (p.min, p.max));
    assert_eq!(params("processSortedSet"), Some((3, Some(3))));
    assert_eq!(params("processArray"), Some((2, Some(2))));
}

/// NodeBB `src/database/redis/pubsub.js` and preact `src/component.js`: a
/// prototype assignment is a Method of the function that is the class.
#[test]
fn a_prototype_assignment_is_a_method_of_its_constructor() {
    let facts = extract_js(
        "const PubSub = function () {\n  this.queue = [];\n};\n\
         PubSub.prototype.publish = function (event, data) {\n  return [event, data];\n};\n\
         export function BaseComponent(props, context) {}\n\
         BaseComponent.prototype.setState = function (update, callback) {};\n\
         BaseComponent.prototype.render = Fragment;\n\
         Imported.prototype.patched = function () {};\n",
    );
    assert_eq!(
        nodes(&facts),
        set(&[
            "a.js/PubSub().:function",
            "a.js/PubSub().publish().:method",
            "a.js/BaseComponent().:function",
            "a.js/BaseComponent().setState().:method",
            "a.js/patched().:method",
        ]),
        "a non-function value (`render = Fragment`) declares nothing; an owner the file does not declare leaves the method module-level",
    );
    let edges = contains(&facts);
    assert!(edges.contains("a.js/PubSub(). -> a.js/PubSub().publish()."), "{edges:?}");
    assert!(edges.contains("a.js/BaseComponent(). -> a.js/BaseComponent().setState()."), "{edges:?}");
    assert_eq!(node(&facts, "publish").params.map(|p| (p.min, p.max)), Some((2, Some(2))));
}

/// `{ y: function … }` and `{ y: () => … }` are Methods of the enclosing object
/// or function — at file level when nothing encloses them.
#[test]
fn an_object_literal_function_pair_is_a_method_of_what_encloses_it() {
    let facts = extract_js(
        "const api = {\n  load: function (id) {},\n  save: async (doc) => doc,\n  name: 'api',\n};\n\
         function setup() {\n  return { onClick: () => {} };\n}\n\
         module.exports = { start: function () {} };\n",
    );
    assert_eq!(
        nodes(&facts),
        set(&[
            "a.js/api.:constant",
            "a.js/api.load().:method",
            "a.js/api.save().:method",
            "a.js/setup().:function",
            "a.js/setup().onClick().:method",
            "a.js/start().:method",
        ]),
    );
    assert_eq!(node(&facts, "load").params.map(|p| (p.min, p.max)), Some((1, Some(1))));
    assert_eq!(node(&facts, "save").params.map(|p| (p.min, p.max)), Some((1, Some(1))));
}

/// The forms this story does not declare: a non-function value, a `this.x`
/// (the fields of S-542), `module.exports = …` itself (the module's own export,
/// S-540), a computed key and a string key.
#[test]
fn a_non_function_value_and_the_module_export_itself_declare_nothing() {
    let facts = extract_js(
        "module.exports = function () {};\n\
         exports.name = 'x';\n\
         Counter.count = 5;\n\
         foo.exports.y = function () {};\n\
         module.foo.z = function () {};\n\
         Counter.bar.w = function () {};\n\
         function Counter() {\n  this.handler = function () {};\n}\n\
         const o = { ['k']: function () {}, 'quoted': () => {}, n: 1 };\n",
    );
    assert_eq!(nodes(&facts), set(&["a.js/Counter().:function", "a.js/o.:constant"]));
}

/// TypeScript shares the family's query but not JavaScript's layer: the same
/// source in a `.ts` file declares none of the assignment forms.
#[test]
fn typescript_declares_none_of_the_javascript_assignment_forms() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let source = "const User = {};\nUser.exists = function () {};\nexports.run = () => {};\n";
    let facts = extract_under(tmp.path(), "a.ts", source);
    assert_eq!(nodes(&facts), set(&["a.ts/User.:constant"]));
}

/// The one-parameter arrow written without parentheses — the commonest
/// CommonJS spelling — declares in every form, with its one parameter.
#[test]
fn an_unparenthesised_arrow_declares_in_every_form() {
    let facts = extract_js(
        "const Bar = {};\nconst Foo = function () {};\nexports.f = x => x;\nmodule.exports.g = y => y;\n\
         Foo.prototype.h = z => z;\nBar.m = v => v;\nconst o = { k: w => w };\n",
    );
    let declared = nodes(&facts);
    for expected in ["a.js/f().:function", "a.js/g().:function", "a.js/Foo().h().:method", "a.js/Bar.m().:function", "a.js/o.k().:method"] {
        assert!(declared.contains(expected), "{expected} in {declared:?}");
    }
    for name in ["f", "g", "h", "m", "k"] {
        assert_eq!(node(&facts, name).params.map(|p| (p.min, p.max)), Some((1, Some(1))), "{name}");
    }
}

/// `module.exports.y = …` is at file level wherever it is written, a
/// function's body included.
#[test]
fn a_module_exports_assignment_in_a_function_body_is_still_module_level() {
    let facts = extract_js("function setup() {\n  module.exports.y = function () {};\n}\n");
    assert_eq!(nodes(&facts), set(&["a.js/setup().:function", "a.js/y().:function"]));
    assert!(contains(&facts).contains("a.js/ -> a.js/y()."), "{:?}", contains(&facts));
    assert!(node(&facts, "y").exported);
}

/// A top-level `let` or `var` object owns what is assigned to it, as a
/// `const` does — the pre-ES2015 spelling.
#[test]
fn a_let_or_var_object_owns_its_assigned_functions() {
    let facts = extract_js("let Cache = {};\nCache.get = function () {};\nvar Store = {};\nStore.put = () => {};\n");
    let edges = contains(&facts);
    assert!(edges.contains("a.js/Cache. -> a.js/Cache.get()."), "{edges:?}");
    assert!(edges.contains("a.js/Store. -> a.js/Store.put()."), "{edges:?}");
}

/// A declaration placed by name is a property, never a binding: it owns
/// nothing by its name, so the function of that name does.
#[test]
fn a_placed_declaration_never_owns_by_its_name() {
    let facts = extract_js("exports.exists = function () {};\nfunction exists() {}\nexists.inner = function () {};\n");
    assert!(contains(&facts).contains("a.js/exists(). -> a.js/exists().inner()."), "{:?}", contains(&facts));
}

/// An object-literal pair written in a function defined by assignment — the
/// NodeBB shape — is a Method of that function.
#[test]
fn a_pair_in_an_assigned_function_is_a_method_of_it() {
    let facts = extract_js("const X = {};\nX.y = function () {\n  return { z: function () {} };\n};\n");
    assert!(contains(&facts).contains("a.js/X.y(). -> a.js/X.y().z()."), "{:?}", contains(&facts));
}

/// Every symbol a `.js` file held before S-539 is unchanged (ADR-07), whatever
/// the new declarations sit beside or around: one of the same name earlier in
/// the same scope, a declaration nested in an assigned body, a shorthand method
/// in a value binding's object, and a function the new forms now own by name.
#[test]
fn every_symbol_the_shared_query_declares_is_unchanged() {
    let source = "exports.exists = function () {\n  function inner() {}\n  const cb = () => inner();\n};\n\
                  function exists() {}\n\
                  const PubSub = function () {};\n\
                  PubSub.prototype.publish = function () {\n  const publish = () => {};\n};\n\
                  const registry = {\n  handle() {},\n  load: function () {\n    function handle() {}\n  },\n};\n\
                  class Store {\n  get() {\n    return { get: () => 1 };\n  }\n}\n\
                  Store.get = function () {};\n\
                  const Topic = module.exports;\n\
                  Topic.exists = () => {};\n\
                  function Topic2() {}\n";
    let base_root = tempfile::tempdir().expect("tempdir");
    write(base_root.path(), ".logos/plugins/javascript/queries/symbols.scm", SHARED_SYMBOLS);
    let before = extract_under(base_root.path(), "a.js", source);
    let after = extract_js(source);

    let (before_nodes, after_nodes) = (nodes_at_lines(&before), nodes_at_lines(&after));
    let lost: Vec<&String> = before_nodes.difference(&after_nodes).collect();
    assert!(lost.is_empty(), "symbols changed: {lost:?}\nafter: {after_nodes:?}");
    let (before_edges, after_edges) = (contains(&before), contains(&after));
    let moved: Vec<&String> = before_edges.difference(&after_edges).collect();
    assert!(moved.is_empty(), "containment changed: {moved:?}");

    let added: BTreeSet<&String> = after_nodes.difference(&before_nodes).collect();
    assert_eq!(
        added,
        set(&[
            "a.js/exists(1).:function@1",
            "a.js/PubSub().publish().:method@7",
            "a.js/registry.load().:method@12",
            "a.js/Store#get().get().:method@18",
            "a.js/Store#get(1).:function@21",
            "a.js/Topic.exists().:function@23",
        ])
        .iter()
        .collect(),
        "each new declaration is numbered after the ones its scope already held",
    );
}

/// Sync ≡ reindex (NFR-RA-06): assignment functions added to, and removed
/// from, a file by a synced edit leave the graph a cold index of the edited
/// tree builds.
#[test]
fn a_synced_edit_of_assignment_functions_equals_a_fresh_index() {
    let user = "const User = module.exports;\nUser.exists = async function (uids) {\n  return lookup(uids);\n};\n\
                function lookup(uids) { return uids; }\n";
    let edited = "const User = module.exports;\nUser.existsBySlug = (slug) => lookup(slug);\n\
                  User.prototype.save = function () {};\nexports.helper = () => lookup([]);\n\
                  function lookup(uids) { return uids; }\n";
    let caller = "const user = require('./user');\nexports.run = () => user.exists([1]);\n";

    let live = tempfile::tempdir().expect("tempdir");
    write(live.path(), "src/user.js", user);
    write(live.path(), "src/main.js", caller);
    coordinate::pin(live.path());
    let engine = Engine::start(live.path()).expect("engine starts");
    engine.index();
    write(live.path(), "src/user.js", edited);
    engine.sync(&["src/user.js".into()]);

    let fresh = tempfile::tempdir().expect("tempdir");
    write(fresh.path(), "src/user.js", edited);
    write(fresh.path(), "src/main.js", caller);
    coordinate::pin(fresh.path());
    let cold = Engine::start(fresh.path()).expect("engine starts");
    cold.index();

    let synced = graph_fingerprint(engine.runtime().unwrap());
    assert_eq!(synced, graph_fingerprint(cold.runtime().unwrap()));
    assert!(synced.contains("User.existsBySlug()."), "the edit reached the synced graph");
    assert!(!synced.contains("User.exists()."), "the removed function left the synced graph");
}
