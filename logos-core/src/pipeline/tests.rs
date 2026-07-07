//! Unit tests for the pipeline's pure helpers (S-010).
//!
//! The orchestration paths (`index`/`sync`/`ensure_indexed`) need a live runtime
//! and a real project tree, so they are exercised end-to-end through the public
//! `Engine` façade in `tests/indexing.rs`. Here we pin the deterministic helpers
//! that the correctness of dirty detection and the containment guarantee rest on.

use std::path::{Path, PathBuf};

use super::{
    admits_file, hash_source, is_config_admitted, is_doc_admitted, load_files, relativize,
    supported_extension, Candidate, LoadedFile, ShadowStore,
};
use crate::config::Config;
use crate::plugin::LanguageRegistry;
use crate::runtime::{Runtime, RuntimeConfig};

#[cfg(feature = "lang-rust")]
use std::collections::HashMap;
#[cfg(feature = "lang-rust")]
use super::{persist_facts_chunked, persist_file};
#[cfg(feature = "lang-rust")]
use crate::extract::{extract_files, Facts, FileInput, SymbolContext};

fn registry() -> LanguageRegistry {
    let tmp = tempfile::tempdir().expect("tempdir");
    LanguageRegistry::load(tmp.path()).expect("embedded grammars load")
}

#[test]
fn hash_is_deterministic_and_content_sensitive() {
    // Dirty detection (FR-SY-03) hinges on the hash being a pure function of the
    // bytes: identical content → identical hash, any change → a different hash.
    let a = hash_source("fn main() {}\n");
    let b = hash_source("fn main() {}\n");
    let c = hash_source("fn main() { } \n"); // one byte different
    assert_eq!(a, b, "the same source must hash identically");
    assert_ne!(a, c, "a changed source must hash differently");
    assert_eq!(a.len(), 64, "blake3 hex digest is 32 bytes = 64 hex chars");
}

#[test]
fn shadow_store_tears_down_db_and_wal_shm_sidecars_on_drop() {
    // CR-052 / FR-GV-19: the shadow store must be fully removed — db AND its
    // `-wal`/`-shm` sidecars — when the guard drops. Simulate the three files a
    // live SQLite WAL store leaves behind, drop the guard, and assert the whole
    // directory is gone.
    let (db, wal, shm, dir) = {
        let shadow = ShadowStore::create().expect("shadow store dir is created");
        let db = shadow.db_path().to_path_buf();
        let dir = db.parent().unwrap().to_path_buf();
        assert!(dir.is_dir(), "the shadow directory exists while the guard lives");

        let sidecar = |suffix: &str| {
            let mut os = db.as_os_str().to_os_string();
            os.push(suffix);
            PathBuf::from(os)
        };
        let wal = sidecar("-wal");
        let shm = sidecar("-shm");
        std::fs::write(&db, b"db").unwrap();
        std::fs::write(&wal, b"wal").unwrap();
        std::fs::write(&shm, b"shm").unwrap();
        (db, wal, shm, dir)
    }; // guard drops here

    assert!(!db.exists(), "the shadow db is removed on drop");
    assert!(!wal.exists(), "the -wal sidecar is removed on drop");
    assert!(!shm.exists(), "the -shm sidecar is removed on drop");
    assert!(!dir.exists(), "the shadow directory itself is removed on drop");
}

#[test]
fn shadow_store_create_yields_distinct_paths_per_call() {
    // Concurrent verifies (and parallel tests) must never collide on one temp
    // path — each `create` mints a unique directory (pid + monotonic counter).
    let a = ShadowStore::create().expect("first shadow store");
    let b = ShadowStore::create().expect("second shadow store");
    assert_ne!(
        a.db_path(),
        b.db_path(),
        "each shadow store gets a distinct path"
    );
}

