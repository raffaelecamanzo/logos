//! A stub extension and a test feature gate (feature `test-support`) — the one
//! extension every surface's tests compose, so each contribution point is proven
//! against the same declaration rather than a per-crate copy.
//!
//! [`StubExtension`] contributes to **every** contribution point: a `GET` and a
//! `POST` route, an overview field, an MCP tool, a CLI subcommand, an init step,
//! a config table, a secret key, a UI manifest (with a Dashboard card) and a
//! telemetry tool. Built with
//! [`StubExtension::gated`], it is active only under a gate that enables
//! [`STUB_FEATURE`] — [`TestGate`] enables exactly the features it is given.
//!
//! Enabled only as a dev-dependency: it never reaches a shipped binary.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use anyhow::Context;
use clap::{Arg, Command};
use logos_core::init::InitOptions;
use logos_core::models::pipeline::{InitAction, InitStep};
use logos_core::observability::traced_extension;
use logos_core::Engine;
use rmcp::model::Tool;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::cli::CliCommand;
use crate::mcp::{parse_arguments, McpTool};
use crate::ui::{UiConfigPanel, UiDashboardCard, UiManifest, UiNavGroup, UiNavItem, UiNavScope};
use crate::{Extension, ExtensionTool, FeatureGate, InitContribution, TomlValue, ToolClass};

/// The stub's id.
pub const STUB_ID: &str = "stub";
/// The feature [`StubExtension::gated`] names.
pub const STUB_FEATURE: &str = "stub-feature";
/// The stub's MCP tool.
pub const STUB_MCP_TOOL: &str = "stub_probe";
/// The telemetry tool the stub's operation reports under, from both its MCP tool
/// and its CLI subcommand.
pub const STUB_TELEMETRY: ExtensionTool = ExtensionTool::new("stub_probe", ToolClass::Navigation);
/// The stub's `GET` route.
pub const STUB_GET_ROUTE: &str = "/api/v1/stub";
/// The stub's `POST` route.
pub const STUB_POST_ROUTE: &str = "/api/v1/stub/action";
/// The key of the field the stub adds to the overview read-model.
pub const STUB_OVERVIEW_FIELD: &str = "stub_overview";
/// The stub's Dashboard card.
pub const STUB_DASHBOARD_CARD: &str = "stub-card";
/// The stub's CLI subcommand.
pub const STUB_SUBCOMMAND: &str = "stub-probe";
/// The file the stub's init step writes, relative to the project root.
pub const STUB_INIT_TARGET: &str = ".logos/stub-extension.txt";
/// The config table the stub owns.
pub const STUB_CONFIG_TABLE: &str = "stub";
/// The secret key the stub owns.
pub const STUB_SECRET_KEY: &str = "stub.token";

/// The schema of the stub's config table ([`STUB_CONFIG_TABLE`]).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StubTable {
    #[serde(default)]
    #[allow(dead_code)] // the schema is the point: nothing reads the value
    greeting: Option<String>,
}

/// An extension contributing to every contribution point.
#[derive(Clone, Default)]
pub struct StubExtension {
    requires: Option<&'static str>,
    posts: Arc<AtomicUsize>,
}

impl StubExtension {
    /// A stub that names no feature, so it is active whenever registered.
    pub fn new() -> Self {
        Self::default()
    }

    /// A stub that is active only when the gate enables `feature`.
    pub fn gated(feature: &'static str) -> Self {
        Self {
            requires: Some(feature),
            ..Self::default()
        }
    }

    /// How many `POST`s reached the stub's `POST` handler — shared by every
    /// clone, so a test keeps one to observe what the registered copy saw.
    pub fn posts(&self) -> usize {
        self.posts.load(Ordering::SeqCst)
    }
}

/// The stub's one operation, its chokepoint: what its MCP tool and its CLI
/// subcommand both answer, reported under [`STUB_TELEMETRY`].
///
/// # Errors
/// Never; the signature is the chokepoint's.
pub fn probe(root: &Path, echo: Option<&str>) -> anyhow::Result<Value> {
    traced_extension(STUB_TELEMETRY, || {
        Ok(json!({ "extension": STUB_ID, "root": root.display().to_string(), "echo": echo }))
    })
}

#[derive(Deserialize)]
struct ProbeArgs {
    echo: Option<String>,
}

fn probe_tool() -> Tool {
    let schema = json!({
        "type": "object",
        "properties": { "echo": { "type": "string", "description": "Echoed back." } }
    });
    let Value::Object(schema) = schema else { unreachable!("a JSON object") };
    Tool::new(STUB_MCP_TOOL, "Probe the stub extension (test-only).", Arc::new(schema))
}

