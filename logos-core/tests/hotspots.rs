//! Integration tests for the temporal-tier surface (S-048, CR-006), exercised
//! end-to-end through the [`Engine`] façade over **real** git fixtures: the
//! hotspot ranking ([FR-GH-06], [UAT-GH-03]), the gate-immunity two-tier rule
//! ([FR-GH-07], [UAT-GH-02], [BR-26]), the tier-labeled scan detail
//! ([FR-GH-07]), and the degraded modes ([FR-GH-08], [UAT-GH-04]).
//!
//! The pure ranking logic is unit-tested inside `history::hotspot`; these prove
//! the cross-store join (`history.db` churn × `logos.db` complexity) and the
//! surface contracts against an actual `git` subprocess + a real index.

#![cfg(feature = "lang-rust")]

use std::path::Path;
use std::process::Command;

use logos_core::Engine;
use tempfile::TempDir;

// ── git fixture helpers (mirroring history::tests conventions) ───────────────

/// Run a git command in `cwd` with a fixed identity, panicking on failure.
fn sh_git(cwd: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["-c", "user.email=dev@logos", "-c", "user.name=Logos Dev"])
        .args(args)
        .output()
        .expect("git is on PATH");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Commit `rel` with `contents` as a single message.
fn commit(cwd: &Path, rel: &str, contents: &str, msg: &str) {
    let path = cwd.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
    sh_git(cwd, &["add", rel]);
    sh_git(cwd, &["commit", "-q", "-m", msg]);
}

/// A high-cyclomatic-complexity Rust function body (many decision points → a
/// large per-function CC, summed into the file's structural-complexity axis).
fn complex_source(name: &str) -> String {
    format!(
        "pub fn {name}(x: i64) -> i64 {{\n\
         {body}\
         \n    x\n}}\n",
        body = (0..8)
            .map(|i| format!("    if x == {i} {{ return {i}; }}\n"))
            .collect::<String>(),
    )
}

/// A trivial (CC≈1) Rust function.
fn calm_source(name: &str) -> String {
    format!("pub fn {name}() -> i64 {{ 0 }}\n")
}

/// Build an indexed git fixture engineering four files across the churn ×
/// complexity quadrants. `hot.rs` is top on **both** axes, so it must rank #1
/// ([UAT-GH-03]). Returns the temp guard (kept alive for `.logos/`).
fn engineered_repo() -> TempDir {
    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);

    // calm.rs: low churn (1 commit), low complexity.
    commit(repo, "src/calm.rs", &calm_source("calm"), "add calm");
    // complex.rs: low churn (1 commit), high complexity.
    commit(
        repo,
        "src/complex.rs",
        &complex_source("complex"),
        "add complex",
    );
    // churny.rs: high churn (4 commits), low complexity.
    for n in 0..4 {
        commit(
            repo,
            "src/churny.rs",
            &format!("pub fn churny() -> i64 {{ {n} }}\n"),
            &format!("churny v{n}"),
        );
    }
    // hot.rs: high churn (5 commits) AND high complexity — the engineered #1.
    // One commit message matches the default defect_patterns (`\bfix\b`), so
    // hot.rs carries a non-zero defect-heuristic count for the scan-detail and
    // hotspot assertions ([FR-GH-05]).
    for n in 0..5 {
        let msg = if n == 2 {
            "fix: crash in hot".to_string()
        } else {
            format!("hot v{n}")
        };
        commit(
            repo,
            "src/hot.rs",
            &format!("{}// rev {n}\n", complex_source("hot")),
            &msg,
        );
    }

    let engine = Engine::start(repo).expect("engine starts");
    engine.index();
    tmp
}

// ── FR-GH-06 / UAT-GH-03: hotspot ranking, limits, deterministic order ───────

