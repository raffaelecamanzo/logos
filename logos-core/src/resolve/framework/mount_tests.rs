//! Unit tests of the mount join (S-822): router, mount, import and export facts
//! read off real TypeScript-family source by the shared `frameworks` query, and
//! the join over them. Store-less: a specifier `./x` names the scanned file
//! `src/x.<ext>` here; in the pass the binder reads it ([`super::super::import_file`]).

use super::*;

/// One scanned file's routes after the join: sorted `METHOD path` names, and the
/// file's refusal count.
type Joined = (Vec<String>, usize);

/// Scan each `(rel, source)` and join them, with `./x` from any importer naming
/// the scanned `src/x.js` or `src/x.ts`.
#[cfg(feature = "lang-typescript")]
fn joined(files: &[(&str, &str)]) -> Vec<Joined> {
    let registry = crate::plugin::LanguageRegistry::load(std::env::temp_dir()).expect("registry loads");
    let mut scanned: Vec<(String, FileMatches)> = files
        .iter()
        .map(|(rel, source)| {
            let ext = rel.rsplit('.').next().expect("an extension");
            let plugin = registry.for_extension(ext).expect("a TypeScript-family plugin");
            let matches = super::super::scan_source(&mut tree_sitter::Parser::new(), plugin, rel, source, None);
            (rel.to_string(), matches)
        })
        .collect();
    let rels: Vec<String> = scanned.iter().map(|(rel, _)| rel.clone()).collect();
    let mut views: Vec<(&str, &mut FileMatches)> = scanned.iter_mut().map(|(rel, m)| (rel.as_str(), m)).collect();
    join(&mut views, |_, spec| {
        let stem = format!("src/{}", spec.trim_start_matches("./"));
        rels.iter().find(|rel| rel.rsplit_once('.').is_some_and(|(s, _)| s == stem)).cloned()
    });
    scanned
        .into_iter()
        .map(|(_, m)| {
            let mut names: Vec<String> = m.routes.iter().map(|r| format!("{} {}", r.method, r.path)).collect();
            names.sort();
            (names, m.refusals.len())
        })
        .collect()
}

/// The routes of one file joined alone, and its refusal count.
#[cfg(feature = "lang-typescript")]
fn alone(rel: &str, source: &str) -> Joined {
    joined(&[(rel, source)]).remove(0)
}

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

/// Each spelling of a router the framework made composes under its mount. A
/// maker one character off — another module's `Router`, a `Routers()`, a
/// `require('expressive')` — makes no router: its registration keeps its
/// written path, and the `use` that would mount it keeps its route.
#[cfg(feature = "lang-typescript")]
#[test]
fn a_router_is_one_the_framework_module_made() {
    let mounted = |head: &str, maker: &str| {
        format!("{head}\nconst app = express();\nconst r = {maker};\nfunction h() {{}}\nr.get('/x', h);\napp.use('/m', r);\n")
    };
    let express = "const express = require('express');";
    for (head, maker) in [
        (express, "express.Router()"),
        ("import express from 'express';", "express.Router()"),
        ("import * as express from 'express';", "express.Router({ mergeParams: true })"),
        ("import express = require('express');", "express.Router()"),
        ("const express = require('express');\nconst { Router } = require('express');", "Router()"),
        ("import express, { Router } from 'express';", "Router()"),
        (express, "require('express').Router()"),
    ] {
        assert_eq!(alone("src/app.ts", &mounted(head, maker)), (names(&["GET /m/x"]), 0), "{head} {maker}");
    }
    // `var` declares a router as `const` does.
    let var = mounted(express, "express.Router()").replace("const r =", "var r =");
    assert_eq!(alone("src/app.js", &var), (names(&["GET /m/x"]), 0), "var");
    for (head, maker) in [
        ("const express = require('express');\nconst other = require('other');", "other.Router()"),
        (express, "express.Routers()"),
        (express, "require('expressive').Router()"),
        ("const express = require('express');\nconst { Router } = require('other');", "Router()"),
        ("const express = require('express');\nconst Router = require('express');", "Router()"),
    ] {
        assert_eq!(alone("src/app.js", &mounted(head, maker)), (names(&["ANY /m", "GET /x"]), 0), "{head} {maker}");
    }
}

