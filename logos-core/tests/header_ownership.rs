//! `.h` headers belong to C or C++ **per repository**, and the rule is
//! **disclosed** where it decided something ([FR-PL-07], [S-572], [S-582],
//! [CR-182], [CR-185]).
//!
//! C and C++ both claim the ambiguous `.h`. C reads it in a repository whose
//! `.c` files outnumber its C++ sources (`.cpp`, `.cc`, `.cxx`, `.hpp`, `.hh`,
//! `.hxx`) — redis's 475 C files beside 8 vendored C++ ones — and C++ reads it
//! otherwise. These tests pin:
//!
//! - the routing: a C repository's headers are labelled `c`, a C++ one's
//!   `cpp`, a repository of headers alone `cpp`; the decision is recorded with
//!   the graph and restored when an engine reopens the store;
//! - sync ≡ reindex as a C++ source arrives and leaves, moving the decision and
//!   re-reading every header;
//! - the words: `logos languages` carries a `note` on the C and C++ rows, and on
//!   no other; in a repository holding both C and C++ sources `status`'s row of
//!   the language that reads the headers carries `headers` and that language's
//!   note, side by side; a repository of one language, or neither (this
//!   repository's shape), grows no key.
//!
//! [FR-PL-07]: ../../docs/specs/requirements/FR-PL-07.md
//! [S-572]: ../../docs/planning/journal.md#s-572-c-is-declared-symbols-only-and-header-ownership-is-disclosed
//! [S-582]: ../../docs/planning/journal.md#s-582-c-repositories-own-their-headers-and-c-declarations-are-faithful
//! [CR-182]: ../../docs/requests/CR-182-c-is-declared-as-limited-support-and-header-ownership-is-disclosed.md
//! [CR-185]: ../../docs/requests/CR-185-c-resolves-includes-prototypes-headers-and-macros.md

#![cfg(all(feature = "lang-c", feature = "lang-cpp", feature = "lang-rust"))]

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use logos_core::model::NodeKind;
use logos_core::Engine;
use serde_json::Value;
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

/// A tree of `(path, contents)` files, indexed.
fn indexed(files: &[(&str, &str)]) -> (TempDir, Engine) {
    let tmp = TempDir::new().unwrap();
    for (rel, contents) in files {
        write(tmp.path(), rel, contents);
    }
    coordinate::pin(tmp.path());
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.index();
    (tmp, engine)
}

/// `status --json`'s per-language rows, as the surface serialises them.
fn status_rows(engine: &Engine) -> Vec<Value> {
    let status = serde_json::to_value(engine.status()).expect("status serialises");
    status["resolution_by_language"]
        .as_array()
        .expect("rows")
        .clone()
}

fn row<'a>(rows: &'a [Value], language: &str) -> Option<&'a Value> {
    rows.iter().find(|r| r["language"] == language)
}

/// Each indexed file's language, by path — read off the store, as the graph
/// records it.
fn languages(root: &Path) -> BTreeMap<String, String> {
    let db = rusqlite::Connection::open(root.join(".logos/logos.db")).expect("the store opens");
    let mut stmt = db.prepare("SELECT path, language FROM files").expect("prepare");
    stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?.unwrap_or_default())))
        .expect("query")
        .map(|row| row.expect("row"))
        .collect()
}

/// The descriptor `note` of `language`, as `logos languages` lists it.
fn described_note(root: &Path, language: &str) -> Option<String> {
    Engine::open(root)
        .languages()
        .languages
        .into_iter()
        .find(|l| l.name == language)
        .and_then(|l| l.note)
}

const C_SOURCE: &str = "#include \"util.h\"\nint run(void) { return helper(); }\n";
const C_HEADER: &str = "#define LIMIT 8\nstruct point { int x; int *y; };\nint helper(void);\n";
const CPP_SOURCE: &str = "int widget() { return 2; }\n";

