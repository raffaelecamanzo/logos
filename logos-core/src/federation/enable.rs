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
//! The same enablement path is what a plain `logos init` should have taken
//! when it was run one directory too high, so the detection of that shape
//! ([`ParentOfRepos`], [FR-IN-08]) lives here too — composed here and merely
//! rendered by the adapter ([NFR-MA-02]), and built out of the candidate scan
//! below rather than a second git-root implementation.
//!
//! [FR-WS-02]: ../../../docs/specs/requirements/FR-WS-02.md
//! [FR-IN-04]: ../../../docs/specs/requirements/FR-IN-04.md
//! [FR-IN-08]: ../../../docs/specs/requirements/FR-IN-08.md
//! [NFR-MA-02]: ../../../docs/specs/requirements/NFR-MA-02.md

use std::fmt;
use std::path::Path;

use anyhow::Result;
use serde::Serialize;

use crate::config::{globs, ZeroAdmissionDiagnostic};
use crate::init::{self, InitOptions};
use crate::models::pipeline::{InitAction, InitResult, InitStep};
use crate::workspace::git_root_known;

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
    /// Whether background warming started, over how many members, and where
    /// to watch it ([FR-WS-02], CR-119) — the fact that `init --workspace`
    /// used to leave unsaid entirely. Populated by the CLI adapter once the
    /// warm supervisor decision is known (this core function returns before
    /// that decision is made — [`enable`] never blocks on indexing).
    pub warm_start: WarmStartDisclosure,
}

/// Background warm-start disclosure ([FR-WS-02], [FR-WS-15], [FR-WS-17],
/// CR-119): whether this run kicked off the detached warm supervisor over the
/// newly-approved members, and where to watch it.
///
/// **One workspace-level field, never per-member text** — the same shape
/// [`WorkingTreeFootprint`] already uses, and for the same reason: an
/// 84-member workspace's disclosure must cost one line, not eighty-four.
#[derive(Debug, Default, Serialize)]
pub struct WarmStartDisclosure {
    /// Newly-approved members handed to the background warm this run — the
    /// delta, not the whole membership. Already-manifested members are not
    /// re-warmed on an incremental re-run (they were warmed, or fell back to
    /// lazy indexing, on the run that first added them), so `0` there is
    /// correct, not a gap.
    pub members: usize,
    /// Whether the detached warm supervisor was actually spawned ([FR-WS-14]).
    /// `false` on an empty delta, or a best-effort spawn failure — the warm is
    /// never fatal to `init --workspace`, so this never blocks the command.
    ///
    /// [FR-WS-14]: ../../../docs/specs/requirements/FR-WS-14.md
    pub started: bool,
    /// The surface reporting warm progress and outcome ([FR-WS-15] status
    /// roll-up, [FR-WS-17] durable sidecar) — named here so the operator does
    /// not have to go looking for it.
    ///
    /// [FR-WS-15]: ../../../docs/specs/requirements/FR-WS-15.md
    /// [FR-WS-17]: ../../../docs/specs/requirements/FR-WS-17.md
    pub status_command: &'static str,
}

impl WarmStartDisclosure {
    /// The single source for the surface name this disclosure carries, so the
    /// core and the CLI's own `workspace status` subcommand cannot drift onto
    /// different names ([FR-WS-15]).
    ///
    /// [FR-WS-15]: ../../../docs/specs/requirements/FR-WS-15.md
    pub const STATUS_COMMAND: &'static str = "logos workspace status";

    /// Build the disclosure for `members` newly-approved delta members,
    /// `started` from whether the CLI's warm-supervisor spawn actually
    /// succeeded (CR-119) — a one-line call site keeps the composition here
    /// rather than duplicated at the CLI adapter (NFR-MA-02).
    #[must_use]
    pub const fn new(members: usize, started: bool) -> Self {
        Self {
            members,
            started,
            status_command: Self::STATUS_COMMAND,
        }
    }

