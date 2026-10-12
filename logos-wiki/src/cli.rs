//! The `logos wiki` subcommand ([FR-WK-02]..[FR-WK-08], [FR-DS-04]): the wiki
//! extension's CLI contribution.
//!
//! It was a variant of the core's `Commands` until the wiki became an extension
//! (S-644); its clap definition moved here verbatim, so `logos wiki --help` and
//! every subcommand read and behave as before. `write`/`read`/`search`/`status`/
//! `materialize`/`delete` are each one [`WikiEngine`] call over the index-guarded
//! engine every core graph command uses ([`CliContext::engine`]), printed by the
//! one CLI printing rule — byte-identical payloads with the MCP twins
//! ([FR-WK-09]). `generate` prints its prompt block as prose on the human path;
//! `skill` (and the hidden `hook` alias of the core's `logos hook --emit`) are
//! filesystem materialization, so they open the engine without the index guard.
//!
//! [FR-WK-02]: ../../docs/specs/requirements/FR-WK-02.md
//! [FR-WK-08]: ../../docs/specs/requirements/FR-WK-08.md
//! [FR-WK-09]: ../../docs/specs/requirements/FR-WK-09.md
//! [FR-DS-04]: ../../docs/specs/requirements/FR-DS-04.md

use std::path::PathBuf;

use anyhow::{bail, Result};
use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use logos_core::Engine;
use logos_ext::cli::{CliCommand, CliContext};

use crate::WikiEngine;

/// Source wiki: write/read/search/status/delete agent-generated pages in
/// the gate-immune `.logos/wiki.db` store (CR-008). Every read carries
/// mandatory provenance (generator, written-at HEAD, per-anchor freshness,
/// the fixed generated-content marker).
#[derive(Parser)]
#[command(name = "wiki")]
struct WikiCli {
    #[command(subcommand)]
    command: WikiCommands,
}

/// `wiki` sub-subcommands (FR-WK-02/04/05/06/07): the read/write/search surface
/// over the wiki store. `write`/`read`/`search`/`status`/`materialize` have
/// payload-identical MCP twins (FR-WK-09); `delete` is CLI-only (destructive,
/// off the agent surface).
#[derive(Subcommand)]
enum WikiCommands {
    /// Upsert a page by slug: byte-verbatim body (1 MiB cap), write-time anchor
    /// resolution, mandatory generator label (FR-WK-02).
    Write {
        /// The page slug (path-like: lowercase/digit/`-`/`_` segments).
        slug: String,
        /// The page title.
        #[arg(long, short = 't')]
        title: String,
        /// The mandatory generator label (e.g. the model/tool that wrote it).
        #[arg(long, short = 'g')]
        generator: String,
        /// Anchor entity id, repeatable: `file:<path>` or `symbol:<symbol>`.
        #[arg(long = "anchor", value_name = "ID")]
        anchors: Vec<String>,
        /// Read the markdown body from a file (`-` reads stdin); otherwise pass
        /// the body as the positional argument.
        #[arg(long, value_name = "PATH", conflicts_with = "body")]
        body_file: Option<PathBuf>,
        /// The markdown body (when `--body-file` is not used).
        body: Option<String>,
    },
    /// Read a page by slug with mandatory provenance + per-anchor freshness
    /// (FR-WK-04). A miss (or an all-anchors-gone auto-prune) exits non-zero.
    Read {
        /// The slug to read.
        slug: String,
    },
    /// FTS5 bm25 search over page titles + bodies, staleness-flagged (FR-WK-05).
    Search {
        /// The search query (omit with `--list`).
        query: Option<String>,
        /// Enumerate all pages instead of searching.
        #[arg(long)]
        list: bool,
    },
    /// Store summary + regeneration work-list: stale, missing-anchor, pruned,
    /// and page-worthy entities without a page (FR-WK-06).
    Status,
    /// Format the `wiki status` work-list into an ordered, offline generation
    /// queue (FR-WK-13): a human-readable prompt block by default, or `--json`
    /// for machines. Each item carries its target slug and a runnable `wiki
    /// write` skeleton. A pure read — no `wiki.db` write, no LLM, no network.
    Generate,
    /// Deterministically assemble the presented tier (FR-WK-20, ADR-57): in SRS
    /// mode, present each Design/Specs category (and the single-file
    /// Architecture page) from the project's authored `docs/specs/**` sources
    /// into `wiki.db` with `generator = "logos:doc-present"`, then run the
    /// reconciliation sweep. A pure deterministic write — no LLM, no network
    /// (NFR-SE-01); byte-identical on re-run. Outside SRS mode (Case 2) this is
    /// a no-op. Run automatically by the UI-gated generation flow ahead of the
    /// LLM queue (FR-WK-18); safe to run manually.
    Materialize,
    /// Explicitly delete a page by slug (FR-WK-07); an unknown slug exits non-zero.
    Delete {
        /// The slug to delete.
        slug: String,
    },
    /// Materialize the embedded wiki-generation skill (FR-WK-08): the canonical
    /// `.agents/skills/logos-wiki/` directory plus the `.claude/skills/logos-wiki`
    /// symlink. Refreshes an existing install or a post-upgrade skill. CLI-only.
    Skill {
        /// Emit the embedded skill (the only `skill` operation; required so the
        /// verb reads `wiki skill --emit`).
        #[arg(long, required = true)]
        emit: bool,
        /// Target base directory (defaults to the project root).
        dir: Option<PathBuf>,
        /// Overwrite an existing install, restoring the embedded content.
        #[arg(long)]
        force: bool,
    },
    /// Install the Claude Code session-start quality-report hook (FR-IN-07,
    /// ADR-49): a marker-tagged hook script plus a non-clobbering merge into
    /// the shared `.claude/settings.json` that surfaces a non-blocking
    /// signal/baseline/violations readout when a session starts, resumes, or is
    /// reopened by `/clear`. Also sweeps the retired SessionEnd hook and its
    /// orphaned script (CR-095). The binary stays offline (NFR-SE-01). Re-emit
    /// with `--force`. CLI-only. (The PostToolUse wiki-augmentation hook this
    /// once also installed was retired — CR-070.) A hidden alias of the core
    /// `logos hook --emit` (S-638): same output, same exit codes.
    #[command(hide = true)]
    Hook {
        /// Emit the hook (the only `hook` operation; required so the verb reads
        /// `hook --emit`).
        #[arg(long, required = true)]
        emit: bool,
        /// Re-emit, replacing an existing managed SessionStart entry.
        #[arg(long)]
        force: bool,
    },
}

