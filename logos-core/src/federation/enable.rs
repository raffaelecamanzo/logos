//! `logos init --workspace` enablement orchestration ([FR-WS-02]): candidate
//! discovery for the approval gate, the non-clobber per-member `init` loop,
//! and the idempotent workspace MCP injection. The manifest write itself is
//! owned by [`super::manifest::upsert`]; the interactive gate and the
//! background index warm are CLI-surface concerns (terminal I/O / process
//! spawning) that live in the `cli` crate, not here.
//!
//! The report also states the **working-tree footprint** enablement leaves
//! behind ([`WorkingTreeFootprint`]): how many members now carry a fresh
//! untracked `.logos/`, and what inside it travels versus what is ignored
//! ([FR-IN-04]). Enabling N members legitimately dirties N repositories — the
//! defect this answers was the silence about it, not the ignore rules.
//!
//! [FR-WS-02]: ../../../docs/specs/requirements/FR-WS-02.md
//! [FR-IN-04]: ../../../docs/specs/requirements/FR-IN-04.md

use std::path::Path;

use anyhow::Result;
use serde::Serialize;

use crate::config::globs;
use crate::init::{self, InitOptions};
use crate::models::pipeline::{InitAction, InitResult, InitStep};

use super::{discover_candidates, Member};

/// The `.mcp.json` server key for the workspace-wide entry — distinct from
/// the per-repo `"logos"` key so a client walking `.mcp.json` up-tree from a
/// member sees two named servers, never a silent shadow ([FR-WS-02] Notes).
///
/// [FR-WS-02]: ../../../docs/specs/requirements/FR-WS-02.md
pub const WORKSPACE_MCP_SERVER_KEY: &str = "logos-workspace";

/// One member's non-clobber `init` outcome during workspace enablement: the
/// full step report on success, or `Degraded` with the reason — a member
/// failure never aborts the rest of the command ([FR-WS-02], [NFR-CC-04]).
///
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum MemberOutcome {
    Ready(InitResult),
    Degraded { reason: String },
}

/// One member's line in a [`WorkspaceEnableReport`].
#[derive(Debug, Serialize)]
pub struct MemberReport {
    pub name: String,
    pub root: String,
    #[serde(flatten)]
    pub outcome: MemberOutcome,
}

/// The full `logos init --workspace` report ([FR-WS-02]).
///
/// [FR-WS-02]: ../../../docs/specs/requirements/FR-WS-02.md
#[derive(Debug, Serialize)]
pub struct WorkspaceEnableReport {
    pub workspace: String,
    pub root: String,
    pub members: Vec<MemberReport>,
    pub manifest: InitStep,
    pub mcp: InitStep,
    /// What enabling this workspace did to the members' working trees
    /// ([FR-WS-02], [FR-IN-04]) — the fact that used to be left unsaid.
    ///
    /// [FR-IN-04]: ../../../docs/specs/requirements/FR-IN-04.md
    pub footprint: WorkingTreeFootprint,
}

/// The working-tree footprint workspace enablement left behind ([FR-WS-02]).
///
/// Enabling the real 84-member workspace made 84 repositories git-dirty, each
/// with one new untracked `.logos/`, and said so nowhere. That silence — not
/// the ignore rules — is the defect: `.logos/` holding checked-in policy beside
/// ignored derived state is [FR-IN-04] working exactly as designed, and
/// widening the ignores to quiet the noise would stop `config.toml`/`rules.toml`
/// travelling, which is the whole point of that requirement. So this reports the
/// footprint and changes nothing about what `init` writes.
///
/// Every field is derived from the per-member step reports `init` already
/// returned — no second filesystem walk over N members ([CRA-07]).
///
/// [FR-WS-02]: ../../../docs/specs/requirements/FR-WS-02.md
/// [FR-IN-04]: ../../../docs/specs/requirements/FR-IN-04.md
/// [CRA-07]: ../../../docs/requests/CR-102-warm-outcome-record-and-spec-corrections.md
#[derive(Debug, Serialize)]
pub struct WorkingTreeFootprint {
    /// Members whose `.logos/` this run created outright — each is now one
    /// git-dirty repository carrying a new untracked directory.
    pub fresh: usize,
    /// Members that already carried a `.logos/`: their trees gained no new
    /// untracked directory, whatever `init` refreshed inside it.
    pub unchanged: usize,
    /// Members whose own `init` failed — nothing was written, nothing dirtied.
    /// Counted so the three buckets add up to the member set; the *diagnosis*
    /// stays on the member row, reported once ([NFR-CC-04]).
    ///
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    pub degraded: usize,
    /// What is meant to be **committed** inside `.logos/` ([FR-IN-04]).
    ///
    /// [FR-IN-04]: ../../../docs/specs/requirements/FR-IN-04.md
    pub committed: Vec<&'static str>,
    /// The derived state the generated `.logos/.gitignore` already keeps out of
    /// git ([FR-IN-04]) — named so the operator can act without reading it.
    ///
    /// [FR-IN-04]: ../../../docs/specs/requirements/FR-IN-04.md
    pub ignored: Vec<&'static str>,
}

