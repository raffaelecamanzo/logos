//! Wiki **menu/scaffold constants** shared with the SPA's `/api/v1/wiki/*`
//! read-models ([`crate::api_v1`], [FR-UI-06], CR-049/[FR-UI-22]).
//!
//! The server-rendered Wiki view (the `/wiki`, `/wiki/search`, `/wiki/page/*slug`
//! HTML routes and their layout/TOC/menu rendering) was **retired** when the web
//! UI collapsed to the embedded client-side SPA ([FR-UI-22], S-192): the Wiki tab
//! now renders in React over the `/api/v1/wiki/*` JSON read-models. What remains
//! here is the small kernel those read-models compose from — the **tiered IA
//! constants** and the **scaffold-label** lookup — kept on the Rust side so the
//! menu's link set and the placeholder/404 boundary stay defined once. `logos-core`
//! cannot depend on `web`, so the Summary-tier set is duplicated by convention and
//! pinned to the engine by [`guided_tour_matches_engine_overview_sections`].
//!
//! [FR-UI-06]: ../../docs/specs/requirements/FR-UI-06.md
//! [FR-UI-22]: ../../docs/specs/requirements/FR-UI-22.md
//! [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md

use logos_core::wiki::DocCategory;

/// The Project Overview agent-page slug — the dashboard's "Project Overview"
/// widget read-model ([`crate::api_v1::overview`]) reads its prose from here
/// through the write-free [`Engine::wiki_read`](logos_core::Engine::wiki_read)
/// accessor (CR-034, S-082). The first [`GUIDED_TOUR`] Summary entry.
pub(crate) const PROJECT_OVERVIEW_SLUG: &str = "overview/project-overview";

/// The **Summary** tier scaffold ([FR-WK-11], [FR-WK-12], CR-028, CR-034 — renamed
/// from "Guided Tour") — the agent-tier Overview pages the work-list seeds and the
/// embedded skill writes (S-105 / [FR-WK-06]). The page set (Project Overview,
/// Getting Started, Key Concepts, How It Works, Known Issues) gives a reader
/// landing on the Wiki tab a full orientation tour, prose generated or not. The
/// `overview/` slug prefix matches the engine's
/// [`OverviewSection`](logos_core::wiki) work-list convention so a written page
/// resolves to its menu entry.
///
/// **The Architecture narrative left this Summary set** (CR-062, ADR-57): the
/// Design-tier "Architecture" entry ([`OVERVIEW_ARCHITECTURE`]) is now the
/// **presented** `docs/specs/architecture.md`, assembled deterministically rather
/// than synthesized by the agent, so it is no longer an agent-authored Overview
/// child.
///
/// `pub(crate)` so the `/api/v1/wiki/nav` read-model ([`crate::api_v1::wiki_nav`])
/// composes the SAME tiered menu from the SAME source — the SPA Wiki menu can
/// never disagree with this scaffold. (The const keeps the `GUIDED_TOUR` name so
/// the [`guided_tour_matches_engine_overview_sections`] drift guard against
/// [`OverviewSection`](logos_core::wiki) stays a one-line equality.)
pub(crate) const GUIDED_TOUR: &[(&str, &str)] = &[
    ("overview/project-overview", "Project Overview"),
    ("overview/getting-started", "Getting Started"),
    ("overview/key-concepts", "Key concepts"),
    ("overview/how-it-works", "How It Works"),
    ("overview/known-issues", "Known issues"),
];

/// The **Design-tier "Architecture" page** slug ([FR-WK-11], CR-034, CR-035,
/// CR-062) — listed **once**, under the **Design** tier (labelled "Architecture"),
/// where it sits beside the consolidated ADRs/Components/Integrations documents.
///
/// Since CR-062/ADR-57 it is a **presented** page — the deterministic assembly of
/// `docs/specs/architecture.md` ([FR-WK-20]) — no longer an agent-synthesized
/// Overview child, so it is **not** in [`GUIDED_TOUR`] (the Summary tier). The
/// slug is preserved (presented pages reuse existing slugs) so the menu/reader
/// route is unchanged; [`scaffold_label`] recognizes it directly so its menu link
/// renders the honest "not yet generated" placeholder until the presented tier
/// materializes it, never a 404.
pub(crate) const OVERVIEW_ARCHITECTURE: &str = "overview/architecture";

/// The Design-tier label for the presented [`OVERVIEW_ARCHITECTURE`] page.
pub(crate) const OVERVIEW_ARCHITECTURE_LABEL: &str = "Architecture";

/// The **Specs-tier "SRS" hub page** slug ([FR-WK-11], [FR-WK-26], CR-064) —
/// listed **first** under the **Specs** tier, ahead of the consolidated
/// Functional/Non-Functional/UAT documents. Like [`OVERVIEW_ARCHITECTURE`] it is a
/// **presented** page (the deterministic assembly of `docs/specs/software-spec.md`,
/// [FR-WK-20]) and is not a [`DocCategory`] slug, so [`scaffold_label`] recognizes
/// it directly — its menu link renders the honest "not yet generated" placeholder
/// until the presented tier materializes it, never a 404. The engine
/// `PRESENTED_SRS_SLUG` mirrors it, pinned byte-identical by a drift guard.
pub(crate) const SPECS_SRS: &str = "specs/srs";

