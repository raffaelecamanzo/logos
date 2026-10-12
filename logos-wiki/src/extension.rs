//! The `wiki` extension ([S-644], CR-213, [ADR-74]): the source wiki composed
//! into a binary through the extension host like any optional capability.
//!
//! The `logos` binary's `main` registers [`WikiExtension`] under its default-on
//! `wiki` feature, and — under `agents` — hands it the in-app generator
//! ([`WikiExtension::with_generator`]); no core crate (`logos-core`, `mcp`, `web`,
//! the `cli` library) depends on this crate ([ADR-74] §3).
//!
//! | Contribution point | What |
//! |---|---|
//! | MCP tools | `wiki_write`, `wiki_read`, `wiki_search`, `wiki_status`, `wiki_materialize`, byte-identical to the `#[tool]`s they were |
//! | CLI subcommand | `logos wiki` and its subcommands |
//! | init step | the `init -i` wiki-skill materialization ([`SkillInit`]) |
//! | config table | `[wiki]`, validated as [`WikiConfig`] ([FR-CF-09]) |
//! | telemetry tools | the fourteen `wiki_*` names ([`crate::telemetry`]) |
//! | web routes (feature `web`) | the wiki read routes under `/api/v1/wiki`, and — with a generator — the generation trigger `POST /wiki/generate` and its SPA shell |
//! | overview field (feature `web`) | `overview_page`, the Project Overview page the Dashboard card renders |
//! | UI manifest | the Wiki view, the `[wiki]` config panel and the Project Overview card ([FR-UI-51]) |
//!
//! [S-644]: ../../docs/planning/journal.md#s-644-the-wiki-is-its-own-crate-and-a-registered-extension
//! [ADR-74]: ../../docs/specs/architecture/decisions/ADR-74.md
//! [FR-CF-09]: ../../docs/specs/requirements/FR-CF-09.md
//! [FR-UI-51]: ../../docs/specs/requirements/FR-UI-51.md

use std::sync::Arc;

use logos_core::config::WikiConfig;
use logos_ext::cli::CliCommand;
use logos_ext::mcp::McpTool;
use logos_ext::ui::{UiConfigPanel, UiDashboardCard, UiManifest, UiNavGroup, UiNavItem, UiNavScope};
use logos_ext::{Extension, ExtensionTool, InitContribution, TomlValue};

use crate::SkillInit;

/// The extension's id — the name `GET /api/v1/extensions` reports.
pub const WIKI_ID: &str = "wiki";

/// The `config.toml` table the extension owns.
pub const WIKI_TABLE: &str = "wiki";

/// The Wiki view's navigation id and client route. The view hosts its own
/// sub-routes (`/wiki/search`, `/wiki/page/*`) under it.
const NAV_ID: &str = "wiki";
const VIEW: &str = "/wiki";

/// The id of the `[wiki]` config panel.
pub const CONFIG_PANEL_ID: &str = "wiki";

/// The id of the Dashboard's Project Overview card.
pub const OVERVIEW_CARD_ID: &str = "project-overview";

/// The key of the field the extension adds to the overview read-model: the
/// Project Overview page, or `null` while none is written.
pub const OVERVIEW_FIELD: &str = "overview_page";

/// The source wiki as an extension ([S-644]).
///
/// [`WikiExtension::new`] is the wiki without in-app generation — what a build
/// without `agents` registers: every read and write surface, and no generation
/// route. [`WikiExtension::with_generator`] adds the generation trigger, run by
/// the generator it is handed.
///
/// [S-644]: ../../docs/planning/journal.md#s-644-the-wiki-is-its-own-crate-and-a-registered-extension
#[derive(Clone, Default)]
pub struct WikiExtension {
    #[cfg(feature = "web")]
    generator: Option<Arc<dyn crate::web::WikiGenerator>>,
}

impl WikiExtension {
    /// The wiki without in-app generation.
    pub fn new() -> Self {
        Self::default()
    }

    /// The wiki with in-app generation by `generator`: the Wiki tab's trigger
    /// route is mounted, and each run is started by a service `generator` binds to
    /// the requested member ([`crate::web::WikiGenerator`]). The `logos` binary
    /// passes the configured `wiki-agent` generator under `agents`; the route
    /// tests pass a mock.
    #[cfg(feature = "web")]
    pub fn with_generator(generator: Arc<dyn crate::web::WikiGenerator>) -> Self {
        Self { generator: Some(generator) }
    }
}

impl Extension for WikiExtension {
    fn id(&self) -> &'static str {
        WIKI_ID
    }

    fn config_tables(&self) -> &'static [&'static str] {
        &[WIKI_TABLE]
    }

    /// `[wiki]` is valid exactly when it parses as [`WikiConfig`] and passes its
    /// bounds — the checks the core ran on it while it owned the table.
    fn validate_config_table(&self, _table: &str, value: &TomlValue) -> Result<(), String> {
        WikiConfig::validate_table(value)
    }

    fn mcp_tools(&self) -> Vec<McpTool> {
        crate::mcp::tools()
    }

    #[cfg(feature = "web")]
    fn web_routes(&self, context: &logos_ext::web::WebContext) -> Vec<logos_ext::web::WebRoute> {
        crate::web::routes(context, self.generator.clone())
    }

    #[cfg(feature = "web")]
    fn overview_fields(&self) -> Vec<logos_ext::web::OverviewField> {
        vec![crate::web::overview_field()]
    }

    fn cli_command(&self) -> Option<CliCommand> {
        Some(crate::cli::command())
    }

    fn init_steps(&self) -> Vec<Arc<dyn InitContribution>> {
        vec![Arc::new(SkillInit)]
    }

    fn ui_manifest(&self) -> Option<UiManifest> {
        Some(UiManifest {
            nav: vec![UiNavItem {
                id: NAV_ID,
                label: "Wiki",
                path: VIEW,
                group: UiNavGroup::A,
                scope: UiNavScope::Member,
                replaced_in_workspace_by: None,
            }],
            views: vec![VIEW],
            config_panels: vec![UiConfigPanel {
                id: CONFIG_PANEL_ID,
                label: "Wiki",
                table: WIKI_TABLE,
            }],
            dashboard_cards: vec![UiDashboardCard {
                id: OVERVIEW_CARD_ID,
                label: "Project Overview",
            }],
        })
    }

    fn telemetry_tools(&self) -> &'static [ExtensionTool] {
        crate::telemetry::TELEMETRY_TOOLS
    }
}
