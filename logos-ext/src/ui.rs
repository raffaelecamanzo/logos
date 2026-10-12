//! The UI contribution point: what an extension adds to the SPA ([FR-UI-51]).
//!
//! The server reports each active extension's manifest, and the SPA mounts its
//! navigation, views and config panels from it — the three things [FR-UI-51]
//! names — and its Dashboard cards, each over a field the extension adds to the
//! overview read-model (`web::OverviewField`, feature `web`). A navigation entry carries every field of the SPA's own registry entry
//! (`NavItem` in `web/ui/src/nav.ts`, in snake case), so it renders exactly as a
//! built-in one does.
//!
//! [FR-UI-51]: ../../docs/specs/requirements/FR-UI-51.md

use serde::Serialize;

/// Everything an extension adds to the SPA.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct UiManifest {
    /// Sidebar entries, in order.
    pub nav: Vec<UiNavItem>,
    /// The client routes this extension's views mount at — a navigation entry's
    /// `path`, and any route a view owns without a sidebar entry of its own.
    pub views: Vec<&'static str>,
    /// Config panels, in order.
    pub config_panels: Vec<UiConfigPanel>,
    /// Dashboard cards, in order — each rendering a field the extension adds to
    /// the overview read-model (`web::OverviewField`, feature `web`). Omitted from the
    /// wire when empty, so a manifest without one serialises as it always did.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub dashboard_cards: Vec<UiDashboardCard>,
}

/// One sidebar entry — the SPA's `NavItem`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UiNavItem {
    /// Stable id.
    pub id: &'static str,
    /// Sidebar / breadcrumb label.
    pub label: &'static str,
    /// The client route the entry resolves to.
    pub path: &'static str,
    /// The sidebar group.
    pub group: UiNavGroup,
    /// The scope the view answers for.
    pub scope: UiNavScope,
    /// The id of the workspace entry that replaces this one in workspace mode,
    /// where this one is then not offered (`NavItem.replacedInWorkspaceBy`) —
    /// how the member Chat gives way to the Workspace Chat. Omitted when `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replaced_in_workspace_by: Option<&'static str>,
}

/// The SPA's sidebar groups (`NavGroup`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum UiNavGroup {
    /// Primary read surfaces.
    A,
    /// Risk and coverage.
    B,
    /// The policy editor and its neighbours.
    C,
}

/// The scope a view answers for (`NavScope`, [ADR-66]).
///
/// [ADR-66]: ../../docs/specs/architecture/decisions/ADR-66.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum UiNavScope {
    /// The whole workspace.
    App,
    /// One member.
    Member,
}

/// One config panel, editing the config table an extension owns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UiConfigPanel {
    /// Stable id.
    pub id: &'static str,
    /// The panel's heading.
    pub label: &'static str,
    /// The `config.toml` table it edits ([`crate::Extension::config_tables`]).
    pub table: &'static str,
}

/// One Dashboard card, rendering a field the extension adds to the overview
/// read-model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UiDashboardCard {
    /// Stable id.
    pub id: &'static str,
    /// The card's title.
    pub label: &'static str,
}
