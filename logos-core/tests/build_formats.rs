//! Integration tests for the build-format artifact plugins (S-064, [CR-010],
//! [ADR-25]): Dockerfile, Makefile, and Shell.
//!
//! These exercise the plugins end-to-end through the public substrate — the
//! `LanguageRegistry` basename/extension claim seam, the artifact routing arm
//! into `extract::config`, and the `[config] node_kind` typed-anchor mechanism —
//! against the *real* grammars (`arborium-dockerfile`, `tree-sitter-make`,
//! `tree-sitter-bash`), proving the FR-CG-06 preflight result holds through the
//! pipeline.
//!
//! Coverage by acceptance criterion:
//! - `Dockerfile`/`Dockerfile.dev`/`Makefile`/`GNUmakefile` are admitted via
//!   basename claims (FR-CG-01); a multi-stage build yields one `DockerfileStage`
//!   per stage and a Makefile yields `MakeTarget` per rule (FR-CG-03);
//! - a repo full of shell scripts moves no metric — the aggregate signal is
//!   byte-identical with the scripts present or deleted (FR-CG-05), Shell being
//!   the layer's deliberate metric-neutral guard;
//! - all three grammars appear in `logos languages` with their claims (FR-CG-04).

use logos_core::extract::{extract, FileInput, SymbolContext};
use logos_core::model::NodeKind;
use logos_core::plugin::LanguageRegistry;

/// The names of every node of `kind` in `facts`, in canonical (sorted) order.
fn names_of(facts: &logos_core::extract::Facts, kind: NodeKind) -> Vec<&str> {
    facts
        .nodes
        .iter()
        .filter(|n| n.kind == kind)
        .map(|n| n.name.as_str())
        .collect()
}

// ── FR-CG-04: the three grammars load and are listed by `logos languages` ────

#[cfg(all(
    feature = "lang-dockerfile",
    feature = "lang-make",
    feature = "lang-shell"
))]
#[test]
fn build_grammars_load_and_are_listed_with_their_claims() {
    let tmp = tempfile::tempdir().unwrap();
    let info = logos_core::Engine::open(tmp.path()).languages();
    let find = |name: &str| {
        info.languages
            .iter()
            .find(|d| d.name == name)
            .unwrap_or_else(|| panic!("`logos languages` must list {name}: {:?}", info.languages))
    };

    let df = find("dockerfile");
    assert!(df.artifact, "dockerfile is an artifact plugin");
    assert!(df.extensions.iter().any(|e| e == "dockerfile"));
    assert!(
        df.filenames.iter().any(|f| f == "Dockerfile"),
        "dockerfile claims the Dockerfile basename: {:?}",
        df.filenames
    );
    assert_eq!(df.abi_version, 15);

    let mk = find("makefile");
    assert!(mk.artifact);
    assert_eq!(
        mk.filenames,
        ["Makefile", "makefile", "GNUmakefile"],
        "makefile claims its three basenames"
    );
    assert_eq!(mk.abi_version, 14, "tree-sitter-make is ABI 14");

    let sh = find("shell");
    assert!(sh.artifact);
    assert!(sh.extensions.iter().any(|e| e == "sh") && sh.extensions.iter().any(|e| e == "bash"));
    assert!(sh.filenames.is_empty(), "shell is extension-claimed only");
    assert_eq!(sh.abi_version, 15);
}

// ── FR-CG-01 / FR-CG-03: Dockerfile basename claims + one stage per `FROM` ───

#[cfg(feature = "lang-dockerfile")]
#[test]
fn dockerfile_basenames_admitted_and_multistage_yields_one_stage_each() {
    let tmp = tempfile::tempdir().unwrap();
    let reg = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");

    // Basename + `Name.*` prefix + extension claims (FR-CG-01).
    for path in [
        "Dockerfile",
        "Dockerfile.dev",
        "Dockerfile.prod",
        "app.dockerfile",
    ] {
        let plugin = reg
            .for_path(path)
            .unwrap_or_else(|| panic!("{path} must be claimed by the dockerfile plugin"));
        assert_eq!(plugin.name(), "dockerfile", "{path} → dockerfile plugin");
    }

    let plugin = reg.for_path("Dockerfile").unwrap();
    let src = "\
FROM rust:1 AS builder
RUN cargo build --release
FROM alpine:3
COPY --from=builder /app /app
";
    let facts = extract(
        &FileInput::new("Dockerfile", src),
        plugin,
        &SymbolContext::default(),
    );

    // One ConfigFile root, one DockerfileStage per `FROM` (FR-CG-03).
    assert_eq!(names_of(&facts, NodeKind::ConfigFile).len(), 1);
    let stages = names_of(&facts, NodeKind::DockerfileStage);
    assert_eq!(
        stages.len(),
        2,
        "one DockerfileStage per build stage: {stages:?}"
    );
    assert!(
        stages.contains(&"builder"),
        "the named stage is anchored by its `AS` alias: {stages:?}"
    );
    // The unnamed final stage (`FROM alpine:3`, no `AS`) exercises the
    // first-source-line fallback: its name is derived, non-empty, never
    // fabricated (NFR-RA-05) — and distinct from the aliased stage.
    assert!(
        stages
            .iter()
            .any(|s| *s != "builder" && !s.is_empty() && s.contains("alpine")),
        "the unnamed stage gets a derived fallback name from its FROM line: {stages:?}"
    );
    // No generic ConfigSection — the typed node_kind override replaces it.
    assert!(
        names_of(&facts, NodeKind::ConfigSection).is_empty(),
        "build formats emit typed anchors, never generic ConfigSection"
    );
}