/// The Specs-tier label for the presented [`SPECS_SRS`] hub page.
pub(crate) const SPECS_SRS_LABEL: &str = "Software Requirements Specification";

/// The **User Guide** tier's title ([FR-WK-11] as modified by CR-062,
/// [FR-WK-23]) — present, right after Summary, only when
/// [`Engine::wiki_guide_pages`](logos_core::Engine::wiki_guide_pages) is
/// non-empty. Unlike [`GUIDED_TOUR`] and [`DESIGN_DOCS`]/[`SPECS_DOCS`] the
/// tier's items have no fixed slug/label contract here — `docs/howto/*.md` is a
/// per-project, dynamic file set, so `wiki_nav` ([`crate::api_v1::wiki_nav`])
/// reads the (slug, title) pairs straight from the engine accessor instead of a
/// local constant.
pub(crate) const USER_GUIDE_TIER_TITLE: &str = "User Guide";

/// The Design-tier consolidated documents ([FR-WK-11] as modified by CR-034 +
/// CR-035) — the consolidated ADRs / Components / Integrations [`DocCategory`]
/// documents grouped under "Design", in fixed [`DocCategory::ALL`] order.
pub(crate) const DESIGN_DOCS: &[DocCategory] =
    &[DocCategory::Adrs, DocCategory::Components, DocCategory::Integrations];

/// The Specs-tier consolidated documents ([FR-WK-11] as modified by CR-034 +
/// CR-035) — the three [`DocCategory`] documents that group under "Specs", in
/// fixed [`DocCategory::ALL`] order. (Frontend Design moved to the Design tier by
/// CR-035.)
pub(crate) const SPECS_DOCS: &[DocCategory] = &[
    DocCategory::FunctionalRequirements,
    DocCategory::NonFunctionalRequirements,
    DocCategory::UserAcceptanceTests,
];

/// The Guided-Tour label for a slug, or `None` if the slug is not a scaffold page.
fn guided_tour_label(slug: &str) -> Option<&'static str> {
    GUIDED_TOUR.iter().find(|(s, _)| *s == slug).map(|(_, label)| *label)
}

