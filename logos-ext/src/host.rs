//! [`ExtensionHost`] — the registered extensions, the optional feature gate, and
//! the active set every surface composes from ([ADR-74], [ADR-75]).
//!
//! [ADR-74]: ../../docs/specs/architecture/decisions/ADR-74.md
//! [ADR-75]: ../../docs/specs/architecture/decisions/ADR-75.md

use std::collections::BTreeSet;
use std::fmt;
use std::sync::Arc;

use logos_core::config::{SecretClaim, TableClaim};

use crate::{cli, mcp, ui, Extension, ExtensionTool, FeatureGate, InitContribution};

/// The extensions one binary composes, and the feature gate (if any) that
/// decides which of them are active.
///
/// Built once, at the composition root, and handed to every surface. A host with
/// no extension contributes nothing anywhere, so that binary's surface is the
/// core's alone.
///
/// Cheap to clone: the extensions and the gate are shared.
#[derive(Clone, Default)]
pub struct ExtensionHost {
    extensions: Vec<Arc<dyn Extension>>,
    gate: Option<Arc<dyn FeatureGate>>,
}

/// A composition the host refuses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostError {
    /// Two extensions share an id.
    DuplicateId(&'static str),
    /// Two extensions declare the same telemetry tool name.
    DuplicateTelemetryTool(&'static str),
}

impl fmt::Display for HostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HostError::DuplicateId(id) => write!(f, "two extensions are registered as `{id}`"),
            HostError::DuplicateTelemetryTool(name) => {
                write!(f, "two extensions declare the telemetry tool `{name}`")
            }
        }
    }
}

impl std::error::Error for HostError {}

impl ExtensionHost {
    /// A host with no extension and no feature gate.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register `extension`.
    ///
    /// # Errors
    /// Its id, or one of its telemetry tool names, is already registered.
    pub fn register(mut self, extension: impl Extension) -> Result<Self, HostError> {
        if self.extensions.iter().any(|e| e.id() == extension.id()) {
            return Err(HostError::DuplicateId(extension.id()));
        }
        let declared: Vec<&'static str> =
            self.extensions.iter().flat_map(|e| e.telemetry_tools()).map(|t| t.name()).collect();
        if let Some(tool) = extension.telemetry_tools().iter().find(|t| declared.contains(&t.name())) {
            return Err(HostError::DuplicateTelemetryTool(tool.name()));
        }
        self.extensions.push(Arc::new(extension));
        Ok(self)
    }

    /// Decide which features are enabled with `gate`. Only a binary that
    /// composes gated extensions supplies one.
    pub fn with_gate(mut self, gate: impl FeatureGate + 'static) -> Self {
        self.gate = Some(Arc::new(gate));
        self
    }

    /// Every registered extension, active or not, in registration order.
    pub fn registered(&self) -> impl Iterator<Item = &dyn Extension> {
        self.extensions.iter().map(|e| &**e)
    }

    /// The features the gate enables — asked on every call, never cached
    /// ([`FeatureGate`] says which surfaces that reaches). With no gate, none.
    fn enabled_features(&self) -> BTreeSet<String> {
        self.gate.as_ref().map(|gate| gate.enabled()).unwrap_or_default()
    }

    /// The **active** extensions, in registration order: every one that names
    /// no feature, and every one whose feature the gate enables — from one
    /// evaluation of the gate. Only these contribute to any surface.
    pub fn active(&self) -> Vec<Arc<dyn Extension>> {
        let enabled = self.enabled_features();
        self.extensions
            .iter()
            .filter(|e| e.required_feature().is_none_or(|feature| enabled.contains(feature)))
            .cloned()
            .collect()
    }

    /// Is the extension `id` active?
    pub fn is_active(&self, id: &str) -> bool {
        self.active().iter().any(|e| e.id() == id)
    }

    /// The active extensions' config tables, as `(extension id, table)`.
    pub fn config_tables(&self) -> Vec<(&'static str, &'static str)> {
        self.active()
            .iter()
            .flat_map(|e| e.config_tables().iter().map(|t| (e.id(), *t)))
            .collect()
    }

    /// The active extensions' secret keys, as `(extension id, table.key)`.
    pub fn secret_keys(&self) -> Vec<(&'static str, &'static str)> {
        self.active()
            .iter()
            .flat_map(|e| e.secret_keys().iter().map(|k| (e.id(), *k)))
            .collect()
    }

