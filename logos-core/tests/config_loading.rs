//! Integration tests for the `config` module (S-006): the `config.toml` /
//! `rules.toml` loaders/validators and the contained discovery walk.
//!
//! These exercise the four acceptance criteria of S-006 end-to-end against real
//! temp-directory fixtures:
//!   1. `config.toml` loads with defaults; unknown keys / bad globs fail (exit 2) — FR-CF-01.
//!   2. Discovery composes gitignore + excludes + `ignored_dirs` — FR-CF-02, FR-IX-02.
//!   3. Files above `max_file_size` are skipped with a notice — FR-CF-04.
//!   4. `rules.toml` parse errors fail loud (exit 2) — FR-CF-03; the walk never
//!      escapes the project root — NFR-SE-04.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use logos_core::config::{self, Config, ConfigError, MaxDead, MaxDeadBaseline, DEFAULT_MAX_FILE_SIZE};
use tempfile::tempdir;

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Write `contents` to `root/rel`, creating parent directories as needed.
fn write(root: &Path, rel: &str, contents: &str) -> PathBuf {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, contents).unwrap();
    path
}

/// Discovered files as a sorted set of root-relative `/`-joined strings.
fn discovered_rels(report: &config::DiscoveryReport, root: &Path) -> BTreeSet<String> {
    let canonical = root.canonicalize().unwrap();
    report
        .files
        .iter()
        .map(|p| {
            p.strip_prefix(&canonical)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect()
}

// ── FR-CF-01: config.toml loads with sensible defaults ────────────────────────

#[test]
fn empty_config_yields_all_defaults() {
    let dir = tempdir().unwrap();
    let path = write(dir.path(), "config.toml", "");
    let config = config::load_config(&path).unwrap();
    assert_eq!(config, Config::default());
    // Spot-check the defaults the FRs name explicitly.
    assert_eq!(config.max_file_size, DEFAULT_MAX_FILE_SIZE); // 2 MiB (FR-CF-04)
    assert_eq!(config.include, vec!["**".to_string()]);
    assert_eq!(config.watcher.debounce_ms, 300); // OQ-04 (resolved: 300 ms)
    assert!(config.semantics.ignored_dirs.iter().any(|d| d == "target"));
    assert!(config.languages.is_empty()); // empty = all compiled-in code grammars (CR-017/S-081)
    assert_eq!(
        config.semantics.entry_points,
        vec!["main".to_string()],
        "dead-code entry points default to the binary root (S-014, FR-AN-01)"
    );
}

#[test]
fn semantics_entry_points_override_parses_verbatim() {
    // S-014 / FR-AN-01: the declared dead-code roots feed the annotation
    // pass directly — a mis-deserialized list would silently break liveness
    // for every binary crate, so the override path is pinned here.
    let dir = tempdir().unwrap();
    let path = write(
        dir.path(),
        "config.toml",
        r#"
            [semantics]
            entry_points = ["start", "worker"]
        "#,
    );
    let config = config::load_config(&path).unwrap();
    assert_eq!(
        config.semantics.entry_points,
        vec!["start".to_string(), "worker".to_string()]
    );
    // A semantics table that sets only entry_points keeps the ignored_dirs
    // defaults (per-field serde defaults, FR-CF-01).
    assert!(config.semantics.ignored_dirs.iter().any(|d| d == "target"));
}

#[test]
fn omitted_fields_use_defaults_while_set_fields_override() {
    let dir = tempdir().unwrap();
    let path = write(
        dir.path(),
        "config.toml",
        r#"
            languages = ["rust"]
            max_file_size = 1024
            exclude = ["**/*.snap"]
        "#,
    );
    let config = config::load_config(&path).unwrap();
    assert_eq!(config.languages, vec!["rust".to_string()]);
    assert_eq!(config.max_file_size, 1024);
    assert_eq!(config.exclude, vec!["**/*.snap".to_string()]);
    // Untouched fields fall back to defaults.
    assert_eq!(config.include, vec!["**".to_string()]);
    assert_eq!(config.watcher.debounce_ms, 300);
}

#[test]
fn documentation_defaults_on_with_the_fr_dg_01_globs() {
    // FR-CF-01 / S-034: documentation indexing defaults on, with the FR-DG-01
    // doc globs, when the `[documentation]` table is omitted.
    let dir = tempdir().unwrap();
    let path = write(dir.path(), "config.toml", "");
    let config = config::load_config(&path).unwrap();
    assert!(
        config.documentation.enabled,
        "documentation is on by default"
    );
    assert_eq!(
        config.documentation.include,
        vec![
            "docs/**/*.md".to_string(),
            "*.md".to_string(),
            "README*".to_string()
        ]
    );
    assert!(config.documentation.exclude.is_empty());
}

#[test]
fn documentation_toggle_and_globs_override() {
    // FR-CF-01: the toggle and globs are overridable; omitted sub-fields keep
    // their defaults.
    let dir = tempdir().unwrap();
    let path = write(
        dir.path(),
        "config.toml",
        r#"
            [documentation]
            enabled = false
            include = ["wiki/**/*.md"]
        "#,
    );
    let config = config::load_config(&path).unwrap();
    assert!(!config.documentation.enabled);
    assert_eq!(
        config.documentation.include,
        vec!["wiki/**/*.md".to_string()]
    );
    // `exclude` was omitted → its default (empty).
    assert!(config.documentation.exclude.is_empty());
}

#[test]
fn bad_documentation_glob_fails_exit_2() {
    // A malformed doc glob is rejected at load like a bad include/exclude glob.
    let dir = tempdir().unwrap();
    let path = write(
        dir.path(),
        "config.toml",
        "[documentation]\ninclude = [\"docs/{a\"]\n",
    );
    let err = config::load_config(&path).unwrap_err();
    assert!(matches!(err, ConfigError::BadGlob { .. }));
    assert_eq!(err.exit_code(), 2);
}

#[test]
fn traversal_documentation_glob_is_rejected_exit_2() {
    // NFR-SE-04: a doc glob that would escape the root is rejected, same as a
    // code include/exclude glob.
    let dir = tempdir().unwrap();
    let path = write(
        dir.path(),
        "config.toml",
        "[documentation]\ninclude = [\"../outside/*.md\"]\n",
    );
    let err = config::load_config(&path).unwrap_err();
    assert!(matches!(err, ConfigError::EscapingPattern { .. }));
    assert_eq!(err.exit_code(), 2);
}

#[test]
fn missing_config_file_resolves_to_defaults() {
    // Policy travels into worktrees but need not exist in every one (NFR-DM-04).
    let dir = tempdir().unwrap();
    let config = config::load_config_from_root(dir.path()).unwrap();
    assert_eq!(config, Config::default());
}

#[test]
fn unknown_top_level_key_fails_with_actionable_message_exit_2() {
    let dir = tempdir().unwrap();
    let path = write(dir.path(), "config.toml", "langauges = [\"rust\"]\n"); // typo
    let err = config::load_config(&path).unwrap_err();
    assert!(matches!(err, ConfigError::Parse { .. }));
    assert_eq!(err.exit_code(), 2);
    // The message names the offending key — actionable (NFR-UX-02).
    assert!(err.to_string().contains("langauges"), "msg: {err}");
}

#[test]
fn unknown_nested_key_fails_exit_2() {
    let dir = tempdir().unwrap();
    let path = write(
        dir.path(),
        "config.toml",
        "[semantics]\nignored_dirs = [\"target\"]\nbogus = 1\n",
    );
    let err = config::load_config(&path).unwrap_err();
    assert!(matches!(err, ConfigError::Parse { .. }));
    assert_eq!(err.exit_code(), 2);
}

#[test]
fn invalid_toml_syntax_fails_exit_2() {
    let dir = tempdir().unwrap();
    let path = write(dir.path(), "config.toml", "languages = [\n"); // unterminated
    let err = config::load_config(&path).unwrap_err();
    assert!(matches!(err, ConfigError::Parse { .. }));
    assert_eq!(err.exit_code(), 2);
}

#[test]
fn bad_exclude_glob_fails_exit_2() {
    let dir = tempdir().unwrap();
    let path = write(dir.path(), "config.toml", "exclude = [\"src/{a\"]\n");
    let err = config::load_config(&path).unwrap_err();
    assert!(matches!(err, ConfigError::BadGlob { .. }));
    assert_eq!(err.exit_code(), 2);
}

#[test]
fn traversal_include_glob_is_rejected_exit_2() {
    // NFR-SE-04: an include glob that would escape the root is rejected.
    let dir = tempdir().unwrap();
    let path = write(dir.path(), "config.toml", "include = [\"../*.rs\"]\n");
    let err = config::load_config(&path).unwrap_err();
    assert!(matches!(err, ConfigError::EscapingPattern { .. }));
    assert_eq!(err.exit_code(), 2);
}

// ── FR-CF-03: rules.toml is the architecture contract ─────────────────────────

#[test]
fn valid_rules_parse() {
    let dir = tempdir().unwrap();
    let path = write(
        dir.path(),
        "rules.toml",
        r#"
            [constraints]
            max_cycles = 0
            max_cc = 15
            no_god_files = 40
            max_fan_in = 20
            max_fan_out = 15
            max_dead = 0
            max_duplicates = 3

            [[layers]]
            name = "domain"
            paths = ["src/domain/**"]
            order = 0

            [[layers]]
            name = "infra"
            paths = ["src/infra/**"]
            order = 2

            [[boundaries]]
            from = "domain"
            to = "infra"
            reason = "the domain must not depend on infrastructure"
        "#,
    );
    let rules = config::load_rules(&path).unwrap();
    assert_eq!(rules.constraints.max_cycles, Some(0));
    assert_eq!(rules.constraints.no_god_files, Some(40));
    // CR-002 / FR-GV-11: the coupling & redundancy budgets parse.
    assert_eq!(rules.constraints.max_fan_in, Some(20));
    assert_eq!(rules.constraints.max_fan_out, Some(15));
    assert_eq!(rules.constraints.max_dead, Some(MaxDead::Absolute(0)));
    assert_eq!(rules.constraints.max_duplicates, Some(3));
    assert_eq!(rules.layers.len(), 2);
    assert_eq!(rules.layers[0].name, "domain");
    assert_eq!(rules.boundaries.len(), 1);
    assert_eq!(rules.boundaries[0].from, "domain");
    assert!(rules.boundaries[0].reason.is_some());
}

/// CR-002 / FR-GV-11: the four coupling/redundancy budgets are individually
/// optional — a contract that sets none leaves them all `None`.
#[test]
fn coupling_redundancy_budgets_are_optional() {
    let dir = tempdir().unwrap();
    let path = write(dir.path(), "rules.toml", "[constraints]\nmax_cc = 10\n");
    let rules = config::load_rules(&path).unwrap();
    assert_eq!(rules.constraints.max_fan_in, None);
    assert_eq!(rules.constraints.max_fan_out, None);
    assert_eq!(rules.constraints.max_dead, None);
    assert_eq!(rules.constraints.max_duplicates, None);
}

/// CR-043 / FR-GV-11: the `max_dead` delta-from-blessed-baseline form loads and
/// validates through the same file-based path as the absolute form, parsed/
/// validated at load like the other `[constraints]` budgets.
#[test]
fn max_dead_delta_form_loads_through_the_file_path() {
    let dir = tempdir().unwrap();

    let delta = write(
        dir.path(),
        "rules.toml",
        "[constraints]\nmax_dead = { baseline = 42, delta = 3 }\n",
    );
    let rules = config::load_rules(&delta).unwrap();
    assert_eq!(
        rules.constraints.max_dead,
        Some(MaxDead::Baseline(MaxDeadBaseline {
            baseline: 42,
            delta: 3
        }))
    );

    // The absolute form still loads unchanged.
    let abs = write(dir.path(), "rules-abs.toml", "[constraints]\nmax_dead = 5\n");
    let abs_rules = config::load_rules(&abs).unwrap();
    assert_eq!(abs_rules.constraints.max_dead, Some(MaxDead::Absolute(5)));

    // A typo'd key in the delta table fails loud at load (exit 2), exactly as
    // any other unknown `[constraints]` key would.
    let bad = write(
        dir.path(),
        "rules-bad.toml",
        "[constraints]\nmax_dead = { baseline = 42, oops = 1 }\n",
    );
    let err = config::load_rules(&bad).unwrap_err();
    assert_eq!(err.exit_code(), 2);
}

#[test]
fn empty_rules_yield_defaults() {
    let dir = tempdir().unwrap();
    let path = write(dir.path(), "rules.toml", "");
    let rules = config::load_rules(&path).unwrap();
    assert!(rules.layers.is_empty());
    assert!(rules.boundaries.is_empty());
    assert_eq!(rules.constraints.max_cc, None);
}

#[test]
fn missing_rules_file_resolves_to_defaults() {
    let dir = tempdir().unwrap();
    let rules = config::load_rules_from_root(dir.path()).unwrap();
    assert_eq!(rules, config::Rules::default());
}

#[test]
fn rules_parse_error_fails_loud_exit_2() {
    // FR-CF-03: a malformed rules.toml fails loud with exit 2.
    let dir = tempdir().unwrap();
    let path = write(dir.path(), "rules.toml", "[[layers]\nname = \"x\"\n"); // broken table
    let err = config::load_rules(&path).unwrap_err();
    assert!(matches!(err, ConfigError::Parse { .. }));
    assert_eq!(err.exit_code(), 2);
}

#[test]
fn rules_unknown_key_fails_exit_2() {
    let dir = tempdir().unwrap();
    let path = write(
        dir.path(),
        "rules.toml",
        "[constraints]\nmax_loops = 3\n", // not a real constraint
    );
    let err = config::load_rules(&path).unwrap_err();
    assert!(matches!(err, ConfigError::Parse { .. }));
    assert_eq!(err.exit_code(), 2);
}

#[test]
fn rules_bad_layer_glob_fails_exit_2() {
    let dir = tempdir().unwrap();
    let path = write(
        dir.path(),
        "rules.toml",
        "[[layers]]\nname = \"x\"\npaths = [\"../escape/**\"]\norder = 0\n",
    );
    let err = config::load_rules(&path).unwrap_err();
    assert!(matches!(err, ConfigError::EscapingPattern { .. }));
    assert_eq!(err.exit_code(), 2);
}

// ── CR-005 / FR-QM-14 / FR-GV-11: the metric-threshold table + budgets ───────

/// FR-QM-14 / FR-GV-11: the `[metric_thresholds]` table and the four structural
/// budgets parse, and omitted threshold keys stay `None` (the documented
/// defaults are applied downstream by the governance engine).
#[test]
fn metric_thresholds_table_and_budgets_parse() {
    let dir = tempdir().unwrap();
    let path = write(
        dir.path(),
        "rules.toml",
        "\
[constraints]
max_nesting_depth = 4
max_brain_methods = 0
max_clone_ratio = 0.0
no_god_containers = true

[metric_thresholds]
nesting_depth = 5
god_methods = 30
clone_similarity = 0.9
clone_min_tokens = 80
",
    );
    let rules = config::load_rules(&path).unwrap();
    assert_eq!(rules.constraints.max_nesting_depth, Some(4));
    assert_eq!(rules.constraints.max_brain_methods, Some(0));
    assert_eq!(rules.constraints.max_clone_ratio, Some(0.0));
    assert_eq!(rules.constraints.no_god_containers, Some(true));
    assert_eq!(rules.metric_thresholds.nesting_depth, Some(5));
    assert_eq!(rules.metric_thresholds.god_methods, Some(30));
    // CR-013: the two near-clone keys parse under [metric_thresholds].
    assert_eq!(rules.metric_thresholds.clone_similarity, Some(0.9));
    assert_eq!(rules.metric_thresholds.clone_min_tokens, Some(80));
    // An omitted key stays None (the default is applied downstream).
    assert_eq!(rules.metric_thresholds.brain_complexity, None);
}

/// CR-013 / FR-AN-06: a `clone_similarity` outside `(0, 1]` fails loud at load
/// (exit 2) — a value at or below 0 (or above 1) is not a valid Jaccard ratio.
#[test]
fn out_of_range_clone_similarity_fails_exit_2() {
    let dir = tempdir().unwrap();
    let path = write(
        dir.path(),
        "rules.toml",
        "[metric_thresholds]\nclone_similarity = 1.5\n",
    );
    let err = config::load_rules(&path).unwrap_err();
    assert!(matches!(err, ConfigError::InvalidValue { .. }));
    assert_eq!(err.exit_code(), 2);
    assert!(err.to_string().contains("metric_thresholds.clone_similarity"));
}

/// CR-013 / FR-EX-09: a non-positive `clone_min_tokens` fails loud at load
/// (exit 2) on the same path as the structural thresholds.
#[test]
fn non_positive_clone_min_tokens_fails_exit_2() {
    let dir = tempdir().unwrap();
    let path = write(
        dir.path(),
        "rules.toml",
        "[metric_thresholds]\nclone_min_tokens = 0\n",
    );
    let err = config::load_rules(&path).unwrap_err();
    assert!(matches!(err, ConfigError::InvalidValue { .. }));
    assert_eq!(err.exit_code(), 2);
    assert!(err.to_string().contains("metric_thresholds.clone_min_tokens"));
}

/// CR-005: a `max_clone_ratio` outside `[0.0, 1.0]` fails loud at load (exit 2),
/// so a misconfiguration can never silently flag every clean project.
#[test]
fn out_of_range_clone_ratio_fails_exit_2() {
    let dir = tempdir().unwrap();
    let path = write(
        dir.path(),
        "rules.toml",
        "[constraints]\nmax_clone_ratio = -0.1\n",
    );
    let err = config::load_rules(&path).unwrap_err();
    assert!(matches!(err, ConfigError::InvalidValue { .. }));
    assert_eq!(err.exit_code(), 2);
    assert!(err.to_string().contains("max_clone_ratio"));
}

/// CR-005: a non-positive metric threshold fails loud at load (exit 2) — a
/// `T_nest = 0` would otherwise flag every function as deeply nested.
#[test]
fn non_positive_metric_threshold_fails_exit_2() {
    let dir = tempdir().unwrap();
    let path = write(
        dir.path(),
        "rules.toml",
        "[metric_thresholds]\nnesting_depth = 0\n",
    );
    let err = config::load_rules(&path).unwrap_err();
    assert!(matches!(err, ConfigError::InvalidValue { .. }));
    assert_eq!(err.exit_code(), 2);
    assert!(err.to_string().contains("metric_thresholds.nesting_depth"));
}

// ── FR-CF-02 / FR-IX-02: discovery composes the three exclusion sources ───────

#[test]
fn discovery_composes_gitignore_excludes_and_ignored_dirs() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    // Indexable sources.
    write(root, "src/main.rs", "fn main() {}");
    write(root, "src/lib.rs", "pub fn lib() {}");
    // Excluded by gitignore.
    write(root, ".gitignore", "ignored_by_git.rs\n");
    write(root, "ignored_by_git.rs", "// gitignored");
    // Excluded by an `ignored_dirs` default (target/, node_modules/, build/).
    write(root, "target/junk.rs", "// build artifact");
    write(root, "node_modules/dep.js", "// vendored dep");
    write(root, "build/out.rs", "// generated");
    // Excluded by a config `exclude` glob.
    write(root, "secret.key", "shhh");

    let config = Config {
        exclude: vec!["**/*.key".to_string()],
        ..Config::default()
    };

    let report = config::discover(root, &config).unwrap();
    let rels = discovered_rels(&report, root);

    // Present: the real sources.
    assert!(rels.contains("src/main.rs"), "got: {rels:?}");
    assert!(rels.contains("src/lib.rs"), "got: {rels:?}");
    // Absent: each of the three exclusion sources (union).
    assert!(!rels.contains("ignored_by_git.rs"), "gitignore");
    assert!(!rels.contains("target/junk.rs"), "ignored_dirs");
    assert!(!rels.contains("node_modules/dep.js"), "ignored_dirs");
    assert!(!rels.contains("build/out.rs"), "ignored_dirs");
    assert!(!rels.contains("secret.key"), "config exclude");
}

#[test]
fn discovery_skips_nested_git_worktrees_and_repos() {
    // Issue 2.2-B: a nested git boundary — a linked worktree (`.git` gitlink file),
    // a vendored repo (`.git` directory), or a submodule — is a separate working
    // tree the `ignore` crate treats as its own repo root, so the parent
    // `.gitignore` cannot reliably mask it. Logos indexes the one root project, so
    // discovery prunes any depth>0 directory that itself contains a `.git` entry.
    // This is what let a removed `.worktrees/<sprint>/…` checkout pollute the graph
    // with duplicate symbols. Neither nested dir name is in the default
    // `ignored_dirs`, so the prune here is the `.git`-boundary rule, not a name match.
    let dir = tempdir().unwrap();
    let root = dir.path();

    write(root, "src/main.rs", "fn main() {}");
    // A linked worktree: a directory carrying a `.git` *gitlink file*.
    write(root, ".worktrees/wt/.git", "gitdir: /elsewhere/.git/worktrees/wt\n");
    write(root, ".worktrees/wt/src/copy.rs", "pub fn copy() {}");
    // A vendored repo: a directory carrying a `.git` *directory*.
    write(root, "external/repo/.git/HEAD", "ref: refs/heads/main\n");
    write(root, "external/repo/src/dep.rs", "pub fn dep() {}");

    let report = config::discover(root, &Config::default()).unwrap();
    let rels = discovered_rels(&report, root);

    assert!(rels.contains("src/main.rs"), "the root project source is indexed: {rels:?}");
    assert!(
        !rels.iter().any(|r| r.starts_with(".worktrees/")),
        "a nested linked worktree is not walked: {rels:?}"
    );
    assert!(
        !rels.iter().any(|r| r.starts_with("external/repo/")),
        "a nested vendored repo is not walked: {rels:?}"
    );
}

// ── FR-CF-05 / CR-029: the broadened built-in default exclusions ──────────────

/// A fresh project (built-in defaults, no `config.toml`) excludes the agent/
/// tooling dirs, the per-language build/output dirs, and the planning/security/
/// notes prose paths — both the code and the documentation under them — while
/// still indexing real source and docs elsewhere (FR-CF-05 AC, CR-029).
#[test]
fn default_exclusions_prune_agent_build_and_prose_paths() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    // Real, indexable content that must survive the broadened defaults.
    write(root, "src/main.rs", "fn main() {}");
    write(root, "README.md", "# project");
    write(root, "docs/guide.md", "# guide"); // docs/ generally is still indexed…
    // Root-anchoring guard (FR-CF-05 Notes): the prose paths are root-anchored
    // `exclude` globs (`docs/planning/**`, `docs/security/**`, `notes/**`), NOT
    // `ignored_dirs` names — so a same-named directory nested under real source
    // must NOT be pruned. If the globs ever regressed to `**/notes/**` etc. these
    // would vanish.
    write(root, "src/notes/internal.rs", "// real source, not root notes/");
    write(root, "app/planning/feature.rs", "// real source, not docs/planning");
    write(root, "lib/security/guard.rs", "// real source, not docs/security");

    // Agent/tooling dirs pruned by the default `ignored_dirs` (FR-CF-05).
    write(root, ".agents/skills/foo/SKILL.md", "skill");
    write(root, ".claude/settings.json", "{}");
    // Per-language build/output/cache dirs pruned by the default `ignored_dirs`.
    write(root, "target/junk.rs", "// rust build");
    write(root, "__pycache__/mod.cpython-312.pyc", "bytecode");
    write(root, "node_modules/dep/index.js", "// vendored");
    write(root, ".venv/lib/site.py", "# venv");
    write(root, "obj/Debug/app.dll", "binary");
    // `ignored_dirs` prunes by NAME at ANY depth (FR-IX-02) — a deeply-nested
    // build/cache dir is pruned just like a root-level one.
    write(root, "src/deep/nested/__pycache__/cache.rs", "bytecode");
    // Planning/security/notes paths pruned by the default `exclude` globs —
    // BOTH code and documentation beneath them (FR-CF-05).
    write(root, "docs/planning/sprint.md", "# sprint plan");
    write(root, "docs/planning/helper.rs", "// planning code too");
    write(root, "docs/security/threat-model.md", "# threats");
    write(root, "notes/scratch.md", "# scratch");
    write(root, "notes/snippet.rs", "// notes code too");

    let report = config::discover(root, &Config::default()).unwrap();
    let rels = discovered_rels(&report, root);

    // Present: real source and the docs that are NOT under planning/security.
    assert!(rels.contains("src/main.rs"), "got: {rels:?}");
    assert!(rels.contains("README.md"), "got: {rels:?}");
    assert!(
        rels.contains("docs/guide.md"),
        "docs/ outside planning/security stays indexed: {rels:?}"
    );
    // Root-anchoring: same-named dirs nested in real source are NOT pruned.
    for nested_source in [
        "src/notes/internal.rs",
        "app/planning/feature.rs",
        "lib/security/guard.rs",
    ] {
        assert!(
            rels.contains(nested_source),
            "{nested_source} is real source — the root-anchored exclude must not prune it: {rels:?}"
        );
    }

    // Absent: every newly-excluded dir and path.
    for excluded in [
        ".agents/skills/foo/SKILL.md",
        ".claude/settings.json",
        "target/junk.rs",
        "__pycache__/mod.cpython-312.pyc",
        "src/deep/nested/__pycache__/cache.rs",
        "node_modules/dep/index.js",
        ".venv/lib/site.py",
        "obj/Debug/app.dll",
        "docs/planning/sprint.md",
        "docs/planning/helper.rs",
        "docs/security/threat-model.md",
        "notes/scratch.md",
        "notes/snippet.rs",
    ] {
        assert!(
            !rels.contains(excluded),
            "{excluded} must be excluded by the broadened defaults: {rels:?}"
        );
    }
}

