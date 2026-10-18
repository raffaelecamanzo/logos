//! The composition root composes an extension's CLI subcommand, init step,
//! telemetry tool and config table, and only while the extension is active
//! (S-639, S-640, [FR-DS-04], [FR-CF-09], [ADR-74], [ADR-75]).
//!
//! Each test runs a **composed** `logos` — `logos_cli::run_from` over a host
//! built here — as a child process, so its stdout, stderr and exit code are
//! observed exactly as a user of a binary composing that host would see them.
//! The child is this test binary re-run on [`composed_logos_child`], the
//! technique that gives a library entry point a real process boundary without a
//! second, test-only binary target in the shipped package.
//!
//! Five hosts: `stub` (the stub extension, `logos_ext::testing`), `disabled`
//! (the same stub behind a feature the gate does not enable), `enabled` (that
//! gated stub with its feature enabled), `skill-reader` (an extension whose init
//! step reports the skill answer, [`SkillReader`]) and `public` (no extension —
//! the host of a `main` that composes none).
//!
//! [FR-DS-04]: ../../docs/specs/requirements/FR-DS-04.md
//! [FR-CF-09]: ../../docs/specs/requirements/FR-CF-09.md
//! [ADR-74]: ../../docs/specs/architecture/decisions/ADR-74.md
//! [ADR-75]: ../../docs/specs/architecture/decisions/ADR-75.md

use std::path::Path;
use std::process::Command;

use logos_ext::testing::{
    StubExtension, TestGate, STUB_CONFIG_TABLE, STUB_FEATURE, STUB_INIT_TARGET,
    STUB_SUBCOMMAND, STUB_TELEMETRY,
};
use logos_ext::ExtensionHost;
use serde_json::Value;
use tempfile::TempDir;

/// The env vars that turn [`composed_logos_child`] into a composed `logos`.
const HOST_VAR: &str = "LOGOS_TEST_COMPOSED_HOST";
const ARGS_VAR: &str = "LOGOS_TEST_COMPOSED_ARGS";

fn host(kind: &str) -> ExtensionHost {
    match kind {
        "stub" => ExtensionHost::new().register(StubExtension::new()).expect("registers"),
        "disabled" => ExtensionHost::new()
            .register(StubExtension::gated(STUB_FEATURE))
            .expect("registers")
            .with_gate(TestGate::enabling(&["another-feature"])),
        "enabled" => ExtensionHost::new()
            .register(StubExtension::gated(STUB_FEATURE))
            .expect("registers")
            .with_gate(TestGate::enabling(&[STUB_FEATURE])),
        "skill-reader" => ExtensionHost::new().register(SkillReader).expect("registers"),
        "public" => ExtensionHost::new(),
        other => panic!("test bug: unknown host {other}"),
    }
}

/// Not a check of its own: run as a child by [`logos`], it IS a composed `logos`
/// and exits with its code. Run by the harness, the variables are unset and it
/// returns at once.
#[test]
fn composed_logos_child() {
    let (Ok(kind), Ok(args)) = (std::env::var(HOST_VAR), std::env::var(ARGS_VAR)) else {
        return;
    };
    let args: Vec<String> = serde_json::from_str(&args).expect("the child's arguments");
    std::process::exit(logos_cli::run_from(&host(&kind), args));
}

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Run {
    /// The one JSON document a `--json` run prints. The harness writes its own
    /// banner first — `test composed_logos_child ... ` on the same line — which
    /// holds no `{`.
    fn json(&self) -> Value {
        let line = self
            .stdout
            .lines()
            .find_map(|l| l.find('{').map(|at| &l[at..]))
            .unwrap_or_else(|| panic!("no JSON document on stdout:\n{}", self.stdout));
        serde_json::from_str(line).expect("a JSON document")
    }
}

/// Run `logos --project <project> <args…>` composed with host `kind`, with the
/// user's Claude Code directory at a path that does not exist, so a developer's
/// own plugin enrolment never makes `init -i` here plugin-aware (FR-IN-12).
fn logos(kind: &str, project: &Path, args: &[&str]) -> Run {
    let mut argv = vec!["logos".to_string(), "--project".to_string(), project.display().to_string()];
    argv.extend(args.iter().map(|a| a.to_string()));
    let out = Command::new(std::env::current_exe().expect("this test binary"))
        .args(["composed_logos_child", "--exact", "--nocapture", "--test-threads=1"])
        .env(HOST_VAR, kind)
        .env(ARGS_VAR, serde_json::to_string(&argv).expect("serialisable"))
        .env("CLAUDE_CONFIG_DIR", project.join(".no-claude-config"))
        .output()
        .expect("the child runs");
    Run {
        code: out.status.code().expect("the child exits"),
        stdout: String::from_utf8(out.stdout).expect("UTF-8 stdout"),
        stderr: String::from_utf8(out.stderr).expect("UTF-8 stderr"),
    }
}