/// A mount's path is its written first argument; with none, the router is
/// mounted at the root, whether it is the first argument or follows middleware
/// a call or function builds. A first argument that may hold a path — a name,
/// a template, an expression — is unknown: the route is refused and counted.
#[cfg(feature = "lang-typescript")]
#[test]
fn a_mount_path_is_a_literal_the_root_or_unknown() {
    let with = |mount: &str| {
        format!(
            "const express = require('express');\nconst app = express();\nconst r = express.Router();\nfunction h() {{}}\nr.get('/x', h);\nr.get('/', h);\n{mount}\n"
        )
    };
    let cases: [(&str, &[&str], usize); 13] = [
        ("app.use('/api', r);", &["GET /api", "GET /api/x"], 0),
        ("app.use('/api/', r);", &["GET /api", "GET /api/x"], 0),
        ("app.use('/api', auth, r);", &["GET /api", "GET /api/x"], 0),
        ("app.use(r);", &["GET /", "GET /x"], 0),
        // A chain holds two calls in one callee: only the mounting one is no route.
        ("app.use('/health', h).use('/api', r);", &["ANY /health", "GET /api", "GET /api/x"], 0),
        ("app.use('/api', r).use('/health', h);", &["ANY /health", "GET /api", "GET /api/x"], 0),
        ("app.use(helmet(), r);", &["GET /", "GET /x"], 0),
        ("app.use((req, res, next) => next(), r);", &["GET /", "GET /x"], 0),
        ("app.use(function (req, res, next) { next(); }, r);", &["GET /", "GET /x"], 0),
        ("app.use(cors, r);", &[], 2),
        ("app.use(`${base}/api`, r);", &[], 2),
        ("app.use(base + '/api', r);", &[], 2),
        // A readable mount promotes, and an unreadable one beside it counts each
        // registration once.
        ("app.use('/a', r);\napp.use(prefix, r);", &["GET /a", "GET /a/x"], 2),
    ];
    for (mount, routes, refused) in cases {
        assert_eq!(alone("src/app.js", &with(mount)), (names(routes), refused), "{mount}");
    }
    // A template-literal registration on a mounted router takes the mount's path
    // (the template patterns keep the receiver-name rule: a `router`, not `r`).
    let template = "const express = require('express');\nconst app = express();\nconst router = express.Router();\nfunction h() {}\nrouter.get(`/t/${id}`, h);\napp.use('/m', router);\n";
    assert_eq!(alone("src/app.js", template), (names(&["GET /m/t/${id}"]), 0));
}

/// A router mounted at two paths has a route under each; one mounted on a
/// router composes through both; a router mounted nowhere, or only on a router
/// mounted nowhere, or in a cycle, is refused — once per registration — and a
/// `use` that mounts a router is no route of its own.
#[cfg(feature = "lang-typescript")]
#[test]
fn a_router_composes_through_every_mount_and_refuses_without_one() {
    let source = "\
const express = require('express');
const app = express();
function h() {}
const twice = express.Router();
twice.get('/t', h);
app.use('/a', twice);
app.use('/b', twice);
const outer = express.Router();
const inner = express.Router();
inner.get('/deep', h);
outer.use('/in', inner);
app.use('/out', outer);
const lonely = express.Router();
lonely.get('/l', h);
lonely.post('/l', h);
const orphan = express.Router();
const child = express.Router();
child.get('/c', h);
orphan.use('/o', child);
const a = express.Router();
const b = express.Router();
a.get('/a', h);
a.use('/a', b);
b.use('/b', a);
app.usex('/near', lonely);
";
    let (routes, refused) = alone("src/app.js", source);
    assert_eq!(routes, names(&["GET /a/t", "GET /b/t", "GET /out/in/deep"]));
    // `lonely` twice (`usex` mounts nothing), `child` once, `a` once.
    assert_eq!(refused, 4);
}