#[test]
fn supported_extension_tracks_loaded_grammars() {
    let reg = registry();
    assert!(
        supported_extension(&reg, None, "src/lib.rs"),
        "rs is a loaded grammar"
    );
    // The S-015 default build compiles the Python grammar in; a feature-gated
    // build without `lang-python` must not claim `.py`.
    assert_eq!(
        supported_extension(&reg, None, "src/script.py"),
        cfg!(feature = "lang-python")
    );
    // Markdown is a registered *documentation* grammar (S-033, CR-003), but the
    // code-discovery gate excludes documentation plugins: admitting `.md` into
    // discovery is S-034's job, so `supported_extension` still reports `.md` as
    // not a (code) source candidate even though the grammar is loaded.
    assert!(!supported_extension(&reg, None, "README.md"));
    // An extensionless file is never a source candidate.
    assert!(!supported_extension(&reg, None, "Makefile"));

    // CR-017 / S-081 / FR-CF-01: a non-empty `languages` allowlist gates code
    // admission by grammar name. `None` (above) admits every loaded code grammar;
    // an explicit list restricts to its members.
    let only_rust: std::collections::HashSet<String> = ["rust".to_string()].into_iter().collect();
    assert!(
        supported_extension(&reg, Some(&only_rust), "src/lib.rs"),
        "a grammar named in the allowlist is admitted"
    );
    #[cfg(feature = "lang-python")]
    assert!(
        !supported_extension(&reg, Some(&only_rust), "src/script.py"),
        "a loaded grammar absent from a non-empty allowlist is gated out"
    );
}

#[cfg(feature = "lang-markdown")]
#[test]
fn admits_file_routes_code_and_default_doc_globs() {
    // S-034: with documentation on (default), `admits_file` is the single gate
    // discovery and sync share. Code is always admitted; markdown only when the
    // doc globs scope it in.
    let reg = registry();
    let docs = Config::default()
        .documentation
        .compile()
        .expect("default doc globs compile")
        .expect("documentation is enabled by default");

    // Code: admitted regardless of the doc globs.
    assert!(admits_file(&reg, Some(&docs), None, None, "src/lib.rs"));
    // Documentation in-glob: under docs/, top-level .md, and a root README.
    assert!(admits_file(&reg, Some(&docs), None, None, "docs/spec.md"));
    assert!(admits_file(
        &reg,
        Some(&docs),
        None,
        None,
        "docs/planning/sprints/s.md"
    ));
    assert!(admits_file(&reg, Some(&docs), None, None, "README.md"));
    assert!(admits_file(&reg, Some(&docs), None, None, "CHANGELOG.md"));
    // Out-of-glob markdown: discovery's `**` walk surfaces it, but the doc gate
    // rejects it — AC1's "an `.md` outside the default globs is not indexed".
    assert!(!admits_file(&reg, Some(&docs), None, None, "notes/scratch.md"));
    assert!(!is_doc_admitted(&reg, Some(&docs), "notes/scratch.md"));
    // A non-grammar extension is never admitted.
    assert!(!admits_file(&reg, Some(&docs), None, None, "data/notes.txt"));
}

#[cfg(feature = "lang-markdown")]
#[test]
fn disabling_documentation_admits_no_markdown_but_keeps_code() {
    // AC3: disabling documentation produces no doc nodes — modelled here as the
    // admission gate refusing every markdown path while code is unaffected.
    let reg = registry();
    let config = Config {
        documentation: crate::config::Documentation {
            enabled: false,
            ..Default::default()
        },
        ..Config::default()
    };
    let docs = config
        .documentation
        .compile()
        .expect("compile is infallible when disabled");
    assert!(
        docs.is_none(),
        "disabled documentation compiles to no matcher"
    );

    assert!(!admits_file(&reg, docs.as_ref(), None, None, "docs/spec.md"));
    assert!(!admits_file(&reg, docs.as_ref(), None, None, "README.md"));
    assert!(
        admits_file(&reg, docs.as_ref(), None, None, "src/lib.rs"),
        "code indexing is independent of the documentation toggle"
    );
}

#[cfg(feature = "lang-markdown")]
#[test]
fn overriding_doc_globs_is_honoured() {
    // AC3: the globs are overridable. A project that only wants `guide/**` docs
    // gets exactly that — the default `docs/`/README scoping no longer applies.
    let reg = registry();
    let config = Config {
        documentation: crate::config::Documentation {
            enabled: true,
            include: vec!["guide/**/*.md".to_string()],
            exclude: Vec::new(),
            ..Default::default()
        },
        ..Config::default()
    };
    let docs = config.documentation.compile().unwrap().unwrap();

    assert!(admits_file(&reg, Some(&docs), None, None, "guide/intro.md"));
    assert!(
        !admits_file(&reg, Some(&docs), None, None, "docs/spec.md"),
        "the default docs/ glob is replaced, not merged"
    );
    assert!(!admits_file(&reg, Some(&docs), None, None, "README.md"));
}