/// A user `config.toml` that overrides `ignored_dirs` / `exclude` REPLACES the
/// built-in defaults it sets (a default-only ignored dir is re-admitted), yet
/// the override still composes with gitignore and the user's own names
/// (FR-CF-02 union of exclusions, CRA-03 overridability).
#[test]
fn user_override_replaces_defaults_and_still_composes_with_gitignore() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    write(root, "src/main.rs", "fn main() {}");
    // `target/` is a built-in default ignored dir; `docs/planning/` a built-in
    // default exclude. Under an override that names NEITHER, both are re-admitted.
    write(root, "target/built.rs", "// was default-pruned");
    write(root, "docs/planning/plan.md", "# was default-pruned");
    // The user's own exclusions.
    write(root, "myout/skip.rs", "// user ignored_dir");
    write(root, "gen/derived.gen", "// user exclude glob");
    // gitignore still composes (FR-CF-02): a gitignored file is excluded
    // regardless of the config override.
    write(root, ".gitignore", "secret.rs\n");
    write(root, "secret.rs", "// gitignored");

    let config = Config {
        semantics: logos_core::config::Semantics {
            ignored_dirs: vec!["myout".to_string()],
            ..Default::default()
        },
        exclude: vec!["**/*.gen".to_string()],
        ..Config::default()
    };

    let report = config::discover(root, &config).unwrap();
    let rels = discovered_rels(&report, root);

    // The override REPLACED the defaults wholesale: the default-only prunes are
    // now indexed.
    assert!(rels.contains("src/main.rs"), "got: {rels:?}");
    assert!(
        rels.contains("target/built.rs"),
        "overriding ignored_dirs drops the default `target` prune: {rels:?}"
    );
    assert!(
        rels.contains("docs/planning/plan.md"),
        "overriding exclude drops the default planning prune: {rels:?}"
    );
    // The override's own exclusions still bite.
    assert!(!rels.contains("myout/skip.rs"), "user ignored_dir");
    assert!(!rels.contains("gen/derived.gen"), "user exclude glob");
    // …and gitignore is still unioned in (FR-CF-02 composition).
    assert!(!rels.contains("secret.rs"), "gitignore still composes");
}

