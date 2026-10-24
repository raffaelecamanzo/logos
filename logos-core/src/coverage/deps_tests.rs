//! Tests for the coverage dev-dependency addition ([S-749], [FR-CV-18],
//! [UAT-CV-07]): the manager read from the lockfile, the argv it runs, the
//! dependency consent, the refusals, and the tools that never need one.
//!
//! No real package manager runs. Each is a stub on the test-only search path
//! (pip: the project's `.venv/bin/python`) that records its working directory
//! and argv under the world's record directory and changes the manifest, the
//! lockfile and the installed packages as the real one would. The project is
//! a fixture copied out and committed, so `git status` shows exactly what the
//! manager changed.
//!
//! [S-749]: ../../../docs/planning/journal.md#s-749-a-coverage-run-adds-a-missing-coverage-dev-dependency-through-the-projects-own-package-manager
//! [FR-CV-18]: ../../../docs/specs/requirements/FR-CV-18.md
//! [UAT-CV-07]: ../../../docs/specs/requirements/UAT-CV-07.md

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use super::{copy_tree, fixture, install, outcome, write, write_executable, World, ENTRY, PLANTED};
use crate::config::{
    load_coverage_dependency_consent, record_coverage_dependency_consent, ConfigError, COVERAGE_DEPENDENCY_CONSENT_FILE,
};
use crate::coverage::{CoverageRunSummary, DependencyOutcome, DependencyRun, RunProgress, RunRequest, ToolOutcome};

/// Every package manager Logos runs from `PATH`.
const MANAGERS: &[&str] = &["npm", "pnpm", "yarn", "bun", "uv", "poetry"];

/// What a node manager stub does: install the provider at `3.2.4`, declare it
/// in `package.json`, and record it in the lockfile `$1` names.
const NODE_ADD: &str = r#"/bin/mkdir -p node_modules/@vitest/coverage-v8
printf '{"name": "@vitest/coverage-v8", "version": "3.2.4"}\n' > node_modules/@vitest/coverage-v8/package.json
printf '{"name": "calc", "private": true, "devDependencies": {"@vitest/coverage-v8": "^3.2.4", "vitest": "^3.2.4"}}\n' > package.json
printf '  @vitest/coverage-v8: 3.2.4\n' >> "$LOCK""#;

/// What a python manager stub does: install pytest-cov into the project
/// `.venv`, and — with `$LOCK` set — declare it and record it in the lock.
const PYTHON_ADD: &str = r#"/bin/mkdir -p .venv/lib/python3.12/site-packages/pytest_cov-5.0.0.dist-info
if [ -n "$LOCK" ]; then printf '# pytest-cov>=5.0.0\n' >> pyproject.toml; printf '# pytest-cov 5.0.0\n' >> "$LOCK"; fi"#;

impl World {
    /// `coverage run` over every tool, each report ingested.
    fn run_adding(&self, consent: bool, dependency_consent: bool) -> anyhow::Result<CoverageRunSummary> {
        self.run_with(&RunRequest { tool: None, consent, entry: ENTRY, dependency_consent }, |_| {})
    }

    /// A stub package manager at `path`, recording under `managers`, that runs
    /// `effect` with `$LOCK` set to `lock` and exits `exit`.
    fn manager_at(&self, path: &Path, lock: &str, effect: &str, exit: i32) {
        let body = format!(
            "#!/bin/sh\n{{ printf 'manager=%s\\n' \"$0\"; printf 'cwd=%s\\n' \"$(pwd -P)\"; for a in \"$@\"; do printf 'arg=%s\\n' \"$a\"; done; \
             printf -- '--\\n'; }} >> '{rec}'\nLOCK='{lock}'\n{effect}\nexit {exit}\n",
            rec = self.rec.join("managers").display(),
        );
        write_executable(path, &body);
    }

    /// Every manager on the search path, each a stub that would add the
    /// dependency its ecosystem needs.
    fn every_manager(&self) {
        for manager in MANAGERS {
            let effect = if matches!(*manager, "uv" | "poetry") { PYTHON_ADD } else { NODE_ADD };
            self.manager_at(&self.bin.join(manager), "", effect, 0);
        }
    }

    /// Each manager invocation: the manager's file name, then its argv.
    fn manager_calls(&self) -> Vec<Vec<String>> {
        self.invocations("managers")
            .into_iter()
            .map(|lines| {
                let manager = lines[0].strip_prefix("manager=").unwrap();
                let name = Path::new(manager).file_name().unwrap().to_string_lossy().into_owned();
                [name].into_iter().chain(lines.iter().filter_map(|l| l.strip_prefix("arg=").map(str::to_string))).collect()
            })
            .collect()
    }

    /// Keep what a package manager installs and what Logos writes out of the
    /// commit, as a real project would, and commit.
    fn commit_project(&self) {
        write(&self.root.join(".gitignore"), "node_modules/\n.venv/\n.logos/\ntarget/\nbuild/\n");
        self.commit();
    }

    /// The project's `git status --porcelain` lines.
    fn status(&self) -> Vec<String> {
        let out = self.git(&["status", "--porcelain"]);
        out.lines().map(str::to_string).collect()
    }

    /// Every file under the project and its bytes, `.git` aside.
    fn tree(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        let mut files = BTreeMap::new();
        let mut pending = vec![self.root.clone()];
        while let Some(dir) = pending.pop() {
            for entry in fs::read_dir(&dir).unwrap().flatten() {
                let path = entry.path();
                if entry.file_type().unwrap().is_dir() {
                    if entry.file_name() != ".git" {
                        pending.push(path);
                    }
                } else {
                    files.insert(path.strip_prefix(&self.root).unwrap().to_path_buf(), fs::read(&path).unwrap_or_default());
                }
            }
        }
        files
    }