#[test]
fn languages_note_the_dot_h_rule_on_the_c_and_cpp_rows_only() {
    let tmp = TempDir::new().unwrap();
    let info = Engine::open(tmp.path()).languages();
    for language in &info.languages {
        let note = language.note.as_deref();
        if language.name == "c" || language.name == "cpp" {
            let note = note.unwrap_or_else(|| panic!("{} carries a note", language.name));
            assert!(note.contains("`.h`"), "{} note names the extension: {note}", language.name);
            // The rule is a count of sources, each language's named.
            assert!(note.contains("more C sources") || note.contains("at least as many"), "{} note states the rule: {note}", language.name);
            assert!(note.contains("`.c`") && note.contains("`.hh`"), "{} note lists the sources: {note}", language.name);
        } else {
            assert_eq!(note, None, "{} has nothing to disclose", language.name);
        }
    }
    // Both rows list `.h`: each reads it in some repository.
    for name in ["c", "cpp"] {
        let listed = info.languages.iter().find(|l| l.name == name).unwrap_or_else(|| panic!("{name} is a row"));
        assert!(listed.extensions.iter().any(|e| e == "h"), "{name} lists .h: {:?}", listed.extensions);
    }
    // On the wire a language with nothing to disclose has no `note` key at all,
    // rather than `"note": null` on every row of the listing.
    let wire = serde_json::to_value(&info).expect("languages serialise");
    for language in wire["languages"].as_array().expect("rows") {
        let has_key = language.get("note").is_some();
        let discloses = language["name"] == "c" || language["name"] == "cpp";
        assert_eq!(has_key, discloses, "{language}");
    }
}

#[test]
fn a_c_repository_reads_its_headers_as_c() {
    let (tmp, engine) = indexed(&[
        ("src/main.c", C_SOURCE),
        ("src/util.c", "int helper(void) { return 1; }\n"),
        ("src/util.h", C_HEADER),
        ("include/api.h", "int run(void);\n"),
    ]);
    let rt = engine.runtime().unwrap();
    let files = languages(tmp.path());
    assert_eq!(files["src/util.h"], "c", "{files:?}");
    assert_eq!(files["include/api.h"], "c", "{files:?}");

    // Read as C, the header's macro is a Macro and its struct a Struct whose
    // pointer member is a Field (FR-EX-31).
    let kinds = rt
        .submit_read(|store| {
            Ok(store
                .all_nodes()?
                .into_iter()
                .filter(|n| n.file_path.as_deref() == Some("src/util.h"))
                .map(|n| (n.name, n.kind))
                .collect::<BTreeMap<_, _>>())
        })
        .expect("read runs");
    assert_eq!(kinds.get("LIMIT"), Some(&NodeKind::Macro), "{kinds:?}");
    assert_eq!(kinds.get("point"), Some(&NodeKind::Struct), "{kinds:?}");
    assert_eq!(kinds.get("y"), Some(&NodeKind::Field), "{kinds:?}");

    // One language, so nothing was decided between two: no row grows a key,
    // and there is no `cpp` row at all.
    let rows = status_rows(&engine);
    assert!(row(&rows, "cpp").is_none(), "{rows:?}");
    let c = row(&rows, "c").expect("a c row");
    assert_eq!(c["files"], 4, "{c}");
    assert!(c.get("headers").is_none() && c.get("note").is_none(), "{c}");
}

#[test]
fn vendored_cpp_beside_a_c_codebase_leaves_the_headers_c_and_discloses_them() {
    // The redis shape: C sources and their headers, plus a little vendored C++.
    // The count is the `.h` files only; the `c` row's `files` counts the `.c` too.
    let (tmp, engine) = indexed(&[
        ("src/a.c", C_SOURCE),
        ("src/b.c", "int b(void) { return 1; }\n"),
        ("src/c.c", "int c(void) { return 2; }\n"),
        ("src/util.h", C_HEADER),
        ("src/api.h", "int run(void);\n"),
        ("deps/vendored/example.cpp", CPP_SOURCE),
    ]);
    assert_eq!(languages(tmp.path())["src/util.h"], "c");
    let rows = status_rows(&engine);
    let c = row(&rows, "c").expect("a c row");
    assert_eq!(c["files"], 5, "{c}");
    assert_eq!(c["headers"], 2, "{c}");
    // The note is the descriptor's own, so `languages` and `status` say it once.
    assert_eq!(c["note"].as_str(), described_note(tmp.path(), "c").as_deref(), "{c}");
    let cpp = row(&rows, "cpp").expect("a cpp row");
    assert_eq!(cpp["files"], 1, "{cpp}");
    assert!(cpp.get("headers").is_none() && cpp.get("note").is_none(), "{cpp}");
}