#[test]
fn include_glob_filters_positively() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    write(root, "src/main.rs", "fn main() {}");
    write(root, "README.md", "# docs");
    write(root, "data/notes.txt", "notes");

    let config = Config {
        include: vec!["**/*.rs".to_string()],
        ..Config::default()
    };

    let report = config::discover(root, &config).unwrap();
    let rels = discovered_rels(&report, root);
    assert_eq!(rels, BTreeSet::from(["src/main.rs".to_string()]));
}

// ── FR-CF-04: oversized files are skipped with a notice ───────────────────────

#[test]
fn oversize_file_is_skipped_with_a_notice() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    write(root, "small.rs", "fn x() {}"); // 9 bytes
    write(root, "huge.rs", &"x".repeat(5_000)); // 5000 bytes

    let config = Config {
        max_file_size: 1_000, // 1 KB ceiling for the test
        ..Config::default()
    };

    let report = config::discover(root, &config).unwrap();
    let rels = discovered_rels(&report, root);

    assert!(rels.contains("small.rs"));
    assert!(!rels.contains("huge.rs"), "oversize file must be skipped");

    assert_eq!(report.skipped_oversize.len(), 1);
    let skip = &report.skipped_oversize[0];
    assert!(skip.path.ends_with("huge.rs"));
    assert_eq!(skip.size, 5_000);
    assert_eq!(skip.max, 1_000);

    // The logged notice names the file, the size, and the limit (FR-CF-04) —
    // asserting the numbers guards against a notice-format regression.
    let notices: Vec<String> = report.notices().collect();
    assert_eq!(notices.len(), 1);
    assert!(notices[0].contains("huge.rs"));
    assert!(notices[0].contains("exceeds max_file_size"));
    assert!(
        notices[0].contains("5000"),
        "notice should carry the size: {}",
        notices[0]
    );
    assert!(
        notices[0].contains("1000"),
        "notice should carry the limit: {}",
        notices[0]
    );
}