/// A registry carrying a synthetic artifact plugin (S-062, CR-010): a YAML-style
/// data format claiming `.yaml`/`.yml` and an extensionless `Dockerfile`
/// basename. It reuses the Rust `LanguageFn` (admission is a pure claim lookup,
/// no parse), proving discovery admits a filename-claimed format from pure
/// descriptor data.
#[cfg(feature = "lang-rust")]
fn registry_with_artifact() -> LanguageRegistry {
    use crate::plugin::{grammars, AbiRange};
    const M: &str = r#"
        name = "artifactish"
        extensions = ["yaml", "yml"]
        module_separator = "/"
        abi_version = 15
        capabilities = []
        artifact = true
        filenames = ["Dockerfile"]
    "#;
    let mut entries = grammars::compiled();
    entries.push(grammars::GrammarEntry {
        manifest_label: "artifactish/plugin.toml",
        manifest_toml: M,
        language: tree_sitter_rust::LANGUAGE,
        embedded_queries: &[],
    });
    LanguageRegistry::load_from(&entries, AbiRange::runtime(), None, &mut |_| {})
        .expect("the synthetic artifact grammar loads")
}

#[cfg(feature = "lang-rust")]
#[test]
fn config_artifacts_are_admitted_by_claim_and_filtered_by_globs() {
    // S-062 discovery acceptance: extension-or-basename admission, the default
    // lock-file excludes, and the disabled-layer no-nodes rule.
    let reg = registry_with_artifact();
    let config = Config::default()
        .config_artifacts
        .compile()
        .expect("default config globs compile")
        .expect("the config layer is on by default");

    // A claimed-extension artifact is admitted, and routes via the *config* gate,
    // never the code gate (supported_extension excludes artifact plugins).
    assert!(is_config_admitted(
        &reg,
        Some(&config),
        "deploy/values.yaml"
    ));
    assert!(!supported_extension(&reg, None, "deploy/values.yaml"));
    assert!(admits_file(&reg, None, Some(&config), None, "deploy/values.yaml"));

    // An extensionless `Dockerfile` is admitted by its basename claim (FR-IX-02
    // as modified), and `Dockerfile.dev` via the `Name.*` prefix rule.
    assert!(is_config_admitted(&reg, Some(&config), "Dockerfile"));
    assert!(is_config_admitted(
        &reg,
        Some(&config),
        "svc/Dockerfile.dev"
    ));

    // Lock files are excluded by the default globs (BR-30), at root and nested.
    assert!(!is_config_admitted(
        &reg,
        Some(&config),
        "package-lock.json"
    ));
    assert!(!is_config_admitted(
        &reg,
        Some(&config),
        "web/package-lock.json"
    ));

    // A file no artifact plugin claims is never config-admitted.
    assert!(!is_config_admitted(&reg, Some(&config), "notes.txt"));
}

#[cfg(feature = "lang-rust")]
#[test]
fn disabling_the_config_layer_admits_no_artifact() {
    // AC: disabling the layer in config.toml produces no config nodes — modelled
    // as the admission gate refusing every artifact while code is unaffected.
    let reg = registry_with_artifact();
    let config = Config {
        config_artifacts: crate::config::ConfigArtifacts {
            enabled: false,
            ..Default::default()
        },
        ..Config::default()
    };
    let globs = config
        .config_artifacts
        .compile()
        .expect("compile is infallible when disabled");
    assert!(
        globs.is_none(),
        "disabled config layer compiles to no matcher"
    );

    assert!(!is_config_admitted(
        &reg,
        globs.as_ref(),
        "deploy/values.yaml"
    ));
    assert!(!admits_file(&reg, None, globs.as_ref(), None, "Dockerfile"));
    assert!(
        admits_file(&reg, None, globs.as_ref(), None, "src/lib.rs"),
        "code indexing is independent of the config-artifact toggle"
    );
}