#[test]
fn engineered_file_ranks_first_with_deterministic_order_and_heuristic_label() {
    let tmp = engineered_repo();
    let engine = Engine::start(tmp.path()).expect("engine starts");

    let report = engine.hotspots(None, false, false).expect("hotspots runs");
    assert!(report.degraded.is_none(), "a real git repo is not degraded");
    assert_eq!(
        report.files.first().map(|h| h.path.as_str()),
        Some("src/hot.rs"),
        "the high-churn + high-complexity file ranks first ({:?})",
        report.files
    );
    // The mandatory heuristic label rides the report (FR-GH-05, NFR-CC-04).
    assert_eq!(report.defect_label, "heuristic");
    assert!(report.tier.contains("non-gated"), "tier label is explicit");

    // Determinism: a second evaluation at the same HEAD + config is byte-for-byte
    // identical (NFR-RA-06). (first_mine is now false → no notice drift.)
    let again = engine.hotspots(None, false, false).expect("hotspots reruns");
    assert_eq!(
        serde_json::to_string(&report.files).unwrap(),
        serde_json::to_string(&again.files).unwrap(),
        "ranking is byte-identical across runs"
    );
}

#[test]
fn limit_truncates_the_ranked_board() {
    let tmp = engineered_repo();
    let engine = Engine::start(tmp.path()).expect("engine starts");

    let full = engine.hotspots(None, false, false).expect("hotspots runs");
    assert!(full.ranked_files >= 2, "fixture has several ranked files");

    let limited = engine
        .hotspots(Some(1), false, false)
        .expect("hotspots --limit 1 runs");
    assert_eq!(limited.files.len(), 1, "--limit caps the returned board");
    assert_eq!(
        limited.ranked_files, full.ranked_files,
        "total is preserved"
    );
    assert_eq!(
        limited.files[0].path, full.files[0].path,
        "the top file is stable under --limit"
    );
}

// ── FR-GH-06 / CR-076: optional production-scope filter (is_test excluded) ──

/// A fixture engineering three files for the production-scope filter
/// ([CR-076](../../docs/requests/CR-076-hotspots-production-scope-filter.md)):
/// `src/tests.rs` is a whole test file (the bare-name plural convention,
/// [S-283](../../docs/planning/journal.md#s-283-recognize-plural-rust-test-file-conventions-in-is_test-path-detection))
/// engineered to top on both axes — it must lead the board when the filter is
/// off and disappear when it is on. `src/prod.rs` is a production file
/// carrying an in-file `#[cfg(test)] mod tests` (its helper is `is_test`, its
/// own `prod` function is not) — the mixed-file AC. `src/calm.rs` is low on
/// both axes.
fn production_scope_repo() -> TempDir {
    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);

    // A branchy function with `ifs` decision points (drives per-function CC).
    let branchy = |name: &str, ifs: usize| -> String {
        let body: String = (0..ifs)
            .map(|i| format!("    if x == {i} {{ return {i}; }}\n"))
            .collect();
        format!("pub fn {name}(x: i64) -> i64 {{\n{body}    x\n}}\n")
    };

    // src/tests.rs: top on both axes (5 commits, 12 branches), but a whole
    // test file by path.
    for n in 0..5 {
        commit(
            repo,
            "src/tests.rs",
            &format!("{}// rev {n}\n", branchy("helper", 12)),
            &format!("tests v{n}"),
        );
    }
    // src/prod.rs: a production function (4 commits, 6 branches — strictly
    // below tests.rs on both axes, even after the trivial `checks_prod` helper
    // adds +1 complexity) plus an in-file `#[cfg(test)] mod tests` whose
    // helper is `is_test` — the file must stay on the board.
    for n in 0..4 {
        let src = format!(
            "{prod}\n#[cfg(test)]\nmod tests {{\n    #[test]\n    fn checks_prod() {{ super::prod(0); }}\n}}\n// rev {n}\n",
            prod = branchy("prod", 6),
        );
        commit(repo, "src/prod.rs", &src, &format!("prod v{n}"));
    }
    // src/calm.rs: low churn, low complexity.
    commit(repo, "src/calm.rs", &calm_source("calm"), "add calm");

    let engine = Engine::start(repo).expect("engine starts");
    engine.index();
    tmp
}