    /// The operator-facing sentence, or `None` when no member was newly
    /// warmed this run (the settled re-run, mirroring
    /// [`WorkingTreeFootprint::notice`]).
    #[must_use]
    pub fn notice(&self) -> Option<String> {
        if self.members == 0 {
            return None;
        }
        let verb = if self.started {
            "has begun"
        } else {
            "could not be started (best-effort — the lazy `ensure_indexed` \
             fallback still covers these members, FR-IX-07)"
        };
        Some(format!(
            "note: background warming {verb} for {} newly-enrolled member{} — `{}` reports \
             progress and outcome.",
            self.members,
            if self.members == 1 { "" } else { "s" },
            self.status_command,
        ))
    }
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
    /// What the generated `.logos/.gitignore` already keeps out of git
    /// ([FR-IN-04]) — named so the operator can act without reading it.
    ///
    /// "Ignored", not "derived": the list is mostly derived state, but
    /// `secrets.toml` is on it because it is a secret ([NFR-SE-07]), not
    /// because it is regenerable, and telling an operator that a secret is
    /// *derived* state would be worse than saying nothing.
    ///
    /// [FR-IN-04]: ../../../docs/specs/requirements/FR-IN-04.md
    /// [NFR-SE-07]: ../../../docs/specs/requirements/NFR-SE-07.md
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
             Commit the policy meant to travel with each repository — {} — and leave the rest \
             ({}) to the generated `.logos/.gitignore`, which already ignores it (FR-IN-04).",
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

/// The **parent-of-repos** shape a plain `logos init` can land in ([FR-IN-08]):
/// the root is not itself a git top-level, and at least one immediate child is.
///
/// `init` succeeds there and creates a store that can never admit a file — the
/// discovery walk prunes every nested git boundary, so the run reports
/// `files_indexed: 0`, `coverage: 1.0` and exit 0, every signal consistent with
/// success ([FR-IX-13]). The whole Workspace Federation feature set is invisible
/// at exactly the root where it is the answer, so this states the shape and
/// names [`ZeroAdmissionDiagnostic::REMEDY`] — the same remedy string `index`
/// already prints, so the two surfaces cannot drift apart.
///
/// **Detection reuses the federation primitives rather than re-deriving either
/// half** ([FR-WS-01]): [`git_root_known`] answers "is the root a repository?",
/// and [`discover_candidates`] — literally `init --workspace`'s own candidate
/// scan — answers "is any immediate child one?". That reuse is load-bearing, not
/// tidy: the nudge names `init --workspace` as the remedy, so the shape it fires
/// on must be exactly the shape that command can act on. A hand-rolled
/// `child.join(".git").exists()` test would diverge on both sides — it would
/// miss an already-indexed non-git member (which `discover_candidates` admits)
/// and fire on a bare `.git` file layout the candidate scan would then reject,
/// handing the operator a confidently wrong instruction, which is worse than the
/// silence this exists to fix ([NFR-CC-04]).
///
/// Reusing the primitive is necessary but was not sufficient: the *tri-state*
/// one is required. [`crate::workspace::is_git_root`] collapses "git is absent"
/// into `false`, which is the safe direction for an admission filter and the
/// **inverting** direction for this negative inference — with no `git` on PATH
/// every root answered "not a repository", so an ordinary repository holding one
/// already-indexed subdirectory was diagnosed as a parent folder of sibling
/// repositories and offered federation. Hence [`git_root_known`], and hence the
/// `!= Some(false)` guard below rather than a `!`.
///
/// [FR-IN-08]: ../../../docs/specs/requirements/FR-IN-08.md
/// [FR-IX-13]: ../../../docs/specs/requirements/FR-IX-13.md
/// [FR-WS-01]: ../../../docs/specs/requirements/FR-WS-01.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParentOfRepos {
    /// The members `logos init --workspace` would offer here, in
    /// [`discover_candidates`]' deterministic name order ([NFR-RA-06]) — the
    /// list the approval gate itself would show, not a re-derived one.
    pub candidates: Vec<Member>,
}