    fn dependency_consent_recorded(&self) -> bool {
        self.user.join(COVERAGE_DEPENDENCY_CONSENT_FILE).exists()
    }
}

/// A vitest project whose vitest `version` is installed without a coverage
/// provider, with `overlay` over it.
fn vitest_project(overlay: &str, version: &str) -> World {
    let w = World::with_project("vitest");
    copy_tree(&fixture(&format!("deps/{overlay}")), &w.root);
    w.stub_at(&w.root.join("node_modules/.bin/vitest"), "vitest", "vitest", 0, "");
    w.node_package("vitest", version);
    w
}

/// A pytest project whose `.venv` holds pytest without pytest-cov, with
/// `overlay` over it.
fn pytest_project(overlay: Option<&str>) -> World {
    let w = World::with_project("pytest-cov");
    if let Some(overlay) = overlay {
        copy_tree(&fixture(&format!("deps/{overlay}")), &w.root);
    }
    w.stub_at(&w.root.join(".venv/bin/pytest"), "pytest-cov", "pytest-cov", 0, "");
    w
}

fn sorted(mut lines: Vec<String>) -> Vec<String> {
    lines.sort();
    lines
}

fn only_dependency(summary: &CoverageRunSummary) -> &DependencyRun {
    assert_eq!(summary.dependencies.len(), 1, "{:#?}", summary.dependencies);
    &summary.dependencies[0]
}

/// UAT-CV-07 step 2–3: pnpm adds the provider at the vitest major, the run
/// ingests coverage, and `git status` shows the manifest and lockfile change
/// uncommitted.
#[test]
fn pnpm_adds_the_provider_at_the_vitest_major_and_the_change_stays_uncommitted() {
    let w = vitest_project("pnpm-vitest", "3.2.4");
    w.every_manager();
    w.manager_at(&w.bin.join("pnpm"), "pnpm-lock.yaml", NODE_ADD, 0);
    w.commit_project();
    let head = w.git(&["rev-parse", "HEAD"]);

    let summary = w.run_adding(true, true).expect("the run");
    assert_eq!(w.manager_calls(), [["pnpm", "add", "-D", "@vitest/coverage-v8@3"]], "only pnpm ran, once");
    let cwd = &w.invocations("managers")[0][1];
    assert_eq!(cwd, &format!("cwd={}", w.root.display()), "in the project root");
    let dependency = only_dependency(&summary);
    assert_eq!(
        (dependency.tool, dependency.package.as_str(), dependency.manager, dependency.command.as_deref()),
        ("vitest", "@vitest/coverage-v8@3", Some("pnpm"), Some("pnpm add -D @vitest/coverage-v8@3"))
    );
    assert_eq!(dependency.files, ["package.json", "pnpm-lock.yaml"]);
    assert_eq!(dependency.outcome, DependencyOutcome::Added);
    let ToolOutcome::Ran { ingests, .. } = outcome(&summary, "vitest") else { panic!("{summary:#?}") };
    assert_eq!(ingests.len(), 1, "the run ingests coverage once the provider is there");
    assert_eq!(summary.consent_recorded, ["vitest"], "the execution consent covers the tool the addition enables");
    assert_eq!(summary.exit_code(), 0);

    assert_eq!(w.status(), [" M package.json", " M pnpm-lock.yaml"]);
    assert_eq!(w.git(&["rev-parse", "HEAD"]), head, "nothing is committed");
    let record = load_coverage_dependency_consent(Some(&w.user)).unwrap();
    assert!(record.covers(&w.root, "pnpm add -D @vitest/coverage-v8@3"), "the consent pins the command");
    let json = serde_json::to_value(&summary).unwrap();
    assert_eq!(
        json["dependencies"][0],
        serde_json::json!({
            "tool": "vitest",
            "package": "@vitest/coverage-v8@3",
            "manager": "pnpm",
            "command": "pnpm add -D @vitest/coverage-v8@3",
            "files": ["package.json", "pnpm-lock.yaml"],
            "outcome": "added",
        })
    );

    // Once the provider is installed, a later run needs nothing.
    let again = w.run_adding(false, false).expect("the second run");
    assert!(again.dependencies.is_empty(), "{:#?}", again.dependencies);
    assert_eq!(w.manager_calls().len(), 1);
    assert!(serde_json::to_value(&again).unwrap().get("dependencies").is_none(), "absent when there is none");
}

/// The pinned major is the installed vitest's, not a constant.
#[test]
fn the_provider_is_pinned_to_the_major_of_the_vitest_installed() {
    let w = vitest_project("pnpm-vitest", "2.1.9");
    w.manager_at(&w.bin.join("pnpm"), "pnpm-lock.yaml", NODE_ADD, 0);
    w.commit_project();
    let summary = w.run_adding(true, true).expect("the run");
    assert_eq!(w.manager_calls(), [["pnpm", "add", "-D", "@vitest/coverage-v8@2"]]);
    assert_eq!(only_dependency(&summary).command.as_deref(), Some("pnpm add -D @vitest/coverage-v8@2"));
}