#[test]
fn production_scope_filter_excludes_whole_test_files_and_keeps_mixed_production_files() {
    let tmp = production_scope_repo();
    let engine = Engine::start(tmp.path()).expect("engine starts");

    // Disabled (default): byte-identical to today's whole-repo board —
    // tests.rs leads on both axes.
    let off = engine.hotspots(None, false, false).expect("hotspots runs");
    assert!(!off.production_scope);
    assert_eq!(
        off.files.first().map(|h| h.path.as_str()),
        Some("src/tests.rs"),
        "filter off: the whole-repo board is unchanged ({:?})",
        off.files
    );

    // Enabled: src/tests.rs is gone; src/prod.rs (mixed file — its production
    // function keeps it) surfaces in its place.
    let on = engine
        .hotspots(None, false, true)
        .expect("hotspots --production-scope");
    assert!(on.production_scope);
    let paths: Vec<&str> = on.files.iter().map(|h| h.path.as_str()).collect();
    assert!(
        !paths.contains(&"src/tests.rs"),
        "the whole test file is excluded from the candidate set: {paths:?}"
    );
    assert!(
        paths.contains(&"src/prod.rs"),
        "a production file with an in-file #[cfg(test)] mod tests stays on the board: {paths:?}"
    );
    assert_eq!(
        on.files.first().map(|h| h.path.as_str()),
        Some("src/prod.rs"),
        "prod.rs leads the narrowed board: {:?}",
        on.files
    );

    // A re-run is byte-identical (NFR-RA-06).
    let again = engine
        .hotspots(None, false, true)
        .expect("hotspots reruns");
    assert_eq!(
        serde_json::to_string(&on.files).unwrap(),
        serde_json::to_string(&again.files).unwrap(),
        "production-scoped ranking is byte-identical across runs"
    );
}

#[test]
fn production_scope_composes_with_limit_and_untested() {
    let tmp = production_scope_repo();
    let engine = Engine::start(tmp.path()).expect("engine starts");

    let limited = engine
        .hotspots(Some(1), false, true)
        .expect("hotspots --production-scope --limit 1");
    assert_eq!(
        limited.files.len(),
        1,
        "--limit composes with production_scope"
    );
    assert_eq!(limited.files[0].path, "src/prod.rs");

    // No coverage ingested → `--untested` retains every remaining (n/a) file,
    // so production_scope narrows the set the untested filter then retains.
    let untested = engine
        .hotspots(None, true, true)
        .expect("hotspots --untested --production-scope");
    let paths: Vec<&str> = untested.files.iter().map(|h| h.path.as_str()).collect();
    assert!(
        !paths.contains(&"src/tests.rs"),
        "the whole test file stays excluded under --untested: {paths:?}"
    );
    assert!(
        paths.contains(&"src/prod.rs"),
        "the mixed production file remains under --untested: {paths:?}"
    );
}

/// All three filters composed TOGETHER in a single call — `--limit`,
/// `--untested`, and `production_scope` — not just each independently
/// (CR-076 AC: "composes with --limit and --untested"). No coverage is
/// ingested, so `--untested` retains every remaining (n/a) file; the
/// narrowed-then-limited board must still lead with the hottest production
/// file.
#[test]
fn production_scope_composes_with_limit_and_untested_simultaneously() {
    let tmp = production_scope_repo();
    let engine = Engine::start(tmp.path()).expect("engine starts");

    let combined = engine
        .hotspots(Some(1), true, true)
        .expect("hotspots --untested --production-scope --limit 1");
    assert!(combined.production_scope);
    assert!(combined.untested);
    assert_eq!(
        combined.ranked_files, 2,
        "production_scope narrows to {{prod.rs, calm.rs}}; --untested retains both \
         (no coverage ingested): {combined:?}"
    );
    assert_eq!(combined.files.len(), 1, "--limit caps the combined board");
    assert_eq!(
        combined.files[0].path, "src/prod.rs",
        "the hottest production file leads under all three filters combined"
    );
}