// ── FR-CG-01 / FR-CG-03: Makefile basename claims + one MakeTarget per rule ──

#[cfg(feature = "lang-make")]
#[test]
fn makefile_basenames_admitted_and_rules_yield_make_targets() {
    let tmp = tempfile::tempdir().unwrap();
    let reg = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");

    for path in ["Makefile", "makefile", "GNUmakefile", "common.mk"] {
        let plugin = reg
            .for_path(path)
            .unwrap_or_else(|| panic!("{path} must be claimed by the makefile plugin"));
        assert_eq!(plugin.name(), "makefile", "{path} → makefile plugin");
    }

    let plugin = reg.for_path("Makefile").unwrap();
    let src = "build: deps\n\tcargo build\n\ntest:\n\tcargo test\n\n.PHONY: build test\n";
    let facts = extract(
        &FileInput::new("Makefile", src),
        plugin,
        &SymbolContext::default(),
    );

    assert_eq!(names_of(&facts, NodeKind::ConfigFile).len(), 1);
    let targets = names_of(&facts, NodeKind::MakeTarget);
    assert_eq!(targets.len(), 3, "one MakeTarget per rule: {targets:?}");
    let joined = targets.join(" | ");
    assert!(
        joined.contains("build"),
        "the build target is anchored: {targets:?}"
    );
    assert!(
        joined.contains("test"),
        "the test target is anchored: {targets:?}"
    );
    // Special targets are rules too: `.PHONY` is emitted as a MakeTarget by
    // design (one anchor per `rule`), locked here so the intent is explicit.
    assert!(
        targets.iter().any(|t| t.starts_with(".PHONY")),
        "the .PHONY special target is anchored as a MakeTarget: {targets:?}"
    );
    assert!(
        names_of(&facts, NodeKind::ConfigSection).is_empty(),
        "make targets are typed anchors, never generic ConfigSection"
    );
}

// ── FR-CG-03: Shell function definitions yield ShellFunction anchors ─────────

#[cfg(feature = "lang-shell")]
#[test]
fn shell_functions_yield_shell_function_anchors() {
    let tmp = tempfile::tempdir().unwrap();
    let reg = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");
    let plugin = reg.for_extension("sh").expect("shell claims .sh");
    assert_eq!(plugin.name(), "shell");
    assert!(
        reg.for_extension("bash").is_some(),
        "shell also claims .bash"
    );

    // Both `name() { … }` and `function name { … }` forms are anchored.
    let src =
        "#!/usr/bin/env bash\ngreet() {\n  echo hello\n}\nfunction farewell {\n  echo bye\n}\n";
    let facts = extract(
        &FileInput::new("scripts/util.sh", src),
        plugin,
        &SymbolContext::default(),
    );

    let mut funcs = names_of(&facts, NodeKind::ShellFunction);
    funcs.sort_unstable();
    assert_eq!(
        funcs,
        ["farewell", "greet"],
        "one ShellFunction per definition"
    );
    assert!(
        facts.nodes.iter().all(|n| n.kind.is_non_code()),
        "every shell node is non-code — the metric-neutral guard (FR-CG-05)"
    );
}

// ── FR-CG-05: a repo full of shell scripts moves no metric ───────────────────
// Gated on lang-rust (a real code signal to perturb) AND lang-shell (so the
// scripts are actually ingested — otherwise the assertion would be vacuous).
#[cfg(all(feature = "lang-rust", feature = "lang-shell"))]
mod neutrality {
    use std::fs;
    use std::path::Path;