impl Extension for StubExtension {
    fn id(&self) -> &'static str {
        STUB_ID
    }

    fn required_feature(&self) -> Option<&'static str> {
        self.requires
    }

    fn config_tables(&self) -> &'static [&'static str] {
        &[STUB_CONFIG_TABLE]
    }

    /// The stub's table admits an optional string `greeting` and nothing else.
    fn validate_config_table(&self, _table: &str, value: &TomlValue) -> Result<(), String> {
        value.clone().try_into::<StubTable>().map(drop).map_err(|e| e.message().to_string())
    }

    fn secret_keys(&self) -> &'static [&'static str] {
        &[STUB_SECRET_KEY]
    }

    fn mcp_tools(&self) -> Vec<McpTool> {
        vec![McpTool::new(probe_tool(), |engine: &Engine, arguments| {
            let args: ProbeArgs = parse_arguments(arguments)?;
            probe(engine.root(), args.echo.as_deref())
        })]
    }

    #[cfg(feature = "web")]
    fn web_routes(&self, _context: &crate::web::WebContext) -> Vec<crate::web::WebRoute> {
        use crate::web::WebRoute;
        vec![
            WebRoute::get(STUB_GET_ROUTE, stub_get, ()),
            WebRoute::post(STUB_POST_ROUTE, stub_post, self.clone()),
        ]
    }

    #[cfg(feature = "web")]
    fn overview_fields(&self) -> Vec<crate::web::OverviewField> {
        vec![crate::web::OverviewField::new(STUB_OVERVIEW_FIELD, |engine: &Engine| {
            Ok(json!({ "extension": STUB_ID, "root": engine.root().display().to_string() }))
        })]
    }

    fn cli_command(&self) -> Option<CliCommand> {
        let command = Command::new(STUB_SUBCOMMAND)
            .about("Probe the stub extension (test-only)")
            .arg(Arg::new("echo").long("echo").value_name("TEXT").help("Echoed back"));
        Some(CliCommand::new(command, |matches, context| {
            let echo = matches.get_one::<String>("echo").map(String::as_str);
            context.print(&probe(context.root, echo)?)?;
            Ok(0)
        }))
    }

    fn init_steps(&self) -> Vec<Arc<dyn InitContribution>> {
        vec![Arc::new(StubInit)]
    }

    fn ui_manifest(&self) -> Option<UiManifest> {
        Some(UiManifest {
            nav: vec![UiNavItem {
                id: STUB_ID,
                label: "Stub",
                path: "/stub",
                group: UiNavGroup::A,
                scope: UiNavScope::Member,
                replaced_in_workspace_by: None,
            }],
            views: vec!["/stub"],
            config_panels: vec![UiConfigPanel {
                id: STUB_ID,
                label: "Stub",
                table: STUB_CONFIG_TABLE,
            }],
            dashboard_cards: vec![UiDashboardCard {
                id: STUB_DASHBOARD_CARD,
                label: "Stub",
            }],
        })
    }

    fn telemetry_tools(&self) -> &'static [ExtensionTool] {
        &[STUB_TELEMETRY]
    }
}

#[cfg(feature = "web")]
async fn stub_get() -> axum::Json<Value> {
    axum::Json(json!({ "extension": STUB_ID }))
}

#[cfg(feature = "web")]
async fn stub_post(axum::extract::State(stub): axum::extract::State<StubExtension>) -> axum::Json<Value> {
    let posts = stub.posts.fetch_add(1, Ordering::SeqCst) + 1;
    axum::Json(json!({ "extension": STUB_ID, "posts": posts }))
}

/// The stub's init step: write [`STUB_INIT_TARGET`] when absent.
struct StubInit;

impl InitContribution for StubInit {
    fn run(&self, root: &Path, _options: &InitOptions) -> anyhow::Result<Vec<InitStep>> {
        let path = root.join(STUB_INIT_TARGET);
        let action = if path.exists() {
            InitAction::Unchanged
        } else {
            fs::write(&path, "written by the stub extension's init step\n")
                .with_context(|| format!("writing {}", path.display()))?;
            InitAction::Created
        };
        Ok(vec![InitStep {
            target: STUB_INIT_TARGET.to_string(),
            action,
            detail: String::new(),
        }])
    }
}

/// A gate that enables one fixed set of features.
#[derive(Debug, Clone, Default)]
pub struct TestGate(pub BTreeSet<String>);

impl TestGate {
    /// A gate enabling exactly `features`.
    pub fn enabling(features: &[&str]) -> Self {
        Self(features.iter().map(|f| f.to_string()).collect())
    }
}

impl FeatureGate for TestGate {
    fn enabled(&self) -> BTreeSet<String> {
        self.0.clone()
    }
}