/// CR-076 AC: "enabling or disabling the filter never changes any gated
/// signal" ([BR-26]). Mirrors [`gate_is_byte_identical_as_the_temporal_tier_advances`]
/// for the new `production_scope` parameter specifically.
#[test]
fn production_scope_toggle_never_moves_the_gate() {
    let tmp = production_scope_repo();
    let repo = tmp.path();
    let engine = Engine::start(repo).expect("engine starts");

    engine.gate(None, true, true).expect("gate --save");
    let baseline = gated_verdict(&engine.gate(None, false, true).expect("gate"));

    engine
        .hotspots(None, false, false)
        .expect("hotspots with the filter off");
    let after_off = gated_verdict(&engine.gate(None, false, true).expect("gate"));
    assert_eq!(
        baseline, after_off,
        "production_scope=false never moves the gate"
    );

    engine
        .hotspots(None, false, true)
        .expect("hotspots with the filter on");
    let after_on = gated_verdict(&engine.gate(None, false, true).expect("gate"));
    assert_eq!(
        baseline, after_on,
        "production_scope=true never moves the gate (BR-26)"
    );
}

// ── FR-GH-07 / UAT-GH-02 / BR-26: gate immunity — the two-tier rule ──────────

/// The gated verdict, stripped of provenance (freshness HEAD/sha, the snapshot
/// id in `message`) that legitimately tracks the commit — what BR-26 pins as a
/// pure function of tree + config.
fn gated_verdict(g: &logos_core::models::quality::GateResult) -> String {
    serde_json::to_string(&serde_json::json!({
        "passed": g.passed,
        "signal": g.signal,
        "baseline_signal": g.baseline_signal,
        "regressions": g.regressions,
        "test_function_count": g.test_function_count,
    }))
    .unwrap()
}

#[test]
fn gate_is_byte_identical_as_the_temporal_tier_advances() {
    let tmp = engineered_repo();
    let repo = tmp.path();
    let engine = Engine::start(repo).expect("engine starts");

    // Save a baseline, then record the gated verdict.
    engine.gate(None, true, true).expect("gate --save");
    let baseline = gated_verdict(&engine.gate(None, false, true).expect("gate"));

    // (1) History advances: a temporal evaluation populates history.db and a
    // commit to a NON-indexed path moves HEAD — neither touches the graph.
    engine
        .hotspots(None, false, false)
        .expect("temporal eval populates history.db");
    assert!(
        repo.join(".logos/history.db").exists(),
        "the temporal read created history.db"
    );
    commit(repo, "NOTES.md", "not indexed\n", "doc-only commit");
    engine
        .hotspots(None, false, false)
        .expect("temporal eval after the advance");
    let after_advance = gated_verdict(&engine.gate(None, false, true).expect("gate"));
    assert_eq!(
        baseline, after_advance,
        "history advancing never moves the gate"
    );

    // (2) history.db deleted: still byte-identical (the gate never reads it).
    std::fs::remove_file(repo.join(".logos/history.db")).expect("delete history.db");
    let after_delete = gated_verdict(&engine.gate(None, false, true).expect("gate"));
    assert_eq!(
        baseline, after_delete,
        "deleting history.db changes nothing gated"
    );

    // (3) A full re-index leaves history.db intact and the gate unchanged.
    engine.hotspots(None, false, false).expect("repopulate history.db");
    engine.index();
    assert!(
        repo.join(".logos/history.db").exists(),
        "history.db survives a full index (FR-GH-01)"
    );
    let after_index = gated_verdict(&engine.gate(None, false, true).expect("gate"));
    assert_eq!(baseline, after_index, "a full index never moves the gate");
}

#[test]
fn a_bare_gate_never_mines_history() {
    let tmp = engineered_repo();
    let repo = tmp.path();
    // A fresh engine that only ever runs `gate` must not create history.db —
    // mining is off the gate path (FR-GH-02, BR-26).
    let engine = Engine::start(repo).expect("engine starts");
    engine.gate(None, true, true).expect("gate --save");
    engine.gate(None, false, true).expect("gate");
    assert!(
        !repo.join(".logos/history.db").exists(),
        "gate spawns no mining — history.db must not exist"
    );
}