#[test]
fn relativize_accepts_relative_and_root_absolute_paths() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().canonicalize().expect("canonical root");

    // A root-relative path passes through, normalised to forward slashes.
    assert_eq!(
        relativize(&root, Path::new("src/a.rs")).as_deref(),
        Some("src/a.rs")
    );
    // An absolute path *under* the root is stripped back to its relative key.
    let abs = root.join("src/b.rs");
    assert_eq!(relativize(&root, &abs).as_deref(), Some("src/b.rs"));
    // A leading `./` is a no-op component and must normalise to the same key the
    // index stored (`src/a.rs`), so a `./`-prefixed sync path is not mistaken for
    // a new file.
    assert_eq!(
        relativize(&root, Path::new("./src/a.rs")).as_deref(),
        Some("src/a.rs")
    );
}

#[test]
fn relativize_rejects_paths_escaping_the_root() {
    // NFR-SE-04: a crafted changed-file set must never reach outside the root.
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().canonicalize().expect("canonical root");

    assert_eq!(relativize(&root, Path::new("../secrets.rs")), None);
    assert_eq!(relativize(&root, Path::new("a/../../b.rs")), None);
    // An absolute path that is not under the root is rejected (strip fails).
    assert_eq!(relativize(&root, &PathBuf::from("/etc/passwd")), None);
    // The root itself has no relative file key.
    assert_eq!(relativize(&root, Path::new("")), None);
}

// ── CR-057 / S-226: chunked Pass-1 persistence ──────────────────────────────
//
// A full index persists file facts in bounded chunked write batches
// (`persist_facts_chunked`) rather than one transaction per file. These pin the
// three invariants the optimization must not break: byte-identical output
// regardless of chunk size ([NFR-RA-06]), wholesale per-chunk rollback on a
// mid-chunk fault ([NFR-RA-07]), and exactly one RW connection ([ADR-02]).
// Gated on `lang-rust` — they parse Rust fixtures through the embedded grammar.

/// Open a fresh runtime over a throwaway on-disk store. The `TempDir` guard is
/// returned so the caller keeps it alive for the runtime's lifetime.
#[cfg(feature = "lang-rust")]
fn open_runtime() -> (tempfile::TempDir, Runtime) {
    let dir = tempfile::tempdir().expect("tempdir");
    let rt = Runtime::open(dir.path().join("logos.db")).expect("runtime opens");
    (dir, rt)
}

/// A set of `n` cross-referencing Rust files: each `f{i}` calls the next file's
/// function (a cross-file ref) and a shared `hub` (an unresolved ref), so the
/// persisted graph carries nodes, intra-file `Contains` edges, and a populated
/// `unresolved_refs` ledger — a realistic multi-file fixture.
#[cfg(feature = "lang-rust")]
fn sample_inputs(n: usize) -> Vec<FileInput> {
    (0..n)
        .map(|i| {
            let next = (i + 1) % n;
            let src = format!("pub fn f{i}() {{ hub(); f{next}(); }}\n");
            FileInput::new(format!("f{i}.rs"), src)
        })
        .collect()
}

/// Extract `inputs` into owned facts through the embedded Rust grammar.
#[cfg(feature = "lang-rust")]
fn extract_sample(inputs: &[FileInput]) -> Vec<Facts> {
    let ctx = SymbolContext::default();
    extract_files(inputs, &registry(), &ctx)
}

/// `(path, blake3-hash)` pairs for `inputs`, exactly as `load_files` produces.
#[cfg(feature = "lang-rust")]
fn hash_pairs(inputs: &[FileInput]) -> Vec<(String, String)> {
    inputs
        .iter()
        .map(|inp| (inp.path.clone(), hash_source(&inp.source)))
        .collect()
}

/// A deterministic, order-independent fingerprint of the persisted graph —
/// every node (with rowid), edge, and reference-ledger row — so two
/// independently built stores compare byte-for-byte on content.
#[cfg(feature = "lang-rust")]
fn persisted_fingerprint(rt: &Runtime) -> String {
    rt.submit_read(|store| {
        let mut lines: Vec<String> = Vec::new();
        for nd in store.all_nodes()? {
            lines.push(format!(
                "N {}|{}|{:?}|{}|{}|{:?}|{:?}",
                nd.id.0,
                nd.symbol.as_str(),
                nd.kind,
                nd.name,
                nd.file_path.as_deref().unwrap_or(""),
                nd.start_line,
                nd.end_line,
            ));
        }
        for e in store.all_edges()? {
            lines.push(format!("E {}|{}|{:?}", e.source.0, e.target.0, e.kind));
        }
        for r in store.unresolved_refs()? {
            lines.push(format!(
                "R {}|{}|{:?}|{:?}|{}|{:?}",
                r.source_symbol, r.target, r.form, r.kind, r.resolved, r.payload
            ));
        }
        lines.sort();
        Ok(lines.join("\n"))
    })
    .expect("read runs")
}

