//! PHP global functions bind across files, and includes are imports (S-551,
//! CR-176, FR-RS-26, FR-RS-03, NFR-RA-05, NFR-RA-06) — exercised end to end
//! through the public [`Engine`] façade against the SHIPPED PHP `symbols.scm`,
//! `references.scm` and `plugin.toml`.
//!
//! A bare call in a file that declares no namespace binds to the exactly-one
//! function of that name another such file declares, under every binding
//! policy — mantisbt's `gpc_get_int`, declared in `core/gpc_api.php` and called
//! from the pages. A namespaced file's bare call reads its own namespace's
//! function first, then the global one, as PHP's fallback does. A name two
//! files declare (a `function_exists` guard in each) binds neither, and a
//! method that only shares the name is never one.
//!
//! `require`, `require_once`, `include` and `include_once` of a string literal,
//! or of `__DIR__ . '/…'`, record an `Imports` row naming the file, read
//! against the including file's directory; any other argument — a variable, a
//! concatenation, an interpolated string, mantisbt's `require_api('…')` helper
//! — records none.
//!
//! Fixtures are written inline into temp directories, like every sibling
//! binding suite: a fixture tree checked into this repository would be indexed
//! into its own graph.
//!
//! [FR-RS-26]: ../../docs/specs/requirements/FR-RS-26.md

#![cfg(feature = "lang-php")]

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use logos_core::model::{EdgeKind, NodeId};
use logos_core::{Engine, Runtime};
use tempfile::TempDir;

#[path = "support/graph_fingerprint.rs"]
mod graph_fingerprint;
use graph_fingerprint::graph_fingerprint;

#[path = "support/coordinate.rs"]
mod coordinate;

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn tree(files: &[(&str, &str)]) -> TempDir {
    let tmp = TempDir::new().unwrap();
    for (rel, text) in files {
        write(tmp.path(), rel, text);
    }
    coordinate::pin(tmp.path());
    tmp
}

fn index(root: &Path) -> Engine {
    let engine = Engine::start(root).expect("engine starts");
    let result = engine.index();
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    engine
}