    /// The active extensions' config tables and secret keys as the core's
    /// config loading routes them ([FR-CF-09]): each table validated by the
    /// extension that owns it. The composition root registers these with the
    /// process ([`logos_core::config::register_extension_config`]).
    ///
    /// [FR-CF-09]: ../../docs/specs/requirements/FR-CF-09.md
    pub fn config_claims(&self) -> (Vec<TableClaim>, Vec<SecretClaim>) {
        claims_of(&self.active())
    }

    /// Register the active extensions' process-wide contributions with this
    /// process: their telemetry tools, so the stats read-model classifies their
    /// events ([`logos_core::observability::register_extension_tools`]), and
    /// their config tables and secret keys, so every config load routes them to
    /// their owner ([`logos_core::config::register_extension_config`],
    /// [FR-CF-09]). The composition root calls it once, before any surface runs.
    ///
    /// # Errors
    /// A telemetry tool or a config claim the process refuses.
    ///
    /// [FR-CF-09]: ../../docs/specs/requirements/FR-CF-09.md
    pub fn register_with_process(&self) -> anyhow::Result<()> {
        // One evaluation of the gate for both, so the tools and the claims
        // registered always come from the same active set.
        let active = self.active();
        let tools: Vec<ExtensionTool> =
            active.iter().flat_map(|e| e.telemetry_tools().iter().copied()).collect();
        let (tables, secrets) = claims_of(&active);
        logos_core::observability::register_extension_tools(&tools)?;
        logos_core::config::register_extension_config(&tables, &secrets)
    }

    /// The active extensions' MCP tools.
    pub fn mcp_tools(&self) -> Vec<mcp::McpTool> {
        self.active().iter().flat_map(|e| e.mcp_tools()).collect()
    }

    /// The active extensions' web routes, built over `context`.
    #[cfg(feature = "web")]
    pub fn web_routes(&self, context: &crate::web::WebContext) -> Vec<crate::web::WebRoute> {
        self.active().iter().flat_map(|e| e.web_routes(context)).collect()
    }

    /// The active extensions' overview fields, in registration order.
    #[cfg(feature = "web")]
    pub fn overview_fields(&self) -> Vec<crate::web::OverviewField> {
        self.active().iter().flat_map(|e| e.overview_fields()).collect()
    }

    /// The active extensions' CLI subcommands.
    pub fn cli_commands(&self) -> Vec<cli::CliCommand> {
        self.active().iter().filter_map(|e| e.cli_command()).collect()
    }

    /// The active extensions' init steps, in registration order.
    pub fn init_steps(&self) -> Vec<Arc<dyn InitContribution>> {
        self.active().iter().flat_map(|e| e.init_steps()).collect()
    }

    /// The active extensions' UI manifests, as `(extension id, manifest)`.
    pub fn ui_manifests(&self) -> Vec<(&'static str, ui::UiManifest)> {
        self.active()
            .iter()
            .filter_map(|e| e.ui_manifest().map(|m| (e.id(), m)))
            .collect()
    }

    /// The active extensions' telemetry tools.
    pub fn telemetry_tools(&self) -> Vec<ExtensionTool> {
        self.active().iter().flat_map(|e| e.telemetry_tools().iter().copied()).collect()
    }
}

/// The config tables and secret keys of `active`, each table validated by the
/// extension that owns it ([`ExtensionHost::config_claims`]).
fn claims_of(active: &[Arc<dyn Extension>]) -> (Vec<TableClaim>, Vec<SecretClaim>) {
    let tables = active
        .iter()
        .flat_map(|extension| {
            extension.config_tables().iter().map(move |table| {
                let owner = Arc::clone(extension);
                TableClaim::new(extension.id(), table, move |value| {
                    owner.validate_config_table(table, value)
                })
            })
        })
        .collect();
    let secrets = active
        .iter()
        .flat_map(|e| e.secret_keys().iter().map(|key| SecretClaim { owner: e.id(), key }))
        .collect();
    (tables, secrets)
}

impl fmt::Debug for ExtensionHost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExtensionHost")
            .field("extensions", &self.registered().map(|e| e.id()).collect::<Vec<_>>())
            .field("gate", &self.gate.is_some())
            .finish()
    }
}

#[cfg(test)]
mod tests;