#[cfg(feature = "lang-rust")]
#[test]
fn chunked_persist_is_byte_identical_to_the_per_file_baseline() {
    // NFR-RA-06 / FR-IX-08: the graph a full index writes must be identical
    // regardless of chunk size — `chunk_files == 1` IS the per-file-transaction
    // baseline. Persist the SAME extracted facts two ways and compare.
    let inputs = sample_inputs(10);
    let facts = extract_sample(&inputs);
    let hashes = hash_pairs(&inputs);
    let hash_by_rel: HashMap<&str, &str> =
        hashes.iter().map(|(p, h)| (p.as_str(), h.as_str())).collect();

    // Baseline: one transaction per file.
    let (_dir_a, rt_a) = open_runtime();
    let mut facts_a = facts.clone();
    let mut warn_a = Vec::new();
    let out_a =
        persist_facts_chunked(&rt_a, &mut facts_a, &hash_by_rel, false, 1, &mut warn_a).unwrap();

    // Chunked: 10 files at chunk 4 → three transactions (4 + 4 + 2), the last
    // partial — this also exercises the flush-the-remainder path.
    let (_dir_b, rt_b) = open_runtime();
    let mut facts_b = facts.clone();
    let mut warn_b = Vec::new();
    let out_b =
        persist_facts_chunked(&rt_b, &mut facts_b, &hash_by_rel, false, 4, &mut warn_b).unwrap();

    // Guard against a vacuous pass: the fixture must actually produce graph
    // content, else two empty stores would compare "equal" over nothing (e.g. an
    // embedded-grammar regression yielding empty facts). The `hub()`/`f{next}()`
    // calls are designed to populate the reference ledger too.
    assert!(
        out_a.nodes > 0 && out_a.edges > 0,
        "the fixture must persist a non-empty graph, got {} nodes / {} edges",
        out_a.nodes,
        out_a.edges
    );
    assert!(
        !persisted_fingerprint(&rt_a).is_empty(),
        "the baseline graph is non-empty"
    );

    assert_eq!(out_a.files, out_b.files, "same file count");
    assert_eq!(
        (out_a.nodes, out_a.edges),
        (out_b.nodes, out_b.edges),
        "same node/edge tally"
    );
    assert_eq!(
        persisted_fingerprint(&rt_a),
        persisted_fingerprint(&rt_b),
        "chunk size must not change the persisted graph (byte-identical, NFR-RA-06)"
    );
    assert!(
        warn_a.is_empty() && warn_b.is_empty(),
        "a clean fixture yields no warnings on either path"
    );
    // Both runs drained the facts they were handed (no per-file clone leftover).
    assert!(facts_a.is_empty() && facts_b.is_empty(), "facts are drained");
}