// ── FR-GH-07: tier-labeled temporal columns in scan detail ───────────────────

/// The gated half of a scan, isolated from the additive temporal tier.
fn gated_scan(s: &logos_core::models::quality::ScanResult) -> String {
    serde_json::to_string(&serde_json::json!({
        "signal": s.signal,
        "metrics": s.metrics,
        "violations": s.violations,
        "worst_offenders": s.worst_offenders,
    }))
    .unwrap()
}

#[test]
fn scan_renders_the_labeled_temporal_tier_without_moving_gated_columns() {
    let tmp = engineered_repo();
    let git = Engine::start(tmp.path()).expect("engine starts");
    let scan_git = git.scan(true).expect("scan runs");

    // The non-gated tier is present, labeled, and never gated (NFR-CC-04, BR-26).
    assert!(
        scan_git.temporal.tier.contains("non-gated"),
        "tier label is explicit"
    );
    assert!(!scan_git.temporal.gated, "the temporal tier is never gated");
    assert_eq!(
        scan_git.temporal.defect_label, "heuristic",
        "FR-GH-05 label"
    );
    // The defect-history column is populated (one `fix:` commit touched
    // hot.rs) AND carries the heuristic label — the label is proven on a
    // non-zero value, not a trivially-zero column ([FR-GH-05], [NFR-CC-04]).
    let hot = scan_git
        .temporal
        .files
        .iter()
        .find(|f| f.path == "src/hot.rs")
        .expect("the temporal columns carry the indexed files");
    assert!(
        hot.defect_commits > 0,
        "the heuristic-labeled defect column is populated: {hot:?}"
    );

    // A byte-identical NON-git copy of the same tree degrades the temporal tier
    // to n/a, while the gated columns stay byte-identical (FR-GH-07).
    let plain = TempDir::new().unwrap();
    for rel in [
        "src/calm.rs",
        "src/complex.rs",
        "src/churny.rs",
        "src/hot.rs",
    ] {
        let src = std::fs::read(tmp.path().join(rel)).unwrap();
        let dst = plain.path().join(rel);
        std::fs::create_dir_all(dst.parent().unwrap()).unwrap();
        std::fs::write(dst, src).unwrap();
    }
    let nogit = Engine::start(plain.path()).expect("engine starts");
    nogit.index();
    let scan_nogit = nogit.scan(true).expect("scan runs");

    assert_eq!(
        gated_scan(&scan_git),
        gated_scan(&scan_nogit),
        "gated columns are byte-identical with the temporal tier present vs n/a"
    );
    assert!(
        scan_nogit.temporal.degraded.is_some() && scan_nogit.temporal.files.is_empty(),
        "a non-git tree's temporal tier is n/a, never fabricated"
    );
}

// ── FR-GH-08 / UAT-GH-04: degraded modes — n/a + notice, exit 0 ──────────────

#[test]
fn non_git_directory_reports_na_with_a_notice() {
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    std::fs::write(tmp.path().join("src/lib.rs"), calm_source("a")).unwrap();
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.index();

    // The call SUCCEEDS (maps to exit 0) and reports n/a, never an error.
    let report = engine
        .hotspots(None, false, false)
        .expect("hotspots succeeds in a non-git dir");
    assert!(report.files.is_empty(), "no fabricated hotspots");
    assert!(report.degraded.is_some(), "the tier is degraded");
    assert!(
        report.notice.as_deref().is_some_and(|n| !n.is_empty()),
        "a one-line notice explains the degrade"
    );
}

#[test]
fn shallow_clone_reports_na_without_partial_numbers() {
    let tmp = engineered_repo();
    // A shallow (`--depth 1`) local clone over file:// — no network (UAT-GH-04).
    let shallow_parent = TempDir::new().unwrap();
    let shallow = shallow_parent.path().join("shallow");
    let url = format!("file://{}", tmp.path().display());
    sh_git(
        shallow_parent.path(),
        &["clone", "-q", "--depth", "1", &url, "shallow"],
    );

    let engine = Engine::start(&shallow).expect("engine starts");
    engine.index();
    let report = engine
        .hotspots(None, false, false)
        .expect("hotspots succeeds on a shallow clone");
    assert!(
        report.files.is_empty(),
        "a shallow clone shows n/a, never misleadingly low churn"
    );
    assert!(report.degraded.is_some(), "the shallow clone degrades");
    assert!(
        report.notice.as_deref().is_some_and(|n| !n.is_empty()),
        "a one-line notice explains the shallow-clone degrade (UAT-GH-04)"
    );
}

