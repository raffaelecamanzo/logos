//! Subcommand dispatch for the `logos` CLI adapter.
//!
//! Split out of `main.rs::run` so the entry point stays a thin setup wrapper and
//! each function stays under the architecture-quality gates
//! (`max_cc`/`max_fn_lines`, `.logos/rules.toml`). This is still pure routing —
//! every arm parses nothing new, calls **exactly one** `Engine` method, and
//! serialises its read-model (NFR-MA-02, ADR-01).
//!
//! Cyclomatic complexity here is driven by the `?` operator, not the number of
//! match arms (arms are free). The heavy `?`/exit-code boilerplate therefore
//! lives once in the [`Output::query`]/[`Output::try_query`]/
//! [`Output::report_gate`] chokepoints (the CLI twin of the MCP adapter's
//! `run`/`run_result` delegators), so the arms below are mostly zero-`?`
//! one-liners. Only the `wiki` command group — the heaviest remaining cluster —
//! is peeled into its own function to keep `dispatch` comfortably under the gate.

use std::path::Path;

use anyhow::Result;
use logos_core::{config::load_config_from_root, Engine};

use crate::{engine, init_options, read_wiki_body, Commands, CoverageCommands, Output, WikiCommands};

/// Route one parsed command to exactly one `Engine` call. Simple reads go
/// through the [`Output`] chokepoints; the four governance verdicts project an
/// exit code via [`Output::report_gate`]; `wiki` (its own subcommand cluster)
/// delegates to [`wiki`]; `serve` owns stdout and delegates to the surface crate.
pub(crate) fn dispatch(command: Commands, root: &Path, out: &Output) -> Result<i32> {
    match command {
        Commands::Init {
            interactive,
            hooks,
            workspace,
            yes,
            exclude,
        } => {
            if workspace {
                crate::workspace_init::run(root, yes, &exclude, out)
            } else {
                out.print(&Engine::init_with(root, &init_options(interactive, hooks))?)?;
                Ok(0)
            }
        }
        Commands::Index => {
            // A malformed `config.toml` is a usage fault that must fail loud with
            // exit 2 (FR-CF-03), not degrade to a silent empty index: `Engine::index`
            // is an infallible surface (ADR-14) that would otherwise swallow the
            // `ConfigError`. Validate up front so the fault propagates — the same
            // loud-failure contract `check`/`gate` honour for `rules.toml`.
            load_config_from_root(root)?;
            out.print(&engine(root, true)?.index())?;
            Ok(0)
        }
        Commands::Sync { paths } => out.query(root, |e| e.sync(&paths)),
        Commands::Status => out.query(root, |e| e.status()),
        Commands::Search { query, kind, limit } => out.query(root, |e| e.search(&query, kind, limit)),
        Commands::Query {
            symbol,
            kind,
            callers,
            callees,
            limit,
        } => {
            // The façade picks ONE Engine call per invocation (FR-CL-05).
            if callers {
                out.query(root, |e| e.callers(&symbol, limit))
            } else if callees {
                out.query(root, |e| e.callees(&symbol, limit))
            } else {
                out.query(root, |e| e.search(&symbol, kind, limit))
            }
        }
        Commands::Context {
            task,
            max_nodes,
            no_code,
        } => out.query(root, |e| e.context(&task.join(" "), max_nodes, !no_code)),
        Commands::Explore { query, max_files } => out.query(root, |e| e.explore(&query, max_files)),
        Commands::Node { symbol, code } => out.query(root, |e| e.node(&symbol, code)),
        Commands::Callers { symbol, limit } => out.query(root, |e| e.callers(&symbol, limit)),
        Commands::Callees { symbol, limit } => out.query(root, |e| e.callees(&symbol, limit)),
        Commands::Impact { symbol, depth } => out.query(root, |e| e.impact(&symbol, depth)),
        Commands::Implements { doc } => out.query(root, |e| e.implements(&doc)),
        Commands::ReferencingDocs { symbol } => out.query(root, |e| e.referencing_docs(&symbol)),
        Commands::Affected { files, tests_only } => out.query(root, |e| e.affected(&files, tests_only)),
        Commands::Scan { path, no_reconcile } => {
            if path.is_some() {
                eprintln!("logos scan: path scoping is not supported yet; scanning the project");
            }
            out.try_query(root, |e| e.scan(!no_reconcile))
        }
        // check/gate/doctor/verify project a verdict to exit 1 on failure
        // (FR-GV-03); the verdict field differs (`.passed` vs `.ok`), supplied here.
        Commands::Check { rules, no_reconcile } => out.report_gate(
            root,
            |e| e.check_rules(rules.as_deref(), !no_reconcile),
            |r| r.passed,
        ),
        Commands::Gate {
            threshold,
            save,
            label,
            no_reconcile,
        } => {
            if label.is_some() {
                eprintln!("logos gate: --label is accepted but not persisted yet");
            }
            out.report_gate(root, |e| e.gate(threshold, save, !no_reconcile), |r| r.passed)
        }
        // CR-052 / FR-GV-18/19/20: structural-integrity guards — a corrupted or
        // drifted graph is a failure a human or CI must see, not a silent read.
        Commands::Doctor => out.report_gate(root, |e| e.doctor(), |r| r.ok),
        Commands::Verify => out.report_gate(root, |e| e.verify(), |r| r.ok),
        Commands::Evolution { limit } => out.try_query(root, |e| e.evolution(limit)),
        Commands::Dsm {
            granularity,
            no_reconcile,
        } => out.try_query(root, |e| e.dsm(granularity, !no_reconcile)),
        Commands::DocGaps { limit, no_reconcile } => {
            out.try_query(root, |e| e.doc_gaps(limit, !no_reconcile))
        }
        Commands::Hotspots {
            limit,
            untested,
            production_scope,
        } => out.try_query(root, |e| e.hotspots(limit, untested, production_scope)),
        // The coverage evidence tier (CR-007): each sub-subcommand is one `Engine`
        // call shared with its MCP twin.
        Commands::Coverage { command } => match command {
            CoverageCommands::Ingest { report, format } => {
                out.try_query(root, |e| e.coverage_ingest(&report, format.as_deref()))
            }
            CoverageCommands::Status => out.try_query(root, |e| e.coverage_status()),
            CoverageCommands::Refresh => out.try_query(root, |e| e.coverage_refresh()),
        },
        Commands::Wiki { command } => wiki(command, root, out),
        // The cross-service query surface (FR-WS-05): each discovers the
        // workspace and serialises one `query::*` read-model over the member
        // registry (logic lives in logos-core, per NFR-MA-02).
        Commands::Xservice { command } => crate::xservice::run_xservice(command, root, out),
        Commands::Workspace { command } => crate::xservice::run_workspace(command, root, out),
        // Reads telemetry.db / the store without a built graph, so `Engine::open`
        // (like the MCP twins), not the index-guarded helper (NFR-OO-05).
        Commands::Stats { window } => {
            out.print(&Engine::open(root).stats(window))?;
            Ok(0)
        }
        Commands::Languages => {
            out.print(&Engine::open(root).languages())?;
            Ok(0)
        }
        // stdout belongs to JSON-RPC until the host disconnects (S-017); the
        // combined-surface orchestration lives in the `web` adapter so this
        // surface stays thin (ADR-27, NFR-MA-02).
        #[cfg(feature = "ui")]
        Commands::Serve { mcp, ui, port } => {
            web::serve_surfaces(root, mcp, ui, port)?;
            Ok(0)
        }
        #[cfg(not(feature = "ui"))]
        Commands::Serve { mcp: _ } => {
            mcp::serve_stdio(root)?;
            Ok(0)
        }
    }
}