/// The `wiki` subcommand: its clap definition, answered by [`run`].
pub(crate) fn command() -> CliCommand {
    CliCommand::new(WikiCli::command(), |matches, context| {
        run(WikiCli::from_arg_matches(matches)?.command, context)
    })
}

/// Run one `wiki` invocation over `context`'s root and output flags.
fn run(command: WikiCommands, context: &CliContext<'_>) -> Result<i32> {
    match command {
        WikiCommands::Write {
            slug,
            title,
            generator,
            anchors,
            body_file,
            body,
        } => {
            let body = read_body(body, body_file)?;
            context.print(&context.engine()?.wiki_write(&slug, &title, &body, &anchors, &generator)?)?;
        }
        WikiCommands::Read { slug } => context.print(&context.engine()?.wiki_read(&slug)?)?,
        WikiCommands::Search { query, list } => {
            context.print(&context.engine()?.wiki_search(query.as_deref().unwrap_or(""), list)?)?;
        }
        WikiCommands::Status => context.print(&context.engine()?.wiki_status()?)?,
        WikiCommands::Generate => {
            let queue = context.engine()?.wiki_generate()?;
            if context.json {
                context.print(&queue)?;
            } else if !context.quiet {
                print!("{}", queue.render_prompt_block());
            }
        }
        WikiCommands::Materialize => context.print(&context.engine()?.wiki_materialize()?)?,
        WikiCommands::Delete { slug } => context.print(&context.engine()?.wiki_delete(&slug)?)?,
        WikiCommands::Skill { emit: _, dir, force } => {
            context.print(&Engine::open(context.root).wiki_skill_emit(dir.as_deref(), force)?)?;
        }
        // The hidden alias of the core's `logos hook --emit` (S-638), kept so a hook
        // script or a habit that spells it `wiki hook --emit` keeps working: the same
        // core call, so the same output, exit codes and telemetry.
        WikiCommands::Hook { emit: _, force } => {
            context.print(&Engine::open(context.root).quality_report_hook_emit(force)?)?;
        }
    }
    Ok(0)
}

/// Resolve the `wiki write` body from either `--body-file` (a path, or `-` for
/// stdin) or the positional argument — exactly one source. Reading a file/stdin
/// is surface I/O, not business logic (the core takes the resolved `&str`); a
/// large markdown body would otherwise blow past the shell's argv limit.
///
/// This is the external-agent write surface the content-validity guard
/// (FR-WK-19) protects: the resolved body is handed unchanged to
/// [`WikiEngine::wiki_write`], whose façade rejects
/// agent-noise before it reaches the store — so the guard applies identically
/// here and to the in-process run, with no separate check needed on this path.
fn read_body(body: Option<String>, body_file: Option<PathBuf>) -> Result<String> {
    match (body, body_file) {
        (_, Some(path)) if path.as_os_str() == "-" => {
            use std::io::Read as _;
            let mut buf = String::new();
            std::io::stdin().read_to_string(&mut buf)?;
            Ok(buf)
        }
        (_, Some(path)) => Ok(std::fs::read_to_string(&path)?),
        (Some(body), None) => Ok(body),
        (None, None) => {
            bail!("wiki write needs a body: pass it as an argument or via --body-file <PATH|->")
        }
    }
}