// ── FR-SC-14: findings column and filter ─────────────────────────────────────

/// A SARIF 2.1.0 report with one result per `(path, level)`, from a scanner the
/// store reads as it reads Semgrep: `error` is high, `warning` medium.
fn sarif(results: &[(&str, &str)]) -> String {
    let results: Vec<String> = results
        .iter()
        .enumerate()
        .map(|(n, (path, level))| {
            format!(
                r#"{{"ruleId":"rule.{n}","level":"{level}","message":{{"text":"finding {n}"}},
                "locations":[{{"physicalLocation":{{"artifactLocation":{{"uri":"{path}"}},
                "region":{{"startLine":1}}}}}}]}}"#
            )
        })
        .collect();
    format!(
        r#"{{"version":"2.1.0","runs":[{{"tool":{{"driver":{{"name":"Semgrep OSS","semanticVersion":"1.85.0",
        "rules":[]}}}},"results":[{}]}}]}}"#,
        results.join(",")
    )
}

/// Two files identical on churn and complexity — only a finding can separate
/// them — indexed and mined, with the history warmed so no board carries the
/// first-mine notice.
fn twin_repo() -> TempDir {
    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);
    for name in ["a", "z"] {
        for n in 0..2 {
            commit(
                repo,
                &format!("src/{name}.rs"),
                &format!("{}// rev {n}\n", complex_source(name)),
                &format!("{name} v{n}"),
            );
        }
    }
    let engine = Engine::start(repo).expect("engine starts");
    engine.index();
    engine.hotspots(None, false, false).expect("warm-up mine");
    tmp
}

fn ingest(engine: &Engine, root: &Path, results: &[(&str, &str)]) {
    let report = root.parent().unwrap().join(format!(
        "{}-report.sarif",
        root.file_name().unwrap().to_string_lossy()
    ));
    std::fs::write(&report, sarif(results)).unwrap();
    engine.security_ingest(&report).expect("the report ingests");
}

fn cell<'a>(report: &'a logos_core::history::HotspotReport, path: &str) -> &'a logos_core::history::PathFindings {
    report
        .files
        .iter()
        .find(|h| h.path == path)
        .and_then(|h| h.findings.as_ref())
        .unwrap_or_else(|| panic!("{path} carries a findings cell"))
}

#[test]
fn a_project_nothing_was_scanned_in_has_no_findings_column() {
    let tmp = twin_repo();
    let engine = Engine::start(tmp.path()).expect("engine starts");

    for report in [
        engine.hotspots(None, false, false).unwrap(),
        engine.hotspots_with_findings(None, false, false, true).unwrap(),
        engine.latest_hotspots_with_findings(None, false, false, true).unwrap(),
    ] {
        assert!(report.files.iter().all(|h| h.findings.is_none()), "no scan, no cell");
        assert!(!report.with_findings, "the filter has nothing to prefer");
        let json = serde_json::to_value(&report).unwrap();
        assert!(json["files"][0].get("findings").is_none(), "{json}");
    }
}