/// npm, yarn and bun are each read from their own lockfile and run with
/// their own argv.
#[test]
fn npm_yarn_and_bun_each_add_the_provider_from_their_own_lockfile() {
    for (lock, manager, argv) in [
        ("package-lock.json", "npm", &["install", "--save-dev", "@vitest/coverage-v8@3"][..]),
        ("yarn.lock", "yarn", &["add", "-D", "@vitest/coverage-v8@3"]),
        ("bun.lock", "bun", &["add", "--dev", "@vitest/coverage-v8@3"]),
    ] {
        let w = vitest_project("no-lockfile", "3.2.4");
        write(&w.root.join(lock), "lock\n");
        w.every_manager();
        w.manager_at(&w.bin.join(manager), lock, NODE_ADD, 0);
        w.commit_project();
        let summary = w.run_adding(true, true).expect("the run");
        let expected: Vec<String> = [manager].iter().chain(argv).map(|s| s.to_string()).collect();
        assert_eq!(w.manager_calls(), [expected], "{lock}");
        assert_eq!(only_dependency(&summary).files, ["package.json", lock], "{lock}");
        assert!(matches!(outcome(&summary, "vitest"), ToolOutcome::Ran { .. }), "{lock}: {summary:#?}");
        assert_eq!(sorted(w.status()), sorted(vec![" M package.json".to_string(), format!(" M {lock}")]), "{lock}");
    }
}

/// A uv project gets pytest-cov by `uv add --dev`, a poetry project by
/// `poetry add --group dev`, and pytest-cov then runs.
#[test]
fn uv_and_poetry_add_pytest_cov_as_a_dev_dependency() {
    for (overlay, lock, argv) in [
        ("uv", "uv.lock", &["uv", "add", "--dev", "pytest-cov"][..]),
        ("poetry", "poetry.lock", &["poetry", "add", "--group", "dev", "pytest-cov"]),
    ] {
        let w = pytest_project(Some(overlay));
        w.every_manager();
        w.manager_at(&w.bin.join(argv[0]), lock, PYTHON_ADD, 0);
        w.commit_project();
        let head = w.git(&["rev-parse", "HEAD"]);
        let summary = w.run_adding(true, true).expect("the run");
        assert_eq!(w.manager_calls(), [argv], "{overlay}");
        let dependency = only_dependency(&summary);
        assert_eq!(
            (dependency.tool, dependency.package.as_str(), dependency.manager),
            ("pytest-cov", "pytest-cov", Some(argv[0])),
            "{overlay}"
        );
        assert_eq!(dependency.files, ["pyproject.toml", lock], "{overlay}");
        assert!(matches!(outcome(&summary, "pytest-cov"), ToolOutcome::Ran { .. }), "{overlay}: {summary:#?}");
        assert_eq!(sorted(w.status()), sorted(vec![" M pyproject.toml".to_string(), format!(" M {lock}")]), "{overlay}");
        assert_eq!(w.git(&["rev-parse", "HEAD"]), head, "{overlay}: nothing is committed");
    }
}

/// A project whose only Python lock is `requirements.txt` gets pytest-cov
/// installed by its virtualenv's pip; no tracked file changes.
#[test]
fn pip_installs_pytest_cov_into_the_project_virtualenv() {
    let w = pytest_project(None);
    write(&w.root.join("requirements.txt"), "pytest==8.3.2\n");
    w.every_manager();
    w.manager_at(&w.root.join(".venv/bin/python"), "", PYTHON_ADD, 0);
    w.commit_project();
    let summary = w.run_adding(true, true).expect("the run");
    assert_eq!(w.manager_calls(), [["python", "-m", "pip", "install", "pytest-cov"]]);
    let dependency = only_dependency(&summary);
    assert_eq!(dependency.command.as_deref(), Some(".venv/bin/python -m pip install pytest-cov"));
    assert_eq!(dependency.files, [".venv/"]);
    assert!(matches!(outcome(&summary, "pytest-cov"), ToolOutcome::Ran { .. }), "{summary:#?}");
    assert!(w.status().is_empty(), "{:?}", w.status());
}

/// Without the dependency consent nothing is installed: the run carries the
/// consent statement — the command, the files, the lifecycle scripts — and
/// the tool reads not installed.
#[test]
fn without_the_dependency_consent_nothing_is_installed_and_the_statement_is_returned() {
    let w = vitest_project("pnpm-vitest", "3.2.4");
    w.every_manager();
    w.commit_project();
    let before = w.tree();

    let summary = w.run_adding(true, false).expect("the run");
    assert!(w.manager_calls().is_empty(), "no package manager ran");
    let dependency = only_dependency(&summary);
    assert_eq!(dependency.command.as_deref(), Some("pnpm add -D @vitest/coverage-v8@3"));
    let DependencyOutcome::Pending { consent } = &dependency.outcome else { panic!("{dependency:#?}") };
    for said in [
        "vitest is installed without a coverage provider",
        "`pnpm add -D @vitest/coverage-v8@3`",
        "with pnpm, changing package.json, pnpm-lock.yaml",
        "Logos never commits",
        "may run dependency lifecycle scripts",
        "`logos coverage run --consent --dependency-consent`",
        "its execution consent for vitest",
        "an MCP client cannot",
    ] {
        assert!(consent.contains(said), "{said:?} in {consent}");
    }
    assert!(matches!(outcome(&summary, "vitest"), ToolOutcome::NotInstalled { .. }), "{summary:#?}");
    assert_eq!(summary.exit_code(), 0, "an offer is not a failure");
    assert!(!w.dependency_consent_recorded());
    let after = w.tree();
    let changed: Vec<_> = after.iter().filter(|(p, b)| before.get(*p) != Some(b)).map(|(p, _)| p).collect();
    assert!(changed.iter().all(|p| p.starts_with(".logos")), "only Logos's own state may change: {changed:?}");
    assert!(w.status().is_empty(), "{:?}", w.status());
}