impl WorkingTreeFootprint {
    /// Every member this enablement touched, degraded ones included.
    #[must_use]
    pub const fn members(&self) -> usize {
        self.fresh + self.unchanged + self.degraded
    }

    /// The operator-facing sentence, or `None` when no member's tree gained a
    /// `.logos/` (the settled re-run, where there is nothing to act on).
    ///
    /// Built here rather than in the adapter, and returned as a whole line the
    /// caller only has to print, so the CLI renders the footprint without
    /// composing it ([NFR-MA-02]) — the same shape
    /// [`DegradedRollup::notice`](super::open_state::DegradedRollup::notice)
    /// already gives the `workspace` commands.
    ///
    /// [NFR-MA-02]: ../../../docs/specs/requirements/NFR-MA-02.md
    #[must_use]
    pub fn notice(&self) -> Option<String> {
        if self.fresh == 0 {
            return None;
        }
        // The fresh/unchanged split is always carried structurally; in prose a
        // "0 already had one" is noise, so the clause appears only when true.
        let already = match self.unchanged {
            0 => String::new(),
            n => format!(" ({n} already had one)"),
        };
        Some(format!(
            "note: {} of {} workspace members now carry a fresh untracked `.logos/`{already}. \
             Commit the policy meant to travel with each repository — {} — and leave the \
             derived state ({}) to the generated `.logos/.gitignore`, which already ignores \
             it (FR-IN-04).",
            self.fresh,
            self.members(),
            self.committed.join(", "),
            self.ignored.join(", "),
        ))
    }
}

/// Whether this run created the member's `.logos/` outright, rather than
/// finding one already there.
///
/// Read off the step report `init` already returned — never a second look at
/// the filesystem ([CRA-07]). The directory is new exactly when *every*
/// `.logos/` target `init` manages came back `Created`: a member holding even
/// one pre-existing target (the hand-edited `config.toml` the non-clobber rule
/// exists for, [FR-IN-01]) already had the directory, so its tree gained no new
/// untracked entry to report. An empty step set is not vacuously fresh — it
/// means `init` wrote nothing at all.
///
/// [CRA-07]: ../../../docs/requests/CR-102-warm-outcome-record-and-spec-corrections.md
/// [FR-IN-01]: ../../../docs/specs/requirements/FR-IN-01.md
fn created_logos_dir(result: &InitResult) -> bool {
    let mut steps = result
        .steps
        .iter()
        .filter(|step| step.target.starts_with(".logos/"));
    steps.next().is_some_and(|first| {
        first.action == InitAction::Created && steps.all(|s| s.action == InitAction::Created)
    })
}

/// Roll the per-member outcomes up into the working-tree footprint
/// ([FR-WS-02]).
///
/// Pure over the summary [`enable`] has already built: it takes no root, opens
/// nothing, and never touches the filesystem, which is what makes the report
/// free at any N ([CRA-07]).
///
/// [FR-WS-02]: ../../../docs/specs/requirements/FR-WS-02.md
/// [CRA-07]: ../../../docs/requests/CR-102-warm-outcome-record-and-spec-corrections.md
fn footprint(members: &[MemberReport]) -> WorkingTreeFootprint {
    let mut report = WorkingTreeFootprint {
        fresh: 0,
        unchanged: 0,
        degraded: 0,
        committed: init::committed_policy(),
        ignored: init::ignored_state(),
    };
    for member in members {
        match &member.outcome {
            MemberOutcome::Ready(result) if created_logos_dir(result) => report.fresh += 1,
            MemberOutcome::Ready(_) => report.unchanged += 1,
            MemberOutcome::Degraded { .. } => report.degraded += 1,
        }
    }
    report
}