#[test]
fn file_exactly_at_max_file_size_is_indexed() {
    // The skip is strict greater-than (FR-CF-04 "larger than"): a file exactly at
    // the limit is indexed. Guards against a `>` → `>=` regression.
    let dir = tempdir().unwrap();
    let root = dir.path();
    write(root, "exact.rs", &"x".repeat(1_000));

    let config = Config {
        max_file_size: 1_000,
        ..Config::default()
    };
    let report = config::discover(root, &config).unwrap();
    assert!(discovered_rels(&report, root).contains("exact.rs"));
    assert!(report.skipped_oversize.is_empty());
}

#[test]
fn raising_max_file_size_indexes_the_previously_skipped_file() {
    // FR-CF-04 acceptance: raising max_file_size indexes the big file.
    let dir = tempdir().unwrap();
    let root = dir.path();
    write(root, "huge.rs", &"x".repeat(5_000));

    let config = Config {
        max_file_size: 10_000,
        ..Config::default()
    };

    let report = config::discover(root, &config).unwrap();
    let rels = discovered_rels(&report, root);
    assert!(rels.contains("huge.rs"));
    assert!(report.skipped_oversize.is_empty());
}

// ── NFR-SE-04: the walk never escapes the project root ────────────────────────

#[test]
fn discover_rejects_a_nonexistent_root() {
    let err = config::discover(Path::new("/no/such/logos/root"), &Config::default()).unwrap_err();
    assert!(matches!(err, ConfigError::InvalidRoot { .. }));
    assert_eq!(err.exit_code(), 2);
}