fn subcommands(kind: &str) -> Vec<String> {
    logos_cli::command(&host(kind)).get_subcommands().map(|c| c.get_name().to_string()).collect()
}

#[test]
fn a_registered_extension_subcommand_runs_over_the_core_root_and_output_flags() {
    let project = TempDir::new().expect("a temp project");

    let help = logos_cli::command(&host("stub")).render_help().to_string();
    assert!(help.contains(STUB_SUBCOMMAND), "`logos --help` lists the subcommand:\n{help}");
    let mut core = subcommands("public");
    core.push(STUB_SUBCOMMAND.to_string());
    assert_eq!(subcommands("stub"), core, "added beside every core subcommand");

    // The global flags reach it wherever they are given.
    for args in [
        &["--json", STUB_SUBCOMMAND, "--echo", "hi"][..],
        &[STUB_SUBCOMMAND, "--echo", "hi", "--json"][..],
    ] {
        let run = logos("stub", project.path(), args);
        assert_eq!(run.code, 0, "{args:?}: {}", run.stderr);
        let answer = run.json();
        assert_eq!(answer["extension"], "stub");
        assert_eq!(answer["echo"], "hi");
        let root = answer["root"].as_str().expect("the root it ran over");
        let name = project.path().file_name().expect("a name").to_str().expect("UTF-8");
        assert!(root.ends_with(name), "--project reaches the extension: {root}");
    }

    // With neither flag it prints the human rendering, as a core subcommand does
    // — the pretty form, which `--json`'s compact form never is.
    let run = logos("stub", project.path(), &[STUB_SUBCOMMAND, "--echo", "plain"]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert!(run.stdout.contains("\"echo\": \"plain\""), "pretty output:\n{}", run.stdout);

    // The core subcommands still route to the core.
    let run = logos("stub", project.path(), &["--json", "languages"]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert!(run.json().get("extension").is_none());
}

#[test]
fn an_unregistered_or_disabled_extension_adds_no_subcommand() {
    let project = TempDir::new().expect("a temp project");
    for kind in ["public", "disabled"] {
        assert!(!subcommands(kind).contains(&STUB_SUBCOMMAND.to_string()), "{kind}");
        let run = logos(kind, project.path(), &[STUB_SUBCOMMAND]);
        assert_eq!(run.code, 2, "{kind}: a usage error, as for any unknown verb");
        assert!(run.stderr.contains("unrecognized subcommand"), "{kind}: {}", run.stderr);
    }

    // The same gated stub with its feature enabled adds the subcommand and runs
    // it, so the refusals above are the gate's doing.
    assert!(subcommands("enabled").contains(&STUB_SUBCOMMAND.to_string()));
    let run = logos("enabled", project.path(), &["--json", STUB_SUBCOMMAND]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.json()["extension"], "stub");
}

fn init_targets(run: &Run) -> Vec<(String, String)> {
    run.json()["steps"]
        .as_array()
        .expect("the init report's steps")
        .iter()
        .map(|s| (s["target"].as_str().unwrap().to_string(), s["action"].as_str().unwrap().to_string()))
        .collect()
}

#[test]
fn a_registered_extension_init_step_runs_with_logos_init() {
    let project = TempDir::new().expect("a temp project");
    let run = logos("stub", project.path(), &["--json", "init"]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    let steps = init_targets(&run);
    assert_eq!(
        steps.last(),
        Some(&(STUB_INIT_TARGET.to_string(), "created".to_string())),
        "after the core steps: {steps:?}"
    );
    assert!(project.path().join(STUB_INIT_TARGET).is_file());

    // Idempotent like the core steps: a re-run reports, and writes, nothing new.
    let rerun = init_targets(&logos("stub", project.path(), &["--json", "init"]));
    assert_eq!(rerun.last(), Some(&(STUB_INIT_TARGET.to_string(), "unchanged".to_string())));

    for kind in ["public", "disabled"] {
        let fresh = TempDir::new().expect("a temp project");
        let run = logos(kind, fresh.path(), &["--json", "init"]);
        assert_eq!(run.code, 0, "{kind}: {}", run.stderr);
        assert!(init_targets(&run).iter().all(|(t, _)| t != STUB_INIT_TARGET), "{kind}");
        assert!(!fresh.path().join(STUB_INIT_TARGET).exists(), "{kind}: no file");
    }

    // The same gated stub with its feature enabled runs its step.
    let fresh = TempDir::new().expect("a temp project");
    let run = logos("enabled", fresh.path(), &["--json", "init"]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(init_targets(&run).last(), Some(&(STUB_INIT_TARGET.to_string(), "created".to_string())));
}

/// An extension whose one init step reads [`InitOptions::materialize_skill`] — the
/// answer to the skill question — and reports it, so a test sees what `init`
/// decided without a TTY.
///
/// [`InitOptions::materialize_skill`]: logos_core::init::InitOptions::materialize_skill
struct SkillReader;

/// The init-step target [`SkillReader`] reports under.
const SKILL_READER_TARGET: &str = "skill-reader";

impl logos_ext::Extension for SkillReader {
    fn id(&self) -> &'static str {
        "skill-reader"
    }

    fn init_steps(&self) -> Vec<std::sync::Arc<dyn logos_ext::InitContribution>> {
        vec![std::sync::Arc::new(SkillReader)]
    }
}

impl logos_ext::InitContribution for SkillReader {
    fn run(
        &self,
        _root: &Path,
        options: &logos_core::init::InitOptions,
    ) -> anyhow::Result<Vec<logos_core::models::pipeline::InitStep>> {
        Ok(vec![logos_core::models::pipeline::InitStep {
            target: SKILL_READER_TARGET.to_string(),
            action: logos_core::models::pipeline::InitAction::Unchanged,
            detail: format!("materialize_skill={}", options.materialize_skill),
        }])
    }
}

/// `init -i` asks whether to materialize skills only for a composed init step —
/// the only reader of the answer — and a non-TTY run takes the question's
/// default, yes; a plain `init` answers no. The build that composes no step asks
/// nothing (`no_extension_surface.rs`, `init_asks_no_chat_or_wiki_question`).
#[test]
fn a_composed_init_step_is_asked_the_skill_question() {
    let reported = |args: &[&str]| -> String {
        let project = TempDir::new().expect("a temp project");
        let run = logos("skill-reader", project.path(), args);
        assert_eq!(run.code, 0, "{args:?}: {}", run.stderr);
        let report = run.json();
        let step = report["steps"]
            .as_array()
            .expect("the init report's steps")
            .iter()
            .find(|s| s["target"] == SKILL_READER_TARGET)
            .unwrap_or_else(|| panic!("{args:?}: the composed step ran: {report}"))
            .clone();
        step["detail"].as_str().expect("a detail").to_string()
    };
    assert_eq!(reported(&["--json", "init", "-i"]), "materialize_skill=true", "`-i` asks, and non-TTY says yes");
    assert_eq!(reported(&["--json", "init"]), "materialize_skill=false", "a plain `init` does not ask");
}

/// A project whose `config.toml` holds `table_body` as the stub's table.
fn project_with_stub_table(table_body: &str) -> TempDir {
    let project = TempDir::new().expect("a temp project");
    let logos = project.path().join(".logos");
    std::fs::create_dir_all(&logos).expect("create .logos");
    std::fs::write(
        logos.join("config.toml"),
        format!("max_file_size = 4096\n\n[{STUB_CONFIG_TABLE}]\n{table_body}"),
    )
    .expect("write the config");
    project
}

#[test]
fn a_registered_extension_owns_its_config_table_and_validates_it() {
    // A load that reads config.toml: `index` over an empty project.
    let ok = project_with_stub_table("greeting = \"hi\"\n");
    let run = logos("stub", ok.path(), &["--json", "index"]);
    assert_eq!(run.code, 0, "the stub owns [stub]: {}", run.stderr);

    let bad = project_with_stub_table("greting = \"typo\"\n");
    let run = logos("stub", bad.path(), &["--json", "index"]);
    assert_eq!(run.code, 2, "a config error: {}", run.stderr);
    assert!(
        run.stderr.contains("invalid value for stub: unknown field `greting`"),
        "validated by the stub: {}",
        run.stderr
    );

    // Unregistered, or registered with its feature disabled, nothing owns [stub]:
    // the core's own error, as it always read.
    for kind in ["public", "disabled"] {
        let run = logos(kind, ok.path(), &["--json", "index"]);
        assert_eq!(run.code, 2, "{kind}: {}", run.stderr);
        assert!(
            run.stderr.contains("TOML parse error at line 3, column 2")
                && run.stderr.contains("unknown field `stub`, expected one of `languages`"),
            "{kind}: {}",
            run.stderr
        );
    }
}

/// The `calls_by_tool` row `logos stats` reports for the stub's tool, if any.
fn stub_usage(kind: &str, project: &Path) -> Option<Value> {
    let run = logos(kind, project, &["--json", "stats"]);
    assert_eq!(run.code, 0, "{kind}: {}", run.stderr);
    run.json()["calls_by_tool"]
        .as_array()
        .expect("the per-tool usage")
        .iter()
        .find(|row| row["tool"] == STUB_TELEMETRY.name())
        .cloned()
}

#[test]
fn a_registered_extension_reports_under_its_declared_telemetry_tool() {
    let project = TempDir::new().expect("a temp project");
    assert_eq!(logos("public", project.path(), &["init"]).code, 0, "a `.logos/` to record into");
    assert_eq!(stub_usage("stub", project.path()), None, "nothing recorded yet");

    assert_eq!(logos("stub", project.path(), &[STUB_SUBCOMMAND]).code, 0);
    let row = stub_usage("stub", project.path()).expect("the call is in the stats read-model");
    assert_eq!(row["surface"], "cli");
    assert_eq!(row["calls"], 1);
    assert_eq!(row["class"], STUB_TELEMETRY.class().as_str(), "classified as declared");

    // `--quiet` suppresses the rendering, never the call: it prints nothing, and
    // the call is recorded all the same.
    let quiet = logos("stub", project.path(), &["--quiet", STUB_SUBCOMMAND]);
    assert_eq!(quiet.code, 0, "{}", quiet.stderr);
    assert!(!quiet.stdout.contains("\"extension\""), "nothing printed:\n{}", quiet.stdout);
    let row = stub_usage("stub", project.path()).expect("still in the read-model");
    assert_eq!(row["calls"], 2, "the quiet call ran");

    // A build without the extension still reports the recorded call — history is
    // never rewritten — but cannot classify a tool it does not compose.
    let row = stub_usage("public", project.path()).expect("history is kept");
    assert_eq!(row["class"], "unregistered");
    // Nor can a build composing the stub with its feature disabled: an inactive
    // extension registers no telemetry tool.
    let row = stub_usage("disabled", project.path()).expect("history is kept");
    assert_eq!(row["class"], "unregistered", "a disabled extension classifies nothing");
    // The same gated stub with its feature enabled registers its tool, and so
    // classifies the recorded calls as declared.
    let row = stub_usage("enabled", project.path()).expect("history is kept");
    assert_eq!(row["class"], STUB_TELEMETRY.class().as_str(), "an enabled gated extension classifies");

    // A disabled stub records nothing: it has no subcommand to run.
    let other = TempDir::new().expect("a temp project");
    assert_eq!(logos("public", other.path(), &["init"]).code, 0);
    assert_eq!(logos("disabled", other.path(), &[STUB_SUBCOMMAND]).code, 2);
    assert_eq!(stub_usage("disabled", other.path()), None);
}

/// An extension adding the subcommand `command` builds, described as `about`.
struct Verb {
    id: &'static str,
    command: fn() -> clap::Command,
}

impl logos_ext::Extension for Verb {
    fn id(&self) -> &'static str {
        self.id
    }

    fn cli_command(&self) -> Option<logos_ext::cli::CliCommand> {
        Some(logos_ext::cli::CliCommand::new((self.command)(), |_, _| Ok(0)))
    }
}

/// Every subcommand the composed `logos` answers to, built — so clap's own
/// `help` is listed — with each one's about text.
fn built_subcommands(host: &ExtensionHost) -> Vec<(String, String)> {
    let mut command = logos_cli::command(host);
    command.build();
    command
        .get_subcommands()
        .map(|c| (c.get_name().to_string(), c.get_about().map(|a| a.to_string()).unwrap_or_default()))
        .collect()
}

/// A subcommand that would shadow a command, or a global flag, is not added:
/// a core verb, clap's own `help`, an alias of a core verb, an earlier
/// extension's verb, and an argument redefining `--json`. The composed command
/// stays exactly the public one, and nothing panics.
#[test]
fn an_extension_subcommand_that_would_shadow_a_command_or_a_global_flag_is_not_added() {
    let public = built_subcommands(&ExtensionHost::new());
    let impostors: [fn() -> clap::Command; 4] = [
        || clap::Command::new("status").about("impostor"),
        || clap::Command::new("help").about("impostor"),
        || clap::Command::new("ext-alias").about("impostor").alias("status"),
        || clap::Command::new("ext-json").about("impostor").arg(clap::Arg::new("json").long("json")),
    ];
    for command in impostors {
        let name = command().get_name().to_string();
        let host = ExtensionHost::new().register(Verb { id: "impostor", command }).expect("registers");
        assert_eq!(built_subcommands(&host), public, "`{name}` is not added");
    }

    let host = ExtensionHost::new()
        .register(Verb { id: "first", command: || clap::Command::new("ext-verb").about("first") })
        .and_then(|h| h.register(Verb { id: "second", command: || clap::Command::new("ext-verb").about("second") }))
        .expect("registers");
    let verbs: Vec<(String, String)> =
        built_subcommands(&host).into_iter().filter(|(name, _)| name == "ext-verb").collect();
    assert_eq!(verbs, [("ext-verb".to_string(), "first".to_string())], "the earlier extension keeps it");
}

/// A core command an extension re-exposes as a hidden subcommand of its own names
/// that alias in its help — and only while the extension is composed, so a build
/// without it names no command it does not have (S-645). The spelling is read from
/// the alias: its extension, its name and its required flags. A visible subcommand,
/// or a hidden one no core command shares a name with, is not an alias.
#[test]
fn a_core_command_names_an_extensions_hidden_alias_only_while_composed() {
    let about = |host: &ExtensionHost| {
        built_subcommands(host).into_iter().find(|(name, _)| name == "hook").expect("`hook` is listed").1
    };
    let public = about(&ExtensionHost::new());
    assert!(!public.contains("alias"), "the public `hook` names no alias: {public}");

    let host = ExtensionHost::new()
        .register(Verb {
            id: "aliasing",
            command: || {
                let emit = clap::Arg::new("emit").long("emit").required(true).action(clap::ArgAction::SetTrue);
                let force = clap::Arg::new("force").long("force").action(clap::ArgAction::SetTrue);
                clap::Command::new("ext-verb").about("x").subcommand(clap::Command::new("hook").hide(true).arg(emit).arg(force))
            },
        })
        .expect("registers");
    assert_eq!(about(&host), format!("{public}. (`logos ext-verb hook --emit` is the hidden alias of this command.)"));

    for command in [
        (|| clap::Command::new("ext-verb").about("x").subcommand(clap::Command::new("hook"))) as fn() -> clap::Command,
        || clap::Command::new("ext-verb").about("x").subcommand(clap::Command::new("not-core").hide(true)),
    ] {
        let host = ExtensionHost::new().register(Verb { id: "near-miss", command }).expect("registers");
        assert_eq!(about(&host), public, "no alias is named");
    }
}

/// Extension subcommands are listed in one slot of `logos --help` — after the
/// core's evidence tiers, the last of which is `security` (S-667), before `hook`
/// — in registration order, rather than after `serve` where clap would append
/// them: where `wiki` stood while it was a core command, after the evidence
/// tiers (S-644).
#[test]
fn an_extension_subcommand_is_listed_after_the_evidence_tiers_and_before_hook() {
    let help = logos_cli::command(&host("stub")).render_help().to_string();
    let at = |verb: &str| {
        help.lines()
            .position(|line| line.trim_start().starts_with(&format!("{verb} ")))
            .unwrap_or_else(|| panic!("`{verb}` is listed:\n{help}"))
    };
    assert_eq!(at(STUB_SUBCOMMAND), at("security") + 1, "directly after security:\n{help}");
    assert_eq!(at("hook"), at(STUB_SUBCOMMAND) + 1, "directly before hook:\n{help}");
    // The core's own order around the slot is unchanged.
    assert!(at("serve") > at("hook") && at("hotspots") < at("coverage") && at("coverage") < at("security"), "{help}");
    let public = logos_cli::command(&host("public")).render_help().to_string();
    assert_eq!(help.lines().count(), public.lines().count() + 1, "one line added, none moved out");
}

/// A build that composes no extension (S-644 AC4, [FR-DS-05]) — composed here as
/// the public host that registers nothing, which is what this repository's `main`
/// hands the library — serves no wiki command, route or tool, and never opens,
/// migrates or deletes an existing `.logos/wiki.db`: the file a build composing the
/// wiki left behind keeps its bytes and its modification time, and gains no SQLite
/// sidecar, through `init -i`, `index`, `status`, `scan`, the web Dashboard and the
/// MCP roster.
///
/// [FR-DS-05]: ../../docs/specs/requirements/FR-DS-05.md
///
/// The fixture lives in a temp directory outside this checkout, so the project root
/// never climbs to the enclosing repository.
#[test]
fn without_the_wiki_an_existing_wiki_db_is_never_opened_migrated_or_deleted() {
    let project = TempDir::new().expect("a temp project");
    let logos_dir = project.path().join(".logos");
    std::fs::create_dir_all(&logos_dir).expect("create .logos");
    std::fs::write(project.path().join("lib.rs"), "pub fn f() {}\n").expect("a source file");
    // A real, older-schema wiki store: any open by the wiki would migrate it.
    let db = logos_dir.join("wiki.db");
    {
        let conn = rusqlite::Connection::open(&db).expect("create a wiki.db");
        conn.execute_batch("CREATE TABLE pages (slug TEXT PRIMARY KEY); PRAGMA user_version = 1;")
            .expect("an old store");
    }
    let before = std::fs::read(&db).expect("the store's bytes");
    let mtime = || std::fs::metadata(&db).expect("the store is there").modified().expect("an mtime");
    let modified_before = mtime();

    // The CLI, composed without the wiki.
    for args in [&["init", "-i", "--json"][..], &["index"], &["status"], &["scan"]] {
        let run = logos("public", project.path(), args);
        assert_eq!(run.code, 0, "logos {args:?}: {}", run.stderr);
    }
    let run = logos("public", project.path(), &["wiki", "status"]);
    assert_eq!(run.code, 2, "no `logos wiki` without the extension: {}", run.stderr);
    assert!(!project.path().join(".agents").exists(), "no skill step ran");

    // The web Dashboard and an API probe of the wiki's routes (in a build with the web
    // surface), and the MCP roster.
    let engine = std::sync::Arc::new(logos_core::Engine::start(project.path()).expect("the engine starts"));
    #[cfg(feature = "ui")]
    probe_web_without_the_wiki(&engine);
    let tools = mcp::LogosMcp::new(std::sync::Arc::clone(&engine), &ExtensionHost::new()).list_tools();
    assert!(tools.len() > 20 && !tools.iter().any(|t| t.name.starts_with("wiki")), "no wiki tool");
    drop(engine);

    assert_eq!(std::fs::read(&db).expect("still there"), before, "wiki.db's bytes are unchanged");
    assert_eq!(mtime(), modified_before, "wiki.db was not written");
    for sidecar in ["wiki.db-wal", "wiki.db-shm", "wiki.db-journal"] {
        assert!(!logos_dir.join(sidecar).exists(), "{sidecar}: wiki.db was never opened");
    }
}

/// The web half of the case above: the Dashboard answers and the wiki's API routes
/// are unrouted (`404`) on a router composed without the wiki.
#[cfg(feature = "ui")]
fn probe_web_without_the_wiki(engine: &std::sync::Arc<logos_core::Engine>) {
    let router = web::hosted_router(
        std::sync::Arc::new(logos_core::federation::Backing::Single(std::sync::Arc::clone(engine))),
        web::IntentToken::generate(),
        &ExtensionHost::new(),
    )
    .expect("the router builds");
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a runtime");
    for (uri, expected) in [
        ("/api/v1/overview", axum::http::StatusCode::OK),
        ("/api/v1/wiki", axum::http::StatusCode::NOT_FOUND),
        ("/api/v1/wiki/page/overview/project-overview", axum::http::StatusCode::NOT_FOUND),
    ] {
        use tower::ServiceExt;
        let request = axum::http::Request::get(uri).body(axum::body::Body::empty()).expect("a request");
        let status = runtime.block_on(router.clone().oneshot(request)).expect("the router answers").status();
        assert_eq!(status, expected, "GET {uri}");
    }
}