#[cfg(feature = "lang-rust")]
#[test]
fn a_mid_chunk_fault_rolls_the_whole_chunk_back() {
    // NFR-RA-07: a chunk is one transaction. If any file in the chunk fails, the
    // WHOLE chunk rolls back with no partial rows. Model exactly what
    // `persist_chunk` does — persist one file against the batch writer, then hit
    // a fault (a later file in the same chunk failing) before the batch commits —
    // and assert the store is left empty.
    let inputs = sample_inputs(3);
    let facts = extract_sample(&inputs);
    let hashes = hash_pairs(&inputs);

    let (_dir, rt) = open_runtime();

    let first = facts[0].clone();
    let first_hash = hashes[0].1.clone();
    let result: anyhow::Result<()> = rt.submit_write(move |w| {
        // The first file of the chunk persists successfully…
        persist_file(w, &first, &first_hash, false)?;
        // …then a later file in the SAME chunk fails, aborting the whole batch.
        anyhow::bail!("injected mid-chunk fault")
    });
    assert!(result.is_err(), "the faulted chunk surfaces an error");

    // Wholesale rollback: the first file's rows never landed — files, nodes,
    // edges, and ledger rows are ALL among what `persist_file` writes, so every
    // table must be empty (NFR-RA-07).
    let (files, nodes, edges, refs) = rt
        .submit_read(|s| {
            Ok((
                s.indexed_files()?.len(),
                s.all_nodes()?.len(),
                s.all_edges()?.len(),
                s.unresolved_refs()?.len(),
            ))
        })
        .expect("read runs");
    assert_eq!(files, 0, "no file row survives a rolled-back chunk (NFR-RA-07)");
    assert_eq!(nodes, 0, "no node survives a rolled-back chunk");
    assert_eq!(edges, 0, "no edge survives a rolled-back chunk");
    assert_eq!(refs, 0, "no ledger row survives a rolled-back chunk");

    // The writer survives the fault and still serves a subsequent healthy chunk —
    // the aborted batch did not poison the single writer.
    let mut facts_ok = facts.clone();
    let hash_by_rel: HashMap<&str, &str> =
        hashes.iter().map(|(p, h)| (p.as_str(), h.as_str())).collect();
    let mut warn = Vec::new();
    let out =
        persist_facts_chunked(&rt, &mut facts_ok, &hash_by_rel, false, 8, &mut warn).unwrap();
    assert_eq!(out.files, 3, "a healthy chunk commits cleanly after the faulted one");
}

#[cfg(feature = "lang-rust")]
#[test]
fn full_index_persist_uses_a_single_rw_connection() {
    // ADR-02: one dedicated thread owns the sole RW connection. `persist_chunk`
    // writes every chunk *exclusively* through `runtime.submit_write`, so proving
    // the runtime funnels ALL write batches onto one writer thread proves the
    // chunked persist opened no second RW connection.
    let (_dir, rt) = open_runtime();

    // A real multi-chunk persist runs to completion on this runtime (3 files at
    // chunk 2 → two chunks), so the chunked path is genuinely exercised here.
    let inputs = sample_inputs(3);
    let mut facts = extract_sample(&inputs);
    let hashes = hash_pairs(&inputs);
    let hash_by_rel: HashMap<&str, &str> =
        hashes.iter().map(|(p, h)| (p.as_str(), h.as_str())).collect();
    let mut warn = Vec::new();
    let out =
        persist_facts_chunked(&rt, &mut facts, &hash_by_rel, false, 2, &mut warn).unwrap();
    assert_eq!(out.files, 3, "the multi-chunk persist committed every file");

    // Every write batch — the chunks above and these probes — reports the same
    // writer thread id, and never the caller's: one dedicated RW connection.
    let tid = || {
        rt.submit_write(|_w| Ok(std::thread::current().id()))
            .expect("write runs")
    };
    let a = tid();
    let b = tid();
    let c = tid();
    assert_eq!(a, b, "every write batch runs on the one writer thread (ADR-02)");
    assert_eq!(b, c, "every write batch runs on the one writer thread (ADR-02)");
    assert_ne!(
        a,
        std::thread::current().id(),
        "writes never run on the caller's thread — one dedicated RW connection"
    );
}