/// Every bound edge of `kind` as `(source file:name, target file:name)`, sorted.
fn edges_of(rt: &Runtime, kind: EdgeKind) -> Vec<(String, String)> {
    rt.submit_read(move |store| {
        let label: HashMap<NodeId, String> = store
            .all_nodes()?
            .into_iter()
            .map(|n| (n.id, format!("{}:{}", n.file_path.unwrap_or_default(), n.name)))
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

/// The `Calls` edges whose target is named `name`, in any file.
fn calls_to_name(rt: &Runtime, name: &str) -> Vec<(String, String)> {
    let suffix = format!(":{name}");
    edges_of(rt, EdgeKind::Calls)
        .into_iter()
        .filter(|(_, t)| t.ends_with(&suffix))
        .collect()
}

/// The `Imports` edges as `(source file, target file)`, sorted.
fn imported_files(rt: &Runtime) -> Vec<(String, String)> {
    rt.submit_read(move |store| {
        let file: HashMap<NodeId, String> = store
            .all_nodes()?
            .into_iter()
            .map(|n| (n.id, n.file_path.unwrap_or_default()))
            .collect();
        let mut out: Vec<(String, String)> = store
            .all_edges()?
            .into_iter()
            .filter(|e| e.kind == EdgeKind::Imports)
            .map(|e| (file[&e.source].clone(), file[&e.target].clone()))
            .collect();
        out.sort();
        Ok(out)
    })
    .expect("read runs")
}

/// The targets of every `Imports` row sourced in `file`, bound or not, sorted.
fn import_rows_in(rt: &Runtime, file: &str) -> Vec<String> {
    let file = file.to_string();
    rt.submit_read(move |store| {
        let in_file: HashSet<String> = store
            .all_nodes()?
            .into_iter()
            .filter(|n| n.file_path.as_deref() == Some(file.as_str()))
            .map(|n| n.symbol.as_str().to_string())
            .collect();
        let mut out: Vec<String> = store
            .unresolved_refs()?
            .into_iter()
            .filter(|r| r.kind == EdgeKind::Imports && in_file.contains(r.source_symbol.as_str()))
            .map(|r| r.target)
            .collect();
        out.sort();
        Ok(out)
    })
    .expect("read runs")
}

/// `.logos/config.toml` selecting the binding `policy`.
fn policy(name: &str) -> (&'static str, String) {
    (".logos/config.toml", format!("[resolution]\npolicy = \"{name}\"\n"))
}

const POLICIES: [&str; 3] = ["strict", "balanced", "aggressive"];

/// mantisbt's shape: an API file of global functions, each guarded the way the
/// application guards nothing — a plain top-level `function`.
const GPC_API: (&str, &str) = (
    "core/gpc_api.php",
    "<?php\nfunction gpc_get_int( $p_var_name, $p_default = null ) {\n\treturn 0;\n}\n\nfunction gpc_get_string( $p_var_name ) {\n\treturn '';\n}\n",
);

/// A page calling the API from its top level and from a function of its own,
/// as `bug_view_page.php` does.
const PAGE: (&str, &str) = (
    "bug_view_page.php",
    "<?php\nrequire_once( 'core.php' );\n\n$f_bug_id = gpc_get_int( 'id' );\n\nfunction print_page() {\n\t$t_id = gpc_get_int( 'id', 0 );\n}\n",
);

// ── Global functions ─────────────────────────────────────────────────────────

/// B2: `gpc_get_int` binds at its sites in another file under every policy —
/// before S-551 only the aggressive workspace fallback reached it.
#[test]
fn a_bare_call_binds_the_one_global_function_another_file_declares_under_every_policy() {
    for name in POLICIES {
        let config = policy(name);
        let tmp = tree(&[GPC_API, PAGE, (config.0, &config.1)]);
        let engine = index(tmp.path());
        let rt = engine.runtime().unwrap();
        assert_eq!(
            calls_to_name(rt, "gpc_get_int"),
            [
                ("bug_view_page.php:bug_view_page".to_string(), "core/gpc_api.php:gpc_get_int".to_string()),
                ("bug_view_page.php:print_page".to_string(), "core/gpc_api.php:gpc_get_int".to_string()),
            ],
            "policy {name}"
        );
    }
}

/// A name two files declare — each behind a `function_exists` guard, as a
/// plugin and the core both do — binds neither, under every policy.
#[test]
fn a_function_two_files_declare_stays_unbound_under_every_policy() {
    let guarded = |file: &'static str| {
        (
            file,
            "<?php\nif ( !function_exists( 'html_header' ) ) {\n\tfunction html_header() {}\n}\n",
        )
    };
    for name in POLICIES {
        let config = policy(name);
        let tmp = tree(&[
            guarded("core/html_api.php"),
            guarded("plugins/Theme/html_api.php"),
            ("page.php", "<?php\nhtml_header();\n"),
            (config.0, &config.1),
        ]);
        let engine = index(tmp.path());
        let rt = engine.runtime().unwrap();
        assert!(calls_to_name(rt, "html_header").is_empty(), "policy {name}");
    }
}

/// A file whose namespace is not known — `namespace { … }` beside
/// `namespace Foo { … }` — may declare a global function: a global function of
/// a name it declares too binds nothing, from the global namespace or through a
/// namespaced call's fallback.
#[test]
fn a_function_a_file_of_unknown_namespace_declares_is_a_rival_of_the_global_one() {
    let tmp = tree(&[
        ("lib/global.php", "<?php\nfunction f() {}\n"),
        (
            "lib/multi.php",
            "<?php\nnamespace {\n\tfunction f() {}\n}\nnamespace Foo {\n\tclass X {}\n}\n",
        ),
        ("page.php", "<?php\nf();\n"),
        ("src/App/C.php", "<?php\nnamespace App;\n\nfunction show() {\n\tf();\n}\n"),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert!(calls_to_name(rt, "f").is_empty(), "{:?}", calls_to_name(rt, "f"));
}

/// A method that only shares the name is never a global function: the call
/// binds the one free function, not the class's method.
#[test]
fn a_method_that_only_shares_the_name_is_never_a_candidate() {
    let tmp = tree(&[
        GPC_API,
        PAGE,
        (
            "core/classes/Request.class.php",
            "<?php\nclass Request {\n\tfunction gpc_get_int( $p_name ) { return 1; }\n}\n",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let targets: Vec<String> = calls_to_name(rt, "gpc_get_int").into_iter().map(|(_, t)| t).collect();
    assert_eq!(targets, ["core/gpc_api.php:gpc_get_int", "core/gpc_api.php:gpc_get_int"]);
}

/// A namespaced file's function is not a global one: a call from a file that
/// declares no namespace never reaches `Lib\format_date` by these rungs, as PHP's
/// would not (the default policy; `aggressive`'s workspace-unique fallback still
/// runs after them).
#[test]
fn a_call_from_the_global_namespace_never_reaches_a_namespaced_function() {
    let tmp = tree(&[
        ("src/Lib/dates.php", "<?php\nnamespace Lib;\n\nfunction format_date() {}\n"),
        ("page.php", "<?php\nformat_date();\n"),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert!(calls_to_name(rt, "format_date").is_empty());
}

/// A namespaced file's bare call reads its own namespace's function first, then
/// the global one — PHP's fallback for an unqualified function name. Another
/// namespace's function is never reached under the default policy.
#[test]
fn a_namespaced_call_reads_its_namespace_then_the_global_function() {
    let tmp = tree(&[
        (
            "src/App/helpers.php",
            "<?php\nnamespace App;\n\nfunction render() {}\n",
        ),
        (
            "src/Other/helpers.php",
            "<?php\nnamespace Other;\n\nfunction only_other() {}\n",
        ),
        (
            "lib/global.php",
            "<?php\nfunction render() {}\n\nfunction log_message() {}\n",
        ),
        (
            "src/App/Controller.php",
            "<?php\nnamespace App;\n\nclass Controller {\n\tpublic function show() {\n\t\trender();\n\t\tlog_message();\n\t\tonly_other();\n\t}\n}\n",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let from_show: Vec<String> = edges_of(rt, EdgeKind::Calls)
        .into_iter()
        .filter(|(s, _)| s == "src/App/Controller.php:show")
        .map(|(_, t)| t)
        .collect();
    assert_eq!(from_show, ["lib/global.php:log_message", "src/App/helpers.php:render"]);
}

/// The global-function rung decides nothing when no global function has the
/// name: a namespaced call neither its namespace nor the global namespace
/// supplies passes on to the next rung — under `aggressive`, the
/// workspace-unique name, as before S-551; under every other policy, nothing.
#[test]
fn a_namespaced_call_no_rung_supplies_reaches_only_the_aggressive_fallback() {
    for name in POLICIES {
        let config = policy(name);
        let tmp = tree(&[
            ("src/Other/helpers.php", "<?php\nnamespace Other;\n\nfunction only_other() {}\n"),
            (
                "src/App/Controller.php",
                "<?php\nnamespace App;\n\nfunction show() {\n\tonly_other();\n}\n",
            ),
            (config.0, &config.1),
        ]);
        let engine = index(tmp.path());
        let rt = engine.runtime().unwrap();
        let bound = calls_to_name(rt, "only_other");
        if name == "aggressive" {
            assert_eq!(
                bound,
                [("src/App/Controller.php:show".to_string(), "src/Other/helpers.php:only_other".to_string())],
                "policy {name}"
            );
        } else {
            assert!(bound.is_empty(), "policy {name}: {bound:?}");
        }
    }
}

/// The global fallback is exactly-one too: two global `log_message`s leave a
/// namespaced call unbound rather than picking one.
#[test]
fn the_global_fallback_of_a_namespaced_call_is_exactly_one() {
    let tmp = tree(&[
        ("lib/a.php", "<?php\nfunction log_message() {}\n"),
        ("lib/b.php", "<?php\nfunction log_message() {}\n"),
        (
            "src/App/Controller.php",
            "<?php\nnamespace App;\n\nfunction show() {\n\tlog_message();\n}\n",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert!(calls_to_name(rt, "log_message").is_empty());
}

/// A function is a member of its namespace, so a `use function` import names
/// it: the call binds from another namespace, and so does the import.
#[test]
fn a_use_function_import_binds_the_function_it_names() {
    let tmp = tree(&[
        ("src/Util/strings.php", "<?php\nnamespace Util;\n\nfunction slugify() {}\n"),
        (
            "src/App/Controller.php",
            "<?php\nnamespace App;\n\nuse function Util\\slugify;\n\nfunction show() {\n\tslugify();\n}\n",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        calls_to_name(rt, "slugify"),
        [("src/App/Controller.php:show".to_string(), "src/Util/strings.php:slugify".to_string())]
    );
    assert_eq!(
        imported_files(rt),
        [("src/App/Controller.php".to_string(), "src/Util/strings.php".to_string())]
    );
}

// ── Includes ─────────────────────────────────────────────────────────────────

/// A literal include — bare or parenthesised, any of the four keywords — and
/// one under `__DIR__` record an `Imports` row naming the file beside the
/// includer, and bind to it.
#[test]
fn literal_includes_record_imports_to_the_files_they_name() {
    let tmp = tree(&[
        ("core.php", "<?php\n"),
        GPC_API,
        ("core/lib/a.php", "<?php\n"),
        ("core/lib/b.php", "<?php\n"),
        ("config/defaults.php", "<?php\n"),
        (
            "core/boot.php",
            "<?php\nrequire_once( 'gpc_api.php' );\nrequire 'lib/a.php';\ninclude( __DIR__ . '/lib/b.php' );\ninclude_once __DIR__ . '/../config/defaults.php';\nrequire \"../core.php\";\n",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let from_boot: Vec<String> = imported_files(rt)
        .into_iter()
        .filter(|(s, _)| s == "core/boot.php")
        .map(|(_, t)| t)
        .collect();
    assert_eq!(
        from_boot,
        ["config/defaults.php", "core.php", "core/gpc_api.php", "core/lib/a.php", "core/lib/b.php"]
    );
}

/// Every keyword in every literal form the query admits records its row and
/// binds: `include`, `include_once`, `require` and `require_once`, each bare and
/// parenthesised, each of a literal and of `__DIR__ . '/…'` — sixteen forms, each
/// naming a file of its own, so a form the query lost is a missing edge rather
/// than a duplicate one.
#[test]
fn every_include_keyword_and_form_records_an_import() {
    let mut files: Vec<(String, String)> = Vec::new();
    let mut boot = String::from("<?php\n");
    for keyword in ["include", "include_once", "require", "require_once"] {
        let forms = [
            format!("{keyword} '{keyword}_bare.php';"),
            format!("{keyword}( '{keyword}_paren.php' );"),
            format!("{keyword} __DIR__ . '/{keyword}_dir.php';"),
            format!("{keyword}( __DIR__ . '/{keyword}_dir_paren.php' );"),
        ];
        for form in forms {
            let file = form.split(['\'', '/']).find(|s| s.ends_with(".php")).unwrap().to_string();
            files.push((format!("core/{file}"), "<?php\n".to_string()));
            boot.push_str(&form);
            boot.push('\n');
        }
    }
    files.push(("core/boot.php".to_string(), boot));
    let tree_files: Vec<(&str, &str)> = files.iter().map(|(p, t)| (p.as_str(), t.as_str())).collect();
    let tmp = tree(&tree_files);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(import_rows_in(rt, "core/boot.php").len(), 16);
    let mut want: Vec<(String, String)> = files[..16]
        .iter()
        .map(|(path, _)| ("core/boot.php".to_string(), path.clone()))
        .collect();
    want.sort();
    assert_eq!(imported_files(rt), want);
}

/// A dynamic include records nothing: a variable, a concatenation with
/// anything but `__DIR__` (a constant such as `FOO` included), an interpolated
/// string, an absolute path, and mantisbt's `require_api( '…' )` helper, which
/// is a call.
#[test]
fn dynamic_includes_record_nothing() {
    let tmp = tree(&[
        GPC_API,
        (
            "core/boot.php",
            "<?php\n$t_dir = __DIR__;\nrequire_once( $t_dir . '/gpc_api.php' );\nrequire_once( $t_require_path );\ninclude \"$t_dir/gpc_api.php\";\nrequire '/etc/mantis/gpc_api.php';\nrequire __DIR__ . 'gpc_api.php';\nrequire_once( dirname( __FILE__ ) . '/gpc_api.php' );\nrequire_api( 'gpc_api.php' );\nrequire FOO . '/gpc_api.php';\nrequire_once( MANTIS_CORE . '/gpc_api.php' );\n",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(import_rows_in(rt, "core/boot.php"), Vec::<String>::new());
    assert!(imported_files(rt).is_empty());
}

/// An include binds no name: its row carries no alias, so an extension-less
/// `require 'lib/helpers'` never shadows a call `helpers()`, which still binds
/// the global function.
#[test]
fn an_include_binds_no_name() {
    let tmp = tree(&[
        ("lib/fns.php", "<?php\nfunction helpers() {}\n"),
        ("page.php", "<?php\nrequire 'lib/helpers';\nhelpers();\n"),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let aliases: Vec<(String, Option<String>)> = rt
        .submit_read(|store| {
            Ok(store
                .unresolved_refs()?
                .into_iter()
                .filter(|r| r.kind == EdgeKind::Imports)
                .map(|r| (r.target, r.alias))
                .collect())
        })
        .expect("read runs");
    assert_eq!(aliases, [(".::lib::helpers".to_string(), None)]);
    assert_eq!(
        calls_to_name(rt, "helpers"),
        [("page.php:page".to_string(), "lib/fns.php:helpers".to_string())]
    );
}

/// An include of a file the repository does not hold records its row and binds
/// nothing; neither does one that climbs out of the repository.
#[test]
fn an_include_of_a_missing_file_stays_unbound() {
    let tmp = tree(&[(
        "page.php",
        "<?php\nrequire_once( 'mantis_offline.php' );\nrequire_once( '../../outside.php' );\n",
    )]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(import_rows_in(rt, "page.php").len(), 2);
    assert!(imported_files(rt).is_empty());
}

// ── Sync ≡ reindex ───────────────────────────────────────────────────────────

/// Index `initial`, overwrite `edits` (an empty text removes the file) and sync
/// them; then index the post-edit tree from scratch. The two graphs must be
/// identical, and the synced one's runtime is handed to `check`.
fn synced_equals_reindexed(initial: &[(&str, &str)], edits: &[(&str, &str)], check: impl FnOnce(&Runtime)) {
    let tmp_a = tree(initial);
    let engine_a = index(tmp_a.path());
    let root_a = tmp_a.path().canonicalize().expect("canonicalize root");
    let mut changed: Vec<PathBuf> = Vec::new();
    for (rel, text) in edits {
        if text.is_empty() {
            fs::remove_file(tmp_a.path().join(rel)).unwrap();
        } else {
            write(tmp_a.path(), rel, text);
        }
        changed.push(root_a.join(rel));
    }
    engine_a.sync(&changed);
    let rt_a = engine_a.runtime().unwrap();

    let mut final_state: BTreeMap<&str, &str> = initial.iter().copied().collect();
    for (rel, text) in edits {
        if text.is_empty() {
            final_state.remove(rel);
        } else {
            final_state.insert(rel, text);
        }
    }
    let files: Vec<(&str, &str)> = final_state.into_iter().collect();
    let tmp_b = tree(&files);
    let engine_b = index(tmp_b.path());

    assert_eq!(
        graph_fingerprint(rt_a),
        graph_fingerprint(engine_b.runtime().unwrap()),
        "sync-to-state must equal index-of-state"
    );
    check(rt_a);
}

const NAMESPACED_CALLER: (&str, &str) = (
    "src/App/Controller.php",
    "<?php\nnamespace App;\n\nfunction show() {\n\tlog_message();\n}\n",
);
const GLOBAL_LOG: (&str, &str) = ("lib/global.php", "<?php\nfunction log_message() {}\n");

#[test]
fn a_global_function_added_in_another_file_binds_on_sync_as_on_reindex() {
    synced_equals_reindexed(&[PAGE], &[GPC_API], |rt| {
        assert_eq!(calls_to_name(rt, "gpc_get_int").len(), 2);
    });
}

#[test]
fn a_duplicate_global_function_added_unbinds_on_sync_as_on_reindex() {
    let twin = ("plugins/Gpc/gpc_api.php", "<?php\nfunction gpc_get_int() {}\n");
    synced_equals_reindexed(&[GPC_API, PAGE], &[twin], |rt| {
        assert!(calls_to_name(rt, "gpc_get_int").is_empty());
    });
}

#[test]
fn the_namespace_gaining_the_function_retargets_its_call_on_sync_as_on_reindex() {
    let own = ("src/App/log.php", "<?php\nnamespace App;\n\nfunction log_message() {}\n");
    synced_equals_reindexed(&[GLOBAL_LOG, NAMESPACED_CALLER], &[own], |rt| {
        assert_eq!(
            calls_to_name(rt, "log_message"),
            [("src/App/Controller.php:show".to_string(), "src/App/log.php:log_message".to_string())]
        );
    });
}

#[test]
fn a_global_function_file_gaining_a_namespace_unbinds_on_sync_as_on_reindex() {
    let moved = ("lib/global.php", "<?php\nnamespace Lib;\n\nfunction log_message() {}\n");
    synced_equals_reindexed(&[GLOBAL_LOG, NAMESPACED_CALLER], &[moved], |rt| {
        assert!(calls_to_name(rt, "log_message").is_empty());
    });
}

#[test]
fn an_included_file_added_binds_on_sync_as_on_reindex() {
    synced_equals_reindexed(&[PAGE], &[("core.php", "<?php\n")], |rt| {
        assert_eq!(imported_files(rt), [("bug_view_page.php".to_string(), "core.php".to_string())]);
    });
}