#[test]
fn a_repository_with_as_much_cpp_as_c_reads_its_headers_as_cpp_and_discloses_them() {
    let (tmp, engine) = indexed(&[
        ("src/main.c", C_SOURCE),
        ("src/util.h", C_HEADER),
        ("src/widget.cpp", CPP_SOURCE),
    ]);
    assert_eq!(languages(tmp.path())["src/util.h"], "cpp");
    let rows = status_rows(&engine);
    let cpp = row(&rows, "cpp").expect("a cpp row");
    assert_eq!(cpp["files"], 2, "{cpp}");
    assert_eq!(cpp["headers"], 1, "{cpp}");
    assert_eq!(cpp["note"].as_str(), described_note(tmp.path(), "cpp").as_deref(), "{cpp}");
    let c = row(&rows, "c").expect("a c row");
    assert!(c.get("headers").is_none() && c.get("note").is_none(), "{c}");
}

#[test]
fn a_header_extension_that_is_not_dot_h_is_a_cpp_source_for_this_rule() {
    // The near miss: `.hh` ends in `h` but is C++-only, so it counts as a C++
    // source (one beside one `.c`), which gives `.h` to C++ — and is not counted
    // as a header. Read as a shared header, it would leave C++ no source and
    // give `.h` to C.
    let (tmp, engine) = indexed(&[
        ("src/main.c", C_SOURCE),
        ("src/util.h", C_HEADER),
        ("src/shapes.hh", "struct Shape { int sides; };\n"),
    ]);
    assert_eq!(languages(tmp.path())["src/util.h"], "cpp");
    let rows = status_rows(&engine);
    let cpp = row(&rows, "cpp").expect("a cpp row");
    assert_eq!(cpp["files"], 2, "{cpp}");
    assert_eq!(cpp["headers"], 1, "{cpp}");
}

#[test]
fn a_mixed_repository_with_no_header_is_not_disclosed() {
    // More C than C++, and not one `.h`: a count of zero beside the note would
    // disclose a rule that changed nothing here.
    let (_tmp, engine) = indexed(&[
        ("src/a.c", C_SOURCE),
        ("src/b.c", "int b(void) { return 1; }\n"),
        ("src/widget.cpp", CPP_SOURCE),
    ]);
    for r in &status_rows(&engine) {
        assert!(r.get("headers").is_none() && r.get("note").is_none(), "{r}");
    }
}

#[test]
fn an_uppercase_dot_h_is_a_header() {
    // The registry reads extensions in any case, so `OLD.H` is a header the rule
    // routes, and the count follows it.
    let (tmp, engine) = indexed(&[
        ("src/main.c", C_SOURCE),
        ("src/more.c", "int more(void) { return 1; }\n"),
        ("src/util.h", C_HEADER),
        ("src/OLD.H", "int old(void);\n"),
        ("src/widget.cpp", CPP_SOURCE),
    ]);
    assert_eq!(languages(tmp.path())["src/OLD.H"], "c");
    let rows = status_rows(&engine);
    let c = row(&rows, "c").expect("a c row");
    assert_eq!(c["files"], 4, "{c}");
    assert_eq!(c["headers"], 2, "{c}");
}

#[test]
fn a_relational_answer_over_a_header_carries_no_header_ownership() {
    // `status`-only, like `call_residue`: the denominator rows a relational
    // answer attaches come from `coverage_by_language`, which discloses nothing.
    let (_tmp, engine) = indexed(&[
        ("src/main.c", C_SOURCE),
        ("src/util.c", "int other(void) { return 1; }\n"),
        ("src/util.h", "static inline int helper(void) { return 1; }\n"),
        ("src/widget.cpp", CPP_SOURCE),
    ]);
    let status = status_rows(&engine);
    assert!(
        row(&status, "c").is_some_and(|r| r.get("headers").is_some()),
        "the fixture discloses on status: {status:?}"
    );

    let answer = serde_json::to_value(engine.callers("helper", None)).expect("callers serialise");
    let rows = answer["resolution_denominator"]["languages"]
        .as_array()
        .unwrap_or_else(|| panic!("a denominator is stated: {answer}"));
    let c = rows
        .iter()
        .find(|r| r["language"] == "c")
        .unwrap_or_else(|| panic!("the header's language is a denominator row: {rows:?}"));
    assert!(c.get("headers").is_none() && c.get("note").is_none(), "{c}");
}

