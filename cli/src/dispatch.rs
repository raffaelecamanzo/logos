//! Subcommand dispatch for the `logos` CLI adapter.
//!
//! Split out of `main.rs::run` so the entry point stays a thin setup wrapper and
//! each function stays under the architecture-quality gates
//! (`max_cc`/`max_fn_lines`, `.logos/rules.toml`). This is still pure routing —
//! every *read* arm parses nothing new, calls **exactly one** `Engine` method,
//! and serialises its read-model (NFR-MA-02, ADR-01); the delegating arms
//! (`Serve`, `Init --workspace`, `Xservice`/`Workspace`, `InternalWarm`) route
//! to a surface module instead, with the logic still in the core.
//!
//! Cyclomatic complexity here is driven by the `?` operator, not the number of
//! match arms (arms are free). The heavy `?`/exit-code boilerplate therefore
//! lives once in the [`Output::query`]/[`Output::try_query`]/
//! [`Output::report_gate`]/[`Output::report_check`] chokepoints (the CLI twin
//! of the MCP adapter's `run`/`run_result` delegators), so the arms below are
//! mostly zero-`?` one-liners. The twins of the read-only MCP tools — the commands
//! that take `--repo` — use the selecting variants ([`Output::query_for`],
//! [`Output::try_query_for`], [`Output::report_gate_for`]), which choose the member
//! the answer is for (`Backing::one_shot`) before the one `Engine` call. An extension's subcommands are not routed here:
//! the library's composed command dispatches them (S-644).

use std::path::Path;

use anyhow::Result;
use logos_core::{config::load_config_from_root, Engine};
use logos_ext::ExtensionHost;

use crate::{engine, init_options, Commands, CoverageCommands, Output};