impl ParentOfRepos {
    /// How many candidate names the explanation lists before eliding the rest
    /// as `… +N more` — the 84-member workspace behind [CR-098] must yield a
    /// readable line, not a wall of names. Deliberately the same bound
    /// [`ZeroAdmissionDiagnostic::SAMPLE_LIMIT`] uses, so the `init` nudge and
    /// the `index` warning elide alike.
    ///
    /// [CR-098]: ../../../docs/requests/CR-098-nested-git-prune-diagnostic.md
    pub const SAMPLE_LIMIT: usize = ZeroAdmissionDiagnostic::SAMPLE_LIMIT;

    /// `Some` **iff** `root` is not a git top-level and the candidate scan finds
    /// at least one member there.
    ///
    /// Both halves matter. An ordinary repository root is a git top-level, so it
    /// is never diagnosed however many sibling repositories it contains — that
    /// is the negative case [FR-IN-08] names. An empty folder, or one whose
    /// children are plain directories, has no candidates and a different cause,
    /// which this must not misattribute.
    ///
    /// The `root` handed in is already git-top-level-resolved by
    /// [`crate::workspace::resolve_root`] at the CLI boundary, so a *sub*
    /// directory of a repository arrives here as its repository root and is
    /// correctly declined.
    ///
    /// [FR-IN-08]: ../../../docs/specs/requirements/FR-IN-08.md
    #[must_use]
    pub fn detect(root: &Path) -> Option<Self> {
        // `Some(false)` — git ran and said "not a repository top level" — is the
        // ONLY answer this may fire on. `Some(true)` is an ordinary repository
        // root; `None` means the `git` binary is absent, so every path answers
        // the same and the negative inference inverts. Read through the
        // `bool`-collapsing `is_git_root`, a missing git turned this diagnosis
        // on an ordinary repository root that happened to hold one indexed
        // subdirectory — the confidently-wrong instruction the type's docs
        // above claim the primitive reuse prevents.
        if git_root_known(root) != Some(false) {
            return None;
        }
        let candidates = discover_candidates(root);
        (!candidates.is_empty()).then_some(Self { candidates })
    }

    /// The y/n question the TTY offer asks, stating the stake (how many
    /// repositories would be enabled) so the answer is informed.
    ///
    /// Separate from the [`fmt::Display`] explanation because the two have
    /// different fates: the explanation always prints, the question only on a
    /// TTY ([FR-IN-08]).
    ///
    /// `host_setup_requested` says the invocation asked for the [FR-IN-02]
    /// host-integration steps (`--interactive` / `--hooks`), which the
    /// [FR-WS-02] path does not apply — it inits every member with
    /// [`InitOptions::default`] and injects only the workspace MCP entry at the
    /// parent. Saying so **here** matters because this question is the moment of
    /// consent: `logos init -i` is what the installation guide tells a new user
    /// to run, so at a parent-of-repos root a bare "yes" would otherwise trade
    /// the managed CLAUDE.md block, the wiki skill and the quality-report hook
    /// away without ever naming them. A "yes" is consent to change the *scope*
    /// of the init, not to silently cancel the flags the user typed.
    ///
    /// [FR-IN-02]: ../../../docs/specs/requirements/FR-IN-02.md
    /// [FR-IN-08]: ../../../docs/specs/requirements/FR-IN-08.md
    /// [FR-WS-02]: ../../../docs/specs/requirements/FR-WS-02.md
    #[must_use]
    pub fn question(&self, host_setup_requested: bool) -> String {
        format!(
            "enable a Logos workspace here instead ({} member {}{})?",
            self.candidates.len(),
            if self.candidates.len() == 1 {
                "repository"
            } else {
                "repositories"
            },
            if host_setup_requested {
                ", without the --interactive/--hooks host setup"
            } else {
                ""
            },
        )
    }
}