    use logos_core::model::NodeKind;
    use logos_core::{Engine, Runtime};
    use tempfile::TempDir;

    fn write(root: &Path, rel: &str, contents: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn shell_function_count(rt: &Runtime) -> usize {
        rt.submit_read(|store| store.all_nodes())
            .expect("read runs")
            .into_iter()
            .filter(|n| n.kind == NodeKind::ShellFunction)
            .count()
    }

    /// The headline acceptance criterion: scan a real code repo, add a pile of
    /// shell scripts, re-scan — the aggregate signal and every normalized metric
    /// are byte-identical (the scripts are `is_non_code`, excluded from the code
    /// subgraph by hydration); delete them and the signal is unchanged again.
    #[test]
    fn shell_scripts_leave_the_signal_byte_identical_present_or_deleted() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "src/api.rs",
            "pub fn entry() -> u32 {\n    core::compute() + helper()\n}\npub fn helper() -> u32 {\n    7\n}\n",
        );
        write(
            tmp.path(),
            "src/core.rs",
            "pub fn compute() -> u32 {\n    leaf() + 1\n}\nfn leaf() -> u32 {\n    41\n}\n",
        );

        let engine = Engine::start(tmp.path()).expect("engine starts");
        engine.index();
        let rt = engine.runtime().unwrap();

        // Measurement 1 — code only.
        let base = engine.scan(true).expect("scan runs").metrics;
        assert!(
            !base.empty && base.aggregate_signal.is_some(),
            "the code fixture yields a real signal"
        );
        assert_eq!(shell_function_count(rt), 0, "no shell ingested yet");

        // Add a repo full of shell scripts, each with functions of varying shape.
        for i in 0..6 {
            write(
                tmp.path(),
                &format!("scripts/task{i}.sh"),
                &format!(
                    "#!/bin/bash\nrun_{i}() {{\n  echo {i}\n  helper_{i}\n}}\nfunction helper_{i} {{\n  true\n}}\n"
                ),
            );
        }
        write(
            tmp.path(),
            "deploy.bash",
            "#!/usr/bin/env bash\ndeploy() {\n  echo deploying\n}\n",
        );

        let with_shell = engine.scan(true).expect("scan runs").metrics;
        assert!(
            shell_function_count(rt) > 0,
            "shell functions were ingested — the invariance assertion is meaningful"
        );
        assert_signal_byte_identical(&base, &with_shell);

        // Delete every shell script and re-scan: the signal returns to baseline.
        for i in 0..6 {
            fs::remove_file(tmp.path().join(format!("scripts/task{i}.sh"))).unwrap();
        }
        fs::remove_file(tmp.path().join("deploy.bash")).unwrap();
        let removed = engine.scan(true).expect("scan runs").metrics;
        assert_eq!(shell_function_count(rt), 0, "shell is gone from the graph");
        assert_signal_byte_identical(&base, &removed);
    }

    /// Byte-identical across the aggregate signal, all five normalized metrics,
    /// AND the production counts. The counts matter: they are the *code-subgraph*
    /// node/edge/function counts (post-hydration, non-code excluded), so a shell
    /// `ShellFunction` leaking into the code graph would inflate `function_count`
    /// here even if the signal happened to round identically — this catches it.
    fn assert_signal_byte_identical(
        a: &logos_core::models::quality::MetricSnapshot,
        b: &logos_core::models::quality::MetricSnapshot,
    ) {
        assert_eq!(
            a.aggregate_signal, b.aggregate_signal,
            "the aggregate signal must be byte-identical with shell present vs absent (FR-CG-05)"
        );
        for (name, x, y) in [
            ("modularity", &a.modularity, &b.modularity),
            ("acyclicity", &a.acyclicity, &b.acyclicity),
            ("depth", &a.depth, &b.depth),
            ("equality", &a.equality, &b.equality),
            ("redundancy", &a.redundancy, &b.redundancy),
        ] {
            assert_eq!(x.raw.to_bits(), y.raw.to_bits(), "{name} raw drifted");
            assert_eq!(
                x.normalized.to_bits(),
                y.normalized.to_bits(),
                "{name} normalized drifted"
            );
        }
        // The code subgraph itself must be unchanged — no shell node entered it.
        assert_eq!(a.node_count, b.node_count, "code node_count drifted");
        assert_eq!(a.edge_count, b.edge_count, "code edge_count drifted");
        assert_eq!(
            a.function_count, b.function_count,
            "code function_count drifted — a ShellFunction leaked into the code graph"
        );
    }
}
