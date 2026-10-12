/*
 * The `wiki` extension's SPA code (S-644, FR-UI-51, CR-213, ADR-74) — what the bundle
 * mounts while the server reports the extension active (`GET /api/v1/extensions`):
 * the Wiki view and its sidebar icon, the Dashboard's Project Overview card, and the
 * `[wiki]` config panel of both Config views. Registered through
 * `virtual:logos-extensions` (`extensions/composed.ts`), and keyed by the ids and
 * routes the server's manifest uses (`logos-wiki/src/extension.rs`,
 * `WikiExtension::ui_manifest`) — the only join between the two halves, proven end to
 * end over the body that server serves (`extensions/fixtures/wiki.real.json`, mounted
 * by `test/wiki.ts`).
 *
 * Nothing here mounts on its own: a build whose server registers no `wiki` extension
 * (a `logos` binary without the `wiki` feature) leaves all of it inert, so the shell
 * offers no Wiki entry, view, card or `[wiki]` fields.
 */

import { IconWiki } from "../../components/icons.tsx";
import type { ExtensionUi } from "../../extensions/registry.ts";
import { ProjectOverviewCard } from "../dashboard/DashboardView.tsx";
import { WikiView } from "./WikiView.tsx";

/** The extension's id — the server's `Extension::id`. */
export const WIKI_EXTENSION_ID = "wiki";

/**
 * The `[wiki]` config panel. The table's one field (`model`) is a field of the core
 * Config view's single `config.toml` form — the rule S-643 set for `[chat]`: a second
 * editor of the same document would race it — so the panel adds no card of its own.
 * Mounting it is what tells that form (and the workspace tier's) the table is edited
 * while the wiki is active (`useMountedConfigTable`); without the extension the
 * `[wiki]` fields are not offered.
 */
function WikiTablePanel(): null {
  return null;
}

export const WIKI_EXTENSION_UI: ExtensionUi = {
  id: WIKI_EXTENSION_ID,
  icons: { wiki: IconWiki },
  views: {
    // S-189 — the Wiki tab. It owns its client sub-routes (`/wiki/search`, the
    // `/wiki/page/*` reader) inside the one mounted view (`views/index.ts`'s
    // longest-prefix match), so the manifest lists the one route.
    "/wiki": WikiView,
  },
  configPanels: {
    wiki: { member: WikiTablePanel, workspace: WikiTablePanel },
  },
  dashboardCards: {
    // CR-034 / S-135 — the agent-written Project Overview's prose snippet, over the
    // `overview_page` field the extension adds to `/api/v1/overview`.
    "project-overview": ProjectOverviewCard,
  },
  // Where the built-in sidebar had it: after Graph — and after Chat, which anchors to
  // Graph too and which the `logos` binary composes first, so the server lists it
  // first (`cli/src/composition.rs`; `nav.ts` keeps the server's order).
  navAfter: { wiki: "graph" },
};