/// A consent recorded earlier in a terminal covers a later run on any
/// surface — the job's run, which never records one, included — and only for
/// the exact command it named.
#[test]
fn a_recorded_dependency_consent_covers_a_later_run_for_that_command_only() {
    let w = vitest_project("pnpm-vitest", "3.2.4");
    w.manager_at(&w.bin.join("pnpm"), "pnpm-lock.yaml", NODE_ADD, 0);
    w.commit_project();
    record_coverage_dependency_consent(Some(&w.user), &w.root, &["pnpm add -D @vitest/coverage-v8@4"]).unwrap();
    let summary = w.run_adding(true, false).expect("the run");
    assert!(matches!(only_dependency(&summary).outcome, DependencyOutcome::Pending { .. }), "another major is asked for");
    assert!(w.manager_calls().is_empty());

    record_coverage_dependency_consent(Some(&w.user), &w.root, &["pnpm add -D @vitest/coverage-v8@3"]).unwrap();
    let engine = crate::Engine::start(&w.root).unwrap().with_scan_environment(w.env());
    // vitest will now start, so the job's run needs the execution consent for
    // it too — what the web dialog records before the job starts.
    let refused = engine.coverage_run_reporting(None, |_| {}).expect_err("the execution consent lacks vitest");
    assert!(format!("{refused:#}").contains("for vitest —"), "{refused:#}");
    assert!(w.manager_calls().is_empty(), "nothing is added before the execution consent");
    crate::config::record_coverage_consent(Some(&w.user), &w.root, &["vitest"], None).unwrap();
    let summary = engine.coverage_run_reporting(None, |_| {}).expect("the job's run");
    assert_eq!(only_dependency(&summary).outcome, DependencyOutcome::Added);
    assert_eq!(w.manager_calls(), [["pnpm", "add", "-D", "@vitest/coverage-v8@3"]]);
}

/// The job's run — what MCP `coverage_run` and the web page start — never
/// gives the dependency consent itself: without a recorded one it adds
/// nothing and records nothing, the execution consent given or not.
#[test]
fn the_jobs_run_never_gives_the_dependency_consent() {
    let w = vitest_project("pnpm-vitest", "3.2.4");
    w.every_manager();
    w.commit_project();
    crate::config::record_coverage_consent(Some(&w.user), &w.root, &["vitest"], None).unwrap();
    let engine = crate::Engine::start(&w.root).unwrap().with_scan_environment(w.env());
    let summary = engine.coverage_run_reporting(None, |_| {}).expect("the job's run");
    assert!(matches!(only_dependency(&summary).outcome, DependencyOutcome::Pending { .. }), "{summary:#?}");
    assert!(w.manager_calls().is_empty(), "no package manager ran");
    assert!(!w.dependency_consent_recorded());
    assert!(w.status().is_empty(), "{:?}", w.status());
}

/// The execution consent counts a tool whose dependency would be added: a
/// run without it refuses naming the tool, and nothing is added.
#[test]
fn the_execution_consent_is_asked_for_a_tool_the_addition_would_enable() {
    let w = vitest_project("pnpm-vitest", "3.2.4");
    w.every_manager();
    w.commit_project();
    let err = w.run_adding(false, true).expect_err("the execution consent is missing");
    assert_eq!(crate::error::exit_code(&err), 2);
    let Some(ConfigError::InvalidValue { message, .. }) = err.downcast_ref::<ConfigError>() else { panic!("{err:#}") };
    assert!(message.contains("for vitest —"), "{message}");
    assert!(w.manager_calls().is_empty());
    assert!(!w.dependency_consent_recorded(), "a refused run records no dependency consent");
}

/// A tool whose addition will not run in this run — no dependency consent,
/// or one Logos refuses — is not asked for in the execution consent: a
/// project consented before its vitest lost its provider is not refused.
#[test]
fn the_execution_consent_never_asks_for_a_tool_whose_addition_will_not_run() {
    for (overlay, dependency_consent) in [("pnpm-vitest", false), ("no-lockfile", true)] {
        let w = vitest_project(overlay, "3.2.4");
        w.every_manager();
        w.commit_project();
        let pending = crate::coverage::consent_pending(&w.root, &w.env(), None).expect("the pending consent");
        assert!(pending.is_empty(), "{overlay}: {pending:?}");
        let summary = w.run_adding(false, dependency_consent).expect(overlay);
        assert!(summary.consent_recorded.is_empty(), "{overlay}");
        assert!(
            matches!(
                only_dependency(&summary).outcome,
                DependencyOutcome::Pending { .. } | DependencyOutcome::Refused { .. }
            ),
            "{overlay}: {summary:#?}"
        );
        assert!(matches!(outcome(&summary, "vitest"), ToolOutcome::NotInstalled { .. }), "{overlay}");
        assert!(w.manager_calls().is_empty(), "{overlay}");
    }
}

