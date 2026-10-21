//! `logos-ext` — the extension host ([extension-host], [ADR-74], [ADR-75]).
//!
//! Optional capabilities — the agentic chat and the source wiki, once CR-213
//! has moved them — are **extensions**: types implementing [`Extension`],
//! registered with an [`ExtensionHost`] at one composition root, the binary's
//! `main`, which hands the host to `logos_cli::run`. Every surface takes the
//! host and asks it for the **active** extensions' contributions (the one
//! exception, the owned skill names, is in the last row):
//!
//! | Contribution | Method | Consumed by |
//! |---|---|---|
//! | config tables | [`Extension::config_tables`], validated by [`Extension::validate_config_table`] | config loading ([FR-CF-09]) |
//! | secret keys | [`Extension::secret_keys`] | config loading ([FR-CF-09]) |
//! | MCP tools | [`Extension::mcp_tools`] | `mcp`: dynamic `ToolRoute`s composed onto the server's router |
//! | web routes and their `POST` allow-list entries | `Extension::web_routes` (feature `web`) | `web`: mounted beside the core table, admitted by its guards |
//! | overview fields | `Extension::overview_fields` (feature `web`) | `web`: added to the Dashboard's overview read-model |
//! | a CLI subcommand | [`Extension::cli_command`] | the `cli` library's composed clap command |
//! | init steps | [`Extension::init_steps`] | `logos init` |
//! | a UI manifest — navigation, views, config panels, Dashboard cards | [`Extension::ui_manifest`] | the SPA, through the server ([FR-UI-51]) |
//! | telemetry tools | [`Extension::telemetry_tools`] | the stats read-model's classification |
//! | owned skill names | [`Extension::owned_skills`], from **every registered** extension | `logos init`: a skill copy under one of these names is never retired |
//!
//! Every method but [`Extension::id`] has a no-op default, so an extension
//! declares only what it contributes, and a later story adds a contribution
//! point's *consumer* without widening this trait.
//!
//! # Feature gate ([ADR-75])
//!
//! An extension may name a feature it needs ([`Extension::required_feature`]).
//! The composing binary may supply a [`FeatureGate`] that reports the enabled
//! features; a build that supplies none enables no feature, so it activates only
//! extensions that name none ([BR-63]). An inactive extension contributes
//! nothing, on any surface — except the one thing that must outlive its
//! licence: the names of the skills it owns ([`Extension::owned_skills`]), so
//! `init` never deletes a skill copy, and the user's edits to it, because the
//! extension is inactive today.
//!
//! # Direction
//!
//! No core crate (`logos-core`, `mcp`, `web`, the `cli` library) depends on an
//! extension crate; they depend on this one, and only a binary's `main` names
//! extensions. This crate depends on `logos-core` — extensions work over
//! `&Engine` — so the core reaches the host only through traits it owns and the
//! host implements ([`InitContribution`]).
//!
//! [extension-host]: ../../docs/specs/architecture/components/extension-host.md
//! [ADR-74]: ../../docs/specs/architecture/decisions/ADR-74.md
//! [ADR-75]: ../../docs/specs/architecture/decisions/ADR-75.md
//! [FR-CF-09]: ../../docs/specs/requirements/FR-CF-09.md
//! [FR-UI-51]: ../../docs/specs/requirements/FR-UI-51.md
//! [BR-63]: ../../docs/specs/software-spec.md#320-distribution

use std::sync::Arc;

pub mod cli;
mod gate;
mod host;
pub mod mcp;
#[cfg(feature = "test-support")]
pub mod testing;
pub mod ui;
#[cfg(feature = "web")]
pub mod web;

pub use gate::FeatureGate;
pub use host::{ExtensionHost, HostError};
pub use logos_core::config::TomlValue;
pub use logos_core::init::InitContribution;
pub use logos_core::observability::{ExtensionTool, ToolClass};