/// The menu-scaffold label for an **always-present** page slug — an Overview
/// (Summary) scaffold page, the Design-tier presented Architecture page
/// ([`OVERVIEW_ARCHITECTURE`], CR-062), or a consolidated [`DocCategory`] document
/// ([FR-WK-11] as modified by CR-034). `Some(label)` means the slug is a scaffold
/// entry that, when it has no page yet, renders the honest "not yet generated"
/// placeholder (200) instead of a 404 ([NFR-CC-04]); `None` means an unknown agent
/// slug, which stays an honest 404. The consolidated slugs are read from the fixed
/// [`DocCategory`] contract (S-134), so the placeholder set tracks the menu's link
/// set automatically.
pub(crate) fn scaffold_label(slug: &str) -> Option<&'static str> {
    guided_tour_label(slug)
        // The Architecture page left GUIDED_TOUR (CR-062) but is still a Design-tier
        // menu entry, so it must resolve to a scaffold placeholder, not a 404.
        .or_else(|| (slug == OVERVIEW_ARCHITECTURE).then_some(OVERVIEW_ARCHITECTURE_LABEL))
        // The presented SRS hub (CR-064, S-269) is a Specs-tier menu entry that is
        // not a DocCategory slug, so it likewise resolves to a scaffold placeholder.
        .or_else(|| (slug == SPECS_SRS).then_some(SPECS_SRS_LABEL))
        .or_else(|| DocCategory::ALL.iter().find(|c| c.slug() == slug).map(|c| c.title()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaffold_label_covers_overview_and_consolidated_slugs_only() {
        // Overview (Summary) scaffold slugs resolve to their label.
        assert_eq!(scaffold_label("overview/project-overview"), Some("Project Overview"));
        // Every consolidated DocCategory slug is a scaffold page (placeholder, not 404).
        for category in DocCategory::ALL {
            assert_eq!(
                scaffold_label(category.slug()),
                Some(category.title()),
                "consolidated slug is a scaffold page: {}",
                category.slug(),
            );
        }
        // An unknown agent slug is not a scaffold page -> stays an honest 404.
        assert_eq!(scaffold_label("does/not/exist"), None);
    }

    #[test]
    fn overview_architecture_is_a_design_scaffold_but_not_a_summary_tour_entry() {
        // CR-062/ADR-57: the Architecture page is retired from the Summary tier
        // (the agent no longer synthesizes it) and is now the presented
        // docs/specs/architecture.md under the Design tier. It must therefore be
        // ABSENT from GUIDED_TOUR (so the OverviewSection drift guard stays a
        // one-line equality) yet still resolve to a scaffold placeholder — or the
        // Design-tier link would 404 until the presented tier materializes it.
        assert!(
            !GUIDED_TOUR.iter().any(|(s, _)| *s == OVERVIEW_ARCHITECTURE),
            "the Architecture page left the Summary tier (CR-062)"
        );
        assert_eq!(
            scaffold_label(OVERVIEW_ARCHITECTURE),
            Some(OVERVIEW_ARCHITECTURE_LABEL),
            "the Design-tier Architecture link renders a placeholder, never a 404"
        );
    }

    /// Drift guard ([CR-028] F-01): the web `GUIDED_TOUR` and the engine
    /// `OverviewSection::ALL` encode the same Guided-Tour set but are duplicated
    /// by convention (`logos-core` cannot depend on `web`). This asserts they stay
    /// byte-identical so a one-sided Overview-page change fails here rather than
    /// shipping a misaligned generation loop (the F-01 bug).
    #[test]
    fn guided_tour_matches_engine_overview_sections() {
        use logos_core::wiki::OverviewSection;
        let engine: Vec<(&str, &str)> =
            OverviewSection::ALL.iter().map(|s| (s.slug(), s.title())).collect();
        let web: Vec<(&str, &str)> = GUIDED_TOUR.to_vec();
        assert_eq!(
            web, engine,
            "web GUIDED_TOUR must match engine OverviewSection::ALL (slug, label) in menu order"
        );
    }

    /// Drift guard ([CR-062]): the Design-tier Architecture page's slug and label
    /// are duplicated by convention — the web `OVERVIEW_ARCHITECTURE`/
    /// `OVERVIEW_ARCHITECTURE_LABEL` here and the engine
    /// `PRESENTED_ARCHITECTURE_SLUG`/`PRESENTED_ARCHITECTURE_TITLE` (`logos-core`
    /// cannot depend on `web`, so the presented tier reuses the same literals).
    /// This pins the pair byte-identical so a one-sided edit fails here rather than
    /// silently breaking the presented Architecture page's menu link/route (the
    /// same F-01 class the `GUIDED_TOUR`/`OverviewSection` guard prevents).
    #[test]
    fn architecture_slug_and_label_match_the_engine_presented_constants() {
        use logos_core::wiki::{PRESENTED_ARCHITECTURE_SLUG, PRESENTED_ARCHITECTURE_TITLE};
        assert_eq!(
            OVERVIEW_ARCHITECTURE, PRESENTED_ARCHITECTURE_SLUG,
            "web OVERVIEW_ARCHITECTURE must match engine PRESENTED_ARCHITECTURE_SLUG"
        );
        assert_eq!(
            OVERVIEW_ARCHITECTURE_LABEL, PRESENTED_ARCHITECTURE_TITLE,
            "web OVERVIEW_ARCHITECTURE_LABEL must match engine PRESENTED_ARCHITECTURE_TITLE"
        );
    }

    #[test]
    fn srs_is_a_specs_scaffold_placeholder_but_not_a_doc_category() {
        // S-269/CR-064: the presented SRS hub is a Specs-tier menu entry that is NOT
        // a DocCategory slug, so it must resolve to a scaffold placeholder (200, never
        // a 404) yet stay out of the DocCategory contract.
        assert_eq!(
            scaffold_label(SPECS_SRS),
            Some(SPECS_SRS_LABEL),
            "the Specs-tier SRS link renders a placeholder, never a 404"
        );
        assert!(
            !DocCategory::ALL.iter().any(|c| c.slug() == SPECS_SRS),
            "the SRS hub is not a DocCategory slug"
        );
    }

    /// Drift guard ([CR-064], S-269): the Specs-tier SRS hub page's slug and label
    /// are duplicated by convention — the web `SPECS_SRS`/`SPECS_SRS_LABEL` here and
    /// the engine `PRESENTED_SRS_SLUG`/`PRESENTED_SRS_TITLE` (`logos-core` cannot
    /// depend on `web`). This pins the pair byte-identical so a one-sided edit fails
    /// here rather than silently breaking the SRS page's menu link/route (the same
    /// F-01 class the Architecture drift guard prevents).
    #[test]
    fn srs_slug_and_label_match_the_engine_presented_constants() {
        use logos_core::wiki::{PRESENTED_SRS_SLUG, PRESENTED_SRS_TITLE};
        assert_eq!(
            SPECS_SRS, PRESENTED_SRS_SLUG,
            "web SPECS_SRS must match engine PRESENTED_SRS_SLUG"
        );
        assert_eq!(
            SPECS_SRS_LABEL, PRESENTED_SRS_TITLE,
            "web SPECS_SRS_LABEL must match engine PRESENTED_SRS_TITLE"
        );
    }

    /// PROJECT_OVERVIEW_SLUG must remain the first Summary scaffold entry — the
    /// dashboard read-model reads the Project Overview prose from this slug.
    #[test]
    fn project_overview_slug_is_the_first_summary_scaffold_entry() {
        assert_eq!(PROJECT_OVERVIEW_SLUG, GUIDED_TOUR[0].0);
        assert_eq!(scaffold_label(PROJECT_OVERVIEW_SLUG), Some("Project Overview"));
    }
}