/// No lockfile, a manager Logos does not run, two managers, an unknown
/// vitest major, a manager missing from `PATH`, a pip project with no
/// virtualenv interpreter: each is refused naming the command to run by
/// hand, starts nothing and changes no file — even with the consent given.
#[test]
fn an_addition_logos_will_not_choose_is_refused_naming_the_hand_command() {
    let vitest = |overlay: &str, version: &str, locks: &[&str]| {
        let w = vitest_project(overlay, version);
        for lock in locks {
            write(&w.root.join(lock), "lock\n");
        }
        w
    };
    let pytest = |files: &[&str]| {
        let w = pytest_project(None);
        for file in files {
            write(&w.root.join(file), "lock\n");
        }
        w
    };
    let cases: Vec<(&str, World, &str, &str)> = vec![
        ("no lockfile", vitest("no-lockfile", "3.2.4", &[]), "no lockfile names", "npm install --save-dev @vitest/coverage-v8@3"),
        ("unknown manager", vitest("no-lockfile", "3.2.4", &["deno.lock"]), "deno.lock is the lockfile of a package manager Logos does not run", "deno add --dev npm:@vitest/coverage-v8@3"),
        (
            "a known and an unknown manager",
            vitest("pnpm-vitest", "3.2.4", &["deno.lock"]),
            "pnpm-lock.yaml, deno.lock name more than one package manager",
            "pnpm add -D @vitest/coverage-v8@3 or deno add --dev npm:@vitest/coverage-v8@3",
        ),
        (
            "two managers",
            vitest("pnpm-vitest", "3.2.4", &["yarn.lock"]),
            "pnpm-lock.yaml, yarn.lock name more than one package manager",
            "pnpm add -D @vitest/coverage-v8@3 or yarn add -D @vitest/coverage-v8@3",
        ),
        ("unknown major", vitest("pnpm-vitest", "workspace:*", &[]), "will not guess the provider's major", "pnpm add -D @vitest/coverage-v8@<vitest major>"),
        ("python, no lockfile", pytest(&[]), "no lockfile names", "python -m pip install pytest-cov"),
        ("python, unknown manager", pytest(&["Pipfile.lock", "requirements.txt"]), "Pipfile.lock is the lockfile", "pipenv install --dev pytest-cov"),
        (
            "python, two unknown managers",
            pytest(&["pdm.lock", "Pipfile.lock"]),
            "pdm.lock, Pipfile.lock are lockfiles of package managers Logos does not run",
            "pdm add --dev pytest-cov or pipenv install --dev pytest-cov",
        ),
        ("pip, no virtualenv python", pytest(&["requirements.txt"]), "no project virtualenv (.venv/bin/python)", "python -m pip install pytest-cov"),
    ];
    for (case, w, reason_says, hand) in cases {
        w.every_manager();
        w.commit_project();
        let before = w.tree();
        let summary = w.run_adding(true, true).expect(case);
        let dependency = only_dependency(&summary);
        let DependencyOutcome::Refused { reason, hand_command } = &dependency.outcome else { panic!("{case}: {dependency:#?}") };
        assert!(reason.contains(reason_says), "{case}: {reason}");
        assert_eq!(hand_command, hand, "{case}");
        let named = match case {
            "unknown major" => Some("pnpm"),
            "pip, no virtualenv python" => Some("pip"),
            _ => None,
        };
        assert_eq!(dependency.manager, named, "{case}: the manager is named only when the lockfile names one");
        assert!(dependency.command.is_none() && dependency.files.is_empty(), "{case}: {dependency:#?}");
        assert!(w.manager_calls().is_empty(), "{case}: nothing is started");
        let after = w.tree();
        let changed: Vec<_> = after.iter().filter(|(p, b)| before.get(*p) != Some(b)).map(|(p, _)| p).collect();
        assert!(changed.iter().all(|p| p.starts_with(".logos")), "{case}: no project file changes: {changed:?}");
        assert!(w.status().is_empty(), "{case}: {:?}", w.status());
        assert!(!w.dependency_consent_recorded(), "{case}: a refusal records no consent");
        assert_eq!(summary.exit_code(), 1, "{case}: what the consent asked for did not happen");
    }

    // Without the consent the refusal is still reported, and is no failure.
    let w = vitest("no-lockfile", "3.2.4", &[]);
    let summary = w.run_adding(true, false).expect("the run");
    assert!(matches!(only_dependency(&summary).outcome, DependencyOutcome::Refused { .. }));
    assert_eq!(summary.exit_code(), 0);
}

/// A manager the lockfile names but `PATH` lacks is refused; one inside the
/// project is never used, even when `PATH` names the directory it is in.
#[test]
fn a_manager_missing_from_the_usable_path_is_refused() {
    let w = vitest_project("no-lockfile", "3.2.4");
    write(&w.root.join("bun.lock"), "lock\n");
    let inside = w.root.join("node_modules/.bin");
    w.manager_at(&inside.join("bun"), "bun.lock", NODE_ADD, 0);
    w.commit_project();
    let env = crate::security::ScanEnvironment {
        search_path: Some(std::env::join_paths([inside.clone(), w.bin.clone()]).unwrap()),
        ..w.env()
    };
    let request = RunRequest { tool: None, consent: true, entry: ENTRY, dependency_consent: true };
    let no_report = |_: &Path, _: Option<crate::history::coverage::CoverageFormat>, _: &crate::config::EffectiveCoverageIngest| {
        anyhow::bail!("no tool runs here")
    };
    let summary = crate::coverage::run(&w.root, &env, &request, |_| {}, no_report).expect("the run");
    let dependency = only_dependency(&summary);
    let DependencyOutcome::Refused { reason, hand_command } = &dependency.outcome else { panic!("{summary:#?}") };
    assert!(reason.contains("bun.lock names bun, but no bun is on the PATH"), "{reason}");
    assert_eq!(hand_command, "bun add --dev @vitest/coverage-v8@3");
    assert_eq!(dependency.manager, Some("bun"));
    assert!(w.manager_calls().is_empty(), "the project's own bun never ran");
}