#[cfg(unix)]
#[test]
fn walk_does_not_follow_a_symlinked_directory_out_of_the_root() {
    use std::os::unix::fs::symlink;

    // A file living entirely outside the project root.
    let outside = tempdir().unwrap();
    write(outside.path(), "secret.rs", "// outside the root");

    let dir = tempdir().unwrap();
    let root = dir.path();
    write(root, "src/main.rs", "fn main() {}");
    // A symlinked directory inside the root pointing at the outside tree.
    symlink(outside.path(), root.join("escape")).unwrap();

    let report = config::discover(root, &Config::default()).unwrap();
    let rels = discovered_rels(&report, root);

    assert!(rels.contains("src/main.rs"));
    // The symlinked dir is not descended (follow_links = false), so the outside
    // file is never reached.
    assert!(
        !rels.iter().any(|r| r.contains("secret.rs")),
        "walk escaped the root via a symlink: {rels:?}"
    );
}

#[cfg(unix)]
#[test]
fn walk_does_not_follow_a_symlinked_file_out_of_the_root() {
    use std::os::unix::fs::symlink;

    let outside = tempdir().unwrap();
    let target = write(outside.path(), "secret.rs", "// outside the root");

    let dir = tempdir().unwrap();
    let root = dir.path();
    write(root, "src/main.rs", "fn main() {}");
    // A symlink *file* inside the root pointing at an outside file.
    symlink(&target, root.join("evil.rs")).unwrap();

    let report = config::discover(root, &Config::default()).unwrap();
    // Every discovered path stays within the canonical root, and the symlink
    // (not a regular file) is never indexed.
    let canonical = root.canonicalize().unwrap();
    for f in &report.files {
        assert!(f.starts_with(&canonical), "{f:?} escaped {canonical:?}");
        assert!(!f.ends_with("evil.rs"), "symlinked file was indexed");
    }
}

