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
#[cfg(unix)]
use std::os::unix::process::CommandExt;
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
///
/// The report carries [`enable::WorkingTreeFootprint`] — enabling N members
/// dirties N repositories, and that no longer goes unsaid (FR-WS-02,
/// FR-IN-04). Its one-line prose form goes to stderr, so stdout stays exactly
/// one machine document.
pub(crate) fn run(root: &Path, yes: bool, exclude: &[String], out: &Output, warm: fn(&[Member], Option<usize>) -> bool) -> Result<i32> {
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
    // `warm_concurrency` rides along: the `[workspace.warm] concurrency`
    // override is read off the manifest by `federation::discover` (S-322,
    // FR-WS-01), so the value reaches the spawn from the same parse that
    // resolved the member set. `None` on a first run is correct rather than a
    // gap — a manifest that does not exist yet declares no override, and the
    // one this command writes never invents the table.
    let (name, already, declared_k) = match existing {
        Some(f) => (f.name, f.members, f.warm_concurrency),
        None => (default_name, Vec::new(), None),
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
    // ONE call with the whole delta — never a per-member loop, which is the
    // exact shape this story replaced. Injected (like `gate`'s `approve`) so a
    // test can assert the call count and the slice it received.
    warm(&approved_new, declared_k);

    out.print(&report)?;
    // The footprint rides the report itself, so both the human and `--json`
    // forms carry it. This extra line is the *prose* half — pre-composed by the
    // core (NFR-MA-02), and on stderr so stdout stays exactly one machine
    // document (FR-CL-02), the same split `xservice::run_workspace` uses for the
    // degraded roll-up.
    if let Some(notice) = report.footprint.notice().filter(|_| !out.quiet) {
        eprintln!("{notice}");
    }
    Ok(0)
}

/// **Should this invocation take the FR-WS-02 workspace path?** The whole
/// `init` routing decision, in one testable unit: `true` outright for an
/// explicit `--workspace`, otherwise the FR-IN-08 parent-of-repos nudge —
/// explain the shape on stderr, and on a TTY offer to take that path instead.
/// `false` at any other root, where nothing is printed and nothing is asked.
///
/// `--workspace` is answered **before** `detect` is called, so the explicit flag
/// still pays nothing for detection and never sees the nudge. That short-circuit
/// used to live in `dispatch`'s `workspace || nudge(..)`, where no test could
/// reach it: every integration test runs non-TTY, so `nudge` returned `false`
/// regardless and `if workspace || { nudge(..); false }` would have passed the
/// entire suite while the TTY offer silently stopped routing anywhere. Folding
/// the disjunction in makes both arms assertable without a terminal.
///
/// **The non-TTY contract is the load-bearing part.** `ask` is
/// [`crate::ask`], which returns its default *without prompting and without
/// reading stdin* when stdin is not a terminal — so an unattended dev-pane or
/// CI `logos init` prints the explanation and completes the plain single-root
/// init rather than wedging forever on a prompt. It is injected (like `gate`'s
/// `approve` above, and with the same `impl FnMut` bound) so that contract is
/// assertable without a terminal: the unit test below passes an `ask` that
/// behaves exactly as the non-TTY one does.
///
/// The default is **decline** — `false`, not `gate`'s `true`. Enabling a
/// workspace writes to N sibling repositories; a prompt the operator did not
/// ask for must not do that by timing out into yes.
///
/// Deliberately **not** `--quiet`-suppressed, unlike `run`'s footprint notice
/// above. That one is advisory about a side effect; this one is the whole
/// reason the command's own output is misleading — an `init` that can only ever
/// produce an empty index must say so (NFR-CC-04), and silencing it under
/// `--quiet` reinstates exactly the silence FR-IN-08 exists to remove. stdout is
/// unaffected either way (FR-CL-02), which is what `--quiet` actually protects.
///
/// `host_setup` is `--interactive || --hooks`: the FR-WS-02 path applies neither
/// (it inits members with `InitOptions::default()`), so the offer names what a
/// "yes" would drop rather than trading it away silently. It is passed rather
/// than decided here because the *sentence* is core-composed like the rest
/// (NFR-MA-02) — the adapter only knows which flags the user typed.
///
/// Composition lives in the core ([`enable::ParentOfRepos`]); this only renders
/// and prompts (NFR-MA-02).
pub(crate) fn nudge(root: &Path, workspace: bool, host_setup: bool, mut ask: impl FnMut(&str, bool) -> bool) -> bool {
    // `then` keeps `detect` lazy, so `--workspace` spawns no `git` at all; the
    // `else` arm then answers `workspace` itself — `true` for the explicit flag,
    // `false` for a root that is not the shape.
    let Some(shape) = (!workspace).then(|| enable::ParentOfRepos::detect(root)).flatten() else { return workspace };
    eprintln!("{shape}");
    ask(&shape.question(host_setup), false)
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
/// `--concurrency`: the FR-WS-01 `[workspace.warm] concurrency` override lands
/// at this one call, and the supervisor honours whatever argv it is given.
///
/// `declared_k` is the workspace's `[workspace.warm] concurrency`, already
/// range-validated at manifest parse time (FR-WS-01, S-322); `None` takes the
/// core-derived default. Resolution stays in the core
/// (`warm::effective_concurrency`) — this call site only forwards.
///
/// Best-effort throughout: an unresolvable `current_exe`, an OS out of
/// processes, or an empty delta simply leaves those members on the lazy
/// `ensure_indexed` fallback (FR-IX-07) — never fatal to the command.
pub(crate) fn spawn_supervisor(members: &[Member], declared_k: Option<usize>) -> bool {
    if members.is_empty() {
        return false;
    }
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let mut cmd = Command::new(exe);
    cmd.args(supervisor_argv(members, declared_k))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // Its OWN process group, so the warm survives the parent's *terminal*, not
    // just the parent's exit. Left in the invoking shell's foreground group,
    // the supervisor and its in-flight children take SIGINT on Ctrl-C and
    // SIGHUP when the window closes — and the bound turned that window from
    // "one member index" into "N/K × one member index", minutes on a large
    // workspace, all of it after the command already returned. A killed
    // in-flight index is also the one failure the FR-IX-07 lazy path does not
    // fully cover (see `run_supervisor`), so not taking the signal matters
    // beyond losing the warm.
    #[cfg(unix)]
    cmd.process_group(0);
    cmd.spawn().is_ok()
}

/// The supervisor's argv: **one** command line carrying the whole member
/// delta and the resolved bound. That the delta is arguments to a single
/// invocation — rather than the loop bound of N invocations — is what makes
/// the process count `1 + K` instead of `N` (FR-WS-14, BR-44); factored out so
/// that property is directly assertable without spawning anything.
///
/// Resolution happens **here** rather than at the spawn site for the same
/// reason: `declared_k` is the raw `[workspace.warm] concurrency` (S-322), and
/// taking it un-resolved is what lets a test assert that a declared K reaches
/// the argv — and that an absent one falls back to the core-derived default —
/// without a process. Resolving at the caller left the only line the story
/// exists for outside every test's reach.
fn supervisor_argv(members: &[Member], declared_k: Option<usize>) -> Vec<OsString> {
    // Everything after `--` is a positional, whatever it starts with. Member
    // roots are canonical absolute paths today, so no current root can look like
    // a flag — but the whole failure mode of this argv is invisibility (the
    // child's stderr is /dev/null and nobody reads its exit code), so a root
    // such as `-legacy` would silently exit 2 and lose the entire warm rather
    // than fail loudly. One token buys immunity.
    let k = warm::effective_concurrency(declared_k).to_string();
    [SUPERVISOR_COMMAND, "--concurrency", &k, "--"]
        .into_iter()
        .map(OsString::from)
        .chain(members.iter().map(|m| m.root.clone().into_os_string()))
        .collect()
}

/// The supervisor itself (FR-WS-14): drive
/// [`warm::warm_queue`] over `members` at the resolved bound, spawning and
/// awaiting one `logos --project <member> --quiet index` child per member and
/// never more than K at once.
///
/// This process opens no store of its own — no `Engine`, no `Runtime`, no
/// connection, and (uniquely among the subcommands) not even the telemetry
/// writer, which `main::run` skips for this arm precisely so the claim is
/// literal: it holds no `.logos` file at any point. `warm_queue` returns the
/// instant the last member finishes, which is this function's — and the
/// process's — exit.
///
/// Every failure here is advisory. A degraded member is reported on stderr and
/// never changes the exit code, and an unresolvable `current_exe` warms
/// nothing while still returning 0: nothing reads a detached child's status,
/// and correctness is carried by FR-IX-07, not by the warm. The stderr report
/// therefore exists for a foreground/diagnostic invocation — in the real
/// detached spawn it goes to `/dev/null`, and the durable per-member readout
/// is S-323's. Killing the supervisor leaves members it never reached on the
/// FR-IX-07 lazy path, exactly as an unspawned child does; a member killed
/// *mid*-index is the one case that path does not fully cover (see
/// [`warm`]'s module docs), which is why the spawn takes its own process group.
pub(crate) fn run_supervisor(members: &[PathBuf], concurrency: Option<usize>) -> i32 {
    let Ok(exe) = std::env::current_exe() else {
        return 0;
    };
    let bound = warm::effective_concurrency(concurrency);
    let summary = warm::warm_queue(members, bound, |root| index_member(&exe, root));
    for m in &summary.members {
        if let Some(reason) = &m.degraded {
            eprintln!("logos workspace warm: {} degraded — {reason}", m.root);
        }
    }
    0
}

/// Index one member to completion: this same binary as `logos --project
/// <member> --quiet index`, awaited (`status`, not `spawn`) because the whole
/// bound rests on a worker slot staying occupied until its member is done.
/// stdio is nulled — a background warm never writes to the user's terminal.
fn index_member(exe: &Path, member_root: &Path) -> Result<(), String> {
    // `--project=<path>` (attached, not a detached value) for the same
    // hyphen-safety reason as the `--` above: a `-`-leading root passed as a
    // separate value would be parsed as a flag by the child.
    let mut project = OsString::from("--project=");
    project.push(member_root);
    let status = Command::new(exe)
        .arg(project)
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
        let argv = supervisor_argv(&members, Some(3));

        let rendered: Vec<&str> = argv.iter().map(|a| a.to_str().unwrap()).collect();
        assert_eq!(
            rendered,
            [SUPERVISOR_COMMAND, "--concurrency", "3", "--", "m0", "m1", "m2", "m3", "m4"],
            "one invocation carries every member and the resolved bound"
        );
    }

    /// A member root beginning with `-` must reach the child as a member, not
    /// as a flag. Without the `--` separator the detached child exits 2 on a
    /// usage error with all three streams nulled, so the entire warm vanishes
    /// with no diagnostic anywhere.
    #[test]
    fn a_hyphen_leading_member_root_still_parses_as_a_member() {
        use clap::Parser;

        let members = vec![member("-legacy"), member("--weird")];
        let mut full = vec![OsString::from("logos")];
        full.extend(supervisor_argv(&members, Some(1)));

        let cli = crate::Cli::try_parse_from(full).expect("a hyphen-leading root still parses");
        let crate::Commands::InternalWarm { members: parsed, .. } = cli.command else {
            panic!("must route to the supervisor variant");
        };
        assert_eq!(parsed, [PathBuf::from("-legacy"), PathBuf::from("--weird")]);
    }

    /// The whole delta rides one command line at scale too — the 84-member
    /// workspace that motivated the bound is ~7 KB of argv, far under ARG_MAX.
    /// Free to assert: no process is spawned.
    #[test]
    fn the_argv_carries_every_member_of_a_large_delta() {
        let members: Vec<Member> = (0..200).map(|i| member(&format!("m{i}"))).collect();
        let argv = supervisor_argv(&members, Some(4));
        assert_eq!(argv.len(), 204, "4 head tokens + 200 members, one command line");
    }

    /// And that single command line is one the real parser accepts — the argv
    /// builder and the clap variant are checked against each other, not each
    /// against an assumption.
    #[test]
    fn the_built_argv_parses_back_into_the_supervisor_variant() {
        use clap::Parser;

        let members = vec![member("api"), member("web")];
        let mut full = vec![OsString::from("logos")];
        full.extend(supervisor_argv(&members, Some(2)));

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
    ///
    /// Asserts the *decision*, not just that the call is survivable: before
    /// `spawn_supervisor` reported it, deleting the empty-delta guard left
    /// every test green while every re-run spawned a stray child.
    #[test]
    fn no_newly_approved_members_spawns_no_supervisor() {
        assert!(
            !spawn_supervisor(&[], None),
            "an empty delta must spawn nothing at all"
        );
    }

    // ── the warm is invoked ONCE, with the whole delta (BR-44) ────────────

    fn git(cwd: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(["-c", "user.email=t@logos", "-c", "user.name=logos-test"])
            .args(args)
            .output()
            .expect("git is on PATH");
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    /// A parent folder holding `n` committed sibling repos.
    fn fixture(names: &[&str]) -> tempfile::TempDir {
        let tmp = tempfile::TempDir::new().unwrap();
        for name in names {
            let dir = tmp.path().join(name);
            std::fs::create_dir_all(&dir).unwrap();
            git(&dir, &["init", "-q", "-b", "main"]);
            std::fs::write(dir.join("a.rs"), "fn a() {}
").unwrap();
            git(&dir, &["add", "."]);
            git(&dir, &["commit", "-q", "-m", "init"]);
        }
        tmp
    }

    /// The regression this story exists to prevent is a per-member **loop** at
    /// this call site. Asserting the argv shape cannot catch that — a loop
    /// calling `spawn_supervisor` once per member builds N perfectly-shaped
    /// single-member command lines. Only the call count can, so `run` takes
    /// the warm as a parameter and the count is asserted here.
    #[test]
    fn run_invokes_the_warm_exactly_once_with_the_whole_delta() {
        use std::sync::Mutex;
        static SEEN: Mutex<Vec<Vec<String>>> = Mutex::new(Vec::new());

        fn record(members: &[Member], _k: Option<usize>) -> bool {
            SEEN.lock()
                .unwrap()
                .push(members.iter().map(|m| m.name.clone()).collect());
            true
        }

        let tmp = fixture(&["api", "web"]);
        SEEN.lock().unwrap().clear();
        let out = Output { json: true, quiet: true };
        assert_eq!(run(tmp.path(), true, &[], &out, record).unwrap(), 0);

        let seen = SEEN.lock().unwrap().clone();
        assert_eq!(
            seen.len(),
            1,
            "exactly ONE warm invocation for N members, never one per member: {seen:?}"
        );
        let mut names = seen[0].clone();
        names.sort();
        assert_eq!(names, ["api", "web"], "the single invocation carries the whole delta");
    }

    /// The declared `[workspace.warm] concurrency` is the K the supervisor
    /// actually honours (S-322, FR-WS-01, BR-44) — asserted end to end at this
    /// seam, because every link in the chain is individually plausible while
    /// the chain is broken: the key can parse, ride the `Federation`, and still
    /// never reach the argv if this call site forwards `None`.
    #[test]
    fn a_declared_warm_concurrency_reaches_the_supervisor_argv() {
        use std::sync::Mutex;
        static SEEN: Mutex<Vec<Option<usize>>> = Mutex::new(Vec::new());

        fn record(_members: &[Member], k: Option<usize>) -> bool {
            SEEN.lock().unwrap().push(k);
            true
        }

        let tmp = fixture(&["api", "web"]);
        // A hand-written manifest declaring the override, with no members yet:
        // both siblings are still a fresh delta, so the warm is invoked.
        std::fs::write(
            tmp.path().join(logos_core::federation::MANIFEST_FILENAME),
            "[workspace]\nname = \"pec\"\n\n[workspace.warm]\nconcurrency = 2\n",
        )
        .unwrap();

        SEEN.lock().unwrap().clear();
        let out = Output { json: true, quiet: true };
        assert_eq!(run(tmp.path(), true, &[], &out, record).unwrap(), 0);

        let seen = SEEN.lock().unwrap().clone();
        assert_eq!(seen, [Some(2)], "the declared override reaches the warm");

        // …and survives resolution into the argv the supervisor is spawned with.
        // Asserted through `supervisor_argv` itself, which owns the resolution
        // `spawn_supervisor` hands it: re-composing `effective_concurrency` in
        // the test body instead would assert the test's own arithmetic, and
        // left `spawn_supervisor` forwarding `None` undetectable.
        let argv = supervisor_argv(&[member("api")], Some(2));
        assert_eq!(argv[1], OsString::from("--concurrency"));
        assert_eq!(argv[2], OsString::from("2"), "the declared K, not the default");

        // The other direction, which is what makes the assertion above mean
        // something: no declaration resolves to the core-derived default.
        let argv = supervisor_argv(&[member("api")], None);
        assert_eq!(
            argv[2],
            OsString::from(warm::default_concurrency().to_string()),
            "absent ⇒ the core-derived default"
        );
    }

    /// A re-run warms only the newly approved delta — the already-manifested
    /// members were warmed (or fell back to lazy indexing) on the run that
    /// first added them. Handing the supervisor the full member set instead is
    /// a plausible regression that no end-to-end assertion would notice.
    #[test]
    fn a_rerun_hands_the_warm_only_the_new_delta() {
        use std::sync::Mutex;
        static SEEN: Mutex<Vec<Vec<String>>> = Mutex::new(Vec::new());

        fn record(members: &[Member], _k: Option<usize>) -> bool {
            SEEN.lock()
                .unwrap()
                .push(members.iter().map(|m| m.name.clone()).collect());
            true
        }

        let tmp = fixture(&["api", "web"]);
        let out = Output { json: true, quiet: true };
        assert_eq!(run(tmp.path(), true, &[], &out, record).unwrap(), 0);

        // A third sibling appears only for the second run.
        let batch = tmp.path().join("batch");
        std::fs::create_dir_all(&batch).unwrap();
        git(&batch, &["init", "-q", "-b", "main"]);
        std::fs::write(batch.join("a.rs"), "fn a() {}\n").unwrap();
        git(&batch, &["add", "."]);
        git(&batch, &["commit", "-q", "-m", "init"]);

        SEEN.lock().unwrap().clear();
        assert_eq!(run(tmp.path(), true, &[], &out, record).unwrap(), 0);

        let seen = SEEN.lock().unwrap().clone();
        assert_eq!(seen.len(), 1, "still one invocation: {seen:?}");
        assert_eq!(
            seen[0], ["batch"],
            "only the newly approved member is warmed, not the manifested pair"
        );
    }

    /// Draining an empty queue is a no-op that reports success, so the
    /// supervisor's own exit path is exercised without launching an indexer.
    #[test]
    fn the_supervisor_over_an_empty_queue_exits_zero() {
        assert_eq!(run_supervisor(&[], Some(2)), 0);
    }

    // ── the plain-`init` parent-of-repos nudge (FR-IN-08) ─────────────────

    /// **The load-bearing non-TTY assertion.** `crate::ask` returns its default
    /// without prompting and without touching stdin when stdin is not a
    /// terminal; this passes an `ask` that does exactly that, and the answer is
    /// decline. So an unattended dev-pane or CI `logos init` at a parent-of-repos
    /// root explains itself and completes the plain single-root init — it cannot
    /// wedge on a prompt, and it cannot silently enable a workspace across N
    /// sibling repositories either.
    ///
    /// The real binary's behaviour is pinned end to end in
    /// `cli/tests/init_workspace.rs`; this pins the *decision* at the seam,
    /// where a change of default would otherwise pass unnoticed.
    #[test]
    fn a_non_tty_ask_declines_into_the_plain_init() {
        let tmp = fixture(&["api", "web"]);
        // Exactly what `crate::ask` does on a non-TTY: yield the default,
        // reading nothing.
        assert!(
            !nudge(tmp.path(), false, false, |_, default| default),
            "a non-TTY answer is the default, and the default is decline"
        );
    }

    /// On a TTY the offer is a real offer: accepting routes the caller into the
    /// FR-WS-02 enablement path.
    #[test]
    fn accepting_the_offer_reports_true() {
        let tmp = fixture(&["api", "web"]);
        assert!(nudge(tmp.path(), false, false, |_, _| true), "an accepted offer branches to --workspace");
    }

    /// An explicit `--workspace` is answered `true` **without** detecting: no
    /// explanation, no offer, and — the half that used to be untestable — no
    /// candidate scan at all. Asserted at a parent-of-repos root, the one place
    /// where a detection that did run would visibly fire.
    #[test]
    fn an_explicit_workspace_flag_short_circuits_before_any_detection() {
        let tmp = fixture(&["api", "web"]);
        let mut asked = 0;
        assert!(
            nudge(tmp.path(), true, false, |_, _| {
                asked += 1;
                false
            }),
            "--workspace routes to the enablement path on its own"
        );
        assert_eq!(asked, 0, "and is never offered a choice it did not ask for");
    }

    /// The other short-circuit arm: not `--workspace`, not the shape ⇒ `false`,
    /// which is what keeps an ordinary `logos init` on the plain path.
    #[test]
    fn neither_the_flag_nor_the_shape_declines() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("notes")).unwrap();
        assert!(!nudge(tmp.path(), false, false, |_, _| true), "no flag and no shape ⇒ plain init");
    }

    /// …and the question the operator is asked names the stake, so "yes" is
    /// informed about how many repositories it is about to touch.
    #[test]
    fn the_offer_names_how_many_repositories_it_would_enable() {
        let tmp = fixture(&["api", "web"]);
        let mut asked: Vec<String> = Vec::new();
        assert!(!nudge(tmp.path(), false, false, |q, d| {
            asked.push(q.to_string());
            d
        }));

        assert_eq!(asked.len(), 1, "asked exactly once, not once per member: {asked:?}");
        assert!(asked[0].contains("2 member repositories"), "{}", asked[0]);
    }

    /// At an ordinary git repository root there is no nudge and no prompt —
    /// asserted by the `ask` never being consulted, which is the only way to
    /// tell "declined" from "never offered".
    #[test]
    fn an_ordinary_repository_root_is_never_nudged_and_never_asked() {
        // A repository that even *contains* sibling checkouts: only its own
        // git-top-level status separates it from the parent-of-repos shape.
        let tmp = fixture(&["api"]);
        let repo = tmp.path().join("api");
        std::fs::create_dir_all(repo.join("vendor")).unwrap();
        git(&repo.join("vendor"), &["init", "-q", "-b", "main"]);

        let mut asks = 0;
        assert!(
            !nudge(&repo, false, false, |_, _| {
                asks += 1;
                true
            }),
            "a repository root is not the shape"
        );
        assert_eq!(asks, 0, "no prompt is ever reached there");
    }

    /// An accepted offer must actually produce a workspace — the two halves the
    /// dispatch line joins, composed here. A real pty is deliberately not used:
    /// the TTY branch of `crate::ask` is stdlib `IsTerminal`, and pulling a pty
    /// dependency in to re-test it would test the standard library rather than
    /// this story.
    #[test]
    fn an_accepted_offer_leads_to_a_written_workspace_manifest() {
        let tmp = fixture(&["api", "web"]);
        assert!(nudge(tmp.path(), false, false, |_, _| true));

        let out = Output { json: true, quiet: true };
        assert_eq!(run(tmp.path(), true, &[], &out, |_, _| true).unwrap(), 0);
        assert!(
            tmp.path().join(logos_core::federation::MANIFEST_FILENAME).is_file(),
            "accepting writes logos.workspace.toml via the FR-WS-02 path"
        );
    }
}
