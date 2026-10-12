//! The CLI contribution point: a subcommand an extension adds to `logos`.
//!
//! The `cli` library composes each active extension's clap [`Command`] into the
//! `logos` command beside the core subcommands, and routes a parsed invocation
//! of it to the extension's handler with the same project root and output mode
//! a core subcommand gets.

use std::fmt;
use std::path::Path;
use std::sync::Arc;

use clap::{ArgMatches, Command};
use logos_core::error::CoreError;
use logos_core::{workspace, Engine};
use serde::Serialize;

/// A subcommand's handler: its parsed arguments and the invocation context in,
/// the process exit code out.
pub type CliHandler = dyn Fn(&ArgMatches, &CliContext<'_>) -> anyhow::Result<i32> + Send + Sync;

/// One subcommand an extension adds.
#[derive(Clone)]
pub struct CliCommand {
    command: Command,
    handler: Arc<CliHandler>,
}

impl CliCommand {
    /// The subcommand `command` (its name is the verb), answered by `handler`.
    ///
    /// The handler's exit code follows the core's contract: 0 success, 1 a
    /// result-level violation; an `Err` maps to the core's exit code for it.
    pub fn new(
        command: Command,
        handler: impl Fn(&ArgMatches, &CliContext<'_>) -> anyhow::Result<i32> + Send + Sync + 'static,
    ) -> Self {
        Self {
            command,
            handler: Arc::new(handler),
        }
    }

    /// The subcommand's name.
    pub fn name(&self) -> &str {
        self.command.get_name()
    }

    /// The subcommand's clap definition.
    pub fn command(&self) -> &Command {
        &self.command
    }

    /// Run one invocation.
    ///
    /// # Errors
    /// Whatever the handler reports.
    pub fn run(&self, matches: &ArgMatches, context: &CliContext<'_>) -> anyhow::Result<i32> {
        (self.handler)(matches, context)
    }
}

impl fmt::Debug for CliCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CliCommand").field("name", &self.name()).finish_non_exhaustive()
    }
}

/// What a subcommand runs against: the resolved project root and the global
/// output flags.
#[derive(Debug, Clone, Copy)]
pub struct CliContext<'a> {
    /// The resolved project root (`--project`, or the working tree's root).
    pub root: &'a Path,
    /// `--json`: machine output.
    pub json: bool,
    /// `--quiet`: suppress non-essential output.
    pub quiet: bool,
}

impl CliContext<'_> {
    /// Print a read-model by the rule every `logos` subcommand follows
    /// ([`print_read_model`]).
    ///
    /// # Errors
    /// The value does not serialise.
    pub fn print<T: Serialize>(&self, value: &T) -> anyhow::Result<()> {
        print_read_model(self.json, self.quiet, value)
    }

    /// A started engine over the project root, refusing a root with no index —
    /// what every core graph command runs against ([`indexed_engine`]).
    ///
    /// # Errors
    /// As [`indexed_engine`].
    pub fn engine(&self) -> anyhow::Result<Engine> {
        indexed_engine(self.root)
    }
}

/// A started engine over `root` for a command that reads an existing index: a
/// root with no `.logos/logos.db` and no seedable primary store (a DB-less linked
/// worktree is seeded, not refused — `Engine::start`, [FR-WT-03]) is the typed
/// [`CoreError::NoIndex`], which names the remedy and maps to the internal exit
/// 3 ([FR-EH-01]). The one guard behind the core graph commands and every
/// extension subcommand.
///
/// # Errors
/// No index at `root`, or the engine does not start.
///
/// [FR-WT-03]: ../../docs/specs/requirements/FR-WT-03.md
/// [FR-EH-01]: ../../docs/specs/requirements/FR-EH-01.md
pub fn indexed_engine(root: &Path) -> anyhow::Result<Engine> {
    if !root.join(".logos").join("logos.db").exists() && workspace::seed_source(root).is_none() {
        return Err(CoreError::NoIndex {
            root: root.to_path_buf(),
        }
        .into());
    }
    Engine::start(root)
}

/// `command` with each core subcommand an extension re-exposes as a hidden
/// subcommand of its own (`logos wiki hook --emit`, S-638) naming that alias in
/// its help: `(`logos <extension> <name> <required flags>` is the hidden alias of
/// this command.)`. Read from the composed `extensions`, never from a feature or a
/// crate name ([ADR-74] §3), so a build composing no alias names none (S-645).
///
/// [ADR-74]: ../../docs/specs/architecture/decisions/ADR-74.md
pub fn name_aliases(command: Command, extensions: &[CliCommand]) -> Command {
    let mut aliases = Vec::new();
    for extension in extensions.iter().map(CliCommand::command) {
        for alias in extension.get_subcommands().filter(|sub| sub.is_hide_set()) {
            if command.find_subcommand(alias.get_name()).is_none() {
                continue;
            }
            let flags: String = alias
                .get_arguments()
                .filter(|arg| arg.is_required_set())
                .filter_map(|arg| arg.get_long().map(|long| format!(" --{long}")))
                .collect();
            let spelling = format!("logos {} {}{flags}", extension.get_name(), alias.get_name());
            aliases.push((alias.get_name().to_string(), spelling));
        }
    }
    aliases.into_iter().fold(command, |command, (core, spelling)| {
        command.mut_subcommand(core, |sub| {
            let about = sub.get_about().map(ToString::to_string).unwrap_or_default();
            sub.about(format!("{}. (`{spelling}` is the hidden alias of this command.)", about.trim_end_matches('.')))
        })
    })
}

/// The one printing rule of the `logos` CLI ([FR-CL-02]): `--json` always emits
/// compact machine output — it IS the essential output — and otherwise the
/// pretty rendering is printed unless `--quiet` suppresses it. The core
/// subcommands and every extension's print through this function.
///
/// # Errors
/// The value does not serialise.
///
/// [FR-CL-02]: ../../docs/specs/requirements/FR-CL-02.md
pub fn print_read_model<T: Serialize>(json: bool, quiet: bool, value: &T) -> anyhow::Result<()> {
    if json {
        println!("{}", serde_json::to_string(value)?);
    } else if !quiet {
        println!("{}", serde_json::to_string_pretty(value)?);
    }
    Ok(())
}