/// The source wiki (CR-008): write/read/search/status/materialize/delete are
/// each one `Engine` call → byte-identical payloads with the MCP twin
/// (FR-WK-09). `generate` bypasses `print` for the human path because
/// `render_prompt_block` returns prose, not a `Serialize` payload; `skill`/
/// `hook` are pure filesystem materialization (no index) so they use the
/// unguarded `Engine::open`.
fn wiki(command: WikiCommands, root: &Path, out: &Output) -> Result<i32> {
    match command {
        WikiCommands::Write {
            slug,
            title,
            generator,
            anchors,
            body_file,
            body,
        } => {
            let body = read_wiki_body(body, body_file)?;
            out.try_query(root, |e| e.wiki_write(&slug, &title, &body, &anchors, &generator))
        }
        WikiCommands::Read { slug } => out.try_query(root, |e| e.wiki_read(&slug)),
        WikiCommands::Search { query, list } => {
            out.try_query(root, |e| e.wiki_search(query.as_deref().unwrap_or(""), list))
        }
        WikiCommands::Status => out.try_query(root, |e| e.wiki_status()),
        WikiCommands::Generate => {
            let queue = engine(root, false)?.wiki_generate()?;
            if out.json {
                out.print(&queue)?;
            } else if !out.quiet {
                print!("{}", queue.render_prompt_block());
            }
            Ok(0)
        }
        WikiCommands::Materialize => out.try_query(root, |e| e.wiki_materialize()),
        WikiCommands::Delete { slug } => out.try_query(root, |e| e.wiki_delete(&slug)),
        WikiCommands::Skill { emit: _, dir, force } => {
            out.print(&Engine::open(root).wiki_skill_emit(dir.as_deref(), force)?)?;
            Ok(0)
        }
        // Emits the SessionEnd quality-report hook into the shared
        // .claude/settings.json (CR-070: the PostToolUse augment hook this
        // once also emitted is retired).
        WikiCommands::Hook { emit: _, force } => {
            let engine = Engine::open(root);
            out.print(&engine.wiki_quality_report_hook_emit(force)?)?;
            Ok(0)
        }
    }
}