/// A router mounted on a router mounted on a router joins all three paths.
#[cfg(feature = "lang-typescript")]
#[test]
fn a_chain_of_mounted_routers_joins_every_path() {
    let source = "const express = require('express');\nconst app = express();\nfunction h() {}\nconst a = express.Router();\nconst b = express.Router();\nconst c = express.Router();\na.get('/x', h);\nb.use('/b', a);\nc.use('/c', b);\napp.use('/app', c);\n";
    assert_eq!(alone("src/app.js", source), (names(&["GET /app/c/b/x"]), 0));
}

/// The store-less projection composes a mount in the file itself, and drops
/// the mount's own route, as the pass does.
#[cfg(feature = "lang-typescript")]
#[test]
fn routes_in_source_composes_a_mount_in_the_same_file() {
    let registry = crate::plugin::LanguageRegistry::load(std::env::temp_dir()).expect("registry loads");
    let plugin = registry.for_extension("js").expect("javascript");
    let source = "const express = require('express');\nconst app = express();\nconst r = express.Router();\nfunction h() {}\nr.get('/x', h);\napp.use('/m', r);\n";
    let routes: Vec<(String, String)> = super::super::routes_in_source(plugin, "src/app.js", source)
        .into_iter()
        .map(|r| (r.method, r.path))
        .collect();
    assert_eq!(routes, [("GET".to_string(), "/m/x".to_string())]);
}

/// A parameter hides the module's router of its name: the router a caller hands
/// in is not followed, so its registration keeps its written path, while the
/// module's own router still composes.
#[cfg(feature = "lang-typescript")]
#[test]
fn a_parameter_hides_an_outer_router() {
    let source = "\
const express = require('express');
const router = express.Router();
function h() {}
router.get('/own', h);
module.exports = function (app, router) {
  router.get('/handed', h);
  app.use('/m', module.exports.router);
};
const register = (router) => router.get('/arrow', h);
const app = express();
app.use('/api', router);
";
    // `module.exports.router` is no name the facts bind: that `use` is middleware.
    assert_eq!(alone("src/app.js", source), (names(&["ANY /m", "GET /api/own", "GET /arrow", "GET /handed"]), 0));
    // An optional parameter hides it too.
    let optional = "import express from 'express';\nconst router = express.Router();\nfunction h() {}\nrouter.get('/own', h);\nexport function register(router?: express.Router) {\n  router.get('/handed', h);\n}\nconst app = express();\napp.use('/api', router);\n";
    assert_eq!(alone("src/app.ts", optional), (names(&["GET /api/own", "GET /handed"]), 0));
}

/// Any other binding of a router's name hides it over its scope: a declaration
/// of another value, a loop or `catch` variable, a destructured parameter. A
/// registration on such a name keeps its written path, and it neither takes the
/// outer router's mount nor counts as that router's refusal. A local of the name
/// an import binds hides the import too.
#[cfg(feature = "lang-typescript")]
#[test]
fn any_other_binding_hides_an_outer_router() {
    let head = "const express = require('express');\nconst app = express();\nfunction h() {}\nconst router = express.Router();\nrouter.get('/a', h);\n";
    let cases: [(&str, &str, &[&str]); 5] = [
        ("app.use('/api', router);", "function make() {\n  const router = createThing();\n  router.get('/inner', h);\n}", &["GET /api/a", "GET /inner"]),
        ("app.use('/api', router);", "for (const router of things) {\n  router.get('/loop', h);\n}", &["GET /api/a", "GET /loop"]),
        ("app.use('/api', router);", "function f({ router }) {\n  router.get('/d', h);\n}", &["GET /api/a", "GET /d"]),
        ("app.use('/api', router);", "try {} catch (router) {\n  router.get('/c', h);\n}", &["GET /api/a", "GET /c"]),
        ("", "function make() {\n  const router = createThing();\n  router.get('/inner', h);\n}", &["GET /inner"]),
    ];
    for (mount, body, routes) in cases {
        let refused = usize::from(mount.is_empty());
        assert_eq!(alone("src/app.js", &format!("{head}{mount}\n{body}\n")), (names(routes), refused), "{body}");
    }
    let items = "const express = require('express');\nconst router = express.Router();\nfunction h() {}\nrouter.get('/i', h);\nmodule.exports = router;\n";
    let app = "const express = require('express');\nconst app = express();\nconst items = require('./items');\nfunction f() {\n  const items = makeOther();\n  app.use('/x', items);\n}\napp.use('/api', items);\n";
    assert_eq!(joined(&[("src/items.js", items), ("src/app.js", app)]), [(names(&["GET /api/i"]), 0), (names(&["ANY /x"]), 0)]);
}