impl fmt::Display for ParentOfRepos {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sample: Vec<&str> = self
            .candidates
            .iter()
            .take(Self::SAMPLE_LIMIT)
            .map(|m| m.name.as_str())
            .collect();
        write!(
            f,
            "logos init: this looks like a parent folder of sibling repositories ({}",
            sample.join(", "),
        )?;
        let elided = self.candidates.len() - sample.len();
        if elided > 0 {
            write!(f, ", … +{elided} more")?;
        }
        write!(
            f,
            "), not a repository itself — a single-root index here would admit no \
             files. Run `{}` to index them as a federated workspace.",
            ZeroAdmissionDiagnostic::REMEDY,
        )
    }
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
            outcome: match crate::Engine::init_with(
                &member.root,
                &InitOptions {
                    workspace_member: true,
                    ..InitOptions::default()
                },
            ) {
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
        // The CLI adapter overwrites this once the warm-supervisor decision is
        // known — `enable` itself never blocks on indexing ([FR-WS-02]).
        warm_start: WarmStartDisclosure::default(),
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

    /// Each member's own `init` runs in the workspace-member context (CR-119):
    /// its next-step message states what is true there, never the single-repo
    /// `logos index` advice, which at the workspace root builds only the root.
    #[test]
    fn enable_inits_each_member_in_the_workspace_member_context() {
        let tmp = TempDir::new().unwrap();
        init_repo(&tmp.path().join("api"));
        let members = discover_candidates(tmp.path());

        let report = enable(tmp.path(), "shop", &members).expect("enables");
        let MemberOutcome::Ready(result) = &report.members[0].outcome else {
            panic!("api must init successfully: {:?}", report.members[0].outcome);
        };
        assert!(
            !result.message.contains("logos index"),
            "a workspace member must not be told to run a command that builds only the root: {}",
            result.message
        );
        assert!(
            result.message.contains("enrolled") && result.message.contains("background warming"),
            "the member message must state what is actually true: {}",
            result.message
        );
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

    /// A workspace where *every* member failed to initialise reaches the same
    /// silence by a different route than the settled re-run above: `fresh` is 0
    /// because nothing was attempted successfully, not because everything was
    /// already there. Pinned separately so a future change to the `fresh == 0`
    /// guard cannot regress one path while the other keeps passing.
    #[test]
    fn a_wholly_degraded_workspace_counts_but_says_nothing() {
        let rows = vec![
            row("a", MemberOutcome::Degraded { reason: "boom".into() }),
            row("b", MemberOutcome::Degraded { reason: "boom".into() }),
        ];
        let f = footprint(&rows);
        assert_eq!((f.fresh, f.unchanged, f.degraded), (0, 0, 2));
        assert_eq!(f.members(), 2);
        assert!(f.notice().is_none(), "nothing was written, so nothing is advised");
    }

    /// The core API is total over an empty member set. `enable`'s only caller
    /// guards this case in the adapter today, so nothing else pins it — and a
    /// roll-up that panicked or narrated an empty workspace would be a surprise
    /// found by a future second caller, not by this suite.
    #[test]
    fn an_empty_member_set_has_no_footprint_and_no_notice() {
        let f = footprint(&[]);
        assert_eq!((f.fresh, f.unchanged, f.degraded), (0, 0, 0));
        assert_eq!(f.members(), 0);
        assert!(f.notice().is_none());
        // The FR-IN-04 explanation is a statement about the layout, not about
        // this run, so it is present even with nothing to report.
        assert!(!f.committed.is_empty() && !f.ignored.is_empty());
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
        //
        // `showUntrackedFiles` is pinned for the same reason `sh_git` pins the
        // identity — a host (or CI) whose gitconfig sets it to `all` expands the
        // one collapsed directory line into one line per file, failing this exact
        // string against code that is behaving perfectly.
        let status = Command::new("git")
            .arg("-C")
            .arg(&member)
            .args(["-c", "status.showUntrackedFiles=normal", "status", "--porcelain"])
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

    // ── WarmStartDisclosure (FR-WS-02, FR-WS-15, FR-WS-17, CR-119) ────────

    /// A settled re-run (no newly-approved delta) says nothing — mirroring
    /// `WorkingTreeFootprint`'s own `fresh == 0` silence.
    #[test]
    fn no_newly_warmed_members_has_no_notice() {
        let d = WarmStartDisclosure {
            members: 0,
            started: false,
            status_command: "logos workspace status",
        };
        assert!(d.notice().is_none());
    }

    /// The disclosure states the count and names the observation surface —
    /// this is the ONE workspace-level field the story requires, never
    /// per-member text.
    #[test]
    fn a_started_warm_names_the_count_and_the_status_surface() {
        let d = WarmStartDisclosure {
            members: 84,
            started: true,
            status_command: "logos workspace status",
        };
        let notice = d.notice().expect("a non-empty delta is worth a word");
        assert!(notice.contains("84"), "{notice}");
        assert!(notice.contains("has begun"), "{notice}");
        assert!(notice.contains("logos workspace status"), "{notice}");
    }

    /// A best-effort spawn failure is disclosed honestly rather than claimed as
    /// success — the warm is still covered by the FR-IX-07 lazy fallback, which
    /// the notice says so a reader is not left thinking nothing will happen.
    #[test]
    fn a_failed_spawn_is_disclosed_not_claimed() {
        let d = WarmStartDisclosure {
            members: 3,
            started: false,
            status_command: "logos workspace status",
        };
        let notice = d.notice().expect("still worth a word");
        assert!(!notice.contains("has begun"), "{notice}");
        assert!(notice.contains("FR-IX-07"), "the fallback is named: {notice}");
    }

    // ── ParentOfRepos: the `logos init` nudge shape (FR-IN-08) ────────────

    /// The positive case: a parent folder of sibling repositories, which is not
    /// itself a repository. `init` there builds a store that can never admit a
    /// file, and this is the detection that makes that sayable.
    #[test]
    fn a_parent_of_sibling_repos_is_detected_with_its_candidates() {
        let tmp = TempDir::new().unwrap();
        init_repo(&tmp.path().join("api"));
        init_repo(&tmp.path().join("web"));

        let shape = ParentOfRepos::detect(tmp.path()).expect("the parent-of-repos shape");
        let names: Vec<&str> = shape.candidates.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["api", "web"], "the members `--workspace` would offer, in name order");
    }

    /// The negative case [FR-IN-08] names outright: at an ordinary git
    /// repository root nothing fires — including one that *contains* sibling
    /// repositories (a vendored dependency, a submodule), where only the
    /// root's own git-top-level status separates "wrong directory" from
    /// "ordinary repository with nested checkouts".
    ///
    /// [FR-IN-08]: ../../../docs/specs/requirements/FR-IN-08.md
    #[test]
    fn an_ordinary_git_repository_root_is_never_detected() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("app");
        init_repo(&root);
        assert!(ParentOfRepos::detect(&root).is_none(), "a bare repository root is not the shape");

        init_repo(&root.join("vendor"));
        assert!(
            ParentOfRepos::detect(&root).is_none(),
            "a repository holding a nested checkout is still a repository, not a parent of repos"
        );
    }

    /// A non-repository folder with no member candidates has a different cause
    /// (empty, or holding plain directories), which the nudge must not
    /// misattribute — naming `init --workspace` there would find zero members.
    #[test]
    fn a_plain_folder_without_candidates_is_not_detected() {
        let tmp = TempDir::new().unwrap();
        assert!(ParentOfRepos::detect(tmp.path()).is_none(), "an empty folder is not the shape");

        fs::create_dir_all(tmp.path().join("notes/deep")).unwrap();
        fs::write(tmp.path().join("notes/a.md"), "x\n").unwrap();
        assert!(
            ParentOfRepos::detect(tmp.path()).is_none(),
            "plain child directories are not member candidates"
        );
    }

    /// **The structural assertion for the reuse requirement.** Detection does
    /// not re-derive the candidate set: what it reports IS
    /// [`discover_candidates`]' output at the same root.
    ///
    /// Asserted over the shape that discriminates — a child that is *not* a git
    /// root but already carries `.logos/logos.db`, which the federation
    /// primitive admits as a member and any hand-rolled `.git`-existence test
    /// would silently drop. A second implementation therefore cannot pass this
    /// while a genuine delegation cannot fail it.
    #[test]
    fn detection_reuses_the_federation_candidate_scan_rather_than_a_second_derivation() {
        let tmp = TempDir::new().unwrap();
        init_repo(&tmp.path().join("api"));
        // A non-git, already-indexed sibling: a member to `discover_candidates`,
        // invisible to a `.git`-existence check.
        let indexed = tmp.path().join("legacy");
        fs::create_dir_all(indexed.join(".logos")).unwrap();
        fs::write(indexed.join(".logos/logos.db"), b"").unwrap();

        let shape = ParentOfRepos::detect(tmp.path()).expect("the parent-of-repos shape");
        assert_eq!(
            shape.candidates,
            discover_candidates(tmp.path()),
            "the reported members are the federation primitive's own output, not a re-derivation"
        );
        let names: Vec<&str> = shape.candidates.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["api", "legacy"], "the already-indexed non-git sibling counts");
    }

    /// The explanation states the shape, samples the members, and names the
    /// remedy — reusing `index`'s own remedy constant so the two surfaces
    /// cannot drift into naming different commands.
    #[test]
    fn the_explanation_states_the_shape_and_names_the_shared_remedy() {
        let tmp = TempDir::new().unwrap();
        init_repo(&tmp.path().join("api"));
        init_repo(&tmp.path().join("web"));

        let text = ParentOfRepos::detect(tmp.path()).unwrap().to_string();
        assert!(text.contains("parent folder of sibling repositories"), "{text}");
        assert!(text.contains("api, web"), "the members are named: {text}");
        assert!(text.contains(ZeroAdmissionDiagnostic::REMEDY), "the remedy is named: {text}");
        assert!(!text.contains("more"), "nothing is elided below the sample limit: {text}");
    }

    /// Beyond the sample limit the remainder is elided rather than dumped — the
    /// 84-member workspace behind CR-098 must produce one readable line.
    #[test]
    fn the_explanation_elides_members_past_the_sample_limit() {
        let tmp = TempDir::new().unwrap();
        let names: Vec<String> = (0..6).map(|i| format!("svc-{i}")).collect();
        for name in &names {
            init_repo(&tmp.path().join(name));
        }

        let shape = ParentOfRepos::detect(tmp.path()).unwrap();
        let text = shape.to_string();
        assert_eq!(shape.candidates.len(), 6);

        // Driven by the constant, not by a copy of its value: the sample bound
        // and the elided remainder must move together if it is ever retuned.
        let limit = ParentOfRepos::SAMPLE_LIMIT;
        let named = names[..limit].join(", ");
        assert!(text.contains(&named), "the first {limit} are named: {text}");
        assert!(
            text.contains(&format!(", … +{} more", names.len() - limit)),
            "the remainder is elided: {text}"
        );
        assert!(!text.contains(&names[limit]), "an elided member is not also named: {text}");
    }

    /// The offer states the stake, and agrees in number with itself.
    #[test]
    fn the_question_states_how_many_repositories_would_be_enabled() {
        let tmp = TempDir::new().unwrap();
        init_repo(&tmp.path().join("api"));
        assert_eq!(
            ParentOfRepos::detect(tmp.path()).unwrap().question(false),
            "enable a Logos workspace here instead (1 member repository)?"
        );

        init_repo(&tmp.path().join("web"));
        assert_eq!(
            ParentOfRepos::detect(tmp.path()).unwrap().question(false),
            "enable a Logos workspace here instead (2 member repositories)?"
        );
    }

    /// `logos init -i` accepted at a parent-of-repos root silently drops the
    /// host-integration steps `-i` exists to install, because the FR-WS-02 path
    /// inits members with `InitOptions::default()`. The offer must say so before
    /// the user answers — the whole point of the nudge is to be non-surprising.
    #[test]
    fn the_question_names_the_host_setup_an_accepted_offer_would_not_apply() {
        let tmp = TempDir::new().unwrap();
        init_repo(&tmp.path().join("api"));
        let shape = ParentOfRepos::detect(tmp.path()).unwrap();

        let asked = shape.question(true);
        assert!(
            asked.contains("without the --interactive/--hooks host setup"),
            "an accepted offer must not trade `-i` away silently: {asked}"
        );
        // …and stays out of the way when the user asked for no such thing.
        assert!(!shape.question(false).contains("--interactive"), "unasked-for noise");
    }
}