#[cfg(feature = "lang-rust")]
#[test]
fn chunk_boundaries_persist_every_file_exactly_once() {
    // FR-IX-08 / NFR-RA-06: the chunk loop must persist every file exactly once
    // at every size boundary — an empty input, an exact multiple of the chunk
    // size (no spurious trailing empty chunk), and a clamped-zero size.

    // Empty input → a clean, zeroed no-op: the drain loop never runs and the
    // final-flush guard (`if !chunk.is_empty()`) must not fire.
    let (_dir_e, rt_e) = open_runtime();
    let mut none: Vec<Facts> = Vec::new();
    let mut warn_e = Vec::new();
    let out_e =
        persist_facts_chunked(&rt_e, &mut none, &HashMap::new(), false, 4, &mut warn_e).unwrap();
    assert_eq!(
        (out_e.files, out_e.nodes, out_e.edges),
        (0, 0, 0),
        "empty input persists nothing"
    );

    // Build one fixture used by both remaining cases.
    let inputs = sample_inputs(8);
    let facts = extract_sample(&inputs);
    let hashes = hash_pairs(&inputs);
    let hash_by_rel: HashMap<&str, &str> =
        hashes.iter().map(|(p, h)| (p.as_str(), h.as_str())).collect();

    // The per-file baseline (chunk 1) both other cases must match.
    let (_dir_1, rt_1) = open_runtime();
    let mut f1 = facts.clone();
    let mut w1 = Vec::new();
    let o1 = persist_facts_chunked(&rt_1, &mut f1, &hash_by_rel, false, 1, &mut w1).unwrap();
    assert_eq!(o1.files, 8);

    // Exact multiple (8 files @ chunk 4 → two full chunks, no partial): the seam
    // must not emit a spurious empty trailing chunk — output stays byte-identical.
    let (_dir_m, rt_m) = open_runtime();
    let mut fm = facts.clone();
    let mut wm = Vec::new();
    let om = persist_facts_chunked(&rt_m, &mut fm, &hash_by_rel, false, 4, &mut wm).unwrap();
    assert_eq!(
        (o1.files, o1.nodes, o1.edges),
        (om.files, om.nodes, om.edges),
        "exact-multiple chunking persists the same tally as the per-file baseline"
    );
    assert_eq!(
        persisted_fingerprint(&rt_1),
        persisted_fingerprint(&rt_m),
        "exact-multiple chunking is byte-identical to the per-file baseline (NFR-RA-06)"
    );

    // A zero chunk size is clamped to 1 (never an infinite non-flushing loop) and
    // still persists every file identically to the baseline.
    let (_dir_z, rt_z) = open_runtime();
    let mut fz = facts.clone();
    let mut wz = Vec::new();
    let oz = persist_facts_chunked(&rt_z, &mut fz, &hash_by_rel, false, 0, &mut wz).unwrap();
    assert_eq!(oz.files, 8, "a clamped-to-1 chunk size still persists every file");
    assert_eq!(
        persisted_fingerprint(&rt_z),
        persisted_fingerprint(&rt_1),
        "clamped chunking matches the per-file baseline"
    );
}

// ── Parallel file load (S-228, FR-IX-09) ────────────────────────────────────

/// Materialise a candidate on disk under `root` and return it. `contents` is raw
/// bytes so a test can plant a non-UTF-8 file that `load_files` must skip.
fn candidate(root: &Path, rel: &str, contents: &[u8]) -> Candidate {
    let abs = root.join(rel);
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&abs, contents).unwrap();
    Candidate {
        abs,
        rel: rel.to_string(),
    }
}

/// A runtime whose shared worker pool has exactly `threads` workers, over a
/// throwaway db. The temp guard is returned so the caller keeps it alive.
fn runtime_with_workers(threads: usize) -> (tempfile::TempDir, Runtime) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let db = tmp.path().join("logos.db");
    let runtime = Runtime::open_with_config(
        &db,
        RuntimeConfig {
            reader_pool_size: 1,
            worker_threads: threads,
            write_queue_capacity: 8,
        },
    )
    .expect("runtime opens");
    (tmp, runtime)
}

/// The comparable projection of a loaded file: `(rel, source, hash)`.
fn load_tuples(loaded: &[LoadedFile]) -> Vec<(String, String, String)> {
    loaded
        .iter()
        .map(|l| (l.rel.clone(), l.source.clone(), l.hash.clone()))
        .collect()
}

/// Run `load_files` on a `threads`-worker pool over `candidates`.
fn run_load(threads: usize, candidates: &[Candidate]) -> (Vec<LoadedFile>, Vec<String>, Vec<String>) {
    let (_tmp, runtime) = runtime_with_workers(threads);
    let mut warnings = Vec::new();
    let mut files_failed = Vec::new();
    let loaded = load_files(&runtime, candidates, &mut warnings, &mut files_failed);
    (loaded, warnings, files_failed)
}

