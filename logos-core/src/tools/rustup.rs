//! The `llvm-tools-preview` step of a `cargo-llvm-cov` install ([FR-CV-17],
//! [ADR-88] §7, S-748): the LLVM tools `cargo-llvm-cov` reads its profiles
//! with are a component of the user's Rust toolchain, added by the user's own
//! `rustup`, never downloaded by Logos.
//!
//! Two fixed command lines, and nothing else, are ever run:
//! - `rustup component list --installed`, to read whether the component is
//!   there;
//! - `rustup component add llvm-tools-preview`, when it is not.
//!
//! Both run only from [`Installer::add_llvm_tools`], once the install consent
//! names the command.
//!
//! `rustup` is looked up on the installer's search path — never an entry
//! inside the project — and runs from the filesystem root ([`RUSTUP_CWD`]),
//! so that no `rust-toolchain.toml` of any project picks the toolchain or
//! makes `rustup` install one: the component goes to the user's active
//! toolchain as their own environment selects it. Without `rustup` nothing is
//! run and nothing changes; the step reports [`RUSTUP_NOT_FOUND`], which names
//! what to do by hand.
//!
//! [FR-CV-17]: ../../../docs/specs/requirements/FR-CV-17.md
//! [ADR-88]: ../../../docs/specs/architecture/decisions/ADR-88.md

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Serialize;

use super::{exited_cleanly, spawn, FailStage, InstallOutcome, Installer};
use crate::jobs::spawn::Launch;

/// The component `cargo-llvm-cov` needs.
pub const LLVM_TOOLS: &str = "llvm-tools-preview";

/// The command that adds it, as the consent names it and as a user runs it by
/// hand.
pub const LLVM_TOOLS_ADD: &str = "rustup component add llvm-tools-preview";

/// Why the component is not added when no `rustup` is on the search path.
pub const RUSTUP_NOT_FOUND: &str = "rustup not found: nothing was changed on the Rust toolchain. \
     Add the llvm-tools-preview component by hand (`rustup component add llvm-tools-preview` on a \
     rustup toolchain), or point cargo-llvm-cov at LLVM's tools with LLVM_COV and LLVM_PROFDATA";

/// `rustup component list --installed`.
const LIST_ARGS: [&str; 3] = ["component", "list", "--installed"];

/// `rustup component add llvm-tools-preview`.
const ADD_ARGS: [&str; 3] = ["component", "add", LLVM_TOOLS];

/// The listed name the component is installed under: `llvm-tools-<target>`,
/// or `llvm-tools-preview-<target>` on a `rustup` from before the rename.
const INSTALLED_PREFIX: &str = "llvm-tools-";

/// The working directory of every `rustup` run. `rustup` reads a
/// `rust-toolchain.toml` in its working directory **and every directory above
/// it**; the filesystem root has none above it, and only an administrator can
/// write there.
#[cfg(unix)]
const RUSTUP_CWD: &str = "/";
#[cfg(not(unix))]
const RUSTUP_CWD: &str = "\\";

/// How long the listing may take.
const LIST_DEADLINE: Duration = Duration::from_secs(60);
/// How long adding the component may take: `rustup` downloads it.
const ADD_DEADLINE: Duration = Duration::from_secs(10 * 60);

/// What the `llvm-tools-preview` step of an install did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ComponentOutcome {
    /// `rustup` already listed it; nothing was run.
    AlreadyPresent,
    /// `rustup component add llvm-tools-preview` succeeded.
    Added,
    /// It did not; `reason` says how, with what `rustup` printed.
    Failed { reason: String },
    /// No `rustup`: nothing was run and the toolchain is as it was. `command`
    /// is what to run by hand.
    NoRustup { reason: &'static str, command: &'static str },
    /// The job the step ran on was cancelled; `rustup` was killed or never
    /// started.
    Cancelled,
}

impl Installer {
    /// Add `llvm-tools-preview` to the user's active toolchain with
    /// `rustup component add llvm-tools-preview`, unless `rustup` already
    /// lists it. Without `rustup`, runs nothing. The caller has the install
    /// consent for it: this asks for none.
    pub fn add_llvm_tools(&self) -> ComponentOutcome {
        let Some((rustup, path)) = self.rustup() else {
            return no_rustup();
        };
        if listed_installed(&rustup, path.as_ref()) {
            return ComponentOutcome::AlreadyPresent;
        }
        let started = Instant::now();
        let outcome = match run_rustup(&rustup, &ADD_ARGS, path.as_ref(), ADD_DEADLINE) {
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => ComponentOutcome::Cancelled,
            Err(e) => ComponentOutcome::Failed {
                reason: format!("could not start {}: {e}", rustup.display()),
            },
            Ok(_) if crate::jobs::cancelled() => ComponentOutcome::Cancelled,
            Ok(finished) => match exited_cleanly("rustup", &finished, FailStage::Install) {
                Ok(()) => ComponentOutcome::Added,
                Err(InstallOutcome::Failed { reason, .. }) => ComponentOutcome::Failed { reason },
                Err(_) => ComponentOutcome::Failed {
                    reason: "rustup did not finish".to_string(),
                },
            },
        };
        log_component(started, &outcome);
        outcome
    }