/// jest, Maven and Gradle (JaCoCo) and `go test` never trigger a dependency
/// change — installed or not — with every manager on `PATH`, a lockfile there
/// and the consent given.
#[test]
fn jest_jacoco_and_go_projects_never_trigger_a_dependency_change() {
    for (project, installed) in [
        ("jest", true),
        ("jest", false),
        ("maven", true),
        ("maven", false),
        ("gradle", true),
        ("gradle", false),
        ("go", true),
        ("go", false),
    ] {
        let w = World::with_project(project);
        write(&w.root.join("package-lock.json"), "lock\n");
        write(&w.root.join("uv.lock"), "lock\n");
        if installed {
            install(&w, project, 0);
        }
        w.every_manager();
        w.commit_project();
        let summary = w.run_adding(true, true).expect(project);
        assert!(summary.dependencies.is_empty(), "{project} ({installed}): {:#?}", summary.dependencies);
        assert!(w.manager_calls().is_empty(), "{project} ({installed})");
        assert!(w.status().is_empty(), "{project} ({installed}): {:?}", w.status());
        assert!(!w.dependency_consent_recorded());
    }
}

/// A pytest on `PATH` without pytest-cov is not the project's: it is
/// reported, and nothing is added to the project for it — even with a
/// lockfile, every manager on `PATH` and the consent given.
#[test]
fn a_pytest_on_path_without_pytest_cov_adds_nothing_to_the_project() {
    let w = World::with_project("pytest-cov");
    copy_tree(&fixture("deps/uv"), &w.root);
    write_executable(
        &w.bin.join("pytest"),
        "#!/bin/sh\nprintf 'This is pytest version 8.3.2\\nregistered third-party plugins:\\n  pytest-xdist-3.6.1 at /x\\n'\n",
    );
    w.every_manager();
    w.commit_project();
    let summary = w.run_adding(true, true).expect("the run");
    assert!(summary.dependencies.is_empty(), "{:#?}", summary.dependencies);
    assert!(w.manager_calls().is_empty());
    assert!(matches!(outcome(&summary, "pytest-cov"), ToolOutcome::NotInstalled { hint: Some(_) }), "{summary:#?}");
}

/// A manager that fails is the dependency's `failed` outcome: its whole
/// output in a log, a masked excerpt in the payload, the run exit 1.
#[test]
fn a_failing_package_manager_is_reported_and_its_secret_stays_in_the_log() {
    let w = vitest_project("pnpm-vitest", "3.2.4");
    w.manager_at(&w.bin.join("pnpm"), "pnpm-lock.yaml", &format!("echo 'error: token={PLANTED} rejected' >&2"), 1);
    w.commit_project();
    let summary = w.run_adding(true, true).expect("the run");
    let DependencyOutcome::Failed { exit_code, stderr, log } = &only_dependency(&summary).outcome else { panic!("{summary:#?}") };
    assert_eq!(*exit_code, Some(1));
    assert!(stderr.starts_with("error: token=") && stderr.contains(crate::history::security::mask::MASK), "{stderr}");
    assert_eq!(log.as_deref(), Some(".logos/coverage/vitest.dependency.failed.log"));
    let logged = fs::read_to_string(w.root.join(".logos/coverage/vitest.dependency.failed.log")).unwrap();
    assert!(logged.contains(PLANTED), "the log keeps the whole output");
    assert!(!serde_json::to_string(&summary).unwrap().contains(PLANTED), "the secret reached the payload");
    assert!(matches!(outcome(&summary, "vitest"), ToolOutcome::NotInstalled { .. }));
    assert_eq!(summary.exit_code(), 1);

    // A later addition that succeeds clears the log, and the secret with it.
    w.manager_at(&w.bin.join("pnpm"), "pnpm-lock.yaml", NODE_ADD, 0);
    let summary = w.run_adding(true, true).expect("the second run");
    assert_eq!(only_dependency(&summary).outcome, DependencyOutcome::Added);
    assert!(!w.root.join(".logos/coverage/vitest.dependency.failed.log").exists(), "the stale log is cleared");
}

/// S-750: an addition that changed the project and then failed puts back
/// what it changed — `package.json` and the lockfile byte for byte, and the
/// provider package it installed — and names each in `restored`; the project
/// reads as it did before, so nothing is half applied.
#[test]
fn a_failed_addition_puts_back_the_files_it_changed() {
    let w = vitest_project("pnpm-vitest", "3.2.4");
    w.manager_at(&w.bin.join("pnpm"), "pnpm-lock.yaml", &format!("{NODE_ADD}\necho 'ERR_PNPM_FETCH_404' >&2"), 1);
    w.commit_project();
    let before = w.tree();

    let summary = w.run_adding(true, true).expect("the run");
    let dependency = only_dependency(&summary);
    assert!(matches!(dependency.outcome, DependencyOutcome::Failed { exit_code: Some(1), .. }), "{dependency:#?}");
    assert_eq!(dependency.restored, ["package.json", "pnpm-lock.yaml", "node_modules/@vitest/coverage-v8"]);
    assert!(w.status().is_empty(), "{:?}", w.status());
    let after = w.tree();
    let changed: Vec<&PathBuf> = after.keys().filter(|p| !p.starts_with(".logos") && before.get(*p) != after.get(*p)).collect();
    assert!(changed.is_empty(), "the project changed: {changed:?}");
    assert!(matches!(outcome(&summary, "vitest"), ToolOutcome::NotInstalled { .. }), "the provider is gone again");
    let json = serde_json::to_value(dependency).unwrap();
    assert_eq!(json["restored"], serde_json::json!(["package.json", "pnpm-lock.yaml", "node_modules/@vitest/coverage-v8"]));

    // An addition that ends added puts nothing back, and says nothing of it.
    w.manager_at(&w.bin.join("pnpm"), "pnpm-lock.yaml", NODE_ADD, 0);
    let summary = w.run_adding(true, true).expect("the second run");
    assert!(only_dependency(&summary).restored.is_empty());
    assert_eq!(w.status(), [" M package.json", " M pnpm-lock.yaml"]);
}