#[test]
fn load_files_output_is_identical_across_worker_counts() {
    // FR-IX-09 / NFR-RA-06: the parallel read+hash yields the same loaded set,
    // the same per-file hashes, and the same warnings / files_failed order for
    // every worker count. A non-UTF-8 file sits *between* readable ones so the
    // ordering of both the loaded vector and the failure accumulators is under
    // test, not just their contents.
    let tmp = tempfile::tempdir().expect("fixture dir");
    let root = tmp.path();
    let mut candidates = Vec::new();
    for i in 0..24 {
        candidates.push(candidate(
            root,
            &format!("src/f{i:02}.rs"),
            format!("pub fn f{i}() {{}}\n").as_bytes(),
        ));
        // Two non-UTF-8 files at *non-adjacent* positions (after f08 and f17) so
        // the ordering of `files_failed` / `warnings` is genuinely under test: a
        // thread-dependent reorder of a multi-element accumulator would change the
        // vector, which a single planted failure could never reveal.
        if i == 8 {
            candidates.push(candidate(root, "src/bad_a.rs", &[0xff, 0xfe, 0x00, 0x9c]));
        }
        if i == 17 {
            candidates.push(candidate(root, "src/bad_b.rs", &[0x00, 0xc0, 0xff, 0xee]));
        }
    }

    let (base_loaded, base_warnings, base_failed) = run_load(1, &candidates);
    // 24 readable files load; the two non-UTF-8 files fail, in candidate order.
    assert_eq!(base_loaded.len(), 24, "every readable candidate is loaded");
    assert_eq!(
        base_failed,
        vec!["src/bad_a.rs".to_string(), "src/bad_b.rs".to_string()],
        "both bad files are recorded as failed, in candidate order"
    );
    assert_eq!(base_warnings.len(), 2, "exactly one skip warning per bad file");
    assert!(base_warnings[0].starts_with("src/bad_a.rs:"), "warnings preserve candidate order");
    assert!(base_warnings[1].starts_with("src/bad_b.rs:"), "warnings preserve candidate order");
    // Loaded order follows candidate order (the bad files are simply absent).
    assert_eq!(base_loaded[0].rel, "src/f00.rs");
    assert_eq!(base_loaded[8].rel, "src/f08.rs");
    assert_eq!(base_loaded[9].rel, "src/f09.rs", "the failed file leaves no gap or reorder");
    // The stored hash is exactly blake3 of the source (FR-SY-03).
    assert_eq!(base_loaded[0].hash, hash_source(&base_loaded[0].source));

    let base_tuples = load_tuples(&base_loaded);
    for threads in [2usize, 4, 8] {
        let (loaded, warnings, failed) = run_load(threads, &candidates);
        assert_eq!(
            load_tuples(&loaded),
            base_tuples,
            "worker count {threads} changed the loaded set/hashes/order (NFR-RA-06)"
        );
        assert_eq!(warnings, base_warnings, "worker count {threads} reordered warnings");
        assert_eq!(failed, base_failed, "worker count {threads} reordered files_failed");
    }
}

#[test]
fn parallel_load_is_stable_under_repeated_multithreaded_runs() {
    // FR-IX-09 stress: a data race in the parallel read+hash would surface as a
    // flaky loaded set or hash. Re-run the multi-threaded load repeatedly and
    // assert every run matches the first (repeated-run stability idiom).
    let tmp = tempfile::tempdir().expect("fixture dir");
    let root = tmp.path();
    let candidates: Vec<Candidate> = (0..48)
        .map(|i| {
            candidate(
                root,
                &format!("d{}/f{i}.rs", i % 6),
                format!("pub fn f{i}() {{ let _ = {i}; }}\n").as_bytes(),
            )
        })
        .collect();

    let (first_loaded, _, _) = run_load(8, &candidates);
    let first = load_tuples(&first_loaded);
    assert_eq!(first.len(), 48, "all files load");
    for run in 0..40 {
        let (loaded, _, _) = run_load(8, &candidates);
        assert_eq!(load_tuples(&loaded), first, "run {run} diverged under 8 workers (data race?)");
    }
}

#[test]
fn load_files_on_empty_candidate_set_is_empty_for_any_worker_count() {
    // The zero-item edge case: `par_iter().collect()` over an empty slice must
    // return an empty result — with no panic and no accumulator entries — on both
    // a single-worker and a multi-worker pool.
    for threads in [1usize, 4] {
        let (loaded, warnings, failed) = run_load(threads, &[]);
        assert!(loaded.is_empty(), "no files loaded from an empty candidate set ({threads}w)");
        assert!(warnings.is_empty(), "no warnings from an empty candidate set ({threads}w)");
        assert!(failed.is_empty(), "no failures from an empty candidate set ({threads}w)");
    }
}