    /// Whether a `rustup` is on the search path: looked up, never run.
    pub(crate) fn has_rustup(&self) -> bool {
        self.rustup().is_some()
    }

    /// The `rustup` on the search path, and the `PATH` it runs with.
    fn rustup(&self) -> Option<(PathBuf, Option<OsString>)> {
        let (search_dirs, path) = self.child_search();
        spawn::find_on_path(&search_dirs, "rustup").map(|rustup| (rustup, path))
    }
}

/// The step's outcome without `rustup`.
pub(crate) fn no_rustup() -> ComponentOutcome {
    ComponentOutcome::NoRustup {
        reason: RUSTUP_NOT_FOUND,
        command: LLVM_TOOLS_ADD,
    }
}

/// Whether `rustup` lists `llvm-tools-preview` installed on the user's active
/// toolchain. A listing that fails reads as not installed: adding the
/// component is what tells.
fn listed_installed(rustup: &Path, path: Option<&OsString>) -> bool {
    run_rustup(rustup, &LIST_ARGS, path, LIST_DEADLINE)
        .ok()
        .filter(|f| f.status.is_some_and(|s| s.success()))
        .is_some_and(|finished| lists_llvm_tools(&finished.stdout))
}

/// Whether `rustup component list --installed` printed the component: one
/// line that is `llvm-tools-<target>` (or the pre-rename
/// `llvm-tools-preview-<target>`).
fn lists_llvm_tools(listing: &str) -> bool {
    listing.lines().map(str::trim).any(|line| {
        line.strip_prefix(INSTALLED_PREFIX)
            .is_some_and(|target| !target.is_empty() && !target.contains(char::is_whitespace))
    })
}

fn run_rustup(
    rustup: &Path,
    args: &[&str],
    path: Option<&OsString>,
    deadline: Duration,
) -> std::io::Result<spawn::Finished> {
    let args: Vec<OsString> = args.iter().map(OsString::from).collect();
    let launch = Launch {
        cwd: Path::new(RUSTUP_CWD),
        env: &[],
        vars: &[],
        isolated: None,
        path,
        deadline: Some(deadline),
    };
    spawn::run(rustup, &args, &launch)
}

/// The step's event, on the install events' target: never what `rustup`
/// printed.
fn log_component(started: Instant, outcome: &ComponentOutcome) {
    let token = match outcome {
        ComponentOutcome::AlreadyPresent => "already_present",
        ComponentOutcome::Added => "added",
        ComponentOutcome::Failed { .. } => "failed",
        ComponentOutcome::NoRustup { .. } => "no_rustup",
        ComponentOutcome::Cancelled => "cancelled",
    };
    tracing::info!(
        target: "logos::tools",
        component = LLVM_TOOLS,
        duration_ms = started.elapsed().as_millis() as u64,
        outcome = token,
        "toolchain component",
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_listing_names_the_component_by_its_installed_name_only() {
        assert!(lists_llvm_tools("cargo-aarch64-apple-darwin\nllvm-tools-aarch64-apple-darwin\n"));
        assert!(lists_llvm_tools("llvm-tools-preview-x86_64-unknown-linux-gnu\n"), "the pre-rename name");
        assert!(lists_llvm_tools("  llvm-tools-x86_64-unknown-linux-gnu  \r\n"));
        // The near misses: another component, the bare prefix, a sentence
        // that mentions it, an empty listing.
        for listing in [
            "llvm-bitcode-linker-aarch64-apple-darwin\nrust-std-aarch64-apple-darwin",
            "llvm-tools-",
            "error: llvm-tools-preview is not installed",
            "",
        ] {
            assert!(!lists_llvm_tools(listing), "{listing:?}");
        }
    }

    #[test]
    fn the_commands_are_fixed_and_the_manual_one_is_what_the_step_runs() {
        assert_eq!(LIST_ARGS, ["component", "list", "--installed"]);
        assert_eq!(format!("rustup {}", ADD_ARGS.join(" ")), LLVM_TOOLS_ADD);
        assert!(RUSTUP_NOT_FOUND.contains(LLVM_TOOLS_ADD));
    }
}