// ── CR-069 / S-277 / FR-IX-10: sanctioned external docs-root symlink following ─

/// End-to-end over the public `config::discover`: under the swe-skills
/// external-docs layout — a `.swe-skills` file naming a sibling docs root and a
/// `docs/specs` directory symlink into it — discovery follows the symlink and
/// indexes the documentation behind it (under its in-tree `docs/specs/…` path),
/// while a source file behind the same symlink stays out (source-code discovery
/// still skips all symlinks), and a symlink escaping both roots is refused.
#[cfg(unix)]
#[test]
fn follows_sanctioned_docs_symlink_layout_indexing_only_docs() {
    use std::os::unix::fs::symlink;

    let base = tempdir().unwrap();
    let base = base.path().canonicalize().unwrap();

    // The sanctioned sibling repo: a doc (typed spec) and a source file.
    let sanctioned = base.join("logos-docs");
    write(&sanctioned, "specs/ADR-46.md", "# ADR-46\n");
    write(&sanctioned, "requests/CR-069.md", "# CR-069\n");
    write(&sanctioned, "specs/embedded.rs", "pub fn behind_symlink() {}\n");
    let sanctioned = sanctioned.canonicalize().unwrap();

    // The project: a real source file, `.swe-skills` → sibling, and the two
    // directory symlinks swe-skills' link-docs.sh creates.
    let proj = base.join("project");
    write(&proj, "src/main.rs", "fn main() {}\n");
    write(&proj, ".swe-skills", &format!("{}\n", sanctioned.display()));
    fs::create_dir_all(proj.join("docs")).unwrap();
    symlink(sanctioned.join("specs"), proj.join("docs/specs")).unwrap();
    symlink(sanctioned.join("requests"), proj.join("docs/requests")).unwrap();

    // A symlink escaping BOTH roots must never be followed, even though it holds
    // a markdown file the doc globs would otherwise admit.
    let outside = tempdir().unwrap();
    let outside = outside.path().canonicalize().unwrap();
    write(&outside, "secret.md", "# secret\n");
    symlink(&outside, proj.join("docs/leak")).unwrap();

    let report = config::discover(&proj, &Config::default()).unwrap();
    let rels = discovered_rels(&report, &proj);

    assert!(rels.contains("src/main.rs"), "real source discovered: {rels:?}");
    assert!(
        rels.contains("docs/specs/ADR-46.md"),
        "doc behind the sanctioned specs symlink is indexed: {rels:?}"
    );
    assert!(
        rels.contains("docs/requests/CR-069.md"),
        "doc behind the sanctioned requests symlink is indexed: {rels:?}"
    );
    assert!(
        !rels.iter().any(|r| r.ends_with("embedded.rs")),
        "a source file behind the symlink is NOT indexed: {rels:?}"
    );
    assert!(
        !rels.iter().any(|r| r.contains("secret.md")),
        "a symlink escaping both roots is not followed: {rels:?}"
    );

    // Idempotent: a second discovery yields the identical report.
    let again = config::discover(&proj, &Config::default()).unwrap();
    assert_eq!(again, report, "re-running discovery is idempotent for the doc nodes");
}

/// With `.swe-skills` absent, the out-of-root docs symlink escapes both roots
/// and is skipped — the prior skip-all-symlinks behaviour ([FR-IX-10] fail-closed).
#[cfg(unix)]
#[test]
fn absent_swe_skills_reproduces_skip_all_for_docs_symlink() {
    use std::os::unix::fs::symlink;

    let base = tempdir().unwrap();
    let base = base.path().canonicalize().unwrap();
    let sanctioned = base.join("logos-docs");
    write(&sanctioned, "specs/ADR-46.md", "# ADR-46\n");
    let sanctioned = sanctioned.canonicalize().unwrap();

    let proj = base.join("project");
    write(&proj, "src/main.rs", "fn main() {}\n");
    fs::create_dir_all(proj.join("docs")).unwrap();
    symlink(sanctioned.join("specs"), proj.join("docs/specs")).unwrap();
    // NB: no `.swe-skills` written.

    let report = config::discover(&proj, &Config::default()).unwrap();
    let rels = discovered_rels(&report, &proj);

    assert!(rels.contains("src/main.rs"));
    assert!(
        !rels.iter().any(|r| r.starts_with("docs/specs")),
        "with no sanctioned root the out-of-root docs symlink is skipped: {rels:?}"
    );
}