/// The router a module exports reaches its mount through every import and
/// export spelling; a namespace import is the module, not its router, and a
/// name the importer rebinds to a router of its own is that router.
#[cfg(feature = "lang-typescript")]
#[test]
fn an_imported_router_composes_through_each_export_and_import_spelling() {
    let router = |export: &str| {
        format!(
            "const express = require('express');\nconst router = express.Router();\nfunction h() {{}}\nrouter.get('/x', h);\n{export}\n"
        )
    };
    let cases = [
        ("module.exports = router;", "const items = require('./items');\napp.use('/m', items);"),
        ("module.exports = router;", "app.use('/m', require('./items'));"),
        ("exports.items = router;", "const { items } = require('./items');\napp.use('/m', items);"),
        ("module.exports.items = router;", "const { items: r } = require('./items');\napp.use('/m', r);"),
        ("export default router;", "import r from './items';\napp.use('/m', r);"),
        ("export { router };", "import { router } from './items';\napp.use('/m', router);"),
        ("export { router as items };", "import { items as r } from './items';\napp.use('/m', r);"),
    ];
    for (export, import) in cases {
        let app = format!("const express = require('express');\nconst app = express();\n{import}\n");
        let files = joined(&[("src/items.js", &router(export)), ("src/app.js", &app)]);
        assert_eq!(files, [(names(&["GET /m/x"]), 0), (names(&[]), 0)], "{export} / {import}");
    }
    let named_const = "const express = require('express');\nfunction h() {}\nexport const items = express.Router();\nitems.get('/x', h);\n";
    let app = "import express from 'express';\nimport { items } from './items';\nconst app = express();\napp.use('/m', items);\n";
    assert_eq!(joined(&[("src/items.ts", named_const), ("src/app.ts", app)])[0], (names(&["GET /m/x"]), 0));
    let export_equals = "import express = require('express');\nconst router = express.Router();\nfunction h() {}\nrouter.get('/x', h);\nexport = router;\n";
    let app = "import express = require('express');\nimport items = require('./items');\nconst app = express();\napp.use('/m', items);\n";
    assert_eq!(
        joined(&[("src/items.ts", export_equals), ("src/app.ts", app)]),
        [(names(&["GET /m/x"]), 0), (names(&[]), 0)],
        "`export =` and `import … = require`"
    );
    // The wrong export, a namespace, and an import the default export does not
    // answer leave the router mounted nowhere.
    for import in [
        "import * as r from './items';\napp.use('/m', r);",
        "import { other } from './items';\napp.use('/m', other);",
        "const r = require('./missing');\napp.use('/m', r);",
    ] {
        let app = format!("const express = require('express');\nconst app = express();\n{import}\n");
        let files = joined(&[("src/items.js", &router("module.exports = router;")), ("src/app.js", &app)]);
        assert_eq!(files[0], (names(&[]), 1), "{import}");
        assert_eq!(files[1], (names(&["ANY /m"]), 0), "{import}: middleware, as before");
    }
}