/// S-750, a review finding: a failed uv addition of pytest-cov puts back
/// `pyproject.toml` and `uv.lock`, and removes pytest-cov's own package,
/// dist-info and `.pth` hook from the project `.venv` — so the pytest-cov row
/// reads not installed again — but leaves a package the manager pulled in
/// beside it, and one whose name only starts like pytest-cov's.
#[test]
fn a_failed_pytest_cov_addition_removes_what_it_installed_in_the_virtualenv() {
    let w = pytest_project(Some("uv"));
    let site = ".venv/lib/python3.12/site-packages";
    let installed = format!(
        "{PYTHON_ADD}\n/bin/mkdir -p {site}/pytest_cov {site}/coverage {site}/pytest_cover-1.0.dist-info\n: > {site}/pytest-cov.pth\nexit 1"
    );
    w.manager_at(&w.bin.join("uv"), "uv.lock", &installed, 1);
    w.commit_project();
    let before = w.tree();

    let summary = w.run_adding(true, true).expect("the run");
    let dependency = only_dependency(&summary);
    assert!(matches!(dependency.outcome, DependencyOutcome::Failed { .. }), "{dependency:#?}");
    assert_eq!(
        dependency.restored,
        [
            "pyproject.toml".to_string(),
            "uv.lock".to_string(),
            format!("{site}/pytest-cov.pth"),
            format!("{site}/pytest_cov"),
            format!("{site}/pytest_cov-5.0.0.dist-info"),
        ]
    );
    assert!(w.status().is_empty(), "{:?}", w.status());
    for (path, bytes) in &before {
        assert_eq!(fs::read(w.root.join(path)).ok().as_ref(), Some(bytes), "{} changed", path.display());
    }
    for left in ["coverage", "pytest_cover-1.0.dist-info"] {
        assert!(w.root.join(site).join(left).exists(), "{left} is not pytest-cov's and stays");
    }
    let again = w.run_adding(false, false).expect("the second run");
    assert!(matches!(outcome(&again, "pytest-cov"), ToolOutcome::NotInstalled { .. }), "the plugin reads missing again");
}

/// S-750, review: a file the consent named that was not there before the
/// addition, and that the manager created before it failed, is removed and
/// named in `restored`.
#[test]
fn a_failed_addition_removes_a_named_file_it_created() {
    let w = pytest_project(Some("uv"));
    fs::remove_file(w.root.join("pyproject.toml")).unwrap();
    write(&w.root.join("pytest.ini"), "[pytest]\n");
    w.manager_at(&w.bin.join("uv"), "uv.lock", PYTHON_ADD, 1);
    w.commit_project();

    let summary = w.run_adding(true, true).expect("the run");
    let dependency = only_dependency(&summary);
    assert_eq!(dependency.files, ["pyproject.toml", "uv.lock"]);
    assert!(dependency.restored.starts_with(&["pyproject.toml".to_string(), "uv.lock".to_string()]), "{dependency:#?}");
    assert!(!w.root.join("pyproject.toml").exists(), "the file the addition created is gone");
    assert!(w.status().is_empty(), "{:?}", w.status());
}

/// S-750, review: a manifest that is a symlink is never kept nor put back —
/// Logos neither writes through it nor replaces it with a regular file; the
/// link stays a link, and is not named in `restored`.
#[test]
fn a_symlinked_manifest_is_never_written_through_or_over() {
    let w = pytest_project(Some("uv"));
    let outside = tempfile::TempDir::new().unwrap();
    let target = outside.path().join("pyproject.toml");
    fs::rename(w.root.join("pyproject.toml"), &target).unwrap();
    std::os::unix::fs::symlink(&target, w.root.join("pyproject.toml")).unwrap();
    write(&w.root.join("pytest.ini"), "[pytest]\n");
    w.manager_at(&w.bin.join("uv"), "uv.lock", PYTHON_ADD, 1);
    w.commit_project();
    let changed_by_the_manager = || fs::read(&target).unwrap();

    let summary = w.run_adding(true, true).expect("the run");
    let dependency = only_dependency(&summary);
    assert!(matches!(dependency.outcome, DependencyOutcome::Failed { .. }), "{dependency:#?}");
    assert!(!dependency.restored.contains(&"pyproject.toml".to_string()), "{dependency:#?}");
    assert!(dependency.restored.contains(&"uv.lock".to_string()), "the regular lockfile is still put back");
    let link = fs::symlink_metadata(w.root.join("pyproject.toml")).unwrap();
    assert!(link.file_type().is_symlink(), "the symlink was replaced by a regular file");
    assert!(String::from_utf8(changed_by_the_manager()).unwrap().contains("pytest-cov"), "Logos wrote through the link");
}