// ── CR-071 / S-279 / FR-IX-10+FR-IX-11: git-ignored sanctioned doc symlinks ───

/// The regression [S-277]'s fixtures missed and [CR-071] closes: when the
/// sanctioned `docs/specs` symlink is **git-ignored** (as on this project's own
/// `main`), discovery still follows it and indexes the docs behind it — the
/// detection pass bypasses the main walk's git-ignore filtering for exactly the
/// sanctioned doc subtree — while a git-ignored non-sanctioned tree stays pruned.
#[cfg(unix)]
#[test]
fn follows_git_ignored_sanctioned_docs_symlink_end_to_end() {
    use std::os::unix::fs::symlink;

    let base = tempdir().unwrap();
    let base = base.path().canonicalize().unwrap();
    let sanctioned = base.join("logos-docs");
    write(&sanctioned, "specs/ADR-46.md", "# ADR-46\n");
    let sanctioned = sanctioned.canonicalize().unwrap();

    let proj = base.join("project");
    write(&proj, "src/main.rs", "fn main() {}\n");
    write(&proj, ".swe-skills", &format!("{}\n", sanctioned.display()));
    fs::create_dir_all(proj.join("docs")).unwrap();
    symlink(sanctioned.join("specs"), proj.join("docs/specs")).unwrap();
    // Git-ignore the symlink AND an ordinary source tree — as this repo does.
    write(&proj, ".gitignore", "/docs/specs\n/generated\n");
    write(&proj, "generated/out.rs", "pub fn g() {}\n");

    let report = config::discover(&proj, &Config::default()).unwrap();
    let rels = discovered_rels(&report, &proj);

    assert!(rels.contains("src/main.rs"), "real source discovered: {rels:?}");
    assert!(
        rels.contains("docs/specs/ADR-46.md"),
        "the git-ignored sanctioned doc is followed and indexed ([CR-071]): {rels:?}"
    );
    assert!(
        !rels.iter().any(|r| r.starts_with("generated/")),
        "a git-ignored non-sanctioned tree stays pruned (bypass is doc-scoped): {rels:?}"
    );
    // Docs index correctly ⇒ no unindexed-doc-symlink warning ([FR-IX-11]).
    assert!(
        report.unindexed_doc_symlinks.is_empty(),
        "no warning when the sanctioned symlink indexes correctly: {:?}",
        report.unindexed_doc_symlinks
    );
    // Idempotent re-index.
    assert_eq!(config::discover(&proj, &Config::default()).unwrap(), report);
}

/// [FR-IX-11]: a git-ignored doc symlink with **no** sanctioned bypass (no
/// `.swe-skills`) is skipped (unchanged) and surfaced as an unindexed drop naming
/// the path + reason, so the silent doc-drop becomes a signal on `index`/`doctor`.
#[cfg(unix)]
#[test]
fn git_ignored_doc_symlink_without_sanction_is_reported_unindexed() {
    use std::os::unix::fs::symlink;

    let base = tempdir().unwrap();
    let base = base.path().canonicalize().unwrap();
    let sanctioned = base.join("logos-docs");
    write(&sanctioned, "specs/ADR-46.md", "# ADR-46\n");
    let sanctioned = sanctioned.canonicalize().unwrap();

    let proj = base.join("project");
    write(&proj, "src/main.rs", "fn main() {}\n");
    fs::create_dir_all(proj.join("docs")).unwrap();
    symlink(sanctioned.join("specs"), proj.join("docs/specs")).unwrap();
    write(&proj, ".gitignore", "/docs/specs\n"); // git-ignored, and NO `.swe-skills`.

    let report = config::discover(&proj, &Config::default()).unwrap();
    let rels = discovered_rels(&report, &proj);

    assert!(
        !rels.iter().any(|r| r.starts_with("docs/specs")),
        "with no sanction the git-ignored doc symlink is skipped: {rels:?}"
    );
    assert_eq!(report.unindexed_doc_symlinks.len(), 1, "one unindexed doc symlink reported");
    let warning = report.unindexed_doc_symlinks[0].to_string();
    assert!(warning.contains("docs/specs"), "warning names the dropped path: {warning}");
    assert!(warning.contains("escapes"), "warning names the reason: {warning}");

    // The `doctor`-side twin (no full index walk) computes the same drop.
    let doctor_side = config::unindexed_doc_symlinks(&proj, &Config::default()).unwrap();
    assert_eq!(doctor_side, report.unindexed_doc_symlinks, "doctor twin agrees with discover");
}

// ── CR-054 / S-213: `.worktrees` / `.playwright-mcp` default ignored_dirs ────

/// A project with NO `.gitignore` at all still keeps `.worktrees/` and
/// `.playwright-mcp/` scratch content out of the graph, via the default
/// `ignored_dirs` alone (CR-054 belt-and-suspenders) — while sibling real
/// source is still admitted.
#[test]
fn default_ignored_dirs_prune_worktrees_and_playwright_mcp_without_gitignore() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    // Real, indexable sibling source — no .gitignore anywhere in this tree.
    write(root, "src/main.rs", "fn main() {}");

    // Scratch dirs a sync/watcher path could otherwise admit if ungitignored.
    write(root, ".worktrees/sprint-1/src/copy.rs", "pub fn copy() {}");
    write(root, ".playwright-mcp/trace.json", "{}");

    let report = config::discover(root, &Config::default()).unwrap();
    let rels = discovered_rels(&report, root);

    assert!(rels.contains("src/main.rs"), "sibling source is admitted: {rels:?}");
    assert!(
        !rels.iter().any(|r| r.starts_with(".worktrees/")),
        ".worktrees/ is pruned by the default ignored_dirs: {rels:?}"
    );
    assert!(
        !rels.iter().any(|r| r.starts_with(".playwright-mcp/")),
        ".playwright-mcp/ is pruned by the default ignored_dirs: {rels:?}"
    );
}