/// A mount's path is read by the shared prefix rule, and a root mount's
/// prefix joins without a doubled separator.
#[test]
fn a_mount_path_follows_the_shared_prefix_rule() {
    let mount = |path: Option<&str>, middleware_first: bool, first: Option<Span>| Mount {
        path: path.map(str::to_string),
        middleware_first,
        first,
        ..Mount::default()
    };
    assert_eq!(mount(Some(" /api "), false, Some((1, 5))).path_for((7, 8)), Some("/api".to_string()));
    assert_eq!(mount(Some("${base}/api"), false, None).path_for((7, 8)), None);
    assert_eq!(mount(None, false, Some((7, 8))).path_for((7, 8)), Some("/".to_string()));
    assert_eq!(mount(None, true, Some((1, 5))).path_for((7, 8)), Some("/".to_string()));
    assert_eq!(mount(None, false, Some((1, 5))).path_for((7, 8)), None);
    assert_eq!(join_under("/", "/items", true), "/items");
    assert_eq!(join_under("/", "/", true), "/");
}

/// The prefixes of a router are sorted and deduplicated, a mount that closes a
/// cycle is unknown without being unrolled, and a router mounted nowhere has
/// none.
#[test]
fn prefixes_are_ordered_and_a_cycle_is_unknown() {
    let mut edges: HashMap<RouterId, Vec<Edge>> = HashMap::new();
    let edge = |path: &str, parent: Option<RouterId>| Edge { path: Some(path.to_string()), parent };
    edges.insert((0, 0), vec![edge("/b", None), edge("/a", None), edge("/a", None)]);
    edges.insert((0, 1), vec![edge("/in", Some((0, 0))), Edge { path: None, parent: None }]);
    edges.insert((0, 2), vec![edge("/x", Some((0, 3)))]);
    edges.insert((0, 3), vec![edge("/y", Some((0, 2)))]);
    // (0, 4) is mounted at the root and on itself.
    edges.insert((0, 4), vec![edge("/r", None), edge("/again", Some((0, 4)))]);
    let at = |router: RouterId| prefixes(router, &edges, &[true], &mut HashMap::new());
    assert_eq!(at((0, 0)), [Some("/a".to_string()), Some("/b".to_string())]);
    assert_eq!(at((0, 1)), [None, Some("/a/in".to_string()), Some("/b/in".to_string())]);
    assert_eq!(at((0, 2)), [None]);
    assert_eq!(at((0, 4)), [None, Some("/r".to_string())]);
    assert!(at((0, 9)).is_empty());
    // A router mounted at `/` under `/a` is `/a` only where its framework absorbs.
    edges.insert((0, 5), vec![edge("/", Some((0, 0)))]);
    assert_eq!(prefixes((0, 5), &edges, &[true], &mut HashMap::new()), [Some("/a".to_string()), Some("/b".to_string())]);
    assert_eq!(prefixes((0, 5), &edges, &[false], &mut HashMap::new()), [Some("/a/".to_string()), Some("/b/".to_string())]);
}

/// A cycle reachable from the application promotes the router's one stated
/// path and counts the registration, without unrolling the cycle — and a cycle
/// with fan-out ends at once (it was exponential in the depth bound before).
#[cfg(feature = "lang-typescript")]
#[test]
#[cfg_attr(coverage, ignore = "wall-clock bound: meaningless under coverage instrumentation")]
fn a_mount_cycle_is_counted_not_unrolled() {
    let head = "const express = require('express');\nconst app = express();\nfunction h() {}\n";
    let pair = format!(
        "{head}const a = express.Router();\nconst b = express.Router();\na.get('/x', h);\napp.use('/api', a);\na.use('/b', b);\nb.use('/a', a);\n"
    );
    assert_eq!(alone("src/app.js", &pair), (names(&["GET /api/x"]), 1));
    let started = std::time::Instant::now();
    let fan = format!(
        "{head}const a = express.Router();\nconst b = express.Router();\na.get('/x', h);\napp.use('/api', a);\na.use('/x', b);\na.use('/y', b);\nb.use('/p', a);\nb.use('/q', a);\nconst r = express.Router();\nr.get('/r', h);\nr.use('/a', r);\nr.use('/b', r);\n"
    );
    assert_eq!(alone("src/app.js", &fan), (names(&["GET /api/x"]), 2));
    let budget = crate::perf::scaled(std::time::Duration::from_secs(10));
    assert!(started.elapsed() < budget, "{:?} over {budget:?}", started.elapsed());
}