/// Candidate members not already part of the workspace, after dropping
/// anything matching an `--exclude` glob ([FR-WS-02]): the approval-gate
/// input. Pure and testable — no interactivity, no writes.
///
/// Reuses the crate's single glob compiler ([`globs::compile`]) rather than
/// hand-rolling a second one — the same "fail loud on a bad glob" primitive
/// `config_artifacts`/`documentation` already share.
///
/// # Errors
/// A malformed `--exclude` glob ([`crate::config::ConfigError::BadGlob`]).
///
/// [FR-WS-02]: ../../../docs/specs/requirements/FR-WS-02.md
pub fn candidates_for_approval(
    root: &Path,
    already_included: &[String],
    exclude: &[String],
) -> Result<Vec<Member>> {
    let excluded = globs::compile(exclude)?;

    Ok(discover_candidates(root)
        .into_iter()
        .filter(|m| !already_included.contains(&m.name))
        .filter(|m| !excluded.is_match(&m.name))
        .collect())
}

/// Run the non-interactive half of `init --workspace` for the approved member
/// set: a non-clobber per-member `init`, the incremental manifest upsert, and
/// the idempotent workspace MCP injection ([FR-WS-02]). Never blocks on
/// indexing — kicking off the background warm is the caller's concern.
///
/// The report carries the [`WorkingTreeFootprint`] this run left in the members'
/// trees, rolled up from the same per-member step reports rather than from a
/// second walk over them ([CRA-07]).
///
/// # Errors
/// Only if the manifest or `.mcp.json` cannot be written; a member's own
/// `init` failure is caught and reported [`MemberOutcome::Degraded`], never
/// fatal to the rest of the command.
///
/// [FR-WS-02]: ../../../docs/specs/requirements/FR-WS-02.md
pub fn enable(root: &Path, name: &str, members: &[Member]) -> Result<WorkspaceEnableReport> {
    let reports: Vec<MemberReport> = members
        .iter()
        .map(|member| MemberReport {
            name: member.name.clone(),
            root: member.root.display().to_string(),
            outcome: match crate::Engine::init_with(&member.root, &InitOptions::default()) {
                Ok(result) => MemberOutcome::Ready(result),
                // `{err:#}` (not `{err}`/`.to_string()`) to keep the causal
                // chain: `Engine::init_with`'s steps wrap I/O failures with
                // `.with_context(...)` (e.g. "writing .logos/config.toml"),
                // so the bare Display would show only that wrapper and drop
                // the actual underlying cause (permission denied, disk
                // full, …) — the same convention `cli/src/main.rs` uses.
                Err(err) => MemberOutcome::Degraded {
                    reason: format!("{err:#}"),
                },
            },
        })
        .collect();

    let footprint = footprint(&reports);
    let member_names: Vec<String> = members.iter().map(|m| m.name.clone()).collect();
    let manifest_step = super::manifest::upsert(root, name, &member_names)?;
    let mcp_step = init::inject_mcp_entry(root, WORKSPACE_MCP_SERVER_KEY, init::mcp_server_entry())?;

    Ok(WorkspaceEnableReport {
        workspace: name.to_string(),
        root: root.display().to_string(),
        members: reports,
        manifest: manifest_step,
        mcp: mcp_step,
        footprint,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;
    use std::process::Command;

    use tempfile::TempDir;

    fn sh_git(cwd: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(["-c", "user.email=test@logos", "-c", "user.name=logos-test"])
            .args(args)
            .output()
            .expect("git is on PATH");
        assert!(out.status.success(), "git {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
    }

    fn init_repo(dir: &Path) {
        fs::create_dir_all(dir).unwrap();
        sh_git(dir, &["init", "-q", "-b", "main"]);
        fs::write(dir.join("f.txt"), "x\n").unwrap();
        sh_git(dir, &["add", "."]);
        sh_git(dir, &["commit", "-q", "-m", "init"]);
    }

    // ── candidates_for_approval (FR-WS-02) ────────────────────────────────

    #[test]
    fn candidates_excludes_already_included_and_globbed_members() {
        let tmp = TempDir::new().unwrap();
        init_repo(&tmp.path().join("api"));
        init_repo(&tmp.path().join("web"));
        init_repo(&tmp.path().join("legacy-billing"));

        let already = vec!["api".to_string()];
        let candidates = candidates_for_approval(tmp.path(), &already, &["legacy-*".to_string()])
            .expect("compiles and scans");

        let names: Vec<&str> = candidates.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["web"], "api already included, legacy-billing excluded");
    }

    #[test]
    fn candidates_rejects_a_malformed_exclude_glob() {
        let tmp = TempDir::new().unwrap();
        assert!(candidates_for_approval(tmp.path(), &[], &["a{b".to_string()]).is_err());
    }

    // ── enable (FR-WS-02) ──────────────────────────────────────────────────

    #[test]
    fn enable_initialises_each_member_writes_manifest_and_injects_one_mcp_entry() {
        let tmp = TempDir::new().unwrap();
        init_repo(&tmp.path().join("api"));
        init_repo(&tmp.path().join("web"));

        let members = discover_candidates(tmp.path());
        assert_eq!(members.len(), 2);

        let report = enable(tmp.path(), "shop", &members).expect("enables");
        assert_eq!(report.workspace, "shop");
        assert_eq!(report.members.len(), 2);
        for m in &report.members {
            assert!(matches!(m.outcome, MemberOutcome::Ready(_)), "{}: {:?}", m.name, m.outcome);
            assert!(tmp.path().join(&m.name).join(".logos/logos.db").is_file());
        }

        let manifest = fs::read_to_string(tmp.path().join(super::super::manifest::MANIFEST_FILENAME)).unwrap();
        assert!(manifest.contains("name = \"shop\""));

        let mcp: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(tmp.path().join(".mcp.json")).unwrap()).unwrap();
        assert!(mcp["mcpServers"]["logos-workspace"].is_object());
        assert!(mcp["mcpServers"].get("logos").is_none(), "no per-repo entry at the parent");
    }

    #[test]
    fn enable_is_idempotent_on_a_second_run() {
        let tmp = TempDir::new().unwrap();
        init_repo(&tmp.path().join("api"));
        let members = discover_candidates(tmp.path());

        enable(tmp.path(), "shop", &members).unwrap();
        // A second run over the same members must not duplicate the MCP entry
        // or clobber the member's own config.
        let config_path = tmp.path().join("api").join(".logos").join("config.toml");
        fs::write(&config_path, "# hand-edited\n").unwrap();

        let report = enable(tmp.path(), "shop", &members).expect("re-enables");
        assert_eq!(report.manifest.action, crate::models::pipeline::InitAction::Unchanged);
        assert_eq!(report.mcp.action, crate::models::pipeline::InitAction::Unchanged);
        assert_eq!(fs::read_to_string(&config_path).unwrap(), "# hand-edited\n", "never clobbered");

        let mcp: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(tmp.path().join(".mcp.json")).unwrap()).unwrap();
        assert_eq!(mcp["mcpServers"].as_object().unwrap().len(), 1, "still exactly one entry");
    }

    #[test]
    fn enable_degrades_a_member_whose_init_fails_without_aborting_the_rest() {
        let tmp = TempDir::new().unwrap();
        init_repo(&tmp.path().join("api"));

        // A member "root" that is actually a plain file: `Engine::init_with`
        // fails trying to create `.logos/` under it, the other member still
        // succeeds.
        let broken_root = tmp.path().join("ghost");
        fs::write(&broken_root, "not a directory").unwrap();
        let broken = Member {
            name: "ghost".to_string(),
            root: broken_root,
        };
        let ok = discover_candidates(tmp.path());
        let members: Vec<Member> = ok.into_iter().chain(std::iter::once(broken)).collect();

        let report = enable(tmp.path(), "shop", &members).expect("the command itself still succeeds");
        let ghost = report.members.iter().find(|m| m.name == "ghost").unwrap();
        assert!(matches!(ghost.outcome, MemberOutcome::Degraded { .. }));
        let api = report.members.iter().find(|m| m.name == "api").unwrap();
        assert!(matches!(api.outcome, MemberOutcome::Ready(_)));
    }

    // ── the working-tree footprint (FR-WS-02, FR-IN-04, CRA-07) ───────────

    fn ready(targets: &[(&str, InitAction)]) -> MemberOutcome {
        MemberOutcome::Ready(InitResult {
            steps: targets
                .iter()
                .map(|(target, action)| InitStep {
                    target: (*target).to_string(),
                    action: *action,
                    detail: String::new(),
                })
                .collect(),
            ..InitResult::default()
        })
    }

    fn row(name: &str, outcome: MemberOutcome) -> MemberReport {
        MemberReport {
            name: name.to_string(),
            root: format!("/nowhere/{name}"),
            outcome,
        }
    }

    /// The count derives from the summary alone ([CRA-07]): these rows name
    /// roots that do not exist, so a second filesystem walk could not classify
    /// them at all — and the split still comes out right.
    #[test]
    fn the_footprint_derives_from_the_summary_with_no_filesystem_walk() {
        const CREATED: InitAction = InitAction::Created;
        let rows = vec![
            row("all-new", ready(&[(".logos/config.toml", CREATED), (".logos/rules.toml", CREATED)])),
            // One pre-existing policy file is enough: the directory was already
            // there, so this member's tree gained no new untracked entry.
            row(
                "half-there",
                ready(&[
                    (".logos/config.toml", InitAction::Unchanged),
                    (".logos/rules.toml", CREATED),
                ]),
            ),
            row("settled", ready(&[(".logos/config.toml", InitAction::Unchanged)])),
            row("broken", MemberOutcome::Degraded { reason: "boom".into() }),
        ];

        let f = footprint(&rows);
        assert_eq!((f.fresh, f.unchanged, f.degraded), (1, 2, 1));
        assert_eq!(f.members(), rows.len(), "the three buckets partition the member set");
    }

    /// Steps outside `.logos/` (the `-i` extras: `CLAUDE.md`, `.mcp.json`)
    /// describe files that are not the directory being counted, so they must
    /// not decide whether it is fresh.
    #[test]
    fn steps_outside_the_logos_dir_do_not_decide_freshness() {
        let rows = vec![row(
            "api",
            ready(&[
                (".logos/config.toml", InitAction::Created),
                ("CLAUDE.md", InitAction::Updated),
                (".mcp.json", InitAction::Unchanged),
            ]),
        )];
        assert_eq!(footprint(&rows).fresh, 1);
    }

    /// A member whose `init` wrote nothing at all is not vacuously "fresh" —
    /// `all` over an empty iterator is `true`, which is exactly the trap.
    #[test]
    fn a_member_with_no_logos_steps_is_not_counted_fresh() {
        let rows = vec![row("api", ready(&[("CLAUDE.md", InitAction::Created)]))];
        let f = footprint(&rows);
        assert_eq!((f.fresh, f.unchanged), (0, 1));
    }

    /// The report states [FR-IN-04]'s two halves, so the operator can act on
    /// the dirty tree without going to look the requirement up.
    #[test]
    fn the_footprint_states_what_travels_and_what_is_ignored() {
        let f = footprint(&[row("api", ready(&[(".logos/config.toml", InitAction::Created)]))]);
        assert!(f.committed.contains(&".logos/config.toml"), "{:?}", f.committed);
        assert!(f.committed.contains(&".logos/rules.toml"), "{:?}", f.committed);
        assert!(f.ignored.contains(&"logos.db*"), "{:?}", f.ignored);

        let notice = f.notice().expect("a fresh member is worth a word");
        assert!(notice.contains(".logos/config.toml") && notice.contains("logos.db*"), "{notice}");
        assert!(notice.contains("FR-IN-04"), "the operator is told where this comes from: {notice}");
        assert!(!notice.contains("already had one"), "no zero-valued clause: {notice}");
    }

    /// With both kinds present the prose names both, so the operator reading
    /// stderr sees the same split the payload carries.
    #[test]
    fn the_notice_names_the_members_that_already_had_a_logos_dir() {
        let rows = vec![
            row("new", ready(&[(".logos/config.toml", InitAction::Created)])),
            row("old", ready(&[(".logos/config.toml", InitAction::Unchanged)])),
        ];
        let notice = footprint(&rows).notice().expect("one member is fresh");
        assert!(notice.contains("1 of 2") && notice.contains("(1 already had one)"), "{notice}");
    }

    /// Nothing new appeared, so there is nothing to say — the settled re-run
    /// must not print a footprint note every time.
    #[test]
    fn a_run_that_created_no_logos_dir_says_nothing() {
        let rows = vec![row("api", ready(&[(".logos/config.toml", InitAction::Unchanged)]))];
        assert!(footprint(&rows).notice().is_none());
    }

    /// End to end over real repositories: a first run reports both members
    /// fresh, a second reports both unchanged. The classification is asserted
    /// against the working trees `enable` actually wrote, not just hand-built
    /// rows.
    #[test]
    fn enable_reports_fresh_members_then_unchanged_ones_on_a_re_run() {
        let tmp = TempDir::new().unwrap();
        init_repo(&tmp.path().join("api"));
        init_repo(&tmp.path().join("web"));
        let members = discover_candidates(tmp.path());

        let first = enable(tmp.path(), "shop", &members).expect("enables");
        assert_eq!((first.footprint.fresh, first.footprint.unchanged), (2, 0));
        assert!(first.footprint.notice().is_some());

        let second = enable(tmp.path(), "shop", &members).expect("re-enables");
        assert_eq!((second.footprint.fresh, second.footprint.unchanged), (0, 2));
        assert!(second.footprint.notice().is_none(), "nothing new to report");
    }

    /// The tempting fix — widening the ignores, or writing into each member's
    /// own `.gitignore` — is exactly what [FR-IN-04] forbids, because it would
    /// stop the policy travelling. This asserts the defect was fixed in the
    /// *report* and nowhere else: tracked files untouched, the member's own
    /// `.gitignore` byte-identical, `config.toml`/`rules.toml` still tracked by
    /// git's own reckoning.
    #[test]
    fn enable_dirties_only_an_untracked_logos_dir_and_leaves_the_policy_tracked() {
        let tmp = TempDir::new().unwrap();
        let member = tmp.path().join("api");
        init_repo(&member);
        let own_gitignore = "# the user's own\ntarget/\n";
        fs::write(member.join(".gitignore"), own_gitignore).unwrap();
        sh_git(&member, &["add", "."]);
        sh_git(&member, &["commit", "-q", "-m", "gitignore"]);

        let members = discover_candidates(tmp.path());
        let report = enable(tmp.path(), "shop", &members).expect("enables");
        assert_eq!(report.footprint.fresh, 1);

        assert_eq!(fs::read_to_string(member.join("f.txt")).unwrap(), "x\n", "tracked file untouched");
        assert_eq!(
            fs::read_to_string(member.join(".gitignore")).unwrap(),
            own_gitignore,
            "the member's own .gitignore is never touched"
        );

        // git's own verdict, not ours: the only dirt is the untracked `.logos/`.
        let status = Command::new("git")
            .arg("-C")
            .arg(&member)
            .args(["status", "--porcelain"])
            .output()
            .expect("git is on PATH");
        assert_eq!(
            String::from_utf8_lossy(&status.stdout).trim(),
            "?? .logos/",
            "one new untracked directory, nothing modified"
        );

        // …and inside it the policy still travels while the derived state does not.
        let ignored = |rel: &str| {
            Command::new("git")
                .arg("-C")
                .arg(&member)
                .args(["check-ignore", "-q", rel])
                .status()
                .expect("git is on PATH")
                .success()
        };
        assert!(!ignored(".logos/config.toml"), "config.toml must stay committable");
        assert!(!ignored(".logos/rules.toml"), "rules.toml must stay committable");
        assert!(ignored(".logos/logos.db"), "the derived store is ignored");
    }
}
