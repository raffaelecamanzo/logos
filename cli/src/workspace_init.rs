//! `logos init --workspace` (FR-WS-02) — the CLI-surface pieces that must
//! not live in the core: the interactive per-candidate approval gate
//! (terminal I/O) and the best-effort background index warm (this binary
//! re-invoked as a detached child process). Candidate discovery, the
//! non-clobber per-member `init`, the incremental manifest write, and the
//! workspace MCP injection are all core business logic
//! ([`logos_core::federation::enable`]) — this module only resolves the
//! anchor, gates, wires the warm, and reports.
//!
//! The warm is **bounded** (FR-WS-14, BR-44): [`spawn_supervisor`] starts
//! exactly one detached child whatever N is, and that child re-enters this
//! module at [`run_supervisor`], which drives
//! [`logos_core::federation::warm::warm_queue`] over the member delta. The
//! scheduling decisions and the bound resolution are core business logic; the
//! two things that genuinely cannot live there — detaching a process that
//! outlives its parent, and spawning/awaiting an `index` child — are here.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::Result;
use logos_core::federation::{self, enable, warm, Member};

use crate::{ask, Output};

/// The supervisor's own subcommand name — **internal**, hidden from help
/// output, and not a public CLI contract (FR-CL-01, FR-WS-14 Notes). Named
/// here so the spawn and the clap variant cannot drift apart silently (the
/// unit test below parses this literal through the real parser).
pub(crate) const SUPERVISOR_COMMAND: &str = "internal-warm";

/// `logos init --workspace [--yes] [--exclude <glob>]...` (FR-WS-02).
///
/// Anchors on an existing manifest via [`federation::discover`] when one is
/// found up-tree (the incremental re-run case — its already-resolved,
/// already-pruned `members` are kept without re-prompting); otherwise `root`
/// itself becomes the new workspace root. Newly discovered candidates go
/// through the approval gate (or `--yes`/`--exclude`), then
/// [`enable::enable`] runs the non-clobber per-member `init`, the manifest
/// upsert, and the workspace MCP injection.
///
/// Returns without blocking on indexing: the **newly** approved members are
/// warmed by a single detached background supervisor at a bounded concurrency
/// ([`spawn_supervisor`], FR-WS-14) — best-effort, never awaited. Members
/// already in the manifest are not re-warmed on every re-run (they were
/// warmed, or fell back to lazy indexing, on the run that first added them) —
/// only the delta this invocation approves. A member whose warm never starts,
/// waits behind the bound, or hasn't finished before first real use still
/// indexes correctly via the engine's lazy `ensure_indexed` fallback
/// (FR-IX-07), so this command never needs to wait on it.
pub(crate) fn run(root: &Path, yes: bool, exclude: &[String], out: &Output) -> Result<i32> {
    let existing = federation::discover(root)?;
    let workspace_root = existing
        .as_ref()
        .map_or_else(|| root.to_path_buf(), |f| f.root.clone());
    // Canonicalise before deriving the name: the common invocation (`cd` into
    // the parent folder, run `logos init --workspace` with no `--project`)
    // has `root == "."`, whose `file_name()` is `None` — canonicalising
    // resolves it to the real directory name. This name is written verbatim
    // into a fresh manifest and preserved on every re-run
    // (`federation::manifest::upsert`), so getting it right here matters once.
    let default_name = workspace_root
        .canonicalize()
        .unwrap_or_else(|_| workspace_root.clone())
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("workspace")
        .to_string();
    let (name, already) = match existing {
        Some(f) => (f.name, f.members),
        None => (default_name, Vec::new()),
    };
    let already_names: Vec<String> = already.iter().map(|m| m.name.clone()).collect();

    let proposed = enable::candidates_for_approval(&workspace_root, &already_names, exclude)?;
    let approved_new = gate(&proposed, yes, |m| {
        ask(
            &format!(
                "include member \"{}\" ({}) in the workspace?",
                m.name,
                m.root.display()
            ),
            true,
        )
    });

    let mut members = already;
    members.extend(approved_new.iter().cloned());

    if members.is_empty() {
        eprintln!(
            "logos init --workspace: no candidate member repositories found under {} — nothing to do",
            workspace_root.display()
        );
        return Ok(0);
    }

    let report = enable::enable(&workspace_root, &name, &members)?;
    spawn_supervisor(&approved_new);

    out.print(&report)?;
    Ok(0)
}