/// An optional capability, composed into the binary through an
/// [`ExtensionHost`] ([ADR-74]).
///
/// Implement [`id`](Self::id) and whichever contribution methods the extension
/// needs; the rest default to contributing nothing. A contribution reaches a
/// surface only while the extension is **active** — registered, and with its
/// feature enabled when it names one ([`ExtensionHost::active`]).
///
/// Implementing [`web_routes`](Self::web_routes) needs this crate's `web`
/// feature, which the web adapter turns on in every build that has one.
///
/// [ADR-74]: ../../docs/specs/architecture/decisions/ADR-74.md
pub trait Extension: Send + Sync + 'static {
    /// The stable id: unique within a host, and the name the server's
    /// capabilities endpoint reports ([FR-UI-51]).
    ///
    /// [FR-UI-51]: ../../docs/specs/requirements/FR-UI-51.md
    fn id(&self) -> &'static str;

    /// The feature this extension needs enabled, or `None` for an extension
    /// that is always active ([ADR-75]).
    ///
    /// [ADR-75]: ../../docs/specs/architecture/decisions/ADR-75.md
    fn required_feature(&self) -> Option<&'static str> {
        None
    }

    /// The `config.toml` tables this extension owns ([FR-CF-09]).
    ///
    /// [FR-CF-09]: ../../docs/specs/requirements/FR-CF-09.md
    fn config_tables(&self) -> &'static [&'static str] {
        &[]
    }

    /// Validate `value`, this extension's `config.toml` table `table` (one of
    /// [`config_tables`](Self::config_tables)), exactly where the core
    /// validates its own: on every load and before every editor write
    /// ([FR-CF-09]). `Err` says what is wrong; the load or write fails with it,
    /// attributed to the table. The default accepts any value.
    ///
    /// [FR-CF-09]: ../../docs/specs/requirements/FR-CF-09.md
    fn validate_config_table(&self, table: &str, value: &TomlValue) -> Result<(), String> {
        let _ = (table, value);
        Ok(())
    }

    /// The `secrets.toml` keys this extension owns, as `table.key` ([FR-CF-09]).
    ///
    /// [FR-CF-09]: ../../docs/specs/requirements/FR-CF-09.md
    fn secret_keys(&self) -> &'static [&'static str] {
        &[]
    }

    /// The MCP tools this extension serves, beside the core roster.
    fn mcp_tools(&self) -> Vec<mcp::McpTool> {
        Vec::new()
    }

    /// The web routes this extension mounts, over the serve's default engine and
    /// backing. Its `POST` routes are its entries in the web surface's `POST`
    /// allow-list, and every route sits behind the surface's guard stack.
    #[cfg(feature = "web")]
    fn web_routes(&self, context: &web::WebContext) -> Vec<web::WebRoute> {
        let _ = context;
        Vec::new()
    }

    /// The fields this extension adds to the Dashboard's overview read-model
    /// (`GET /api/v1/overview`), each read over the requested member's engine —
    /// the data its Dashboard cards render ([`ui::UiManifest::dashboard_cards`]).
    #[cfg(feature = "web")]
    fn overview_fields(&self) -> Vec<web::OverviewField> {
        Vec::new()
    }

    /// The CLI subcommand this extension adds to `logos`.
    fn cli_command(&self) -> Option<cli::CliCommand> {
        None
    }

    /// The steps this extension adds to `logos init`, run after the core's.
    fn init_steps(&self) -> Vec<Arc<dyn InitContribution>> {
        Vec::new()
    }

    /// The names of the agent skills this extension owns and materializes. Unlike
    /// every other contribution, these are read from every **registered**
    /// extension, active or not ([`ExtensionHost::owned_skills`]): a skill copy
    /// under one of these names is never retired by `init`, so an extension that
    /// is inactive today (unlicensed, expired) keeps the copy, and the user's
    /// edits to it, for the day it is active again. The default owns none.
    fn owned_skills(&self) -> Vec<String> {
        Vec::new()
    }

    /// What this extension adds to the SPA: navigation and config panels
    /// ([FR-UI-51]).
    ///
    /// [FR-UI-51]: ../../docs/specs/requirements/FR-UI-51.md
    fn ui_manifest(&self) -> Option<ui::UiManifest> {
        None
    }

    /// The telemetry tools this extension's calls report under
    /// ([`logos_core::observability::traced_extension`]).
    fn telemetry_tools(&self) -> &'static [ExtensionTool] {
        &[]
    }
}