/// Route one parsed command to exactly one `Engine` call. Simple reads go
/// through the [`Output`] chokepoints; `gate`/`doctor`/`verify` project a
/// verdict via [`Output::report_gate`], `check` via its own tri-state
/// [`Output::report_check`] (FR-GV-22); `serve` owns stdout and delegates to the
/// surface crate.
/// `host` reaches the two arms extensions contribute to: `init` runs their init
/// steps, `serve` their routes and tools (FR-DS-04, ADR-74).
pub(crate) fn dispatch(command: Commands, root: &Path, out: &Output, host: &ExtensionHost) -> Result<i32> {
    match command {
        Commands::Init {
            interactive,
            hooks,
            workspace,
            yes,
            exclude,
        } => {
            // The whole routing decision is `nudge`'s, so it is unit-testable
            // without a terminal: `--workspace` short-circuits inside it, and a
            // plain `init` at a parent-of-repos root gets the stderr explanation
            // and, on a TTY only, the offer (FR-IN-08). Declining — the default,
            // and the whole of the non-TTY path — falls through to the unchanged
            // single-root `init` below.
            //
            // `nudge` reports *which* entry point took the workspace path, not
            // merely that one did; `then_some` turns that into the CR-103
            // fallback right, so an explicit `--workspace` passes `None` and
            // cannot acquire it by coincidence. Why the payload is the raw
            // `(interactive, hooks)` pair and not a built `InitOptions`:
            // `init_options` *prompts* under `-i`, so building it here would
            // interrogate the user on the accepted-offer path that goes on to
            // enable members and never wants it. The rationale for the split
            // lives once, on `workspace_init::run`'s `fallback` parameter.
            if let Some(nudged) = crate::workspace_init::nudge(root, workspace, interactive || hooks, crate::ask) {
                crate::workspace_init::run(root, nudged.then_some((interactive, hooks)), yes, &exclude, out, crate::workspace_init::spawn_supervisor, host)
            } else {
                let steps = host.init_steps();
                out.print(&Engine::init_with_contributions(root, &init_options(interactive, hooks, &steps, crate::ask), &steps)?)?;
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
            // A run that reached persistence and persisted nothing, or whose
            // pipeline failed, exits 1 (FR-CL-03, FR-EH-05); a degraded run with
            // some files failed, or one admitting no file (FR-IX-13), exits 0.
            let result = engine(root, true)?.index();
            out.print(&result)?;
            Ok(crate::violation_code(!result.failed))
        }
        // The hidden bounded-warm supervisor (FR-WS-14). One of the delegating
        // arms (alongside `Serve`, `Init --workspace` and `Xservice`/`Workspace`,
        // which route to a surface module rather than the façade) — and the only
        // one that opens no store whatever: it just supervises `index` children.
        Commands::InternalWarm {
            concurrency,
            members,
        } => Ok(crate::workspace_init::run_supervisor(&members, concurrency)),
        Commands::Sync { paths } => {
            // As `index` above: a malformed `config.toml` is a usage fault (exit 2,
            // FR-CF-03), validated up front — `Engine::sync` would fold it into a
            // `failed` run (exit 1, FR-EH-05) instead.
            load_config_from_root(root)?;
            out.report_gate(root, |e| Ok(e.sync(&paths)), |r| !r.failed)
        }
        Commands::Status { member } => out.query_for(root, &member, |e| e.status()),
        Commands::Search { query, kind, limit, member } => out.query_for(root, &member, |e| e.search(&query, kind, limit)),
        Commands::Query {
            symbol,
            kind,
            callers,
            callees,
            limit,
            member,
        } => {
            // The façade picks ONE Engine call per invocation (FR-CL-05).
            if callers {
                out.query_for(root, &member, |e| e.callers(&symbol, limit))
            } else if callees {
                out.query_for(root, &member, |e| e.callees(&symbol, limit))
            } else {
                out.query_for(root, &member, |e| e.search(&symbol, kind, limit))
            }
        }
        Commands::Context {
            task,
            max_nodes,
            no_code,
            member,
        } => out.query_for(root, &member, |e| e.context(&task.join(" "), max_nodes, !no_code)),
        Commands::Explore { query, max_files, member } => out.query_for(root, &member, |e| e.explore(&query, max_files)),
        Commands::Node { symbol, code, member } => out.query_for(root, &member, |e| e.node(&symbol, code)),
        Commands::Callers { symbol, limit, member } => out.query_for(root, &member, |e| e.callers(&symbol, limit)),
        Commands::Callees { symbol, limit, member } => out.query_for(root, &member, |e| e.callees(&symbol, limit)),
        Commands::Impact { symbol, depth, member } => out.query_for(root, &member, |e| e.impact(&symbol, depth)),
        Commands::ImpactIntersection { items, depth, member } => {
            out.query_for(root, &member, |e| e.impact_intersection(&items, depth))
        }
        Commands::Precedent { target, limit, member } => out.query_for(root, &member, |e| e.precedent(&target, limit)),
        Commands::BranchOverlap { refs, base, merge, member } => out.query_for(root, &member, |e| {
            e.branch_overlap(&refs, base.as_deref(), merge.as_deref())
        }),
        Commands::Implements { doc } => out.query(root, |e| e.implements(&doc)),
        Commands::ReferencingDocs { symbol } => out.query(root, |e| e.referencing_docs(&symbol)),
        Commands::Affected { files, tests_only } => out.query(root, |e| e.affected(&files, tests_only)),
        Commands::Scan { path, no_reconcile } => {
            if path.is_some() {
                eprintln!("logos scan: path scoping is not supported yet; scanning the project");
            }
            out.try_query(root, |e| e.scan(!no_reconcile))
        }
        // The report tier (FR-IN-07, CR-095): always exit 0 — unlike
        // check/gate below, this reports and never gates. Two renderings of one
        // read-model; `--hook-json` is the agent-host payload the installed hook
        // script execs.
        Commands::QualityReport { hook_json } => {
            if hook_json {
                out.try_query(root, |e| e.quality_report_hook_payload())
            } else {
                out.try_query(root, |e| e.quality_readout())
            }
        }
        // check/gate/doctor/verify project a verdict to exit 1 on failure
        // (FR-GV-03); the verdict field differs (`.passed` vs `.ok`), supplied
        // here. `check` alone gets its own chokepoint (`report_check`): its
        // verdict is a tri-state `Option<bool>`, not a plain bool, because a
        // report over no rules contract carries no verdict at all (FR-GV-22).
        Commands::Check { rules, no_reconcile, allow_no_rules } => out.report_check(
            root,
            |e| e.check_rules(rules.as_deref(), !no_reconcile),
            allow_no_rules,
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
        // ARCHITECTURE health, and it PROJECTS ITS VERDICT (FR-GV-20 AC 1:
        // "`session_end`, `check_rules`, and `health` all fail (exit 1)" on
        // admission drift; UAT-GV-11 steps 4-5 turn on the same pass/fail
        // distinction). `HealthInfo::ok` is `fts_ok && structural_ok`, and
        // `structural_ok` is where the admission tripwire folds in — so `ok` is
        // the field to gate on, and an FTS desync (a Correctness fault under
        // ADR-14) rides the same exit code rather than printing `"ok": false`
        // at exit 0. The read-model is still returned in full: diagnosing the
        // desync is what `health` is FOR, so the fault is reported, not raised.
        Commands::Health { no_reconcile, member } => {
            out.report_gate_for(root, &member, |e| e.health(!no_reconcile), |r| r.ok)
        }
        // CR-052 / FR-GV-18/19/20: structural-integrity guards — a corrupted or
        // drifted graph is a failure a human or CI must see, not a silent read.
        Commands::Doctor => out.report_gate(root, |e| e.doctor(), |r| r.ok),
        Commands::Verify => out.report_gate(root, |e| e.verify(), |r| r.ok),
        // The session gate (FR-GV-04/05) without an MCP host (FR-CL-06):
        // `session-start` records the baseline and always exits 0;
        // `session-end` projects the regression verdict onto exit 1 the way
        // `gate` does, so a CLI-directed agent can detect a failing gate.
        Commands::SessionStart => out.try_query(root, |e| e.session_start()),
        Commands::SessionEnd => out.report_gate(root, |e| e.session_end(), |r| r.passed),
        Commands::Evolution { limit, member } => out.try_query_for(root, &member, |e| e.evolution(limit)),
        Commands::Dsm {
            granularity,
            no_reconcile,
            member,
        } => out.try_query_for(root, &member, |e| e.dsm(granularity, !no_reconcile)),
        Commands::DocGaps { limit, no_reconcile } => {
            out.try_query(root, |e| e.doc_gaps(limit, !no_reconcile))
        }
        Commands::Hotspots {
            limit,
            untested,
            production_scope,
            member,
        } => out.try_query_for(root, &member, |e| e.hotspots(limit, untested, production_scope)),
        // The coverage evidence tier (CR-007): each sub-subcommand is one `Engine`
        // call shared with its MCP twin.
        Commands::Coverage { command } => match command {
            CoverageCommands::Ingest { report, format } => {
                out.try_query(root, |e| e.coverage_ingest(&report, format.as_deref()))
            }
            CoverageCommands::Status { member } => out.try_query_for(root, &member, |e| e.coverage_status()),
            CoverageCommands::Refresh => out.try_query(root, |e| e.coverage_refresh()),
        },
        // The session-start quality-report hook (S-638): a core command, not a
        // wiki one.
        Commands::Hook(args) => hook_emit(root, args, out),
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
        Commands::Serve { mcp, ui, port, standalone } => {
            web::serve_surfaces(root, mcp, ui, port, standalone, host)?;
            Ok(0)
        }
        #[cfg(not(feature = "ui"))]
        Commands::Serve { mcp: _ } => {
            mcp::serve_stdio(root, host)?;
            Ok(0)
        }
    }
}

/// Emit the session-start quality-report hook into the shared
/// .claude/settings.json and sweep the retired SessionEnd entry (CR-095; CR-070
/// retired the PostToolUse augment hook this once also emitted) — `logos hook
/// --emit`. A composed extension that re-exposes it as a hidden alias makes the
/// same core call.
fn hook_emit(root: &Path, args: crate::HookArgs, out: &Output) -> Result<i32> {
    let engine = Engine::open(root);
    out.print(&engine.quality_report_hook_emit(args.force)?)?;
    Ok(0)
}