#[test]
fn under_the_filter_a_fresh_high_finding_ranks_above_an_identical_hotspot() {
    let tmp = twin_repo();
    let engine = Engine::start(tmp.path()).expect("engine starts");
    ingest(&engine, tmp.path(), &[("src/z.rs", "error")]);

    let plain = engine.hotspots(None, false, false).unwrap();
    let paths: Vec<&str> = plain.files.iter().map(|h| h.path.as_str()).collect();
    assert_eq!(paths, ["src/a.rs", "src/z.rs"], "the filter off keeps the path tie-break");
    assert_eq!(plain.files[0].score, plain.files[1].score, "the twins are identical");
    assert_eq!(cell(&plain, "src/z.rs").count, 1, "the column shows without the filter");
    assert_eq!(cell(&plain, "src/a.rs").count, 0, "a scanned file with none reads zero");

    for filtered in [
        engine.hotspots_with_findings(None, false, false, true).unwrap(),
        engine.latest_hotspots_with_findings(None, false, false, true).unwrap(),
    ] {
        let paths: Vec<&str> = filtered.files.iter().map(|h| h.path.as_str()).collect();
        assert_eq!(paths, ["src/z.rs", "src/a.rs"], "the fresh high finding leads: {paths:?}");
        assert_eq!(
            serde_json::to_value(cell(&filtered, "src/z.rs").highest).unwrap(),
            "high"
        );
        assert!(filtered.with_findings);
    }
}

#[test]
fn a_stale_finding_is_labelled_and_excluded_from_the_count_and_the_ranking() {
    let tmp = twin_repo();
    let engine = Engine::start(tmp.path()).expect("engine starts");
    ingest(&engine, tmp.path(), &[("src/z.rs", "error")]);
    // The file moves after the report was ingested: its anchor no longer matches.
    std::fs::write(tmp.path().join("src/z.rs"), "// moved\n").unwrap();

    let filtered = engine.hotspots_with_findings(None, false, false, true).unwrap();
    let z = cell(&filtered, "src/z.rs");
    assert_eq!((z.count, z.highest, z.stale), (0, None, 1), "labelled stale, not counted");
    let paths: Vec<&str> = filtered.files.iter().map(|h| h.path.as_str()).collect();
    assert_eq!(paths, ["src/a.rs", "src/z.rs"], "a stale finding ranks nothing: {paths:?}");
}

#[test]
fn a_suppressed_finding_leaves_the_count_and_an_expired_suppression_returns_it() {
    let tmp = twin_repo();
    let engine = Engine::start(tmp.path()).expect("engine starts");
    ingest(&engine, tmp.path(), &[("src/z.rs", "error")]);
    let fingerprint = engine
        .security_findings(&logos_core::security::FindingsFilter::default())
        .unwrap()
        .findings[0]
        .finding
        .fingerprint
        .clone();
    let ignore = |expires: &str| {
        std::fs::write(
            tmp.path().join(".logos/security-ignore.toml"),
            format!("[[ignore]]\nfingerprint = \"{fingerprint}\"\nreason = \"accepted risk\"\nexpires = {expires}\n"),
        )
        .unwrap();
    };

    ignore("2999-01-01");
    let suppressed = engine.hotspots(None, false, false).unwrap();
    assert_eq!(cell(&suppressed, "src/z.rs").count, 0, "an unexpired entry suppresses it");

    ignore("2000-01-01");
    let expired = engine.hotspots(None, false, false).unwrap();
    assert_eq!(cell(&expired, "src/z.rs").count, 1, "an expired entry no longer does");
}

#[test]
fn an_invalid_ignore_file_fails_the_board_only_once_a_scan_is_stored() {
    let tmp = twin_repo();
    let engine = Engine::start(tmp.path()).expect("engine starts");
    std::fs::write(tmp.path().join(".logos/security-ignore.toml"), "not toml [[[").unwrap();

    // Nothing scanned: there is nothing to label, so the board does not depend on the file.
    let board = engine.hotspots(None, false, false).expect("a never-scanned board ignores the file");
    assert!(board.files.iter().all(|h| h.findings.is_none()));

    // A stored report makes the suppressions load-bearing: an unreadable file is an error
    // (exit 2), never a silent board of unsuppressed findings.
    ingest(&engine, tmp.path(), &[("src/z.rs", "error")]);
    for result in [
        engine.hotspots(None, false, false),
        engine.latest_hotspots_with_findings(None, false, false, true),
    ] {
        let err = result.expect_err("a stored report with an unreadable ignore file must not rank");
        assert!(format!("{err:#}").contains("security-ignore.toml"), "{err:#}");
    }
}