/// S-750, review: a run reports a dependency step only when it starts one —
/// an addition awaiting its consent, or refused, sends no `dependency_*`
/// event, so the page never shows one going.
#[test]
fn an_addition_that_does_not_start_reports_no_progress() {
    let w = vitest_project("pnpm-vitest", "3.2.4");
    w.manager_at(&w.bin.join("pnpm"), "pnpm-lock.yaml", NODE_ADD, 0);
    w.commit_project();
    let mut steps = Vec::new();
    let request = RunRequest { tool: None, consent: true, entry: ENTRY, dependency_consent: false };
    let summary = w.run_with(&request, |step| steps.push(step.clone())).expect("the run");
    assert!(matches!(only_dependency(&summary).outcome, DependencyOutcome::Pending { .. }), "{summary:#?}");
    let dependency_steps: Vec<_> = steps
        .iter()
        .filter(|s| matches!(s, RunProgress::DependencyStarted { .. } | RunProgress::DependencyFinished(_)))
        .collect();
    assert!(dependency_steps.is_empty(), "{dependency_steps:?}");
    assert!(w.manager_calls().is_empty());
}

/// A manager that cannot start is `failed`, never `added`.
#[test]
fn a_manager_that_cannot_start_is_a_failed_addition() {
    let w = vitest_project("pnpm-vitest", "3.2.4");
    write_executable(&w.bin.join("pnpm"), "#!/nonexistent/interpreter\n");
    w.commit_project();
    let summary = w.run_adding(true, true).expect("the run");
    let DependencyOutcome::Failed { exit_code, stderr, log } = &only_dependency(&summary).outcome else { panic!("{summary:#?}") };
    assert_eq!(*exit_code, None);
    assert!(stderr.starts_with("could not start "), "{stderr}");
    assert_eq!(log.as_deref(), Some(".logos/coverage/vitest.dependency.failed.log"));
    assert!(matches!(outcome(&summary, "vitest"), ToolOutcome::NotInstalled { .. }));
    assert_eq!(summary.exit_code(), 1);
}

/// A run that finds no missing dependency reads no dependency consent
/// record: one Logos cannot parse never fails a run that would not use it.
#[test]
fn a_run_with_nothing_to_add_never_reads_the_dependency_consent() {
    let w = World::with_project("jest");
    install(&w, "jest", 0);
    w.commit_project();
    write(&w.user.join(COVERAGE_DEPENDENCY_CONSENT_FILE), "not a consent record Logos wrote\n");
    let summary = w.run_adding(true, true).expect("the run does not read the record");
    assert!(summary.dependencies.is_empty());
    assert!(matches!(outcome(&summary, "jest"), ToolOutcome::Ran { .. }), "{summary:#?}");
}

/// A run cancelled before its package manager starts never starts it: the
/// addition reads `cancelled`, nothing in the project changes, and the run
/// exits 1.
#[test]
fn a_cancelled_run_starts_no_package_manager() {
    let w = vitest_project("pnpm-vitest", "3.2.4");
    w.manager_at(&w.bin.join("pnpm"), "pnpm-lock.yaml", NODE_ADD, 0);
    w.commit_project();
    let summary = crate::jobs::foreground(|| {
        crate::jobs::this_run().expect("a foreground run").cancel();
        w.run_adding(true, true).expect("the run")
    });
    assert_eq!(only_dependency(&summary).outcome, DependencyOutcome::Cancelled);
    assert!(w.manager_calls().is_empty(), "no package manager started");
    assert!(w.status().is_empty(), "{:?}", w.status());
    assert_eq!(summary.exit_code(), 1);
}

/// Sprint 108 review (S-736 × S-749): the run starts only the tools its
/// execution consent was checked for. Here the project declares jest but has
/// not installed it, so the consent the run records names vitest alone — and
/// the package manager, adding vitest's provider, installs jest as well. jest
/// is then runnable, but no consent names it: it is not started, and the next
/// run asks for it.
#[test]
fn a_tool_the_addition_happens_to_install_is_not_started_without_its_consent() {
    let w = vitest_project("pnpm-vitest", "3.2.4");
    let both = r#"{"name": "calc", "private": true, "devDependencies": {"@vitest/coverage-v8": "^3.2.4", "jest": "^29.7.0", "vitest": "^3.2.4"}}"#;
    write(&w.root.join("package.json"), &both.replace(r#""@vitest/coverage-v8": "^3.2.4", "#, ""));
    let parked = w.rec.join("jest-to-install");
    w.stub_at(&parked, "jest", "jest", 0, "");
    let effect = format!(
        "{NODE_ADD}\nprintf '%s\\n' '{both}' > package.json\n/bin/mkdir -p node_modules/.bin node_modules/jest\n\
         /bin/cp '{parked}' node_modules/.bin/jest\nprintf '{{\"name\": \"jest\", \"version\": \"29.7.0\"}}\\n' > node_modules/jest/package.json",
        parked = parked.display()
    );
    w.every_manager();
    w.manager_at(&w.bin.join("pnpm"), "pnpm-lock.yaml", &effect, 0);
    w.commit_project();
    assert!(!w.root.join("node_modules/.bin/jest").exists(), "jest is declared, not installed");

    let summary = w.run_adding(true, true).expect("the run");
    assert_eq!(only_dependency(&summary).outcome, DependencyOutcome::Added);
    assert_eq!(summary.consent_recorded, ["vitest"], "the consent names what the run was checked for");
    assert!(w.root.join("node_modules/.bin/jest").exists(), "the addition installed jest");
    assert_eq!(w.argvs("jest"), Vec::<Vec<String>>::new(), "jest never started: {summary:#?}");
    assert!(matches!(outcome(&summary, "jest"), ToolOutcome::NotInstalled { .. }), "{summary:#?}");

    let pending = crate::coverage::consent_pending(&w.root, &w.env(), None).expect("the pending consent");
    assert!(!pending.is_empty(), "the next run asks for jest");
}