#[test]
fn headers_with_no_source_are_cpp_and_not_disclosed() {
    // Nothing says these headers are C: neither language has a source, so the
    // outright claimant reads them, and no rule decided between two.
    let (tmp, engine) = indexed(&[("include/api.h", "int run(void);\n")]);
    assert_eq!(languages(tmp.path())["include/api.h"], "cpp");
    let rows = status_rows(&engine);
    let cpp = row(&rows, "cpp").expect("a cpp row");
    assert!(cpp.get("headers").is_none() && cpp.get("note").is_none(), "{cpp}");
}

#[test]
fn a_repository_with_no_c_or_cpp_files_is_unchanged() {
    // This repository's shape: neither language is a row, no row grows a key,
    // and the store records no decision.
    let (_tmp, engine) = indexed(&[("src/lib.rs", "pub fn a() {}\n")]);
    let rows = status_rows(&engine);
    assert!(row(&rows, "c").is_none() && row(&rows, "cpp").is_none(), "{rows:?}");
    for r in &rows {
        assert!(r.get("headers").is_none() && r.get("note").is_none(), "{r}");
    }
    let recorded = engine
        .runtime()
        .unwrap()
        .submit_read(|store| store.project_metadata(logos_core::graph_store::SHARED_OWNERS_KEY))
        .expect("read runs");
    assert_eq!(recorded, None);
}

#[test]
fn a_reopened_engine_reads_headers_as_the_graph_was_built() {
    // The decision is recorded with the graph: a process that runs no index —
    // a later `callers`, `status` — routes `.h` as the index did.
    let (tmp, engine) = indexed(&[("src/main.c", C_SOURCE), ("src/util.h", C_HEADER)]);
    assert_eq!(
        engine.registry().and_then(|r| r.for_path("src/util.h")).map(|p| p.name().to_string()),
        Some("c".to_string())
    );
    drop(engine);
    let reopened = Engine::start(tmp.path()).expect("engine reopens");
    assert_eq!(
        reopened.registry().and_then(|r| r.for_path("src/util.h")).map(|p| p.name().to_string()),
        Some("c".to_string()),
        "the reopened engine restores the recorded decision"
    );
    // And a fresh registry, which nothing has decided for, reads `.h` as C++.
    let fresh = logos_core::plugin::LanguageRegistry::load(tmp.path()).expect("registry loads");
    assert_eq!(fresh.for_path("src/util.h").map(|p| p.name()), Some("cpp"));
}

#[test]
fn sync_equals_reindex_as_a_cpp_source_moves_the_headers() {
    // One `.c`: the headers are C. A `.cpp` beside it ties the count, so C++
    // reads them and every header is re-read; removing it gives them back to C.
    // Each state equals a cold index of the same tree (NFR-RA-06).
    let (tmp, engine) = indexed(&[
        ("src/main.c", C_SOURCE),
        ("src/util.h", C_HEADER),
        ("include/api.h", "struct api { int (*run)(void); };\n"),
    ]);
    let rt = engine.runtime().unwrap();
    let cold = |tmp: &TempDir| {
        let copy = TempDir::new().unwrap();
        for rel in ["src/main.c", "src/util.h", "include/api.h", "src/widget.cpp"] {
            if let Ok(text) = fs::read_to_string(tmp.path().join(rel)) {
                write(copy.path(), rel, &text);
            }
        }
        coordinate::pin(copy.path());
        let cold = Engine::start(copy.path()).expect("engine starts");
        cold.index();
        let rt = cold.runtime().unwrap();
        (graph_fingerprint(rt), languages(copy.path()))
    };
    assert_eq!(languages(tmp.path())["include/api.h"], "c");

    let cpp = "src/widget.cpp";
    write(tmp.path(), cpp, CPP_SOURCE);
    let result = engine.sync(&[cpp.into()]);
    assert_eq!(result.files_modified, 2, "both headers are re-read: {result:?}");
    assert_eq!(languages(tmp.path())["include/api.h"], "cpp");
    assert_eq!((graph_fingerprint(rt), languages(tmp.path())), cold(&tmp));

    fs::remove_file(tmp.path().join(cpp)).unwrap();
    engine.sync(&[cpp.into()]);
    assert_eq!(languages(tmp.path())["include/api.h"], "c");
    assert_eq!((graph_fingerprint(rt), languages(tmp.path())), cold(&tmp));
}