// ── Root-relative loaders: the present-file branch + I/O errors ───────────────

#[test]
fn load_config_from_root_reads_the_logos_subdir() {
    // Exercises the present-file branch of load_config_from_root (and the
    // CONFIG_RELPATH constant) — not just the missing-file default path.
    let dir = tempdir().unwrap();
    write(dir.path(), ".logos/config.toml", "max_file_size = 512\n");
    let config = config::load_config_from_root(dir.path()).unwrap();
    assert_eq!(config.max_file_size, 512);
}

#[test]
fn load_rules_from_root_reads_the_logos_subdir() {
    let dir = tempdir().unwrap();
    write(
        dir.path(),
        ".logos/rules.toml",
        "[constraints]\nmax_cycles = 1\n",
    );
    let rules = config::load_rules_from_root(dir.path()).unwrap();
    assert_eq!(rules.constraints.max_cycles, Some(1));
}

#[test]
fn load_config_from_a_missing_explicit_path_is_an_io_error_exit_2() {
    // The explicit-path loaders (unlike *_from_root) treat a missing file as a
    // hard error — surfaced as ConfigError::Io with exit code 2.
    let err = config::load_config(Path::new("/no/such/config.toml")).unwrap_err();
    assert!(matches!(err, ConfigError::Io { .. }));
    assert_eq!(err.exit_code(), 2);
}

#[test]
fn load_rules_from_a_missing_explicit_path_is_an_io_error_exit_2() {
    let err = config::load_rules(Path::new("/no/such/rules.toml")).unwrap_err();
    assert!(matches!(err, ConfigError::Io { .. }));
    assert_eq!(err.exit_code(), 2);
}

// ── S-011: the [resolution] binding-policy knob ──────────────────────────────

#[test]
fn resolution_policy_defaults_to_balanced_and_parses_all_variants() {
    use logos_core::config::BindingPolicy;

    let dir = tempdir().unwrap();
    // Absent table → the balanced default.
    let cfg = config::load_config_from_root(dir.path()).expect("empty config loads");
    assert_eq!(cfg.resolution.policy, BindingPolicy::Balanced);

    for (text, expected) in [
        ("strict", BindingPolicy::Strict),
        ("balanced", BindingPolicy::Balanced),
        ("aggressive", BindingPolicy::Aggressive),
    ] {
        write(
            dir.path(),
            ".logos/config.toml",
            &format!("[resolution]\npolicy = \"{text}\"\n"),
        );
        let cfg = config::load_config_from_root(dir.path()).expect("policy parses");
        assert_eq!(cfg.resolution.policy, expected, "policy {text}");
    }
}

#[test]
fn unknown_resolution_policy_fails_loud() {
    // The fail-fast half of FR-CF-01: a typo'd policy is a parse error
    // (exit 2), never a silent fall-back to the default.
    let dir = tempdir().unwrap();
    write(
        dir.path(),
        ".logos/config.toml",
        "[resolution]\npolicy = \"agressive\"\n",
    );
    assert!(matches!(
        config::load_config_from_root(dir.path()),
        Err(ConfigError::Parse { .. })
    ));
}

// ── FR-GV-12 / CR-002: [[forbidden_imports]] parse + glob validation ─────────

#[test]
fn forbidden_imports_parse() {
    let dir = tempdir().unwrap();
    let path = write(
        dir.path(),
        "rules.toml",
        r#"
            [[forbidden_imports]]
            from   = "src/web/**"
            to     = "src/db/**"
            reason = "the web layer must not import the db directly"
        "#,
    );
    let rules = config::load_rules(&path).unwrap();
    assert_eq!(rules.forbidden_imports.len(), 1);
    assert_eq!(rules.forbidden_imports[0].from, "src/web/**");
    assert_eq!(rules.forbidden_imports[0].to, "src/db/**");
    assert!(rules.forbidden_imports[0].reason.is_some());
}

#[test]
fn forbidden_imports_reason_is_optional() {
    let dir = tempdir().unwrap();
    let path = write(
        dir.path(),
        "rules.toml",
        "[[forbidden_imports]]\nfrom = \"a/**\"\nto = \"b/**\"\n",
    );
    let rules = config::load_rules(&path).unwrap();
    assert_eq!(rules.forbidden_imports.len(), 1);
    assert!(rules.forbidden_imports[0].reason.is_none());
}

#[test]
fn forbidden_imports_unknown_key_fails_exit_2() {
    let dir = tempdir().unwrap();
    let path = write(
        dir.path(),
        "rules.toml",
        "[[forbidden_imports]]\nfrom = \"a/**\"\nto = \"b/**\"\nwhy = \"nope\"\n",
    );
    let err = config::load_rules(&path).unwrap_err();
    assert!(matches!(err, ConfigError::Parse { .. }));
    assert_eq!(err.exit_code(), 2);
}

#[test]
fn forbidden_imports_bad_glob_fails_exit_2() {
    // NFR-SE-04 / NFR-UX-02: an invalid glob fails at load with exit 2, before
    // any evaluation runs (FR-GV-12).
    let dir = tempdir().unwrap();
    let path = write(
        dir.path(),
        "rules.toml",
        "[[forbidden_imports]]\nfrom = \"src/{web\"\nto = \"src/db/**\"\n",
    );
    let err = config::load_rules(&path).unwrap_err();
    assert!(matches!(err, ConfigError::BadGlob { .. }));
    assert_eq!(err.exit_code(), 2);
    // NFR-UX-02: the message names the offending glob so it is actionable.
    assert!(
        err.to_string().contains("src/{web"),
        "error names the bad glob: {err}"
    );
}

#[test]
fn forbidden_imports_escaping_glob_fails_exit_2() {
    let dir = tempdir().unwrap();
    let path = write(
        dir.path(),
        "rules.toml",
        "[[forbidden_imports]]\nfrom = \"../escape/**\"\nto = \"src/db/**\"\n",
    );
    let err = config::load_rules(&path).unwrap_err();
    assert!(matches!(err, ConfigError::EscapingPattern { .. }));
    assert_eq!(err.exit_code(), 2);
    // NFR-UX-02: the message names the offending glob so it is actionable.
    assert!(
        err.to_string().contains("../escape/**"),
        "error names the escaping glob: {err}"
    );
}