/// Filter `candidates` down to the approved set: `--yes` accepts every one
/// without prompting (the scriptable path); otherwise each is offered to
/// `approve` (the real gate calls [`ask`] — a stderr y/n prompt, defaulting
/// to include, that degrades to `true` without printing anything on a
/// non-TTY stdin). Taking `approve` as a parameter keeps this filtering logic
/// testable without a real terminal.
fn gate(candidates: &[Member], yes: bool, mut approve: impl FnMut(&Member) -> bool) -> Vec<Member> {
    candidates
        .iter()
        .filter(|m| yes || approve(m))
        .cloned()
        .collect()
}

/// Start the **single** detached warm supervisor for the newly approved
/// members (FR-WS-14, BR-44) — one child process whatever N is, replacing the
/// per-member `logos index` fan-out that made the process and peak-RSS cost
/// scale with the member count (84 members meant 84 concurrent rayon-parallel
/// indexers, breaching NFR-PE-06 ~84× and NFR-PE-08's no-contention premise).
///
/// Detached, not a thread: `init --workspace` must return without blocking
/// (FR-WS-02) *and* the warm must outlive it — a thread would be killed with
/// the process, a spawned child is reparented. The effective bound is resolved
/// **here**, in the parent that owns the workspace context, and passed down as
/// `--concurrency`, so the manifest override of FR-WS-01/S-322 lands at this
/// one call without the supervisor changing.
///
/// Best-effort throughout: an unresolvable `current_exe`, an OS out of
/// processes, or an empty delta simply leaves those members on the lazy
/// `ensure_indexed` fallback (FR-IX-07) — never fatal to the command.
fn spawn_supervisor(members: &[Member]) {
    if members.is_empty() {
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let _ = Command::new(exe)
        .args(supervisor_argv(members, warm::effective_concurrency(None)))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

/// The supervisor's argv: **one** command line carrying the whole member
/// delta and the resolved bound. That the delta is arguments to a single
/// invocation — rather than the loop bound of N invocations — is what makes
/// the process count `1 + K` instead of `N` (FR-WS-14, BR-44); factored out so
/// that property is directly assertable without spawning anything.
fn supervisor_argv(members: &[Member], bound: usize) -> Vec<OsString> {
    let mut argv = vec![
        OsString::from(SUPERVISOR_COMMAND),
        OsString::from("--concurrency"),
        OsString::from(bound.to_string()),
    ];
    argv.extend(members.iter().map(|m| m.root.clone().into_os_string()));
    argv
}

/// The supervisor itself (FR-WS-14): drive
/// [`warm::warm_queue`] over `members` at the resolved bound, spawning and
/// awaiting one `logos --project <member> --quiet index` child per member and
/// never more than K at once.
///
/// This process opens no store — it only supervises children — so it holds no
/// `.logos` lock at any point, and `warm_queue` returns the instant the last
/// member finishes, which is this function's (and the process's) exit. A
/// degraded member is reported on stderr and never changes the exit code: the
/// warm is advisory, and nothing reads a detached child's status. Killing it
/// mid-queue leaves the unwarmed members on the FR-IX-07 lazy path, exactly as
/// an unspawned child does.
pub(crate) fn run_supervisor(members: &[PathBuf], concurrency: Option<usize>) -> i32 {
    let Ok(exe) = std::env::current_exe() else {
        return 0;
    };
    let bound = warm::effective_concurrency(concurrency);
    let summary = warm::warm_queue(members, bound, |root| index_member(&exe, root));
    for (root, reason) in summary
        .members
        .iter()
        .filter_map(|m| m.degraded.as_ref().map(|r| (&m.root, r)))
    {
        eprintln!("logos workspace warm: {root} degraded — {reason}");
    }
    0
}

/// Index one member to completion: this same binary as `logos --project
/// <member> --quiet index`, awaited (`status`, not `spawn`) because the whole
/// bound rests on a worker slot staying occupied until its member is done.
/// stdio is nulled — a background warm never writes to the user's terminal.
///
/// Spelled `std::result::Result` because this module imports `anyhow::Result`
/// for [`run`], and the failure here is a plain reason string handed to
/// [`warm::warm_queue`], not an `anyhow::Error`.
fn index_member(exe: &Path, member_root: &Path) -> std::result::Result<(), String> {
    let status = Command::new(exe)
        .arg("--project")
        .arg(member_root)
        .arg("--quiet")
        .arg("index")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|err| format!("spawn failed: {err}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("index failed: {status}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn member(name: &str) -> Member {
        Member {
            name: name.to_string(),
            root: PathBuf::from(name),
        }
    }

    #[test]
    fn yes_includes_every_candidate_without_prompting() {
        let candidates = vec![member("a"), member("b")];
        let mut asked = 0;
        let approved = gate(&candidates, true, |_| {
            asked += 1;
            false
        });
        assert_eq!(approved.len(), 2, "every candidate included under --yes");
        assert_eq!(asked, 0, "the predicate is never consulted under --yes");
    }

    #[test]
    fn gate_filters_by_the_approval_predicate() {
        let candidates = vec![member("a"), member("b")];
        let approved = gate(&candidates, false, |m| m.name == "a");
        let names: Vec<&str> = approved.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["a"]);
    }

    #[test]
    fn gate_over_no_candidates_is_empty() {
        assert!(gate(&[], false, |_| true).is_empty());
    }

    // ── the supervisor entry point (FR-WS-14, FR-CL-01) ───────────────────

    /// The spawn names [`SUPERVISOR_COMMAND`] as a bare string, so the clap
    /// variant must answer to that exact literal — otherwise a rename (or
    /// clap's own kebab-casing) would silently turn every warm into an
    /// exit-2 usage error inside a detached, stdio-nulled child nobody
    /// watches.
    #[test]
    fn the_supervisor_command_literal_parses_through_the_real_parser() {
        use clap::Parser;

        let cli = crate::Cli::try_parse_from([
            "logos",
            SUPERVISOR_COMMAND,
            "--concurrency",
            "3",
            "/tmp/a",
            "/tmp/b",
        ])
        .expect("the spawned argv parses");

        let crate::Commands::InternalWarm {
            concurrency,
            members,
        } = cli.command
        else {
            panic!("the literal must route to the supervisor variant");
        };
        assert_eq!(concurrency, Some(3));
        assert_eq!(members, [PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b")]);
    }

    /// Invoked without `--concurrency` (a human poking at it, or a future
    /// caller that has no bound to pass), the supervisor still resolves one.
    #[test]
    fn the_supervisor_command_accepts_no_concurrency_and_no_members() {
        use clap::Parser;

        let cli = crate::Cli::try_parse_from(["logos", SUPERVISOR_COMMAND])
            .expect("both arguments are optional");
        assert!(matches!(
            cli.command,
            crate::Commands::InternalWarm {
                concurrency: None,
                ..
            }
        ));
    }

    /// The whole delta rides **one** command line, so approving N members
    /// spawns one supervisor rather than N indexers (BR-44) — the argv is
    /// `internal-warm --concurrency K <root>…`, never N of them.
    #[test]
    fn the_whole_delta_becomes_a_single_supervisor_command_line() {
        let members: Vec<Member> = (0..5).map(|i| member(&format!("m{i}"))).collect();
        let argv = supervisor_argv(&members, 3);

        let rendered: Vec<&str> = argv.iter().map(|a| a.to_str().unwrap()).collect();
        assert_eq!(
            rendered,
            [SUPERVISOR_COMMAND, "--concurrency", "3", "m0", "m1", "m2", "m3", "m4"],
            "one invocation carries every member and the resolved bound"
        );
    }

    /// And that single command line is one the real parser accepts — the argv
    /// builder and the clap variant are checked against each other, not each
    /// against an assumption.
    #[test]
    fn the_built_argv_parses_back_into_the_supervisor_variant() {
        use clap::Parser;

        let members = vec![member("api"), member("web")];
        let mut full = vec![OsString::from("logos")];
        full.extend(supervisor_argv(&members, 2));

        let cli = crate::Cli::try_parse_from(full).expect("the spawned argv parses");
        let crate::Commands::InternalWarm {
            concurrency,
            members: parsed,
        } = cli.command
        else {
            panic!("must route to the supervisor variant");
        };
        assert_eq!(concurrency, Some(2));
        assert_eq!(parsed, [PathBuf::from("api"), PathBuf::from("web")]);
    }

    /// An empty delta (the common incremental re-run: nothing new approved)
    /// must not spawn a supervisor at all — one process to immediately exit
    /// is pure waste on every re-run of a settled workspace.
    #[test]
    fn no_newly_approved_members_spawns_no_supervisor() {
        // `spawn_supervisor` returns before touching `Command` for an empty
        // slice; if it did not, this would leave a stray child behind.
        spawn_supervisor(&[]);
    }

    /// Draining an empty queue is a no-op that reports success, so the
    /// supervisor's own exit path is exercised without launching an indexer.
    #[test]
    fn the_supervisor_over_an_empty_queue_exits_zero() {
        assert_eq!(run_supervisor(&[], Some(2)), 0);
    }
}