#[test]
fn a_modified_source_is_counted_once() {
    // A tie gives `.h` to C++. Editing the `.c` re-extracts it as a stored file
    // that is also in the batch: counted twice, it would tip the tie to C and
    // re-read the header under C, where a fresh index keeps it C++.
    let (tmp, engine) = indexed(&[
        ("src/main.c", C_SOURCE),
        ("src/widget.cpp", CPP_SOURCE),
        ("src/util.h", C_HEADER),
    ]);
    assert_eq!(languages(tmp.path())["src/util.h"], "cpp");
    write(tmp.path(), "src/main.c", "int run(void) { return 0; }\n");
    let result = engine.sync(&["src/main.c".into()]);
    assert_eq!(result.files_modified, 1, "only the edited file is re-read: {result:?}");
    assert_eq!(languages(tmp.path())["src/util.h"], "cpp");
}

#[test]
fn a_source_that_does_not_load_decides_nothing() {
    // The walk sees two C sources and one C++ one, but one C file is not UTF-8
    // and is never stored: the files that load tie, so C++ reads `.h` — the
    // decision a sync, which counts only stored files, reaches too.
    let tmp = TempDir::new().unwrap();
    for (rel, contents) in [("src/main.c", C_SOURCE), ("src/widget.cpp", CPP_SOURCE), ("src/util.h", C_HEADER)] {
        write(tmp.path(), rel, contents);
    }
    fs::write(tmp.path().join("src/latin1.c"), b"int caf\xe9(void) { return 0; }\n").unwrap();
    coordinate::pin(tmp.path());
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.index();
    assert_eq!(languages(tmp.path())["src/util.h"], "cpp");
    let result = engine.sync(&["src/widget.cpp".into()]);
    assert_eq!(result.files_modified, 0, "nothing moved, nothing is re-read: {result:?}");
    assert_eq!(languages(tmp.path())["src/util.h"], "cpp");
}

#[test]
fn a_navigation_read_keeps_reading_headers_as_the_graph_was_built() {
    // A config change makes the first navigation read walk the tree (the
    // prologue purge). Two C++ sources written since the index would move `.h`
    // to C++ — but this read re-reads no header, so the registry keeps routing
    // `.h` as the graph holds it until a sync re-reads them.
    let (tmp, engine) = indexed(&[("src/main.c", C_SOURCE), ("src/util.h", C_HEADER)]);
    write(tmp.path(), "src/a.cpp", CPP_SOURCE);
    write(tmp.path(), "src/b.cpp", CPP_SOURCE);
    write(tmp.path(), ".logos/config.toml", "exclude = [\"docs/**\"]\n");
    let _ = engine.search("helper", None, None);
    assert_eq!(
        engine.registry().and_then(|r| r.for_path("src/util.h")).map(|p| p.name().to_string()),
        Some("c".to_string())
    );
    assert_eq!(languages(tmp.path())["src/util.h"], "c");
}

#[test]
fn an_allowlist_decides_over_the_sources_it_admits() {
    // `languages = ["c"]`: the two C++ files are never indexed, so they count
    // for nothing and C — admitted — reads the header.
    let (tmp, _engine) = indexed(&[
        (".logos/config.toml", "languages = [\"c\"]\n"),
        ("src/a.c", C_SOURCE),
        ("src/b.cpp", CPP_SOURCE),
        ("src/c.cpp", CPP_SOURCE),
        ("src/util.h", C_HEADER),
    ]);
    let files = languages(tmp.path());
    assert_eq!(files.get("src/util.h").map(String::as_str), Some("c"), "{files:?}");
    assert!(!files.contains_key("src/b.cpp"), "{files:?}");
}

#[test]
fn a_header_the_new_reader_does_not_admit_leaves_the_graph() {
    // `languages = ["c"]`: with its last C source gone, C++ reads `.h` — and C++
    // is not admitted, so a fresh index holds no header. The sync that removes
    // the source removes the header with it (NFR-RA-06).
    let (tmp, engine) = indexed(&[
        (".logos/config.toml", "languages = [\"c\"]\n"),
        ("src/a.c", C_SOURCE),
        ("src/util.h", C_HEADER),
    ]);
    assert_eq!(languages(tmp.path()).get("src/util.h").map(String::as_str), Some("c"));
    fs::remove_file(tmp.path().join("src/a.c")).unwrap();
    engine.sync(&["src/a.c".into()]);
    let files = languages(tmp.path());
    assert!(files.is_empty(), "{files:?}");
}
